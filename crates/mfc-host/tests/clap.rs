//! Test d'integrazione del backend CLAP con il plugin di prova `mfc-test-clap`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use mfc_core::PluginInstance;
use mfc_host::clap::ClapBackend;
use mfc_host::{ExternalInstance, FormatBackend, Host, MidiEvent, PluginFormat, PluginInfo};

const SYNTH_ID: &str = "com.mixfonocraft.test.synth";
const GAIN_ID: &str = "com.mixfonocraft.test.gain";
const SR: f32 = 48_000.0;

/// Compila il plugin di prova (una volta) e lo copia in una cartella temporanea come `MFC Test.clap`.
fn plugin_path() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        // target/<profilo>/deps/<test> → target/<profilo>
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
        let release = profile_dir.file_name().is_some_and(|n| n == "release");
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["build", "-p", "mfc-test-clap"]);
        if release {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("avvio cargo");
        assert!(status.success(), "build di mfc-test-clap fallita");

        let lib_name = format!(
            "{}mfc_test_clap{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        );
        let lib = profile_dir.join(&lib_name);
        assert!(lib.exists(), "libreria non trovata: {}", lib.display());

        let dir = std::env::temp_dir().join(format!("mfc_clap_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("MFC Test.clap");
        install(&lib, &dest);
        dest
    })
}

#[cfg(not(target_os = "macos"))]
fn install(lib: &Path, dest: &Path) {
    std::fs::copy(lib, dest).unwrap();
}

#[cfg(target_os = "macos")]
fn install(lib: &Path, dest: &Path) {
    let macos = dest.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::copy(lib, macos.join("MFC Test")).unwrap();
    std::fs::write(
        dest.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>MFC Test</string>
<key>CFBundleIdentifier</key><string>com.mixfonocraft.test.clap</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>"#,
    )
    .unwrap();
}

fn infos() -> Vec<PluginInfo> {
    ClapBackend::new().scan(plugin_path()).expect("scan")
}

fn info(id: &str) -> PluginInfo {
    infos().into_iter().find(|p| p.id == id).expect("plugin presente")
}

fn host() -> Host {
    Host::with_backends(vec![Box::new(ClapBackend::new())])
}

fn create(host: &Host, id: &str, instance_id: &str) -> Arc<dyn ExternalInstance> {
    let mut pi = PluginInstance::new(info(id).kind);
    pi.instance_id = Some(instance_id.into());
    host.get_or_create(&pi, SR).expect("istanza")
}

/// Elabora `blocks` blocchi da `len` campioni; eventi solo nel primo. Restituisce il canale sinistro.
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

#[test]
fn candidates() {
    let b = ClapBackend::new();
    assert_eq!(b.format(), PluginFormat::Clap);
    assert!(b.is_candidate(plugin_path()));
    assert!(!b.is_candidate(Path::new("/tmp/x.vst3")));
    assert!(!b.is_candidate(Path::new("/tmp/clap")));
}

#[test]
fn scan_finds_both_plugins() {
    let found = infos();
    assert_eq!(found.len(), 2);
    let synth = found.iter().find(|p| p.id == SYNTH_ID).unwrap();
    assert_eq!(synth.name, "MFC Test Synth");
    assert_eq!(synth.vendor, "Mixfonocraft");
    assert_eq!(synth.version, "1.0.0");
    assert!(synth.is_instrument && !synth.is_effect);
    assert!(synth.category.contains("instrument"));
    assert!(synth.category.contains('|'));
    assert_eq!(synth.format, PluginFormat::Clap);
    assert_eq!(
        synth.kind,
        mfc_host::make_kind(PluginFormat::Clap, plugin_path(), SYNTH_ID)
    );
    let parsed = mfc_host::parse_kind(&synth.kind).unwrap();
    assert_eq!(parsed.path, plugin_path());
    assert_eq!(parsed.id, SYNTH_ID);

    let gain = found.iter().find(|p| p.id == GAIN_ID).unwrap();
    assert_eq!(gain.name, "MFC Test Gain");
    assert!(gain.is_effect && !gain.is_instrument);

    // Scansione tramite l'host (cartella).
    let h = host();
    let report = h.scan(&[plugin_path().parent().unwrap().to_path_buf()]);
    assert_eq!(report.plugins.len(), 2, "errori: {:?}", report.errors);
    assert!(ClapBackend::new().scan(Path::new("/nonexistent/x.clap")).is_err());
}

#[test]
fn synth_plays_note_and_stops() {
    let h = host();
    let inst = create(&h, SYNTH_ID, "synth1");
    assert_eq!(inst.info().id, SYNTH_ID);
    // Silenzio senza note.
    assert_eq!(peak(&run(inst.as_ref(), &[], 2, 256)), 0.0);

    // La4 = 440 Hz: 0.1 s → ~88 attraversamenti dello zero.
    let out = run(inst.as_ref(), &[note_on(69)], 10, 480);
    let p = peak(&out);
    assert!((p - 0.5).abs() < 0.02, "picco {p} (gain 0.5 · velocity 1)");
    let hz = zero_crossings(&out) as f32 / 2.0 / 0.1;
    assert!((hz - 440.0).abs() < 15.0, "frequenza {hz}");

    // Note off → silenzio.
    let out = run(inst.as_ref(), &[note_off(69)], 2, 256);
    assert_eq!(peak(&out), 0.0);

    // AllNotesOff spegne le note tenute.
    run(inst.as_ref(), &[note_on(60)], 1, 256);
    assert!(peak(&run(inst.as_ref(), &[], 1, 256)) > 0.1);
    let out = run(inst.as_ref(), &[MidiEvent::AllNotesOff { offset: 0 }], 2, 256);
    assert_eq!(peak(&out), 0.0);
}

#[test]
fn params_and_display() {
    let h = host();
    let inst = create(&h, SYNTH_ID, "synth2");
    let params = inst.params();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].id, 0);
    assert_eq!(params[0].name, "Gain");
    assert!((params[0].default - 0.5).abs() < 1e-9);
    assert_eq!(params[0].steps, 0);
    assert!(params[0].automatable);
    assert!((inst.param_normalized(0) - 0.5).abs() < 1e-9);
    assert_eq!(inst.param_display(0, 0.25), "25 %");

    run(inst.as_ref(), &[note_on(69)], 1, 256);
    inst.set_param_normalized(0, 0.0);
    assert_eq!(inst.param_normalized(0), 0.0);
    // Il blocco successivo riceve il cambio: silenzio.
    assert_eq!(peak(&run(inst.as_ref(), &[], 2, 256)), 0.0);
    inst.set_param_normalized(0, 1.0);
    let p = peak(&run(inst.as_ref(), &[], 4, 256));
    assert!((p - 1.0).abs() < 0.02, "picco {p}");
}

#[test]
fn param_flush_without_process_and_state_roundtrip() {
    let h = host();
    let a = create(&h, SYNTH_ID, "a");
    // Nessun process: il cambio arriva al plugin tramite flush.
    a.set_param_normalized(0, 0.25);
    let state = a.save_state().unwrap();
    assert_eq!(state, 0.25f64.to_le_bytes().to_vec());

    let b = create(&h, SYNTH_ID, "b");
    assert!((b.param_normalized(0) - 0.5).abs() < 1e-9);
    b.load_state(&state).unwrap();
    assert!((b.param_normalized(0) - 0.25).abs() < 1e-9);
    let p = peak(&run(b.as_ref(), &[note_on(69)], 4, 256));
    assert!((p - 0.25).abs() < 0.02, "picco {p}");
    assert!(b.load_state(&[1, 2]).is_err());

    // Stato e parametri ripristinati da `get_or_create`.
    let mut pi = PluginInstance::new(info(SYNTH_ID).kind);
    pi.instance_id = Some("c".into());
    pi.opaque_state = Some(state);
    let c = h.get_or_create(&pi, SR).unwrap();
    assert!((c.param_normalized(0) - 0.25).abs() < 1e-9);
    assert!(h.take_errors().is_empty());
}

#[test]
fn effect_processes_input() {
    let h = host();
    let fx = create(&h, GAIN_ID, "fx");
    assert!((fx.param_normalized(0) - 1.0).abs() < 1e-9);
    let n = 128;
    let in_l: Vec<f32> = (0..n).map(|i| (i as f32 / n as f32) - 0.5).collect();
    let in_r: Vec<f32> = in_l.iter().map(|v| -v).collect();
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    assert!(fx.process(&[], Some((&in_l, &in_r)), &mut l, &mut r));
    assert_eq!(l, in_l);
    assert_eq!(r, in_r);

    fx.set_param_normalized(0, 0.5);
    assert!(fx.process(&[], Some((&in_l, &in_r)), &mut l, &mut r));
    for i in 0..n {
        assert!((l[i] - in_l[i] * 0.5).abs() < 1e-6);
        assert!((r[i] - in_r[i] * 0.5).abs() < 1e-6);
    }
    assert!(!fx.has_editor());
    fx.close_editor();
    fx.reset();
    fx.idle();
}

#[test]
fn prepare_new_sample_rate_and_drop() {
    let h = host();
    let inst = create(&h, SYNTH_ID, "sr");
    run(inst.as_ref(), &[note_on(69)], 2, 256);
    inst.prepare(96_000.0, 1024).unwrap();
    inst.set_param_normalized(0, 1.0);
    // A 96 kHz, 0.1 s = 9600 campioni.
    let out = run(inst.as_ref(), &[note_on(69)], 10, 960);
    let hz = zero_crossings(&out) as f32 / 2.0 / 0.1;
    assert!((hz - 440.0).abs() < 15.0, "frequenza {hz}");
    // Blocco più grande del massimo preparato: elaborato a pezzi o rifiutato, senza crash.
    let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
    let _ = inst.process(&[], None, &mut l, &mut r);
    // Cambio di sample rate tramite l'host.
    let mut pi = PluginInstance::new(info(SYNTH_ID).kind);
    pi.instance_id = Some("sr".into());
    let same = h.get_or_create(&pi, 44_100.0).unwrap();
    assert!(Arc::ptr_eq(&same, &inst));
    run(same.as_ref(), &[], 2, 256);
    drop(same);
    h.retain(&[]);
    drop(inst);
    drop(h);
}

#[test]
fn processing_from_another_thread() {
    let h = host();
    let inst = create(&h, SYNTH_ID, "thr");
    let audio = inst.clone();
    let t = std::thread::spawn(move || {
        // Il thread audio non blocca mai: un blocco può essere rifiutato se la UI è occupata.
        let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
        let mut sent = false;
        let mut p = 0.0f32;
        for _ in 0..200 {
            let ev = [note_on(69)];
            let events: &[MidiEvent] = if sent { &[] } else { &ev };
            if audio.process(events, None, &mut l, &mut r) {
                sent = true;
                p = p.max(peak(&l));
            }
        }
        p
    });
    // Chiamate UI concorrenti.
    for _ in 0..20 {
        let _ = inst.params();
        inst.idle();
        inst.set_param_normalized(0, 0.5);
    }
    let p = t.join().unwrap();
    assert!(p > 0.1);
}

#[test]
fn param_saved_while_engine_paused() {
    let h = host();
    let inst = create(&h, SYNTH_ID, "paused");
    run(inst.as_ref(), &[note_on(69)], 2, 256);
    // Il motore non chiama più `process`: il salvataggio deve comunque includere il cambio.
    inst.set_param_normalized(0, 0.75);
    assert_eq!(inst.save_state().unwrap(), 0.75f64.to_le_bytes().to_vec());
    // L'elaborazione riparte normalmente.
    let p = peak(&run(inst.as_ref(), &[], 4, 256));
    assert!((p - 0.75).abs() < 0.02, "picco {p}");
}
