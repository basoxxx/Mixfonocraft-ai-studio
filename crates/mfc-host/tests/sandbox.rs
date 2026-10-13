//! Test d'integrazione della modalità sandbox (`mfc-plugin-host`) con i plugin di prova
//! `mfc-test-clap` e `mfc-test-vst3`.
//!
//! In sandbox l'audio ha UN blocco di latenza: il risultato di `process` è quello del
//! blocco precedente (il primo blocco è silenzio).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mfc_core::PluginInstance;
use mfc_host::sandbox::{SandboxBackend, SandboxConfig};
use mfc_host::{ExternalInstance, FormatBackend, Host, MidiEvent, PluginFormat, PluginInfo};

const SYNTH_ID: &str = "com.mixfonocraft.test.synth";
const GAIN_ID: &str = "com.mixfonocraft.test.gain";
const SR: f32 = 48_000.0;

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mfc-plugin-host"))
}

/// Configurazione per i test: attesa del blocco precedente generosa (deterministica
/// anche su macchine cariche) e nessuna istantanea periodica dello stato.
fn config() -> SandboxConfig {
    let mut c = SandboxConfig::new(helper());
    c.process_wait = Some(Duration::from_secs(10));
    c.state_snapshot_interval = None;
    c
}

fn host() -> Host {
    let h = Host::new_sandboxed(helper());
    h.set_sandbox_config(config());
    h
}

/// target/<profilo>/deps/<test> → target/<profilo>
fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

fn cargo_build(package: &str) {
    let profile = profile_dir();
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "-p", package, "--target-dir"])
        .arg(profile.parent().unwrap());
    if profile.file_name().is_some_and(|n| n == "release") {
        cmd.arg("--release");
    }
    assert!(
        cmd.status().expect("avvio cargo").success(),
        "build di {package} fallita"
    );
}

fn lib_path(name: &str) -> PathBuf {
    let lib = profile_dir().join(format!(
        "{}{name}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(lib.exists(), "libreria non trovata: {}", lib.display());
    lib
}

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mfc_sandbox_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(target_os = "macos")]
fn write_plist(bundle: &Path, id: &str) {
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>MFC Test</string>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>"#
        ),
    )
    .unwrap();
}

/// Plugin CLAP di prova installato come `MFC Test.clap` (una volta).
fn clap_path() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        cargo_build("mfc-test-clap");
        let lib = lib_path("mfc_test_clap");
        let dir = temp_root().join("clap");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("MFC Test.clap");
        if cfg!(target_os = "macos") {
            std::fs::create_dir_all(dest.join("Contents/MacOS")).unwrap();
            std::fs::copy(&lib, dest.join("Contents/MacOS/MFC Test")).unwrap();
            #[cfg(target_os = "macos")]
            write_plist(&dest, "com.mixfonocraft.test.clap");
        } else {
            std::fs::copy(&lib, &dest).unwrap();
        }
        dest
    })
}

/// Bundle VST3 di prova (una volta).
fn vst3_path() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        cargo_build("mfc-test-vst3");
        let lib = lib_path("mfc_test_vst3");
        let bundle = temp_root().join("vst3").join("MFC Test.vst3");
        let bin = if cfg!(target_os = "windows") {
            bundle.join("Contents/x86_64-win/MFC Test.vst3")
        } else if cfg!(target_os = "macos") {
            bundle.join("Contents/MacOS/MFC Test")
        } else {
            bundle.join(format!("Contents/{}-linux/MFC Test.so", std::env::consts::ARCH))
        };
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::copy(&lib, &bin).unwrap();
        #[cfg(target_os = "macos")]
        write_plist(&bundle, "com.mixfonocraft.test.vst3");
        bundle
    })
}

fn clap_info(id: &str) -> PluginInfo {
    SandboxBackend::new(PluginFormat::Clap, config())
        .scan(clap_path())
        .expect("scansione")
        .into_iter()
        .find(|p| p.id == id)
        .expect("plugin presente")
}

fn create(host: &Host, info: &PluginInfo, instance_id: &str) -> Arc<dyn ExternalInstance> {
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some(instance_id.into());
    host.get_or_create(&pi, SR).expect("istanza")
}

/// Elabora `blocks` blocchi da `len` campioni (eventi nel primo) e restituisce il canale
/// sinistro. Ogni blocco restituisce il risultato del precedente.
fn run(inst: &dyn ExternalInstance, events: &[MidiEvent], blocks: usize, len: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let (mut l, mut r) = (vec![0.0f32; len], vec![0.0f32; len]);
    for b in 0..blocks {
        l.fill(9.0);
        r.fill(9.0);
        let ev: &[MidiEvent] = if b == 0 { events } else { &[] };
        assert!(inst.process(ev, None, &mut l, &mut r), "process rifiutato");
        assert_eq!(l, r, "uscita stereo identica");
        out.extend_from_slice(&l);
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn zero_crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
}

fn note_on(key: u8) -> MidiEvent {
    MidiEvent::NoteOn {
        offset: 0,
        key,
        velocity: 1.0,
        note_id: 1,
    }
}

fn note_off(key: u8) -> MidiEvent {
    MidiEvent::NoteOff {
        offset: 0,
        key,
        velocity: 0.0,
        note_id: 1,
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "timeout: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn kill_process(pid: u32) {
    let status = if cfg!(windows) {
        Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status()
    } else {
        Command::new("kill").args(["-9", &pid.to_string()]).status()
    };
    assert!(status.expect("kill").success());
}

#[test]
fn scan_in_child_process() {
    let backend = SandboxBackend::new(PluginFormat::Clap, config());
    assert!(backend.is_candidate(clap_path()));
    let found = backend.scan(clap_path()).expect("scansione");
    assert_eq!(found.len(), 2);
    let synth = found.iter().find(|p| p.id == SYNTH_ID).unwrap();
    assert_eq!(synth.name, "MFC Test Synth");
    assert!(synth.is_instrument);
    assert_eq!(
        synth.kind,
        mfc_host::make_kind(PluginFormat::Clap, clap_path(), SYNTH_ID)
    );
    // Stesso risultato della scansione in-process.
    assert_eq!(found, mfc_host::clap::ClapBackend::new().scan(clap_path()).unwrap());

    // VST3 e scansione di cartelle tramite l'host in modalità sandbox.
    let h = host();
    let report = h.scan(&[
        clap_path().parent().unwrap().to_path_buf(),
        vst3_path().parent().unwrap().to_path_buf(),
    ]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.plugins.len(), 4);
    assert_eq!(h.catalog().len(), 4);

    // File non valido: errore pulito dal processo figlio.
    let bogus = temp_root().join("bogus.clap");
    std::fs::write(&bogus, b"non sono un plugin").unwrap();
    assert!(backend.scan(&bogus).is_err());
    assert!(backend.scan(Path::new("/nonexistent/x.clap")).is_err());
}

#[test]
fn synth_renders_440_with_one_block_latency() {
    let h = host();
    let inst = create(&h, &clap_info(SYNTH_ID), "synth");
    assert!(inst.is_alive());
    assert!(inst.process_id().is_some());
    assert_ne!(inst.process_id(), Some(std::process::id()));
    assert_eq!(inst.info().id, SYNTH_ID);
    // Silenzio senza note.
    assert_eq!(peak(&run(inst.as_ref(), &[], 3, 256)), 0.0);

    // La4 = 440 Hz: 11 blocchi da 480; il primo è il risultato del blocco precedente
    // (silenzio), i 10 successivi sono 0.1 s di nota.
    let len = 480;
    let out = run(inst.as_ref(), &[note_on(69)], 11, len);
    assert_eq!(peak(&out[..len]), 0.0, "un blocco di latenza");
    let note = &out[len..];
    let p = peak(note);
    assert!((p - 0.5).abs() < 0.02, "picco {p} (gain 0.5 · velocity 1)");
    let hz = zero_crossings(note) as f32 / 2.0 / 0.1;
    assert!((hz - 440.0).abs() < 15.0, "frequenza {hz}");
    // La latenza riportata comprende il blocco della sandbox.
    assert_eq!(inst.latency(), len as u32);

    // Note off: il blocco successivo è ancora la nota, poi silenzio.
    let out = run(inst.as_ref(), &[note_off(69)], 3, 256);
    assert!(peak(&out[..256]) > 0.1);
    assert_eq!(peak(&out[256..]), 0.0);

    // AllNotesOff.
    run(inst.as_ref(), &[note_on(60)], 2, 256);
    let out = run(inst.as_ref(), &[MidiEvent::AllNotesOff { offset: 0 }], 3, 256);
    assert_eq!(peak(&out[256..]), 0.0);
    assert!(h.take_errors().is_empty());
}

#[test]
fn params_through_the_sandbox() {
    let h = host();
    let inst = create(&h, &clap_info(SYNTH_ID), "params");
    let params = inst.params();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "Gain");
    assert!((params[0].default - 0.5).abs() < 1e-9);
    assert!(params[0].automatable);
    assert!((inst.param_normalized(0) - 0.5).abs() < 1e-9);
    assert_eq!(inst.param_display(0, 0.25), "25 %");

    run(inst.as_ref(), &[note_on(69)], 2, 256);
    inst.set_param_normalized(0, 0.0);
    assert_eq!(inst.param_normalized(0), 0.0);
    // Primo blocco = risultato già calcolato, poi silenzio.
    let out = run(inst.as_ref(), &[], 3, 256);
    assert_eq!(peak(&out[256..]), 0.0);
    inst.set_param_normalized(0, 1.0);
    let out = run(inst.as_ref(), &[], 5, 256);
    let p = peak(&out[512..]);
    assert!((p - 1.0).abs() < 0.02, "picco {p}");

    // Parametri del progetto applicati da `get_or_create`.
    let mut pi = PluginInstance::new(clap_info(SYNTH_ID).kind);
    pi.instance_id = Some("params2".into());
    pi.params.insert(mfc_host::param_key(0), 0.75);
    let b = h.get_or_create(&pi, SR).unwrap();
    assert!((b.param_normalized(0) - 0.75).abs() < 1e-6);
    // Il reset (anche dal thread audio) spegne le note.
    run(b.as_ref(), &[note_on(69)], 2, 128);
    b.reset();
    let out = run(b.as_ref(), &[], 3, 128);
    assert_eq!(peak(&out[128..]), 0.0);
}

#[test]
fn state_roundtrip_and_effect() {
    let h = host();
    let a = create(&h, &clap_info(SYNTH_ID), "a");
    a.set_param_normalized(0, 0.25);
    let state = a.save_state().unwrap();
    assert_eq!(state, 0.25f64.to_le_bytes().to_vec());

    let b = create(&h, &clap_info(SYNTH_ID), "b");
    assert!((b.param_normalized(0) - 0.5).abs() < 1e-9);
    b.load_state(&state).unwrap();
    assert!((b.param_normalized(0) - 0.25).abs() < 1e-9);
    let p = peak(&run(b.as_ref(), &[note_on(69)], 5, 256));
    assert!((p - 0.25).abs() < 0.02, "picco {p}");
    // Stato non valido: errore del plugin riportato attraverso il processo.
    assert!(b.load_state(&[1, 2]).is_err());
    assert!(b.is_alive());

    // Stato ripristinato da `get_or_create` (opaque_state).
    let mut pi = PluginInstance::new(clap_info(SYNTH_ID).kind);
    pi.instance_id = Some("c".into());
    pi.opaque_state = Some(state);
    let c = h.get_or_create(&pi, SR).unwrap();
    assert!((c.param_normalized(0) - 0.25).abs() < 1e-9);

    // Effetto: uscita = ingresso del blocco precedente · gain.
    let fx = create(&h, &clap_info(GAIN_ID), "fx");
    fx.set_param_normalized(0, 0.5);
    let n = 128;
    let in1: Vec<f32> = (0..n).map(|i| (i as f32 / n as f32) - 0.5).collect();
    let in2: Vec<f32> = in1.iter().map(|v| v * -2.0).collect();
    let (mut l, mut r) = (vec![9.0; n], vec![9.0; n]);
    assert!(fx.process(&[], Some((&in1, &in1)), &mut l, &mut r));
    assert!(l.iter().all(|v| *v == 0.0), "primo blocco: silenzio");
    assert!(fx.process(&[], Some((&in2, &in2)), &mut l, &mut r));
    for i in 0..n {
        assert!((l[i] - in1[i] * 0.5).abs() < 1e-6);
    }
    assert!(fx.process(&[], Some((&in1, &in1)), &mut l, &mut r));
    for i in 0..n {
        assert!((r[i] - in2[i] * 0.5).abs() < 1e-6);
    }
    // Editor: non supportato su Linux; sempre "esterno" in sandbox.
    assert!(fx.editor_is_external());
    assert!(!fx.editor_is_open());
    if cfg!(target_os = "linux") {
        assert!(!fx.has_editor());
        assert!(fx.open_editor(mfc_host::ParentWindow::X11(0)).is_err());
    }
    fx.close_editor();
    fx.idle();
    assert!(h.take_errors().is_empty());
}

#[test]
fn vst3_through_the_sandbox() {
    let h = host();
    let infos = SandboxBackend::new(PluginFormat::Vst3, config())
        .scan(vst3_path())
        .unwrap();
    let info = infos.iter().find(|p| p.name == "MFC Test Synth").unwrap();
    let inst = create(&h, info, "v");
    let len = 512;
    let out = run(inst.as_ref(), &[note_on(69)], 11, len);
    assert_eq!(peak(&out[..len]), 0.0);
    let p = peak(&out[len..]);
    assert!((p - 0.5).abs() < 0.01, "picco {p}");
    let hz = zero_crossings(&out[len..]) as f32 / 2.0 / (10.0 * len as f32 / SR);
    assert!((hz - 440.0).abs() < 15.0, "frequenza {hz}");
    inst.set_param_normalized(0, 0.8);
    let state = inst.save_state().unwrap();
    assert!(state.starts_with(b"MFV3"));
    // Nuovo sample rate tramite l'host: stessa istanza, riconfigurata nel processo.
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some("v".into());
    let same = h.get_or_create(&pi, 44_100.0).unwrap();
    assert!(Arc::ptr_eq(&same, &inst));
    run(inst.as_ref(), &[], 2, 256);
    // Chiusura dell'istanza: il processo termina senza essere segnalato come crash.
    drop(same);
    drop(inst);
    h.retain(&[]);
    h.idle();
    assert_eq!(h.pending_drops(), 0);
    assert!(h.take_errors().is_empty());
}

#[test]
fn crash_is_contained_and_restart_restores_state() {
    let h = host();
    let mut pi = PluginInstance::new(clap_info(SYNTH_ID).kind);
    pi.instance_id = Some("crash".into());
    pi.opaque_state = Some(0.25f64.to_le_bytes().to_vec());
    let inst = h.get_or_create(&pi, SR).unwrap();
    assert!((inst.param_normalized(0) - 0.25).abs() < 1e-9);
    run(inst.as_ref(), &[note_on(69)], 3, 256);
    let old_pid = inst.process_id().unwrap();

    // Il processo del plugin muore (come per un crash).
    kill_process(old_pid);
    wait_until("istanza segnata come morta", || !inst.is_alive());
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let start = Instant::now();
    for _ in 0..10 {
        assert!(!inst.process(&[], None, &mut l, &mut r));
    }
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "process non deve bloccare"
    );
    assert!(inst.save_state().is_err());
    assert!(inst.params().is_empty());
    assert_eq!(inst.param_normalized(0), 0.0);
    assert!(inst.load_state(&[0; 8]).is_err());
    inst.set_param_normalized(0, 0.9);
    inst.reset();
    inst.idle();
    inst.close_editor();
    assert!(inst.crash_reason().is_some());

    // L'host riporta l'errore una sola volta.
    assert_eq!(h.is_alive("crash"), Some(false));
    assert_eq!(h.crashed_ids(), vec!["crash".to_string()]);
    let errors = h.take_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("Plugin MFC Test Synth si è chiuso in modo anomalo"),
        "{errors:?}"
    );
    h.idle();
    assert!(h.take_errors().is_empty());
    // Il motore (adattatore) resta funzionante: silenzio, nessun panic.
    let mut g = mfc_host::wrapper::ExternalGenerator::new(inst.clone());
    mfc_plugins::Generator::note_on(&mut g, mfc_plugins::NoteOn::simple(1, 69, 1.0));
    mfc_plugins::Generator::process(&mut g, &mut l, &mut r);

    // Riavvio: nuovo processo, stato ripristinato (quello caricato alla creazione).
    h.restart("crash").expect("riavvio");
    assert!(inst.is_alive());
    assert_eq!(h.is_alive("crash"), Some(true));
    assert!(h.crashed_ids().is_empty());
    let new_pid = inst.process_id().unwrap();
    assert_ne!(new_pid, old_pid);
    // Il cambio di parametro fatto a processo morto è in coda: si applica dopo lo stato.
    assert!((inst.param_normalized(0) - 0.9).abs() < 1e-9);
    inst.set_param_normalized(0, 0.25);
    let p = peak(&run(inst.as_ref(), &[note_on(69)], 5, 256));
    assert!((p - 0.25).abs() < 0.02, "picco {p}");

    // Lo stato salvato più di recente viene ripristinato dal riavvio successivo.
    inst.set_param_normalized(0, 0.75);
    inst.save_state().unwrap();
    kill_process(new_pid);
    wait_until("seconda morte", || !inst.is_alive());
    assert_eq!(h.take_errors().len(), 1);
    h.restart("crash").expect("secondo riavvio");
    assert!((inst.param_normalized(0) - 0.75).abs() < 1e-9);
    // Riavvio di un'istanza viva: consentito (processo nuovo, stesso stato).
    h.restart("crash").expect("riavvio a caldo");
    assert!((inst.param_normalized(0) - 0.75).abs() < 1e-9);
    assert!(h.take_errors().is_empty());
    assert!(h.restart("inesistente").is_err());
}

#[test]
fn engine_hook_and_mode_switch() {
    let h = Host::new();
    assert!(!h.sandbox_enabled());
    // Instanza in-process prima del cambio di modalità.
    let info = clap_info(SYNTH_ID);
    let a = create(&h, &info, "inproc");
    assert!(!a.editor_is_external());
    assert!(a.process_id().is_none());
    assert!(a.restart().is_err());
    h.set_sandbox_config(config());
    h.set_sandbox(true).expect("helper configurato");
    assert!(h.sandbox_enabled());
    let b = create(&h, &info, "sandboxed");
    assert!(b.editor_is_external());
    assert!(b.process_id().is_some());
    // L'istanza esistente resta in-process.
    let a2 = create(&h, &info, "inproc");
    assert!(Arc::ptr_eq(&a, &a2));
    h.set_sandbox(false).unwrap();
    let c = create(&h, &info, "inproc2");
    assert!(c.process_id().is_none());

    // Generatore del motore su un'istanza sandbox: con il ritardo di un sotto-blocco
    // la nota arriva comunque.
    h.set_sandbox(true).unwrap();
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some("hook".into());
    pi.params.insert(mfc_host::param_key(0), 1.0);
    let mut g = mfc_plugins::ExternalHost::create_generator(&h, &pi, SR).expect("generatore");
    g.note_on(mfc_plugins::NoteOn::simple(1, 69, 1.0));
    let mut out = Vec::new();
    for _ in 0..75 {
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        g.process(&mut l, &mut r);
        out.extend_from_slice(&l);
    }
    assert_eq!(peak(&out[..64]), 0.0);
    let hz = zero_crossings(&out[64..]) as f32 / 2.0 / (74.0 * 64.0 / SR);
    assert!((hz - 440.0).abs() < 15.0, "frequenza {hz}");
    assert!(peak(&out) > 0.9);
    assert_eq!(h.take_errors(), Vec::<String>::new());
}

#[test]
fn bad_kinds_and_missing_helper() {
    let h = host();
    let mut pi = PluginInstance::new("clap:/nonexistent/x.clap::id");
    pi.instance_id = Some("x".into());
    match h.get_or_create(&pi, SR) {
        Err(mfc_host::HostError::Load { .. }) => {}
        Err(e) => panic!("errore inatteso: {e}"),
        Ok(_) => panic!("istanza inattesa"),
    }
    let mut pi = PluginInstance::new(clap_info(SYNTH_ID).kind.replace(SYNTH_ID, "com.nope"));
    pi.instance_id = Some("y".into());
    assert!(h.get_or_create(&pi, SR).is_err());

    let h = Host::new();
    h.set_sandbox_config(SandboxConfig::new("/nonexistent/mfc-plugin-host"));
    // Helper configurato inesistente: si cerca accanto all'eseguibile (target/<profilo>).
    h.set_sandbox(true).expect("helper accanto all'eseguibile di test");
    assert_eq!(h.sandbox_config().unwrap().helper.file_name(), helper().file_name());
}

/// Diagnostica (non eseguita di default): blocchi da 64 campioni con l'attesa automatica
/// (metà blocco), come nel motore. Stampa quanti blocchi arrivano in tempo e il
/// round-trip medio. `cargo test -p mfc-host --test sandbox -- --ignored --nocapture`
#[test]
#[ignore]
fn default_wait_report() {
    let mut cfg = SandboxConfig::new(helper());
    cfg.state_snapshot_interval = None;
    let backend = SandboxBackend::new(PluginFormat::Clap, cfg);
    let kind = mfc_host::parse_kind(&clap_info(SYNTH_ID).kind).unwrap();
    let inst = backend.instantiate(&kind, SR, 512).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let blocks = 20_000;
    let mut ok = 0;
    let start = Instant::now();
    for i in 0..blocks {
        let ev = [note_on(69)];
        let events: &[MidiEvent] = if i == 0 { &ev } else { &[] };
        if inst.process(events, None, &mut l, &mut r) {
            ok += 1;
        }
    }
    let per_block = start.elapsed() / blocks;
    println!(
        "blocchi in tempo: {ok}/{blocks}; tempo medio per blocco da 64: {per_block:?} (budget {:?})",
        Duration::from_secs_f64(64.0 / SR as f64)
    );
}
