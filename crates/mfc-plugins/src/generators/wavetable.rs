//! Wavetable: oscillatore a tabelle d'onda con morphing tra 8 frame, inviluppo sulla
//! posizione, unisono desintonizzato, ADSR e filtro.
//!
//! Le tabelle sono generate in modo additivo (una volta, condivise tra le istanze) con più
//! livelli di banda limitata: a ogni nota si legge il livello con le armoniche sotto Nyquist,
//! quindi niente aliasing anche sulle note alte.

use std::sync::OnceLock;

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, midi_to_hz, pan_gains, Adsr, ExpDecay, Rng, Svf};

/// Nomi dei banchi di tabelle.
const TABLES: &[&str] = &["basic", "pwm", "harmonic", "formant", "digital"];

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::WAVETABLE,
    name: "Wavetable",
    role: PluginRole::Generator,
    description: "Oscillatore wavetable con 5 banchi da 8 frame (basic: sine→tri→saw→square, pwm, \
                  harmonic, formant, digital), morphing con inviluppo, unisono, ADSR e filtro.",
    params: &[
        ParamDesc::choice("table", "Table", TABLES, 0),
        ParamDesc::new("position", "Position", 0.0, 1.0, 0.3, ""),
        ParamDesc::new("pos_env", "Position env", -1.0, 1.0, 0.0, ""),
        ParamDesc::new("pos_decay", "Position decay", 0.01, 10.0, 0.5, "s"),
        ParamDesc::choice("unison", "Unison", &["1", "2", "3", "4", "5"], 0),
        ParamDesc::new("detune", "Detune", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("spread", "Stereo spread", 0.0, 1.0, 0.7, ""),
        ParamDesc::new("attack", "Attack", 0.0, 10.0, 0.005, "s"),
        ParamDesc::new("decay", "Decay", 0.0, 10.0, 0.5, "s"),
        ParamDesc::new("sustain", "Sustain", 0.0, 1.0, 0.7, ""),
        ParamDesc::new("release", "Release", 0.0, 10.0, 0.25, "s"),
        ParamDesc::new("cutoff", "Filter cutoff", 20.0, 20_000.0, 20_000.0, "Hz"),
        ParamDesc::new("resonance", "Filter resonance", 0.0, 1.0, 0.1, ""),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.5, ""),
    ],
};

/// Campioni per frame (potenza di 2).
const SIZE: usize = 1024;
const FRAMES: usize = 8;
/// Armoniche massime per livello di banda limitata (ogni livello dimezza).
const MIP_HARMONICS: [usize; 8] = [256, 128, 64, 32, 16, 8, 4, 2];
const MIPS: usize = MIP_HARMONICS.len();
const VOICES: usize = 12;
const MAX_UNISON: usize = 5;

/// Tutte le tabelle: `[banco][frame][livello][campione]`, appiattite.
struct Bank {
    data: Vec<f32>,
}

impl Bank {
    fn index(table: usize, frame: usize, mip: usize) -> usize {
        ((table * FRAMES + frame) * MIPS + mip) * SIZE
    }

    fn frame(&self, table: usize, frame: usize, mip: usize) -> &[f32] {
        let i = Self::index(table, frame, mip);
        &self.data[i..i + SIZE]
    }
}

/// Ampiezza dell'armonica `h` (1 = fondamentale) nel frame `f` del banco `table`.
fn harmonic(table: usize, f: usize, h: usize, rng_val: f32) -> f32 {
    let x = f as f32 / (FRAMES - 1) as f32;
    let hf = h as f32;
    match table {
        // Sine → triangle → saw → square, con interpolazione tra le forme vicine.
        0 => {
            let sine = if h == 1 { 1.0 } else { 0.0 };
            let tri = if h % 2 == 1 {
                let s = if (h / 2).is_multiple_of(2) { 1.0 } else { -1.0 };
                s * 8.0 / (std::f32::consts::PI.powi(2) * hf * hf)
            } else {
                0.0
            };
            let saw = 2.0 / (std::f32::consts::PI * hf);
            let square = if h % 2 == 1 {
                4.0 / (std::f32::consts::PI * hf)
            } else {
                0.0
            };
            let shapes = [sine, tri, saw, square];
            let t = x * 3.0;
            let k = (t.floor() as usize).min(2);
            let w = t - k as f32;
            shapes[k] * (1.0 - w) + shapes[k + 1] * w
        }
        // Onda rettangolare con duty da 50% a ~3%.
        1 => {
            let duty = 0.5 - 0.47 * x;
            2.0 / (std::f32::consts::PI * hf) * (std::f32::consts::PI * hf * duty).sin()
        }
        // Serie armonica che si apre: da sola fondamentale a spettro ricco con armoniche pari.
        2 => {
            let n = 1.0 + x * 40.0;
            if hf <= n {
                1.0 / hf.sqrt() * (1.0 - (hf - 1.0) / n).max(0.0)
            } else {
                0.0
            }
        }
        // Saw filtrata da due formanti (vocali scure → chiare).
        3 => {
            let f1 = 3.0 + 9.0 * x;
            let f2 = 8.0 + 22.0 * x;
            let peak = |c: f32| (-((hf - c) / (0.25 * c)).powi(2)).exp();
            (1.0 / hf) * (0.15 + peak(f1) + 0.7 * peak(f2)) * 2.0
        }
        // Spettri pseudo-casuali sempre più densi (suoni digitali).
        _ => {
            let density = 0.15 + 0.8 * x;
            if rng_val < density {
                (1.0 / hf.powf(0.7)) * (0.5 + rng_val)
            } else {
                0.0
            }
        }
    }
}

fn bank() -> &'static Bank {
    static BANK: OnceLock<Bank> = OnceLock::new();
    BANK.get_or_init(|| {
        let mut data = vec![0.0f32; TABLES.len() * FRAMES * MIPS * SIZE];
        // Seno tabulato: le armoniche si leggono a passo h (niente sin() nel ciclo interno).
        let sine: Vec<f32> = (0..SIZE)
            .map(|i| (std::f32::consts::TAU * i as f32 / SIZE as f32).sin())
            .collect();
        let mut amps = vec![0.0f32; MIP_HARMONICS[0] + 1];
        for table in 0..TABLES.len() {
            for f in 0..FRAMES {
                let mut rng = Rng::new(0xD1_6170 + f as u32 * 7919);
                for (h, a) in amps.iter_mut().enumerate().skip(1) {
                    *a = harmonic(table, f, h, rng.next_f32());
                }
                for (mip, &limit) in MIP_HARMONICS.iter().enumerate() {
                    let start = Bank::index(table, f, mip);
                    let out = &mut data[start..start + SIZE];
                    for (h, &a) in amps.iter().enumerate().take(limit + 1).skip(1) {
                        if a == 0.0 {
                            continue;
                        }
                        for (i, o) in out.iter_mut().enumerate() {
                            *o += a * sine[(i * h) % SIZE];
                        }
                    }
                    // Picco normalizzato a 1 per frame: il morphing non cambia il volume.
                    let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
                    if peak > 1e-9 {
                        out.iter_mut().for_each(|x| *x /= peak);
                    }
                }
            }
        }
        Bank { data }
    })
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    key: u8,
    fine: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    phases: [f32; MAX_UNISON],
    env: Adsr,
    pos_env: ExpDecay,
    svf: [Svf; 2],
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth wavetable polifonico.
pub struct Wavetable {
    sr: f32,
    bank: &'static Bank,
    voices: Vec<Voice>,
    order: u64,
    rng: Rng,
    pitch_offset: f32,
    table: usize,
    position: f32,
    pos_env: f32,
    pos_decay: f32,
    unison: usize,
    detune: f32,
    spread: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
    resonance: f32,
    amp: f32,
}

impl Default for Wavetable {
    fn default() -> Self {
        Self::new()
    }
}

impl Wavetable {
    pub fn new() -> Self {
        let voice = Voice {
            tag: VoiceTag::default(),
            key: 60,
            fine: 0.0,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            phases: [0.0; MAX_UNISON],
            env: Adsr::new(DEFAULT_SR),
            pos_env: ExpDecay::new(0.5, DEFAULT_SR),
            svf: [Svf::new(DEFAULT_SR); 2],
        };
        let mut s = Self {
            sr: DEFAULT_SR,
            bank: bank(),
            voices: vec![voice; VOICES],
            order: 0,
            rng: Rng::new(0x7AB1E),
            pitch_offset: 0.0,
            table: 0,
            position: 0.3,
            pos_env: 0.0,
            pos_decay: 0.5,
            unison: 1,
            detune: 0.2,
            spread: 0.7,
            attack: 0.005,
            decay: 0.5,
            sustain: 0.7,
            release: 0.25,
            cutoff: 20_000.0,
            resonance: 0.1,
            amp: 0.5,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.configure();
        s
    }

    fn configure(&mut self) {
        for v in &mut self.voices {
            v.env.set_adsr(self.attack, self.decay, self.sustain, self.release);
            v.pos_env.set_time(self.pos_decay);
            for f in &mut v.svf {
                f.set_cutoff(self.cutoff);
                f.set_resonance(self.resonance);
            }
        }
    }
}

/// Livello di banda limitata per una fondamentale `hz` al sample rate `sr`.
fn mip_for(sr: f32, hz: f32) -> usize {
    let allowed = 0.45 * sr / hz.max(1.0);
    MIP_HARMONICS
        .iter()
        .position(|&h| h as f32 <= allowed)
        .unwrap_or(MIPS - 1)
}

/// Lettura con interpolazione lineare; `phase` in 0..1.
#[inline]
fn read(t: &[f32], phase: f32) -> f32 {
    let x = phase * SIZE as f32;
    let i = x as usize;
    let frac = x - i as f32;
    let a = t[i & (SIZE - 1)];
    let b = t[(i + 1) & (SIZE - 1)];
    a + (b - a) * frac
}

impl Generator for Wavetable {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        for v in &mut self.voices {
            v.env.set_sample_rate(sample_rate);
            v.pos_env.set_sample_rate(sample_rate);
            for f in &mut v.svf {
                f.set_sample_rate(sample_rate);
            }
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "table" => self.table = (v as usize).min(TABLES.len() - 1),
            "position" => self.position = v,
            "pos_env" => self.pos_env = v,
            "unison" => self.unison = (v as usize + 1).min(MAX_UNISON),
            "detune" => self.detune = v,
            "spread" => self.spread = v,
            "amp" => self.amp = v,
            "pos_decay" => self.pos_decay = v,
            "attack" => self.attack = v,
            "decay" => self.decay = v,
            "sustain" => self.sustain = v,
            "release" => self.release = v,
            "cutoff" => self.cutoff = v,
            "resonance" => self.resonance = v,
            _ => return,
        }
        self.configure();
    }

    fn note_on(&mut self, note: NoteOn) {
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let (gl, gr) = balance_gains(note.pan);
        let unison = self.unison;
        let v = &mut self.voices[idx];
        v.tag = VoiceTag {
            active: true,
            released: false,
            id: note.voice_id,
            order: self.order,
        };
        v.key = note.key.min(127);
        v.fine = if note.fine_pitch_cents.is_finite() {
            note.fine_pitch_cents
        } else {
            0.0
        };
        v.vel = note.velocity.clamp(0.0, 1.0);
        v.gl = gl;
        v.gr = gr;
        for (k, p) in v.phases.iter_mut().enumerate() {
            // Una sola voce: fase fissa (attacco ripetibile); in unisono fasi sparse.
            *p = if unison == 1 || k == 0 {
                0.0
            } else {
                self.rng.next_f32()
            };
        }
        v.env.note_on();
        v.pos_env.trigger(1.0);
        for f in &mut v.svf {
            f.reset();
        }
    }

    fn note_off(&mut self, voice_id: u64) {
        for v in &mut self.voices {
            if v.tag.active && !v.tag.released && v.tag.id == voice_id {
                v.tag.released = true;
                v.env.note_off();
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
                v.env.note_off();
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
        let filter_on = self.cutoff < 19_999.0;
        let unison = self.unison;
        // Desintonia simmetrica (±50 cent a detune massimo) e pan delle voci di unisono.
        let mut ratios = [1.0f32; MAX_UNISON];
        let mut pans = [(1.0f32, 1.0f32); MAX_UNISON];
        for k in 0..unison {
            let spread = if unison == 1 {
                0.0
            } else {
                k as f32 / (unison - 1) as f32 * 2.0 - 1.0
            };
            ratios[k] = 2f32.powf(spread * self.detune * 50.0 / 1200.0);
            let (l, r) = pan_gains(spread * self.spread);
            // Normalizza la somma dell'unisono (potenza costante).
            let norm = std::f32::consts::SQRT_2 / (unison as f32).sqrt();
            pans[k] = (l * norm, r * norm);
        }
        let bank = self.bank;
        let table = self.table;
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let hz = midi_to_hz(v.key as f32 + (v.fine + self.pitch_offset) / 100.0);
            let incs: [f32; MAX_UNISON] = std::array::from_fn(|k| hz * ratios[k] / self.sr);
            // Livello scelto sulla voce di unisono più acuta.
            let mip = mip_for(self.sr, hz * ratios[unison - 1]);
            let gain = v.vel * self.amp;
            for i in 0..n {
                let pos = (self.position + self.pos_env * v.pos_env.next_sample()).clamp(0.0, 1.0);
                let fpos = pos * (FRAMES - 1) as f32;
                let f0 = (fpos as usize).min(FRAMES - 2);
                let w = fpos - f0 as f32;
                let ta = bank.frame(table, f0, mip);
                let tb = bank.frame(table, f0 + 1, mip);
                let (mut l, mut r) = (0.0, 0.0);
                for k in 0..unison {
                    let ph = v.phases[k];
                    let x = read(ta, ph) * (1.0 - w) + read(tb, ph) * w;
                    l += x * pans[k].0;
                    r += x * pans[k].1;
                    let mut np = ph + incs[k];
                    if np >= 1.0 {
                        np -= 1.0;
                    }
                    v.phases[k] = np;
                }
                if filter_on {
                    l = v.svf[0].process(l);
                    r = v.svf[1].process(r);
                }
                let e = v.env.next_sample() * gain;
                left[i] += l * e * v.gl;
                right[i] += r * e * v.gr;
                if v.env.is_idle() {
                    v.tag.active = false;
                    break;
                }
            }
        }
    }

    fn active_voices(&self) -> usize {
        count_active(&self.voices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_levels_stay_below_nyquist() {
        let w = Wavetable::new();
        for key in [0u8, 36, 60, 84, 108, 127] {
            let hz = midi_to_hz(key as f32);
            let mip = mip_for(w.sr, hz);
            if mip < MIPS - 1 {
                assert!(MIP_HARMONICS[mip] as f32 * hz <= 0.45 * w.sr + 1.0, "key {key}");
            }
        }
        // Frame normalizzati.
        let b = bank();
        for t in 0..TABLES.len() {
            for f in 0..FRAMES {
                let peak = b.frame(t, f, 0).iter().fold(0f32, |m, x| m.max(x.abs()));
                assert!((peak - 1.0).abs() < 1e-3, "tabella {t} frame {f}: {peak}");
            }
        }
    }
}
