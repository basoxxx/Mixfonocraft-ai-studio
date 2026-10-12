//! Stato dell'applicazione indipendente dalla UI: progetto, undo, motore audio,
//! selezioni. Tutte le modifiche passano da [`Studio::exec`] (comandi di mfc-core).

use std::path::{Path, PathBuf};

use mfc_core::{ChannelId, Command, History, Outcome, PatternId, Project};
use mfc_engine::backend::null::NullBackend;
use mfc_engine::{Engine, EngineHandle, PlayMode};

/// Vista principale nell'area centrale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    ChannelRack,
    PianoRoll,
    Playlist,
    Mixer,
}

/// Oggetto il cui editor plugin è aperto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTarget {
    Channel(ChannelId),
    Effect { insert: u16, slot: u16 },
}

enum Backend {
    Null(#[allow(dead_code)] NullBackend),
    #[cfg(feature = "realtime")]
    Device(#[allow(dead_code)] mfc_engine::backend::device::DeviceBackend),
}

/// Stato complessivo dello studio.
pub struct Studio {
    pub project: Project,
    pub history: History,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub view: View,
    pub song_mode: bool,
    pub selected_channel: Option<ChannelId>,
    pub selected_insert: u16,
    pub plugin_editor: Option<PluginTarget>,
    /// Messaggio di stato (ultima operazione/errore).
    pub status: String,
    pub engine: EngineHandle,
    /// `true` se l'audio esce su una scheda reale.
    pub audible: bool,
    _backend: Backend,
}

impl Studio {
    pub fn new(project: Project) -> Self {
        let (handle, backend, audible) = start_engine(&project);
        let selected_channel = project.channels.first().map(|c| c.id);
        let mut s = Self {
            project,
            history: History::default(),
            path: None,
            dirty: false,
            view: View::ChannelRack,
            song_mode: false,
            selected_channel,
            selected_insert: 1,
            plugin_editor: None,
            status: if audible {
                "Audio attivo".into()
            } else {
                "Audio non disponibile: transport silenzioso".into()
            },
            engine: handle,
            audible,
            _backend: backend,
        };
        s.apply_mode();
        s
    }

    /// Pattern selezionato (o il primo).
    pub fn current_pattern(&self) -> Option<PatternId> {
        self.project
            .current_pattern
            .or_else(|| self.project.patterns.first().map(|p| p.id))
    }

    /// Esegue un comando con undo; aggiorna motore e stato.
    pub fn exec(&mut self, cmd: Command) -> Option<Outcome> {
        match self.history.execute(&mut self.project, &cmd) {
            Ok(o) => {
                self.dirty = true;
                self.after_change();
                Some(o)
            }
            Err(e) => {
                self.status = format!("Errore: {e}");
                None
            }
        }
    }

    pub fn undo(&mut self) {
        if let Some(l) = self.history.undo(&mut self.project) {
            self.status = format!("Annullato: {l}");
            self.after_change();
        }
    }

    pub fn redo(&mut self) {
        if let Some(l) = self.history.redo(&mut self.project) {
            self.status = format!("Ripristinato: {l}");
            self.after_change();
        }
    }

    fn after_change(&mut self) {
        if self.selected_channel.is_some_and(|c| self.project.channel(c).is_none()) {
            self.selected_channel = self.project.channels.first().map(|c| c.id);
        }
        if let Some(PluginTarget::Channel(c)) = self.plugin_editor {
            if self.project.channel(c).is_none() {
                self.plugin_editor = None;
            }
        }
        self.engine.load_project(&self.project);
        self.apply_mode();
    }

    /// Applica la modalità PAT/SONG al motore.
    pub fn apply_mode(&mut self) {
        let mode = match (self.song_mode, self.current_pattern()) {
            (false, Some(p)) => PlayMode::Pattern(p),
            _ => PlayMode::Song,
        };
        self.engine.set_mode(mode);
    }

    pub fn set_song_mode(&mut self, song: bool) {
        self.song_mode = song;
        self.apply_mode();
    }

    pub fn toggle_play(&mut self) {
        if self.engine.shared.is_playing() {
            self.engine.pause();
        } else {
            self.engine.play();
        }
    }

    pub fn stop(&mut self) {
        self.engine.stop();
    }

    /// Sostituisce il progetto (nuovo/apertura/import).
    pub fn replace_project(&mut self, project: Project, path: Option<PathBuf>) {
        self.engine.stop();
        self.project = project;
        self.history.clear();
        self.path = path;
        self.dirty = false;
        self.selected_channel = self.project.channels.first().map(|c| c.id);
        self.plugin_editor = None;
        self.after_change();
    }

    /// Apre .mfc, .flp o .mid.
    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let project = match ext.as_str() {
            "flp" => {
                let (_, imp) = mfc_flp::read_file(path).map_err(|e| e.to_string())?;
                if !imp.warnings.is_empty() {
                    self.status = format!("FLP importato con {} avvisi", imp.warnings.len());
                }
                imp.project
            }
            "mid" | "midi" => {
                let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
                let mut p = Project::new(path.file_stem().and_then(|s| s.to_str()).unwrap_or("MIDI"));
                let imp = mfc_midi::parse_midi(&bytes, p.time.ppq).map_err(|e| e.to_string())?;
                let (pat, _) = mfc_midi::import_into_project(&mut p, &imp, "MIDI").map_err(|e| e.to_string())?;
                p.current_pattern = Some(pat);
                p
            }
            _ => {
                let txt = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                Project::from_json(&txt).map_err(|e| e.to_string())?
            }
        };
        let keep_path = (ext == "mfc").then(|| path.to_path_buf());
        self.replace_project(project, keep_path);
        if self.status.is_empty() || !self.status.starts_with("FLP") {
            self.status = format!("Aperto {}", path.display());
        }
        Ok(())
    }

    /// Salva in .mfc (o .flp se l'estensione lo indica).
    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("flp"))
        {
            mfc_flp::write_file(path, &self.project).map_err(|e| e.to_string())?;
            self.status = format!("Esportato FLP {}", path.display());
            return Ok(());
        }
        let txt = self.project.to_json().map_err(|e| e.to_string())?;
        std::fs::write(path, txt).map_err(|e| e.to_string())?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        self.status = format!("Salvato {}", path.display());
        Ok(())
    }

    /// Render del brano (o del pattern in modalità PAT) su WAV.
    pub fn render(&mut self, path: &Path) -> Result<(), String> {
        let opts = mfc_engine::RenderOptions {
            pattern: if self.song_mode { None } else { self.current_pattern() },
            ..Default::default()
        };
        let out = mfc_engine::render_to_file(&self.project, path, &opts, mfc_engine::WavFormat::Int24)
            .map_err(|e| e.to_string())?;
        self.status = format!("Render {:.1} s → {}", out.mix.seconds(), path.display());
        Ok(())
    }
}

fn start_engine(project: &Project) -> (EngineHandle, Backend, bool) {
    #[cfg(feature = "realtime")]
    {
        use mfc_engine::backend::device::{default_sample_rate, DeviceBackend};
        if let Ok(sr) = default_sample_rate() {
            let (engine, handle) = Engine::new(project, sr as f32);
            if let Ok(dev) = DeviceBackend::start(engine) {
                return (handle, Backend::Device(dev), true);
            }
        }
    }
    let (engine, handle) = Engine::new(project, 48_000.0);
    (handle, Backend::Null(NullBackend::start(engine, 512)), false)
}
