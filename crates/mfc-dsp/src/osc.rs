//! Oscillatori band-limited (PolyBLEP / PolyBLAMP) e rumore deterministico.

use std::f32::consts::TAU;

/// Forma d'onda dell'[`Oscillator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Waveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    /// Onda quadra/impulsiva (vedi [`Oscillator::set_pulse_width`]).
    Square,
}

/// Generatore pseudo-casuale xorshift32: deterministico e real-time safe.
#[derive(Debug, Clone, Copy)]
pub struct Rng {
    state: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        // Lo stato zero è un punto fisso di xorshift.
        let s = seed ^ 0x9E37_79B9;
        Self {
            state: if s == 0 { 0x2545_F491 } else { s },
        }
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Valore uniforme in 0..1.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }

    /// Valore uniforme in -1..1 (rumore bianco).
    #[inline]
    pub fn next_bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }
}

impl Default for Rng {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Residuo PolyBLEP per un gradino di ampiezza 2 alla fase `t` (0..1), incremento `dt`.
#[inline]
pub fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

/// Residuo PolyBLAMP (integrale di [`poly_blep`]) per gli spigoli di pendenza.
#[inline]
pub fn poly_blamp(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt - 1.0;
        -x * x * x / 3.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt + 1.0;
        x * x * x / 3.0
    } else {
        0.0
    }
}

#[inline]
fn wrap01(x: f32) -> f32 {
    x - x.floor()
}

/// Oscillatore band-limited con fase normalizzata 0..1.
#[derive(Debug, Clone, Copy)]
pub struct Oscillator {
    waveform: Waveform,
    phase: f32,
    inc: f32,
    freq: f32,
    sample_rate: f32,
    pulse_width: f32,
}

impl Oscillator {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            waveform: Waveform::Sine,
            phase: 0.0,
            inc: 0.0,
            freq: 0.0,
            sample_rate: sample_rate.max(1.0),
            pulse_width: 0.5,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.set_frequency(self.freq);
    }

    /// Frequenza in Hz (limitata sotto Nyquist).
    #[inline]
    pub fn set_frequency(&mut self, hz: f32) {
        self.freq = if hz.is_finite() { hz.max(0.0) } else { 0.0 };
        self.inc = (self.freq / self.sample_rate).min(0.49);
    }

    pub fn frequency(&self) -> f32 {
        self.freq
    }

    pub fn set_waveform(&mut self, waveform: Waveform) {
        self.waveform = waveform;
    }

    pub fn waveform(&self) -> Waveform {
        self.waveform
    }

    /// Larghezza d'impulso per [`Waveform::Square`] (0.5 = quadra).
    pub fn set_pulse_width(&mut self, pw: f32) {
        self.pulse_width = if pw.is_finite() { pw.clamp(0.01, 0.99) } else { 0.5 };
    }

    /// Reimposta la fase (0..1).
    pub fn reset(&mut self, phase: f32) {
        self.phase = if phase.is_finite() { wrap01(phase) } else { 0.0 };
    }

    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Restituisce la fase corrente e avanza (utile per forme d'onda personalizzate).
    #[inline]
    pub fn advance(&mut self) -> f32 {
        let p = self.phase;
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        p
    }

    /// Campione successivo in -1..1 (circa).
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let dt = self.inc;
        let t = self.advance();
        match self.waveform {
            Waveform::Sine => (TAU * t).sin(),
            Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Waveform::Square => {
                let pw = self.pulse_width;
                let mut v = if t < pw { 1.0 } else { -1.0 };
                v += poly_blep(t, dt);
                v -= poly_blep(wrap01(t - pw), dt);
                v
            }
            Waveform::Triangle => {
                // Sfasata di 1/4 per partire da zero in salita.
                let t = wrap01(t + 0.25);
                let mut v = 1.0 - 4.0 * (t - 0.5).abs();
                v += 4.0 * dt * (poly_blamp(t, dt) - poly_blamp(wrap01(t + 0.5), dt));
                v
            }
        }
    }
}

impl Default for Oscillator {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::zero_cross_freq;

    fn render(wf: Waveform, hz: f32, n: usize) -> Vec<f32> {
        let mut o = Oscillator::new(48_000.0);
        o.set_waveform(wf);
        o.set_frequency(hz);
        (0..n).map(|_| o.next_sample()).collect()
    }

    #[test]
    fn frequency_by_zero_crossings() {
        for wf in [Waveform::Sine, Waveform::Triangle, Waveform::Saw, Waveform::Square] {
            for hz in [55.0, 440.0, 1234.0, 5000.0] {
                let x = render(wf, hz, 48_000);
                let f = zero_cross_freq(&x, 48_000.0);
                assert!((f - hz).abs() / hz < 0.01, "{wf:?} {hz}: misurato {f}");
                assert!(x.iter().all(|v| v.is_finite() && v.abs() < 1.5));
            }
        }
    }

    #[test]
    fn saw_is_bounded_and_zero_mean() {
        let x = render(Waveform::Saw, 1000.0, 48_000);
        let mean: f32 = x.iter().sum::<f32>() / x.len() as f32;
        assert!(mean.abs() < 0.01);
    }

    #[test]
    fn pulse_width_and_reset() {
        let mut o = Oscillator::new(48_000.0);
        o.set_waveform(Waveform::Square);
        o.set_pulse_width(0.25);
        o.set_frequency(100.0);
        let x: Vec<f32> = (0..48_000).map(|_| o.next_sample()).collect();
        let pos = x.iter().filter(|v| **v > 0.0).count() as f32 / x.len() as f32;
        assert!((pos - 0.25).abs() < 0.01);
        o.reset(0.5);
        assert_eq!(o.phase(), 0.5);
        o.set_frequency(f32::NAN);
        assert!(o.next_sample().is_finite());
    }

    #[test]
    fn rng_is_deterministic_and_uniform() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let mut sum = 0.0;
        for _ in 0..10_000 {
            let v = a.next_bipolar();
            assert_eq!(v, b.next_bipolar());
            assert!((-1.0..1.0).contains(&v));
            sum += v;
        }
        assert!((sum / 10_000.0f32).abs() < 0.05);
        assert_ne!(Rng::new(0).next_u32(), 0);
    }
}
