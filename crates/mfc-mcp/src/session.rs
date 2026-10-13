//! Stato della sessione MCP: progetto aperto, cronologia undo, transport live.

use std::path::{Path, PathBuf};

use mfc_core::{Command, History, Outcome, Project};
use mfc_engine::backend::null::NullBackend;
use mfc_engine::{Engine, EngineHandle};

/// Trasporto live (motore che gira in un thread).
pub struct Transport {
    pub handle: EngineHandle,
    _null: Option<NullBackend>,
    #[cfg(feature = "realtime")]
    _device: Option<mfc_engine::backend::device::DeviceBackend>,
    /// `true` se l'audio va a una scheda reale.
    pub audible: bool,
}

impl Transport {
    /// Transport che usa un motore già avviato da altri (la GUI nel collegamento live):
    /// la sessione lo comanda e lo risincronizza, ma non ne possiede il backend audio.
    pub fn attached(handle: EngineHandle, audible: bool) -> Self {
        Transport {
            handle,
            _null: None,
            #[cfg(feature = "realtime")]
            _device: None,
            audible,
        }
    }
}

/// Sessione di lavoro.
pub struct Session {
    pub project: Project,
    pub history: History,
    /// Percorso del progetto aperto/salvato.
    pub path: Option<PathBuf>,
    /// Cartella in cui scrivere render ed export con percorsi relativi.
    pub workdir: PathBuf,
    pub transport: Option<Transport>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

impl Session {
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            project: Project::new("Untitled"),
            history: History::default(),
            path: None,
            workdir,
            transport: None,
        }
    }

    /// Risolve un percorso relativo rispetto alla cartella di lavoro.
    pub fn resolve(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workdir.join(path)
        }
    }

    /// Esegue un comando con undo e sincronizza il motore live.
    pub fn exec(&mut self, cmd: Command) -> Result<Outcome, String> {
        let out = self
            .history
            .execute(&mut self.project, &cmd)
            .map_err(|e| e.to_string())?;
        self.sync();
        Ok(out)
    }

    /// Sostituisce l'intero progetto (open/import/new).
    pub fn replace_project(&mut self, project: Project, path: Option<PathBuf>) {
        self.project = project;
        self.history.clear();
        self.path = path;
        self.sync();
    }

    pub fn undo(&mut self) -> Option<String> {
        let l = self.history.undo(&mut self.project);
        self.sync();
        l
    }

    pub fn redo(&mut self) -> Option<String> {
        let l = self.history.redo(&mut self.project);
        self.sync();
        l
    }

    /// Ricarica il progetto nel motore live, se attivo.
    pub fn sync(&self) {
        if let Some(t) = &self.transport {
            t.handle.load_project(&self.project);
        }
    }

    /// Avvia (se serve) il motore live e restituisce l'handle.
    pub fn ensure_transport(&mut self) -> Result<&EngineHandle, String> {
        if self.transport.is_none() {
            self.transport = Some(start_transport(&self.project)?);
        }
        Ok(&self.transport.as_ref().unwrap().handle)
    }

    pub fn stop_transport(&mut self) {
        self.transport = None;
    }
}

#[cfg(feature = "realtime")]
fn start_transport(project: &Project) -> Result<Transport, String> {
    use mfc_engine::backend::device::{default_sample_rate, DeviceBackend};
    if let Ok(sr) = default_sample_rate() {
        let (engine, handle) = Engine::new(project, sr as f32);
        if let Ok(dev) = DeviceBackend::start(engine) {
            return Ok(Transport {
                handle,
                _null: None,
                _device: Some(dev),
                audible: true,
            });
        }
    }
    let (engine, handle) = Engine::new(project, 48_000.0);
    Ok(Transport {
        handle,
        _null: Some(NullBackend::start(engine, 512)),
        _device: None,
        audible: false,
    })
}

#[cfg(not(feature = "realtime"))]
fn start_transport(project: &Project) -> Result<Transport, String> {
    let (engine, handle) = Engine::new(project, 48_000.0);
    Ok(Transport {
        handle,
        _null: Some(NullBackend::start(engine, 512)),
        audible: false,
    })
}
