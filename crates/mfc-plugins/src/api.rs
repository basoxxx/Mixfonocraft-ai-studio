//! Contratto dei plugin: descrittori, generatori, effetti.
//!
//! Regole real-time: `process` non deve allocare né bloccare. Le allocazioni
//! (buffer, delay line, caricamento campioni) avvengono in costruzione o in
//! `set_sample_rate`.

use std::sync::Arc;

/// Ruolo del plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRole {
    Generator,
    Effect,
}

/// Descrittore di un parametro. I valori sono "reali" (non normalizzati).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDesc {
    /// Chiave stabile usata in `PluginInstance::params` (snake_case).
    pub name: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Unità di misura ("Hz", "dB", "ms", "%", "st", "" ...).
    pub unit: &'static str,
    /// Parametro a scelta discreta (valori interi `min..=max`); `choices[i]` etichetta il valore `min+i`.
    pub choices: &'static [&'static str],
}

impl ParamDesc {
    pub const fn new(name: &'static str, label: &'static str, min: f32, max: f32, default: f32, unit: &'static str) -> Self {
        Self { name, label, min, max, default, unit, choices: &[] }
    }
    pub const fn choice(name: &'static str, label: &'static str, choices: &'static [&'static str], default: usize) -> Self {
        Self { name, label, min: 0.0, max: (choices.len() - 1) as f32, default: default as f32, unit: "", choices }
    }
    /// Converte un valore reale in 0..=1.
    pub fn normalize(&self, v: f32) -> f32 {
        if self.max == self.min { 0.0 } else { ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0) }
    }
    /// Converte 0..=1 in valore reale.
    pub fn denormalize(&self, n: f32) -> f32 {
        let v = self.min + (self.max - self.min) * n.clamp(0.0, 1.0);
        if self.choices.is_empty() { v } else { v.round() }
    }
    pub fn clamp(&self, v: f32) -> f32 {
        v.clamp(self.min.min(self.max), self.max.max(self.min))
    }
}

/// Descrittore statico di un plugin.
#[derive(Debug, Clone, Copy)]
pub struct PluginDescriptor {
    /// Id stabile, es. `"mfc.3xosc"`.
    pub id: &'static str,
    pub name: &'static str,
    pub role: PluginRole,
    pub description: &'static str,
    pub params: &'static [ParamDesc],
}

impl PluginDescriptor {
    pub fn param(&self, name: &str) -> Option<&'static ParamDesc> {
        self.params.iter().find(|p| p.name == name)
    }
}

/// Evento di nota per un generatore.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteOn {
    /// Id univoco della voce (usato da `note_off`).
    pub voice_id: u64,
    pub key: u8,
    /// 0..=1
    pub velocity: f32,
    /// -1..=1
    pub pan: f32,
    /// Microtuning della nota in cent.
    pub fine_pitch_cents: f32,
    /// Nota di riferimento del canale (il campione suona all'altezza originale su questa nota).
    pub root_key: u8,
    pub mod_x: f32,
    pub mod_y: f32,
    /// Glide dalla nota precedente.
    pub slide: bool,
    /// Sample da saltare all'inizio (audio clip avviate a metà).
    pub start_offset_samples: u64,
}

impl NoteOn {
    pub fn simple(voice_id: u64, key: u8, velocity: f32) -> Self {
        Self {
            voice_id,
            key,
            velocity,
            pan: 0.0,
            fine_pitch_cents: 0.0,
            root_key: 60,
            mod_x: 0.5,
            mod_y: 0.5,
            slide: false,
            start_offset_samples: 0,
        }
    }
}

/// Strumento (canale del Channel Rack).
pub trait Generator: Send {
    fn descriptor(&self) -> &'static PluginDescriptor;
    fn set_sample_rate(&mut self, sample_rate: f32);
    /// Imposta un parametro (valore reale). Nomi sconosciuti vengono ignorati.
    fn set_param(&mut self, name: &str, value: f32);
    fn note_on(&mut self, note: NoteOn);
    fn note_off(&mut self, voice_id: u64);
    fn all_notes_off(&mut self);
    /// Pitch del canale (cent) applicato a tutte le voci.
    fn set_pitch_offset(&mut self, _cents: f32) {}
    /// SOMMA il segnale nei buffer (non sovrascrive). `left.len() == right.len()`.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Numero di voci attive (0 = silenzioso, il motore può saltare il processing).
    fn active_voices(&self) -> usize;
}

/// Effetto (slot dell'insert del mixer).
pub trait Effect: Send {
    fn descriptor(&self) -> &'static PluginDescriptor;
    fn set_sample_rate(&mut self, sample_rate: f32);
    fn set_param(&mut self, name: &str, value: f32);
    /// Azzera lo stato interno (code di riverbero, delay...).
    fn reset(&mut self);
    /// Elabora in-place. `left.len() == right.len()`.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Latenza introdotta (sample).
    fn latency(&self) -> usize {
        0
    }
}

/// Audio campionato condiviso tra istanze.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleData {
    pub sample_rate: f32,
    /// Un vettore per canale (1 = mono, 2 = stereo).
    pub channels: Vec<Vec<f32>>,
}

impl SampleData {
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub type SharedSample = Arc<SampleData>;
