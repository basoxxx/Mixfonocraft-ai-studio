//! Backend VST3.
//!
//! - `module`: caricamento del bundle e fabbrica.
//! - [`instance`]: istanza ([`Vst3Instance`]) che implementa [`ExternalInstance`].
//! - `host_ctx`: contesto host (IHostApplication, IComponentHandler, IPlugFrame, IRunLoop).
//! - `com`: liste eventi, code dei parametri e stream in memoria.
//!
//! Interfacce dai binding MIT/Apache del crate `vst3` (coupler-rs).
//!
//! Stato salvato: `"MFV3"`, versione u32, poi lunghezza u64 + dati per lo stato del
//! componente e per quello del controller (little endian).
//!
//! Editor su Linux: i timer e i file descriptor registrati dal plugin con `IRunLoop`
//! vengono serviti in [`ExternalInstance::idle`], che la GUI deve chiamare spesso.

// I tipi degli enum dei binding cambiano per piattaforma (i32 su Windows, u32 altrove):
// i cast servono su almeno una delle due.
#![allow(clippy::unnecessary_cast)]

mod com;
mod host_ctx;
pub mod instance;
pub(crate) mod module;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use instance::Vst3Instance;

use crate::{make_kind, ExternalInstance, FormatBackend, HostError, ParsedKind, PluginFormat, PluginInfo};
use module::{cid_to_hex, hex_to_cid, ClassInfo, Module, AUDIO_CLASS};

/// Backend del formato VST3.
#[derive(Default)]
pub struct Vst3Backend;

impl Vst3Backend {
    pub fn new() -> Self {
        Self
    }
}

/// `PluginInfo` di una classe audio.
fn plugin_info(path: &Path, c: &ClassInfo) -> PluginInfo {
    let id = cid_to_hex(&c.cid);
    let is_instrument = c.sub_categories.contains("Instrument");
    let is_effect = !is_instrument || c.sub_categories.contains("Fx");
    PluginInfo {
        kind: make_kind(PluginFormat::Vst3, path, &id),
        format: PluginFormat::Vst3,
        path: path.to_path_buf(),
        id,
        name: c.name.clone(),
        vendor: c.vendor.clone(),
        version: c.version.clone(),
        category: c.sub_categories.clone(),
        is_instrument,
        is_effect,
    }
}

impl FormatBackend for Vst3Backend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn default_search_paths(&self) -> Vec<PathBuf> {
        crate::scan::vst3_default_paths()
    }

    fn is_candidate(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
    }

    fn scan(&self, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
        let module = Module::load(path)?;
        Ok(module
            .classes()
            .iter()
            .filter(|c| c.category == AUDIO_CLASS)
            .map(|c| plugin_info(path, c))
            .collect())
    }

    fn instantiate(
        &self,
        kind: &ParsedKind,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<Arc<dyn ExternalInstance>, HostError> {
        let cid = hex_to_cid(&kind.id).ok_or_else(|| HostError::BadKind(kind.id.clone()))?;
        let module = Module::load(&kind.path)?;
        let class = module
            .classes()
            .into_iter()
            .find(|c| c.cid == cid && c.category == AUDIO_CLASS)
            .ok_or_else(|| HostError::NotFound(format!("{} in {}", kind.id, kind.path.display())))?;
        let info = plugin_info(&kind.path, &class);
        let inst = Vst3Instance::create(module, info, &cid, sample_rate, max_block)?;
        Ok(inst)
    }
}
