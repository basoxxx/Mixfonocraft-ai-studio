//! # mfc-gui
//!
//! Interfaccia grafica egui di Mixfonocraft AI Studio: Browser, Channel Rack con
//! step sequencer, Piano Roll, Playlist, Mixer ed editor dei plugin.

pub mod app;
pub mod live;
#[cfg(target_os = "macos")]
mod macos;
pub mod open_queue;
#[cfg(feature = "snapshot")]
pub mod snapshot;
pub mod state;
pub mod theme;
pub mod views;

pub use app::StudioApp;
pub use state::{load_project, Loaded, PluginTarget, Studio, View};

/// Opzioni di avvio della finestra.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Apre subito il server MCP live (`mixfonocraft mcp --attach` si collega alla GUI).
    pub live_mcp: bool,
    /// Cartella di lavoro dei percorsi relativi del server live (default: vedi
    /// [`live::default_workdir`]).
    pub workdir: Option<std::path::PathBuf>,
}

/// Avvia la finestra principale.
pub fn run(project: mfc_core::Project, path: Option<std::path::PathBuf>) -> eframe::Result {
    run_with(project, path, RunOptions::default())
}

/// Avvia la finestra principale con opzioni.
pub fn run_with(project: mfc_core::Project, path: Option<std::path::PathBuf>, opts: RunOptions) -> eframe::Result {
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
    // Event loop creato qui (non da eframe) per poter intervenire tra la sua creazione e
    // il lancio dell'applicazione: su macOS è lì che si aggancia l'apertura dei documenti.
    let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    #[cfg(target_os = "macos")]
    macos::install_open_documents_handler();
    let mut app = eframe::create_native(
        "Mixfonocraft AI Studio",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            open_queue::set_context(&cc.egui_ctx);
            let mut studio = Studio::new(project);
            studio.path = path;
            let mut app = StudioApp::new(studio);
            if opts.live_mcp {
                app.start_live(&cc.egui_ctx, opts.workdir.clone());
            }
            app.workdir = opts.workdir;
            Ok(Box::new(app))
        }),
        &event_loop,
    );
    event_loop.run_app(&mut app)?;
    Ok(())
}
