//! Synth Nova: intonazione, preset, mono/legato, unison, filtro.

use mfc_plugins::{create_generator, default_instance, descriptor, ids, preset_values, presets, Generator, NoteOn};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;

fn make(params: &[(&str, f32)]) -> Box<dyn Generator> {
    let mut inst = default_instance(ids::NOVA).unwrap();
    for (k, v) in params {
        inst.params.insert((*k).to_string(), *v);
    }
    create_generator(&inst, SR)
}

fn render(g: &mut dyn Generator, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (seconds * SR) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    for (bl, br) in l.chunks_mut(BLOCK).zip(r.chunks_mut(BLOCK)) {
        g.process(bl, br);
    }
    (l, r)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn zero_cross_freq(x: &[f32]) -> f32 {
    let mut first = None;
    let mut last = 0;
    let mut count = 0usize;
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            if first.is_none() {
                first = Some(i);
            } else {
                count += 1;
            }
            last = i;
        }
    }
    match first {
        Some(f) if count > 0 => count as f32 * SR / (last - f) as f32,
        _ => 0.0,
    }
}

/// Sinusoide pura da osc 1: filtro aperto, niente osc 2.
const PURE: &[(&str, f32)] = &[
    ("osc1_wave", 0.0),
    ("osc2_level", 0.0),
    ("flt_cutoff", 20_000.0),
    ("flt_env", 0.0),
];

#[test]
fn tuning_follows_key_octave_and_semitones() {
    for (extra, key, hz) in [
        (vec![], 69u8, 440.0f32),
        (vec![("osc1_oct", 4.0)], 69, 880.0),
        (vec![("osc1_oct", 2.0)], 69, 220.0),
        (vec![("osc1_semi", 7.0)], 57, 329.63),
    ] {
        let mut params = PURE.to_vec();
        params.extend(extra.iter().copied());
        let mut g = make(&params);
        g.note_on(NoteOn::simple(1, key, 1.0));
        let (l, _) = render(g.as_mut(), 0.5);
        let f = zero_cross_freq(&l[4800..]);
        assert!((f - hz).abs() / hz < 0.01, "{params:?}: {f} Hz invece di {hz}");
    }
}

#[test]
fn pitch_offset_bends() {
    let mut g = make(PURE);
    g.set_pitch_offset(1200.0);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, _) = render(g.as_mut(), 0.3);
    let f = zero_cross_freq(&l[4800..]);
    assert!((f - 880.0).abs() < 9.0, "{f}");
}

#[test]
fn every_preset_plays_and_releases() {
    let desc = descriptor(ids::NOVA).unwrap();
    let list = presets(ids::NOVA);
    assert!(list.len() >= 12);
    for p in list {
        for (k, v) in p.values {
            let d = desc
                .param(k)
                .unwrap_or_else(|| panic!("{}: parametro sconosciuto {k}", p.name));
            assert!(*v >= d.min && *v <= d.max, "{}: {k}={v} fuori range", p.name);
        }
        let values = preset_values(desc, p);
        assert_eq!(values.len(), desc.params.len());
        let mut g = make(&values);
        // Accordo di tre note.
        for (i, k) in [48u8, 52, 55].iter().enumerate() {
            g.note_on(NoteOn::simple(i as u64 + 1, *k, 0.9));
        }
        let (l, r) = render(g.as_mut(), 1.0);
        assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{}: NaN", p.name);
        let pk = peak(&l).max(peak(&r));
        assert!(pk > 0.01 && pk < 2.0, "{}: picco {pk}", p.name);
        for i in 1..=3 {
            g.note_off(i);
        }
        let mut blocks = 0;
        while g.active_voices() > 0 {
            render(g.as_mut(), BLOCK as f32 / SR);
            blocks += 1;
            assert!(blocks < (12.0 * SR) as usize / BLOCK, "{}: voci infinite", p.name);
        }
    }
}

#[test]
fn mono_mode_uses_one_voice_and_returns_to_held_note() {
    let mut params = PURE.to_vec();
    params.push(("voice_mode", 1.0));
    let mut g = make(&params);
    g.note_on(NoteOn::simple(1, 57, 1.0));
    g.note_on(NoteOn::simple(2, 69, 1.0));
    assert_eq!(g.active_voices(), 1);
    let (l, _) = render(g.as_mut(), 0.3);
    assert!((zero_cross_freq(&l[4800..]) - 440.0).abs() < 5.0);
    // Rilascio della nota alta: torna a quella ancora tenuta (220 Hz).
    g.note_off(2);
    let (l, _) = render(g.as_mut(), 0.3);
    assert!(peak(&l) > 0.05, "la voce non deve spegnersi");
    assert!((zero_cross_freq(&l[4800..]) - 220.0).abs() < 3.0);
    g.note_off(1);
    render(g.as_mut(), 1.0);
    assert_eq!(g.active_voices(), 0);
}

#[test]
fn glide_moves_pitch_gradually() {
    let mut params = PURE.to_vec();
    params.extend([("voice_mode", 2.0), ("voice_glide", 0.2)]);
    let mut g = make(&params);
    g.note_on(NoteOn::simple(1, 57, 1.0));
    render(g.as_mut(), 0.2);
    g.note_on(NoteOn::simple(2, 69, 1.0));
    let (l, _) = render(g.as_mut(), 0.05);
    let early = zero_cross_freq(&l);
    assert!(early > 225.0 && early < 400.0, "a metà glide: {early}");
    let (l, _) = render(g.as_mut(), 1.5);
    assert!((zero_cross_freq(&l[48_000..]) - 440.0).abs() < 5.0);
}

#[test]
fn unison_spread_makes_stereo() {
    let mut g = make(&[("osc1_unison", 6.0), ("osc1_spread", 1.0), ("osc2_level", 0.0)]);
    g.note_on(NoteOn::simple(1, 60, 1.0));
    let (l, r) = render(g.as_mut(), 0.5);
    let diff: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
    assert!(rms(&diff) > 0.1 * rms(&l), "unison senza spread stereo");
    let mut mono = make(&[("osc2_level", 0.0)]);
    mono.note_on(NoteOn::simple(1, 60, 1.0));
    let (l, r) = render(mono.as_mut(), 0.5);
    let diff: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
    assert!(rms(&diff) < 1e-4, "una voce deve essere centrata");
}

#[test]
fn filter_cutoff_and_envelope_shape_brightness() {
    // Energia delle alte frequenze stimata dalla differenza tra campioni consecutivi.
    let hf = |params: &[(&str, f32)]| {
        let mut g = make(params);
        g.note_on(NoteOn::simple(1, 48, 1.0));
        let (l, _) = render(g.as_mut(), 0.4);
        let d: Vec<f32> = l.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&d[2400..]) / rms(&l[2400..]).max(1e-9)
    };
    let dark = hf(&[("flt_cutoff", 300.0), ("flt_env", 0.0)]);
    let bright = hf(&[("flt_cutoff", 8_000.0), ("flt_env", 0.0)]);
    assert!(bright > dark * 2.0, "cutoff: {dark} vs {bright}");
    let env = hf(&[("flt_cutoff", 300.0), ("flt_env", 1.0), ("fenv_sustain", 1.0)]);
    assert!(env > dark * 2.0, "env amount: {dark} vs {env}");
    let steep = hf(&[("flt_cutoff", 300.0), ("flt_env", 0.0), ("flt_slope", 1.0)]);
    assert!(steep < dark, "24 dB più ripido: {dark} vs {steep}");
}

#[test]
fn lfo_volume_makes_tremolo() {
    let mut g = make(&[
        ("osc1_wave", 0.0),
        ("osc2_level", 0.0),
        ("lfo1_dest", 4.0),
        ("lfo1_amount", 1.0),
        ("lfo1_rate", 4.0),
        ("aenv_sustain", 1.0),
    ]);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, _) = render(g.as_mut(), 1.0);
    let win = 1200; // 25 ms
    let levels: Vec<f32> = l[4800..].chunks(win).map(peak).collect();
    let (lo, hi) = levels
        .iter()
        .fold((f32::MAX, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(lo < hi * 0.3, "tremolo assente: {lo} {hi}");
}

/// Numero di discese del livello (inviluppo a finestre da 5 ms) sotto la metà dell'escursione.
fn tremolo_cycles(x: &[f32]) -> usize {
    let levels: Vec<f32> = x.chunks(240).map(peak).collect();
    let (lo, hi) = levels
        .iter()
        .fold((f32::MAX, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
    let mid = 0.5 * (lo + hi);
    levels.windows(2).filter(|w| w[0] >= mid && w[1] < mid).count()
}

fn sync_index(label: &str) -> f32 {
    mfc_plugins::sync::SYNC_CHOICES
        .iter()
        .position(|c| *c == label)
        .unwrap() as f32
}

/// Tremolo sinusoidale a profondità piena; il rate (4 Hz) va ignorato con il sync attivo.
fn tremolo(sync: f32) -> Box<dyn Generator> {
    make(&[
        ("osc1_wave", 0.0),
        ("osc2_level", 0.0),
        ("lfo1_shape", 0.0),
        ("lfo1_dest", 4.0),
        ("lfo1_amount", 1.0),
        ("lfo1_rate", 4.0),
        ("lfo1_sync", sync),
        ("aenv_sustain", 1.0),
    ])
}

#[test]
fn synced_lfo_period_follows_tempo() {
    // 1/4 a 120 BPM = 2 Hz → 8 cicli in 4 s; a 60 BPM → 4; 1/8 a 120 → 16.
    for (label, bpm, cycles) in [("1/4", 120.0, 8usize), ("1/4", 60.0, 4), ("1/8", 120.0, 16)] {
        let mut g = tremolo(sync_index(label));
        g.set_tempo(bpm);
        g.note_on(NoteOn::simple(1, 69, 1.0));
        let (l, _) = render(g.as_mut(), 4.0);
        let n = tremolo_cycles(&l);
        assert!(
            n.abs_diff(cycles) <= 1,
            "{label} a {bpm} BPM: {n} cicli invece di {cycles}"
        );
    }
    // Sync Off: vale il rate (4 Hz → 16 cicli), indipendente dal tempo.
    let mut g = tremolo(0.0);
    g.set_tempo(60.0);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, _) = render(g.as_mut(), 4.0);
    let n = tremolo_cycles(&l);
    assert!(n.abs_diff(16) <= 1, "{n}");
}

#[test]
fn wobble_preset_is_synced_to_eighths() {
    let p = presets(ids::NOVA).iter().find(|p| p.name == "Wobble Bass").unwrap();
    let v = preset_values(descriptor(ids::NOVA).unwrap(), p);
    let sync = v.iter().find(|(k, _)| *k == "lfo1_sync").unwrap().1 as usize;
    assert_eq!(mfc_plugins::sync::SYNC_CHOICES[sync], "1/8");
}
