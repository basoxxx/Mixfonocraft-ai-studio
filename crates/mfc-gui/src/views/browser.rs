//! Browser stile FL Studio: pattern, canali, generatori, effetti e automazioni.
//!
//! Click su un generatore aggiunge un canale; click su un effetto lo aggiunge all'insert
//! selezionato; click su un'automazione la piazza in fondo alla Playlist.

use egui::{Id, Ui};
use mfc_core::{AutomationId, ChannelId, ClipSource, Command, PatternId, Project, PLAYLIST_TRACKS};
use mfc_plugins::{default_instance, registry, PluginRole};

use crate::state::Studio;
use crate::views::channel_rack::add_generator;

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

/// Disegna il browser.
pub fn show(ui: &mut Ui, studio: &mut Studio) {
    egui::ScrollArea::vertical()
        .id_salt("browser_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::CollapsingHeader::new("Pattern")
                .default_open(true)
                .show(ui, |ui| patterns(ui, studio));
            egui::CollapsingHeader::new("Canali")
                .default_open(true)
                .show(ui, |ui| channels(ui, studio));
            egui::CollapsingHeader::new("Generatori")
                .default_open(true)
                .show(ui, |ui| generators(ui, studio));
            egui::CollapsingHeader::new("Effetti")
                .default_open(false)
                .show(ui, |ui| effects(ui, studio));
            egui::CollapsingHeader::new("Automazioni")
                .default_open(false)
                .show(ui, |ui| automations(ui, studio));
        });
}

fn patterns(ui: &mut Ui, studio: &mut Studio) {
    let rename: Option<(PatternId, String)> = ui.ctx().data(|d| d.get_temp(rename_id()));
    let current = studio.current_pattern();
    let items: Vec<(PatternId, String)> = studio.project.patterns.iter().map(|p| (p.id, p.name.clone())).collect();
    for (id, name) in items {
        if let Some((_, buf)) = rename.as_ref().filter(|(rid, _)| *rid == id) {
            let mut buf = buf.clone();
            let r = ui.text_edit_singleline(&mut buf);
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
        let resp = ui.selectable_label(current == Some(id), name.as_str());
        if resp.clicked() {
            studio.exec(Command::SelectPattern { pattern: id });
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
    let items: Vec<(ChannelId, String)> = studio.project.channels.iter().map(|c| (c.id, c.name.clone())).collect();
    if items.is_empty() {
        ui.weak("Nessun canale");
    }
    let selected = studio.selected_channel;
    for (id, name) in items {
        if ui.selectable_label(selected == Some(id), name).clicked() {
            studio.selected_channel = Some(id);
        }
    }
}

fn generators(ui: &mut Ui, studio: &mut Studio) {
    for d in registry().iter().filter(|d| d.role == PluginRole::Generator) {
        let resp = ui
            .selectable_label(false, d.name)
            .on_hover_text(format!("{}\nClic: aggiunge un canale", d.description));
        if resp.clicked() {
            add_generator(studio, d.id);
        }
    }
}

fn effects(ui: &mut Ui, studio: &mut Studio) {
    ui.weak("Clic: aggiunge all'insert selezionato");
    for d in registry().iter().filter(|d| d.role == PluginRole::Effect) {
        let resp = ui.selectable_label(false, d.name).on_hover_text(d.description);
        if resp.clicked() {
            if let Some(plugin) = default_instance(d.id) {
                let insert = studio.selected_insert;
                studio.exec(Command::AddEffect {
                    insert,
                    plugin,
                    slot: None,
                });
            }
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
        ui.weak("Nessuna automazione");
    }
    for (id, name) in items {
        let resp = ui
            .selectable_label(false, name)
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
