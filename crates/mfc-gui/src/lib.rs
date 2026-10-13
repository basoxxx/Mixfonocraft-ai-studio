//! # mfc-gui
//!
//! Interfaccia grafica egui di Mixfonocraft AI Studio: Browser, Channel Rack con
//! step sequencer, Piano Roll, Playlist, Mixer ed editor dei plugin.

pub mod app;
pub mod autosave;
pub mod input;
pub mod plugin_window;
pub mod plugins;
#[cfg(feature = "snapshot")]
pub mod snapshot;
pub mod state;
pub mod theme;
pub mod views;

pub use app::StudioApp;
pub use state::{PluginTarget, Studio, View};

/// Avvia la finestra principale.
pub fn run(project: mfc_core::Project, path: Option<std::path::PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: {
            let vp = egui::ViewportBuilder::default()
                .with_title("Mixfonocraft AI Studio")
                .with_app_id("mixfonocraft-studio")
                .with_inner_size([1440.0, 900.0])
                .with_min_inner_size([960.0, 600.0]);
            match eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
                Ok(icon) => vp.with_icon(std::sync::Arc::new(icon)),
                Err(_) => vp,
            }
        },
        ..Default::default()
    };
    eframe::run_native(
        "Mixfonocraft AI Studio",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let mut studio = Studio::new(project);
            studio.path = path;
            let mut app = StudioApp::new(studio);
            app.autosave.start();
            Ok(Box::new(app))
        }),
    )
}
