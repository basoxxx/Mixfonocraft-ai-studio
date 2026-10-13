//! FM2: sintesi FM a due operatori (modulatore → portante) con feedback, inviluppo
//! dell'indice di modulazione e ADSR d'ampiezza. Campane, bassi metallici, e-piano, pluck.

use std::f32::consts::TAU;

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, midi_to_hz, Adsr};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::FM,
    name: "FM2",
    role: PluginRole::Generator,
    description: "Sintesi FM a due operatori: rapporto di frequenza, indice con inviluppo proprio, \
                  feedback del modulatore e ADSR d'ampiezza.",
    params: &[
        ParamDesc::new("ratio", "Mod ratio", 0.25, 16.0, 2.0, "×"),
        ParamDesc::new("index", "Mod index", 0.0, 20.0, 3.0, ""),
        ParamDesc::new("index_decay", "Index decay", 0.005, 5.0, 0.3, "s"),
        ParamDesc::new("index_sustain", "Index sustain", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("feedback", "Feedback", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("attack", "Attack", 0.0, 10.0, 0.002, "s"),
        ParamDesc::new("decay", "Decay", 0.0, 10.0, 0.6, "s"),
        ParamDesc::new("sustain", "Sustain", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("release", "Release", 0.0, 10.0, 0.3, "s"),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.5, ""),
    ],
};

const VOICES: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    key: u8,
    fine: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    phase_c: f32,
    phase_m: f32,
    prev_m: f32,
    env: Adsr,
    index_env: Adsr,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth FM polifonico.
pub struct Fm {
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    pitch_offset: f32,
    ratio: f32,
    index: f32,
    index_decay: f32,
    index_sustain: f32,
    feedback: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    amp: f32,
}

impl Default for Fm {
    fn default() -> Self {
        Self::new()
    }
}

impl Fm {
    pub fn new() -> Self {
        let voice = Voice {
            tag: VoiceTag::default(),
            key: 60,
            fine: 0.0,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            phase_c: 0.0,
            phase_m: 0.0,
            prev_m: 0.0,
            env: Adsr::new(DEFAULT_SR),
            index_env: Adsr::new(DEFAULT_SR),
        };
        let mut s = Self {
            sr: DEFAULT_SR,
            voices: vec![voice; VOICES],
            order: 0,
            pitch_offset: 0.0,
            ratio: 2.0,
            index: 3.0,
            index_decay: 0.3,
            index_sustain: 0.2,
            feedback: 0.0,
            attack: 0.002,
            decay: 0.6,
            sustain: 0.5,
            release: 0.3,
            amp: 0.5,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.configure();
        s
    }

    fn configure(&mut self) {
        for v in &mut self.voices {
            v.env.set_adsr(self.attack, self.decay, self.sustain, self.release);
            v.index_env
                .set_adsr(0.0, self.index_decay, self.index_sustain, self.release);
        }
    }
}

impl Generator for Fm {
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
            v.index_env.set_sample_rate(sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "ratio" => self.ratio = v,
            "index" => self.index = v,
            "feedback" => self.feedback = v,
            "amp" => self.amp = v,
            "index_decay" => self.index_decay = v,
            "index_sustain" => self.index_sustain = v,
            "attack" => self.attack = v,
            "decay" => self.decay = v,
            "sustain" => self.sustain = v,
            "release" => self.release = v,
            _ => return,
        }
        self.configure();
    }

    fn note_on(&mut self, note: NoteOn) {
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let (gl, gr) = balance_gains(note.pan);
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
        v.phase_c = 0.0;
        v.phase_m = 0.0;
        v.prev_m = 0.0;
        v.env.note_on();
        v.index_env.note_on();
    }

    fn note_off(&mut self, voice_id: u64) {
        for v in &mut self.voices {
            if v.tag.active && !v.tag.released && v.tag.id == voice_id {
                v.tag.released = true;
                v.env.note_off();
                v.index_env.note_off();
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
                v.env.note_off();
                v.index_env.note_off();
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
        let nyquist = 0.5 * self.sr;
        // Feedback limitato: oltre ~1.2 rad il modulatore degenera in rumore.
        let fb = self.feedback * 1.2;
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let hz = midi_to_hz(v.key as f32 + (v.fine + self.pitch_offset) / 100.0);
            let inc_c = hz / self.sr;
            // Modulatore sopra Nyquist: solo aliasing, lo si spegne.
            let mod_hz = hz * self.ratio;
            let inc_m = if mod_hz < nyquist { mod_hz / self.sr } else { 0.0 };
            // L'indice scala con la velocity, come nei synth FM classici.
            let index = self.index * (0.4 + 0.6 * v.vel);
            let gain = v.vel * self.amp;
            for i in 0..n {
                let m = (TAU * v.phase_m + fb * v.prev_m).sin();
                v.prev_m = m;
                let depth = index * v.index_env.next_sample();
                let c = (TAU * v.phase_c + depth * m).sin();
                let e = v.env.next_sample() * gain;
                left[i] += c * e * v.gl;
                right[i] += c * e * v.gr;
                v.phase_c += inc_c;
                if v.phase_c >= 1.0 {
                    v.phase_c -= 1.0;
                }
                v.phase_m += inc_m;
                if v.phase_m >= 1.0 {
                    v.phase_m -= 1.0;
                }
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
