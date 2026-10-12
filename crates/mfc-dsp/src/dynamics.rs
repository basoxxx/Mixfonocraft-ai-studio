//! Dinamica: inseguitore d'inviluppo e calcolo del guadagno con soft knee.

use crate::util::{flush_denormal, time_coef};

/// Modalità del rilevatore di livello.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetectorMode {
    #[default]
    Peak,
    Rms,
}

/// Inseguitore d'inviluppo con attacco/rilascio separati (lineare, non dB).
#[derive(Debug, Clone, Copy)]
pub struct EnvelopeFollower {
    mode: DetectorMode,
    attack: f32,
    release: f32,
    attack_coef: f32,
    release_coef: f32,
    sample_rate: f32,
    env: f32,
}

impl EnvelopeFollower {
    /// Tempi in secondi.
    pub fn new(attack: f32, release: f32, sample_rate: f32) -> Self {
        let mut f = Self {
            mode: DetectorMode::Peak,
            attack,
            release,
            attack_coef: 0.0,
            release_coef: 0.0,
            sample_rate: sample_rate.max(1.0),
            env: 0.0,
        };
        f.update();
        f
    }

    fn update(&mut self) {
        self.attack_coef = time_coef(self.attack, self.sample_rate);
        self.release_coef = time_coef(self.release, self.sample_rate);
    }

    pub fn set_mode(&mut self, mode: DetectorMode) {
        self.mode = mode;
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.update();
    }

    pub fn set_times(&mut self, attack: f32, release: f32) {
        self.attack = if attack.is_finite() { attack.max(0.0) } else { 0.0 };
        self.release = if release.is_finite() { release.max(0.0) } else { 0.0 };
        self.update();
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }

    /// Livello corrente (ampiezza lineare).
    pub fn value(&self) -> f32 {
        match self.mode {
            DetectorMode::Peak => self.env,
            DetectorMode::Rms => self.env.sqrt(),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let v = match self.mode {
            DetectorMode::Peak => x.abs(),
            DetectorMode::Rms => x * x,
        };
        let v = if v.is_finite() { v } else { 0.0 };
        let c = if v > self.env {
            self.attack_coef
        } else {
            self.release_coef
        };
        self.env = flush_denormal(v + c * (self.env - v));
        self.value()
    }
}

/// Curva statica di compressione (dB in → riduzione dB ≤ 0) con soft knee.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainComputer {
    pub threshold_db: f32,
    /// Rapporto ≥ 1 (`f32::INFINITY` = limiter).
    pub ratio: f32,
    pub knee_db: f32,
}

impl GainComputer {
    pub fn new(threshold_db: f32, ratio: f32, knee_db: f32) -> Self {
        Self {
            threshold_db,
            ratio,
            knee_db,
        }
    }

    /// Variazione di guadagno in dB (≤ 0) per un livello d'ingresso in dB.
    #[inline]
    pub fn gain_db(&self, level_db: f32) -> f32 {
        let ratio = if self.ratio.is_nan() { 1.0 } else { self.ratio.max(1.0) };
        let slope = 1.0 / ratio - 1.0;
        let knee = if self.knee_db.is_finite() {
            self.knee_db.max(0.0)
        } else {
            0.0
        };
        let over = level_db - self.threshold_db;
        if !over.is_finite() {
            return 0.0;
        }
        if 2.0 * over <= -knee {
            0.0
        } else if knee > 0.0 && 2.0 * over.abs() < knee {
            let o = over + knee * 0.5;
            slope * o * o / (2.0 * knee)
        } else {
            slope * over
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follower_attack_release() {
        let mut f = EnvelopeFollower::new(0.001, 0.1, 48_000.0);
        let mut v = 0.0;
        for _ in 0..480 {
            v = f.process(1.0);
        }
        assert!(v > 0.6 && v <= 1.0);
        for _ in 0..4800 {
            v = f.process(0.0);
        }
        assert!(v > 0.3 && v < 0.5, "{v}");
        let mut r = EnvelopeFollower::new(0.3, 0.3, 48_000.0);
        r.set_mode(DetectorMode::Rms);
        for i in 0..96_000 {
            v = r.process((i as f32 * 0.05).sin());
        }
        assert!((v - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.1);
    }

    #[test]
    fn gain_computer_curve() {
        let g = GainComputer::new(-20.0, 4.0, 0.0);
        assert_eq!(g.gain_db(-30.0), 0.0);
        assert!((g.gain_db(-10.0) + 7.5).abs() < 1e-5);
        let soft = GainComputer::new(-20.0, 4.0, 10.0);
        assert_eq!(soft.gain_db(-26.0), 0.0);
        let at_thr = soft.gain_db(-20.0);
        assert!(at_thr < 0.0 && at_thr > -1.0);
        assert!((soft.gain_db(0.0) + 15.0).abs() < 1e-4);
        // Continuità ai bordi del ginocchio.
        assert!((soft.gain_db(-15.0) - g.gain_db(-15.0)).abs() < 1e-4);
        let lim = GainComputer::new(-1.0, f32::INFINITY, 0.0);
        assert!((lim.gain_db(5.0) + 6.0).abs() < 1e-5);
    }
}
