//! Backend CLAP basato su `clack-host`.
//!
//! - [`ClapBackend`]: scansione dei file `.clap` (bundle su macOS) e creazione delle istanze.
//! - [`instance::ClapInstance`]: istanza attiva che implementa [`ExternalInstance`].
//! - [`handlers`]: callback dell'host verso il plugin (flag atomici gestiti in `idle`).

mod handlers;
mod instance;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clack_host::prelude::PluginEntry;

use crate::{ExternalInstance, FormatBackend, HostError, ParsedKind, PluginFormat, PluginInfo};

pub use instance::ClapInstance;

/// Backend del formato CLAP.
#[derive(Default)]
pub struct ClapBackend;

impl ClapBackend {
    pub fn new() -> Self {
        Self
    }
}

/// Carica (o riusa dalla cache di clack) l'entry di un file/bundle `.clap`.
pub(crate) fn load_entry(path: &Path) -> Result<PluginEntry, HostError> {
    // SAFETY: caricare una libreria esterna è intrinsecamente non verificabile; il file è
    // scelto dall'utente come plugin CLAP.
    unsafe { PluginEntry::load(path) }.map_err(|e| HostError::Load {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

fn cstr_opt(s: Option<&std::ffi::CStr>) -> String {
    s.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Descrittori dei plugin contenuti nell'entry.
pub(crate) fn describe(entry: &PluginEntry, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| HostError::Plugin(format!("{}: nessuna plugin factory", path.display())))?;
    let mut out = Vec::new();
    for d in factory.plugin_descriptors() {
        let Some(id) = d.id().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let features: Vec<String> = d.features().map(|f| f.to_string_lossy().into_owned()).collect();
        let has = |f: &str| features.iter().any(|x| x.eq_ignore_ascii_case(f));
        let name = cstr_opt(d.name());
        out.push(PluginInfo {
            kind: crate::make_kind(PluginFormat::Clap, path, &id),
            format: PluginFormat::Clap,
            path: path.to_path_buf(),
            name: if name.is_empty() { id.clone() } else { name },
            id,
            vendor: cstr_opt(d.vendor()),
            version: cstr_opt(d.version()),
            category: features.join("|"),
            is_instrument: has("instrument"),
            is_effect: has("audio-effect"),
        });
    }
    Ok(out)
}

impl FormatBackend for ClapBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }

    fn default_search_paths(&self) -> Vec<PathBuf> {
        crate::scan::clap_default_paths()
    }

    fn is_candidate(&self, path: &Path) -> bool {
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("clap")) {
            return false;
        }
        // macOS: bundle (cartella); altrove: file di libreria.
        if cfg!(target_os = "macos") {
            path.is_dir() || path.is_file()
        } else {
            path.is_file()
        }
    }

    fn scan(&self, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
        if !path.exists() {
            return Err(HostError::Load {
                path: path.to_path_buf(),
                reason: "file inesistente".into(),
            });
        }
        let entry = load_entry(path)?;
        describe(&entry, path)
    }

    fn instantiate(
        &self,
        kind: &ParsedKind,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<Arc<dyn ExternalInstance>, HostError> {
        if kind.format != PluginFormat::Clap {
            return Err(HostError::BadKind(kind.id.clone()));
        }
        let inst = ClapInstance::new(&kind.path, &kind.id, sample_rate, max_block)?;
        Ok(Arc::new(inst))
    }
}
