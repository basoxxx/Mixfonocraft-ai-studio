//! Editor generico dei parametri plugin: una finestra con un controllo per parametro,
//! campione (sampler / audio clip), reset e mini tastiera di prova per i canali.

use egui::{pos2, vec2, Align2, Color32, Context, FontId, Id, Rect, RichText, Sense, Stroke, Ui};
use mfc_core::{ChannelId, ChannelKind, Command, PluginInstance, Project};
use mfc_plugins::{descriptor, ParamDesc, PluginDescriptor};

use crate::state::{PluginTarget, Studio};
use crate::theme::{self, pal};
use crate::views::channel_rack::{live, settle};
use crate::views::key_name;
use crate::views::widgets::{self, Icon};

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
    egui::Window::new(RichText::new(format!("{plugin_name}  ·  {}", resolved.title)).font(theme::bold_font(13.0)))
        .id(Id::new("plugin_editor_window"))
        .open(&mut open)
        .resizable(true)
        .collapsible(false)
        .default_width(520.0)
        .default_pos(ctx.content_rect().center() - vec2(260.0, 220.0))
        .show(ctx, |ui| body(ui, studio, target, &resolved, &plugin_name));
    if !open {
        studio.plugin_editor = None;
    }
}

fn body(ui: &mut Ui, studio: &mut Studio, target: PluginTarget, r: &Resolved, plugin_name: &str) {
    let inst = &r.inst;
    if inst.is_external() {
        external_body(ui, studio, target, r, plugin_name);
        return;
    }
    let Some(desc) = descriptor(&inst.kind) else {
        ui.label(RichText::new(format!("Plugin esterno non supportato: {plugin_name}")).color(pal::YELLOW));
        ui.label(
            RichText::new(format!(
                "Lo stato originale ({} byte) è preservato e verrà riscritto all'export .flp.",
                inst.opaque_state.as_ref().map_or(0, Vec::len)
            ))
            .color(pal::TEXT_DIM),
        );
        return;
    };
    // Intestazione del plugin.
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
        ui.painter().rect_filled(ir, 6.0, pal::BG_INSET);
        let icon = if desc.role == mfc_plugins::PluginRole::Generator {
            Icon::Wave
        } else {
            Icon::Plugin
        };
        widgets::paint_icon(ui.painter(), ir.shrink(8.0), icon, pal::ACCENT);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(desc.name).font(theme::bold_font(15.0)).color(pal::TEXT));
            ui.label(RichText::new(desc.description).size(11.0).color(pal::TEXT_DIM));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        });
    });
    ui.add_space(6.0);

    if let Some((channel, kind, _)) = r.channel {
        if matches!(kind, ChannelKind::Sampler | ChannelKind::AudioClip) {
            sample_row(ui, studio, channel, inst);
            ui.add_space(4.0);
        }
    }

    egui::ScrollArea::vertical()
        .max_height(420.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            params_grid(ui, studio, target, inst, desc);
        });

    if let Some((channel, _, root)) = r.channel {
        ui.add_space(8.0);
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

/// Gruppo di un parametro: prefisso tipo `osc1`, `band3`, altrimenti "Generale".
pub fn param_group(name: &str) -> String {
    match name.split_once('_') {
        Some((pre, _)) if pre.chars().last().is_some_and(|c| c.is_ascii_digit()) => {
            let (word, num): (String, String) = pre.chars().partition(|c| !c.is_ascii_digit());
            let word = match word.as_str() {
                "osc" => "Oscillatore",
                "band" => "Banda",
                other => other,
            };
            format!("{word} {num}")
        }
        _ => "Generale".to_string(),
    }
}

/// Valore leggibile con unità.
pub fn format_value(p: &ParamDesc, v: f32) -> String {
    match p.unit {
        "Hz" if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        "Hz" => format!("{v:.0} Hz"),
        "ms" => format!("{v:.0} ms"),
        "s" => format!("{v:.3} s"),
        "dB" => format!("{v:+.1} dB"),
        "%" => format!("{v:.0}%"),
        "" if p.max - p.min <= 2.0 => format!("{:.0}%", (v - p.min) / (p.max - p.min).max(1e-6) * 100.0),
        u => format!("{v:.2} {u}"),
    }
}

fn params_grid(ui: &mut Ui, studio: &mut Studio, target: PluginTarget, inst: &PluginInstance, desc: &PluginDescriptor) {
    let key = salt(target);
    let mut groups: Vec<(String, Vec<&ParamDesc>)> = Vec::new();
    for p in desc.params {
        let g = param_group(p.name);
        match groups.iter_mut().find(|(n, _)| *n == g) {
            Some((_, v)) => v.push(p),
            None => groups.push((g, vec![p])),
        }
    }
    for (name, params) in groups {
        egui::Frame::new()
            .fill(pal::BG_PANEL)
            .stroke(Stroke::new(1.0, pal::BORDER))
            .corner_radius(6)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::section_label(ui, &name);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                    for p in params {
                        let committed = inst.params.get(p.name).copied().unwrap_or(p.default);
                        param_control(ui, studio, target, &key, p, committed);
                    }
                });
            });
        ui.add_space(6.0);
    }
}

const CELL_W: f32 = 64.0;

fn param_control(ui: &mut Ui, studio: &mut Studio, target: PluginTarget, key: &str, p: &ParamDesc, value: f32) {
    ui.allocate_ui(vec2(if p.choices.is_empty() { CELL_W } else { 120.0 }, 70.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if !p.choices.is_empty() {
                ui.add_space(8.0);
                let cur = (value.round() - p.min).max(0.0) as usize;
                let text = p.choices.get(cur).copied().unwrap_or("?");
                egui::ComboBox::from_id_salt(format!("{key}:{}", p.name))
                    .width(112.0)
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        for (i, label) in p.choices.iter().enumerate() {
                            if ui.selectable_label(i == cur, *label).clicked() {
                                studio.exec(param_cmd(target, p.name, p.min + i as f32));
                                ui.close();
                            }
                        }
                    });
                ui.label(RichText::new(p.label).size(10.5).color(pal::TEXT_DIM));
            } else if is_toggle_param(p) {
                ui.add_space(12.0);
                let on = value >= 0.5;
                if widgets::toggle(ui, on, if on { "ON" } else { "OFF" }, pal::ACCENT, vec2(48.0, 22.0)).clicked() {
                    studio.exec(param_cmd(target, p.name, if on { 0.0 } else { 1.0 }));
                }
                ui.label(RichText::new(p.label).size(10.5).color(pal::TEXT_DIM));
            } else {
                let id = Id::new(format!("{key}:{}:live", p.name));
                let mut v = live(ui.ctx(), id, value);
                let r = widgets::labeled_knob(ui, p.label, &mut v, p.min, p.max, p.default, 38.0, |x| {
                    format_value(p, x)
                });
                if let Some(nv) = settle(ui.ctx(), id, &r, v, value) {
                    studio.exec(param_cmd(target, p.name, nv));
                }
                r.on_hover_text(format!("{}: {}  (doppio click = default)", p.label, format_value(p, v)));
            }
        });
    });
}

/// Editor di un plugin VST3/CLAP: pulsante per l'interfaccia originale e controlli
/// generici per i parametri esposti dal plugin.
fn external_body(ui: &mut Ui, studio: &mut Studio, target: PluginTarget, r: &Resolved, plugin_name: &str) {
    let inst = &r.inst;
    let id = inst.instance_id.clone().unwrap_or_else(|| inst.kind.clone());
    let live_inst = studio.host.instance(&id);
    let format = mfc_host::parse_kind(&inst.kind).map_or("?", |k| k.format.label());
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
        ui.painter().rect_filled(ir, 6.0, pal::BG_INSET);
        widgets::paint_icon(ui.painter(), ir.shrink(8.0), Icon::Plugin, pal::ACCENT);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(plugin_name).font(theme::bold_font(15.0)).color(pal::TEXT));
            let vendor = live_inst.as_ref().map(|i| i.info().vendor.clone()).unwrap_or_default();
            ui.label(
                RichText::new(format!("{format}  {vendor}"))
                    .size(11.0)
                    .color(pal::TEXT_DIM),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can_open =
                crate::plugin_window::PluginWindow::SUPPORTED && live_inst.as_ref().is_some_and(|i| i.has_editor());
            let open = studio.plugin_windows.contains_key(&id);
            let btn = ui
                .add_enabled(
                    can_open,
                    egui::Button::new(if open { "Interfaccia aperta" } else { "Apri interfaccia" }),
                )
                .on_disabled_hover_text("Interfaccia originale non disponibile: usa i controlli qui sotto");
            if btn.clicked() {
                let title = format!("{plugin_name} · {}", r.title);
                studio.open_plugin_window(&id, &title);
            }
        });
    });
    ui.add_space(6.0);
    let Some(live_inst) = live_inst else {
        ui.label(
            RichText::new("Plugin non caricato: controlla che sia ancora installato nello stesso percorso.")
                .color(pal::YELLOW),
        );
        ui.label(RichText::new(&inst.kind).size(10.5).color(pal::TEXT_FAINT));
        return;
    };

    // Filtro sui parametri (alcuni plugin ne espongono centinaia).
    let filter_id = Id::new(("ext_param_filter", &id));
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Parametri").color(pal::TEXT_DIM));
        if ui
            .add(
                egui::TextEdit::singleline(&mut filter)
                    .hint_text("cerca…")
                    .desired_width(160.0),
            )
            .changed()
        {
            ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
        }
    });
    ui.add_space(4.0);
    let needle = filter.to_lowercase();
    let params: Vec<mfc_host::ExternalParam> = live_inst
        .params()
        .into_iter()
        .filter(|p| !p.hidden && (needle.is_empty() || p.name.to_lowercase().contains(&needle)))
        .collect();
    let key = salt(target);
    egui::ScrollArea::vertical()
        .max_height(420.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if params.is_empty() {
                ui.label(RichText::new("Nessun parametro").color(pal::TEXT_FAINT));
            }
            egui::Grid::new(("ext_params", &key))
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for p in &params {
                        let name = mfc_host::param_key(p.id);
                        let current = live_inst.param_normalized(p.id) as f32;
                        // Valore registrato nel progetto (-1 = mai toccato): decide se committare.
                        let saved = inst.params.get(&name).copied().unwrap_or(-1.0);
                        ui.label(RichText::new(&p.name).color(pal::TEXT));
                        let wid = Id::new(format!("{key}:{name}:live"));
                        let mut v = live(ui.ctx(), wid, current);
                        let text = live_inst.param_display(p.id, v as f64);
                        let r = ui.add(
                            egui::Slider::new(&mut v, 0.0..=1.0)
                                .show_value(false)
                                .text(format!("{text} {}", p.unit)),
                        );
                        if r.changed() {
                            // Ascolto immediato durante il trascinamento; commit al rilascio.
                            live_inst.set_param_normalized(p.id, v as f64);
                        }
                        if r.double_clicked() {
                            live_inst.set_param_normalized(p.id, p.default);
                            studio.exec(param_cmd(target, &name, p.default as f32));
                        } else if let Some(nv) = settle(ui.ctx(), wid, &r, v, saved) {
                            studio.exec(param_cmd(target, &name, nv));
                        }
                        ui.end_row();
                    }
                });
        });

    if let Some((channel, _, root)) = r.channel {
        ui.add_space(8.0);
        test_keyboard(ui, studio, channel, root);
    }
}

/// Tastiera di prova di due ottave dalla nota di riferimento: tenere premuto suona la nota.
fn test_keyboard(ui: &mut Ui, studio: &mut Studio, channel: ChannelId, root: u8) {
    widgets::section_label(ui, "Tastiera di prova");
    let start = root - root % 12;
    let whites: Vec<u8> = (0..24u8)
        .map(|k| start.saturating_add(k))
        .filter(|k| !is_black(*k) && *k <= 127)
        .collect();
    let w = (ui.available_width() / whites.len() as f32).clamp(14.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w * whites.len() as f32, 64.0), Sense::click_and_drag());
    let p = ui.painter();
    let ptr = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on());
    // Tasti neri sopra i bianchi: hit-test prima sui neri.
    let mut blacks = Vec::new();
    let mut hit: Option<u8> = None;
    for (i, k) in whites.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.left() + i as f32 * w, rect.top()),
            vec2(w - 1.0, rect.height()),
        );
        if k % 12 != 4 && k % 12 != 11 && *k < 127 {
            let br = Rect::from_min_size(
                pos2(r.right() - w * 0.3, rect.top()),
                vec2(w * 0.6, rect.height() * 0.6),
            );
            blacks.push((k + 1, br));
        }
        if ptr.is_some_and(|q| r.contains(q)) {
            hit = Some(*k);
        }
    }
    for (k, br) in &blacks {
        if ptr.is_some_and(|q| br.contains(q)) {
            hit = Some(*k);
        }
    }
    let id = Id::new(("test_key_down", channel.0));
    let was: Option<u8> = ui.data(|d| d.get_temp(id));
    if was != hit {
        if let Some(k) = was {
            studio.engine.preview_off(channel, k);
        }
        if let Some(k) = hit {
            studio.engine.preview_on(channel, k, TEST_VELOCITY);
        }
        ui.data_mut(|d| match hit {
            Some(k) => {
                d.insert_temp(id, k);
            }
            None => d.remove::<u8>(id),
        });
    }
    for (i, k) in whites.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.left() + i as f32 * w, rect.top()),
            vec2(w - 1.0, rect.height()),
        );
        let c = if hit == Some(*k) {
            pal::ACCENT
        } else {
            Color32::from_rgb(0xe6, 0xe9, 0xec)
        };
        p.rect_filled(
            r,
            egui::CornerRadius {
                sw: 3,
                se: 3,
                ..Default::default()
            },
            c,
        );
        if k % 12 == 0 {
            p.text(
                r.center_bottom() - vec2(0.0, 8.0),
                Align2::CENTER_CENTER,
                key_name(*k),
                FontId::proportional(8.5),
                Color32::from_gray(90),
            );
        }
    }
    for (k, br) in &blacks {
        let c = if hit == Some(*k) {
            pal::ACCENT
        } else {
            Color32::from_rgb(0x1b, 0x1e, 0x22)
        };
        p.rect_filled(
            *br,
            egui::CornerRadius {
                sw: 2,
                se: 2,
                ..Default::default()
            },
            c,
        );
    }
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
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
    fn groups_and_formatting() {
        assert_eq!(param_group("osc2_coarse"), "Oscillatore 2");
        assert_eq!(param_group("band7_gain"), "Banda 7");
        assert_eq!(param_group("cutoff"), "Generale");
        assert_eq!(param_group("pitch_semitones"), "Generale");
        let hz = ParamDesc::new("cutoff", "Cutoff", 20.0, 20000.0, 1000.0, "Hz");
        assert_eq!(format_value(&hz, 2500.0), "2.50 kHz");
        assert_eq!(format_value(&hz, 440.0), "440 Hz");
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
