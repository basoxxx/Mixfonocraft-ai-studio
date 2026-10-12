//! Test di integrazione di mfc-midi: round trip, parsing SMF costruiti a mano,
//! esportazione della Playlist e importazione nel progetto.

use mfc_core::{ChannelKind, Command, Note, Outcome, PatternId, PluginInstance, Project};
use mfc_midi::{export_pattern, export_song, import_into_project, parse_midi, ImportedTrack, MidiImport};
use midly::num::{u15, u28, u4, u7};
use midly::{Format, Fps, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

fn add_channel(p: &mut Project, name: &str, plugin: &str) -> mfc_core::ChannelId {
    match (Command::AddChannel {
        name: name.into(),
        kind: ChannelKind::Generator,
        plugin: PluginInstance::new(plugin),
        mixer_insert: None,
    })
    .apply(p)
    .unwrap()
    {
        Outcome::Channel(id) => id,
        o => panic!("esito inatteso {o:?}"),
    }
}

fn add_notes(p: &mut Project, pat: PatternId, ch: mfc_core::ChannelId, notes: Vec<Note>) {
    Command::AddNotes {
        pattern: pat,
        channel: ch,
        notes,
    }
    .apply(p)
    .unwrap();
}

/// Progetto con due canali (melodico e batteria) e un pattern con note.
fn demo_project() -> (Project, PatternId, mfc_core::ChannelId, mfc_core::ChannelId) {
    let mut p = Project::new("Demo");
    let pat = p.patterns[0].id;
    let lead = add_channel(&mut p, "Lead", "mfc.3xosc");
    let kick = add_channel(&mut p, "Kick", "mfc.drumsynth");
    add_notes(
        &mut p,
        pat,
        lead,
        vec![
            Note::new(0, 96, 60, 1.0),
            Note::new(96, 48, 64, 0.5),
            Note::new(144, 96, 67, 0.25),
            Note::new(384, 96, 60, 0.7),
        ],
    );
    add_notes(
        &mut p,
        pat,
        kick,
        vec![Note::new(0, 24, 36, 0.9), Note::new(192, 24, 36, 0.9)],
    );
    (p, pat, lead, kick)
}

fn on(ch: u8, key: u8, vel: u8) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel: u4::from(ch),
        message: MidiMessage::NoteOn {
            key: u7::from(key),
            vel: u7::from(vel),
        },
    }
}

fn off(ch: u8, key: u8) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel: u4::from(ch),
        message: MidiMessage::NoteOff {
            key: u7::from(key),
            vel: u7::from(0),
        },
    }
}

fn ev(delta: u32, kind: TrackEventKind<'static>) -> TrackEvent<'static> {
    TrackEvent {
        delta: u28::from(delta),
        kind,
    }
}

fn end() -> TrackEvent<'static> {
    ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack))
}

/// Costruisce un SMF con le tracce indicate (ogni traccia termina con EndOfTrack).
fn build(format: Format, ppq: u16, tracks: Vec<Vec<TrackEvent<'static>>>) -> Vec<u8> {
    let mut smf = Smf::new(Header::new(format, Timing::Metrical(u15::from(ppq))));
    for mut t in tracks {
        t.push(end());
        smf.tracks.push(t);
    }
    let mut out = Vec::new();
    smf.write_std(&mut out).unwrap();
    out
}

fn track_by_name<'a>(imp: &'a MidiImport, name: &str) -> &'a ImportedTrack {
    imp.tracks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("traccia {name} non trovata"))
}

fn sorted(mut v: Vec<Note>) -> Vec<Note> {
    v.sort_by(|a, b| a.position.cmp(&b.position).then(a.key.cmp(&b.key)));
    v
}

#[test]
fn export_then_parse_roundtrip() {
    let (p, pat, lead, kick) = demo_project();
    let bytes = export_pattern(&p, pat).unwrap();
    let imp = parse_midi(&bytes, p.time.ppq).unwrap();

    // SMF memorizza microsecondi per battuta: tolleranza di arrotondamento.
    assert!((imp.tempo_bpm.unwrap() - p.tempo).abs() < 1e-3);
    assert_eq!(imp.time_signature, Some((4, 4)));
    assert_eq!(imp.tracks.len(), 2);

    for (cid, midi_ch) in [(lead, 0u8), (kick, 9u8)] {
        let name = p.channel(cid).unwrap().name.clone();
        let t = track_by_name(&imp, &name);
        assert_eq!(t.midi_channel, midi_ch, "canale MIDI di {name}");
        let orig = sorted(p.pattern(pat).unwrap().notes[&cid].clone());
        let got = sorted(t.notes.clone());
        assert_eq!(orig.len(), got.len(), "numero note di {name}");
        for (a, b) in orig.iter().zip(&got) {
            assert_eq!(a.position, b.position);
            assert_eq!(a.length, b.length);
            assert_eq!(a.key, b.key);
            assert!((a.velocity - b.velocity).abs() <= 1.0 / 127.0 + 1e-6, "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn format0_is_split_by_channel() {
    let track = vec![
        ev(0, TrackEventKind::Meta(MetaMessage::TrackName(b"Mixed"))),
        ev(0, on(0, 60, 100)),
        ev(0, on(2, 64, 100)),
        ev(96, off(0, 60)),
        ev(0, off(2, 64)),
    ];
    let bytes = build(Format::SingleTrack, 96, vec![track]);
    let imp = parse_midi(&bytes, 96).unwrap();
    assert_eq!(imp.tracks.len(), 2);
    let a = track_by_name(&imp, "Mixed ch1");
    let b = track_by_name(&imp, "Mixed ch3");
    assert_eq!((a.midi_channel, b.midi_channel), (0, 2));
    assert_eq!(a.notes.len(), 1);
    assert_eq!((a.notes[0].position, a.notes[0].length, a.notes[0].key), (0, 96, 60));
    assert_eq!((b.notes[0].position, b.notes[0].length, b.notes[0].key), (0, 96, 64));
}

#[test]
fn overlapping_same_key_notes_pair_fifo() {
    let track = vec![
        ev(0, on(0, 60, 100)),
        ev(48, on(0, 60, 90)),
        ev(48, off(0, 60)),
        ev(48, off(0, 60)),
    ];
    let imp = parse_midi(&build(Format::Parallel, 96, vec![track]), 96).unwrap();
    let notes = &imp.tracks[0].notes;
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0].position, notes[0].length), (0, 96));
    assert_eq!((notes[1].position, notes[1].length), (48, 96));
    assert!((notes[0].velocity - 100.0 / 127.0).abs() < 1e-6);
    assert!((notes[1].velocity - 90.0 / 127.0).abs() < 1e-6);
}

#[test]
fn ppq_is_rescaled_480_to_96() {
    let track = vec![ev(480, on(0, 60, 127)), ev(240, off(0, 60))];
    let imp = parse_midi(&build(Format::Parallel, 480, vec![track]), 96).unwrap();
    let n = imp.tracks[0].notes[0];
    assert_eq!((n.position, n.length), (96, 48));
    assert!((n.velocity - 1.0).abs() < 1e-6);
}

#[test]
fn noteon_velocity_zero_is_note_off() {
    let track = vec![ev(0, on(0, 60, 100)), ev(96, on(0, 60, 0))];
    let imp = parse_midi(&build(Format::Parallel, 96, vec![track]), 96).unwrap();
    assert_eq!(imp.tracks.len(), 1);
    let n = imp.tracks[0].notes[0];
    assert_eq!((n.position, n.length), (0, 96));
}

#[test]
fn zero_length_and_unterminated_notes() {
    // Nota con inizio e fine allo stesso tick → durata 1; nota aperta → fine all'ultimo evento.
    let track = vec![
        ev(0, on(0, 60, 100)),
        ev(0, off(0, 60)),
        ev(96, on(0, 64, 100)),
        ev(48, on(0, 67, 100)),
    ];
    let imp = parse_midi(&build(Format::Parallel, 96, vec![track]), 96).unwrap();
    let notes = sorted(imp.tracks[0].notes.clone());
    assert_eq!((notes[0].position, notes[0].length), (0, 1));
    assert_eq!((notes[1].position, notes[1].length), (96, 48));
    assert_eq!((notes[2].position, notes[2].length), (144, 1));
}

#[test]
fn empty_tracks_are_skipped_and_tempo_is_read() {
    let meta = vec![
        ev(0, TrackEventKind::Meta(MetaMessage::Tempo(u24_from(500_000)))),
        ev(0, TrackEventKind::Meta(MetaMessage::TimeSignature(3, 2, 24, 8))),
    ];
    let empty = vec![ev(0, TrackEventKind::Meta(MetaMessage::TrackName(b"Empty")))];
    let imp = parse_midi(&build(Format::Parallel, 96, vec![meta, empty]), 96).unwrap();
    assert!(imp.tracks.is_empty());
    assert_eq!(imp.tempo_bpm, Some(120.0));
    assert_eq!(imp.time_signature, Some((3, 4)));
}

fn u24_from(v: u32) -> midly::num::u24 {
    midly::num::u24::from(v)
}

#[test]
fn export_song_places_clips_at_their_positions() {
    let (mut p, pat, lead, _kick) = demo_project();
    Command::PlaceClip {
        track: 0,
        position: 0,
        length: None,
        source: mfc_core::ClipSource::Pattern(pat),
    }
    .apply(&mut p)
    .unwrap();
    Command::PlaceClip {
        track: 1,
        position: 768,
        length: None,
        source: mfc_core::ClipSource::Pattern(pat),
    }
    .apply(&mut p)
    .unwrap();
    let bytes = export_song(&p).unwrap();
    let imp = parse_midi(&bytes, p.time.ppq).unwrap();

    let name = p.channel(lead).unwrap().name.clone();
    let lead_notes = sorted(track_by_name(&imp, &name).notes.clone());
    let bar2 = p.time.ticks_per_bar() * 2;
    let positions: Vec<u64> = lead_notes.iter().map(|n| n.position).collect();
    let expected: Vec<u64> = [0, 96, 144, 384]
        .iter()
        .flat_map(|&o| [o, o + bar2])
        .collect::<Vec<_>>();
    let mut expected = expected;
    expected.sort();
    assert_eq!(positions, expected);
    assert_eq!(lead_notes.len(), 8);
}

#[test]
fn export_song_honors_offset_length_and_mute() {
    let mut p = Project::new("Clip");
    let pat = p.patterns[0].id;
    let lead = add_channel(&mut p, "Lead", "mfc.3xosc");
    add_notes(
        &mut p,
        pat,
        lead,
        vec![Note::new(40, 100, 60, 1.0), Note::new(120, 20, 64, 1.0)],
    );
    // Finestra [96, 192) della sorgente, posizionata a 0 nel brano.
    let Outcome::Clip(cid) = (Command::PlaceClip {
        track: 0,
        position: 0,
        length: Some(96),
        source: mfc_core::ClipSource::Pattern(pat),
    })
    .apply(&mut p)
    .unwrap() else {
        panic!()
    };
    Command::MoveClip {
        clip: cid,
        track: None,
        position: None,
        length: Some(96),
        offset: Some(96),
    }
    .apply(&mut p)
    .unwrap();
    let imp = parse_midi(&export_song(&p).unwrap(), p.time.ppq).unwrap();
    let notes = sorted(imp.tracks[0].notes.clone());
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0].position, notes[0].length, notes[0].key), (0, 44, 60));
    assert_eq!((notes[1].position, notes[1].length, notes[1].key), (24, 20, 64));

    Command::SetClipMuted { clip: cid, muted: true }.apply(&mut p).unwrap();
    let imp = parse_midi(&export_song(&p).unwrap(), p.time.ppq).unwrap();
    assert!(imp.tracks.is_empty());
}

#[test]
fn import_into_project_creates_pattern_and_channels() {
    let (src, pat, _, _) = demo_project();
    let imp = parse_midi(&export_pattern(&src, pat).unwrap(), src.time.ppq).unwrap();

    let mut dst = Project::new("Target");
    let before_patterns = dst.patterns.len();
    let (pid, channels) = import_into_project(&mut dst, &imp, "Imported").unwrap();

    assert_eq!(dst.patterns.len(), before_patterns + 1);
    assert_eq!(dst.pattern(pid).unwrap().name, "Imported");
    assert_eq!(channels.len(), imp.tracks.len());
    assert_eq!(dst.channels.len(), imp.tracks.len());
    assert!((dst.tempo - src.tempo).abs() < 1e-3);
    for (cid, t) in channels.iter().zip(&imp.tracks) {
        let ch = dst.channel(*cid).unwrap();
        assert_eq!(ch.name, t.name);
        let expected_plugin = if t.midi_channel == 9 {
            "mfc.drumsynth"
        } else {
            "mfc.3xosc"
        };
        assert_eq!(ch.plugin.kind, expected_plugin);
        assert_eq!(dst.pattern(pid).unwrap().notes[cid].len(), t.notes.len());
    }
}

#[test]
fn failed_import_leaves_project_untouched() {
    let mut p = Project::new("Keep");
    let before = p.clone();
    let imp = MidiImport {
        tempo_bpm: Some(130.0),
        time_signature: None,
        tracks: vec![ImportedTrack {
            name: "Bad".into(),
            midi_channel: 0,
            notes: vec![Note::new(0, 0, 60, 1.0)],
        }],
    };
    assert!(import_into_project(&mut p, &imp, "X").is_err());
    assert_eq!(p, before);
}

#[test]
fn garbage_and_truncated_input_is_error() {
    assert!(parse_midi(b"not a midi file", 96).is_err());
    assert!(parse_midi(&[], 96).is_err());
    let (p, pat, _, _) = demo_project();
    let bytes = export_pattern(&p, pat).unwrap();
    // midly è tollerante con i file troncati: basta che non vada in panic.
    let _ = parse_midi(&bytes[..bytes.len() / 2], 96);
    // Byte pseudo-casuali: nessun panic.
    let mut seed: u32 = 0x1234_5678;
    for len in 0..512usize {
        let buf: Vec<u8> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let _ = parse_midi(&buf, 96);
    }
}

#[test]
fn smpte_timing_is_unsupported() {
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Timecode(Fps::Fps25, 40)));
    smf.tracks.push(vec![end()]);
    let mut out = Vec::new();
    smf.write_std(&mut out).unwrap();
    assert!(matches!(parse_midi(&out, 96), Err(mfc_midi::MidiError::Unsupported(_))));
}

#[test]
fn export_rejects_unknown_pattern() {
    let p = Project::new("x");
    assert!(export_pattern(&p, PatternId(9999)).is_err());
}
