//! Synth wavetable Prism: intonazione, preset, morph della posizione, band-limiting, mono.

use mfc_plugins::generators::prism::{WaveBank, FRAMES, TABLE_NAMES};
use mfc_plugins::{create_generator, default_instance, descriptor, ids, preset_values, presets, Generator, NoteOn};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;

fn make(params: &[(&str, f32)]) -> Box<dyn Generator> {
    let mut inst = default_instance(ids::PRISM).unwrap();
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

/// Ampiezza della componente a `hz` (DFT a una frequenza con finestra di Hann).
fn magnitude(x: &[f32], hz: f32) -> f32 {
    let n = x.len();
    let w = std::f64::consts::TAU * hz as f64 / SR as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in x.iter().enumerate() {
        let win = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
        let v = *s as f64 * win;
        re += v * (w * i as f64).cos();
        im -= v * (w * i as f64).sin();
    }
    ((re * re + im * im).sqrt() * 4.0 / n as f64) as f32
}

/// Ampiezze delle armoniche 1..=8.
fn harmonics(x: &[f32], f0: f32) -> Vec<f32> {
    (1..=8).map(|h| magnitude(x, f0 * h as f32)).collect()
}

/// Tabella Basic in posizione 0 (seno), filtro aperto.
const PURE: &[(&str, f32)] = &[("flt_cutoff", 20_000.0), ("flt_env", 0.0), ("aenv_sustain", 1.0)];

#[test]
fn sine_frame_tuning_follows_key_octave_and_semitones() {
    for (extra, key, hz) in [
        (vec![], 69u8, 440.0f32),
        (vec![("wt1_oct", 4.0)], 69, 880.0),
        (vec![("wt1_oct", 2.0)], 69, 220.0),
        (vec![("wt1_semi", 7.0)], 57, 329.63),
        (vec![], 36, 65.41),
    ] {
        let mut params = PURE.to_vec();
        params.extend(extra.iter().copied());
        let mut g = make(&params);
        g.note_on(NoteOn::simple(1, key, 1.0));
        let (l, _) = render(g.as_mut(), 0.5);
        let f = zero_cross_freq(&l[4800..]);
        assert!((f - hz).abs() / hz < 0.005, "{params:?}: {f} Hz invece di {hz}");
        let h = harmonics(&l[4800..], hz);
        assert!(
            h[1..].iter().sum::<f32>() < 0.01 * h[0],
            "{params:?}: il frame 0 deve essere un seno"
        );
    }
}

#[test]
fn pitch_offset_bends() {
    let mut g = make(PURE);
    g.set_pitch_offset(-1200.0);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, _) = render(g.as_mut(), 0.3);
    let f = zero_cross_freq(&l[4800..]);
    assert!((f - 220.0).abs() < 2.0, "{f}");
}

#[test]
fn every_preset_plays_and_releases() {
    let desc = descriptor(ids::PRISM).unwrap();
    let list = presets(ids::PRISM);
    assert!(list.len() >= 12);
    for p in list {
        for (k, v) in p.values {
            let d = desc
                .param(k)
                .unwrap_or_else(|| panic!("{}: parametro sconosciuto {k}", p.name));
            assert!(*v >= d.min && *v <= d.max, "{}: {k}={v} fuori range", p.name);
        }
        let values = preset_values(desc, p);
        let mut g = make(&values);
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
            let (l, _) = render(g.as_mut(), BLOCK as f32 / SR);
            assert!(l.iter().all(|x| x.is_finite()), "{}: NaN in coda", p.name);
            blocks += 1;
            assert!(blocks < (12.0 * SR) as usize / BLOCK, "{}: voci infinite", p.name);
        }
    }
}

#[test]
fn tables_are_normalised_and_frames_differ() {
    let bank = WaveBank::shared();
    for (t, name) in TABLE_NAMES.iter().enumerate() {
        let first = bank.cycle(t, 0, 0);
        let last = bank.cycle(t, FRAMES - 1, 0);
        for f in 0..FRAMES {
            let c = bank.cycle(t, f, 0);
            let pk = peak(c);
            assert!((pk - 1.0).abs() < 1e-3, "{name} frame {f}: picco {pk}");
            assert!(c.iter().all(|x| x.is_finite()));
            // Campione di guardia uguale al primo.
            assert_eq!(c[c.len() - 1], c[0]);
            // Niente componente continua.
            let dc = c[..c.len() - 1].iter().sum::<f32>() / (c.len() - 1) as f32;
            assert!(dc.abs() < 1e-3, "{name} frame {f}: DC {dc}");
        }
        let diff: f32 = first.iter().zip(last).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 10.0, "{name}: primo e ultimo frame uguali");
    }
}

#[test]
fn position_morph_changes_timbre() {
    let spectrum = |pos: f32| {
        let mut params = PURE.to_vec();
        params.push(("wt1_pos", pos));
        let mut g = make(&params);
        g.note_on(NoteOn::simple(1, 57, 1.0));
        let (l, _) = render(g.as_mut(), 0.4);
        harmonics(&l[4800..], 220.0)
    };
    let sine = spectrum(0.0);
    let saw = spectrum(2.0 / 3.0);
    let square = spectrum(1.0);
    assert!(sine[1] < 0.01 * sine[0]);
    // Dente di sega: armonica 2 a circa metà della fondamentale.
    assert!((saw[1] / saw[0] - 0.5).abs() < 0.1, "saw: {saw:?}");
    // Quadra: solo armoniche dispari, la terza a circa 1/3.
    assert!(square[1] < 0.05 * square[0], "square: {square:?}");
    assert!((square[2] / square[0] - 1.0 / 3.0).abs() < 0.06, "square: {square:?}");
}

#[test]
fn envelope_moves_position() {
    // Inviluppo di modulazione dal seno alla quadra: il timbro cambia nel tempo.
    let mut params = PURE.to_vec();
    params.extend([
        ("wt1_pos_env", 1.0),
        ("menv_attack", 0.0),
        ("menv_decay", 1.0),
        ("menv_sustain", 0.0),
    ]);
    let mut g = make(&params);
    g.note_on(NoteOn::simple(1, 57, 1.0));
    let (l, _) = render(g.as_mut(), 1.5);
    let early = harmonics(&l[480..2880], 220.0);
    let late = harmonics(&l[60_000..], 220.0);
    assert!(early[2] > 0.15 * early[0], "attacco brillante: {early:?}");
    assert!(late[2] < 0.02 * late[0], "coda sinusoidale: {late:?}");
}

#[test]
fn lfo_moves_position() {
    let mut params = PURE.to_vec();
    params.extend([("wt1_table", 1.0), ("wt1_pos_lfo", 1.0), ("lfo_rate", 1.0)]);
    let mut g = make(&params);
    g.note_on(NoteOn::simple(1, 57, 1.0));
    let (l, _) = render(g.as_mut(), 1.0);
    let ratios: Vec<f32> = l
        .chunks(4800)
        .map(|c| {
            let h = harmonics(c, 220.0);
            h[1] / h[0].max(1e-6)
        })
        .collect();
    let (lo, hi) = ratios
        .iter()
        .fold((f32::MAX, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(hi - lo > 0.2, "PWM fermo: {ratios:?}");
}

#[test]
fn high_notes_are_band_limited() {
    // Quadra a 2232.56 Hz (periodo di 43 campioni ogni 2 cicli): le armoniche cadono
    // sui bin pari della DFT a passo SR/43, l'aliasing finirebbe sui bin dispari.
    let f0 = 2.0 * SR / 43.0;
    let cents = 1200.0 * (f0 / 440.0f32).log2() - 100.0 * (97.0 - 69.0);
    let mut params = PURE.to_vec();
    params.push(("wt1_pos", 1.0));
    let mut g = make(&params);
    let mut note = NoteOn::simple(1, 97, 1.0);
    note.fine_pitch_cents = cents;
    g.note_on(note);
    let (l, _) = render(g.as_mut(), 0.4);
    let x = &l[4800..4800 + 43 * 300];
    let bin = |j: usize| {
        let w = std::f64::consts::TAU * j as f64 / 43.0;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, s) in x.iter().enumerate() {
            re += *s as f64 * (w * i as f64).cos();
            im -= *s as f64 * (w * i as f64).sin();
        }
        re * re + im * im
    };
    let (mut harm, mut alias) = (0.0, 0.0);
    for j in 1..=21 {
        if j % 2 == 0 {
            harm += bin(j);
        } else {
            alias += bin(j);
        }
    }
    assert!(alias < 1e-3 * harm, "aliasing: {}", alias / harm);
}

#[test]
fn unison_spread_makes_stereo() {
    let mut g = make(&[("wt1_pos", 0.66), ("wt1_unison", 5.0), ("wt1_spread", 1.0)]);
    g.note_on(NoteOn::simple(1, 60, 1.0));
    let (l, r) = render(g.as_mut(), 0.5);
    let diff: f32 = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum();
    assert!(diff > 0.1 * l.iter().map(|x| x.abs()).sum::<f32>());
}

#[test]
fn mono_mode_uses_one_voice_and_glides() {
    let mut params = PURE.to_vec();
    params.extend([("voice_mode", 2.0), ("voice_glide", 0.2)]);
    let mut g = make(&params);
    g.note_on(NoteOn::simple(1, 57, 1.0));
    render(g.as_mut(), 0.2);
    g.note_on(NoteOn::simple(2, 69, 1.0));
    assert_eq!(g.active_voices(), 1);
    let (l, _) = render(g.as_mut(), 0.05);
    let early = zero_cross_freq(&l);
    assert!(early > 225.0 && early < 400.0, "a metà glide: {early}");
    let (l, _) = render(g.as_mut(), 1.5);
    assert!((zero_cross_freq(&l[48_000..]) - 440.0).abs() < 3.0);
    g.note_off(2);
    let (l, _) = render(g.as_mut(), 1.5);
    assert!(
        (zero_cross_freq(&l[48_000..]) - 220.0).abs() < 3.0,
        "torna alla nota tenuta"
    );
    g.note_off(1);
    render(g.as_mut(), 1.0);
    assert_eq!(g.active_voices(), 0);
}
