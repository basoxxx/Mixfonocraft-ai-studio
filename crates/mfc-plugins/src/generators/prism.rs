//! Prism: synth wavetable polifonico, 16 voci.
//!
//! - Otto wavetable generate proceduralmente alla prima istanza (mai nel thread audio) e
//!   condivise tra le istanze: [`FRAMES`] frame per tabella, ognuno band-limited con un
//!   livello di mip per ottava (sintesi additiva via FFT inversa interna).
//! - Due oscillatori wavetable con tabella, posizione, ottava/semitoni/fine, livello e
//!   unison fino a 8 voci con detune e spread; la posizione è modulabile da un inviluppo
//!   dedicato e dall'LFO. Interpolazione lineare tra campioni e tra frame.
//! - Filtro LP/HP/BP 12/24 dB con inviluppo, key tracking e velocity; ADSR d'ampiezza;
//!   LFO su pitch, cutoff e posizione; modalità Poly, Mono e Legato con glide.
//! - Preset di fabbrica in [`PRESETS`].
//!
//! Il controllo è aggiornato ogni [`CTRL`] campioni; nessuna allocazione nel thread audio.

use super::{
    apply_defaults, count_active, frames, glide_coef, param_value, pick_voice, MonoStack, PolyVoice, VoiceTag,
    DEFAULT_SR, OFF_ON, VOICE_MODES,
};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole, Preset};
use crate::ids;
use mfc_dsp::{midi_to_hz, pan_gains, Adsr, Lfo, LfoShape, Rng, Svf, SvfMode};
use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

/// Nomi delle wavetable incorporate.
pub const TABLE_NAMES: &[&str] = &[
    "Basic",
    "PWM",
    "Vocal",
    "Harmonic Sweep",
    "Digital",
    "Organ",
    "Metallic",
    "Noise",
];
const OCTAVES: &[&str] = &["-3", "-2", "-1", "0", "+1", "+2", "+3"];
const UNISON: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8"];
const FILTER_MODES: &[&str] = &["Low pass", "High pass", "Band pass"];
const SLOPES: &[&str] = &["12 dB", "24 dB"];
const LFO_SHAPES: &[&str] = &["Sine", "Triangle", "Saw", "Square", "S&H"];

macro_rules! wt_params {
    ($p:literal, $table:expr, $fine:expr, $level:expr) => {
        [
            ParamDesc::choice(concat!($p, "_table"), "Tabella", TABLE_NAMES, $table),
            ParamDesc::new(concat!($p, "_pos"), "Posizione", 0.0, 1.0, 0.0, ""),
            ParamDesc::new(concat!($p, "_pos_env"), "Pos. ← env", -1.0, 1.0, 0.0, ""),
            ParamDesc::new(concat!($p, "_pos_lfo"), "Pos. ← LFO", 0.0, 1.0, 0.0, ""),
            ParamDesc::choice(concat!($p, "_oct"), "Ottava", OCTAVES, 3),
            ParamDesc::new(concat!($p, "_semi"), "Semitoni", -12.0, 12.0, 0.0, "st"),
            ParamDesc::new(concat!($p, "_fine"), "Fine", -100.0, 100.0, $fine, "ct"),
            ParamDesc::new(concat!($p, "_level"), "Livello", 0.0, 1.0, $level, ""),
            ParamDesc::choice(concat!($p, "_unison"), "Unison", UNISON, 0),
            ParamDesc::new(concat!($p, "_detune"), "Detune", 0.0, 1.0, 0.25, ""),
            ParamDesc::new(concat!($p, "_spread"), "Spread", 0.0, 1.0, 0.7, ""),
        ]
    };
}

const WT1: [ParamDesc; 11] = wt_params!("wt1", 0, 0.0, 0.8);
const WT2: [ParamDesc; 11] = wt_params!("wt2", 0, 7.0, 0.0);

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::PRISM,
    name: "Prism",
    role: PluginRole::Generator,
    description: "Synth wavetable: 2 oscillatori con 8 tabelle morphabili, unison, posizione modulata da \
                  inviluppo e LFO, filtro 12/24 dB con inviluppo, mono/legato e preset di fabbrica.",
    params: &[
        WT1[0],
        WT1[1],
        WT1[2],
        WT1[3],
        WT1[4],
        WT1[5],
        WT1[6],
        WT1[7],
        WT1[8],
        WT1[9],
        WT1[10],
        WT2[0],
        WT2[1],
        WT2[2],
        WT2[3],
        WT2[4],
        WT2[5],
        WT2[6],
        WT2[7],
        WT2[8],
        WT2[9],
        WT2[10],
        ParamDesc::choice("flt_mode", "Tipo", FILTER_MODES, 0),
        ParamDesc::choice("flt_slope", "Pendenza", SLOPES, 0),
        ParamDesc::new("flt_cutoff", "Cutoff", 20.0, 20_000.0, 6_000.0, "Hz"),
        ParamDesc::new("flt_reso", "Risonanza", 0.0, 1.0, 0.15, ""),
        ParamDesc::new("flt_env", "Env amount", -1.0, 1.0, 0.3, ""),
        ParamDesc::new("flt_key", "Key track", 0.0, 1.0, 0.3, ""),
        ParamDesc::new("flt_vel", "Velocity", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("fenv_attack", "Attack", 0.0, 10.0, 0.005, "s"),
        ParamDesc::new("fenv_decay", "Decay", 0.0, 10.0, 0.4, "s"),
        ParamDesc::new("fenv_sustain", "Sustain", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("fenv_release", "Release", 0.0, 10.0, 0.3, "s"),
        ParamDesc::new("menv_attack", "Attack", 0.0, 10.0, 0.0, "s"),
        ParamDesc::new("menv_decay", "Decay", 0.0, 10.0, 0.8, "s"),
        ParamDesc::new("menv_sustain", "Sustain", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("menv_release", "Release", 0.0, 10.0, 0.5, "s"),
        ParamDesc::new("aenv_attack", "Attack", 0.0, 10.0, 0.005, "s"),
        ParamDesc::new("aenv_decay", "Decay", 0.0, 10.0, 0.3, "s"),
        ParamDesc::new("aenv_sustain", "Sustain", 0.0, 1.0, 0.8, ""),
        ParamDesc::new("aenv_release", "Release", 0.0, 10.0, 0.25, "s"),
        ParamDesc::choice("lfo_shape", "Forma", LFO_SHAPES, 0),
        ParamDesc::new("lfo_rate", "Rate", 0.01, 30.0, 2.0, "Hz"),
        ParamDesc::new("lfo_pitch", "→ Pitch", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("lfo_cutoff", "→ Cutoff", 0.0, 1.0, 0.0, ""),
        ParamDesc::choice("lfo_retrig", "Retrigger", OFF_ON, 0),
        ParamDesc::choice("voice_mode", "Modalità", VOICE_MODES, 0),
        ParamDesc::new("voice_glide", "Glide", 0.0, 2.0, 0.0, "s"),
        ParamDesc::new("voice_vel", "Sens. velocity", 0.0, 1.0, 0.6, ""),
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
    preset!("Morph Lead", "Lead", [
        "wt1_pos" => 0.66, "wt1_pos_env" => -0.3, "wt1_unison" => 2.0, "wt1_detune" => 0.2,
        "wt2_table" => 1.0, "wt2_pos" => 0.3, "wt2_oct" => 4.0, "wt2_level" => 0.35,
        "flt_cutoff" => 5_000.0, "flt_env" => 0.25,
        "menv_decay" => 0.6,
        "aenv_attack" => 0.01, "aenv_sustain" => 0.9, "aenv_release" => 0.3,
        "lfo_rate" => 5.5, "lfo_pitch" => 0.12,
        "voice_mode" => 2.0, "voice_glide" => 0.05,
        "amp" => 1.1,
    ]),
    preset!("PWM Strings", "Pad", [
        "wt1_table" => 1.0, "wt1_pos" => 0.3, "wt1_pos_lfo" => 0.5, "wt1_unison" => 3.0, "wt1_detune" => 0.25,
        "wt1_spread" => 1.0,
        "wt2_table" => 1.0, "wt2_pos" => 0.1, "wt2_oct" => 4.0, "wt2_level" => 0.3,
        "flt_cutoff" => 4_000.0, "flt_env" => 0.1,
        "aenv_attack" => 0.4, "aenv_sustain" => 0.9, "aenv_release" => 1.2,
        "lfo_rate" => 0.4,
        "amp" => 0.75,
    ]),
    preset!("Vocal Pad", "Pad", [
        "wt1_table" => 2.0, "wt1_pos_env" => 1.0, "wt1_unison" => 2.0, "wt1_detune" => 0.15,
        "wt2_table" => 2.0, "wt2_pos" => 0.5, "wt2_level" => 0.6, "wt2_fine" => 8.0,
        "flt_cutoff" => 5_000.0, "flt_env" => 0.0,
        "menv_attack" => 2.0, "menv_sustain" => 0.6, "menv_release" => 2.0,
        "aenv_attack" => 0.6, "aenv_decay" => 1.0, "aenv_sustain" => 0.9, "aenv_release" => 2.5,
        "amp" => 0.8,
    ]),
    preset!("Choir", "Pad", [
        "wt1_table" => 2.0, "wt1_pos" => 0.25, "wt1_unison" => 4.0, "wt1_detune" => 0.15, "wt1_spread" => 0.9,
        "wt2_table" => 2.0, "wt2_pos" => 0.75, "wt2_unison" => 2.0, "wt2_level" => 0.5,
        "flt_cutoff" => 6_000.0, "flt_env" => 0.0,
        "aenv_attack" => 0.8, "aenv_decay" => 1.0, "aenv_sustain" => 0.9, "aenv_release" => 2.0,
        "lfo_rate" => 5.0, "lfo_pitch" => 0.08,
        "amp" => 0.8,
    ]),
    preset!("Harmonic Sweep", "FX", [
        "wt1_table" => 3.0, "wt1_pos_lfo" => 1.0, "wt1_unison" => 2.0, "wt1_detune" => 0.1,
        "flt_cutoff" => 12_000.0, "flt_env" => 0.0,
        "aenv_attack" => 0.3, "aenv_sustain" => 1.0, "aenv_release" => 1.5,
        "lfo_shape" => 1.0, "lfo_rate" => 0.25, "lfo_retrig" => 1.0,
        "amp" => 0.8,
    ]),
    preset!("Digital Bass", "Bass", [
        "wt1_table" => 4.0, "wt1_pos" => 0.5, "wt1_oct" => 2.0,
        "wt2_oct" => 2.0, "wt2_level" => 0.5, "wt2_fine" => 0.0,
        "flt_cutoff" => 2_500.0, "flt_env" => 0.4, "flt_slope" => 1.0,
        "fenv_decay" => 0.3, "fenv_sustain" => 0.0,
        "aenv_attack" => 0.0, "aenv_sustain" => 1.0, "aenv_release" => 0.08,
        "voice_mode" => 1.0, "voice_glide" => 0.04,
        "amp" => 1.2,
    ]),
    preset!("Growl Bass", "Bass", [
        "wt1_table" => 2.0, "wt1_pos" => 0.2, "wt1_pos_lfo" => 0.7, "wt1_oct" => 2.0,
        "wt2_table" => 0.0, "wt2_pos" => 0.66, "wt2_oct" => 2.0, "wt2_level" => 0.4, "wt2_fine" => 0.0,
        "flt_slope" => 1.0, "flt_cutoff" => 1_500.0, "flt_reso" => 0.3, "flt_env" => 0.2,
        "aenv_attack" => 0.0, "aenv_sustain" => 1.0, "aenv_release" => 0.1,
        "lfo_rate" => 4.0, "lfo_retrig" => 1.0, "lfo_shape" => 1.0,
        "voice_mode" => 1.0,
        "amp" => 1.8,
    ]),
    preset!("Organ", "Keys", [
        "wt1_table" => 5.0, "wt1_pos" => 0.7,
        "flt_cutoff" => 12_000.0, "flt_env" => 0.0,
        "aenv_attack" => 0.005, "aenv_sustain" => 1.0, "aenv_release" => 0.08,
        "lfo_rate" => 6.0, "lfo_pitch" => 0.06,
        "amp" => 0.75,
    ]),
    preset!("Metal Bell", "Keys", [
        "wt1_table" => 6.0, "wt1_pos" => 0.4, "wt1_pos_env" => -0.4,
        "wt2_oct" => 4.0, "wt2_level" => 0.3, "wt2_fine" => 0.0,
        "flt_cutoff" => 12_000.0, "flt_env" => 0.0,
        "menv_decay" => 1.5,
        "aenv_attack" => 0.0, "aenv_decay" => 2.0, "aenv_sustain" => 0.0, "aenv_release" => 1.5,
        "amp" => 0.8,
    ]),
    preset!("Digital Keys", "Keys", [
        "wt1_table" => 4.0, "wt1_pos" => 0.1, "wt1_pos_env" => 0.5,
        "wt2_oct" => 4.0, "wt2_level" => 0.3, "wt2_fine" => 0.0,
        "flt_cutoff" => 7_000.0, "flt_env" => 0.1, "flt_vel" => 0.5,
        "menv_decay" => 0.8,
        "aenv_attack" => 0.002, "aenv_decay" => 1.2, "aenv_sustain" => 0.3, "aenv_release" => 0.4,
        "amp" => 0.85,
    ]),
    preset!("Wavetable Pluck", "Pluck", [
        "wt1_table" => 3.0, "wt1_pos" => 0.8, "wt1_pos_env" => -0.8,
        "wt2_table" => 1.0, "wt2_pos" => 0.5, "wt2_level" => 0.3, "wt2_oct" => 4.0, "wt2_fine" => 0.0,
        "flt_cutoff" => 3_000.0, "flt_env" => 0.5,
        "fenv_decay" => 0.25, "fenv_sustain" => 0.0,
        "menv_decay" => 0.25,
        "aenv_attack" => 0.0, "aenv_decay" => 0.5, "aenv_sustain" => 0.0, "aenv_release" => 0.3,
        "amp" => 1.5,
    ]),
    preset!("Prism Saw", "Lead", [
        "wt1_pos" => 0.667, "wt1_unison" => 6.0, "wt1_detune" => 0.35, "wt1_spread" => 1.0,
        "wt2_pos" => 0.667, "wt2_oct" => 4.0, "wt2_unison" => 3.0, "wt2_level" => 0.3,
        "flt_cutoff" => 7_000.0, "flt_env" => 0.2,
        "aenv_attack" => 0.01, "aenv_sustain" => 0.9, "aenv_release" => 0.35,
        "amp" => 1.0,
    ]),
    preset!("Noise Sweep", "FX", [
        "wt1_table" => 7.0, "wt1_pos_lfo" => 0.8, "wt1_unison" => 3.0, "wt1_detune" => 0.4,
        "wt2_table" => 3.0, "wt2_pos" => 0.5, "wt2_pos_lfo" => 1.0, "wt2_level" => 0.4,
        "flt_mode" => 2.0, "flt_cutoff" => 1_200.0, "flt_reso" => 0.5, "flt_env" => 0.0,
        "lfo_shape" => 2.0, "lfo_rate" => 0.2, "lfo_cutoff" => 0.8, "lfo_retrig" => 1.0,
        "aenv_attack" => 0.6, "aenv_sustain" => 1.0, "aenv_release" => 2.5,
        "amp" => 0.5,
    ]),
];

/// Voci di polifonia.
const VOICES: usize = 16;
/// Voci di unison massime per oscillatore.
const MAX_UNISON: usize = 8;
/// Campioni tra due aggiornamenti dei controlli.
const CTRL: usize = 32;
/// Detune massimo dell'unison (semitoni, ai lati).
const MAX_DETUNE_ST: f32 = 0.5;
/// Guadagno di uscita per voce.
const VOICE_GAIN: f32 = 0.3;

/// Frame per tabella.
pub const FRAMES: usize = 32;
/// Lunghezza del ciclo al livello di mip più ricco.
pub const TABLE_LEN: usize = 2048;
/// Livelli di mip: il livello `l` contiene al massimo `MAX_HARM >> l` armoniche.
const MIPS: usize = 10;
const MAX_HARM: usize = 512;

/// Lunghezza del ciclo al livello `l` (almeno 16 campioni per armonica, minimo 128).
const fn mip_len(l: usize) -> usize {
    let n = (MAX_HARM >> l) * 16;
    if n > TABLE_LEN {
        TABLE_LEN
    } else if n < 128 {
        128
    } else {
        n
    }
}

/// Banco di wavetable: tabelle × frame × mip, ogni ciclo con un campione di guardia.
pub struct WaveBank {
    data: Vec<f32>,
    /// Offset di ogni mip dentro un frame.
    mip_off: [usize; MIPS],
    frame_stride: usize,
}

impl WaveBank {
    fn build() -> Self {
        let mut mip_off = [0; MIPS];
        let mut off = 0;
        for (l, m) in mip_off.iter_mut().enumerate() {
            *m = off;
            off += mip_len(l) + 1;
        }
        let frame_stride = off;
        let mut data = vec![0.0f32; TABLE_NAMES.len() * FRAMES * frame_stride];
        let mut spec = vec![(0.0f64, 0.0f64); MAX_HARM + 1];
        let mut re = vec![0.0f64; TABLE_LEN];
        let mut im = vec![0.0f64; TABLE_LEN];
        for t in 0..TABLE_NAMES.len() {
            for f in 0..FRAMES {
                let pos = f as f64 / (FRAMES - 1) as f64;
                spectrum(t, f, pos, &mut spec, &mut re, &mut im);
                let base = (t * FRAMES + f) * frame_stride;
                let mut peak = 0.0f64;
                for l in 0..MIPS {
                    let n = mip_len(l);
                    let harm = MAX_HARM >> l;
                    re[..n].fill(0.0);
                    im[..n].fill(0.0);
                    for (h, c) in spec.iter().enumerate().take(harm + 1).skip(1) {
                        re[h] = c.0 * 0.5;
                        im[h] = c.1 * 0.5;
                        re[n - h] = c.0 * 0.5;
                        im[n - h] = -c.1 * 0.5;
                    }
                    fft(&mut re[..n], &mut im[..n], 1.0);
                    if l == 0 {
                        peak = re[..n].iter().fold(0.0f64, |m, x| m.max(x.abs()));
                    }
                    let norm = if peak > 1e-9 { 1.0 / peak } else { 0.0 };
                    let dst = &mut data[base + mip_off[l]..base + mip_off[l] + n + 1];
                    for (d, x) in dst.iter_mut().zip(&re[..n]) {
                        *d = (*x * norm) as f32;
                    }
                    dst[n] = dst[0];
                }
            }
        }
        Self {
            data,
            mip_off,
            frame_stride,
        }
    }

    /// Banco condiviso, generato alla prima richiesta (non real-time).
    pub fn shared() -> &'static WaveBank {
        static BANK: OnceLock<WaveBank> = OnceLock::new();
        BANK.get_or_init(WaveBank::build)
    }

    /// Ciclo (con guardia) di una tabella, frame e livello di mip.
    pub fn cycle(&self, table: usize, frame: usize, mip: usize) -> &[f32] {
        let base = (table * FRAMES + frame) * self.frame_stride + self.mip_off[mip];
        &self.data[base..base + mip_len(mip) + 1]
    }

    /// Lettura senza aliasing del frame `frame` (0..FRAMES-1, frazionario) alla fase `phase`
    /// (cicli) per una frequenza `hz` a `sr`.
    pub fn sample(&self, table: usize, frame: f32, phase: f32, hz: f32, sr: f32) -> f32 {
        let mip = mip_for(hz, sr);
        let fa = (frame.max(0.0) as usize).min(FRAMES - 1);
        let fb = (fa + 1).min(FRAMES - 1);
        let r = Reader {
            a: self.offset(table, fa, mip),
            b: self.offset(table, fb, mip),
            len: mip_len(mip),
            mix: frame - fa as f32,
        };
        r.read(&self.data, phase - phase.floor())
    }

    fn offset(&self, table: usize, frame: usize, mip: usize) -> usize {
        (table * FRAMES + frame) * self.frame_stride + self.mip_off[mip]
    }
}

/// Livello di mip con tutte le armoniche sotto ~0.45·sr.
fn mip_for(hz: f32, sr: f32) -> usize {
    let ratio = MAX_HARM as f32 * hz.abs() / (0.45 * sr);
    if ratio <= 1.0 {
        0
    } else {
        (ratio.log2().ceil() as usize).min(MIPS - 1)
    }
}

/// Lettore di due frame adiacenti allo stesso livello di mip.
#[derive(Debug, Clone, Copy, Default)]
struct Reader {
    a: usize,
    b: usize,
    len: usize,
    mix: f32,
}

impl Reader {
    #[inline]
    fn read(&self, data: &[f32], phase: f32) -> f32 {
        let x = phase * self.len as f32;
        let i = (x as usize).min(self.len - 1);
        let t = x - i as f32;
        let (a0, a1) = (data[self.a + i], data[self.a + i + 1]);
        let (b0, b1) = (data[self.b + i], data[self.b + i + 1]);
        let sa = a0 + (a1 - a0) * t;
        let sb = b0 + (b1 - b0) * t;
        sa + (sb - sa) * self.mix
    }
}

/// FFT complessa radix-2 in place, non normalizzata: `sign` -1 diretta, +1 inversa.
fn fft(re: &mut [f64], im: &mut [f64], sign: f64) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = sign * TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Coefficiente di un'armonica in fase seno con ampiezza `a` e sfasamento `phi` (radianti).
#[inline]
fn sine_coef(a: f64, phi: f64) -> (f64, f64) {
    // Re(c·e^{iθ}) = a·sin(θ + φ)  ⇒  c = a·e^{i(φ - π/2)}.
    let p = phi - PI / 2.0;
    (a * p.cos(), a * p.sin())
}

/// Spettro (armoniche 1..=MAX_HARM) del frame `pos` (0..1) della tabella `t`.
/// `re`/`im` sono buffer di lavoro di [`TABLE_LEN`] campioni.
fn spectrum(t: usize, frame: usize, pos: f64, spec: &mut [(f64, f64)], re: &mut [f64], im: &mut [f64]) {
    spec.fill((0.0, 0.0));
    let harm = |h: usize| h as f64;
    match t {
        // Basic: seno → triangolo → dente di sega → quadra.
        0 => {
            let s = (pos * 3.0).min(2.999_999);
            let seg = s as usize;
            let mix = s - seg as f64;
            let shape = |k: usize, h: usize| -> f64 {
                let hf = harm(h);
                match k {
                    0 => f64::from(u8::from(h == 1)),
                    1 if h % 2 == 1 => {
                        let sign = if h % 4 == 1 { 1.0 } else { -1.0 };
                        8.0 / (PI * PI * hf * hf) * sign
                    }
                    2 => 2.0 / (PI * hf) * if h % 2 == 1 { 1.0 } else { -1.0 },
                    3 if h % 2 == 1 => 4.0 / (PI * hf),
                    _ => 0.0,
                }
            };
            for (h, c) in spec.iter_mut().enumerate().skip(1) {
                let a = shape(seg, h) * (1.0 - mix) + shape(seg + 1, h) * mix;
                *c = sine_coef(a, 0.0);
            }
        }
        // PWM: impulso con duty dal 50% al 5% (differenza di due denti di sega).
        1 => {
            let duty = 0.5 - 0.45 * pos;
            for (h, c) in spec.iter_mut().enumerate().skip(1) {
                let hf = harm(h);
                let k = 2.0 / (PI * hf);
                // c = -i·k·(1 - e^{-i2πhd})
                let ang = -TAU * hf * duty;
                let (er, ei) = (1.0 - ang.cos(), -ang.sin());
                *c = (k * ei, -k * er);
            }
        }
        // Vocal: formanti delle vocali a → e → i → o → u su una fondamentale di 120 Hz.
        2 => {
            const VOWELS: [[f64; 3]; 5] = [
                [730.0, 1090.0, 2440.0],
                [530.0, 1840.0, 2480.0],
                [270.0, 2290.0, 3010.0],
                [570.0, 840.0, 2410.0],
                [300.0, 870.0, 2240.0],
            ];
            let s = (pos * 4.0).min(3.999_999);
            let seg = s as usize;
            let mix = s - seg as f64;
            let amps = [1.0, 0.6, 0.3];
            let bws = [90.0, 110.0, 160.0];
            for (h, c) in spec.iter_mut().enumerate().skip(1) {
                let fh = 120.0 * harm(h);
                let mut a = 0.0;
                for k in 0..3 {
                    let fk = VOWELS[seg][k] * (1.0 - mix) + VOWELS[seg + 1][k] * mix;
                    let d = (fh - fk) / bws[k];
                    a += amps[k] / (1.0 + d * d);
                }
                a = (a + 0.04) / harm(h).sqrt();
                *c = sine_coef(a, PI * harm(h) * harm(h) / 40.0);
            }
        }
        // Harmonic Sweep: una banda di armoniche che sale di 7 ottave, sopra la fondamentale.
        3 => {
            let center = 1.0 + pos * 7.0;
            for (h, c) in spec.iter_mut().enumerate().skip(1) {
                let d = harm(h).log2() - center;
                let mut a = (-d * d / (2.0 * 0.45 * 0.45)).exp();
                if h == 1 {
                    a += 0.5;
                }
                *c = sine_coef(a, PI * harm(h) * harm(h) / 64.0);
            }
        }
        // Digital: seno con fase distorta, quantizzato in ampiezza e nel tempo (in calo con la posizione).
        4 => {
            let levels = 2f64.powf(7.0 - 5.5 * pos).round().max(2.0);
            let hold = 1 + (pos * pos * 96.0) as usize;
            let mut held = 0.0;
            for (n, (r, i)) in re.iter_mut().zip(im.iter_mut()).enumerate() {
                if n % hold == 0 {
                    let th = TAU * n as f64 / TABLE_LEN as f64;
                    let x = (th + 0.9 * pos * (3.0 * th).sin()).sin();
                    held = (x * levels).round() / levels;
                }
                *r = held;
                *i = 0.0;
            }
            analyze(spec, re, im);
        }
        // Organ: registri a tiranti (8', 4', 2⅔', 2', 1⅗', 1⅓', 1') sempre più pieni.
        5 => {
            const HARMS: [usize; 7] = [1, 2, 3, 4, 5, 6, 8];
            const REGS: [[f64; 7]; 5] = [
                [8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [8.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [8.0, 8.0, 8.0, 0.0, 0.0, 0.0, 0.0],
                [8.0, 6.0, 8.0, 6.0, 0.0, 4.0, 0.0],
                [8.0, 8.0, 8.0, 8.0, 6.0, 8.0, 8.0],
            ];
            let s = (pos * 4.0).min(3.999_999);
            let seg = s as usize;
            let mix = s - seg as f64;
            for k in 0..7 {
                let bar = |r: f64| if r > 0.0 { 2f64.powf((r - 8.0) / 2.0) } else { 0.0 };
                let a = bar(REGS[seg][k]) * (1.0 - mix) + bar(REGS[seg + 1][k]) * mix;
                spec[HARMS[k]] = sine_coef(a, 0.0);
            }
        }
        // Metallic: parziali rade con spaziatura crescente (suono a campana/anello).
        6 => {
            let step = 1.7 + 4.0 * pos;
            for k in 0..18 {
                let kf = k as f64;
                let h = (1.0 + kf * step + kf * kf * 0.35 * pos).round() as usize;
                let side = h + 1 + (5.0 * pos).round() as usize;
                let a = 1.0 / (1.0 + kf).powf(0.8);
                for (hh, aa) in [(h, a), (side, a * 0.5 * pos)] {
                    if hh <= MAX_HARM && aa > 0.0 {
                        let c = sine_coef(aa, kf * 1.3);
                        spec[hh].0 += c.0;
                        spec[hh].1 += c.1;
                    }
                }
            }
        }
        // Noise: ampiezze e fasi casuali, spettro sempre più brillante.
        _ => {
            let mut rng = Rng::new(0xA5A5 + frame as u32 * 7919);
            let tilt = 1.3 - pos;
            for (h, c) in spec.iter_mut().enumerate().skip(1) {
                let a = (0.2 + f64::from(rng.next_f32())) / harm(h).powf(tilt);
                let phi = TAU * f64::from(rng.next_f32());
                *c = sine_coef(if h == 1 { a + 0.6 } else { a }, phi);
            }
        }
    }
}

/// Spettro di un ciclo nel dominio del tempo (`re`, [`TABLE_LEN`] campioni).
fn analyze(spec: &mut [(f64, f64)], re: &mut [f64], im: &mut [f64]) {
    fft(re, im, -1.0);
    let k = 2.0 / TABLE_LEN as f64;
    for (h, c) in spec.iter_mut().enumerate().skip(1) {
        *c = (re[h] * k, im[h] * k);
    }
}

/// Impostazioni di un oscillatore wavetable.
#[derive(Debug, Clone, Copy)]
struct OscCfg {
    table: usize,
    pos: f32,
    pos_env: f32,
    pos_lfo: f32,
    /// Ottava (indice), semitoni, cent.
    tuning: [f32; 3],
    transpose: f32,
    level: f32,
    unison: usize,
    detune: f32,
    spread: f32,
    offsets: [f32; MAX_UNISON],
    gains: [(f32, f32); MAX_UNISON],
}

impl OscCfg {
    fn new() -> Self {
        Self {
            table: 0,
            pos: 0.0,
            pos_env: 0.0,
            pos_lfo: 0.0,
            tuning: [3.0, 0.0, 0.0],
            transpose: 0.0,
            level: 0.8,
            unison: 1,
            detune: 0.25,
            spread: 0.7,
            offsets: [0.0; MAX_UNISON],
            gains: [(0.0, 0.0); MAX_UNISON],
        }
    }

    fn update(&mut self) {
        let [oct, semi, fine] = self.tuning;
        self.transpose = (oct - 3.0) * 12.0 + semi + fine / 100.0;
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

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    vel: f32,
    pitch: f32,
    target: f32,
    phase: [[f32; MAX_UNISON]; 2],
    inc: [[f32; MAX_UNISON]; 2],
    readers: [Reader; 2],
    rng: Rng,
    svf: [[Svf; 2]; 2],
    fenv: Adsr,
    menv: Adsr,
    aenv: Adsr,
    fenv_level: f32,
    menv_level: f32,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth Prism.
pub struct Prism {
    sr: f32,
    bank: &'static WaveBank,
    voices: Vec<Voice>,
    order: u64,
    rng: Rng,
    pitch_offset: f32,
    osc: [OscCfg; 2],
    flt_mode: SvfMode,
    flt_24: bool,
    cutoff: f32,
    reso: f32,
    flt_env: f32,
    flt_key: f32,
    flt_vel: f32,
    fenv: [f32; 4],
    menv: [f32; 4],
    aenv: [f32; 4],
    lfo: Lfo,
    lfo_pitch: f32,
    lfo_cutoff: f32,
    lfo_retrig: bool,
    mode: u8,
    glide: f32,
    vel_sens: f32,
    amp: f32,
    last_pitch: Option<f32>,
    stack: MonoStack,
}

impl Default for Prism {
    fn default() -> Self {
        Self::new()
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

impl Prism {
    pub fn new() -> Self {
        let voice = Voice {
            tag: VoiceTag::default(),
            vel: 1.0,
            pitch: 60.0,
            target: 60.0,
            phase: [[0.0; MAX_UNISON]; 2],
            inc: [[0.0; MAX_UNISON]; 2],
            readers: [Reader::default(); 2],
            rng: Rng::new(1),
            svf: [[Svf::new(DEFAULT_SR); 2]; 2],
            fenv: Adsr::new(DEFAULT_SR),
            menv: Adsr::new(DEFAULT_SR),
            aenv: Adsr::new(DEFAULT_SR),
            fenv_level: 0.0,
            menv_level: 0.0,
        };
        let mut s = Self {
            sr: DEFAULT_SR,
            bank: WaveBank::shared(),
            voices: vec![voice; VOICES],
            order: 0,
            rng: Rng::new(0x9815),
            pitch_offset: 0.0,
            osc: [OscCfg::new(); 2],
            flt_mode: SvfMode::Lowpass,
            flt_24: false,
            cutoff: 6_000.0,
            reso: 0.15,
            flt_env: 0.3,
            flt_key: 0.3,
            flt_vel: 0.2,
            fenv: [0.005, 0.4, 0.2, 0.3],
            menv: [0.0, 0.8, 0.0, 0.5],
            aenv: [0.005, 0.3, 0.8, 0.25],
            lfo: Lfo::new(DEFAULT_SR),
            lfo_pitch: 0.0,
            lfo_cutoff: 0.0,
            lfo_retrig: false,
            mode: 0,
            glide: 0.0,
            vel_sens: 0.6,
            amp: 0.7,
            last_pitch: None,
            stack: MonoStack::new(),
        };
        for (i, v) in s.voices.iter_mut().enumerate() {
            v.rng = Rng::new(0x7A11 + i as u32 * 7919);
        }
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.configure();
        s
    }

    fn configure(&mut self) {
        let [fa, fd, fs, fr] = self.fenv;
        let [ma, md, ms, mr] = self.menv;
        let [aa, ad, as_, ar] = self.aenv;
        let (mode, reso) = (self.flt_mode, self.reso);
        for v in &mut self.voices {
            v.fenv.set_adsr(fa, fd, fs, fr);
            v.menv.set_adsr(ma, md, ms, mr);
            v.aenv.set_adsr(aa, ad, as_, ar);
            for ch in &mut v.svf {
                ch[0].set_mode(mode);
                ch[0].set_resonance(reso);
                ch[1].set_mode(mode);
                ch[1].set_resonance(0.0);
            }
        }
    }

    fn mono(&self) -> bool {
        self.mode != 0
    }

    fn trigger(v: &mut Voice) {
        v.fenv.note_on();
        v.menv.note_on();
        v.aenv.note_on();
    }

    fn release(v: &mut Voice) {
        v.tag.released = true;
        v.fenv.note_off();
        v.menv.note_off();
        v.aenv.note_off();
    }

    fn reset_voice(v: &mut Voice, unison: [usize; 2], rng: &mut Rng) {
        for (bank, n) in v.phase.iter_mut().zip(unison) {
            for p in bank.iter_mut() {
                *p = if n > 1 { rng.next_f32() } else { 0.0 };
            }
        }
        for ch in &mut v.svf {
            for f in ch {
                f.reset();
            }
        }
    }

    fn mono_play(&mut self, key: u8, vel: f32, id: u64, retrigger: bool) {
        let glide = self.glide > 0.0;
        let unison = [self.osc[0].unison, self.osc[1].unison];
        let mut rng = self.rng;
        let v = &mut self.voices[0];
        let was_active = v.tag.active && !v.tag.released;
        v.target = key as f32;
        if !glide || !v.tag.active {
            v.pitch = key as f32;
        }
        v.tag.id = id;
        if retrigger || !was_active {
            self.order += 1;
            if !v.tag.active {
                Self::reset_voice(v, unison, &mut rng);
            }
            v.tag = VoiceTag {
                active: true,
                released: false,
                id,
                order: self.order,
            };
            v.vel = vel;
            Self::trigger(v);
        }
        self.rng = rng;
    }
}

impl Generator for Prism {
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
            v.fenv.set_sample_rate(sample_rate);
            v.menv.set_sample_rate(sample_rate);
            v.aenv.set_sample_rate(sample_rate);
            for ch in &mut v.svf {
                for f in ch {
                    f.set_sample_rate(sample_rate);
                }
            }
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        if let Some((pre, field)) = name.split_once('_') {
            let idx = match pre {
                "wt1" => Some(0),
                "wt2" => Some(1),
                _ => None,
            };
            if let Some(i) = idx {
                let o = &mut self.osc[i];
                match field {
                    "table" => o.table = (v as usize).min(TABLE_NAMES.len() - 1),
                    "pos" => o.pos = v,
                    "pos_env" => o.pos_env = v,
                    "pos_lfo" => o.pos_lfo = v,
                    "oct" => o.tuning[0] = v,
                    "semi" => o.tuning[1] = v,
                    "fine" => o.tuning[2] = v,
                    "level" => o.level = v,
                    "unison" => o.unison = v as usize + 1,
                    "detune" => o.detune = v,
                    "spread" => o.spread = v,
                    _ => return,
                }
                o.update();
                return;
            }
        }
        match name {
            "flt_mode" => {
                self.flt_mode = match v as usize {
                    1 => SvfMode::Highpass,
                    2 => SvfMode::Bandpass,
                    _ => SvfMode::Lowpass,
                };
                self.configure();
            }
            "flt_slope" => self.flt_24 = v >= 0.5,
            "flt_cutoff" => self.cutoff = v,
            "flt_reso" => {
                self.reso = v;
                self.configure();
            }
            "flt_env" => self.flt_env = v,
            "flt_key" => self.flt_key = v,
            "flt_vel" => self.flt_vel = v,
            "fenv_attack" => self.fenv[0] = v,
            "fenv_decay" => self.fenv[1] = v,
            "fenv_sustain" => self.fenv[2] = v,
            "fenv_release" => self.fenv[3] = v,
            "menv_attack" => self.menv[0] = v,
            "menv_decay" => self.menv[1] = v,
            "menv_sustain" => self.menv[2] = v,
            "menv_release" => self.menv[3] = v,
            "aenv_attack" => self.aenv[0] = v,
            "aenv_decay" => self.aenv[1] = v,
            "aenv_sustain" => self.aenv[2] = v,
            "aenv_release" => self.aenv[3] = v,
            "lfo_shape" => self.lfo.set_shape(lfo_shape(v)),
            "lfo_rate" => self.lfo.set_rate(v),
            "lfo_pitch" => self.lfo_pitch = v,
            "lfo_cutoff" => self.lfo_cutoff = v,
            "lfo_retrig" => self.lfo_retrig = v >= 0.5,
            "voice_mode" => {
                let mono = v as u8 != 0;
                if mono != self.mono() {
                    self.all_notes_off();
                }
                self.mode = v as u8;
            }
            "voice_glide" => self.glide = v,
            "voice_vel" => self.vel_sens = v,
            "amp" => self.amp = v,
            _ => {}
        }
        if name.starts_with("fenv_") || name.starts_with("menv_") || name.starts_with("aenv_") {
            self.configure();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = note.key.min(127);
        let vel = note.velocity.clamp(0.0, 1.0);
        if self.lfo_retrig {
            self.lfo.reset(0.0);
        }
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
        let unison = [self.osc[0].unison, self.osc[1].unison];
        let mut rng = self.rng;
        let v = &mut self.voices[idx];
        v.tag = VoiceTag {
            active: true,
            released: false,
            id: note.voice_id,
            order: self.order,
        };
        v.vel = vel;
        v.pitch = start;
        v.target = pitch;
        Self::reset_voice(v, unison, &mut rng);
        Self::trigger(v);
        self.rng = rng;
    }

    fn note_off(&mut self, voice_id: u64) {
        if self.mono() {
            if !self.stack.remove(voice_id) || self.voices[0].tag.id != voice_id {
                return;
            }
            if let Some((key, id, vel)) = self.stack.last() {
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
        let bank = self.bank;
        let data = &bank.data[..];
        let nyq = 0.45 * sr;
        let filter_bypass = self.flt_mode == SvfMode::Lowpass
            && self.cutoff >= 19_999.0
            && self.flt_env <= 0.0
            && self.lfo_cutoff <= 0.0;
        let glide = glide_coef(self.glide, CTRL, sr);
        let bend = self.pitch_offset / 100.0;
        let mut start = 0;
        while start < n {
            let len = CTRL.min(n - start);
            let lfo = self.lfo.next_sample();
            for _ in 1..len {
                self.lfo.next_sample();
            }
            let pitch_mod = lfo * self.lfo_pitch * self.lfo_pitch * 12.0;
            for v in self.voices.iter_mut().filter(|v| v.tag.active) {
                v.pitch += (v.target - v.pitch) * glide;
                let base = v.pitch + bend + pitch_mod;
                for (k, cfg) in self.osc.iter().enumerate() {
                    if cfg.level <= 0.0 {
                        continue;
                    }
                    let f = midi_to_hz(base + cfg.transpose);
                    for (inc, off) in v.inc[k].iter_mut().zip(cfg.offsets).take(cfg.unison) {
                        *inc = f * semis(off) / sr;
                    }
                    let pos = (cfg.pos + cfg.pos_env * v.menv_level + cfg.pos_lfo * lfo * 0.5).clamp(0.0, 1.0);
                    let frame = pos * (FRAMES - 1) as f32;
                    let fa = (frame as usize).min(FRAMES - 1);
                    let fb = (fa + 1).min(FRAMES - 1);
                    // Mip scelto sulla voce di unison più acuta.
                    let top = f * semis(cfg.offsets[cfg.unison - 1].max(0.0));
                    let mip = mip_for(top, sr);
                    v.readers[k] = Reader {
                        a: bank.offset(cfg.table, fa, mip),
                        b: bank.offset(cfg.table, fb, mip),
                        len: mip_len(mip),
                        mix: frame - fa as f32,
                    };
                }
                if !filter_bypass {
                    let oct = self.flt_key * (v.pitch - 60.0) / 12.0
                        + self.flt_env * 5.0 * v.fenv_level
                        + self.flt_vel * 2.0 * (v.vel - 1.0)
                        + lfo * self.lfo_cutoff * 4.0;
                    let fc = (self.cutoff * oct.exp2()).clamp(20.0, nyq);
                    for ch in &mut v.svf {
                        ch[0].set_cutoff(fc);
                        if self.flt_24 {
                            ch[1].set_cutoff(fc);
                        }
                    }
                }
                let gain = self.amp * (1.0 - self.vel_sens * (1.0 - v.vel)) * VOICE_GAIN;
                for i in start..start + len {
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    for (k, cfg) in self.osc.iter().enumerate() {
                        if cfg.level <= 0.0 {
                            continue;
                        }
                        let rd = v.readers[k];
                        for ((p, inc), (gl, gr)) in v.phase[k].iter_mut().zip(v.inc[k]).zip(cfg.gains).take(cfg.unison)
                        {
                            let x = rd.read(data, *p);
                            *p += inc;
                            *p -= p.floor();
                            l += x * gl;
                            r += x * gr;
                        }
                    }
                    v.fenv_level = v.fenv.next_sample();
                    v.menv_level = v.menv.next_sample();
                    if !filter_bypass {
                        l = v.svf[0][0].process(l);
                        r = v.svf[1][0].process(r);
                        if self.flt_24 {
                            l = v.svf[0][1].process(l);
                            r = v.svf[1][1].process(r);
                        }
                    }
                    let e = v.aenv.next_sample() * gain;
                    left[i] += l * e;
                    right[i] += r * e;
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

/// Semitoni → rapporto di frequenza.
#[inline]
fn semis(st: f32) -> f32 {
    (st / 12.0).exp2()
}
