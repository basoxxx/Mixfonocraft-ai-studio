//! Istanza CLAP attiva.
//!
//! Threading:
//! - `main` (mutex): `PluginInstance` di clack, usato solo dalle chiamate lato UI;
//! - `audio` (mutex separato): processore audio + buffer preallocati; `process` lo prende
//!   con `try_lock` (se occupato restituisce `false`). Il thread UI lo prende solo per
//!   `prepare`, `flush` dei parametri a processore fermo e teardown (ordine: main → audio).
//! - i cambi di parametro passano da una coda limitata (nessuna allocazione) e da una
//!   tabella di valori atomici letta da `param_normalized`.

use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, PluginGui, Window};
use clack_extensions::latency::PluginLatency;
use clack_extensions::note_ports::{NoteDialect, NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::events::event_types::{MidiEvent as ClapMidiEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::events::Match;
use clack_host::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;

use super::handlers::{Flags, MainThreadImpl, MfcHost, SharedHandlerImpl};
use crate::{EditorSize, ExternalInstance, ExternalParam, HostError, MidiEvent, ParentWindow, PluginInfo};

/// Capacità delle code di eventi (ingresso/uscita) e dei cambi di parametro.
const MAX_EVENTS: usize = 1024;

/// Parametro con range reale e valore normalizzato corrente (atomico).
struct ParamSlot {
    id: u32,
    min: f64,
    max: f64,
    stepped: bool,
    /// Valore normalizzato (bit di f64).
    value: AtomicU64,
}

impl ParamSlot {
    fn to_plain(&self, n: f64) -> f64 {
        let n = if n.is_finite() { n.clamp(0.0, 1.0) } else { 0.0 };
        let v = self.min + n * (self.max - self.min);
        if self.stepped {
            v.round()
        } else {
            v
        }
    }
    fn to_normalized(&self, v: f64) -> f64 {
        let range = self.max - self.min;
        if range > 0.0 && v.is_finite() {
            ((v - self.min) / range).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
    fn get(&self) -> f64 {
        f64::from_bits(self.value.load(Ordering::Relaxed))
    }
    fn set(&self, n: f64) {
        self.value.store(n.to_bits(), Ordering::Relaxed);
    }
}

/// Tabella dei parametri (ordinata per id, fissa dopo la creazione).
struct ParamTable {
    slots: Vec<ParamSlot>,
    list: Vec<ExternalParam>,
}

impl ParamTable {
    fn slot(&self, id: u32) -> Option<&ParamSlot> {
        self.slots
            .binary_search_by_key(&id, |s| s.id)
            .ok()
            .map(|i| &self.slots[i])
    }
}

/// Estensioni del plugin (copiabili, interrogate una volta).
#[derive(Clone, Copy)]
struct Exts {
    params: Option<PluginParams>,
    state: Option<PluginState>,
    gui: Option<PluginGui>,
    audio_ports: Option<PluginAudioPorts>,
    note_ports: Option<PluginNotePorts>,
    latency: Option<PluginLatency>,
}

/// Parte main-thread. `PluginInstance` non è `Send`: lo diventa qui perché è usato solo
/// sotto mutex dalle chiamate lato UI (contratto di [`ExternalInstance`]).
struct MainState {
    plugin: PluginInstance<MfcHost>,
    exts: Exts,
    sample_rate: f32,
    max_block: usize,
    editor_open: bool,
}

// SAFETY: vedi doc di `MainState`; l'accesso è serializzato dal mutex.
unsafe impl Send for MainState {}

/// Come vengono inviate le note.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Clap,
    Midi,
}

/// Parte audio: processore e buffer preallocati.
struct AudioState {
    processor: PluginAudioProcessor<MfcHost>,
    params_ext: Option<PluginParams>,
    in_events: EventBuffer,
    out_events: EventBuffer,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    /// Canali di ingresso/uscita (tutte le porte, in sequenza) e canali per porta.
    in_bufs: Vec<Vec<f32>>,
    in_layout: Vec<usize>,
    out_bufs: Vec<Vec<f32>>,
    out_layout: Vec<usize>,
    max_block: usize,
    dialect: Dialect,
    /// Note tenute per tasto (per AllNotesOff).
    held: [u16; 128],
    steady_time: u64,
}

/// Istanza CLAP.
pub struct ClapInstance {
    info: PluginInfo,
    flags: Arc<Flags>,
    params: ParamTable,
    queue_tx: Sender<(u32, f64)>,
    queue_rx: Receiver<(u32, f64)>,
    /// `true` dopo il primo `start_processing` riuscito (dal thread audio).
    started: AtomicBool,
    reset_pending: AtomicBool,
    latency: AtomicU32,
    main: Mutex<MainState>,
    audio: Mutex<Option<AudioState>>,
}

fn plugin_err(e: impl std::fmt::Display) -> HostError {
    HostError::Plugin(e.to_string())
}

impl ClapInstance {
    /// Crea, inizializza e attiva un'istanza (da chiamare dal thread UI/main).
    pub fn new(path: &Path, id: &str, sample_rate: f32, max_block: usize) -> Result<Self, HostError> {
        let entry = super::load_entry(path)?;
        let info = super::describe(&entry, path)?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| HostError::NotFound(id.to_string()))?;
        let host_info = HostInfo::new(
            "Mixfonocraft",
            "Mixfonocraft",
            "https://github.com/basoxxx/Mixfonocraft-ai-studio",
            env!("CARGO_PKG_VERSION"),
        )
        .map_err(plugin_err)?;
        let cid = CString::new(id).map_err(|_| HostError::BadKind(id.to_string()))?;
        let flags = Arc::new(Flags::default());
        let (f1, f2) = (flags.clone(), flags.clone());
        let mut plugin = PluginInstance::<MfcHost>::new(
            move |_| SharedHandlerImpl { flags: f1 },
            move |_| MainThreadImpl { flags: f2 },
            &entry,
            &cid,
            &host_info,
        )
        .map_err(plugin_err)?;

        let exts = {
            let h = plugin.plugin_handle();
            Exts {
                params: h.get_extension(),
                state: h.get_extension(),
                gui: h.get_extension(),
                audio_ports: h.get_extension(),
                note_ports: h.get_extension(),
                latency: h.get_extension(),
            }
        };
        let params = read_params(&mut plugin, exts.params);
        let (queue_tx, queue_rx) = crossbeam_channel::bounded(MAX_EVENTS);
        let inst = Self {
            info,
            flags,
            params,
            queue_tx,
            queue_rx,
            started: AtomicBool::new(false),
            reset_pending: AtomicBool::new(false),
            latency: AtomicU32::new(0),
            main: Mutex::new(MainState {
                plugin,
                exts,
                sample_rate,
                max_block,
                editor_open: false,
            }),
            audio: Mutex::new(None),
        };
        inst.prepare(sample_rate, max_block)?;
        Ok(inst)
    }

    /// Ferma e disattiva il processore (thread UI, con entrambi i lock).
    fn deactivate(main: &mut MainState, audio: &mut Option<AudioState>) {
        if let Some(a) = audio.take() {
            let stopped = a.processor.into_stopped();
            main.plugin.deactivate(stopped);
        }
    }

    /// Attiva con la configurazione corrente di `main`.
    fn activate(&self, main: &mut MainState) -> Result<AudioState, HostError> {
        let max_block = main.max_block.max(1);
        let exts = main.exts;
        // Porte audio e note: lette a plugin inattivo.
        let (in_layout, out_layout, dialect) = {
            let h = main.plugin.plugin_handle();
            let ports = |is_input: bool| -> Vec<usize> {
                let Some(ap) = exts.audio_ports else {
                    return Vec::new();
                };
                let mut buf = AudioPortInfoBuffer::new();
                (0..ap.count(&h, is_input).min(16))
                    .map(|i| {
                        ap.get(&h, i, is_input, &mut buf)
                            .map(|p| p.channel_count.min(32) as usize)
                            .unwrap_or(0)
                    })
                    .collect()
            };
            let dialect = match exts.note_ports {
                Some(np) if np.count(&h, true) > 0 => {
                    let mut buf = NotePortInfoBuffer::new();
                    match np.get(&h, 0, true, &mut buf) {
                        Some(p)
                            if !p.supported_dialects.supports(NoteDialect::Clap)
                                && p.supported_dialects.supports(NoteDialect::Midi) =>
                        {
                            Dialect::Midi
                        }
                        _ => Dialect::Clap,
                    }
                }
                _ => Dialect::Clap,
            };
            (ports(true), ports(false), dialect)
        };
        let config = PluginAudioConfiguration {
            sample_rate: f64::from(main.sample_rate),
            min_frames_count: 1,
            max_frames_count: u32::try_from(max_block).unwrap_or(u32::MAX),
        };
        let processor = main.plugin.activate(|_, _| (), config).map_err(plugin_err)?;
        if let Some(lat) = exts.latency {
            let h = main.plugin.plugin_handle();
            self.latency.store(lat.get(&h), Ordering::Relaxed);
        }
        let n_in: usize = in_layout.iter().sum();
        let n_out: usize = out_layout.iter().sum();
        Ok(AudioState {
            processor: processor.into(),
            params_ext: exts.params,
            in_events: EventBuffer::with_capacity(MAX_EVENTS * 2),
            out_events: EventBuffer::with_capacity(MAX_EVENTS),
            in_ports: AudioPorts::with_capacity(n_in, in_layout.len()),
            out_ports: AudioPorts::with_capacity(n_out, out_layout.len()),
            in_bufs: vec![vec![0.0; max_block]; n_in],
            in_layout,
            out_bufs: vec![vec![0.0; max_block]; n_out],
            out_layout,
            max_block,
            dialect,
            held: [0; 128],
            steady_time: 0,
        })
    }

    /// Invia i cambi di parametro in coda tramite `params.flush` (processore fermo).
    /// Con `force` (solo thread UI, es. prima di salvare lo stato) un processore avviato viene
    /// fermato per consegnare i cambi anche se il motore non sta chiamando `process`;
    /// il successivo `process` lo riavvia.
    fn flush_locked(&self, audio: &mut Option<AudioState>, main: Option<&mut MainState>, force: bool) {
        let has_queue = !self.queue_rx.is_empty();
        let requested = Flags::take(&self.flags.flush);
        if !has_queue && !requested {
            return;
        }
        match audio {
            Some(a) => {
                if a.processor.is_started() && force && has_queue {
                    a.processor.ensure_processing_stopped();
                    self.started.store(false, Ordering::Release);
                }
                if a.processor.is_started() {
                    // Arriveranno con il prossimo `process`.
                    if requested {
                        self.flags.flush.store(true, Ordering::Release);
                    }
                    return;
                }
                a.in_events.clear();
                a.out_events.clear();
                self.drain_params(&mut a.in_events, MAX_EVENTS);
                if let Some(p) = a.params_ext {
                    let mut handle = a.processor.plugin_handle();
                    p.flush_active(&mut handle, &a.in_events.as_input(), &mut a.out_events.as_output());
                }
                self.read_output_params(&a.out_events);
                a.in_events.clear();
                a.out_events.clear();
            }
            None => {
                let Some(main) = main else { return };
                let mut ins = EventBuffer::with_capacity(MAX_EVENTS);
                let mut outs = EventBuffer::with_capacity(MAX_EVENTS);
                self.drain_params(&mut ins, MAX_EVENTS);
                if let (Some(p), Some(mut h)) = (main.exts.params, main.plugin.inactive_plugin_handle()) {
                    p.flush(&mut h, &ins.as_input(), &mut outs.as_output());
                }
                self.read_output_params(&outs);
            }
        }
    }

    /// Svuota la coda dei parametri nel buffer (al massimo `limit` eventi, tempo 0).
    fn drain_params(&self, buf: &mut EventBuffer, limit: usize) {
        let mut n = 0;
        while n < limit {
            let Ok((id, plain)) = self.queue_rx.try_recv() else {
                break;
            };
            if let Some(cid) = ClapId::from_raw(id) {
                buf.push(&ParamValueEvent::new(0, cid, Pckn::match_all(), plain));
                n += 1;
            }
        }
    }

    /// Aggiorna la cache con i valori di parametro emessi dal plugin.
    fn read_output_params(&self, out: &EventBuffer) {
        for ev in out {
            if let Some(p) = ev.as_event::<ParamValueEvent>() {
                if let Some(slot) = p.param_id().and_then(|id| self.params.slot(id.get())) {
                    slot.set(slot.to_normalized(p.value()));
                }
            }
        }
    }

    /// Rilegge i valori correnti dal plugin (thread UI).
    fn refresh_values(&self, main: &mut MainState) {
        let Some(p) = main.exts.params else { return };
        let h = main.plugin.plugin_handle();
        for slot in &self.params.slots {
            if let Some(v) = ClapId::from_raw(slot.id).and_then(|id| p.get_value(&h, id)) {
                slot.set(slot.to_normalized(v));
            }
        }
    }

    fn destroy_editor(main: &mut MainState) {
        if !main.editor_open {
            return;
        }
        main.editor_open = false;
        if let Some(gui) = main.exts.gui {
            let h = main.plugin.plugin_handle();
            let _ = gui.hide(&h);
            gui.destroy(&h);
        }
    }

    /// Elabora un blocco (al massimo `max_block` campioni) con la parte audio già bloccata.
    #[allow(clippy::too_many_arguments)]
    fn process_chunk(
        &self,
        a: &mut AudioState,
        events: &[MidiEvent],
        start: usize,
        last: bool,
        input: Option<(&[f32], &[f32])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        let len = out_l.len();
        let AudioState {
            processor,
            in_events,
            out_events,
            in_ports,
            out_ports,
            in_bufs,
            in_layout,
            out_bufs,
            out_layout,
            dialect,
            held,
            steady_time,
            ..
        } = a;
        in_events.clear();
        out_events.clear();
        let mut count = 0usize;
        if start == 0 {
            Flags::take(&self.flags.flush);
            self.drain_params(in_events, MAX_EVENTS / 2);
            count = in_events.len() as usize;
        }
        let end = start + len;
        for ev in events {
            let off = match *ev {
                MidiEvent::NoteOn { offset, .. }
                | MidiEvent::NoteOff { offset, .. }
                | MidiEvent::AllNotesOff { offset } => offset as usize,
            };
            if off < start || (off >= end && !last) {
                continue;
            }
            let t = (off - start).min(len.saturating_sub(1)) as u32;
            match *ev {
                MidiEvent::NoteOn {
                    key, velocity, note_id, ..
                } => {
                    let k = key.min(127);
                    held[k as usize] = held[k as usize].saturating_add(1);
                    if count < MAX_EVENTS {
                        push_note(in_events, *dialect, true, t, k, velocity, note_id);
                        count += 1;
                    }
                }
                MidiEvent::NoteOff {
                    key, velocity, note_id, ..
                } => {
                    let k = key.min(127);
                    held[k as usize] = held[k as usize].saturating_sub(1);
                    if count < MAX_EVENTS {
                        push_note(in_events, *dialect, false, t, k, velocity, note_id);
                        count += 1;
                    }
                }
                MidiEvent::AllNotesOff { .. } => {
                    for k in 0..128u8 {
                        if held[k as usize] > 0 && count < MAX_EVENTS {
                            push_note(in_events, *dialect, false, t, k, 0.0, -1);
                            count += 1;
                        }
                        held[k as usize] = 0;
                    }
                    if *dialect == Dialect::Midi && count < MAX_EVENTS {
                        in_events.push(&ClapMidiEvent::new(t, 0, [0xB0, 123, 0]));
                        count += 1;
                    }
                }
            }
        }
        in_events.sort();

        // Ingresso: porta principale ← stereo dato (o silenzio), altre porte a zero.
        let mut ch = 0;
        for (port, &cc) in in_layout.iter().enumerate() {
            for c in 0..cc {
                let buf = &mut in_bufs[ch][..len];
                match (port, input) {
                    (0, Some((l, r))) => {
                        let src = if c == 0 { l } else { r };
                        let n = src.len().saturating_sub(start).min(len);
                        if n > 0 {
                            buf[..n].copy_from_slice(&src[start..start + n]);
                        }
                        buf[n..].fill(0.0);
                    }
                    _ => buf.fill(0.0),
                }
                ch += 1;
            }
        }
        for b in out_bufs.iter_mut() {
            b[..len].fill(0.0);
        }

        let ok = {
            let ins = in_ports.with_input_buffers(input_ports(in_bufs, in_layout, len));
            let mut outs = out_ports.with_output_buffers(output_ports(out_bufs, out_layout, len));
            let Ok(started) = processor.ensure_processing_started() else {
                return false;
            };
            let r = started.process(
                &ins,
                &mut outs,
                &in_events.as_input(),
                &mut out_events.as_output(),
                Some(*steady_time),
                None,
            );
            r.is_ok()
        };
        *steady_time = steady_time.wrapping_add(len as u64);
        self.read_output_params(out_events);
        if !ok {
            return false;
        }

        // Uscita: porta principale → stereo (mono duplicato).
        match out_layout.first().copied().unwrap_or(0) {
            0 => {
                out_l.fill(0.0);
                out_r.fill(0.0);
            }
            1 => {
                out_l.copy_from_slice(&out_bufs[0][..len]);
                out_r.copy_from_slice(&out_bufs[0][..len]);
            }
            _ => {
                out_l.copy_from_slice(&out_bufs[0][..len]);
                out_r.copy_from_slice(&out_bufs[1][..len]);
            }
        }
        true
    }
}

fn push_note(buf: &mut EventBuffer, dialect: Dialect, on: bool, t: u32, key: u8, velocity: f32, note_id: i32) {
    let vel = f64::from(velocity.clamp(0.0, 1.0));
    match dialect {
        Dialect::Clap => {
            let nid: Match<u32> = if note_id >= 0 {
                Match::Specific(note_id as u32)
            } else {
                Match::All
            };
            let pckn = Pckn::new(0u16, 0u16, u16::from(key), nid);
            if on {
                buf.push(&NoteOnEvent::new(t, pckn, vel));
            } else {
                buf.push(&NoteOffEvent::new(t, pckn, vel));
            }
        }
        Dialect::Midi => {
            let v = (vel * 127.0).round() as u8;
            let data = if on { [0x90, key, v.max(1)] } else { [0x80, key, v] };
            buf.push(&ClapMidiEvent::new(t, 0, data));
        }
    }
}

type F64In<'b> = core::array::IntoIter<InputChannel<'b, f64>, 0>;
type F64Out<'b> = core::array::IntoIter<&'b mut [f64], 0>;

/// Porte di ingresso dai canali preallocati (nessuna allocazione).
fn input_ports<'b>(
    bufs: &'b mut [Vec<f32>],
    layout: &'b [usize],
    len: usize,
) -> impl ExactSizeIterator<Item = AudioPortBuffer<impl Iterator<Item = InputChannel<'b, f32>>, F64In<'b>>> + 'b {
    let mut rest: &'b mut [Vec<f32>] = bufs;
    layout.iter().map(move |&cc| {
        let (head, tail) = std::mem::take(&mut rest).split_at_mut(cc);
        rest = tail;
        AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::F32(head.iter_mut().map(move |b| InputChannel::variable(&mut b[..len]))),
        }
    })
}

/// Porte di uscita dai canali preallocati (nessuna allocazione).
fn output_ports<'b>(
    bufs: &'b mut [Vec<f32>],
    layout: &'b [usize],
    len: usize,
) -> impl ExactSizeIterator<Item = AudioPortBuffer<impl Iterator<Item = &'b mut [f32]>, F64Out<'b>>> + 'b {
    let mut rest: &'b mut [Vec<f32>] = bufs;
    layout.iter().map(move |&cc| {
        let (head, tail) = std::mem::take(&mut rest).split_at_mut(cc);
        rest = tail;
        AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::F32(head.iter_mut().map(move |b| &mut b[..len])),
        }
    })
}

/// Legge l'elenco dei parametri (thread UI, plugin inattivo).
fn read_params(plugin: &mut PluginInstance<MfcHost>, ext: Option<PluginParams>) -> ParamTable {
    let mut slots = Vec::new();
    let mut list = Vec::new();
    if let Some(p) = ext {
        let h = plugin.plugin_handle();
        let mut buf = ParamInfoBuffer::new();
        for i in 0..p.count(&h) {
            let Some(info) = p.get_info(&h, i, &mut buf) else {
                continue;
            };
            let (min, max) = if info.max_value >= info.min_value {
                (info.min_value, info.max_value)
            } else {
                (info.max_value, info.min_value)
            };
            let stepped = info.flags.contains(ParamInfoFlags::IS_STEPPED);
            let id = info.id.get();
            let slot = ParamSlot {
                id,
                min,
                max,
                stepped,
                value: AtomicU64::new(0),
            };
            let current = p.get_value(&h, info.id).unwrap_or(info.default_value);
            slot.set(slot.to_normalized(current));
            list.push(ExternalParam {
                id,
                name: String::from_utf8_lossy(info.name).into_owned(),
                unit: String::new(),
                default: slot.to_normalized(info.default_value),
                steps: if stepped {
                    (max - min).round().clamp(0.0, f64::from(u32::MAX)) as u32
                } else {
                    0
                },
                automatable: info.flags.contains(ParamInfoFlags::IS_AUTOMATABLE),
                hidden: info.flags.contains(ParamInfoFlags::IS_HIDDEN),
            });
            slots.push(slot);
        }
    }
    slots.sort_by_key(|s| s.id);
    slots.dedup_by_key(|s| s.id);
    ParamTable { slots, list }
}

impl ExternalInstance for ClapInstance {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn prepare(&self, sample_rate: f32, max_block: usize) -> Result<(), HostError> {
        let mut main = self.main.lock();
        let mut audio = self.audio.lock();
        Self::deactivate(&mut main, &mut audio);
        self.started.store(false, Ordering::Release);
        main.sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        main.max_block = max_block.max(1);
        let state = self.activate(&mut main)?;
        *audio = Some(state);
        self.flags.flush.store(true, Ordering::Release);
        self.flush_locked(&mut audio, Some(&mut main), false);
        Ok(())
    }

    fn process(
        &self,
        events: &[MidiEvent],
        input: Option<(&[f32], &[f32])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        let n = out_l.len().min(out_r.len());
        let Some(mut guard) = self.audio.try_lock() else {
            return false;
        };
        let Some(a) = guard.as_mut() else {
            return false;
        };
        if self.reset_pending.swap(false, Ordering::AcqRel) {
            a.held = [0; 128];
            a.processor.reset();
        }
        if a.processor.ensure_processing_started().is_err() {
            return false;
        }
        self.started.store(true, Ordering::Release);
        let max = a.max_block;
        let mut start = 0;
        loop {
            let len = (n - start).min(max);
            let last = start + len >= n;
            if !self.process_chunk(
                a,
                events,
                start,
                last,
                input,
                &mut out_l[start..start + len],
                &mut out_r[start..start + len],
            ) {
                return false;
            }
            start += len;
            if last {
                break;
            }
        }
        true
    }

    fn set_param_normalized(&self, id: u32, value: f64) {
        let Some(slot) = self.params.slot(id) else { return };
        let n = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        slot.set(n);
        let _ = self.queue_tx.try_send((id, slot.to_plain(n)));
        // A processore fermo il cambio va consegnato con `flush`.
        if !self.started.load(Ordering::Acquire) {
            if let Some(mut audio) = self.audio.try_lock() {
                self.flush_locked(&mut audio, None, false);
            }
        }
    }

    fn reset(&self) {
        self.reset_pending.store(true, Ordering::Release);
    }

    fn params(&self) -> Vec<ExternalParam> {
        self.params.list.clone()
    }

    fn param_normalized(&self, id: u32) -> f64 {
        self.params.slot(id).map(|s| s.get()).unwrap_or(0.0)
    }

    fn param_display(&self, id: u32, value: f64) -> String {
        let Some(slot) = self.params.slot(id) else {
            return String::new();
        };
        let plain = slot.to_plain(value);
        let mut main = self.main.lock();
        if let (Some(p), Some(cid)) = (main.exts.params, ClapId::from_raw(id)) {
            let h = main.plugin.plugin_handle();
            let mut buf = [0u8; 256];
            if let Ok(text) = p.value_to_text(&h, cid, plain, &mut buf) {
                let s = String::from_utf8_lossy(text).trim().to_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
        format!("{plain:.3}")
    }

    fn save_state(&self) -> Result<Vec<u8>, HostError> {
        let mut main = self.main.lock();
        {
            let mut audio = self.audio.lock();
            self.flush_locked(&mut audio, Some(&mut main), true);
        }
        let state = main
            .exts
            .state
            .ok_or_else(|| HostError::Unsupported("estensione state assente".into()))?;
        let h = main.plugin.plugin_handle();
        let mut out = Vec::new();
        state.save(&h, &mut out).map_err(plugin_err)?;
        Ok(out)
    }

    fn load_state(&self, data: &[u8]) -> Result<(), HostError> {
        let mut main = self.main.lock();
        let state = main
            .exts
            .state
            .ok_or_else(|| HostError::Unsupported("estensione state assente".into()))?;
        let result = {
            let h = main.plugin.plugin_handle();
            let mut reader = data;
            state.load(&h, &mut reader).map_err(plugin_err)
        };
        self.refresh_values(&mut main);
        result
    }

    fn has_editor(&self) -> bool {
        let Some(api) = GuiApiType::default_for_current_platform() else {
            return false;
        };
        let mut main = self.main.lock();
        let Some(gui) = main.exts.gui else { return false };
        let h = main.plugin.plugin_handle();
        gui.is_api_supported(
            &h,
            GuiConfiguration {
                api_type: api,
                is_floating: false,
            },
        )
    }

    fn open_editor(&self, parent: ParentWindow) -> Result<EditorSize, HostError> {
        let mut main = self.main.lock();
        let gui = main
            .exts
            .gui
            .ok_or_else(|| HostError::Unsupported("il plugin non ha un editor".into()))?;
        Self::destroy_editor(&mut main);
        // SAFETY: l'handle è fornito dalla GUI dell'host e resta valido finché l'editor
        // non viene chiuso con `close_editor` (contratto di `open_editor`).
        let window = unsafe {
            match parent {
                ParentWindow::AppKit(view) => Window::from_cocoa_nsview(view),
                ParentWindow::Win32(hwnd) => Window::from_win32_hwnd(hwnd as *mut std::ffi::c_void),
                ParentWindow::X11(id) => Window::from_x11_handle(id as std::ffi::c_ulong),
            }
        };
        let config = GuiConfiguration {
            api_type: window.api_type(),
            is_floating: false,
        };
        let h = main.plugin.plugin_handle();
        if !gui.is_api_supported(&h, config) {
            return Err(HostError::Unsupported(format!("API GUI {:?}", config.api_type)));
        }
        gui.create(&h, config).map_err(plugin_err)?;
        if !config.api_type.uses_logical_size() {
            let _ = gui.set_scale(&h, 1.0);
        }
        let size = gui.get_size(&h).unwrap_or(clack_extensions::gui::GuiSize {
            width: 640,
            height: 480,
        });
        // SAFETY: vedi sopra.
        if let Err(e) = unsafe { gui.set_parent(&h, window) } {
            gui.destroy(&h);
            return Err(plugin_err(e));
        }
        let _ = gui.show(&h);
        main.editor_open = true;
        self.flags.resize.store(0, Ordering::Release);
        Ok(EditorSize {
            width: size.width,
            height: size.height,
        })
    }

    fn close_editor(&self) {
        let mut main = self.main.lock();
        Self::destroy_editor(&mut main);
    }

    fn take_editor_resize(&self) -> Option<EditorSize> {
        self.flags.take_resize().map(|s| EditorSize {
            width: s.width,
            height: s.height,
        })
    }

    fn idle(&self) {
        let mut main = self.main.lock();
        if Flags::take(&self.flags.callback) {
            main.plugin.call_on_main_thread_callback();
        }
        if Flags::take(&self.flags.restart) {
            let mut audio = self.audio.lock();
            Self::deactivate(&mut main, &mut audio);
            self.started.store(false, Ordering::Release);
            match self.activate(&mut main) {
                Ok(s) => *audio = Some(s),
                Err(e) => eprintln!("[clap] riattivazione di {} fallita: {e}", self.info.name),
            }
        }
        if Flags::take(&self.flags.gui_closed) && Flags::take(&self.flags.gui_destroyed) {
            Self::destroy_editor(&mut main);
        }
        if Flags::take(&self.flags.latency_changed) {
            if let Some(lat) = main.exts.latency {
                let h = main.plugin.plugin_handle();
                self.latency.store(lat.get(&h), Ordering::Relaxed);
            }
        }
        {
            let mut audio = self.audio.lock();
            self.flush_locked(&mut audio, Some(&mut main), false);
        }
        if Flags::take(&self.flags.rescan_values) {
            self.refresh_values(&mut main);
        }
    }

    fn latency(&self) -> u32 {
        self.latency.load(Ordering::Relaxed)
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        // Ordine: editor → stop/disattivazione → distruzione (drop di `PluginInstance`).
        let main = self.main.get_mut();
        Self::destroy_editor(main);
        let audio = self.audio.get_mut();
        Self::deactivate(main, audio);
    }
}
