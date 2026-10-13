//! Widget dell'interfaccia: knob ad arco, LED, display LCD, icone vettoriali,
//! pulsanti a icona, intestazioni di pannello, meter a segmenti.

use std::f32::consts::{PI, TAU};

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2,
};

use crate::theme::{bold_font, mix, mono, pal};

// ------------------------------------------------------------------ meter

/// Converte un picco lineare in frazione 0..1 su scala -60..+6 dB.
pub fn meter_fraction(peak: f32) -> f32 {
    if peak <= 1e-6 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 60.0) / 66.0).clamp(0.0, 1.0)
}

fn meter_color(f: f32) -> Color32 {
    if f > 60.0 / 66.0 {
        pal::RED()
    } else if f > 48.0 / 66.0 {
        pal::YELLOW()
    } else {
        mix(Color32::from_rgb(0x3b, 0xa8, 0x5a), pal::GREEN(), f)
    }
}

/// Meter stereo a segmenti: orizzontale se `size.x >= size.y`, altrimenti verticale.
pub fn meter(ui: &mut Ui, left: f32, right: f32, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint_meter(ui, rect, left, right);
    resp.on_hover_text(format!(
        "L {:.1} dB   R {:.1} dB",
        20.0 * left.max(1e-6).log10(),
        20.0 * right.max(1e-6).log10()
    ))
}

/// Disegna un meter stereo nel rettangolo dato.
pub fn paint_meter(ui: &Ui, rect: Rect, left: f32, right: f32) {
    let p = ui.painter();
    p.rect_filled(rect, 2.0, pal::BG_INSET());
    let horizontal = rect.width() >= rect.height();
    let seg = 3.0;
    for (i, v) in [left, right].into_iter().enumerate() {
        let f = meter_fraction(v);
        let lane = if horizontal {
            let h = (rect.height() - 3.0) / 2.0;
            Rect::from_min_size(
                pos2(rect.left() + 1.0, rect.top() + 1.0 + i as f32 * (h + 1.0)),
                vec2(rect.width() - 2.0, h),
            )
        } else {
            let w = (rect.width() - 3.0) / 2.0;
            Rect::from_min_size(
                pos2(rect.left() + 1.0 + i as f32 * (w + 1.0), rect.top() + 1.0),
                vec2(w, rect.height() - 2.0),
            )
        };
        let len = if horizontal { lane.width() } else { lane.height() };
        let n = (len / seg).floor() as usize;
        let lit = (f * n as f32).round() as usize;
        for k in 0..n {
            let t = k as f32 / n.max(1) as f32;
            let on = k < lit;
            let c = if on {
                meter_color(t)
            } else {
                mix(pal::BG_INSET(), pal::BG_WIDGET(), 0.35)
            };
            let r = if horizontal {
                Rect::from_min_size(
                    pos2(lane.left() + k as f32 * seg, lane.top()),
                    vec2(seg - 1.0, lane.height()),
                )
            } else {
                Rect::from_min_size(
                    pos2(lane.left(), lane.bottom() - (k + 1) as f32 * seg + 1.0),
                    vec2(lane.width(), seg - 1.0),
                )
            };
            p.rect_filled(r, 0.0, c);
        }
    }
}

// ------------------------------------------------------------------ knob

/// Corsa del knob: lineare o logaritmica (frequenze, velocità).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KnobScale {
    #[default]
    Linear,
    /// Logaritmica: pura se `min > 0`, altrimenti con un offset (vedi [`knob_to_norm`]).
    Log,
}

/// Scala adatta a un parametro: logaritmica per le unità in Hz o per i nomi tipo
/// cutoff/freq/rate (purché l'intervallo sia positivo e ampio), e per i tempi degli
/// inviluppi (attack/decay/release/glide in s o ms); altrimenti lineare.
pub fn knob_scale_for(name: &str, unit: &str, min: f32, max: f32) -> KnobScale {
    let n = name.to_ascii_lowercase();
    let time = matches!(unit, "s" | "ms") && ["attack", "decay", "release", "glide"].iter().any(|k| n.contains(k));
    let freq_like = time || unit.eq_ignore_ascii_case("Hz") || ["cutoff", "freq", "rate"].iter().any(|k| n.contains(k));
    // Su intervalli stretti (es. 0..1 normalizzato) la corsa logaritmica non serve.
    let wide = min >= 0.0 && max > min && ((min == 0.0 && max >= 10.0) || (min > 0.0 && max / min >= 10.0));
    if freq_like && wide {
        KnobScale::Log
    } else {
        KnobScale::Linear
    }
}

/// Offset della curva logaritmica: `min` stesso se positivo (log puro), altrimenti
/// un millesimo dell'intervallo (tre decadi di escursione a partire da `min`).
fn log_offset(min: f32, max: f32) -> f32 {
    if min > 0.0 {
        min
    } else {
        ((max - min) / 1000.0).max(1e-6)
    }
}

/// Valore → posizione 0..1 lungo la corsa del knob.
pub fn knob_to_norm(value: f32, min: f32, max: f32, scale: KnobScale) -> f32 {
    if max == min {
        return 0.0;
    }
    let (lo, hi) = (min.min(max), max.max(min));
    let v = value.clamp(lo, hi);
    let t = match scale {
        KnobScale::Linear => (v - min) / (max - min),
        KnobScale::Log => {
            let c = log_offset(lo, hi);
            ((v - lo + c) / c).ln() / ((hi - lo + c) / c).ln()
        }
    };
    t.clamp(0.0, 1.0)
}

/// Posizione 0..1 → valore (inversa di [`knob_to_norm`]).
pub fn knob_from_norm(t: f32, min: f32, max: f32, scale: KnobScale) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match scale {
        KnobScale::Linear => min + (max - min) * t,
        KnobScale::Log => {
            let (lo, hi) = (min.min(max), max.max(min));
            if hi == lo {
                return lo;
            }
            let c = log_offset(lo, hi);
            let v = lo + c * (((hi - lo + c) / c).ln() * t).exp() - c;
            v.clamp(lo, hi)
        }
    }
}

/// Knob ad arco (trascina in verticale, Shift = fine, doppio click = default).
/// `bipolar` disegna l'arco dal centro (pan, pitch).
#[allow(clippy::too_many_arguments)]
pub fn knob_ex(
    ui: &mut Ui,
    value: &mut f32,
    min: f32,
    max: f32,
    default: f32,
    diameter: f32,
    bipolar: bool,
    color: Color32,
) -> Response {
    knob_scaled(
        ui,
        value,
        min,
        max,
        default,
        diameter,
        bipolar,
        color,
        KnobScale::Linear,
    )
}

/// Come [`knob_ex`], con corsa lineare o logaritmica.
#[allow(clippy::too_many_arguments)]
pub fn knob_scaled(
    ui: &mut Ui,
    value: &mut f32,
    min: f32,
    max: f32,
    default: f32,
    diameter: f32,
    bipolar: bool,
    color: Color32,
    scale: KnobScale,
) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(diameter, diameter), Sense::click_and_drag());
    if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.15 } else { 1.0 };
        let dt = -resp.drag_delta().y / 160.0 * fine;
        if dt != 0.0 {
            let t = knob_to_norm(*value, min, max, scale) + dt;
            *value = knob_from_norm(t, min, max, scale).clamp(min.min(max), max.max(min));
            resp.mark_changed();
        }
    }
    if resp.double_clicked() {
        *value = default;
        resp.mark_changed();
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let t = knob_to_norm(*value, min, max, scale);
    let bipolar = bipolar && scale == KnobScale::Linear;
    paint_knob(ui, rect, t, bipolar, color, resp.hovered() || resp.dragged());
    resp
}

/// Knob standard con colore d'accento.
pub fn knob(ui: &mut Ui, value: &mut f32, min: f32, max: f32, default: f32, diameter: f32) -> Response {
    let bipolar = min < 0.0 && max > 0.0;
    knob_ex(ui, value, min, max, default, diameter, bipolar, pal::ACCENT())
}

const KNOB_START: f32 = 0.75 * PI; // 135°
const KNOB_SWEEP: f32 = 1.5 * PI; // 270°

fn arc_points(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let n = (((a1 - a0).abs() / TAU) * 64.0).ceil().max(2.0) as usize;
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

/// Disegna il knob alla posizione `t` (0..1) della corsa.
fn paint_knob(ui: &Ui, rect: Rect, t: f32, bipolar: bool, color: Color32, hot: bool) {
    let p = ui.painter();
    let c = rect.center();
    let r = rect.width() / 2.0;
    let ring_w = (r * 0.18).max(2.0);
    let ring_r = r - ring_w / 2.0;
    // Traccia di fondo.
    p.add(Shape::line(
        arc_points(c, ring_r, KNOB_START, KNOB_START + KNOB_SWEEP),
        Stroke::new(ring_w, pal::BG_INSET()),
    ));
    // Arco del valore.
    let from: f32 = if bipolar { 0.5 } else { 0.0 };
    let (a0, a1) = (
        KNOB_START + KNOB_SWEEP * from.min(t),
        KNOB_START + KNOB_SWEEP * from.max(t),
    );
    if a1 - a0 > 0.01 {
        p.add(Shape::line(arc_points(c, ring_r, a0, a1), Stroke::new(ring_w, color)));
    }
    // Corpo.
    let body_r = r - ring_w - 1.5;
    p.circle_filled(c + vec2(0.0, 1.0), body_r, pal::SHADOW());
    p.circle_filled(c, body_r, if hot { pal::BG_HOVER() } else { pal::BG_WIDGET() });
    let rim = if pal::is_light() {
        pal::BORDER()
    } else {
        mix(pal::BG_HOVER(), Color32::WHITE, 0.08)
    };
    p.circle_stroke(c, body_r, Stroke::new(1.0, rim));
    // Indicatore.
    let a = KNOB_START + KNOB_SWEEP * t;
    let dir = vec2(a.cos(), a.sin());
    p.line_segment(
        [c + dir * body_r * 0.25, c + dir * (body_r - 1.0)],
        Stroke::new(2.0, if hot { pal::TEXT_STRONG() } else { pal::TEXT() }),
    );
}

/// Knob con etichetta sotto e valore in tooltip/hover. Restituisce la risposta del knob.
#[allow(clippy::too_many_arguments)]
pub fn labeled_knob(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    default: f32,
    diameter: f32,
    text: impl Fn(f32) -> String,
) -> Response {
    labeled_knob_scaled(ui, label, value, min, max, default, diameter, KnobScale::Linear, text)
}

/// Come [`labeled_knob`], con corsa lineare o logaritmica.
#[allow(clippy::too_many_arguments)]
pub fn labeled_knob_scaled(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    default: f32,
    diameter: f32,
    scale: KnobScale,
    text: impl Fn(f32) -> String,
) -> Response {
    let mut out = None;
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let bipolar = min < 0.0 && max > 0.0;
        let r = knob_scaled(ui, value, min, max, default, diameter, bipolar, pal::ACCENT(), scale);
        let shown = if r.hovered() || r.dragged() {
            text(*value)
        } else {
            label.to_string()
        };
        let color = if r.hovered() || r.dragged() {
            pal::ACCENT()
        } else {
            pal::TEXT_DIM()
        };
        ui.label(egui::RichText::new(shown).size(10.5).color(color));
        out = Some(r);
    });
    out.unwrap()
}

// ------------------------------------------------------------------ led / toggle

/// LED circolare cliccabile (mute di canale, abilitazione slot).
pub fn led(ui: &mut Ui, on: bool, color: Color32, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    let c = rect.center();
    let r = size * 0.36;
    if on {
        p.circle_filled(c, r * 1.9, color.gamma_multiply(0.18));
    }
    p.circle_filled(
        c,
        r,
        if on {
            color
        } else {
            mix(pal::BG_INSET(), pal::BG_HOVER(), 0.5)
        },
    );
    p.circle_stroke(c, r, Stroke::new(1.0, pal::BORDER()));
    if on {
        p.circle_filled(c - vec2(r * 0.3, r * 0.3), r * 0.35, Color32::from_white_alpha(110));
    }
    resp
}

/// Pulsante rettangolare con stato acceso/spento (mute, solo, PAT/SONG...).
pub fn toggle(ui: &mut Ui, on: bool, text: &str, on_color: Color32, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let hot = resp.hovered();
    let fill = if on {
        on_color
    } else if hot {
        pal::BG_HOVER()
    } else {
        pal::BG_WIDGET()
    };
    p.rect_filled(rect, 4.0, fill);
    p.rect_stroke(rect, 4.0, Stroke::new(1.0, pal::BORDER()), StrokeKind::Inside);
    let fg = if on {
        Color32::from_rgb(0x14, 0x14, 0x14)
    } else if hot {
        pal::TEXT_STRONG()
    } else {
        pal::TEXT_DIM()
    };
    p.text(rect.center(), Align2::CENTER_CENTER, text, bold_font(11.0), fg);
    resp
}

/// Gruppo segmentato a scelta singola; restituisce l'indice cliccato.
pub fn segmented(ui: &mut Ui, items: &[&str], selected: usize, height: f32) -> Option<usize> {
    let font = bold_font(11.0);
    let widths: Vec<f32> = items
        .iter()
        .map(|t| ui.fonts_mut(|f| f.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x) + 18.0)
        .collect();
    let total: f32 = widths.iter().sum();
    let (rect, _) = ui.allocate_exact_size(vec2(total, height), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, 5.0, pal::BG_INSET());
    let mut x = rect.left();
    let mut clicked = None;
    for (i, (t, w)) in items.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*w, height)).shrink(2.0);
        let resp = ui.interact(r, ui.id().with(("seg", i, *t)), Sense::click());
        let on = i == selected;
        if on {
            p.rect_filled(r, 4.0, pal::BG_HOVER());
            p.rect_filled(
                Rect::from_min_size(pos2(r.left() + 6.0, r.bottom() - 2.0), vec2(r.width() - 12.0, 2.0)),
                1.0,
                pal::ACCENT(),
            );
        } else if resp.hovered() {
            p.rect_filled(r, 4.0, pal::BG_WIDGET());
        }
        let fg = if on {
            pal::TEXT_STRONG()
        } else if resp.hovered() {
            pal::TEXT()
        } else {
            pal::TEXT_DIM()
        };
        p.text(r.center() - vec2(0.0, 0.5), Align2::CENTER_CENTER, *t, font.clone(), fg);
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

// ------------------------------------------------------------------ lcd

/// Display LCD incassato con testo monospazio arancione. Restituisce la risposta (cliccabile/trascinabile).
pub fn lcd(ui: &mut Ui, text: &str, caption: Option<&str>, width: f32, height: f32, sense: Sense) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), sense);
    let p = ui.painter();
    p.rect_filled(rect, 4.0, pal::BG_INSET());
    p.rect_stroke(
        rect,
        4.0,
        Stroke::new(
            1.0,
            if resp.hovered() {
                pal::ACCENT_SOFT()
            } else {
                pal::BORDER()
            },
        ),
        StrokeKind::Inside,
    );
    // riflesso superiore
    p.line_segment(
        [rect.left_top() + vec2(4.0, 1.5), rect.right_top() + vec2(-4.0, 1.5)],
        Stroke::new(1.0, Color32::from_white_alpha(8)),
    );
    let size = (height * 0.56).min(18.0);
    if let Some(cap) = caption {
        p.text(
            rect.left_top() + vec2(6.0, 3.0),
            Align2::LEFT_TOP,
            cap,
            FontId::proportional(8.5),
            pal::TEXT_FAINT(),
        );
        p.text(
            rect.right_center() + vec2(-7.0, 3.0),
            Align2::RIGHT_CENTER,
            text,
            mono(size),
            pal::LCD(),
        );
    } else {
        p.text(rect.center(), Align2::CENTER_CENTER, text, mono(size), pal::LCD());
    }
    resp
}

// ------------------------------------------------------------------ icone

/// Icone vettoriali disegnate con il painter (nitide a ogni scala).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Record,
    Plus,
    Close,
    Playlist,
    Rack,
    Piano,
    Mixer,
    Undo,
    Redo,
    Save,
    Folder,
    Loop,
    Metronome,
    Plugin,
    Wave,
    Arrow,
}

/// Disegna un'icona centrata in `rect`.
pub fn paint_icon(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let st = Stroke::new((s * 0.16).max(1.4), color);
    match icon {
        Icon::Play => {
            let pts = vec![
                c + vec2(-s * 0.45, -s * 0.6),
                c + vec2(s * 0.65, 0.0),
                c + vec2(-s * 0.45, s * 0.6),
            ];
            p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Pause => {
            for dx in [-0.32, 0.32] {
                p.rect_filled(
                    Rect::from_center_size(c + vec2(s * dx, 0.0), vec2(s * 0.34, s * 1.2)),
                    1.5,
                    color,
                );
            }
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(s * 1.1, s * 1.1)), 2.0, color);
        }
        Icon::Record => {
            p.circle_filled(c, s * 0.58, color);
        }
        Icon::Plus => {
            p.line_segment([c - vec2(s * 0.6, 0.0), c + vec2(s * 0.6, 0.0)], st);
            p.line_segment([c - vec2(0.0, s * 0.6), c + vec2(0.0, s * 0.6)], st);
        }
        Icon::Close => {
            p.line_segment([c - vec2(s * 0.5, s * 0.5), c + vec2(s * 0.5, s * 0.5)], st);
            p.line_segment([c + vec2(-s * 0.5, s * 0.5), c + vec2(s * 0.5, -s * 0.5)], st);
        }
        Icon::Playlist => {
            for (i, w) in [1.0f32, 0.6, 0.85].iter().enumerate() {
                let y = c.y - s * 0.55 + i as f32 * s * 0.55;
                let x0 = c.x - s * 0.7 + (i as f32 * 0.25) * s;
                p.rect_filled(
                    Rect::from_min_size(pos2(x0, y - s * 0.17), vec2(s * 1.4 * w * 0.75, s * 0.34)),
                    1.5,
                    color,
                );
            }
        }
        Icon::Rack => {
            for row in 0..3 {
                for col in 0..4 {
                    let on = (row + col) % 3 == 0;
                    let r = Rect::from_min_size(
                        pos2(
                            c.x - s * 0.75 + col as f32 * s * 0.4,
                            c.y - s * 0.6 + row as f32 * s * 0.42,
                        ),
                        vec2(s * 0.3, s * 0.32),
                    );
                    p.rect_filled(r, 1.0, if on { color } else { color.gamma_multiply(0.35) });
                }
            }
        }
        Icon::Piano => {
            let r = Rect::from_center_size(c, vec2(s * 1.5, s * 1.3));
            p.rect_stroke(r, 1.5, Stroke::new(st.width * 0.8, color), StrokeKind::Inside);
            for i in 1..4 {
                let x = r.left() + r.width() * i as f32 / 4.0;
                p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(1.0, color));
            }
            for i in [1.0f32, 2.0, 3.0] {
                let x = r.left() + r.width() * i / 4.0;
                p.rect_filled(
                    Rect::from_center_size(pos2(x, r.top() + r.height() * 0.3), vec2(s * 0.2, r.height() * 0.6)),
                    0.0,
                    color,
                );
            }
        }
        Icon::Mixer => {
            for (i, k) in [0.3f32, 0.65, 0.45].iter().enumerate() {
                let x = c.x - s * 0.55 + i as f32 * s * 0.55;
                p.line_segment(
                    [pos2(x, c.y - s * 0.7), pos2(x, c.y + s * 0.7)],
                    Stroke::new(1.2, color.gamma_multiply(0.6)),
                );
                let y = c.y - s * 0.7 + k * s * 1.4;
                p.rect_filled(Rect::from_center_size(pos2(x, y), vec2(s * 0.36, s * 0.22)), 1.0, color);
            }
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let pts = arc_points(c + vec2(0.0, s * 0.15), s * 0.5, -PI * 0.9, PI * 0.35)
                .into_iter()
                .map(|q| pos2(c.x + (q.x - c.x) * flip, q.y))
                .collect::<Vec<_>>();
            p.add(Shape::line(pts.clone(), st));
            let tip = pts[0];
            p.add(Shape::convex_polygon(
                vec![
                    tip + vec2(-s * 0.32 * flip, -s * 0.05),
                    tip + vec2(s * 0.12 * flip, -s * 0.38),
                    tip + vec2(s * 0.18 * flip, s * 0.2),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Save => {
            let r = Rect::from_center_size(c, vec2(s * 1.3, s * 1.3));
            p.rect_stroke(r, 2.0, st, StrokeKind::Inside);
            p.rect_filled(
                Rect::from_min_size(r.left_top() + vec2(s * 0.3, 0.0), vec2(s * 0.7, s * 0.4)),
                0.0,
                color,
            );
        }
        Icon::Folder => {
            let r = Rect::from_center_size(c + vec2(0.0, s * 0.08), vec2(s * 1.5, s * 1.1));
            p.rect_filled(
                Rect::from_min_size(r.left_top() - vec2(0.0, s * 0.18), vec2(s * 0.6, s * 0.3)),
                1.5,
                color,
            );
            p.rect_filled(r, 2.0, color);
        }
        Icon::Loop => {
            let pts = arc_points(c, s * 0.55, 0.3, TAU - 0.5);
            p.add(Shape::line(pts.clone(), st));
            let tip = *pts.last().unwrap();
            p.add(Shape::convex_polygon(
                vec![
                    tip + vec2(-s * 0.3, -s * 0.15),
                    tip + vec2(s * 0.25, -s * 0.25),
                    tip + vec2(0.0, s * 0.25),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Metronome => {
            let pts = vec![
                c + vec2(-s * 0.55, s * 0.7),
                c + vec2(-s * 0.2, -s * 0.7),
                c + vec2(s * 0.2, -s * 0.7),
                c + vec2(s * 0.55, s * 0.7),
            ];
            p.add(Shape::closed_line(pts, Stroke::new(st.width * 0.8, color)));
            p.line_segment([c + vec2(0.0, s * 0.45), c + vec2(s * 0.45, -s * 0.5)], st);
        }
        Icon::Plugin => {
            let r = Rect::from_center_size(c, vec2(s * 1.2, s * 1.2));
            p.rect_stroke(r, 3.0, st, StrokeKind::Inside);
            p.circle_filled(r.center(), s * 0.22, color);
        }
        Icon::Wave => {
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let t = i as f32 / 24.0;
                    pos2(
                        c.x - s * 0.8 + t * s * 1.6,
                        c.y - (t * TAU * 1.5).sin() * s * 0.5 * (1.0 - (t - 0.5).abs()),
                    )
                })
                .collect();
            p.add(Shape::line(pts, st));
        }
        Icon::Arrow => {
            let pts = vec![
                c + vec2(-s * 0.3, -s * 0.5),
                c + vec2(s * 0.35, 0.0),
                c + vec2(-s * 0.3, s * 0.5),
            ];
            p.add(Shape::line(pts, st));
        }
    }
}

/// Pulsante a icona quadrato. `active` lo evidenzia con il colore indicato.
pub fn icon_button(ui: &mut Ui, icon: Icon, active: bool, active_color: Color32, size: f32, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    let hot = resp.hovered();
    let fill = if active {
        mix(pal::BG_WIDGET(), active_color, 0.22)
    } else if hot {
        pal::BG_HOVER()
    } else {
        pal::BG_WIDGET()
    };
    p.rect_filled(rect, 5.0, fill);
    p.rect_stroke(
        rect,
        5.0,
        Stroke::new(
            1.0,
            if active {
                active_color.gamma_multiply(0.7)
            } else {
                pal::BORDER()
            },
        ),
        StrokeKind::Inside,
    );
    let col = if active {
        active_color
    } else if hot {
        pal::TEXT_STRONG()
    } else {
        pal::TEXT()
    };
    paint_icon(p, rect.shrink(size * 0.24), icon, col);
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

// ------------------------------------------------------------------ pannelli

/// Altezza standard delle barre d'intestazione dei pannelli.
pub const HEADER_H: f32 = 32.0;

/// Barra d'intestazione di una vista: icona + titolo a sinistra, contenuti a destra.
pub fn view_header(ui: &mut Ui, icon: Icon, title: &str, subtitle: &str, add: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius {
            nw: 6,
            ne: 6,
            sw: 0,
            se: 0,
        },
        pal::BG_RAISED(),
    );
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, pal::BORDER()),
    );
    let ir = Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(16.0, 16.0));
    paint_icon(p, ir, icon, pal::ACCENT());
    let t = p.text(
        pos2(rect.left() + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        bold_font(13.0),
        pal::TEXT(),
    );
    if !subtitle.is_empty() {
        p.text(
            pos2(t.right() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            subtitle,
            FontId::proportional(11.5),
            pal::TEXT_DIM(),
        );
    }
    let right = Rect::from_min_max(
        pos2(rect.left() + 260.0, rect.top()),
        rect.right_bottom() - vec2(8.0, 0.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    add(&mut child);
}

/// Cornice di un pannello-vista (sfondo + bordo arrotondato).
pub fn view_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(pal::BG_PANEL())
        .stroke(Stroke::new(1.0, pal::BORDER()))
        .corner_radius(6)
        .inner_margin(0)
}

/// Etichetta di sezione (maiuscoletto, attenuata).
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(bold_font(10.0))
            .color(pal::TEXT_FAINT()),
    );
}

/// Rettangolo arrotondato con leggero gradiente verticale (simulato con due metà).
pub fn gradient_rect(p: &egui::Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    if radius <= 0.0 {
        p.add(Shape::mesh(mesh));
    } else {
        // Angoli arrotondati: riempimento base + gradiente nell'area interna.
        p.rect_filled(rect, radius, mix(top, bottom, 0.5));
        let inner = rect.shrink2(vec2(0.0, radius));
        let mut m = egui::Mesh::default();
        m.colored_vertex(inner.left_top(), mix(top, bottom, radius / rect.height()));
        m.colored_vertex(inner.right_top(), mix(top, bottom, radius / rect.height()));
        m.colored_vertex(inner.left_bottom(), mix(top, bottom, 1.0 - radius / rect.height()));
        m.colored_vertex(inner.right_bottom(), mix(top, bottom, 1.0 - radius / rect.height()));
        m.add_triangle(0, 1, 2);
        m.add_triangle(1, 3, 2);
        p.add(Shape::mesh(m));
    }
}

/// Id stabile per stato temporaneo dei widget.
pub fn tmp_id(base: &str, key: impl std::hash::Hash + std::fmt::Debug) -> Id {
    Id::new((base, key))
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

    const EPS: f32 = 1e-3;

    #[test]
    fn knob_scale_detection() {
        assert_eq!(knob_scale_for("cutoff", "Hz", 20.0, 20_000.0), KnobScale::Log);
        assert_eq!(
            knob_scale_for("flt_cutoff", "", 0.0, 1.0),
            KnobScale::Linear,
            "normalizzato"
        );
        assert_eq!(knob_scale_for("lfo1_rate", "Hz", 0.01, 20.0), KnobScale::Log);
        assert_eq!(knob_scale_for("freq", "", 0.0, 10_000.0), KnobScale::Log);
        assert_eq!(knob_scale_for("gain", "dB", -24.0, 24.0), KnobScale::Linear);
        assert_eq!(knob_scale_for("detune", "Hz", -50.0, 50.0), KnobScale::Linear);
        assert_eq!(knob_scale_for("aenv_attack", "s", 0.0, 10.0), KnobScale::Log);
        assert_eq!(knob_scale_for("release", "ms", 5.0, 2000.0), KnobScale::Log);
        assert_eq!(knob_scale_for("aenv_sustain", "", 0.0, 1.0), KnobScale::Linear);
        assert_eq!(
            knob_scale_for("decay", "", 0.0, 1.0),
            KnobScale::Linear,
            "senza unità di tempo"
        );
    }

    #[test]
    fn knob_mapping_endpoints() {
        for (min, max) in [(20.0f32, 20_000.0f32), (0.0, 1000.0), (0.01, 20.0), (-1.0, 1.0)] {
            for scale in [KnobScale::Linear, KnobScale::Log] {
                assert!(knob_to_norm(min, min, max, scale).abs() < EPS, "{min}..{max} {scale:?}");
                assert!((knob_to_norm(max, min, max, scale) - 1.0).abs() < EPS);
                assert!((knob_from_norm(0.0, min, max, scale) - min).abs() < EPS * (max - min));
                assert!((knob_from_norm(1.0, min, max, scale) - max).abs() < EPS * (max - min));
                // Fuori corsa: limitato.
                assert_eq!(knob_to_norm(max * 10.0 + 1.0, min, max, scale), 1.0);
                assert_eq!(knob_to_norm(min - 5.0, min, max, scale), 0.0);
            }
        }
        // Intervallo degenere.
        assert_eq!(knob_to_norm(5.0, 5.0, 5.0, KnobScale::Log), 0.0);
        assert_eq!(knob_from_norm(0.7, 5.0, 5.0, KnobScale::Log), 5.0);
    }

    #[test]
    fn knob_mapping_round_trip_and_monotonic() {
        for (min, max) in [(20.0f32, 20_000.0f32), (0.0, 18_000.0), (0.05, 30.0)] {
            let mut prev = -1.0;
            for i in 0..=200 {
                let t = i as f32 / 200.0;
                let v = knob_from_norm(t, min, max, KnobScale::Log);
                assert!(v > prev || i == 0, "monotona: {min}..{max} t={t}");
                prev = v;
                let back = knob_to_norm(v, min, max, KnobScale::Log);
                assert!((back - t).abs() < EPS, "{min}..{max}: {t} -> {v} -> {back}");
            }
        }
    }

    #[test]
    fn log_knob_is_musical() {
        // Log puro con min > 0: il centro corsa è la media geometrica (632 Hz).
        let mid = knob_from_norm(0.5, 20.0, 20_000.0, KnobScale::Log);
        assert!((mid - (20.0f32 * 20_000.0).sqrt()).abs() < 1.0, "{mid}");
        // 1 kHz cade oltre metà corsa, molto prima che sulla scala lineare.
        let t = knob_to_norm(1000.0, 20.0, 20_000.0, KnobScale::Log);
        assert!(t > 0.5 && t < 0.6, "{t}");
        // Con min = 0 la curva parte da 0 ma dedica comunque metà corsa alle basse frequenze.
        let low = knob_from_norm(0.5, 0.0, 20_000.0, KnobScale::Log);
        assert!(low > 0.0 && low < 2000.0, "{low}");
    }

    #[test]
    fn arc_has_points() {
        let pts = arc_points(Pos2::ZERO, 10.0, 0.0, PI);
        assert!(pts.len() >= 3);
        assert!((pts[0].x - 10.0).abs() < 1e-4);
    }
}
