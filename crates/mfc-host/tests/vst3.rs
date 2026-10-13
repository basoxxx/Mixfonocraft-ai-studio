//! Test d'integrazione del backend VST3 con il plugin di prova `mfc-test-vst3`.
//!
//! Il plugin viene compilato una volta per binario di test e impacchettato in un bundle
//! `.vst3` nella cartella temporanea.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use mfc_core::PluginInstance;
use mfc_host::vst3::Vst3Backend;
use mfc_host::{ExternalInstance, FormatBackend, Host, MidiEvent, PluginInfo};

const SR: f32 = 48_000.0;

/// Compila il plugin di prova e costruisce il bundle (una sola volta).
fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(build_bundle)
}

fn build_bundle() -> PathBuf {
    // target/<profilo>/deps/vst3-xxxx → target/<profilo>
    let exe = std::env::current_exe().expect("current_exe");
    let profile_dir = exe.parent().and_then(Path::parent).expect("cartella del profilo");
    let target_dir = profile_dir.parent().expect("target dir");
    let release = profile_dir.file_name().is_some_and(|n| n == "release");
    let mut cmd = std::process::Command::new(env!("CARGO"));
    cmd.args(["build", "-p", "mfc-test-vst3", "--target-dir"])
        .arg(target_dir);
    if release {
        cmd.arg("--release");
    }
    let status = cmd.status().expect("avvio di cargo build");
    assert!(status.success(), "compilazione di mfc-test-vst3 fallita");
    let out_dir = target_dir.join(if release { "release" } else { "debug" });
    let lib = if cfg!(target_os = "windows") {
        out_dir.join("mfc_test_vst3.dll")
    } else if cfg!(target_os = "macos") {
        out_dir.join("libmfc_test_vst3.dylib")
    } else {
        out_dir.join("libmfc_test_vst3.so")
    };
    assert!(lib.exists(), "libreria non trovata: {}", lib.display());

    let root = std::env::temp_dir().join(format!("mfc-vst3-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bundle = root.join("MFC Test.vst3");
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
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>MFC Test</string>
<key>CFBundleIdentifier</key><string>com.mixfonocraft.test.vst3</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>"#,
    )
    .unwrap();
    bundle
}

fn scan() -> Vec<PluginInfo> {
    Vst3Backend::new().scan(bundle()).expect("scansione")
}

fn find(name: &str) -> PluginInfo {
    scan()
        .into_iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("classe {name} non trovata"))
}

fn host() -> Host {
    Host::with_backends(vec![Box::new(Vst3Backend::new())])
}

fn instance(host: &Host, info: &PluginInfo, id: &str) -> Arc<dyn ExternalInstance> {
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some(id.into());
    host.get_or_create(&pi, SR).expect("creazione istanza")
}

/// Elabora `blocks` blocchi da `len` sample; gli eventi vanno nel primo.
fn render(inst: &dyn ExternalInstance, events: &[MidiEvent], blocks: usize, len: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(blocks * len);
    let (mut l, mut r) = (vec![0.0f32; len], vec![0.0f32; len]);
    for b in 0..blocks {
        l.fill(9.0);
        r.fill(9.0);
        let ev = if b == 0 { events } else { &[] };
        assert!(inst.process(ev, None, &mut l, &mut r), "blocco non elaborato");
        assert_eq!(l, r, "uscita stereo diversa");
        out.extend_from_slice(&l);
    }
    out
}

/// Frequenza stimata dai passaggi per lo zero in salita.
fn frequency(x: &[f32], sr: f32) -> f32 {
    let ups = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    ups as f32 * sr / x.len() as f32
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
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
fn scan_finds_both_classes() {
    let found = scan();
    assert_eq!(found.len(), 2, "{found:?}");
    let synth = found.iter().find(|p| p.name == "MFC Test Synth").expect("synth");
    let fx = found.iter().find(|p| p.name == "MFC Test Gain").expect("gain");
    assert!(synth.is_instrument && !synth.is_effect);
    assert!(fx.is_effect && !fx.is_instrument);
    assert_eq!(synth.category, "Instrument|Synth");
    assert_eq!(synth.vendor, "Mixfonocraft Test");
    assert_eq!(synth.version, "1.2.3");
    for p in &found {
        assert_eq!(p.format, mfc_host::PluginFormat::Vst3);
        assert_eq!(p.id.len(), 32);
        assert!(p.id.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
        assert_eq!(
            p.kind,
            mfc_host::make_kind(mfc_host::PluginFormat::Vst3, bundle(), &p.id)
        );
        assert_eq!(p.path, bundle());
    }
    assert_ne!(synth.id, fx.id);
    // Il bundle è riconosciuto dal backend e dall'host.
    assert!(Vst3Backend::new().is_candidate(bundle()));
    assert_eq!(host().scan_file(bundle()).unwrap().len(), 2);
}

#[test]
fn synth_plays_notes_and_follows_params() {
    let host = host();
    let info = find("MFC Test Synth");
    let inst = instance(&host, &info, "synth1");
    assert_eq!(inst.info().name, "MFC Test Synth");

    // Silenzio senza note.
    let quiet = render(&*inst, &[], 4, 256);
    assert_eq!(peak(&quiet), 0.0);

    // La 440 Hz con gain 0.5.
    let audio = render(&*inst, &[note_on(69)], 94, 512);
    let f = frequency(&audio, SR);
    assert!((f - 440.0).abs() < 3.0, "frequenza {f}");
    let p = peak(&audio);
    assert!((p - 0.5).abs() < 0.01, "picco {p}");

    // Parametri.
    let params = inst.params();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "Gain");
    assert_eq!(params[0].id, 0);
    assert!((params[0].default - 0.5).abs() < 1e-9);
    assert!(params[0].automatable && !params[0].hidden);
    assert!(!inst.param_display(0, 0.25).is_empty());
    assert!((inst.param_normalized(0) - 0.5).abs() < 1e-9);

    // Gain a zero → silenzio dal blocco successivo.
    inst.set_param_normalized(0, 0.0);
    assert_eq!(inst.param_normalized(0), 0.0);
    assert_eq!(peak(&render(&*inst, &[], 2, 128)), 0.0);
    inst.set_param_normalized(0, 1.0);
    assert!(peak(&render(&*inst, &[], 4, 128)) > 0.9);

    // Note off → silenzio.
    assert_eq!(peak(&render(&*inst, &[note_off(69)], 2, 128)), 0.0);

    // All notes off.
    render(&*inst, &[note_on(60)], 1, 128);
    assert!(peak(&render(&*inst, &[], 1, 128)) > 0.1);
    assert_eq!(
        peak(&render(&*inst, &[MidiEvent::AllNotesOff { offset: 0 }], 1, 128)),
        0.0
    );

    // Offset dell'evento nel blocco: prima dell'offset silenzio.
    let ev = MidiEvent::NoteOn {
        offset: 100,
        key: 69,
        velocity: 1.0,
        note_id: 2,
    };
    let block = render(&*inst, &[ev], 1, 256);
    assert!(peak(&block) > 0.0);
    inst.reset();
    render(&*inst, &[], 1, 64);
    assert_eq!(peak(&render(&*inst, &[], 1, 64)), 0.0, "reset non ha spento le note");

    assert!(!inst.has_editor());
    assert!(inst.open_editor(mfc_host::ParentWindow::X11(0)).is_err());
    inst.close_editor();
    inst.idle();
    assert_eq!(inst.latency(), 0);
}

#[test]
fn state_roundtrip() {
    let host = host();
    let info = find("MFC Test Synth");
    let a = instance(&host, &info, "a");
    a.set_param_normalized(0, 0.8);
    let state = a.save_state().expect("save_state");
    assert!(state.starts_with(b"MFV3"));

    let b = instance(&host, &info, "b");
    assert!((b.param_normalized(0) - 0.5).abs() < 1e-9);
    b.load_state(&state).expect("load_state");
    assert!((b.param_normalized(0) - 0.8).abs() < 1e-9);
    // Lo stato arriva anche al processore.
    let audio = render(&*b, &[note_on(69)], 10, 480);
    assert!((peak(&audio) - 0.8).abs() < 0.01, "picco {}", peak(&audio));

    // Ripristino tramite il progetto (opaque_state).
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some("c".into());
    pi.opaque_state = Some(state);
    let c = host.get_or_create(&pi, SR).unwrap();
    assert!((c.param_normalized(0) - 0.8).abs() < 1e-9);
    assert!(host.take_errors().is_empty());

    // Stato non valido → errore, nessun crash.
    assert!(b.load_state(b"garbage").is_err());
}

#[test]
fn effect_applies_gain_in_place() {
    let host = host();
    let info = find("MFC Test Gain");
    let fx = instance(&host, &info, "fx");
    let n = 256;
    let in_l: Vec<f32> = (0..n).map(|i| (i as f32 / n as f32) - 0.5).collect();
    let in_r: Vec<f32> = in_l.iter().map(|x| -x).collect();
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
    // Tramite l'adattatore del motore (in-place).
    let mut effect = host_effect(&host, &info);
    let (mut l, mut r) = (vec![1.0f32; 64], vec![-1.0f32; 64]);
    effect.process(&mut l, &mut r);
    assert!((l[0] - 0.5).abs() < 1e-6 && (r[63] + 0.5).abs() < 1e-6);
}

fn host_effect(host: &Host, info: &PluginInfo) -> Box<dyn mfc_plugins::Effect> {
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some("fx".into());
    mfc_plugins::ExternalHost::create_effect(host, &pi, SR).expect("effetto")
}

#[test]
fn engine_hook_creates_generators() {
    let info = find("MFC Test Synth");
    let host = Arc::new(host());
    mfc_plugins::set_external_host(Some(host.clone()));
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some("hook".into());
    pi.params.insert(mfc_host::param_key(0), 1.0);
    let mut g = mfc_plugins::create_generator(&pi, SR);
    g.note_on(mfc_plugins::NoteOn::simple(1, 69, 1.0));
    let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
    g.process(&mut l, &mut r);
    let f = frequency(&l, SR);
    assert!((f - 440.0).abs() < 15.0, "frequenza {f}");
    assert!(peak(&l) > 0.9, "picco {}", peak(&l));
    g.note_off(1);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    g.process(&mut l, &mut r);
    assert_eq!(peak(&l), 0.0);
    // Stessa istanza riusata.
    assert_eq!(host.live_ids(), vec!["hook".to_string()]);
    mfc_plugins::set_external_host(None);
    drop(g);
    host.retain(&[]);
    assert!(host.live_ids().is_empty());
}

#[test]
fn prepare_and_teardown() {
    let info = find("MFC Test Synth");
    let host = host();
    let inst = instance(&host, &info, "p");
    // Nuovo sample rate tramite l'host (stessa istanza, riconfigurata).
    let mut pi = PluginInstance::new(info.kind.clone());
    pi.instance_id = Some("p".into());
    let again = host.get_or_create(&pi, 44_100.0).unwrap();
    assert!(Arc::ptr_eq(&inst, &again));
    let audio = render(&*inst, &[note_on(69)], 86, 512);
    let f = frequency(&audio, 44_100.0);
    assert!((f - 440.0).abs() < 3.0, "frequenza {f}");
    // prepare diretto con blocco piccolo: i blocchi più lunghi vengono spezzati.
    inst.prepare(SR, 64).unwrap();
    let audio = render(&*inst, &[note_on(81)], 20, 2400);
    let f = frequency(&audio, SR);
    assert!((f - 880.0).abs() < 4.0, "frequenza {f}");
    drop(again);
    drop(inst);
    host.retain(&[]);

    // Creazione/distruzione ripetuta, anche in parallelo.
    let threads: Vec<_> = (0..4)
        .map(|t| {
            let info = info.clone();
            std::thread::spawn(move || {
                let backend = Vst3Backend::new();
                let parsed = mfc_host::parse_kind(&info.kind).unwrap();
                for _ in 0..5 {
                    let i = backend.instantiate(&parsed, SR, 512).unwrap();
                    let (mut l, mut r) = (vec![0.0; 128], vec![0.0; 128]);
                    assert!(i.process(&[note_on(60 + t)], None, &mut l, &mut r));
                    i.save_state().unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }

    // Kind con class id inesistente → errore pulito.
    let mut bad = mfc_host::parse_kind(&info.kind).unwrap();
    bad.id = "00000000000000000000000000000000".into();
    assert!(Vst3Backend::new().instantiate(&bad, SR, 512).is_err());
    // Bundle inesistente → errore pulito.
    assert!(Vst3Backend::new().scan(Path::new("/nonexistent/X.vst3")).is_err());
}

/// Nota 0: il plugin di prova la tratta come un `performEdit` del gain dalla sua GUI.
fn gui_edit(velocity: f32) -> MidiEvent {
    MidiEvent::NoteOn {
        offset: 0,
        key: 0,
        velocity,
        note_id: 9,
    }
}

#[test]
fn gui_param_edits_are_reported_once() {
    let host = host();
    let info = find("MFC Test Synth");
    let inst = instance(&host, &info, "synth_gui_edit");
    assert!(inst.take_param_changes().is_empty());
    // I cambi dell'host non sono modifiche della GUI.
    inst.set_param_normalized(0, 0.8);
    render(&*inst, &[], 2, 128);
    inst.idle();
    assert!(inst.take_param_changes().is_empty());

    render(&*inst, &[gui_edit(0.25)], 1, 128);
    let changes = inst.take_param_changes();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].0, 0);
    assert!((changes[0].1 - 0.25).abs() < 1e-6, "{changes:?}");
    assert!(inst.take_param_changes().is_empty());

    // Dall'host, per instance_id, solo l'ultimo valore.
    render(&*inst, &[gui_edit(0.5)], 1, 128);
    render(&*inst, &[gui_edit(0.75)], 1, 128);
    let all = host.take_param_changes();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].0, "synth_gui_edit");
    assert!((all[0].2 - 0.75).abs() < 1e-6);
}
