//! Effetti nativi (slot degli insert del mixer).

pub mod bitcrusher;
pub mod chorus;
pub mod compressor;
pub mod delay;
pub mod distortion;
pub mod eq;
pub mod filter;
pub mod flanger;
pub mod gain;
pub mod gate;
pub mod limiter;
pub mod phaser;
pub mod reverb;
pub mod stereo;

use crate::api::Effect;
use crate::ids;
use mfc_dsp::DelayLine;

pub(crate) use crate::generators::{apply_defaults, frames, param_value, DEFAULT_SR, OFF_ON};

/// Sample rate massimo per cui i buffer sono preallocati nel costruttore.
pub(crate) const MAX_SR: f32 = 192_000.0;

/// Crea un effetto per id; `None` se sconosciuto.
pub fn create(id: &str) -> Option<Box<dyn Effect>> {
    Some(match id {
        ids::EQ => Box::new(eq::Eq::new()),
        ids::COMPRESSOR => Box::new(compressor::Compressor::new()),
        ids::LIMITER => Box::new(limiter::Limiter::new()),
        ids::GATE => Box::new(gate::Gate::new()),
        ids::REVERB => Box::new(reverb::Reverb::new()),
        ids::DELAY => Box::new(delay::Delay::new()),
        ids::CHORUS => Box::new(chorus::Chorus::new()),
        ids::FLANGER => Box::new(flanger::Flanger::new()),
        ids::PHASER => Box::new(phaser::Phaser::new()),
        ids::DISTORTION => Box::new(distortion::Distortion::new()),
        ids::BITCRUSHER => Box::new(bitcrusher::Bitcrusher::new()),
        ids::FILTER => Box::new(filter::Filter::new()),
        ids::STEREO => Box::new(stereo::Stereo::new()),
        ids::GAIN => Box::new(gain::Gain::new()),
        _ => return None,
    })
}

/// Sample rate accettabile.
#[inline]
pub(crate) fn sr_ok(sr: f32) -> bool {
    sr.is_finite() && (1_000.0..=1_536_000.0).contains(&sr)
}

/// Campioni necessari per `seconds` al sample rate massimo tra `sr` e [`MAX_SR`].
pub(crate) fn capacity(seconds: f32, sr: f32) -> usize {
    (seconds * sr.max(MAX_SR)).ceil() as usize + 8
}

/// Garantisce che la delay line contenga almeno `seconds` a `sr` (rialloca solo se serve, fuori dal thread audio).
pub(crate) fn ensure_delay(dl: &mut DelayLine, seconds: f32, sr: f32) {
    let need = capacity(seconds, sr);
    if dl.max_delay() < need {
        *dl = DelayLine::new(need);
    } else {
        dl.reset();
    }
}
