//! Modello dati del progetto, ricostruito clean-room sull'architettura di FL Studio:
//! Channel Rack → Pattern (step/piano roll) → Playlist → Mixer (insert + master).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::time::{Tick, TimeBase};

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Identificativo stabile di un canale del Channel Rack.
    ChannelId
);
id_type!(
    /// Identificativo stabile di un pattern.
    PatternId
);
id_type!(
    /// Identificativo stabile di una clip nella Playlist.
    ClipId
);
id_type!(
    /// Identificativo stabile di una clip di automazione.
    AutomationId
);

/// Numero di insert del mixer escluso il master (come FL Studio 20+).
pub const MIXER_INSERTS: usize = 125;
/// Indice dell'insert master.
pub const MASTER: u16 = 0;
/// Slot effetto per insert.
pub const FX_SLOTS: usize = 10;
/// Tracce della Playlist.
pub const PLAYLIST_TRACKS: usize = 500;

/// Posizione del fader di canale che corrisponde a 0 dB (come il default di FL Studio).
pub const CHANNEL_UNITY: f32 = 0.78;
/// Posizione del fader del mixer che corrisponde a 0 dB.
pub const MIXER_UNITY: f32 = 0.8;

/// Guadagno lineare di un fader: `pos / unity` (fader lineare in ampiezza).
pub fn fader_gain(pos: f32, unity: f32) -> f32 {
    (pos / unity).max(0.0)
}

/// Colore RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color(pub u8, pub u8, pub u8);

impl Default for Color {
    fn default() -> Self {
        Color(0x5f, 0x75, 0x81)
    }
}

impl Color {
    pub fn from_u32(rgb: u32) -> Self {
        Color((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }
    pub fn to_u32(self) -> u32 {
        (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32
    }
}

/// Istanza di un plugin (generatore o effetto). I parametri sono valori "reali"
/// nel range dichiarato dal descrittore del plugin (vedi crate `mfc-plugins`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PluginInstance {
    /// Id del plugin, es. `"mfc.3xosc"`, `"mfc.sampler"`, `"mfc.reverb"`.
    pub kind: String,
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    /// Percorso di un campione audio (sampler, audio clip, drum pad).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_path: Option<String>,
    /// Stato opaco preservato (es. blob di un plugin FL sconosciuto importato da .flp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opaque_state: Option<Vec<u8>>,
    /// Nome originale del plugin quando importato e non supportato nativamente.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_name: Option<String>,
}

impl PluginInstance {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ..Default::default()
        }
    }
    pub fn with_param(mut self, name: impl Into<String>, value: f32) -> Self {
        self.params.insert(name.into(), value);
        self
    }
    pub fn param(&self, name: &str) -> Option<f32> {
        self.params.get(name).copied()
    }
}

/// Tipologia di canale nel Channel Rack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// Canale sampler (campione triggerato dalle note).
    Sampler,
    /// Plugin generatore (synth).
    #[default]
    Generator,
    /// Audio clip (campione posizionato in Playlist, non rintonato).
    AudioClip,
    /// Layer: inoltra le note ai canali figli.
    Layer,
    /// Canale che ospita una clip di automazione.
    Automation,
}

/// Canale del Channel Rack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    #[serde(default)]
    pub color: Color,
    #[serde(default)]
    pub kind: ChannelKind,
    pub plugin: PluginInstance,
    /// Posizione fader 0..=1.25; [`CHANNEL_UNITY`] (default) = 0 dB. Vedi [`Channel::gain`].
    pub volume: f32,
    /// Pan -1 (sinistra) ..= 1 (destra).
    pub pan: f32,
    /// Pitch in cent (-4800..=4800).
    #[serde(default)]
    pub pitch_cents: f32,
    #[serde(default)]
    pub muted: bool,
    /// Insert del mixer di destinazione (0 = master).
    #[serde(default)]
    pub mixer_insert: u16,
    /// Nota di riferimento (MIDI). Default 60 = C5 nella nomenclatura FL.
    #[serde(default = "default_root")]
    pub root_note: u8,
    /// Figli (solo per i canali `Layer`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layer_children: Vec<ChannelId>,
}

fn default_root() -> u8 {
    60
}

impl Channel {
    /// Guadagno lineare del fader del canale.
    pub fn gain(&self) -> f32 {
        fader_gain(self.volume, CHANNEL_UNITY)
    }

    pub fn new(id: ChannelId, name: impl Into<String>, kind: ChannelKind, plugin: PluginInstance) -> Self {
        Self {
            id,
            name: name.into(),
            color: Color::default(),
            kind,
            plugin,
            volume: CHANNEL_UNITY,
            pan: 0.0,
            pitch_cents: 0.0,
            muted: false,
            mixer_insert: MASTER,
            root_note: 60,
            layer_children: Vec::new(),
        }
    }
}

/// Nota (anche gli step dello step sequencer sono note di durata 1 step).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub position: Tick,
    pub length: Tick,
    /// Numero MIDI 0..=127.
    pub key: u8,
    /// Velocity 0..=1.
    pub velocity: f32,
    /// Pan della nota -1..=1.
    #[serde(default)]
    pub pan: f32,
    /// Fine pitch in cent (-120..=120).
    #[serde(default)]
    pub fine_pitch: f32,
    /// Release 0..=1.
    #[serde(default = "half")]
    pub release: f32,
    /// Nota slide (glide verso la nota successiva).
    #[serde(default)]
    pub slide: bool,
    /// Controllo modulazione X/Y (0..=1).
    #[serde(default = "half")]
    pub mod_x: f32,
    #[serde(default = "half")]
    pub mod_y: f32,
}

fn half() -> f32 {
    0.5
}

impl Note {
    pub fn new(position: Tick, length: Tick, key: u8, velocity: f32) -> Self {
        Self {
            position,
            length,
            key,
            velocity,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            slide: false,
            mod_x: 0.5,
            mod_y: 0.5,
        }
    }
    pub fn end(&self) -> Tick {
        self.position + self.length
    }
}

/// Pattern: contiene le note di ciascun canale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub id: PatternId,
    pub name: String,
    #[serde(default)]
    pub color: Color,
    /// Lunghezza esplicita; `None` = calcolata dalle note (arrotondata alla battuta).
    #[serde(default)]
    pub length: Option<Tick>,
    #[serde(default)]
    pub notes: BTreeMap<ChannelId, Vec<Note>>,
}

impl Pattern {
    pub fn new(id: PatternId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            color: Color::default(),
            length: None,
            notes: BTreeMap::new(),
        }
    }

    /// Fine dell'ultima nota.
    pub fn content_end(&self) -> Tick {
        self.notes.values().flatten().map(Note::end).max().unwrap_or(0)
    }

    /// Lunghezza effettiva: esplicita o contenuto arrotondato alla battuta (minimo 1 battuta).
    pub fn effective_length(&self, tb: &TimeBase) -> Tick {
        if let Some(l) = self.length {
            return l;
        }
        let bar = tb.ticks_per_bar();
        let end = self.content_end();
        end.div_ceil(bar).max(1) * bar
    }

    /// Mantiene le note ordinate per posizione e tono.
    pub fn sort_notes(&mut self) {
        for v in self.notes.values_mut() {
            v.sort_by(|a, b| a.position.cmp(&b.position).then(a.key.cmp(&b.key)));
        }
    }
}

/// Sorgente di una clip in Playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum ClipSource {
    Pattern(PatternId),
    /// Audio clip: riferimento a un canale `AudioClip`.
    Audio(ChannelId),
    Automation(AutomationId),
}

/// Clip posizionata in Playlist.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlaylistClip {
    pub id: ClipId,
    pub track: u16,
    pub position: Tick,
    pub length: Tick,
    /// Offset di inizio dentro la sorgente.
    #[serde(default)]
    pub offset: Tick,
    pub source: ClipSource,
    #[serde(default)]
    pub muted: bool,
}

impl PlaylistClip {
    pub fn end(&self) -> Tick {
        self.position + self.length
    }
}

/// Traccia della Playlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlaylistTrack {
    pub name: String,
    #[serde(default)]
    pub color: Color,
    #[serde(default)]
    pub muted: bool,
}

/// Playlist (arrangiamento).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Playlist {
    /// Tracce con nome/colore/mute personalizzati (indice → traccia).
    #[serde(default)]
    pub tracks: BTreeMap<u16, PlaylistTrack>,
    #[serde(default)]
    pub clips: Vec<PlaylistClip>,
}

impl Playlist {
    pub fn end(&self) -> Tick {
        self.clips.iter().map(PlaylistClip::end).max().unwrap_or(0)
    }
    pub fn track_muted(&self, track: u16) -> bool {
        self.tracks.get(&track).is_some_and(|t| t.muted)
    }
}

/// Slot effetto di un insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectSlot {
    pub plugin: PluginInstance,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Dry/wet 0..=1.
    #[serde(default = "one")]
    pub mix: f32,
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}

impl EffectSlot {
    pub fn new(plugin: PluginInstance) -> Self {
        Self {
            plugin,
            enabled: true,
            mix: 1.0,
        }
    }
}

/// Mandata da un insert a un altro.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub target: u16,
    /// Guadagno lineare della mandata.
    pub gain: f32,
}

/// Insert del mixer (indice 0 = master).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixerInsert {
    pub name: String,
    #[serde(default)]
    pub color: Color,
    /// Posizione fader 0..=1.25; [`MIXER_UNITY`] (default) = 0 dB. Vedi [`MixerInsert::gain`].
    pub volume: f32,
    pub pan: f32,
    /// Separazione stereo -1 (mono) ..= 1 (allargato).
    #[serde(default)]
    pub stereo_separation: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub phase_invert: bool,
    #[serde(default)]
    pub swap_lr: bool,
    #[serde(default)]
    pub effects: Vec<EffectSlot>,
    /// Instradamenti in uscita. Per default ogni insert va al master.
    #[serde(default)]
    pub routes: Vec<Route>,
}

impl MixerInsert {
    /// Guadagno lineare del fader dell'insert.
    pub fn gain(&self) -> f32 {
        fader_gain(self.volume, MIXER_UNITY)
    }

    pub fn new(index: u16) -> Self {
        let name = if index == MASTER {
            "Master".to_string()
        } else {
            format!("Insert {index}")
        };
        let routes = if index == MASTER {
            vec![]
        } else {
            vec![Route {
                target: MASTER,
                gain: 1.0,
            }]
        };
        Self {
            name,
            color: Color::default(),
            volume: MIXER_UNITY,
            pan: 0.0,
            stereo_separation: 0.0,
            muted: false,
            solo: false,
            phase_invert: false,
            swap_lr: false,
            effects: Vec::new(),
            routes,
        }
    }
}

/// Mixer: master + 125 insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mixer {
    pub inserts: Vec<MixerInsert>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            inserts: (0..=MIXER_INSERTS as u16).map(MixerInsert::new).collect(),
        }
    }
}

/// Bersaglio di un'automazione.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationTarget {
    MasterVolume,
    Tempo,
    ChannelVolume { channel: ChannelId },
    ChannelPan { channel: ChannelId },
    ChannelPitch { channel: ChannelId },
    ChannelParam { channel: ChannelId, param: String },
    MixerVolume { insert: u16 },
    MixerPan { insert: u16 },
    EffectParam { insert: u16, slot: u16, param: String },
    EffectMix { insert: u16, slot: u16 },
}

/// Punto di automazione: `value` è normalizzato 0..=1 e mappato sul range del bersaglio.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AutomationPoint {
    pub position: Tick,
    pub value: f32,
    /// Tensione della curva verso il punto successivo (-1..=1, 0 = lineare).
    #[serde(default)]
    pub tension: f32,
}

/// Clip di automazione.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationClip {
    pub id: AutomationId,
    pub name: String,
    pub target: AutomationTarget,
    /// Valore minimo/massimo reale mappato su 0..=1.
    pub min: f32,
    pub max: f32,
    pub points: Vec<AutomationPoint>,
}

impl AutomationClip {
    /// Valore normalizzato a una posizione relativa all'inizio della clip.
    pub fn value_at(&self, pos: f64) -> Option<f32> {
        let pts = &self.points;
        let first = pts.first()?;
        if pos <= first.position as f64 {
            return Some(first.value);
        }
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if pos <= b.position as f64 {
                let span = (b.position - a.position) as f64;
                if span <= 0.0 {
                    return Some(b.value);
                }
                let t = ((pos - a.position as f64) / span) as f32;
                return Some(a.value + (b.value - a.value) * apply_tension(t, a.tension));
            }
        }
        pts.last().map(|p| p.value)
    }

    /// Valore reale (mappato su min..max).
    pub fn real_value_at(&self, pos: f64) -> Option<f32> {
        self.value_at(pos).map(|v| self.min + (self.max - self.min) * v)
    }

    pub fn length(&self) -> Tick {
        self.points.last().map(|p| p.position).unwrap_or(0)
    }
}

/// Curva con tensione stile FL: t^(e^(k*tension)).
pub fn apply_tension(t: f32, tension: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if tension.abs() < 1e-4 {
        return t;
    }
    t.powf((tension * 3.0).exp())
}

/// Metadati del progetto (Project info).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProjectInfo {
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub comments: String,
    #[serde(default)]
    pub url: String,
    /// Versione di FL Studio con cui è stato salvato (se importato da .flp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fl_version: Option<String>,
}

/// Vista principale salvata dalla GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiView {
    #[default]
    ChannelRack,
    PianoRoll,
    Playlist,
    Mixer,
}

/// Stato dell'interfaccia salvato nel progetto, ripristinato all'apertura nella GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UiState {
    pub view: UiView,
    /// Modalità Song (`true`) o Pattern.
    pub song_mode: bool,
}

/// Progetto completo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Versione del formato `.mfc`.
    pub format_version: u32,
    pub info: ProjectInfo,
    /// Tempo in BPM.
    pub tempo: f64,
    pub time: TimeBase,
    /// Fader master 0..=1.25 ([`MIXER_UNITY`] = 0 dB).
    pub master_volume: f32,
    /// Pitch master in cent.
    #[serde(default)]
    pub master_pitch: f32,
    /// Swing globale 0..=1.
    #[serde(default)]
    pub swing: f32,
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    pub playlist: Playlist,
    pub mixer: Mixer,
    #[serde(default)]
    pub automations: Vec<AutomationClip>,
    /// Pattern selezionato (per la modalità "pattern").
    #[serde(default)]
    pub current_pattern: Option<PatternId>,
    /// Contatore per generare id univoci.
    pub next_id: u32,
    /// Vista e modalità dell'ultima sessione GUI (assente nei progetti creati da CLI/MCP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiState>,
}

pub const FORMAT_VERSION: u32 = 1;

impl Default for Project {
    fn default() -> Self {
        Self::new("Untitled")
    }
}

impl Project {
    /// Progetto vuoto con un pattern iniziale (come FL Studio).
    pub fn new(title: impl Into<String>) -> Self {
        let mut p = Self {
            format_version: FORMAT_VERSION,
            info: ProjectInfo {
                title: title.into(),
                ..Default::default()
            },
            tempo: 140.0,
            time: TimeBase::default(),
            master_volume: MIXER_UNITY,
            master_pitch: 0.0,
            swing: 0.0,
            channels: Vec::new(),
            patterns: Vec::new(),
            playlist: Playlist::default(),
            mixer: Mixer::default(),
            automations: Vec::new(),
            current_pattern: None,
            next_id: 1,
            ui: None,
        };
        let pid = PatternId(p.alloc_id());
        p.patterns.push(Pattern::new(pid, "Pattern 1"));
        p.current_pattern = Some(pid);
        p
    }

    pub fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn channel(&self, id: ChannelId) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }
    pub fn channel_mut(&mut self, id: ChannelId) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }
    pub fn pattern(&self, id: PatternId) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }
    pub fn pattern_mut(&mut self, id: PatternId) -> Option<&mut Pattern> {
        self.patterns.iter_mut().find(|p| p.id == id)
    }
    pub fn automation(&self, id: AutomationId) -> Option<&AutomationClip> {
        self.automations.iter().find(|a| a.id == id)
    }
    pub fn automation_mut(&mut self, id: AutomationId) -> Option<&mut AutomationClip> {
        self.automations.iter_mut().find(|a| a.id == id)
    }
    pub fn insert(&self, index: u16) -> Option<&MixerInsert> {
        self.mixer.inserts.get(index as usize)
    }
    pub fn insert_mut(&mut self, index: u16) -> Option<&mut MixerInsert> {
        self.mixer.inserts.get_mut(index as usize)
    }

    /// Fine del brano (ultima clip della Playlist).
    pub fn song_length(&self) -> Tick {
        self.playlist.end()
    }

    /// Lunghezza in secondi a tempo costante.
    pub fn song_seconds(&self) -> f64 {
        self.time.ticks_to_seconds(self.song_length() as f64, self.tempo)
    }

    /// Serializza nel formato nativo `.mfc` (JSON).
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Legge il formato nativo `.mfc` (JSON).
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_project_defaults() {
        let p = Project::new("Demo");
        assert_eq!(p.patterns.len(), 1);
        assert_eq!(p.mixer.inserts.len(), MIXER_INSERTS + 1);
        assert_eq!(p.mixer.inserts[0].routes.len(), 0);
        assert_eq!(p.mixer.inserts[5].routes[0].target, MASTER);
        assert_eq!(p.song_length(), 0);
    }

    #[test]
    fn pattern_length() {
        let tb = TimeBase::default();
        let mut pat = Pattern::new(PatternId(1), "p");
        assert_eq!(pat.effective_length(&tb), 384);
        pat.notes
            .entry(ChannelId(1))
            .or_default()
            .push(Note::new(380, 10, 60, 1.0));
        assert_eq!(pat.effective_length(&tb), 768);
        pat.length = Some(100);
        assert_eq!(pat.effective_length(&tb), 100);
    }

    #[test]
    fn automation_interp() {
        let a = AutomationClip {
            id: AutomationId(1),
            name: "a".into(),
            target: AutomationTarget::MasterVolume,
            min: 0.0,
            max: 2.0,
            points: vec![
                AutomationPoint {
                    position: 0,
                    value: 0.0,
                    tension: 0.0,
                },
                AutomationPoint {
                    position: 100,
                    value: 1.0,
                    tension: 0.0,
                },
            ],
        };
        assert_eq!(a.value_at(-5.0), Some(0.0));
        assert!((a.value_at(50.0).unwrap() - 0.5).abs() < 1e-6);
        assert!((a.real_value_at(50.0).unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(a.value_at(500.0), Some(1.0));
    }

    #[test]
    fn json_roundtrip() {
        let mut p = Project::new("RT");
        let id = ChannelId(p.alloc_id());
        p.channels.push(Channel::new(
            id,
            "Kick",
            ChannelKind::Generator,
            PluginInstance::new("mfc.kick"),
        ));
        let s = p.to_json().unwrap();
        assert_eq!(Project::from_json(&s).unwrap(), p);
    }
}
