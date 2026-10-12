//! Channel Rack: canali del progetto e step sequencer del pattern corrente.
//!
//! Ogni riga ha LED di mute, pan, volume, insert del mixer, nome e i passi del pattern.
//! Un click su uno step esegue `SetStep`; se il canale contiene note che non sono passi,
//! la riga mostra una mini piano roll al posto dei passi.

use egui::{vec2, Color32, Context, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use mfc_core::{
    fader_gain, ChannelId, ChannelKind, Command, Note, Outcome, Pattern, PatternId, Project, Tick, TimeBase,
    CHANNEL_UNITY, MIXER_INSERTS,
};
use mfc_plugins::{default_instance, descriptor, ids, registry, PluginRole};

use crate::state::{PluginTarget, Studio, View};
use crate::views::widgets;

/// Lunghezze selezionabili del pattern (in passi da 1/16).
pub const STEP_COUNTS: [u32; 3] = [16, 32, 64];
const STEP_W: f32 = 18.0;
const STEP_H: f32 = 22.0;
const NAME_W: f32 = 120.0;
const KNOB_D: f32 = 18.0;
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
    color: Color32,
    index: usize,
    count: usize,
    muted: bool,
    pan: f32,
    volume: f32,
    mixer_insert: u16,
    root: u8,
    /// Passi attivi (indice = passo).
    steps: Vec<bool>,
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
            Row {
                id: c.id,
                name: c.name.clone(),
                color: Color32::from_rgb(c.color.0, c.color.1, c.color.2),
                index,
                count,
                muted: c.muted,
                pan: c.pan,
                volume: c.volume,
                mixer_insert: c.mixer_insert,
                root: c.root_note,
                steps: (0..steps)
                    .map(|s| pat.is_some_and(|p| step_is_on(p, c.id, s, &tb)))
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

/// Disegna il Channel Rack.
pub fn show(ui: &mut Ui, studio: &mut Studio) {
    let pattern = studio.current_pattern();
    let steps = pattern.map_or(0, |p| pattern_steps(&studio.project, p));
    header(ui, studio, pattern, steps);
    ui.separator();
    if studio.project.channels.is_empty() {
        ui.label("Nessun canale: usa \"Aggiungi canale\" per crearne uno.");
        return;
    }
    let rows = rows(&studio.project, pattern, steps);
    egui::ScrollArea::both()
        .id_salt("rack_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in &rows {
                row_ui(ui, studio, pattern, steps, row);
            }
        });
}

fn header(ui: &mut Ui, studio: &mut Studio, pattern: Option<PatternId>, steps: u32) {
    ui.horizontal(|ui| {
        ui.strong("Channel Rack");
        ui.menu_button("Aggiungi canale", |ui| {
            for d in registry().iter().filter(|d| d.role == PluginRole::Generator) {
                if ui.button(d.name).on_hover_text(d.description).clicked() {
                    add_generator(studio, d.id);
                    ui.close();
                }
            }
        });
        if let Some(pid) = pattern {
            ui.separator();
            ui.label("Passi");
            egui::ComboBox::from_id_salt("rack_steps")
                .selected_text(steps.to_string())
                .show_ui(ui, |ui| {
                    for n in STEP_COUNTS {
                        if ui.selectable_label(steps == n, n.to_string()).clicked() {
                            let len = Tick::from(n) * studio.project.time.ticks_per_step();
                            studio.exec(Command::SetPattern {
                                pattern: pid,
                                name: None,
                                length: Some(Some(len)),
                                color: None,
                            });
                            ui.close();
                        }
                    }
                });
        }
    });
}

fn row_ui(ui: &mut Ui, studio: &mut Studio, pattern: Option<PatternId>, steps: u32, row: &Row) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 2.0);
        mute_led(ui, studio, row);
        pan_knob(ui, studio, row);
        volume_knob(ui, studio, row);
        insert_drag(ui, studio, row);
        name_button(ui, studio, row);
        if ui.small_button("🎹").on_hover_text("Apri nel Piano Roll").clicked() {
            studio.selected_channel = Some(row.id);
            studio.view = View::PianoRoll;
        }
        ui.add_space(6.0);
        if row.free.is_empty() {
            for s in 0..steps {
                if s > 0 && s % 4 == 0 {
                    ui.add_space(4.0);
                }
                if let Some(pid) = pattern {
                    step_button(ui, studio, pid, row, s);
                }
            }
        } else {
            mini_roll(ui, studio, row, steps);
        }
    });
}

fn mute_led(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
    let color = if row.muted {
        Color32::from_gray(80)
    } else {
        Color32::from_rgb(120, 220, 90)
    };
    ui.painter().circle_filled(rect.center(), 5.0, color);
    if resp.clicked() {
        studio.exec(channel_cmd(row.id, None, None, Some(!row.muted), None));
    }
    resp.on_hover_text(if row.muted { "Riattiva canale" } else { "Muta canale" });
}

fn pan_knob(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let id = Id::new(("rack_pan", row.id.0));
    let mut v = live(ui.ctx(), id, row.pan);
    let r = widgets::knob(ui, &mut v, -1.0, 1.0, 0.0, KNOB_D);
    if let Some(nv) = settle(ui.ctx(), id, &r, v, row.pan) {
        studio.exec(channel_cmd(row.id, None, Some(nv), None, None));
    }
    r.on_hover_text(format!("Pan {v:+.2}"));
}

fn volume_knob(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let id = Id::new(("rack_vol", row.id.0));
    let mut v = live(ui.ctx(), id, row.volume);
    let r = widgets::knob(ui, &mut v, 0.0, 1.25, CHANNEL_UNITY, KNOB_D);
    if let Some(nv) = settle(ui.ctx(), id, &r, v, row.volume) {
        studio.exec(channel_cmd(row.id, Some(nv), None, None, None));
    }
    r.on_hover_text(format!("Volume {}", db_text(fader_gain(v, CHANNEL_UNITY))));
}

fn insert_drag(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let id = Id::new(("rack_ins", row.id.0));
    let mut v = live(ui.ctx(), id, f32::from(row.mixer_insert));
    let r = ui.add(
        egui::DragValue::new(&mut v)
            .range(0.0..=MIXER_INSERTS as f32)
            .speed(0.1)
            .fixed_decimals(0),
    );
    if let Some(nv) = settle(ui.ctx(), id, &r, v, f32::from(row.mixer_insert)) {
        let insert = nv.round() as u16;
        if insert != row.mixer_insert {
            studio.exec(channel_cmd(row.id, None, None, None, Some(insert)));
        }
    }
    r.on_hover_text("Insert del mixer di destinazione");
}

fn name_button(ui: &mut Ui, studio: &mut Studio, row: &Row) {
    let selected = studio.selected_channel == Some(row.id);
    let resp = ui.add_sized(
        vec2(NAME_W, STEP_H),
        egui::Button::new(row.name.as_str()).selected(selected),
    );
    if resp.clicked() {
        studio.selected_channel = Some(row.id);
        studio.plugin_editor = Some(PluginTarget::Channel(row.id));
    }
    if resp.double_clicked() {
        studio.selected_channel = Some(row.id);
        studio.view = View::PianoRoll;
    }
    resp.context_menu(|ui| row_menu(ui, studio, row));
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

fn step_button(ui: &mut Ui, studio: &mut Studio, pattern: PatternId, row: &Row, step: u32) {
    let on = row.steps.get(step as usize).copied().unwrap_or(false);
    let (rect, resp) = ui.allocate_exact_size(vec2(STEP_W, STEP_H), Sense::click());
    let group_bg = if (step / 4).is_multiple_of(2) {
        Color32::from_gray(62)
    } else {
        Color32::from_gray(44)
    };
    let fill = if on { row.color } else { group_bg };
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, fill);
    if resp.hovered() {
        painter.rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
    }
    if resp.clicked() {
        studio.exec(Command::SetStep {
            pattern,
            channel: row.id,
            step,
            on: !on,
            velocity: None,
        });
        studio.engine.preview_on(row.id, row.root, STEP_VELOCITY);
        studio.engine.preview_off(row.id, row.root);
    }
}

/// Mini piano roll per i canali con note che non sono passi.
fn mini_roll(ui: &mut Ui, studio: &mut Studio, row: &Row, steps: u32) {
    let total = (Tick::from(steps) * studio.project.time.ticks_per_step()).max(1) as f32;
    let w = steps as f32 * STEP_W + (steps / 4) as f32 * 4.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w.max(STEP_W), STEP_H), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_gray(30));
    let lo = row.free.iter().map(|n| n.key).min().unwrap_or(60);
    let hi = row.free.iter().map(|n| n.key).max().unwrap_or(60);
    let span = (i32::from(hi) - i32::from(lo) + 1).max(12) as f32;
    let note_h = rect.height() / span;
    for n in &row.free {
        let x0 = rect.left() + n.position as f32 / total * rect.width();
        let x1 = (rect.left() + n.end() as f32 / total * rect.width()).min(rect.right());
        let y = rect.bottom() - (f32::from(n.key) - f32::from(lo) + 1.0) / span * rect.height();
        let r = Rect::from_min_max(egui::pos2(x0, y - note_h), egui::pos2(x1.max(x0 + 1.5), y));
        painter.rect_filled(r, 1.0, row.color);
    }
    if resp.clicked() {
        studio.selected_channel = Some(row.id);
        studio.view = View::PianoRoll;
    }
    resp.on_hover_text("Note non di passo: clic per aprire il Piano Roll");
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
