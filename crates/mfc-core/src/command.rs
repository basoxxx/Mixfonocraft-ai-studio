//! Comandi di modifica del progetto, condivisi da GUI, CLI e server MCP.
//! Ogni modifica passa da qui → una sola fonte di verità e undo/redo uniforme.

use serde::{Deserialize, Serialize};

use crate::project::*;
use crate::time::{Tick, TimeBase, TimeSignature};
use crate::CoreError;

/// Risultato di un comando (id creati).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    None,
    Channel(ChannelId),
    Pattern(PatternId),
    Clip(ClipId),
    Automation(AutomationId),
    Count(usize),
    Many(Vec<Outcome>),
}

/// Selettore di note per rimozione/modifica.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NoteFilter {
    /// Solo note che iniziano in `[from, to)`.
    #[serde(default)]
    pub from: Option<Tick>,
    #[serde(default)]
    pub to: Option<Tick>,
    #[serde(default)]
    pub key: Option<u8>,
}

impl NoteFilter {
    pub fn matches(&self, n: &Note) -> bool {
        self.from.is_none_or(|f| n.position >= f)
            && self.to.is_none_or(|t| n.position < t)
            && self.key.is_none_or(|k| n.key == k)
    }
}

/// Tutte le operazioni di editing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    // ---- Progetto ----
    SetTempo {
        bpm: f64,
    },
    SetTimeSignature {
        numerator: u8,
        denominator: u8,
    },
    SetMasterVolume {
        volume: f32,
    },
    SetMasterPitch {
        cents: f32,
    },
    SetSwing {
        swing: f32,
    },
    SetInfo {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        artist: Option<String>,
        #[serde(default)]
        genre: Option<String>,
        #[serde(default)]
        comments: Option<String>,
    },

    // ---- Channel Rack ----
    AddChannel {
        name: String,
        #[serde(default)]
        kind: ChannelKind,
        plugin: PluginInstance,
        #[serde(default)]
        mixer_insert: Option<u16>,
    },
    RemoveChannel {
        channel: ChannelId,
    },
    CloneChannel {
        channel: ChannelId,
    },
    SetChannel {
        channel: ChannelId,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        volume: Option<f32>,
        #[serde(default)]
        pan: Option<f32>,
        #[serde(default)]
        pitch_cents: Option<f32>,
        #[serde(default)]
        muted: Option<bool>,
        #[serde(default)]
        mixer_insert: Option<u16>,
        #[serde(default)]
        root_note: Option<u8>,
        #[serde(default)]
        color: Option<Color>,
    },
    SetChannelParam {
        channel: ChannelId,
        param: String,
        value: f32,
    },
    SetChannelSample {
        channel: ChannelId,
        path: Option<String>,
    },
    MoveChannel {
        channel: ChannelId,
        index: usize,
    },

    // ---- Pattern ----
    AddPattern {
        name: String,
    },
    RemovePattern {
        pattern: PatternId,
    },
    ClonePattern {
        pattern: PatternId,
    },
    SetPattern {
        pattern: PatternId,
        #[serde(default)]
        name: Option<String>,
        /// `Some(None)` azzera la lunghezza esplicita.
        #[serde(default)]
        length: Option<Option<Tick>>,
        #[serde(default)]
        color: Option<Color>,
    },
    SelectPattern {
        pattern: PatternId,
    },

    // ---- Step sequencer ----
    /// Attiva/disattiva lo step `step` (1/16) del canale nel pattern.
    SetStep {
        pattern: PatternId,
        channel: ChannelId,
        step: u32,
        on: bool,
        #[serde(default)]
        velocity: Option<f32>,
    },
    /// Imposta un'intera riga di step da stringa, es. `"x...x...x...x..."`.
    SetStepRow {
        pattern: PatternId,
        channel: ChannelId,
        row: String,
    },

    // ---- Piano roll ----
    AddNotes {
        pattern: PatternId,
        channel: ChannelId,
        notes: Vec<Note>,
    },
    RemoveNotes {
        pattern: PatternId,
        channel: ChannelId,
        #[serde(default)]
        filter: NoteFilter,
    },
    QuantizeNotes {
        pattern: PatternId,
        #[serde(default)]
        channel: Option<ChannelId>,
        grid: Tick,
        #[serde(default)]
        quantize_length: bool,
    },
    TransposeNotes {
        pattern: PatternId,
        #[serde(default)]
        channel: Option<ChannelId>,
        semitones: i32,
    },
    SetNoteVelocity {
        pattern: PatternId,
        channel: ChannelId,
        #[serde(default)]
        filter: NoteFilter,
        velocity: f32,
    },

    // ---- Playlist ----
    PlaceClip {
        track: u16,
        position: Tick,
        #[serde(default)]
        length: Option<Tick>,
        source: ClipSource,
    },
    RemoveClip {
        clip: ClipId,
    },
    MoveClip {
        clip: ClipId,
        #[serde(default)]
        track: Option<u16>,
        #[serde(default)]
        position: Option<Tick>,
        #[serde(default)]
        length: Option<Tick>,
        #[serde(default)]
        offset: Option<Tick>,
    },
    SetClipMuted {
        clip: ClipId,
        muted: bool,
    },
    SetPlaylistTrack {
        track: u16,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        muted: Option<bool>,
        #[serde(default)]
        color: Option<Color>,
    },

    // ---- Mixer ----
    SetInsert {
        insert: u16,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        volume: Option<f32>,
        #[serde(default)]
        pan: Option<f32>,
        #[serde(default)]
        stereo_separation: Option<f32>,
        #[serde(default)]
        muted: Option<bool>,
        #[serde(default)]
        solo: Option<bool>,
        #[serde(default)]
        phase_invert: Option<bool>,
        #[serde(default)]
        swap_lr: Option<bool>,
    },
    /// Crea/aggiorna (gain `Some`) o rimuove (gain `None`) una mandata.
    SetRoute {
        from: u16,
        to: u16,
        gain: Option<f32>,
    },
    AddEffect {
        insert: u16,
        plugin: PluginInstance,
        #[serde(default)]
        slot: Option<u16>,
    },
    RemoveEffect {
        insert: u16,
        slot: u16,
    },
    SetEffect {
        insert: u16,
        slot: u16,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        mix: Option<f32>,
    },
    SetEffectParam {
        insert: u16,
        slot: u16,
        param: String,
        value: f32,
    },

    // ---- Automazione ----
    AddAutomation {
        name: String,
        target: AutomationTarget,
        min: f32,
        max: f32,
        points: Vec<AutomationPoint>,
    },
    SetAutomationPoints {
        automation: AutomationId,
        points: Vec<AutomationPoint>,
    },
    RemoveAutomation {
        automation: AutomationId,
    },

    /// Più comandi atomici (un solo passo di undo).
    Batch {
        commands: Vec<Command>,
    },
}

fn check_insert(p: &Project, i: u16) -> Result<(), CoreError> {
    if (i as usize) < p.mixer.inserts.len() {
        Ok(())
    } else {
        Err(CoreError::NotFound(format!("mixer insert {i}")))
    }
}

fn check_range(name: &str, v: f32, lo: f32, hi: f32) -> Result<f32, CoreError> {
    if v.is_finite() && v >= lo && v <= hi {
        Ok(v)
    } else {
        Err(CoreError::Invalid(format!("{name}={v} fuori range {lo}..={hi}")))
    }
}

fn pat(p: &mut Project, id: PatternId) -> Result<&mut Pattern, CoreError> {
    p.pattern_mut(id)
        .ok_or_else(|| CoreError::NotFound(format!("pattern {id}")))
}

fn chan_exists(p: &Project, id: ChannelId) -> Result<(), CoreError> {
    p.channel(id)
        .map(|_| ())
        .ok_or_else(|| CoreError::NotFound(format!("channel {id}")))
}

impl Command {
    /// Breve descrizione per la cronologia di undo.
    pub fn label(&self) -> String {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("op").and_then(|o| o.as_str().map(String::from)));
        s.unwrap_or_else(|| "edit".into())
    }

    /// Applica il comando. In caso d'errore il progetto può essere parzialmente modificato:
    /// usare [`crate::History::execute`] per atomicità.
    pub fn apply(&self, p: &mut Project) -> Result<Outcome, CoreError> {
        use Command::*;
        match self {
            SetTempo { bpm } => {
                if !(10.0..=999.0).contains(bpm) {
                    return Err(CoreError::Invalid(format!("tempo {bpm} fuori range 10..=999")));
                }
                p.tempo = *bpm;
            }
            SetTimeSignature { numerator, denominator } => {
                if *numerator == 0 || !matches!(denominator, 1 | 2 | 4 | 8 | 16) {
                    return Err(CoreError::Invalid("metrica non valida".into()));
                }
                p.time.signature = TimeSignature {
                    numerator: *numerator,
                    denominator: *denominator,
                };
            }
            SetMasterVolume { volume } => p.master_volume = check_range("volume", *volume, 0.0, 1.25)?,
            SetMasterPitch { cents } => p.master_pitch = check_range("pitch", *cents, -1200.0, 1200.0)?,
            SetSwing { swing } => p.swing = check_range("swing", *swing, 0.0, 1.0)?,
            SetInfo {
                title,
                artist,
                genre,
                comments,
            } => {
                if let Some(v) = title {
                    p.info.title = v.clone();
                }
                if let Some(v) = artist {
                    p.info.artist = v.clone();
                }
                if let Some(v) = genre {
                    p.info.genre = v.clone();
                }
                if let Some(v) = comments {
                    p.info.comments = v.clone();
                }
            }

            AddChannel {
                name,
                kind,
                plugin,
                mixer_insert,
            } => {
                let id = ChannelId(p.alloc_id());
                let mut plugin = plugin.clone();
                assign_instance_id(p, &mut plugin, false);
                let mut ch = Channel::new(id, name.clone(), *kind, plugin);
                if let Some(i) = mixer_insert {
                    check_insert(p, *i)?;
                    ch.mixer_insert = *i;
                }
                p.channels.push(ch);
                return Ok(Outcome::Channel(id));
            }
            RemoveChannel { channel } => {
                chan_exists(p, *channel)?;
                p.channels.retain(|c| c.id != *channel);
                for c in &mut p.channels {
                    c.layer_children.retain(|x| x != channel);
                }
                for pt in &mut p.patterns {
                    pt.notes.remove(channel);
                }
                p.playlist.clips.retain(|c| c.source != ClipSource::Audio(*channel));
            }
            CloneChannel { channel } => {
                let src = p
                    .channel(*channel)
                    .cloned()
                    .ok_or_else(|| CoreError::NotFound(format!("channel {channel}")))?;
                let id = ChannelId(p.alloc_id());
                let mut ch = src;
                ch.id = id;
                ch.name = format!("{} #2", ch.name);
                assign_instance_id(p, &mut ch.plugin, true);
                let idx = p.channels.iter().position(|c| c.id == *channel).unwrap();
                p.channels.insert(idx + 1, ch);
                return Ok(Outcome::Channel(id));
            }
            SetChannel {
                channel,
                name,
                volume,
                pan,
                pitch_cents,
                muted,
                mixer_insert,
                root_note,
                color,
            } => {
                if let Some(i) = mixer_insert {
                    check_insert(p, *i)?;
                }
                let vol = volume.map(|v| check_range("volume", v, 0.0, 1.25)).transpose()?;
                let pan = pan.map(|v| check_range("pan", v, -1.0, 1.0)).transpose()?;
                let pitch = pitch_cents
                    .map(|v| check_range("pitch", v, -4800.0, 4800.0))
                    .transpose()?;
                if root_note.is_some_and(|r| r > 127) {
                    return Err(CoreError::Invalid("root_note > 127".into()));
                }
                let ch = p
                    .channel_mut(*channel)
                    .ok_or_else(|| CoreError::NotFound(format!("channel {channel}")))?;
                if let Some(v) = name {
                    ch.name = v.clone();
                }
                if let Some(v) = vol {
                    ch.volume = v;
                }
                if let Some(v) = pan {
                    ch.pan = v;
                }
                if let Some(v) = pitch {
                    ch.pitch_cents = v;
                }
                if let Some(v) = muted {
                    ch.muted = *v;
                }
                if let Some(v) = mixer_insert {
                    ch.mixer_insert = *v;
                }
                if let Some(v) = root_note {
                    ch.root_note = *v;
                }
                if let Some(v) = color {
                    ch.color = *v;
                }
            }
            SetChannelParam { channel, param, value } => {
                if !value.is_finite() {
                    return Err(CoreError::Invalid("valore non finito".into()));
                }
                let ch = p
                    .channel_mut(*channel)
                    .ok_or_else(|| CoreError::NotFound(format!("channel {channel}")))?;
                ch.plugin.params.insert(param.clone(), *value);
            }
            SetChannelSample { channel, path } => {
                let ch = p
                    .channel_mut(*channel)
                    .ok_or_else(|| CoreError::NotFound(format!("channel {channel}")))?;
                ch.plugin.sample_path = path.clone();
            }
            MoveChannel { channel, index } => {
                let from = p
                    .channels
                    .iter()
                    .position(|c| c.id == *channel)
                    .ok_or_else(|| CoreError::NotFound(format!("channel {channel}")))?;
                let ch = p.channels.remove(from);
                let idx = (*index).min(p.channels.len());
                p.channels.insert(idx, ch);
            }

            AddPattern { name } => {
                let id = PatternId(p.alloc_id());
                p.patterns.push(Pattern::new(id, name.clone()));
                return Ok(Outcome::Pattern(id));
            }
            RemovePattern { pattern } => {
                if p.pattern(*pattern).is_none() {
                    return Err(CoreError::NotFound(format!("pattern {pattern}")));
                }
                p.patterns.retain(|x| x.id != *pattern);
                p.playlist.clips.retain(|c| c.source != ClipSource::Pattern(*pattern));
                if p.current_pattern == Some(*pattern) {
                    p.current_pattern = p.patterns.first().map(|x| x.id);
                }
            }
            ClonePattern { pattern } => {
                let mut np = p
                    .pattern(*pattern)
                    .cloned()
                    .ok_or_else(|| CoreError::NotFound(format!("pattern {pattern}")))?;
                let id = PatternId(p.alloc_id());
                np.id = id;
                np.name = format!("{} #2", np.name);
                p.patterns.push(np);
                return Ok(Outcome::Pattern(id));
            }
            SetPattern {
                pattern,
                name,
                length,
                color,
            } => {
                let pt = pat(p, *pattern)?;
                if let Some(v) = name {
                    pt.name = v.clone();
                }
                if let Some(v) = length {
                    pt.length = *v;
                }
                if let Some(v) = color {
                    pt.color = *v;
                }
            }
            SelectPattern { pattern } => {
                pat(p, *pattern)?;
                p.current_pattern = Some(*pattern);
            }

            SetStep {
                pattern,
                channel,
                step,
                on,
                velocity,
            } => {
                chan_exists(p, *channel)?;
                let tb = p.time;
                let root = p.channel(*channel).map(|c| c.root_note).unwrap_or(60);
                let pt = pat(p, *pattern)?;
                set_step(pt, &tb, *channel, root, *step, *on, velocity.unwrap_or(0.78));
            }
            SetStepRow { pattern, channel, row } => {
                chan_exists(p, *channel)?;
                let tb = p.time;
                let root = p.channel(*channel).map(|c| c.root_note).unwrap_or(60);
                let pt = pat(p, *pattern)?;
                let steps: Vec<char> = row.chars().filter(|c| !c.is_whitespace() && *c != '|').collect();
                let step_len = tb.ticks_per_step();
                let span = steps.len() as Tick * step_len;
                // Sostituisce solo le note dentro la riga.
                let notes = pt.notes.entry(*channel).or_default();
                notes.retain(|n| n.position >= span);
                for (i, c) in steps.iter().enumerate() {
                    let vel = match c {
                        'x' | 'X' | '1' | '#' => Some(if *c == 'X' { 1.0 } else { 0.78 }),
                        'o' => Some(0.5),
                        _ => None,
                    };
                    if let Some(v) = vel {
                        notes.push(Note::new(i as Tick * step_len, step_len, root, v));
                    }
                }
                pt.sort_notes();
                return Ok(Outcome::Count(steps.len()));
            }

            AddNotes {
                pattern,
                channel,
                notes,
            } => {
                chan_exists(p, *channel)?;
                for n in notes {
                    if n.key > 127 || n.length == 0 || !(0.0..=1.0).contains(&n.velocity) {
                        return Err(CoreError::Invalid(format!("nota non valida: {n:?}")));
                    }
                }
                let pt = pat(p, *pattern)?;
                pt.notes.entry(*channel).or_default().extend_from_slice(notes);
                pt.sort_notes();
                return Ok(Outcome::Count(notes.len()));
            }
            RemoveNotes {
                pattern,
                channel,
                filter,
            } => {
                let pt = pat(p, *pattern)?;
                let mut removed = 0;
                if let Some(v) = pt.notes.get_mut(channel) {
                    let before = v.len();
                    v.retain(|n| !filter.matches(n));
                    removed = before - v.len();
                    if v.is_empty() {
                        pt.notes.remove(channel);
                    }
                }
                return Ok(Outcome::Count(removed));
            }
            QuantizeNotes {
                pattern,
                channel,
                grid,
                quantize_length,
            } => {
                let pt = pat(p, *pattern)?;
                for (cid, v) in pt.notes.iter_mut() {
                    if channel.is_some_and(|c| c != *cid) {
                        continue;
                    }
                    for n in v.iter_mut() {
                        n.position = TimeBase::quantize(n.position, *grid);
                        if *quantize_length {
                            n.length = TimeBase::quantize(n.length, *grid).max((*grid).max(1));
                        }
                    }
                }
                pt.sort_notes();
            }
            TransposeNotes {
                pattern,
                channel,
                semitones,
            } => {
                let pt = pat(p, *pattern)?;
                for (cid, v) in pt.notes.iter_mut() {
                    if channel.is_some_and(|c| c != *cid) {
                        continue;
                    }
                    for n in v.iter_mut() {
                        n.key = (n.key as i32 + semitones).clamp(0, 127) as u8;
                    }
                }
            }
            SetNoteVelocity {
                pattern,
                channel,
                filter,
                velocity,
            } => {
                let vel = check_range("velocity", *velocity, 0.0, 1.0)?;
                let pt = pat(p, *pattern)?;
                let mut count = 0;
                if let Some(v) = pt.notes.get_mut(channel) {
                    for n in v.iter_mut().filter(|n| filter.matches(n)) {
                        n.velocity = vel;
                        count += 1;
                    }
                }
                return Ok(Outcome::Count(count));
            }

            PlaceClip {
                track,
                position,
                length,
                source,
            } => {
                if *track as usize >= PLAYLIST_TRACKS {
                    return Err(CoreError::Invalid(format!("traccia {track} >= {PLAYLIST_TRACKS}")));
                }
                let default_len = match source {
                    ClipSource::Pattern(pid) => p
                        .pattern(*pid)
                        .map(|x| x.effective_length(&p.time))
                        .ok_or_else(|| CoreError::NotFound(format!("pattern {pid}")))?,
                    ClipSource::Audio(cid) => {
                        chan_exists(p, *cid)?;
                        p.time.ticks_per_bar()
                    }
                    ClipSource::Automation(aid) => p
                        .automation(*aid)
                        .map(|a| a.length().max(1))
                        .ok_or_else(|| CoreError::NotFound(format!("automation {aid}")))?,
                };
                let id = ClipId(p.alloc_id());
                p.playlist.clips.push(PlaylistClip {
                    id,
                    track: *track,
                    position: *position,
                    length: length.unwrap_or(default_len).max(1),
                    offset: 0,
                    source: *source,
                    muted: false,
                });
                return Ok(Outcome::Clip(id));
            }
            RemoveClip { clip } => {
                let before = p.playlist.clips.len();
                p.playlist.clips.retain(|c| c.id != *clip);
                if before == p.playlist.clips.len() {
                    return Err(CoreError::NotFound(format!("clip {clip}")));
                }
            }
            MoveClip {
                clip,
                track,
                position,
                length,
                offset,
            } => {
                if track.is_some_and(|t| t as usize >= PLAYLIST_TRACKS) {
                    return Err(CoreError::Invalid("traccia fuori range".into()));
                }
                let c = p
                    .playlist
                    .clips
                    .iter_mut()
                    .find(|c| c.id == *clip)
                    .ok_or_else(|| CoreError::NotFound(format!("clip {clip}")))?;
                if let Some(v) = track {
                    c.track = *v;
                }
                if let Some(v) = position {
                    c.position = *v;
                }
                if let Some(v) = length {
                    c.length = (*v).max(1);
                }
                if let Some(v) = offset {
                    c.offset = *v;
                }
            }
            SetClipMuted { clip, muted } => {
                let c = p
                    .playlist
                    .clips
                    .iter_mut()
                    .find(|c| c.id == *clip)
                    .ok_or_else(|| CoreError::NotFound(format!("clip {clip}")))?;
                c.muted = *muted;
            }
            SetPlaylistTrack {
                track,
                name,
                muted,
                color,
            } => {
                if *track as usize >= PLAYLIST_TRACKS {
                    return Err(CoreError::Invalid("traccia fuori range".into()));
                }
                let t = p.playlist.tracks.entry(*track).or_insert_with(|| PlaylistTrack {
                    name: format!("Track {}", track + 1),
                    ..Default::default()
                });
                if let Some(v) = name {
                    t.name = v.clone();
                }
                if let Some(v) = muted {
                    t.muted = *v;
                }
                if let Some(v) = color {
                    t.color = *v;
                }
            }

            SetInsert {
                insert,
                name,
                volume,
                pan,
                stereo_separation,
                muted,
                solo,
                phase_invert,
                swap_lr,
            } => {
                check_insert(p, *insert)?;
                let vol = volume.map(|v| check_range("volume", v, 0.0, 1.25)).transpose()?;
                let pan = pan.map(|v| check_range("pan", v, -1.0, 1.0)).transpose()?;
                let sep = stereo_separation
                    .map(|v| check_range("stereo_separation", v, -1.0, 1.0))
                    .transpose()?;
                let ins = p.insert_mut(*insert).unwrap();
                if let Some(v) = name {
                    ins.name = v.clone();
                }
                if let Some(v) = vol {
                    ins.volume = v;
                }
                if let Some(v) = pan {
                    ins.pan = v;
                }
                if let Some(v) = sep {
                    ins.stereo_separation = v;
                }
                if let Some(v) = muted {
                    ins.muted = *v;
                }
                if let Some(v) = solo {
                    ins.solo = *v;
                }
                if let Some(v) = phase_invert {
                    ins.phase_invert = *v;
                }
                if let Some(v) = swap_lr {
                    ins.swap_lr = *v;
                }
            }
            SetRoute { from, to, gain } => {
                check_insert(p, *from)?;
                check_insert(p, *to)?;
                if from == to {
                    return Err(CoreError::Invalid("un insert non può instradarsi su se stesso".into()));
                }
                if gain.is_some() && routes_to(p, *to, *from) {
                    return Err(CoreError::Invalid(format!("routing {from}→{to} crea un ciclo")));
                }
                let ins = p.insert_mut(*from).unwrap();
                ins.routes.retain(|r| r.target != *to);
                if let Some(g) = gain {
                    ins.routes.push(Route {
                        target: *to,
                        gain: check_range("gain", *g, 0.0, 2.0)?,
                    });
                    ins.routes.sort_by_key(|r| r.target);
                }
            }
            AddEffect { insert, plugin, slot } => {
                check_insert(p, *insert)?;
                let mut plugin = plugin.clone();
                assign_instance_id(p, &mut plugin, false);
                let ins = p.insert_mut(*insert).unwrap();
                if ins.effects.len() >= FX_SLOTS {
                    return Err(CoreError::Invalid(format!(
                        "insert {insert}: tutti i {FX_SLOTS} slot sono occupati"
                    )));
                }
                let idx = slot
                    .map(|s| (s as usize).min(ins.effects.len()))
                    .unwrap_or(ins.effects.len());
                ins.effects.insert(idx, EffectSlot::new(plugin));
                return Ok(Outcome::Count(idx));
            }
            RemoveEffect { insert, slot } => {
                check_insert(p, *insert)?;
                let ins = p.insert_mut(*insert).unwrap();
                if *slot as usize >= ins.effects.len() {
                    return Err(CoreError::NotFound(format!("slot {slot} dell'insert {insert}")));
                }
                ins.effects.remove(*slot as usize);
            }
            SetEffect {
                insert,
                slot,
                enabled,
                mix,
            } => {
                check_insert(p, *insert)?;
                let mix = mix.map(|v| check_range("mix", v, 0.0, 1.0)).transpose()?;
                let fx = p
                    .insert_mut(*insert)
                    .unwrap()
                    .effects
                    .get_mut(*slot as usize)
                    .ok_or_else(|| CoreError::NotFound(format!("slot {slot}")))?;
                if let Some(v) = enabled {
                    fx.enabled = *v;
                }
                if let Some(v) = mix {
                    fx.mix = v;
                }
            }
            SetEffectParam {
                insert,
                slot,
                param,
                value,
            } => {
                check_insert(p, *insert)?;
                if !value.is_finite() {
                    return Err(CoreError::Invalid("valore non finito".into()));
                }
                let fx = p
                    .insert_mut(*insert)
                    .unwrap()
                    .effects
                    .get_mut(*slot as usize)
                    .ok_or_else(|| CoreError::NotFound(format!("slot {slot}")))?;
                fx.plugin.params.insert(param.clone(), *value);
            }

            AddAutomation {
                name,
                target,
                min,
                max,
                points,
            } => {
                validate_points(points)?;
                let id = AutomationId(p.alloc_id());
                let mut pts = points.clone();
                pts.sort_by_key(|x| x.position);
                p.automations.push(AutomationClip {
                    id,
                    name: name.clone(),
                    target: target.clone(),
                    min: *min,
                    max: *max,
                    points: pts,
                });
                return Ok(Outcome::Automation(id));
            }
            SetAutomationPoints { automation, points } => {
                validate_points(points)?;
                let a = p
                    .automation_mut(*automation)
                    .ok_or_else(|| CoreError::NotFound(format!("automation {automation}")))?;
                a.points = points.clone();
                a.points.sort_by_key(|x| x.position);
            }
            RemoveAutomation { automation } => {
                if p.automation(*automation).is_none() {
                    return Err(CoreError::NotFound(format!("automation {automation}")));
                }
                p.automations.retain(|a| a.id != *automation);
                p.playlist
                    .clips
                    .retain(|c| c.source != ClipSource::Automation(*automation));
            }

            Batch { commands } => {
                let mut out = Vec::with_capacity(commands.len());
                for c in commands {
                    out.push(c.apply(p)?);
                }
                return Ok(Outcome::Many(out));
            }
        }
        Ok(Outcome::None)
    }
}

/// Assegna un `instance_id` nuovo ai plugin esterni (all'aggiunta o alla clonazione).
fn assign_instance_id(p: &mut Project, plugin: &mut PluginInstance, force_new: bool) {
    if plugin.is_external() && (force_new || plugin.instance_id.is_none()) {
        plugin.instance_id = Some(format!("ext{}", p.alloc_id()));
    }
}

fn validate_points(points: &[AutomationPoint]) -> Result<(), CoreError> {
    for pt in points {
        check_range("automation value", pt.value, 0.0, 1.0)?;
        check_range("tension", pt.tension, -1.0, 1.0)?;
    }
    Ok(())
}

/// `true` se `from` raggiunge `to` seguendo le mandate.
fn routes_to(p: &Project, from: u16, to: u16) -> bool {
    let mut stack = vec![from];
    let mut seen = vec![false; p.mixer.inserts.len()];
    while let Some(i) = stack.pop() {
        if i == to {
            return true;
        }
        if std::mem::replace(&mut seen[i as usize], true) {
            continue;
        }
        stack.extend(p.mixer.inserts[i as usize].routes.iter().map(|r| r.target));
    }
    false
}

fn set_step(pt: &mut Pattern, tb: &TimeBase, ch: ChannelId, root: u8, step: u32, on: bool, vel: f32) {
    let len = tb.ticks_per_step();
    let pos = step as Tick * len;
    let notes = pt.notes.entry(ch).or_default();
    notes.retain(|n| n.position != pos);
    if on {
        notes.push(Note::new(pos, len, root, vel.clamp(0.0, 1.0)));
    }
    if notes.is_empty() {
        pt.notes.remove(&ch);
    }
    pt.sort_notes();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj_with_channel() -> (Project, ChannelId, PatternId) {
        let mut p = Project::new("t");
        let pid = p.patterns[0].id;
        let Outcome::Channel(cid) = Command::AddChannel {
            name: "Kick".into(),
            kind: ChannelKind::Generator,
            plugin: PluginInstance::new("mfc.kick"),
            mixer_insert: Some(1),
        }
        .apply(&mut p)
        .unwrap() else {
            panic!()
        };
        (p, cid, pid)
    }

    #[test]
    fn step_row_and_steps() {
        let (mut p, c, pt) = proj_with_channel();
        Command::SetStepRow {
            pattern: pt,
            channel: c,
            row: "x...x...x...x...".into(),
        }
        .apply(&mut p)
        .unwrap();
        assert_eq!(p.pattern(pt).unwrap().notes[&c].len(), 4);
        Command::SetStep {
            pattern: pt,
            channel: c,
            step: 4,
            on: false,
            velocity: None,
        }
        .apply(&mut p)
        .unwrap();
        let n = &p.pattern(pt).unwrap().notes[&c];
        assert_eq!(n.iter().map(|n| n.position).collect::<Vec<_>>(), vec![0, 192, 288]);
    }

    #[test]
    fn routing_cycle_rejected() {
        let mut p = Project::new("t");
        Command::SetRoute {
            from: 1,
            to: 2,
            gain: Some(1.0),
        }
        .apply(&mut p)
        .unwrap();
        assert!(Command::SetRoute {
            from: 2,
            to: 1,
            gain: Some(1.0)
        }
        .apply(&mut p)
        .is_err());
        // Il master non può rientrare in un insert che va già al master.
        assert!(Command::SetRoute {
            from: 0,
            to: 1,
            gain: Some(1.0)
        }
        .apply(&mut p)
        .is_err());
        assert!(Command::SetRoute {
            from: 2,
            to: 3,
            gain: Some(0.5)
        }
        .apply(&mut p)
        .is_ok());
    }

    #[test]
    fn remove_channel_cleans_notes() {
        let (mut p, c, pt) = proj_with_channel();
        Command::AddNotes {
            pattern: pt,
            channel: c,
            notes: vec![Note::new(0, 96, 60, 1.0)],
        }
        .apply(&mut p)
        .unwrap();
        Command::RemoveChannel { channel: c }.apply(&mut p).unwrap();
        assert!(p.pattern(pt).unwrap().notes.is_empty());
    }

    #[test]
    fn quantize_transpose() {
        let (mut p, c, pt) = proj_with_channel();
        Command::AddNotes {
            pattern: pt,
            channel: c,
            notes: vec![Note::new(13, 20, 60, 1.0)],
        }
        .apply(&mut p)
        .unwrap();
        Command::QuantizeNotes {
            pattern: pt,
            channel: None,
            grid: 24,
            quantize_length: true,
        }
        .apply(&mut p)
        .unwrap();
        Command::TransposeNotes {
            pattern: pt,
            channel: Some(c),
            semitones: 12,
        }
        .apply(&mut p)
        .unwrap();
        let n = p.pattern(pt).unwrap().notes[&c][0];
        assert_eq!((n.position, n.length, n.key), (24, 24, 72));
    }

    #[test]
    fn external_plugins_get_unique_instance_ids() {
        let mut p = Project::new("t");
        let ext = PluginInstance::new("vst3:/x/Synth.vst3::ABCD");
        let Outcome::Channel(a) = Command::AddChannel {
            name: "A".into(),
            kind: ChannelKind::Generator,
            plugin: ext.clone(),
            mixer_insert: None,
        }
        .apply(&mut p)
        .unwrap() else {
            panic!()
        };
        let Outcome::Channel(b) = Command::CloneChannel { channel: a }.apply(&mut p).unwrap() else {
            panic!()
        };
        let ia = p.channel(a).unwrap().plugin.instance_id.clone().unwrap();
        let ib = p.channel(b).unwrap().plugin.instance_id.clone().unwrap();
        assert_ne!(ia, ib);
        Command::AddEffect {
            insert: 1,
            plugin: PluginInstance::new("clap:/x/fx.clap::com.fx"),
            slot: None,
        }
        .apply(&mut p)
        .unwrap();
        assert!(p.insert(1).unwrap().effects[0].plugin.instance_id.is_some());
        // I plugin nativi non ricevono id.
        Command::AddEffect {
            insert: 1,
            plugin: PluginInstance::new("mfc.reverb"),
            slot: None,
        }
        .apply(&mut p)
        .unwrap();
        assert!(p.insert(1).unwrap().effects[1].plugin.instance_id.is_none());
    }

    #[test]
    fn command_json() {
        let c: Command = serde_json::from_str(r#"{"op":"set_tempo","bpm":128}"#).unwrap();
        assert_eq!(c, Command::SetTempo { bpm: 128.0 });
        assert_eq!(c.label(), "set_tempo");
    }
}
