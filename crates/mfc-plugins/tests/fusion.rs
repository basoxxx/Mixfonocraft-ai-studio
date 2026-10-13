//! Synth FM Fusion: intonazione, preset, algoritmi, feedback, anti-aliasing, mono.

use mfc_plugins::{create_generator, default_instance, descriptor, ids, preset_values, presets, Generator, NoteOn};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;

fn make(params: &[(&str, f32)]) -> Box<dyn Generator> {
    let mut inst = default_instance(ids::FUSION).unwrap();
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

/// Ampiezza delle armoniche 2..=8 rispetto alla fondamentale.
fn overtones(x: &[f32], f0: f32) -> f32 {
    let fund = magnitude(x, f0).max(1e-9);
    (2..=8).map(|h| magnitude(x, f0 * h as f32)).sum::<f32>() / fund
}

/// Sola portante (op 1) senza modulazione e con inviluppo pieno.
const PURE: &[(&str, f32)] = &[("op2_level", 0.0), ("op1_sustain", 1.0)];

#[test]
fn pure_carrier_tuning_follows_key_and_ratio() {
    for (extra, key, hz) in [
        (vec![], 69u8, 440.0f32),
        (vec![("op1_ratio", 2.0)], 69, 880.0),
        (vec![("op1_ratio", 0.5)], 69, 220.0),
        (vec![("op1_fine", 100.0)], 57, 233.08),
        (vec![], 48, 130.81),
    ] {
        let mut params = PURE.to_vec();
        params.extend(extra.iter().copied());
        let mut g = make(&params);
        g.note_on(NoteOn::simple(1, key, 1.0));
        let (l, _) = render(g.as_mut(), 0.5);
        let f = zero_cross_freq(&l[4800..]);
        assert!((f - hz).abs() / hz < 0.005, "{params:?}: {f} Hz invece di {hz}");
        // Sinusoide pura: niente armoniche.
        assert!(overtones(&l[4800..], hz) < 0.01, "{params:?}: portante non pura");
    }
}

#[test]
fn pitch_offset_bends() {
    let mut g = make(PURE);
    g.set_pitch_offset(1200.0);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, _) = render(g.as_mut(), 0.3);
    let f = zero_cross_freq(&l[4800..]);
    assert!((f - 880.0).abs() < 4.0, "{f}");
}

#[test]
fn every_preset_plays_and_releases() {
    let desc = descriptor(ids::FUSION).unwrap();
    let list = presets(ids::FUSION);
    assert!(list.len() >= 12);
    for name in [
        "E-Piano DX",
        "Bell",
        "Bass FM",
        "Brass FM",
        "Organ",
        "Mallet",
        "Pluck",
        "Pad",
        "Lead",
        "Metallic FX",
        "Kalimba",
        "Sub Bass",
    ] {
        assert!(list.iter().any(|p| p.name == name), "manca il preset {name}");
    }
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
fn modulator_level_adds_sidebands() {
    let spectrum = |level: f32| {
        let mut g = make(&[("op1_sustain", 1.0), ("op2_level", level), ("op2_sustain", 1.0)]);
        g.note_on(NoteOn::simple(1, 57, 1.0));
        let (l, _) = render(g.as_mut(), 0.5);
        overtones(&l[4800..], 220.0)
    };
    let (none, some, lots) = (spectrum(0.0), spectrum(0.15), spectrum(0.6));
    assert!(none < 0.01, "{none}");
    assert!(some > 0.1, "{some}");
    assert!(
        lots > some * 2.0,
        "indice maggiore → spettro più ricco: {some} vs {lots}"
    );
}

#[test]
fn algorithm_changes_spectrum() {
    // Stessi operatori: in pila (1) ogni modulatore arricchisce lo spettro,
    // in additivo (8) restano solo le armoniche 1 e 2.
    let base = [
        ("op1_sustain", 1.0),
        ("op2_level", 0.4),
        ("op2_sustain", 1.0),
        ("op3_ratio", 2.0),
        ("op3_level", 0.4),
        ("op3_sustain", 1.0),
        ("op4_level", 0.4),
        ("op4_sustain", 1.0),
    ];
    let render_algo = |algo: f32| {
        let mut p = base.to_vec();
        p.push(("fm_algo", algo));
        let mut g = make(&p);
        g.note_on(NoteOn::simple(1, 57, 1.0));
        let (l, _) = render(g.as_mut(), 0.5);
        l[4800..].to_vec()
    };
    let stack = render_algo(0.0);
    let additive = render_algo(7.0);
    let high = |x: &[f32]| (3..=8).map(|h| magnitude(x, 220.0 * h as f32)).sum::<f32>();
    assert!(
        high(&additive) < 0.01 * peak(&additive),
        "additivo: {}",
        high(&additive)
    );
    assert!(high(&stack) > 0.1 * peak(&stack), "pila: {}", high(&stack));
    // Ogni algoritmo produce un suono diverso.
    let all: Vec<Vec<f32>> = (0..8).map(|a| render_algo(a as f32)).collect();
    for a in 0..8 {
        assert!(all[a].iter().all(|x| x.is_finite()));
        for b in a + 1..8 {
            let diff: f32 = all[a].iter().zip(&all[b]).map(|(x, y)| (x - y).abs()).sum();
            assert!(diff > 1.0, "algoritmi {a} e {b} identici");
        }
    }
}

#[test]
fn feedback_makes_op4_brighter() {
    let render_fb = |fb: f32| {
        let mut g = make(&[
            ("fm_algo", 7.0),
            ("op1_level", 0.0),
            ("op2_level", 0.0),
            ("op4_level", 1.0),
            ("op4_sustain", 1.0),
            ("fm_feedback", fb),
        ]);
        g.note_on(NoteOn::simple(1, 57, 1.0));
        let (l, _) = render(g.as_mut(), 0.5);
        assert!(l.iter().all(|x| x.is_finite()));
        overtones(&l[4800..], 220.0)
    };
    let (dry, fb) = (render_fb(0.0), render_fb(1.0));
    assert!(dry < 0.01, "{dry}");
    assert!(fb > 0.3, "feedback senza armoniche: {fb}");
}

#[test]
fn high_notes_with_heavy_modulation_stay_bounded() {
    // Indici enormi su una nota acuta: il limite di Carson riduce la modulazione
    // e il suono resta vicino alla sola portante (4186 Hz), senza aliasing.
    let mut g = make(&[
        ("op1_sustain", 1.0),
        ("op2_ratio", 16.0),
        ("op2_level", 1.0),
        ("op2_sustain", 1.0),
        ("fm_feedback", 1.0),
        ("op4_level", 1.0),
        ("op4_ratio", 16.0),
        ("fm_algo", 1.0),
    ]);
    g.note_on(NoteOn::simple(1, 108, 1.0));
    let (l, _) = render(g.as_mut(), 0.3);
    assert!(l.iter().all(|x| x.is_finite()));
    let x = &l[4800..];
    let fund = magnitude(x, 4186.0);
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    assert!(fund > 0.9 * rms * std::f32::consts::SQRT_2, "{fund} vs rms {rms}");
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
    assert!((zero_cross_freq(&l[4800..]) - 440.0).abs() < 3.0);
    g.note_off(2);
    let (l, _) = render(g.as_mut(), 0.3);
    assert!(peak(&l) > 0.05);
    assert!((zero_cross_freq(&l[4800..]) - 220.0).abs() < 2.0);
    g.note_off(1);
    render(g.as_mut(), 1.0);
    assert_eq!(g.active_voices(), 0);
}

#[test]
fn legato_glide_moves_pitch_gradually() {
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
    assert!((zero_cross_freq(&l[48_000..]) - 440.0).abs() < 3.0);
}

#[test]
fn vibrato_moves_pitch() {
    let mut g = make(&[
        ("op2_level", 0.0),
        ("op1_sustain", 1.0),
        ("lfo_pitch", 1.0),
        ("lfo_rate", 2.0),
    ]);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, _) = render(g.as_mut(), 1.0);
    let freqs: Vec<f32> = l.chunks(4800).map(zero_cross_freq).collect();
    let (lo, hi) = freqs.iter().fold((f32::MAX, 0.0f32), |(a, b), &f| (a.min(f), b.max(f)));
    assert!(hi / lo > 1.1, "vibrato assente: {lo} {hi}");
}
