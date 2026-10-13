//! Finestra principale: menu, transport, browser, viste centrali, editor plugin.

use std::path::{Path, PathBuf};

use egui::{Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText, Sense, Stroke};
use mfc_core::{ChannelKind, Command, Outcome, Project};

use crate::state::{Studio, View};
use crate::theme::{self, pal};
use crate::views;

/// Azioni che richiedono un file (menu, scorciatoie, drag & drop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileAction {
    Open,
    SaveAs,
    RenderWav,
    ExportFlp,
    ExportMidi,
}

impl FileAction {
    fn title(self) -> &'static str {
        match self {
            FileAction::Open => "Apri progetto",
            FileAction::SaveAs => "Salva progetto",
            FileAction::RenderWav => "Esporta WAV",
            FileAction::ExportFlp => "Esporta FLP",
            FileAction::ExportMidi => "Esporta MIDI",
        }
    }

    /// Estensione aggiunta se il percorso scelto non ne ha una.
    fn ext(self) -> &'static str {
        match self {
            FileAction::Open => "",
            FileAction::SaveAs => "mfc",
            FileAction::RenderWav => "wav",
            FileAction::ExportFlp => "flp",
            FileAction::ExportMidi => "mid",
        }
    }

    /// Filtri del dialogo nativo (il primo è quello predefinito).
    fn filters(self) -> &'static [(&'static str, &'static [&'static str])] {
        match self {
            FileAction::Open => &[
                ("Progetti e file supportati", &["mfc", "flp", "mid", "midi"]),
                ("Progetti Mixfonocraft", &["mfc"]),
                ("FL Studio", &["flp"]),
                ("MIDI", &["mid", "midi"]),
            ],
            FileAction::SaveAs => &[("Progetti Mixfonocraft", &["mfc"]), ("FL Studio", &["flp"])],
            FileAction::RenderWav => &[("WAV", &["wav"])],
            FileAction::ExportFlp => &[("FL Studio", &["flp"])],
            FileAction::ExportMidi => &[("MIDI", &["mid", "midi"])],
        }
    }
}

/// Dialogo "percorso file" di ripiego, usato solo se i dialoghi nativi non sono disponibili.
struct FileDialog {
    action: FileAction,
    path: String,
}

/// Cosa fare di un file trascinato sulla finestra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropAction {
    /// Progetto o MIDI: apre (sostituisce il progetto corrente).
    OpenProject,
    /// Campione audio: nuovo canale Sampler.
    LoadSample,
    /// Plugin VST3/CLAP: scansione e aggiunta (strumento → canale, effetto → insert).
    AddPlugin,
    Unsupported,
}

/// Instrada un file trascinato in base all'estensione (senza distinguere le maiuscole).
fn drop_action(path: &Path) -> DropAction {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "mfc" | "flp" | "mid" | "midi" => DropAction::OpenProject,
        "wav" => DropAction::LoadSample,
        "vst3" | "clap" => DropAction::AddPlugin,
        _ => DropAction::Unsupported,
    }
}

/// Nome di file suggerito dal titolo del progetto (caratteri non validi sostituiti).
fn default_file_name(title: &str, ext: &str) -> String {
    let clean: String = title
        .trim()
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = if clean.is_empty() {
        "progetto".to_string()
    } else {
        clean
    };
    format!("{stem}.{ext}")
}

/// Aggiunge l'estensione predefinita se il percorso scelto non ne ha una.
fn with_default_extension(path: PathBuf, ext: &str) -> PathBuf {
    if path.extension().is_some() || ext.is_empty() {
        path
    } else {
        path.with_extension(ext)
    }
}

/// I dialoghi nativi richiedono un display (su Linux: X11 o Wayland).
fn native_dialogs_available() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        true
    }
}

/// L'applicazione eframe.
pub struct StudioApp {
    pub studio: Studio,
    piano_roll: views::piano_roll::PianoRoll,
    playlist: views::playlist::PlaylistView,
    dialog: Option<FileDialog>,
    audio_settings: views::settings::AudioSettings,
    /// Autosalvataggio e recupero (vedi `autosave.rs`).
    pub autosave: crate::autosave::Autosave,
}

impl StudioApp {
    pub fn new(studio: Studio) -> Self {
        Self {
            studio,
            piano_roll: Default::default(),
            playlist: Default::default(),
            dialog: None,
            audio_settings: Default::default(),
            autosave: Default::default(),
        }
    }

    /// Disegna l'intera interfaccia in `ui` (usato anche dai test headless).
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if !theme::ensure(ui.ctx()) {
            return;
        }
        self.shortcuts(ui.ctx());
        self.handle_dropped_files(ui.ctx());
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
        views::settings::show(ui.ctx(), &mut self.studio, &mut self.audio_settings);
        self.file_dialog(ui.ctx());
        // HOOK autosave: salvataggio periodico e finestra di recupero.
        self.autosave.tick(&mut self.studio, ui.ctx());
        drop_overlay(ui.ctx());
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
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
        let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
        let record = KeyboardShortcut::new(Modifiers::COMMAND, Key::R);
        // Ctrl+Shift+S va controllato prima di Ctrl+S: egui accetta Ctrl+S anche con Shift premuto.
        let (mut do_open, mut do_save, mut do_save_as) = (false, false, false);
        ctx.input_mut(|i| {
            if i.consume_shortcut(&undo) {
                self.studio.undo();
            }
            if i.consume_shortcut(&redo) {
                self.studio.redo();
            }
            do_save_as = i.consume_shortcut(&save_as);
            do_save = !do_save_as && i.consume_shortcut(&save);
            do_open = i.consume_shortcut(&open);
            if i.consume_shortcut(&record) {
                self.studio.toggle_record();
            }
        });
        // Tastiera del computer come pianoforte (solo se nessun campo di testo ha il focus).
        if !typing && self.studio.live.handle_keyboard(ctx) {
            ctx.request_repaint();
        }
        self.studio.pump_live();
        self.studio.tick_plugins();
        // Le azioni su file sono fuori dal lock dell'input: i dialoghi nativi sono bloccanti.
        if do_open {
            self.request_file(FileAction::Open);
        }
        if do_save_as {
            self.request_file(FileAction::SaveAs);
        }
        if do_save {
            self.save_project();
        }
        ctx.input_mut(|i| {
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
                ui.set_min_width(240.0);
                if ui.button("Nuovo progetto").clicked() {
                    self.studio.replace_project(Project::new("Untitled"), None);
                    ui.close();
                }
                ui.separator();
                if ui.add(egui::Button::new("Apri…").shortcut_text("Ctrl+O")).clicked() {
                    self.request_file(FileAction::Open);
                    ui.close();
                }
                if ui.add(egui::Button::new("Salva").shortcut_text("Ctrl+S")).clicked() {
                    self.save_project();
                    ui.close();
                }
                if ui
                    .add(egui::Button::new("Salva con nome…").shortcut_text("Ctrl+Shift+S"))
                    .clicked()
                {
                    self.request_file(FileAction::SaveAs);
                    ui.close();
                }
                ui.separator();
                for (label, action) in [
                    ("Esporta audio WAV…", FileAction::RenderWav),
                    ("Esporta progetto FL Studio (.flp)…", FileAction::ExportFlp),
                    ("Esporta MIDI…", FileAction::ExportMidi),
                ] {
                    if ui.button(label).clicked() {
                        self.request_file(action);
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
            ui.menu_button(RichText::new("Audio").color(pal::TEXT_DIM), |ui| {
                ui.set_min_width(200.0);
                if ui.button("Impostazioni audio…").clicked() {
                    self.audio_settings.request_open(&self.studio);
                    ui.close();
                }
                ui.separator();
                ui.checkbox(
                    &mut self.studio.live.typing_keyboard,
                    "Tastiera del computer come piano",
                );
                ui.label(
                    RichText::new(format!("Ottava {} (Shift+↑/↓) · Z–M e Q–P", self.studio.live.octave))
                        .small()
                        .color(pal::TEXT_DIM),
                );
                ui.separator();
                ui.menu_button("Ingresso MIDI", |ui| {
                    let current = self.studio.live.midi_port.clone();
                    if ui.radio(current.is_none(), "Nessuno").clicked() {
                        let _ = self.studio.live.connect_midi(None);
                        ui.close();
                    }
                    let ports = crate::input::LiveInput::midi_ports();
                    if ports.is_empty() {
                        ui.label(RichText::new("Nessun dispositivo MIDI").color(pal::TEXT_DIM));
                    }
                    for port in ports {
                        if ui.radio(current.as_deref() == Some(port.as_str()), &port).clicked() {
                            self.studio.status = match self.studio.live.connect_midi(Some(&port)) {
                                Ok(()) => format!("MIDI collegato: {port}"),
                                Err(e) => format!("MIDI: {e}"),
                            };
                            ui.close();
                        }
                    }
                });
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
            let rec = self.studio.recording;
            if icon_button(
                ui,
                Icon::Record,
                rec,
                pal::RED,
                32.0,
                "Registra nel pattern le note da tastiera/MIDI (Ctrl+R)",
            )
            .clicked()
            {
                self.studio.toggle_record();
            }
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
            let (dot, dot_resp) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::click());
            let c = if self.studio.audible { pal::GREEN } else { pal::YELLOW };
            ui.painter().circle_filled(dot.center(), 3.5, c);
            let tip = if self.studio.audible {
                "Uscita audio attiva · clic per le impostazioni"
            } else {
                "Uscita audio non disponibile · clic per le impostazioni"
            };
            if dot_resp.on_hover_text(tip).clicked() {
                self.audio_settings.request_open(&self.studio);
            }
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
                ui.label(RichText::new("·").color(pal::TEXT_FAINT));
                let cpu = self.studio.engine.shared.cpu_load();
                let cpu_color = if cpu > 0.8 { pal::RED } else { pal::TEXT_FAINT };
                ui.label(
                    RichText::new(format!("CPU {:.0}%", cpu * 100.0))
                        .size(11.0)
                        .color(cpu_color),
                );
            });
        });
    }

    /// Nome di file suggerito per l'azione (vuoto per l'apertura).
    fn suggested_name(&self, action: FileAction) -> String {
        let title = &self.studio.project.info.title;
        match action {
            FileAction::Open => String::new(),
            FileAction::SaveAs => self
                .studio
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| default_file_name(title, "mfc")),
            FileAction::RenderWav => default_file_name(title, "wav"),
            FileAction::ExportFlp => default_file_name(title, "flp"),
            FileAction::ExportMidi => default_file_name(title, "mid"),
        }
    }

    /// Avvia un'azione su file: dialogo nativo se disponibile, altrimenti campo di testo.
    fn request_file(&mut self, action: FileAction) {
        if !native_dialogs_available() {
            self.dialog = Some(FileDialog {
                action,
                path: self.suggested_name(action),
            });
            return;
        }
        if let Some(path) = self.pick_native(action) {
            self.run_file_action(action, path);
        }
    }

    /// Dialogo nativo (apertura o salvataggio). `None` = annullato.
    fn pick_native(&self, action: FileAction) -> Option<PathBuf> {
        let mut dlg = rfd::FileDialog::new().set_title(action.title());
        for (name, exts) in action.filters() {
            dlg = dlg.add_filter(*name, exts);
        }
        if let Some(dir) = self.studio.path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        if action == FileAction::Open {
            dlg.pick_file()
        } else {
            dlg.set_file_name(self.suggested_name(action)).save_file()
        }
    }

    /// Salva sul percorso corrente; senza percorso chiede dove salvare.
    fn save_project(&mut self) {
        match self.studio.path.clone() {
            Some(p) => {
                if let Err(e) = self.studio.save_as(&p) {
                    self.studio.status = format!("Errore: {e}");
                }
            }
            None => self.request_file(FileAction::SaveAs),
        }
    }

    /// Esegue l'azione sul percorso scelto e aggiorna la barra di stato.
    fn run_file_action(&mut self, action: FileAction, path: PathBuf) {
        let path = if action == FileAction::Open {
            path
        } else {
            with_default_extension(path, action.ext())
        };
        let res = match action {
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
        match res {
            Ok(()) if matches!(action, FileAction::ExportFlp | FileAction::ExportMidi) => {
                self.studio.status = format!("Esportato {}", path.display());
            }
            Ok(()) => {}
            Err(e) => self.studio.status = format!("Errore: {e}"),
        }
    }

    /// Gestisce i file rilasciati sulla finestra in questo frame.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            let path = file.path().to_path_buf();
            match drop_action(&path) {
                DropAction::OpenProject => self.run_file_action(FileAction::Open, path),
                DropAction::LoadSample => self.add_sample_channel(&path),
                DropAction::AddPlugin => match self.studio.host.scan_file(&path) {
                    Ok(found) if !found.is_empty() => {
                        crate::plugins::save_catalog(&self.studio.host);
                        views::browser::add_external(&mut self.studio, &found[0]);
                        self.studio.status = format!("Plugin aggiunto: {}", found[0].name);
                    }
                    Ok(_) => self.studio.status = "Nessun plugin nel file".into(),
                    Err(e) => self.studio.status = format!("Plugin non caricabile: {e}"),
                },
                DropAction::Unsupported => {
                    let name = path
                        .file_name()
                        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    self.studio.status = format!("Formato non supportato: {name}");
                }
            }
        }
    }

    /// Aggiunge un canale Sampler con il campione indicato.
    fn add_sample_channel(&mut self, path: &Path) {
        let name = path
            .file_stem()
            .map_or_else(|| "Campione".to_string(), |n| n.to_string_lossy().into_owned());
        let Some(plugin) = mfc_plugins::default_instance(mfc_plugins::ids::SAMPLER) else {
            self.studio.status = "Plugin sampler non disponibile".into();
            return;
        };
        let Some(Outcome::Channel(channel)) = self.studio.exec(Command::AddChannel {
            name: name.clone(),
            kind: ChannelKind::Sampler,
            plugin,
            mixer_insert: None,
        }) else {
            return;
        };
        let sample = Command::SetChannelSample {
            channel,
            path: Some(path.display().to_string()),
        };
        if self.studio.exec(sample).is_some() {
            self.studio.status = format!("Campione aggiunto: {name}");
        }
    }

    fn file_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.dialog.as_mut() else { return };
        let title = d.action.title();
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Percorso (dialogo nativo non disponibile):");
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
            let path = PathBuf::from(d.path.trim());
            self.run_file_action(d.action, path);
        }
    }
}

/// Velo semitrasparente con bordo tratteggiato mentre un file è trascinato sulla finestra.
fn drop_overlay(ctx: &egui::Context) {
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    if !hovering {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop_overlay"),
    ));
    let screen = ctx.content_rect();
    painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0x10, 0x12, 0x15, 205));
    let r = screen.shrink(14.0);
    let outline = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    let mut border = Vec::new();
    egui::Shape::dashed_line_many(&outline, Stroke::new(2.0, pal::ACCENT), 12.0, 7.0, &mut border);
    painter.extend(border);
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        "Rilascia per aprire",
        theme::bold_font(20.0),
        pal::ACCENT,
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_routing_by_extension() {
        assert_eq!(drop_action(Path::new("a/progetto.MFC")), DropAction::OpenProject);
        assert_eq!(drop_action(Path::new("x.flp")), DropAction::OpenProject);
        assert_eq!(drop_action(Path::new("x.mid")), DropAction::OpenProject);
        assert_eq!(drop_action(Path::new("x.midi")), DropAction::OpenProject);
        assert_eq!(drop_action(Path::new("kick.WAV")), DropAction::LoadSample);
        assert_eq!(drop_action(Path::new("Synth.vst3")), DropAction::AddPlugin);
        assert_eq!(drop_action(Path::new("/p/Delay.CLAP")), DropAction::AddPlugin);
        assert_eq!(drop_action(Path::new("nota.txt")), DropAction::Unsupported);
        assert_eq!(drop_action(Path::new("senza_estensione")), DropAction::Unsupported);
    }

    #[test]
    fn default_names_are_sanitized() {
        assert_eq!(default_file_name("a/b:c", "wav"), "a_b_c.wav");
        assert_eq!(default_file_name("  ", "mid"), "progetto.mid");
        assert_eq!(
            with_default_extension(PathBuf::from("mix"), "wav"),
            PathBuf::from("mix.wav")
        );
        assert_eq!(
            with_default_extension(PathBuf::from("mix.flp"), "wav"),
            PathBuf::from("mix.flp")
        );
    }
}
