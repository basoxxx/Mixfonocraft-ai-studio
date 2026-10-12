//! Editor generico dei parametri plugin: una finestra con un controllo per parametro,
//! campione (sampler / audio clip), reset e mini tastiera di prova per i canali.

use egui::{vec2, Context, Id, Ui};
use mfc_core::{ChannelId, ChannelKind, Command, PluginInstance, Project};
use mfc_plugins::{descriptor, ParamDesc, PluginDescriptor};

use crate::state::{PluginTarget, Studio};
use crate::views::channel_rack::{live, settle};
use crate::views::key_name;

/// Velocity della tastiera di prova.
const TEST_VELOCITY: f32 = 0.8;

/// Dati del bersaglio risolti dal progetto.
struct Resolved {
    /// Nome del canale o dell'insert/slot.
    title: String,
    inst: PluginInstance,
    /// Solo per i canali: id, tipo e nota di riferimento.
    channel: Option<(ChannelId, ChannelKind, u8)>,
}

fn resolve(project: &Project, target: PluginTarget) -> Option<Resolved> {
    match target {
        PluginTarget::Channel(id) => project.channel(id).map(|c| Resolved {
            title: c.name.clone(),
            inst: c.plugin.clone(),
            channel: Some((c.id, c.kind, c.root_note)),
        }),
        PluginTarget::Effect { insert, slot } => {
            let fx = project.insert(insert)?.effects.get(slot as usize)?;
            Some(Resolved {
                title: format!("Insert {insert} · FX {}", slot + 1),
                inst: fx.plugin.clone(),
                channel: None,
            })
        }
    }
}

/// `true` per i parametri on/off (checkbox): intervallo 0..1 e nome tipico.
pub fn is_toggle_param(p: &ParamDesc) -> bool {
    p.choices.is_empty()
        && p.min == 0.0
        && p.max == 1.0
        && (matches!(p.name, "reverse" | "loop" | "ping_pong" | "swap" | "key_track") || p.name.starts_with("invert"))
}

/// Comando per impostare un parametro del bersaglio.
fn param_cmd(target: PluginTarget, param: &str, value: f32) -> Command {
    match target {
        PluginTarget::Channel(channel) => Command::SetChannelParam {
            channel,
            param: param.to_string(),
            value,
        },
        PluginTarget::Effect { insert, slot } => Command::SetEffectParam {
            insert,
            slot,
            param: param.to_string(),
            value,
        },
    }
}

/// Chiave stabile del bersaglio, usata per gli id dei widget.
fn salt(target: PluginTarget) -> String {
    match target {
        PluginTarget::Channel(c) => format!("ch{}", c.0),
        PluginTarget::Effect { insert, slot } => format!("fx{insert}_{slot}"),
    }
}

/// Mostra la finestra dell'editor se un bersaglio è aperto.
pub fn show(ctx: &Context, studio: &mut Studio) {
    let Some(target) = studio.plugin_editor else {
        return;
    };
    let Some(resolved) = resolve(&studio.project, target) else {
        studio.plugin_editor = None;
        return;
    };
    let plugin_name = match descriptor(&resolved.inst.kind) {
        Some(d) => d.name.to_string(),
        None => resolved
            .inst
            .foreign_name
            .clone()
            .unwrap_or_else(|| resolved.inst.kind.clone()),
    };
    let mut open = true;
    egui::Window::new(format!("{} · {plugin_name}", resolved.title))
        .open(&mut open)
        .resizable(true)
        .default_width(440.0)
        .show(ctx, |ui| body(ui, studio, target, &resolved));
    if !open {
        studio.plugin_editor = None;
    }
}

fn body(ui: &mut Ui, studio: &mut Studio, target: PluginTarget, r: &Resolved) {
    let inst = &r.inst;
    let Some(desc) = descriptor(&inst.kind) else {
        ui.label(format!(
            "Plugin esterno non supportato: {}",
            inst.foreign_name.as_deref().unwrap_or(&inst.kind)
        ));
        ui.label(format!(
            "Stato opaco preservato: {} byte",
            inst.opaque_state.as_ref().map_or(0, Vec::len)
        ));
        return;
    };
    ui.weak(desc.description);

    if let Some((channel, kind, _)) = r.channel {
        if matches!(kind, ChannelKind::Sampler | ChannelKind::AudioClip) {
            sample_row(ui, studio, channel, inst);
            ui.separator();
        }
    }

    params_grid(ui, studio, target, inst, desc);

    ui.separator();
    if ui
        .button("Reset")
        .on_hover_text("Ripristina tutti i parametri ai valori di default")
        .clicked()
    {
        let cmds: Vec<Command> = desc
            .params
            .iter()
            .map(|p| param_cmd(target, p.name, p.default))
            .collect();
        studio.exec(Command::Batch { commands: cmds });
    }

    if let Some((channel, _, root)) = r.channel {
        ui.separator();
        test_keyboard(ui, studio, channel, root);
    }
}

fn sample_row(ui: &mut Ui, studio: &mut Studio, channel: ChannelId, inst: &PluginInstance) {
    let id = Id::new(("sample_path", channel.0));
    let mut buf: String = ui
        .ctx()
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| inst.sample_path.clone().unwrap_or_default());
    ui.horizontal(|ui| {
        ui.label("Campione");
        if ui
            .add(egui::TextEdit::singleline(&mut buf).desired_width(260.0))
            .changed()
        {
            let stored = buf.clone();
            ui.ctx().data_mut(|d| d.insert_temp(id, stored));
        }
        if ui.button("Carica").clicked() {
            let path = buf.trim();
            studio.exec(Command::SetChannelSample {
                channel,
                path: (!path.is_empty()).then(|| path.to_string()),
            });
        }
    });
}

fn params_grid(ui: &mut Ui, studio: &mut Studio, target: PluginTarget, inst: &PluginInstance, desc: &PluginDescriptor) {
    let key = salt(target);
    egui::Grid::new(format!("plugin_params_{key}"))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for p in desc.params {
                let committed = inst.params.get(p.name).copied().unwrap_or(p.default);
                ui.label(p.label);
                param_control(ui, studio, target, &key, p, committed);
                ui.end_row();
            }
        });
}

fn param_control(ui: &mut Ui, studio: &mut Studio, target: PluginTarget, key: &str, p: &ParamDesc, value: f32) {
    if !p.choices.is_empty() {
        let cur = (value.round() - p.min).max(0.0) as usize;
        let text = p.choices.get(cur).copied().unwrap_or("?");
        egui::ComboBox::from_id_salt(format!("{key}:{}", p.name))
            .selected_text(text)
            .show_ui(ui, |ui| {
                for (i, label) in p.choices.iter().enumerate() {
                    if ui.selectable_label(i == cur, *label).clicked() {
                        studio.exec(param_cmd(target, p.name, p.min + i as f32));
                        ui.close();
                    }
                }
            });
    } else if is_toggle_param(p) {
        let mut on = value >= 0.5;
        if ui.checkbox(&mut on, "").changed() {
            studio.exec(param_cmd(target, p.name, if on { 1.0 } else { 0.0 }));
        }
    } else {
        let id = Id::new(format!("{key}:{}:live", p.name));
        let mut v = live(ui.ctx(), id, value);
        let suffix = if p.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", p.unit)
        };
        let r = ui.add(egui::Slider::new(&mut v, p.min..=p.max).suffix(suffix));
        if let Some(nv) = settle(ui.ctx(), id, &r, v, value) {
            studio.exec(param_cmd(target, p.name, nv));
        }
    }
}

/// Un'ottava di pulsanti a partire dalla nota di riferimento; tenere premuto suona la nota.
fn test_keyboard(ui: &mut Ui, studio: &mut Studio, channel: ChannelId, root: u8) {
    ui.label("Tastiera di prova");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        for k in 0..12u8 {
            let key = root.saturating_add(k).min(127);
            let id = Id::new(("test_key", channel.0, key));
            let down = ui
                .add(egui::Button::new(key_name(key)).min_size(vec2(34.0, 24.0)))
                .is_pointer_button_down_on();
            let was = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
            if down && !was {
                studio.engine.preview_on(channel, key, TEST_VELOCITY);
            } else if !down && was {
                studio.engine.preview_off(channel, key);
            }
            if down != was {
                ui.ctx().data_mut(|d| d.insert_temp(id, down));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(name: &'static str, min: f32, max: f32) -> ParamDesc {
        ParamDesc::new(name, name, min, max, 0.0, "")
    }

    #[test]
    fn toggle_params_are_detected() {
        assert!(is_toggle_param(&param("reverse", 0.0, 1.0)));
        assert!(is_toggle_param(&param("invert_phase", 0.0, 1.0)));
        assert!(is_toggle_param(&param("key_track", 0.0, 1.0)));
        assert!(!is_toggle_param(&param("reverse", 0.0, 2.0)));
        assert!(!is_toggle_param(&param("cutoff", 0.0, 1.0)));
    }

    #[test]
    fn foreign_plugin_has_no_descriptor() {
        assert!(descriptor("ext.vst").is_none());
    }

    #[test]
    fn param_command_targets_channel_or_effect() {
        let c = param_cmd(PluginTarget::Channel(ChannelId(3)), "cutoff", 0.5);
        assert!(matches!(
            c,
            Command::SetChannelParam {
                channel: ChannelId(3),
                ..
            }
        ));
        let e = param_cmd(PluginTarget::Effect { insert: 2, slot: 1 }, "mix", 0.5);
        assert!(matches!(e, Command::SetEffectParam { insert: 2, slot: 1, .. }));
    }
}
