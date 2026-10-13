//! Generatori nativi (strumenti del Channel Rack).

pub mod boobass;
pub mod drumkit;
pub mod drumsynth;
pub mod fusion;
pub mod kick;
pub mod nova;
pub mod osc3;
pub mod prism;
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
    let load = || {
        inst.sample_path
            .as_deref()
            .and_then(|p| crate::sample::load_sample(p).ok())
    };
    match inst.kind.as_str() {
        ids::SAMPLER => Box::new(sampler::Sampler::new(load())),
        ids::AUDIO_CLIP => Box::new(sampler::Sampler::new_audio_clip(load())),
        ids::OSC3 => Box::new(osc3::Osc3::new()),
        ids::BOOBASS => Box::new(boobass::BooBass::new()),
        ids::KICK => Box::new(kick::Kick::new()),
        ids::DRUMSYNTH => Box::new(drumsynth::DrumSynth::new()),
        ids::SUPERSAW => Box::new(supersaw::Supersaw::new()),
        ids::NOVA => Box::new(nova::Nova::new()),
        ids::FUSION => Box::new(fusion::Fusion::new()),
        ids::PRISM => Box::new(prism::Prism::new()),
        ids::DRUMKIT => Box::new(drumkit::Drumkit::new()),
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

/// Note tenute ricordate in modalità mono/legato.
pub(crate) const MONO_STACK: usize = 32;

/// Pila delle note tenute per le modalità mono e legato: (tasto, id, velocity).
#[derive(Debug, Clone, Copy)]
pub(crate) struct MonoStack {
    notes: [(u8, u64, f32); MONO_STACK],
    len: usize,
}

impl MonoStack {
    pub fn new() -> Self {
        Self {
            notes: [(0, 0, 0.0); MONO_STACK],
            len: 0,
        }
    }

    /// Aggiunge una nota (se la pila è piena scarta la più vecchia).
    pub fn push(&mut self, key: u8, id: u64, vel: f32) {
        if self.len == MONO_STACK {
            self.notes.copy_within(1..MONO_STACK, 0);
            self.len -= 1;
        }
        self.notes[self.len] = (key, id, vel);
        self.len += 1;
    }

    /// Rimuove la nota con l'id indicato; `false` se non c'era.
    pub fn remove(&mut self, id: u64) -> bool {
        if let Some(pos) = self.notes[..self.len].iter().position(|e| e.1 == id) {
            self.notes.copy_within(pos + 1..self.len, pos);
            self.len -= 1;
            true
        } else {
            false
        }
    }

    /// Ultima nota ancora tenuta.
    pub fn last(&self) -> Option<(u8, u64, f32)> {
        self.len.checked_sub(1).map(|i| self.notes[i])
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// Modalità di voce comuni ai synth polifonici.
pub(crate) const VOICE_MODES: &[&str] = &["Poly", "Mono", "Legato"];

/// Coefficiente di glide (one-pole) per un blocco di `block` campioni.
pub(crate) fn glide_coef(glide: f32, block: usize, sr: f32) -> f32 {
    if glide > 0.0 {
        1.0 - (-(block as f32) / (glide * sr)).exp()
    } else {
        1.0
    }
}
