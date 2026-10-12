//! Kick in stile 808: seno con inviluppo di pitch, click, distorsione e tono.

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR, OFF_ON};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, semitones_to_ratio, soft_clip, time_coef, ExpDecay, OnePole, Rng};
use std::f32::consts::TAU;

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::KICK,
    name: "Kick",
    role: PluginRole::Generator,
    description: "Cassa sintetica stile 808: sweep di pitch, decadimento, click e saturazione.",
    params: &[
        ParamDesc::new("start_freq", "Start frequency", 40.0, 2000.0, 280.0, "Hz"),
        ParamDesc::new("end_freq", "End frequency", 20.0, 200.0, 48.0, "Hz"),
        ParamDesc::new("pitch_decay", "Pitch decay", 0.001, 1.0, 0.045, "s"),
        ParamDesc::new("amp_decay", "Amp decay", 0.02, 4.0, 0.6, "s"),
        ParamDesc::new("click", "Click", 0.0, 1.0, 0.4, ""),
        ParamDesc::new("distortion", "Distortion", 0.0, 1.0, 0.1, ""),
        ParamDesc::new("tone", "Tone", 0.0, 1.0, 0.7, ""),
        ParamDesc::choice("key_track", "Key tracking", OFF_ON, 0),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 1.0, ""),
    ],
};

const VOICES: usize = 8;

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    phase: f32,
    ratio: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    /// Inviluppo di pitch (1 → 0).
    penv: f32,
    amp: ExpDecay,
    click: ExpDecay,
    click_hp: OnePole,
    tone_lp: OnePole,
    rng: Rng,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Generatore di kick (one-shot: il note off viene ignorato).
pub struct Kick {
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    pitch_offset: f32,
    start_freq: f32,
    end_freq: f32,
    pitch_decay: f32,
    pitch_coef: f32,
    amp_decay: f32,
    click: f32,
    distortion: f32,
    tone: f32,
    key_track: bool,
    amp: f32,
}

impl Default for Kick {
    fn default() -> Self {
        Self::new()
    }
}

impl Kick {
    pub fn new() -> Self {
        let voice = Voice {
            tag: VoiceTag::default(),
            phase: 0.0,
            ratio: 1.0,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            penv: 0.0,
            amp: ExpDecay::new(0.6, DEFAULT_SR),
            click: ExpDecay::new(0.004, DEFAULT_SR),
            click_hp: OnePole::new(1500.0, DEFAULT_SR),
            tone_lp: OnePole::new(8000.0, DEFAULT_SR),
            rng: Rng::new(0x4B1C),
        };
        let mut s = Self {
            sr: DEFAULT_SR,
            voices: vec![voice; VOICES],
            order: 0,
            pitch_offset: 0.0,
            start_freq: 280.0,
            end_freq: 48.0,
            pitch_decay: 0.045,
            pitch_coef: 0.0,
            amp_decay: 0.6,
            click: 0.4,
            distortion: 0.1,
            tone: 0.7,
            key_track: false,
            amp: 1.0,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.configure();
        s
    }

    fn configure(&mut self) {
        self.pitch_coef = time_coef(self.pitch_decay, self.sr);
        let tone_hz = 200.0 * 100f32.powf(self.tone);
        for v in &mut self.voices {
            v.amp.set_sample_rate(self.sr);
            v.amp.set_time(self.amp_decay);
            v.click.set_sample_rate(self.sr);
            v.click_hp.set_cutoff(1500.0, self.sr);
            v.tone_lp.set_cutoff(tone_hz, self.sr);
        }
    }
}

impl Generator for Kick {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sr = sample_rate;
            self.configure();
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else { return };
        match name {
            "start_freq" => self.start_freq = v,
            "end_freq" => self.end_freq = v,
            "pitch_decay" => self.pitch_decay = v,
            "amp_decay" => self.amp_decay = v,
            "click" => self.click = v,
            "distortion" => self.distortion = v,
            "tone" => self.tone = v,
            "key_track" => self.key_track = v >= 0.5,
            "amp" => self.amp = v,
            _ => return,
        }
        if matches!(name, "pitch_decay" | "amp_decay" | "tone") {
            self.configure();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let track = if self.key_track { note.key as f32 - 60.0 } else { 0.0 };
        let fine = if note.fine_pitch_cents.is_finite() { note.fine_pitch_cents } else { 0.0 };
        let (gl, gr) = balance_gains(note.pan);
        let v = &mut self.voices[idx];
        v.tag = VoiceTag { active: true, released: false, id: note.voice_id, order: self.order };
        v.phase = 0.0;
        v.ratio = semitones_to_ratio((track + fine / 100.0).clamp(-48.0, 48.0));
        v.vel = note.velocity.clamp(0.0, 1.0);
        v.gl = gl;
        v.gr = gr;
        v.penv = 1.0;
        v.amp.trigger(1.0);
        v.click.trigger(1.0);
        v.click_hp.reset();
        v.tone_lp.reset();
    }

    fn note_off(&mut self, _voice_id: u64) {}

    fn all_notes_off(&mut self) {
        // Sfuma velocemente le voci attive.
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
                v.amp.set_time(0.01);
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
        let offset = semitones_to_ratio(self.pitch_offset / 100.0);
        let drive = 1.0 + self.distortion * 8.0;
        let makeup = 1.0 / soft_clip(drive).max(1e-3);
        let inv_sr = 1.0 / self.sr;
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let ratio = v.ratio * offset;
            let gain = v.vel * self.amp;
            for i in 0..n {
                let f = (self.end_freq + (self.start_freq - self.end_freq) * v.penv) * ratio;
                v.penv *= self.pitch_coef;
                v.phase += f * inv_sr;
                v.phase -= v.phase.floor();
                let body = (TAU * v.phase).sin();
                let click = v.click_hp.highpass(v.rng.next_bipolar()) * v.click.next() * self.click;
                let mut x = (body + click * 0.8) * v.amp.next();
                if self.distortion > 0.0 {
                    x = soft_clip(x * drive) * makeup;
                }
                let y = v.tone_lp.lowpass(x) * gain;
                left[i] += y * v.gl;
                right[i] += y * v.gr;
                if !v.amp.is_active() {
                    v.tag.active = false;
                    // Ripristina il decadimento dopo un eventuale all_notes_off.
                    v.amp.set_time(self.amp_decay);
                    break;
                }
            }
        }
    }

    fn active_voices(&self) -> usize {
        count_active(&self.voices)
    }
}
