//! Genera screenshot PNG di tutte le viste: `cargo run -p mfc-gui --release --features snapshot
//! --example screenshot -- <progetto.mfc> <cartella_uscita> [larghezza altezza scala]`.

use mfc_gui::snapshot::SoftwareRenderer;
use mfc_gui::{PluginTarget, Studio, StudioApp, View};

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
    let mut app = StudioApp::new(Studio::new(project));
    let ctx = egui::Context::default();
    mfc_gui::theme::apply(&ctx);
    let mut r = SoftwareRenderer::default();

    let views = [
        ("1_channel_rack", View::ChannelRack),
        ("2_piano_roll", View::PianoRoll),
        ("3_playlist", View::Playlist),
        ("4_mixer", View::Mixer),
    ];
    for (name, view) in views {
        app.studio.view = view;
        if view == View::PianoRoll {
            // Seleziona il canale con più note per il piano roll.
            let best = app.studio.project.channels.iter().max_by_key(|c| {
                app.studio
                    .project
                    .patterns
                    .iter()
                    .filter_map(|p| p.notes.get(&c.id))
                    .map(Vec::len)
                    .sum::<usize>()
            });
            app.studio.selected_channel = best.map(|c| c.id);
            if let Some(p) = app.studio.project.patterns.last() {
                app.studio.project.current_pattern = Some(p.id);
            }
        }
        let shot = r.capture(&ctx, [w, h], scale, 4, |ui| app.show(ui));
        let path = out_dir.join(format!("{name}.png"));
        shot.save_png(&path).unwrap();
        println!("{}", path.display());
    }
    // Editor plugin aperto sul primo canale.
    app.studio.view = View::ChannelRack;
    app.studio.plugin_editor = app.studio.project.channels.last().map(|c| PluginTarget::Channel(c.id));
    let shot = r.capture(&ctx, [w, h], scale, 4, |ui| app.show(ui));
    let path = out_dir.join("5_plugin_editor.png");
    shot.save_png(&path).unwrap();
    println!("{}", path.display());
}
