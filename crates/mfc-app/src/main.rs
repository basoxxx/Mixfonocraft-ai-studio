//! Mixfonocraft AI Studio: applicazione desktop. Apre la GUI, opzionalmente con un
//! progetto passato come argomento (doppio click su un file .mfc/.flp/.mid associato).

// In release niente finestra console su Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use mfc_core::Project;

fn load(path: &Path) -> Result<(Project, Option<PathBuf>), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "flp" => mfc_flp::read_file(path)
            .map(|(_, r)| (r.project, None))
            .map_err(|e| e.to_string()),
        "mid" | "midi" => {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            let mut p = Project::new(path.file_stem().and_then(|s| s.to_str()).unwrap_or("MIDI"));
            let imp = mfc_midi::parse_midi(&bytes, p.time.ppq).map_err(|e| e.to_string())?;
            let (pat, _) = mfc_midi::import_into_project(&mut p, &imp, "MIDI").map_err(|e| e.to_string())?;
            p.current_pattern = Some(pat);
            Ok((p, None))
        }
        _ => {
            let txt = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            Project::from_json(&txt)
                .map(|p| (p, Some(path.to_path_buf())))
                .map_err(|e| e.to_string())
        }
    }
}

fn main() {
    // Ignora argomenti di sistema (es. -psn_ su macOS).
    let file = std::env::args_os().skip(1).map(PathBuf::from).find(|p| p.is_file());
    let (project, path) = match file.as_deref().map(load) {
        Some(Ok(loaded)) => loaded,
        Some(Err(e)) => {
            eprintln!("impossibile aprire il progetto: {e}");
            (Project::new("Untitled"), None)
        }
        None => (Project::new("Untitled"), None),
    };
    if let Err(e) = mfc_gui::run(project, path) {
        eprintln!("errore della finestra: {e}");
        std::process::exit(1);
    }
}
