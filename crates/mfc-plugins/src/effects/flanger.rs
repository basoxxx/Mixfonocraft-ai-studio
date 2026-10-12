//! Flanger stereo: ritardo corto modulato con feedback positivo o negativo.

use super::{apply_defaults, ensure_delay, frames, param_value, sr_ok, DEFAULT_SR, MAX_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{flush_denormal, soft_clip, DelayLine, Lfo, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::FLANGER,
    name: "Flanger",
    role: PluginRole::Effect,
    description: "Flanger stereo con feedback bipolare e sfasamento dell'LFO tra i canali.",
    params: &[
        ParamDesc::new("rate", "Rate", 0.05, 5.0, 0.3, "Hz"),
        ParamDesc::new("depth", "Depth", 0.0, 5.0, 2.0, "ms"),
        ParamDesc::new("delay", "Delay", 0.1, 10.0, 1.0, "ms"),
        ParamDesc::new("feedback", "Feedback", -0.95, 0.95, 0.6, ""),
        ParamDesc::new("mix", "Mix", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("stereo", "Stereo phase", 0.0, 1.0, 0.25, ""),
    ],
};

const MAX_S: f32 = 0.02;

/// Flanger stereo.
pub struct Flanger {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    lines: [DelayLine; 2],
    lfo: [Lfo; 2],
    rate: f32,
    depth_ms: f32,
    stereo: f32,
    feedback: f32,
    delay_ms: Smoother,
    mix: Smoother,
}

impl Default for Flanger {
    fn default() -> Self {
        Self::new()
    }
}

impl Flanger {
    pub fn new() -> Self {
        let cap = super::capacity(MAX_S, MAX_SR);
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            lines: [DelayLine::new(cap), DelayLine::new(cap)],
            lfo: [Lfo::new(DEFAULT_SR); 2],
            rate: 0.3,
            depth_ms: 2.0,
            stereo: 0.25,
            feedback: 0.6,
            delay_ms: Smoother::new(0.05, DEFAULT_SR, 1.0),
            mix: Smoother::new(0.02, DEFAULT_SR, 0.5),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.set_sample_rate(DEFAULT_SR);
        s
    }
}

impl Effect for Flanger {
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
            l.set_rate(self.rate);
        }
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
                for l in &mut self.lfo {
                    l.set_rate(v);
                }
            }
            "depth" => self.depth_ms = v,
            "delay" => self.delay_ms.set_target(v),
            "feedback" => self.feedback = v,
            "mix" => self.mix.set_target(v),
            "stereo" => self.stereo = v,
            _ => {}
        }
    }

    fn reset(&mut self) {
        for d in &mut self.lines {
            d.reset();
        }
        self.lfo[0].reset(0.0);
        self.lfo[1].reset(self.stereo);
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
                let sweep = 0.5 + 0.5 * self.lfo[ch].next_sample();
                let d = (base + self.depth_ms * sweep) * ms_to_samples;
                let x = buf[i];
                // Interpolazione lineare: guadagno ≤ 1, anello di feedback stabile.
                let y = self.lines[ch].read(d.max(1.0) - 1.0);
                self.lines[ch].write(flush_denormal(soft_clip(x + y * self.feedback)));
                buf[i] = x * (1.0 - mix) + y * mix;
            }
        }
    }
}
