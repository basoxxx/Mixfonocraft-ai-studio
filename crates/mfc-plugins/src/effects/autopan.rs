//! Auto Pan / Tremolo sincronizzato al tempo.

use std::f32::consts::TAU;

use super::sync::{division_beats, BeatClock, DIVISIONS};
use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, time_coef};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::AUTOPAN,
    name: "Auto Pan",
    role: PluginRole::Effect,
    description: "LFO sincronizzato al tempo su pan (auto pan) o volume (tremolo), con forma \
                  sinusoidale, triangolare o quadra.",
    params: &[
        ParamDesc::choice("mode", "Mode", &["pan", "tremolo"], 0),
        ParamDesc::choice("rate", "Rate", DIVISIONS, 2),
        ParamDesc::new("depth", "Depth", 0.0, 1.0, 0.5, ""),
        ParamDesc::choice("shape", "Shape", &["sine", "triangle", "square"], 0),
        ParamDesc::new("offset", "Phase offset", 0.0, 1.0, 0.0, ""),
    ],
};

/// Auto pan / tremolo.
pub struct AutoPan {
    sr: f32,
    clock: BeatClock,
    tremolo: bool,
    rate: f32,
    depth: f32,
    shape: u8,
    offset: f32,
    /// Guadagni (L, R) correnti, smussati per evitare click con l'onda quadra.
    gl: f32,
    gr: f32,
    coef: f32,
}

impl Default for AutoPan {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoPan {
    pub fn new() -> Self {
        let mut s = Self {
            sr: DEFAULT_SR,
            clock: BeatClock::default(),
            tremolo: false,
            rate: 2.0,
            depth: 0.5,
            shape: 0,
            offset: 0.0,
            gl: 1.0,
            gr: 1.0,
            coef: time_coef(0.002, DEFAULT_SR),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s
    }

    /// LFO bipolare (-1..1) alla fase `p`.
    fn lfo(&self, p: f32) -> f32 {
        match self.shape {
            0 => (TAU * p).sin(),
            1 => 1.0 - 4.0 * ((p + 0.25).rem_euclid(1.0) - 0.5).abs(),
            _ => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }

    /// Guadagni (L, R) di destinazione alla fase `p`.
    fn targets(&self, p: f32) -> (f32, f32) {
        let x = self.lfo(p);
        if self.tremolo {
            let g = 1.0 - self.depth * (0.5 - 0.5 * x);
            (g, g)
        } else {
            balance_gains(x * self.depth)
        }
    }
}

impl Effect for AutoPan {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if sr_ok(sample_rate) {
            self.sr = sample_rate;
            self.coef = time_coef(0.002, sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "mode" => self.tremolo = v >= 0.5,
            "rate" => self.rate = v,
            "depth" => self.depth = v,
            "shape" => self.shape = v as u8,
            "offset" => self.offset = v,
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.gl = 1.0;
        self.gr = 1.0;
    }

    fn set_transport(&mut self, tempo_bpm: f64, beat: f64, playing: bool) {
        self.clock.set_transport(tempo_bpm, beat, playing);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = frames(left, right);
        let cycle = division_beats(self.rate);
        let step = self.clock.step(self.sr);
        for i in 0..n {
            let (tl, tr) = self.targets(self.clock.phase(cycle, self.offset));
            self.gl = tl + self.coef * (self.gl - tl);
            self.gr = tr + self.coef * (self.gr - tr);
            left[i] *= self.gl;
            right[i] *= self.gr;
            self.clock.beat += step;
        }
    }
}
