//! Plugin CLAP di prova per i test d'integrazione di `mfc-host`.
//!
//! Espone due plugin:
//! - "MFC Test Synth": sinusoide monofonica (ultima nota), uscita = sin · velocity · gain;
//! - "MFC Test Gain": effetto stereo, uscita = ingresso · gain.
//!
//! Entrambi hanno il parametro id 0 "Gain" (0..1) e salvano come stato i byte del gain.
//! Nel synth la nota 0 simula una modifica del gain dalla GUI del plugin (vedi [`GUI_EDIT_KEY`]).

use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts, PluginAudioPortsImpl,
};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts, PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams, PluginMainThreadParams,
    PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::entry::prelude::*;
use clack_plugin::events::event_types::ParamValueEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::{Match, Pckn};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use clack_plugin::utils::Cookie;

/// Id del plugin strumento.
pub const SYNTH_ID: &str = "com.mixfonocraft.test.synth";
/// Id del plugin effetto.
pub const GAIN_ID: &str = "com.mixfonocraft.test.gain";
/// Solo test: nel synth una nota con questo tasto non suona ma imposta il gain alla sua
/// velocity e lo comunica all'host come evento di parametro in uscita, come farebbe la GUI.
pub const GUI_EDIT_KEY: u16 = 0;

// ---------------------------------------------------------------------------
// Stato condiviso

/// Gain condiviso tra thread principale e audio.
pub struct Shared {
    gain: AtomicU64,
}

impl Shared {
    fn new(default: f64) -> Self {
        Self {
            gain: AtomicU64::new(default.to_bits()),
        }
    }
    fn gain(&self) -> f64 {
        f64::from_bits(self.gain.load(Ordering::Relaxed))
    }
    fn set_gain(&self, v: f64) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        self.gain.store(v.to_bits(), Ordering::Relaxed);
    }
    /// Applica gli eventi di parametro in ingresso.
    fn apply_params(&self, input: &InputEvents) {
        for ev in input {
            if let Some(p) = ev.as_event::<ParamValueEvent>() {
                if p.param_id().map(|id| id.get()) == Some(0) {
                    self.set_gain(p.value());
                }
            }
        }
    }
}

impl PluginShared<'_> for Shared {}

/// Thread principale: solo un riferimento allo stato condiviso.
pub struct MainThread<'a> {
    shared: &'a Shared,
    default: f64,
}

impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}

impl PluginMainThreadParams for MainThread<'_> {
    fn count(&self) -> u32 {
        1
    }
    fn get_info(&self, param_index: u32, info: &mut ParamInfoWriter) {
        if param_index != 0 {
            return;
        }
        info.set(&ParamInfo {
            id: ClapId::new(0),
            flags: ParamInfoFlags::IS_AUTOMATABLE,
            cookie: Cookie::empty(),
            name: b"Gain",
            module: b"",
            min_value: 0.0,
            max_value: 1.0,
            default_value: self.default,
        });
    }
    fn get_value(&self, param_id: ClapId) -> Option<f64> {
        (param_id.get() == 0).then(|| self.shared.gain())
    }
    fn value_to_text(&self, param_id: ClapId, value: f64, writer: &mut ParamDisplayWriter) -> std::fmt::Result {
        if param_id.get() != 0 {
            return Err(std::fmt::Error);
        }
        write!(writer, "{:.0} %", value * 100.0)
    }
    fn text_to_value(&self, param_id: ClapId, text: &CStr) -> Option<f64> {
        if param_id.get() != 0 {
            return None;
        }
        let s = text.to_str().ok()?.trim().trim_end_matches('%').trim();
        s.parse::<f64>().ok().map(|v| v / 100.0)
    }
    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply_params(input);
    }
}

impl PluginStateImpl for MainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        output.write_all(&self.shared.gain().to_le_bytes())?;
        Ok(())
    }
    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut b = [0u8; 8];
        input.read_exact(&mut b)?;
        self.shared.set_gain(f64::from_le_bytes(b));
        Ok(())
    }
}

fn stereo_port(writer: &mut AudioPortInfoWriter, name: &[u8], in_place: bool) {
    writer.set(&AudioPortInfo {
        id: ClapId::new(0),
        name,
        channel_count: 2,
        flags: AudioPortFlags::IS_MAIN,
        port_type: Some(AudioPortType::STEREO),
        in_place_pair: in_place.then(|| ClapId::new(0)),
    });
}

// ---------------------------------------------------------------------------
// Synth

/// Strumento: sinusoide sull'ultima nota.
pub struct TestSynth;

/// Elaborazione audio dello strumento.
pub struct SynthAudio<'a> {
    shared: &'a Shared,
    sample_rate: f64,
    phase: f64,
    /// (tasto, velocity) della nota attiva.
    note: Option<(u16, f64)>,
}

impl Plugin for TestSynth {
    type AudioProcessor<'a> = SynthAudio<'a>;
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginParams>()
            .register::<PluginState>()
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>();
    }
}

impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        // Lo strumento (default 0.5) non ha ingressi audio; l'effetto sì.
        if is_input && self.default < 1.0 {
            0
        } else {
            1
        }
    }
    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 && PluginAudioPortsImpl::count(self, is_input) > 0 {
            stereo_port(writer, if is_input { b"In" } else { b"Out" }, self.default >= 1.0);
        }
    }
}

impl PluginNotePortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        u32::from(is_input)
    }
    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if index == 0 && is_input {
            writer.set(&NotePortInfo {
                id: ClapId::new(0),
                name: b"Note",
                supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI,
                preferred_dialect: Some(NoteDialect::Clap),
            });
        }
    }
}

impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for SynthAudio<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &MainThread<'a>,
        shared: &'a Shared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            shared,
            sample_rate: audio_config.sample_rate,
            phase: 0.0,
            note: None,
        })
    }

    fn process(&mut self, _process: Process, mut audio: Audio, events: Events) -> Result<ProcessStatus, PluginError> {
        // Eventi applicati a inizio blocco (sufficiente per i test).
        for ev in events.input {
            match ev.as_core_event() {
                Some(CoreEventSpace::NoteOn(n)) => {
                    if let Match::Specific(k) = n.pckn().key {
                        if k == GUI_EDIT_KEY {
                            // Simula una modifica fatta dall'utente nella GUI del plugin.
                            self.shared.set_gain(n.velocity());
                            let ev = ParamValueEvent::new(0, ClapId::new(0), Pckn::match_all(), self.shared.gain());
                            let _ = events.output.try_push(ev);
                        } else {
                            self.note = Some((k, n.velocity()));
                        }
                    }
                }
                Some(CoreEventSpace::NoteOff(n)) => self.note_off(n.pckn().key),
                Some(CoreEventSpace::NoteChoke(n)) => self.note_off(n.pckn().key),
                Some(CoreEventSpace::ParamValue(p)) => {
                    if p.param_id().map(|id| id.get()) == Some(0) {
                        self.shared.set_gain(p.value());
                    }
                }
                Some(CoreEventSpace::Midi(m)) => {
                    let [st, d1, d2] = m.data();
                    match st & 0xf0 {
                        0x90 if d2 > 0 => self.note = Some((u16::from(d1), f64::from(d2) / 127.0)),
                        0x80 | 0x90 => self.note_off(Match::Specific(u16::from(d1))),
                        0xb0 if d1 == 123 || d1 == 120 => self.note = None,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let gain = self.shared.gain();
        let mut port = audio.output_port(0).ok_or(PluginError::Message("nessuna uscita"))?;
        let mut channels = port.channels()?.into_f32().ok_or(PluginError::Message("solo f32"))?;
        let n = channels.frames_count() as usize;
        let (freq, amp) = match self.note {
            Some((k, v)) => (440.0 * 2f64.powf((f64::from(k) - 69.0) / 12.0), v * gain),
            None => (0.0, 0.0),
        };
        let step = freq / self.sample_rate;
        let start_phase = self.phase;
        for ch in channels.iter_mut() {
            let mut ph = start_phase;
            for s in ch.iter_mut().take(n) {
                *s = ((ph * std::f64::consts::TAU).sin() * amp) as f32;
                ph = (ph + step).fract();
            }
            self.phase = ph;
        }
        if self.note.is_none() {
            self.phase = 0.0;
        }
        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.note = None;
        self.phase = 0.0;
    }
}

impl SynthAudio<'_> {
    fn note_off(&mut self, key: Match<u16>) {
        match (key, self.note) {
            (Match::All, _) => self.note = None,
            (Match::Specific(k), Some((cur, _))) if k == cur => self.note = None,
            _ => {}
        }
    }
}

impl PluginAudioProcessorParams for SynthAudio<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply_params(input);
    }
}

// ---------------------------------------------------------------------------
// Gain

/// Effetto: guadagno stereo.
pub struct TestGain;

/// Elaborazione audio dell'effetto.
pub struct GainAudio<'a> {
    shared: &'a Shared,
}

impl Plugin for TestGain {
    type AudioProcessor<'a> = GainAudio<'a>;
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginParams>()
            .register::<PluginState>()
            .register::<PluginAudioPorts>();
    }
}

impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for GainAudio<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &MainThread<'a>,
        shared: &'a Shared,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self { shared })
    }

    fn process(&mut self, _process: Process, mut audio: Audio, events: Events) -> Result<ProcessStatus, PluginError> {
        self.shared.apply_params(events.input);
        let gain = self.shared.gain() as f32;
        for mut pair in &mut audio {
            let Some(mut channels) = pair.channels()?.into_f32() else {
                continue;
            };
            for cp in channels.iter_mut() {
                match cp {
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(o) => o.fill(0.0),
                    ChannelPair::InputOutput(i, o) => {
                        for (o, i) in o.iter_mut().zip(i) {
                            *o = i * gain;
                        }
                    }
                    ChannelPair::InPlace(b) => {
                        for s in b {
                            *s *= gain;
                        }
                    }
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

impl PluginAudioProcessorParams for GainAudio<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply_params(input);
    }
}

// ---------------------------------------------------------------------------
// Entry e factory

/// Factory con i due plugin.
pub struct Factory {
    synth: PluginDescriptor,
    gain: PluginDescriptor,
}

impl PluginFactoryImpl for Factory {
    fn plugin_count(&self) -> u32 {
        2
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        match index {
            0 => Some(&self.synth),
            1 => Some(&self.gain),
            _ => None,
        }
    }

    fn create_plugin<'a>(&'a self, host_info: HostInfo<'a>, plugin_id: &CStr) -> Option<PluginInstance<'a>> {
        if Some(plugin_id) == self.synth.id() {
            Some(PluginInstance::new::<TestSynth>(
                host_info,
                &self.synth,
                |_| Ok(Shared::new(0.5)),
                |_, shared| Ok(MainThread { shared, default: 0.5 }),
            ))
        } else if Some(plugin_id) == self.gain.id() {
            Some(PluginInstance::new::<TestGain>(
                host_info,
                &self.gain,
                |_| Ok(Shared::new(1.0)),
                |_, shared| Ok(MainThread { shared, default: 1.0 }),
            ))
        } else {
            None
        }
    }
}

/// Entry del modulo.
pub struct TestEntry {
    factory: PluginFactoryWrapper<Factory>,
}

impl Entry for TestEntry {
    fn new(_bundle_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        let synth = PluginDescriptor::new(SYNTH_ID, "MFC Test Synth")
            .with_vendor("Mixfonocraft")
            .with_version("1.0.0")
            .with_features([c"instrument", c"synthesizer", c"stereo"]);
        let gain = PluginDescriptor::new(GAIN_ID, "MFC Test Gain")
            .with_vendor("Mixfonocraft")
            .with_version("1.0.0")
            .with_features([c"audio-effect", c"utility", c"stereo"]);
        Ok(Self {
            factory: PluginFactoryWrapper::new(Factory { synth, gain }),
        })
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.factory);
    }
}

clack_export_entry!(TestEntry);
