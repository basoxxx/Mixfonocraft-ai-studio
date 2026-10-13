//! Percorso completo con un plugin CLAP reale (`mfc-test-clap`): scansione da file,
//! aggiunta dal browser, note nel pattern, render, salvataggio dello stato, rimozione.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;

use mfc_core::{Command, Note, Outcome, Project};
use mfc_gui::{plugins, views, Studio};

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
    let dir = std::env::temp_dir().join(format!("mfc_gui_e2e_{}", std::process::id()));
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
<key>CFBundleIdentifier</key><string>com.mixfonocraft.test.clap.gui</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>"#,
    )
    .unwrap();
}

#[test]
fn clap_plugin_from_browser_to_render_and_save() {
    let path = build_plugin();
    let mut studio = Studio::new(Project::new("e2e"));
    let found = studio.host.scan_file(&path).expect("scan");
    let synth = found.iter().find(|p| p.is_instrument).expect("synth").clone();
    assert!(found.iter().any(|p| p.is_effect && !p.is_instrument));

    // Aggiunta dal browser → nuovo canale selezionato con istanza viva.
    views::browser::add_external(&mut studio, &synth);
    let channel = studio.selected_channel.expect("canale");
    let inst_id = studio
        .project
        .channel(channel)
        .unwrap()
        .plugin
        .instance_id
        .clone()
        .expect("instance_id");
    assert!(studio.host.instance(&inst_id).is_some(), "istanza creata dal motore");

    // Una nota nel pattern e render offline (istanze separate da quelle live).
    let Some(Outcome::Pattern(pattern)) = studio.exec(Command::AddPattern { name: "P".into() }) else {
        panic!("pattern");
    };
    let ppq = studio.project.time.ppq as u64;
    studio.exec(Command::AddNotes {
        pattern,
        channel,
        notes: vec![Note::new(0, ppq * 2, 69, 0.9)],
    });
    let opts = mfc_engine::RenderOptions {
        pattern: Some(pattern),
        ..Default::default()
    };
    let out = mfc_engine::render(&plugins::detached_copy(&studio.project), &opts).expect("render");
    let peak = out.mix.left.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.01, "il plugin deve suonare (picco {peak})");
    studio.host.retain(&plugins::external_ids(&studio.project));
    assert_eq!(studio.host.live_ids(), vec![inst_id.clone()]);

    // Salvataggio: lo stato del plugin finisce nel progetto.
    let file = std::env::temp_dir().join(format!("mfc_gui_e2e_{}.mfc", std::process::id()));
    studio.save_as(&file).expect("salva");
    let saved = Project::from_json(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let plugin = &saved.channel(channel).unwrap().plugin;
    assert!(plugin.opaque_state.as_ref().is_some_and(|s| !s.is_empty()));
    std::fs::remove_file(file).ok();

    // Rimozione del canale → istanza chiusa (distrutta dall'idle sul thread UI).
    studio.exec(Command::RemoveChannel { channel });
    assert!(studio.host.live_ids().is_empty());
    studio.tick_plugins();
}
