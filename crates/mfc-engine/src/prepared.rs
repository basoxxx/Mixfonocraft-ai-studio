//! Compilazione di un [`Project`] in uno stato runtime pronto per il thread audio:
//! generatori ed effetti istanziati, eventi di nota appiattiti e ordinati,
//! ordine topologico del mixer, maschera di ascolto per il solo.
//!
//! Va costruito FUORI dal thread audio (alloca, carica campioni).

use std::collections::HashMap;

use mfc_core::{
    AutomationClip, AutomationTarget, ChannelId, ChannelKind, ClipSource, PatternId, Project, Tick, TimeBase, MASTER,
};
use mfc_plugins::{create_effect, create_generator, Effect, Generator};

/// Evento di nota pronto per la riproduzione (posizioni in tick, frazionari per lo swing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoteEvent {
    pub on: f64,
    pub off: f64,
    /// Indice in `Prepared::channels`.
    pub chan: usize,
    pub key: u8,
    pub velocity: f32,
    pub pan: f32,
    pub fine_pitch: f32,
    pub slide: bool,
    pub mod_x: f32,
    pub mod_y: f32,
    /// Offset d'inizio (tick) per le audio clip avviate a metà.
    pub start_offset: f64,
}

pub(crate) struct ChannelRt {
    pub id: ChannelId,
    /// Chiave strutturale: se uguale tra due versioni il generatore viene riusato (le voci continuano).
    pub key: String,
    pub gen: Box<dyn Generator>,
    pub insert: usize,
    pub gain: f32,
    pub pan: f32,
    pub pitch: f32,
    pub muted: bool,
    pub root: u8,
    pub params: Vec<(String, f32)>,
}

pub(crate) struct SlotRt {
    pub key: String,
    pub fx: Option<Box<dyn Effect>>,
    pub enabled: bool,
    pub mix: f32,
    pub params: Vec<(String, f32)>,
}

pub(crate) struct InsertRt {
    pub slots: Vec<SlotRt>,
    pub gain: f32,
    pub pan: f32,
    pub separation: f32,
    pub muted: bool,
    pub phase_invert: bool,
    pub swap_lr: bool,
    pub routes: Vec<(usize, f32)>,
    /// Udibile secondo la logica di solo.
    pub audible: bool,
}

/// Bersaglio di automazione risolto in indici.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AutoTarget {
    MasterVolume,
    Tempo,
    ChannelVolume(usize),
    ChannelPan(usize),
    ChannelPitch(usize),
    ChannelParam(usize, String),
    MixerVolume(usize),
    MixerPan(usize),
    EffectParam(usize, usize, String),
    EffectMix(usize, usize),
}

pub(crate) struct AutoRt {
    pub start: f64,
    pub end: f64,
    pub offset: f64,
    pub clip: AutomationClip,
    pub target: AutoTarget,
}

/// Sequenza di eventi (song o pattern) ordinata per `on`.
#[derive(Default)]
pub(crate) struct Sequence {
    pub events: Vec<NoteEvent>,
    pub length: f64,
}

/// Stato runtime completo.
pub struct Prepared {
    pub(crate) sample_rate: f32,
    pub(crate) time: TimeBase,
    pub(crate) tempo: f64,
    pub(crate) master_gain: f32,
    pub(crate) master_pitch: f32,
    pub(crate) channels: Vec<ChannelRt>,
    pub(crate) inserts: Vec<InsertRt>,
    /// Ordine di elaborazione degli insert (il master per ultimo).
    pub(crate) order: Vec<usize>,
    pub(crate) song: Sequence,
    pub(crate) patterns: Vec<(PatternId, Sequence)>,
    pub(crate) automations: Vec<AutoRt>,
    pub(crate) insert_names: Vec<String>,
}

impl Prepared {
    /// Compila il progetto.
    pub fn build(project: &Project, sample_rate: f32) -> Self {
        let chan_index: HashMap<ChannelId, usize> =
            project.channels.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
        let n_inserts = project.mixer.inserts.len().max(1);

        let channels = project
            .channels
            .iter()
            .map(|c| {
                let mut gen = create_generator(&c.plugin, sample_rate);
                let pitch = c.pitch_cents + project.master_pitch;
                gen.set_pitch_offset(pitch);
                ChannelRt {
                    id: c.id,
                    key: channel_key(c),
                    gen,
                    insert: (c.mixer_insert as usize).min(n_inserts - 1),
                    gain: c.gain(),
                    pan: c.pan,
                    pitch,
                    muted: c.muted,
                    root: c.root_note,
                    params: c.plugin.params.iter().map(|(k, v)| (k.clone(), *v)).collect(),
                }
            })
            .collect();

        let mut inserts: Vec<InsertRt> = project
            .mixer
            .inserts
            .iter()
            .map(|ins| InsertRt {
                slots: ins
                    .effects
                    .iter()
                    .map(|s| SlotRt {
                        key: plugin_key(&s.plugin),
                        fx: create_effect(&s.plugin, sample_rate),
                        enabled: s.enabled,
                        mix: s.mix,
                        params: s.plugin.params.iter().map(|(k, v)| (k.clone(), *v)).collect(),
                    })
                    .collect(),
                gain: ins.gain(),
                pan: ins.pan,
                separation: ins.stereo_separation,
                muted: ins.muted,
                phase_invert: ins.phase_invert,
                swap_lr: ins.swap_lr,
                routes: ins
                    .routes
                    .iter()
                    .filter(|r| (r.target as usize) < n_inserts)
                    .map(|r| (r.target as usize, r.gain))
                    .collect(),
                audible: true,
            })
            .collect();
        if inserts.is_empty() {
            inserts.push(InsertRt {
                slots: vec![],
                gain: 1.0,
                pan: 0.0,
                separation: 0.0,
                muted: false,
                phase_invert: false,
                swap_lr: false,
                routes: vec![],
                audible: true,
            });
        }

        let order = topo_order(&mut inserts);
        apply_solo(project, &mut inserts);

        let song = build_song(project, &chan_index);
        let patterns = project
            .patterns
            .iter()
            .map(|p| {
                let len = p.effective_length(&project.time) as f64;
                let mut events = Vec::new();
                push_pattern_notes(project, &chan_index, p.id, 0.0, 0.0, len, &mut events);
                events.sort_by(|a, b| a.on.total_cmp(&b.on));
                (p.id, Sequence { events, length: len })
            })
            .collect();

        let automations = build_automations(project, &chan_index);

        Self {
            sample_rate,
            time: project.time,
            tempo: project.tempo,
            master_gain: mfc_core::fader_gain(project.master_volume, mfc_core::MIXER_UNITY),
            master_pitch: project.master_pitch,
            channels,
            inserts,
            order,
            song,
            patterns,
            automations,
            insert_names: project.mixer.inserts.iter().map(|i| i.name.clone()).collect(),
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Lunghezza del brano in tick.
    pub fn song_length(&self) -> f64 {
        self.song.length
    }

    pub(crate) fn sequence(&self, pattern: Option<PatternId>) -> Option<&Sequence> {
        match pattern {
            None => Some(&self.song),
            Some(id) => self.patterns.iter().find(|(p, _)| *p == id).map(|(_, s)| s),
        }
    }
}

fn plugin_key(p: &mfc_core::PluginInstance) -> String {
    format!("{}|{}", p.kind, p.sample_path.as_deref().unwrap_or(""))
}

fn channel_key(c: &mfc_core::Channel) -> String {
    format!("{}|{}", c.id, plugin_key(&c.plugin))
}

/// Ordine topologico (Kahn) degli insert seguendo le mandate; il master è sempre ultimo.
/// Eventuali cicli (possibili in progetti importati) vengono spezzati rimuovendo le mandate residue.
fn topo_order(inserts: &mut [InsertRt]) -> Vec<usize> {
    let n = inserts.len();
    let mut indeg = vec![0usize; n];
    for ins in inserts.iter() {
        for &(t, _) in &ins.routes {
            indeg[t] += 1;
        }
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0 && i != MASTER as usize).collect();
    let mut order = Vec::with_capacity(n);
    let mut done = vec![false; n];
    while let Some(i) = queue.pop() {
        if done[i] {
            continue;
        }
        done[i] = true;
        order.push(i);
        for &(t, _) in &inserts[i].routes {
            indeg[t] -= 1;
            if indeg[t] == 0 && t != MASTER as usize {
                queue.push(t);
            }
        }
    }
    // Nodi in ciclo: elaborati in ordine di indice, mandate verso nodi già elaborati rimosse.
    for i in 0..n {
        if !done[i] && i != MASTER as usize {
            done[i] = true;
            order.push(i);
        }
    }
    let pos: Vec<usize> = {
        let mut p = vec![usize::MAX; n];
        for (k, &i) in order.iter().enumerate() {
            p[i] = k;
        }
        p
    };
    for (i, ins) in inserts.iter_mut().enumerate() {
        if i == MASTER as usize {
            ins.routes.clear();
        } else {
            ins.routes.retain(|&(t, _)| t == MASTER as usize || pos[t] > pos[i]);
        }
    }
    order.push(MASTER as usize);
    order
}

/// Logica solo stile FL: se un insert è in solo, restano udibili lui, ciò che lo alimenta
/// e il percorso verso il master.
fn apply_solo(project: &Project, inserts: &mut [InsertRt]) {
    let soloed: Vec<usize> = project
        .mixer
        .inserts
        .iter()
        .enumerate()
        .filter(|(i, ins)| ins.solo && *i != MASTER as usize)
        .map(|(i, _)| i)
        .collect();
    if soloed.is_empty() {
        return;
    }
    let n = inserts.len();
    let mut audible = vec![false; n];
    audible[MASTER as usize] = true;
    for &s in &soloed {
        // a valle
        let mut stack = vec![s];
        while let Some(i) = stack.pop() {
            if !std::mem::replace(&mut audible[i], true) || i == s {
                stack.extend(inserts[i].routes.iter().map(|r| r.0));
            }
        }
        // a monte
        let mut stack = vec![s];
        let mut seen = vec![false; n];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            audible[i] = true;
            for (j, ins) in inserts.iter().enumerate() {
                if ins.routes.iter().any(|r| r.0 == i) {
                    stack.push(j);
                }
            }
        }
    }
    for (i, ins) in inserts.iter_mut().enumerate() {
        ins.audible = audible[i];
    }
}

/// Spostamento swing per una nota (in tick) sugli step dispari.
fn swing_shift(project: &Project, pos: Tick) -> f64 {
    if project.swing <= 0.0 {
        return 0.0;
    }
    let step = project.time.ticks_per_step();
    if pos % (2 * step) == step {
        project.swing as f64 * step as f64 * 0.5
    } else {
        0.0
    }
}

/// Aggiunge le note del pattern `pid` comprese in `[offset, offset+len)`, traslate a `base`.
fn push_pattern_notes(
    project: &Project,
    chan_index: &HashMap<ChannelId, usize>,
    pid: PatternId,
    base: f64,
    offset: f64,
    len: f64,
    out: &mut Vec<NoteEvent>,
) {
    let Some(pat) = project.pattern(pid) else { return };
    for (cid, notes) in &pat.notes {
        let Some(&ci) = chan_index.get(cid) else { continue };
        let ch = &project.channels[ci];
        // Canali layer: inoltra ai figli.
        let targets: Vec<usize> = if ch.kind == ChannelKind::Layer {
            ch.layer_children.iter().filter_map(|c| chan_index.get(c).copied()).collect()
        } else {
            vec![ci]
        };
        for n in notes {
            let start = n.position as f64 + swing_shift(project, n.position);
            if start < offset || start >= offset + len {
                continue;
            }
            let on = base + start - offset;
            let off = (base + (n.end() as f64 - offset)).min(base + len).max(on + 1.0);
            for &t in &targets {
                out.push(NoteEvent {
                    on,
                    off,
                    chan: t,
                    key: n.key,
                    velocity: n.velocity,
                    pan: n.pan,
                    fine_pitch: n.fine_pitch,
                    slide: n.slide,
                    mod_x: n.mod_x,
                    mod_y: n.mod_y,
                    start_offset: 0.0,
                });
            }
        }
    }
}

fn build_song(project: &Project, chan_index: &HashMap<ChannelId, usize>) -> Sequence {
    let mut events = Vec::new();
    for clip in &project.playlist.clips {
        if clip.muted || project.playlist.track_muted(clip.track) {
            continue;
        }
        match clip.source {
            ClipSource::Pattern(pid) => {
                let pat_len = project.pattern(pid).map(|p| p.effective_length(&project.time)).unwrap_or(0) as f64;
                if pat_len <= 0.0 {
                    continue;
                }
                // Le clip più lunghe del pattern lo ripetono (come il loop dei pattern in Playlist).
                let mut written = 0.0;
                let mut local_off = clip.offset as f64 % pat_len;
                while written < clip.length as f64 {
                    let seg = (pat_len - local_off).min(clip.length as f64 - written);
                    push_pattern_notes(
                        project,
                        chan_index,
                        pid,
                        clip.position as f64 + written,
                        local_off,
                        seg,
                        &mut events,
                    );
                    written += seg;
                    local_off = 0.0;
                }
            }
            ClipSource::Audio(cid) => {
                let Some(&ci) = chan_index.get(&cid) else { continue };
                let root = project.channels[ci].root_note;
                events.push(NoteEvent {
                    on: clip.position as f64,
                    off: clip.end() as f64,
                    chan: ci,
                    key: root,
                    velocity: 1.0,
                    pan: 0.0,
                    fine_pitch: 0.0,
                    slide: false,
                    mod_x: 0.5,
                    mod_y: 0.5,
                    start_offset: clip.offset as f64,
                });
            }
            ClipSource::Automation(_) => {}
        }
    }
    events.sort_by(|a, b| a.on.total_cmp(&b.on));
    Sequence { events, length: project.song_length() as f64 }
}

fn build_automations(project: &Project, chan_index: &HashMap<ChannelId, usize>) -> Vec<AutoRt> {
    let mut out = Vec::new();
    for clip in &project.playlist.clips {
        let ClipSource::Automation(aid) = clip.source else { continue };
        if clip.muted || project.playlist.track_muted(clip.track) {
            continue;
        }
        let Some(a) = project.automation(aid) else { continue };
        let ch = |c: &ChannelId| chan_index.get(c).copied();
        let n_ins = project.mixer.inserts.len();
        let ins = |i: u16| ((i as usize) < n_ins).then_some(i as usize);
        let target = match &a.target {
            AutomationTarget::MasterVolume => Some(AutoTarget::MasterVolume),
            AutomationTarget::Tempo => Some(AutoTarget::Tempo),
            AutomationTarget::ChannelVolume { channel } => ch(channel).map(AutoTarget::ChannelVolume),
            AutomationTarget::ChannelPan { channel } => ch(channel).map(AutoTarget::ChannelPan),
            AutomationTarget::ChannelPitch { channel } => ch(channel).map(AutoTarget::ChannelPitch),
            AutomationTarget::ChannelParam { channel, param } => {
                ch(channel).map(|c| AutoTarget::ChannelParam(c, param.clone()))
            }
            AutomationTarget::MixerVolume { insert } => ins(*insert).map(AutoTarget::MixerVolume),
            AutomationTarget::MixerPan { insert } => ins(*insert).map(AutoTarget::MixerPan),
            AutomationTarget::EffectParam { insert, slot, param } => {
                ins(*insert).map(|i| AutoTarget::EffectParam(i, *slot as usize, param.clone()))
            }
            AutomationTarget::EffectMix { insert, slot } => {
                ins(*insert).map(|i| AutoTarget::EffectMix(i, *slot as usize))
            }
        };
        if let Some(target) = target {
            out.push(AutoRt {
                start: clip.position as f64,
                end: clip.end() as f64,
                offset: clip.offset as f64,
                clip: a.clone(),
                target,
            });
        }
    }
    out
}
