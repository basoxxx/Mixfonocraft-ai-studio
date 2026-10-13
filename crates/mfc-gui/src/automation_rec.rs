//! Registrazione delle automazioni dai controlli della GUI.
//!
//! Quando la registrazione è attiva e il transport suona:
//! - ogni modifica committata con [`Studio::exec`] che tocca un parametro automatizzabile
//!   (volume/pan/pitch/parametri dei canali, volume/pan del mixer, parametri e mix degli
//!   effetti, volume master; anche i parametri dei plugin esterni dall'editor generico)
//!   alimenta il registratore ([`Studio::auto_rec_observe`]);
//! - i parametri cambiati nell'interfaccia originale dei plugin VST3/CLAP arrivano da
//!   [`mfc_host::Host::take_param_changes`] in [`Studio::poll_plugin_param_changes`]: vengono
//!   copiati nei `params` del progetto (`p<id>`) senza passare dall'undo e registrati.
//!
//! Allo stop, alla fine della registrazione o quando il transport si ferma, i punti diventano
//! un unico `Batch` di [`Command::RecordAutomation`] (un solo passo di undo).

use mfc_core::automation_rec::default_name;
use mfc_core::{AutomationRecorder, AutomationTarget, Command, PluginInstance, Project, Tick};

use crate::state::Studio;

/// Stato della registrazione delle automazioni nello [`Studio`].
#[derive(Debug, Default)]
pub struct AutoRecState {
    pub recorder: AutomationRecorder,
}

/// Fader massimo (canali, mixer, master).
const FADER_MAX: f32 = 1.25;
/// Range del pitch di canale in cent.
const PITCH_RANGE: f32 = 4800.0;

/// Range reale di un parametro di plugin: dal descrittore per i nativi, 0..1 (normalizzato)
/// per i parametri `p<id>` dei plugin esterni.
pub fn param_range(plugin: &PluginInstance, param: &str) -> Option<(f32, f32)> {
    if plugin.is_external() {
        return mfc_host::parse_param_key(param).map(|_| (0.0, 1.0));
    }
    let d = mfc_plugins::descriptor(&plugin.kind)?;
    let p = d.params.iter().find(|p| p.name == param)?;
    (p.max > p.min).then_some((p.min, p.max))
}

/// Bersagli toccati da un comando: `(bersaglio, min, max, valore reale)`.
pub fn targets_of(project: &Project, cmd: &Command) -> Vec<(AutomationTarget, f32, f32, f32)> {
    let mut out = Vec::new();
    match cmd {
        Command::SetMasterVolume { volume } => out.push((AutomationTarget::MasterVolume, 0.0, FADER_MAX, *volume)),
        Command::SetChannel {
            channel,
            volume,
            pan,
            pitch_cents,
            ..
        } => {
            let channel = *channel;
            if let Some(v) = volume {
                out.push((AutomationTarget::ChannelVolume { channel }, 0.0, FADER_MAX, *v));
            }
            if let Some(v) = pan {
                out.push((AutomationTarget::ChannelPan { channel }, -1.0, 1.0, *v));
            }
            if let Some(v) = pitch_cents {
                out.push((
                    AutomationTarget::ChannelPitch { channel },
                    -PITCH_RANGE,
                    PITCH_RANGE,
                    *v,
                ));
            }
        }
        Command::SetChannelParam { channel, param, value } => {
            if let Some((lo, hi)) = project.channel(*channel).and_then(|c| param_range(&c.plugin, param)) {
                let t = AutomationTarget::ChannelParam {
                    channel: *channel,
                    param: param.clone(),
                };
                out.push((t, lo, hi, *value));
            }
        }
        Command::SetInsert {
            insert, volume, pan, ..
        } => {
            if let Some(v) = volume {
                out.push((AutomationTarget::MixerVolume { insert: *insert }, 0.0, FADER_MAX, *v));
            }
            if let Some(v) = pan {
                out.push((AutomationTarget::MixerPan { insert: *insert }, -1.0, 1.0, *v));
            }
        }
        Command::SetEffect {
            insert,
            slot,
            mix: Some(v),
            ..
        } => {
            let t = AutomationTarget::EffectMix {
                insert: *insert,
                slot: *slot,
            };
            out.push((t, 0.0, 1.0, *v));
        }
        Command::SetEffectParam {
            insert,
            slot,
            param,
            value,
        } => {
            let fx = project.insert(*insert).and_then(|i| i.effects.get(*slot as usize));
            if let Some((lo, hi)) = fx.and_then(|f| param_range(&f.plugin, param)) {
                let t = AutomationTarget::EffectParam {
                    insert: *insert,
                    slot: *slot,
                    param: param.clone(),
                };
                out.push((t, lo, hi, *value));
            }
        }
        _ => {}
    }
    out
}

impl Studio {
    /// Posizione del transport se si sta registrando e suonando.
    fn auto_rec_position(&self) -> Option<Tick> {
        (self.recording && self.engine.shared.is_playing()).then(|| self.engine.shared.position().max(0.0) as Tick)
    }

    /// Nome leggibile della clip per un bersaglio.
    fn auto_rec_name(&self, target: &AutomationTarget) -> String {
        let plugin_param = |inst: &PluginInstance, owner: String, param: &str| {
            let label = match mfc_host::parse_param_key(param) {
                Some(id) => inst
                    .instance_id
                    .as_deref()
                    .and_then(|iid| self.host.instance(iid))
                    .and_then(|i| i.params().into_iter().find(|p| p.id == id).map(|p| p.name)),
                None => mfc_plugins::descriptor(&inst.kind)
                    .and_then(|d| d.params.iter().find(|p| p.name == param))
                    .map(|p| p.label.to_string()),
            };
            label.map(|l| format!("{owner} · {l}"))
        };
        let named = match target {
            AutomationTarget::ChannelParam { channel, param } => self
                .project
                .channel(*channel)
                .and_then(|c| plugin_param(&c.plugin, c.name.clone(), param)),
            AutomationTarget::EffectParam { insert, slot, param } => self
                .project
                .insert(*insert)
                .and_then(|i| i.effects.get(*slot as usize).map(|f| (i, f)))
                .and_then(|(i, f)| plugin_param(&f.plugin, format!("{} · FX {}", i.name, slot + 1), param)),
            _ => None,
        };
        named.unwrap_or_else(|| default_name(&self.project, target))
    }

    /// Da chiamare dopo ogni comando eseguito con successo: se si registra, i parametri
    /// toccati dal comando alimentano il registratore delle automazioni.
    pub(crate) fn auto_rec_observe(&mut self, cmd: &Command) {
        let Some(pos) = self.auto_rec_position() else { return };
        for (target, min, max, real) in targets_of(&self.project, cmd) {
            let name = self.auto_rec_name(&target);
            self.auto_rec.recorder.record_real(&target, &name, min, max, real, pos);
        }
    }

    /// Da chiamare a ogni frame (in [`Studio::tick_plugins`]): legge i parametri cambiati
    /// nelle interfacce originali dei plugin esterni, li copia nel progetto senza undo e, se
    /// si registra, li registra. Se il transport si è fermato chiude la registrazione.
    pub fn poll_plugin_param_changes(&mut self) {
        let changes = self.host.take_param_changes();
        let pos = self.auto_rec_position();
        for (iid, id, value) in changes {
            let key = mfc_host::param_key(id);
            let v = value as f32;
            let mut targets = Vec::new();
            for c in self.project.channels.iter_mut() {
                if c.plugin.instance_id.as_deref() == Some(iid.as_str()) {
                    c.plugin.params.insert(key.clone(), v);
                    targets.push(AutomationTarget::ChannelParam {
                        channel: c.id,
                        param: key.clone(),
                    });
                }
            }
            for (i, ins) in self.project.mixer.inserts.iter_mut().enumerate() {
                for (s, fx) in ins.effects.iter_mut().enumerate() {
                    if fx.plugin.instance_id.as_deref() == Some(iid.as_str()) {
                        fx.plugin.params.insert(key.clone(), v);
                        targets.push(AutomationTarget::EffectParam {
                            insert: i as u16,
                            slot: s as u16,
                            param: key.clone(),
                        });
                    }
                }
            }
            if targets.is_empty() {
                continue;
            }
            self.dirty = true;
            if let Some(pos) = pos {
                for t in targets {
                    let name = self.auto_rec_name(&t);
                    self.auto_rec.recorder.record(&t, &name, 0.0, 1.0, v, pos);
                }
            }
        }
        if !self.engine.shared.is_playing() && !self.auto_rec.recorder.is_empty() {
            self.finish_automation_recording();
        }
    }

    /// Scrive le automazioni registrate nel progetto (un solo passo di undo).
    pub fn finish_automation_recording(&mut self) {
        let commands = self.auto_rec.recorder.finish();
        if commands.is_empty() {
            return;
        }
        let n = commands.len();
        if self.exec(Command::Batch { commands }).is_some() {
            self.status = if n == 1 {
                "Registrata 1 automazione".into()
            } else {
                format!("Registrate {n} automazioni")
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfc_core::{ChannelId, ChannelKind, ClipSource, Outcome};

    #[test]
    fn commands_map_to_targets_with_ranges() {
        let mut p = Project::new("t");
        let Outcome::Channel(c) = Command::AddChannel {
            name: "Synth".into(),
            kind: ChannelKind::Generator,
            plugin: mfc_plugins::default_instance("mfc.3xosc").unwrap(),
            mixer_insert: None,
        }
        .apply(&mut p)
        .unwrap() else {
            panic!()
        };
        let vol = Command::SetChannel {
            channel: c,
            name: None,
            volume: Some(0.5),
            pan: Some(-0.5),
            pitch_cents: None,
            muted: Some(true),
            mixer_insert: None,
            root_note: None,
            color: None,
        };
        let t = targets_of(&p, &vol);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], (AutomationTarget::ChannelVolume { channel: c }, 0.0, 1.25, 0.5));
        assert_eq!(t[1], (AutomationTarget::ChannelPan { channel: c }, -1.0, 1.0, -0.5));

        let d = mfc_plugins::descriptor("mfc.3xosc").unwrap();
        let param = &d.params[0];
        let t = targets_of(
            &p,
            &Command::SetChannelParam {
                channel: c,
                param: param.name.into(),
                value: param.min,
            },
        );
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].1, t[0].2), (param.min, param.max));
        // Parametro sconosciuto / canale inesistente: niente.
        let unknown = Command::SetChannelParam {
            channel: c,
            param: "nope".into(),
            value: 1.0,
        };
        assert!(targets_of(&p, &unknown).is_empty());
        let missing = Command::SetChannelParam {
            channel: ChannelId(999),
            param: param.name.into(),
            value: 1.0,
        };
        assert!(targets_of(&p, &missing).is_empty());
        // Comandi non automatizzabili.
        assert!(targets_of(&p, &Command::SetTempo { bpm: 100.0 }).is_empty());
        let clip = Command::PlaceClip {
            track: 0,
            position: 0,
            length: None,
            source: ClipSource::Pattern(p.patterns[0].id),
        };
        assert!(targets_of(&p, &clip).is_empty());
    }

    #[test]
    fn external_params_are_normalized() {
        let mut ext = PluginInstance::new("clap:/x.clap::com.x");
        assert_eq!(param_range(&ext, "p7"), Some((0.0, 1.0)));
        assert_eq!(param_range(&ext, "gain"), None);
        ext.kind = "mfc.reverb".into();
        assert!(param_range(&ext, "p7").is_none());
    }
}
