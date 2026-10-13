//! Playlist: arrangiamento di pattern, audio clip e automazioni su tracce.
//!
//! Click su area vuota = piazza il pattern corrente; trascina = sposta; bordo destro =
//! ridimensiona; click destro = elimina; click sul righello = sposta la testina.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use mfc_core::{ClipId, ClipSource, Command, PlaylistClip, Tick, TimeBase, PLAYLIST_TRACKS};

use crate::state::Studio;
use crate::theme::{self, pal};
use crate::views::widgets::{self, Icon};

const HEADER_W: f32 = 132.0;
const RULER_H: f32 = 22.0;
const RESIZE_PX: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    Resize,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    clip: PlaylistClip,
    kind: DragKind,
    origin: Pos2,
}

/// Stato della vista Playlist.
pub struct PlaylistView {
    pub zoom_x: f32,
    pub track_h: f32,
    /// Aggancio in tick (0 = battuta).
    pub snap_beats: bool,
    drag: Option<Drag>,
}

impl Default for PlaylistView {
    fn default() -> Self {
        Self {
            zoom_x: 0.16,
            track_h: 40.0,
            snap_beats: false,
            drag: None,
        }
    }
}

fn dragged(d: &Drag, delta: egui::Vec2, zoom_x: f32, track_h: f32, snap: Tick) -> PlaylistClip {
    let mut c = d.clip;
    let dt = (delta.x / zoom_x).round() as i64;
    match d.kind {
        DragKind::Move => {
            c.position = TimeBase::quantize((c.position as i64 + dt).max(0) as Tick, snap);
            let dtr = (delta.y / track_h).round() as i32;
            c.track = (c.track as i32 + dtr).clamp(0, PLAYLIST_TRACKS as i32 - 1) as u16;
        }
        DragKind::Resize => {
            let end = TimeBase::quantize((c.end() as i64 + dt).max(c.position as i64 + 1) as Tick, snap);
            c.length = end.saturating_sub(c.position).max(snap.max(1));
        }
    }
    c
}

/// Colore di una clip: quello del pattern/canale sorgente.
fn source_color(project: &mfc_core::Project, src: &ClipSource) -> Color32 {
    match src {
        ClipSource::Pattern(id) => project
            .patterns
            .iter()
            .position(|p| p.id == *id)
            .map(|i| theme::item_color(project.patterns[i].color, i))
            .unwrap_or(pal::ACCENT()),
        ClipSource::Audio(id) => project
            .channels
            .iter()
            .position(|c| c.id == *id)
            .map(|i| theme::item_color(project.channels[i].color, i))
            .unwrap_or(pal::GREEN()),
        ClipSource::Automation(_) => pal::YELLOW(),
    }
}

impl PlaylistView {
    pub fn show(&mut self, ui: &mut egui::Ui, studio: &mut Studio) {
        let tb = studio.project.time;
        let snap = if self.snap_beats {
            tb.ticks_per_beat()
        } else {
            tb.ticks_per_bar()
        };
        let pat_name = studio
            .current_pattern()
            .and_then(|p| studio.project.pattern(p))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let bars = studio.project.song_length() as f64 / tb.ticks_per_bar() as f64;
        widgets::view_header(
            ui,
            Icon::Playlist,
            "Playlist",
            &format!("Arrangiamento · {bars:.0} battute"),
            |ui| {
                ui.label(
                    RichText::new(format!("Click per piazzare: {pat_name}"))
                        .size(11.0)
                        .color(pal::TEXT_DIM()),
                );
                ui.add_space(8.0);
                let r = widgets::toggle(
                    ui,
                    self.snap_beats,
                    if self.snap_beats { "SNAP BEAT" } else { "SNAP BAR" },
                    pal::BLUE(),
                    vec2(78.0, 22.0),
                );
                if r.clicked() {
                    self.snap_beats = !self.snap_beats;
                }
                ui.add(
                    egui::Slider::new(&mut self.zoom_x, 0.02..=1.0)
                        .logarithmic(true)
                        .show_value(false),
                );
                ui.label(
                    RichText::new("ZOOM")
                        .font(theme::bold_font(9.5))
                        .color(pal::TEXT_FAINT()),
                );
            },
        );

        let used_tracks = studio
            .project
            .playlist
            .clips
            .iter()
            .map(|c| c.track as usize + 1)
            .max()
            .unwrap_or(0);
        let n_tracks = (used_tracks + 8).clamp(16, PLAYLIST_TRACKS);
        let end = studio.project.song_length().max(tb.ticks_per_bar() * 16) + tb.ticks_per_bar() * 8;
        let width = HEADER_W + end as f32 * self.zoom_x;
        let height = RULER_H + n_tracks as f32 * self.track_h;
        let playhead =
            (studio.song_mode || !studio.engine.shared.is_playing()).then(|| studio.engine.shared.position());

        egui::ScrollArea::both()
            .id_salt("playlist_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (resp, painter) = ui.allocate_painter(vec2(width, height), Sense::click_and_drag());
                let o = resp.rect.min;
                let clip_r = ui.clip_rect();
                let head_x = clip_r.left().max(o.x);
                let ruler_y = clip_r.top().max(o.y);
                let x_of = |t: f64| o.x + HEADER_W + t as f32 * self.zoom_x;
                let y_of = |tr: u16| o.y + RULER_H + tr as f32 * self.track_h;
                let tick_at = |x: f32| ((x - o.x - HEADER_W) / self.zoom_x).max(0.0) as Tick;
                let track_at = |y: f32| {
                    (((y - o.y - RULER_H) / self.track_h).floor().max(0.0) as u16).min(PLAYLIST_TRACKS as u16 - 1)
                };
                let bar = tb.ticks_per_bar();

                // Campione trascinato dal browser: audio clip alla posizione (agganciata).
                if let Some((path, at)) =
                    crate::views::samples::drop_target(ui, resp.rect.intersect(clip_r), "Rilascia: audio clip")
                {
                    let tick = tick_at(at.x) / snap.max(1) * snap.max(1);
                    crate::views::samples::add_audio_clip(studio, &path, track_at(at.y), tick);
                }

                // Sfondo: battute alternate, poi righe delle tracce.
                let mut b = 0;
                while b * bar < end {
                    let x0 = x_of((b * bar) as f64);
                    let x1 = x_of(((b + 1) * bar) as f64);
                    if x1 >= clip_r.left() && x0 <= clip_r.right() {
                        let c = if (b / 4) % 2 == 0 { pal::GRID_A() } else { pal::GRID_B() };
                        painter.rect_filled(
                            Rect::from_min_max(pos2(x0, o.y + RULER_H), pos2(x1, o.y + height)),
                            0.0,
                            c,
                        );
                        painter.line_segment(
                            [pos2(x0, o.y + RULER_H), pos2(x0, o.y + height)],
                            Stroke::new(1.0, pal::GRID_LINE()),
                        );
                        if self.zoom_x * tb.ticks_per_beat() as f32 > 10.0 {
                            for q in 1..tb.signature.numerator as u64 {
                                let xb = x_of((b * bar + q * tb.ticks_per_beat()) as f64);
                                painter.line_segment(
                                    [pos2(xb, o.y + RULER_H), pos2(xb, o.y + height)],
                                    Stroke::new(1.0, theme::mix(pal::GRID_A(), pal::GRID_LINE(), 0.5)),
                                );
                            }
                        }
                    }
                    b += 1;
                }
                for tr in 0..n_tracks as u16 {
                    let y = y_of(tr) + self.track_h;
                    painter.line_segment([pos2(o.x, y), pos2(o.x + width, y)], Stroke::new(1.0, pal::BG_INSET()));
                }

                // Clip.
                let clips = studio.project.playlist.clips.clone();
                let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
                let preview = self
                    .drag
                    .as_ref()
                    .zip(pointer)
                    .map(|(d, p)| (d.clip.id, dragged(d, p - d.origin, self.zoom_x, self.track_h, snap)));
                let rect_of = |c: &PlaylistClip| {
                    Rect::from_min_max(
                        pos2(x_of(c.position as f64) + 1.0, y_of(c.track) + 2.0),
                        pos2(x_of(c.end() as f64) - 1.0, y_of(c.track) + self.track_h - 2.0),
                    )
                };
                for c in &clips {
                    let shown = match preview {
                        Some((id, moved)) if id == c.id => moved,
                        _ => *c,
                    };
                    let r = rect_of(&shown);
                    if r.right() < clip_r.left() || r.left() > clip_r.right() {
                        continue;
                    }
                    let base = source_color(&studio.project, &c.source);
                    let muted = c.muted || studio.project.playlist.track_muted(c.track);
                    let col = if muted {
                        theme::mix(base, pal::BG_PANEL(), 0.7)
                    } else {
                        base
                    };
                    let body = theme::mix(col, pal::BG_INSET(), 0.62);
                    painter.rect_filled(r, 4.0, body);
                    let head = Rect::from_min_size(r.min, vec2(r.width(), 14.0f32.min(r.height())));
                    painter.rect_filled(
                        head,
                        egui::CornerRadius {
                            nw: 4,
                            ne: 4,
                            sw: 0,
                            se: 0,
                        },
                        col,
                    );
                    painter.rect_stroke(
                        r,
                        4.0,
                        Stroke::new(1.0, theme::mix(col, Color32::BLACK, 0.3)),
                        StrokeKind::Inside,
                    );
                    let label = match c.source {
                        ClipSource::Pattern(p) => studio.project.pattern(p).map(|x| x.name.clone()).unwrap_or_default(),
                        ClipSource::Audio(ch) => studio.project.channel(ch).map(|x| x.name.clone()).unwrap_or_default(),
                        ClipSource::Automation(a) => {
                            studio.project.automation(a).map(|x| x.name.clone()).unwrap_or_default()
                        }
                    };
                    painter.with_clip_rect(head.shrink(1.0)).text(
                        head.left_center() + vec2(5.0, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        theme::bold_font(10.0),
                        Color32::from_black_alpha(220),
                    );
                    let body_r = Rect::from_min_max(pos2(r.left() + 2.0, head.bottom() + 2.0), r.max - vec2(2.0, 3.0));
                    let p2 = painter.with_clip_rect(body_r.intersect(clip_r));
                    match c.source {
                        ClipSource::Pattern(pid) => {
                            if let Some(p) = studio.project.pattern(pid) {
                                let plen = p.effective_length(&tb).max(1) as f32;
                                let notes: Vec<_> = p.notes.values().flatten().collect();
                                let (lo, hi) = notes
                                    .iter()
                                    .fold((127u8, 0u8), |(a, b), n| (a.min(n.key), b.max(n.key)));
                                let span = (hi.saturating_sub(lo) as f32).max(1.0);
                                let note_col = theme::mix(col, Color32::WHITE, 0.25);
                                let mut rep = 0.0;
                                while rep < shown.length as f32 {
                                    for n in &notes {
                                        let x0 = body_r.left()
                                            + (rep + n.position as f32 - shown.offset as f32) * self.zoom_x;
                                        if x0 > body_r.right() || x0 < body_r.left() - 1.0 {
                                            continue;
                                        }
                                        let x1 = (x0 + n.length as f32 * self.zoom_x - 1.0).min(body_r.right());
                                        let y = body_r.bottom()
                                            - 1.0
                                            - (n.key - lo) as f32 / span * (body_r.height() - 3.0);
                                        p2.line_segment(
                                            [pos2(x0, y), pos2(x1.max(x0 + 1.5), y)],
                                            Stroke::new(2.0, note_col),
                                        );
                                    }
                                    rep += plen;
                                }
                            }
                        }
                        ClipSource::Automation(aid) => {
                            if let Some(a) = studio.project.automation(aid) {
                                let pts: Vec<Pos2> = (0..=48)
                                    .map(|i| {
                                        let t = i as f64 / 48.0 * shown.length as f64;
                                        let v = a.value_at(t + shown.offset as f64).unwrap_or(0.0);
                                        pos2(
                                            body_r.left() + t as f32 * self.zoom_x,
                                            body_r.bottom() - v * body_r.height(),
                                        )
                                    })
                                    .collect();
                                p2.add(egui::Shape::line(
                                    pts,
                                    Stroke::new(1.5, theme::mix(col, Color32::WHITE, 0.3)),
                                ));
                            }
                        }
                        ClipSource::Audio(_) => {
                            widgets::paint_icon(
                                &p2,
                                Rect::from_center_size(body_r.center(), vec2(body_r.height(), body_r.height())),
                                Icon::Wave,
                                theme::mix(col, Color32::WHITE, 0.3),
                            );
                        }
                    }
                }
                if let Some(ph) = playhead {
                    let x = x_of(ph);
                    painter.line_segment(
                        [pos2(x, o.y + RULER_H), pos2(x, o.y + height)],
                        Stroke::new(1.5, pal::ACCENT()),
                    );
                }

                // Intestazioni delle tracce (sticky a sinistra).
                for tr in 0..n_tracks as u16 {
                    let y = y_of(tr);
                    if y + self.track_h < clip_r.top() || y > clip_r.bottom() {
                        continue;
                    }
                    let header = Rect::from_min_size(pos2(head_x, y), vec2(HEADER_W, self.track_h));
                    let track = studio.project.playlist.tracks.get(&tr);
                    let muted = track.is_some_and(|t| t.muted);
                    let used = clips.iter().any(|c| c.track == tr);
                    let name = track
                        .map(|t| t.name.clone())
                        .unwrap_or_else(|| format!("Track {}", tr + 1));
                    painter.rect_filled(header, 0.0, pal::BG_RAISED());
                    painter.line_segment(
                        [header.left_bottom(), header.right_bottom()],
                        Stroke::new(1.0, pal::BORDER()),
                    );
                    let led = pos2(header.left() + 12.0, header.center().y);
                    painter.circle_filled(
                        led,
                        4.0,
                        if muted {
                            theme::mix(pal::BG_INSET(), pal::BG_HOVER(), 0.5)
                        } else {
                            pal::GREEN()
                        },
                    );
                    let tc = if muted {
                        pal::TEXT_FAINT()
                    } else if used {
                        pal::TEXT()
                    } else {
                        pal::TEXT_DIM()
                    };
                    painter.with_clip_rect(header.shrink(2.0)).text(
                        pos2(header.left() + 24.0, header.center().y),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(11.5),
                        tc,
                    );
                }
                painter.line_segment(
                    [pos2(head_x + HEADER_W, o.y), pos2(head_x + HEADER_W, o.y + height)],
                    Stroke::new(1.0, pal::BORDER()),
                );

                // Righello (sticky in alto).
                let ruler = Rect::from_min_max(pos2(o.x, ruler_y), pos2(o.x + width, ruler_y + RULER_H));
                painter.rect_filled(ruler, 0.0, pal::BG_RAISED());
                painter.line_segment(
                    [ruler.left_bottom(), ruler.right_bottom()],
                    Stroke::new(1.0, pal::BORDER()),
                );
                let label_every = if self.zoom_x * bar as f32 > 36.0 {
                    1
                } else if self.zoom_x * bar as f32 > 14.0 {
                    4
                } else {
                    8
                };
                let mut b = 0;
                while b * bar <= end {
                    let x = x_of((b * bar) as f64);
                    if x >= clip_r.left() - 40.0 && x <= clip_r.right() {
                        painter.line_segment(
                            [
                                pos2(x, ruler.bottom() - if b % 4 == 0 { 9.0 } else { 5.0 }),
                                pos2(x, ruler.bottom()),
                            ],
                            Stroke::new(1.0, pal::TEXT_FAINT()),
                        );
                        if b % label_every == 0 {
                            painter.text(
                                pos2(x + 4.0, ruler.center().y - 1.0),
                                Align2::LEFT_CENTER,
                                b + 1,
                                theme::bold_font(10.5),
                                pal::TEXT_DIM(),
                            );
                        }
                    }
                    b += 1;
                }
                painter.rect_filled(
                    Rect::from_min_size(pos2(head_x, ruler_y), vec2(HEADER_W, RULER_H)),
                    0.0,
                    pal::BG_RAISED(),
                );
                if let Some(ph) = playhead {
                    let x = x_of(ph);
                    if x > head_x + HEADER_W {
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                pos2(x - 5.0, ruler.top() + 4.0),
                                pos2(x + 5.0, ruler.top() + 4.0),
                                pos2(x, ruler.bottom() - 2.0),
                            ],
                            pal::ACCENT(),
                            Stroke::NONE,
                        ));
                    }
                }

                // Interazione.
                let in_header = |p: Pos2| p.x < head_x + HEADER_W;
                let in_ruler = |p: Pos2| p.y < ruler_y + RULER_H;
                let hit = |p: Pos2| clips.iter().rev().find(|c| rect_of(c).contains(p)).copied();
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos().filter(|p| !in_header(*p) && !in_ruler(*p)) {
                        if let Some(c) = hit(p) {
                            let kind = if p.x > rect_of(&c).right() - RESIZE_PX {
                                DragKind::Resize
                            } else {
                                DragKind::Move
                            };
                            self.drag = Some(Drag {
                                clip: c,
                                kind,
                                origin: p,
                            });
                        }
                    }
                }
                if resp.drag_stopped() {
                    if let (Some(d), Some(p)) = (self.drag.take(), resp.interact_pointer_pos()) {
                        let m = dragged(&d, p - d.origin, self.zoom_x, self.track_h, snap);
                        if m != d.clip {
                            studio.exec(Command::MoveClip {
                                clip: m.id,
                                track: Some(m.track),
                                position: Some(m.position),
                                length: Some(m.length),
                                offset: None,
                            });
                        }
                    }
                }
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        if in_ruler(p) {
                            studio.engine.seek((tick_at(p.x) / snap * snap) as f64);
                        } else if in_header(p) {
                            let tr = track_at(p.y);
                            let muted = studio.project.playlist.track_muted(tr);
                            studio.exec(Command::SetPlaylistTrack {
                                track: tr,
                                name: None,
                                muted: Some(!muted),
                                color: None,
                            });
                        } else if hit(p).is_none() {
                            if let Some(pat) = studio.current_pattern() {
                                let pos = tick_at(p.x) / snap * snap;
                                studio.exec(Command::PlaceClip {
                                    track: track_at(p.y),
                                    position: pos,
                                    length: None,
                                    source: ClipSource::Pattern(pat),
                                });
                            }
                        }
                    }
                }
                if resp.secondary_clicked() {
                    if let Some(c) = resp.interact_pointer_pos().and_then(hit) {
                        studio.exec(Command::RemoveClip { clip: c.id });
                    }
                }
                if resp.double_clicked() {
                    if let Some(ClipSource::Pattern(pid)) = resp.interact_pointer_pos().and_then(hit).map(|c| c.source)
                    {
                        studio.exec(Command::SelectPattern { pattern: pid });
                        studio.view = crate::state::View::PianoRoll;
                    }
                }
                if let Some(p) = resp.hover_pos() {
                    if let Some(c) = hit(p) {
                        let icon = if p.x > rect_of(&c).right() - RESIZE_PX {
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

/// Usata nei test: id della clip sotto una posizione.
#[allow(dead_code)]
pub(crate) fn clip_at(clips: &[PlaylistClip], track: u16, tick: Tick) -> Option<ClipId> {
    clips
        .iter()
        .rev()
        .find(|c| c.track == track && tick >= c.position && tick < c.end())
        .map(|c| c.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_and_hit() {
        let c = PlaylistClip {
            id: ClipId(1),
            track: 2,
            position: 384,
            length: 384,
            offset: 0,
            source: ClipSource::Pattern(mfc_core::PatternId(1)),
            muted: false,
        };
        let d = Drag {
            clip: c,
            kind: DragKind::Move,
            origin: pos2(0.0, 0.0),
        };
        let m = dragged(&d, vec2(384.0 * 0.1, 28.0), 0.1, 28.0, 384);
        assert_eq!((m.position, m.track), (768, 3));
        let r = dragged(
            &Drag {
                kind: DragKind::Resize,
                ..d
            },
            vec2(-1000.0, 0.0),
            0.1,
            28.0,
            384,
        );
        assert_eq!(r.length, 384);
        assert_eq!(clip_at(&[c], 2, 500), Some(ClipId(1)));
        assert_eq!(clip_at(&[c], 1, 500), None);
    }
}
