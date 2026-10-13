//! Backend VST3 (in costruzione).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{ExternalInstance, FormatBackend, HostError, ParsedKind, PluginFormat, PluginInfo};

/// Backend del formato VST3.
#[derive(Default)]
pub struct Vst3Backend;

impl Vst3Backend {
    pub fn new() -> Self {
        Self
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
        Err(HostError::Unsupported(format!(
            "VST3 non ancora implementato: {}",
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
            "VST3 non ancora implementato: {}",
            kind.path.display()
        )))
    }
}
