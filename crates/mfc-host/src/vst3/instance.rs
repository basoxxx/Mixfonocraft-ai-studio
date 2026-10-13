//! Istanza VST3 che implementa [`ExternalInstance`].
//!
//! Threading:
//! - `process` (thread audio) usa solo `proc` con `try_lock`: buffer, lista eventi e code
//!   dei parametri sono preallocati in `prepare`.
//! - i cambi di parametro arrivano da qualunque thread tramite code lock-free
//!   (`Shared::to_proc` verso il processore, `Shared::to_ctrl` verso il controller);
//!   `to_ctrl` viene svuotata sul thread UI (`idle` e le altre chiamate UI).
//! - le chiamate UI (controller, stato, editor, `prepare`) sono serializzate da `ui`;
//!   l'ordine dei lock è sempre `ui` → `proc`.

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper, Interface};

use super::com::{note_event, stream_ptr, EventList, MemStream, ParamChanges};
use super::host_ctx::{host_ptr, HostContext, Shared};
use super::module::Module;
use crate::{EditorSize, ExternalInstance, ExternalParam, HostError, MidiEvent, ParentWindow, PluginInfo};

/// Magic dello stato salvato.
const STATE_MAGIC: &[u8; 4] = b"MFV3";
const STATE_VERSION: u32 = 1;

fn ok(r: tresult) -> bool {
    r == kResultOk
}

fn plugin_err(what: &str, r: tresult) -> HostError {
    HostError::Plugin(format!("{what} (tresult {r})"))
}

/// Puntatori COM del plugin. Rilasciati esplicitamente in `Drop` nell'ordine corretto.
struct Com {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// Controller creato separatamente (va inizializzato/terminato a parte).
    separate: bool,
    /// Punti di connessione (componente, controller) se connessi.
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
}

/// Bus audio con i puntatori ai canali (preallocati).
struct Bus {
    ptrs: Vec<*mut f32>,
    scratch: Vec<Vec<f32>>,
}

impl Bus {
    fn new(channels: usize, max_block: usize) -> Self {
        Self {
            ptrs: vec![std::ptr::null_mut(); channels],
            scratch: (0..channels).map(|_| vec![0.0; max_block]).collect(),
        }
    }
}

/// Stato del thread audio.
struct ProcState {
    active: bool,
    max_block: usize,
    sample_rate: f64,
    inputs: Vec<Bus>,
    outputs: Vec<Bus>,
    in_bufs: Vec<AudioBusBuffers>,
    out_bufs: Vec<AudioBusBuffers>,
    has_event_input: bool,
    /// Indici dei bus audio principali.
    main_in: Option<usize>,
    main_out: Option<usize>,
    events: ComWrapper<EventList>,
    in_params: ComWrapper<ParamChanges>,
    out_params: ComWrapper<ParamChanges>,
    context: ProcessContext,
    /// Note tenute per tasto (per "all notes off" / reset).
    held: [u16; 128],
}

// SAFETY: i puntatori grezzi nei bus puntano a buffer posseduti da `ProcState` o ai
// buffer passati a `process`, e le liste COM con `UnsafeCell` sono usate solo durante
// `process`/`prepare`: tutto l'accesso avviene sotto il mutex `Vst3Instance::proc`.
unsafe impl Send for ProcState {}

impl ProcState {
    fn new() -> Self {
        // SAFETY: `ProcessContext` è POD.
        let mut context: ProcessContext = unsafe { std::mem::zeroed() };
        context.tempo = 120.0;
        context.timeSigNumerator = 4;
        context.timeSigDenominator = 4;
        context.state = (ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kTimeSigValid
            | ProcessContext_::StatesAndFlags_::kContTimeValid) as u32;
        Self {
            active: false,
            max_block: 0,
            sample_rate: 0.0,
            inputs: Vec::new(),
            outputs: Vec::new(),
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            has_event_input: false,
            main_in: None,
            main_out: None,
            events: ComWrapper::new(EventList::new()),
            in_params: ComWrapper::new(ParamChanges::new()),
            out_params: ComWrapper::new(ParamChanges::new()),
            context,
            held: [0; 128],
        }
    }
}

/// Stato del thread UI.
struct UiState {
    view: Option<ComPtr<IPlugView>>,
    has_editor: Option<bool>,
}

// SAFETY: la vista è usata solo dal thread UI, sotto il mutex `Vst3Instance::ui`.
unsafe impl Send for UiState {}

/// Istanza di un plugin VST3.
pub struct Vst3Instance {
    info: PluginInfo,
    shared: Arc<Shared>,
    proc: Mutex<ProcState>,
    ui: Mutex<UiState>,
    latency: AtomicU32,
    pending_reset: AtomicBool,
    com: ManuallyDrop<Com>,
    host: ComWrapper<HostContext>,
    /// Ultimo campo: il modulo viene scaricato dopo aver rilasciato tutto il resto.
    _module: Arc<Module>,
}

// SAFETY: `ComPtr` delle interfacce VST3 sono Send/Sync; l'uso concorrente segue le regole
// di threading VST3 (processore sul thread audio sotto `proc`, controller/stato/editor
// sul thread UI sotto `ui`). Gli altri campi sono atomici o protetti da mutex.
unsafe impl Send for Vst3Instance {}
unsafe impl Sync for Vst3Instance {}

impl Vst3Instance {
    /// Crea, inizializza e attiva un'istanza della classe `cid`.
    pub(crate) fn create(
        module: Arc<Module>,
        info: PluginInfo,
        cid: &TUID,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<Arc<Self>, HostError> {
        let shared = Arc::new(Shared::new());
        let host = HostContext::new(shared.clone());
        let component: ComPtr<IComponent> = module
            .create(cid)
            .ok_or_else(|| HostError::Plugin(format!("{}: creazione del componente fallita", info.name)))?;
        // SAFETY: chiamate VST3 su oggetti validi; il contesto host vive quanto l'istanza.
        unsafe {
            let r = component.initialize(host_ptr::<FUnknown>(&host));
            if !ok(r) {
                return Err(plugin_err("IComponent::initialize", r));
            }
            let Some(processor) = component.cast::<IAudioProcessor>() else {
                component.terminate();
                return Err(HostError::Plugin(format!("{}: IAudioProcessor mancante", info.name)));
            };
            let mut inst = Self {
                info,
                shared,
                proc: Mutex::new(ProcState::new()),
                ui: Mutex::new(UiState {
                    view: None,
                    has_editor: None,
                }),
                latency: AtomicU32::new(0),
                pending_reset: AtomicBool::new(false),
                com: ManuallyDrop::new(Com {
                    component,
                    processor,
                    controller: None,
                    separate: false,
                    connection: None,
                }),
                host,
                _module: module.clone(),
            };
            // Da qui in poi gli errori passano per `Drop`, che smonta ciò che esiste.
            inst.setup_controller(&module);
            inst.prepare(sample_rate, max_block)?;
            Ok(Arc::new(inst))
        }
    }

    /// Controller: dal componente (componente singolo) o creato dalla fabbrica, poi
    /// handler, connessione e trasferimento dello stato del componente.
    unsafe fn setup_controller(&mut self, module: &Module) {
        let host_unknown = host_ptr::<FUnknown>(&self.host);
        let handler = host_ptr::<IComponentHandler>(&self.host);
        let com = &mut *self.com;
        if let Some(c) = com.component.cast::<IEditController>() {
            com.controller = Some(c);
        } else {
            let mut cid: TUID = [0; 16];
            if ok(com.component.getControllerClassId(&mut cid)) && cid.iter().any(|b| *b != 0) {
                if let Some(c) = module.create::<IEditController>(&cid) {
                    if ok(c.initialize(host_unknown)) {
                        com.controller = Some(c);
                        com.separate = true;
                    }
                }
            }
        }
        let Some(ctrl) = com.controller.clone() else { return };
        ctrl.setComponentHandler(handler);
        if com.separate {
            if let (Some(a), Some(b)) = (
                com.component.cast::<IConnectionPoint>(),
                ctrl.cast::<IConnectionPoint>(),
            ) {
                a.connect(b.as_ptr());
                b.connect(a.as_ptr());
                com.connection = Some((a, b));
            }
            let s = MemStream::new(Vec::new());
            if ok(com.component.getState(stream_ptr(&s))) {
                s.rewind();
                ctrl.setComponentState(stream_ptr(&s));
            }
        }
    }

    fn controller(&self) -> Option<&ComPtr<IEditController>> {
        self.com.controller.as_ref()
    }

    /// Applica al controller i cambi accodati (thread UI).
    fn sync_controller(&self) {
        let ctrl = self.controller();
        while let Some((id, v)) = self.shared.to_ctrl.pop() {
            if let Some(ctrl) = ctrl {
                // SAFETY: chiamata sul thread UI a un controller valido.
                unsafe {
                    ctrl.setParamNormalized(id, v);
                }
            }
        }
    }

    /// Disattiva il processing (con `proc` già bloccato).
    unsafe fn deactivate(com: &Com, p: &mut ProcState) {
        if p.active {
            com.processor.setProcessing(0);
            com.component.setActive(0);
            p.active = false;
        }
    }

    /// Attiva i bus principali, negozia le disposizioni e alloca i buffer (componente
    /// inattivo).
    unsafe fn configure_buses(com: &Com, p: &mut ProcState, max_block: usize) {
        let c = &com.component;
        let audio = MediaTypes_::kAudio as MediaType;
        let event = MediaTypes_::kEvent as MediaType;
        let input = BusDirections_::kInput as BusDirection;
        let output = BusDirections_::kOutput as BusDirection;
        let n_in = c.getBusCount(audio, input).clamp(0, 64);
        let n_out = c.getBusCount(audio, output).clamp(0, 64);
        let bus_info = |dir: BusDirection, i: i32| -> Option<BusInfo> {
            let mut bi: BusInfo = std::mem::zeroed();
            ok(c.getBusInfo(audio, dir, i, &mut bi)).then_some(bi)
        };
        // Bus principale: il primo `kMain`, altrimenti il primo.
        let main_of = |dir: BusDirection, n: i32| -> Option<i32> {
            (n > 0).then(|| {
                (0..n)
                    .find(|i| bus_info(dir, *i).is_some_and(|b| b.busType == BusTypes_::kMain as BusType))
                    .unwrap_or(0)
            })
        };
        let main_in = main_of(input, n_in);
        let main_out = main_of(output, n_out);
        for i in 0..n_in {
            c.activateBus(audio, input, i, u8::from(Some(i) == main_in));
        }
        for i in 0..n_out {
            c.activateBus(audio, output, i, u8::from(Some(i) == main_out));
        }
        let n_ev = c.getBusCount(event, input).max(0);
        for i in 0..n_ev {
            c.activateBus(event, input, i, u8::from(i == 0));
        }
        p.has_event_input = n_ev > 0;

        // Stereo sui bus principali; gli altri come li propone il plugin.
        let current = |dir: BusDirection, i: i32| -> SpeakerArrangement {
            let mut a: SpeakerArrangement = 0;
            if ok(com.processor.getBusArrangement(dir, i, &mut a)) {
                a
            } else {
                SpeakerArr::kStereo
            }
        };
        let mut ins: Vec<SpeakerArrangement> = (0..n_in)
            .map(|i| {
                if Some(i) == main_in {
                    SpeakerArr::kStereo
                } else {
                    current(input, i)
                }
            })
            .collect();
        let mut outs: Vec<SpeakerArrangement> = (0..n_out)
            .map(|i| {
                if Some(i) == main_out {
                    SpeakerArr::kStereo
                } else {
                    current(output, i)
                }
            })
            .collect();
        let r = com
            .processor
            .setBusArrangements(ins.as_mut_ptr(), n_in, outs.as_mut_ptr(), n_out);
        if !ok(r) {
            // Rifiutato: restano le disposizioni del plugin (mono, surround, ...).
            ins = (0..n_in).map(|i| current(input, i)).collect();
            outs = (0..n_out).map(|i| current(output, i)).collect();
        }
        let channels = |dir: BusDirection, i: i32, a: SpeakerArrangement| -> usize {
            match a.count_ones() as usize {
                0 => bus_info(dir, i).map_or(0, |b| b.channelCount.clamp(0, 64) as usize),
                n => n.min(64),
            }
        };
        p.inputs = (0..n_in)
            .map(|i| Bus::new(channels(input, i, ins[i as usize]), max_block))
            .collect();
        p.outputs = (0..n_out)
            .map(|i| Bus::new(channels(output, i, outs[i as usize]), max_block))
            .collect();
        let empty = AudioBusBuffers {
            numChannels: 0,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: std::ptr::null_mut(),
            },
        };
        p.in_bufs = vec![empty; p.inputs.len()];
        p.out_bufs = vec![empty; p.outputs.len()];
        p.main_in = main_in.map(|i| i as usize);
        p.main_out = main_out.map(|i| i as usize);
    }

    /// Note off per tutti i tasti tenuti.
    fn all_notes_off(p: &mut ProcState, offset: i32) {
        for k in 0..128 {
            if p.held[k] > 0 {
                p.held[k] = 0;
                p.events.push(note_event(false, offset, k as i16, 0.0, -1));
            }
        }
    }

    /// Elabora un sotto-blocco `[start, start + len)` (lock `proc` preso).
    #[allow(clippy::too_many_arguments)]
    unsafe fn process_chunk(
        &self,
        p: &mut ProcState,
        events: &[MidiEvent],
        input: Option<(&[f32], &[f32])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
        start: usize,
        len: usize,
        first: bool,
        last: bool,
    ) -> bool {
        // Parametri accodati: tutti nel primo sotto-blocco.
        p.in_params.clear();
        p.out_params.clear();
        if first {
            while let Some((id, v)) = self.shared.to_proc.pop() {
                if !p.in_params.set(id, v) {
                    break;
                }
            }
        }

        // Eventi del sotto-blocco (offset relativi, quelli oltre la fine nell'ultimo).
        p.events.clear();
        if first && self.pending_reset.swap(false, Ordering::AcqRel) {
            Self::all_notes_off(p, 0);
        }
        for e in events {
            let off = match *e {
                MidiEvent::NoteOn { offset, .. }
                | MidiEvent::NoteOff { offset, .. }
                | MidiEvent::AllNotesOff { offset } => offset as usize,
            };
            if off < start || (off >= start + len && !last) {
                continue;
            }
            let rel = (off - start).min(len - 1) as i32;
            match *e {
                MidiEvent::NoteOn {
                    key, velocity, note_id, ..
                } => {
                    let k = key.min(127) as usize;
                    p.held[k] = p.held[k].saturating_add(1);
                    p.events.push(note_event(true, rel, k as i16, velocity, note_id));
                }
                MidiEvent::NoteOff {
                    key, velocity, note_id, ..
                } => {
                    let k = key.min(127) as usize;
                    p.held[k] = p.held[k].saturating_sub(1);
                    p.events.push(note_event(false, rel, k as i16, velocity, note_id));
                }
                MidiEvent::AllNotesOff { .. } => Self::all_notes_off(p, rel),
            }
        }
        p.events.sort_by_offset();

        let ProcState {
            inputs,
            outputs,
            in_bufs,
            out_bufs,
            main_in,
            main_out,
            ..
        } = p;

        // Ingressi: copia nei buffer interni (il plugin può scriverci).
        for (bi, bus) in inputs.iter_mut().enumerate() {
            let nch = bus.scratch.len();
            let src = input.filter(|_| Some(bi) == *main_in);
            for c in 0..nch {
                let buf = &mut bus.scratch[c][..len];
                match src {
                    Some((l, r)) => {
                        let (l, r) = (&l[start..start + len], &r[start..start + len]);
                        if nch == 1 {
                            for i in 0..len {
                                buf[i] = 0.5 * (l[i] + r[i]);
                            }
                        } else if c == 0 {
                            buf.copy_from_slice(l);
                        } else if c == 1 {
                            buf.copy_from_slice(r);
                        } else {
                            buf.fill(0.0);
                        }
                    }
                    None => buf.fill(0.0),
                }
                bus.ptrs[c] = bus.scratch[c].as_mut_ptr();
            }
            in_bufs[bi] = bus_buffers(&mut bus.ptrs);
        }

        // Uscite: canali 0/1 del bus principale direttamente nei buffer del chiamante.
        let ol = &mut out_l[start..start + len];
        let or = &mut out_r[start..start + len];
        ol.fill(0.0);
        or.fill(0.0);
        let mut main_channels = 0;
        for (bi, bus) in outputs.iter_mut().enumerate() {
            let is_main = Some(bi) == *main_out;
            let nch = bus.scratch.len();
            if is_main {
                main_channels = nch;
            }
            for c in 0..nch {
                bus.ptrs[c] = match (is_main, c) {
                    (true, 0) => ol.as_mut_ptr(),
                    (true, 1) => or.as_mut_ptr(),
                    _ => {
                        let s = &mut bus.scratch[c];
                        s[..len].fill(0.0);
                        s.as_mut_ptr()
                    }
                };
            }
            out_bufs[bi] = bus_buffers(&mut bus.ptrs);
        }

        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: len as int32,
            numInputs: in_bufs.len() as int32,
            numOutputs: out_bufs.len() as int32,
            inputs: if in_bufs.is_empty() {
                std::ptr::null_mut()
            } else {
                in_bufs.as_mut_ptr()
            },
            outputs: if out_bufs.is_empty() {
                std::ptr::null_mut()
            } else {
                out_bufs.as_mut_ptr()
            },
            inputParameterChanges: com_ptr::<IParameterChanges, _>(&p.in_params),
            outputParameterChanges: com_ptr::<IParameterChanges, _>(&p.out_params),
            inputEvents: if p.has_event_input {
                com_ptr::<IEventList, _>(&p.events)
            } else {
                std::ptr::null_mut()
            },
            outputEvents: std::ptr::null_mut(),
            processContext: &mut p.context,
        };
        let r = self.com.processor.process(&mut data);
        p.context.projectTimeSamples += len as i64;
        p.context.continousTimeSamples += len as i64;
        // Cambi di parametro fatti dal plugin → controller (thread UI).
        p.out_params.for_each_last(|id, v| self.shared.to_ctrl.push(id, v));
        if !ok(r) {
            ol.fill(0.0);
            or.fill(0.0);
            return false;
        }
        if main_channels == 1 {
            or.copy_from_slice(ol);
        }
        true
    }

    /// Gestisce le richieste di `restartComponent` (thread UI).
    fn handle_restart(&self) {
        let flags = self.shared.restart.swap(0, Ordering::AcqRel);
        if flags == 0 {
            return;
        }
        let mut reconfigure = flags & (RestartFlags_::kReloadComponent | RestartFlags_::kIoChanged) != 0;
        if flags & RestartFlags_::kLatencyChanged != 0 {
            // Riavvio solo se la latenza è davvero cambiata (evita cicli con i plugin che
            // la notificano a ogni attivazione).
            // SAFETY: lettura della latenza sul thread UI.
            let now = unsafe { self.com.processor.getLatencySamples() };
            reconfigure |= now != self.latency.load(Ordering::Relaxed);
        }
        if reconfigure {
            let (sr, mb) = {
                let p = self.proc.lock();
                (p.sample_rate as f32, p.max_block)
            };
            if mb > 0 {
                let _ = self.prepare(sr, mb);
            }
        }
    }
}

/// `AudioBusBuffers` che punta ai canali indicati.
fn bus_buffers(ptrs: &mut [*mut f32]) -> AudioBusBuffers {
    AudioBusBuffers {
        numChannels: ptrs.len() as int32,
        silenceFlags: 0,
        __field0: AudioBusBuffers__type0 {
            channelBuffers32: if ptrs.is_empty() {
                std::ptr::null_mut()
            } else {
                ptrs.as_mut_ptr()
            },
        },
    }
}

/// Puntatore (senza addRef) a un'interfaccia di un oggetto COM dell'host.
fn com_ptr<I: Interface, C: vst3::Class>(w: &ComWrapper<C>) -> *mut I {
    w.as_com_ref::<I>().map_or(std::ptr::null_mut(), |r| r.as_ptr())
}

/// Stato serializzato: `MFV3`, versione, lunghezza + dati del componente e del controller.
fn encode_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(24 + component.len() + controller.len());
    v.extend_from_slice(STATE_MAGIC);
    v.extend_from_slice(&STATE_VERSION.to_le_bytes());
    v.extend_from_slice(&(component.len() as u64).to_le_bytes());
    v.extend_from_slice(component);
    v.extend_from_slice(&(controller.len() as u64).to_le_bytes());
    v.extend_from_slice(controller);
    v
}

/// Inverso di [`encode_state`].
fn decode_state(data: &[u8]) -> Option<(&[u8], &[u8])> {
    fn take<'a>(d: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        if d.len() < n {
            return None;
        }
        let (a, b) = d.split_at(n);
        *d = b;
        Some(a)
    }
    fn len(d: &mut &[u8]) -> Option<usize> {
        let b: [u8; 8] = take(d, 8)?.try_into().ok()?;
        usize::try_from(u64::from_le_bytes(b)).ok()
    }
    let mut d = data;
    if take(&mut d, 4)? != STATE_MAGIC {
        return None;
    }
    let ver = u32::from_le_bytes(take(&mut d, 4)?.try_into().ok()?);
    if ver != STATE_VERSION {
        return None;
    }
    let n = len(&mut d)?;
    let comp = take(&mut d, n)?;
    let n = len(&mut d)?;
    let ctrl = take(&mut d, n)?;
    d.is_empty().then_some((comp, ctrl))
}

impl ExternalInstance for Vst3Instance {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn prepare(&self, sample_rate: f32, max_block: usize) -> Result<(), HostError> {
        let _ui = self.ui.lock();
        let mut p = self.proc.lock();
        let max_block = max_block.clamp(1, 1 << 16);
        let com = &*self.com;
        // SAFETY: riconfigurazione sul thread UI con il processore fermo (lock `proc`).
        unsafe {
            Self::deactivate(com, &mut p);
            Self::configure_buses(com, &mut p, max_block);
            if !ok(com
                .processor
                .canProcessSampleSize(SymbolicSampleSizes_::kSample32 as int32))
            {
                return Err(HostError::Unsupported(format!(
                    "{}: niente audio a 32 bit",
                    self.info.name
                )));
            }
            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                maxSamplesPerBlock: max_block as int32,
                sampleRate: sample_rate as f64,
            };
            let r = com.processor.setupProcessing(&mut setup);
            if !ok(r) {
                return Err(plugin_err("setupProcessing", r));
            }
            let r = com.component.setActive(1);
            if !ok(r) {
                return Err(plugin_err("setActive", r));
            }
            p.active = true;
            // `kNotImplemented` è ammesso.
            com.processor.setProcessing(1);
            p.max_block = max_block;
            p.sample_rate = sample_rate as f64;
            p.context.sampleRate = sample_rate as f64;
            self.latency.store(com.processor.getLatencySamples(), Ordering::Relaxed);
        }
        // Le richieste di riavvio fatte durante l'attivazione sono già soddisfatte.
        self.shared.restart.store(0, Ordering::Release);
        Ok(())
    }

    fn process(
        &self,
        events: &[MidiEvent],
        input: Option<(&[f32], &[f32])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        let Some(mut guard) = self.proc.try_lock() else {
            return false;
        };
        let p = &mut *guard;
        if !p.active || p.max_block == 0 {
            return false;
        }
        let mut n = out_l.len().min(out_r.len());
        if let Some((l, r)) = input {
            n = n.min(l.len()).min(r.len());
        }
        if n == 0 {
            return true;
        }
        let mut start = 0;
        let mut all_ok = true;
        while start < n {
            let len = (n - start).min(p.max_block);
            let last = start + len >= n;
            // SAFETY: buffer dimensionati in `prepare`; il plugin accede solo durante la
            // chiamata.
            all_ok &= unsafe { self.process_chunk(p, events, input, out_l, out_r, start, len, start == 0, last) };
            start += len;
        }
        all_ok
    }

    fn set_param_normalized(&self, id: u32, value: f64) {
        let v = value.clamp(0.0, 1.0);
        self.shared.to_proc.push(id, v);
        self.shared.to_ctrl.push(id, v);
    }

    fn reset(&self) {
        // Può arrivare dal thread audio: le note vengono spente al blocco successivo.
        self.pending_reset.store(true, Ordering::Release);
    }

    fn params(&self) -> Vec<ExternalParam> {
        let _ui = self.ui.lock();
        self.sync_controller();
        let Some(ctrl) = self.controller() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // SAFETY: chiamate al controller sul thread UI.
        unsafe {
            for i in 0..ctrl.getParameterCount().max(0) {
                let mut pi: ParameterInfo = std::mem::zeroed();
                if !ok(ctrl.getParameterInfo(i, &mut pi)) {
                    continue;
                }
                use ParameterInfo_::ParameterFlags_ as F;
                let mut name = super::module::wstr(&pi.title);
                if name.is_empty() {
                    name = super::module::wstr(&pi.shortTitle);
                }
                out.push(ExternalParam {
                    id: pi.id,
                    name,
                    unit: super::module::wstr(&pi.units),
                    default: pi.defaultNormalizedValue.clamp(0.0, 1.0),
                    steps: pi.stepCount.max(0) as u32,
                    automatable: pi.flags & F::kCanAutomate != 0,
                    hidden: pi.flags & (F::kIsHidden | F::kIsReadOnly) != 0,
                });
            }
        }
        out
    }

    fn param_normalized(&self, id: u32) -> f64 {
        let _ui = self.ui.lock();
        self.sync_controller();
        // SAFETY: controller valido, thread UI.
        self.controller().map_or(0.0, |c| unsafe { c.getParamNormalized(id) })
    }

    fn param_display(&self, id: u32, value: f64) -> String {
        let _ui = self.ui.lock();
        if let Some(c) = self.controller() {
            let mut s: String128 = [0; 128];
            // SAFETY: buffer String128 valido.
            if ok(unsafe { c.getParamStringByValue(id, value, &mut s) }) {
                let t = super::module::wstr(&s);
                if !t.is_empty() {
                    return t;
                }
            }
        }
        format!("{value:.3}")
    }

    fn save_state(&self) -> Result<Vec<u8>, HostError> {
        let _ui = self.ui.lock();
        self.sync_controller();
        let com = &*self.com;
        // SAFETY: stream validi per la durata delle chiamate.
        unsafe {
            let cs = MemStream::new(Vec::new());
            let r = com.component.getState(stream_ptr(&cs));
            if !ok(r) && r != kNotImplemented {
                return Err(plugin_err("IComponent::getState", r));
            }
            let ks = MemStream::new(Vec::new());
            if let Some(ctrl) = self.controller() {
                let r = ctrl.getState(stream_ptr(&ks));
                if !ok(r) && r != kNotImplemented {
                    return Err(plugin_err("IEditController::getState", r));
                }
            }
            Ok(encode_state(&cs.take(), &ks.take()))
        }
    }

    fn load_state(&self, data: &[u8]) -> Result<(), HostError> {
        let (comp, ctrl_state) = decode_state(data)
            .ok_or_else(|| HostError::Plugin(format!("{}: stato VST3 non valido", self.info.name)))?;
        let _ui = self.ui.lock();
        // Cambi accodati prima del caricamento: superati dallo stato.
        self.sync_controller();
        let com = &*self.com;
        // SAFETY: stream validi per la durata delle chiamate; processore fermo durante
        // `setState` del componente (lock `proc`).
        unsafe {
            {
                let _p = self.proc.lock();
                self.shared.to_proc.clear();
                let s = MemStream::new(comp.to_vec());
                let r = com.component.setState(stream_ptr(&s));
                if !ok(r) {
                    return Err(plugin_err("IComponent::setState", r));
                }
            }
            if let Some(ctrl) = self.controller() {
                let s = MemStream::new(comp.to_vec());
                ctrl.setComponentState(stream_ptr(&s));
                if !ctrl_state.is_empty() {
                    let s = MemStream::new(ctrl_state.to_vec());
                    ctrl.setState(stream_ptr(&s));
                }
            }
        }
        Ok(())
    }

    fn has_editor(&self) -> bool {
        let mut ui = self.ui.lock();
        if let Some(h) = ui.has_editor {
            return h;
        }
        let h = self.controller().is_some_and(|c| {
            // SAFETY: la vista creata viene rilasciata subito.
            unsafe { ComPtr::from_raw(c.createView(c"editor".as_ptr())).is_some() }
        });
        ui.has_editor = Some(h);
        h
    }

    fn open_editor(&self, parent: ParentWindow) -> Result<EditorSize, HostError> {
        let mut ui = self.ui.lock();
        self.sync_controller();
        // SAFETY: vista e finestra padre valide sul thread UI.
        unsafe {
            if let Some(v) = &ui.view {
                return Ok(view_size(v));
            }
            let ctrl = self
                .controller()
                .ok_or_else(|| HostError::Unsupported(format!("{}: nessun controller", self.info.name)))?;
            let (ptype, handle) = match parent {
                ParentWindow::AppKit(p) => (c"NSView", p),
                ParentWindow::Win32(h) => (c"HWND", h as *mut std::ffi::c_void),
                ParentWindow::X11(id) => (c"X11EmbedWindowID", id as usize as *mut std::ffi::c_void),
            };
            let view = ComPtr::from_raw(ctrl.createView(c"editor".as_ptr()))
                .ok_or_else(|| HostError::Unsupported(format!("{}: nessun editor", self.info.name)))?;
            if !ok(view.isPlatformTypeSupported(ptype.as_ptr())) {
                return Err(HostError::Unsupported(format!(
                    "{}: piattaforma {} non supportata dall'editor",
                    self.info.name,
                    ptype.to_string_lossy()
                )));
            }
            view.setFrame(host_ptr::<IPlugFrame>(&self.host));
            let r = view.attached(handle, ptype.as_ptr());
            if !ok(r) {
                view.setFrame(std::ptr::null_mut());
                return Err(plugin_err("IPlugView::attached", r));
            }
            let size = view_size(&view);
            *self.shared.resize.lock() = None;
            ui.view = Some(view);
            ui.has_editor = Some(true);
            Ok(size)
        }
    }

    fn close_editor(&self) {
        let view = self.ui.lock().view.take();
        if let Some(v) = view {
            // SAFETY: vista aperta da `open_editor`.
            unsafe {
                v.removed();
                v.setFrame(std::ptr::null_mut());
            }
        }
    }

    fn take_editor_resize(&self) -> Option<EditorSize> {
        self.shared.resize.lock().take()
    }

    fn idle(&self) {
        {
            let _ui = self.ui.lock();
            self.sync_controller();
            self.shared.run_loop_idle();
        }
        self.handle_restart();
    }

    fn latency(&self) -> u32 {
        self.latency.load(Ordering::Relaxed)
    }

    fn take_param_changes(&self) -> Vec<(u32, f64)> {
        self.shared.gui_edits.drain()
    }
}

/// Dimensione corrente della vista (minimo 1×1, default 640×480).
unsafe fn view_size(v: &ComPtr<IPlugView>) -> EditorSize {
    let mut r: ViewRect = std::mem::zeroed();
    if ok(v.getSize(&mut r)) && r.right > r.left && r.bottom > r.top {
        EditorSize {
            width: (r.right - r.left) as u32,
            height: (r.bottom - r.top) as u32,
        }
    } else {
        EditorSize {
            width: 640,
            height: 480,
        }
    }
}

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        // Ordine: editor → stop processing → disattivazione → disconnessione → terminate
        // controller e componente → rilascio → (dopo, i campi) contesto host e modulo.
        let view = self.ui.get_mut().view.take();
        let p = self.proc.get_mut();
        // SAFETY: nessun altro riferimento all'istanza (siamo in `Drop`).
        unsafe {
            if let Some(v) = view {
                v.removed();
                v.setFrame(std::ptr::null_mut());
            }
            let com = &mut *self.com;
            Self::deactivate(com, p);
            if let Some((a, b)) = com.connection.take() {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if com.separate {
                if let Some(ctrl) = &com.controller {
                    ctrl.terminate();
                }
            }
            com.component.terminate();
            self.shared.clear_run_loop();
            ManuallyDrop::drop(&mut self.com);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_blob_roundtrip() {
        let blob = encode_state(b"abc", b"");
        assert!(blob.starts_with(STATE_MAGIC));
        assert_eq!(decode_state(&blob), Some((&b"abc"[..], &b""[..])));
        let blob = encode_state(b"", b"xy");
        assert_eq!(decode_state(&blob), Some((&b""[..], &b"xy"[..])));
        assert_eq!(decode_state(&blob[..blob.len() - 1]), None);
        let mut extra = blob.clone();
        extra.push(0);
        assert_eq!(decode_state(&extra), None);
        assert_eq!(decode_state(b"MFV3"), None);
        assert_eq!(decode_state(b"garbage-garbage-garbage"), None);
    }
}
