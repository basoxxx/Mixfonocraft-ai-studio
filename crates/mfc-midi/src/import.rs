//! Importazione di file MIDI standard (SMF formato 0 e 1).

use std::collections::{BTreeMap, VecDeque};

use mfc_core::{ChannelId, ChannelKind, Command, Note, Outcome, PatternId, PluginInstance, Project, TimeBase};
use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};

use crate::MidiError;

/// Canale MIDI 10 (indice 9): percussioni secondo la convenzione General MIDI.
const DRUM_MIDI_CHANNEL: u8 = 9;

/// Traccia importata: un canale MIDI di una traccia sorgente, con le sue note.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    pub name: String,
    /// Canale MIDI sorgente, 0..=15.
    pub midi_channel: u8,
    pub notes: Vec<Note>,
}

/// Risultato del parsing di un file MIDI, già nel PPQ di destinazione.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MidiImport {
    /// Tempo del primo evento di tempo (solo valori 10..=999 BPM).
    pub tempo_bpm: Option<f64>,
    /// Metrica del primo evento di metrica (solo denominatori 1, 2, 4, 8, 16).
    pub time_signature: Option<(u8, u8)>,
    pub tracks: Vec<ImportedTrack>,
}

/// Nota grezza in tick del file sorgente.
struct RawNote {
    start: u64,
    end: u64,
    key: u8,
    vel: u8,
}

/// Parsa un file MIDI standard e restituisce le note in tick del PPQ `target_ppq`.
///
/// Le tracce di formato 0 vengono divise per canale MIDI, così come i canali
/// presenti in una traccia di formato 1. Le note sovrapposte sullo stesso tono
/// vengono accoppiate in modo FIFO; un `NoteOn` con velocity 0 equivale a un
/// `NoteOff`. Le note non terminate finiscono all'ultimo evento della traccia.
/// Le note di durata nulla ottengono durata 1. Le tracce vuote vengono omesse.
pub fn parse_midi(bytes: &[u8], target_ppq: u16) -> Result<MidiImport, MidiError> {
    if target_ppq == 0 {
        return Err(MidiError::Invalid("PPQ di destinazione nullo".into()));
    }
    let smf = Smf::parse(bytes).map_err(|e| MidiError::Parse(e.to_string()))?;
    let source_ppq = match smf.header.timing {
        Timing::Metrical(ppq) => ppq.as_int(),
        Timing::Timecode(..) => return Err(MidiError::Unsupported("timing SMPTE".into())),
    };
    if source_ppq == 0 {
        return Err(MidiError::Parse("PPQ nullo".into()));
    }
    if smf.header.format == Format::Sequential {
        return Err(MidiError::Unsupported("formato 2 (sequenziale)".into()));
    }
    let tb = TimeBase {
        ppq: target_ppq,
        ..TimeBase::default()
    };

    let mut tempo: Option<(u64, f64)> = None;
    let mut signature: Option<(u64, (u8, u8))> = None;
    let mut tracks = Vec::new();

    for (index, track) in smf.tracks.iter().enumerate() {
        let mut abs: u64 = 0;
        let mut name: Option<String> = None;
        let mut active: BTreeMap<(u8, u8), VecDeque<(u64, u8)>> = BTreeMap::new();
        let mut groups: BTreeMap<u8, Vec<RawNote>> = BTreeMap::new();

        for event in track {
            abs += u64::from(event.delta.as_int());
            match event.kind {
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            active
                                .entry((ch, key.as_int()))
                                .or_default()
                                .push_back((abs, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            let open = active.get_mut(&(ch, key.as_int())).and_then(VecDeque::pop_front);
                            if let Some((start, vel)) = open {
                                groups.entry(ch).or_default().push(RawNote {
                                    start,
                                    end: abs,
                                    key: key.as_int(),
                                    vel,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                TrackEventKind::Meta(MetaMessage::TrackName(bytes)) => {
                    if name.is_none() {
                        name = Some(String::from_utf8_lossy(bytes).trim().to_string());
                    }
                }
                TrackEventKind::Meta(MetaMessage::Tempo(us)) => {
                    let us = us.as_int();
                    if us > 0 && tempo.is_none_or(|(t, _)| abs < t) {
                        let bpm = 60_000_000.0 / f64::from(us);
                        if (10.0..=999.0).contains(&bpm) {
                            tempo = Some((abs, bpm));
                        }
                    }
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(num, den_pow, _, _)) => {
                    let den = 1u8.checked_shl(u32::from(den_pow)).unwrap_or(0);
                    let valid = num > 0 && matches!(den, 1 | 2 | 4 | 8 | 16);
                    if valid && signature.is_none_or(|(t, _)| abs < t) {
                        signature = Some((abs, (num, den)));
                    }
                }
                _ => {}
            }
        }

        // Note rimaste aperte: terminano all'ultimo evento della traccia.
        for ((ch, key), queue) in active {
            for (start, vel) in queue {
                groups.entry(ch).or_default().push(RawNote {
                    start,
                    end: abs,
                    key,
                    vel,
                });
            }
        }

        let base = name.unwrap_or_else(|| format!("Track {}", index + 1));
        let multi = groups.len() > 1;
        for (ch, raw) in groups {
            if raw.is_empty() {
                continue;
            }
            let mut notes: Vec<Note> = raw
                .iter()
                .map(|r| {
                    let position = tb.rescale_from(r.start, source_ppq);
                    let end = tb.rescale_from(r.end, source_ppq);
                    let length = end.saturating_sub(position).max(1);
                    Note::new(position, length, r.key, f32::from(r.vel) / 127.0)
                })
                .collect();
            notes.sort_by(|a, b| a.position.cmp(&b.position).then(a.key.cmp(&b.key)));
            let name = if multi {
                format!("{base} ch{}", ch + 1)
            } else {
                base.clone()
            };
            tracks.push(ImportedTrack {
                name,
                midi_channel: ch,
                notes,
            });
        }
    }

    Ok(MidiImport {
        tempo_bpm: tempo.map(|(_, bpm)| bpm),
        time_signature: signature.map(|(_, sig)| sig),
        tracks,
    })
}

/// Inserisce un [`MidiImport`] nel progetto: crea un pattern e un canale per traccia,
/// aggiunge le note e applica tempo e metrica se presenti.
///
/// I canali MIDI 10 (indice 9) usano il plugin `mfc.drumsynth`, gli altri `mfc.3xosc`.
/// L'operazione è atomica: in caso d'errore il progetto non viene modificato.
pub fn import_into_project(
    project: &mut Project,
    import: &MidiImport,
    pattern_name: &str,
) -> Result<(PatternId, Vec<ChannelId>), MidiError> {
    let mut work = project.clone();
    if let Some(bpm) = import.tempo_bpm {
        Command::SetTempo { bpm }.apply(&mut work)?;
    }
    if let Some((numerator, denominator)) = import.time_signature {
        Command::SetTimeSignature { numerator, denominator }.apply(&mut work)?;
    }

    let add_pattern = Command::AddPattern {
        name: pattern_name.to_string(),
    };
    let pattern = match add_pattern.apply(&mut work)? {
        Outcome::Pattern(id) => id,
        other => return Err(MidiError::Invalid(format!("esito inatteso: {other:?}"))),
    };

    let mut channels = Vec::with_capacity(import.tracks.len());
    for (index, track) in import.tracks.iter().enumerate() {
        let plugin = if track.midi_channel == DRUM_MIDI_CHANNEL {
            "mfc.drumsynth"
        } else {
            "mfc.3xosc"
        };
        let name = if track.name.trim().is_empty() {
            format!("Track {}", index + 1)
        } else {
            track.name.clone()
        };
        let add_channel = Command::AddChannel {
            name,
            kind: ChannelKind::Generator,
            plugin: PluginInstance::new(plugin),
            mixer_insert: None,
        };
        let channel = match add_channel.apply(&mut work)? {
            Outcome::Channel(id) => id,
            other => return Err(MidiError::Invalid(format!("esito inatteso: {other:?}"))),
        };
        if !track.notes.is_empty() {
            Command::AddNotes {
                pattern,
                channel,
                notes: track.notes.clone(),
            }
            .apply(&mut work)?;
        }
        channels.push(channel);
    }

    *project = work;
    Ok((pattern, channels))
}
