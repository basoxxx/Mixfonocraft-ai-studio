//! # mfc-dsp
//!
//! Primitive DSP real-time safe in `f32`: conversioni, oscillatori band-limited,
//! inviluppi, filtri, LFO, delay line, interpolazione, dinamica e misuratori.
//!
//! Regola generale: le allocazioni avvengono solo nei costruttori (es. [`delay::DelayLine::new`]);
//! i metodi per campione (`next_sample`, `process`, `tick`...) non allocano né bloccano.

pub mod delay;
pub mod dynamics;
pub mod env;
pub mod filter;
pub mod lfo;
pub mod meter;
pub mod osc;
pub mod resample;
pub mod util;

pub use delay::DelayLine;
pub use dynamics::{DetectorMode, EnvelopeFollower, GainComputer};
pub use env::{Adsr, ExpDecay, Stage};
pub use filter::{Biquad, BiquadKind, DcBlocker, OnePole, Svf, SvfMode, SvfOutput};
pub use lfo::{Lfo, LfoShape};
pub use meter::Meter;
pub use osc::{Oscillator, Rng, Waveform};
pub use util::*;

#[cfg(test)]
pub(crate) mod test_util {
    /// Conta gli attraversamenti dello zero (da negativo a non negativo) e stima la frequenza.
    pub fn zero_cross_freq(x: &[f32], sample_rate: f32) -> f32 {
        let mut count = 0usize;
        let mut first = None;
        let mut last = 0usize;
        for i in 1..x.len() {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                if first.is_none() {
                    first = Some(i);
                } else {
                    count += 1;
                }
                last = i;
            }
        }
        match first {
            Some(f) if count > 0 => count as f32 * sample_rate / (last - f) as f32,
            _ => 0.0,
        }
    }
}
