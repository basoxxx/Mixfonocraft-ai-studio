//! # mfc-gui
//!
//! Interfaccia grafica egui di Mixfonocraft AI Studio: Browser, Channel Rack con
//! step sequencer, Piano Roll, Playlist, Mixer ed editor dei plugin.

pub mod app;
pub mod state;
pub mod views;

pub use app::StudioApp;
pub use state::{PluginTarget, Studio, View};

/// Avvia la finestra principale.
pub fn run(project: mfc_core::Project, path: Option<std::path::PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Mixfonocraft AI Studio")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Mixfonocraft AI Studio",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let mut studio = Studio::new(project);
            studio.path = path;
            Ok(Box::new(StudioApp::new(studio)))
        }),
    )
}
