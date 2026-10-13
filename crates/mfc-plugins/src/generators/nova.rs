//! Nova: synth sottrattivo polifonico per fare musica "da subito".
//!
//! - Due oscillatori (sine, triangle, saw, square/PW) con ottava, semitoni, fine e
//!   unison fino a 8 voci con detune e spread stereo; sub-oscillatore e rumore.
//! - Filtro multimodo (LP/HP/BP/notch) 12 o 24 dB/ottava con drive, inviluppo dedicato,
//!   key tracking e velocity.
//! - Due LFO globali con destinazione (pitch, cutoff, PW, volume, pan).
//! - Modalità Poly, Mono e Legato con glide.
//! - Preset di fabbrica in [`PRESETS`].
//!
//! Il controllo (frequenze, cutoff, LFO, glide) è aggiornato ogni [`CTRL`] campioni;
//! l'audio e gli inviluppi a ogni campione. Nessuna allocazione nel thread audio.

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR, OFF_ON};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole, Preset};
use crate::ids;
use mfc_dsp::{midi_to_hz, pan_gains, Adsr, Lfo, LfoShape, Oscillator, Rng, Svf, SvfMode, Waveform};

const WAVES: &[&str] = &["Sine", "Triangle", "Saw", "Square"];
const OCTAVES: &[&str] = &["-3", "-2", "-1", "0", "+1", "+2", "+3"];
const UNISON: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8"];
const SUB_WAVES: &[&str] = &["Sine", "Square"];
const FILTER_MODES: &[&str] = &["Low pass", "High pass", "Band pass", "Notch"];
const SLOPES: &[&str] = &["12 dB", "24 dB"];
const LFO_SHAPES: &[&str] = &["Sine", "Triangle", "Saw", "Square", "S&H"];
const LFO_DEST: &[&str] = &["Off", "Pitch", "Cutoff", "PW", "Volume", "Pan"];
const VOICE_MODES: &[&str] = &["Poly", "Mono", "Legato"];

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::NOVA,
    name: "Nova",
    role: PluginRole::Generator,
    description: "Synth sottrattivo: 2 oscillatori con unison, sub e rumore, filtro 12/24 dB con inviluppo, \
                  2 LFO, mono/legato con glide e preset di fabbrica.",
    params: &[
        ParamDesc::choice("osc1_wave", "Forma", WAVES, 2),
        ParamDesc::choice("osc1_oct", "Ottava", OCTAVES, 3),
        ParamDesc::new("osc1_semi", "Semitoni", -12.0, 12.0, 0.0, "st"),
        ParamDesc::new("osc1_fine", "Fine", -100.0, 100.0, 0.0, "ct"),
        ParamDesc::new("osc1_pw", "PW", 0.05, 0.95, 0.5, ""),
        ParamDesc::new("osc1_level", "Livello", 0.0, 1.0, 0.8, ""),
        ParamDesc::choice("osc1_unison", "Unison", UNISON, 0),
        ParamDesc::new("osc1_detune", "Detune", 0.0, 1.0, 0.25, ""),
        ParamDesc::new("osc1_spread", "Spread", 0.0, 1.0, 0.7, ""),
        ParamDesc::choice("osc2_wave", "Forma", WAVES, 2),
        ParamDesc::choice("osc2_oct", "Ottava", OCTAVES, 3),
        ParamDesc::new("osc2_semi", "Semitoni", -12.0, 12.0, 0.0, "st"),
        ParamDesc::new("osc2_fine", "Fine", -100.0, 100.0, 7.0, "ct"),
        ParamDesc::new("osc2_pw", "PW", 0.05, 0.95, 0.5, ""),
        ParamDesc::new("osc2_level", "Livello", 0.0, 1.0, 0.5, ""),
        ParamDesc::choice("osc2_unison", "Unison", UNISON, 0),
        ParamDesc::new("osc2_detune", "Detune", 0.0, 1.0, 0.25, ""),
        ParamDesc::new("osc2_spread", "Spread", 0.0, 1.0, 0.7, ""),
        ParamDesc::new("sub_level", "Sub", 0.0, 1.0, 0.0, ""),
        ParamDesc::choice("sub_wave", "Forma sub", SUB_WAVES, 0),
        ParamDesc::new("noise_level", "Rumore", 0.0, 1.0, 0.0, ""),
        ParamDesc::choice("flt_mode", "Tipo", FILTER_MODES, 0),
        ParamDesc::choice("flt_slope", "Pendenza", SLOPES, 0),
        ParamDesc::new("flt_cutoff", "Cutoff", 20.0, 20_000.0, 4_000.0, "Hz"),
        ParamDesc::new("flt_reso", "Risonanza", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("flt_drive", "Drive", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("flt_env", "Env amount", -1.0, 1.0, 0.35, ""),
        ParamDesc::new("flt_key", "Key track", 0.0, 1.0, 0.3, ""),
        ParamDesc::new("flt_vel", "Velocity", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("fenv_attack", "Attack", 0.0, 10.0, 0.005, "s"),
        ParamDesc::new("fenv_decay", "Decay", 0.0, 10.0, 0.35, "s"),
        ParamDesc::new("fenv_sustain", "Sustain", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("fenv_release", "Release", 0.0, 10.0, 0.3, "s"),
        ParamDesc::new("aenv_attack", "Attack", 0.0, 10.0, 0.005, "s"),
        ParamDesc::new("aenv_decay", "Decay", 0.0, 10.0, 0.3, "s"),
        ParamDesc::new("aenv_sustain", "Sustain", 0.0, 1.0, 0.8, ""),
        ParamDesc::new("aenv_release", "Release", 0.0, 10.0, 0.25, "s"),
        ParamDesc::choice("lfo1_shape", "Forma", LFO_SHAPES, 0),
        ParamDesc::new("lfo1_rate", "Rate", 0.01, 30.0, 5.0, "Hz"),
        ParamDesc::choice("lfo1_dest", "Destinazione", LFO_DEST, 1),
        ParamDesc::new("lfo1_amount", "Quantità", 0.0, 1.0, 0.0, ""),
        ParamDesc::choice("lfo1_retrig", "Retrigger", OFF_ON, 0),
        ParamDesc::choice("lfo2_shape", "Forma", LFO_SHAPES, 1),
        ParamDesc::new("lfo2_rate", "Rate", 0.01, 30.0, 0.5, "Hz"),
        ParamDesc::choice("lfo2_dest", "Destinazione", LFO_DEST, 2),
        ParamDesc::new("lfo2_amount", "Quantità", 0.0, 1.0, 0.0, ""),
        ParamDesc::choice("lfo2_retrig", "Retrigger", OFF_ON, 0),
        ParamDesc::choice("voice_mode", "Modalità", VOICE_MODES, 0),
        ParamDesc::new("voice_glide", "Glide", 0.0, 2.0, 0.0, "s"),
        ParamDesc::new("voice_vel", "Sens. velocity", 0.0, 1.0, 0.6, ""),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.7, ""),
    ],
};

/// Preset di fabbrica: valori che differiscono dai default del descrittore.
pub static PRESETS: &[Preset] = &[
    Preset {
        name: "Init",
        category: "Base",
        values: &[],
    },
    Preset {
        name: "Supersaw Lead",
        category: "Lead",
        values: &[
            ("osc1_unison", 6.0),
            ("osc1_detune", 0.35),
            ("osc1_spread", 0.9),
            ("osc2_oct", 4.0),
            ("osc2_level", 0.35),
            ("osc2_unison", 4.0),
            ("osc2_detune", 0.3),
            ("flt_cutoff", 6_000.0),
            ("flt_reso", 0.15),
            ("flt_env", 0.25),
            ("aenv_attack", 0.01),
            ("aenv_sustain", 0.9),
            ("aenv_release", 0.35),
            ("lfo1_rate", 5.5),
            ("lfo1_amount", 0.12),
            ("amp", 0.9),
        ],
    },
    Preset {
        name: "Soft Lead",
        category: "Lead",
        values: &[
            ("osc1_wave", 1.0),
            ("osc2_level", 0.25),
            ("osc2_fine", 5.0),
            ("flt_cutoff", 2_500.0),
            ("flt_env", 0.2),
            ("lfo1_rate", 5.5),
            ("lfo1_amount", 0.08),
            ("voice_mode", 2.0),
            ("voice_glide", 0.06),
            ("amp", 1.2),
        ],
    },
    Preset {
        name: "Chiptune Lead",
        category: "Lead",
        values: &[
            ("osc1_wave", 3.0),
            ("osc1_pw", 0.25),
            ("osc2_level", 0.0),
            ("flt_cutoff", 20_000.0),
            ("flt_env", 0.0),
            ("flt_key", 0.0),
            ("aenv_attack", 0.0),
            ("aenv_sustain", 1.0),
            ("aenv_release", 0.08),
            ("lfo1_rate", 6.5),
            ("lfo1_amount", 0.06),
            ("voice_mode", 1.0),
            ("voice_glide", 0.03),
            ("amp", 1.3),
        ],
    },
    Preset {
        name: "Hoover",
        category: "Lead",
        values: &[
            ("osc1_unison", 7.0),
            ("osc1_detune", 0.7),
            ("osc1_spread", 1.0),
            ("osc2_oct", 2.0),
            ("osc2_level", 0.6),
            ("osc2_unison", 3.0),
            ("osc2_detune", 0.5),
            ("flt_cutoff", 4_000.0),
            ("flt_env", 0.1),
            ("lfo1_rate", 6.0),
            ("lfo1_amount", 0.1),
            ("voice_mode", 2.0),
            ("voice_glide", 0.15),
            ("amp", 0.5),
        ],
    },
    Preset {
        name: "Acid Bass",
        category: "Bass",
        values: &[
            ("osc1_oct", 2.0),
            ("osc2_level", 0.0),
            ("flt_slope", 1.0),
            ("flt_cutoff", 300.0),
            ("flt_reso", 0.75),
            ("flt_drive", 0.4),
            ("flt_env", 0.7),
            ("flt_key", 0.5),
            ("fenv_decay", 0.25),
            ("fenv_sustain", 0.0),
            ("aenv_attack", 0.0),
            ("aenv_sustain", 1.0),
            ("aenv_release", 0.05),
            ("voice_mode", 1.0),
            ("voice_glide", 0.08),
        ],
    },
    Preset {
        name: "Deep Sub",
        category: "Bass",
        values: &[
            ("osc1_wave", 0.0),
            ("osc1_oct", 2.0),
            ("osc2_wave", 1.0),
            ("osc2_oct", 2.0),
            ("osc2_level", 0.3),
            ("sub_level", 0.6),
            ("flt_cutoff", 900.0),
            ("flt_env", 0.1),
            ("aenv_attack", 0.003),
            ("aenv_sustain", 1.0),
            ("aenv_release", 0.1),
            ("voice_mode", 1.0),
        ],
    },
    Preset {
        name: "Reese Bass",
        category: "Bass",
        values: &[
            ("osc1_oct", 2.0),
            ("osc1_unison", 1.0),
            ("osc1_detune", 0.3),
            ("osc2_oct", 2.0),
            ("osc2_fine", 14.0),
            ("osc2_level", 0.8),
            ("flt_slope", 1.0),
            ("flt_cutoff", 700.0),
            ("flt_env", 0.1),
            ("lfo2_rate", 0.3),
            ("lfo2_amount", 0.25),
            ("aenv_sustain", 1.0),
            ("aenv_release", 0.15),
            ("voice_mode", 1.0),
            ("amp", 0.6),
        ],
    },
    Preset {
        name: "Wobble Bass",
        category: "Bass",
        values: &[
            ("osc1_oct", 2.0),
            ("osc2_wave", 3.0),
            ("osc2_oct", 2.0),
            ("osc2_level", 0.6),
            ("flt_slope", 1.0),
            ("flt_cutoff", 250.0),
            ("flt_reso", 0.5),
            ("flt_drive", 0.5),
            ("flt_env", 0.0),
            ("lfo1_rate", 3.0),
            ("lfo1_dest", 2.0),
            ("lfo1_amount", 0.7),
            ("lfo1_retrig", 1.0),
            ("aenv_sustain", 1.0),
            ("aenv_release", 0.1),
            ("voice_mode", 1.0),
            ("amp", 0.6),
        ],
    },
    Preset {
        name: "Warm Pad",
        category: "Pad",
        values: &[
            ("osc1_unison", 4.0),
            ("osc1_detune", 0.3),
            ("osc1_spread", 1.0),
            ("osc2_wave", 3.0),
            ("osc2_pw", 0.35),
            ("osc2_unison", 2.0),
            ("osc2_fine", -8.0),
            ("flt_cutoff", 1_800.0),
            ("flt_reso", 0.1),
            ("flt_env", 0.2),
            ("fenv_attack", 1.2),
            ("fenv_decay", 2.0),
            ("fenv_sustain", 0.6),
            ("fenv_release", 2.0),
            ("aenv_attack", 0.8),
            ("aenv_decay", 1.0),
            ("aenv_sustain", 0.9),
            ("aenv_release", 2.5),
            ("lfo2_rate", 0.15),
            ("lfo2_amount", 0.2),
            ("amp", 0.5),
        ],
    },
    Preset {
        name: "Strings",
        category: "Pad",
        values: &[
            ("osc1_unison", 3.0),
            ("osc1_detune", 0.2),
            ("osc2_oct", 4.0),
            ("osc2_level", 0.3),
            ("osc2_unison", 2.0),
            ("flt_cutoff", 3_500.0),
            ("flt_env", 0.1),
            ("aenv_attack", 0.35),
            ("aenv_sustain", 0.9),
            ("aenv_release", 1.0),
            ("lfo1_rate", 5.0),
            ("lfo1_amount", 0.06),
            ("amp", 0.55),
        ],
    },
    Preset {
        name: "Sweep FX",
        category: "FX",
        values: &[
            ("osc1_level", 0.2),
            ("osc2_level", 0.0),
            ("noise_level", 1.0),
            ("flt_mode", 2.0),
            ("flt_cutoff", 800.0),
            ("flt_reso", 0.7),
            ("flt_env", 0.0),
            ("lfo1_shape", 2.0),
            ("lfo1_rate", 0.2),
            ("lfo1_dest", 2.0),
            ("lfo1_amount", 1.0),
            ("lfo1_retrig", 1.0),
            ("aenv_attack", 0.8),
            ("aenv_sustain", 1.0),
            ("aenv_release", 3.0),
            ("amp", 2.0),
        ],
    },
    Preset {
        name: "Pluck",
        category: "Pluck",
        values: &[
            ("osc2_wave", 3.0),
            ("osc2_oct", 4.0),
            ("osc2_level", 0.4),
            ("flt_cutoff", 400.0),
            ("flt_reso", 0.3),
            ("flt_env", 0.8),
            ("fenv_attack", 0.0),
            ("fenv_decay", 0.25),
            ("fenv_sustain", 0.0),
            ("fenv_release", 0.2),
            ("aenv_attack", 0.0),
            ("aenv_decay", 0.5),
            ("aenv_sustain", 0.0),
            ("aenv_release", 0.3),
            ("amp", 1.3),
        ],
    },
    Preset {
        name: "Bell Keys",
        category: "Keys",
        values: &[
            ("osc1_wave", 0.0),
            ("osc2_wave", 0.0),
            ("osc2_oct", 5.0),
            ("osc2_semi", 7.0),
            ("osc2_fine", 0.0),
            ("osc2_level", 0.45),
            ("flt_cutoff", 12_000.0),
            ("flt_env", 0.0),
            ("aenv_attack", 0.0),
            ("aenv_decay", 1.8),
            ("aenv_sustain", 0.0),
            ("aenv_release", 1.5),
        ],
    },
    Preset {
        name: "Electric Piano",
        category: "Keys",
        values: &[
            ("osc1_wave", 1.0),
            ("osc2_wave", 0.0),
            ("osc2_oct", 4.0),
            ("osc2_fine", 0.0),
            ("osc2_level", 0.35),
            ("flt_cutoff", 5_000.0),
            ("flt_env", 0.1),
            ("flt_vel", 0.6),
            ("aenv_attack", 0.002),
            ("aenv_decay", 1.2),
            ("aenv_sustain", 0.3),
            ("aenv_release", 0.4),
            ("lfo1_rate", 4.5),
            ("lfo1_dest", 4.0),
            ("lfo1_amount", 0.15),
        ],
    },
    Preset {
        name: "Brass",
        category: "Keys",
        values: &[
            ("osc2_fine", 6.0),
            ("osc2_level", 0.7),
            ("flt_slope", 1.0),
            ("flt_cutoff", 900.0),
            ("flt_env", 0.55),
            ("fenv_attack", 0.06),
            ("fenv_decay", 0.4),
            ("fenv_sustain", 0.5),
            ("aenv_attack", 0.04),
            ("aenv_sustain", 0.85),
            ("aenv_release", 0.2),
            ("amp", 0.6),
        ],
    },
];

/// Voci di polifonia.
const VOICES: usize = 16;
/// Voci di unison massime per oscillatore.
const MAX_UNISON: usize = 8;
/// Campioni tra due aggiornamenti dei controlli.
const CTRL: usize = 32;
/// Note tenute ricordate in modalità mono.
const STACK: usize = 32;
/// Detune massimo dell'unison (semitoni, ai lati).
const MAX_DETUNE_ST: f32 = 0.5;

/// Impostazioni di un oscillatore.
#[derive(Debug, Clone, Copy)]
struct OscCfg {
    wave: Waveform,
    /// Trasposizione in semitoni (ottava, semitoni, fine).
    transpose: f32,
    pw: f32,
    level: f32,
    unison: usize,
    detune: f32,
    spread: f32,
    /// Detune in semitoni per voce di unison.
    offsets: [f32; MAX_UNISON],
    /// Guadagni (L, R) per voce di unison, normalizzati.
    gains: [(f32, f32); MAX_UNISON],
}

impl OscCfg {
    fn new() -> Self {
        Self {
            wave: Waveform::Saw,
            transpose: 0.0,
            pw: 0.5,
            level: 0.8,
            unison: 1,
            detune: 0.25,
            spread: 0.7,
            offsets: [0.0; MAX_UNISON],
            gains: [(0.0, 0.0); MAX_UNISON],
        }
    }

    /// Ricalcola detune e pan delle voci di unison.
    fn update(&mut self) {
        let n = self.unison.clamp(1, MAX_UNISON);
        let norm = self.level / (n as f32).sqrt();
        for j in 0..MAX_UNISON {
            let pos = if n == 1 {
                0.0
            } else {
                j as f32 / (n - 1) as f32 * 2.0 - 1.0
            };
            self.offsets[j] = pos * self.detune * self.detune * MAX_DETUNE_ST * 2.0;
            let (l, r) = pan_gains(pos * self.spread);
            self.gains[j] = if j < n { (l * norm, r * norm) } else { (0.0, 0.0) };
        }
    }
}

/// Un LFO con la sua destinazione.
#[derive(Debug, Clone, Copy)]
struct LfoCfg {
    lfo: Lfo,
    dest: u8,
    amount: f32,
    retrig: bool,
}

/// Destinazioni (indici di [`LFO_DEST`]).
const DEST_PITCH: u8 = 1;
const DEST_CUTOFF: u8 = 2;
const DEST_PW: u8 = 3;
const DEST_VOLUME: u8 = 4;
const DEST_PAN: u8 = 5;

/// Modulazioni globali di un blocco di controllo.
#[derive(Debug, Clone, Copy, Default)]
struct Mods {
    pitch_st: f32,
    cutoff_oct: f32,
    pw: f32,
    volume: f32,
    pan: f32,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    key: u8,
    vel: f32,
    /// Altezza corrente e di arrivo (semitoni MIDI, glide).
    pitch: f32,
    target: f32,
    osc: [[Oscillator; MAX_UNISON]; 2],
    sub: Oscillator,
    rng: Rng,
    /// Filtri [canale][stadio].
    svf: [[Svf; 2]; 2],
    fenv: Adsr,
    aenv: Adsr,
    fenv_level: f32,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth Nova.
pub struct Nova {
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    rng: Rng,
    pitch_offset: f32,
    osc: [OscCfg; 2],
    /// Indice d'ottava, semitoni e cent di ciascun oscillatore.
    tuning: [[f32; 3]; 2],
    sub_level: f32,
    sub_square: bool,
    noise_level: f32,
    flt_mode: SvfMode,
    flt_24: bool,
    cutoff: f32,
    reso: f32,
    drive: f32,
    flt_env: f32,
    flt_key: f32,
    flt_vel: f32,
    fenv: [f32; 4],
    aenv: [f32; 4],
    lfos: [LfoCfg; 2],
    mode: u8,
    glide: f32,
    vel_sens: f32,
    amp: f32,
    /// Ultima nota suonata (partenza del glide in polifonia).
    last_pitch: Option<f32>,
    /// Note tenute in modalità mono: (tasto, id, velocity).
    stack: [(u8, u64, f32); STACK],
    stack_len: usize,
}

impl Default for Nova {
    fn default() -> Self {
        Self::new()
    }
}

fn waveform(i: f32) -> Waveform {
    match i as usize {
        0 => Waveform::Sine,
        1 => Waveform::Triangle,
        3 => Waveform::Square,
        _ => Waveform::Saw,
    }
}

fn lfo_shape(i: f32) -> LfoShape {
    match i as usize {
        1 => LfoShape::Triangle,
        2 => LfoShape::Saw,
        3 => LfoShape::Square,
        4 => LfoShape::SampleHold,
        _ => LfoShape::Sine,
    }
}

impl Nova {
    pub fn new() -> Self {
        let osc = Oscillator::new(DEFAULT_SR);
        let voice = Voice {
            tag: VoiceTag::default(),
            key: 60,
            vel: 1.0,
            pitch: 60.0,
            target: 60.0,
            osc: [[osc; MAX_UNISON]; 2],
            sub: osc,
            rng: Rng::new(1),
            svf: [[Svf::new(DEFAULT_SR); 2]; 2],
            fenv: Adsr::new(DEFAULT_SR),
            aenv: Adsr::new(DEFAULT_SR),
            fenv_level: 0.0,
        };
        let lfo = LfoCfg {
            lfo: Lfo::new(DEFAULT_SR),
            dest: 0,
            amount: 0.0,
            retrig: false,
        };
        let mut s = Self {
            sr: DEFAULT_SR,
            voices: vec![voice; VOICES],
            order: 0,
            rng: Rng::new(0x4E0A),
            pitch_offset: 0.0,
            osc: [OscCfg::new(); 2],
            tuning: [[3.0, 0.0, 0.0]; 2],
            sub_level: 0.0,
            sub_square: false,
            noise_level: 0.0,
            flt_mode: SvfMode::Lowpass,
            flt_24: false,
            cutoff: 4_000.0,
            reso: 0.2,
            drive: 0.0,
            flt_env: 0.35,
            flt_key: 0.3,
            flt_vel: 0.2,
            fenv: [0.005, 0.35, 0.2, 0.3],
            aenv: [0.005, 0.3, 0.8, 0.25],
            lfos: [lfo; 2],
            mode: 0,
            glide: 0.0,
            vel_sens: 0.6,
            amp: 0.7,
            last_pitch: None,
            stack: [(0, 0, 0.0); STACK],
            stack_len: 0,
        };
        for (i, v) in s.voices.iter_mut().enumerate() {
            v.rng = Rng::new(0x9E37 + i as u32 * 7919);
        }
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.configure_envs();
        s
    }

    fn configure_envs(&mut self) {
        let [fa, fd, fs, fr] = self.fenv;
        let [aa, ad, as_, ar] = self.aenv;
        let mode = self.flt_mode;
        let reso = self.reso;
        for v in &mut self.voices {
            v.fenv.set_adsr(fa, fd, fs, fr);
            v.aenv.set_adsr(aa, ad, as_, ar);
            for ch in &mut v.svf {
                ch[0].set_mode(mode);
                ch[0].set_resonance(reso);
                // Secondo stadio (24 dB) senza risonanza, per non raddoppiare il picco.
                ch[1].set_mode(mode);
                ch[1].set_resonance(0.0);
            }
        }
    }

    fn mono(&self) -> bool {
        self.mode != 0
    }

    /// Altezza di base (semitoni MIDI) per un tasto.
    fn key_pitch(&self, key: u8) -> f32 {
        key as f32
    }

    /// Avvia (o riprende in legato) la voce mono sul tasto indicato.
    fn mono_play(&mut self, key: u8, vel: f32, id: u64, retrigger: bool) {
        let pitch = self.key_pitch(key);
        let glide = self.glide > 0.0;
        let v = &mut self.voices[0];
        let was_active = v.tag.active && !v.tag.released;
        v.key = key;
        v.target = pitch;
        if !glide || !v.tag.active {
            v.pitch = pitch;
        }
        v.tag.id = id;
        if retrigger || !was_active {
            self.order += 1;
            v.tag = VoiceTag {
                active: true,
                released: false,
                id,
                order: self.order,
            };
            v.vel = vel;
            if !was_active {
                for ch in &mut v.svf {
                    for f in ch {
                        f.reset();
                    }
                }
            }
            v.fenv.note_on();
            v.aenv.note_on();
        }
    }

    fn stack_remove(&mut self, id: u64) -> bool {
        if let Some(pos) = self.stack[..self.stack_len].iter().position(|e| e.1 == id) {
            self.stack.copy_within(pos + 1..self.stack_len, pos);
            self.stack_len -= 1;
            true
        } else {
            false
        }
    }

    fn lfo_retrigger(&mut self) {
        for l in &mut self.lfos {
            if l.retrig {
                l.lfo.reset(0.0);
            }
        }
    }

    /// Valori degli LFO per il prossimo blocco di controllo (avanzati di `n` campioni).
    fn next_mods(&mut self, n: usize) -> Mods {
        let mut m = Mods {
            volume: 1.0,
            ..Default::default()
        };
        for l in &mut self.lfos {
            let x = l.lfo.next_sample();
            for _ in 1..n {
                l.lfo.next_sample();
            }
            if l.amount <= 0.0 {
                continue;
            }
            let a = l.amount;
            match l.dest {
                DEST_PITCH => m.pitch_st += x * a * a * 12.0,
                DEST_CUTOFF => m.cutoff_oct += x * a * 4.0,
                DEST_PW => m.pw += x * a * 0.45,
                DEST_VOLUME => m.volume *= 1.0 - a * (0.5 - 0.5 * x),
                DEST_PAN => m.pan += x * a,
                _ => {}
            }
        }
        m
    }
}

impl Generator for Nova {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        for l in &mut self.lfos {
            l.lfo.set_sample_rate(sample_rate);
        }
        for v in &mut self.voices {
            v.fenv.set_sample_rate(sample_rate);
            v.aenv.set_sample_rate(sample_rate);
            for ch in &mut v.svf {
                for f in ch {
                    f.set_sample_rate(sample_rate);
                }
            }
            for bank in &mut v.osc {
                for o in bank {
                    o.set_sample_rate(sample_rate);
                }
            }
            v.sub.set_sample_rate(sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        if let Some((pre, field)) = name.split_once('_') {
            let osc_idx = match pre {
                "osc1" => Some(0),
                "osc2" => Some(1),
                _ => None,
            };
            if let Some(i) = osc_idx {
                let o = &mut self.osc[i];
                match field {
                    "wave" => o.wave = waveform(v),
                    "oct" | "semi" | "fine" => {
                        let slot = match field {
                            "oct" => 0,
                            "semi" => 1,
                            _ => 2,
                        };
                        self.tuning[i][slot] = v;
                        let [oct, semi, fine] = self.tuning[i];
                        self.osc[i].transpose = (oct - 3.0) * 12.0 + semi + fine / 100.0;
                        return;
                    }
                    "pw" => o.pw = v,
                    "level" => o.level = v,
                    "unison" => o.unison = v as usize + 1,
                    "detune" => o.detune = v,
                    "spread" => o.spread = v,
                    _ => return,
                }
                self.osc[i].update();
                return;
            }
            let lfo_idx = match pre {
                "lfo1" => Some(0),
                "lfo2" => Some(1),
                _ => None,
            };
            if let Some(i) = lfo_idx {
                let l = &mut self.lfos[i];
                match field {
                    "shape" => l.lfo.set_shape(lfo_shape(v)),
                    "rate" => l.lfo.set_rate(v),
                    "dest" => l.dest = v as u8,
                    "amount" => l.amount = v,
                    "retrig" => l.retrig = v >= 0.5,
                    _ => {}
                }
                return;
            }
        }
        match name {
            "sub_level" => self.sub_level = v,
            "sub_wave" => self.sub_square = v >= 0.5,
            "noise_level" => self.noise_level = v,
            "flt_mode" => {
                self.flt_mode = match v as usize {
                    1 => SvfMode::Highpass,
                    2 => SvfMode::Bandpass,
                    3 => SvfMode::Notch,
                    _ => SvfMode::Lowpass,
                };
                self.configure_envs();
            }
            "flt_slope" => self.flt_24 = v >= 0.5,
            "flt_cutoff" => self.cutoff = v,
            "flt_reso" => {
                self.reso = v;
                self.configure_envs();
            }
            "flt_drive" => self.drive = v,
            "flt_env" => self.flt_env = v,
            "flt_key" => self.flt_key = v,
            "flt_vel" => self.flt_vel = v,
            "fenv_attack" => self.fenv[0] = v,
            "fenv_decay" => self.fenv[1] = v,
            "fenv_sustain" => self.fenv[2] = v,
            "fenv_release" => self.fenv[3] = v,
            "aenv_attack" => self.aenv[0] = v,
            "aenv_decay" => self.aenv[1] = v,
            "aenv_sustain" => self.aenv[2] = v,
            "aenv_release" => self.aenv[3] = v,
            "voice_mode" => {
                let mono = v as u8 != 0;
                if mono != self.mono() {
                    self.all_notes_off();
                    self.stack_len = 0;
                }
                self.mode = v as u8;
            }
            "voice_glide" => self.glide = v,
            "voice_vel" => self.vel_sens = v,
            "amp" => self.amp = v,
            _ => {}
        }
        if name.starts_with("fenv_") || name.starts_with("aenv_") {
            self.configure_envs();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = note.key.min(127);
        let vel = note.velocity.clamp(0.0, 1.0);
        self.lfo_retrigger();
        if self.mono() {
            if self.stack_len == STACK {
                self.stack.copy_within(1..STACK, 0);
                self.stack_len -= 1;
            }
            self.stack[self.stack_len] = (key, note.voice_id, vel);
            self.stack_len += 1;
            let legato = self.mode == 2;
            self.mono_play(key, vel, note.voice_id, !legato);
            self.last_pitch = Some(self.key_pitch(key));
            return;
        }
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let pitch = self.key_pitch(key)
            + if note.fine_pitch_cents.is_finite() {
                note.fine_pitch_cents / 100.0
            } else {
                0.0
            };
        let start = match (self.glide > 0.0, self.last_pitch) {
            (true, Some(p)) => p,
            _ => pitch,
        };
        self.last_pitch = Some(pitch);
        let unison = [self.osc[0].unison, self.osc[1].unison];
        let mut rng = self.rng;
        let v = &mut self.voices[idx];
        v.tag = VoiceTag {
            active: true,
            released: false,
            id: note.voice_id,
            order: self.order,
        };
        v.key = key;
        v.vel = vel;
        v.pitch = start;
        v.target = pitch;
        // Fase fissa con una sola voce (attacco ripetibile), casuale con l'unison.
        for (bank, n) in v.osc.iter_mut().zip(unison) {
            for o in bank.iter_mut() {
                o.reset(if n > 1 { rng.next_f32() } else { 0.0 });
            }
        }
        v.sub.reset(0.0);
        for ch in &mut v.svf {
            for f in ch {
                f.reset();
            }
        }
        v.fenv.note_on();
        v.aenv.note_on();
        self.rng = rng;
    }

    fn note_off(&mut self, voice_id: u64) {
        if self.mono() {
            if !self.stack_remove(voice_id) {
                return;
            }
            let current = self.voices[0].tag.id == voice_id;
            if !current {
                return;
            }
            if self.stack_len > 0 {
                // Torna all'ultima nota ancora tenuta, senza riattaccare.
                let (key, id, vel) = self.stack[self.stack_len - 1];
                self.mono_play(key, vel, id, false);
            } else {
                let v = &mut self.voices[0];
                if v.tag.active && !v.tag.released {
                    v.tag.released = true;
                    v.fenv.note_off();
                    v.aenv.note_off();
                }
            }
            return;
        }
        for v in &mut self.voices {
            if v.tag.active && !v.tag.released && v.tag.id == voice_id {
                v.tag.released = true;
                v.fenv.note_off();
                v.aenv.note_off();
            }
        }
    }

    fn all_notes_off(&mut self) {
        self.stack_len = 0;
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
                v.fenv.note_off();
                v.aenv.note_off();
            }
        }
    }

    fn set_pitch_offset(&mut self, cents: f32) {
        if cents.is_finite() {
            self.pitch_offset = cents.clamp(-9600.0, 9600.0);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = frames(left, right);
        let nyq = 0.45 * self.sr;
        let filter_bypass = self.flt_mode == SvfMode::Lowpass
            && self.cutoff >= 19_999.0
            && self.flt_env <= 0.0
            && self.lfos.iter().all(|l| l.dest != DEST_CUTOFF || l.amount <= 0.0);
        let drive_gain = 1.0 + self.drive * 9.0;
        let drive_norm = 1.0 / drive_gain.tanh().max(1e-3);
        let glide_coef = if self.glide > 0.0 {
            1.0 - (-(CTRL as f32) / (self.glide * self.sr)).exp()
        } else {
            1.0
        };
        let bend = self.pitch_offset / 100.0;
        let mut start = 0;
        while start < n {
            let len = CTRL.min(n - start);
            let m = self.next_mods(len);
            let (pan_l, pan_r) = if m.pan != 0.0 {
                pan_gains(m.pan.clamp(-1.0, 1.0))
            } else {
                (1.0, 1.0)
            };
            let pan_norm = if m.pan != 0.0 { std::f32::consts::SQRT_2 } else { 1.0 };
            for v in self.voices.iter_mut().filter(|v| v.tag.active) {
                // Glide e frequenze.
                v.pitch += (v.target - v.pitch) * glide_coef;
                let base = v.pitch + bend + m.pitch_st;
                for (bank, cfg) in v.osc.iter_mut().zip(&self.osc) {
                    if cfg.level <= 0.0 {
                        continue;
                    }
                    let pw = (cfg.pw + m.pw).clamp(0.05, 0.95);
                    for (o, off) in bank.iter_mut().zip(cfg.offsets).take(cfg.unison) {
                        o.set_waveform(cfg.wave);
                        o.set_pulse_width(pw);
                        o.set_frequency(midi_to_hz(base + cfg.transpose + off));
                    }
                }
                if self.sub_level > 0.0 {
                    v.sub.set_waveform(if self.sub_square {
                        Waveform::Square
                    } else {
                        Waveform::Sine
                    });
                    v.sub.set_frequency(midi_to_hz(base - 12.0));
                }
                // Cutoff: base · key tracking · inviluppo · velocity · LFO.
                if !filter_bypass {
                    let oct = self.flt_key * (v.pitch - 60.0) / 12.0
                        + self.flt_env * 5.0 * v.fenv_level
                        + self.flt_vel * 2.0 * (v.vel - 1.0)
                        + m.cutoff_oct;
                    let fc = (self.cutoff * oct.exp2()).clamp(20.0, nyq);
                    for ch in &mut v.svf {
                        ch[0].set_cutoff(fc);
                        if self.flt_24 {
                            ch[1].set_cutoff(fc);
                        }
                    }
                }
                let gain = self.amp * m.volume * (1.0 - self.vel_sens * (1.0 - v.vel)) * 0.35;
                let (gl, gr) = (gain * pan_l * pan_norm, gain * pan_r * pan_norm);
                for i in start..start + len {
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    for (bank, cfg) in v.osc.iter_mut().zip(&self.osc) {
                        if cfg.level <= 0.0 {
                            continue;
                        }
                        for (o, (ol, or)) in bank.iter_mut().zip(cfg.gains).take(cfg.unison) {
                            let x = o.next_sample();
                            l += x * ol;
                            r += x * or;
                        }
                    }
                    let mut mono = 0.0;
                    if self.sub_level > 0.0 {
                        mono += v.sub.next_sample() * self.sub_level;
                    }
                    if self.noise_level > 0.0 {
                        mono += v.rng.next_bipolar() * self.noise_level * 0.5;
                    }
                    l += mono * std::f32::consts::FRAC_1_SQRT_2;
                    r += mono * std::f32::consts::FRAC_1_SQRT_2;
                    if self.drive > 0.0 {
                        l = (l * drive_gain).tanh() * drive_norm;
                        r = (r * drive_gain).tanh() * drive_norm;
                    }
                    v.fenv_level = v.fenv.next_sample();
                    if !filter_bypass {
                        l = v.svf[0][0].process(l);
                        r = v.svf[1][0].process(r);
                        if self.flt_24 {
                            l = v.svf[0][1].process(l);
                            r = v.svf[1][1].process(r);
                        }
                    }
                    let e = v.aenv.next_sample();
                    left[i] += l * e * gl;
                    right[i] += r * e * gr;
                    if v.aenv.is_idle() {
                        v.tag.active = false;
                        break;
                    }
                }
            }
            start += len;
        }
    }

    fn active_voices(&self) -> usize {
        count_active(&self.voices)
    }
}
