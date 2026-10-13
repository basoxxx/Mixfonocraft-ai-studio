//! Browser stile FL Studio: pattern, canali, generatori, effetti e automazioni.
//!
//! Click su un generatore aggiunge un canale; click su un effetto lo aggiunge all'insert
//! selezionato; click su un'automazione la piazza in fondo alla Playlist.

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, Response, RichText, Sense, Stroke, Ui};
use mfc_core::{
    AutomationId, ChannelId, ChannelKind, ClipSource, Command, Outcome, PatternId, Project, PLAYLIST_TRACKS,
};
use mfc_plugins::{default_instance, registry, PluginRole};

use crate::state::Studio;
use crate::theme::{self, pal};
use crate::views::channel_rack::add_generator;
use crate::views::widgets::{self, Icon};

/// Chiave temporanea dell'editor di rinomina dei pattern: (pattern, testo in modifica).
fn rename_id() -> Id {
    Id::new("browser_rename")
}

/// Prima traccia libera della Playlist (successiva all'ultima usata).
pub fn next_free_track(project: &Project) -> u16 {
    project
        .playlist
        .clips
        .iter()
        .map(|c| usize::from(c.track) + 1)
        .max()
        .unwrap_or(0)
        .min(PLAYLIST_TRACKS - 1) as u16
}

/// Tipo di marcatore a sinistra di una voce.
enum Mark {
    Swatch(Color32),
    Icon(Icon, Color32),
}

/// Riga del browser: marcatore, testo, sottotitolo opzionale; evidenziata se selezionata.
fn item(ui: &mut Ui, mark: Mark, label: &str, sub: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 4.0, theme::mix(pal::BG_RAISED(), pal::ACCENT(), 0.14));
        p.rect_filled(
            Rect::from_min_size(rect.left_top() + vec2(0.0, 4.0), vec2(3.0, rect.height() - 8.0)),
            1.5,
            pal::ACCENT(),
        );
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, pal::BG_RAISED());
    }
    let mx = rect.left() + 16.0;
    match mark {
        Mark::Swatch(c) => {
            p.rect_filled(
                Rect::from_center_size(pos2(mx, rect.center().y), vec2(8.0, 8.0)),
                2.0,
                c,
            );
        }
        Mark::Icon(i, c) => widgets::paint_icon(
            p,
            Rect::from_center_size(pos2(mx, rect.center().y), vec2(13.0, 13.0)),
            i,
            c,
        ),
    }
    let tc = if selected {
        pal::TEXT_STRONG()
    } else if resp.hovered() {
        pal::TEXT()
    } else {
        theme::mix(pal::TEXT(), pal::TEXT_DIM(), 0.4)
    };
    let t = p.text(
        pos2(rect.left() + 28.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        tc,
    );
    if !sub.is_empty() && t.right() + 30.0 < rect.right() {
        p.text(
            pos2(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            sub,
            FontId::proportional(10.0),
            pal::TEXT_FAINT(),
        );
    }
    resp
}

/// Intestazione di sezione apribile; restituisce `true` se aperta.
fn section(ui: &mut Ui, title: &str, count: usize, default_open: bool) -> bool {
    let id = Id::new(("browser_section", title));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(default_open);
    ui.add_space(4.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    let p = ui.painter();
    let c = if resp.hovered() { pal::TEXT() } else { pal::TEXT_DIM() };
    let a = pos2(rect.left() + 10.0, rect.center().y);
    let tri = if open {
        vec![a + vec2(-3.5, -2.0), a + vec2(3.5, -2.0), a + vec2(0.0, 2.5)]
    } else {
        vec![a + vec2(-2.0, -3.5), a + vec2(2.5, 0.0), a + vec2(-2.0, 3.5)]
    };
    p.add(egui::Shape::convex_polygon(tri, c, Stroke::NONE));
    p.text(
        pos2(rect.left() + 20.0, rect.center().y),
        Align2::LEFT_CENTER,
        title.to_uppercase(),
        theme::bold_font(10.5),
        c,
    );
    let badge = format!("{count}");
    let br = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 15.0));
    p.rect_filled(br, 7.5, pal::BG_INSET());
    p.text(
        br.center(),
        Align2::CENTER_CENTER,
        badge,
        FontId::proportional(9.5),
        pal::TEXT_FAINT(),
    );
    open
}

/// Disegna il browser.
pub fn show(ui: &mut Ui, studio: &mut Studio) {
    // Intestazione.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), widgets::HEADER_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        rect,
        egui::CornerRadius {
            nw: 6,
            ne: 6,
            sw: 0,
            se: 0,
        },
        pal::BG_RAISED(),
    );
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, pal::BORDER()),
    );
    widgets::paint_icon(
        p,
        Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(15.0, 15.0)),
        Icon::Folder,
        pal::ACCENT(),
    );
    p.text(
        pos2(rect.left() + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Browser",
        theme::bold_font(13.0),
        pal::TEXT(),
    );
    let mut btn = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                rect.right_top() - vec2(30.0, 0.0),
                rect.right_bottom(),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if crate::views::samples::collapse_button(&mut btn, true).clicked() {
        crate::ui_settings::toggle_browser(studio);
    }

    egui::ScrollArea::vertical()
        .id_salt("browser_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    if section(ui, "Pattern", studio.project.patterns.len(), true) {
                        patterns(ui, studio);
                    }
                    if section(ui, "Canali", studio.project.channels.len(), true) {
                        channels(ui, studio);
                    }
                    let gens = registry()
                        .iter()
                        .filter(|d| d.role == PluginRole::Generator && d.id != mfc_plugins::ids::SILENT)
                        .count();
                    if section(ui, "Strumenti", gens, true) {
                        generators(ui, studio);
                    }
                    let fx = registry().iter().filter(|d| d.role == PluginRole::Effect).count();
                    if section(ui, "Effetti", fx, false) {
                        effects(ui, studio);
                    }
                    let folders = studio.ui.settings.sample_folders.len();
                    if section(ui, "Campioni", folders, true) {
                        crate::views::samples::section(ui, studio);
                    }
                    let ext = studio.host.catalog().len();
                    if section(ui, "Plugin VST3 / CLAP", ext, ext > 0) {
                        external_plugins(ui, studio);
                    }
                    if section(ui, "Automazioni", studio.project.automations.len(), false) {
                        automations(ui, studio);
                    }
                });
        });
}

fn patterns(ui: &mut Ui, studio: &mut Studio) {
    let rename: Option<(PatternId, String)> = ui.ctx().data(|d| d.get_temp(rename_id()));
    let current = studio.current_pattern();
    let tb = studio.project.time;
    let items: Vec<(PatternId, String, Color32, String)> = studio
        .project
        .patterns
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let bars = p.effective_length(&tb) as f64 / tb.ticks_per_bar() as f64;
            (
                p.id,
                p.name.clone(),
                theme::item_color(p.color, i),
                format!("{bars:.0} bt"),
            )
        })
        .collect();
    for (id, name, color, sub) in items {
        if let Some((_, buf)) = rename.as_ref().filter(|(rid, _)| *rid == id) {
            let mut buf = buf.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY));
            if r.lost_focus() {
                let new_name = buf.trim().to_string();
                if !new_name.is_empty() {
                    studio.exec(Command::SetPattern {
                        pattern: id,
                        name: Some(new_name),
                        length: None,
                        color: None,
                    });
                }
                ui.ctx().data_mut(|d| d.remove::<(PatternId, String)>(rename_id()));
            } else {
                r.request_focus();
                ui.ctx().data_mut(|d| d.insert_temp(rename_id(), (id, buf)));
            }
            continue;
        }
        let resp = item(ui, Mark::Swatch(color), &name, &sub, current == Some(id));
        if resp.clicked() {
            studio.exec(Command::SelectPattern { pattern: id });
        }
        if resp.double_clicked() {
            studio.view = crate::state::View::PianoRoll;
        }
        resp.context_menu(|ui| {
            if ui.button("Clona").clicked() {
                studio.exec(Command::ClonePattern { pattern: id });
                ui.close();
            }
            if ui.button("Rinomina").clicked() {
                let start = (id, name.clone());
                ui.ctx().data_mut(|d| d.insert_temp(rename_id(), start));
                ui.close();
            }
            if ui.button("Elimina").clicked() {
                studio.exec(Command::RemovePattern { pattern: id });
                ui.close();
            }
        });
    }
}

fn channels(ui: &mut Ui, studio: &mut Studio) {
    let items: Vec<(ChannelId, String, Color32, String)> = studio
        .project
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let sub = if c.mixer_insert == 0 {
                "M".to_string()
            } else {
                format!("{}", c.mixer_insert)
            };
            (c.id, c.name.clone(), theme::item_color(c.color, i), sub)
        })
        .collect();
    if items.is_empty() {
        ui.label(RichText::new("   Nessun canale").color(pal::TEXT_FAINT()));
    }
    let selected = studio.selected_channel;
    for (id, name, color, sub) in items {
        if item(ui, Mark::Swatch(color), &name, &sub, selected == Some(id)).clicked() {
            studio.selected_channel = Some(id);
        }
    }
}

fn generators(ui: &mut Ui, studio: &mut Studio) {
    for d in registry()
        .iter()
        .filter(|d| d.role == PluginRole::Generator && d.id != mfc_plugins::ids::SILENT)
    {
        let resp = item(ui, Mark::Icon(Icon::Wave, pal::ACCENT()), d.name, "", false)
            .on_hover_text(format!("{}\nClic: aggiunge un canale", d.description));
        if resp.clicked() {
            add_generator(studio, d.id);
        }
    }
}

fn effects(ui: &mut Ui, studio: &mut Studio) {
    let insert = studio.selected_insert;
    for d in registry().iter().filter(|d| d.role == PluginRole::Effect) {
        let resp = item(ui, Mark::Icon(Icon::Plugin, pal::BLUE()), d.name, "", false)
            .on_hover_text(format!("{}\nClic: aggiunge all'insert {insert}", d.description));
        if resp.clicked() {
            if let Some(plugin) = default_instance(d.id) {
                studio.exec(Command::AddEffect {
                    insert,
                    plugin,
                    slot: None,
                });
            }
        }
    }
}

/// Scansiona le cartelle standard dei plugin e salva il catalogo.
pub fn scan_plugins(studio: &mut Studio) {
    let paths = studio.host.search_paths(&[]);
    let report = studio.host.scan(&paths);
    crate::plugins::save_catalog(&studio.host);
    studio.status = format!(
        "Scansione: {} plugin in {} file{}",
        report.plugins.len(),
        report.scanned,
        if report.errors.is_empty() {
            String::new()
        } else {
            format!(" ({} non leggibili)", report.errors.len())
        }
    );
}

/// Aggiunge un plugin del catalogo: strumento → nuovo canale, effetto → insert selezionato.
pub fn add_external(studio: &mut Studio, info: &mfc_host::PluginInfo) {
    let plugin = crate::plugins::new_instance(info);
    if info.is_instrument {
        if let Some(Outcome::Channel(c)) = studio.exec(Command::AddChannel {
            name: info.name.clone(),
            kind: ChannelKind::Generator,
            plugin,
            mixer_insert: None,
        }) {
            studio.selected_channel = Some(c);
        }
    } else {
        studio.exec(Command::AddEffect {
            insert: studio.selected_insert,
            plugin,
            slot: None,
        });
    }
}

fn external_plugins(ui: &mut Ui, studio: &mut Studio) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        if ui
            .small_button("Scansiona")
            .on_hover_text("Cerca i plugin nelle cartelle standard VST3 e CLAP")
            .clicked()
        {
            scan_plugins(studio);
        }
        ui.label(
            RichText::new("o trascina un .vst3/.clap")
                .size(10.5)
                .color(pal::TEXT_FAINT()),
        );
    });
    let insert = studio.selected_insert;
    for info in studio.host.catalog() {
        let (mark, hint) = if info.is_instrument {
            (
                Mark::Icon(Icon::Wave, pal::ACCENT()),
                "Clic: aggiunge un canale".to_string(),
            )
        } else {
            (
                Mark::Icon(Icon::Plugin, pal::BLUE()),
                format!("Clic: aggiunge all'insert {insert}"),
            )
        };
        let sub = info.format.label();
        let resp = item(ui, mark, &info.name, sub, false).on_hover_text(format!(
            "{} · {}\n{}\n{}",
            info.vendor,
            info.category,
            info.path.display(),
            hint
        ));
        if resp.clicked() {
            add_external(studio, &info);
        }
    }
}

fn automations(ui: &mut Ui, studio: &mut Studio) {
    let items: Vec<(AutomationId, String)> = studio
        .project
        .automations
        .iter()
        .map(|a| (a.id, a.name.clone()))
        .collect();
    if items.is_empty() {
        ui.label(RichText::new("   Nessuna automazione").color(pal::TEXT_FAINT()));
    }
    for (id, name) in items {
        let resp = item(ui, Mark::Icon(Icon::Wave, pal::YELLOW()), &name, "", false)
            .on_hover_text("Clic: piazza in fondo alla Playlist");
        if resp.clicked() {
            let track = next_free_track(&studio.project);
            let position = studio.project.song_length();
            studio.exec(Command::PlaceClip {
                track,
                position,
                length: None,
                source: ClipSource::Automation(id),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfc_core::{ClipId, PlaylistClip};

    #[test]
    fn free_track_follows_last_used() {
        let mut p = Project::new("t");
        assert_eq!(next_free_track(&p), 0);
        p.playlist.clips.push(PlaylistClip {
            id: ClipId(1),
            track: 3,
            position: 0,
            length: 96,
            offset: 0,
            source: ClipSource::Automation(AutomationId(9)),
            muted: false,
        });
        assert_eq!(next_free_track(&p), 4);
    }
}
