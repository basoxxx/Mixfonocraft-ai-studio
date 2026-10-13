//! Channel Rack: canali del progetto e step sequencer del pattern corrente.
//!
//! Ogni riga ha LED di mute, pan, volume, insert del mixer, nome e i passi del pattern.
//! Un click su uno step esegue `SetStep`; se il canale contiene note che non sono passi,
//! la riga mostra una mini piano roll al posto dei passi.

use egui::{vec2, Color32, Context, Id, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui};
use mfc_core::{
    fader_gain, ChannelId, ChannelKind, Command, Note, Outcome, Pattern, PatternId, Project, Tick, TimeBase,
    CHANNEL_UNITY, MIXER_INSERTS,
};
use mfc_plugins::{default_instance, descriptor, ids, registry, PluginRole};

use crate::state::{PluginTarget, Studio, View};
use crate::theme::{self, pal};
use crate::views::widgets::{self, Icon};

/// Lunghezze selezionabili del pattern (in passi da 1/16).
pub const STEP_COUNTS: [u32; 3] = [16, 32, 64];
const STEP_W: f32 = 22.0;
const STEP_H: f32 = 26.0;
const NAME_W: f32 = 132.0;
const KNOB_D: f32 = 24.0;
/// Velocity dei passi creati con il click.
const STEP_VELOCITY: f32 = 0.78;

/// Numero di passi del pattern (lunghezza effettiva divisa per la durata di un passo).
pub fn pattern_steps(project: &Project, pattern: PatternId) -> u32 {
    let tb = project.time;
    project
        .pattern(pattern)
        .map_or(0, |p| (p.effective_length(&tb) / tb.ticks_per_step()) as u32)
}

/// `true` se il canale ha una nota di passo (1/16) nel passo `step`.
pub fn step_is_on(pattern: &Pattern, channel: ChannelId, step: u32, tb: &TimeBase) -> bool {
    let pos = step as Tick * tb.ticks_per_step();
    pattern
        .notes
        .get(&channel)
        .is_some_and(|notes| notes.iter().any(|n| n.position == pos))
}

/// `true` se la nota non è un passo: durata diversa, tono diverso dalla root o fuori griglia.
pub fn is_free_note(note: &Note, root: u8, tb: &TimeBase) -> bool {
    let step = tb.ticks_per_step();
    note.length != step || note.key != root || !note.position.is_multiple_of(step)
}

/// Testo dB per un guadagno lineare.
pub fn db_text(gain: f32) -> String {
    if gain <= 1e-6 {
        "-∞ dB".into()
    } else {
        format!("{:+.1} dB", 20.0 * gain.log10())
    }
}

/// Valore mostrato da un controllo continuo: durante il drag quello temporaneo, altrimenti
/// il valore committato nel progetto.
pub(crate) fn live(ctx: &Context, id: Id, committed: f32) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(id)).unwrap_or(committed)
}

/// Gestisce un controllo continuo. Durante il drag memorizza il valore in corso; a rilascio
/// (o a click / doppio click) restituisce il valore da committare, se diverso da quello
/// del progetto.
pub(crate) fn settle(ctx: &Context, id: Id, resp: &Response, value: f32, committed: f32) -> Option<f32> {
    if resp.dragged() {
        ctx.data_mut(|d| d.insert_temp(id, value));
        return None;
    }
    if resp.drag_stopped() || resp.changed() {
        ctx.data_mut(|d| d.remove::<f32>(id));
        if (value - committed).abs() > 1e-6 {
            return Some(value);
        }
    }
    None
}

/// Comando `SetChannel` che modifica solo i campi indicati.
fn channel_cmd(
    channel: ChannelId,
    volume: Option<f32>,
    pan: Option<f32>,
    muted: Option<bool>,
    mixer_insert: Option<u16>,
) -> Command {
    Command::SetChannel {
        channel,
        name: None,
        volume,
        pan,
        pitch_cents: None,
        muted,
        mixer_insert,
        root_note: None,
        color: None,
    }
}

/// Aggiunge un canale per il generatore `id` (dal registro dei plugin) e lo seleziona.
pub fn add_generator(studio: &mut Studio, id: &str) -> Option<ChannelId> {
    let name = descriptor(id)?.name;
    let plugin = default_instance(id)?;
    let kind = match id {
        ids::SAMPLER => ChannelKind::Sampler,
        ids::AUDIO_CLIP => ChannelKind::AudioClip,
        _ => ChannelKind::Generator,
    };
    match studio.exec(Command::AddChannel {
        name: name.to_string(),
        kind,
        plugin,
        mixer_insert: None,
    })? {
        Outcome::Channel(c) => {
            studio.selected_channel = Some(c);
            Some(c)
        }
        _ => None,
    }
}

/// Snapshot di una riga, copiato dal progetto per non tenere in prestito lo `Studio`.
struct Row {
    id: ChannelId,
    name: String,
    plugin: String,
    color: Color32,
    index: usize,
    count: usize,
    muted: bool,
    pan: f32,
    volume: f32,
    mixer_insert: u16,
    root: u8,
    /// Velocity dei passi attivi (indice = passo).
    steps: Vec<Option<f32>>,
    /// Note che non sono passi: se presenti la riga mostra la mini piano roll.
    free: Vec<Note>,
}

fn rows(project: &Project, pattern: Option<PatternId>, steps: u32) -> Vec<Row> {
    let tb = project.time;
    let pat = pattern.and_then(|p| project.pattern(p));
    let count = project.channels.len();
    project
        .channels
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let notes: &[Note] = pat.and_then(|p| p.notes.get(&c.id)).map_or(&[], Vec::as_slice);
            let tps = tb.ticks_per_step();
            Row {
                id: c.id,
                name: c.name.clone(),
                plugin: descriptor(&c.plugin.kind)
                    .map(|d| d.name.to_string())
                    .or_else(|| c.plugin.foreign_name.clone())
                    .unwrap_or_else(|| c.plugin.kind.clone()),
                color: theme::item_color(c.color, index),
                index,
                count,
                muted: c.muted,
                pan: c.pan,
                volume: c.volume,
                mixer_insert: c.mixer_insert,
                root: c.root_note,
                steps: (0..steps)
                    .map(|s| notes.iter().find(|n| n.position == s as Tick * tps).map(|n| n.velocity))
                    .collect(),
                free: notes
                    .iter()
                    .copied()
                    .filter(|n| is_free_note(n, c.root_note, &tb))
                    .collect(),
            }
        })
        .collect()
}

const ROW_H: f32 = 34.0;
const ROW_GAP: f32 = 4.0;
const LEFT_W: f32 = 318.0;

/// Disegna il Channel Rack.
pub fn show(ui: &mut Ui, studio: &mut Studio) {
    let area = ui.max_rect();
    let pattern = studio.current_pattern();
    let steps = pattern.map_or(0, |p| pattern_steps(&studio.project, p));
    let pat_name = pattern
        .and_then(|p| studio.project.pattern(p))
        .map(|p| p.name.clone())
        .unwrap_or_default();
    widgets::view_header(ui, Icon::Rack, "Channel Rack", &pat_name, |ui| {
        ui.menu_button(
            RichText::new("+ Canale")
                .font(theme::bold_font(11.5))
                .color(pal::ACCENT()),
            |ui| {
                ui.set_min_width(220.0);
                generator_menu(ui, studio);
            },
        );
        if let Some(pid) = pattern {
            let sel = STEP_COUNTS.iter().position(|n| *n == steps).unwrap_or(usize::MAX);
            if let Some(i) = widgets::segmented(ui, &["64", "32", "16"], 2usize.wrapping_sub(sel), 24.0) {
                let n = STEP_COUNTS[2 - i];
                let len = Tick::from(n) * studio.project.time.ticks_per_step();
                studio.exec(Command::SetPattern {
                    pattern: pid,
                    name: None,
                    length: Some(Some(len)),
                    color: None,
                });
            }
            ui.label(
                RichText::new("PASSI")
                    .font(theme::bold_font(9.5))
                    .color(pal::TEXT_FAINT()),
            );
        }
    });
    let rows = rows(&studio.project, pattern, steps);
    let playing_step = (!studio.song_mode && studio.engine.shared.is_playing() && steps > 0)
        .then(|| (studio.engine.shared.position() / studio.project.time.ticks_per_step() as f64) as u32 % steps);
    egui::ScrollArea::both()
        .id_salt("rack_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(8.0);
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(10, 0))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = ROW_GAP;
                    for row in &rows {
                        row_ui(ui, studio, pattern, steps, row, playing_step);
                    }
                    add_row(ui, studio, steps);
                });
            ui.add_space(10.0);
        });
    // Fine della pittura degli step quando il pulsante viene rilasciato.
    if !ui.input(|i| i.pointer.primary_down()) {
        ui.data_mut(|d| d.remove::<bool>(Id::new("rack_paint")));
    }
    // Campione trascinato dal browser: nuovo canale Sampler.
    if let Some((path, _)) = crate::views::samples::drop_target(ui, area, "Rilascia: nuovo canale Sampler") {
        crate::views::samples::add_sample_channel(studio, &path);
    }
}

fn generator_menu(ui: &mut Ui, studio: &mut Studio) {
    for d in registry()
        .iter()
        .filter(|d| d.role == PluginRole::Generator && d.id != ids::SILENT)
    {
        if ui.button(d.name).on_hover_text(d.description).clicked() {
            add_generator(studio, d.id);
            ui.close();
        }
    }
}

fn steps_width(steps: u32) -> f32 {
    steps as f32 * STEP_W + (steps.saturating_sub(1) / 4) as f32 * 6.0
}

fn row_ui(ui: &mut Ui, studio: &mut Studio, pattern: Option<PatternId>, steps: u32, row: &Row, playing: Option<u32>) {
    let selected = studio.selected_channel == Some(row.id);
    let width = LEFT_W + 12.0 + steps_width(steps).max(STEP_W * 16.0) + 8.0;
    let (rect, bg_resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    let p = ui.painter().clone();
    let bg = if selected {
        theme::mix(pal::BG_RAISED(), row.color, 0.10)
    } else {
        pal::BG_RAISED()
    };
    p.rect_filled(rect, 5.0, bg);
    p.rect_stroke(
        rect,
        5.0,
        Stroke::new(
            1.0,
            if selected {
                row.color.gamma_multiply(0.6)
            } else {
                pal::BORDER()
            },
        ),
        StrokeKind::Inside,
    );
    // Banda colore del canale.
    p.rect_filled(
        Rect::from_min_size(rect.left_top(), vec2(4.0, ROW_H)),
        egui::CornerRadius {
            nw: 5,
            sw: 5,
            ne: 0,
            se: 0,
        },
        row.color,
    );
    if bg_resp.clicked() {
        studio.selected_channel = Some(row.id);
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(10.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 6.0;
    // LED mute.
    let r = widgets::led(ui, !row.muted, pal::GREEN(), 16.0).on_hover_text(if row.muted {
        "Riattiva canale"
    } else {
        "Muta canale"
    });
    if r.clicked() {
        studio.exec(channel_cmd(row.id, None, None, Some(!row.muted), None));
    }
    pan_knob(ui, studio, row);
    volume_knob(ui, studio, row);
    insert_box(ui, studio, row);
    name_plate(ui, studio, row, selected);
    if widgets::icon_button(ui, Icon::Piano, false, pal::ACCENT(), 24.0, "Apri nel Piano Roll").clicked() {
        studio.selected_channel = Some(row.id);
        studio.view = View::PianoRoll;
    }
    ui.add_space(6.0);
    if row.free.is_empty() {
        if let Some(pid) = pattern {
            step_strip(ui, studio, pid, row, steps, playing);
        }
    } else {
        mini_roll(ui, studio, row, steps);
    }
}

fn pan_knob(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let id = Id::new(("rack_pan", row.id.0));
    let mut v = live(ui.ctx(), id, row.pan);
    let r = widgets::knob_ex(ui, &mut v, -1.0, 1.0, 0.0, KNOB_D, true, pal::BLUE());
    if let Some(nv) = settle(ui.ctx(), id, &r, v, row.pan) {
        studio.exec(channel_cmd(row.id, None, Some(nv), None, None));
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

fn volume_knob(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let id = Id::new(("rack_vol", row.id.0));
    let mut v = live(ui.ctx(), id, row.volume);
    let r = widgets::knob_ex(ui, &mut v, 0.0, 1.25, CHANNEL_UNITY, KNOB_D, false, pal::ACCENT());
    if let Some(nv) = settle(ui.ctx(), id, &r, v, row.volume) {
        studio.exec(channel_cmd(row.id, Some(nv), None, None, None));
    }
    r.on_hover_text(format!("Volume {}", db_text(fader_gain(v, CHANNEL_UNITY))));
}

fn insert_box(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let id = Id::new(("rack_ins", row.id.0));
    let committed = f32::from(row.mixer_insert);
    let v = live(ui.ctx(), id, committed);
    let label = if v.round() as u16 == 0 {
        "M".to_string()
    } else {
        format!("{}", v.round() as u16)
    };
    let r = widgets::lcd(ui, &label, None, 30.0, 22.0, Sense::click_and_drag())
        .on_hover_text("Insert del mixer (trascina per cambiare, M = master)");
    let mut nv = v;
    if r.dragged() {
        nv = (v - r.drag_delta().y * 0.1).clamp(0.0, MIXER_INSERTS as f32);
        ui.data_mut(|d| d.insert_temp(id, nv));
    }
    if r.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(id));
        let insert = nv.round() as u16;
        if insert != row.mixer_insert {
            studio.exec(channel_cmd(row.id, None, None, None, Some(insert)));
        }
    }
    if r.clicked() {
        studio.selected_insert = row.mixer_insert;
        studio.view = View::Mixer;
    }
}

fn name_plate(ui: &mut Ui, studio: &mut Studio, row: &Row, selected: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(NAME_W, 28.0), Sense::click());
    let p = ui.painter();
    let hot = resp.hovered();
    let fill = if selected {
        theme::mix(pal::BG_WIDGET(), row.color, 0.28)
    } else if hot {
        pal::BG_HOVER()
    } else {
        pal::BG_WIDGET()
    };
    p.rect_filled(rect, 4.0, fill);
    p.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, if selected { row.color } else { pal::BORDER() }),
        StrokeKind::Inside,
    );
    let text_col = if row.muted { pal::TEXT_FAINT() } else { pal::TEXT() };
    let clip = p.with_clip_rect(rect.shrink(2.0));
    clip.text(
        rect.left_center() + vec2(8.0, -5.5),
        egui::Align2::LEFT_CENTER,
        &row.name,
        theme::bold_font(11.5),
        text_col,
    );
    clip.text(
        rect.left_center() + vec2(8.0, 7.0),
        egui::Align2::LEFT_CENTER,
        &row.plugin,
        egui::FontId::proportional(9.0),
        pal::TEXT_FAINT(),
    );
    if resp.clicked() {
        studio.selected_channel = Some(row.id);
        studio.plugin_editor = Some(PluginTarget::Channel(row.id));
    }
    if resp.double_clicked() {
        studio.selected_channel = Some(row.id);
        studio.view = View::PianoRoll;
    }
    resp.on_hover_text("Click: editor del plugin · Doppio click: Piano Roll · Tasto destro: opzioni")
        .context_menu(|ui| row_menu(ui, studio, row));
}

fn row_menu(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    if ui.button("Clona").clicked() {
        if let Some(Outcome::Channel(c)) = studio.exec(Command::CloneChannel { channel: row.id }) {
            studio.selected_channel = Some(c);
        }
        ui.close();
    }
    if ui.button("Elimina").clicked() {
        studio.exec(Command::RemoveChannel { channel: row.id });
        ui.close();
    }
    ui.separator();
    if ui.add_enabled(row.index > 0, egui::Button::new("Sposta su")).clicked() {
        studio.exec(Command::MoveChannel {
            channel: row.id,
            index: row.index - 1,
        });
        ui.close();
    }
    if ui
        .add_enabled(row.index + 1 < row.count, egui::Button::new("Sposta giù"))
        .clicked()
    {
        studio.exec(Command::MoveChannel {
            channel: row.id,
            index: row.index + 1,
        });
        ui.close();
    }
}

/// Striscia dei passi: click per accendere/spegnere, trascina per "dipingere".
fn step_strip(ui: &mut Ui, studio: &mut Studio, pattern: PatternId, row: &Row, steps: u32, playing: Option<u32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(steps_width(steps), STEP_H), Sense::hover());
    let paint_id = Id::new("rack_paint");
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let pressed = ui.input(|i| i.pointer.primary_pressed());
    let down = ui.input(|i| i.pointer.primary_down());
    let p = ui.painter().clone();
    let mut x = rect.left();
    for s in 0..steps {
        if s > 0 && s % 4 == 0 {
            x += 6.0;
        }
        let r = Rect::from_min_size(egui::pos2(x, rect.top()), vec2(STEP_W - 3.0, STEP_H));
        x += STEP_W;
        let vel = row.steps.get(s as usize).copied().flatten();
        let on = vel.is_some();
        let hot = pointer.is_some_and(|pt| r.contains(pt)) && ui.rect_contains_pointer(r);
        // Gruppi da 4 alternati (come FL): tinte diverse.
        let group_dark = (s / 4) % 2 == 1;
        let off_fill = if group_dark { pal::BG_WIDGET() } else { pal::BG_HOVER() };
        if on {
            let v = vel.unwrap_or(0.78);
            let c = theme::mix(theme::mix(row.color, Color32::BLACK, 0.45), row.color, 0.35 + 0.65 * v);
            p.rect_filled(r, 3.0, c);
            p.rect_filled(
                Rect::from_min_size(r.left_top() + vec2(2.0, 2.0), vec2(r.width() - 4.0, 3.0)),
                1.5,
                Color32::from_white_alpha(70),
            );
        } else {
            p.rect_filled(
                r,
                3.0,
                if hot {
                    theme::mix(pal::BG_HOVER(), pal::ACCENT_SOFT(), 0.3)
                } else {
                    off_fill
                },
            );
            p.rect_filled(
                Rect::from_min_size(r.left_bottom() - vec2(-2.0, 4.0), vec2(r.width() - 4.0, 2.0)),
                1.0,
                Color32::from_black_alpha(60),
            );
        }
        p.rect_stroke(
            r,
            3.0,
            Stroke::new(
                1.0,
                if hot {
                    Color32::from_white_alpha(120)
                } else {
                    pal::BORDER()
                },
            ),
            StrokeKind::Inside,
        );
        if playing == Some(s) {
            p.rect_stroke(r.expand(1.5), 4.0, Stroke::new(1.5, pal::ACCENT()), StrokeKind::Outside);
        }
        if !hot {
            continue;
        }
        if pressed {
            // Il primo step decide se si accende o si spegne per tutto il gesto.
            let target = !on;
            ui.data_mut(|d| d.insert_temp(paint_id, target));
            toggle_step(studio, pattern, row, s, target);
        } else if down {
            if let Some(target) = ui.data(|d| d.get_temp::<bool>(paint_id)) {
                if target != on {
                    toggle_step(studio, pattern, row, s, target);
                }
            }
        }
    }
}

fn toggle_step(studio: &mut Studio, pattern: PatternId, row: &Row, step: u32, on: bool) {
    studio.exec(Command::SetStep {
        pattern,
        channel: row.id,
        step,
        on,
        velocity: None,
    });
    if on {
        studio.engine.preview_on(row.id, row.root, STEP_VELOCITY);
        studio.engine.preview_off(row.id, row.root);
    }
}

/// Mini piano roll per i canali con note che non sono passi.
fn mini_roll(ui: &mut Ui, studio: &mut Studio, row: &Row, steps: u32) {
    let total = (Tick::from(steps) * studio.project.time.ticks_per_step()).max(1) as f32;
    let w = steps_width(steps).max(STEP_W * 16.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, STEP_H), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, pal::BG_INSET());
    // Linee di battuta/beat.
    let beats = (steps / 4).max(1);
    for b in 1..beats {
        let x = rect.left() + rect.width() * b as f32 / beats as f32;
        let c = if b % 4 == 0 { pal::LINE() } else { pal::GRID_A() };
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0, c),
        );
    }
    let lo = row.free.iter().map(|n| n.key).min().unwrap_or(60);
    let hi = row.free.iter().map(|n| n.key).max().unwrap_or(60);
    let span = (i32::from(hi) - i32::from(lo) + 1).max(12) as f32;
    let note_h = ((rect.height() - 4.0) / span).max(2.0);
    for n in &row.free {
        let x0 = rect.left() + n.position as f32 / total * rect.width();
        let x1 = (rect.left() + n.end() as f32 / total * rect.width()).min(rect.right());
        let y = rect.bottom() - 2.0 - (f32::from(n.key) - f32::from(lo)) / span * (rect.height() - 4.0);
        let r = Rect::from_min_max(egui::pos2(x0, y - note_h), egui::pos2((x1 - 1.0).max(x0 + 2.0), y));
        p.rect_filled(r, 1.0, row.color);
    }
    if resp.hovered() {
        p.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, row.color.gamma_multiply(0.7)),
            StrokeKind::Inside,
        );
    }
    if resp.clicked() {
        studio.selected_channel = Some(row.id);
        studio.view = View::PianoRoll;
    }
    resp.on_hover_text("Melodia: clic per aprire il Piano Roll");
}

/// Riga finale per aggiungere un canale.
fn add_row(ui: &mut Ui, studio: &mut Studio, steps: u32) {
    let width = LEFT_W + 12.0 + steps_width(steps).max(STEP_W * 16.0) + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let p = ui.painter();
    let c = if resp.hovered() {
        pal::ACCENT()
    } else {
        pal::TEXT_FAINT()
    };
    let dash = Stroke::new(1.0, c.gamma_multiply(0.6));
    let pts = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    for w in pts.windows(2) {
        p.add(egui::Shape::dashed_line(&[w[0], w[1]], dash, 5.0, 4.0));
    }
    widgets::paint_icon(
        p,
        Rect::from_center_size(rect.center() - vec2(64.0, 0.0), vec2(14.0, 14.0)),
        Icon::Plus,
        c,
    );
    p.text(
        rect.center() + vec2(6.0, 0.0),
        egui::Align2::CENTER_CENTER,
        "Aggiungi canale",
        theme::bold_font(11.5),
        c,
    );
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(220.0);
        generator_menu(ui, studio);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfc_core::PatternId;

    #[test]
    fn step_detection_uses_step_grid() {
        let tb = TimeBase::default();
        let tps = tb.ticks_per_step();
        let mut pat = Pattern::new(PatternId(1), "p");
        let ch = ChannelId(7);
        pat.notes
            .entry(ch)
            .or_default()
            .push(Note::new(4 * tps, tps, 60, STEP_VELOCITY));
        assert!(step_is_on(&pat, ch, 4, &tb));
        assert!(!step_is_on(&pat, ch, 3, &tb));
        assert!(!step_is_on(&pat, ChannelId(8), 4, &tb));
    }

    #[test]
    fn free_notes_are_not_steps() {
        let tb = TimeBase::default();
        let tps = tb.ticks_per_step();
        assert!(!is_free_note(&Note::new(0, tps, 60, 0.5), 60, &tb));
        assert!(is_free_note(&Note::new(0, tps * 2, 60, 0.5), 60, &tb));
        assert!(is_free_note(&Note::new(0, tps, 62, 0.5), 60, &tb));
        assert!(is_free_note(&Note::new(1, tps, 60, 0.5), 60, &tb));
    }

    #[test]
    fn steps_follow_pattern_length() {
        let mut p = Project::new("t");
        let pid = p.patterns[0].id;
        assert_eq!(pattern_steps(&p, pid), 16);
        let len = 32 * p.time.ticks_per_step();
        p.pattern_mut(pid).unwrap().length = Some(len);
        assert_eq!(pattern_steps(&p, pid), 32);
    }

    #[test]
    fn db_text_formats() {
        assert_eq!(db_text(0.0), "-∞ dB");
        assert_eq!(db_text(1.0), "+0.0 dB");
    }
}
