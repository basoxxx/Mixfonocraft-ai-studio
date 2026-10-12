//! Test semantici: Project → FLP → bytes → FLP → Project.

use mfc_core::{
    Channel, ChannelId, ChannelKind, ClipId, ClipSource, Color, EffectSlot, Note, Pattern, PatternId, PlaylistClip,
    PlaylistTrack, PluginInstance, Project, Route, TimeBase,
};
use mfc_flp::{from_project, id, parse, to_project, write, FlpFile, RawEvent, TextEncoding};

struct Fixture {
    project: Project,
    kick: ChannelId,
    bass: ChannelId,
    clip: ChannelId,
    p1: PatternId,
    p2: PatternId,
}

fn build() -> Fixture {
    let mut p = Project::new("Demo FLP");
    p.info.artist = "Mixfonocraft".into();
    p.info.genre = "Techno".into();
    p.info.comments = "Commento con accenti: è ò".into();
    p.info.url = "https://example.org".into();
    p.tempo = 128.0;
    p.time = TimeBase {
        ppq: 96,
        ..Default::default()
    };
    p.master_pitch = -50.0;

    let kick = ChannelId(p.alloc_id());
    let mut sampler = PluginInstance::new("mfc.sampler");
    sampler.sample_path = Some("C:\\Samples\\Kick 01.wav".into());
    let mut k = Channel::new(kick, "Kick", ChannelKind::Sampler, sampler);
    k.mixer_insert = 2;
    k.volume = 0.6;
    k.pan = -0.25;
    k.color = Color(200, 40, 40);
    k.root_note = 48;
    p.channels.push(k);

    let bass = ChannelId(p.alloc_id());
    let mut b = Channel::new(bass, "Bass", ChannelKind::Generator, PluginInstance::new("mfc.3xosc"));
    b.mixer_insert = 3;
    b.pitch_cents = 100.0;
    b.muted = true;
    p.channels.push(b);

    let clip = ChannelId(p.alloc_id());
    let mut ac = PluginInstance::new("mfc.audioclip");
    ac.sample_path = Some("/audio/vox.wav".into());
    p.channels.push(Channel::new(clip, "Vox", ChannelKind::AudioClip, ac));

    let p1 = p.patterns[0].id;
    p.patterns[0].name = "Beat".into();
    p.patterns[0].color = Color(10, 20, 30);
    for i in 0..4u64 {
        p.patterns[0]
            .notes
            .entry(kick)
            .or_default()
            .push(Note::new(i * 96, 24, 60, 100.0 / 128.0));
    }
    let p2 = PatternId(p.alloc_id());
    let mut pat2 = Pattern::new(p2, "Bassline");
    let mut slide = Note::new(96, 48, 38, 0.5);
    slide.slide = true;
    slide.pan = 0.5;
    slide.fine_pitch = -20.0;
    slide.release = 0.25;
    slide.mod_x = 0.75;
    let notes = pat2.notes.entry(bass).or_default();
    notes.push(Note::new(0, 96, 36, 1.0));
    notes.push(slide);
    p.patterns.push(pat2);
    p.current_pattern = Some(p2);

    let add_clip = |p: &mut Project, track, position, length, source, muted| {
        let id = ClipId(p.alloc_id());
        p.playlist.clips.push(PlaylistClip {
            id,
            track,
            position,
            length,
            offset: 0,
            source,
            muted,
        });
    };
    add_clip(&mut p, 0, 0, 384, ClipSource::Pattern(p1), false);
    add_clip(&mut p, 0, 384, 384, ClipSource::Pattern(p1), false);
    add_clip(&mut p, 1, 384, 768, ClipSource::Pattern(p2), true);
    add_clip(&mut p, 4, 768, 192, ClipSource::Audio(clip), false);
    p.playlist.tracks.insert(
        1,
        PlaylistTrack {
            name: "Bass track".into(),
            color: Color(1, 2, 3),
            muted: false,
        },
    );
    p.playlist.tracks.insert(
        4,
        PlaylistTrack {
            name: String::new(),
            color: Color(9, 9, 9),
            muted: true,
        },
    );

    // Mixer: canale → insert 2 → master; insert 3 → insert 2 (mandata a metà).
    let m = &mut p.mixer.inserts;
    m[2].name = "Drums".into();
    m[2].volume = 0.5;
    m[2].pan = 0.25;
    m[2].solo = true;
    m[2].effects.push(EffectSlot::new(PluginInstance::new("mfc.reverb")));
    let mut d = EffectSlot::new(PluginInstance::new("mfc.delay"));
    d.enabled = false;
    d.mix = 0.5;
    m[2].effects.push(d);
    m[3].name = "Bass bus".into();
    m[3].muted = true;
    m[3].stereo_separation = -0.5;
    m[3].routes = vec![Route { target: 2, gain: 0.5 }];
    m[0].volume = 0.7;
    Fixture {
        project: p,
        kick,
        bass,
        clip,
        p1,
        p2,
    }
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn export_import_roundtrip() {
    let fx = build();
    let src = &fx.project;
    let flp = from_project(src);
    let bytes = write(&flp);
    let parsed = parse(&bytes).unwrap();
    assert_eq!(parsed, flp);
    assert_eq!(write(&parsed), bytes, "ri-scrittura non byte-identica");
    assert_eq!(parsed.header.n_channels, 3);
    assert_eq!(parsed.header.ppq, 96);

    let res = to_project(&parsed).unwrap();
    let p = &res.project;

    // Metadati.
    assert_eq!(p.info.title, "Demo FLP");
    assert_eq!(p.info.artist, "Mixfonocraft");
    assert_eq!(p.info.genre, "Techno");
    assert_eq!(p.info.comments, src.info.comments);
    assert_eq!(p.info.url, src.info.url);
    assert_eq!(p.info.fl_version.as_deref(), Some(mfc_flp::EXPORT_VERSION));
    assert_eq!(p.tempo, 128.0);
    assert_eq!(p.time.ppq, 96);
    assert_eq!(p.time.signature, src.time.signature);
    assert_eq!(p.master_pitch, -50.0);

    // Canali.
    assert_eq!(p.channels.len(), 3);
    let (k, b, c) = (&p.channels[0], &p.channels[1], &p.channels[2]);
    assert_eq!((k.name.as_str(), k.kind), ("Kick", ChannelKind::Sampler));
    assert_eq!(k.plugin.kind, "mfc.sampler");
    assert_eq!(k.plugin.sample_path.as_deref(), Some("C:\\Samples\\Kick 01.wav"));
    assert_eq!(k.mixer_insert, 2);
    assert!(close(k.volume, 0.6, 1e-3));
    assert!(close(k.pan, -0.25, 1e-3));
    assert_eq!(k.color, Color(200, 40, 40));
    assert_eq!(k.root_note, 48);
    assert!(!k.muted);
    assert_eq!((b.name.as_str(), b.kind), ("Bass", ChannelKind::Generator));
    assert_eq!(b.plugin.kind, "mfc.3xosc");
    assert_eq!(b.mixer_insert, 3);
    assert_eq!(b.pitch_cents, 100.0);
    assert!(b.muted);
    assert_eq!((c.name.as_str(), c.kind), ("Vox", ChannelKind::AudioClip));
    assert_eq!(c.plugin.sample_path.as_deref(), Some("/audio/vox.wav"));
    let cmap = [(fx.kick, k.id), (fx.bass, b.id), (fx.clip, c.id)];
    let map_c = |id: ChannelId| cmap.iter().find(|(a, _)| *a == id).unwrap().1;

    // Pattern e note.
    assert_eq!(p.patterns.len(), 2);
    let (q1, q2) = (&p.patterns[0], &p.patterns[1]);
    assert_eq!(q1.name, "Beat");
    assert_eq!(q1.color, Color(10, 20, 30));
    assert_eq!(q2.name, "Bassline");
    assert_eq!(p.current_pattern, Some(q2.id));
    let pmap = [(fx.p1, q1.id), (fx.p2, q2.id)];
    for (orig, new) in pmap {
        let a = src.pattern(orig).unwrap();
        let bpat = p.pattern(new).unwrap();
        assert_eq!(a.notes.len(), bpat.notes.len());
        for (cid, notes) in &a.notes {
            let got = &bpat.notes[&map_c(*cid)];
            assert_eq!(notes.len(), got.len());
            for (x, y) in notes.iter().zip(got) {
                assert_eq!(
                    (x.position, x.length, x.key, x.slide),
                    (y.position, y.length, y.key, y.slide)
                );
                assert!(close(x.velocity, y.velocity, 1.0 / 128.0));
                assert!(close(x.pan, y.pan, 1.0 / 64.0));
                assert!(close(x.fine_pitch, y.fine_pitch, 1.0));
                assert!(close(x.release, y.release, 1.0 / 128.0));
                assert!(close(x.mod_x, y.mod_x, 1.0 / 128.0));
            }
        }
    }
    assert!(q2.notes[&b.id].iter().any(|n| n.slide));

    // Playlist.
    assert_eq!(p.playlist.clips.len(), 4);
    let mut got: Vec<_> = p
        .playlist
        .clips
        .iter()
        .map(|c| (c.track, c.position, c.length, c.source, c.muted))
        .collect();
    got.sort_by_key(|x| (x.0, x.1));
    let mut want: Vec<_> = src
        .playlist
        .clips
        .iter()
        .map(|c| {
            let s = match c.source {
                ClipSource::Pattern(id) => ClipSource::Pattern(pmap.iter().find(|x| x.0 == id).unwrap().1),
                ClipSource::Audio(id) => ClipSource::Audio(map_c(id)),
                o => o,
            };
            (c.track, c.position, c.length, s, c.muted)
        })
        .collect();
    want.sort_by_key(|x| (x.0, x.1));
    assert_eq!(got, want);
    assert_eq!(p.playlist.tracks[&1].name, "Bass track");
    assert_eq!(p.playlist.tracks[&1].color, Color(1, 2, 3));
    assert!(p.playlist.tracks[&4].muted);
    assert_eq!(p.playlist.tracks.len(), 2);

    // Mixer.
    let m = &p.mixer.inserts;
    assert_eq!(m.len(), mfc_core::MIXER_INSERTS + 1);
    assert_eq!(m[2].name, "Drums");
    assert_eq!(m[3].name, "Bass bus");
    assert_eq!(m[5].name, "Insert 5");
    assert_eq!(m[0].name, "Master");
    assert!(close(m[2].volume, 0.5, 1e-3));
    assert!(close(m[0].volume, 0.7, 1e-3));
    assert!(close(m[2].pan, 0.25, 1e-3));
    assert!(m[2].solo && !m[2].muted);
    assert!(m[3].muted);
    assert!(close(m[3].stereo_separation, -0.5, 1.0 / 64.0));
    assert_eq!(m[2].routes, vec![Route { target: 0, gain: 1.0 }]);
    assert_eq!(m[3].routes, vec![Route { target: 2, gain: 0.5 }]);
    assert!(m[0].routes.is_empty());
    assert_eq!(m[2].effects.len(), 2);
    assert_eq!(m[2].effects[0].plugin.kind, "mfc.reverb");
    assert!(m[2].effects[0].enabled);
    assert!(close(m[2].effects[0].mix, 1.0, 1e-3));
    assert_eq!(m[2].effects[1].plugin.kind, "mfc.delay");
    assert!(!m[2].effects[1].enabled);
    assert!(close(m[2].effects[1].mix, 0.5, 1e-3));
    assert!(m[4].effects.is_empty());

    // Nessun evento esportato resta non mappato.
    assert!(
        res.warnings.iter().all(|w| !w.contains("non mappati")),
        "{:?}",
        res.warnings
    );

    // Seconda passata: export dell'import è stabile.
    let again = write(&from_project(p));
    let p3 = to_project(&parse(&again).unwrap()).unwrap().project;
    assert_eq!(p3.channels.len(), 3);
    assert_eq!(p3.playlist.clips.len(), 4);
    assert_eq!(p3.mixer.inserts[3].routes, m[3].routes);
}

#[test]
fn foreign_plugins_are_preserved() {
    let mut p = Project::new("Foreign");
    let cid = ChannelId(p.alloc_id());
    let mut pi = PluginInstance::new("mfc.silent");
    pi.foreign_name = Some("Fruity Wrapper".into());
    pi.opaque_state = Some(vec![1, 2, 3, 4, 5]);
    p.channels.push(Channel::new(cid, "Serum", ChannelKind::Generator, pi));
    let mut fx = PluginInstance::new("foreign");
    fx.foreign_name = Some("Fruity Vocoder".into());
    fx.opaque_state = Some(vec![9; 300]);
    p.mixer.inserts[1].effects.push(EffectSlot::new(fx));

    let res = to_project(&parse(&write(&from_project(&p))).unwrap()).unwrap();
    let ch = &res.project.channels[0];
    assert_eq!(ch.name, "Serum");
    assert_eq!(ch.plugin.kind, "mfc.silent");
    assert_eq!(ch.plugin.foreign_name.as_deref(), Some("Fruity Wrapper"));
    assert_eq!(ch.plugin.opaque_state.as_deref(), Some(&[1u8, 2, 3, 4, 5][..]));
    let slot = &res.project.mixer.inserts[1].effects[0];
    assert_eq!(slot.plugin.kind, "foreign");
    assert!(!slot.enabled);
    assert_eq!(slot.plugin.foreign_name.as_deref(), Some("Fruity Vocoder"));
    assert_eq!(slot.plugin.opaque_state.as_ref().map(Vec::len), Some(300));
    assert!(res.warnings.iter().any(|w| w.contains("Fruity Wrapper")));
    assert!(res.warnings.iter().any(|w| w.contains("Fruity Vocoder")));
}

/// File "vecchio" (FL 9) con testo ASCII, tempo coarse/fine ed eventi ignoti.
#[test]
fn ascii_text_by_version_and_unmapped_warnings() {
    let enc = TextEncoding::Ascii;
    let f = FlpFile {
        header: mfc_flp::FlpHeader {
            format: 0,
            n_channels: 1,
            ppq: 192,
        },
        events: vec![
            RawEvent::text(id::FL_VERSION, "9.0.3", enc),
            RawEvent::num(id::TEMPO_COARSE, 90),
            RawEvent::num(id::TEMPO_FINE, 500),
            RawEvent::text(id::TITLE, "Vecchio progetto", enc),
            RawEvent::num(id::CHAN_NEW, 0),
            RawEvent::num(id::CHAN_TYPE, 0),
            RawEvent::text(id::CHAN_NAME_OLD, "Snare", enc),
            RawEvent::num(id::CHAN_VOL_WORD, 6400),
            RawEvent::num(id::PAT_NEW, 1),
            RawEvent::data(id::PAT_NOTES, {
                let mut d = Vec::new();
                mfc_flp::structs::FlNote {
                    position: 192,
                    flags: 0,
                    rack_channel: 0,
                    length: 0,
                    key: 62,
                    group: 0,
                    fine_pitch: 120,
                    u1: 0,
                    release: 64,
                    midi_channel: 0,
                    pan: 64,
                    velocity: 128,
                    mod_x: 128,
                    mod_y: 128,
                }
                .write(&mut d);
                d
            }),
            RawEvent::num(42, 7),
            RawEvent::data(250, vec![1, 2]),
        ],
        trailing: Vec::new(),
    };
    assert_eq!(f.text_encoding(), TextEncoding::Ascii);
    let bytes = write(&f);
    assert_eq!(write(&parse(&bytes).unwrap()), bytes);
    let res = to_project(&f).unwrap();
    let p = &res.project;
    assert_eq!(p.info.title, "Vecchio progetto");
    assert_eq!(p.tempo, 90.5);
    assert_eq!(p.time.ppq, 192);
    assert_eq!(p.channels[0].name, "Snare");
    assert!(close(p.channels[0].volume, 0.5, 1e-4));
    let n = &p.patterns[0].notes[&p.channels[0].id][0];
    // Nota da step sequencer (lunghezza 0) → 1 step.
    assert_eq!((n.position, n.length, n.key), (192, 48, 62));
    assert_eq!(n.velocity, 1.0);
    assert!(res.warnings.iter().any(|w| w.contains("42×1") && w.contains("250×1")));

    // Lo stesso titolo letto come UTF-16 sarebbe diverso: la codifica conta.
    let utf = mfc_flp::encode_text("Vecchio progetto", TextEncoding::Utf16);
    assert_ne!(utf, mfc_flp::encode_text("Vecchio progetto", enc));
    assert_eq!(mfc_flp::decode_text(&utf, TextEncoding::Utf16), "Vecchio progetto");
}

#[test]
fn utf16_titles_from_exported_file() {
    let mut p = Project::new("Ελληνικά ♫ 日本");
    p.info.genre = "Città".into();
    let f = from_project(&p);
    assert_eq!(f.text_encoding(), TextEncoding::Utf16);
    assert_eq!(f.text(id::TITLE).as_deref(), Some("Ελληνικά ♫ 日本"));
    let res = to_project(&parse(&write(&f)).unwrap()).unwrap();
    assert_eq!(res.project.info.title, "Ελληνικά ♫ 日本");
    assert_eq!(res.project.info.genre, "Città");
}

#[test]
fn empty_file_imports_with_defaults() {
    let res = to_project(&FlpFile::default()).unwrap();
    assert_eq!(res.project.patterns.len(), 1);
    assert!(res.project.channels.is_empty());
    assert!(!res.warnings.is_empty());
}

#[test]
fn read_and_write_file_helpers() {
    let dir = std::env::temp_dir().join(format!("mfc-flp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("demo.flp");
    let fx = build();
    mfc_flp::write_file(&path, &fx.project).unwrap();
    let (file, res) = mfc_flp::read_file(&path).unwrap();
    assert_eq!(write(&file), std::fs::read(&path).unwrap());
    assert_eq!(res.project.channels.len(), 3);
    assert!(mfc_flp::read_file(dir.join("missing.flp")).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
