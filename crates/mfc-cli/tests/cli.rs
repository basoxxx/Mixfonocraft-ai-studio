//! Test di integrazione della CLI: progetto demo, render, conversioni, binario e server MCP.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use mfc_cli::{
    build_demo, flp_inspect, load_project, parse_format, plugins_json, render_wav, save_project, Format, RenderRequest,
    Summary, DEMO_BARS, DEMO_BPM,
};
use mfc_core::{ClipSource, Project};
use serde_json::Value;

/// Cartella temporanea univoca, rimossa al termine del test.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mfc-cli-{tag}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("creazione cartella temporanea");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bar_ticks(p: &Project) -> u64 {
    p.time.ticks_per_bar()
}

#[test]
fn demo_has_expected_structure() {
    let p = build_demo().expect("demo");
    assert_eq!(p.channels.len(), 5, "cinque canali");
    assert_eq!(p.patterns.len(), 2, "due pattern");
    assert_eq!(p.tempo, DEMO_BPM);
    assert_eq!(p.automations.len(), 1, "una clip di automazione");
    assert!(p
        .playlist
        .clips
        .iter()
        .any(|c| matches!(c.source, ClipSource::Automation(_))));
    assert!(p
        .playlist
        .clips
        .iter()
        .any(|c| matches!(c.source, ClipSource::Pattern(_))));
    // Il brano dura esattamente DEMO_BARS battute (ultima clip di automazione compresa).
    assert_eq!(p.song_length(), (DEMO_BARS as u64) * bar_ticks(&p));
    // Ogni canale ha note in almeno un pattern, tranne che nei soli step di batteria.
    let total_notes: usize = p.patterns.iter().flat_map(|pt| pt.notes.values()).map(Vec::len).sum();
    assert!(total_notes > 100, "note sufficienti: {total_notes}");
}

#[test]
fn demo_renders_non_silent_with_expected_duration_and_fade() {
    let p = build_demo().expect("demo");
    let tmp = TempDir::new("render");
    let out = tmp.path("demo.wav");
    let req = RenderRequest {
        sample_rate: 22_050,
        tail_seconds: 0.2,
        ..Default::default()
    };
    let report = render_wav(&p, &out, &req).expect("render");
    let expected = p.song_seconds() + 0.2;
    assert!(
        (report.seconds - expected).abs() < 0.01,
        "durata {} attesa {}",
        report.seconds,
        expected
    );
    assert!(report.peak_db > -30.0, "render quasi silenzioso: {} dB", report.peak_db);
    assert!(report.peak_db < 0.0, "render in clipping: {} dB", report.peak_db);
    assert!(!report.clipping);
    assert!(out.exists());

    // La dissolvenza porta il master a zero: la coda dopo la fine è silenziosa.
    let audio = mfc_engine::render(
        &p,
        &mfc_engine::RenderOptions {
            sample_rate: 22_050,
            tail_seconds: 0.2,
            ..Default::default()
        },
    )
    .expect("render in memoria")
    .mix;
    let tail_start = audio.frames() - (0.1 * 22_050.0) as usize;
    let tail_peak = audio.left[tail_start..]
        .iter()
        .chain(&audio.right[tail_start..])
        .fold(0f32, |m, x| m.max(x.abs()));
    assert!(tail_peak < 1e-3, "coda non silenziosa: {tail_peak}");
}

#[test]
fn flp_roundtrip_keeps_channels_and_tempo() {
    let tmp = TempDir::new("convert");
    let demo = build_demo().expect("demo");
    let mfc = tmp.path("demo.mfc");
    let flp = tmp.path("demo.flp");
    let back = tmp.path("back.mfc");

    save_project(&demo, &mfc).expect("salva .mfc");
    let from_mfc = load_project(&mfc).expect("carica .mfc");
    assert_eq!(from_mfc.project, demo, "il .mfc è un round-trip esatto");

    save_project(&from_mfc.project, &flp).expect("esporta .flp");
    let from_flp = load_project(&flp).expect("importa .flp");
    assert_eq!(from_flp.project.channels.len(), demo.channels.len());
    assert!(
        (from_flp.project.tempo - demo.tempo).abs() < 1e-6,
        "tempo perso nel .flp"
    );

    save_project(&from_flp.project, &back).expect("riscrive .mfc");
    let final_project = load_project(&back).expect("ricarica .mfc");
    assert_eq!(final_project.project.channels.len(), 5);
    assert!((final_project.project.tempo - DEMO_BPM).abs() < 1e-6);
    assert_eq!(final_project.project.patterns.len(), 2);
}

#[test]
fn midi_export_then_import_keeps_notes_and_tempo() {
    let tmp = TempDir::new("midi");
    let demo = build_demo().expect("demo");
    let mid = tmp.path("demo.mid");
    save_project(&demo, &mid).expect("esporta .mid");
    let loaded = load_project(&mid).expect("importa .mid");
    let notes: usize = loaded
        .project
        .patterns
        .iter()
        .flat_map(|pt| pt.notes.values())
        .map(Vec::len)
        .sum();
    assert!(notes > 0, "nessuna nota importata dal MIDI");
    assert_eq!(
        loaded.project.playlist.clips.len(),
        1,
        "il MIDI è piazzato come una clip"
    );
    assert!((loaded.project.tempo - DEMO_BPM).abs() < 1e-6);
}

#[test]
fn format_detection_and_render_options() {
    assert_eq!(Format::from_path(Path::new("a.MFC")).unwrap(), Format::Mfc);
    assert_eq!(Format::from_path(Path::new("a.flp")).unwrap(), Format::Flp);
    assert_eq!(Format::from_path(Path::new("a.midi")).unwrap(), Format::Midi);
    assert!(Format::from_path(Path::new("a.wav")).is_err());
    assert!(Format::from_path(Path::new("nessuna_estensione")).is_err());
    assert!(parse_format("int16").is_ok());
    assert!(parse_format("float32").is_ok());
    assert!(parse_format("mp3").is_err());
}

#[test]
fn summary_describes_demo() {
    let p = build_demo().expect("demo");
    let s = Summary::from_project(&p);
    assert_eq!(s.channels.len(), 5);
    assert_eq!(s.patterns.len(), 2);
    assert!((s.length_bars - DEMO_BARS).abs() < 1e-9);
    assert!((s.length_seconds - 15.0).abs() < 1e-6, "8 battute a 128 BPM = 15 s");
    // Master sempre presente, più il lead con i suoi due effetti.
    assert_eq!(s.inserts[0].index, 0);
    let lead = s.inserts.iter().find(|i| i.index == 5).expect("insert 5 in uso");
    assert_eq!(lead.effects, vec!["mfc.reverb".to_string(), "mfc.delay".to_string()]);
    let text = s.to_text();
    assert!(text.contains("128.0 BPM"), "{text}");
    assert!(text.contains("Lead"));
    let json = s.to_json();
    assert_eq!(json["channels"].as_array().map(Vec::len), Some(5));
}

#[test]
fn flp_inspect_paginates_and_filters() {
    let tmp = TempDir::new("inspect");
    let flp = tmp.path("demo.flp");
    save_project(&build_demo().expect("demo"), &flp).expect("esporta .flp");

    let page = flp_inspect(&flp, 0, 5, None).expect("ispezione");
    assert_eq!(page.rows.len(), 5);
    assert_eq!(page.matching, page.total);
    assert!(page.total > 5);

    let next = flp_inspect(&flp, 5, 5, None).expect("seconda pagina");
    assert_eq!(
        next.rows.first().map(|r| r.index),
        page.rows.last().map(|r| r.index + 1)
    );

    let version = flp_inspect(&flp, 0, 10, Some(mfc_flp::id::FL_VERSION)).expect("filtro id");
    assert_eq!(version.matching, 1, "un solo evento di versione");
    assert!(version.rows.iter().all(|r| r.id == mfc_flp::id::FL_VERSION));
}

#[test]
fn plugin_catalog_lists_parameters() {
    let v = plugins_json();
    let plugins = v["plugins"].as_array().expect("array");
    assert_eq!(plugins.len(), mfc_plugins::registry().len());
    let kick = plugins.iter().find(|p| p["id"] == "mfc.kick").expect("kick");
    assert!(kick["params"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "start_freq"));
}

#[test]
fn info_json_from_binary() {
    let tmp = TempDir::new("info");
    let mfc = tmp.path("demo.mfc");
    save_project(&build_demo().expect("demo"), &mfc).expect("salva");

    let out = Command::new(env!("CARGO_BIN_EXE_mixfonocraft"))
        .arg("info")
        .arg("--json")
        .arg(&mfc)
        .output()
        .expect("avvio del binario");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON valido");
    assert_eq!(v["tempo"].as_f64(), Some(DEMO_BPM));
    assert_eq!(v["channels"].as_array().map(Vec::len), Some(5));
    assert_eq!(v["patterns"].as_array().map(Vec::len), Some(2));
    assert_eq!(v["time_signature"], "4/4");
}

#[test]
fn binary_reports_errors_on_stderr() {
    let tmp = TempDir::new("badext");
    let bad = tmp.path("nope.wav");
    fs::write(&bad, b"x").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mixfonocraft"))
        .arg("info")
        .arg(&bad)
        .output()
        .expect("avvio del binario");
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "nessun output su stdout in caso d'errore");
    assert!(!out.stderr.is_empty());
}

#[test]
fn mcp_stdio_answers_initialize_and_lists_tools() {
    let tmp = TempDir::new("mcp");
    let mut child = Command::new(env!("CARGO_BIN_EXE_mixfonocraft"))
        .arg("mcp")
        .arg("--workdir")
        .arg(&tmp.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("avvio del server MCP");

    {
        let mut stdin = child.stdin.take().expect("stdin");
        let lines = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ];
        for l in lines {
            writeln!(stdin, "{l}").expect("scrittura su stdin");
        }
        // Chiudendo stdin il server termina.
    }
    let out = child.wait_with_output().expect("attesa del server");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let responses: Vec<Value> = String::from_utf8(out.stdout)
        .expect("UTF-8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("ogni riga di stdout è JSON-RPC"))
        .collect();
    assert_eq!(
        responses.len(),
        2,
        "initialize e tools/list (la notifica non ha risposta)"
    );

    let init = responses.iter().find(|r| r["id"] == 1).expect("risposta initialize");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "mixfonocraft");

    let list = responses.iter().find(|r| r["id"] == 2).expect("risposta tools/list");
    let tools = list["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "project_new",
        "channel_add",
        "notes_add",
        "playlist_place",
        "render_wav",
        "flp_inspect",
    ] {
        assert!(names.contains(&expected), "manca il tool {expected}");
    }
    assert!(tools.iter().all(|t| t["inputSchema"].is_object()));
}

#[test]
fn mcp_stdio_survives_invalid_json() {
    let tmp = TempDir::new("mcp-bad");
    let mut child = Command::new(env!("CARGO_BIN_EXE_mixfonocraft"))
        .args(["mcp", "--workdir"])
        .arg(&tmp.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("avvio del server MCP");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(stdin, "{{non json").unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":7,"method":"ping"}}"#).unwrap();
    }
    let out = child.wait_with_output().expect("attesa");
    let lines: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["id"], 7);
}

#[test]
fn new_project_sets_title_and_tempo() {
    let p = mfc_cli::new_project("Prova", Some(96.0)).expect("nuovo progetto");
    assert_eq!(p.info.title, "Prova");
    assert_eq!(p.tempo, 96.0);
    assert_eq!(p.patterns.len(), 1);
    assert!(mfc_cli::new_project("x", Some(5.0)).is_err(), "tempo fuori range");
}
