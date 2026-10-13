//! Collegamento MCP live: un client sul socket locale modifica il progetto aperto nella GUI.

use std::io::{BufRead, BufReader, Write};

use mfc_core::Project;
use mfc_gui::live::LiveMcp;
use mfc_gui::Studio;
use serde_json::{json, Value};

fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
}

#[test]
fn live_tools_edit_the_gui_project() {
    let dir = std::env::temp_dir().join(format!("mfc_live_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Endpoint isolato per il test.
    std::env::set_var("MIXFONOCRAFT_SOCKET", dir.join("live.sock"));
    let ctx = egui::Context::default();
    let mut studio = Studio::new(Project::new("GUI"));
    let mut live = LiveMcp::start(&ctx, dir.clone()).expect("endpoint live");
    // Una seconda istanza non può servire lo stesso endpoint.
    assert!(LiveMcp::start(&ctx, dir.clone()).is_err());

    let client = std::thread::spawn(move || {
        let stream = mfc_mcp::live::connect().expect("connessione alla GUI");
        let mut out = stream.try_clone().unwrap();
        let mut lines = BufReader::new(stream).lines();
        let mut ask = |msg: Value| -> Value {
            writeln!(out, "{msg}").unwrap();
            out.flush().unwrap();
            serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
        };
        let init = ask(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "mixfonocraft");
        let ch = ask(call(
            2,
            "channel_add",
            json!({"name": "Kick", "plugin": "mfc.kick", "insert": 1}),
        ));
        assert_eq!(ch["result"]["isError"], false, "{ch}");
        let r = ask(call(
            3,
            "steps_set",
            json!({"channel": ch["result"]["structuredContent"]["id"], "row": "x...x...x...x..."}),
        ));
        assert_eq!(r["result"]["isError"], false, "{r}");
        let r = ask(call(4, "project_set", json!({"tempo": 132})));
        assert_eq!(r["result"]["structuredContent"]["tempo"], 132.0);
        let r = ask(call(5, "transport", json!({"action": "status"})));
        assert_eq!(
            r["result"]["structuredContent"]["running"], true,
            "usa il motore della GUI: {r}"
        );
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !client.is_finished() {
        live.serve(&mut studio);
        assert!(std::time::Instant::now() < deadline, "timeout del client");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    client.join().unwrap();

    // Il progetto della GUI è quello modificato dall'agente, con undo condiviso.
    assert_eq!(studio.project.tempo, 132.0);
    assert_eq!(studio.project.channels.len(), 1);
    assert_eq!(studio.project.channels[0].name, "Kick");
    assert!(studio.dirty);
    assert_eq!(live.calls, 4);
    studio.undo();
    assert_eq!(studio.project.tempo, 140.0);

    // Il socket viene rimosso alla chiusura.
    let sock = dir.join("live.sock");
    assert!(sock.exists() || cfg!(not(unix)));
    drop(live);
    assert!(!sock.exists());
}
