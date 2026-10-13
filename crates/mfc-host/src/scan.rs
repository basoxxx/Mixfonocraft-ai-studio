//! Ricerca dei plugin su disco.
//!
//! Le cartelle standard seguono le specifiche dei formati:
//! - VST3: <https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical+Documentation/Locations+Format/Plugin+Locations.html>
//! - CLAP: `clap/entry.h` (CLAP_PATH e cartelle di sistema/utente).

use std::path::{Path, PathBuf};

use crate::{FormatBackend, PluginInfo};

/// Esito di una scansione.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub plugins: Vec<PluginInfo>,
    /// (percorso, errore) per i bundle che non è stato possibile leggere.
    pub errors: Vec<(PathBuf, String)>,
    /// Bundle/file candidati esaminati.
    pub scanned: usize,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

/// Cartelle standard VST3 del sistema operativo corrente.
pub fn vst3_default_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        if let Some(h) = home() {
            v.push(h.join("Library/Audio/Plug-Ins/VST3"));
        }
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        v.push(PathBuf::from("/Network/Library/Audio/Plug-Ins/VST3"));
    } else if cfg!(windows) {
        if let Some(p) = env_path("LOCALAPPDATA") {
            v.push(p.join("Programs/Common/VST3"));
        }
        if let Some(p) = env_path("COMMONPROGRAMFILES") {
            v.push(p.join("VST3"));
        }
    } else {
        if let Some(h) = home() {
            v.push(h.join(".vst3"));
        }
        v.push(PathBuf::from("/usr/lib/vst3"));
        v.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    v
}

/// Cartelle standard CLAP del sistema operativo corrente (più `CLAP_PATH`).
pub fn clap_default_paths() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("CLAP_PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        if let Some(h) = home() {
            v.push(h.join("Library/Audio/Plug-Ins/CLAP"));
        }
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    } else if cfg!(windows) {
        if let Some(p) = env_path("LOCALAPPDATA") {
            v.push(p.join("Programs/Common/CLAP"));
        }
        if let Some(p) = env_path("COMMONPROGRAMFILES") {
            v.push(p.join("CLAP"));
        }
    } else {
        if let Some(h) = home() {
            v.push(h.join(".clap"));
        }
        v.push(PathBuf::from("/usr/lib/clap"));
    }
    v
}

/// Trova i candidati sotto `root` (ricorsivo, senza entrare nei bundle trovati).
pub fn find_candidates(backends: &[&dyn FormatBackend], root: &Path, max_depth: usize) -> Vec<(usize, PathBuf)> {
    let mut out = Vec::new();
    walk(backends, root, max_depth, &mut out);
    out
}

fn walk(backends: &[&dyn FormatBackend], dir: &Path, depth: usize, out: &mut Vec<(usize, PathBuf)>) {
    if let Some(i) = backends.iter().position(|b| b.is_candidate(dir)) {
        out.push((i, dir.to_path_buf()));
        return;
    }
    if depth == 0 || !dir.is_dir() {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(backends, &p, depth - 1, out);
        } else if let Some(i) = backends.iter().position(|b| b.is_candidate(&p)) {
            out.push((i, p));
        }
    }
}

/// Scansiona le cartelle indicate con tutti i backend.
pub fn scan_paths(backends: &[&dyn FormatBackend], roots: &[PathBuf]) -> ScanReport {
    let mut report = ScanReport::default();
    for root in roots {
        for (i, path) in find_candidates(backends, root, 6) {
            report.scanned += 1;
            match backends[i].scan(&path) {
                Ok(found) => {
                    for p in found {
                        if !report.plugins.iter().any(|q| q.kind == p.kind) {
                            report.plugins.push(p);
                        }
                    }
                }
                Err(e) => report.errors.push((path, e.to_string())),
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExternalInstance, HostError, ParsedKind, PluginFormat};
    use std::sync::Arc;

    struct Ext(&'static str, PluginFormat);

    impl FormatBackend for Ext {
        fn format(&self) -> PluginFormat {
            self.1
        }
        fn default_search_paths(&self) -> Vec<PathBuf> {
            vec![]
        }
        fn is_candidate(&self, path: &Path) -> bool {
            path.extension().is_some_and(|e| e == self.0)
        }
        fn scan(&self, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
            if path.to_string_lossy().contains("broken") {
                return Err(HostError::Plugin("rotto".into()));
            }
            Ok(vec![PluginInfo {
                kind: crate::make_kind(self.1, path, "id"),
                format: self.1,
                path: path.to_path_buf(),
                id: "id".into(),
                name: path.file_stem().unwrap().to_string_lossy().into(),
                vendor: String::new(),
                version: String::new(),
                category: String::new(),
                is_instrument: true,
                is_effect: false,
            }])
        }
        fn instantiate(&self, _: &ParsedKind, _: f32, _: usize) -> Result<Arc<dyn ExternalInstance>, HostError> {
            Err(HostError::Unsupported("test".into()))
        }
    }

    #[test]
    fn walks_folders_and_stops_at_bundles() {
        let root = std::env::temp_dir().join(format!("mfc_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Bundle VST3 (cartella) con contenuto annidato da NON esplorare.
        std::fs::create_dir_all(root.join("Vendor/Synth.vst3/Contents/x86_64-linux")).unwrap();
        std::fs::write(root.join("Vendor/Synth.vst3/Contents/x86_64-linux/Inner.vst3"), b"").unwrap();
        std::fs::write(root.join("Delay.clap"), b"").unwrap();
        std::fs::write(root.join("broken.clap"), b"").unwrap();
        std::fs::write(root.join("readme.txt"), b"").unwrap();
        let v = Ext("vst3", PluginFormat::Vst3);
        let c = Ext("clap", PluginFormat::Clap);
        let backends: Vec<&dyn FormatBackend> = vec![&v, &c];
        let r = scan_paths(&backends, std::slice::from_ref(&root));
        let mut names: Vec<_> = r.plugins.iter().map(|p| p.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["Delay", "Synth"]);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.scanned, 3);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn default_paths_exist_for_both_formats() {
        assert!(!vst3_default_paths().is_empty());
        assert!(!clap_default_paths().is_empty());
    }
}
