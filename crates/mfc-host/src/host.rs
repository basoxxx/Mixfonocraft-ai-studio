//! Host: catalogo dei plugin, registro delle istanze vive, aggancio al motore.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mfc_core::PluginInstance;
use mfc_plugins::{Effect, ExternalHost, Generator};
use parking_lot::{Mutex, RwLock};

use crate::scan::{self, ScanReport};
use crate::wrapper::{ExternalEffect, ExternalGenerator};
use crate::{parse_kind, parse_param_key, ExternalInstance, FormatBackend, HostError, PluginInfo, DEFAULT_MAX_BLOCK};

struct Live {
    kind: String,
    instance: Arc<dyn ExternalInstance>,
    sample_rate: f32,
    /// Ultimi valori dei parametri applicati dal progetto: a ogni ricompilazione si
    /// riapplicano solo quelli cambiati (undo/redo), senza sovrascrivere le modifiche
    /// fatte nel frattempo dall'interfaccia del plugin.
    applied: HashMap<u32, f32>,
}

/// Applica i parametri del progetto diversi da quelli già applicati.
fn apply_params(inst: &PluginInstance, instance: &dyn ExternalInstance, applied: &mut HashMap<u32, f32>) {
    for (k, v) in &inst.params {
        if let Some(pid) = parse_param_key(k) {
            if applied.get(&pid) != Some(v) {
                instance.set_param_normalized(pid, (*v as f64).clamp(0.0, 1.0));
                applied.insert(pid, *v);
            }
        }
    }
}

/// Host dei plugin esterni.
pub struct Host {
    backends: Vec<Box<dyn FormatBackend>>,
    live: Mutex<HashMap<String, Live>>,
    catalog: RwLock<Vec<PluginInfo>>,
    /// Ultimi errori di creazione (per mostrarli nella GUI).
    errors: Mutex<Vec<String>>,
    /// Istanze rimosse dal progetto ma forse ancora usate dal motore: vengono distrutte
    /// da [`Host::idle`] sul thread UI, mai dal thread audio.
    graveyard: Mutex<Vec<Arc<dyn ExternalInstance>>>,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    /// Host con i backend VST3 e CLAP.
    pub fn new() -> Self {
        Self::with_backends(vec![
            Box::new(crate::vst3::Vst3Backend::new()),
            Box::new(crate::clap::ClapBackend::new()),
        ])
    }

    pub fn with_backends(backends: Vec<Box<dyn FormatBackend>>) -> Self {
        Self {
            backends,
            live: Mutex::new(HashMap::new()),
            catalog: RwLock::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            graveyard: Mutex::new(Vec::new()),
        }
    }

    fn backend(&self, format: crate::PluginFormat) -> Option<&dyn FormatBackend> {
        self.backends.iter().find(|b| b.format() == format).map(|b| b.as_ref())
    }

    /// Cartelle standard di tutti i formati più quelle aggiuntive indicate.
    pub fn search_paths(&self, extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.backends.iter().flat_map(|b| b.default_search_paths()).collect();
        v.extend(extra.iter().cloned());
        v.dedup();
        v
    }

    /// Scansiona le cartelle e aggiorna il catalogo. Gli errori dei singoli bundle non
    /// interrompono la scansione e finiscono nel report.
    pub fn scan(&self, paths: &[PathBuf]) -> ScanReport {
        let backends: Vec<&dyn FormatBackend> = self.backends.iter().map(|b| b.as_ref()).collect();
        let report = scan::scan_paths(&backends, paths);
        let mut cat = report.plugins.clone();
        cat.sort_by_key(|p| p.name.to_lowercase());
        *self.catalog.write() = cat;
        report
    }

    /// Scansiona un singolo bundle/file (usato dal drag & drop).
    pub fn scan_file(&self, path: &Path) -> Result<Vec<PluginInfo>, HostError> {
        let backend = self
            .backends
            .iter()
            .find(|b| b.is_candidate(path))
            .ok_or_else(|| HostError::Unsupported(format!("{}", path.display())))?;
        let found = backend.scan(path)?;
        let mut cat = self.catalog.write();
        for p in &found {
            if !cat.iter().any(|c| c.kind == p.kind) {
                cat.push(p.clone());
            }
        }
        Ok(found)
    }

    /// Catalogo corrente.
    pub fn catalog(&self) -> Vec<PluginInfo> {
        self.catalog.read().clone()
    }

    /// Sostituisce il catalogo (es. caricato da cache).
    pub fn set_catalog(&self, plugins: Vec<PluginInfo>) {
        *self.catalog.write() = plugins;
    }

    /// Informazioni di catalogo per un kind.
    pub fn info(&self, kind: &str) -> Option<PluginInfo> {
        self.catalog.read().iter().find(|p| p.kind == kind).cloned()
    }

    /// Istanza viva per `instance_id`.
    pub fn instance(&self, instance_id: &str) -> Option<Arc<dyn ExternalInstance>> {
        self.live.lock().get(instance_id).map(|l| l.instance.clone())
    }

    /// Restituisce l'istanza per il `PluginInstance` del progetto, creandola se serve
    /// (ripristinando stato e parametri salvati).
    pub fn get_or_create(
        &self,
        inst: &PluginInstance,
        sample_rate: f32,
    ) -> Result<Arc<dyn ExternalInstance>, HostError> {
        let id = inst.instance_id.clone().unwrap_or_else(|| inst.kind.clone());
        {
            let mut live = self.live.lock();
            if let Some(l) = live.get_mut(&id) {
                if l.kind == inst.kind {
                    if (l.sample_rate - sample_rate).abs() > 0.5 {
                        l.instance.prepare(sample_rate, DEFAULT_MAX_BLOCK)?;
                        l.sample_rate = sample_rate;
                    }
                    apply_params(inst, l.instance.as_ref(), &mut l.applied);
                    return Ok(l.instance.clone());
                }
            }
        }
        let parsed = parse_kind(&inst.kind).ok_or_else(|| HostError::BadKind(inst.kind.clone()))?;
        let backend = self
            .backend(parsed.format)
            .ok_or_else(|| HostError::Unsupported(parsed.format.label().into()))?;
        let instance = backend.instantiate(&parsed, sample_rate, DEFAULT_MAX_BLOCK)?;
        if let Some(state) = &inst.opaque_state {
            if let Err(e) = instance.load_state(state) {
                self.errors
                    .lock()
                    .push(format!("{}: stato non ripristinato ({e})", instance.info().name));
            }
        }
        let mut applied = HashMap::new();
        apply_params(inst, instance.as_ref(), &mut applied);
        self.live.lock().insert(
            id,
            Live {
                kind: inst.kind.clone(),
                instance: instance.clone(),
                sample_rate,
                applied,
            },
        );
        Ok(instance)
    }

    /// Chiude le istanze che non compaiono più nel progetto.
    pub fn retain(&self, keep: &[String]) {
        let mut live = self.live.lock();
        let mut grave = self.graveyard.lock();
        live.retain(|id, l| {
            let keep_it = keep.iter().any(|k| k == id);
            if !keep_it {
                l.instance.close_editor();
                grave.push(l.instance.clone());
            }
            keep_it
        });
    }

    /// Da chiamare periodicamente dal thread UI: `idle` delle istanze vive e distruzione
    /// di quelle rimosse che il motore non usa più (così il plugin non viene mai
    /// distrutto dal thread audio).
    pub fn idle(&self) {
        let live: Vec<Arc<dyn ExternalInstance>> = self.live.lock().values().map(|l| l.instance.clone()).collect();
        for i in &live {
            i.idle();
        }
        let dead: Vec<Arc<dyn ExternalInstance>> = {
            let mut grave = self.graveyard.lock();
            let (dead, alive) = grave.drain(..).partition(|i| Arc::strong_count(i) == 1);
            *grave = alive;
            dead
        };
        drop(dead);
    }

    /// Istanze in attesa di distruzione.
    pub fn pending_drops(&self) -> usize {
        self.graveyard.lock().len()
    }

    /// Id delle istanze vive.
    pub fn live_ids(&self) -> Vec<String> {
        self.live.lock().keys().cloned().collect()
    }

    /// Stato di tutte le istanze vive: (instance_id, blob). Usato prima del salvataggio.
    pub fn collect_states(&self) -> Vec<(String, Result<Vec<u8>, HostError>)> {
        let live: Vec<(String, Arc<dyn ExternalInstance>)> = self
            .live
            .lock()
            .iter()
            .map(|(k, l)| (k.clone(), l.instance.clone()))
            .collect();
        live.into_iter().map(|(k, i)| (k, i.save_state())).collect()
    }

    /// Errori accumulati (svuotati alla lettura).
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.errors.lock())
    }

    fn report(&self, inst: &PluginInstance, e: HostError) {
        self.errors
            .lock()
            .push(format!("{}: {e}", inst.foreign_name.as_deref().unwrap_or(&inst.kind)));
    }
}

impl ExternalHost for Host {
    fn create_generator(&self, inst: &PluginInstance, sample_rate: f32) -> Option<Box<dyn Generator>> {
        match self.get_or_create(inst, sample_rate) {
            Ok(i) => Some(Box::new(ExternalGenerator::new(i))),
            Err(e) => {
                self.report(inst, e);
                None
            }
        }
    }

    fn create_effect(&self, inst: &PluginInstance, sample_rate: f32) -> Option<Box<dyn Effect>> {
        match self.get_or_create(inst, sample_rate) {
            Ok(i) => Some(Box::new(ExternalEffect::new(i))),
            Err(e) => {
                self.report(inst, e);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrapper::tests::FakeInstance;
    use crate::{ParsedKind, PluginFormat};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeBackend(Arc<AtomicUsize>);

    impl FormatBackend for FakeBackend {
        fn format(&self) -> PluginFormat {
            PluginFormat::Vst3
        }
        fn default_search_paths(&self) -> Vec<PathBuf> {
            vec![]
        }
        fn is_candidate(&self, path: &Path) -> bool {
            path.extension().is_some_and(|e| e == "vst3")
        }
        fn scan(&self, _: &Path) -> Result<Vec<PluginInfo>, HostError> {
            Ok(vec![FakeInstance::new().info])
        }
        fn instantiate(&self, _: &ParsedKind, _: f32, _: usize) -> Result<Arc<dyn ExternalInstance>, HostError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(FakeInstance::new()))
        }
    }

    #[test]
    fn instances_are_reused_and_state_restored() {
        let count = Arc::new(AtomicUsize::new(0));
        let host = Host::with_backends(vec![Box::new(FakeBackend(count.clone()))]);
        let mut pi = PluginInstance::new("vst3:/fake.vst3::00");
        pi.instance_id = Some("ext1".into());
        pi.opaque_state = Some(0.25f64.to_le_bytes().to_vec());
        let a = host.get_or_create(&pi, 48_000.0).unwrap();
        let b = host.get_or_create(&pi, 48_000.0).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(a.param_normalized(0), 0.25);
        // Parametri salvati nel progetto applicati alla creazione.
        let mut pi2 = PluginInstance::new("vst3:/fake.vst3::00");
        pi2.instance_id = Some("ext2".into());
        pi2.params.insert("p0".into(), 0.75);
        let i2 = host.get_or_create(&pi2, 48_000.0).unwrap();
        assert_eq!(i2.param_normalized(0), 0.75);
        // Modifica dall'interfaccia del plugin: una ricompilazione non la sovrascrive...
        i2.set_param_normalized(0, 0.1);
        host.get_or_create(&pi2, 48_000.0).unwrap();
        assert_eq!(i2.param_normalized(0), 0.1);
        // ...ma un nuovo valore nel progetto (es. undo) sì.
        pi2.params.insert("p0".into(), 0.5);
        host.get_or_create(&pi2, 48_000.0).unwrap();
        assert_eq!(i2.param_normalized(0), 0.5);
        drop(i2);
        assert_eq!(host.collect_states().len(), 2);
        host.retain(&["ext2".into()]);
        assert_eq!(host.live_ids(), vec!["ext2".to_string()]);
        // `a`/`b` (come il motore) tengono viva l'istanza rimossa: non va distrutta.
        host.idle();
        assert_eq!(host.pending_drops(), 1);
        drop((a, b));
        host.idle();
        assert_eq!(host.pending_drops(), 0);
        // Generatore tramite il trait del motore.
        assert!(ExternalHost::create_generator(&host, &pi, 48_000.0).is_some());
        // Kind non valido → errore registrato, nessun generatore.
        let bad = PluginInstance::new("vst3:broken");
        assert!(ExternalHost::create_generator(&host, &bad, 48_000.0).is_none());
        assert_eq!(host.take_errors().len(), 1);
    }
}
