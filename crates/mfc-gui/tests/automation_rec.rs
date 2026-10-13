//! Registrazione delle automazioni dalla GUI: manopole durante la riproduzione e parametri
//! cambiati nell'interfaccia originale di un plugin CLAP (`mfc-test-clap`).

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

use mfc_core::{AutomationTarget, ChannelId, ChannelKind, ClipSource, Command, Note, Outcome, Project};
use mfc_gui::{views, Studio};

/// Attende (al massimo `secs` secondi, chiamando `tick` a ogni giro) che `cond` sia vera.
fn wait_until(studio: &mut Studio, secs: f64, mut cond: impl FnMut(&mut Studio) -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        studio.tick_plugins();
        if cond(studio) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Progetto con un canale nativo e una clip di pattern lunga in Playlist (modalità song).
fn project() -> (Project, ChannelId) {
    let mut p = Project::new("auto");
    let Outcome::Channel(c) = Command::AddChannel {
        name: "Synth".into(),
        kind: ChannelKind::Generator,
        plugin: mfc_plugins::default_instance("mfc.3xosc").unwrap(),
        mixer_insert: Some(1),
    }
    .apply(&mut p)
    .unwrap() else {
        panic!()
    };
    let bar = p.time.ticks_per_bar();
    Command::PlaceClip {
        track: 0,
        position: 0,
        length: Some(bar * 16),
        source: ClipSource::Pattern(p.patterns[0].id),
    }
    .apply(&mut p)
    .unwrap();
    (p, c)
}

fn set_volume(channel: ChannelId, v: f32) -> Command {
    Command::SetChannel {
        channel,
        name: None,
        volume: Some(v),
        pan: None,
        pitch_cents: None,
        muted: None,
        mixer_insert: None,
        root_note: None,
        color: None,
    }
}

fn clip_for<'a>(p: &'a Project, target: &AutomationTarget) -> Option<&'a mfc_core::AutomationClip> {
    p.automations.iter().find(|a| &a.target == target)
}

#[test]
fn knob_moves_while_recording_become_automation_clips() {
    let (p, channel) = project();
    let mut studio = Studio::new(p);
    studio.set_song_mode(true);

    // Senza registrazione nessuna automazione.
    studio.exec(set_volume(channel, 0.7));
    assert!(studio.auto_rec.recorder.is_empty());

    studio.toggle_record();
    assert!(studio.recording);
    assert!(
        wait_until(&mut studio, 3.0, |s| s.engine.shared.is_playing()
            && s.engine.shared.position() > 1.0),
        "il transport deve partire"
    );
    let undo_before = studio.history.undo_labels().count();
    studio.exec(set_volume(channel, 0.3));
    std::thread::sleep(Duration::from_millis(80));
    studio.exec(set_volume(channel, 0.9));
    studio.exec(Command::SetInsert {
        insert: 1,
        name: None,
        volume: None,
        pan: Some(0.5),
        stereo_separation: None,
        muted: None,
        solo: None,
        phase_invert: None,
        swap_lr: None,
    });
    let param = mfc_plugins::descriptor("mfc.3xosc").unwrap().params[0];
    studio.exec(Command::SetChannelParam {
        channel,
        param: param.name.into(),
        value: param.max,
    });
    // Comandi non automatizzabili: ignorati.
    studio.exec(Command::SetTempo { bpm: 150.0 });
    assert!(!studio.auto_rec.recorder.is_empty());
    assert!(studio.project.automations.is_empty(), "niente clip prima dello stop");

    // Stop: un solo Batch con tutte le automazioni.
    studio.stop();
    assert!(studio.auto_rec.recorder.is_empty());
    let p = &studio.project;
    assert_eq!(p.automations.len(), 3, "{:?}", p.automations);

    let vol = clip_for(p, &AutomationTarget::ChannelVolume { channel }).expect("volume");
    assert_eq!((vol.min, vol.max), (0.0, 1.25));
    assert!(vol.points.len() >= 2, "{:?}", vol.points);
    let first = vol.points.first().unwrap();
    let last = vol.points.last().unwrap();
    assert!((first.value * 1.25 - 0.3).abs() < 1e-4, "{:?}", vol.points);
    assert!((last.value * 1.25 - 0.9).abs() < 1e-4, "{:?}", vol.points);
    assert!(last.position > first.position);

    let pan = clip_for(p, &AutomationTarget::MixerPan { insert: 1 }).expect("pan mixer");
    assert_eq!(pan.points.len(), 1);
    assert!((pan.points[0].value - 0.75).abs() < 1e-6);

    let prm = clip_for(
        p,
        &AutomationTarget::ChannelParam {
            channel,
            param: param.name.into(),
        },
    )
    .expect("parametro");
    assert_eq!((prm.min, prm.max), (param.min, param.max));
    assert_eq!(prm.points[0].value, 1.0);

    // Ogni clip è in Playlist, su tracce libere (la 0 ha il pattern).
    for a in &p.automations {
        let c = p
            .playlist
            .clips
            .iter()
            .find(|c| c.source == ClipSource::Automation(a.id))
            .expect("clip in playlist");
        assert_ne!(c.track, 0);
        assert!(c.length >= 1);
    }

    // Un solo passo di undo per la registrazione.
    let labels: Vec<String> = studio.history.undo_labels().map(String::from).collect();
    assert_eq!(labels.len(), undo_before + 6, "{labels:?}");
    assert_eq!(labels[0], "batch");
    studio.undo();
    assert!(studio.project.automations.is_empty());
    assert_eq!(studio.project.playlist.clips.len(), 1);
}

/// Compila il plugin di prova e lo installa in una cartella temporanea come `MFC Test.clap`.
fn build_plugin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let mut cmd = Proc::new(env!("CARGO"));
    cmd.args(["build", "-p", "mfc-test-clap"]);
    if profile_dir.file_name().is_some_and(|n| n == "release") {
        cmd.arg("--release");
    }
    assert!(
        cmd.status().expect("avvio cargo").success(),
        "build di mfc-test-clap fallita"
    );
    let lib = profile_dir.join(format!(
        "{}mfc_test_clap{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    let dir = std::env::temp_dir().join(format!("mfc_gui_autorec_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("MFC Test.clap");
    install(&lib, &dest);
    dest
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
<key>CFBundleIdentifier</key><string>com.mixfonocraft.test.clap.autorec</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>"#,
    )
    .unwrap();
}

#[test]
fn plugin_gui_param_changes_are_mirrored_and_recorded() {
    let path = build_plugin();
    let (p, _) = project();
    let mut studio = Studio::new(p);
    let found = studio.host.scan_file(&path).expect("scan");
    let synth = found.iter().find(|p| p.is_instrument).expect("synth").clone();
    views::browser::add_external(&mut studio, &synth);
    let channel = studio.selected_channel.expect("canale");
    let pattern = studio.project.patterns[0].id;
    // Nota 0 = il plugin di prova "muove" il suo gain (param 0) alla velocity, come la sua GUI.
    let ppq = studio.project.time.ppq as u64;
    studio.exec(Command::AddNotes {
        pattern,
        channel,
        notes: vec![Note::new(0, ppq, 0, 0.25)],
    });
    studio.set_song_mode(true);
    studio.toggle_record();
    let undo_before = studio.history.undo_labels().count();
    let key = mfc_host::param_key(0);
    let mirrored = wait_until(&mut studio, 5.0, |s| {
        s.project
            .channel(channel)
            .and_then(|c| c.plugin.params.get(&key).copied())
            .is_some_and(|v| (v - 0.25).abs() < 1e-6)
    });
    assert!(mirrored, "il cambio fatto dal plugin deve arrivare nel progetto");
    assert!(studio.dirty);
    // Copiato senza passare dall'undo.
    assert_eq!(studio.history.undo_labels().count(), undo_before);

    studio.toggle_record();
    assert!(!studio.recording);
    let target = AutomationTarget::ChannelParam { channel, param: key };
    let a = clip_for(&studio.project, &target).expect("automazione del parametro del plugin");
    assert_eq!((a.min, a.max), (0.0, 1.0));
    assert!((a.points[0].value - 0.25).abs() < 1e-6, "{:?}", a.points);
    assert!(a.name.contains("Gain"), "{}", a.name);
    assert!(studio
        .project
        .playlist
        .clips
        .iter()
        .any(|c| c.source == ClipSource::Automation(a.id)));
    studio.stop();
}
