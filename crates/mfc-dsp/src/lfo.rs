//! Oscillatore a bassa frequenza per la modulazione.

use crate::osc::Rng;
use std::f32::consts::TAU;

/// Forma d'onda dell'[`Lfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    /// Dente di sega crescente.
    Saw,
    Square,
    /// Sample & hold: valore casuale nuovo a ogni ciclo.
    SampleHold,
}

/// LFO bipolare (-1..=1) con rate in Hz.
#[derive(Debug, Clone, Copy)]
pub struct Lfo {
    shape: LfoShape,
    phase: f32,
    inc: f32,
    rate: f32,
    sample_rate: f32,
    held: f32,
    rng: Rng,
}

impl Lfo {
    pub fn new(sample_rate: f32) -> Self {
        let mut rng = Rng::new(0x5EED);
        let held = rng.next_bipolar();
        let mut l = Self {
            shape: LfoShape::Sine,
            phase: 0.0,
            inc: 0.0,
            rate: 1.0,
            sample_rate: sample_rate.max(1.0),
            held,
            rng,
        };
        l.set_rate(1.0);
        l
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.set_rate(self.rate);
    }

    /// Frequenza in Hz (0..=sample_rate/4).
    pub fn set_rate(&mut self, hz: f32) {
        self.rate = if hz.is_finite() {
            hz.clamp(0.0, self.sample_rate * 0.25)
        } else {
            0.0
        };
        self.inc = self.rate / self.sample_rate;
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    pub fn set_shape(&mut self, shape: LfoShape) {
        self.shape = shape;
    }

    /// Reimposta la fase (0..1).
    pub fn reset(&mut self, phase: f32) {
        self.phase = if phase.is_finite() { phase - phase.floor() } else { 0.0 };
    }

    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Valore corrente in -1..=1, poi avanza.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let p = self.phase;
        let out = match self.shape {
            LfoShape::Sine => (TAU * p).sin(),
            LfoShape::Triangle => {
                let t = (p + 0.25).fract();
                1.0 - 4.0 * (t - 0.5).abs()
            }
            LfoShape::Saw => 2.0 * p - 1.0,
            LfoShape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::SampleHold => self.held,
        };
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.held = self.rng.next_bipolar();
        }
        out
    }
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_are_bounded_and_periodic() {
        for shape in [
            LfoShape::Sine,
            LfoShape::Triangle,
            LfoShape::Saw,
            LfoShape::Square,
            LfoShape::SampleHold,
        ] {
            let mut l = Lfo::new(1024.0);
            l.set_shape(shape);
            l.set_rate(2.0);
            let x: Vec<f32> = (0..2048).map(|_| l.next_sample()).collect();
            assert!(x.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "{shape:?}");
            if shape != LfoShape::SampleHold {
                assert!((x[0] - x[512]).abs() < 1e-3, "{shape:?} periodo");
            }
        }
    }

    #[test]
    fn sample_hold_changes_each_cycle() {
        let mut l = Lfo::new(1024.0);
        l.set_shape(LfoShape::SampleHold);
        l.set_rate(8.0);
        let a = l.next_sample();
        for _ in 0..127 {
            assert_eq!(l.next_sample(), a);
        }
        assert_ne!(l.next_sample(), a);
    }
}
