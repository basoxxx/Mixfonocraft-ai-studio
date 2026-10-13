//! Aggancio per i plugin esterni (VST3/CLAP).
//!
//! `mfc-plugins` non dipende dall'host: l'applicazione registra un [`ExternalHost`]
//! all'avvio (crate `mfc-host`) e `create_generator` / `create_effect` gli delegano i kind
//! con prefisso `vst3:` / `clap:`.

use std::sync::{Arc, RwLock};

use mfc_core::PluginInstance;

use crate::api::{Effect, Generator};

/// Fabbrica di generatori/effetti per plugin esterni.
pub trait ExternalHost: Send + Sync {
    /// Crea (o riusa, tramite `instance_id`) un generatore. `None` = non disponibile.
    fn create_generator(&self, inst: &PluginInstance, sample_rate: f32) -> Option<Box<dyn Generator>>;
    /// Crea (o riusa) un effetto. `None` = non disponibile (lo slot viene bypassato).
    fn create_effect(&self, inst: &PluginInstance, sample_rate: f32) -> Option<Box<dyn Effect>>;
}

static HOST: RwLock<Option<Arc<dyn ExternalHost>>> = RwLock::new(None);

/// Registra (o rimuove con `None`) l'host dei plugin esterni.
pub fn set_external_host(host: Option<Arc<dyn ExternalHost>>) {
    if let Ok(mut h) = HOST.write() {
        *h = host;
    }
}

/// Host registrato, se presente.
pub fn external_host() -> Option<Arc<dyn ExternalHost>> {
    HOST.read().ok().and_then(|h| h.clone())
}
