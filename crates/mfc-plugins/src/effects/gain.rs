//! Gain: guadagno, pan (bilanciamento) e inversione di fase.

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR, OFF_ON};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, db_to_gain, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::GAIN,
    name: "Gain",
    role: PluginRole::Effect,
    description: "Guadagno, bilanciamento e inversione di polarità.",
    params: &[
        ParamDesc::new("gain", "Gain", -60.0, 24.0, 0.0, "dB"),
        ParamDesc::new("pan", "Pan", -1.0, 1.0, 0.0, ""),
        ParamDesc::choice("invert", "Invert phase", OFF_ON, 0),
    ],
};

/// Effetto di guadagno con smoothing.
pub struct Gain {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    gain_db: f32,
    pan: f32,
    invert: bool,
    gl: Smoother,
    gr: Smoother,
}

impl Default for Gain {
    fn default() -> Self {
        Self::new()
    }
}

impl Gain {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            gain_db: 0.0,
            pan: 0.0,
            invert: false,
            gl: Smoother::new(0.01, DEFAULT_SR, 1.0),
            gr: Smoother::new(0.01, DEFAULT_SR, 1.0),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    fn targets(&self) -> (f32, f32) {
        let g = db_to_gain(self.gain_db) * if self.invert { -1.0 } else { 1.0 };
        let (l, r) = balance_gains(self.pan);
        (g * l, g * r)
    }
}

impl Effect for Gain {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if sr_ok(sample_rate) {
            self.gl.set_sample_rate(sample_rate);
            self.gr.set_sample_rate(sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "gain" => self.gain_db = v,
            "pan" => self.pan = v,
            "invert" => self.invert = v >= 0.5,
            _ => return,
        }
        let (l, r) = self.targets();
        self.gl.set_target(l);
        self.gr.set_target(r);
    }

    fn reset(&mut self) {
        let (l, r) = self.targets();
        self.gl.set_immediate(l);
        self.gr.set_immediate(r);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        for i in 0..n {
            left[i] *= self.gl.next_sample();
            right[i] *= self.gr.next_sample();
        }
    }
}
