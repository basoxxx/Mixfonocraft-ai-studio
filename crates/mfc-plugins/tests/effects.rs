//! Comportamento degli effetti: stabilità, reset delle code, risposta attesa.

use mfc_dsp::{db_to_gain, Rng};
use mfc_plugins::{create_effect, default_instance, ids, registry, Effect, PluginRole};
use std::f32::consts::TAU;

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;

fn make(id: &str, params: &[(&str, f32)]) -> Box<dyn Effect> {
    let mut inst = default_instance(id).unwrap();
    for (k, v) in params {
        inst.params.insert((*k).to_string(), *v);
    }
    create_effect(&inst, SR).unwrap()
}

fn run(fx: &mut dyn Effect, l: &mut [f32], r: &mut [f32]) {
    for (bl, br) in l.chunks_mut(BLOCK).zip(r.chunks_mut(BLOCK)) {
        fx.process(bl, br);
    }
}

fn noise(n: usize, amp: f32, seed: u32) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..n).map(|_| rng.next_bipolar() * amp).collect()
}

fn sine(n: usize, hz: f32, amp: f32) -> Vec<f32> {
    (0..n).map(|i| amp * (TAU * hz * i as f32 / SR).sin()).collect()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn effect_ids() -> Vec<&'static str> {
    registry()
        .iter()
        .filter(|d| d.role == PluginRole::Effect)
        .map(|d| d.id)
        .collect()
}

#[test]
fn silence_and_noise_stay_finite() {
    for id in effect_ids() {
        let mut fx = make(id, &[]);
        let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
        run(fx.as_mut(), &mut l, &mut r);
        assert!(
            l.iter().chain(&r).all(|x| x.is_finite() && x.abs() < 1e-3),
            "{id}: silenzio"
        );
        let (mut l, mut r) = (noise(96_000, 0.9, 1), noise(96_000, 0.9, 2));
        run(fx.as_mut(), &mut l, &mut r);
        assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{id}: NaN");
        assert!(peak(&l).max(peak(&r)) < 8.0, "{id}: picco {}", peak(&l).max(peak(&r)));
    }
}

#[test]
fn extreme_settings_stay_bounded() {
    for d in registry().iter().filter(|d| d.role == PluginRole::Effect) {
        for use_max in [false, true] {
            let params: Vec<(&str, f32)> = d
                .params
                .iter()
                .map(|p| (p.name, if use_max { p.max } else { p.min }))
                .collect();
            let mut fx = make(d.id, &params);
            let (mut l, mut r) = (noise(48_000, 1.0, 3), noise(48_000, 1.0, 4));
            run(fx.as_mut(), &mut l, &mut r);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{} max={use_max}", d.id);
            // L'EQ con 7 bande risonanti sovrapposte può legittimamente amplificare molto.
            if d.id != ids::EQ {
                assert!(peak(&l).max(peak(&r)) < 100.0, "{} max={use_max}: {}", d.id, peak(&l));
            }
        }
    }
}

#[test]
fn reset_clears_tails() {
    for id in effect_ids() {
        let mut fx = make(id, &[]);
        let (mut l, mut r) = (noise(48_000, 0.5, 5), noise(48_000, 0.5, 6));
        run(fx.as_mut(), &mut l, &mut r);
        fx.reset();
        let (mut l, mut r) = (vec![0.0; 4_800], vec![0.0; 4_800]);
        run(fx.as_mut(), &mut l, &mut r);
        assert!(peak(&l).max(peak(&r)) < 1e-6, "{id}: coda dopo reset {}", peak(&l));
    }
}

#[test]
fn reverb_and_delay_have_tails() {
    for id in [ids::REVERB, ids::DELAY] {
        let mut fx = make(id, &[]);
        let (mut l, mut r) = (vec![0.0; 48_000], vec![0.0; 48_000]);
        l[0] = 1.0;
        r[0] = 1.0;
        run(fx.as_mut(), &mut l, &mut r);
        assert!(peak(&l[4_800..]) > 1e-3, "{id}: nessuna coda");
    }
    // Delay: primo eco al tempo impostato.
    let mut fx = make(
        ids::DELAY,
        &[("time", 100.0), ("dry", 0.0), ("wet", 1.0), ("cutoff", 20_000.0)],
    );
    let (mut l, mut r) = (vec![0.0; 9_600], vec![0.0; 9_600]);
    l[0] = 1.0;
    run(fx.as_mut(), &mut l, &mut r);
    let echo = l
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert!((echo as i32 - 4_800).abs() <= 2, "eco a {echo}");
}

#[test]
fn eq_peak_boosts_1k() {
    let mut fx = make(
        ids::EQ,
        &[
            ("band4_type", 3.0),
            ("band4_freq", 1_000.0),
            ("band4_gain", 12.0),
            ("band4_q", 1.0),
        ],
    );
    let (mut l, mut r) = (sine(48_000, 1_000.0, 0.1), sine(48_000, 1_000.0, 0.1));
    run(fx.as_mut(), &mut l, &mut r);
    let gain = peak(&l[24_000..]) / 0.1;
    assert!((gain - db_to_gain(12.0)).abs() < 0.2, "guadagno {gain}");
    // Lontano dalla banda resta invariato.
    let mut fx = make(
        ids::EQ,
        &[
            ("band4_type", 3.0),
            ("band4_freq", 1_000.0),
            ("band4_gain", 12.0),
            ("band4_q", 4.0),
        ],
    );
    let (mut l, mut r) = (sine(48_000, 100.0, 0.1), sine(48_000, 100.0, 0.1));
    run(fx.as_mut(), &mut l, &mut r);
    assert!((peak(&l[24_000..]) / 0.1 - 1.0).abs() < 0.05);
    // Low cut attenua i bassi.
    let mut fx = make(ids::EQ, &[("band1_type", 1.0), ("band1_freq", 500.0)]);
    let (mut l, mut r) = (sine(48_000, 50.0, 0.5), sine(48_000, 50.0, 0.5));
    run(fx.as_mut(), &mut l, &mut r);
    assert!(peak(&l[24_000..]) < 0.01);
}

#[test]
fn compressor_reduces_loud_signal() {
    let mut fx = make(
        ids::COMPRESSOR,
        &[("threshold", -30.0), ("ratio", 10.0), ("attack", 1.0), ("knee", 0.0)],
    );
    let (mut l, mut r) = (sine(48_000, 200.0, 1.0), sine(48_000, 200.0, 1.0));
    run(fx.as_mut(), &mut l, &mut r);
    let out = peak(&l[24_000..]);
    assert!(out < 0.2, "{out}");
    // Sotto soglia: invariato.
    let mut fx = make(ids::COMPRESSOR, &[("threshold", -6.0), ("knee", 0.0)]);
    let (mut l, mut r) = (sine(48_000, 200.0, 0.1), sine(48_000, 200.0, 0.1));
    run(fx.as_mut(), &mut l, &mut r);
    assert!((peak(&l[24_000..]) - 0.1).abs() < 1e-3);
}

#[test]
fn limiter_respects_ceiling() {
    let ceiling = db_to_gain(-1.0);
    let mut fx = make(ids::LIMITER, &[("ceiling", -1.0), ("gain", 6.0)]);
    assert!(fx.latency() > 0);
    let (mut l, mut r) = (noise(96_000, 1.5, 7), sine(96_000, 300.0, 2.0));
    run(fx.as_mut(), &mut l, &mut r);
    assert!(peak(&l).max(peak(&r)) <= ceiling + 1e-6);
    assert!(peak(&r[48_000..]) > 0.9 * ceiling);
    // Il segnale sotto soglia passa invariato, solo ritardato della latenza.
    let mut fx = make(ids::LIMITER, &[("ceiling", 0.0)]);
    let lat = fx.latency();
    let input = sine(4_800, 440.0, 0.5);
    let (mut l, mut r) = (input.clone(), input.clone());
    run(fx.as_mut(), &mut l, &mut r);
    for i in lat..4_800 {
        assert!((l[i] - input[i - lat]).abs() < 1e-5);
    }
}

#[test]
fn gate_attenuates_quiet_signal() {
    let mut fx = make(ids::GATE, &[("threshold", -30.0), ("range", -60.0)]);
    let (mut l, mut r) = (noise(48_000, 0.003, 8), noise(48_000, 0.003, 9));
    run(fx.as_mut(), &mut l, &mut r);
    assert!(peak(&l[24_000..]) < 1e-5);
    let (mut l, mut r) = (sine(48_000, 200.0, 0.5), sine(48_000, 200.0, 0.5));
    run(fx.as_mut(), &mut l, &mut r);
    assert!((peak(&l[24_000..]) - 0.5).abs() < 0.01);
}

#[test]
fn filter_gain_stereo_and_distortion() {
    let mut fx = make(ids::FILTER, &[("type", 0.0), ("cutoff", 500.0), ("resonance", 0.0)]);
    let (mut l, mut r) = (sine(48_000, 5_000.0, 0.5), sine(48_000, 5_000.0, 0.5));
    run(fx.as_mut(), &mut l, &mut r);
    assert!(peak(&l[24_000..]) < 0.01);

    let mut fx = make(ids::GAIN, &[("gain", -6.0206)]);
    let (mut l, mut r) = (vec![1.0; 48_000], vec![1.0; 48_000]);
    run(fx.as_mut(), &mut l, &mut r);
    assert!((l[47_999] - 0.5).abs() < 1e-3);
    let mut fx = make(ids::GAIN, &[("invert", 1.0)]);
    let (mut l, mut r) = (vec![1.0; 256], vec![1.0; 256]);
    run(fx.as_mut(), &mut l, &mut r);
    assert_eq!(l[0], -1.0);

    let mut fx = make(ids::STEREO, &[("width", 0.0)]);
    let (mut l, mut r) = (noise(4_800, 0.5, 10), noise(4_800, 0.5, 11));
    run(fx.as_mut(), &mut l, &mut r);
    assert!(l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1e-6));
    let mut fx = make(ids::STEREO, &[("swap", 1.0)]);
    let (mut l, mut r) = (vec![1.0; 64], vec![0.0; 64]);
    run(fx.as_mut(), &mut l, &mut r);
    assert!(l[10].abs() < 1e-6 && (r[10] - 1.0).abs() < 1e-6);

    for t in 0..4 {
        let mut fx = make(ids::DISTORTION, &[("type", t as f32), ("drive", 48.0)]);
        let (mut l, mut r) = (sine(9_600, 100.0, 1.0), sine(9_600, 100.0, 1.0));
        run(fx.as_mut(), &mut l, &mut r);
        assert!(peak(&l) < 1.5 && peak(&l) > 0.3, "distorsione {t}: {}", peak(&l));
    }

    let mut fx = make(ids::BITCRUSHER, &[("bits", 2.0)]);
    let (mut l, mut r) = (sine(4_800, 100.0, 0.9), sine(4_800, 100.0, 0.9));
    run(fx.as_mut(), &mut l, &mut r);
    assert!(l.iter().all(|x| [-1.0, -0.5, 0.0, 0.5, 1.0].contains(x)));
}

#[test]
fn modulation_effects_change_the_signal() {
    for id in [ids::CHORUS, ids::FLANGER, ids::PHASER] {
        let mut fx = make(id, &[]);
        let input = noise(48_000, 0.5, 12);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(fx.as_mut(), &mut l, &mut r);
        let diff: f32 = l.iter().zip(&input).map(|(a, b)| (a - b).abs()).sum::<f32>() / input.len() as f32;
        assert!(diff > 0.01, "{id}: {diff}");
    }
}

#[test]
fn sample_rate_changes_are_safe() {
    for id in effect_ids() {
        let mut fx = make(id, &[]);
        for sr in [22_050.0, 44_100.0, 96_000.0, 192_000.0, 384_000.0] {
            fx.set_sample_rate(sr);
            let (mut l, mut r) = (noise(4_096, 0.8, 13), noise(4_096, 0.8, 14));
            run(fx.as_mut(), &mut l, &mut r);
            assert!(
                l.iter().chain(&r).all(|x| x.is_finite() && x.abs() < 8.0),
                "{id} @ {sr}"
            );
        }
    }
}
