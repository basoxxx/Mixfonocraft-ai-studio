//! Distorsione a waveshaping: soft, hard, foldback, tube (asimmetrica).

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{db_to_gain, soft_clip, Biquad, BiquadKind, DcBlocker, Smoother};

/// Curve di distorsione (valore del parametro `type`).
pub const TYPES: &[&str] = &["soft", "hard", "foldback", "tube"];

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::DISTORTION,
    name: "Distortion",
    role: PluginRole::Effect,
    description: "Saturazione/distorsione con quattro curve, filtro di tono e mix dry/wet.",
    params: &[
        ParamDesc::choice("type", "Type", TYPES, 0),
        ParamDesc::new("drive", "Drive", 0.0, 48.0, 12.0, "dB"),
        ParamDesc::new("tone", "Tone", 500.0, 20_000.0, 12_000.0, "Hz"),
        ParamDesc::new("mix", "Mix", 0.0, 1.0, 1.0, ""),
        ParamDesc::new("output", "Output", -24.0, 12.0, 0.0, "dB"),
    ],
};

#[inline]
fn shape(kind: u8, x: f32) -> f32 {
    match kind {
        1 => x.clamp(-1.0, 1.0),
        2 => {
            // Ripiegamento a onda triangolare: continuo e limitato a ±1.
            let t = 0.25 * x + 0.25;
            4.0 * (t - t.round()).abs() - 1.0
        }
        3 => {
            if x >= 0.0 {
                1.0 - (-x).exp()
            } else {
                let x = x.max(-20.0);
                -(1.0 - (1.2 * x).exp()) / 1.2
            }
        }
        _ => soft_clip(x),
    }
}

/// Distorsione stereo.
pub struct Distortion {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    kind: u8,
    tone: f32,
    drive: Smoother,
    mix: Smoother,
    out: Smoother,
    lp: [Biquad; 2],
    dc: [DcBlocker; 2],
}

impl Default for Distortion {
    fn default() -> Self {
        Self::new()
    }
}

impl Distortion {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            kind: 0,
            tone: 12_000.0,
            drive: Smoother::new(0.01, DEFAULT_SR, 1.0),
            mix: Smoother::new(0.01, DEFAULT_SR, 1.0),
            out: Smoother::new(0.01, DEFAULT_SR, 1.0),
            lp: [Biquad::new(); 2],
            dc: [DcBlocker::new(DEFAULT_SR); 2],
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    fn update_tone(&mut self) {
        for b in &mut self.lp {
            b.set(BiquadKind::Lowpass, self.tone, 0.707, 0.0, self.sr);
        }
    }
}

impl Effect for Distortion {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        for s in [&mut self.drive, &mut self.mix, &mut self.out] {
            s.set_sample_rate(sample_rate);
        }
        for d in &mut self.dc {
            d.set_sample_rate(sample_rate);
        }
        self.update_tone();
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "type" => self.kind = v as u8,
            "drive" => self.drive.set_target(db_to_gain(v)),
            "tone" => {
                self.tone = v;
                self.update_tone();
            }
            "mix" => self.mix.set_target(v),
            "output" => self.out.set_target(db_to_gain(v)),
            _ => {}
        }
    }

    fn reset(&mut self) {
        for s in [&mut self.drive, &mut self.mix, &mut self.out] {
            s.set_immediate(s.target());
        }
        for b in &mut self.lp {
            b.reset();
        }
        for d in &mut self.dc {
            d.reset();
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        let tube = self.kind == 3;
        for i in 0..n {
            let drive = self.drive.next_sample();
            let mix = self.mix.next_sample();
            let out = self.out.next_sample();
            for (c, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                let x = buf[i];
                let mut y = shape(self.kind, x * drive);
                if tube {
                    y = self.dc[c].process(y);
                }
                y = self.lp[c].process(y);
                buf[i] = (x + (y - x) * mix) * out;
            }
        }
    }
}
