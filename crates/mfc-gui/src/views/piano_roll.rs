//! Piano Roll: editing delle note del canale selezionato nel pattern corrente.
//!
//! Click su area vuota = nuova nota; trascina il corpo = sposta; trascina il bordo destro =
//! ridimensiona; click destro = elimina. Le modifiche diventano un solo comando al rilascio.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind};
use mfc_core::{ChannelId, Command, Note, NoteFilter, PatternId, Tick, TimeBase};

use crate::state::Studio;

const KEYS_W: f32 = 48.0;
const RESIZE_PX: f32 = 6.0;

/// Griglia di aggancio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    None,
    Sixth,
    Quarter,
    Half,
    Step,
    Beat,
    Bar,
}

impl Snap {
    pub const ALL: [Snap; 7] = [Snap::None, Snap::Sixth, Snap::Quarter, Snap::Half, Snap::Step, Snap::Beat, Snap::Bar];
    pub fn label(self) -> &'static str {
        match self {
            Snap::None => "(nessuno)",
            Snap::Sixth => "1/6 step",
            Snap::Quarter => "1/4 step",
            Snap::Half => "1/2 step",
            Snap::Step => "Step",
            Snap::Beat => "Beat",
            Snap::Bar => "Battuta",
        }
    }
    pub fn ticks(self, tb: &TimeBase) -> Tick {
        let step = tb.ticks_per_step();
        match self {
            Snap::None => 1,
            Snap::Sixth => (step / 6).max(1),
            Snap::Quarter => (step / 4).max(1),
            Snap::Half => (step / 2).max(1),
            Snap::Step => step,
            Snap::Beat => tb.ticks_per_beat(),
            Snap::Bar => tb.ticks_per_bar(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    Resize,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    note: Note,
    kind: DragKind,
    origin: Pos2,
}

/// Stato della vista Piano Roll.
pub struct PianoRoll {
    /// Pixel per tick.
    pub zoom_x: f32,
    /// Altezza di un tasto in pixel.
    pub key_h: f32,
    pub snap: Snap,
    pub new_length: Tick,
    pub velocity: f32,
    pub show_ghosts: bool,
    drag: Option<Drag>,
    scrolled: bool,
}

impl Default for PianoRoll {
    fn default() -> Self {
        Self {
            zoom_x: 0.5,
            key_h: 12.0,
            snap: Snap::Step,
            new_length: 24,
            velocity: 0.78,
            show_ghosts: true,
            drag: None,
            scrolled: false,
        }
    }
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

/// Applica il drag alla nota originale.
fn dragged_note(d: &Drag, delta: egui::Vec2, zoom_x: f32, key_h: f32, snap: Tick) -> Note {
    let mut n = d.note;
    let dt = (delta.x / zoom_x).round() as i64;
    match d.kind {
        DragKind::Move => {
            let pos = (n.position as i64 + dt).max(0) as Tick;
            n.position = TimeBase::quantize(pos, snap);
            let dk = (-delta.y / key_h).round() as i32;
            n.key = (n.key as i32 + dk).clamp(0, 127) as u8;
        }
        DragKind::Resize => {
            let end = (n.end() as i64 + dt).max(n.position as i64 + 1) as Tick;
            let end = TimeBase::quantize(end, snap).max(n.position + 1);
            n.length = end - n.position;
        }
    }
    n
}

/// Comando che sostituisce `old` con `new` nel pattern.
pub fn replace_note_cmd(pattern: PatternId, channel: ChannelId, old: &Note, new: Note) -> Command {
    Command::Batch {
        commands: vec![
            Command::RemoveNotes {
                pattern,
                channel,
                filter: NoteFilter { from: Some(old.position), to: Some(old.position + 1), key: Some(old.key) },
            },
            Command::AddNotes { pattern, channel, notes: vec![new] },
        ],
    }
}

impl PianoRoll {
    pub fn show(&mut self, ui: &mut egui::Ui, studio: &mut Studio) {
        let (Some(pattern), Some(channel)) = (studio.current_pattern(), studio.selected_channel) else {
            ui.label("Seleziona un canale nel Channel Rack per modificarne le note.");
            return;
        };
        let tb = studio.project.time;
        let snap = self.snap.ticks(&tb);

        // Barra strumenti.
        ui.horizontal(|ui| {
            let ch_name = studio.project.channel(channel).map(|c| c.name.clone()).unwrap_or_default();
            ui.strong(format!("Piano Roll — {ch_name}"));
            ui.separator();
            egui::ComboBox::from_id_salt("pr_snap").selected_text(self.snap.label()).show_ui(ui, |ui| {
                for s in Snap::ALL {
                    ui.selectable_value(&mut self.snap, s, s.label());
                }
            });
            ui.label("Vel");
            ui.add(egui::Slider::new(&mut self.velocity, 0.0..=1.0).show_value(false));
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.zoom_x, 0.1..=3.0).logarithmic(true).show_value(false));
            ui.checkbox(&mut self.show_ghosts, "Ghost");
            if ui.button("Quantizza").clicked() {
                studio.exec(Command::QuantizeNotes { pattern, channel: Some(channel), grid: snap, quantize_length: false });
            }
            if ui.button("Svuota").clicked() {
                studio.exec(Command::RemoveNotes { pattern, channel, filter: NoteFilter::default() });
            }
            for (label, st) in [("−12", -12), ("−1", -1), ("+1", 1), ("+12", 12)] {
                if ui.small_button(label).clicked() {
                    studio.exec(Command::TransposeNotes { pattern, channel: Some(channel), semitones: st });
                }
            }
        });

        let Some(pat) = studio.project.pattern(pattern) else { return };
        let len = pat.effective_length(&tb).max(tb.ticks_per_bar() * 4) + tb.ticks_per_bar();
        let width = KEYS_W + len as f32 * self.zoom_x;
        let height = 128.0 * self.key_h;
        let notes: Vec<Note> = pat.notes.get(&channel).cloned().unwrap_or_default();
        let ghosts: Vec<Note> = if self.show_ghosts {
            pat.notes.iter().filter(|(c, _)| **c != channel).flat_map(|(_, v)| v.iter().copied()).collect()
        } else {
            Vec::new()
        };
        let playhead = (!studio.song_mode && studio.engine.shared.is_playing()).then(|| studio.engine.shared.position());

        let mut scroll = egui::ScrollArea::both().id_salt("piano_roll_scroll").auto_shrink([false, false]);
        if !self.scrolled {
            // Centra sull'area C4–C6 al primo disegno.
            scroll = scroll.vertical_scroll_offset((127.0 - 84.0) * self.key_h);
            self.scrolled = true;
        }
        scroll.show(ui, |ui| {
            let (resp, painter) = ui.allocate_painter(vec2(width, height), Sense::click_and_drag());
            let origin = resp.rect.min;
            let x_of = |t: f64| origin.x + KEYS_W + t as f32 * self.zoom_x;
            let y_of = |k: u8| origin.y + (127 - k as i32) as f32 * self.key_h;
            let tick_at = |x: f32| (((x - origin.x - KEYS_W) / self.zoom_x).max(0.0)) as Tick;
            let key_at = |y: f32| (127 - ((y - origin.y) / self.key_h).floor() as i32).clamp(0, 127) as u8;
            let clip = ui.clip_rect();

            // Sfondo righe.
            let visual = ui.visuals().clone();
            let bg = visual.extreme_bg_color;
            let dark = bg.gamma_multiply(0.75);
            for k in 0..=127u8 {
                let y = y_of(k);
                if y + self.key_h < clip.top() || y > clip.bottom() {
                    continue;
                }
                let row = Rect::from_min_size(pos2(origin.x + KEYS_W, y), vec2(width - KEYS_W, self.key_h));
                painter.rect_filled(row, 0.0, if is_black(k) { dark } else { bg });
                // Tastiera.
                let key_rect = Rect::from_min_size(pos2(origin.x, y), vec2(KEYS_W, self.key_h));
                painter.rect_filled(key_rect, 0.0, if is_black(k) { Color32::from_gray(30) } else { Color32::from_gray(220) });
                painter.line_segment([key_rect.left_bottom(), key_rect.right_bottom()], Stroke::new(0.5, Color32::from_gray(90)));
                if k % 12 == 0 {
                    painter.text(
                        key_rect.right_center() - vec2(3.0, 0.0),
                        Align2::RIGHT_CENTER,
                        crate::views::key_name(k),
                        FontId::proportional(9.0),
                        Color32::from_gray(40),
                    );
                }
            }
            // Griglia verticale.
            let grid = snap.max(tb.ticks_per_step() / 4).max(1);
            let mut t = 0;
            while t <= len {
                let x = x_of(t as f64);
                if x >= clip.left() - 1.0 && x <= clip.right() + 1.0 {
                    let w = if t % tb.ticks_per_bar() == 0 {
                        1.5
                    } else if t % tb.ticks_per_beat() == 0 {
                        0.8
                    } else {
                        0.3
                    };
                    painter.line_segment([pos2(x, origin.y), pos2(x, origin.y + height)], Stroke::new(w, visual.weak_text_color()));
                }
                t += grid;
            }

            let note_rect = |n: &Note| {
                Rect::from_min_max(pos2(x_of(n.position as f64), y_of(n.key)), pos2(x_of(n.end() as f64), y_of(n.key) + self.key_h))
            };
            for g in &ghosts {
                painter.rect_filled(note_rect(g), 2.0, Color32::from_rgba_unmultiplied(120, 120, 120, 50));
            }

            // Note del canale (quella trascinata viene disegnata nella posizione provvisoria).
            let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
            let drag_preview = self.drag.as_ref().zip(pointer).map(|(d, p)| (d.note, dragged_note(d, p - d.origin, self.zoom_x, self.key_h, snap)));
            for n in &notes {
                let shown = match drag_preview {
                    Some((orig, moved)) if orig == *n => moved,
                    _ => *n,
                };
                let r = note_rect(&shown);
                let a = (0.35 + 0.65 * shown.velocity).clamp(0.0, 1.0);
                let fill = Color32::from_rgb((110.0 * a + 60.0) as u8, (200.0 * a + 40.0) as u8, (120.0 * a + 50.0) as u8);
                painter.rect_filled(r, 2.0, fill);
                painter.rect_stroke(r, 2.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Inside);
                if shown.slide {
                    painter.text(r.left_center() + vec2(3.0, 0.0), Align2::LEFT_CENTER, "↗", FontId::proportional(9.0), Color32::BLACK);
                }
            }
            if let Some(ph) = playhead {
                let x = x_of(ph);
                painter.line_segment([pos2(x, origin.y), pos2(x, origin.y + height)], Stroke::new(1.5, Color32::from_rgb(255, 120, 60)));
            }

            // Interazione.
            let hit = |p: Pos2| notes.iter().rev().find(|n| note_rect(n).contains(p)).copied();
            if resp.drag_started() {
                if let Some(p) = resp.interact_pointer_pos() {
                    if let Some(n) = hit(p) {
                        let kind = if p.x > note_rect(&n).right() - RESIZE_PX { DragKind::Resize } else { DragKind::Move };
                        self.drag = Some(Drag { note: n, kind, origin: p });
                    }
                }
            }
            if resp.drag_stopped() {
                if let (Some(d), Some(p)) = (self.drag.take(), resp.interact_pointer_pos()) {
                    let moved = dragged_note(&d, p - d.origin, self.zoom_x, self.key_h, snap);
                    if moved != d.note {
                        studio.exec(replace_note_cmd(pattern, channel, &d.note, moved));
                        self.new_length = moved.length;
                    }
                }
            }
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    if p.x > origin.x + KEYS_W {
                        if hit(p).is_none() {
                            let pos = tick_at(p.x) / snap * snap;
                            let key = key_at(p.y);
                            let note = Note::new(pos, self.new_length.max(1), key, self.velocity);
                            studio.exec(Command::AddNotes { pattern, channel, notes: vec![note] });
                            studio.engine.preview_on(channel, key, self.velocity);
                            studio.engine.preview_off(channel, key);
                        }
                    } else {
                        // Click sulla tastiera = anteprima.
                        let key = key_at(p.y);
                        studio.engine.preview_on(channel, key, self.velocity);
                        studio.engine.preview_off(channel, key);
                    }
                }
            }
            if resp.secondary_clicked() {
                if let Some(n) = resp.interact_pointer_pos().and_then(hit) {
                    studio.exec(Command::RemoveNotes {
                        pattern,
                        channel,
                        filter: NoteFilter { from: Some(n.position), to: Some(n.position + 1), key: Some(n.key) },
                    });
                }
            }
            // Cursore contestuale.
            if let Some(p) = resp.hover_pos() {
                if let Some(n) = hit(p) {
                    let icon = if p.x > note_rect(&n).right() - RESIZE_PX {
                        egui::CursorIcon::ResizeHorizontal
                    } else {
                        egui::CursorIcon::Grab
                    };
                    ui.ctx().set_cursor_icon(icon);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_math() {
        let d = Drag { note: Note::new(24, 24, 60, 1.0), kind: DragKind::Move, origin: pos2(0.0, 0.0) };
        // 48 px a zoom 1 = 48 tick a destra, 2 tasti su.
        let n = dragged_note(&d, vec2(48.0, -24.0), 1.0, 12.0, 24);
        assert_eq!((n.position, n.key), (72, 62));
        let d = Drag { kind: DragKind::Resize, ..d };
        let n = dragged_note(&d, vec2(-100.0, 0.0), 1.0, 12.0, 24);
        assert!(n.length >= 1 && n.position == 24);
    }

    #[test]
    fn snaps() {
        let tb = TimeBase::default();
        assert_eq!(Snap::Step.ticks(&tb), 24);
        assert_eq!(Snap::Bar.ticks(&tb), 384);
        assert_eq!(Snap::Sixth.ticks(&tb), 4);
    }
}
