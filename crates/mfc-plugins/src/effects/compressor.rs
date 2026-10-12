//! Compressore stereo-linked con soft knee (stile "Fruity Compressor").

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{db_to_gain, gain_to_db, time_coef, GainComputer, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::COMPRESSOR,
    name: "Compressor",
    role: PluginRole::Effect,
    description: "Compressore feed-forward con rilevamento di picco collegato sui due canali.",
    params: &[
        ParamDesc::new("threshold", "Threshold", -60.0, 0.0, -18.0, "dB"),
        ParamDesc::new("ratio", "Ratio", 1.0, 20.0, 4.0, ":1"),
        ParamDesc::new("attack", "Attack", 0.1, 200.0, 10.0, "ms"),
        ParamDesc::new("release", "Release", 5.0, 2000.0, 100.0, "ms"),
        ParamDesc::new("knee", "Knee", 0.0, 24.0, 6.0, "dB"),
        ParamDesc::new("makeup", "Makeup gain", 0.0, 24.0, 0.0, "dB"),
    ],
};

/// Compressore stereo.
pub struct Compressor {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    computer: GainComputer,
    attack_ms: f32,
    release_ms: f32,
    attack_coef: f32,
    release_coef: f32,
    /// Riduzione di guadagno corrente (dB, ≤ 0).
    gr_db: f32,
    makeup: Smoother,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            computer: GainComputer::new(-18.0, 4.0, 6.0),
            attack_ms: 10.0,
            release_ms: 100.0,
            attack_coef: 0.0,
            release_coef: 0.0,
            gr_db: 0.0,
            makeup: Smoother::new(0.01, DEFAULT_SR, 1.0),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    fn update_times(&mut self) {
        self.attack_coef = time_coef(self.attack_ms * 0.001, self.sr);
        self.release_coef = time_coef(self.release_ms * 0.001, self.sr);
    }

    /// Riduzione di guadagno corrente in dB (≤ 0), per il metering.
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }
}

impl Effect for Compressor {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if sr_ok(sample_rate) {
            self.sr = sample_rate;
            self.makeup.set_sample_rate(sample_rate);
            self.update_times();
            self.reset();
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "threshold" => self.computer.threshold_db = v,
            "ratio" => self.computer.ratio = v,
            "knee" => self.computer.knee_db = v,
            "makeup" => self.makeup.set_target(db_to_gain(v)),
            "attack" => {
                self.attack_ms = v;
                self.update_times();
            }
            "release" => {
                self.release_ms = v;
                self.update_times();
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.gr_db = 0.0;
        self.makeup.set_immediate(self.makeup.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        for i in 0..n {
            let level = left[i].abs().max(right[i].abs());
            let target = self.computer.gain_db(gain_to_db(level));
            let coef = if target < self.gr_db {
                self.attack_coef
            } else {
                self.release_coef
            };
            self.gr_db = target + coef * (self.gr_db - target);
            let g = db_to_gain(self.gr_db) * self.makeup.next_sample();
            left[i] *= g;
            right[i] *= g;
        }
    }
}
