//! Interpolazione per la riproduzione di campioni a velocità variabile.

/// Interpolazione lineare tra `a` e `b`.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolazione cubica di Hermite (Catmull-Rom) tra `x0` e `x1`, `t` in 0..1.
#[inline]
pub fn hermite4(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

#[inline]
fn at(buf: &[f32], i: i64) -> f32 {
    if i < 0 {
        0.0
    } else {
        buf.get(i as usize).copied().unwrap_or(0.0)
    }
}

/// Legge `buf` alla posizione frazionaria `pos` (lineare); fuori dai limiti vale 0.
#[inline]
pub fn sample_linear(buf: &[f32], pos: f64) -> f32 {
    if !pos.is_finite() {
        return 0.0;
    }
    let i = pos.floor();
    let t = (pos - i) as f32;
    let i = i as i64;
    lerp(at(buf, i), at(buf, i + 1), t)
}

/// Legge `buf` alla posizione frazionaria `pos` (Hermite 4 punti); fuori dai limiti vale 0.
#[inline]
pub fn sample_hermite(buf: &[f32], pos: f64) -> f32 {
    if !pos.is_finite() {
        return 0.0;
    }
    let i = pos.floor();
    let t = (pos - i) as f32;
    let i = i as i64;
    hermite4(at(buf, i - 1), at(buf, i), at(buf, i + 1), at(buf, i + 2), t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolators_hit_samples_and_lines() {
        let buf: Vec<f32> = (0..10).map(|i| i as f32).collect();
        assert_eq!(sample_linear(&buf, 3.0), 3.0);
        assert!((sample_linear(&buf, 3.25) - 3.25).abs() < 1e-6);
        assert!((sample_hermite(&buf, 4.5) - 4.5).abs() < 1e-5);
        assert_eq!(sample_hermite(&buf, 5.0), 5.0);
        assert_eq!(sample_linear(&buf, -5.0), 0.0);
        assert_eq!(sample_hermite(&buf, 100.0), 0.0);
        assert_eq!(sample_linear(&buf, f64::NAN), 0.0);
    }

    #[test]
    fn hermite_reconstructs_sine_well() {
        let buf: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin()).collect();
        for k in 10..2000 {
            let pos = k as f64 * 0.1;
            let exact = (pos as f32 * 0.1).sin();
            assert!((sample_hermite(&buf, pos) - exact).abs() < 1e-3);
            assert!((sample_linear(&buf, pos) - exact).abs() < 2e-3);
        }
    }
}
