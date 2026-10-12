//! Piano Roll: editing delle note del canale selezionato nel pattern corrente.
//!
//! Click su area vuota = nuova nota; trascina il corpo = sposta; trascina il bordo destro =
//! ridimensiona; click destro = elimina. Le modifiche diventano un solo comando al rilascio.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use mfc_core::{ChannelId, Command, Note, NoteFilter, PatternId, Tick, TimeBase};

use crate::state::Studio;
use crate::theme::{self, pal};
use crate::views::widgets::{self, Icon};

const KEYS_W: f32 = 56.0;
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
    pub const ALL: [Snap; 7] = [
        Snap::None,
        Snap::Sixth,
        Snap::Quarter,
        Snap::Half,
        Snap::Step,
        Snap::Beat,
        Snap::Bar,
    ];
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
            zoom_x: 0.75,
            key_h: 14.0,
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
                filter: NoteFilter {
                    from: Some(old.position),
                    to: Some(old.position + 1),
                    key: Some(old.key),
                },
            },
            Command::AddNotes {
                pattern,
                channel,
                notes: vec![new],
            },
        ],
    }
}

const RULER_H: f32 = 22.0;
const VEL_H: f32 = 76.0;

impl PianoRoll {
    pub fn show(&mut self, ui: &mut egui::Ui, studio: &mut Studio) {
        let (Some(pattern), Some(channel)) = (studio.current_pattern(), studio.selected_channel) else {
            widgets::view_header(ui, Icon::Piano, "Piano Roll", "", |_| {});
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Seleziona un canale nel Channel Rack per modificarne le note.").color(pal::TEXT_DIM),
                );
            });
            return;
        };
        let tb = studio.project.time;
        let snap = self.snap.ticks(&tb);
        let ch_index = studio
            .project
            .channels
            .iter()
            .position(|c| c.id == channel)
            .unwrap_or(0);
        let (ch_name, ch_color) = studio
            .project
            .channel(channel)
            .map(|c| (c.name.clone(), theme::item_color(c.color, ch_index)))
            .unwrap_or_default();
        let pat_name = studio
            .project
            .pattern(pattern)
            .map(|p| p.name.clone())
            .unwrap_or_default();

        widgets::view_header(
            ui,
            Icon::Piano,
            "Piano Roll",
            &format!("{ch_name}  ·  {pat_name}"),
            |ui| {
                for (label, st) in [("+12", 12), ("+1", 1), ("−1", -1), ("−12", -12)] {
                    if ui
                        .add(egui::Button::new(RichText::new(label).size(11.0)).min_size(vec2(30.0, 22.0)))
                        .on_hover_text("Trasponi")
                        .clicked()
                    {
                        studio.exec(Command::TransposeNotes {
                            pattern,
                            channel: Some(channel),
                            semitones: st,
                        });
                    }
                }
                ui.add_space(6.0);
                if ui.button("Svuota").clicked() {
                    studio.exec(Command::RemoveNotes {
                        pattern,
                        channel,
                        filter: NoteFilter::default(),
                    });
                }
                if ui.button("Quantizza").clicked() {
                    studio.exec(Command::QuantizeNotes {
                        pattern,
                        channel: Some(channel),
                        grid: snap,
                        quantize_length: false,
                    });
                }
                ui.add_space(6.0);
                let ghost = widgets::toggle(ui, self.show_ghosts, "GHOST", pal::BLUE, vec2(56.0, 22.0))
                    .on_hover_text("Mostra le note degli altri canali");
                if ghost.clicked() {
                    self.show_ghosts = !self.show_ghosts;
                }
                ui.add(
                    egui::Slider::new(&mut self.zoom_x, 0.1..=3.0)
                        .logarithmic(true)
                        .show_value(false),
                );
                ui.label(RichText::new("ZOOM").font(theme::bold_font(9.5)).color(pal::TEXT_FAINT));
                egui::ComboBox::from_id_salt("pr_snap")
                    .width(96.0)
                    .selected_text(self.snap.label())
                    .show_ui(ui, |ui| {
                        for s in Snap::ALL {
                            ui.selectable_value(&mut self.snap, s, s.label());
                        }
                    });
                ui.label(RichText::new("SNAP").font(theme::bold_font(9.5)).color(pal::TEXT_FAINT));
            },
        );

        let Some(pat) = studio.project.pattern(pattern) else {
            return;
        };
        let len = pat.effective_length(&tb).max(tb.ticks_per_bar() * 4) + tb.ticks_per_bar();
        let width = KEYS_W + len as f32 * self.zoom_x;
        let notes: Vec<Note> = pat.notes.get(&channel).cloned().unwrap_or_default();
        let ghosts: Vec<Note> = if self.show_ghosts {
            pat.notes
                .iter()
                .filter(|(c, _)| **c != channel)
                .flat_map(|(_, v)| v.iter().copied())
                .collect()
        } else {
            Vec::new()
        };
        let playhead =
            (!studio.song_mode && studio.engine.shared.is_playing()).then(|| studio.engine.shared.position());

        // Area note (in alto) + lane velocity (in basso, fissa).
        let avail = ui.available_rect_before_wrap();
        let vel_rect = Rect::from_min_max(pos2(avail.left(), avail.bottom() - VEL_H), avail.right_bottom());
        let grid_rect = Rect::from_min_max(avail.left_top(), pos2(avail.right(), vel_rect.top()));
        let mut grid_ui = ui.new_child(egui::UiBuilder::new().max_rect(grid_rect));
        let height = RULER_H + 128.0 * self.key_h;
        let mut scroll = egui::ScrollArea::both()
            .id_salt("piano_roll_scroll")
            .auto_shrink([false, false]);
        if !self.scrolled {
            scroll = scroll.vertical_scroll_offset((127.0 - 84.0) * self.key_h);
            self.scrolled = true;
        }
        let mut h_offset = 0.0;
        let out = scroll.show(&mut grid_ui, |ui| {
            let (resp, painter) = ui.allocate_painter(vec2(width, height), Sense::click_and_drag());
            let clip = ui.clip_rect();
            let origin = resp.rect.min;
            // Elementi "sticky": tastiera a sinistra e righello in alto restano visibili.
            let keys_x = clip.left().max(origin.x);
            let ruler_y = clip.top().max(origin.y);
            let notes_top = origin.y + RULER_H;
            let x_of = |t: f64| origin.x + KEYS_W + t as f32 * self.zoom_x;
            let y_of = |k: u8| notes_top + (127 - k as i32) as f32 * self.key_h;
            let tick_at = |x: f32| (((x - origin.x - KEYS_W) / self.zoom_x).max(0.0)) as Tick;
            let key_at = |y: f32| (127 - ((y - notes_top) / self.key_h).floor() as i32).clamp(0, 127) as u8;

            // Sfondo: battute alternate e righe dei tasti neri più scure.
            let bar = tb.ticks_per_bar();
            let mut b = 0;
            while b * bar < len {
                let x0 = x_of((b * bar) as f64);
                let x1 = x_of(((b + 1) * bar) as f64);
                let c = if b % 2 == 0 {
                    Color32::from_rgb(0x1d, 0x21, 0x26)
                } else {
                    Color32::from_rgb(0x19, 0x1c, 0x21)
                };
                painter.rect_filled(
                    Rect::from_min_max(pos2(x0, notes_top), pos2(x1, origin.y + height)),
                    0.0,
                    c,
                );
                b += 1;
            }
            for k in 0..=127u8 {
                let y = y_of(k);
                if y + self.key_h < clip.top() || y > clip.bottom() {
                    continue;
                }
                let row = Rect::from_min_size(pos2(origin.x + KEYS_W, y), vec2(width - KEYS_W, self.key_h));
                if is_black(k) {
                    painter.rect_filled(row, 0.0, Color32::from_black_alpha(55));
                }
                if k % 12 == 0 {
                    painter.line_segment(
                        [row.left_bottom(), row.right_bottom()],
                        Stroke::new(1.0, Color32::from_rgb(0x2c, 0x31, 0x37)),
                    );
                }
            }
            // Griglia verticale.
            let grid = snap.max(tb.ticks_per_step() / 2).max(1);
            let mut t = 0;
            while t <= len {
                let x = x_of(t as f64);
                if x >= clip.left() - 1.0 && x <= clip.right() + 1.0 {
                    let (w, c) = if t % bar == 0 {
                        (1.0, Color32::from_rgb(0x3a, 0x40, 0x48))
                    } else if t % tb.ticks_per_beat() == 0 {
                        (1.0, Color32::from_rgb(0x2a, 0x2f, 0x35))
                    } else {
                        (1.0, Color32::from_rgb(0x22, 0x26, 0x2b))
                    };
                    painter.line_segment([pos2(x, notes_top), pos2(x, origin.y + height)], Stroke::new(w, c));
                }
                t += grid;
            }

            let note_rect = |n: &Note| {
                Rect::from_min_max(
                    pos2(x_of(n.position as f64) + 0.5, y_of(n.key) + 0.5),
                    pos2(x_of(n.end() as f64) - 0.5, y_of(n.key) + self.key_h - 0.5),
                )
            };
            for g in &ghosts {
                painter.rect_filled(note_rect(g), 2.0, Color32::from_white_alpha(16));
            }

            // Note (quella trascinata appare nella posizione provvisoria).
            let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
            let drag_preview = self
                .drag
                .as_ref()
                .zip(pointer)
                .map(|(d, p)| (d.note, dragged_note(d, p - d.origin, self.zoom_x, self.key_h, snap)));
            for n in &notes {
                let shown = match drag_preview {
                    Some((orig, moved)) if orig == *n => moved,
                    _ => *n,
                };
                let r = note_rect(&shown);
                let fill = theme::mix(
                    theme::mix(ch_color, Color32::BLACK, 0.55),
                    ch_color,
                    0.35 + 0.65 * shown.velocity,
                );
                widgets::gradient_rect(&painter, r, 2.5, theme::mix(fill, Color32::WHITE, 0.18), fill);
                painter.rect_stroke(
                    r,
                    2.5,
                    Stroke::new(1.0, theme::mix(ch_color, Color32::BLACK, 0.6)),
                    StrokeKind::Inside,
                );
                if r.width() > 28.0 && self.key_h >= 10.0 {
                    painter.with_clip_rect(r).text(
                        r.left_center() + vec2(4.0, 0.0),
                        Align2::LEFT_CENTER,
                        crate::views::key_name(shown.key),
                        FontId::proportional((self.key_h - 3.0).min(10.5)),
                        Color32::from_black_alpha(200),
                    );
                }
                if shown.slide {
                    painter.text(
                        r.right_center() - vec2(4.0, 0.0),
                        Align2::RIGHT_CENTER,
                        "↗",
                        FontId::proportional(9.0),
                        Color32::BLACK,
                    );
                }
            }
            if let Some(ph) = playhead {
                let x = x_of(ph);
                painter.line_segment(
                    [pos2(x, notes_top), pos2(x, origin.y + height)],
                    Stroke::new(1.5, pal::ACCENT),
                );
            }

            // Tastiera (sticky a sinistra).
            let kb = Rect::from_min_max(pos2(keys_x, notes_top), pos2(keys_x + KEYS_W, origin.y + height));
            painter.rect_filled(kb, 0.0, Color32::from_rgb(0xd8, 0xdc, 0xe0));
            for k in 0..=127u8 {
                let y = y_of(k);
                if y + self.key_h < clip.top() || y > clip.bottom() {
                    continue;
                }
                let key_rect = Rect::from_min_size(pos2(keys_x, y), vec2(KEYS_W, self.key_h));
                if is_black(k) {
                    let br = Rect::from_min_size(key_rect.min, vec2(KEYS_W * 0.62, self.key_h));
                    painter.rect_filled(
                        br,
                        egui::CornerRadius {
                            ne: 2,
                            se: 2,
                            nw: 0,
                            sw: 0,
                        },
                        Color32::from_rgb(0x1a, 0x1d, 0x21),
                    );
                } else if matches!(k % 12, 0 | 5) {
                    painter.line_segment(
                        [key_rect.left_bottom(), key_rect.right_bottom()],
                        Stroke::new(1.0, Color32::from_gray(150)),
                    );
                }
                if k % 12 == 0 {
                    painter.text(
                        key_rect.right_center() - vec2(4.0, 0.0),
                        Align2::RIGHT_CENTER,
                        crate::views::key_name(k),
                        FontId::proportional(9.0),
                        Color32::from_gray(70),
                    );
                }
            }
            painter.line_segment([kb.right_top(), kb.right_bottom()], Stroke::new(1.0, pal::BORDER));

            // Righello (sticky in alto).
            let ruler = Rect::from_min_max(pos2(origin.x, ruler_y), pos2(origin.x + width, ruler_y + RULER_H));
            painter.rect_filled(ruler, 0.0, pal::BG_RAISED);
            painter.line_segment(
                [ruler.left_bottom(), ruler.right_bottom()],
                Stroke::new(1.0, pal::BORDER),
            );
            let mut b = 0;
            while b * bar <= len {
                let x = x_of((b * bar) as f64);
                painter.line_segment(
                    [pos2(x, ruler.bottom() - 8.0), pos2(x, ruler.bottom())],
                    Stroke::new(1.0, pal::TEXT_FAINT),
                );
                painter.text(
                    pos2(x + 4.0, ruler.center().y),
                    Align2::LEFT_CENTER,
                    (b + 1).to_string(),
                    theme::bold_font(10.5),
                    pal::TEXT_DIM,
                );
                for q in 1..tb.signature.numerator as u64 {
                    let xb = x_of((b * bar + q * tb.ticks_per_beat()) as f64);
                    painter.line_segment(
                        [pos2(xb, ruler.bottom() - 4.0), pos2(xb, ruler.bottom())],
                        Stroke::new(1.0, pal::LINE),
                    );
                }
                b += 1;
            }
            painter.rect_filled(
                Rect::from_min_size(pos2(keys_x, ruler_y), vec2(KEYS_W, RULER_H)),
                0.0,
                pal::BG_RAISED,
            );
            if let Some(ph) = playhead {
                let x = x_of(ph);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        pos2(x - 5.0, ruler.top() + 4.0),
                        pos2(x + 5.0, ruler.top() + 4.0),
                        pos2(x, ruler.bottom() - 2.0),
                    ],
                    pal::ACCENT,
                    Stroke::NONE,
                ));
            }

            // Interazione.
            let in_keys = |p: Pos2| p.x < keys_x + KEYS_W;
            let in_ruler = |p: Pos2| p.y < ruler_y + RULER_H;
            let hit = |p: Pos2| notes.iter().rev().find(|n| note_rect(n).contains(p)).copied();
            if resp.drag_started() {
                if let Some(p) = resp.interact_pointer_pos().filter(|p| !in_keys(*p) && !in_ruler(*p)) {
                    if let Some(n) = hit(p) {
                        let kind = if p.x > note_rect(&n).right() - RESIZE_PX {
                            DragKind::Resize
                        } else {
                            DragKind::Move
                        };
                        self.drag = Some(Drag {
                            note: n,
                            kind,
                            origin: p,
                        });
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
                    if in_ruler(p) {
                        // Click sul righello: niente (il transport in modalità pattern riparte dal loop).
                    } else if in_keys(p) {
                        let key = key_at(p.y);
                        studio.engine.preview_on(channel, key, self.velocity);
                        studio.engine.preview_off(channel, key);
                    } else if hit(p).is_none() {
                        let pos = tick_at(p.x) / snap * snap;
                        let key = key_at(p.y);
                        let note = Note::new(pos, self.new_length.max(1), key, self.velocity);
                        studio.exec(Command::AddNotes {
                            pattern,
                            channel,
                            notes: vec![note],
                        });
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
                        filter: NoteFilter {
                            from: Some(n.position),
                            to: Some(n.position + 1),
                            key: Some(n.key),
                        },
                    });
                }
            }
            if let Some(p) = resp.hover_pos() {
                if in_keys(p) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                } else if let Some(n) = hit(p) {
                    let icon = if p.x > note_rect(&n).right() - RESIZE_PX {
                        egui::CursorIcon::ResizeHorizontal
                    } else {
                        egui::CursorIcon::Grab
                    };
                    ui.ctx().set_cursor_icon(icon);
                }
            }
        });
        h_offset += out.state.offset.x;

        // Lane della velocity: trascina in verticale su una nota per cambiarne la velocity.
        self.velocity_lane(ui, studio, vel_rect, pattern, channel, &notes, ch_color, h_offset);
        ui.advance_cursor_after_rect(avail);
    }

    #[allow(clippy::too_many_arguments)]
    fn velocity_lane(
        &mut self,
        ui: &mut egui::Ui,
        studio: &mut Studio,
        rect: Rect,
        pattern: PatternId,
        channel: ChannelId,
        notes: &[Note],
        color: Color32,
        h_offset: f32,
    ) {
        let resp = ui.interact(rect, ui.id().with("vel_lane"), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, pal::BG_RAISED);
        p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, pal::BORDER));
        let lane = Rect::from_min_max(
            pos2(rect.left() + KEYS_W, rect.top() + 8.0),
            rect.right_bottom() - vec2(0.0, 6.0),
        );
        p.text(
            pos2(rect.left() + 8.0, rect.top() + 12.0),
            Align2::LEFT_TOP,
            "VEL",
            theme::bold_font(9.5),
            pal::TEXT_FAINT,
        );
        for frac in [0.25f32, 0.5, 0.75] {
            let y = lane.bottom() - lane.height() * frac;
            p.line_segment(
                [pos2(lane.left(), y), pos2(lane.right(), y)],
                Stroke::new(1.0, Color32::from_rgb(0x2a, 0x2f, 0x35)),
            );
        }
        let x_of = |t: Tick| lane.left() - h_offset + t as f32 * self.zoom_x;
        let id = ui.id().with("vel_edit");
        let editing: Option<(Tick, u8, f32)> = ui.data(|d| d.get_temp(id));
        for n in notes {
            let x = x_of(n.position);
            if x < lane.left() - 4.0 || x > lane.right() + 4.0 {
                continue;
            }
            let v = match editing {
                Some((pos, key, v)) if pos == n.position && key == n.key => v,
                _ => n.velocity,
            };
            let top = lane.bottom() - lane.height() * v;
            p.line_segment(
                [pos2(x + 1.5, lane.bottom()), pos2(x + 1.5, top)],
                Stroke::new(2.0, color.gamma_multiply(0.8)),
            );
            p.circle_filled(pos2(x + 1.5, top), 3.5, color);
        }
        if let Some(pt) = resp.interact_pointer_pos() {
            let v = ((lane.bottom() - pt.y) / lane.height()).clamp(0.0, 1.0);
            if resp.drag_started() || resp.clicked() || editing.is_none() && resp.dragged() {
                if let Some(n) = notes.iter().min_by(|a, b| {
                    (x_of(a.position) - pt.x)
                        .abs()
                        .total_cmp(&(x_of(b.position) - pt.x).abs())
                }) {
                    if (x_of(n.position) - pt.x).abs() < 12.0 {
                        ui.data_mut(|d| d.insert_temp(id, (n.position, n.key, v)));
                    }
                }
            } else if let Some((pos, key, _)) = editing {
                ui.data_mut(|d| d.insert_temp(id, (pos, key, v)));
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            if let Some((pos, key, v)) = ui.data(|d| d.get_temp::<(Tick, u8, f32)>(id)) {
                studio.exec(Command::SetNoteVelocity {
                    pattern,
                    channel,
                    filter: NoteFilter {
                        from: Some(pos),
                        to: Some(pos + 1),
                        key: Some(key),
                    },
                    velocity: v,
                });
                self.velocity = v;
            }
            ui.data_mut(|d| d.remove::<(Tick, u8, f32)>(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_math() {
        let d = Drag {
            note: Note::new(24, 24, 60, 1.0),
            kind: DragKind::Move,
            origin: pos2(0.0, 0.0),
        };
        // 48 px a zoom 1 = 48 tick a destra, 2 tasti su.
        let n = dragged_note(&d, vec2(48.0, -24.0), 1.0, 12.0, 24);
        assert_eq!((n.position, n.key), (72, 62));
        let d = Drag {
            kind: DragKind::Resize,
            ..d
        };
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
