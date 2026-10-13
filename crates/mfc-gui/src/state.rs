//! Stato dell'applicazione indipendente dalla UI: progetto, undo, motore audio,
//! selezioni. Tutte le modifiche passano da [`Studio::exec`] (comandi di mfc-core).

use std::path::{Path, PathBuf};

use mfc_core::{ChannelId, Command, History, Outcome, PatternId, Project};
use mfc_engine::backend::null::NullBackend;
use mfc_engine::{Engine, EngineHandle, PlayMode};

use crate::input::{LiveEvent, LiveInput};
use crate::plugin_window::PluginWindow;
use std::collections::HashMap;
use std::sync::Arc;

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

/// Configurazione dell'uscita audio (dispositivo, frequenza, buffer).
#[cfg(feature = "realtime")]
pub type AudioConfig = mfc_engine::backend::device::DeviceConfig;

/// Senza la feature `realtime` non esiste un'uscita audio configurabile.
#[cfg(not(feature = "realtime"))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioConfig;

enum Backend {
    Null(#[allow(dead_code)] NullBackend),
    #[cfg(feature = "realtime")]
    Device(#[allow(dead_code)] mfc_engine::backend::device::DeviceBackend),
}

impl Backend {
    /// `true` se l'audio esce su una scheda reale.
    fn is_device(&self) -> bool {
        match self {
            Backend::Null(_) => false,
            #[cfg(feature = "realtime")]
            Backend::Device(_) => true,
        }
    }
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
    /// Configurazione audio scelta dall'utente (applicata da [`Studio::restart_audio`]).
    pub audio_config: AudioConfig,
    /// Host dei plugin VST3/CLAP (registrato nel motore).
    pub host: Arc<mfc_host::Host>,
    /// Finestre aperte con l'interfaccia originale dei plugin, per `instance_id`.
    pub plugin_windows: HashMap<String, PluginWindow>,
    /// Ingresso live: tastiera del computer e MIDI.
    pub live: LiveInput,
    /// Registrazione delle note live nel pattern corrente (durante la riproduzione).
    pub recording: bool,
    /// Registrazione delle automazioni dai controlli (vedi [`crate::automation_rec`]).
    pub auto_rec: crate::automation_rec::AutoRecState,
    /// Backend attivo; `None` solo durante il riavvio, per rilasciare il dispositivo.
    _backend: Option<Backend>,
}

impl Studio {
    pub fn new(project: Project) -> Self {
        let audio_config: AudioConfig = Default::default();
        // L'host va registrato prima di compilare il progetto nel motore.
        let host = Arc::new(mfc_host::Host::new());
        crate::plugins::load_catalog(&host);
        mfc_plugins::set_external_host(Some(host.clone() as Arc<dyn mfc_plugins::ExternalHost>));
        let (handle, backend, err) = start_engine(&project, &audio_config);
        let audible = backend.is_device();
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
                match err {
                    Some(e) => format!("Audio non disponibile ({e}): transport silenzioso"),
                    None => "Audio non disponibile: transport silenzioso".into(),
                }
            },
            host,
            plugin_windows: HashMap::new(),
            live: LiveInput::new(handle.clone()),
            recording: false,
            auto_rec: Default::default(),
            engine: handle,
            audible,
            audio_config,
            _backend: Some(backend),
        };
        s.apply_mode();
        s
    }

    /// Applica una nuova configurazione audio: ferma il backend attuale e ne avvia uno nuovo
    /// con il progetto corrente. Se il dispositivo non si apre, ricade sul backend silenzioso.
    #[cfg(feature = "realtime")]
    pub fn restart_audio(&mut self, cfg: AudioConfig) {
        self.engine.stop();
        // Rilascia il dispositivo prima di riaprirlo.
        self._backend = None;
        let (handle, backend, err) = start_engine(&self.project, &cfg);
        self.audible = backend.is_device();
        self.live.set_engine(handle.clone());
        self.engine = handle;
        self._backend = Some(backend);
        self.apply_mode();
        self.status = match (err, self.audible) {
            (None, true) => format!(
                "Audio attivo: {} · {} Hz · buffer {}",
                cfg.device.as_deref().unwrap_or("predefinito"),
                self.engine.sample_rate() as u32,
                cfg.buffer_size.map_or("predefinito".to_string(), |n| n.to_string()),
            ),
            (Some(e), _) => format!("Audio non disponibile ({e}): transport silenzioso"),
            (None, false) => "Audio non disponibile: transport silenzioso".into(),
        };
        self.audio_config = cfg;
    }

    /// Pattern selezionato (o il primo).
    pub fn current_pattern(&self) -> Option<PatternId> {
        self.project
            .current_pattern
            .or_else(|| self.project.patterns.first().map(|p| p.id))
    }

    /// Esegue un comando con undo; aggiorna motore e stato.
    pub fn exec(&mut self, cmd: Command) -> Option<Outcome> {
        // Prima di rimuovere un plugin esterno se ne salva lo stato nel progetto,
        // così l'undo lo ripristina com'era.
        if matches!(
            cmd,
            Command::RemoveChannel { .. } | Command::RemoveEffect { .. } | Command::Batch { .. }
        ) {
            self.sync_plugin_states();
        }
        match self.history.execute(&mut self.project, &cmd) {
            Ok(o) => {
                self.dirty = true;
                self.after_change();
                self.auto_rec_observe(&cmd);
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
        let ids = crate::plugins::external_ids(&self.project);
        self.plugin_windows.retain(|id, _| ids.contains(id));
        self.host.retain(&ids);
    }

    /// Copia nel progetto lo stato dei plugin esterni (prima di salvare/rimuovere).
    pub fn sync_plugin_states(&mut self) {
        let errors = crate::plugins::sync_states(&self.host, &mut self.project);
        if !errors.is_empty() {
            self.status = format!("Stato plugin non salvato: {}", errors.join("; "));
        }
    }

    /// Lavoro periodico del thread UI: idle dei plugin, finestre native, errori dell'host.
    pub fn tick_plugins(&mut self) {
        self.host.idle();
        self.plugin_windows.retain(|_, w| w.poll());
        self.poll_plugin_param_changes();
        let errors = self.host.take_errors();
        if !errors.is_empty() {
            self.status = format!("Plugin: {}", errors.join("; "));
        }
    }

    /// Apre (o porta in primo piano) l'interfaccia originale di un plugin esterno.
    pub fn open_plugin_window(&mut self, instance_id: &str, title: &str) {
        if let Some(w) = self.plugin_windows.get(instance_id) {
            w.focus();
            return;
        }
        let Some(inst) = self.host.instance(instance_id) else {
            self.status = "Plugin non caricato".into();
            return;
        };
        match PluginWindow::open(inst, title) {
            Ok(w) => {
                self.plugin_windows.insert(instance_id.to_string(), w);
            }
            Err(e) => self.status = format!("Interfaccia del plugin: {e}"),
        }
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
        self.finish_recording();
        self.engine.stop();
    }

    /// Attiva/disattiva la registrazione; se il transport è fermo parte la riproduzione.
    pub fn toggle_record(&mut self) {
        if self.recording {
            self.finish_recording();
            self.recording = false;
        } else {
            self.recording = true;
            if !self.engine.shared.is_playing() {
                self.engine.play();
            }
        }
    }

    /// Pattern in registrazione, sua lunghezza e griglia di quantizzazione (1/16).
    fn record_target(&self) -> Option<(PatternId, mfc_core::Tick, mfc_core::Tick)> {
        let pat = self.current_pattern()?;
        let len = self.project.pattern(pat)?.effective_length(&self.project.time);
        Some((pat, len.max(1), self.project.time.ticks_per_step()))
    }

    /// Chiude le note ancora tenute e le scrive nel pattern.
    fn finish_recording(&mut self) {
        self.finish_automation_recording();
        let Some((pat, len, snap)) = self.record_target() else {
            return;
        };
        let pos = self.engine.shared.position();
        let notes = self.live.recorder.flush(pos, len, snap);
        self.commit_recorded(pat, notes);
    }

    fn commit_recorded(&mut self, pattern: PatternId, notes: Vec<mfc_core::Note>) {
        let Some(channel) = self.selected_channel else { return };
        if !notes.is_empty() {
            let n = notes.len();
            self.exec(Command::AddNotes {
                pattern,
                channel,
                notes,
            });
            self.status = format!("Registrate {n} note");
        }
    }

    /// Da chiamare a ogni frame: instrada le note live al canale selezionato e, se si
    /// sta registrando, le aggiunge al pattern corrente.
    pub fn pump_live(&mut self) {
        self.live.set_target(self.selected_channel);
        let events = self.live.drain();
        let playing = self.engine.shared.is_playing();
        if !self.recording || !playing || events.is_empty() {
            return;
        }
        let Some((pat, len, snap)) = self.record_target() else {
            return;
        };
        // In modalità song la posizione è assoluta: si registra comunque nel pattern ciclico.
        let pos = self.engine.shared.position() % len as f64;
        let mut notes = Vec::new();
        for ev in events {
            match ev {
                LiveEvent::On { key, velocity } => self.live.recorder.on(key, velocity, pos),
                LiveEvent::Off { key } => notes.extend(self.live.recorder.off(key, pos, len, snap)),
            }
        }
        self.commit_recorded(pat, notes);
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
        // Gli id delle istanze sono per-progetto: niente riuso tra progetti diversi.
        self.plugin_windows.clear();
        self.host.retain(&[]);
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
        self.sync_plugin_states();
        let txt = self.project.to_json().map_err(|e| e.to_string())?;
        std::fs::write(path, txt).map_err(|e| e.to_string())?;
        // HOOK autosave: il salvataggio normale rende superflua la copia automatica.
        crate::autosave::discard_for_title(&self.project.info.title);
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
        // Istanze separate da quelle del motore live, ripartendo dal loro stato attuale.
        self.sync_plugin_states();
        let project = crate::plugins::detached_copy(&self.project);
        let res = mfc_engine::render_to_file(&project, path, &opts, mfc_engine::WavFormat::Int24);
        self.host.retain(&crate::plugins::external_ids(&self.project));
        let out = res.map_err(|e| e.to_string())?;
        self.status = format!("Render {:.1} s → {}", out.mix.seconds(), path.display());
        Ok(())
    }
}

/// Avvia il motore sull'uscita scelta; ricade sul backend silenzioso se non disponibile.
/// Restituisce l'errore del dispositivo, se c'è stato.
fn start_engine(project: &Project, cfg: &AudioConfig) -> (EngineHandle, Backend, Option<String>) {
    #[cfg(feature = "realtime")]
    let err = match start_device(project, cfg) {
        Ok((handle, dev)) => return (handle, Backend::Device(dev), None),
        Err(e) => Some(e),
    };
    #[cfg(not(feature = "realtime"))]
    let err = {
        let _ = cfg;
        None
    };
    let (engine, handle) = Engine::new(project, 48_000.0);
    (handle, Backend::Null(NullBackend::start(engine, 512)), err)
}

#[cfg(feature = "realtime")]
fn start_device(
    project: &Project,
    cfg: &AudioConfig,
) -> Result<(EngineHandle, mfc_engine::backend::device::DeviceBackend), String> {
    use mfc_engine::backend::device::{resolved_sample_rate, DeviceBackend};
    let sr = resolved_sample_rate(cfg).map_err(|e| e.to_string())?;
    let (engine, handle) = Engine::new(project, sr as f32);
    let dev = DeviceBackend::start_with(engine, cfg).map_err(|e| e.to_string())?;
    Ok((handle, dev))
}
