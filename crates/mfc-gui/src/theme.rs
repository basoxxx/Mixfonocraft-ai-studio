//! Sistema visivo: palette, font e stile egui di Mixfonocraft.
//!
//! Estetica da DAW professionale: superfici grafite a più livelli di profondità,
//! un solo accento arancione per stato attivo/selezione, colori per canale.

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Theme};

/// Palette dell'interfaccia, commutabile a runtime tra scura e chiara.
///
/// Ogni colore è una funzione (`pal::ACCENT()`) che legge la palette corrente,
/// scelta con [`pal::set_light`]. I nomi in maiuscolo restano quelli delle
/// costanti originali: per adattare codice vecchio basta `pal::X()` → `pal::X()`.
#[allow(non_snake_case)]
pub mod pal {
    use std::sync::atomic::{AtomicU8, Ordering};

    use egui::Color32;

    /// Tabella dei colori di una variante del tema.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Palette {
        pub bg_app: Color32,
        pub bg_panel: Color32,
        pub bg_raised: Color32,
        pub bg_widget: Color32,
        pub bg_hover: Color32,
        pub bg_inset: Color32,
        pub border: Color32,
        pub line: Color32,
        pub text: Color32,
        pub text_dim: Color32,
        pub text_faint: Color32,
        pub text_strong: Color32,
        pub accent: Color32,
        pub accent_soft: Color32,
        pub green: Color32,
        pub red: Color32,
        pub yellow: Color32,
        pub blue: Color32,
        pub lcd: Color32,
        pub grid_a: Color32,
        pub grid_b: Color32,
        pub grid_line: Color32,
        pub grid_line_strong: Color32,
        pub shadow: Color32,
        pub swatches: [Color32; 10],
    }

    const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
        Color32::from_rgb(r, g, b)
    }

    /// Tema scuro: superfici grafite a più livelli di profondità.
    pub static DARK: Palette = Palette {
        bg_app: rgb(0x10, 0x12, 0x15),
        bg_panel: rgb(0x19, 0x1c, 0x20),
        bg_raised: rgb(0x22, 0x26, 0x2b),
        bg_widget: rgb(0x2b, 0x30, 0x36),
        bg_hover: rgb(0x36, 0x3c, 0x43),
        bg_inset: rgb(0x0c, 0x0e, 0x10),
        border: rgb(0x08, 0x09, 0x0b),
        line: rgb(0x30, 0x35, 0x3c),
        text: rgb(0xe2, 0xe6, 0xea),
        text_dim: rgb(0x92, 0x9a, 0xa3),
        text_faint: rgb(0x5d, 0x64, 0x6c),
        text_strong: rgb(0xff, 0xff, 0xff),
        accent: rgb(0xff, 0x8a, 0x1e),
        accent_soft: rgb(0xc2, 0x69, 0x17),
        green: rgb(0x5c, 0xd6, 0x7a),
        red: rgb(0xff, 0x55, 0x55),
        yellow: rgb(0xff, 0xd1, 0x66),
        blue: rgb(0x5a, 0xb8, 0xff),
        lcd: rgb(0xff, 0xa4, 0x4a),
        grid_a: rgb(0x1c, 0x20, 0x25),
        grid_b: rgb(0x18, 0x1b, 0x20),
        grid_line: rgb(0x2a, 0x2f, 0x35),
        grid_line_strong: rgb(0x3a, 0x40, 0x48),
        shadow: Color32::from_black_alpha(120),
        swatches: [
            rgb(0xff, 0x8a, 0x1e),
            rgb(0x5a, 0xb8, 0xff),
            rgb(0x8f, 0xd9, 0x5b),
            rgb(0xff, 0x6b, 0x8b),
            rgb(0xb3, 0x8c, 0xff),
            rgb(0x3f, 0xd1, 0xc4),
            rgb(0xff, 0xd1, 0x66),
            rgb(0xff, 0x7a, 0x50),
            rgb(0x6f, 0x9b, 0xff),
            rgb(0xe0, 0x7c, 0xe6),
        ],
    };

    /// Tema chiaro: grigi in stile FL Studio "light", stesso accento arancione
    /// (leggermente più scuro per il contrasto sul chiaro).
    pub static LIGHT: Palette = Palette {
        bg_app: rgb(0xa9, 0xae, 0xb4),
        bg_panel: rgb(0xd6, 0xd9, 0xdd),
        bg_raised: rgb(0xe4, 0xe6, 0xe9),
        bg_widget: rgb(0xf0, 0xf1, 0xf3),
        bg_hover: rgb(0xfa, 0xfa, 0xfb),
        bg_inset: rgb(0xbd, 0xc2, 0xc8),
        border: rgb(0x8c, 0x93, 0x9b),
        line: rgb(0xb4, 0xb9, 0xc0),
        text: rgb(0x1b, 0x1f, 0x24),
        text_dim: rgb(0x48, 0x50, 0x59),
        text_faint: rgb(0x6c, 0x74, 0x7d),
        text_strong: rgb(0x00, 0x00, 0x00),
        accent: rgb(0xe0, 0x67, 0x0a),
        accent_soft: rgb(0xf0, 0xa2, 0x5c),
        green: rgb(0x23, 0x9a, 0x48),
        red: rgb(0xd8, 0x34, 0x34),
        yellow: rgb(0xc2, 0x8a, 0x00),
        blue: rgb(0x1f, 0x78, 0xd1),
        lcd: rgb(0xa0, 0x45, 0x00),
        grid_a: rgb(0xcd, 0xd1, 0xd6),
        grid_b: rgb(0xc4, 0xc9, 0xcf),
        grid_line: rgb(0xae, 0xb4, 0xbb),
        grid_line_strong: rgb(0x92, 0x99, 0xa1),
        shadow: Color32::from_black_alpha(45),
        swatches: [
            rgb(0xf0, 0x7a, 0x12),
            rgb(0x2f, 0x8f, 0xe0),
            rgb(0x5c, 0xb0, 0x2c),
            rgb(0xe8, 0x4a, 0x6e),
            rgb(0x8c, 0x62, 0xe6),
            rgb(0x1c, 0xa8, 0x9c),
            rgb(0xe0, 0xa8, 0x22),
            rgb(0xec, 0x5c, 0x30),
            rgb(0x4a, 0x74, 0xe8),
            rgb(0xc4, 0x52, 0xcc),
        ],
    };

    /// 0 = scuro, 1 = chiaro.
    static CURRENT: AtomicU8 = AtomicU8::new(0);

    /// Seleziona la palette chiara (`true`) o scura (`false`) per tutta l'applicazione.
    pub fn set_light(light: bool) {
        CURRENT.store(light as u8, Ordering::Relaxed);
    }

    /// `true` se la palette corrente è quella chiara.
    pub fn is_light() -> bool {
        CURRENT.load(Ordering::Relaxed) == 1
    }

    /// Palette corrente.
    pub fn current() -> &'static Palette {
        if is_light() {
            &LIGHT
        } else {
            &DARK
        }
    }

    /// Sfondo dell'applicazione (dietro ai pannelli).
    pub fn BG_APP() -> Color32 {
        current().bg_app
    }
    /// Pannelli principali.
    pub fn BG_PANEL() -> Color32 {
        current().bg_panel
    }
    /// Superfici rialzate (header, finestre, strip).
    pub fn BG_RAISED() -> Color32 {
        current().bg_raised
    }
    /// Widget a riposo.
    pub fn BG_WIDGET() -> Color32 {
        current().bg_widget
    }
    /// Widget in hover.
    pub fn BG_HOVER() -> Color32 {
        current().bg_hover
    }
    /// Aree incassate (display, tracce, griglie).
    pub fn BG_INSET() -> Color32 {
        current().bg_inset
    }
    /// Bordi scuri.
    pub fn BORDER() -> Color32 {
        current().border
    }
    /// Linee di separazione chiare.
    pub fn LINE() -> Color32 {
        current().line
    }
    pub fn TEXT() -> Color32 {
        current().text
    }
    pub fn TEXT_DIM() -> Color32 {
        current().text_dim
    }
    pub fn TEXT_FAINT() -> Color32 {
        current().text_faint
    }
    /// Testo in massima evidenza (hover, selezione): bianco sullo scuro, nero sul chiaro.
    pub fn TEXT_STRONG() -> Color32 {
        current().text_strong
    }
    /// Accento principale.
    pub fn ACCENT() -> Color32 {
        current().accent
    }
    pub fn ACCENT_SOFT() -> Color32 {
        current().accent_soft
    }
    pub fn GREEN() -> Color32 {
        current().green
    }
    pub fn RED() -> Color32 {
        current().red
    }
    pub fn YELLOW() -> Color32 {
        current().yellow
    }
    pub fn BLUE() -> Color32 {
        current().blue
    }
    /// Colore delle cifre nei display LCD.
    pub fn LCD() -> Color32 {
        current().lcd
    }
    /// Griglie (playlist, piano roll): fasce alternate di battute.
    pub fn GRID_A() -> Color32 {
        current().grid_a
    }
    pub fn GRID_B() -> Color32 {
        current().grid_b
    }
    /// Linee della griglia (beat).
    pub fn GRID_LINE() -> Color32 {
        current().grid_line
    }
    /// Linee delle battute / ottave.
    pub fn GRID_LINE_STRONG() -> Color32 {
        current().grid_line_strong
    }
    /// Ombra dei widget in rilievo.
    pub fn SHADOW() -> Color32 {
        current().shadow
    }
    /// Colori assegnati automaticamente a canali e pattern.
    pub fn SWATCHES() -> [Color32; 10] {
        current().swatches
    }
}

/// Scelta del tema da parte dell'utente.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    #[default]
    Scuro,
    Chiaro,
    /// Segue il tema del sistema operativo (se noto), altrimenti scuro.
    Sistema,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Scuro, ThemeMode::Chiaro, ThemeMode::Sistema];

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Scuro => "Scuro",
            ThemeMode::Chiaro => "Chiaro",
            ThemeMode::Sistema => "Sistema",
        }
    }

    /// `true` se la modalità corrisponde alla palette chiara, dato il tema di sistema.
    pub fn is_light(self, system: Option<Theme>) -> bool {
        match self {
            ThemeMode::Scuro => false,
            ThemeMode::Chiaro => true,
            ThemeMode::Sistema => system == Some(Theme::Light),
        }
    }
}

/// Allinea palette e stile egui alla modalità scelta. Da chiamare a ogni frame:
/// costa un confronto e riapplica lo stile solo quando cambia.
pub fn sync(ctx: &egui::Context, mode: ThemeMode) {
    let light = mode.is_light(ctx.system_theme());
    let id = egui::Id::new("mfc_theme_light");
    let applied: Option<bool> = ctx.data(|d| d.get_temp(id));
    if applied != Some(light) || pal::is_light() != light {
        pal::set_light(light);
        apply_style(ctx);
        ctx.data_mut(|d| d.insert_temp(id, light));
        ctx.request_repaint();
    }
}

/// Colore di default del modello (non personalizzato).
const MODEL_DEFAULT: mfc_core::Color = mfc_core::Color(0x5f, 0x75, 0x81);

/// Colore di visualizzazione: quello scelto dall'utente o uno della palette per indice.
pub fn item_color(c: mfc_core::Color, index: usize) -> Color32 {
    if c == MODEL_DEFAULT {
        pal::SWATCHES()[index % pal::SWATCHES().len()]
    } else {
        Color32::from_rgb(c.0, c.1, c.2)
    }
}

/// Interpolazione lineare tra due colori.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Famiglia per titoli e etichette in grassetto.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// Font per display LCD e valori numerici.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, bold())
}

/// Installa i font incorporati (Inter, JetBrains Mono) mantenendo i fallback di egui.
pub fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    f.font_data.insert(
        "inter".into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Medium.ttf"
        ))),
    );
    f.font_data.insert(
        "inter-bold".into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-Bold.ttf"))),
    );
    f.font_data.insert(
        "jbmono".into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono-SemiBold.ttf"
        ))),
    );
    let fallback_prop = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = f.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut prop = vec!["inter".to_string()];
    prop.extend(fallback_prop.iter().cloned());
    let mut boldf = vec!["inter-bold".to_string()];
    boldf.extend(fallback_prop.iter().cloned());
    let mut monof = vec!["jbmono".to_string()];
    monof.extend(fallback_mono);
    f.families.insert(FontFamily::Proportional, prop);
    f.families.insert(bold(), boldf);
    f.families.insert(FontFamily::Monospace, monof);
    f
}

/// Applica il tema una sola volta per contesto. Restituisce `false` nel frame in cui
/// viene installato: i nuovi font sono disponibili solo dal frame successivo.
pub fn ensure(ctx: &egui::Context) -> bool {
    let id = egui::Id::new("mfc_theme_applied");
    if ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        return true;
    }
    apply(ctx);
    ctx.data_mut(|d| d.insert_temp(id, true));
    ctx.request_repaint();
    false
}

/// Applica font, colori e metriche al contesto (per la palette corrente).
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    apply_style(ctx);
}

/// Applica colori e metriche della palette corrente (senza ricaricare i font).
pub fn apply_style(ctx: &egui::Context) {
    let light = pal::is_light();
    ctx.set_theme(if light { Theme::Light } else { Theme::Dark });
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(10.5)),
            (TextStyle::Body, FontId::proportional(12.5)),
            (TextStyle::Button, FontId::proportional(12.5)),
            (TextStyle::Heading, FontId::new(15.0, bold())),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(6.0, 5.0);
        s.button_padding = egui::vec2(9.0, 4.0);
        s.interact_size = egui::vec2(24.0, 22.0);
        s.window_margin = Margin::same(12);
        s.menu_margin = Margin::same(6);
        s.slider_rail_height = 4.0;
        s.combo_height = 300.0;

        let v = &mut style.visuals;
        v.dark_mode = !light;
        v.override_text_color = None;
        v.panel_fill = pal::BG_PANEL();
        v.window_fill = pal::BG_RAISED();
        v.window_stroke = Stroke::new(1.0, pal::BORDER());
        v.window_corner_radius = CornerRadius::same(8);
        v.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(if light { 60 } else { 140 }),
        };
        v.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: Color32::from_black_alpha(if light { 50 } else { 120 }),
        };
        v.menu_corner_radius = CornerRadius::same(6);
        v.extreme_bg_color = pal::BG_INSET();
        v.faint_bg_color = pal::BG_RAISED();
        v.code_bg_color = pal::BG_INSET();
        v.text_edit_bg_color = Some(if light { Color32::WHITE } else { pal::BG_INSET() });
        v.hyperlink_color = pal::BLUE();
        v.warn_fg_color = pal::YELLOW();
        v.error_fg_color = pal::RED();
        v.selection.bg_fill = pal::ACCENT().gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, pal::ACCENT());
        v.slider_trailing_fill = true;
        v.collapsing_header_frame = false;
        v.indent_has_left_vline = false;
        v.striped = false;

        let r = CornerRadius::same(4);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = pal::BG_PANEL();
        w.noninteractive.weak_bg_fill = pal::BG_PANEL();
        w.noninteractive.bg_stroke = Stroke::new(1.0, pal::LINE());
        w.noninteractive.fg_stroke = Stroke::new(1.0, pal::TEXT_DIM());
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = pal::BG_WIDGET();
        w.inactive.weak_bg_fill = pal::BG_WIDGET();
        w.inactive.bg_stroke = Stroke::new(1.0, pal::BORDER());
        w.inactive.fg_stroke = Stroke::new(1.0, pal::TEXT());
        w.inactive.corner_radius = r;
        w.inactive.expansion = 0.0;

        w.hovered.bg_fill = pal::BG_HOVER();
        w.hovered.weak_bg_fill = pal::BG_HOVER();
        w.hovered.bg_stroke = Stroke::new(1.0, pal::ACCENT_SOFT());
        w.hovered.fg_stroke = Stroke::new(1.5, pal::TEXT_STRONG());
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = mix(pal::BG_HOVER(), pal::ACCENT(), 0.25);
        w.active.weak_bg_fill = mix(pal::BG_HOVER(), pal::ACCENT(), 0.25);
        w.active.bg_stroke = Stroke::new(1.0, pal::ACCENT());
        w.active.fg_stroke = Stroke::new(1.5, pal::TEXT_STRONG());
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = pal::BG_HOVER();
        w.open.weak_bg_fill = pal::BG_HOVER();
        w.open.bg_stroke = Stroke::new(1.0, pal::ACCENT_SOFT());
        w.open.fg_stroke = Stroke::new(1.0, pal::TEXT());
        w.open.corner_radius = r;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(item_color(MODEL_DEFAULT, 0), pal::SWATCHES()[0]);
        assert_eq!(item_color(MODEL_DEFAULT, 11), pal::SWATCHES()[1]);
        assert_eq!(item_color(mfc_core::Color(1, 2, 3), 0), Color32::from_rgb(1, 2, 3));
        assert_eq!(
            mix(Color32::BLACK, Color32::WHITE, 0.5),
            Color32::from_rgb(128, 128, 128)
        );
    }

    /// Rapporto di contrasto WCAG tra due colori.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn palettes_have_readable_text() {
        for (name, p) in [("scuro", &pal::DARK), ("chiaro", &pal::LIGHT)] {
            for bg in [p.bg_panel, p.bg_raised, p.bg_widget] {
                assert!(contrast(p.text, bg) >= 7.0, "{name}: testo");
                assert!(contrast(p.text_dim, bg) >= 4.0, "{name}: testo attenuato");
                assert!(contrast(p.text_faint, bg) >= 2.0, "{name}: testo debole");
            }
            assert!(contrast(p.lcd, p.bg_inset) >= 3.0, "{name}: LCD");
            assert!(contrast(p.accent, p.bg_panel) >= 2.3, "{name}: accento");
        }
        // Il chiaro è davvero chiaro, lo scuro davvero scuro.
        assert!(pal::LIGHT.bg_panel.r() > 0xc0 && pal::DARK.bg_panel.r() < 0x30);
    }

    #[test]
    fn fonts_load() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let f = fonts();
        assert!(f.families[&FontFamily::Proportional][0] == "inter");
    }
}
