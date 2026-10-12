//! Playlist: arrangiamento di pattern, audio clip e automazioni su tracce.
//!
//! Click su area vuota = piazza il pattern corrente; trascina = sposta; bordo destro =
//! ridimensiona; click destro = elimina; click sul righello = sposta la testina.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind};
use mfc_core::{ClipId, ClipSource, Command, PlaylistClip, Tick, TimeBase, PLAYLIST_TRACKS};

use crate::state::Studio;

const HEADER_W: f32 = 110.0;
const RULER_H: f32 = 18.0;
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
            zoom_x: 0.12,
            track_h: 28.0,
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

fn source_color(src: &ClipSource) -> Color32 {
    match src {
        ClipSource::Pattern(_) => Color32::from_rgb(92, 140, 190),
        ClipSource::Audio(_) => Color32::from_rgb(110, 175, 110),
        ClipSource::Automation(_) => Color32::from_rgb(190, 120, 170),
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
        ui.horizontal(|ui| {
            ui.strong("Playlist");
            ui.separator();
            ui.checkbox(&mut self.snap_beats, "Aggancio al beat");
            ui.label("Zoom");
            ui.add(
                egui::Slider::new(&mut self.zoom_x, 0.02..=1.0)
                    .logarithmic(true)
                    .show_value(false),
            );
            let pat_name = studio
                .current_pattern()
                .and_then(|p| studio.project.pattern(p))
                .map(|p| p.name.clone())
                .unwrap_or_default();
            ui.label(format!("Click per piazzare: {pat_name}"));
        });

        let used_tracks = studio
            .project
            .playlist
            .clips
            .iter()
            .map(|c| c.track as usize + 1)
            .max()
            .unwrap_or(0);
        let n_tracks = (used_tracks + 6).clamp(16, PLAYLIST_TRACKS);
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
                let x_of = |t: f64| o.x + HEADER_W + t as f32 * self.zoom_x;
                let y_of = |tr: u16| o.y + RULER_H + tr as f32 * self.track_h;
                let tick_at = |x: f32| ((x - o.x - HEADER_W) / self.zoom_x).max(0.0) as Tick;
                let track_at = |y: f32| {
                    (((y - o.y - RULER_H) / self.track_h).floor().max(0.0) as u16).min(PLAYLIST_TRACKS as u16 - 1)
                };
                let visual = ui.visuals().clone();
                let clip_r = ui.clip_rect();

                // Righello.
                painter.rect_filled(Rect::from_min_size(o, vec2(width, RULER_H)), 0.0, visual.faint_bg_color);
                let bar = tb.ticks_per_bar();
                let mut t = 0;
                let mut n = 1;
                while t <= end {
                    let x = x_of(t as f64);
                    if x >= clip_r.left() - 40.0 && x <= clip_r.right() {
                        painter.text(
                            pos2(x + 2.0, o.y + 2.0),
                            Align2::LEFT_TOP,
                            n,
                            FontId::monospace(10.0),
                            visual.text_color(),
                        );
                        painter.line_segment(
                            [pos2(x, o.y + RULER_H), pos2(x, o.y + height)],
                            Stroke::new(0.6, visual.weak_text_color()),
                        );
                    }
                    t += bar;
                    n += 1;
                }
                // Tracce.
                for tr in 0..n_tracks as u16 {
                    let y = y_of(tr);
                    if y + self.track_h < clip_r.top() || y > clip_r.bottom() {
                        continue;
                    }
                    let header = Rect::from_min_size(pos2(o.x, y), vec2(HEADER_W, self.track_h));
                    let track = studio.project.playlist.tracks.get(&tr);
                    let muted = track.is_some_and(|t| t.muted);
                    let name = track
                        .map(|t| t.name.clone())
                        .unwrap_or_else(|| format!("Track {}", tr + 1));
                    painter.rect_filled(header, 0.0, visual.faint_bg_color);
                    painter.text(
                        header.left_center() + vec2(18.0, 0.0),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(11.0),
                        if muted {
                            visual.weak_text_color()
                        } else {
                            visual.text_color()
                        },
                    );
                    let led = Rect::from_center_size(header.left_center() + vec2(8.0, 0.0), vec2(8.0, 8.0));
                    painter.rect_filled(
                        led,
                        4.0,
                        if muted {
                            Color32::DARK_GRAY
                        } else {
                            Color32::from_rgb(120, 220, 120)
                        },
                    );
                    painter.line_segment(
                        [pos2(o.x, y + self.track_h), pos2(o.x + width, y + self.track_h)],
                        Stroke::new(0.4, visual.weak_text_color()),
                    );
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
                        pos2(x_of(c.position as f64), y_of(c.track) + 1.0),
                        pos2(x_of(c.end() as f64), y_of(c.track) + self.track_h - 1.0),
                    )
                };
                for c in &clips {
                    let shown = match preview {
                        Some((id, moved)) if id == c.id => moved,
                        _ => *c,
                    };
                    let r = rect_of(&shown);
                    let mut fill = source_color(&c.source);
                    if c.muted {
                        fill = fill.gamma_multiply(0.35);
                    }
                    painter.rect_filled(r, 3.0, fill);
                    painter.rect_stroke(
                        r,
                        3.0,
                        Stroke::new(1.0, Color32::from_black_alpha(150)),
                        StrokeKind::Inside,
                    );
                    let label = match c.source {
                        ClipSource::Pattern(p) => studio.project.pattern(p).map(|x| x.name.clone()).unwrap_or_default(),
                        ClipSource::Audio(ch) => studio.project.channel(ch).map(|x| x.name.clone()).unwrap_or_default(),
                        ClipSource::Automation(a) => {
                            studio.project.automation(a).map(|x| x.name.clone()).unwrap_or_default()
                        }
                    };
                    painter.with_clip_rect(r).text(
                        r.left_top() + vec2(3.0, 2.0),
                        Align2::LEFT_TOP,
                        label,
                        FontId::proportional(10.0),
                        Color32::BLACK,
                    );
                    // Mini-anteprima delle note del pattern.
                    if let ClipSource::Pattern(pid) = c.source {
                        if let Some(p) = studio.project.pattern(pid) {
                            let plen = p.effective_length(&tb).max(1) as f32;
                            let notes: Vec<_> = p.notes.values().flatten().collect();
                            let (lo, hi) = notes
                                .iter()
                                .fold((127u8, 0u8), |(a, b), n| (a.min(n.key), b.max(n.key)));
                            let span = (hi.saturating_sub(lo) as f32).max(1.0);
                            let body = Rect::from_min_max(r.min + vec2(0.0, 13.0), r.max - vec2(0.0, 2.0));
                            let mut rep = 0.0;
                            while rep < shown.length as f32 {
                                for n in &notes {
                                    let x0 = body.left() + (rep + n.position as f32) * self.zoom_x;
                                    if x0 > body.right() {
                                        continue;
                                    }
                                    let x1 = (x0 + n.length as f32 * self.zoom_x).min(body.right());
                                    let y = body.bottom() - (n.key - lo) as f32 / span * body.height();
                                    painter.line_segment(
                                        [pos2(x0, y), pos2(x1.max(x0 + 1.0), y)],
                                        Stroke::new(1.5, Color32::from_black_alpha(120)),
                                    );
                                }
                                rep += plen;
                            }
                        }
                    }
                }
                if let Some(ph) = playhead {
                    let x = x_of(ph);
                    painter.line_segment(
                        [pos2(x, o.y), pos2(x, o.y + height)],
                        Stroke::new(1.5, Color32::from_rgb(255, 120, 60)),
                    );
                }

                // Interazione.
                let hit = |p: Pos2| clips.iter().rev().find(|c| rect_of(c).contains(p)).copied();
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos() {
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
                        if p.y < o.y + RULER_H {
                            studio.engine.seek((tick_at(p.x) / snap * snap) as f64);
                        } else if p.x < o.x + HEADER_W {
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
