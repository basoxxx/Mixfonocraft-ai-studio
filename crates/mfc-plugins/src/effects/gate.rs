//! Noise gate con hold e range (attenuazione a gate chiuso).

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{db_to_gain, time_coef, EnvelopeFollower};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::GATE,
    name: "Gate",
    role: PluginRole::Effect,
    description: "Noise gate stereo-linked con attacco, hold, release e range.",
    params: &[
        ParamDesc::new("threshold", "Threshold", -80.0, 0.0, -40.0, "dB"),
        ParamDesc::new("attack", "Attack", 0.1, 100.0, 1.0, "ms"),
        ParamDesc::new("hold", "Hold", 0.0, 500.0, 20.0, "ms"),
        ParamDesc::new("release", "Release", 5.0, 2000.0, 100.0, "ms"),
        ParamDesc::new("range", "Range", -80.0, 0.0, -80.0, "dB"),
    ],
};

/// Isteresi tra apertura e chiusura (dB).
const HYSTERESIS_DB: f32 = 3.0;

/// Noise gate stereo.
pub struct Gate {
    sr: f32,
    detector: EnvelopeFollower,
    open_thr: f32,
    close_thr: f32,
    attack_ms: f32,
    hold_ms: f32,
    release_ms: f32,
    attack_coef: f32,
    release_coef: f32,
    hold_samples: u32,
    hold_count: u32,
    open: bool,
    floor: f32,
    gain: f32,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    pub fn new() -> Self {
        let mut s = Self {
            sr: DEFAULT_SR,
            detector: EnvelopeFollower::new(0.0001, 0.01, DEFAULT_SR),
            open_thr: 0.01,
            close_thr: 0.007,
            attack_ms: 1.0,
            hold_ms: 20.0,
            release_ms: 100.0,
            attack_coef: 0.0,
            release_coef: 0.0,
            hold_samples: 0,
            hold_count: 0,
            open: false,
            floor: 0.0,
            gain: 0.0,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.update();
        s.reset();
        s
    }

    fn update(&mut self) {
        self.attack_coef = time_coef(self.attack_ms * 0.001, self.sr);
        self.release_coef = time_coef(self.release_ms * 0.001, self.sr);
        self.hold_samples = (self.hold_ms * 0.001 * self.sr) as u32;
    }
}

impl Effect for Gate {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if sr_ok(sample_rate) {
            self.sr = sample_rate;
            self.detector.set_sample_rate(sample_rate);
            self.update();
            self.reset();
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "threshold" => {
                self.open_thr = db_to_gain(v);
                self.close_thr = db_to_gain(v - HYSTERESIS_DB);
            }
            "attack" => self.attack_ms = v,
            "hold" => self.hold_ms = v,
            "release" => self.release_ms = v,
            "range" => self.floor = db_to_gain(v),
            _ => return,
        }
        self.update();
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.open = false;
        self.hold_count = 0;
        self.gain = self.floor;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = frames(left, right);
        for i in 0..n {
            let level = self.detector.process(left[i].abs().max(right[i].abs()));
            if level >= self.open_thr {
                self.open = true;
                self.hold_count = self.hold_samples;
            } else if self.open && level < self.close_thr {
                if self.hold_count > 0 {
                    self.hold_count -= 1;
                } else {
                    self.open = false;
                }
            }
            let target = if self.open { 1.0 } else { self.floor };
            let coef = if target > self.gain {
                self.attack_coef
            } else {
                self.release_coef
            };
            self.gain = target + coef * (self.gain - target);
            left[i] *= self.gain;
            right[i] *= self.gain;
        }
    }
}
