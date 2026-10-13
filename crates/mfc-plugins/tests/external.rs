//! L'host esterno registrato riceve i kind vst3:/clap:; senza host restano muti.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mfc_core::PluginInstance;
use mfc_plugins::*;

struct FakeHost(AtomicUsize);

impl ExternalHost for FakeHost {
    fn create_generator(&self, _inst: &PluginInstance, sample_rate: f32) -> Option<Box<dyn Generator>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        // Un generatore nativo qualsiasi fa da segnaposto.
        let mut g = create_generator(&default_instance(ids::KICK).unwrap(), sample_rate);
        g.set_sample_rate(sample_rate);
        Some(g)
    }
    fn create_effect(&self, _inst: &PluginInstance, sample_rate: f32) -> Option<Box<dyn Effect>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        create_effect(&default_instance(ids::GAIN).unwrap(), sample_rate)
    }
}

#[test]
fn external_kinds_are_delegated() {
    // Senza host: generatore muto, effetto assente.
    set_external_host(None);
    let ext = PluginInstance::new("vst3:/x/Synth.vst3::ABCD");
    let mut g = create_generator(&ext, 48_000.0);
    g.note_on(NoteOn::simple(1, 60, 1.0));
    assert_eq!(g.active_voices(), 0);
    assert!(create_effect(&PluginInstance::new("clap:/x/fx.clap::id"), 48_000.0).is_none());

    let host = Arc::new(FakeHost(AtomicUsize::new(0)));
    set_external_host(Some(host.clone()));
    let mut g = create_generator(&ext, 48_000.0);
    g.note_on(NoteOn::simple(1, 60, 1.0));
    assert!(g.active_voices() > 0);
    assert!(create_effect(&PluginInstance::new("clap:/x/fx.clap::id"), 48_000.0).is_some());
    // I plugin nativi non passano dall'host.
    create_generator(&default_instance(ids::OSC3).unwrap(), 48_000.0);
    assert_eq!(host.0.load(Ordering::SeqCst), 2);
    set_external_host(None);
}
