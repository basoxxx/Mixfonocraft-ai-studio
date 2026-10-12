//! Esportazione di pattern e Playlist in file MIDI standard (SMF formato 1).

use std::collections::BTreeMap;

use mfc_core::{ChannelId, ClipSource, CoreError, PatternId, Project, Tick, TimeSignature};
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

use crate::MidiError;

/// Canale MIDI 10 (indice 9), riservato alle percussioni.
const DRUM_MIDI_CHANNEL: u8 = 9;
/// Plugin che vengono assegnati al canale percussioni.
const DRUM_PLUGINS: [&str; 2] = ["mfc.drumsynth", "mfc.kick"];
/// Canali MIDI 0..=15 senza il canale percussioni, assegnati in round robin.
const MELODIC_CHANNELS: [u8; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];

/// Evento grezzo: (tick assoluto, è NoteOn, tono, velocity).
type RawEvent = (u64, bool, u8, u8);

/// Esporta un pattern come SMF formato 1.
///
/// La prima traccia contiene tempo, metrica e nome del pattern; seguono
/// una traccia per canale con note nel pattern. Il PPQ del file è quello del progetto.
pub fn export_pattern(project: &Project, pattern: PatternId) -> Result<Vec<u8>, MidiError> {
    let pat = project
        .pattern(pattern)
        .ok_or_else(|| CoreError::NotFound(format!("pattern {pattern}")))?;

    let ids: Vec<ChannelId> = pat
        .notes
        .iter()
        .filter(|(_, notes)| !notes.is_empty())
        .map(|(id, _)| *id)
        .collect();
    let midi_channels = assign_midi_channels(project, &ids);
    let names: Vec<String> = ids.iter().map(|id| channel_name(project, *id)).collect();

    let mut smf = Smf::new(header_for(project.time.ppq)?);
    smf.tracks
        .push(meta_track(&pat.name, project.tempo, project.time.signature));
    for (id, name) in ids.iter().zip(&names) {
        let mut raw = Vec::new();
        for note in &pat.notes[id] {
            push_note(&mut raw, note.position, note.length, note.key, note.velocity);
        }
        smf.tracks.push(channel_track(name, midi_channels[id], raw));
    }
    write_smf(&smf)
}

/// Esporta l'intera Playlist come SMF formato 1.
///
/// Per ogni clip non mutata (e su traccia non mutata) di tipo pattern vengono
/// emesse le note del pattern spostate alla posizione della clip, considerando
/// `offset` e `length`: le note sono tagliate alla fine della clip e quelle fuori
/// dalla finestra `[offset, offset + length)` vengono scartate.
pub fn export_song(project: &Project) -> Result<Vec<u8>, MidiError> {
    let mut per_channel: BTreeMap<ChannelId, Vec<RawEvent>> = BTreeMap::new();

    for clip in &project.playlist.clips {
        if clip.muted || project.playlist.track_muted(clip.track) {
            continue;
        }
        let ClipSource::Pattern(pid) = clip.source else {
            continue;
        };
        let pat = project
            .pattern(pid)
            .ok_or_else(|| CoreError::NotFound(format!("pattern {pid}")))?;
        let window_start = clip.offset;
        let window_end = clip.offset.saturating_add(clip.length);

        for (cid, notes) in &pat.notes {
            let raw = per_channel.entry(*cid).or_default();
            for note in notes {
                let start = note.position.max(window_start);
                let end = note.end().min(window_end);
                if start >= end {
                    continue;
                }
                let song_start = clip.position + (start - clip.offset);
                push_note(raw, song_start, end - start, note.key, note.velocity);
            }
        }
    }

    per_channel.retain(|_, raw| !raw.is_empty());
    let ids: Vec<ChannelId> = per_channel.keys().copied().collect();
    let midi_channels = assign_midi_channels(project, &ids);
    let names: Vec<String> = ids.iter().map(|id| channel_name(project, *id)).collect();
    let title = if project.info.title.trim().is_empty() {
        "Song".to_string()
    } else {
        project.info.title.clone()
    };

    let mut smf = Smf::new(header_for(project.time.ppq)?);
    smf.tracks
        .push(meta_track(&title, project.tempo, project.time.signature));
    for (id, name) in ids.iter().zip(&names) {
        let raw = per_channel.remove(id).unwrap_or_default();
        smf.tracks.push(channel_track(name, midi_channels[id], raw));
    }
    write_smf(&smf)
}

/// Assegna un canale MIDI a ciascun canale: i canali percussivi vanno al canale 10,
/// gli altri in round robin sui canali 0..=15 escluso il 9.
fn assign_midi_channels(project: &Project, ids: &[ChannelId]) -> BTreeMap<ChannelId, u8> {
    let mut out = BTreeMap::new();
    let mut next = 0usize;
    for &id in ids {
        let is_drum = project
            .channel(id)
            .is_some_and(|c| DRUM_PLUGINS.contains(&c.plugin.kind.as_str()));
        let midi = if is_drum {
            DRUM_MIDI_CHANNEL
        } else {
            let ch = MELODIC_CHANNELS[next % MELODIC_CHANNELS.len()];
            next += 1;
            ch
        };
        out.insert(id, midi);
    }
    out
}

fn channel_name(project: &Project, id: ChannelId) -> String {
    project
        .channel(id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("Channel {id}"))
}

/// Intestazione SMF formato 1 con il PPQ indicato.
fn header_for(ppq: u16) -> Result<Header, MidiError> {
    if ppq == 0 || ppq > 0x7FFF {
        return Err(MidiError::Invalid(format!("PPQ {ppq} fuori range 1..=32767")));
    }
    Ok(Header::new(Format::Parallel, Timing::Metrical(u15::from(ppq))))
}

/// Accoda una nota (NoteOn all'inizio, NoteOff alla fine; durata minima 1 tick).
fn push_note(raw: &mut Vec<RawEvent>, start: Tick, length: Tick, key: u8, velocity: f32) {
    let end = start + length.max(1);
    let key = key.min(127);
    raw.push((start, true, key, velocity_to_midi(velocity)));
    raw.push((end, false, key, 0));
}

/// Velocity 0..=1 → 1..=127 (mai 0, altrimenti sarebbe un NoteOff).
fn velocity_to_midi(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 127.0).round().clamp(1.0, 127.0) as u8
}

/// Traccia meta: nome, tempo e metrica.
fn meta_track(name: &str, bpm: f64, sig: TimeSignature) -> Vec<TrackEvent<'_>> {
    let us = (60_000_000.0 / bpm.max(1.0))
        .round()
        .clamp(1.0, f64::from(0xFF_FFFF_u32)) as u32;
    let den_pow = if sig.denominator.is_power_of_two() {
        sig.denominator.trailing_zeros() as u8
    } else {
        2
    };
    vec![
        event(0, TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes()))),
        event(0, TrackEventKind::Meta(MetaMessage::Tempo(u24::from(us)))),
        event(
            0,
            TrackEventKind::Meta(MetaMessage::TimeSignature(sig.numerator, den_pow, 24, 8)),
        ),
        end_of_track(),
    ]
}

/// Traccia di un canale: nome, poi note ordinate con NoteOff prima di NoteOn a parità di tick.
fn channel_track(name: &str, midi_channel: u8, mut raw: Vec<RawEvent>) -> Vec<TrackEvent<'_>> {
    raw.sort_by_key(|&(tick, on, _, _)| (tick, on));
    let mut track = vec![event(0, TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())))];
    let channel = u4::from(midi_channel);
    let mut prev: u64 = 0;
    for (tick, on, key, vel) in raw {
        let message = if on {
            MidiMessage::NoteOn {
                key: u7::from(key),
                vel: u7::from(vel),
            }
        } else {
            MidiMessage::NoteOff {
                key: u7::from(key),
                vel: u7::from(vel),
            }
        };
        track.push(TrackEvent {
            delta: u28::from((tick - prev) as u32),
            kind: TrackEventKind::Midi { channel, message },
        });
        prev = tick;
    }
    track.push(end_of_track());
    track
}

fn event(delta: u32, kind: TrackEventKind<'_>) -> TrackEvent<'_> {
    TrackEvent {
        delta: u28::from(delta),
        kind,
    }
}

fn end_of_track<'a>() -> TrackEvent<'a> {
    event(0, TrackEventKind::Meta(MetaMessage::EndOfTrack))
}

fn write_smf(smf: &Smf) -> Result<Vec<u8>, MidiError> {
    let mut out = Vec::new();
    smf.write_std(&mut out)?;
    Ok(out)
}
