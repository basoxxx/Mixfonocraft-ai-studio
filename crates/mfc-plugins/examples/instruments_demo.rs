//! Ascolto dei preset di Fusion, Prism e Drumkit:
//! `cargo run -p mfc-plugins --release --example instruments_demo -- cartella`.
//! Scrive `fusion.wav`, `prism.wav` e `drumkit.wav` nella cartella indicata (default: corrente).
//! I synth suonano una breve frase per preset (basso, accordi o melodia secondo la categoria),
//! la drum machine due battute di groove per ogni kit.

use mfc_plugins::{create_generator, default_instance, descriptor, ids, preset_values, presets, Generator, NoteOn};
use std::path::PathBuf;

const SR: u32 = 48_000;
const BPM: f32 = 120.0;

/// (battito di inizio, durata in battiti, tasto)
type Phrase = &'static [(f32, f32, u8)];

const BASS: Phrase = &[
    (0.0, 0.5, 33),
    (0.5, 0.5, 45),
    (1.0, 0.5, 33),
    (1.5, 0.5, 43),
    (2.0, 0.5, 29),
    (2.5, 0.5, 41),
    (3.0, 0.5, 31),
    (3.5, 0.5, 43),
];
const CHORDS: Phrase = &[
    (0.0, 1.9, 57),
    (0.0, 1.9, 60),
    (0.0, 1.9, 64),
    (2.0, 1.9, 53),
    (2.0, 1.9, 57),
    (2.0, 1.9, 60),
];
const MELODY: Phrase = &[
    (0.0, 0.5, 69),
    (0.5, 0.5, 72),
    (1.0, 0.5, 76),
    (1.5, 0.5, 74),
    (2.0, 1.0, 72),
    (3.0, 0.5, 71),
    (3.5, 0.5, 67),
];

/// Groove di due battute: kick, snare/clap, hi-hat (aperto in levare), tom e piatti.
const BEAT: Phrase = &[
    (0.0, 0.1, 49),
    (0.0, 0.1, 36),
    (0.75, 0.1, 36),
    (1.5, 0.1, 36),
    (2.5, 0.1, 36),
    (4.0, 0.1, 36),
    (4.75, 0.1, 36),
    (5.5, 0.1, 36),
    (1.0, 0.1, 38),
    (3.0, 0.1, 38),
    (3.0, 0.1, 39),
    (5.0, 0.1, 38),
    (5.75, 0.1, 37),
    (6.0, 0.1, 48),
    (6.25, 0.1, 48),
    (6.5, 0.1, 45),
    (6.75, 0.1, 45),
    (7.0, 0.1, 41),
    (7.25, 0.1, 41),
    (7.5, 0.1, 38),
    (7.5, 0.1, 39),
    (2.0, 0.1, 56),
    (2.5, 0.1, 56),
    (4.0, 0.1, 51),
    (4.5, 0.1, 51),
    (5.0, 0.1, 51),
    (5.5, 0.1, 51),
];
/// Hi-hat a ottavi con l'aperto sul levare del quarto battito, shaker in sedicesimi.
fn hats() -> Vec<(f32, f32, u8)> {
    let mut v = Vec::new();
    for i in 0..8 {
        let b = i as f32 * 0.5;
        v.push((b, 0.1, if i == 7 { 46 } else { 42 }));
    }
    for i in 0..8 {
        v.push((4.0 + i as f32 * 0.25, 0.1, 54));
    }
    v
}

fn phrase_for(category: &str) -> Phrase {
    match category {
        "Bass" => BASS,
        "Pad" | "Keys" | "Base" | "FX" => CHORDS,
        _ => MELODY,
    }
}

/// Suona gli eventi e restituisce (L, R).
fn play(g: &mut dyn Generator, notes: &[(f32, f32, u8)], beats: f32, tail: f32, accent: bool) -> (Vec<f32>, Vec<f32>) {
    let beat = 60.0 / BPM * SR as f32;
    let total = (beats * beat) as usize + (tail * SR as f32) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let mut events: Vec<(usize, bool, u64, u8, f32)> = Vec::new();
    for (i, (start, len, key)) in notes.iter().enumerate() {
        // Accento sui battiti per la batteria, velocity costante per i synth.
        let vel = if accent && start.fract() != 0.0 { 0.7 } else { 0.85 };
        events.push(((start * beat) as usize, true, i as u64 + 1, *key, vel));
        events.push((((start + len) * beat) as usize, false, i as u64 + 1, *key, vel));
    }
    events.sort_by_key(|e| (e.0, e.1));
    let mut pos = 0;
    for (at, on, id, key, vel) in events.into_iter().chain(std::iter::once((total, false, 0, 0, 0.0))) {
        if at > pos {
            g.process(&mut l[pos..at], &mut r[pos..at]);
            pos = at;
        }
        if id == 0 {
            continue;
        }
        if on {
            g.note_on(NoteOn::simple(id, key, vel));
        } else {
            g.note_off(id);
        }
    }
    (l, r)
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut groove = BEAT.to_vec();
    groove.extend(hats());
    groove.extend(hats().into_iter().map(|(b, l, k)| (b + 4.0, l, k)));
    for (id, file) in [
        (ids::FUSION, "fusion.wav"),
        (ids::PRISM, "prism.wav"),
        (ids::DRUMKIT, "drumkit.wav"),
    ] {
        let desc = descriptor(id).unwrap();
        let out = dir.join(file);
        let mut wav = hound::WavWriter::create(&out, spec).unwrap();
        println!("== {}", desc.name);
        for p in presets(id) {
            let mut inst = default_instance(id).unwrap();
            for (k, v) in preset_values(desc, p) {
                inst.params.insert(k.to_string(), v);
            }
            let mut g = create_generator(&inst, SR as f32);
            let (l, r) = if id == ids::DRUMKIT {
                play(g.as_mut(), &groove, 8.0, 1.5, true)
            } else {
                play(g.as_mut(), phrase_for(p.category), 4.0, 1.5, false)
            };
            println!(
                "{:<16} {:<8} picco {:.2}",
                p.name,
                p.category,
                l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()))
            );
            for (a, b) in l.iter().zip(&r) {
                wav.write_sample((a.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
                wav.write_sample((b.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
            }
        }
        wav.finalize().unwrap();
        println!("→ {}", out.display());
    }
}
