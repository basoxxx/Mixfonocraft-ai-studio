//! Mixer: una striscia per insert (meter, fader, pan, mute/solo, instradamento) e, sotto,
//! il pannello degli effetti e delle mandate dell'insert selezionato.

use std::collections::BTreeSet;

use egui::{vec2, Id, RichText, Ui};
use mfc_core::{fader_gain, Command, MixerInsert, Project, Route, FX_SLOTS, MASTER, MIXER_INSERTS, MIXER_UNITY};
use mfc_plugins::{default_instance, descriptor, registry, PluginRole};

use crate::state::{PluginTarget, Studio};
use crate::views::channel_rack::{db_text, live, settle};
use crate::views::widgets;

const STRIP_W: f32 = 74.0;
const FADER_H: f32 = 150.0;
/// Insert vuoti mostrati in coda a quelli usati.
const EXTRA_EMPTY: usize = 4;
const FADER_MAX: f32 = 1.25;

/// `true` se l'insert ha qualcosa di diverso dal default (nome, effetti, mandate, livelli).
fn is_touched(index: u16, ins: &MixerInsert) -> bool {
    let def = MixerInsert::new(index);
    ins.name != def.name
        || ins.color != def.color
        || !ins.effects.is_empty()
        || ins.routes != def.routes
        || (ins.volume - MIXER_UNITY).abs() > 1e-6
        || ins.pan.abs() > 1e-6
        || ins.stereo_separation.abs() > 1e-6
        || ins.muted
        || ins.solo
        || ins.phase_invert
        || ins.swap_lr
}

/// Indici degli insert da mostrare (senza master): quelli usati da canali, effetti, mandate
/// o modificati, l'insert selezionato e alcuni vuoti. Con `show_all` tutti.
pub fn visible_inserts(project: &Project, selected: u16, show_all: bool) -> Vec<u16> {
    if show_all {
        return (1..=MIXER_INSERTS as u16).collect();
    }
    let mut set: BTreeSet<u16> = BTreeSet::new();
    set.insert(selected);
    for c in &project.channels {
        set.insert(c.mixer_insert);
    }
    for (i, ins) in project.mixer.inserts.iter().enumerate().skip(1) {
        if is_touched(i as u16, ins) {
            set.insert(i as u16);
        }
        set.extend(ins.routes.iter().map(|r| r.target));
    }
    let mut empty_left = EXTRA_EMPTY;
    for i in 1..=MIXER_INSERTS as u16 {
        if empty_left == 0 {
            break;
        }
        if set.insert(i) {
            empty_left -= 1;
        }
    }
    set.into_iter().filter(|&i| i != MASTER).collect()
}

/// Comando `SetInsert` che modifica solo i livelli/flag indicati.
fn insert_cmd(insert: u16, volume: Option<f32>, pan: Option<f32>, muted: Option<bool>, solo: Option<bool>) -> Command {
    Command::SetInsert {
        insert,
        name: None,
        volume,
        pan,
        stereo_separation: None,
        muted,
        solo,
        phase_invert: None,
        swap_lr: None,
    }
}

/// Dati copiati di una striscia.
struct Strip {
    index: u16,
    name: String,
    volume: f32,
    pan: f32,
    muted: bool,
    solo: bool,
    /// L'insert selezionato instrada verso questa striscia.
    routed_from_selected: bool,
    selected: bool,
}

fn strips(project: &Project, selected: u16, visible: &[u16]) -> Vec<Strip> {
    let sel_routes: &[Route] = project.insert(selected).map_or(&[], |i| i.routes.as_slice());
    let master = project.insert(MASTER);
    let mut out = vec![Strip {
        index: MASTER,
        name: "Master".into(),
        volume: project.master_volume,
        pan: 0.0,
        muted: master.is_some_and(|m| m.muted),
        solo: false,
        routed_from_selected: false,
        selected: selected == MASTER,
    }];
    for &i in visible {
        if let Some(ins) = project.insert(i) {
            out.push(Strip {
                index: i,
                name: ins.name.clone(),
                volume: ins.volume,
                pan: ins.pan,
                muted: ins.muted,
                solo: ins.solo,
                routed_from_selected: sel_routes.iter().any(|r| r.target == i),
                selected: selected == i,
            });
        }
    }
    out
}

/// Disegna il mixer.
pub fn show(ui: &mut Ui, studio: &mut Studio) {
    let show_all_id = Id::new("mixer_show_all");
    let mut show_all = ui.ctx().data(|d| d.get_temp::<bool>(show_all_id)).unwrap_or(false);
    ui.horizontal(|ui| {
        ui.strong("Mixer");
        if ui.checkbox(&mut show_all, "Mostra tutti").changed() {
            ui.ctx().data_mut(|d| d.insert_temp(show_all_id, show_all));
        }
    });

    let selected = studio.selected_insert;
    let visible = visible_inserts(&studio.project, selected, show_all);
    let strips = strips(&studio.project, selected, &visible);
    egui::ScrollArea::horizontal()
        .id_salt("mixer_strips")
        .auto_shrink([false, false])
        .max_height(FADER_H + 110.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for s in &strips {
                    strip_ui(ui, studio, s);
                }
            });
        });

    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("mixer_detail")
        .auto_shrink([false, false])
        .show(ui, |ui| detail_ui(ui, studio));
}

fn strip_ui(ui: &mut Ui, studio: &mut Studio, s: &Strip) {
    ui.vertical(|ui| {
        ui.set_width(STRIP_W);
        let name = egui::Button::new(s.name.as_str())
            .selected(s.selected)
            .min_size(vec2(STRIP_W - 6.0, 18.0));
        if ui.add(name).clicked() {
            studio.selected_insert = s.index;
        }
        ui.horizontal(|ui| {
            let (l, r) = studio.engine.shared.take_meter(usize::from(s.index));
            widgets::meter(ui, l, r, vec2(10.0, FADER_H));
            fader(ui, studio, s);
        });
        let gain = fader_gain(s.volume, MIXER_UNITY);
        ui.label(RichText::new(db_text(gain)).small());
        if s.index == MASTER {
            return;
        }
        pan_knob(ui, studio, s);
        ui.horizontal(|ui| {
            let mut muted = s.muted;
            if ui.toggle_value(&mut muted, "M").changed() {
                studio.exec(insert_cmd(s.index, None, None, Some(muted), None));
            }
            let mut solo = s.solo;
            if ui.toggle_value(&mut solo, "S").changed() {
                studio.exec(insert_cmd(s.index, None, None, None, Some(solo)));
            }
        });
        let sel = studio.selected_insert;
        let route = ui
            .add_enabled(!s.selected, egui::Button::new("→").selected(s.routed_from_selected))
            .on_hover_text(format!("Instrada insert {sel} verso questo insert"));
        if route.clicked() {
            let gain = if s.routed_from_selected { None } else { Some(1.0) };
            studio.exec(Command::SetRoute {
                from: sel,
                to: s.index,
                gain,
            });
        }
    });
}

fn fader(ui: &mut Ui, studio: &mut Studio, s: &Strip) {
    let id = Id::new(("mixer_fader", s.index));
    let mut v = live(ui.ctx(), id, s.volume);
    let r = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = FADER_H;
            ui.add(egui::Slider::new(&mut v, 0.0..=FADER_MAX).vertical().show_value(false))
        })
        .inner;
    if let Some(nv) = settle(ui.ctx(), id, &r, v, s.volume) {
        if s.index == MASTER {
            studio.exec(Command::SetMasterVolume { volume: nv });
        } else {
            studio.exec(insert_cmd(s.index, Some(nv), None, None, None));
        }
    }
    r.on_hover_text(format!("Fader {v:.2}"));
}

fn pan_knob(ui: &mut Ui, studio: &mut Studio, s: &Strip) {
    let id = Id::new(("mixer_pan", s.index));
    let mut v = live(ui.ctx(), id, s.pan);
    let r = widgets::knob(ui, &mut v, -1.0, 1.0, 0.0, 20.0);
    if let Some(nv) = settle(ui.ctx(), id, &r, v, s.pan) {
        studio.exec(insert_cmd(s.index, None, Some(nv), None, None));
    }
    r.on_hover_text(format!("Pan {v:+.2}"));
}

/// Nome mostrato di un plugin: descrittore, altrimenti nome esterno o id.
fn plugin_label(kind: &str, foreign: Option<&str>) -> String {
    descriptor(kind).map(|d| d.name).or(foreign).unwrap_or(kind).to_string()
}

fn detail_ui(ui: &mut Ui, studio: &mut Studio) {
    let sel = studio.selected_insert;
    let Some(ins) = studio.project.insert(sel).cloned() else {
        ui.label("Insert non valido.");
        return;
    };
    let title = if sel == MASTER {
        "Master".to_string()
    } else {
        format!("Insert {sel}: {}", ins.name)
    };
    ui.strong(title);

    ui.label("Effetti");
    for slot in 0..FX_SLOTS {
        ui.horizontal(|ui| {
            ui.monospace(format!("{:>2}", slot + 1));
            if let Some(fx) = ins.effects.get(slot) {
                let mut enabled = fx.enabled;
                if ui.checkbox(&mut enabled, "").changed() {
                    studio.exec(Command::SetEffect {
                        insert: sel,
                        slot: slot as u16,
                        enabled: Some(enabled),
                        mix: None,
                    });
                }
                let name = plugin_label(&fx.plugin.kind, fx.plugin.foreign_name.as_deref());
                ui.add_sized(vec2(170.0, 18.0), egui::Label::new(name));
                let id = Id::new(("fx_mix", sel, slot));
                let mut mix = live(ui.ctx(), id, fx.mix);
                let r = ui.add(egui::Slider::new(&mut mix, 0.0..=1.0).text("Mix"));
                if let Some(nv) = settle(ui.ctx(), id, &r, mix, fx.mix) {
                    studio.exec(Command::SetEffect {
                        insert: sel,
                        slot: slot as u16,
                        enabled: None,
                        mix: Some(nv),
                    });
                }
                if ui.button("Modifica").clicked() {
                    studio.plugin_editor = Some(PluginTarget::Effect {
                        insert: sel,
                        slot: slot as u16,
                    });
                }
                if ui.small_button("✖").on_hover_text("Rimuovi effetto").clicked() {
                    studio.exec(Command::RemoveEffect {
                        insert: sel,
                        slot: slot as u16,
                    });
                }
            } else if slot == ins.effects.len() {
                egui::ComboBox::from_id_salt(("fx_add", sel, slot))
                    .selected_text("— aggiungi effetto —")
                    .show_ui(ui, |ui| {
                        for d in registry().iter().filter(|d| d.role == PluginRole::Effect) {
                            if ui
                                .selectable_label(false, d.name)
                                .on_hover_text(d.description)
                                .clicked()
                            {
                                if let Some(plugin) = default_instance(d.id) {
                                    studio.exec(Command::AddEffect {
                                        insert: sel,
                                        plugin,
                                        slot: None,
                                    });
                                }
                                ui.close();
                            }
                        }
                    });
            } else {
                ui.weak("(vuoto)");
            }
        });
    }

    ui.separator();
    ui.label("Mandate (uscite verso altri insert)");
    if ins.routes.is_empty() {
        ui.weak("Nessuna mandata");
    }
    for r in &ins.routes {
        ui.horizontal(|ui| {
            let target_name = studio
                .project
                .insert(r.target)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            ui.label(format!("→ {} {target_name}", r.target));
            let id = Id::new(("route_gain", sel, r.target));
            let mut g = live(ui.ctx(), id, r.gain);
            let resp = ui.add(
                egui::DragValue::new(&mut g)
                    .range(0.0..=2.0)
                    .speed(0.01)
                    .fixed_decimals(2),
            );
            if let Some(nv) = settle(ui.ctx(), id, &resp, g, r.gain) {
                studio.exec(Command::SetRoute {
                    from: sel,
                    to: r.target,
                    gain: Some(nv),
                });
            }
            if ui.small_button("✖").on_hover_text("Rimuovi mandata").clicked() {
                studio.exec(Command::SetRoute {
                    from: sel,
                    to: r.target,
                    gain: None,
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfc_core::{ChannelKind, Outcome, PluginInstance};

    #[test]
    fn default_project_shows_master_and_empty_tail() {
        let p = Project::new("t");
        let v = visible_inserts(&p, MASTER, false);
        assert_eq!(v, vec![1, 2, 3, 4]);
        assert!(!v.contains(&MASTER));
    }

    #[test]
    fn used_and_selected_inserts_are_visible() {
        let mut p = Project::new("t");
        let Outcome::Channel(_) = Command::AddChannel {
            name: "Kick".into(),
            kind: ChannelKind::Generator,
            plugin: PluginInstance::new("mfc.kick"),
            mixer_insert: Some(40),
        }
        .apply(&mut p)
        .unwrap() else {
            panic!("canale non creato");
        };
        let v = visible_inserts(&p, 90, false);
        assert!(v.contains(&40));
        assert!(v.contains(&90));
        assert_eq!(visible_inserts(&p, MASTER, true).len(), MIXER_INSERTS);
    }

    #[test]
    fn route_target_marks_insert_used() {
        let mut p = Project::new("t");
        Command::SetRoute {
            from: 2,
            to: 70,
            gain: Some(0.5),
        }
        .apply(&mut p)
        .unwrap();
        assert!(visible_inserts(&p, MASTER, false).contains(&70));
    }

    #[test]
    fn plugin_label_fallbacks() {
        assert_eq!(plugin_label("mfc.reverb", None), "Reverb");
        assert_eq!(plugin_label("ext.vst", Some("Vendor X")), "Vendor X");
        assert_eq!(plugin_label("ext.vst", None), "ext.vst");
    }
}
