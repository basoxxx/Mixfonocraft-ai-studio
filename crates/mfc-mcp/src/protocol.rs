//! Server MCP (Model Context Protocol) su JSON-RPC 2.0, trasporto stdio
//! (un messaggio JSON per riga).

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::session::Session;
use crate::tools::{self, Tool};

/// Versioni del protocollo supportate (la prima è la preferita).
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Mixfonocraft AI Studio: DAW clean-room in stile FL Studio. \
Flusso tipico: project_new → channel_add (vedi plugin_list) → pattern_add / steps_set / notes_add → \
playlist_place → mixer_* → render_wav. Le posizioni sono in tick (PPQ del progetto, default 96: \
1 beat = 96 tick, 1 step = 24 tick, 1 battuta 4/4 = 384 tick). Ogni modifica è annullabile con undo.";

/// Server MCP con la sua sessione.
pub struct Server {
    pub session: Session,
    tools: Vec<Tool>,
    initialized: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self::new(Session::default())
    }
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

impl Server {
    pub fn new(session: Session) -> Self {
        Self { session, tools: tools::all(), initialized: false }
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Gestisce un messaggio (richiesta, notifica o batch). Restituisce la risposta, se dovuta.
    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        if let Value::Array(batch) = msg {
            let out: Vec<Value> = batch.into_iter().filter_map(|m| self.handle(m)).collect();
            return (!out.is_empty()).then_some(Value::Array(out));
        }
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // Risposte del client a nostre richieste: ignorate (non ne facciamo).
            return id.filter(|_| msg.get("result").is_none() && msg.get("error").is_none())
                .map(|id| error(id, -32600, "richiesta non valida"));
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            // Notifica.
            if method == "notifications/initialized" {
                self.initialized = true;
            }
            return None;
        };
        Some(match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == requested).unwrap_or(&PROTOCOL_VERSIONS[0]);
                ok(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "mixfonocraft", "title": "Mixfonocraft AI Studio", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => ok(id, json!({})),
            "tools/list" => ok(
                id,
                json!({"tools": self.tools.iter().map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": (t.schema)(),
                })).collect::<Vec<_>>()}),
            ),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return Some(error(id, -32602, "parametro 'name' mancante"));
                };
                let Some(tool) = self.tools.iter().find(|t| t.name == name) else {
                    return Some(error(id, -32602, format!("tool sconosciuto: {name}")));
                };
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let result = (tool.handler)(&mut self.session, &args);
                ok(id, tool_result(result))
            }
            "resources/list" => ok(id, json!({"resources": []})),
            "prompts/list" => ok(id, json!({"prompts": []})),
            _ => error(id, -32601, format!("metodo non supportato: {method}")),
        })
    }

    /// Ciclo stdio: legge una riga JSON per messaggio e scrive le risposte su `out`.
    pub fn run<R: BufRead, W: Write>(&mut self, input: R, mut out: W) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => self.handle(msg),
                Err(e) => Some(error(Value::Null, -32700, format!("JSON non valido: {e}"))),
            };
            if let Some(r) = reply {
                serde_json::to_writer(&mut out, &r)?;
                out.write_all(b"\n")?;
                out.flush()?;
            }
        }
        Ok(())
    }

    /// Avvia il server su stdin/stdout.
    pub fn run_stdio(&mut self) -> std::io::Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        self.run(stdin.lock(), stdout.lock())
    }
}

fn tool_result(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => {
            let text = serde_json::to_string_pretty(&v).unwrap_or_default();
            let mut res = json!({"content": [{"type": "text", "text": text}], "isError": false});
            if v.is_object() {
                res["structuredContent"] = v;
            }
            res
        }
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}
