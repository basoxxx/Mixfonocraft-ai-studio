//! Ascolto dei preset di Nova: `cargo run -p mfc-plugins --release --example nova_presets -- nova.wav`.
//! Ogni preset suona una breve frase (basso, accordi o melodia secondo la categoria).

use mfc_plugins::{create_generator, default_instance, descriptor, ids, preset_values, presets, Generator, NoteOn};

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

fn phrase_for(category: &str) -> Phrase {
    match category {
        "Bass" => BASS,
        "Pad" | "Keys" | "Base" | "FX" => CHORDS,
        _ => MELODY,
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "nova_presets.wav".into());
    let desc = descriptor(ids::NOVA).unwrap();
    let beat = 60.0 / BPM * SR as f32;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = hound::WavWriter::create(&out, spec).unwrap();
    for p in presets(ids::NOVA) {
        let mut inst = default_instance(ids::NOVA).unwrap();
        for (k, v) in preset_values(desc, p) {
            inst.params.insert(k.to_string(), v);
        }
        let mut g: Box<dyn Generator> = create_generator(&inst, SR as f32);
        let phrase = phrase_for(p.category);
        // 4 battiti di frase + 1.5 s di coda.
        let total = (4.0 * beat) as usize + SR as usize * 3 / 2;
        let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
        let mut events: Vec<(usize, bool, u64, u8)> = Vec::new();
        for (i, (start, len, key)) in phrase.iter().enumerate() {
            events.push(((start * beat) as usize, true, i as u64 + 1, *key));
            events.push((((start + len) * beat) as usize, false, i as u64 + 1, *key));
        }
        events.sort_by_key(|e| (e.0, e.1));
        let mut pos = 0;
        for (at, on, id, key) in events.into_iter().chain(std::iter::once((total, false, 0, 0))) {
            if at > pos {
                g.process(&mut l[pos..at], &mut r[pos..at]);
                pos = at;
            }
            if id == 0 {
                continue;
            }
            if on {
                g.note_on(NoteOn::simple(id, key, 0.85));
            } else {
                g.note_off(id);
            }
        }
        println!(
            "{:<16} {:<6} picco {:.2}",
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
    println!("→ {out}");
}
