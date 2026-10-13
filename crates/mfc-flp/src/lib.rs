//! # mfc-flp
//!
//! Lettura/scrittura dei progetti FL Studio (`.flp`) per interoperabilità.
//!
//! La mappatura del formato è derivata **esclusivamente** da documentazione pubblica della
//! community (PyFLP, Monad.FLParser, note "FLP format"); nessun codice Image-Line è stato
//! usato. Vedi `docs/FLP_FORMAT.md`.
//!
//! Tre livelli:
//! 1. eventi lossless: [`parse`] / [`write`] (round-trip byte-identico), [`inspect`];
//! 2. import semantico: [`to_project`] → [`ImportResult`];
//! 3. export semantico: [`from_project`] / [`from_project_report`] (con avvisi).

use std::path::Path;

pub mod events;
pub mod export;
pub mod import;
pub mod plugin_map;
pub mod reader;
pub mod structs;
pub mod writer;

pub use events::{
    decode_text, encode_text, encoding_for_version, event_name, id, inspect, EventKind, EventSummary, EventValue,
    FlpFile, FlpHeader, RawEvent, TextEncoding,
};
pub use export::{from_project, from_project_report, ExportResult, EXPORT_VERSION};
pub use import::{to_project, ImportResult};
pub use reader::{parse, parse_events};
pub use writer::{write, write_events};

/// Errori del crate.
#[derive(Debug, thiserror::Error)]
pub enum FlpError {
    #[error("file troncato: {0}")]
    Truncated(&'static str),
    #[error("magic non valido: atteso '{0}'")]
    BadMagic(&'static str),
    #[error("header non valido: {0}")]
    BadHeader(String),
    #[error("lunghezza varint non valida all'offset {0}")]
    BadVarint(usize),
    #[error("evento {id} troncato all'offset {offset}")]
    TruncatedEvent { offset: usize, id: u8 },
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Legge un `.flp` da disco e lo importa nel modello.
pub fn read_file(path: impl AsRef<Path>) -> Result<(FlpFile, ImportResult), FlpError> {
    let bytes = std::fs::read(path)?;
    let file = parse(&bytes)?;
    let res = to_project(&file)?;
    Ok((file, res))
}

/// Esporta un progetto in un `.flp` su disco.
pub fn write_file(path: impl AsRef<Path>, project: &mfc_core::Project) -> Result<(), FlpError> {
    std::fs::write(path, write(&from_project(project)))?;
    Ok(())
}
