//! Generatore silenzioso: segnaposto per plugin sconosciuti o esterni.

use crate::api::{Generator, NoteOn, PluginDescriptor, PluginRole};
use crate::ids;

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::SILENT,
    name: "Silent",
    role: PluginRole::Generator,
    description: "Nessun suono: segnaposto per plugin non supportati (lo stato originale è preservato).",
    params: &[],
};

/// Non produce audio e non ha voci.
#[derive(Debug, Default)]
pub struct Silent;

impl Silent {
    pub fn new() -> Self {
        Self
    }
}

impl Generator for Silent {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
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
}
