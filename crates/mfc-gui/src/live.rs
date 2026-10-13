//! Server MCP live dentro la GUI: le richieste arrivano dall'endpoint locale
//! ([`mfc_mcp::live`]) e vengono eseguite sulla sessione della GUI, con lo stesso undo.
//! Un client si collega con `mixfonocraft mcp --attach`.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use mfc_mcp::live::{LiveListener, LiveRequest};
use mfc_mcp::session::Transport;
use mfc_mcp::{Server, Session};
use serde_json::Value;

use crate::state::Studio;

/// Collegamento MCP live attivo.
pub struct LiveMcp {
    listener: LiveListener,
    rx: Receiver<LiveRequest>,
    server: Server,
    /// Chiamate di tool servite.
    pub calls: usize,
    /// Ultimo tool eseguito.
    pub last_tool: Option<String>,
}

impl LiveMcp {
    /// Apre l'endpoint; `ctx` viene risvegliato a ogni richiesta.
    pub fn start(ctx: &egui::Context, workdir: PathBuf) -> std::io::Result<Self> {
        let ctx = ctx.clone();
        let (listener, rx) = LiveListener::start(Arc::new(move || ctx.request_repaint()))?;
        Ok(Self {
            listener,
            rx,
            server: Server::new(Session::new(workdir)),
            calls: 0,
            last_tool: None,
        })
    }

    /// Endpoint servito (socket o file di porta).
    pub fn endpoint(&self) -> String {
        self.listener.path().display().to_string()
    }

    /// Cartella di lavoro dei percorsi relativi.
    pub fn workdir(&self) -> &std::path::Path {
        &self.server.session.workdir
    }

    /// Serve le richieste in coda (da chiamare a ogni frame, sul thread della GUI).
    pub fn serve(&mut self, studio: &mut Studio) {
        while let Ok(req) = self.rx.try_recv() {
            let reply = self.handle(studio, req.message);
            let _ = req.reply.send(reply);
        }
    }

    /// Esegue un messaggio sulla sessione della GUI: progetto, undo e percorso vengono
    /// prestati alla sessione MCP, che usa anche il motore audio della GUI.
    pub fn handle(&mut self, studio: &mut Studio, message: Value) -> Option<Value> {
        let call = tool_call(&message);
        {
            let s = &mut self.server.session;
            std::mem::swap(&mut s.project, &mut studio.project);
            std::mem::swap(&mut s.history, &mut studio.history);
            s.path = studio.path.clone();
            s.transport = Some(Transport::attached(studio.engine.clone(), studio.audible));
        }
        let before = call.as_ref().map(|_| self.server.session.project.clone());
        let reply = self.server.handle(message);
        {
            let s = &mut self.server.session;
            std::mem::swap(&mut s.project, &mut studio.project);
            std::mem::swap(&mut s.history, &mut studio.history);
            studio.path = s.path.take();
            s.transport = None;
        }
        if let Some((name, args)) = call {
            let ok = reply
                .as_ref()
                .is_some_and(|r| r["result"]["isError"] == Value::Bool(false));
            let changed = before.as_ref() != Some(&studio.project);
            studio.after_external_change(&name, &args, changed, ok);
            self.calls += 1;
            self.last_tool = Some(name);
        }
        reply
    }
}

/// `(nome, argomenti)` se il messaggio è una `tools/call`.
fn tool_call(msg: &Value) -> Option<(String, Value)> {
    if msg.get("method")?.as_str()? != "tools/call" {
        return None;
    }
    let params = msg.get("params")?;
    let name = params.get("name")?.as_str()?.to_string();
    Some((name, params.get("arguments").cloned().unwrap_or(Value::Null)))
}

/// Cartella di lavoro predefinita per il server live: quella del progetto aperto, poi
/// `~/Music/Mixfonocraft` se esiste, poi la home, poi la directory corrente.
pub fn default_workdir(project_path: Option<&std::path::Path>) -> PathBuf {
    if let Some(dir) = project_path.and_then(|p| p.parent()).filter(|d| d.is_dir()) {
        return dir.to_path_buf();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    if let Some(home) = home {
        let music = home.join("Music").join("Mixfonocraft");
        return if music.is_dir() { music } else { home };
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
