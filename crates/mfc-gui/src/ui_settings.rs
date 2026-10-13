//! Preferenze dell'interfaccia salvate in `ui.json` nella cartella di configurazione:
//! tema, dimensioni dei pannelli, browser aperto/chiuso, cartelle dei campioni.
//!
//! Contiene anche le parti di layout legate a queste preferenze (pannello del
//! browser ridimensionabile e richiudibile, menu "Vista", scorciatoia F8), così che
//! `app.rs` debba solo richiamarle.

use std::collections::BTreeMap;
use std::path::PathBuf;

use egui::{Key, RichText};
use serde::{Deserialize, Serialize};

use crate::state::Studio;
use crate::theme::{self, pal, ThemeMode};
use crate::views;

/// Id del pannello del browser (chiave in `panel_sizes`).
pub const BROWSER_PANEL: &str = "browser";
/// Larghezza predefinita e limiti del browser.
pub const BROWSER_DEFAULT_W: f32 = 230.0;
pub const BROWSER_MIN_W: f32 = 170.0;
pub const BROWSER_MAX_W: f32 = 480.0;
/// Larghezza della striscia mostrata quando il browser è chiuso.
const COLLAPSED_W: f32 = 26.0;

/// Preferenze persistenti dell'interfaccia.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: ThemeMode,
    /// Browser visibile (F8 lo apre/chiude).
    pub browser_open: bool,
    /// Dimensioni dei pannelli ridimensionabili (larghezza o altezza), per id.
    pub panel_sizes: BTreeMap<String, f32>,
    /// Cartelle mostrate nella sezione "Campioni" del browser.
    pub sample_folders: Vec<PathBuf>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            browser_open: true,
            panel_sizes: BTreeMap::new(),
            sample_folders: default_sample_folders(),
        }
    }
}

impl UiSettings {
    /// Dimensione salvata del pannello, limitata all'intervallo ammesso.
    pub fn panel_size(&self, id: &str, default: f32, range: std::ops::RangeInclusive<f32>) -> f32 {
        self.panel_sizes
            .get(id)
            .copied()
            .filter(|v| v.is_finite())
            .unwrap_or(default)
            .clamp(*range.start(), *range.end())
    }

    /// Registra la dimensione di un pannello; `true` se è cambiata.
    pub fn set_panel_size(&mut self, id: &str, size: f32) -> bool {
        let size = size.round();
        if self.panel_sizes.get(id).is_some_and(|v| (v - size).abs() < 0.5) {
            return false;
        }
        self.panel_sizes.insert(id.to_string(), size);
        true
    }

    /// Aggiunge una cartella di campioni (senza duplicati); `true` se aggiunta.
    pub fn add_sample_folder(&mut self, dir: PathBuf) -> bool {
        if self.sample_folders.contains(&dir) {
            return false;
        }
        self.sample_folders.push(dir);
        true
    }

    pub fn from_json(txt: &str) -> Option<Self> {
        serde_json::from_str(txt).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Cartella "Musica" dell'utente, se esiste (Music / Musica / XDG_MUSIC_DIR).
pub fn default_sample_folders() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(x) = std::env::var_os("XDG_MUSIC_DIR") {
        candidates.push(PathBuf::from(x));
    }
    if let Some(h) = home {
        candidates.push(h.join("Music"));
        candidates.push(h.join("Musica"));
    }
    candidates.into_iter().find(|p| p.is_dir()).into_iter().collect()
}

fn settings_path() -> Option<PathBuf> {
    crate::plugins::config_dir().map(|d| d.join("ui.json"))
}

/// Preferenze e stato runtime dell'interfaccia (campo `Studio::ui`).
#[derive(Default)]
pub struct UiState {
    pub settings: UiSettings,
    /// `true` se le preferenze vanno salvate su disco quando cambiano (app reale;
    /// falso nei test e negli screenshot, che non toccano la configurazione).
    persist: bool,
    /// Browser dei campioni: alberi aperti, caricamento e ascolto in corso.
    pub samples: views::samples::SampleBrowser,
}

impl UiState {
    /// Carica `ui.json` (o i default) e abilita il salvataggio automatico.
    pub fn load() -> Self {
        let settings = settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| UiSettings::from_json(&t))
            .unwrap_or_default();
        Self {
            settings,
            persist: true,
            samples: Default::default(),
        }
    }

    /// Salva le preferenze (solo se caricate da disco).
    pub fn save(&self) {
        if !self.persist {
            return;
        }
        let Some(p) = settings_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.settings.to_json());
    }
}

/// Lavoro di inizio frame: tema, scorciatoia F8, caricamenti dei campioni.
pub fn frame_start(ctx: &egui::Context, studio: &mut Studio) {
    theme::sync(ctx, studio.ui.settings.theme);
    if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::F8)) {
        toggle_browser(studio);
    }
    views::samples::poll(ctx, studio);
}

/// Apre/chiude il browser (F8) e salva la preferenza.
pub fn toggle_browser(studio: &mut Studio) {
    studio.ui.settings.browser_open = !studio.ui.settings.browser_open;
    studio.ui.save();
}

/// Imposta il tema e salva la preferenza.
pub fn set_theme(studio: &mut Studio, mode: ThemeMode) {
    if studio.ui.settings.theme != mode {
        studio.ui.settings.theme = mode;
        studio.ui.save();
    }
}

/// Voci del menu "Vista": tema e browser.
pub fn view_menu(ui: &mut egui::Ui, studio: &mut Studio) {
    ui.set_min_width(220.0);
    ui.label(RichText::new("Tema").small().color(pal::TEXT_DIM()));
    let current = studio.ui.settings.theme;
    for mode in ThemeMode::ALL {
        let label = match mode {
            ThemeMode::Sistema => "Sistema (segue il sistema operativo)",
            other => other.label(),
        };
        if ui.radio(current == mode, label).clicked() {
            set_theme(studio, mode);
        }
    }
    ui.separator();
    let open = studio.ui.settings.browser_open;
    if ui
        .add(egui::Button::new(if open { "Nascondi browser" } else { "Mostra browser" }).shortcut_text("F8"))
        .clicked()
    {
        toggle_browser(studio);
        ui.close();
    }
    if ui.button("Larghezza browser predefinita").clicked() {
        studio.ui.settings.panel_sizes.remove(BROWSER_PANEL);
        studio.ui.save();
        // La memoria del pannello di egui ha la precedenza: va azzerata.
        ui.ctx()
            .data_mut(|d| d.remove::<egui::containers::panel::PanelState>(egui::Id::new("browser")));
        ui.close();
    }
}

/// Pannello sinistro del browser: ridimensionabile (larghezza salvata) o richiuso
/// in una striscia sottile con il pulsante per riaprirlo.
pub fn browser_panel(ui: &mut egui::Ui, studio: &mut Studio) {
    let frame = egui::Frame::new().fill(pal::BG_APP()).inner_margin(egui::Margin {
        left: 6,
        right: 0,
        top: 6,
        bottom: 6,
    });
    if !studio.ui.settings.browser_open {
        egui::Panel::left("browser_collapsed")
            .exact_size(COLLAPSED_W)
            .resizable(false)
            .frame(frame)
            .show(ui, |ui| {
                views::widgets::view_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    if views::samples::collapse_button(ui, false).clicked() {
                        toggle_browser(studio);
                    }
                });
            });
        return;
    }
    let range = BROWSER_MIN_W..=BROWSER_MAX_W;
    let width = studio
        .ui
        .settings
        .panel_size(BROWSER_PANEL, BROWSER_DEFAULT_W, range.clone());
    let resp = egui::Panel::left("browser")
        .resizable(true)
        .default_size(width)
        .size_range(range)
        .frame(frame)
        .show(ui, |ui| {
            views::widgets::view_frame().show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                views::browser::show(ui, studio);
            });
        });
    remember_size(ui.ctx(), studio, BROWSER_PANEL, resp.response.rect.width());
}

/// Salva la dimensione di un pannello ridimensionabile quando l'utente rilascia il bordo.
/// Da usare anche per eventuali pannelli inferiori (`rect.height()`).
pub fn remember_size(ctx: &egui::Context, studio: &mut Studio, id: &str, size: f32) {
    if ctx.input(|i| i.pointer.any_down()) {
        return;
    }
    if studio.ui.settings.set_panel_size(id, size) {
        studio.ui.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_and_defaults() {
        let mut s = UiSettings {
            theme: ThemeMode::Chiaro,
            browser_open: false,
            ..Default::default()
        };
        s.set_panel_size(BROWSER_PANEL, 301.4);
        s.add_sample_folder(PathBuf::from("/campioni"));
        assert!(!s.add_sample_folder(PathBuf::from("/campioni")));
        let back = UiSettings::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.panel_size(BROWSER_PANEL, 200.0, 170.0..=480.0), 301.0);
        // Campi mancanti: valori predefiniti.
        let partial = UiSettings::from_json(r#"{"theme":"Sistema"}"#).unwrap();
        assert_eq!(partial.theme, ThemeMode::Sistema);
        assert!(partial.browser_open);
        assert_eq!(partial.panel_size("x", 220.0, 170.0..=480.0), 220.0);
        // Valori fuori intervallo vengono limitati.
        let mut wide = UiSettings::default();
        wide.set_panel_size("x", 9000.0);
        assert_eq!(wide.panel_size("x", 220.0, 170.0..=480.0), 480.0);
    }

    #[test]
    fn set_panel_size_reports_changes() {
        let mut s = UiSettings::default();
        assert!(s.set_panel_size("b", 250.0));
        assert!(!s.set_panel_size("b", 250.2));
        assert!(s.set_panel_size("b", 260.0));
    }

    #[test]
    fn browser_width_is_stable_and_collapsible() {
        let mut app = crate::StudioApp::new(Studio::new(mfc_core::Project::new("t")));
        app.studio.ui.settings.set_panel_size(BROWSER_PANEL, 300.0);
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            ..Default::default()
        };
        for _ in 0..3 {
            ctx.run_ui(input(), |ui| app.show(ui)).drop_without_applying_deltas();
        }
        // La larghezza salvata è quella applicata: nessuna deriva tra un avvio e l'altro.
        assert_eq!(app.studio.ui.settings.panel_sizes[BROWSER_PANEL], 300.0);
        toggle_browser(&mut app.studio);
        for _ in 0..2 {
            ctx.run_ui(input(), |ui| app.show(ui)).drop_without_applying_deltas();
        }
        assert!(!app.studio.ui.settings.browser_open);
        assert_eq!(app.studio.ui.settings.panel_sizes[BROWSER_PANEL], 300.0);
    }

    #[test]
    fn theme_mode_resolution() {
        use egui::Theme;
        assert!(!ThemeMode::Scuro.is_light(Some(Theme::Light)));
        assert!(ThemeMode::Chiaro.is_light(None));
        assert!(ThemeMode::Sistema.is_light(Some(Theme::Light)));
        assert!(!ThemeMode::Sistema.is_light(Some(Theme::Dark)));
        assert!(!ThemeMode::Sistema.is_light(None));
    }
}
