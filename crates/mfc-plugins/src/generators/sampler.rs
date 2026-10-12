//! Sampler (canale "Sampler" di FL) e motore delle audio clip.
//!
//! Il campione è riprodotto con interpolazione di Hermite; il pitch segue la nota rispetto
//! a `root_key` (solo sampler) e compensa la differenza di sample rate.

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR, OFF_ON};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole, SharedSample};
use crate::ids;
use mfc_dsp::resample::sample_hermite;
use mfc_dsp::{balance_gains, semitones_to_ratio, Adsr, Svf};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::SAMPLER,
    name: "Sampler",
    role: PluginRole::Generator,
    description: "Riproduce un campione audio intonato sulla nota (root key), con inviluppo, loop e filtro.",
    params: &[
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 1.0, ""),
        ParamDesc::new("pitch_semitones", "Pitch", -48.0, 48.0, 0.0, "st"),
        ParamDesc::new("start", "Start", 0.0, 1.0, 0.0, ""),
        ParamDesc::choice("reverse", "Reverse", OFF_ON, 0),
        ParamDesc::choice("loop", "Loop", OFF_ON, 0),
        ParamDesc::new("env_amount", "Volume envelope", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("attack", "Attack", 0.0, 10.0, 0.0, "s"),
        ParamDesc::new("decay", "Decay", 0.0, 10.0, 0.5, "s"),
        ParamDesc::new("sustain", "Sustain", 0.0, 1.0, 1.0, ""),
        ParamDesc::new("release", "Release", 0.0, 10.0, 0.1, "s"),
        ParamDesc::new("cutoff", "Filter cutoff", 20.0, 20_000.0, 20_000.0, "Hz"),
        ParamDesc::new("resonance", "Filter resonance", 0.0, 1.0, 0.0, ""),
    ],
};

pub static AUDIO_CLIP_DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::AUDIO_CLIP,
    name: "Audio Clip",
    role: PluginRole::Generator,
    description: "Clip audio della Playlist: altezza originale, avvio a metà clip, solo anti-click.",
    params: &[
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 1.0, ""),
        ParamDesc::choice("reverse", "Reverse", OFF_ON, 0),
    ],
};

const SAMPLER_VOICES: usize = 32;
const CLIP_VOICES: usize = 8;
/// Durata del fade anti-click (s).
const DECLICK: f32 = 0.005;
/// Sopra questa soglia il filtro è escluso.
const FILTER_BYPASS_HZ: f32 = 19_999.0;

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    key: u8,
    root: u8,
    fine_cents: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    /// Posizione (frame del campione) e incremento per campione d'uscita (segno = direzione).
    pos: f64,
    step: f64,
    /// Regione di loop/riproduzione [lo, hi).
    lo: f64,
    hi: f64,
    looped: bool,
    /// Note off ignorato (campione suonato fino in fondo).
    one_shot: bool,
    env: Adsr,
    gate: Adsr,
    svf: [Svf; 2],
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

impl Voice {
    fn new(sr: f32) -> Self {
        Self {
            tag: VoiceTag::default(),
            key: 60,
            root: 60,
            fine_cents: 0.0,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            pos: 0.0,
            step: 1.0,
            lo: 0.0,
            hi: 0.0,
            looped: false,
            one_shot: false,
            env: Adsr::new(sr),
            gate: Adsr::new(sr),
            svf: [Svf::new(sr); 2],
        }
    }

    fn release(&mut self) {
        if !self.one_shot {
            self.tag.released = true;
            self.env.note_off();
            self.gate.note_off();
        }
    }
}

/// Sampler polifonico; con `clip = true` si comporta da audio clip (niente key tracking).
pub struct Sampler {
    desc: &'static PluginDescriptor,
    clip: bool,
    sample: Option<SharedSample>,
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    pitch_offset: f32,
    amp: f32,
    pitch_st: f32,
    start: f32,
    reverse: bool,
    looped: bool,
    env_amount: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
    resonance: f32,
}

impl Sampler {
    /// Sampler intonato (`mfc.sampler`). `None` = nessun campione → silenzioso.
    pub fn new(sample: Option<SharedSample>) -> Self {
        Self::build(&DESCRIPTOR, false, sample, SAMPLER_VOICES)
    }

    /// Motore per le audio clip (`mfc.audioclip`).
    pub fn new_audio_clip(sample: Option<SharedSample>) -> Self {
        Self::build(&AUDIO_CLIP_DESCRIPTOR, true, sample, CLIP_VOICES)
    }

    fn build(desc: &'static PluginDescriptor, clip: bool, sample: Option<SharedSample>, n: usize) -> Self {
        let mut s = Self {
            desc,
            clip,
            sample: sample.filter(|s| !s.is_empty()),
            sr: DEFAULT_SR,
            voices: vec![Voice::new(DEFAULT_SR); n],
            order: 0,
            pitch_offset: 0.0,
            amp: 1.0,
            pitch_st: 0.0,
            start: 0.0,
            reverse: false,
            looped: false,
            env_amount: 0.0,
            attack: 0.0,
            decay: 0.5,
            sustain: 1.0,
            release: 0.1,
            cutoff: 20_000.0,
            resonance: 0.0,
        };
        apply_defaults(desc, |n, v| s.set_param(n, v));
        s.configure_envelopes();
        s.configure_filters();
        s
    }

    /// Sostituisce il campione (non real-time: chiamare fuori dal thread audio). Ferma le voci.
    pub fn set_sample(&mut self, sample: Option<SharedSample>) {
        self.sample = sample.filter(|s| !s.is_empty());
        for v in &mut self.voices {
            v.tag.active = false;
        }
    }

    pub fn sample(&self) -> Option<&SharedSample> {
        self.sample.as_ref()
    }

    fn env_on(&self) -> bool {
        !self.clip && self.env_amount > 0.0
    }

    fn configure_envelopes(&mut self) {
        let env_on = self.env_on();
        let release = if env_on { self.release } else { DECLICK };
        for v in &mut self.voices {
            v.env.set_adsr(self.attack, self.decay, self.sustain, self.release);
            v.gate.set_adsr(0.0, 0.0, 1.0, release);
        }
    }

    fn configure_filters(&mut self) {
        for v in &mut self.voices {
            for f in &mut v.svf {
                f.set_cutoff(self.cutoff);
                f.set_resonance(self.resonance);
            }
        }
    }

    /// Incremento di posizione per la voce (pitch + compensazione sample rate).
    fn step_for(&self, key: u8, root: u8, fine_cents: f32, sample_sr: f32) -> f64 {
        let sr_ratio = sample_sr as f64 / self.sr as f64;
        if self.clip {
            return sr_ratio;
        }
        let st = key as f32 - root as f32 + self.pitch_st + (fine_cents + self.pitch_offset) / 100.0;
        semitones_to_ratio(st.clamp(-96.0, 96.0)) as f64 * sr_ratio
    }

    fn update_steps(&mut self) {
        let Some(sample_sr) = self.sample.as_ref().map(|s| s.sample_rate) else {
            return;
        };
        for i in 0..self.voices.len() {
            let v = self.voices[i];
            if v.tag.active {
                let mag = self.step_for(v.key, v.root, v.fine_cents, sample_sr);
                self.voices[i].step = mag.copysign(v.step);
            }
        }
    }
}

impl Generator for Sampler {
    fn descriptor(&self) -> &'static PluginDescriptor {
        self.desc
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        for v in &mut self.voices {
            v.env.set_sample_rate(sample_rate);
            v.gate.set_sample_rate(sample_rate);
            for f in &mut v.svf {
                f.set_sample_rate(sample_rate);
            }
        }
        self.update_steps();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(self.desc, name, value) else {
            return;
        };
        match name {
            "amp" => self.amp = v,
            "pitch_semitones" => {
                self.pitch_st = v;
                self.update_steps();
            }
            "start" => self.start = v,
            "reverse" => self.reverse = v >= 0.5,
            "loop" => self.looped = v >= 0.5,
            "env_amount" => {
                self.env_amount = v;
                self.configure_envelopes();
            }
            "attack" => self.attack = v,
            "decay" => self.decay = v,
            "sustain" => self.sustain = v,
            "release" => self.release = v,
            "cutoff" => self.cutoff = v,
            "resonance" => self.resonance = v,
            _ => return,
        }
        match name {
            "attack" | "decay" | "sustain" | "release" => self.configure_envelopes(),
            "cutoff" | "resonance" => self.configure_filters(),
            _ => {}
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let Some(sample) = self.sample.as_ref() else { return };
        let len = sample.len() as f64;
        let sample_sr = sample.sample_rate;
        let step = self.step_for(note.key, note.root_key, note.fine_pitch_cents, sample_sr);
        let looped = self.looped && !self.clip;
        let env_on = self.env_on();
        // Inizio regione: parametro start (sampler) oppure offset della clip (convertito in frame del campione).
        let skip = if self.clip {
            note.start_offset_samples as f64 * step
        } else {
            (self.start as f64 * len).floor()
        };
        let skip = skip.min(len);
        let (lo, hi, pos, dir) = if self.reverse {
            (0.0, len - skip, len - skip - 1.0, -1.0)
        } else {
            (skip, len, skip, 1.0)
        };
        if hi - lo < 1.0 {
            return;
        }
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let vel = note.velocity.clamp(0.0, 1.0);
        let (gl, gr) = balance_gains(note.pan);
        let v = &mut self.voices[idx];
        v.tag = VoiceTag {
            active: true,
            released: false,
            id: note.voice_id,
            order: self.order,
        };
        v.key = note.key;
        v.root = note.root_key;
        v.fine_cents = if note.fine_pitch_cents.is_finite() {
            note.fine_pitch_cents
        } else {
            0.0
        };
        v.vel = vel;
        v.gl = gl;
        v.gr = gr;
        v.pos = pos;
        v.step = step * dir;
        v.lo = lo;
        v.hi = hi;
        v.looped = looped;
        v.one_shot = !env_on && !looped && !self.clip;
        v.env.reset();
        v.env.note_on();
        // Le clip avviate a metà usano un breve fade-in.
        let fade_in = if self.clip && note.start_offset_samples > 0 {
            DECLICK
        } else {
            0.0
        };
        v.gate.reset();
        v.gate.set_attack(fade_in);
        v.gate.note_on();
        for f in &mut v.svf {
            f.reset();
        }
    }

    fn note_off(&mut self, voice_id: u64) {
        for v in &mut self.voices {
            if v.tag.active && !v.tag.released && v.tag.id == voice_id {
                v.release();
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.tag.active {
                // Anche le one-shot vengono sfumate.
                v.one_shot = false;
                v.release();
            }
        }
    }

    fn set_pitch_offset(&mut self, cents: f32) {
        if cents.is_finite() && cents != self.pitch_offset {
            self.pitch_offset = cents.clamp(-9600.0, 9600.0);
            self.update_steps();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let Some(sample) = self.sample.as_deref() else { return };
        let n = frames(left, right);
        let ch_l = &sample.channels[0];
        let ch_r = sample.channels.get(1).unwrap_or(ch_l);
        let env_on = self.env_on();
        let amt = self.env_amount;
        let filter_on = self.cutoff < FILTER_BYPASS_HZ;
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let base = v.vel * self.amp;
            for i in 0..n {
                if v.pos < v.lo || v.pos >= v.hi {
                    if !v.looped {
                        v.tag.active = false;
                        break;
                    }
                    let span = v.hi - v.lo;
                    v.pos = v.lo + (v.pos - v.lo).rem_euclid(span);
                }
                let g_env = v.gate.next_sample();
                let g = if env_on {
                    (1.0 - amt) * g_env + amt * v.env.next_sample()
                } else {
                    g_env
                };
                let mut l = sample_hermite(ch_l, v.pos);
                let mut r = sample_hermite(ch_r, v.pos);
                if filter_on {
                    l = v.svf[0].process(l);
                    r = v.svf[1].process(r);
                }
                let g = g * base;
                left[i] += l * g * v.gl;
                right[i] += r * g * v.gr;
                v.pos += v.step;
                let done = if env_on {
                    v.gate.is_idle() && v.env.is_idle()
                } else {
                    v.gate.is_idle()
                };
                if done {
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
