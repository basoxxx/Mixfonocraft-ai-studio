//! Finestra principale: menu, transport, browser, viste centrali, editor plugin.

use egui::{Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use mfc_core::{Command, Project};

use crate::state::{Studio, View};
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
        self.shortcuts(ui.ctx());
        egui::Panel::top("transport").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.studio.status).small());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let title = &self.studio.project.info.title;
                    let dirty = if self.studio.dirty { " •" } else { "" };
                    ui.label(RichText::new(format!("{title}{dirty}")).small());
                });
            });
        });
        egui::Panel::left("browser")
            .resizable(true)
            .default_size(190.0)
            .show(ui, |ui| {
                views::browser::show(ui, &mut self.studio);
            });
        egui::CentralPanel::default().show(ui, |ui| match self.studio.view {
            View::ChannelRack => views::channel_rack::show(ui, &mut self.studio),
            View::PianoRoll => self.piano_roll.show(ui, &mut self.studio),
            View::Playlist => self.playlist.show(ui, &mut self.studio),
            View::Mixer => views::mixer::show(ui, &mut self.studio),
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
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Nuovo").clicked() {
                    self.studio.replace_project(Project::new("Untitled"), None);
                    ui.close();
                }
                for (label, action, default) in [
                    ("Apri… (.mfc/.flp/.mid)", FileAction::Open, ""),
                    ("Salva con nome…", FileAction::SaveAs, "project.mfc"),
                    ("Esporta WAV…", FileAction::RenderWav, "mix.wav"),
                    ("Esporta FLP…", FileAction::ExportFlp, "project.flp"),
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
            ui.menu_button("Modifica", |ui| {
                if ui
                    .add_enabled(self.studio.history.can_undo(), egui::Button::new("Annulla (Ctrl+Z)"))
                    .clicked()
                {
                    self.studio.undo();
                }
                if ui
                    .add_enabled(self.studio.history.can_redo(), egui::Button::new("Ripeti (Ctrl+Y)"))
                    .clicked()
                {
                    self.studio.redo();
                }
            });
            ui.separator();

            // PAT / SONG
            let song = self.studio.song_mode;
            if ui.selectable_label(!song, "PAT").clicked() {
                self.studio.set_song_mode(false);
            }
            if ui.selectable_label(song, "SONG").clicked() {
                self.studio.set_song_mode(true);
            }
            let playing = self.studio.engine.shared.is_playing();
            if ui
                .button(if playing { "⏸" } else { "▶" })
                .on_hover_text("Play/Pausa (Spazio)")
                .clicked()
            {
                self.studio.toggle_play();
            }
            if ui.button("⏹").on_hover_text("Stop").clicked() {
                self.studio.stop();
            }

            // Tempo
            let mut bpm = self.studio.project.tempo;
            let r = ui.add(
                egui::DragValue::new(&mut bpm)
                    .range(10.0..=999.0)
                    .speed(0.1)
                    .fixed_decimals(3)
                    .suffix(" BPM"),
            );
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                self.studio.exec(Command::SetTempo { bpm });
            }

            // Posizione
            let tb = self.studio.project.time;
            let pos = self.studio.engine.shared.position().max(0.0) as u64;
            let bar = pos / tb.ticks_per_bar() + 1;
            let beat = pos % tb.ticks_per_bar() / tb.ticks_per_beat() + 1;
            let tick = pos % tb.ticks_per_beat();
            ui.label(
                RichText::new(format!("{bar:>3}:{beat:02}:{tick:02}"))
                    .monospace()
                    .strong(),
            );

            // Selettore pattern
            let current = self.studio.current_pattern();
            let name = current
                .and_then(|p| self.studio.project.pattern(p))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "—".into());
            egui::ComboBox::from_id_salt("pattern_sel")
                .selected_text(name)
                .show_ui(ui, |ui| {
                    let pats: Vec<_> = self
                        .studio
                        .project
                        .patterns
                        .iter()
                        .map(|p| (p.id, p.name.clone()))
                        .collect();
                    for (id, n) in pats {
                        if ui.selectable_label(Some(id) == current, n).clicked() {
                            self.studio.exec(Command::SelectPattern { pattern: id });
                        }
                    }
                });
            if ui.button("➕").on_hover_text("Nuovo pattern").clicked() {
                let n = self.studio.project.patterns.len() + 1;
                if let Some(mfc_core::Outcome::Pattern(id)) = self.studio.exec(Command::AddPattern {
                    name: format!("Pattern {n}"),
                }) {
                    self.studio.exec(Command::SelectPattern { pattern: id });
                }
            }
            ui.separator();

            for (v, label, key) in [
                (View::Playlist, "Playlist", "F5"),
                (View::ChannelRack, "Channel Rack", "F6"),
                (View::PianoRoll, "Piano Roll", "F7"),
                (View::Mixer, "Mixer", "F9"),
            ] {
                if ui
                    .selectable_label(self.studio.view == v, label)
                    .on_hover_text(key)
                    .clicked()
                {
                    self.studio.view = v;
                }
            }
            ui.separator();
            let (l, r) = self.studio.engine.shared.take_output_meter();
            views::widgets::meter(ui, l, r, egui::vec2(60.0, 14.0));
            if !self.studio.audible {
                ui.label(RichText::new("🔇").color(Color32::GRAY))
                    .on_hover_text("Nessun dispositivo audio");
            }
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
