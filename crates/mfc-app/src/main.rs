//! Mixfonocraft AI Studio: applicazione desktop. Apre la GUI, opzionalmente con un
//! progetto passato come argomento. Su macOS i documenti aperti dal Finder arrivano invece
//! come Apple Event, gestiti da `mfc_gui` (vedi `mfc_gui::open_queue`).
//!
//! Opzioni: `--mcp` avvia subito il collegamento MCP live, `--workdir DIR` ne imposta la
//! cartella di lavoro.

// In release niente finestra console su Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use mfc_core::Project;

fn main() {
    let mut opts = mfc_gui::RunOptions::default();
    let mut file = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--mcp") => opts.live_mcp = true,
            Some("--workdir") => opts.workdir = args.next().map(PathBuf::from),
            // Ignora argomenti di sistema (es. -psn_ su macOS).
            _ => {
                let p = PathBuf::from(a);
                if file.is_none() && p.is_file() {
                    file = Some(p);
                }
            }
        }
    }
    let (project, path) = match file.as_deref().map(mfc_gui::load_project) {
        Some(Ok(loaded)) => (loaded.project, loaded.save_path),
        Some(Err(e)) => {
            eprintln!("impossibile aprire il progetto: {e}");
            (Project::new("Untitled"), None)
        }
        None => (Project::new("Untitled"), None),
    };
    if let Err(e) = mfc_gui::run_with(project, path, opts) {
        eprintln!("errore della finestra: {e}");
        std::process::exit(1);
    }
}
