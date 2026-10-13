//! # mfc-core
//!
//! Modello di progetto di Mixfonocraft AI Studio, una ricostruzione clean-room
//! dell'architettura di FL Studio (Channel Rack, Step Sequencer, Piano Roll,
//! Playlist, Mixer, automazioni), più il sistema di [`Command`] con undo/redo.

pub mod automation_rec;
pub mod command;
pub mod history;
pub mod project;
pub mod time;

pub use automation_rec::{AutomationRecorder, Thinning};
pub use command::{Command, NoteFilter, Outcome};
pub use history::History;
pub use project::*;
pub use time::{Tick, TimeBase, TimeSignature, DEFAULT_PPQ};

/// Errori del modello.
#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CoreError {
    #[error("non trovato: {0}")]
    NotFound(String),
    #[error("valore non valido: {0}")]
    Invalid(String),
}
