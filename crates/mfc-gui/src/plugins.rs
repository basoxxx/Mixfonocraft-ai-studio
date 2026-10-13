//! Collegamento tra lo studio e l'host dei plugin VST3/CLAP: catalogo su disco,
//! istanze usate dal progetto, sincronizzazione dello stato prima del salvataggio.

use std::path::PathBuf;

use mfc_core::{PluginInstance, Project};
use mfc_host::{param_key, parse_param_key, Host, PluginInfo};

/// Cartella di configurazione dell'utente.
pub fn config_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support/Mixfonocraft"))
    } else if cfg!(windows) {
        env("APPDATA").map(|a| a.join("Mixfonocraft"))
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))
            .map(|c| c.join("mixfonocraft"))
    }
}

fn catalog_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("plugins.json"))
}

/// Carica il catalogo salvato dall'ultima scansione.
pub fn load_catalog(host: &Host) {
    let Some(p) = catalog_path() else { return };
    if let Ok(txt) = std::fs::read_to_string(p) {
        if let Ok(list) = serde_json::from_str::<Vec<PluginInfo>>(&txt) {
            host.set_catalog(list);
        }
    }
}

/// Salva il catalogo corrente.
pub fn save_catalog(host: &Host) {
    let Some(p) = catalog_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(txt) = serde_json::to_string_pretty(&host.catalog()) {
        let _ = std::fs::write(p, txt);
    }
}

/// Tutti i plugin del progetto (canali ed effetti), mutabili.
pub fn plugins_mut(project: &mut Project) -> impl Iterator<Item = &mut PluginInstance> {
    let channels = project.channels.iter_mut().map(|c| &mut c.plugin);
    let fx = project
        .mixer
        .inserts
        .iter_mut()
        .flat_map(|i| i.effects.iter_mut().map(|e| &mut e.plugin));
    channels.chain(fx)
}

/// Id delle istanze esterne usate dal progetto.
pub fn external_ids(project: &Project) -> Vec<String> {
    let channels = project.channels.iter().map(|c| &c.plugin);
    let fx = project
        .mixer
        .inserts
        .iter()
        .flat_map(|i| i.effects.iter().map(|e| &e.plugin));
    channels
        .chain(fx)
        .filter(|p| p.is_external())
        .filter_map(|p| p.instance_id.clone())
        .collect()
}

/// Copia nel progetto lo stato corrente delle istanze vive (blob opaco e valori dei
/// parametri già presenti), così il salvataggio e l'undo ritrovano ciò che si sente.
/// Restituisce gli errori dei plugin che non hanno fornito lo stato.
pub fn sync_states(host: &Host, project: &mut Project) -> Vec<String> {
    let mut errors = Vec::new();
    for (id, state) in host.collect_states() {
        let Some(inst) = host.instance(&id) else { continue };
        for p in plugins_mut(project) {
            if p.instance_id.as_deref() != Some(id.as_str()) {
                continue;
            }
            match &state {
                Ok(blob) => p.opaque_state = Some(blob.clone()),
                Err(e) => errors.push(format!("{}: {e}", inst.info().name)),
            }
            let keys: Vec<u32> = p.params.keys().filter_map(|k| parse_param_key(k)).collect();
            for pid in keys {
                p.params.insert(param_key(pid), inst.param_normalized(pid) as f32);
            }
        }
    }
    errors
}

/// Nuova istanza di progetto per un plugin del catalogo (l'`instance_id` lo assegna il comando).
pub fn new_instance(info: &PluginInfo) -> PluginInstance {
    let mut p = PluginInstance::new(info.kind.clone());
    p.foreign_name = Some(info.name.clone());
    p
}

/// Progetto con istanze esterne distinte da quelle del motore live (per il render offline).
pub fn detached_copy(project: &Project) -> Project {
    let mut p = project.clone();
    for inst in plugins_mut(&mut p) {
        if inst.is_external() {
            if let Some(id) = &inst.instance_id {
                inst.instance_id = Some(format!("render:{id}"));
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfc_core::{ChannelKind, Command, History};

    #[test]
    fn external_ids_and_detached_copy() {
        let mut p = Project::new("t");
        let mut h = History::default();
        h.execute(
            &mut p,
            &Command::AddChannel {
                name: "Synth".into(),
                kind: ChannelKind::Generator,
                plugin: PluginInstance::new("clap:/x.clap::com.x"),
                mixer_insert: None,
            },
        )
        .unwrap();
        let ids = external_ids(&p);
        assert_eq!(ids.len(), 1);
        let d = detached_copy(&p);
        assert_eq!(external_ids(&d), vec![format!("render:{}", ids[0])]);
    }
}
