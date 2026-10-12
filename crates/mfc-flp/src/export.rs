//! Export semantico: [`Project`] → [`FlpFile`] in stile FL Studio 20 (UTF-16, PPQ del progetto).
//! L'ordine degli eventi segue quello documentato da PyFLP/Monad.FLParser per quanto noto.

use std::collections::HashMap;

use mfc_core::{ChannelId, ChannelKind, ClipSource, Color, MixerInsert, PatternId, Project, PLAYLIST_TRACKS};

use crate::events::{id, FlpFile, FlpHeader, RawEvent, TextEncoding};
use crate::plugin_map::{fl_effect_name, fl_generator_name, FL_WRAPPER};
use crate::structs::{
    insert_flag, insert_flags_write, mixer_param, FlLevels, FlMixerParam, FlNote, FlPlaylistItem, FlTrackData,
    ITEM_FLAG_MUTED, NOTE_FLAG_SLIDE, PATTERN_BASE,
};

/// Versione di FL dichiarata nei file esportati.
pub const EXPORT_VERSION: &str = "20.8.4.2576";
/// Build corrispondente.
pub const EXPORT_BUILD: u32 = 2576;
/// Insert scritti: master + 125 + "current" (FL ≥ 12.9).
pub const EXPORT_INSERTS: usize = 127;
/// Slot effetto per insert.
pub const EXPORT_SLOTS: u16 = 10;
/// Indice massimo di traccia (tracce invertite) per FL ≥ 12.9.1.
const TRACK_MAX_IDX: u16 = 499;

const ENC: TextEncoding = TextEncoding::Utf16;

fn rgba(c: Color) -> u32 {
    u32::from_le_bytes([c.0, c.1, c.2, 0])
}

struct Out(Vec<RawEvent>);

impl Out {
    fn num(&mut self, id: u8, v: u32) {
        self.0.push(RawEvent::num(id, v));
    }
    fn text(&mut self, id: u8, s: &str) {
        self.0.push(RawEvent::text(id, s, ENC));
    }
    fn text_nonempty(&mut self, id: u8, s: &str) {
        if !s.is_empty() {
            self.text(id, s);
        }
    }
    fn data(&mut self, id: u8, d: Vec<u8>) {
        self.0.push(RawEvent::data(id, d));
    }
}

/// Plugin del canale in termini FL: (tipo canale, nome interno, dati plugin).
fn channel_plugin(ch: &mfc_core::Channel) -> (u8, Option<String>, Option<Vec<u8>>) {
    let pl = &ch.plugin;
    match ch.kind {
        ChannelKind::Sampler | ChannelKind::Automation => (0, None, None),
        ChannelKind::AudioClip => (2, None, None),
        ChannelKind::Layer => (3, None, None),
        ChannelKind::Generator => {
            if let Some(f) = pl.foreign_name.as_ref().filter(|f| !f.is_empty()) {
                let ty = if f == FL_WRAPPER { 4 } else { 2 };
                (ty, Some(f.clone()), pl.opaque_state.clone())
            } else if let Some(f) = fl_generator_name(&pl.kind) {
                (2, Some(f.to_string()), pl.opaque_state.clone())
            } else {
                // Nessun equivalente FL: canale sampler vuoto con lo stesso nome.
                (0, None, None)
            }
        }
    }
}

/// Converte il progetto in un file FLP.
pub fn from_project(p: &Project) -> FlpFile {
    let mut o = Out(Vec::new());
    let ppq = p.time.ppq.max(1);

    // Progetto.
    o.data(
        id::FL_VERSION,
        crate::events::encode_text(EXPORT_VERSION, TextEncoding::Ascii),
    );
    o.num(id::FL_BUILD, EXPORT_BUILD);
    o.num(id::TEMPO, (p.tempo * 1000.0).round().clamp(10_000.0, 999_000.0) as u32);
    let pat_iid: HashMap<PatternId, u16> = p
        .patterns
        .iter()
        .enumerate()
        .map(|(i, pt)| (pt.id, (i as u16).saturating_add(1)))
        .collect();
    let cur = p.current_pattern.and_then(|id| pat_iid.get(&id).copied()).unwrap_or(1);
    o.num(id::CURRENT_PATTERN, cur as u32);
    o.num(id::SWING, (p.swing.clamp(0.0, 1.0) * 128.0).round() as u32);
    o.num(
        id::MAIN_VOLUME,
        (p.master_volume.clamp(0.0, 1.25) * 100.0).round() as u32,
    );
    o.num(
        id::MAIN_PITCH,
        (p.master_pitch.clamp(-1200.0, 1200.0).round() as i16) as u16 as u32,
    );
    o.num(id::TIME_SIG_NUM, p.time.signature.numerator as u32);
    o.num(id::TIME_SIG_BEAT, p.time.signature.denominator as u32);
    o.text_nonempty(id::TITLE, &p.info.title);
    o.text_nonempty(id::COMMENTS, &p.info.comments);
    o.text_nonempty(id::URL, &p.info.url);
    o.text_nonempty(id::GENRE, &p.info.genre);
    o.text_nonempty(id::ARTISTS, &p.info.artist);

    let chan_iid: HashMap<ChannelId, u16> = p.channels.iter().enumerate().map(|(i, c)| (c.id, i as u16)).collect();

    // Note dei pattern (primo blocco PatNew).
    for (i, pt) in p.patterns.iter().enumerate() {
        let mut notes: Vec<FlNote> = Vec::new();
        for (cid, list) in &pt.notes {
            let Some(&rack) = chan_iid.get(cid) else { continue };
            for n in list {
                notes.push(FlNote {
                    position: n.position.min(u32::MAX as u64) as u32,
                    flags: if n.slide { NOTE_FLAG_SLIDE } else { 0 },
                    rack_channel: rack,
                    length: n.length.min(u32::MAX as u64) as u32,
                    key: n.key as u16,
                    group: 0,
                    fine_pitch: (120.0 + n.fine_pitch.clamp(-120.0, 120.0)).round() as u8,
                    u1: 0,
                    release: (n.release.clamp(0.0, 1.0) * 128.0).round() as u8,
                    midi_channel: 0,
                    pan: (64.0 + n.pan.clamp(-1.0, 1.0) * 64.0).round() as u8,
                    velocity: (n.velocity.clamp(0.0, 1.0) * 128.0).round() as u8,
                    mod_x: (n.mod_x.clamp(0.0, 1.0) * 256.0).round().min(255.0) as u8,
                    mod_y: (n.mod_y.clamp(0.0, 1.0) * 256.0).round().min(255.0) as u8,
                });
            }
        }
        if notes.is_empty() {
            continue;
        }
        notes.sort_by(|a, b| a.position.cmp(&b.position).then(a.key.cmp(&b.key)));
        let mut d = Vec::with_capacity(notes.len() * 24);
        for n in &notes {
            n.write(&mut d);
        }
        o.num(id::PAT_NEW, i as u32 + 1);
        o.data(id::PAT_NOTES, d);
    }
    // Metadati dei pattern (secondo blocco PatNew).
    for (i, pt) in p.patterns.iter().enumerate() {
        o.num(id::PAT_NEW, i as u32 + 1);
        o.text_nonempty(id::PAT_NAME, &pt.name);
        o.num(id::PAT_COLOR, rgba(pt.color));
    }

    // Canali.
    for (i, ch) in p.channels.iter().enumerate() {
        let (ty, internal, data) = channel_plugin(ch);
        o.num(id::CHAN_NEW, i as u32);
        o.num(id::CHAN_TYPE, ty as u32);
        if let Some(n) = &internal {
            o.text(id::PLUGIN_INTERNAL_NAME, n);
        }
        o.text(id::PLUGIN_NAME, &ch.name);
        if let Some(d) = data {
            o.data(id::PLUGIN_DATA, d);
        }
        o.num(id::PLUGIN_COLOR, rgba(ch.color));
        o.num(id::CHAN_ENABLED, !ch.muted as u32);
        o.num(id::CHAN_ROUTED_TO, ch.mixer_insert.min(125) as u32);
        if matches!(ty, 0 | 2) && internal.is_none() {
            if let Some(sp) = &ch.plugin.sample_path {
                o.text(id::CHAN_SAMPLE_PATH, sp);
            }
        }
        let vol = (ch.volume.clamp(0.0, 1.0) * 12800.0).round() as u32;
        let pan = (6400.0 + ch.pan.clamp(-1.0, 1.0) * 6400.0).round() as i32;
        let pitch = ch.pitch_cents.clamp(-4800.0, 4800.0).round() as i32;
        o.data(id::CHAN_LEVELS, FlLevels::write(pan, vol, pitch));
        o.num(id::CHAN_ROOT_NOTE, ch.root_note as u32);
        if ch.kind == ChannelKind::Layer {
            for kid in &ch.layer_children {
                if let Some(&k) = chan_iid.get(kid) {
                    o.num(id::CHAN_CHILDREN, k as u32);
                }
            }
        }
    }

    // Arrangement e Playlist.
    o.num(id::ARRANGEMENT_NEW, 0);
    o.text(id::ARRANGEMENT_NAME, "Arrangement");
    let mut items: Vec<FlPlaylistItem> = Vec::new();
    for c in &p.playlist.clips {
        if c.track as usize >= PLAYLIST_TRACKS {
            continue;
        }
        let (item_index, start, end) = match c.source {
            ClipSource::Pattern(pid) => {
                let Some(&iid) = pat_iid.get(&pid) else { continue };
                let (s, e) = if c.offset > 0 {
                    (c.offset as f32, c.offset.saturating_add(c.length) as f32)
                } else {
                    (-1.0, -1.0)
                };
                (PATTERN_BASE.saturating_add(iid), s, e)
            }
            ClipSource::Audio(cid) => {
                let Some(&iid) = chan_iid.get(&cid) else { continue };
                let (s, e) = if c.offset > 0 {
                    let b = |t: u64| (t as f64 / ppq as f64) as f32;
                    (b(c.offset), b(c.offset.saturating_add(c.length)))
                } else {
                    (-1.0, -1.0)
                };
                (iid, s, e)
            }
            ClipSource::Automation(_) => continue,
        };
        items.push(FlPlaylistItem {
            position: c.position.min(u32::MAX as u64) as u32,
            pattern_base: PATTERN_BASE,
            item_index,
            length: c.length.min(u32::MAX as u64) as u32,
            track_rvidx: TRACK_MAX_IDX - c.track,
            group: 0,
            flags: 64 | if c.muted { ITEM_FLAG_MUTED } else { 0 },
            start_offset: start,
            end_offset: end,
        });
    }
    items.sort_by_key(|it| (it.position, TRACK_MAX_IDX - it.track_rvidx));
    if !items.is_empty() {
        let mut d = Vec::with_capacity(items.len() * 32);
        for it in &items {
            it.write(&mut d);
        }
        o.data(id::PLAYLIST_ITEMS, d);
    }
    let default_track = Color(0x48, 0x51, 0x56);
    for t in 0..PLAYLIST_TRACKS as u16 {
        let tr = p.playlist.tracks.get(&t);
        let color = tr.map(|x| x.color).unwrap_or(default_track);
        let enabled = !tr.is_some_and(|x| x.muted);
        let c = color;
        o.data(
            id::TRACK_DATA,
            FlTrackData::write(t as u32 + 1, [c.0, c.1, c.2, 0], enabled),
        );
        if let Some(tr) = tr {
            o.text_nonempty(id::TRACK_NAME, &tr.name);
        }
    }
    o.num(id::ARRANGEMENT_CURRENT, 0);

    // Mixer: parametri globali.
    let inserts: Vec<MixerInsert> = (0..EXPORT_INSERTS)
        .map(|i| {
            p.mixer
                .inserts
                .get(i)
                .cloned()
                .unwrap_or_else(|| MixerInsert::new(i as u16))
        })
        .collect();
    let mut params = Vec::new();
    for (i, mi) in inserts.iter().enumerate() {
        let insert = i as u16;
        let mut push = |id: u8, slot: u16, msg: i32| {
            FlMixerParam { id, insert, slot, msg }.write(&mut params);
        };
        for (k, fx) in mi.effects.iter().take(EXPORT_SLOTS as usize).enumerate() {
            push(mixer_param::SLOT_ENABLED, k as u16, fx.enabled as i32);
            push(
                mixer_param::SLOT_MIX,
                k as u16,
                (fx.mix.clamp(0.0, 1.0) * 12800.0 - 6400.0).round() as i32,
            );
        }
        for r in &mi.routes {
            if (r.target as usize) < EXPORT_INSERTS {
                let id = mixer_param::ROUTE_VOL_START + r.target as u8;
                push(id, 0, (r.gain.max(0.0) * 12800.0).round().min(i32::MAX as f32) as i32);
            }
        }
        push(
            mixer_param::VOLUME,
            0,
            (mi.volume.clamp(0.0, 1.0) * 16000.0).round() as i32,
        );
        push(mixer_param::PAN, 0, (mi.pan.clamp(-1.0, 1.0) * 6400.0).round() as i32);
        push(
            mixer_param::STEREO_SEPARATION,
            0,
            (-mi.stereo_separation.clamp(-1.0, 1.0) * 64.0).round() as i32,
        );
    }
    o.data(id::MIXER_PARAMS, params);

    // Mixer: insert.
    for (i, mi) in inserts.iter().enumerate() {
        let mut flags = insert_flag::ENABLE_EFFECTS;
        if !mi.muted {
            flags |= insert_flag::ENABLED;
        }
        if mi.solo {
            flags |= insert_flag::SOLO;
        }
        if mi.phase_invert {
            flags |= insert_flag::POLARITY_REVERSED;
        }
        if mi.swap_lr {
            flags |= insert_flag::SWAP_LEFT_RIGHT;
        }
        o.data(id::INSERT_FLAGS, insert_flags_write(flags));
        for k in 0..EXPORT_SLOTS {
            if let Some(fx) = mi.effects.get(k as usize) {
                let name = fx
                    .plugin
                    .foreign_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .or_else(|| fl_effect_name(&fx.plugin.kind).map(str::to_string));
                if let Some(name) = name {
                    o.text(id::PLUGIN_INTERNAL_NAME, &name);
                    if let Some(d) = &fx.plugin.opaque_state {
                        o.data(id::PLUGIN_DATA, d.clone());
                    }
                }
            }
            o.num(id::SLOT_INDEX, k as u32);
        }
        o.num(id::INSERT_COLOR, rgba(mi.color));
        if mi.name != MixerInsert::new(i as u16).name {
            o.text_nonempty(id::INSERT_NAME, &mi.name);
        }
        let mut routing = vec![0u8; EXPORT_INSERTS];
        for r in &mi.routes {
            if let Some(b) = routing.get_mut(r.target as usize) {
                *b = 1;
            }
        }
        o.data(id::INSERT_ROUTING, routing);
        o.num(id::INSERT_INPUT, u32::MAX);
        o.num(id::INSERT_OUTPUT, u32::MAX);
    }

    FlpFile {
        header: FlpHeader {
            format: 0,
            n_channels: p.channels.len().min(u16::MAX as usize) as u16,
            ppq,
        },
        events: o.0,
        trailing: Vec::new(),
    }
}
