//! Sistema visivo: palette, font e stile egui di Mixfonocraft.
//!
//! Estetica da DAW professionale: superfici grafite a più livelli di profondità,
//! un solo accento arancione per stato attivo/selezione, colori per canale.

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Theme};

/// Palette dell'interfaccia.
pub mod pal {
    use egui::Color32;

    /// Sfondo dell'applicazione (dietro ai pannelli).
    pub const BG_APP: Color32 = Color32::from_rgb(0x10, 0x12, 0x15);
    /// Pannelli principali.
    pub const BG_PANEL: Color32 = Color32::from_rgb(0x19, 0x1c, 0x20);
    /// Superfici rialzate (header, finestre, strip).
    pub const BG_RAISED: Color32 = Color32::from_rgb(0x22, 0x26, 0x2b);
    /// Widget a riposo.
    pub const BG_WIDGET: Color32 = Color32::from_rgb(0x2b, 0x30, 0x36);
    /// Widget in hover.
    pub const BG_HOVER: Color32 = Color32::from_rgb(0x36, 0x3c, 0x43);
    /// Aree incassate (display, tracce, griglie).
    pub const BG_INSET: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x10);
    /// Bordi scuri.
    pub const BORDER: Color32 = Color32::from_rgb(0x08, 0x09, 0x0b);
    /// Linee di separazione chiare.
    pub const LINE: Color32 = Color32::from_rgb(0x30, 0x35, 0x3c);
    pub const TEXT: Color32 = Color32::from_rgb(0xe2, 0xe6, 0xea);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(0x92, 0x9a, 0xa3);
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x5d, 0x64, 0x6c);
    /// Accento principale.
    pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x1e);
    pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xc2, 0x69, 0x17);
    pub const GREEN: Color32 = Color32::from_rgb(0x5c, 0xd6, 0x7a);
    pub const RED: Color32 = Color32::from_rgb(0xff, 0x55, 0x55);
    pub const YELLOW: Color32 = Color32::from_rgb(0xff, 0xd1, 0x66);
    pub const BLUE: Color32 = Color32::from_rgb(0x5a, 0xb8, 0xff);
    /// Colore delle cifre nei display LCD.
    pub const LCD: Color32 = Color32::from_rgb(0xff, 0xa4, 0x4a);

    /// Colori assegnati automaticamente a canali e pattern.
    pub const SWATCHES: [Color32; 10] = [
        Color32::from_rgb(0xff, 0x8a, 0x1e),
        Color32::from_rgb(0x5a, 0xb8, 0xff),
        Color32::from_rgb(0x8f, 0xd9, 0x5b),
        Color32::from_rgb(0xff, 0x6b, 0x8b),
        Color32::from_rgb(0xb3, 0x8c, 0xff),
        Color32::from_rgb(0x3f, 0xd1, 0xc4),
        Color32::from_rgb(0xff, 0xd1, 0x66),
        Color32::from_rgb(0xff, 0x7a, 0x50),
        Color32::from_rgb(0x6f, 0x9b, 0xff),
        Color32::from_rgb(0xe0, 0x7c, 0xe6),
    ];
}

/// Colore di default del modello (non personalizzato).
const MODEL_DEFAULT: mfc_core::Color = mfc_core::Color(0x5f, 0x75, 0x81);

/// Colore di visualizzazione: quello scelto dall'utente o uno della palette per indice.
pub fn item_color(c: mfc_core::Color, index: usize) -> Color32 {
    if c == MODEL_DEFAULT {
        pal::SWATCHES[index % pal::SWATCHES.len()]
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

/// Applica font, colori e metriche al contesto.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(Theme::Dark);
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
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = pal::BG_PANEL;
        v.window_fill = pal::BG_RAISED;
        v.window_stroke = Stroke::new(1.0, pal::BORDER);
        v.window_corner_radius = CornerRadius::same(8);
        v.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.menu_corner_radius = CornerRadius::same(6);
        v.extreme_bg_color = pal::BG_INSET;
        v.faint_bg_color = pal::BG_RAISED;
        v.code_bg_color = pal::BG_INSET;
        v.text_edit_bg_color = Some(pal::BG_INSET);
        v.hyperlink_color = pal::BLUE;
        v.warn_fg_color = pal::YELLOW;
        v.error_fg_color = pal::RED;
        v.selection.bg_fill = pal::ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, pal::ACCENT);
        v.slider_trailing_fill = true;
        v.collapsing_header_frame = false;
        v.indent_has_left_vline = false;
        v.striped = false;

        let r = CornerRadius::same(4);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = pal::BG_PANEL;
        w.noninteractive.weak_bg_fill = pal::BG_PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, pal::LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, pal::TEXT_DIM);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = pal::BG_WIDGET;
        w.inactive.weak_bg_fill = pal::BG_WIDGET;
        w.inactive.bg_stroke = Stroke::new(1.0, pal::BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, pal::TEXT);
        w.inactive.corner_radius = r;
        w.inactive.expansion = 0.0;

        w.hovered.bg_fill = pal::BG_HOVER;
        w.hovered.weak_bg_fill = pal::BG_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, pal::ACCENT_SOFT);
        w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = mix(pal::BG_HOVER, pal::ACCENT, 0.25);
        w.active.weak_bg_fill = mix(pal::BG_HOVER, pal::ACCENT, 0.25);
        w.active.bg_stroke = Stroke::new(1.0, pal::ACCENT);
        w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = pal::BG_HOVER;
        w.open.weak_bg_fill = pal::BG_HOVER;
        w.open.bg_stroke = Stroke::new(1.0, pal::ACCENT_SOFT);
        w.open.fg_stroke = Stroke::new(1.0, pal::TEXT);
        w.open.corner_radius = r;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(item_color(MODEL_DEFAULT, 0), pal::SWATCHES[0]);
        assert_eq!(item_color(MODEL_DEFAULT, 11), pal::SWATCHES[1]);
        assert_eq!(item_color(mfc_core::Color(1, 2, 3), 0), Color32::from_rgb(1, 2, 3));
        assert_eq!(
            mix(Color32::BLACK, Color32::WHITE, 0.5),
            Color32::from_rgb(128, 128, 128)
        );
    }

    #[test]
    fn fonts_load() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let f = fonts();
        assert!(f.families[&FontFamily::Proportional][0] == "inter");
    }
}
