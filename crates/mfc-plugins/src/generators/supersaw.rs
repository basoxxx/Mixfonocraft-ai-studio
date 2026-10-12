//! Supersaw: sette denti di sega desintonizzati con spread stereo, ADSR e filtro.

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, midi_to_hz, pan_gains, Adsr, Oscillator, Rng, Svf, Waveform};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::SUPERSAW,
    name: "Supersaw",
    role: PluginRole::Generator,
    description: "Sette saw desintonizzate (stile JP-8000) con spread stereo, ADSR e filtro passa-basso.",
    params: &[
        ParamDesc::new("detune", "Detune", 0.0, 1.0, 0.35, ""),
        ParamDesc::new("mix", "Mix (side saws)", 0.0, 1.0, 0.6, ""),
        ParamDesc::new("spread", "Stereo spread", 0.0, 1.0, 0.8, ""),
        ParamDesc::new("attack", "Attack", 0.0, 10.0, 0.01, "s"),
        ParamDesc::new("decay", "Decay", 0.0, 10.0, 0.4, "s"),
        ParamDesc::new("sustain", "Sustain", 0.0, 1.0, 0.8, ""),
        ParamDesc::new("release", "Release", 0.0, 10.0, 0.3, "s"),
        ParamDesc::new("cutoff", "Filter cutoff", 20.0, 20_000.0, 12_000.0, "Hz"),
        ParamDesc::new("resonance", "Filter resonance", 0.0, 1.0, 0.1, ""),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.6, ""),
    ],
};

const VOICES: usize = 16;
const SAWS: usize = 7;
/// Scostamenti relativi di frequenza a detune massimo (il centro è l'indice 3).
const OFFSETS: [f32; SAWS] = [-0.110, -0.063, -0.0195, 0.0, 0.0199, 0.0622, 0.1075];

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    key: u8,
    fine: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    saws: [Oscillator; SAWS],
    env: Adsr,
    svf: [Svf; 2],
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth supersaw polifonico.
pub struct Supersaw {
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    rng: Rng,
    pitch_offset: f32,
    detune: f32,
    mix: f32,
    spread: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
    resonance: f32,
    amp: f32,
    /// Guadagni (L, R) per ciascuna saw, già normalizzati.
    gains: [(f32, f32); SAWS],
}

impl Default for Supersaw {
    fn default() -> Self {
        Self::new()
    }
}

impl Supersaw {
    pub fn new() -> Self {
        let mut saw = Oscillator::new(DEFAULT_SR);
        saw.set_waveform(Waveform::Saw);
        let voice = Voice {
            tag: VoiceTag::default(),
            key: 60,
            fine: 0.0,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            saws: [saw; SAWS],
            env: Adsr::new(DEFAULT_SR),
            svf: [Svf::new(DEFAULT_SR); 2],
        };
        let mut s = Self {
            sr: DEFAULT_SR,
            voices: vec![voice; VOICES],
            order: 0,
            rng: Rng::new(0x5A5A),
            pitch_offset: 0.0,
            detune: 0.35,
            mix: 0.6,
            spread: 0.8,
            attack: 0.01,
            decay: 0.4,
            sustain: 0.8,
            release: 0.3,
            cutoff: 12_000.0,
            resonance: 0.1,
            amp: 0.6,
            gains: [(0.0, 0.0); SAWS],
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.configure();
        s
    }

    fn configure(&mut self) {
        let center = 1.0 - 0.55 * self.mix;
        let side = 0.05 + 1.2 * self.mix - 0.7 * self.mix * self.mix;
        let norm = 0.5 / (center * center + 6.0 * side * side).sqrt();
        for (k, g) in self.gains.iter_mut().enumerate() {
            if k == SAWS / 2 {
                let (l, r) = pan_gains(0.0);
                *g = (l * center * norm, r * center * norm);
            } else {
                let dir = if k % 2 == 0 { -1.0 } else { 1.0 };
                let pan = dir * self.spread * (OFFSETS[k].abs() / 0.11).min(1.0);
                let (l, r) = pan_gains(pan);
                *g = (l * side * norm, r * side * norm);
            }
        }
        for v in &mut self.voices {
            v.env.set_adsr(self.attack, self.decay, self.sustain, self.release);
            for f in &mut v.svf {
                f.set_cutoff(self.cutoff);
                f.set_resonance(self.resonance);
            }
        }
    }
}

impl Generator for Supersaw {
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
            for f in &mut v.svf {
                f.set_sample_rate(sample_rate);
            }
            for o in &mut v.saws {
                o.set_sample_rate(sample_rate);
            }
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else { return };
        match name {
            "detune" => {
                self.detune = v;
                return;
            }
            "amp" => {
                self.amp = v;
                return;
            }
            "mix" => self.mix = v,
            "spread" => self.spread = v,
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
        let v = &mut self.voices[idx];
        v.tag = VoiceTag { active: true, released: false, id: note.voice_id, order: self.order };
        v.key = note.key.min(127);
        v.fine = if note.fine_pitch_cents.is_finite() { note.fine_pitch_cents } else { 0.0 };
        v.vel = note.velocity.clamp(0.0, 1.0);
        v.gl = gl;
        v.gr = gr;
        // Fasi casuali: carattere "free running" tipico della supersaw.
        for o in &mut v.saws {
            o.reset(self.rng.next_f32());
        }
        v.env.note_on();
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
        let gains = self.gains;
        let amount = self.detune * self.detune.sqrt();
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let hz = midi_to_hz(v.key as f32 + (v.fine + self.pitch_offset) / 100.0);
            for (o, off) in v.saws.iter_mut().zip(OFFSETS) {
                o.set_frequency(hz * (1.0 + off * amount));
            }
            let gain = v.vel * self.amp;
            for i in 0..n {
                let (mut l, mut r) = (0.0, 0.0);
                for (o, (gl, gr)) in v.saws.iter_mut().zip(gains) {
                    let x = o.next();
                    l += x * gl;
                    r += x * gr;
                }
                if filter_on {
                    l = v.svf[0].process(l);
                    r = v.svf[1].process(r);
                }
                let e = v.env.next() * gain;
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
