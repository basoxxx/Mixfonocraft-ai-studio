//! Plugin VST3 minimale usato dai test d'integrazione di `mfc-host`.
//!
//! Due classi, entrambe a componente singolo (il componente implementa anche
//! `IEditController`), senza GUI:
//! - "MFC Test Synth" (`Instrument|Synth`): sinusoide dell'ultima nota, ampiezza
//!   `velocity * gain`; parametro 0 "Gain" (default 0.5); stato = gain (f64 LE).
//! - "MFC Test Gain" (`Fx`): ingresso stereo moltiplicato per il parametro 0 "Gain"
//!   (default 1.0, lineare = normalizzato).
//!
//! Solo test: nel synth una nota con tasto [`GUI_EDIT_KEY`] non suona ma imposta il gain
//! alla sua velocity tramite `IComponentHandler::performEdit`, come farebbe la GUI.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]
// Tipi degli enum dei binding diversi per piattaforma (i32 su Windows, u32 altrove).
#![allow(clippy::unnecessary_cast)]

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{uid, Class, ComPtr, ComRef, ComWrapper};

/// Class id del synth.
pub const SYNTH_CID: TUID = uid(0x4D464354, 0x53796E74, 0x68000000, 0x00000001);
/// Class id dell'effetto.
pub const GAIN_CID: TUID = uid(0x4D464354, 0x4761696E, 0x00000000, 0x00000002);

const VENDOR: &str = "Mixfonocraft Test";

/// Tasto della nota che simula una modifica del gain dalla GUI del plugin.
pub const GUI_EDIT_KEY: i16 = 0;

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let n = src.len().min(dst.len().saturating_sub(1));
    for (d, s) in dst.iter_mut().zip(src.bytes().take(n)) {
        *d = s as c_char;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s as TChar;
        len += 1;
    }
    let end = len.min(dst.len().saturating_sub(1));
    if let Some(z) = dst.get_mut(end) {
        *z = 0;
    }
}

/// Stato della voce del synth.
#[derive(Default)]
struct Voice {
    key: i16,
    velocity: f32,
    active: bool,
    phase: f64,
}

/// Componente + processore + controller del plugin di prova.
struct TestPlugin {
    is_synth: bool,
    gain: AtomicU64,
    sample_rate: AtomicU64,
    voice: Mutex<Voice>,
    /// `IComponentHandler` dell'host (per `performEdit`).
    handler: Mutex<Option<ComPtr<IComponentHandler>>>,
}

impl TestPlugin {
    fn new(is_synth: bool) -> Self {
        let default = if is_synth { 0.5 } else { 1.0 };
        Self {
            is_synth,
            gain: AtomicU64::new(f64::to_bits(default)),
            sample_rate: AtomicU64::new(f64::to_bits(44_100.0)),
            voice: Mutex::new(Voice::default()),
            handler: Mutex::new(None),
        }
    }

    fn gain(&self) -> f64 {
        f64::from_bits(self.gain.load(Ordering::Relaxed))
    }

    fn set_gain(&self, v: f64) {
        self.gain.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn default_gain(&self) -> f64 {
        if self.is_synth {
            0.5
        } else {
            1.0
        }
    }

    /// Modifica del gain "dall'interfaccia": aggiorna il valore e lo notifica all'host.
    unsafe fn gui_edit(&self, v: f64) {
        self.set_gain(v);
        let handler = self.handler.lock().ok().and_then(|h| h.clone());
        if let Some(h) = handler {
            h.beginEdit(0);
            h.performEdit(0, self.gain());
            h.endEdit(0);
        }
    }

    unsafe fn read_gain(&self, stream: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(stream) else {
            return kInvalidArgument;
        };
        let mut buf = [0u8; 8];
        let mut read = 0;
        if s.read(buf.as_mut_ptr().cast(), 8, &mut read) != kResultOk || read != 8 {
            return kResultFalse;
        }
        self.set_gain(f64::from_le_bytes(buf));
        kResultOk
    }
}

impl Class for TestPlugin {
    type Interfaces = (IComponent, IAudioProcessor, IEditController);
}

impl IPluginBaseTrait for TestPlugin {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        if let Ok(mut h) = self.handler.lock() {
            *h = None;
        }
        kResultOk
    }
}

impl IComponentTrait for TestPlugin {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        // Componente singolo: nessun controller separato.
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> int32 {
        let (media, dir) = (media as MediaTypes, dir as BusDirections);
        match (media, dir) {
            (MediaTypes_::kAudio, BusDirections_::kOutput) => 1,
            (MediaTypes_::kAudio, BusDirections_::kInput) => i32::from(!self.is_synth),
            (MediaTypes_::kEvent, BusDirections_::kInput) => i32::from(self.is_synth),
            _ => 0,
        }
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: int32, bus: *mut BusInfo) -> tresult {
        if bus.is_null() || index != 0 || index >= self.getBusCount(media, dir) {
            return kInvalidArgument;
        }
        let bus = &mut *bus;
        bus.mediaType = media;
        bus.direction = dir;
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as uint32;
        if media as MediaTypes == MediaTypes_::kEvent {
            bus.channelCount = 1;
            copy_wstring("MIDI", &mut bus.name);
        } else {
            bus.channelCount = 2;
            let name = if dir as BusDirections == BusDirections_::kInput {
                "Input"
            } else {
                "Output"
            };
            copy_wstring(name, &mut bus.name);
        }
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: int32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, state: TBool) -> tresult {
        if state == 0 {
            if let Ok(mut v) = self.voice.lock() {
                *v = Voice::default();
            }
        }
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        self.read_gain(state)
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut buf = self.gain().to_le_bytes();
        let mut written = 0;
        if s.write(buf.as_mut_ptr().cast(), 8, &mut written) != kResultOk || written != 8 {
            return kResultFalse;
        }
        kResultOk
    }
}

impl IAudioProcessorTrait for TestPlugin {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: int32,
        outputs: *mut SpeakerArrangement,
        num_outs: int32,
    ) -> tresult {
        let want_ins = i32::from(!self.is_synth);
        if num_ins != want_ins || num_outs != 1 || outputs.is_null() {
            return kResultFalse;
        }
        if *outputs != SpeakerArr::kStereo {
            return kResultFalse;
        }
        if num_ins == 1 && (inputs.is_null() || *inputs != SpeakerArr::kStereo) {
            return kResultFalse;
        }
        kResultTrue
    }
    unsafe fn getBusArrangement(&self, dir: BusDirection, index: int32, arr: *mut SpeakerArrangement) -> tresult {
        if arr.is_null() || index != 0 || index >= self.getBusCount(MediaTypes_::kAudio as MediaType, dir) {
            return kInvalidArgument;
        }
        *arr = SpeakerArr::kStereo;
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size as SymbolicSampleSizes == SymbolicSampleSizes_::kSample32 {
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn getLatencySamples(&self) -> uint32 {
        0
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        if setup.is_null() {
            return kInvalidArgument;
        }
        self.sample_rate.store((*setup).sampleRate.to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        if data.is_null() {
            return kInvalidArgument;
        }
        let data = &*data;
        // Parametri: ultimo punto di ogni coda.
        if let Some(changes) = ComRef::from_raw(data.inputParameterChanges) {
            for i in 0..changes.getParameterCount() {
                if let Some(q) = ComRef::from_raw(changes.getParameterData(i)) {
                    let n = q.getPointCount();
                    let (mut off, mut val) = (0, 0.0);
                    if q.getParameterId() == 0 && n > 0 && q.getPoint(n - 1, &mut off, &mut val) == kResultOk {
                        self.set_gain(val);
                    }
                }
            }
        }
        let Ok(mut voice) = self.voice.lock() else {
            return kResultFalse;
        };
        if let Some(events) = ComRef::from_raw(data.inputEvents) {
            for i in 0..events.getEventCount() {
                let mut e: Event = std::mem::zeroed();
                if events.getEvent(i, &mut e) != kResultOk {
                    continue;
                }
                match e.r#type as Event_::EventTypes {
                    Event_::EventTypes_::kNoteOnEvent => {
                        let n = e.__field0.noteOn;
                        if n.pitch == GUI_EDIT_KEY {
                            self.gui_edit(f64::from(n.velocity));
                        } else if n.velocity > 0.0 {
                            voice.key = n.pitch;
                            voice.velocity = n.velocity;
                            voice.active = true;
                        } else if voice.key == n.pitch {
                            voice.active = false;
                        }
                    }
                    Event_::EventTypes_::kNoteOffEvent if voice.key == e.__field0.noteOff.pitch => {
                        voice.active = false;
                    }
                    _ => {}
                }
            }
        }
        let n = data.numSamples.max(0) as usize;
        if n == 0 || data.numOutputs < 1 || data.outputs.is_null() {
            return kResultOk;
        }
        let out = &*data.outputs;
        if out.numChannels < 1 || out.__field0.channelBuffers32.is_null() {
            return kResultOk;
        }
        let out_ch = std::slice::from_raw_parts(out.__field0.channelBuffers32, out.numChannels as usize);
        let gain = self.gain() as f32;
        if self.is_synth {
            let sr = f64::from_bits(self.sample_rate.load(Ordering::Relaxed)).max(1.0);
            let freq = 440.0 * 2f64.powf((voice.key as f64 - 69.0) / 12.0);
            let step = std::f64::consts::TAU * freq / sr;
            for i in 0..n {
                let v = if voice.active {
                    let s = voice.phase.sin() as f32 * voice.velocity * gain;
                    voice.phase = (voice.phase + step) % std::f64::consts::TAU;
                    s
                } else {
                    0.0
                };
                for ch in out_ch {
                    if !ch.is_null() {
                        *ch.add(i) = v;
                    }
                }
            }
        } else {
            let ins = if data.numInputs >= 1 && !data.inputs.is_null() {
                let b = &*data.inputs;
                if b.numChannels >= 1 && !b.__field0.channelBuffers32.is_null() {
                    std::slice::from_raw_parts(b.__field0.channelBuffers32, b.numChannels as usize)
                } else {
                    &[]
                }
            } else {
                &[]
            };
            for (c, och) in out_ch.iter().enumerate() {
                if och.is_null() {
                    continue;
                }
                let ich = ins.get(c).or(ins.first()).copied().unwrap_or(std::ptr::null_mut());
                for i in 0..n {
                    let x = if ich.is_null() { 0.0 } else { *ich.add(i) };
                    *och.add(i) = x * gain;
                }
            }
        }
        out_silence(data);
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> uint32 {
        0
    }
}

/// Azzera i flag di silenzio delle uscite.
unsafe fn out_silence(data: &ProcessData) {
    for i in 0..data.numOutputs.max(0) as usize {
        (*data.outputs.add(i)).silenceFlags = 0;
    }
}

impl IEditControllerTrait for TestPlugin {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        self.read_gain(state)
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> int32 {
        1
    }
    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        if index != 0 || info.is_null() {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.id = 0;
        copy_wstring("Gain", &mut info.title);
        copy_wstring("Gain", &mut info.shortTitle);
        copy_wstring("x", &mut info.units);
        info.stepCount = 0;
        info.defaultNormalizedValue = self.default_gain();
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate;
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: ParamID, value: ParamValue, string: *mut String128) -> tresult {
        if id != 0 || string.is_null() {
            return kInvalidArgument;
        }
        copy_wstring(&format!("{value:.2}"), &mut *string);
        kResultOk
    }
    unsafe fn getParamValueByString(&self, _id: ParamID, _s: *mut TChar, _v: *mut ParamValue) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        value
    }
    unsafe fn plainParamToNormalized(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        value
    }
    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        if id == 0 {
            self.gain()
        } else {
            0.0
        }
    }
    unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
        if id != 0 {
            return kInvalidArgument;
        }
        self.set_gain(value);
        kResultOk
    }
    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        if let Ok(mut h) = self.handler.lock() {
            *h = ComRef::from_raw(handler).map(|r| r.to_com_ptr());
        }
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

/// Fabbrica delle due classi.
struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory3,);
}

/// (cid, nome, sottocategorie)
const CLASSES: [(TUID, &str, &str); 2] = [
    (SYNTH_CID, "MFC Test Synth", "Instrument|Synth"),
    (GAIN_CID, "MFC Test Gain", "Fx"),
];

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        if info.is_null() {
            return kInvalidArgument;
        }
        let info = &mut *info;
        copy_cstring(VENDOR, &mut info.vendor);
        copy_cstring("https://example.invalid", &mut info.url);
        copy_cstring("", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> int32 {
        CLASSES.len() as int32
    }
    unsafe fn getClassInfo(&self, index: int32, info: *mut PClassInfo) -> tresult {
        let Some((cid, name, _)) = CLASSES.get(index as usize) else {
            return kInvalidArgument;
        };
        if info.is_null() {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.cid = *cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
        copy_cstring(name, &mut info.name);
        kResultOk
    }
    unsafe fn createInstance(&self, cid: FIDString, iid: FIDString, obj: *mut *mut c_void) -> tresult {
        if cid.is_null() || iid.is_null() || obj.is_null() {
            return kInvalidArgument;
        }
        let cid = *(cid as *const TUID);
        let plugin = if cid == SYNTH_CID {
            TestPlugin::new(true)
        } else if cid == GAIN_CID {
            TestPlugin::new(false)
        } else {
            *obj = std::ptr::null_mut();
            return kInvalidArgument;
        };
        let Some(unknown) = ComWrapper::new(plugin).to_com_ptr::<FUnknown>() else {
            return kInternalError;
        };
        let ptr = unknown.as_ptr();
        ((*(*ptr).vtbl).queryInterface)(ptr, iid as *const TUID, obj)
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: int32, info: *mut PClassInfo2) -> tresult {
        let Some((cid, name, sub)) = CLASSES.get(index as usize) else {
            return kInvalidArgument;
        };
        if info.is_null() {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.cid = *cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
        copy_cstring(name, &mut info.name);
        info.classFlags = 0;
        copy_cstring(sub, &mut info.subCategories);
        copy_cstring(VENDOR, &mut info.vendor);
        copy_cstring("1.2.3", &mut info.version);
        copy_cstring("VST 3.7", &mut info.sdkVersion);
        kResultOk
    }
}

impl IPluginFactory3Trait for Factory {
    unsafe fn getClassInfoUnicode(&self, index: int32, info: *mut PClassInfoW) -> tresult {
        let Some((cid, name, sub)) = CLASSES.get(index as usize) else {
            return kInvalidArgument;
        };
        if info.is_null() {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.cid = *cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
        copy_wstring(name, &mut info.name);
        info.classFlags = 0;
        copy_cstring(sub, &mut info.subCategories);
        copy_wstring(VENDOR, &mut info.vendor);
        copy_wstring("1.2.3", &mut info.version);
        copy_wstring("VST 3.7", &mut info.sdkVersion);
        kResultOk
    }
    unsafe fn setHostContext(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
}

/// Punto d'ingresso standard VST3.
#[no_mangle]
pub extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    match ComWrapper::new(Factory).to_com_ptr::<IPluginFactory>() {
        Some(p) => p.into_raw(),
        None => std::ptr::null_mut(),
    }
}

/// Ingresso del modulo (Linux).
#[no_mangle]
pub extern "system" fn ModuleEntry(_handle: *mut c_void) -> bool {
    true
}

/// Uscita del modulo (Linux).
#[no_mangle]
pub extern "system" fn ModuleExit() -> bool {
    true
}

/// Ingresso del modulo (Windows).
#[no_mangle]
pub extern "system" fn InitDll() -> bool {
    true
}

/// Uscita del modulo (Windows).
#[no_mangle]
pub extern "system" fn ExitDll() -> bool {
    true
}

/// Ingresso del bundle (macOS).
#[no_mangle]
pub extern "system" fn bundleEntry(_bundle: *mut c_void) -> bool {
    true
}

/// Uscita del bundle (macOS).
#[no_mangle]
pub extern "system" fn bundleExit() -> bool {
    true
}
