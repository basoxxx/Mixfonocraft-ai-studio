//! Chorus stereo: linee di ritardo modulate da LFO sfasati tra i canali.

use super::{apply_defaults, ensure_delay, frames, param_value, sr_ok, DEFAULT_SR, MAX_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use crate::sync::{lfo_rate, sanitize_bpm, DEFAULT_BPM, SYNC_CHOICES};
use mfc_dsp::{flush_denormal, DelayLine, Lfo, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::CHORUS,
    name: "Chorus",
    role: PluginRole::Effect,
    description: "Chorus stereo con LFO sinusoidale, feedback e spread di fase.",
    params: &[
        ParamDesc::new("rate", "Rate", 0.05, 5.0, 0.8, "Hz"),
        ParamDesc::choice("sync", "Sync", SYNC_CHOICES, 0),
        ParamDesc::new("depth", "Depth", 0.0, 10.0, 3.0, "ms"),
        ParamDesc::new("delay", "Delay", 5.0, 30.0, 12.0, "ms"),
        ParamDesc::new("feedback", "Feedback", 0.0, 0.9, 0.0, ""),
        ParamDesc::new("mix", "Mix", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("spread", "Stereo spread", 0.0, 1.0, 0.5, ""),
    ],
};

const MAX_S: f32 = 0.045;

/// Chorus stereo.
pub struct Chorus {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    lines: [DelayLine; 2],
    lfo: [Lfo; 2],
    rate: f32,
    /// Divisione di sync (indice in [`SYNC_CHOICES`], 0 = Off).
    sync: f32,
    /// Tempo del progetto (BPM).
    bpm: f32,
    depth_ms: f32,
    spread: f32,
    feedback: f32,
    delay_ms: Smoother,
    mix: Smoother,
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl Chorus {
    pub fn new() -> Self {
        let cap = super::capacity(MAX_S, MAX_SR);
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            lines: [DelayLine::new(cap), DelayLine::new(cap)],
            lfo: [Lfo::new(DEFAULT_SR); 2],
            sync: 0.0,
            bpm: DEFAULT_BPM,
            rate: 0.8,
            depth_ms: 3.0,
            spread: 0.5,
            feedback: 0.0,
            delay_ms: Smoother::new(0.05, DEFAULT_SR, 12.0),
            mix: Smoother::new(0.02, DEFAULT_SR, 0.5),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.set_sample_rate(DEFAULT_SR);
        s
    }

    /// Rate libero o derivato dal tempo (sync attivo).
    fn apply_rate(&mut self) {
        let hz = lfo_rate(self.rate, self.sync, self.bpm);
        for l in &mut self.lfo {
            l.set_rate(hz);
        }
    }
}

impl Effect for Chorus {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        for d in &mut self.lines {
            ensure_delay(d, MAX_S, sample_rate);
        }
        for l in &mut self.lfo {
            l.set_sample_rate(sample_rate);
        }
        self.apply_rate();
        self.delay_ms.set_sample_rate(sample_rate);
        self.mix.set_sample_rate(sample_rate);
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "rate" => {
                self.rate = v;
                self.apply_rate();
            }
            "sync" => {
                self.sync = v;
                self.apply_rate();
            }
            "depth" => self.depth_ms = v,
            "delay" => self.delay_ms.set_target(v),
            "feedback" => self.feedback = v,
            "mix" => self.mix.set_target(v),
            "spread" => self.spread = v,
            _ => {}
        }
    }

    fn set_tempo(&mut self, bpm: f32) {
        self.bpm = sanitize_bpm(bpm);
        self.apply_rate();
    }

    fn reset(&mut self) {
        for d in &mut self.lines {
            d.reset();
        }
        self.lfo[0].reset(0.0);
        self.lfo[1].reset(0.5 * self.spread);
        self.delay_ms.set_immediate(self.delay_ms.target());
        self.mix.set_immediate(self.mix.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        let ms_to_samples = 0.001 * self.sr;
        for i in 0..n {
            let base = self.delay_ms.next_sample();
            let mix = self.mix.next_sample();
            for (ch, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                let d = (base + self.depth_ms * self.lfo[ch].next_sample()) * ms_to_samples;
                let x = buf[i];
                let y = self.lines[ch].read_cubic(d.max(1.0) - 1.0);
                self.lines[ch].write(flush_denormal(x + y * self.feedback));
                buf[i] = x * (1.0 - mix) + y * mix;
            }
        }
    }
}
