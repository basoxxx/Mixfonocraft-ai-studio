//! Collegamento live tra un client MCP e la GUI.
//!
//! La GUI apre un endpoint locale ([`LiveListener`]) e riceve le richieste JSON-RPC su un
//! canale: le esegue sulla propria sessione, così l'agente e l'utente vedono lo stesso
//! progetto, con undo condiviso. `mixfonocraft mcp --attach` fa da ponte stdio ↔ endpoint
//! ([`bridge_stdio`]).
//!
//! Endpoint:
//! - macOS e Linux: socket Unix (permessi 0600) in `$XDG_RUNTIME_DIR`, oppure nella cartella
//!   temporanea dell'utente; la variabile `MIXFONOCRAFT_SOCKET` lo sostituisce.
//! - Windows: TCP su 127.0.0.1 con porta casuale; porta e token sono scritti in un file
//!   nella cartella temporanea dell'utente e il client deve inviare il token come prima riga.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use serde_json::Value;

/// Richiesta ricevuta dall'endpoint: messaggio JSON-RPC e canale per la risposta
/// (`None` per le notifiche).
pub struct LiveRequest {
    pub message: Value,
    pub reply: Sender<Option<Value>>,
}

/// Percorso del socket Unix (o del file di porta su Windows).
pub fn endpoint_path() -> PathBuf {
    if let Some(p) = std::env::var_os("MIXFONOCRAFT_SOCKET") {
        return PathBuf::from(p);
    }
    #[cfg(unix)]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir).join("mixfonocraft.sock");
        }
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
        std::env::temp_dir().join(format!("mixfonocraft-{user}.sock"))
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join("mixfonocraft-live.txt")
    }
}

// ---------------------------------------------------------------- stream

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
type Stream = std::net::TcpStream;

/// Si collega all'endpoint della GUI, se è aperto.
pub fn connect() -> std::io::Result<Stream> {
    #[cfg(unix)]
    {
        Stream::connect(endpoint_path())
    }
    #[cfg(not(unix))]
    {
        let info = std::fs::read_to_string(endpoint_path())?;
        let mut parts = info.split_whitespace();
        let port: u16 = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "file di porta non valido"))?;
        let token = parts.next().unwrap_or("");
        let mut s = Stream::connect(("127.0.0.1", port))?;
        writeln!(s, "{token}")?;
        Ok(s)
    }
}

// ---------------------------------------------------------------- listener

/// Endpoint locale aperto dalla GUI. Alla distruzione rimuove il socket (o il file di porta).
pub struct LiveListener {
    path: PathBuf,
}

impl LiveListener {
    /// Apre l'endpoint. Le richieste arrivano su `Receiver`; `wake` viene chiamato a ogni
    /// richiesta (es. `ctx.request_repaint()`) perché chi riceve possa servirla subito.
    /// Fallisce se un'altra istanza sta già servendo l'endpoint.
    pub fn start(wake: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<(LiveListener, Receiver<LiveRequest>)> {
        let path = endpoint_path();
        let (tx, rx) = mpsc::channel();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            use std::os::unix::net::UnixListener;
            if path.exists() {
                if Stream::connect(&path).is_ok() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        format!("un'altra istanza usa già {}", path.display()),
                    ));
                }
                // Socket rimasto da un'istanza chiusa male.
                let _ = std::fs::remove_file(&path);
            }
            let listener = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            thread::Builder::new().name("mcp-live-accept".into()).spawn(move || {
                for stream in listener.incoming().flatten() {
                    spawn_connection(stream, tx.clone(), wake.clone(), None);
                }
            })?;
        }
        #[cfg(not(unix))]
        {
            if connect().is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "un'altra istanza sta già servendo il collegamento live",
                ));
            }
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();
            let token = random_token();
            std::fs::write(&path, format!("{port} {token}\n"))?;
            thread::Builder::new().name("mcp-live-accept".into()).spawn(move || {
                for stream in listener.incoming().flatten() {
                    spawn_connection(stream, tx.clone(), wake.clone(), Some(token.clone()));
                }
            })?;
        }
        Ok((LiveListener { path }, rx))
    }

    /// Endpoint servito.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for LiveListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(not(unix))]
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Serve una connessione: una riga JSON per messaggio, risposta sulla stessa connessione.
fn spawn_connection(stream: Stream, tx: Sender<LiveRequest>, wake: Arc<dyn Fn() + Send + Sync>, token: Option<String>) {
    let _ = thread::Builder::new().name("mcp-live-conn".into()).spawn(move || {
        let Ok(write_half) = stream.try_clone() else { return };
        let mut out = std::io::BufWriter::new(write_half);
        let mut lines = BufReader::new(stream).lines();
        if let Some(expected) = token {
            match lines.next() {
                Some(Ok(t)) if t.trim() == expected => {}
                _ => return,
            }
        }
        for line in lines {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(message) => {
                    let (rtx, rrx) = mpsc::channel();
                    if tx.send(LiveRequest { message, reply: rtx }).is_err() {
                        break;
                    }
                    wake();
                    match rrx.recv() {
                        Ok(r) => r,
                        Err(_) => break,
                    }
                }
                Err(e) => Some(serde_json::json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": format!("JSON non valido: {e}")},
                })),
            };
            if let Some(r) = reply {
                if serde_json::to_writer(&mut out, &r).is_err() || out.write_all(b"\n").is_err() || out.flush().is_err()
                {
                    break;
                }
            }
        }
    });
}

// ---------------------------------------------------------------- ponte

/// Ponte stdio ↔ GUI: inoltra stdin all'endpoint e le risposte su stdout, fino alla
/// chiusura di uno dei due lati. Restituisce `Ok(false)` se la GUI non è in ascolto.
pub fn bridge_stdio() -> std::io::Result<bool> {
    let Ok(stream) = connect() else { return Ok(false) };
    bridge(stream, std::io::stdin().lock(), std::io::stdout())?;
    Ok(true)
}

/// Inoltra `input` → `stream` (riga per riga) e `stream` → `output`.
pub fn bridge<R: BufRead, W: Write + Send + 'static>(stream: Stream, input: R, output: W) -> std::io::Result<()> {
    let reader = stream.try_clone()?;
    let pump = thread::spawn(move || -> std::io::Result<()> {
        let mut output = output;
        for line in BufReader::new(reader).lines() {
            let line = line?;
            output.write_all(line.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
        Ok(())
    });
    let mut writer = stream;
    for line in input.lines() {
        let line = line?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    // Fine dell'input: chiude il lato di scrittura e attende le ultime risposte.
    let _ = writer.shutdown(std::net::Shutdown::Write);
    pump.join().unwrap_or(Ok(()))
}
