//! Drum machine Drumkit: pad, kit, choke degli hi-hat, polifonia, preset.

use mfc_plugins::generators::drumkit::{pad_for_key, PAD_KEYS};
use mfc_plugins::{create_generator, default_instance, descriptor, ids, preset_values, presets, Generator, NoteOn};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;

fn make(params: &[(&str, f32)]) -> Box<dyn Generator> {
    let mut inst = default_instance(ids::DRUMKIT).unwrap();
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

/// Attraversamenti dello zero al secondo (indice grezzo di brillantezza).
fn zcr(x: &[f32]) -> f32 {
    let n = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    n as f32 * SR / x.len() as f32
}

/// Suona un solo colpo e restituisce il canale sinistro + destro (mono).
fn hit(params: &[(&str, f32)], key: u8, seconds: f32) -> Vec<f32> {
    let mut g = make(params);
    g.note_on(NoteOn::simple(1, key, 1.0));
    let (l, r) = render(g.as_mut(), seconds);
    l.iter().zip(&r).map(|(a, b)| a + b).collect()
}

#[test]
fn every_preset_plays_all_pads_and_falls_silent() {
    let desc = descriptor(ids::DRUMKIT).unwrap();
    let list = presets(ids::DRUMKIT);
    assert!(list.len() >= 6);
    for p in list {
        for (k, v) in p.values {
            let d = desc
                .param(k)
                .unwrap_or_else(|| panic!("{}: parametro sconosciuto {k}", p.name));
            assert!(*v >= d.min && *v <= d.max, "{}: {k}={v} fuori range", p.name);
        }
        let values = preset_values(desc, p);
        let mut g = make(&values);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for (i, key) in PAD_KEYS.iter().enumerate() {
            g.note_on(NoteOn::simple(i as u64 + 1, *key, 1.0));
            g.note_off(i as u64 + 1);
            let (bl, br) = render(g.as_mut(), 0.1);
            assert!(
                peak(&bl[..2400]).max(peak(&br[..2400])) > 0.005,
                "{}: pad {key} muto",
                p.name
            );
            l.extend(bl);
            r.extend(br);
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{}: NaN", p.name);
        let pk = peak(&l).max(peak(&r));
        assert!(pk > 0.01 && pk < 2.0, "{}: picco {pk}", p.name);
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
fn keys_map_to_general_midi_pads() {
    for (i, k) in PAD_KEYS.iter().enumerate() {
        assert_eq!(pad_for_key(*k), i);
    }
    // Alias GM e ripiego sul pad più vicino.
    assert_eq!(pad_for_key(35), pad_for_key(36));
    assert_eq!(pad_for_key(40), pad_for_key(38));
    assert_eq!(pad_for_key(44), pad_for_key(42));
    assert_eq!(pad_for_key(0), pad_for_key(36));
    assert_eq!(pad_for_key(127), pad_for_key(56));
    assert_eq!(pad_for_key(60), pad_for_key(56));
    // Un tasto fuori mappa suona come il pad corrispondente.
    assert_eq!(hit(&[], 35, 0.2), hit(&[], 36, 0.2));
}

#[test]
fn different_pads_sound_different() {
    let kick = hit(&[], 36, 0.3);
    let snare = hit(&[], 38, 0.3);
    let hat = hit(&[], 42, 0.3);
    let (zk, zs, zh) = (zcr(&kick[..4800]), zcr(&snare[..4800]), zcr(&hat[..2400]));
    assert!(zk < 400.0, "kick troppo brillante: {zk}");
    assert!(zs > zk * 3.0, "snare vs kick: {zs} {zk}");
    assert!(zh > zs, "hat vs snare: {zh} {zs}");
    // Tutti i pad di un kit sono distinti.
    let all: Vec<Vec<f32>> = PAD_KEYS.iter().map(|k| hit(&[], *k, 0.15)).collect();
    for a in 0..all.len() {
        for b in a + 1..all.len() {
            let diff: f32 = all[a].iter().zip(&all[b]).map(|(x, y)| (x - y).abs()).sum();
            assert!(diff > 1.0, "pad {} e {} identici", PAD_KEYS[a], PAD_KEYS[b]);
        }
    }
}

#[test]
fn closed_hat_chokes_open_hat() {
    let mut g = make(&[]);
    g.note_on(NoteOn::simple(1, 46, 1.0));
    let (open, _) = render(g.as_mut(), 0.3);
    let mut g = make(&[]);
    g.note_on(NoteOn::simple(1, 46, 1.0));
    let (mut a, _) = render(g.as_mut(), 0.05);
    g.note_on(NoteOn::simple(2, 42, 1.0));
    let (b, _) = render(g.as_mut(), 0.25);
    a.extend(b);
    let tail = 4_800..9_600; // 100-200 ms
    assert!(rms(&open[tail.clone()]) > 0.005, "l'hat aperto deve suonare a lungo");
    assert!(rms(&a[tail.clone()]) < 0.05 * rms(&open[tail]), "choke inefficace");
    render(g.as_mut(), 0.2);
    assert_eq!(g.active_voices(), 0);
}

#[test]
fn kits_differ() {
    let kicks: Vec<Vec<f32>> = (0..6).map(|k| hit(&[("kit", k as f32)], 36, 0.5)).collect();
    let snares: Vec<Vec<f32>> = (0..6).map(|k| hit(&[("kit", k as f32)], 38, 0.3)).collect();
    for set in [&kicks, &snares] {
        for a in 0..6 {
            for b in a + 1..6 {
                let diff: Vec<f32> = set[a].iter().zip(&set[b]).map(|(x, y)| x - y).collect();
                assert!(rms(&diff) > 0.1 * rms(&set[a]), "kit {a} e {b} uguali");
            }
        }
    }
    // Il kick 808 dura più del 909, quello trap più di tutti.
    let tail = |x: &[f32]| rms(&x[14_400..24_000]);
    assert!(tail(&kicks[0]) > 2.0 * tail(&kicks[1]), "808 vs 909");
    assert!(tail(&kicks[5]) > tail(&kicks[0]), "trap vs 808");
}

#[test]
fn group_controls_tune_decay_and_level() {
    // Intonazione: +12 st raddoppia la frequenza della coda del kick.
    let low = hit(&[], 36, 0.6);
    let high = hit(&[("kick_tune", 12.0)], 36, 0.6);
    let (zl, zh) = (zcr(&low[9_600..19_200]), zcr(&high[9_600..19_200]));
    assert!((zh / zl - 2.0).abs() < 0.2, "tune: {zl} → {zh}");
    // Decay e livello.
    let short = hit(&[("kick_decay", 0.25)], 36, 0.6);
    assert!(rms(&short[14_400..24_000]) < 0.2 * rms(&low[14_400..24_000]));
    let quiet = hit(&[("kick_level", 0.5)], 36, 0.3);
    assert!((peak(&quiet) / peak(&low) - 0.5).abs() < 0.05);
    // I controlli di un gruppo non toccano gli altri pad.
    assert_eq!(hit(&[("kick_level", 0.0)], 38, 0.2), hit(&[], 38, 0.2));
    assert!(peak(&hit(&[("kick_level", 0.0)], 36, 0.2)) < 1e-6);
}

#[test]
fn pads_overlap_polyphonically_and_ignore_note_off() {
    let mut g = make(&[]);
    g.note_on(NoteOn::simple(1, 36, 1.0));
    g.note_on(NoteOn::simple(2, 42, 1.0));
    g.note_on(NoteOn::simple(3, 49, 1.0));
    assert_eq!(g.active_voices(), 3);
    g.note_off(1);
    g.note_off(3);
    let (l, _) = render(g.as_mut(), 0.2);
    assert_eq!(g.active_voices(), 2, "kick e crash continuano, l'hat chiuso è finito");
    assert!(rms(&l[4800..]) > 0.01);
    g.all_notes_off();
    render(g.as_mut(), 0.1);
    assert_eq!(g.active_voices(), 0);
}

#[test]
fn velocity_scales_level() {
    let mut g = make(&[]);
    g.note_on(NoteOn::simple(1, 38, 0.3));
    let (l, r) = render(g.as_mut(), 0.2);
    let soft: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a + b).collect();
    let loud = hit(&[], 38, 0.2);
    let ratio = peak(&soft) / peak(&loud);
    assert!(ratio > 0.3 && ratio < 0.6, "velocity: {ratio}");
}
