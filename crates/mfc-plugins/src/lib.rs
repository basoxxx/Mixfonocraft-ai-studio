//! # mfc-plugins
//!
//! Generatori ed effetti nativi (clean-room, ispirati ai plugin di FL Studio).
//! Vedi [`api`] per il contratto e [`registry`] per l'elenco.

pub mod api;
pub mod effects;
pub mod generators;
pub mod sample;

pub use api::*;
pub use sample::{load_sample, SampleError};

use mfc_core::PluginInstance;

/// Id stabili dei plugin nativi.
pub mod ids {
    // Generatori
    pub const SAMPLER: &str = "mfc.sampler";
    pub const AUDIO_CLIP: &str = "mfc.audioclip";
    pub const OSC3: &str = "mfc.3xosc";
    pub const BOOBASS: &str = "mfc.boobass";
    pub const KICK: &str = "mfc.kick";
    pub const DRUMSYNTH: &str = "mfc.drumsynth";
    pub const SUPERSAW: &str = "mfc.supersaw";
    pub const SILENT: &str = "mfc.silent";
    pub const FM: &str = "mfc.fm";
    pub const PLUCK: &str = "mfc.pluck";
    pub const WAVETABLE: &str = "mfc.wavetable";
    // Effetti
    pub const EQ: &str = "mfc.eq";
    pub const COMPRESSOR: &str = "mfc.compressor";
    pub const LIMITER: &str = "mfc.limiter";
    pub const GATE: &str = "mfc.gate";
    pub const REVERB: &str = "mfc.reverb";
    pub const DELAY: &str = "mfc.delay";
    pub const CHORUS: &str = "mfc.chorus";
    pub const FLANGER: &str = "mfc.flanger";
    pub const PHASER: &str = "mfc.phaser";
    pub const DISTORTION: &str = "mfc.distortion";
    pub const BITCRUSHER: &str = "mfc.bitcrusher";
    pub const FILTER: &str = "mfc.filter";
    pub const STEREO: &str = "mfc.stereo";
    pub const GAIN: &str = "mfc.gain";
    pub const DUCKER: &str = "mfc.ducker";
    pub const AUTOPAN: &str = "mfc.autopan";
    pub const TRANSIENT: &str = "mfc.transient";
}

/// Tutti i descrittori registrati (generatori poi effetti).
pub fn registry() -> &'static [&'static PluginDescriptor] {
    REGISTRY
}

/// Descrittore per id.
pub fn descriptor(id: &str) -> Option<&'static PluginDescriptor> {
    REGISTRY.iter().copied().find(|d| d.id == id)
}

/// Istanza `PluginInstance` con tutti i parametri ai valori di default.
pub fn default_instance(id: &str) -> Option<PluginInstance> {
    let d = descriptor(id)?;
    let mut inst = PluginInstance::new(id);
    for p in d.params {
        inst.params.insert(p.name.to_string(), p.default);
    }
    Some(inst)
}

/// Crea un generatore da un'istanza del progetto, applicando i parametri e caricando
/// l'eventuale campione (`sample_path`). Plugin sconosciuti → generatore silenzioso.
/// NON chiamare dal thread audio (alloca, legge da disco).
pub fn create_generator(inst: &PluginInstance, sample_rate: f32) -> Box<dyn Generator> {
    let mut g = generators::create(inst);
    g.set_sample_rate(sample_rate);
    for (k, v) in &inst.params {
        g.set_param(k, *v);
    }
    g
}

/// Crea un effetto; `None` se l'id non è un effetto noto (lo slot viene bypassato).
pub fn create_effect(inst: &PluginInstance, sample_rate: f32) -> Option<Box<dyn Effect>> {
    let mut fx = effects::create(&inst.kind)?;
    fx.set_sample_rate(sample_rate);
    for (k, v) in &inst.params {
        fx.set_param(k, *v);
    }
    Some(fx)
}

static REGISTRY: &[&PluginDescriptor] = &[
    &generators::sampler::DESCRIPTOR,
    &generators::sampler::AUDIO_CLIP_DESCRIPTOR,
    &generators::osc3::DESCRIPTOR,
    &generators::boobass::DESCRIPTOR,
    &generators::kick::DESCRIPTOR,
    &generators::drumsynth::DESCRIPTOR,
    &generators::supersaw::DESCRIPTOR,
    &generators::fm::DESCRIPTOR,
    &generators::pluck::DESCRIPTOR,
    &generators::wavetable::DESCRIPTOR,
    &generators::silent::DESCRIPTOR,
    &effects::eq::DESCRIPTOR,
    &effects::compressor::DESCRIPTOR,
    &effects::limiter::DESCRIPTOR,
    &effects::gate::DESCRIPTOR,
    &effects::reverb::DESCRIPTOR,
    &effects::delay::DESCRIPTOR,
    &effects::chorus::DESCRIPTOR,
    &effects::flanger::DESCRIPTOR,
    &effects::phaser::DESCRIPTOR,
    &effects::distortion::DESCRIPTOR,
    &effects::bitcrusher::DESCRIPTOR,
    &effects::filter::DESCRIPTOR,
    &effects::stereo::DESCRIPTOR,
    &effects::gain::DESCRIPTOR,
    &effects::ducker::DESCRIPTOR,
    &effects::autopan::DESCRIPTOR,
    &effects::transient::DESCRIPTOR,
];
