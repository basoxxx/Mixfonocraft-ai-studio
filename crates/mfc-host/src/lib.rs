//! # mfc-host
//!
//! Hosting di plugin **VST3** e **CLAP** per Mixfonocraft.
//!
//! - [`scan`]: ricerca dei bundle nelle cartelle standard e lettura delle classi.
//! - [`ExternalInstance`]: contratto comune a un'istanza di plugin (audio, parametri,
//!   stato, editor nativo), implementato dai backend [`vst3`] e [`clap`].
//! - [`Host`]: registro delle istanze vive (riusate tra le ricompilazioni del motore)
//!   e implementazione di [`mfc_plugins::ExternalHost`].
//! - [`wrapper`]: adattatori verso i trait `Generator` / `Effect` del motore.
//!
//! Kind dei plugin: `vst3:<percorso bundle>::<class id>` e `clap:<percorso file>::<plugin id>`.

pub mod clap;
pub mod host;
pub mod scan;
pub mod vst3;
pub mod wrapper;

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use host::Host;

/// Formato del plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    Vst3,
    Clap,
}

impl PluginFormat {
    pub fn prefix(self) -> &'static str {
        match self {
            PluginFormat::Vst3 => "vst3:",
            PluginFormat::Clap => "clap:",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PluginFormat::Vst3 => "VST3",
            PluginFormat::Clap => "CLAP",
        }
    }
}

/// Una classe di plugin trovata durante la scansione.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Kind completo da usare in `PluginInstance::kind`.
    pub kind: String,
    pub format: PluginFormat,
    /// Percorso del bundle (`.vst3`) o del file (`.clap`).
    pub path: PathBuf,
    /// Class id (VST3, esadecimale) o plugin id (CLAP).
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// Categoria leggibile (es. "Instrument|Synth", "Fx|Reverb").
    pub category: String,
    pub is_instrument: bool,
    pub is_effect: bool,
}

/// Kind scomposto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKind {
    pub format: PluginFormat,
    pub path: PathBuf,
    pub id: String,
}

/// Costruisce il kind di un plugin esterno.
pub fn make_kind(format: PluginFormat, path: &std::path::Path, id: &str) -> String {
    format!("{}{}::{}", format.prefix(), path.display(), id)
}

/// Scompone un kind `vst3:` / `clap:`; `None` se non è un plugin esterno valido.
pub fn parse_kind(kind: &str) -> Option<ParsedKind> {
    let (format, rest) = if let Some(r) = kind.strip_prefix("vst3:") {
        (PluginFormat::Vst3, r)
    } else if let Some(r) = kind.strip_prefix("clap:") {
        (PluginFormat::Clap, r)
    } else {
        return None;
    };
    let (path, id) = rest.rsplit_once("::")?;
    if path.is_empty() || id.is_empty() {
        return None;
    }
    Some(ParsedKind {
        format,
        path: PathBuf::from(path),
        id: id.to_string(),
    })
}

/// Parametro di un plugin esterno (valori normalizzati 0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalParam {
    pub id: u32,
    pub name: String,
    /// Unità (es. "dB", "Hz"), può essere vuota.
    pub unit: String,
    pub default: f64,
    /// Numero di passi per parametri discreti (0 = continuo).
    pub steps: u32,
    pub automatable: bool,
    pub hidden: bool,
}

/// Nome del parametro esterno usato in `PluginInstance::params` e nelle automazioni.
pub fn param_key(id: u32) -> String {
    format!("p{id}")
}

/// Inverso di [`param_key`].
pub fn parse_param_key(name: &str) -> Option<u32> {
    name.strip_prefix('p')?.parse().ok()
}

/// Evento MIDI verso un plugin, con offset in sample nel blocco.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiEvent {
    NoteOn {
        offset: u32,
        key: u8,
        velocity: f32,
        note_id: i32,
    },
    NoteOff {
        offset: u32,
        key: u8,
        velocity: f32,
        note_id: i32,
    },
    /// Tutte le note spente (stop del transport).
    AllNotesOff { offset: u32 },
}

/// Handle nativo della finestra che ospita l'editor del plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentWindow {
    /// `NSView*` (macOS).
    AppKit(*mut c_void),
    /// `HWND` (Windows).
    Win32(isize),
    /// Window id X11 (Linux).
    X11(u64),
}

/// Dimensione dell'editor in pixel logici.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorSize {
    pub width: u32,
    pub height: u32,
}

/// Errori dell'host.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("kind non valido: {0}")]
    BadKind(String),
    #[error("impossibile caricare {path}: {reason}")]
    Load { path: PathBuf, reason: String },
    #[error("plugin non trovato nel modulo: {0}")]
    NotFound(String),
    #[error("il plugin ha rifiutato l'operazione: {0}")]
    Plugin(String),
    #[error("non supportato: {0}")]
    Unsupported(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Un'istanza di plugin esterno, condivisa tra motore (thread audio) e GUI (thread UI).
///
/// Regole di threading per le implementazioni:
/// - `process` è chiamato dal thread audio: non deve bloccare (usare `try_lock`) né allocare.
///   Se lo stato interno è occupato restituisce `false` e il chiamante usa silenzio/dry.
/// - `set_param_normalized` può essere chiamato da qualunque thread: accoda il cambio,
///   applicato al blocco audio successivo (e notificato al controller/GUI del plugin).
/// - tutti gli altri metodi sono chiamati dal thread UI (o comunque non dal thread audio).
pub trait ExternalInstance: Send + Sync {
    fn info(&self) -> &PluginInfo;

    /// (Ri)configura sample rate e blocco massimo; chiamato prima di usare l'istanza
    /// e quando cambia il sample rate. Gestisce internamente disattivazione/riattivazione.
    fn prepare(&self, sample_rate: f32, max_block: usize) -> Result<(), HostError>;

    /// Elabora un blocco. `input` è presente per gli effetti (stereo); le uscite vengono
    /// SOVRASCRITTE. Restituisce `false` se il blocco non è stato elaborato.
    fn process(
        &self,
        events: &[MidiEvent],
        input: Option<(&[f32], &[f32])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool;

    /// Cambia un parametro (valore normalizzato 0..1), da qualsiasi thread.
    fn set_param_normalized(&self, id: u32, value: f64);

    /// Azzera code e note (stop).
    fn reset(&self);

    /// Elenco dei parametri.
    fn params(&self) -> Vec<ExternalParam>;
    /// Valore normalizzato corrente.
    fn param_normalized(&self, id: u32) -> f64;
    /// Testo del valore come lo mostra il plugin (es. "-6.0 dB").
    fn param_display(&self, id: u32, value: f64) -> String;

    /// Stato completo del plugin (blob opaco, salvato nel progetto).
    fn save_state(&self) -> Result<Vec<u8>, HostError>;
    /// Ripristina uno stato salvato con [`ExternalInstance::save_state`].
    fn load_state(&self, data: &[u8]) -> Result<(), HostError>;

    /// Il plugin ha un'interfaccia grafica propria.
    fn has_editor(&self) -> bool;
    /// Apre l'editor dentro la finestra nativa indicata; restituisce la dimensione richiesta.
    fn open_editor(&self, parent: ParentWindow) -> Result<EditorSize, HostError>;
    /// Chiude l'editor (idempotente).
    fn close_editor(&self);
    /// Richiesta di ridimensionamento fatta dal plugin dopo l'apertura, se presente.
    fn take_editor_resize(&self) -> Option<EditorSize> {
        None
    }
    /// Chiamato periodicamente dal thread UI (timer/idle richiesti da alcuni plugin).
    fn idle(&self) {}

    /// Latenza introdotta (sample).
    fn latency(&self) -> u32 {
        0
    }
}

/// Backend di un formato.
pub trait FormatBackend: Send + Sync {
    fn format(&self) -> PluginFormat;
    /// Cartelle di sistema/utente in cui cercare i plugin.
    fn default_search_paths(&self) -> Vec<PathBuf>;
    /// `true` se il percorso è un bundle/file di questo formato.
    fn is_candidate(&self, path: &std::path::Path) -> bool;
    /// Classi di plugin contenute nel bundle/file.
    fn scan(&self, path: &std::path::Path) -> Result<Vec<PluginInfo>, HostError>;
    /// Crea e attiva un'istanza.
    fn instantiate(
        &self,
        kind: &ParsedKind,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<Arc<dyn ExternalInstance>, HostError>;
}

/// Blocco massimo usato dal motore (sotto-blocchi da `mfc_engine::CHUNK` = 64 sample, con margine).
pub const DEFAULT_MAX_BLOCK: usize = 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_roundtrip() {
        let k = make_kind(
            PluginFormat::Vst3,
            std::path::Path::new("/Library/Audio/Plug-Ins/VST3/A b.vst3"),
            "ABCDEF01",
        );
        assert_eq!(k, "vst3:/Library/Audio/Plug-Ins/VST3/A b.vst3::ABCDEF01");
        let p = parse_kind(&k).unwrap();
        assert_eq!(p.format, PluginFormat::Vst3);
        assert_eq!(p.id, "ABCDEF01");
        // Percorsi Windows con ':' (C:\...) e id CLAP con punti.
        let c = parse_kind(r"clap:C:\Program Files\Common Files\CLAP\x.clap::com.vendor.synth").unwrap();
        assert_eq!(c.path, PathBuf::from(r"C:\Program Files\Common Files\CLAP\x.clap"));
        assert_eq!(c.id, "com.vendor.synth");
        assert!(parse_kind("mfc.3xosc").is_none());
        assert!(parse_kind("vst3:nocolon").is_none());
    }

    #[test]
    fn param_keys() {
        assert_eq!(param_key(42), "p42");
        assert_eq!(parse_param_key("p42"), Some(42));
        assert_eq!(parse_param_key("cutoff"), None);
    }
}
