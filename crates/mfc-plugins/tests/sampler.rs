//! Sampler e audio clip con un WAV temporaneo.

use mfc_plugins::{create_generator, default_instance, ids, Generator, NoteOn};
use std::f32::consts::TAU;
use std::path::PathBuf;

const SR: f32 = 48_000.0;

/// Percorso temporaneo univoco, rimosso al drop.
struct TempWav(PathBuf);

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_sine(tag: &str, hz: f32, sample_rate: u32, seconds: f32, channels: u16) -> TempWav {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!("mfc_test_{tag}_{}_{nanos}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).expect("crea wav");
    let n = (seconds * sample_rate as f32) as usize;
    for i in 0..n {
        let v = (0.5 * (TAU * hz * i as f32 / sample_rate as f32).sin() * 32_767.0) as i16;
        for _ in 0..channels {
            w.write_sample(v).expect("scrivi");
        }
    }
    w.finalize().expect("finalizza");
    TempWav(path)
}

fn make(id: &str, wav: &TempWav, params: &[(&str, f32)]) -> Box<dyn Generator> {
    let mut inst = default_instance(id).unwrap();
    inst.sample_path = Some(wav.0.to_string_lossy().into_owned());
    for (k, v) in params {
        inst.params.insert((*k).to_string(), *v);
    }
    create_generator(&inst, SR)
}

fn render(g: &mut dyn Generator, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (seconds * SR) as usize;
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    for (bl, br) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        g.process(bl, br);
    }
    (l, r)
}

fn zero_cross_freq(x: &[f32]) -> f32 {
    let idx: Vec<usize> = (1..x.len()).filter(|&i| x[i - 1] < 0.0 && x[i] >= 0.0).collect();
    if idx.len() < 2 {
        return 0.0;
    }
    (idx.len() - 1) as f32 * SR / (idx[idx.len() - 1] - idx[0]) as f32
}

fn note(id: u64, key: u8) -> NoteOn {
    let mut n = NoteOn::simple(id, key, 1.0);
    n.root_key = 60;
    n
}

#[test]
fn sampler_pitch_follows_key_and_resamples() {
    // 44.1 kHz → riprodotto a 48 kHz: la compensazione del sample rate è necessaria.
    let wav = write_sine("pitch", 1_000.0, 44_100, 1.0, 1);
    let mut g = make(ids::SAMPLER, &wav, &[]);
    g.note_on(note(1, 60));
    let (l, r) = render(g.as_mut(), 0.5);
    let f = zero_cross_freq(&l);
    assert!((f - 1_000.0).abs() < 10.0, "root: {f}");
    assert!((zero_cross_freq(&r) - 1_000.0).abs() < 10.0);
    assert!(l.iter().all(|x| x.abs() < 1.0));

    let mut g = make(ids::SAMPLER, &wav, &[]);
    g.note_on(note(1, 72));
    let (l, _) = render(g.as_mut(), 0.4);
    let f = zero_cross_freq(&l);
    assert!((f - 2_000.0).abs() < 20.0, "root+12: {f}");

    // pitch_semitones e pitch del canale.
    let mut g = make(ids::SAMPLER, &wav, &[("pitch_semitones", -12.0)]);
    g.set_pitch_offset(1200.0);
    g.note_on(note(1, 60));
    let (l, _) = render(g.as_mut(), 0.5);
    assert!((zero_cross_freq(&l) - 1_000.0).abs() < 10.0);
}

#[test]
fn sampler_one_shot_and_envelope() {
    let wav = write_sine("oneshot", 500.0, 48_000, 0.3, 2);
    // Inviluppo disattivato: il campione suona fino in fondo anche dopo il note off.
    let mut g = make(ids::SAMPLER, &wav, &[]);
    g.note_on(note(1, 60));
    render(g.as_mut(), 0.05);
    g.note_off(1);
    let (l, _) = render(g.as_mut(), 0.1);
    assert!(l.iter().any(|x| x.abs() > 0.3));
    render(g.as_mut(), 0.3);
    assert_eq!(g.active_voices(), 0);

    // Con inviluppo: il note off avvia il release.
    let mut g = make(ids::SAMPLER, &wav, &[("env_amount", 1.0), ("release", 0.02)]);
    g.note_on(note(1, 60));
    render(g.as_mut(), 0.05);
    g.note_off(1);
    render(g.as_mut(), 0.05);
    assert_eq!(g.active_voices(), 0);

    // Loop: continua oltre la durata del campione finché non arriva il note off.
    let mut g = make(ids::SAMPLER, &wav, &[("loop", 1.0)]);
    g.note_on(note(1, 60));
    let (l, _) = render(g.as_mut(), 1.0);
    assert!(l[40_000..].iter().any(|x| x.abs() > 0.3));
    assert_eq!(g.active_voices(), 1);
    g.note_off(1);
    render(g.as_mut(), 0.05);
    assert_eq!(g.active_voices(), 0);

    // Reverse e start non fanno esplodere nulla.
    let mut g = make(
        ids::SAMPLER,
        &wav,
        &[("reverse", 1.0), ("start", 0.5), ("cutoff", 2_000.0)],
    );
    g.note_on(note(1, 60));
    let (l, _) = render(g.as_mut(), 0.3);
    assert!(l.iter().all(|x| x.is_finite()) && l.iter().any(|x| x.abs() > 0.1));
    assert_eq!(g.active_voices(), 0);
}

#[test]
fn audio_clip_keeps_original_pitch_and_offset() {
    let wav = write_sine("clip", 1_000.0, 48_000, 0.5, 1);
    let mut g = make(ids::AUDIO_CLIP, &wav, &[]);
    g.note_on(note(1, 84));
    let (l, _) = render(g.as_mut(), 0.3);
    assert!((zero_cross_freq(&l) - 1_000.0).abs() < 10.0);

    // Avvio a metà: termina prima.
    let mut g = make(ids::AUDIO_CLIP, &wav, &[]);
    let mut n = note(1, 60);
    n.start_offset_samples = 19_200;
    g.note_on(n);
    render(g.as_mut(), 0.11);
    assert_eq!(g.active_voices(), 0);

    // Note off: fade breve.
    let mut g = make(ids::AUDIO_CLIP, &wav, &[]);
    g.note_on(note(7, 60));
    render(g.as_mut(), 0.1);
    g.note_off(7);
    render(g.as_mut(), 0.02);
    assert_eq!(g.active_voices(), 0);
}
