//! Bitcrusher: riduzione della risoluzione (bit) e del sample rate (sample & hold).

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::Smoother;

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::BITCRUSHER,
    name: "Bitcrusher",
    role: PluginRole::Effect,
    description: "Quantizzazione a pochi bit e downsampling per suoni lo-fi.",
    params: &[
        ParamDesc::new("bits", "Bits", 1.0, 24.0, 8.0, "bit"),
        ParamDesc::new("downsample", "Downsample", 1.0, 64.0, 1.0, "x"),
        ParamDesc::new("mix", "Mix", 0.0, 1.0, 1.0, ""),
    ],
};

/// Bitcrusher stereo.
pub struct Bitcrusher {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    levels: f32,
    factor: f32,
    counter: f32,
    held: [f32; 2],
    mix: Smoother,
}

impl Default for Bitcrusher {
    fn default() -> Self {
        Self::new()
    }
}

impl Bitcrusher {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            levels: 128.0,
            factor: 1.0,
            counter: 0.0,
            held: [0.0; 2],
            mix: Smoother::new(0.01, DEFAULT_SR, 1.0),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    #[inline]
    fn quantize(&self, x: f32) -> f32 {
        ((x * self.levels).round() / self.levels).clamp(-1.0, 1.0)
    }
}

impl Effect for Bitcrusher {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if sr_ok(sample_rate) {
            self.mix.set_sample_rate(sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            // Livelli per semionda: 2^(bit-1) (bit frazionari ammessi).
            "bits" => self.levels = 2f32.powf(v - 1.0),
            "downsample" => self.factor = v,
            "mix" => self.mix.set_target(v),
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.counter = 0.0;
        self.held = [0.0; 2];
        self.mix.set_immediate(self.mix.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        for i in 0..n {
            self.counter -= 1.0;
            if self.counter <= 0.0 {
                self.counter += self.factor;
                self.held = [self.quantize(left[i]), self.quantize(right[i])];
            }
            let m = self.mix.next_sample();
            left[i] += (self.held[0] - left[i]) * m;
            right[i] += (self.held[1] - right[i]) * m;
        }
    }
}
