use mfc_mcp::{Server, Session};
use serde_json::{json, Value};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mfc_mcp_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Client {
    server: Server,
    next: u64,
}

impl Client {
    fn new(tag: &str) -> Self {
        let mut c = Client { server: Server::new(Session::new(tmpdir(tag))), next: 1 };
        let r = c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}));
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert!(c.server.handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        c
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.server.handle(json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params})).unwrap()
    }

    /// Chiama un tool e restituisce `structuredContent`; panica se `isError`.
    fn call(&mut self, name: &str, args: Value) -> Value {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        assert_eq!(res["isError"], false, "{name} fallito: {}", res["content"][0]["text"]);
        res.get("structuredContent").cloned().unwrap_or(Value::Null)
    }

    fn call_err(&mut self, name: &str, args: Value) -> String {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        assert_eq!(r["result"]["isError"], true, "{name} doveva fallire");
        r["result"]["content"][0]["text"].as_str().unwrap().to_string()
    }
}

#[test]
fn tools_list_has_valid_schemas() {
    let mut c = Client::new("list");
    let r = c.request("tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 40, "solo {} tool", tools.len());
    for t in tools {
        assert!(t["name"].is_string() && t["description"].is_string());
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
    }
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for n in ["project_new", "channel_add", "steps_set", "notes_add", "playlist_place", "mixer_add_effect", "render_wav", "flp_import", "flp_export", "midi_import", "transport"] {
        assert!(names.contains(&n), "manca {n}");
    }
}

#[test]
fn protocol_errors() {
    let mut c = Client::new("err");
    let r = c.request("nope", json!({}));
    assert_eq!(r["error"]["code"], -32601);
    let r = c.request("tools/call", json!({"name": "does_not_exist"}));
    assert_eq!(r["error"]["code"], -32602);
    let msg = c.call_err("channel_add", json!({"name": "x", "plugin": "mfc.nonexistent"}));
    assert!(msg.contains("plugin sconosciuto"));
    let msg = c.call_err("project_set", json!({"tempo": 5000}));
    assert!(msg.contains("tempo"));
    assert_eq!(c.request("ping", json!({}))["result"], json!({}));
}

#[test]
fn full_production_workflow() {
    let mut c = Client::new("flow");
    c.call("project_new", json!({"title": "Demo", "tempo": 128}));
    let kick = c.call("channel_add", json!({"name": "Kick", "plugin": "mfc.kick", "insert": 1}))["id"].as_u64().unwrap();
    let hat = c.call("channel_add", json!({"name": "Hat", "plugin": "mfc.drumsynth", "insert": 2, "params": {"type": 3}}))["id"].as_u64().unwrap();
    let bass = c.call("channel_add", json!({"name": "Bass", "plugin": "mfc.3xosc", "insert": 3}))["id"].as_u64().unwrap();

    c.call("steps_set", json!({"channel": kick, "row": "x...x...x...x..."}));
    c.call("steps_set", json!({"channel": hat, "row": "..x...x...x...x."}));
    c.call("notes_add", json!({"channel": bass, "notes": [
        {"step": 0, "length": 48, "key": "C3"}, {"step": 4, "length": 48, "key": "D#3"},
        {"step": 8, "length": 48, "key": "G2"}, {"step": 12, "length": 48, "key": 41, "velocity": 1.0}
    ]}));
    let notes = c.call("notes_list", json!({"channel": bass}));
    assert_eq!(notes["notes"][bass.to_string()].as_array().unwrap().len(), 4);
    assert_eq!(notes["notes"][bass.to_string()][0]["key"], 36);

    let pats = c.call("pattern_list", json!({}));
    let pat = pats["patterns"][0]["id"].as_u64().unwrap();
    let placed = c.call("playlist_place", json!({"track": 0, "bar": 0, "pattern": pat, "repeat": 4}));
    assert_eq!(placed["clips"].as_array().unwrap().len(), 4);
    assert_eq!(placed["end"], 4 * 384);

    c.call("mixer_add_effect", json!({"insert": 3, "plugin": "mfc.reverb", "params": {"wet": 0.3}}));
    c.call("mixer_set", json!({"insert": 2, "volume": 0.6, "pan": 0.3, "name": "Hats"}));
    c.call("mixer_route", json!({"from": 1, "to": 4, "gain": 0.5}));
    let mixer = c.call("mixer_list", json!({}));
    assert!(mixer["inserts"].as_array().unwrap().iter().any(|i| i["name"] == "Hats"));
    let msg = c.call_err("mixer_route", json!({"from": 0, "to": 1}));
    assert!(msg.contains("ciclo"));

    let auto = c.call("automation_add", json!({
        "name": "Fade", "target": {"type": "master_volume"}, "min": 0.0, "max": 0.8,
        "points": [{"position": 0, "value": 1.0}, {"position": 1536, "value": 0.2}], "track": 5, "position": 0
    }));
    assert!(auto["outcome"].is_object());

    let info = c.call("project_info", json!({}));
    assert_eq!(info["tempo"], 128.0);
    assert_eq!(info["song_length_bars"], 4.0);

    let r = c.call("render_wav", json!({"path": "demo.wav", "stems": true, "tail_seconds": 0.5, "sample_rate": 22050}));
    let secs = r["seconds"].as_f64().unwrap();
    assert!((secs - (4.0 * 4.0 * 60.0 / 128.0 + 0.5)).abs() < 0.01, "{secs}");
    assert!(r["peak_db"].as_f64().unwrap() > -40.0);
    assert_eq!(r["stems"].as_array().unwrap().len(), 4, "{r}");
    let path = r["path"].as_str().unwrap();
    assert!(hound::WavReader::open(path).is_ok());

    // Salvataggio nativo + riapertura.
    c.call("project_save", json!({"path": "demo.mfc"}));
    c.call("project_new", json!({}));
    let info = c.call("project_open", json!({"path": "demo.mfc"}));
    assert_eq!(info["channels"].as_array().unwrap().len(), 3);

    // FL Studio round trip.
    c.call("flp_export", json!({"path": "demo.flp"}));
    let ins = c.call("flp_inspect", json!({"path": "demo.flp", "limit": 5}));
    assert!(ins["total_events"].as_u64().unwrap() > 10);
    let info = c.call("flp_import", json!({"path": "demo.flp"}));
    assert_eq!(info["tempo"], 128.0);
    assert_eq!(info["channels"].as_array().unwrap().len(), 3);
    assert_eq!(info["clips"].as_u64().unwrap() >= 4, true);

    // MIDI round trip.
    c.call("midi_export", json!({"path": "demo.mid"}));
    let m = c.call("midi_import", json!({"path": "demo.mid", "pattern_name": "From MIDI"}));
    assert!(!m["channels"].as_array().unwrap().is_empty());
    c.call("undo", json!({}));
    let pats = c.call("pattern_list", json!({}));
    assert!(pats["patterns"].as_array().unwrap().iter().all(|p| p["name"] != "From MIDI"));
}

#[test]
fn undo_redo_via_tools() {
    let mut c = Client::new("undo");
    c.call("project_set", json!({"tempo": 100}));
    c.call("project_set", json!({"tempo": 110}));
    c.call("undo", json!({}));
    assert_eq!(c.call("project_info", json!({}))["tempo"], 100.0);
    c.call("redo", json!({}));
    assert_eq!(c.call("project_info", json!({}))["tempo"], 110.0);
    let h = c.call("history", json!({}));
    assert_eq!(h["undo"].as_array().unwrap().len(), 2);
}

#[test]
fn raw_commands_are_atomic() {
    let mut c = Client::new("raw");
    c.call_err("apply_commands", json!({"commands": [{"op": "set_tempo", "bpm": 90}, {"op": "set_tempo", "bpm": 9000}]}));
    assert_eq!(c.call("project_info", json!({}))["tempo"], 140.0);
    c.call("apply_commands", json!({"commands": [{"op": "set_tempo", "bpm": 90}, {"op": "add_pattern", "name": "B"}]}));
    assert_eq!(c.call("project_info", json!({}))["tempo"], 90.0);
}

#[test]
fn transport_runs_headless() {
    let mut c = Client::new("transport");
    let k = c.call("channel_add", json!({"name": "K", "plugin": "mfc.kick"}))["id"].as_u64().unwrap();
    c.call("steps_set", json!({"channel": k, "row": "x...x..."}));
    let s = c.call("transport", json!({"action": "play", "mode": "pattern"}));
    assert_eq!(s["running"], true);
    std::thread::sleep(std::time::Duration::from_millis(250));
    let s = c.call("transport", json!({"action": "status"}));
    assert_eq!(s["playing"], true);
    assert!(s["position_ticks"].as_f64().unwrap() > 0.0);
    c.call("preview_note", json!({"channel": k, "key": "C5", "duration_ms": 10}));
    let s = c.call("transport", json!({"action": "stop"}));
    assert_eq!(s["running"], true);
}

#[test]
fn stdio_loop() {
    let mut server = Server::new(Session::new(tmpdir("stdio")));
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#, "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, "\n",
        "not json\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_info","arguments":{}}}"#, "\n",
    );
    let mut out = Vec::new();
    server.run(input.as_bytes(), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(lines[1]["error"]["code"], -32700);
    assert_eq!(lines[2]["result"]["isError"], false);
}
