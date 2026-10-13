//! Fusion: synth FM (phase modulation) a 4 operatori, 16 voci.
//!
//! - Quattro operatori sinusoidali con rapporto di frequenza, fine, livello, ADSR e
//!   sensibilità alla velocity; feedback sull'operatore 4.
//! - Otto algoritmi classici ([`ALGO_NAMES`]): pile, rami paralleli, additivo.
//! - Anti-aliasing: l'indice di modulazione di ogni modulatore è ridotto quando le bande
//!   laterali (regola di Carson) supererebbero Nyquist, e gli operatori vicini a Nyquist
//!   sfumano a zero. Le portanti sono normalizzate sul loro numero.
//! - Filtro passa-basso globale, LFO per vibrato e modulazione dell'indice (con ritardo),
//!   modalità Poly, Mono e Legato con glide. Preset di fabbrica in [`PRESETS`].
//!
//! Frequenze, indici e LFO sono aggiornati ogni [`CTRL`] campioni; operatori e inviluppi
//! a ogni campione. Nessuna allocazione nel thread audio.

use super::{
    apply_defaults, count_active, frames, glide_coef, param_value, pick_voice, MonoStack, PolyVoice, VoiceTag,
    DEFAULT_SR, VOICE_MODES,
};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole, Preset};
use crate::ids;
use mfc_dsp::{cents_to_ratio, midi_to_hz, Adsr, Lfo, Svf, SvfMode};
use std::sync::OnceLock;

/// Nomi degli algoritmi (frecce: modulatore → portante).
pub const ALGO_NAMES: &[&str] = &[
    "1: 4→3→2→1",
    "2: (3+4)→2→1",
    "3: (4→3)+2→1",
    "4: 2→1 | 4→3",
    "5: (2+3+4)→1",
    "6: 4→(1,2,3)",
    "7: 4→3 | 2 | 1",
    "8: additivo",
];

/// Per ogni algoritmo: maschera dei modulatori di ciascun operatore (bit j = operatore j+1)
/// e maschera delle portanti. Un operatore è modulato solo da operatori con indice maggiore.
const ALGOS: [([u8; 4], u8); 8] = [
    ([0b0010, 0b0100, 0b1000, 0], 0b0001),
    ([0b0010, 0b1100, 0, 0], 0b0001),
    ([0b0110, 0, 0b1000, 0], 0b0001),
    ([0b0010, 0, 0b1000, 0], 0b0101),
    ([0b1110, 0, 0, 0], 0b0001),
    ([0b1000, 0b1000, 0b1000, 0], 0b0111),
    ([0, 0, 0b1000, 0], 0b0111),
    ([0, 0, 0, 0], 0b1111),
];

macro_rules! op_params {
    ($p:literal, $ratio:expr, $level:expr) => {
        [
            ParamDesc::new(concat!($p, "_ratio"), "Rapporto", 0.5, 16.0, $ratio, "×"),
            ParamDesc::new(concat!($p, "_fine"), "Fine", -100.0, 100.0, 0.0, "ct"),
            ParamDesc::new(concat!($p, "_level"), "Livello", 0.0, 1.0, $level, ""),
            ParamDesc::new(concat!($p, "_attack"), "Attack", 0.0, 10.0, 0.002, "s"),
            ParamDesc::new(concat!($p, "_decay"), "Decay", 0.0, 10.0, 0.6, "s"),
            ParamDesc::new(concat!($p, "_sustain"), "Sustain", 0.0, 1.0, 0.6, ""),
            ParamDesc::new(concat!($p, "_release"), "Release", 0.0, 10.0, 0.3, "s"),
            ParamDesc::new(concat!($p, "_vel"), "Sens. velocity", 0.0, 1.0, 0.4, ""),
        ]
    };
}

const OP1: [ParamDesc; 8] = op_params!("op1", 1.0, 1.0);
const OP2: [ParamDesc; 8] = op_params!("op2", 1.0, 0.3);
const OP3: [ParamDesc; 8] = op_params!("op3", 2.0, 0.0);
const OP4: [ParamDesc; 8] = op_params!("op4", 1.0, 0.0);

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::FUSION,
    name: "Fusion",
    role: PluginRole::Generator,
    description: "Synth FM a 4 operatori: 8 algoritmi, rapporti, ADSR e velocity per operatore, feedback, \
                  filtro, vibrato e preset di fabbrica (piani elettrici, campane, bassi, ottoni).",
    params: &[
        OP1[0],
        OP1[1],
        OP1[2],
        OP1[3],
        OP1[4],
        OP1[5],
        OP1[6],
        OP1[7],
        OP2[0],
        OP2[1],
        OP2[2],
        OP2[3],
        OP2[4],
        OP2[5],
        OP2[6],
        OP2[7],
        OP3[0],
        OP3[1],
        OP3[2],
        OP3[3],
        OP3[4],
        OP3[5],
        OP3[6],
        OP3[7],
        OP4[0],
        OP4[1],
        OP4[2],
        OP4[3],
        OP4[4],
        OP4[5],
        OP4[6],
        OP4[7],
        ParamDesc::choice("fm_algo", "Algoritmo", ALGO_NAMES, 0),
        ParamDesc::new("fm_feedback", "Feedback op 4", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("flt_cutoff", "Cutoff", 20.0, 20_000.0, 20_000.0, "Hz"),
        ParamDesc::new("flt_reso", "Risonanza", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("lfo_rate", "Rate", 0.01, 20.0, 5.0, "Hz"),
        ParamDesc::new("lfo_pitch", "Vibrato", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("lfo_mod", "Mod. indice", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("lfo_delay", "Ritardo LFO", 0.0, 4.0, 0.0, "s"),
        ParamDesc::choice("voice_mode", "Modalità", VOICE_MODES, 0),
        ParamDesc::new("voice_glide", "Glide", 0.0, 2.0, 0.0, "s"),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.7, ""),
    ],
};

/// Preset scritto come elenco `"parametro" => valore`.
macro_rules! preset {
    ($name:literal, $cat:literal, [$($k:literal => $v:expr),* $(,)?]) => {
        Preset {
            name: $name,
            category: $cat,
            values: &[$(($k, $v)),*],
        }
    };
}

/// Preset di fabbrica: valori che differiscono dai default del descrittore.
pub static PRESETS: &[Preset] = &[
    preset!("Init", "Base", []),
    preset!("E-Piano DX", "Keys", [
        "fm_algo" => 3.0,
        "op1_level" => 1.0, "op1_fine" => -3.0, "op1_decay" => 2.8, "op1_sustain" => 0.0, "op1_release" => 0.6,
        "op1_vel" => 0.6,
        "op2_ratio" => 14.0, "op2_level" => 0.1, "op2_decay" => 0.25, "op2_sustain" => 0.0, "op2_release" => 0.2,
        "op2_vel" => 0.9,
        "op3_ratio" => 1.0, "op3_level" => 0.9, "op3_fine" => 3.0, "op3_decay" => 2.5, "op3_sustain" => 0.0,
        "op3_release" => 0.6, "op3_vel" => 0.6,
        "op4_level" => 0.2, "op4_decay" => 1.5, "op4_sustain" => 0.1, "op4_vel" => 0.9,
        "lfo_rate" => 4.0, "lfo_pitch" => 0.05,
        "amp" => 0.75,
    ]),
    preset!("Bell", "Keys", [
        "fm_algo" => 3.0,
        "op1_attack" => 0.0, "op1_decay" => 4.0, "op1_sustain" => 0.0, "op1_release" => 2.0,
        "op2_ratio" => 3.5, "op2_level" => 0.3, "op2_decay" => 3.0, "op2_sustain" => 0.0, "op2_release" => 2.0,
        "op3_ratio" => 2.0, "op3_level" => 0.5, "op3_attack" => 0.0, "op3_decay" => 2.5, "op3_sustain" => 0.0,
        "op3_release" => 1.5,
        "op4_ratio" => 1.41, "op4_level" => 0.3, "op4_decay" => 2.0, "op4_sustain" => 0.0, "op4_release" => 1.5,
        "amp" => 0.8,
    ]),
    preset!("Bass FM", "Bass", [
        "fm_algo" => 4.0,
        "op1_attack" => 0.0, "op1_decay" => 1.0, "op1_sustain" => 0.8, "op1_release" => 0.08,
        "op2_level" => 0.4, "op2_attack" => 0.0, "op2_decay" => 0.35, "op2_sustain" => 0.2, "op2_release" => 0.08,
        "op2_vel" => 0.7,
        "op3_level" => 0.12, "op3_attack" => 0.0, "op3_decay" => 0.15, "op3_sustain" => 0.0,
        "op4_level" => 0.2, "op4_sustain" => 0.4, "op4_release" => 0.08,
        "fm_feedback" => 0.3,
        "flt_cutoff" => 6_000.0,
        "voice_mode" => 1.0, "voice_glide" => 0.03,
        "amp" => 1.0,
    ]),
    preset!("Brass FM", "Lead", [
        "fm_algo" => 3.0,
        "op1_attack" => 0.05, "op1_decay" => 0.5, "op1_sustain" => 0.85, "op1_release" => 0.2,
        "op2_level" => 0.32, "op2_attack" => 0.09, "op2_decay" => 0.4, "op2_sustain" => 0.25, "op2_release" => 0.2,
        "op3_ratio" => 1.0, "op3_fine" => 6.0, "op3_level" => 0.8, "op3_attack" => 0.06, "op3_decay" => 0.5,
        "op3_sustain" => 0.85, "op3_release" => 0.2,
        "op4_level" => 0.28, "op4_attack" => 0.1, "op4_decay" => 0.4, "op4_sustain" => 0.25, "op4_release" => 0.2,
        "lfo_rate" => 5.5, "lfo_pitch" => 0.12, "lfo_delay" => 0.4,
        "flt_cutoff" => 7_000.0,
        "amp" => 1.1,
    ]),
    preset!("Organ", "Keys", [
        "fm_algo" => 7.0,
        "op1_attack" => 0.005, "op1_sustain" => 1.0, "op1_release" => 0.08,
        "op2_ratio" => 2.0, "op2_level" => 0.7, "op2_attack" => 0.005, "op2_sustain" => 1.0, "op2_release" => 0.08,
        "op3_ratio" => 3.0, "op3_level" => 0.5, "op3_attack" => 0.005, "op3_sustain" => 1.0, "op3_release" => 0.08,
        "op4_ratio" => 4.0, "op4_level" => 0.4, "op4_attack" => 0.005, "op4_sustain" => 1.0, "op4_release" => 0.08,
        "fm_feedback" => 0.12,
        "lfo_rate" => 6.5, "lfo_pitch" => 0.08,
        "amp" => 1.1,
    ]),
    preset!("Mallet", "Pluck", [
        "fm_algo" => 3.0,
        "op1_attack" => 0.0, "op1_decay" => 0.6, "op1_sustain" => 0.0, "op1_release" => 0.4,
        "op2_ratio" => 4.0, "op2_level" => 0.25, "op2_attack" => 0.0, "op2_decay" => 0.06, "op2_sustain" => 0.0,
        "op3_ratio" => 4.0, "op3_level" => 0.3, "op3_attack" => 0.0, "op3_decay" => 0.15, "op3_sustain" => 0.0,
        "op3_release" => 0.15,
        "op4_level" => 0.1, "op4_attack" => 0.0, "op4_decay" => 0.1, "op4_sustain" => 0.0,
        "amp" => 1.5,
    ]),
    preset!("Pluck", "Pluck", [
        "fm_algo" => 0.0,
        "op1_attack" => 0.0, "op1_decay" => 0.7, "op1_sustain" => 0.0, "op1_release" => 0.25,
        "op2_level" => 0.45, "op2_attack" => 0.0, "op2_decay" => 0.18, "op2_sustain" => 0.0,
        "op3_ratio" => 3.0, "op3_level" => 0.2, "op3_attack" => 0.0, "op3_decay" => 0.08, "op3_sustain" => 0.0,
        "amp" => 1.0,
    ]),
    preset!("Pad", "Pad", [
        "fm_algo" => 3.0,
        "op1_fine" => -7.0, "op1_attack" => 1.2, "op1_decay" => 2.0, "op1_sustain" => 0.8, "op1_release" => 2.5,
        "op2_ratio" => 2.0, "op2_level" => 0.18, "op2_attack" => 1.5, "op2_decay" => 2.0, "op2_sustain" => 0.6,
        "op2_release" => 2.5,
        "op3_ratio" => 1.0, "op3_fine" => 7.0, "op3_level" => 1.0, "op3_attack" => 1.0, "op3_decay" => 2.0,
        "op3_sustain" => 0.8, "op3_release" => 2.5,
        "op4_level" => 0.15, "op4_attack" => 2.0, "op4_decay" => 2.0, "op4_sustain" => 0.7, "op4_release" => 2.5,
        "lfo_rate" => 0.3, "lfo_mod" => 0.3,
        "flt_cutoff" => 5_000.0,
        "amp" => 0.8,
    ]),
    preset!("Lead", "Lead", [
        "fm_algo" => 2.0,
        "op1_attack" => 0.01, "op1_sustain" => 0.9, "op1_release" => 0.2,
        "op2_level" => 0.3, "op2_sustain" => 0.8, "op2_release" => 0.2,
        "op3_ratio" => 2.0, "op3_level" => 0.2, "op3_sustain" => 0.7, "op3_release" => 0.2,
        "op4_level" => 0.4, "op4_sustain" => 1.0, "op4_release" => 0.2,
        "fm_feedback" => 0.6,
        "lfo_rate" => 5.5, "lfo_pitch" => 0.25, "lfo_delay" => 0.3,
        "flt_cutoff" => 8_000.0,
        "voice_mode" => 2.0, "voice_glide" => 0.06,
        "amp" => 0.9,
    ]),
    preset!("Metallic FX", "FX", [
        "fm_algo" => 1.0,
        "op1_attack" => 0.0, "op1_decay" => 3.0, "op1_sustain" => 0.2, "op1_release" => 2.0,
        "op2_ratio" => 2.76, "op2_level" => 0.5, "op2_decay" => 3.0, "op2_sustain" => 0.3, "op2_release" => 2.0,
        "op3_ratio" => 5.43, "op3_level" => 0.4, "op3_decay" => 2.0, "op3_sustain" => 0.3, "op3_release" => 2.0,
        "op4_ratio" => 9.1, "op4_level" => 0.35, "op4_decay" => 2.0, "op4_sustain" => 0.3, "op4_release" => 2.0,
        "fm_feedback" => 0.5,
        "lfo_rate" => 2.5, "lfo_mod" => 0.6,
        "flt_cutoff" => 9_000.0, "flt_reso" => 0.3,
        "amp" => 0.35,
    ]),
    preset!("Kalimba", "Pluck", [
        "fm_algo" => 3.0,
        "op1_attack" => 0.0, "op1_decay" => 1.2, "op1_sustain" => 0.0, "op1_release" => 0.6,
        "op2_ratio" => 6.0, "op2_level" => 0.08, "op2_attack" => 0.0, "op2_decay" => 0.08, "op2_sustain" => 0.0,
        "op2_vel" => 0.8,
        "op3_ratio" => 5.4, "op3_level" => 0.25, "op3_attack" => 0.0, "op3_decay" => 0.25, "op3_sustain" => 0.0,
        "op3_release" => 0.2,
        "op4_level" => 0.15, "op4_attack" => 0.0, "op4_decay" => 0.1, "op4_sustain" => 0.0,
        "amp" => 1.5,
    ]),
    preset!("Sub Bass", "Bass", [
        "fm_algo" => 0.0,
        "op1_attack" => 0.003, "op1_sustain" => 1.0, "op1_release" => 0.1,
        "op2_level" => 0.06, "op2_attack" => 0.0, "op2_sustain" => 0.5,
        "flt_cutoff" => 2_000.0,
        "voice_mode" => 1.0,
        "amp" => 1.2,
    ]),
    preset!("Glass Keys", "Keys", [
        "fm_algo" => 5.0,
        "op1_attack" => 0.0, "op1_decay" => 2.0, "op1_sustain" => 0.2, "op1_release" => 0.8,
        "op2_ratio" => 3.0, "op2_level" => 0.6, "op2_attack" => 0.0, "op2_decay" => 1.0, "op2_sustain" => 0.0,
        "op2_release" => 0.5,
        "op3_ratio" => 5.0, "op3_level" => 0.4, "op3_attack" => 0.0, "op3_decay" => 0.5, "op3_sustain" => 0.0,
        "op3_release" => 0.4,
        "op4_ratio" => 1.0, "op4_level" => 0.2, "op4_attack" => 0.0, "op4_decay" => 1.0, "op4_sustain" => 0.1,
        "amp" => 0.9,
    ]),
];

/// Voci di polifonia.
const VOICES: usize = 16;
/// Campioni tra due aggiornamenti dei controlli.
const CTRL: usize = 32;
/// Indice di modulazione (in cicli) a livello 1: ≈ 7.5 radianti.
const MOD_DEPTH: f32 = 1.2;
/// Feedback massimo (cicli) dell'operatore 4.
const FB_DEPTH: f32 = 0.3;
/// Guadagno di uscita per voce (headroom per gli accordi).
const VOICE_GAIN: f32 = 0.35;
/// Punti della tabella del seno.
const SINE_N: usize = 4096;

/// Tabella del seno condivisa (un ciclo + punto di guardia), creata alla prima istanza.
fn sine_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=SINE_N)
            .map(|i| (std::f64::consts::TAU * i as f64 / SINE_N as f64).sin() as f32)
            .collect()
    })
}

/// Seno di una fase espressa in cicli (qualsiasi valore finito), con interpolazione lineare.
#[inline]
fn sine(table: &[f32], cycles: f32) -> f32 {
    let p = cycles - cycles.floor();
    let x = p * SINE_N as f32;
    let i = (x as usize).min(SINE_N - 1);
    let f = x - i as f32;
    let a = table[i];
    a + (table[i + 1] - a) * f
}

/// Impostazioni di un operatore.
#[derive(Debug, Clone, Copy)]
struct OpCfg {
    ratio: f32,
    fine: f32,
    level: f32,
    adsr: [f32; 4],
    vel: f32,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    vel: f32,
    /// Altezza corrente e di arrivo (semitoni MIDI, glide).
    pitch: f32,
    target: f32,
    /// Fasi degli operatori in cicli.
    phase: [f32; 4],
    env: [Adsr; 4],
    /// Ultime due uscite dell'operatore 4 (feedback mediato, stabile).
    fb: [f32; 2],
    svf: Svf,
    /// Campioni dall'attacco (ritardo dell'LFO).
    age: u32,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth Fusion.
pub struct Fusion {
    sr: f32,
    sine: &'static [f32],
    voices: Vec<Voice>,
    order: u64,
    pitch_offset: f32,
    ops: [OpCfg; 4],
    algo: usize,
    feedback: f32,
    cutoff: f32,
    reso: f32,
    lfo: Lfo,
    lfo_pitch: f32,
    lfo_mod: f32,
    lfo_delay: f32,
    mode: u8,
    glide: f32,
    amp: f32,
    last_pitch: Option<f32>,
    stack: MonoStack,
}

impl Default for Fusion {
    fn default() -> Self {
        Self::new()
    }
}

impl Fusion {
    pub fn new() -> Self {
        let voice = Voice {
            tag: VoiceTag::default(),
            vel: 1.0,
            pitch: 60.0,
            target: 60.0,
            phase: [0.0; 4],
            env: [Adsr::new(DEFAULT_SR); 4],
            fb: [0.0; 2],
            svf: Svf::new(DEFAULT_SR),
            age: 0,
        };
        let op = OpCfg {
            ratio: 1.0,
            fine: 0.0,
            level: 0.0,
            adsr: [0.002, 0.6, 0.6, 0.3],
            vel: 0.4,
        };
        let mut s = Self {
            sr: DEFAULT_SR,
            sine: sine_table(),
            voices: vec![voice; VOICES],
            order: 0,
            pitch_offset: 0.0,
            ops: [op; 4],
            algo: 0,
            feedback: 0.0,
            cutoff: 20_000.0,
            reso: 0.0,
            lfo: Lfo::new(DEFAULT_SR),
            lfo_pitch: 0.0,
            lfo_mod: 0.0,
            lfo_delay: 0.0,
            mode: 0,
            glide: 0.0,
            amp: 0.7,
            last_pitch: None,
            stack: MonoStack::new(),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.configure();
        s
    }

    fn configure(&mut self) {
        let ops = self.ops;
        let reso = self.reso;
        for v in &mut self.voices {
            for (e, o) in v.env.iter_mut().zip(&ops) {
                let [a, d, s, r] = o.adsr;
                e.set_adsr(a, d, s, r);
            }
            v.svf.set_mode(SvfMode::Lowpass);
            v.svf.set_resonance(reso);
        }
    }

    fn mono(&self) -> bool {
        self.mode != 0
    }

    fn voice_start(v: &mut Voice, order: u64, id: u64) {
        v.tag = VoiceTag {
            active: true,
            released: false,
            id,
            order,
        };
        v.phase = [0.0; 4];
        v.fb = [0.0; 2];
        v.svf.reset();
        v.age = 0;
    }

    /// Avvia (o riprende in legato) la voce mono sul tasto indicato.
    fn mono_play(&mut self, key: u8, vel: f32, id: u64, retrigger: bool) {
        let glide = self.glide > 0.0;
        let v = &mut self.voices[0];
        let was_active = v.tag.active && !v.tag.released;
        v.target = key as f32;
        if !glide || !v.tag.active {
            v.pitch = key as f32;
        }
        v.tag.id = id;
        if retrigger || !was_active {
            self.order += 1;
            if was_active {
                // Riattacco senza azzerare le fasi (niente click).
                v.tag.order = self.order;
            } else {
                Self::voice_start(v, self.order, id);
            }
            v.vel = vel;
            for e in &mut v.env {
                e.note_on();
            }
        }
    }

    fn release(v: &mut Voice) {
        v.tag.released = true;
        for e in &mut v.env {
            e.note_off();
        }
    }
}

impl Generator for Fusion {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        self.lfo.set_sample_rate(sample_rate);
        for v in &mut self.voices {
            for e in &mut v.env {
                e.set_sample_rate(sample_rate);
            }
            v.svf.set_sample_rate(sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        if let Some((pre, field)) = name.split_once('_') {
            let idx = match pre {
                "op1" => Some(0),
                "op2" => Some(1),
                "op3" => Some(2),
                "op4" => Some(3),
                _ => None,
            };
            if let Some(i) = idx {
                let o = &mut self.ops[i];
                match field {
                    "ratio" => o.ratio = v,
                    "fine" => o.fine = v,
                    "level" => o.level = v,
                    "attack" => o.adsr[0] = v,
                    "decay" => o.adsr[1] = v,
                    "sustain" => o.adsr[2] = v,
                    "release" => o.adsr[3] = v,
                    "vel" => o.vel = v,
                    _ => {}
                }
                if matches!(field, "attack" | "decay" | "sustain" | "release") {
                    self.configure();
                }
                return;
            }
        }
        match name {
            "fm_algo" => self.algo = (v as usize).min(ALGOS.len() - 1),
            "fm_feedback" => self.feedback = v,
            "flt_cutoff" => self.cutoff = v,
            "flt_reso" => {
                self.reso = v;
                self.configure();
            }
            "lfo_rate" => self.lfo.set_rate(v),
            "lfo_pitch" => self.lfo_pitch = v,
            "lfo_mod" => self.lfo_mod = v,
            "lfo_delay" => self.lfo_delay = v,
            "voice_mode" => {
                let mono = v as u8 != 0;
                if mono != self.mono() {
                    self.all_notes_off();
                }
                self.mode = v as u8;
            }
            "voice_glide" => self.glide = v,
            "amp" => self.amp = v,
            _ => {}
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = note.key.min(127);
        let vel = note.velocity.clamp(0.0, 1.0);
        if self.mono() {
            self.stack.push(key, note.voice_id, vel);
            let legato = self.mode == 2;
            self.mono_play(key, vel, note.voice_id, !legato);
            self.last_pitch = Some(key as f32);
            return;
        }
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let pitch = key as f32
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
        let v = &mut self.voices[idx];
        Self::voice_start(v, self.order, note.voice_id);
        v.vel = vel;
        v.pitch = start;
        v.target = pitch;
        for e in &mut v.env {
            e.note_on();
        }
    }

    fn note_off(&mut self, voice_id: u64) {
        if self.mono() {
            if !self.stack.remove(voice_id) || self.voices[0].tag.id != voice_id {
                return;
            }
            if let Some((key, id, vel)) = self.stack.last() {
                // Torna all'ultima nota ancora tenuta, senza riattaccare.
                self.mono_play(key, vel, id, false);
            } else {
                let v = &mut self.voices[0];
                if v.tag.active && !v.tag.released {
                    Self::release(v);
                }
            }
            return;
        }
        for v in &mut self.voices {
            if v.tag.active && !v.tag.released && v.tag.id == voice_id {
                Self::release(v);
            }
        }
    }

    fn all_notes_off(&mut self) {
        self.stack.clear();
        for v in &mut self.voices {
            if v.tag.active {
                Self::release(v);
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
        let sr = self.sr;
        let nyq = 0.5 * sr;
        let (mods, carriers) = ALGOS[self.algo];
        let n_car = carriers.count_ones().max(1) as f32;
        let filter_on = self.cutoff < 19_999.0;
        let glide = glide_coef(self.glide, CTRL, sr);
        let bend = self.pitch_offset / 100.0;
        let delay_samples = self.lfo_delay * sr;
        let table = self.sine;
        let ops = self.ops;
        // Rapporti con il fine (costanti nel blocco).
        let ratios = ops.map(|o| o.ratio * cents_to_ratio(o.fine));
        // Bersagli di ciascun operatore (per il limite di Carson).
        let mut targets = [0u8; 4];
        for (t, m) in mods.iter().enumerate() {
            for (j, tj) in targets.iter_mut().enumerate() {
                if m & (1 << j) != 0 {
                    *tj |= 1 << t;
                }
            }
        }
        let mut start = 0;
        while start < n {
            let len = CTRL.min(n - start);
            let lfo = self.lfo.next_sample();
            for _ in 1..len {
                self.lfo.next_sample();
            }
            for v in self.voices.iter_mut().filter(|v| v.tag.active) {
                v.pitch += (v.target - v.pitch) * glide;
                let fade_in = if delay_samples > 1.0 {
                    (v.age as f32 / delay_samples).min(1.0)
                } else {
                    1.0
                };
                v.age = v.age.saturating_add(len as u32);
                let vib = lfo * fade_in * self.lfo_pitch * self.lfo_pitch * 2.0;
                let f0 = midi_to_hz(v.pitch + bend + vib);
                let mod_scale = 1.0 + lfo * fade_in * self.lfo_mod * 0.6;
                let mut inc = [0.0f32; 4];
                let mut freq = [0.0f32; 4];
                // Guadagno d'uscita (portanti) e profondità di modulazione (in cicli).
                let mut car = [0.0f32; 4];
                let mut depth = [0.0f32; 4];
                for k in 0..4 {
                    freq[k] = f0 * ratios[k];
                    inc[k] = freq[k] / sr;
                }
                for k in 0..4 {
                    let o = &ops[k];
                    let velf = 1.0 - o.vel * (1.0 - v.vel);
                    // Sfuma gli operatori che si avvicinano a Nyquist.
                    let fade = ((0.48 * sr - freq[k]) / (0.08 * sr)).clamp(0.0, 1.0);
                    let g = o.level * velf * fade;
                    if carriers & (1 << k) != 0 {
                        car[k] = g / n_car;
                    }
                    if targets[k] != 0 {
                        let beta = g * MOD_DEPTH * mod_scale.max(0.0) * std::f32::consts::TAU;
                        // Carson: f_portante + (β + 1)·f_mod deve restare sotto Nyquist.
                        let mut ft = 0.0f32;
                        for (t, f) in freq.iter().enumerate() {
                            if targets[k] & (1 << t) != 0 {
                                ft = ft.max(*f);
                            }
                        }
                        let beta_max = ((nyq - ft) / freq[k].max(1.0) - 1.0).max(0.0);
                        let limit = if beta > beta_max { beta_max / beta } else { 1.0 };
                        depth[k] = g * MOD_DEPTH * mod_scale.max(0.0) * limit;
                    }
                }
                let fb_limit = ((nyq / freq[3].max(1.0) - 1.0) / 8.0).clamp(0.0, 1.0);
                let fb = self.feedback * FB_DEPTH * fb_limit;
                if filter_on {
                    let fc = self.cutoff.clamp(20.0, 0.45 * sr);
                    v.svf.set_cutoff(fc);
                }
                let gain = self.amp * VOICE_GAIN;
                for i in start..start + len {
                    let mut out = [0.0f32; 4];
                    let mut sum = 0.0f32;
                    let mut alive = false;
                    for k in (0..4).rev() {
                        let mut m = 0.0;
                        let mask = mods[k];
                        if mask != 0 {
                            for (j, oj) in out.iter().enumerate().skip(k + 1) {
                                if mask & (1 << j) != 0 {
                                    m += oj;
                                }
                            }
                        }
                        if k == 3 && fb > 0.0 {
                            m += fb * 0.5 * (v.fb[0] + v.fb[1]);
                        }
                        let e = v.env[k].next_sample();
                        let s = sine(table, v.phase[k] + m) * e;
                        v.phase[k] += inc[k];
                        v.phase[k] -= v.phase[k].floor();
                        out[k] = s * depth[k];
                        if carriers & (1 << k) != 0 {
                            sum += s * car[k];
                            alive |= !v.env[k].is_idle();
                        }
                        if k == 3 {
                            v.fb[1] = v.fb[0];
                            v.fb[0] = s * ops[3].level;
                        }
                    }
                    if filter_on {
                        sum = v.svf.process(sum);
                    }
                    let y = sum * gain;
                    left[i] += y;
                    right[i] += y;
                    if !alive {
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
