//! Delay line circolare con lettura frazionaria (lineare e Hermite).

use crate::resample::hermite4;

/// Delay line preallocata. `read*(d)` restituisce il campione scritto `d` campioni
/// prima dell'ultimo `write` (d = 0 → ultimo campione scritto).
#[derive(Debug, Clone)]
pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    /// Alloca spazio per un ritardo massimo di `max_delay` campioni (+ margine per l'interpolazione).
    pub fn new(max_delay: usize) -> Self {
        Self {
            buf: vec![0.0; max_delay.max(1) + 4],
            pos: 0,
        }
    }

    /// Ritardo massimo leggibile.
    pub fn max_delay(&self) -> usize {
        self.buf.len() - 4
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
    }

    /// Lettura a ritardo intero.
    #[inline]
    pub fn read_int(&self, delay: usize) -> f32 {
        let len = self.buf.len();
        let d = delay.min(len - 1);
        let idx = (self.pos + len - 1 - d) % len;
        self.buf[idx]
    }

    #[inline]
    fn split(&self, delay: f32, lo: f32) -> (usize, f32) {
        let d = if delay.is_finite() {
            delay.clamp(lo, self.max_delay() as f32)
        } else {
            lo
        };
        let i = d.floor();
        (i as usize, d - i)
    }

    /// Lettura frazionaria con interpolazione lineare.
    #[inline]
    pub fn read(&self, delay: f32) -> f32 {
        let (i, t) = self.split(delay, 0.0);
        let a = self.read_int(i);
        let b = self.read_int(i + 1);
        a + (b - a) * t
    }

    /// Lettura frazionaria con interpolazione cubica di Hermite (4 punti).
    #[inline]
    pub fn read_cubic(&self, delay: f32) -> f32 {
        let (i, t) = self.split(delay, 0.0);
        let xm1 = if i == 0 { self.read_int(0) } else { self.read_int(i - 1) };
        hermite4(xm1, self.read_int(i), self.read_int(i + 1), self.read_int(i + 2), t)
    }

    /// Scrive `x` e legge con ritardo `delay` (lineare): `y[n] = x[n - delay]`.
    #[inline]
    pub fn process(&mut self, x: f32, delay: f32) -> f32 {
        self.write(x);
        self.read(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_integer_delay() {
        for d in [0usize, 1, 7, 100] {
            let mut dl = DelayLine::new(128);
            let out: Vec<f32> = (0..300)
                .map(|i| dl.process(if i == 5 { 1.0 } else { 0.0 }, d as f32))
                .collect();
            for (i, y) in out.iter().enumerate() {
                let expected = if i == 5 + d { 1.0 } else { 0.0 };
                assert_eq!(*y, expected, "d={d} i={i}");
            }
        }
    }

    #[test]
    fn fractional_reads_interpolate() {
        let mut dl = DelayLine::new(16);
        for i in 0..10 {
            dl.write(i as f32);
        }
        // Ultimo scritto = 9: ritardo 2.5 → tra 7 e 6.
        assert!((dl.read(2.5) - 6.5).abs() < 1e-6);
        assert!((dl.read_cubic(2.5) - 6.5).abs() < 1e-5);
        assert_eq!(dl.read_int(1000), dl.read_int(dl.max_delay() + 3));
        assert!(dl.read(f32::NAN).is_finite());
        dl.reset();
        assert_eq!(dl.read(3.0), 0.0);
    }
}
