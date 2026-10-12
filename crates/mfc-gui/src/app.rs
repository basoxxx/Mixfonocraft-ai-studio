//! Finestra principale: menu, transport, browser, viste centrali, editor plugin.

use egui::{Align, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use mfc_core::{Command, Project};

use crate::state::{Studio, View};
use crate::theme::{self, pal};
use crate::views;

/// Dialogo "percorso file" minimale (nessuna dipendenza da dialoghi nativi).
#[derive(Debug, Clone, PartialEq)]
enum FileAction {
    Open,
    SaveAs,
    RenderWav,
    ExportFlp,
    ExportMidi,
}

struct FileDialog {
    action: FileAction,
    path: String,
}

/// L'applicazione eframe.
pub struct StudioApp {
    pub studio: Studio,
    piano_roll: views::piano_roll::PianoRoll,
    playlist: views::playlist::PlaylistView,
    dialog: Option<FileDialog>,
}

impl StudioApp {
    pub fn new(studio: Studio) -> Self {
        Self {
            studio,
            piano_roll: Default::default(),
            playlist: Default::default(),
            dialog: None,
        }
    }

    /// Disegna l'intera interfaccia in `ui` (usato anche dai test headless).
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if !theme::ensure(ui.ctx()) {
            return;
        }
        self.shortcuts(ui.ctx());
        egui::Panel::top("transport")
            .exact_size(52.0)
            .frame(
                egui::Frame::new()
                    .fill(pal::BG_RAISED)
                    .inner_margin(egui::Margin::symmetric(10, 0))
                    .stroke(egui::Stroke::new(1.0, pal::BORDER)),
            )
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .exact_size(24.0)
            .frame(
                egui::Frame::new()
                    .fill(pal::BG_RAISED)
                    .inner_margin(egui::Margin::symmetric(10, 0))
                    .stroke(egui::Stroke::new(1.0, pal::BORDER)),
            )
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("browser")
            .resizable(true)
            .default_size(220.0)
            .size_range(170.0..=360.0)
            .frame(egui::Frame::new().fill(pal::BG_APP).inner_margin(egui::Margin {
                left: 6,
                right: 0,
                top: 6,
                bottom: 6,
            }))
            .show(ui, |ui| {
                views::widgets::view_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    views::browser::show(ui, &mut self.studio);
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal::BG_APP).inner_margin(6))
            .show(ui, |ui| {
                views::widgets::view_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    match self.studio.view {
                        View::ChannelRack => views::channel_rack::show(ui, &mut self.studio),
                        View::PianoRoll => self.piano_roll.show(ui, &mut self.studio),
                        View::Playlist => self.playlist.show(ui, &mut self.studio),
                        View::Mixer => views::mixer::show(ui, &mut self.studio),
                    }
                });
            });
        views::plugin_editor::show(ui.ctx(), &mut self.studio);
        self.file_dialog(ui.ctx());
        if self.studio.engine.shared.is_playing() {
            ui.ctx().request_repaint();
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        ctx.input_mut(|i| {
            if i.consume_shortcut(&undo) {
                self.studio.undo();
            }
            if i.consume_shortcut(&redo) {
                self.studio.redo();
            }
            if i.consume_shortcut(&save) {
                match self.studio.path.clone() {
                    Some(p) => {
                        if let Err(e) = self.studio.save_as(&p) {
                            self.studio.status = e;
                        }
                    }
                    None => {
                        self.dialog = Some(FileDialog {
                            action: FileAction::SaveAs,
                            path: "project.mfc".into(),
                        })
                    }
                }
            }
            if typing {
                return;
            }
            if i.key_pressed(Key::Space) {
                self.studio.toggle_play();
            }
            // Scorciatoie di FL Studio.
            if i.key_pressed(Key::F5) {
                self.studio.view = View::Playlist;
            }
            if i.key_pressed(Key::F6) {
                self.studio.view = View::ChannelRack;
            }
            if i.key_pressed(Key::F7) {
                self.studio.view = View::PianoRoll;
            }
            if i.key_pressed(Key::F9) {
                self.studio.view = View::Mixer;
            }
            if i.key_pressed(Key::L) {
                let song = !self.studio.song_mode;
                self.studio.set_song_mode(song);
            }
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        use views::widgets::{icon_button, lcd, segmented, Icon};
        let h = ui.available_height();
        let full = ui.max_rect();
        // Gruppo destro ancorato al bordo: viste, undo/redo, meter master.
        let right_w = 400.0;
        let right_rect = egui::Rect::from_min_max(egui::pos2(full.right() - right_w, full.top()), full.right_bottom());
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right_rect)
                .layout(Layout::right_to_left(Align::Center)),
        );
        self.top_bar_right(&mut right, h);
        let left_rect =
            egui::Rect::from_min_max(full.left_top(), egui::pos2(full.right() - right_w - 8.0, full.bottom()));
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(left_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        left.set_clip_rect(left_rect);
        let ui = &mut left;
        {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Logo.
            let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
            paint_logo(ui.painter(), r);
            ui.label(
                RichText::new("MIXFONOCRAFT")
                    .font(theme::bold_font(12.5))
                    .color(pal::TEXT),
            );
            ui.add_space(8.0);

            ui.menu_button(RichText::new("File").color(pal::TEXT_DIM), |ui| {
                ui.set_min_width(220.0);
                if ui.button("Nuovo progetto").clicked() {
                    self.studio.replace_project(Project::new("Untitled"), None);
                    ui.close();
                }
                ui.separator();
                for (label, action, default) in [
                    ("Apri…  (.mfc · .flp · .mid)", FileAction::Open, ""),
                    ("Salva con nome…", FileAction::SaveAs, "project.mfc"),
                ] {
                    if ui.button(label).clicked() {
                        self.dialog = Some(FileDialog {
                            action,
                            path: default.into(),
                        });
                        ui.close();
                    }
                }
                ui.separator();
                for (label, action, default) in [
                    ("Esporta audio WAV…", FileAction::RenderWav, "mix.wav"),
                    (
                        "Esporta progetto FL Studio (.flp)…",
                        FileAction::ExportFlp,
                        "project.flp",
                    ),
                    ("Esporta MIDI…", FileAction::ExportMidi, "song.mid"),
                ] {
                    if ui.button(label).clicked() {
                        self.dialog = Some(FileDialog {
                            action,
                            path: default.into(),
                        });
                        ui.close();
                    }
                }
            });
            ui.menu_button(RichText::new("Modifica").color(pal::TEXT_DIM), |ui| {
                ui.set_min_width(200.0);
                if ui
                    .add_enabled(
                        self.studio.history.can_undo(),
                        egui::Button::new("Annulla").shortcut_text("Ctrl+Z"),
                    )
                    .clicked()
                {
                    self.studio.undo();
                }
                if ui
                    .add_enabled(
                        self.studio.history.can_redo(),
                        egui::Button::new("Ripeti").shortcut_text("Ctrl+Y"),
                    )
                    .clicked()
                {
                    self.studio.redo();
                }
            });
            toolbar_sep(ui, h);

            // Transport.
            if let Some(i) = segmented(ui, &["PAT", "SONG"], self.studio.song_mode as usize, 28.0) {
                self.studio.set_song_mode(i == 1);
            }
            let playing = self.studio.engine.shared.is_playing();
            let play_icon = if playing { Icon::Pause } else { Icon::Play };
            if icon_button(ui, play_icon, playing, pal::GREEN, 32.0, "Play / Pausa (Spazio)").clicked() {
                self.studio.toggle_play();
            }
            if icon_button(ui, Icon::Stop, false, pal::TEXT, 32.0, "Stop").clicked() {
                self.studio.stop();
            }
            ui.add_enabled_ui(false, |ui| {
                icon_button(
                    ui,
                    Icon::Record,
                    false,
                    pal::RED,
                    32.0,
                    "Registrazione (non ancora disponibile)",
                );
            });
            toolbar_sep(ui, h);

            // Tempo: trascina in verticale, rilascio = commit.
            let id = egui::Id::new("tempo_drag");
            let committed = self.studio.project.tempo;
            let mut bpm = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(committed);
            let r = lcd(
                ui,
                &format!("{bpm:>7.3}"),
                Some("BPM"),
                96.0,
                32.0,
                egui::Sense::click_and_drag(),
            )
            .on_hover_text("Trascina per cambiare il tempo (Shift = fine), doppio click = 140");
            if r.dragged() {
                let fine = if ui.input(|i| i.modifiers.shift) { 0.01 } else { 0.25 };
                bpm = (bpm - r.drag_delta().y as f64 * fine).clamp(10.0, 999.0);
                ui.data_mut(|d| d.insert_temp(id, bpm));
            }
            if r.drag_stopped() {
                ui.data_mut(|d| d.remove::<f64>(id));
                self.studio.exec(Command::SetTempo {
                    bpm: (bpm * 1000.0).round() / 1000.0,
                });
            }
            if r.double_clicked() {
                self.studio.exec(Command::SetTempo { bpm: 140.0 });
            }

            let tb = self.studio.project.time;
            let pos = self.studio.engine.shared.position().max(0.0) as u64;
            let bar = pos / tb.ticks_per_bar() + 1;
            let beat = pos % tb.ticks_per_bar() / tb.ticks_per_beat() + 1;
            let tick = pos % tb.ticks_per_beat();
            lcd(
                ui,
                &format!("{bar:>3}:{beat:02}:{tick:02}"),
                Some(if self.studio.song_mode { "SONG" } else { "PAT" }),
                104.0,
                32.0,
                egui::Sense::hover(),
            );
            let sig = tb.signature;
            lcd(
                ui,
                &format!("{}/{}", sig.numerator, sig.denominator),
                Some("TEMPO"),
                52.0,
                32.0,
                egui::Sense::hover(),
            );
            toolbar_sep(ui, h);

            // Selettore pattern.
            let current = self.studio.current_pattern();
            let pats: Vec<(usize, mfc_core::PatternId, String, mfc_core::Color)> = self
                .studio
                .project
                .patterns
                .iter()
                .enumerate()
                .map(|(i, p)| (i, p.id, p.name.clone(), p.color))
                .collect();
            let (cur_idx, cur_name, cur_col) = pats
                .iter()
                .find(|p| Some(p.1) == current)
                .map(|p| (p.0, p.2.clone(), p.3))
                .unwrap_or((0, "—".into(), mfc_core::Color::default()));
            let swatch = theme::item_color(cur_col, cur_idx);
            ui.add_space(2.0);
            let (sr, _) = ui.allocate_exact_size(egui::vec2(6.0, 24.0), egui::Sense::hover());
            ui.painter().rect_filled(sr, 2.0, swatch);
            egui::ComboBox::from_id_salt("pattern_sel")
                .width(150.0)
                .selected_text(RichText::new(cur_name).font(theme::bold_font(12.0)))
                .show_ui(ui, |ui| {
                    for (i, id, n, c) in &pats {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(4.0, 16.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 1.0, theme::item_color(*c, *i));
                            if ui.selectable_label(Some(*id) == current, n).clicked() {
                                self.studio.exec(Command::SelectPattern { pattern: *id });
                            }
                        });
                    }
                });
            if icon_button(ui, Icon::Plus, false, pal::ACCENT, 24.0, "Nuovo pattern").clicked() {
                let n = self.studio.project.patterns.len() + 1;
                if let Some(mfc_core::Outcome::Pattern(id)) = self.studio.exec(Command::AddPattern {
                    name: format!("Pattern {n}"),
                }) {
                    self.studio.exec(Command::SelectPattern { pattern: id });
                }
            }
        }
    }

    fn top_bar_right(&mut self, ui: &mut egui::Ui, h: f32) {
        use views::widgets::{icon_button, Icon};
        ui.spacing_mut().item_spacing.x = 6.0;
        let (l, r) = self.studio.engine.shared.take_output_meter();
        let (mr, _) = ui.allocate_exact_size(egui::vec2(112.0, h), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(mr.left(), mr.center().y - 9.0),
            egui::Align2::LEFT_CENTER,
            "MASTER",
            theme::bold_font(8.5),
            pal::TEXT_FAINT,
        );
        let meter_rect = egui::Rect::from_min_size(egui::pos2(mr.left(), mr.center().y - 2.0), egui::vec2(112.0, 12.0));
        views::widgets::paint_meter(ui, meter_rect, l, r);
        toolbar_sep(ui, h);
        for (v, icon, tip) in [
            (View::Mixer, Icon::Mixer, "Mixer (F9)"),
            (View::PianoRoll, Icon::Piano, "Piano Roll (F7)"),
            (View::ChannelRack, Icon::Rack, "Channel Rack (F6)"),
            (View::Playlist, Icon::Playlist, "Playlist (F5)"),
        ] {
            if icon_button(ui, icon, self.studio.view == v, pal::ACCENT, 32.0, tip).clicked() {
                self.studio.view = v;
            }
        }
        toolbar_sep(ui, h);
        if icon_button(ui, Icon::Redo, false, pal::TEXT, 26.0, "Ripeti (Ctrl+Y)").clicked() {
            self.studio.redo();
        }
        if icon_button(ui, Icon::Undo, false, pal::TEXT, 26.0, "Annulla (Ctrl+Z)").clicked() {
            self.studio.undo();
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            let c = if self.studio.audible { pal::GREEN } else { pal::YELLOW };
            ui.painter().circle_filled(dot.center(), 3.5, c);
            ui.label(RichText::new(&self.studio.status).size(11.0).color(pal::TEXT_DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let p = &self.studio.project;
                let dirty = if self.studio.dirty { " •" } else { "" };
                ui.label(
                    RichText::new(format!("{}{dirty}", p.info.title))
                        .size(11.0)
                        .color(pal::TEXT),
                );
                ui.label(RichText::new("·").color(pal::TEXT_FAINT));
                ui.label(
                    RichText::new(format!(
                        "{} canali · {} pattern · {:.1} battute",
                        p.channels.len(),
                        p.patterns.len(),
                        p.song_length() as f64 / p.time.ticks_per_bar() as f64
                    ))
                    .size(11.0)
                    .color(pal::TEXT_FAINT),
                );
            });
        });
    }

    fn file_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.dialog.as_mut() else { return };
        let title = match d.action {
            FileAction::Open => "Apri progetto",
            FileAction::SaveAs => "Salva progetto",
            FileAction::RenderWav => "Esporta WAV",
            FileAction::ExportFlp => "Esporta FLP",
            FileAction::ExportMidi => "Esporta MIDI",
        };
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Percorso:");
                let r = ui.text_edit_singleline(&mut d.path);
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    confirmed = true;
                }
                ui.horizontal(|ui| {
                    confirmed |= ui.button("OK").clicked();
                    cancelled |= ui.button("Annulla").clicked();
                });
            });
        if cancelled {
            self.dialog = None;
        } else if confirmed {
            let d = self.dialog.take().unwrap();
            let path = std::path::PathBuf::from(d.path.trim());
            let res = match d.action {
                FileAction::Open => self.studio.open(&path),
                FileAction::SaveAs => self.studio.save_as(&path),
                FileAction::RenderWav => self.studio.render(&path),
                FileAction::ExportFlp => mfc_flp::write_file(&path, &self.studio.project)
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                FileAction::ExportMidi => mfc_midi::export_song(&self.studio.project)
                    .map_err(|e| e.to_string())
                    .and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())),
            };
            self.studio.status = match res {
                Ok(()) if self.studio.status.is_empty() => format!("OK: {}", path.display()),
                Ok(()) => std::mem::take(&mut self.studio.status),
                Err(e) => format!("Errore: {e}"),
            };
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

/// Separatore verticale della toolbar.
fn toolbar_sep(ui: &mut egui::Ui, h: f32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, h * 0.55), egui::Sense::hover());
    ui.painter()
        .line_segment([r.center_top(), r.center_bottom()], egui::Stroke::new(1.0, pal::LINE));
}

/// Marchio: quadrato arrotondato con barre arancioni (come l'icona dell'app).
fn paint_logo(p: &egui::Painter, r: egui::Rect) {
    p.rect_filled(r, 6.0, egui::Color32::from_rgb(0x2b, 0x2f, 0x36));
    let bars = [0.4f32, 0.7, 0.95, 0.55, 0.8];
    let w = r.width() / 9.0;
    for (i, h) in bars.iter().enumerate() {
        let x = r.left() + w * (1.0 + i as f32 * 1.5);
        let bh = (r.height() - 8.0) * h;
        let br = egui::Rect::from_min_size(egui::pos2(x, r.bottom() - 4.0 - bh), egui::vec2(w, bh));
        p.rect_filled(br, 1.5, theme::mix(pal::YELLOW, pal::ACCENT, 1.0 - h));
    }
}
