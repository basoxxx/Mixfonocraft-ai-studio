//! Filtri: state-variable TPT (Zavalishin), biquad RBJ, one-pole e DC blocker.

use crate::util::flush_denormal;
use std::f64::consts::PI as PI64;

#[inline]
fn sane(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback
    }
}

/// Uscita selezionata dello [`Svf`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvfMode {
    #[default]
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
    Peak,
}

/// Tutte le uscite simultanee dello [`Svf`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SvfOutput {
    pub low: f32,
    pub band: f32,
    pub high: f32,
}

/// Filtro state-variable TPT (topology-preserving): stabile anche con cutoff modulato per campione.
#[derive(Debug, Clone, Copy)]
pub struct Svf {
    mode: SvfMode,
    sample_rate: f32,
    cutoff: f32,
    q: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    pub fn new(sample_rate: f32) -> Self {
        let mut f = Self {
            mode: SvfMode::Lowpass,
            sample_rate: sample_rate.max(1.0),
            cutoff: 1000.0,
            q: std::f32::consts::FRAC_1_SQRT_2,
            k: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: 0.0,
            ic2: 0.0,
        };
        f.update();
        f
    }

    fn update(&mut self) {
        let fc = self.cutoff.min(0.49 * self.sample_rate);
        let g = (std::f32::consts::PI * fc / self.sample_rate).tan();
        self.k = 1.0 / self.q;
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.update();
    }

    pub fn set_mode(&mut self, mode: SvfMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> SvfMode {
        self.mode
    }

    /// Frequenza di taglio in Hz (economico: chiamabile ogni campione).
    #[inline]
    pub fn set_cutoff(&mut self, hz: f32) {
        self.cutoff = sane(hz, 1.0, 1e6, 1000.0);
        self.update();
    }

    pub fn cutoff(&self) -> f32 {
        self.cutoff
    }

    /// Fattore di merito Q (0.1..=100).
    pub fn set_q(&mut self, q: f32) {
        self.q = sane(q, 0.1, 100.0, std::f32::consts::FRAC_1_SQRT_2);
        self.update();
    }

    /// Risonanza 0..=1 mappata esponenzialmente su Q 0.707..≈21.
    pub fn set_resonance(&mut self, res: f32) {
        let r = sane(res, 0.0, 1.0, 0.0);
        self.set_q(std::f32::consts::FRAC_1_SQRT_2 * 30f32.powf(r));
    }

    /// Imposta cutoff e Q in un'unica passata.
    pub fn set(&mut self, cutoff: f32, q: f32) {
        self.cutoff = sane(cutoff, 1.0, 1e6, 1000.0);
        self.q = sane(q, 0.1, 100.0, std::f32::consts::FRAC_1_SQRT_2);
        self.update();
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Elabora un campione restituendo tutte le uscite.
    #[inline]
    pub fn tick(&mut self, x: f32) -> SvfOutput {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = flush_denormal(2.0 * v1 - self.ic1);
        self.ic2 = flush_denormal(2.0 * v2 - self.ic2);
        SvfOutput {
            low: v2,
            band: v1,
            high: x - self.k * v1 - v2,
        }
    }

    /// Elabora un campione con l'uscita del modo corrente.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let o = self.tick(x);
        match self.mode {
            SvfMode::Lowpass => o.low,
            SvfMode::Highpass => o.high,
            SvfMode::Bandpass => o.band,
            SvfMode::Notch => o.low + o.high,
            SvfMode::Peak => o.low - o.high,
        }
    }
}

impl Default for Svf {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

/// Tipo di biquad (formule RBJ "Audio EQ Cookbook").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BiquadKind {
    #[default]
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
    Peaking,
    LowShelf,
    HighShelf,
    Allpass,
}

/// Biquad in forma trasposta diretta II.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
    sample_rate: f32,
}

impl Biquad {
    /// Filtro passante (identità).
    pub fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
            sample_rate: 48_000.0,
        }
    }

    /// Crea un filtro già configurato.
    pub fn with(kind: BiquadKind, freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Self {
        let mut b = Self::new();
        b.set(kind, freq, q, gain_db, sample_rate);
        b
    }

    /// Riporta il filtro all'identità (lo stato è preservato).
    pub fn set_identity(&mut self) {
        self.b0 = 1.0;
        self.b1 = 0.0;
        self.b2 = 0.0;
        self.a1 = 0.0;
        self.a2 = 0.0;
    }

    /// Calcola i coefficienti. `gain_db` è usato solo da peaking/shelf. Non resetta lo stato.
    pub fn set(&mut self, kind: BiquadKind, freq: f32, q: f32, gain_db: f32, sample_rate: f32) {
        let sr = sane(sample_rate, 1.0, 1e7, 48_000.0) as f64;
        self.sample_rate = sr as f32;
        let f = sane(freq, 1.0, 1e7, 1000.0).min(0.499 * sr as f32) as f64;
        let q = sane(q, 0.025, 100.0, 0.707) as f64;
        let gain = sane(gain_db, -96.0, 96.0, 0.0) as f64;
        let a = 10f64.powf(gain / 40.0);
        let w0 = 2.0 * PI64 * f / sr;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            BiquadKind::Lowpass => {
                let b = (1.0 - cs) / 2.0;
                (b, 1.0 - cs, b, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
            }
            BiquadKind::Highpass => {
                let b = (1.0 + cs) / 2.0;
                (b, -(1.0 + cs), b, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
            }
            BiquadKind::Bandpass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BiquadKind::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BiquadKind::Allpass => (1.0 - alpha, -2.0 * cs, 1.0 + alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BiquadKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            BiquadKind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cs + sa),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - sa),
                (a + 1.0) + (a - 1.0) * cs + sa,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - sa,
            ),
            BiquadKind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cs + sa),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - sa),
                (a + 1.0) - (a - 1.0) * cs + sa,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - sa,
            ),
        };
        self.b0 = (b0 / a0) as f32;
        self.b1 = (b1 / a0) as f32;
        self.b2 = (b2 / a0) as f32;
        self.a1 = (a1 / a0) as f32;
        self.a2 = (a2 / a0) as f32;
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = flush_denormal(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = flush_denormal(self.b2 * x - self.a2 * y);
        y
    }

    /// Risposta in ampiezza (dB) alla frequenza `freq`, per test e visualizzazione.
    pub fn magnitude_db_at(&self, freq: f32) -> f32 {
        let w = 2.0 * PI64 * freq as f64 / self.sample_rate as f64;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (b0, b1, b2, a1, a2) = (
            self.b0 as f64,
            self.b1 as f64,
            self.b2 as f64,
            self.a1 as f64,
            self.a2 as f64,
        );
        // z^-1 = cos(w) - j sin(w)
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -(b1 * s1 + b2 * s2);
        let dr = 1.0 + a1 * c1 + a2 * c2;
        let di = -(a1 * s1 + a2 * s2);
        let mag2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-300);
        (10.0 * mag2.max(1e-30).log10()) as f32
    }
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new()
    }
}

/// Filtro a un polo (6 dB/ottava): passa-basso e passa-alto complementare.
#[derive(Debug, Clone, Copy, Default)]
pub struct OnePole {
    a: f32,
    z: f32,
}

impl OnePole {
    pub fn new(cutoff: f32, sample_rate: f32) -> Self {
        let mut f = Self::default();
        f.set_cutoff(cutoff, sample_rate);
        f
    }

    pub fn set_cutoff(&mut self, cutoff: f32, sample_rate: f32) {
        let sr = sane(sample_rate, 1.0, 1e7, 48_000.0);
        let fc = sane(cutoff, 0.0, 0.5 * sr, 1000.0);
        self.a = 1.0 - (-std::f32::consts::TAU * fc / sr).exp();
    }

    pub fn reset(&mut self) {
        self.z = 0.0;
    }

    #[inline]
    pub fn lowpass(&mut self, x: f32) -> f32 {
        self.z = flush_denormal(self.z + self.a * (x - self.z));
        self.z
    }

    #[inline]
    pub fn highpass(&mut self, x: f32) -> f32 {
        x - self.lowpass(x)
    }
}

/// Rimuove la componente continua (passa-alto ≈ 10 Hz).
#[derive(Debug, Clone, Copy)]
pub struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    pub fn new(sample_rate: f32) -> Self {
        let mut d = Self {
            r: 0.995,
            x1: 0.0,
            y1: 0.0,
        };
        d.set_sample_rate(sample_rate);
        d
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sane(sample_rate, 100.0, 1e7, 48_000.0);
        self.r = 1.0 - std::f32::consts::TAU * 10.0 / sr;
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = flush_denormal(y);
        y
    }
}

impl Default for DcBlocker {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Guadagno misurato (dB) su un seno stazionario.
    fn measure(mut f: impl FnMut(f32) -> f32, hz: f32) -> f32 {
        let n = 48_000;
        let mut peak = 0.0f32;
        for i in 0..n {
            let x = (std::f32::consts::TAU * hz * i as f32 / SR).sin();
            let y = f(x);
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        crate::util::gain_to_db(peak)
    }

    #[test]
    fn biquad_lowpass_response() {
        let b = Biquad::with(BiquadKind::Lowpass, 1000.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, SR);
        assert!((b.magnitude_db_at(1000.0) + 3.01).abs() < 0.1);
        assert!(b.magnitude_db_at(100.0).abs() < 0.1);
        assert!(b.magnitude_db_at(10_000.0) < -20.0);
        let mut b2 = b;
        assert!((measure(|x| b2.process(x), 1000.0) + 3.0).abs() < 0.2);
        let mut b3 = b;
        assert!(measure(|x| b3.process(x), 10_000.0) < -20.0);
    }

    #[test]
    fn biquad_other_kinds() {
        let hp = Biquad::with(BiquadKind::Highpass, 1000.0, 0.707, 0.0, SR);
        assert!(hp.magnitude_db_at(100.0) < -20.0 && hp.magnitude_db_at(10_000.0).abs() < 0.5);
        let pk = Biquad::with(BiquadKind::Peaking, 1000.0, 1.0, 12.0, SR);
        assert!((pk.magnitude_db_at(1000.0) - 12.0).abs() < 0.05);
        assert!(pk.magnitude_db_at(50.0).abs() < 0.5);
        let ls = Biquad::with(BiquadKind::LowShelf, 200.0, 0.707, -6.0, SR);
        assert!((ls.magnitude_db_at(20.0) + 6.0).abs() < 0.3 && ls.magnitude_db_at(5000.0).abs() < 0.3);
        let hs = Biquad::with(BiquadKind::HighShelf, 2000.0, 0.707, 6.0, SR);
        assert!((hs.magnitude_db_at(15_000.0) - 6.0).abs() < 0.5 && hs.magnitude_db_at(50.0).abs() < 0.2);
        let n = Biquad::with(BiquadKind::Notch, 1000.0, 2.0, 0.0, SR);
        assert!(n.magnitude_db_at(1000.0) < -40.0);
        let bp = Biquad::with(BiquadKind::Bandpass, 1000.0, 2.0, 0.0, SR);
        assert!(bp.magnitude_db_at(1000.0).abs() < 0.1 && bp.magnitude_db_at(100.0) < -20.0);
        let ap = Biquad::with(BiquadKind::Allpass, 1000.0, 0.707, 0.0, SR);
        for f in [50.0, 1000.0, 15_000.0] {
            assert!(ap.magnitude_db_at(f).abs() < 0.01);
        }
    }

    #[test]
    fn svf_responses() {
        for (mode, f, lo, hi) in [
            (SvfMode::Lowpass, 100.0, -0.5, 0.5),
            (SvfMode::Lowpass, 1000.0, -3.5, -2.5),
            (SvfMode::Lowpass, 10_000.0, -50.0, -20.0),
            (SvfMode::Highpass, 100.0, -60.0, -20.0),
            (SvfMode::Highpass, 10_000.0, -0.5, 0.5),
            (SvfMode::Bandpass, 1000.0, -3.5, -2.5),
            (SvfMode::Notch, 1000.0, -200.0, -30.0),
        ] {
            let mut s = Svf::new(SR);
            s.set_mode(mode);
            s.set(1000.0, std::f32::consts::FRAC_1_SQRT_2);
            let db = measure(|x| s.process(x), f);
            assert!(db > lo && db < hi, "{mode:?} @ {f}: {db}");
        }
    }

    #[test]
    fn svf_modulated_is_stable() {
        let mut s = Svf::new(SR);
        s.set_resonance(1.0);
        let mut rng = crate::osc::Rng::new(3);
        for i in 0..96_000 {
            s.set_cutoff(20.0 + 20_000.0 * (0.5 + 0.5 * (i as f32 * 0.01).sin()));
            let y = s.process(rng.next_bipolar());
            assert!(y.is_finite() && y.abs() < 100.0);
        }
    }

    #[test]
    fn one_pole_and_dc_blocker() {
        let mut lp = OnePole::new(1000.0, SR);
        let mut hp = OnePole::new(1000.0, SR);
        assert!(measure(|x| lp.lowpass(x), 50.0).abs() < 0.1);
        assert!(measure(|x| hp.highpass(x), 50.0) < -20.0);
        let mut dc = DcBlocker::new(SR);
        let mut y = 1.0;
        for _ in 0..48_000 {
            y = dc.process(1.0);
        }
        assert!(y.abs() < 1e-3);
    }
}
