//! Genera screenshot PNG di tutte le viste, in tema scuro e chiaro:
//! `cargo run -p mfc-gui --release --features snapshot --example screenshot -- <progetto.mfc>
//! <cartella_uscita> [larghezza altezza scala]`.

use mfc_gui::snapshot::SoftwareRenderer;
use mfc_gui::theme::ThemeMode;
use mfc_gui::{PluginTarget, Studio, StudioApp, View};

/// Cartella temporanea con un paio di loop renderizzati dal progetto, mostrata nella
/// sezione "Campioni" del browser.
fn demo_samples(project: &mfc_core::Project) -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir()
        .join("mfc_screenshot_samples")
        .join("Campioni demo");
    let loops = dir.join("Loop");
    std::fs::create_dir_all(&loops).ok()?;
    for (i, p) in project.patterns.iter().take(3).enumerate() {
        let opts = mfc_engine::RenderOptions {
            pattern: Some(p.id),
            tail_seconds: 0.2,
            ..Default::default()
        };
        let name = format!("{:02} {}.wav", i + 1, p.name);
        let path = if i == 0 { dir.join(name) } else { loops.join(name) };
        if !path.exists() {
            mfc_engine::render_to_file(project, &path, &opts, mfc_engine::WavFormat::Int16).ok()?;
        }
    }
    Some(dir)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let project_path = args.get(1).map(String::as_str).unwrap_or("examples/demo.mfc");
    let out_dir = std::path::PathBuf::from(args.get(2).map(String::as_str).unwrap_or("screenshots"));
    let w: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1440);
    let h: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(900);
    let scale: f32 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    std::fs::create_dir_all(&out_dir).unwrap();

    let txt = std::fs::read_to_string(project_path).expect("progetto");
    let project = mfc_core::Project::from_json(&txt).expect("progetto .mfc valido");
    let samples = demo_samples(&project);
    let mut app = StudioApp::new(Studio::new(project));
    // Preferenze predefinite (niente ui.json dell'utente) + cartella di campioni demo aperta.
    app.studio.ui.settings.sample_folders = samples.iter().cloned().collect();
    if let Some(dir) = &samples {
        app.studio.ui.samples.expand(dir.clone());
    }
    let ctx = egui::Context::default();
    mfc_gui::theme::apply(&ctx);
    let mut r = SoftwareRenderer::default();

    for (prefix, mode) in [("", ThemeMode::Scuro), ("light_", ThemeMode::Chiaro)] {
        app.studio.ui.settings.theme = mode;
        app.studio.plugin_editor = None;
        let views = [
            ("1_channel_rack", View::ChannelRack),
            ("2_piano_roll", View::PianoRoll),
            ("3_playlist", View::Playlist),
            ("4_mixer", View::Mixer),
        ];
        for (name, view) in views {
            app.studio.view = view;
            if view == View::PianoRoll {
                // Canale melodico per il piano roll (Lead nel demo), altrimenti il primo.
                let best = app.studio.project.channels.iter().find(|c| c.name == "Lead");
                app.studio.selected_channel = best.map(|c| c.id);
                if let Some(p) = app.studio.project.patterns.last() {
                    app.studio.project.current_pattern = Some(p.id);
                }
            }
            let shot = r.capture(&ctx, [w, h], scale, 4, |ui| app.show(ui));
            let path = out_dir.join(format!("{prefix}{name}.png"));
            shot.save_png(&path).unwrap();
            println!("{}", path.display());
        }
        // Editor plugin aperto sull'ultimo canale (Nova nel demo).
        app.studio.view = View::ChannelRack;
        app.studio.plugin_editor = app.studio.project.channels.last().map(|c| PluginTarget::Channel(c.id));
        let shot = r.capture(&ctx, [w, h], scale, 4, |ui| app.show(ui));
        let path = out_dir.join(format!("{prefix}5_plugin_editor.png"));
        shot.save_png(&path).unwrap();
        println!("{}", path.display());
    }
}
