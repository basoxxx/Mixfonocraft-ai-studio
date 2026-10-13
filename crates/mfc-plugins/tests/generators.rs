//! Comportamento dei generatori: livello, terminazione delle voci, intonazione.

use mfc_plugins::{create_generator, default_instance, ids, Generator, NoteOn};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;

fn make(id: &str, params: &[(&str, f32)]) -> Box<dyn Generator> {
    let mut inst = default_instance(id).unwrap();
    for (k, v) in params {
        inst.params.insert((*k).to_string(), *v);
    }
    create_generator(&inst, SR)
}

/// Elabora `seconds` a blocchi da 256 campioni e restituisce (sinistro, destro).
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

/// Note on → uscita non nulla e limitata; note off → voci a zero entro 6 s.
fn check_lifecycle(id: &str, params: &[(&str, f32)]) {
    let mut g = make(id, params);
    g.note_on(NoteOn::simple(1, 60, 0.9));
    assert!(g.active_voices() > 0, "{id} {params:?}: nessuna voce");
    let (l, r) = render(g.as_mut(), 0.5);
    assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{id}: NaN");
    let p = peak(&l).max(peak(&r));
    assert!(p > 1e-3 && p < 4.0, "{id} {params:?}: picco {p}");
    g.note_off(1);
    let mut blocks = 0;
    while g.active_voices() > 0 {
        let (l, r) = render(g.as_mut(), BLOCK as f32 / SR);
        assert!(l.iter().chain(&r).all(|x| x.is_finite() && x.abs() < 4.0));
        blocks += 1;
        assert!(
            blocks < (6.0 * SR) as usize / BLOCK,
            "{id} {params:?}: le voci non terminano"
        );
    }
}

#[test]
fn synth_lifecycles() {
    check_lifecycle(ids::OSC3, &[]);
    check_lifecycle(ids::BOOBASS, &[]);
    check_lifecycle(ids::KICK, &[]);
    check_lifecycle(ids::SUPERSAW, &[]);
    for shape in 0..6 {
        check_lifecycle(ids::OSC3, &[("osc1_shape", shape as f32), ("osc2_shape", shape as f32)]);
    }
    for t in 0..8 {
        check_lifecycle(ids::DRUMSYNTH, &[("type", t as f32)]);
    }
}

#[test]
fn drumsynth_sounds_are_distinct() {
    // Centroide spettrale grezzo: rapporto tra energia della derivata e del segnale.
    let brightness = |t: f32| {
        let mut g = make(ids::DRUMSYNTH, &[("type", t)]);
        g.note_on(NoteOn::simple(1, 60, 1.0));
        let (l, _) = render(g.as_mut(), 0.1);
        let e: f32 = l.iter().map(|x| x * x).sum();
        let d: f32 = l.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        d / e.max(1e-12)
    };
    let kick = brightness(0.0);
    let snare = brightness(1.0);
    let hat = brightness(3.0);
    assert!(kick < snare && snare < hat, "kick {kick} snare {snare} hat {hat}");
    // L'open hat dura più del closed hat.
    let length = |t: f32| {
        let mut g = make(ids::DRUMSYNTH, &[("type", t)]);
        g.note_on(NoteOn::simple(1, 60, 1.0));
        let mut n = 0;
        while g.active_voices() > 0 && n < 1000 {
            render(g.as_mut(), BLOCK as f32 / SR);
            n += 1;
        }
        n
    };
    assert!(length(4.0) > 2 * length(3.0));
}

#[test]
fn osc3_sine_pitch() {
    let params = [("osc2_volume", 0.0), ("osc3_volume", 0.0), ("osc1_shape", 0.0)];
    let mut g = make(ids::OSC3, &params);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, r) = render(g.as_mut(), 1.0);
    let f = zero_cross_freq(&l[4800..]);
    assert!((f - 440.0).abs() < 4.4, "{f}");
    assert!((zero_cross_freq(&r[4800..]) - 440.0).abs() < 4.4);
    // Pitch del canale e fine pitch della nota.
    let mut g = make(ids::OSC3, &params);
    g.set_pitch_offset(1200.0);
    let mut note = NoteOn::simple(1, 57, 1.0);
    note.fine_pitch_cents = 0.0;
    g.note_on(note);
    let (l, _) = render(g.as_mut(), 1.0);
    assert!((zero_cross_freq(&l[4800..]) - 440.0).abs() < 4.4);
    // Coarse dell'oscillatore.
    let mut g = make(
        ids::OSC3,
        &[("osc2_volume", 0.0), ("osc3_volume", 0.0), ("osc1_coarse", 12.0)],
    );
    g.note_on(NoteOn::simple(1, 57, 1.0));
    let (l, _) = render(g.as_mut(), 1.0);
    assert!((zero_cross_freq(&l[4800..]) - 440.0).abs() < 4.4);
}

#[test]
fn osc3_slide_glides_existing_voice() {
    let mut g = make(ids::OSC3, &[("osc2_volume", 0.0), ("osc3_volume", 0.0), ("glide", 0.1)]);
    g.note_on(NoteOn::simple(1, 57, 1.0));
    render(g.as_mut(), 0.2);
    let mut slide = NoteOn::simple(2, 69, 1.0);
    slide.slide = true;
    g.note_on(slide);
    assert_eq!(g.active_voices(), 1);
    let (l, _) = render(g.as_mut(), 1.0);
    assert!((zero_cross_freq(&l[9600..]) - 440.0).abs() < 4.4);
    g.note_off(1);
    render(g.as_mut(), 2.0);
    assert_eq!(g.active_voices(), 0);
}

#[test]
fn voice_stealing_caps_polyphony() {
    for id in [ids::OSC3, ids::SUPERSAW, ids::KICK, ids::DRUMSYNTH] {
        let mut g = make(id, &[]);
        for i in 0..100u64 {
            g.note_on(NoteOn::simple(i, 30 + (i % 60) as u8, 0.7));
            render(g.as_mut(), 0.001);
        }
        let n = g.active_voices();
        assert!(n > 0 && n <= 32, "{id}: {n}");
        let (l, _) = render(g.as_mut(), 0.05);
        assert!(peak(&l) < 16.0);
        g.all_notes_off();
        render(g.as_mut(), 6.0);
        assert_eq!(g.active_voices(), 0, "{id}");
    }
}

#[test]
fn kick_pitch_settles_to_end_freq() {
    let mut g = make(
        ids::KICK,
        &[
            ("start_freq", 300.0),
            ("end_freq", 50.0),
            ("pitch_decay", 0.005),
            ("amp_decay", 2.0),
        ],
    );
    g.note_on(NoteOn::simple(1, 60, 1.0));
    let (l, _) = render(g.as_mut(), 0.5);
    let f = zero_cross_freq(&l[(0.1 * SR) as usize..]);
    assert!((f - 50.0).abs() < 1.5, "{f}");
    // Key tracking: un'ottava sopra raddoppia.
    let mut g = make(
        ids::KICK,
        &[
            ("start_freq", 300.0),
            ("end_freq", 50.0),
            ("pitch_decay", 0.005),
            ("amp_decay", 2.0),
            ("key_track", 1.0),
        ],
    );
    g.note_on(NoteOn::simple(1, 72, 1.0));
    let (l, _) = render(g.as_mut(), 0.5);
    let f = zero_cross_freq(&l[(0.1 * SR) as usize..]);
    assert!((f - 100.0).abs() < 3.0, "{f}");
}

#[test]
fn velocity_and_pan_are_respected() {
    let mut loud = make(ids::OSC3, &[]);
    let mut soft = make(ids::OSC3, &[]);
    loud.note_on(NoteOn::simple(1, 60, 1.0));
    soft.note_on(NoteOn::simple(1, 60, 0.25));
    let (a, _) = render(loud.as_mut(), 0.2);
    let (b, _) = render(soft.as_mut(), 0.2);
    assert!(peak(&b) < 0.5 * peak(&a));
    let mut g = make(ids::SUPERSAW, &[]);
    let mut note = NoteOn::simple(1, 60, 1.0);
    note.pan = -1.0;
    g.note_on(note);
    let (l, r) = render(g.as_mut(), 0.2);
    assert!(peak(&l) > 0.01 && peak(&r) < 1e-6);
}

#[test]
fn process_adds_to_buffers() {
    let mut g = make(ids::SILENT, &[]);
    g.note_on(NoteOn::simple(1, 60, 1.0));
    let (mut l, mut r) = (vec![0.25; 128], vec![0.5; 128]);
    g.process(&mut l, &mut r);
    assert!(l.iter().all(|x| *x == 0.25) && r.iter().all(|x| *x == 0.5));
    assert_eq!(g.active_voices(), 0);
    let mut g = make(ids::OSC3, &[]);
    g.note_on(NoteOn::simple(1, 60, 1.0));
    let (mut l, mut r) = (vec![10.0; 512], vec![10.0; 512]);
    g.process(&mut l, &mut r);
    assert!(l.iter().any(|x| (*x - 10.0).abs() > 1e-3));
}

#[test]
fn sampler_without_file_is_silent() {
    for id in [ids::SAMPLER, ids::AUDIO_CLIP] {
        let mut g = make(id, &[]);
        g.note_on(NoteOn::simple(1, 60, 1.0));
        assert_eq!(g.active_voices(), 0);
        let (l, r) = render(g.as_mut(), 0.1);
        assert_eq!(peak(&l) + peak(&r), 0.0);
        let mut inst = default_instance(id).unwrap();
        inst.sample_path = Some("/non/esiste.wav".into());
        let mut g = create_generator(&inst, SR);
        g.note_on(NoteOn::simple(1, 60, 1.0));
        assert_eq!(g.active_voices(), 0);
    }
}

#[test]
fn fm_and_pluck_lifecycles() {
    check_lifecycle(ids::FM, &[]);
    check_lifecycle(ids::FM, &[("feedback", 1.0), ("index", 20.0), ("ratio", 16.0)]);
    check_lifecycle(ids::FM, &[("sustain", 1.0), ("release", 0.05)]);
    check_lifecycle(ids::PLUCK, &[]);
    check_lifecycle(ids::PLUCK, &[("decay", 20.0), ("release", 0.05), ("damping", 0.0)]);
    check_lifecycle(ids::PLUCK, &[("brightness", 0.0), ("damping", 1.0)]);
}

#[test]
fn fm_without_modulation_is_a_sine_at_pitch() {
    let mut g = make(ids::FM, &[("index", 0.0), ("sustain", 1.0)]);
    g.note_on(NoteOn::simple(1, 69, 1.0));
    let (l, _) = render(g.as_mut(), 1.0);
    let f = zero_cross_freq(&l[4800..]);
    assert!((f - 440.0).abs() < 4.4, "{f}");
    // Con modulazione lo spettro si arricchisce (più energia nella derivata).
    let bright = |index: f32| {
        let mut g = make(ids::FM, &[("index", index), ("index_sustain", 1.0), ("sustain", 1.0)]);
        g.note_on(NoteOn::simple(1, 57, 1.0));
        let (l, _) = render(g.as_mut(), 0.2);
        let e: f32 = l.iter().map(|x| x * x).sum();
        let d: f32 = l.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        d / e.max(1e-12)
    };
    assert!(bright(6.0) > 3.0 * bright(0.0));
}

#[test]
fn pluck_is_in_tune_and_decays() {
    let mut g = make(ids::PLUCK, &[("damping", 0.2)]);
    g.note_on(NoteOn::simple(1, 57, 1.0));
    let (l, _) = render(g.as_mut(), 1.0);
    // Periodo per autocorrelazione: la corda è ricca di armoniche, gli zeri non bastano.
    let x = &l[2400..12_000];
    let corr = |lag: usize| -> f32 { x.iter().zip(&x[lag..]).map(|(a, b)| a * b).sum() };
    let lag = (48..960).max_by(|a, b| corr(*a).total_cmp(&corr(*b))).unwrap();
    let f = SR / lag as f32;
    assert!((f - 220.0).abs() < 3.0, "{f}");
    let early = peak(&l[..4800]);
    let late = peak(&l[38_400..]);
    assert!(late < early * 0.8, "deve decadere: {early} → {late}");
    // Note off: si spegne in fretta col release corto.
    g.note_off(1);
    let mut blocks = 0;
    while g.active_voices() > 0 {
        render(g.as_mut(), BLOCK as f32 / SR);
        blocks += 1;
        assert!(blocks < (2.0 * SR) as usize / BLOCK, "release troppo lungo");
    }
}
