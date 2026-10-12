//! # mfc-midi
//!
//! Importazione ed esportazione di file MIDI standard (SMF, formati 0 e 1)
//! per Mixfonocraft AI Studio.
//!
//! - [`parse_midi`] legge un file `.mid` e restituisce le tracce come note
//!   del modello `mfc-core`, riscalate al PPQ del progetto.
//! - [`import_into_project`] inserisce il risultato in un [`mfc_core::Project`]
//!   tramite i comandi del modello (quindi con undo/redo).
//! - [`export_pattern`] ed [`export_song`] producono file SMF di formato 1.

mod export;
mod import;

pub use export::{export_pattern, export_song};
pub use import::{import_into_project, parse_midi, ImportedTrack, MidiImport};

/// Errori dell'importazione/esportazione MIDI.
#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    /// Il file non è un SMF valido o è troncato.
    #[error("file MIDI non valido: {0}")]
    Parse(String),
    /// Funzionalità MIDI non supportata (es. timing SMPTE, formato 2).
    #[error("MIDI non supportato: {0}")]
    Unsupported(String),
    /// Parametro non valido per l'esportazione o l'importazione.
    #[error("valore non valido: {0}")]
    Invalid(String),
    /// Errore di scrittura.
    #[error("errore di I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Errore proveniente dal modello di progetto.
    #[error(transparent)]
    Core(#[from] mfc_core::CoreError),
}
