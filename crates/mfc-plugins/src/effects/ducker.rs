//! Ducker: volume shaper sincronizzato al tempo (stile "LFO Tool"/"Volume Shaper").
//! Abbassa il volume a ogni inizio di ciclo e lo riporta su con una curva regolabile:
//! il pompaggio della techno senza bisogno di una sorgente sidechain.

use super::sync::{division_beats, BeatClock, DIVISIONS};
use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{gain_to_db, time_coef};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::DUCKER,
    name: "Ducker",
    role: PluginRole::Effect,
    description: "Volume shaper sincronizzato al tempo: a ogni ciclo il volume scende di 'depth' e \
                  risale entro 'release' (frazione del ciclo) con curva regolabile.",
    params: &[
        ParamDesc::choice("rate", "Rate", DIVISIONS, 3),
        ParamDesc::new("depth", "Depth", 0.0, 1.0, 0.8, ""),
        ParamDesc::new("release", "Release", 0.05, 1.0, 0.6, ""),
        ParamDesc::new("curve", "Curve", -1.0, 1.0, 0.3, ""),
        ParamDesc::new("offset", "Phase offset", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("smooth", "Smoothing", 0.0, 20.0, 1.0, "ms"),
    ],
};

/// Volume shaper sincronizzato.
pub struct Ducker {
    sr: f32,
    clock: BeatClock,
    rate: f32,
    depth: f32,
    release: f32,
    curve: f32,
    offset: f32,
    smooth_ms: f32,
    smooth_coef: f32,
    g: f32,
    /// Riduzione massima dall'ultima lettura (metering).
    g_min: f32,
}

impl Default for Ducker {
    fn default() -> Self {
        Self::new()
    }
}

impl Ducker {
    pub fn new() -> Self {
        let mut s = Self {
            sr: DEFAULT_SR,
            clock: BeatClock::default(),
            rate: 3.0,
            depth: 0.8,
            release: 0.6,
            curve: 0.3,
            offset: 0.0,
            smooth_ms: 1.0,
            smooth_coef: 0.0,
            g: 1.0,
            g_min: 1.0,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s
    }

    /// Guadagno della curva alla fase `phase` (0..1).
    pub fn shape(&self, phase: f32) -> f32 {
        if phase >= self.release {
            return 1.0;
        }
        let t = phase / self.release;
        // curve > 0: resta giù più a lungo; curve < 0: risale subito.
        let shaped = t.powf(2f32.powf(self.curve * 2.0));
        1.0 - self.depth * (1.0 - shaped)
    }
}

impl Effect for Ducker {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if sr_ok(sample_rate) {
            self.sr = sample_rate;
            self.smooth_coef = time_coef(self.smooth_ms * 0.001, sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "rate" => self.rate = v,
            "depth" => self.depth = v,
            "release" => self.release = v,
            "curve" => self.curve = v,
            "offset" => self.offset = v,
            "smooth" => {
                self.smooth_ms = v;
                self.smooth_coef = time_coef(v * 0.001, self.sr);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.g = 1.0;
        self.g_min = 1.0;
    }

    fn set_transport(&mut self, tempo_bpm: f64, beat: f64, playing: bool) {
        self.clock.set_transport(tempo_bpm, beat, playing);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = frames(left, right);
        let cycle = division_beats(self.rate);
        let step = self.clock.step(self.sr);
        let mut g_min = self.g_min;
        for i in 0..n {
            let target = self.shape(self.clock.phase(cycle, self.offset));
            self.g = target + self.smooth_coef * (self.g - target);
            g_min = g_min.min(self.g);
            left[i] *= self.g;
            right[i] *= self.g;
            self.clock.beat += step;
        }
        self.g_min = g_min;
    }

    fn take_gain_reduction_db(&mut self) -> f32 {
        let g = std::mem::replace(&mut self.g_min, 1.0);
        if g >= 1.0 {
            0.0
        } else {
            gain_to_db(g.max(1e-6))
        }
    }
}
