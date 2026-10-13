//! Propagazione del tempo ai plugin: costruzione del grafo, reinstallazione,
//! automazione del tempo e render offline. Usa un host "esterno" sonda che registra
//! le chiamate a `set_tempo` (binario di test dedicato: l'host è globale).

use std::sync::{Arc, Mutex};

use mfc_core::*;
use mfc_engine::*;
use mfc_plugins::{Effect, ExternalHost, Generator, NoteOn, PluginDescriptor};

/// Registro condiviso: (id istanza, ultimo BPM ricevuto).
type Log = Arc<Mutex<Vec<(usize, f32)>>>;

struct Probe {
    id: usize,
    log: Log,
}

impl Probe {
    fn record(&self, bpm: f32) {
        self.log.lock().unwrap().push((self.id, bpm));
    }
}

impl Generator for Probe {
    fn descriptor(&self) -> &'static PluginDescriptor {
        mfc_plugins::descriptor(mfc_plugins::ids::SILENT).unwrap()
    }
    fn set_sample_rate(&mut self, _sample_rate: f32) {}
    fn set_param(&mut self, _name: &str, _value: f32) {}
    fn note_on(&mut self, _note: NoteOn) {}
    fn note_off(&mut self, _voice_id: u64) {}
    fn all_notes_off(&mut self) {}
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}
    fn active_voices(&self) -> usize {
        0
    }
    fn set_tempo(&mut self, bpm: f32) {
        self.record(bpm);
    }
}

impl Effect for Probe {
    fn descriptor(&self) -> &'static PluginDescriptor {
        mfc_plugins::descriptor(mfc_plugins::ids::GAIN).unwrap()
    }
    fn set_sample_rate(&mut self, _sample_rate: f32) {}
    fn set_param(&mut self, _name: &str, _value: f32) {}
    fn reset(&mut self) {}
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}
    fn set_tempo(&mut self, bpm: f32) {
        self.record(bpm);
    }
}

#[derive(Default)]
struct ProbeHost {
    next: Mutex<usize>,
    gen_log: Log,
    fx_log: Log,
}

impl ProbeHost {
    fn next_id(&self) -> usize {
        let mut n = self.next.lock().unwrap();
        *n += 1;
        *n
    }
}

impl ExternalHost for ProbeHost {
    fn create_generator(&self, _inst: &PluginInstance, _sr: f32) -> Option<Box<dyn Generator>> {
        Some(Box::new(Probe {
            id: self.next_id(),
            log: self.gen_log.clone(),
        }))
    }
    fn create_effect(&self, _inst: &PluginInstance, _sr: f32) -> Option<Box<dyn Effect>> {
        Some(Box::new(Probe {
            id: self.next_id(),
            log: self.fx_log.clone(),
        }))
    }
}

/// Ultimo BPM ricevuto da ciascuna istanza.
fn last_per_instance(log: &Log) -> Vec<(usize, f32)> {
    let log = log.lock().unwrap();
    let mut out: Vec<(usize, f32)> = Vec::new();
    for &(id, bpm) in log.iter() {
        match out.iter_mut().find(|(i, _)| *i == id) {
            Some(e) => e.1 = bpm,
            None => out.push((id, bpm)),
        }
    }
    out
}

fn project(tempo: f64) -> Project {
    let mut p = Project::new("tempo");
    p.tempo = tempo;
    Command::AddChannel {
        name: "Probe".into(),
        kind: ChannelKind::Generator,
        plugin: PluginInstance::new("vst3:probe"),
        mixer_insert: Some(1),
    }
    .apply(&mut p)
    .unwrap();
    p.mixer.inserts[1]
        .effects
        .push(EffectSlot::new(PluginInstance::new("vst3:probe-fx")));
    p
}

fn add_tempo_automation(p: &mut Project, bpm: f32) {
    let Outcome::Automation(a) = Command::AddAutomation {
        name: "tempo".into(),
        target: AutomationTarget::Tempo,
        min: 60.0,
        max: 240.0,
        points: vec![AutomationPoint {
            position: 0,
            value: (bpm - 60.0) / 180.0,
            tension: 0.0,
        }],
    }
    .apply(p)
    .unwrap() else {
        panic!()
    };
    Command::PlaceClip {
        track: 1,
        position: 0,
        length: Some(1536),
        source: ClipSource::Automation(a),
    }
    .apply(p)
    .unwrap();
}

#[test]
fn engine_propagates_tempo_to_plugins() {
    let host = Arc::new(ProbeHost::default());
    mfc_plugins::set_external_host(Some(host.clone()));

    // 1) Costruzione del grafo.
    let p = project(100.0);
    let (mut eng, handle) = Engine::new(&p, 48_000.0);
    let gens = last_per_instance(&host.gen_log);
    let fxs = last_per_instance(&host.fx_log);
    assert_eq!(gens.len(), 1, "{gens:?}");
    assert_eq!(fxs.len(), 1, "{fxs:?}");
    assert_eq!(gens[0].1, 100.0);
    assert_eq!(fxs[0].1, 100.0);
    let (gen_id, fx_id) = (gens[0].0, fxs[0].0);

    // 2) Reinstallazione con un altro tempo: le istanze riusate ricevono il nuovo BPM.
    let mut p2 = p.clone();
    Command::SetTempo { bpm: 90.0 }.apply(&mut p2).unwrap();
    assert!(handle.load_project(&p2));
    let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
    eng.process(&mut l, &mut r);
    handle.collect_garbage();
    let gens = last_per_instance(&host.gen_log);
    let fxs = last_per_instance(&host.fx_log);
    assert!(gens.contains(&(gen_id, 90.0)), "{gens:?}");
    assert!(fxs.contains(&(fx_id, 90.0)), "{fxs:?}");

    // 3) Automazione del tempo durante la riproduzione.
    let mut p3 = p2.clone();
    add_tempo_automation(&mut p3, 150.0);
    assert!(handle.load_project(&p3));
    eng.play();
    for _ in 0..4 {
        eng.process(&mut l, &mut r);
    }
    assert!((eng.tempo() - 150.0).abs() < 1e-3, "{}", eng.tempo());
    let gens = last_per_instance(&host.gen_log);
    let fxs = last_per_instance(&host.fx_log);
    let g = gens.iter().find(|(i, _)| *i == gen_id).unwrap().1;
    let f = fxs.iter().find(|(i, _)| *i == fx_id).unwrap().1;
    assert!((g - 150.0).abs() < 1e-3, "{gens:?}");
    assert!((f - 150.0).abs() < 1e-3, "{fxs:?}");
    // Nessuna chiamata ridondante a tempo costante.
    let calls = host.gen_log.lock().unwrap().len();
    for _ in 0..8 {
        eng.process(&mut l, &mut r);
    }
    assert_eq!(host.gen_log.lock().unwrap().len(), calls);

    // 4) Render offline: le nuove istanze ricevono tempo di progetto e automazione.
    host.gen_log.lock().unwrap().clear();
    let opts = RenderOptions {
        sample_rate: 48_000,
        tail_seconds: 0.0,
        max_seconds: 1.0,
        ..Default::default()
    };
    render(&p3, &opts).unwrap();
    let log = host.gen_log.lock().unwrap().clone();
    assert!(log.iter().any(|(_, b)| *b == 90.0), "{log:?}");
    assert!(log.iter().any(|(_, b)| (b - 150.0).abs() < 1e-3), "{log:?}");

    mfc_plugins::set_external_host(None);
}
