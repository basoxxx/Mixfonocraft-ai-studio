//! Mixer: una striscia per insert (meter, fader, pan, mute/solo, instradamento) e, sotto,
//! il pannello degli effetti e delle mandate dell'insert selezionato.

use std::collections::BTreeSet;

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, RichText, Sense, Stroke, StrokeKind, Ui};
use mfc_core::{fader_gain, Command, MixerInsert, Project, Route, FX_SLOTS, MASTER, MIXER_INSERTS, MIXER_UNITY};
use mfc_plugins::{default_instance, descriptor, registry, PluginRole};

use crate::state::{PluginTarget, Studio};
use crate::theme::{self, pal};
use crate::views::channel_rack::{db_text, live, settle};
use crate::views::widgets::{self, Icon};

const STRIP_W: f32 = 80.0;
/// Insert vuoti mostrati in coda a quelli usati.
const EXTRA_EMPTY: usize = 4;
const FADER_MAX: f32 = 1.25;

/// `true` se l'insert ha qualcosa di diverso dal default (nome, effetti, mandate, livelli).
fn is_touched(index: u16, ins: &MixerInsert) -> bool {
    let def = MixerInsert::new(index);
    ins.name != def.name
        || ins.color != def.color
        || !ins.effects.is_empty()
        || ins.routes != def.routes
        || (ins.volume - MIXER_UNITY).abs() > 1e-6
        || ins.pan.abs() > 1e-6
        || ins.stereo_separation.abs() > 1e-6
        || ins.muted
        || ins.solo
        || ins.phase_invert
        || ins.swap_lr
}

/// Indici degli insert da mostrare (senza master): quelli usati da canali, effetti, mandate
/// o modificati, l'insert selezionato e alcuni vuoti. Con `show_all` tutti.
pub fn visible_inserts(project: &Project, selected: u16, show_all: bool) -> Vec<u16> {
    if show_all {
        return (1..=MIXER_INSERTS as u16).collect();
    }
    let mut set: BTreeSet<u16> = BTreeSet::new();
    set.insert(selected);
    for c in &project.channels {
        set.insert(c.mixer_insert);
    }
    for (i, ins) in project.mixer.inserts.iter().enumerate().skip(1) {
        if is_touched(i as u16, ins) {
            set.insert(i as u16);
        }
        set.extend(ins.routes.iter().map(|r| r.target));
    }
    let mut empty_left = EXTRA_EMPTY;
    for i in 1..=MIXER_INSERTS as u16 {
        if empty_left == 0 {
            break;
        }
        if set.insert(i) {
            empty_left -= 1;
        }
    }
    set.into_iter().filter(|&i| i != MASTER).collect()
}

/// Comando `SetInsert` che modifica solo i livelli/flag indicati.
fn insert_cmd(insert: u16, volume: Option<f32>, pan: Option<f32>, muted: Option<bool>, solo: Option<bool>) -> Command {
    Command::SetInsert {
        insert,
        name: None,
        volume,
        pan,
        stereo_separation: None,
        muted,
        solo,
        phase_invert: None,
        swap_lr: None,
    }
}

/// Dati copiati di una striscia.
struct Strip {
    index: u16,
    name: String,
    color: Color32,
    volume: f32,
    pan: f32,
    muted: bool,
    solo: bool,
    fx: Vec<bool>,
    /// L'insert selezionato instrada verso questa striscia.
    routed_from_selected: bool,
    selected: bool,
}

/// Colore di un insert: quello del primo canale che vi è instradato.
fn insert_color(project: &Project, index: u16) -> Color32 {
    if index == MASTER {
        return pal::ACCENT;
    }
    project
        .channels
        .iter()
        .enumerate()
        .find(|(_, c)| c.mixer_insert == index)
        .map(|(i, c)| theme::item_color(c.color, i))
        .unwrap_or(Color32::from_rgb(0x55, 0x5d, 0x66))
}

fn strips(project: &Project, selected: u16, visible: &[u16]) -> Vec<Strip> {
    let sel_routes: &[Route] = project.insert(selected).map_or(&[], |i| i.routes.as_slice());
    let mut out = Vec::new();
    for &i in std::iter::once(&MASTER).chain(visible) {
        if let Some(ins) = project.insert(i) {
            out.push(Strip {
                index: i,
                name: if i == MASTER { "Master".into() } else { ins.name.clone() },
                color: insert_color(project, i),
                volume: if i == MASTER { project.master_volume } else { ins.volume },
                pan: ins.pan,
                muted: ins.muted,
                solo: ins.solo,
                fx: (0..FX_SLOTS)
                    .map(|k| ins.effects.get(k).is_some_and(|e| e.enabled))
                    .collect(),
                routed_from_selected: i != MASTER && sel_routes.iter().any(|r| r.target == i),
                selected: selected == i,
            });
        }
    }
    out
}

const FX_COL_W: f32 = 300.0;

/// Disegna il mixer.
pub fn show(ui: &mut Ui, studio: &mut Studio) {
    let show_all_id = Id::new("mixer_show_all");
    let mut show_all = ui.ctx().data(|d| d.get_temp::<bool>(show_all_id)).unwrap_or(false);
    let selected = studio.selected_insert;
    let visible = visible_inserts(&studio.project, selected, show_all);
    widgets::view_header(
        ui,
        Icon::Mixer,
        "Mixer",
        &format!("{} insert visibili", visible.len()),
        |ui| {
            if widgets::toggle(ui, show_all, "TUTTI", pal::BLUE, vec2(56.0, 22.0))
                .on_hover_text("Mostra tutti i 125 insert")
                .clicked()
            {
                show_all = !show_all;
                ui.ctx().data_mut(|d| d.insert_temp(show_all_id, show_all));
            }
        },
    );
    let strips = strips(&studio.project, selected, &visible);
    let avail = ui.available_rect_before_wrap();
    let strips_rect = Rect::from_min_max(avail.min, pos2(avail.right() - FX_COL_W, avail.bottom()));
    let fx_rect = Rect::from_min_max(pos2(avail.right() - FX_COL_W, avail.top()), avail.max);

    let mut left = ui.new_child(egui::UiBuilder::new().max_rect(strips_rect));
    egui::ScrollArea::horizontal()
        .id_salt("mixer_strips")
        .auto_shrink([false, false])
        .show(&mut left, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                let h = strips_rect.height() - 20.0;
                for s in &strips {
                    strip_ui(ui, studio, s, h);
                    if s.index == MASTER {
                        ui.add_space(8.0);
                    }
                }
            });
        });

    let p = ui.painter();
    p.rect_filled(
        fx_rect,
        egui::CornerRadius {
            se: 6,
            ..Default::default()
        },
        pal::BG_RAISED,
    );
    p.line_segment(
        [fx_rect.left_top(), fx_rect.left_bottom()],
        Stroke::new(1.0, pal::BORDER),
    );
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(fx_rect.shrink(10.0)));
    egui::ScrollArea::vertical()
        .id_salt("mixer_detail")
        .auto_shrink([false, false])
        .show(&mut right, |ui| detail_ui(ui, studio));
    ui.advance_cursor_after_rect(avail);
}

fn strip_ui(ui: &mut Ui, studio: &mut Studio, s: &Strip, h: f32) {
    let w = if s.index == MASTER { STRIP_W + 18.0 } else { STRIP_W };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if resp.clicked() {
        studio.selected_insert = s.index;
    }
    let p = ui.painter().clone();
    let bg = if s.selected {
        theme::mix(pal::BG_RAISED, s.color, 0.10)
    } else {
        pal::BG_RAISED
    };
    p.rect_filled(rect, 5.0, bg);
    p.rect_stroke(
        rect,
        5.0,
        Stroke::new(1.0, if s.selected { s.color } else { pal::BORDER }),
        StrokeKind::Inside,
    );
    // Intestazione: numero + nome + banda colore.
    let label = if s.index == MASTER {
        "M".to_string()
    } else {
        s.index.to_string()
    };
    p.text(
        pos2(rect.center().x, rect.top() + 10.0),
        Align2::CENTER_CENTER,
        label,
        theme::bold_font(10.0),
        pal::TEXT_FAINT,
    );
    p.with_clip_rect(rect.shrink(3.0)).text(
        pos2(rect.center().x, rect.top() + 25.0),
        Align2::CENTER_CENTER,
        &s.name,
        theme::bold_font(11.0),
        if s.muted { pal::TEXT_FAINT } else { pal::TEXT },
    );
    p.rect_filled(
        Rect::from_min_size(pos2(rect.left() + 6.0, rect.top() + 35.0), vec2(w - 12.0, 3.0)),
        1.5,
        s.color,
    );
    // Indicatori slot FX.
    let dot_w = (w - 12.0) / FX_SLOTS as f32;
    for (k, on) in s.fx.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.left() + 6.0 + k as f32 * dot_w, rect.top() + 43.0),
            vec2(dot_w - 2.0, 4.0),
        );
        p.rect_filled(r, 1.0, if *on { pal::GREEN } else { pal::BG_INSET });
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                rect.min + vec2(4.0, 52.0),
                rect.max - vec2(4.0, 6.0),
            ))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 6.0;
    if s.index != MASTER {
        pan_knob(ui, studio, s);
    } else {
        ui.add_space(30.0);
    }
    // Area fader + meter.
    let bottom_h = if s.index == MASTER { 30.0 } else { 86.0 };
    let fader_h = (ui.available_height() - bottom_h).max(80.0);
    let (area, _) = ui.allocate_exact_size(vec2(w - 10.0, fader_h), Sense::hover());
    let (l, r) = studio.engine.shared.take_meter(usize::from(s.index));
    let meter_rect = Rect::from_min_size(pos2(area.right() - 14.0, area.top()), vec2(12.0, area.height()));
    widgets::paint_meter(ui, meter_rect, l, r);
    let fader_rect = Rect::from_min_max(area.min, pos2(meter_rect.left() - 4.0, area.bottom()));
    fader(ui, studio, s, fader_rect);
    let gain = fader_gain(live(ui.ctx(), Id::new(("mixer_fader", s.index)), s.volume), MIXER_UNITY);
    widgets::lcd(
        ui,
        &db_text(gain).replace(" dB", ""),
        None,
        w - 14.0,
        20.0,
        Sense::hover(),
    );
    if s.index == MASTER {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let bw = (w - 14.0) / 2.0 - 2.0;
        if widgets::toggle(ui, s.muted, "M", pal::RED, vec2(bw, 20.0))
            .on_hover_text("Mute")
            .clicked()
        {
            studio.exec(insert_cmd(s.index, None, None, Some(!s.muted), None));
        }
        if widgets::toggle(ui, s.solo, "S", pal::YELLOW, vec2(bw, 20.0))
            .on_hover_text("Solo")
            .clicked()
        {
            studio.exec(insert_cmd(s.index, None, None, None, Some(!s.solo)));
        }
    });
    let sel = studio.selected_insert;
    if !s.selected {
        let r = widgets::toggle(ui, s.routed_from_selected, "ROUTE", pal::ACCENT, vec2(w - 14.0, 20.0))
            .on_hover_text(format!("Instrada l'insert {sel} verso questo insert"));
        if r.clicked() {
            let gain = if s.routed_from_selected { None } else { Some(1.0) };
            studio.exec(Command::SetRoute {
                from: sel,
                to: s.index,
                gain,
            });
        }
    } else {
        widgets::toggle(
            ui,
            true,
            "SELEZ.",
            theme::mix(pal::BG_WIDGET, s.color, 0.6),
            vec2(w - 14.0, 20.0),
        );
    }
}

/// Curva "audio taper": posizione 0..1 del cappuccio per un valore di fader.
pub fn fader_pos(v: f32) -> f32 {
    (v / FADER_MAX).clamp(0.0, 1.0).sqrt()
}

/// Inversa di [`fader_pos`].
pub fn fader_value(pos: f32) -> f32 {
    pos.clamp(0.0, 1.0).powi(2) * FADER_MAX
}

/// Fader verticale con scala in dB e cappuccio trascinabile.
fn fader(ui: &mut Ui, studio: &mut Studio, s: &Strip, rect: Rect) {
    let id = Id::new(("mixer_fader", s.index));
    let mut v = live(ui.ctx(), id, s.volume);
    let mut resp = ui.interact(rect, id.with("i"), Sense::click_and_drag());
    let track_top = rect.top() + 8.0;
    let track_bot = rect.bottom() - 8.0;
    let y_of = |val: f32| track_bot - fader_pos(val) * (track_bot - track_top);
    if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.2 } else { 1.0 };
        let f = (fader_pos(v) - resp.drag_delta().y / (track_bot - track_top) * fine).clamp(0.0, 1.0);
        v = fader_value(f);
        resp.mark_changed();
    }
    if resp.double_clicked() {
        v = MIXER_UNITY;
        resp.mark_changed();
    }
    let p = ui.painter();
    let cx = rect.left() + rect.width() * 0.62;
    // Scala dB.
    for db in [6.0f32, 0.0, -6.0, -12.0, -24.0, -48.0] {
        let val = 10f32.powf(db / 20.0) * MIXER_UNITY;
        if val > FADER_MAX {
            continue;
        }
        let y = y_of(val);
        p.line_segment(
            [pos2(cx - 9.0, y), pos2(cx - 5.0, y)],
            Stroke::new(1.0, pal::TEXT_FAINT),
        );
        let t = if db == 0.0 {
            "0".to_string()
        } else {
            format!("{db:+.0}")
        };
        p.text(
            pos2(cx - 11.0, y),
            Align2::RIGHT_CENTER,
            t,
            FontId::proportional(8.5),
            if db == 0.0 { pal::TEXT_DIM } else { pal::TEXT_FAINT },
        );
    }
    // Binario.
    let track = Rect::from_center_size(
        pos2(cx, (track_top + track_bot) / 2.0),
        vec2(4.0, track_bot - track_top),
    );
    p.rect_filled(track, 2.0, pal::BG_INSET);
    let fill = Rect::from_min_max(pos2(track.left(), y_of(v)), track.max);
    p.rect_filled(fill, 2.0, theme::mix(s.color, pal::BG_INSET, 0.45));
    // Cappuccio.
    let cap = Rect::from_center_size(pos2(cx, y_of(v)), vec2(26.0, 14.0));
    let hot = resp.hovered() || resp.dragged();
    widgets::gradient_rect(
        p,
        cap,
        3.0,
        if hot {
            Color32::from_rgb(0x6a, 0x72, 0x7b)
        } else {
            Color32::from_rgb(0x5a, 0x61, 0x69)
        },
        Color32::from_rgb(0x34, 0x39, 0x3f),
    );
    p.rect_stroke(cap, 3.0, Stroke::new(1.0, pal::BORDER), StrokeKind::Inside);
    p.line_segment(
        [
            pos2(cap.left() + 4.0, cap.center().y),
            pos2(cap.right() - 4.0, cap.center().y),
        ],
        Stroke::new(1.5, s.color),
    );
    if let Some(nv) = settle(ui.ctx(), id, &resp, v, s.volume) {
        if s.index == MASTER {
            studio.exec(Command::SetMasterVolume { volume: nv });
        } else {
            studio.exec(insert_cmd(s.index, Some(nv), None, None, None));
        }
    }
    resp.on_hover_text(format!(
        "Volume {} (doppio click = 0 dB, Shift = fine)",
        db_text(fader_gain(v, MIXER_UNITY))
    ));
}

fn pan_knob(ui: &mut Ui, studio: &mut Studio, s: &Strip) {
    let id = Id::new(("mixer_pan", s.index));
    let mut v = live(ui.ctx(), id, s.pan);
    let r = widgets::knob_ex(ui, &mut v, -1.0, 1.0, 0.0, 28.0, true, pal::BLUE);
    if let Some(nv) = settle(ui.ctx(), id, &r, v, s.pan) {
        studio.exec(insert_cmd(s.index, None, Some(nv), None, None));
    }
    let side = if v.abs() < 0.01 {
        "C".to_string()
    } else if v < 0.0 {
        format!("{:.0}% L", -v * 100.0)
    } else {
        format!("{:.0}% R", v * 100.0)
    };
    r.on_hover_text(format!("Pan {side}"));
}

/// Nome mostrato di un plugin: descrittore, altrimenti nome esterno o id.
fn plugin_label(kind: &str, foreign: Option<&str>) -> String {
    descriptor(kind).map(|d| d.name).or(foreign).unwrap_or(kind).to_string()
}

fn detail_ui(ui: &mut Ui, studio: &mut Studio) {
    let sel = studio.selected_insert;
    let Some(ins) = studio.project.insert(sel).cloned() else {
        ui.label("Insert non valido.");
        return;
    };
    let color = insert_color(&studio.project, sel);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(4.0, 30.0), Sense::hover());
        ui.painter().rect_filled(r, 2.0, color);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let num = if sel == MASTER {
                "MASTER".to_string()
            } else {
                format!("INSERT {sel}")
            };
            ui.label(RichText::new(num).font(theme::bold_font(9.5)).color(pal::TEXT_FAINT));
            let name = if sel == MASTER {
                "Master".to_string()
            } else {
                ins.name.clone()
            };
            ui.label(RichText::new(name).font(theme::bold_font(15.0)).color(pal::TEXT));
        });
    });
    ui.add_space(8.0);
    widgets::section_label(ui, "Effetti");
    ui.spacing_mut().item_spacing.y = 3.0;
    for slot in 0..FX_SLOTS {
        fx_slot(ui, studio, sel, slot, &ins);
    }

    ui.add_space(10.0);
    widgets::section_label(ui, "Mandate");
    if ins.routes.is_empty() {
        ui.label(
            RichText::new(if sel == MASTER {
                "Uscita audio"
            } else {
                "Nessuna mandata"
            })
            .color(pal::TEXT_FAINT),
        );
    }
    for r in &ins.routes {
        ui.horizontal(|ui| {
            let target_name = studio
                .project
                .insert(r.target)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            let tc = insert_color(&studio.project, r.target);
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().rect_filled(dot, 2.0, tc);
            ui.label(
                RichText::new(format!(
                    "{}  {target_name}",
                    if r.target == MASTER {
                        "M".to_string()
                    } else {
                        r.target.to_string()
                    }
                ))
                .color(pal::TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::Close, false, pal::RED, 20.0, "Rimuovi mandata").clicked() {
                    studio.exec(Command::SetRoute {
                        from: sel,
                        to: r.target,
                        gain: None,
                    });
                }
                let id = Id::new(("route_gain", sel, r.target));
                let mut g = live(ui.ctx(), id, r.gain);
                let resp = widgets::knob_ex(ui, &mut g, 0.0, 2.0, 1.0, 20.0, false, tc);
                if let Some(nv) = settle(ui.ctx(), id, &resp, g, r.gain) {
                    studio.exec(Command::SetRoute {
                        from: sel,
                        to: r.target,
                        gain: Some(nv),
                    });
                }
                resp.on_hover_text(format!("Livello mandata {}", db_text(g)));
            });
        });
    }
}

fn fx_slot(ui: &mut Ui, studio: &mut Studio, sel: u16, slot: usize, ins: &MixerInsert) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let p = ui.painter().clone();
    let fx = ins.effects.get(slot);
    let hot = resp.hovered();
    p.rect_filled(
        rect,
        4.0,
        if fx.is_some() || hot {
            pal::BG_WIDGET
        } else {
            pal::BG_PANEL
        },
    );
    p.rect_stroke(rect, 4.0, Stroke::new(1.0, pal::BORDER), StrokeKind::Inside);
    p.text(
        pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!("{}", slot + 1),
        theme::mono(10.0),
        pal::TEXT_FAINT,
    );
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(6.0, 3.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut row;
    ui.add_space(18.0);
    match fx {
        Some(fx) => {
            if widgets::led(ui, fx.enabled, pal::GREEN, 16.0)
                .on_hover_text("Attiva / bypass")
                .clicked()
            {
                studio.exec(Command::SetEffect {
                    insert: sel,
                    slot: slot as u16,
                    enabled: Some(!fx.enabled),
                    mix: None,
                });
            }
            let name = plugin_label(&fx.plugin.kind, fx.plugin.foreign_name.as_deref());
            let col = if fx.enabled { pal::TEXT } else { pal::TEXT_FAINT };
            ui.label(RichText::new(name).font(theme::bold_font(11.5)).color(col));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if widgets::icon_button(ui, Icon::Close, false, pal::RED, 22.0, "Rimuovi effetto").clicked() {
                    studio.exec(Command::RemoveEffect {
                        insert: sel,
                        slot: slot as u16,
                    });
                }
                if widgets::icon_button(ui, Icon::Plugin, false, pal::ACCENT, 22.0, "Apri l'editor").clicked() {
                    studio.plugin_editor = Some(PluginTarget::Effect {
                        insert: sel,
                        slot: slot as u16,
                    });
                }
                let id = Id::new(("fx_mix", sel, slot));
                let mut mix = live(ui.ctx(), id, fx.mix);
                let r = widgets::knob_ex(ui, &mut mix, 0.0, 1.0, 1.0, 22.0, false, pal::ACCENT);
                if let Some(nv) = settle(ui.ctx(), id, &r, mix, fx.mix) {
                    studio.exec(Command::SetEffect {
                        insert: sel,
                        slot: slot as u16,
                        enabled: None,
                        mix: Some(nv),
                    });
                }
                r.on_hover_text(format!("Mix {:.0}%", mix * 100.0));
            });
            if resp.double_clicked() {
                studio.plugin_editor = Some(PluginTarget::Effect {
                    insert: sel,
                    slot: slot as u16,
                });
            }
        }
        None => {
            let txt = if slot == ins.effects.len() {
                "Aggiungi effetto…"
            } else {
                "Slot vuoto"
            };
            ui.label(
                RichText::new(txt)
                    .size(11.5)
                    .color(if hot && slot == ins.effects.len() {
                        pal::ACCENT
                    } else {
                        pal::TEXT_FAINT
                    }),
            );
            if slot == ins.effects.len() {
                egui::Popup::menu(&resp).show(|ui| {
                    ui.set_min_width(200.0);
                    for d in registry().iter().filter(|d| d.role == PluginRole::Effect) {
                        if ui.button(d.name).on_hover_text(d.description).clicked() {
                            if let Some(plugin) = default_instance(d.id) {
                                studio.exec(Command::AddEffect {
                                    insert: sel,
                                    plugin,
                                    slot: None,
                                });
                            }
                            ui.close();
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfc_core::{ChannelKind, Outcome, PluginInstance};

    #[test]
    fn default_project_shows_master_and_empty_tail() {
        let p = Project::new("t");
        let v = visible_inserts(&p, MASTER, false);
        assert_eq!(v, vec![1, 2, 3, 4]);
        assert!(!v.contains(&MASTER));
    }

    #[test]
    fn used_and_selected_inserts_are_visible() {
        let mut p = Project::new("t");
        let Outcome::Channel(_) = Command::AddChannel {
            name: "Kick".into(),
            kind: ChannelKind::Generator,
            plugin: PluginInstance::new("mfc.kick"),
            mixer_insert: Some(40),
        }
        .apply(&mut p)
        .unwrap() else {
            panic!("canale non creato");
        };
        let v = visible_inserts(&p, 90, false);
        assert!(v.contains(&40));
        assert!(v.contains(&90));
        assert_eq!(visible_inserts(&p, MASTER, true).len(), MIXER_INSERTS);
    }

    #[test]
    fn route_target_marks_insert_used() {
        let mut p = Project::new("t");
        Command::SetRoute {
            from: 2,
            to: 70,
            gain: Some(0.5),
        }
        .apply(&mut p)
        .unwrap();
        assert!(visible_inserts(&p, MASTER, false).contains(&70));
    }

    #[test]
    fn fader_taper_roundtrip() {
        for v in [0.0f32, 0.1, 0.4, MIXER_UNITY, FADER_MAX] {
            assert!((fader_value(fader_pos(v)) - v).abs() < 1e-5);
        }
        // 0 dB all'80% della corsa, -12 dB ben sopra il fondo.
        assert!((fader_pos(MIXER_UNITY) - 0.8).abs() < 1e-3);
        assert!(fader_pos(MIXER_UNITY * 0.25) > 0.35);
    }

    #[test]
    fn plugin_label_fallbacks() {
        assert_eq!(plugin_label("mfc.reverb", None), "Reverb");
        assert_eq!(plugin_label("ext.vst", Some("Vendor X")), "Vendor X");
        assert_eq!(plugin_label("ext.vst", None), "ext.vst");
    }
}
