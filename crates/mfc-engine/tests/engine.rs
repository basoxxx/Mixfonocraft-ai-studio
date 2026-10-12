use mfc_core::*;
use mfc_engine::*;

fn add_channel(p: &mut Project, name: &str, plugin: &str, insert: u16) -> ChannelId {
    let inst = mfc_plugins::default_instance(plugin).unwrap();
    match (Command::AddChannel {
        name: name.into(),
        kind: ChannelKind::Generator,
        plugin: inst,
        mixer_insert: Some(insert),
    })
    .apply(p)
    .unwrap()
    {
        Outcome::Channel(c) => c,
        _ => unreachable!(),
    }
}

/// Progetto demo: kick su insert 1, basso su insert 2, 4 battute.
fn demo() -> (Project, ChannelId, ChannelId, PatternId) {
    let mut p = Project::new("demo");
    p.tempo = 120.0;
    let pat = p.patterns[0].id;
    let kick = add_channel(&mut p, "Kick", mfc_plugins::ids::KICK, 1);
    let bass = add_channel(&mut p, "Bass", mfc_plugins::ids::OSC3, 2);
    Command::SetStepRow {
        pattern: pat,
        channel: kick,
        row: "x...x...x...x...".into(),
    }
    .apply(&mut p)
    .unwrap();
    Command::AddNotes {
        pattern: pat,
        channel: bass,
        notes: vec![Note::new(0, 96, 36, 0.8), Note::new(192, 96, 43, 0.8)],
    }
    .apply(&mut p)
    .unwrap();
    for bar in 0..4 {
        Command::PlaceClip {
            track: 0,
            position: bar * 384,
            length: None,
            source: ClipSource::Pattern(pat),
        }
        .apply(&mut p)
        .unwrap();
    }
    (p, kick, bass, pat)
}

fn opts() -> RenderOptions {
    RenderOptions {
        sample_rate: 48_000,
        tail_seconds: 0.0,
        ..Default::default()
    }
}

#[test]
fn song_render_has_exact_length_and_signal() {
    let (p, ..) = demo();
    let out = render(&p, &opts()).unwrap();
    // 4 battute a 120 BPM = 8 s
    assert_eq!(out.mix.frames(), 8 * 48_000);
    assert!(out.mix.peak() > 0.05, "peak {}", out.mix.peak());
    assert!(out.mix.left.iter().all(|x| x.is_finite()));
}

#[test]
fn render_is_deterministic() {
    let (p, ..) = demo();
    let a = render(&p, &opts()).unwrap();
    let b = render(&p, &opts()).unwrap();
    assert_eq!(a.mix, b.mix);
}

#[test]
fn kick_onsets_are_sample_accurate() {
    let mut p = Project::new("onset");
    p.tempo = 120.0;
    let pat = p.patterns[0].id;
    let k = add_channel(&mut p, "Kick", mfc_plugins::ids::KICK, 0);
    // Step 4 → beat 2 → 0.5 s.
    Command::SetStep {
        pattern: pat,
        channel: k,
        step: 4,
        on: true,
        velocity: Some(1.0),
    }
    .apply(&mut p)
    .unwrap();
    Command::PlaceClip {
        track: 0,
        position: 0,
        length: None,
        source: ClipSource::Pattern(pat),
    }
    .apply(&mut p)
    .unwrap();
    let out = render(&p, &opts()).unwrap();
    let first = out.mix.left.iter().position(|x| x.abs() > 1e-4).unwrap();
    assert!((24_000..24_000 + 4).contains(&first), "onset a {first}");
}

#[test]
fn mute_and_solo() {
    let (mut p, ..) = demo();
    let full = render(&p, &opts()).unwrap();
    // Mute insert 1 (kick) e 2 (basso) → silenzio.
    for i in [1, 2] {
        Command::SetInsert {
            insert: i,
            name: None,
            volume: None,
            pan: None,
            stereo_separation: None,
            muted: Some(true),
            solo: None,
            phase_invert: None,
            swap_lr: None,
        }
        .apply(&mut p)
        .unwrap();
    }
    assert!(render(&p, &opts()).unwrap().mix.peak() < 1e-6);

    // Solo dell'insert 2: resta solo il basso.
    let (mut p, ..) = demo();
    Command::SetInsert {
        insert: 2,
        name: None,
        volume: None,
        pan: None,
        stereo_separation: None,
        muted: None,
        solo: Some(true),
        phase_invert: None,
        swap_lr: None,
    }
    .apply(&mut p)
    .unwrap();
    let solo = render(&p, &RenderOptions { stems: true, ..opts() }).unwrap();
    assert!(solo.mix.peak() > 0.01);
    assert_ne!(solo.mix, full.mix);
    assert!(
        solo.stems.iter().all(|s| s.insert == 2),
        "{:?}",
        solo.stems.iter().map(|s| s.insert).collect::<Vec<_>>()
    );
}

#[test]
fn routing_through_insert_chain_with_effects() {
    let (mut p, ..) = demo();
    // 1 → 3 → master, con riverbero su 3.
    Command::SetRoute {
        from: 1,
        to: 0,
        gain: None,
    }
    .apply(&mut p)
    .unwrap();
    Command::SetRoute {
        from: 1,
        to: 3,
        gain: Some(1.0),
    }
    .apply(&mut p)
    .unwrap();
    let rev = mfc_plugins::default_instance(mfc_plugins::ids::REVERB).unwrap();
    Command::AddEffect {
        insert: 3,
        plugin: rev,
        slot: None,
    }
    .apply(&mut p)
    .unwrap();
    let out = render(
        &p,
        &RenderOptions {
            stems: true,
            tail_seconds: 1.0,
            ..opts()
        },
    )
    .unwrap();
    let ins: Vec<u16> = out.stems.iter().map(|s| s.insert).collect();
    assert!(ins.contains(&1) && ins.contains(&2) && ins.contains(&3), "{ins:?}");
    for s in &out.stems {
        assert_eq!(s.audio.frames(), out.mix.frames());
    }
    // La coda del riverbero continua dopo la fine del brano.
    let tail = &out.mix.left[8 * 48_000..];
    assert!(tail.iter().any(|x| x.abs() > 1e-5));
}

#[test]
fn pattern_mode_loops() {
    let (p, _, _, pat) = demo();
    let o = RenderOptions {
        pattern: Some(pat),
        pattern_loops: 3,
        ..opts()
    };
    let out = render(&p, &o).unwrap();
    assert_eq!(out.mix.frames(), 3 * 2 * 48_000);
    // Ogni giro ha la stessa energia (gli oscillatori sono free-running, quindi la forma
    // d'onda non è identica campione per campione).
    let n = 2 * 48_000;
    let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
    let r: Vec<f32> = (0..3).map(|i| rms(&out.mix.left[i * n..(i + 1) * n])).collect();
    for x in &r[1..] {
        assert!((x / r[0] - 1.0).abs() < 0.05, "rms per giro {r:?}");
    }
    // E il kick riparte esattamente all'inizio di ogni giro.
    let (mut p2, _, bass, pat2) = demo();
    Command::RemoveChannel { channel: bass }.apply(&mut p2).unwrap();
    let out = render(
        &p2,
        &RenderOptions {
            pattern: Some(pat2),
            pattern_loops: 2,
            ..opts()
        },
    )
    .unwrap();
    // Stesso onset (al campione) e stessa energia: il click usa rumore free-running.
    let l = &out.mix.left;
    let onset = |s: &[f32]| s.iter().position(|x| x.abs() > 1e-2);
    let (oa, ob) = (onset(&l[..n]).unwrap(), onset(&l[n..2 * n]).unwrap());
    assert!(oa.abs_diff(ob) <= 2, "onset {oa} vs {ob}");
    let (ra, rb) = (rms(&l[..n / 8]), rms(&l[n..n + n / 8]));
    assert!((rb / ra - 1.0).abs() < 0.05, "rms kick {ra} vs {rb}");
}

#[test]
fn automation_master_volume_fades_out() {
    let (mut p, ..) = demo();
    let Outcome::Automation(a) = Command::AddAutomation {
        name: "fade".into(),
        target: AutomationTarget::MasterVolume,
        min: 0.0,
        max: MIXER_UNITY,
        points: vec![
            AutomationPoint {
                position: 0,
                value: 1.0,
                tension: 0.0,
            },
            AutomationPoint {
                position: 1536,
                value: 0.0,
                tension: 0.0,
            },
        ],
    }
    .apply(&mut p)
    .unwrap() else {
        panic!()
    };
    Command::PlaceClip {
        track: 1,
        position: 0,
        length: Some(1536),
        source: ClipSource::Automation(a),
    }
    .apply(&mut p)
    .unwrap();
    let out = render(&p, &opts()).unwrap();
    let sec = 48_000;
    let peak = |s: &[f32]| s.iter().fold(0f32, |m, x| m.max(x.abs()));
    let first = peak(&out.mix.left[..2 * sec]);
    let last = peak(&out.mix.left[6 * sec..8 * sec]);
    assert!(last < first * 0.5, "first {first} last {last}");
}

#[test]
fn tempo_automation_changes_length() {
    let (mut p, ..) = demo();
    let Outcome::Automation(a) = Command::AddAutomation {
        name: "tempo".into(),
        target: AutomationTarget::Tempo,
        min: 60.0,
        max: 240.0,
        // valore costante 240 BPM → la metà del tempo.
        points: vec![AutomationPoint {
            position: 0,
            value: 1.0,
            tension: 0.0,
        }],
    }
    .apply(&mut p)
    .unwrap() else {
        panic!()
    };
    Command::PlaceClip {
        track: 1,
        position: 0,
        length: Some(1536),
        source: ClipSource::Automation(a),
    }
    .apply(&mut p)
    .unwrap();
    let out = render(&p, &opts()).unwrap();
    let secs = out.mix.seconds();
    assert!((secs - 4.0).abs() < 0.05, "{secs}");
}

#[test]
fn live_install_keeps_transport() {
    let (p, ..) = demo();
    let (mut eng, handle) = Engine::new(&p, 48_000.0);
    eng.play();
    let mut l = vec![0.0; 512];
    let mut r = vec![0.0; 512];
    for _ in 0..50 {
        eng.process(&mut l, &mut r);
    }
    let before = eng.position();
    let mut p2 = p.clone();
    Command::SetTempo { bpm: 90.0 }.apply(&mut p2).unwrap();
    assert!(handle.load_project(&p2));
    eng.process(&mut l, &mut r);
    assert!(eng.position() >= before);
    handle.collect_garbage();
    assert_eq!(handle.shared.tempo(), 90.0);
    assert!(handle.shared.is_playing());
}

#[test]
fn preview_notes_sound_while_stopped() {
    let (p, kick, ..) = demo();
    let (mut eng, handle) = Engine::new(&p, 48_000.0);
    handle.preview_on(kick, 60, 1.0);
    let mut l = vec![0.0; 2048];
    let mut r = vec![0.0; 2048];
    eng.process(&mut l, &mut r);
    assert!(l.iter().any(|x| x.abs() > 1e-3));
    assert!(!eng.is_playing());
}

#[test]
fn wav_writer_roundtrip() {
    let (p, ..) = demo();
    let dir = std::env::temp_dir().join(format!("mfc_engine_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mix.wav");
    let out = render_to_file(&p, &path, &RenderOptions { stems: true, ..opts() }, WavFormat::Int16).unwrap();
    let reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.duration() as usize, out.mix.frames());
    let stems = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(stems, 1 + out.stems.len());
    std::fs::remove_dir_all(dir).ok();
}
