//! Coda dei documenti da aprire ricevuti dal sistema mentre l'app gira (su macOS: doppio
//! click nel Finder, `open -a`, trascinamento sull'icona del Dock). La GUI la svuota a ogni frame.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Accoda dei file da aprire e risveglia la GUI (se esiste già).
pub fn push(paths: impl IntoIterator<Item = PathBuf>) {
    let mut q = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    q.extend(paths);
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

/// Estrae i file accodati.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Registra il contesto egui da risvegliare (una volta, alla creazione della finestra).
pub(crate) fn set_context(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
}
