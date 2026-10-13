//! Test headless: renderizza tutte le viste e l'editor plugin senza finestra, verificando
//! che il solo rendering non modifichi il progetto.

use egui::{vec2, Id, Pos2, RawInput, Rect};
use mfc_core::{ChannelId, ChannelKind, ClipSource, Command, Note, Outcome, PluginInstance, Project};
use mfc_gui::{PluginTarget, Studio, StudioApp, View};
use mfc_plugins::default_instance;

fn apply(p: &mut Project, cmd: Command) -> Outcome {
    cmd.apply(p).expect("comando demo valido")
}

fn add_channel(p: &mut Project, name: &str, plugin: &str, insert: u16) -> ChannelId {
    let plugin = default_instance(plugin).expect("plugin noto");
    match apply(
        p,
        Command::AddChannel {
            name: name.into(),
            kind: ChannelKind::Generator,
            plugin,
            mixer_insert: Some(insert),
        },
    ) {
        Outcome::Channel(c) => c,
        other => panic!("esito inatteso: {other:?}"),
    }
}

/// Progetto dimostrativo: kick a passi, 3xosc con note libere, un effetto e una clip.
fn demo_project() -> Project {
    let mut p = Project::new("Demo");
    let pattern = p.patterns[0].id;
    let kick = add_channel(&mut p, "Kick", "mfc.kick", 1);
    let osc = add_channel(&mut p, "3xOsc", "mfc.3xosc", 2);
    for step in [0, 4, 8, 12] {
        apply(
            &mut p,
            Command::SetStep {
                pattern,
                channel: kick,
                step,
                on: true,
                velocity: None,
            },
        );
    }
    let bar = p.time.ticks_per_bar();
    apply(
        &mut p,
        Command::AddNotes {
            pattern,
            channel: osc,
            notes: vec![Note::new(0, bar, 60, 0.8), Note::new(bar, bar, 64, 0.8)],
        },
    );
    apply(
        &mut p,
        Command::AddEffect {
            insert: 1,
            plugin: default_instance("mfc.reverb").expect("reverb noto"),
            slot: None,
        },
    );
    apply(
        &mut p,
        Command::PlaceClip {
            track: 0,
            position: 0,
            length: None,
            source: ClipSource::Pattern(pattern),
        },
    );
    p
}

fn render(app: &mut StudioApp, ctx: &egui::Context) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
        ..Default::default()
    };
    ctx.run_ui(input, |ui| app.show(ui)).drop_without_applying_deltas();
}

fn render_frames(app: &mut StudioApp, ctx: &egui::Context, frames: usize) {
    for _ in 0..frames {
        render(app, ctx);
    }
}

#[test]
fn every_view_renders_without_touching_project() {
    let mut app = StudioApp::new(Studio::new(demo_project()));
    let before = app.studio.project.clone();
    let ctx = egui::Context::default();
    for view in [View::ChannelRack, View::PianoRoll, View::Playlist, View::Mixer] {
        app.studio.view = view;
        render_frames(&mut app, &ctx, 2);
    }
    assert_eq!(app.studio.project, before);
}

#[test]
fn mixer_with_all_inserts_renders() {
    let mut app = StudioApp::new(Studio::new(demo_project()));
    let before = app.studio.project.clone();
    let ctx = egui::Context::default();
    app.studio.view = View::Mixer;
    ctx.data_mut(|d| d.insert_temp(Id::new("mixer_show_all"), true));
    render_frames(&mut app, &ctx, 2);
    assert_eq!(app.studio.project, before);
}

#[test]
fn plugin_editor_on_channel_renders() {
    let mut app = StudioApp::new(Studio::new(demo_project()));
    let before = app.studio.project.clone();
    let ctx = egui::Context::default();
    let kick = app.studio.project.channels[0].id;
    app.studio.plugin_editor = Some(PluginTarget::Channel(kick));
    render_frames(&mut app, &ctx, 2);
    assert_eq!(app.studio.project, before);
}

#[test]
fn plugin_editor_on_effect_renders() {
    let mut app = StudioApp::new(Studio::new(demo_project()));
    let before = app.studio.project.clone();
    let ctx = egui::Context::default();
    app.studio.plugin_editor = Some(PluginTarget::Effect { insert: 1, slot: 0 });
    render_frames(&mut app, &ctx, 2);
    assert_eq!(app.studio.project, before);
}

#[test]
fn plugin_editor_on_foreign_effect_renders() {
    let mut project = demo_project();
    let foreign = PluginInstance {
        kind: "ext.vst".into(),
        foreign_name: Some("Vendor X".into()),
        opaque_state: Some(vec![1, 2, 3]),
        ..Default::default()
    };
    apply(
        &mut project,
        Command::AddEffect {
            insert: 1,
            plugin: foreign,
            slot: None,
        },
    );
    let mut app = StudioApp::new(Studio::new(project));
    let before = app.studio.project.clone();
    let ctx = egui::Context::default();
    app.studio.plugin_editor = Some(PluginTarget::Effect { insert: 1, slot: 1 });
    render_frames(&mut app, &ctx, 2);
    assert_eq!(app.studio.project, before);
}

#[test]
fn opens_song_mode_on_first_pattern_without_saved_ui() {
    let mut p = demo_project();
    // Come dopo una sessione MCP: l'ultimo pattern creato è quello selezionato.
    let Outcome::Pattern(last) = apply(&mut p, Command::AddPattern { name: "Crash".into() }) else {
        panic!()
    };
    p.current_pattern = Some(last);
    let first = p.patterns[0].id;
    let studio = Studio::new(p.clone());
    assert!(studio.song_mode, "con clip in Playlist si apre in Song");
    assert_eq!(studio.view, View::Playlist);
    assert_eq!(studio.current_pattern(), Some(first));

    // Senza clip: Pattern sul Channel Rack.
    let studio = Studio::new(Project::new("Vuoto"));
    assert!(!studio.song_mode);
    assert_eq!(studio.view, View::ChannelRack);
}

#[test]
fn saved_ui_state_is_restored() {
    let dir = std::env::temp_dir().join(format!("mfc_gui_ui_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui.mfc");
    let mut studio = Studio::new(demo_project());
    studio.view = View::Mixer;
    studio.set_song_mode(false);
    studio.save_as(&path).unwrap();
    let mut other = Studio::new(Project::new("x"));
    other.open(&path).unwrap();
    assert_eq!(other.view, View::Mixer);
    assert!(!other.song_mode);
    assert_eq!(other.path.as_deref(), Some(path.as_path()));
}
