//! Backend CLAP (in costruzione).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{ExternalInstance, FormatBackend, HostError, ParsedKind, PluginFormat, PluginInfo};

/// Backend del formato CLAP.
#[derive(Default)]
pub struct ClapBackend;

impl ClapBackend {
    pub fn new() -> Self {
        Self
    }
}

impl FormatBackend for ClapBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }
    fn default_search_paths(&self) -> Vec<PathBuf> {
        crate::scan::clap_default_paths()
    }
    fn is_candidate(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("clap"))
    }
    fn scan(&self, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
        Err(HostError::Unsupported(format!(
            "CLAP non ancora implementato: {}",
            path.display()
        )))
    }
    fn instantiate(
        &self,
        kind: &ParsedKind,
        _sample_rate: f32,
        _max_block: usize,
    ) -> Result<Arc<dyn ExternalInstance>, HostError> {
        Err(HostError::Unsupported(format!(
            "CLAP non ancora implementato: {}",
            kind.path.display()
        )))
    }
}
