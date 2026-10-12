//! Generatori nativi (strumenti del Channel Rack).

pub mod boobass;
pub mod drumsynth;
pub mod kick;
pub mod osc3;
pub mod sampler;
pub mod silent;
pub mod supersaw;

use crate::api::{Generator, PluginDescriptor};
use crate::ids;
use mfc_core::PluginInstance;

/// Sample rate iniziale prima di `set_sample_rate`.
pub(crate) const DEFAULT_SR: f32 = 48_000.0;

/// Etichette per i parametri booleani.
pub(crate) const OFF_ON: &[&str] = &["off", "on"];

/// Crea il generatore indicato da `inst.kind` (sconosciuto → silenzioso).
/// Per sampler e audio clip carica `inst.sample_path` (fallimento → silenzioso). Non real-time.
pub fn create(inst: &PluginInstance) -> Box<dyn Generator> {
    let load = || inst.sample_path.as_deref().and_then(|p| crate::sample::load_sample(p).ok());
    match inst.kind.as_str() {
        ids::SAMPLER => Box::new(sampler::Sampler::new(load())),
        ids::AUDIO_CLIP => Box::new(sampler::Sampler::new_audio_clip(load())),
        ids::OSC3 => Box::new(osc3::Osc3::new()),
        ids::BOOBASS => Box::new(boobass::BooBass::new()),
        ids::KICK => Box::new(kick::Kick::new()),
        ids::DRUMSYNTH => Box::new(drumsynth::DrumSynth::new()),
        ids::SUPERSAW => Box::new(supersaw::Supersaw::new()),
        _ => Box::new(silent::Silent::new()),
    }
}

/// Valida un parametro: nome noto, valore finito, limitato al range (arrotondato se a scelta).
pub(crate) fn param_value(desc: &PluginDescriptor, name: &str, value: f32) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    let p = desc.param(name)?;
    let v = p.clamp(value);
    Some(if p.choices.is_empty() { v } else { v.round() })
}

/// Applica tutti i default del descrittore tramite `set`.
pub(crate) fn apply_defaults(desc: &PluginDescriptor, mut set: impl FnMut(&str, f32)) {
    for p in desc.params {
        set(p.name, p.default);
    }
}

/// Stato di allocazione comune a tutte le voci.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VoiceTag {
    pub active: bool,
    pub released: bool,
    pub id: u64,
    /// Contatore di avvio (più basso = più vecchia).
    pub order: u64,
}

pub(crate) trait PolyVoice {
    fn tag(&self) -> &VoiceTag;
}

/// Slot per una nuova nota: prima voce libera, altrimenti la più vecchia in release, altrimenti la più vecchia.
pub(crate) fn pick_voice<V: PolyVoice>(voices: &[V]) -> usize {
    let mut best = 0;
    let mut best_score = (u8::MAX, u64::MAX);
    for (i, v) in voices.iter().enumerate() {
        let t = v.tag();
        if !t.active {
            return i;
        }
        let score = (u8::from(!t.released), t.order);
        if score < best_score {
            best_score = score;
            best = i;
        }
    }
    best
}

/// Numero di voci attive.
pub(crate) fn count_active<V: PolyVoice>(voices: &[V]) -> usize {
    voices.iter().filter(|v| v.tag().active).count()
}

/// Lunghezza comune dei due buffer.
#[inline]
pub(crate) fn frames(left: &[f32], right: &[f32]) -> usize {
    left.len().min(right.len())
}

#[cfg(test)]
pub(crate) mod test_util {
    /// Stima la frequenza dominante contando gli attraversamenti dello zero (ascendenti).
    pub fn zero_cross_freq(x: &[f32], sample_rate: f32) -> f32 {
        let mut first = None;
        let mut last = 0;
        let mut count = 0usize;
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
