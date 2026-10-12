//! DrumSynth: batteria sintetica (kick, snare, clap, hi-hat, tom, rim, cowbell) da rumore e oscillatori.

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR, OFF_ON};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, semitones_to_ratio, soft_clip, time_coef, ExpDecay, OnePole, Rng, Svf, SvfMode};
use std::f32::consts::TAU;

/// Suoni disponibili (valore del parametro `type`).
pub const TYPES: &[&str] = &["kick", "snare", "clap", "closed_hat", "open_hat", "tom", "rim", "cowbell"];

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::DRUMSYNTH,
    name: "DrumSynth",
    role: PluginRole::Generator,
    description: "Percussioni sintetiche: kick, snare, clap, hi-hat chiuso/aperto, tom, rim, cowbell.",
    params: &[
        ParamDesc::choice("type", "Type", TYPES, 0),
        ParamDesc::new("tone", "Tone", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("decay", "Decay", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("snappy", "Snappy", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("pitch", "Pitch", -24.0, 24.0, 0.0, "st"),
        ParamDesc::choice("key_track", "Key tracking (rel. C5 = 60)", OFF_ON, 0),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 1.0, ""),
    ],
};

const VOICES: usize = 16;
/// Frequenze dei sei oscillatori quadri "metallici" (stile 808).
const METAL: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Tom,
    Rim,
    Cowbell,
}

impl Kind {
    fn from_value(v: f32) -> Self {
        match v as i32 {
            1 => Self::Snare,
            2 => Self::Clap,
            3 => Self::ClosedHat,
            4 => Self::OpenHat,
            5 => Self::Tom,
            6 => Self::Rim,
            7 => Self::Cowbell,
            _ => Self::Kick,
        }
    }
}

#[inline]
fn square(phase: f32) -> f32 {
    if phase < 0.5 {
        1.0
    } else {
        -1.0
    }
}

#[inline]
fn advance(phase: &mut f32, inc: f32) -> f32 {
    *phase += inc;
    *phase -= phase.floor();
    *phase
}

/// Configura un filtro e restituisce il suo Q.
fn bp(f: &mut Svf, mode: SvfMode, hz: f32, q: f32) -> f32 {
    f.set_mode(mode);
    f.set(hz, q);
    q
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    kind: Kind,
    t: u32,
    ratio: f32,
    tone: f32,
    snappy: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    ph: [f32; 6],
    rng: Rng,
    body: ExpDecay,
    noise: ExpDecay,
    aux: ExpDecay,
    penv: f32,
    pcoef: f32,
    /// Normalizzazione del passa-banda (1/Q).
    bp_norm: f32,
    burst_len: u32,
    f1: Svf,
    hp: OnePole,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

impl Voice {
    fn new(sr: f32, seed: u32) -> Self {
        Self {
            tag: VoiceTag::default(),
            kind: Kind::Kick,
            t: 0,
            ratio: 1.0,
            tone: 0.5,
            snappy: 0.5,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            ph: [0.0; 6],
            rng: Rng::new(seed),
            body: ExpDecay::new(0.3, sr),
            noise: ExpDecay::new(0.2, sr),
            aux: ExpDecay::new(0.01, sr),
            penv: 0.0,
            pcoef: 0.0,
            bp_norm: 1.0,
            burst_len: 1,
            f1: Svf::new(sr),
            hp: OnePole::new(1000.0, sr),
        }
    }

    /// Configura inviluppi e filtri per il suono scelto.
    fn setup(&mut self, sr: f32, decay_mul: f32) {
        let (r, tone, dm) = (self.ratio, self.tone, decay_mul);
        for e in [&mut self.body, &mut self.noise, &mut self.aux] {
            e.set_sample_rate(sr);
            e.reset();
        }
        self.ph = [0.0; 6];
        self.penv = 1.0;
        self.t = 0;
        self.f1.set_sample_rate(sr);
        self.f1.reset();
        self.hp.reset();
        let q = match self.kind {
            Kind::Kick => {
                self.pcoef = time_coef(0.03, sr);
                self.body.set_time(0.45 * dm);
                self.body.trigger(1.0);
                self.aux.set_time(0.004);
                self.aux.trigger(0.3 + 0.7 * tone);
                self.hp.set_cutoff(2000.0, sr);
                1.0
            }
            Kind::Snare => {
                self.pcoef = time_coef(0.02, sr);
                self.body.set_time(0.15 * dm);
                self.body.trigger(1.0);
                self.noise.set_time(0.25 * dm);
                self.noise.trigger(1.0);
                bp(&mut self.f1, SvfMode::Highpass, 1200.0 + 3000.0 * tone, 0.7)
            }
            Kind::Clap => {
                self.burst_len = ((0.009 * sr) as u32).max(1);
                self.aux.set_time(0.012);
                self.aux.trigger(1.0);
                self.noise.set_time(0.3 * dm);
                bp(&mut self.f1, SvfMode::Bandpass, (900.0 + 900.0 * tone) * r.sqrt(), 1.8)
            }
            Kind::ClosedHat | Kind::OpenHat => {
                let len = if self.kind == Kind::ClosedHat { 0.06 } else { 0.5 };
                self.body.set_time(len * dm);
                self.body.trigger(1.0);
                self.hp.set_cutoff(6500.0, sr);
                bp(&mut self.f1, SvfMode::Bandpass, (8000.0 + 4000.0 * tone).min(0.45 * sr), 1.2)
            }
            Kind::Tom => {
                self.pcoef = time_coef(0.06, sr);
                self.body.set_time(0.5 * dm);
                self.body.trigger(1.0);
                self.aux.set_time(0.008);
                self.aux.trigger(0.3);
                1.0
            }
            Kind::Rim => {
                self.body.set_time(0.03 * dm);
                self.body.trigger(1.0);
                self.aux.set_time(0.012 * dm);
                self.aux.trigger(1.0);
                bp(&mut self.f1, SvfMode::Bandpass, (2500.0 + 2000.0 * tone).min(0.45 * sr), 2.5)
            }
            Kind::Cowbell => {
                self.aux.set_time(0.06);
                self.aux.trigger(0.6);
                self.body.set_time(0.4 * dm);
                self.body.trigger(0.4);
                bp(&mut self.f1, SvfMode::Bandpass, (800.0 * (0.8 + 0.4 * tone) * r).min(0.45 * sr), 3.0)
            }
        };
        self.bp_norm = 1.0 / q;
    }

    #[inline]
    fn tick(&mut self, dt: f32) -> f32 {
        let r = self.ratio;
        let noise = self.rng.next_bipolar();
        let out = match self.kind {
            Kind::Kick => {
                let f = (45.0 + 30.0 * self.tone) * r * (1.0 + 3.0 * self.penv);
                self.penv *= self.pcoef;
                let body = (TAU * advance(&mut self.ph[0], f * dt)).sin() * self.body.next();
                body + self.hp.highpass(noise) * self.aux.next() * 0.5
            }
            Kind::Snare => {
                let bend = 1.0 + 0.3 * self.penv;
                self.penv *= self.pcoef;
                let a = (TAU * advance(&mut self.ph[0], 185.0 * r * bend * dt)).sin();
                let b = (TAU * advance(&mut self.ph[1], 330.0 * r * bend * dt)).sin();
                let body = (a + 0.6 * b) * 0.6 * self.body.next() * (1.0 - 0.5 * self.snappy);
                let n = self.f1.process(noise) * self.noise.next() * (0.3 + 0.9 * self.snappy);
                body + 0.8 * n
            }
            Kind::Clap => {
                if self.t > 0 && self.t < 3 * self.burst_len && self.t % self.burst_len == 0 {
                    self.aux.trigger(1.0);
                }
                if self.t == 3 * self.burst_len {
                    self.noise.trigger(0.8);
                }
                let e = self.aux.next() + self.noise.next();
                self.f1.process(noise) * self.bp_norm * e * 1.6
            }
            Kind::ClosedHat | Kind::OpenHat => {
                let k = r * (0.8 + 0.4 * self.tone);
                let mut metal = 0.0;
                for (ph, f) in self.ph.iter_mut().zip(METAL) {
                    metal += square(advance(ph, f * k * dt));
                }
                let x = metal / 6.0 * (1.0 - 0.5 * self.snappy) + noise * 0.5 * self.snappy;
                let band = self.f1.process(x) * self.bp_norm;
                self.hp.highpass(band) * self.body.next() * 1.8
            }
            Kind::Tom => {
                let f = (80.0 + 120.0 * self.tone) * r * (1.0 + 0.6 * self.penv);
                self.penv *= self.pcoef;
                let body = (TAU * advance(&mut self.ph[0], f * dt)).sin() * self.body.next();
                body + noise * self.aux.next() * 0.3
            }
            Kind::Rim => {
                let a = (TAU * advance(&mut self.ph[0], 1700.0 * r * dt)).sin() * self.body.next();
                let n = self.f1.process(noise) * self.bp_norm * self.aux.next();
                0.6 * a + 0.8 * n
            }
            Kind::Cowbell => {
                let x = 0.5 * (square(advance(&mut self.ph[0], 540.0 * r * dt)) + square(advance(&mut self.ph[1], 800.0 * r * dt)));
                let band = self.f1.process(x) * self.bp_norm;
                band * (self.aux.next() + self.body.next()) * 2.0
            }
        };
        self.t = self.t.saturating_add(1);
        out
    }

    fn alive(&self, min_len: u32) -> bool {
        self.t < min_len || self.body.is_active() || self.noise.is_active() || self.aux.is_active()
    }
}

/// Drum machine sintetica polifonica (one-shot).
pub struct DrumSynth {
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    pitch_offset: f32,
    kind: Kind,
    tone: f32,
    decay: f32,
    snappy: f32,
    pitch: f32,
    key_track: bool,
    amp: f32,
}

impl Default for DrumSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl DrumSynth {
    pub fn new() -> Self {
        let voices = (0..VOICES).map(|i| Voice::new(DEFAULT_SR, 0xD0_0000 + i as u32)).collect();
        let mut s = Self {
            sr: DEFAULT_SR,
            voices,
            order: 0,
            pitch_offset: 0.0,
            kind: Kind::Kick,
            tone: 0.5,
            decay: 0.5,
            snappy: 0.5,
            pitch: 0.0,
            key_track: false,
            amp: 1.0,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s
    }
}

impl Generator for DrumSynth {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sr = sample_rate;
            for v in &mut self.voices {
                v.tag.active = false;
            }
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else { return };
        match name {
            "type" => self.kind = Kind::from_value(v),
            "tone" => self.tone = v,
            "decay" => self.decay = v,
            "snappy" => self.snappy = v,
            "pitch" => self.pitch = v,
            "key_track" => self.key_track = v >= 0.5,
            "amp" => self.amp = v,
            _ => {}
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let track = if self.key_track { note.key as f32 - 60.0 } else { 0.0 };
        let fine = if note.fine_pitch_cents.is_finite() { note.fine_pitch_cents } else { 0.0 };
        let st = (track + self.pitch + (fine + self.pitch_offset) / 100.0).clamp(-48.0, 48.0);
        let (gl, gr) = balance_gains(note.pan);
        let decay_mul = 2f32.powf((self.decay - 0.5) * 4.0);
        let v = &mut self.voices[idx];
        v.tag = VoiceTag { active: true, released: false, id: note.voice_id, order: self.order };
        v.kind = self.kind;
        v.ratio = semitones_to_ratio(st);
        v.tone = self.tone;
        v.snappy = self.snappy;
        v.vel = note.velocity.clamp(0.0, 1.0);
        v.gl = gl;
        v.gr = gr;
        v.setup(self.sr, decay_mul);
    }

    fn note_off(&mut self, _voice_id: u64) {}

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
                v.body.set_time(0.01);
                v.noise.set_time(0.01);
                v.aux.set_time(0.01);
                v.t = u32::MAX;
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
        let dt = 1.0 / self.sr;
        let min_len = (0.06 * self.sr) as u32;
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let gain = v.vel * self.amp;
            for i in 0..n {
                let y = soft_clip(v.tick(dt)) * gain;
                left[i] += y * v.gl;
                right[i] += y * v.gr;
                if !v.alive(min_len) {
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
