//! Misuratore di picco (con decadimento) e RMS.

use crate::util::{gain_to_db, time_coef};

/// Misuratore peak/RMS per i VU del mixer.
#[derive(Debug, Clone, Copy)]
pub struct Meter {
    peak: f32,
    rms_sq: f32,
    rms_coef: f32,
    peak_decay: f32,
    sample_rate: f32,
}

impl Meter {
    /// RMS su ≈300 ms, picco con caduta di 20 dB/s.
    pub fn new(sample_rate: f32) -> Self {
        let mut m = Self {
            peak: 0.0,
            rms_sq: 0.0,
            rms_coef: 0.0,
            peak_decay: 1.0,
            sample_rate: 1.0,
        };
        m.set_sample_rate(sample_rate);
        m
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.rms_coef = time_coef(0.3, self.sample_rate);
        self.peak_decay = 10f32.powf(-20.0 / 20.0 / self.sample_rate);
    }

    pub fn reset(&mut self) {
        self.peak = 0.0;
        self.rms_sq = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) {
        let x = if x.is_finite() { x } else { 0.0 };
        self.peak = (self.peak * self.peak_decay).max(x.abs());
        self.rms_sq = x * x + self.rms_coef * (self.rms_sq - x * x);
    }

    pub fn process_block(&mut self, block: &[f32]) {
        for &x in block {
            self.process(x);
        }
    }

    pub fn peak(&self) -> f32 {
        self.peak
    }

    pub fn rms(&self) -> f32 {
        self.rms_sq.max(0.0).sqrt()
    }

    pub fn peak_db(&self) -> f32 {
        gain_to_db(self.peak)
    }

    pub fn rms_db(&self) -> f32 {
        gain_to_db(self.rms())
    }
}

impl Default for Meter {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peak_and_rms() {
        let mut m = Meter::new(48_000.0);
        let x: Vec<f32> = (0..48_000).map(|i| 0.5 * (i as f32 * 0.1).sin()).collect();
        m.process_block(&x);
        assert!((m.peak() - 0.5).abs() < 0.01);
        assert!((m.rms() - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        m.process_block(&vec![0.0; 48_000]);
        assert!((m.peak_db() - (-6.02 - 20.0)).abs() < 0.5);
        m.reset();
        assert_eq!(m.peak(), 0.0);
    }
}
