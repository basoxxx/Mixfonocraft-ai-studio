//! Hosting dei plugin in un processo separato ("sandbox").
//!
//! Ogni istanza VST3/CLAP gira in un processo `mfc-plugin-host` dedicato (vedi
//! [`server`]), che usa gli stessi backend in-process. Se il plugin va in crash muore
//! solo il processo figlio: l'app continua, l'istanza diventa "morta" (`process`
//! restituisce `false`, le altre chiamate errori o valori di default) e l'[`crate::Host`]
//! riporta l'errore "Plugin X si è chiuso in modo anomalo". Con
//! [`crate::Host::restart`] il processo viene ricreato e l'ultimo stato salvato ripristinato.
//!
//! # Comunicazione
//!
//! Un solo canale: stdin/stdout del figlio, con frame binari a lunghezza prefissata
//! ([`proto`]). Richieste di controllo (rispecchiano [`crate::ExternalInstance`]),
//! cambi di parametro, reset e blocchi audio viaggiano sullo stesso flusso; nel figlio un
//! thread lettore smista i blocchi audio a un thread dedicato e le richieste al main thread.
//!
//! # Audio e latenza
//!
//! Il thread audio non scambia mai dati con la pipe: [`SandboxInstance`] consegna il
//! blocco a un thread IPC tramite uno slot singolo lock-free (stato atomico + buffer
//! preallocati) e restituisce il risultato del blocco PRECEDENTE. Questo introduce
//! **un blocco di latenza** (la durata dell'ultimo blocco elaborato, tipicamente
//! `mfc_engine::CHUNK` = 64 campioni), riportata da `latency()` insieme a quella del plugin.
//!
//! Il blocco precedente ha avuto a disposizione il tempo di un intero blocco, ma il motore
//! elabora più sotto-blocchi consecutivi nella stessa callback: per questo `process`
//! attende il risultato precedente per un tempo limitato ([`SandboxConfig::process_wait`],
//! di default metà della durata del blocco) con un'attesa attiva senza lock né syscall
//! bloccanti. Se il risultato non arriva in tempo restituisce `false` (silenzio per gli
//! strumenti, segnale dry per gli effetti) e gli eventi MIDI vengono conservati per il
//! blocco successivo. Il costo è quindi un round-trip IPC per sotto-blocco: con plugin
//! pesanti o buffer audio molto piccoli conviene l'hosting in-process.
//!
//! # Editor
//!
//! In modalità sandbox l'editor si apre in una finestra di proprietà del processo figlio
//! ([`crate::ExternalInstance::editor_is_external`] = `true`): `open_editor` ignora la
//! finestra padre e la GUI deve solo controllare [`crate::ExternalInstance::editor_is_open`].
//! Supportato su macOS e Windows; su Linux `has_editor` è `false`.

pub mod proto;
mod proxy;
pub mod server;
mod window;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

pub use proxy::SandboxInstance;
pub use server::ScanOutput;

use crate::{ExternalInstance, FormatBackend, HostError, ParsedKind, PluginFormat, PluginInfo};

/// Nome dell'eseguibile helper.
pub const HELPER_NAME: &str = if cfg!(windows) {
    "mfc-plugin-host.exe"
} else {
    "mfc-plugin-host"
};

/// Variabile d'ambiente che, se impostata, indica il percorso dell'helper.
pub const HELPER_ENV: &str = "MFC_PLUGIN_HOST";

/// Percorsi candidati dell'helper per l'eseguibile `exe`, in ordine di preferenza:
/// stessa cartella, cartella del profilo cargo (per test ed esempi in `deps/`),
/// `Contents/MacOS` del bundle `.app` che contiene l'eseguibile.
pub fn helper_candidates(exe: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = exe.parent() {
        v.push(dir.join(HELPER_NAME));
        if dir.file_name().is_some_and(|n| n == "deps") {
            if let Some(profile) = dir.parent() {
                v.push(profile.join(HELPER_NAME));
            }
        }
    }
    if let Some(app) = exe
        .ancestors()
        .find(|a| a.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
    {
        v.push(app.join("Contents").join("MacOS").join(HELPER_NAME));
    }
    v.dedup();
    v
}

/// Cerca l'helper: variabile [`HELPER_ENV`], poi accanto all'eseguibile corrente
/// (vedi [`helper_candidates`]).
pub fn find_helper() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(HELPER_ENV).map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    helper_candidates(&exe).into_iter().find(|p| p.is_file())
}

/// Configurazione della modalità sandbox.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxConfig {
    /// Percorso di `mfc-plugin-host`.
    pub helper: PathBuf,
    /// Attesa massima, nel thread audio, del risultato del blocco precedente.
    /// `None` = metà della durata del blocco corrente.
    pub process_wait: Option<Duration>,
    /// Tempo massimo di risposta del processo a un blocco audio; oltre, il plugin è
    /// considerato bloccato e il processo viene terminato.
    pub block_timeout: Duration,
    /// Tempo massimo di risposta alle richieste di controllo (stato, editor, ...).
    pub call_timeout: Duration,
    /// Tempo massimo per caricare il plugin all'avvio del processo.
    pub startup_timeout: Duration,
    /// Tempo massimo della scansione di un bundle/file.
    pub scan_timeout: Duration,
    /// Ogni quanto `idle` salva lo stato del plugin (usato da `restart`); `None` = mai
    /// (resta lo stato alla creazione o l'ultimo salvato/caricato esplicitamente).
    pub state_snapshot_interval: Option<Duration>,
}

impl SandboxConfig {
    pub fn new(helper: impl Into<PathBuf>) -> Self {
        Self {
            helper: helper.into(),
            process_wait: None,
            block_timeout: Duration::from_secs(5),
            call_timeout: Duration::from_secs(20),
            startup_timeout: Duration::from_secs(30),
            scan_timeout: Duration::from_secs(60),
            state_snapshot_interval: Some(Duration::from_secs(15)),
        }
    }

    /// Configurazione con l'helper trovato da [`find_helper`].
    pub fn detect() -> Option<Self> {
        find_helper().map(Self::new)
    }

    pub(crate) fn command(&self) -> Command {
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut cmd = Command::new(&self.helper);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: nessuna console per l'helper.
            cmd.creation_flags(0x0800_0000);
        }
        cmd
    }
}

/// Descrizione dell'uscita di un processo terminato.
pub(crate) fn describe_exit(status: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("segnale {sig}");
        }
    }
    match status.code() {
        Some(c) => format!("codice di uscita {c} (0x{:08X})", c as u32),
        None => "terminato".into(),
    }
}

/// Scansiona un bundle/file nel processo helper (`--scan`): un plugin che va in crash
/// durante la scansione non porta giù l'app.
pub fn scan_in_child(config: &SandboxConfig, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
    let mut cmd = config.command();
    let mut child = cmd
        .arg("--scan")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| HostError::Load {
            path: config.helper.clone(),
            reason: format!("avvio di {HELPER_NAME}: {e}"),
        })?;
    let mut stdout = child.stdout.take().expect("stdout in pipe");
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let mut v = Vec::new();
        let r = stdout.read_to_end(&mut v).map(|_| v);
        let _ = tx.send(r);
    });
    let out = match rx.recv_timeout(config.scan_timeout) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HostError::Io(e));
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HostError::Load {
                path: path.to_path_buf(),
                reason: "la scansione non risponde (processo terminato)".into(),
            });
        }
    };
    let status = child.wait()?;
    match serde_json::from_slice::<ScanOutput>(&out) {
        Ok(ScanOutput::Ok(v)) => Ok(v),
        Ok(ScanOutput::Err(reason)) => Err(HostError::Load {
            path: path.to_path_buf(),
            reason,
        }),
        Err(_) => Err(HostError::Load {
            path: path.to_path_buf(),
            reason: format!(
                "il plugin si è chiuso in modo anomalo durante la scansione ({})",
                describe_exit(status)
            ),
        }),
    }
}

/// Backend di un formato che ospita le istanze in processi `mfc-plugin-host`.
/// Il riconoscimento dei file è quello del backend in-process; scansione e istanze
/// avvengono nel processo figlio.
pub struct SandboxBackend {
    inner: Box<dyn FormatBackend>,
    config: SandboxConfig,
}

impl SandboxBackend {
    /// Backend sandbox per il formato indicato.
    pub fn new(format: PluginFormat, config: SandboxConfig) -> Self {
        let inner: Box<dyn FormatBackend> = match format {
            PluginFormat::Vst3 => Box::new(crate::vst3::Vst3Backend::new()),
            PluginFormat::Clap => Box::new(crate::clap::ClapBackend::new()),
        };
        Self { inner, config }
    }

    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }
}

impl FormatBackend for SandboxBackend {
    fn format(&self) -> PluginFormat {
        self.inner.format()
    }

    fn default_search_paths(&self) -> Vec<PathBuf> {
        self.inner.default_search_paths()
    }

    fn is_candidate(&self, path: &Path) -> bool {
        self.inner.is_candidate(path)
    }

    fn scan(&self, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
        scan_in_child(&self.config, path)
    }

    fn instantiate(
        &self,
        kind: &ParsedKind,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<Arc<dyn ExternalInstance>, HostError> {
        if kind.format != self.format() {
            return Err(HostError::BadKind(kind.id.clone()));
        }
        Ok(Arc::new(SandboxInstance::spawn(
            self.config.clone(),
            kind,
            sample_rate,
            max_block,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_candidates_cover_bundle_and_cargo_layouts() {
        let c = helper_candidates(Path::new("/opt/mfc/mixfonocraft-studio"));
        assert_eq!(c, vec![PathBuf::from("/opt/mfc").join(HELPER_NAME)]);
        let c = helper_candidates(Path::new("/w/target/debug/deps/sandbox-1234"));
        assert_eq!(
            c,
            vec![
                PathBuf::from("/w/target/debug/deps").join(HELPER_NAME),
                PathBuf::from("/w/target/debug").join(HELPER_NAME),
            ]
        );
        let c = helper_candidates(Path::new("/Applications/Mixfonocraft.app/Contents/Resources/bin/tool"));
        assert_eq!(
            c.last().unwrap(),
            &PathBuf::from("/Applications/Mixfonocraft.app/Contents/MacOS").join(HELPER_NAME)
        );
        // Eseguibile già in Contents/MacOS: nessun duplicato.
        let c = helper_candidates(Path::new(
            "/Applications/Mixfonocraft.app/Contents/MacOS/mixfonocraft-studio",
        ));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn missing_helper_is_a_clean_error() {
        let cfg = SandboxConfig::new("/nonexistent/mfc-plugin-host");
        assert!(matches!(
            scan_in_child(&cfg, Path::new("/tmp/x.clap")),
            Err(HostError::Load { .. })
        ));
        let backend = SandboxBackend::new(PluginFormat::Clap, cfg);
        let kind = crate::parse_kind("clap:/tmp/x.clap::id").unwrap();
        assert!(backend.instantiate(&kind, 48_000.0, 512).is_err());
        let vst = crate::parse_kind("vst3:/tmp/x.vst3::00").unwrap();
        assert!(matches!(
            backend.instantiate(&vst, 48_000.0, 512),
            Err(HostError::BadKind(_))
        ));
    }
}
