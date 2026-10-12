//! Widget riutilizzabili: meter stereo, knob rotativo.

use egui::{pos2, vec2, Color32, Rect, Response, Sense, Stroke, Ui, Vec2};

/// Converte un picco lineare in frazione 0..1 su scala -60..+6 dB.
pub fn meter_fraction(peak: f32) -> f32 {
    if peak <= 1e-6 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 60.0) / 66.0).clamp(0.0, 1.0)
}

fn meter_color(f: f32) -> Color32 {
    if f > 60.0 / 66.0 {
        Color32::from_rgb(230, 70, 60)
    } else if f > 48.0 / 66.0 {
        Color32::from_rgb(230, 200, 70)
    } else {
        Color32::from_rgb(90, 200, 110)
    }
}

/// Meter stereo orizzontale (se `size.x >= size.y`) o verticale.
pub fn meter(ui: &mut Ui, left: f32, right: f32, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, Color32::from_gray(25));
    let horizontal = size.x >= size.y;
    for (i, v) in [left, right].into_iter().enumerate() {
        let f = meter_fraction(v);
        let r = if horizontal {
            let h = rect.height() / 2.0;
            Rect::from_min_size(pos2(rect.left(), rect.top() + i as f32 * h), vec2(rect.width() * f, h - 1.0))
        } else {
            let w = rect.width() / 2.0;
            Rect::from_min_max(pos2(rect.left() + i as f32 * w, rect.bottom() - rect.height() * f), pos2(rect.left() + (i as f32 + 1.0) * w - 1.0, rect.bottom()))
        };
        p.rect_filled(r, 0.0, meter_color(f));
    }
    resp.on_hover_text(format!("L {:.1} dB  R {:.1} dB", 20.0 * left.max(1e-6).log10(), 20.0 * right.max(1e-6).log10()))
}

/// Knob rotativo: trascina verticalmente, doppio click = `default`. Restituisce la risposta
/// (usare `drag_stopped()`/`double_clicked()` per committare il valore).
pub fn knob(ui: &mut Ui, value: &mut f32, min: f32, max: f32, default: f32, diameter: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(diameter, diameter), Sense::click_and_drag());
    if resp.dragged() {
        let delta = -resp.drag_delta().y / 150.0 * (max - min);
        *value = (*value + delta).clamp(min, max);
        resp.mark_changed();
    }
    if resp.double_clicked() {
        *value = default;
        resp.mark_changed();
    }
    let p = ui.painter_at(rect.expand(1.0));
    let c = rect.center();
    let r = diameter / 2.0 - 1.0;
    p.circle_filled(c, r, ui.visuals().widgets.inactive.bg_fill);
    p.circle_stroke(c, r, Stroke::new(1.0, ui.visuals().widgets.inactive.fg_stroke.color));
    let t = if max > min { (*value - min) / (max - min) } else { 0.0 };
    let angle = (-135.0 + 270.0 * t).to_radians();
    let tip = c + vec2(angle.sin(), -angle.cos()) * (r - 1.0);
    p.line_segment([c, tip], Stroke::new(2.0, Color32::from_rgb(255, 160, 60)));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_scale() {
        assert_eq!(meter_fraction(0.0), 0.0);
        assert!((meter_fraction(1.0) - 60.0 / 66.0).abs() < 1e-6);
        assert_eq!(meter_fraction(100.0), 1.0);
    }
}
