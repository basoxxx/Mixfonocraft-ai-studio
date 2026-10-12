//! Stereo Shaper: larghezza mid/side, pan, scambio L/R e inversione di fase per canale.

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR, OFF_ON};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::STEREO,
    name: "Stereo Shaper",
    role: PluginRole::Effect,
    description: "Larghezza stereo (0 = mono, 1 = invariato, 2 = extra wide), pan, swap e inversione di fase.",
    params: &[
        ParamDesc::new("width", "Width", 0.0, 2.0, 1.0, ""),
        ParamDesc::new("pan", "Pan", -1.0, 1.0, 0.0, ""),
        ParamDesc::choice("swap", "Swap L/R", OFF_ON, 0),
        ParamDesc::choice("invert_l", "Invert left", OFF_ON, 0),
        ParamDesc::choice("invert_r", "Invert right", OFF_ON, 0),
    ],
};

/// Elaborazione stereo mid/side.
pub struct Stereo {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    swap: bool,
    pan: f32,
    invert_l: bool,
    invert_r: bool,
    width: Smoother,
    gl: Smoother,
    gr: Smoother,
}

impl Default for Stereo {
    fn default() -> Self {
        Self::new()
    }
}

impl Stereo {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            swap: false,
            pan: 0.0,
            invert_l: false,
            invert_r: false,
            width: Smoother::new(0.01, DEFAULT_SR, 1.0),
            gl: Smoother::new(0.01, DEFAULT_SR, 1.0),
            gr: Smoother::new(0.01, DEFAULT_SR, 1.0),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    fn update(&mut self) {
        let (l, r) = balance_gains(self.pan);
        self.gl.set_target(if self.invert_l { -l } else { l });
        self.gr.set_target(if self.invert_r { -r } else { r });
    }
}

impl Effect for Stereo {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if sr_ok(sample_rate) {
            for s in [&mut self.width, &mut self.gl, &mut self.gr] {
                s.set_sample_rate(sample_rate);
            }
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "width" => self.width.set_target(v),
            "pan" => self.pan = v,
            "swap" => self.swap = v >= 0.5,
            "invert_l" => self.invert_l = v >= 0.5,
            "invert_r" => self.invert_r = v >= 0.5,
            _ => return,
        }
        self.update();
    }

    fn reset(&mut self) {
        for s in [&mut self.width, &mut self.gl, &mut self.gr] {
            s.set_immediate(s.target());
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        for i in 0..n {
            let (l, r) = if self.swap {
                (right[i], left[i])
            } else {
                (left[i], right[i])
            };
            let mid = 0.5 * (l + r);
            let side = 0.5 * (l - r) * self.width.next_sample();
            left[i] = (mid + side) * self.gl.next_sample();
            right[i] = (mid - side) * self.gr.next_sample();
        }
    }
}
