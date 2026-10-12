//! Utilità: conversioni dB/Hz/cent, leggi di pan, saturazione, smoothing dei parametri.

use std::f32::consts::FRAC_PI_4;

/// Soglia inferiore in dB (silenzio).
pub const MIN_DB: f32 = -150.0;

/// dB → guadagno lineare (≤ [`MIN_DB`] → 0).
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    if db <= MIN_DB {
        0.0
    } else {
        10f32.powf(db * 0.05)
    }
}

/// Guadagno lineare → dB (limitato inferiormente a [`MIN_DB`]).
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    let g = gain.abs();
    if g > 0.0 {
        (20.0 * g.log10()).max(MIN_DB)
    } else {
        MIN_DB
    }
}

/// Nota MIDI (anche frazionaria) → Hz, con A4 = 69 = 440 Hz.
#[inline]
pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// Hz → nota MIDI frazionaria.
#[inline]
pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz.max(1e-6) / 440.0).log2()
}

/// Cent → rapporto di frequenza.
#[inline]
pub fn cents_to_ratio(cents: f32) -> f32 {
    2f32.powf(cents / 1200.0)
}

/// Semitoni → rapporto di frequenza.
#[inline]
pub fn semitones_to_ratio(semitones: f32) -> f32 {
    2f32.powf(semitones / 12.0)
}

/// Pan a potenza costante: `pan` in -1..=1 → (L, R); al centro entrambi ≈ 0.707.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 };
    let a = (p + 1.0) * FRAC_PI_4;
    (a.cos(), a.sin())
}

/// Bilanciamento: al centro guadagno unitario, ai lati attenua il canale opposto.
#[inline]
pub fn balance_gains(pan: f32) -> (f32, f32) {
    let p = if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 };
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

/// Saturazione morbida (approssimazione razionale di tanh), uscita in -1..=1.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// Clip rigido simmetrico a `limit`.
#[inline]
pub fn hard_clip(x: f32, limit: f32) -> f32 {
    let l = limit.abs();
    x.clamp(-l, l)
}

/// Azzera i valori denormali (e quelli non finiti) per evitare cali di prestazioni o propagazione di NaN.
#[inline]
pub fn flush_denormal(x: f32) -> f32 {
    if x.abs() < 1e-20 || !x.is_finite() {
        0.0
    } else {
        x
    }
}

/// Coefficiente di un filtro a un polo con costante di tempo `seconds`.
#[inline]
pub fn time_coef(seconds: f32, sample_rate: f32) -> f32 {
    let n = seconds * sample_rate;
    if n <= 1e-3 || !n.is_finite() {
        0.0
    } else {
        (-1.0 / n).exp()
    }
}

/// Smoothing a un polo dei parametri (evita "zipper noise").
#[derive(Debug, Clone, Copy)]
pub struct Smoother {
    current: f32,
    target: f32,
    coef: f32,
    time: f32,
    sample_rate: f32,
}

impl Smoother {
    /// `time` = costante di tempo in secondi.
    pub fn new(time: f32, sample_rate: f32, initial: f32) -> Self {
        let mut s = Self {
            current: initial,
            target: initial,
            coef: 0.0,
            time,
            sample_rate,
        };
        s.update();
        s
    }

    fn update(&mut self) {
        self.coef = time_coef(self.time.max(0.0), self.sample_rate.max(1.0));
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.update();
    }

    pub fn set_time(&mut self, time: f32) {
        self.time = time;
        self.update();
    }

    /// Nuovo valore obiettivo (raggiunto gradualmente).
    pub fn set_target(&mut self, target: f32) {
        if target.is_finite() {
            self.target = target;
        }
    }

    /// Salta subito al valore.
    pub fn set_immediate(&mut self, value: f32) {
        if value.is_finite() {
            self.target = value;
            self.current = value;
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn value(&self) -> f32 {
        self.current
    }

    /// `true` quando il valore ha raggiunto l'obiettivo.
    pub fn is_settled(&self) -> bool {
        self.current == self.target
    }

    /// Avanza di un campione e restituisce il valore corrente.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        if self.current != self.target {
            let next = self.target + (self.current - self.target) * self.coef;
            // Aggancio al target quando l'arrotondamento f32 non fa più progressi.
            let stalled = next == self.current;
            self.current = next;
            if stalled || (next - self.target).abs() <= 1e-6 * self.target.abs().max(1e-3) {
                self.current = self.target;
            }
        }
        self.current
    }
}

impl Default for Smoother {
    fn default() -> Self {
        Self::new(0.01, 48_000.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert!((gain_to_db(2.0) - 6.0206).abs() < 1e-3);
        assert_eq!(gain_to_db(0.0), MIN_DB);
        assert_eq!(db_to_gain(MIN_DB), 0.0);
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((midi_to_hz(81.0) - 880.0).abs() < 1e-2);
        assert!((hz_to_midi(220.0) - 57.0).abs() < 1e-4);
        assert!((cents_to_ratio(1200.0) - 2.0).abs() < 1e-5);
        assert!((semitones_to_ratio(-12.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn pan_laws() {
        let (l, r) = pan_gains(0.0);
        assert!((l - r).abs() < 1e-6 && (l * l + r * r - 1.0).abs() < 1e-5);
        let (l, r) = pan_gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        assert_eq!(balance_gains(0.0), (1.0, 1.0));
        assert_eq!(balance_gains(1.0), (0.0, 1.0));
        assert_eq!(pan_gains(f32::NAN), pan_gains(0.0));
    }

    #[test]
    fn clip_and_denormal() {
        assert!((soft_clip(10.0) - 1.0).abs() < 1e-6);
        assert!(soft_clip(0.1) > 0.09 && soft_clip(0.1) < 0.1);
        assert_eq!(hard_clip(2.0, 1.0), 1.0);
        assert_eq!(flush_denormal(1e-30), 0.0);
        assert_eq!(flush_denormal(f32::NAN), 0.0);
        assert_eq!(flush_denormal(0.5), 0.5);
    }

    #[test]
    fn smoother_converges() {
        let mut s = Smoother::new(0.01, 48_000.0, 0.0);
        s.set_target(1.0);
        let first = s.next_sample();
        assert!(first > 0.0 && first < 0.01);
        for _ in 0..48_000 {
            s.next_sample();
        }
        assert!(s.is_settled(), "{} {}", s.value(), s.target());
        assert_eq!(s.value(), 1.0);
    }
}
