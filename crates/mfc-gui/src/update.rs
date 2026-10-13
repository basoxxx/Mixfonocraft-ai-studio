//! Aggiornamenti automatici dalle GitHub Release (senza firma del codice).
//!
//! - Nucleo puro e testato: parsing della release, confronto di versioni (semver),
//!   scelta del pacchetto per sistema/architettura, impostazioni persistenti,
//!   argomenti dell'installer Windows e script di sostituzione del bundle macOS.
//! - Operazioni di rete (controllo, download con avanzamento) eseguite su thread in
//!   background: non bloccano mai l'interfaccia e restituiscono errori come testo.
//! - Installazione: Windows avvia il setup Inno in modalità silenziosa (che riapre
//!   l'app), macOS sostituisce il bundle con uno script esterno dopo l'uscita,
//!   Linux e versioni portatili aprono la pagina della release.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};

/// Repository GitHub delle release.
pub const REPO: &str = "basoxxx/Mixfonocraft-ai-studio";
/// Endpoint API dell'ultima release pubblicata (esclude bozze e pre-release).
pub const LATEST_API: &str = "https://api.github.com/repos/basoxxx/Mixfonocraft-ai-studio/releases/latest";
/// Pagina web delle release.
pub const RELEASES_PAGE: &str = "https://github.com/basoxxx/Mixfonocraft-ai-studio/releases/latest";
/// Versione in esecuzione.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Intervallo minimo tra due controlli automatici (secondi).
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

fn user_agent() -> String {
    format!("Mixfonocraft-AI-Studio/{CURRENT_VERSION} (+https://github.com/{REPO})")
}

// ------------------------------------------------------------------ Release

/// Pacchetto allegato a una release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    /// Dimensione in byte dichiarata da GitHub (0 se assente).
    #[serde(default)]
    pub size: u64,
}

/// Risposta grezza dell'API GitHub (solo i campi usati).
#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// Release pubblicata, già validata.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// Titolo della release (o il tag se assente).
    pub name: String,
    /// Note di rilascio (Markdown grezzo).
    pub notes: String,
    /// Pagina web della release.
    pub page_url: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

/// Versione da un tag `vX.Y.Z` (il prefisso `v` è facoltativo).
pub fn parse_version(tag: &str) -> Option<Version> {
    let t = tag.trim();
    let t = t.strip_prefix(['v', 'V']).unwrap_or(t);
    Version::parse(t).ok()
}

/// Interpreta la risposta JSON di `releases/latest`.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let r: ApiRelease = serde_json::from_str(json).map_err(|e| format!("risposta non valida: {e}"))?;
    if r.draft {
        return Err("la release è una bozza".into());
    }
    let version =
        parse_version(&r.tag_name).ok_or_else(|| format!("tag di versione non riconosciuto: {}", r.tag_name))?;
    let name = r
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| r.tag_name.clone());
    let page_url = r
        .html_url
        .filter(|u| u.starts_with("https://"))
        .unwrap_or_else(|| format!("https://github.com/{REPO}/releases/tag/{}", r.tag_name));
    Ok(Release {
        version,
        tag: r.tag_name,
        name,
        notes: r.body.unwrap_or_default(),
        page_url,
        prerelease: r.prerelease,
        assets: r.assets,
    })
}

/// `true` se `candidate` è più recente della versione `current` (stringa semver).
/// Una versione corrente non interpretabile non propone aggiornamenti.
pub fn is_newer(current: &str, candidate: &Version) -> bool {
    parse_version(current).is_some_and(|c| *candidate > c)
}

// ----------------------------------------------------------------- Pacchetti

/// Piattaforma per la scelta del pacchetto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacArm64,
    MacX86_64,
    Windows,
    Linux,
    Other,
}

impl Platform {
    /// Piattaforma della build in esecuzione.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            if cfg!(target_arch = "aarch64") {
                Platform::MacArm64
            } else {
                Platform::MacX86_64
            }
        } else if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }

    /// Suffisso del nome del pacchetto installabile (`None` = solo notifica).
    fn asset_suffix(self) -> Option<&'static str> {
        match self {
            Platform::MacArm64 => Some("-macos-arm64.dmg"),
            Platform::MacX86_64 => Some("-macos-x86_64.dmg"),
            // Unico pacchetto Windows: x86_64 (funziona anche in emulazione su ARM64).
            Platform::Windows => Some("-windows-x86_64-setup.exe"),
            Platform::Linux | Platform::Other => None,
        }
    }
}

/// Pacchetto da scaricare per la piattaforma, se la release ne contiene uno.
pub fn select_asset(assets: &[Asset], platform: Platform) -> Option<&Asset> {
    let suffix = platform.asset_suffix()?;
    assets.iter().find(|a| {
        let n = a.name.to_ascii_lowercase();
        n.starts_with("mixfonocraft-") && n.ends_with(suffix) && a.browser_download_url.starts_with("https://")
    })
}

/// Nome di file sicuro per il pacchetto (nessun separatore o componente speciale).
pub fn safe_file_name(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.starts_with('.');
    ok.then_some(name)
}

// --------------------------------------------------------------- Impostazioni

/// Impostazioni degli aggiornamenti, salvate in `settings.json` sotto la chiave `updates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Controlla gli aggiornamenti all'avvio (al massimo una volta al giorno).
    pub check_on_startup: bool,
    /// Ultimo controllo (secondi Unix, 0 = mai).
    pub last_check: u64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            last_check: 0,
        }
    }
}

const SETTINGS_KEY: &str = "updates";

impl UpdateSettings {
    /// `true` se all'avvio va fatto un controllo automatico.
    pub fn auto_check_due(&self, now: u64) -> bool {
        // Orologio tornato indietro: meglio ricontrollare.
        self.check_on_startup && (now < self.last_check || now - self.last_check >= CHECK_INTERVAL_SECS)
    }

    /// Legge le impostazioni dal contenuto di `settings.json` (default se assenti o illeggibili).
    pub fn from_json(txt: &str) -> Self {
        serde_json::from_str::<serde_json::Value>(txt)
            .ok()
            .and_then(|v| v.get(SETTINGS_KEY).cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// Scrive le impostazioni in `settings.json` preservando le altre chiavi esistenti.
    pub fn merge_into_json(&self, existing: Option<&str>) -> String {
        let mut root = existing
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        if let (Some(obj), Ok(me)) = (root.as_object_mut(), serde_json::to_value(self)) {
            obj.insert(SETTINGS_KEY.into(), me);
        }
        serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".into())
    }

    /// Carica dalla cartella di configurazione dell'utente.
    pub fn load() -> Self {
        settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Self::from_json(&t))
            .unwrap_or_default()
    }

    /// Salva nella cartella di configurazione (errori ignorati: non sono critici).
    pub fn save(&self) {
        let Some(p) = settings_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let existing = std::fs::read_to_string(&p).ok();
        let _ = std::fs::write(&p, self.merge_into_json(existing.as_deref()));
    }
}

/// Percorso di `settings.json`.
pub fn settings_path() -> Option<PathBuf> {
    crate::plugins::config_dir().map(|d| d.join("settings.json"))
}

/// Ora corrente in secondi Unix.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------- Rete

fn agent(global_timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(user_agent())
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_global(global_timeout)
        .build()
        .into()
}

fn net_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(403 | 429) => "limite di richieste GitHub raggiunto, riprova più tardi".into(),
        ureq::Error::StatusCode(404) => "nessuna release pubblicata".into(),
        ureq::Error::StatusCode(c) => format!("risposta HTTP {c}"),
        other => format!("rete non disponibile ({other})"),
    }
}

/// Scarica e interpreta l'ultima release (bloccante: usare da un thread).
pub fn fetch_latest() -> Result<Release, String> {
    let mut resp = agent(Some(Duration::from_secs(30)))
        .get(LATEST_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(net_error)?;
    let txt = resp
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(net_error)?;
    parse_release(&txt)
}

/// Controllo in background: il risultato arriva sul canale, poi si chiede un ridisegno.
pub fn spawn_check(ctx: Option<egui::Context>) -> std::sync::mpsc::Receiver<Result<Release, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("mfc-update-check".into()).spawn({
        let tx = tx.clone();
        move || {
            let _ = tx.send(fetch_latest());
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        }
    });
    if let Err(e) = spawned {
        let _ = tx.send(Err(format!("impossibile avviare il controllo: {e}")));
    }
    rx
}

/// Stato condiviso di un download in corso.
#[derive(Default)]
pub struct Progress {
    /// Byte ricevuti.
    pub bytes: AtomicU64,
    /// Byte attesi (0 = sconosciuto).
    pub total: AtomicU64,
    /// Richiesta di annullamento.
    pub cancel: AtomicBool,
    result: Mutex<Option<Result<PathBuf, String>>>,
}

impl Progress {
    /// Frazione completata, se il totale è noto.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.bytes.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32)
    }

    /// Risultato finale (una sola volta).
    pub fn take_result(&self) -> Option<Result<PathBuf, String>> {
        self.result.lock().ok().and_then(|mut r| r.take())
    }

    fn finish(&self, r: Result<PathBuf, String>) {
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(r);
        }
    }
}

/// Cartella temporanea degli aggiornamenti.
pub fn update_dir() -> PathBuf {
    std::env::temp_dir().join("mixfonocraft-update")
}

/// Avvia il download del pacchetto in background.
pub fn spawn_download(asset: Asset, ctx: Option<egui::Context>) -> Arc<Progress> {
    let progress = Arc::new(Progress::default());
    let p = progress.clone();
    let spawned = std::thread::Builder::new()
        .name("mfc-update-download".into())
        .spawn(move || {
            let r = download(&asset, &update_dir(), &p);
            p.finish(r);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        progress.finish(Err(format!("impossibile avviare il download: {e}")));
    }
    progress
}

/// Scarica `asset` in `dir` verificandone la dimensione (bloccante).
fn download(asset: &Asset, dir: &Path, p: &Progress) -> Result<PathBuf, String> {
    let name = safe_file_name(&asset.name).ok_or("nome del pacchetto non valido")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cartella temporanea: {e}"))?;
    let dest = dir.join(name);
    let part = dir.join(format!("{name}.part"));

    // Nessun timeout globale: il pacchetto può essere grande e la rete lenta.
    let resp = agent(None).get(&asset.browser_download_url).call().map_err(net_error)?;
    let declared = resp.body().content_length();
    if let Some(len) = declared {
        if asset.size > 0 && len != asset.size {
            return Err(format!("dimensione inattesa ({len} byte invece di {})", asset.size));
        }
    }
    let expected = if asset.size > 0 {
        asset.size
    } else {
        declared.unwrap_or(0)
    };
    p.total.store(expected, Ordering::Relaxed);

    let res = (|| -> Result<u64, String> {
        let mut reader = resp.into_body().into_reader();
        let mut file = std::fs::File::create(&part).map_err(|e| format!("scrittura: {e}"))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            if p.cancel.load(Ordering::Relaxed) {
                return Err("download annullato".into());
            }
            let n = reader.read(&mut buf).map_err(|e| format!("download interrotto: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| format!("scrittura: {e}"))?;
            written += n as u64;
            p.bytes.store(written, Ordering::Relaxed);
        }
        file.flush().map_err(|e| format!("scrittura: {e}"))?;
        Ok(written)
    })();
    let written = match res {
        Ok(w) => w,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    if expected > 0 && written != expected {
        let _ = std::fs::remove_file(&part);
        return Err(format!("download incompleto ({written} di {expected} byte)"));
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest).map_err(|e| format!("scrittura: {e}"))?;
    Ok(dest)
}

// ---------------------------------------------------------------- Installazione

/// Esito dell'installazione.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// L'installer è partito: l'app deve chiudersi subito.
    Quit,
    /// Installazione manuale (pagina o immagine disco aperte): messaggio per l'utente.
    Manual(String),
}

/// Apre un URL o un file con l'applicazione predefinita del sistema.
pub fn open_external(target: &str) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(target);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", target]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("impossibile aprire {target}: {e}"))
}

/// Installa il pacchetto scaricato. `installer = None` (nessun pacchetto per questa
/// piattaforma) apre la pagina della release.
pub fn install(installer: Option<&Path>, page_url: &str) -> Result<InstallOutcome, String> {
    let Some(installer) = installer else {
        open_external(page_url)?;
        return Ok(InstallOutcome::Manual("Pagina della release aperta nel browser".into()));
    };
    #[cfg(windows)]
    {
        install_windows(installer, page_url)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = page_url;
        install_macos(installer)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = installer;
        open_external(page_url)?;
        Ok(InstallOutcome::Manual("Pagina della release aperta nel browser".into()))
    }
}

/// `true` se la cartella di installazione è per utente (sotto il profilo dell'utente).
pub fn is_per_user_dir(install_dir: &Path, user_roots: &[PathBuf]) -> bool {
    let dir = install_dir.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    user_roots.iter().any(|root| {
        let r = root.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
        let r = r.trim_end_matches('\\');
        !r.is_empty() && (dir == r || dir.starts_with(&format!("{r}\\")))
    })
}

/// Argomenti del setup Inno per l'aggiornamento silenzioso nella stessa cartella.
/// `/RELAUNCH` fa riaprire l'app al termine (voce `[Run]` dello script .iss).
pub fn windows_installer_args(install_dir: &Path, per_user: bool) -> Vec<String> {
    vec![
        "/VERYSILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/CLOSEAPPLICATIONS".into(),
        "/RELAUNCH".into(),
        if per_user { "/CURRENTUSER" } else { "/ALLUSERS" }.into(),
        format!("/DIR={}", install_dir.display()),
    ]
}

#[cfg(windows)]
fn install_windows(setup: &Path, page_url: &str) -> Result<InstallOutcome, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("cartella dell'app sconosciuta")?;
    // Senza il disinstallatore Inno è la versione portatile (zip): aggiornamento manuale.
    if !dir.join("unins000.exe").is_file() {
        open_external(page_url)?;
        return Ok(InstallOutcome::Manual(
            "Versione portatile: scarica il nuovo pacchetto dalla pagina della release".into(),
        ));
    }
    let roots: Vec<PathBuf> = ["LOCALAPPDATA", "APPDATA", "USERPROFILE"]
        .iter()
        .filter_map(|k| std::env::var_os(k).map(PathBuf::from))
        .collect();
    let args = windows_installer_args(dir, is_per_user_dir(dir, &roots));
    std::process::Command::new(setup)
        .args(&args)
        .spawn()
        .map_err(|e| format!("impossibile avviare l'installer: {e}"))?;
    Ok(InstallOutcome::Quit)
}

/// Bundle `.app` che contiene l'eseguibile (`X.app/Contents/MacOS/bin` → `X.app`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_app = bundle.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app).then(|| bundle.to_path_buf())
}

/// Racchiude una stringa tra apici singoli per la shell POSIX.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Script che, dopo l'uscita del processo `pid`, sostituisce `bundle` con `new_app`
/// (copia `ditto` in una cartella sorella e scambio), rimuove la quarantena, smonta
/// l'immagine disco e riapre l'app. In caso di errore ripristina il bundle originale.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn macos_helper_script(pid: u32, bundle: &Path, new_app: &Path, mount: &Path, log: &Path) -> String {
    let parent = bundle.parent().unwrap_or(Path::new("/"));
    let stem = bundle
        .file_name()
        .map_or_else(|| "Mixfonocraft.app".into(), |n| n.to_string_lossy().into_owned());
    let staged = parent.join(format!(".{stem}.mfc-new"));
    let old = parent.join(format!(".{stem}.mfc-old"));
    let q = |p: &Path| sh_quote(&p.to_string_lossy());
    format!(
        r#"#!/bin/sh
# Aggiornamento di Mixfonocraft AI Studio (generato dall'app, si cancella da solo).
PID={pid}
APP={app}
SRC={src}
NEW={new}
OLD={old}
MNT={mnt}
exec >>{log} 2>&1
echo "aggiornamento: attesa uscita del processo $PID"
i=0
while kill -0 "$PID" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -gt 600 ]; then
    echo "timeout: l'app non si è chiusa"
    hdiutil detach -quiet "$MNT" || hdiutil detach -force -quiet "$MNT"
    rm -f "$0"
    exit 1
  fi
  sleep 0.5
done
rm -rf "$NEW" "$OLD"
if ditto "$SRC" "$NEW"; then
  xattr -dr com.apple.quarantine "$NEW" 2>/dev/null
  if mv "$APP" "$OLD"; then
    if mv "$NEW" "$APP"; then
      rm -rf "$OLD"
      echo "aggiornamento completato"
    else
      echo "scambio fallito: ripristino"
      mv "$OLD" "$APP"
      rm -rf "$NEW"
    fi
  else
    echo "impossibile spostare il bundle corrente"
    rm -rf "$NEW"
  fi
else
  echo "copia fallita"
  rm -rf "$NEW"
fi
hdiutil detach -quiet "$MNT" || hdiutil detach -force -quiet "$MNT"
open "$APP"
rm -f "$0"
"#,
        app = q(bundle),
        src = q(new_app),
        new = q(&staged),
        old = q(&old),
        mnt = q(mount),
        log = q(log),
    )
}

/// `true` se nella cartella si possono creare file (prova reale, non solo permessi).
#[cfg(target_os = "macos")]
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".mfc-write-test-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(target_os = "macos")]
fn install_macos(dmg: &Path) -> Result<InstallOutcome, String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let manual = |why: &str| -> Result<InstallOutcome, String> {
        open_external(&dmg.to_string_lossy())?;
        Ok(InstallOutcome::Manual(format!(
            "{why}: trascina Mixfonocraft nella cartella Applicazioni dall'immagine disco aperta"
        )))
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let Some(bundle) = bundle_from_exe(&exe) else {
        return manual("App non avviata da un bundle .app");
    };
    let parent = bundle.parent().ok_or("cartella del bundle sconosciuta")?;
    // Bundle in sola lettura (es. avviato dall'immagine disco o in App Translocation).
    if !dir_writable(parent) || !dir_writable(&bundle) {
        return manual("Cartella dell'app non scrivibile");
    }

    let pid = std::process::id();
    let work = dmg.parent().map_or_else(update_dir, Path::to_path_buf);
    let mount = work.join(format!("mnt-{pid}"));
    std::fs::create_dir_all(&mount).map_err(|e| format!("punto di montaggio: {e}"))?;
    let status = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"])
        .arg(&mount)
        .arg(dmg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("hdiutil non disponibile: {e}"))?;
    if !status.success() {
        return Err("impossibile montare l'immagine disco".into());
    }
    let detach = || {
        let _ = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
    };
    let mut apps: Vec<PathBuf> = std::fs::read_dir(&mount)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
                .collect()
        })
        .unwrap_or_default();
    apps.sort_by_key(|p| p.file_name() != Some(std::ffi::OsStr::new("Mixfonocraft.app")));
    let Some(new_app) = apps.into_iter().next() else {
        detach();
        return Err("nessuna app nell'immagine disco".into());
    };

    let script = work.join(format!("update-{pid}.sh"));
    let log = work.join("update.log");
    let body = macos_helper_script(pid, &bundle, &new_app, &mount, &log);
    if let Err(e) = std::fs::write(&script, body) {
        detach();
        return Err(format!("script di aggiornamento: {e}"));
    }
    let spawned = Command::new("/bin/sh")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    if let Err(e) = spawned {
        detach();
        return Err(format!("impossibile avviare lo script di aggiornamento: {e}"));
    }
    Ok(InstallOutcome::Quit)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/github_release_latest.json");

    #[test]
    fn parses_release_fixture() {
        let r = parse_release(FIXTURE).unwrap();
        assert_eq!(r.version, Version::new(0, 5, 0));
        assert_eq!(r.tag, "v0.5.0");
        assert_eq!(r.name, "Mixfonocraft AI Studio v0.5.0");
        assert!(r.notes.contains("Nova"));
        assert_eq!(
            r.page_url,
            "https://github.com/basoxxx/Mixfonocraft-ai-studio/releases/tag/v0.5.0"
        );
        assert_eq!(r.assets.len(), 5);
        assert!(!r.prerelease);
    }

    #[test]
    fn rejects_bad_release_json() {
        assert!(parse_release("non json").is_err());
        assert!(parse_release(r#"{"tag_name":"latest"}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v1.0.0","draft":true}"#).is_err());
        // Campi facoltativi assenti: valori predefiniti.
        let r = parse_release(r#"{"tag_name":"1.2.3"}"#).unwrap();
        assert_eq!(r.version, Version::new(1, 2, 3));
        assert!(r.assets.is_empty());
        assert!(r.page_url.ends_with("/releases/tag/1.2.3"));
    }

    #[test]
    fn version_comparison() {
        let v = |s| parse_version(s).unwrap();
        assert_eq!(v("v0.4.0"), Version::new(0, 4, 0));
        assert_eq!(v(" V1.10.2 "), Version::new(1, 10, 2));
        assert!(parse_version("v1.0").is_none());
        assert!(is_newer("0.4.0", &v("v0.5.0")));
        assert!(is_newer("0.4.0", &v("v0.4.1")));
        assert!(is_newer("0.9.0", &v("v0.10.0")));
        assert!(!is_newer("0.4.0", &v("v0.4.0")));
        assert!(!is_newer("0.5.0", &v("v0.4.9")));
        assert!(is_newer("1.0.0-beta.1", &v("1.0.0")));
        assert!(!is_newer("1.0.0", &v("1.0.0-rc.1")));
        assert!(!is_newer("non-valida", &v("9.9.9")));
        assert!(parse_version(CURRENT_VERSION).is_some());
    }

    #[test]
    fn selects_asset_per_platform() {
        let r = parse_release(FIXTURE).unwrap();
        let pick = |p| select_asset(&r.assets, p).map(|a| a.name.as_str());
        assert_eq!(pick(Platform::MacArm64), Some("Mixfonocraft-0.5.0-macos-arm64.dmg"));
        assert_eq!(pick(Platform::MacX86_64), Some("Mixfonocraft-0.5.0-macos-x86_64.dmg"));
        assert_eq!(
            pick(Platform::Windows),
            Some("Mixfonocraft-0.5.0-windows-x86_64-setup.exe")
        );
        assert_eq!(pick(Platform::Linux), None);
        assert_eq!(pick(Platform::Other), None);
        let only_msi = vec![Asset {
            name: "Mixfonocraft-0.5.0-windows-x86_64.msi".into(),
            browser_download_url: "https://example.invalid/x.msi".into(),
            size: 1,
        }];
        assert_eq!(select_asset(&only_msi, Platform::Windows), None);
    }

    #[test]
    fn file_names_are_checked() {
        assert_eq!(
            safe_file_name("Mixfonocraft-0.5.0-macos-arm64.dmg"),
            Some("Mixfonocraft-0.5.0-macos-arm64.dmg")
        );
        assert_eq!(safe_file_name("../x.dmg"), None);
        assert_eq!(safe_file_name("a\\b.exe"), None);
        assert_eq!(safe_file_name(".."), None);
        assert_eq!(safe_file_name(""), None);
    }

    #[test]
    fn settings_round_trip_and_merge() {
        let s = UpdateSettings::default();
        assert!(s.check_on_startup);
        assert!(s.auto_check_due(1_000_000));
        let s = UpdateSettings {
            check_on_startup: true,
            last_check: 1_000_000,
        };
        assert!(!s.auto_check_due(1_000_000 + 3600));
        assert!(s.auto_check_due(1_000_000 + CHECK_INTERVAL_SECS));
        assert!(s.auto_check_due(10), "orologio indietro");
        let off = UpdateSettings {
            check_on_startup: false,
            last_check: 0,
        };
        assert!(!off.auto_check_due(1_000_000));

        // Le altre chiavi del file restano intatte.
        let merged = s.merge_into_json(Some(r#"{"theme":"scuro","updates":{"check_on_startup":false}}"#));
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["theme"], "scuro");
        assert_eq!(UpdateSettings::from_json(&merged), s);
        assert_eq!(UpdateSettings::from_json("rotto"), UpdateSettings::default());
        assert_eq!(UpdateSettings::from_json("{}"), UpdateSettings::default());
        assert_eq!(
            UpdateSettings::from_json(r#"{"updates":{"check_on_startup":false}}"#),
            UpdateSettings {
                check_on_startup: false,
                last_check: 0
            }
        );
        let fresh: serde_json::Value = serde_json::from_str(&s.merge_into_json(Some("[1,2]"))).unwrap();
        assert_eq!(fresh["updates"]["last_check"], 1_000_000);
    }

    #[test]
    fn windows_args_and_scope() {
        let dir = PathBuf::from(r"C:\Users\Ada\AppData\Local\Programs\Mixfonocraft AI Studio");
        let roots = vec![PathBuf::from(r"C:\Users\Ada\AppData\Local")];
        assert!(is_per_user_dir(&dir, &roots));
        assert!(!is_per_user_dir(
            Path::new(r"C:\Program Files\Mixfonocraft AI Studio"),
            &roots
        ));
        assert!(!is_per_user_dir(Path::new(r"C:\Users\Ada\AppData\LocalX\a"), &roots));
        assert!(!is_per_user_dir(&dir, &[PathBuf::new()]));
        let args = windows_installer_args(&dir, true);
        for a in [
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
            "/RELAUNCH",
            "/CURRENTUSER",
        ] {
            assert!(args.iter().any(|x| x == a), "manca {a}");
        }
        assert_eq!(args.last().unwrap(), &format!("/DIR={}", dir.display()));
        assert!(windows_installer_args(&dir, false).iter().any(|x| x == "/ALLUSERS"));
    }

    #[test]
    fn macos_bundle_and_script() {
        assert_eq!(
            bundle_from_exe(Path::new(
                "/Applications/Mixfonocraft.app/Contents/MacOS/mixfonocraft-studio"
            )),
            Some(PathBuf::from("/Applications/Mixfonocraft.app"))
        );
        assert_eq!(bundle_from_exe(Path::new("/usr/local/bin/mixfonocraft-studio")), None);
        assert_eq!(bundle_from_exe(Path::new("/x/Foo/Contents/MacOS/bin")), None);

        let s = macos_helper_script(
            42,
            Path::new("/Users/ada/Apps/Mix'fono.app"),
            Path::new("/tmp/u/mnt-42/Mixfonocraft.app"),
            Path::new("/tmp/u/mnt-42"),
            Path::new("/tmp/u/update.log"),
        );
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(s.contains("PID=42\n"));
        assert!(s.contains(r"APP='/Users/ada/Apps/Mix'\''fono.app'"));
        assert!(s.contains(r"NEW='/Users/ada/Apps/.Mix'\''fono.app.mfc-new'"));
        assert!(s.contains("ditto \"$SRC\" \"$NEW\""));
        assert!(s.contains("xattr -dr com.apple.quarantine \"$NEW\""));
        assert!(s.contains("hdiutil detach -quiet \"$MNT\""));
        assert!(s.contains("open \"$APP\""));
        assert_eq!(sh_quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn progress_fraction() {
        let p = Progress::default();
        assert_eq!(p.fraction(), None);
        p.total.store(200, Ordering::Relaxed);
        p.bytes.store(50, Ordering::Relaxed);
        assert_eq!(p.fraction(), Some(0.25));
        assert!(p.take_result().is_none());
        p.finish(Ok(PathBuf::from("x")));
        assert_eq!(p.take_result(), Some(Ok(PathBuf::from("x"))));
        assert!(p.take_result().is_none());
    }
}
