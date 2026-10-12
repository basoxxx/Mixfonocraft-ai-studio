//! Caricamento di campioni WAV (via `hound`) in [`SampleData`].

use crate::api::{SampleData, SharedSample};
use std::path::Path;
use std::sync::Arc;

/// Errori di caricamento di un campione.
#[derive(Debug, thiserror::Error)]
pub enum SampleError {
    #[error("impossibile leggere il WAV: {0}")]
    Wav(#[from] hound::Error),
    #[error("formato non supportato: {0}")]
    Unsupported(String),
    #[error("il file non contiene audio")]
    Empty,
}

/// Carica un WAV (PCM intero 8/16/24/32 bit o float 32 bit). Oltre 2 canali vengono tenuti i primi due.
/// NON chiamare dal thread audio.
pub fn load_sample(path: impl AsRef<Path>) -> Result<SharedSample, SampleError> {
    let reader = hound::WavReader::open(path.as_ref())?;
    read_wav(reader).map(Arc::new)
}

fn read_wav<R: std::io::Read>(mut reader: hound::WavReader<R>) -> Result<SampleData, SampleError> {
    let spec = reader.spec();
    let in_ch = spec.channels as usize;
    if in_ch == 0 || spec.sample_rate == 0 {
        return Err(SampleError::Unsupported(format!("{} canali a {} Hz", spec.channels, spec.sample_rate)));
    }
    let out_ch = in_ch.min(2);
    let frames = reader.duration() as usize;
    let mut channels = vec![Vec::with_capacity(frames); out_ch];
    let mut push = |i: usize, v: f32| {
        let c = i % in_ch;
        if c < out_ch {
            channels[c].push(if v.is_finite() { v } else { 0.0 });
        }
    };
    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => {
            for (i, s) in reader.samples::<f32>().enumerate() {
                push(i, s?);
            }
        }
        (hound::SampleFormat::Int, bits @ 1..=32) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f64;
            for (i, s) in reader.samples::<i32>().enumerate() {
                push(i, (s? as f64 * scale) as f32);
            }
        }
        (fmt, bits) => return Err(SampleError::Unsupported(format!("{fmt:?} a {bits} bit"))),
    }
    // Frame incompleto finale: pareggia le lunghezze.
    let len = channels.iter().map(Vec::len).min().unwrap_or(0);
    for c in &mut channels {
        c.truncate(len);
    }
    if len == 0 {
        return Err(SampleError::Empty);
    }
    Ok(SampleData { sample_rate: spec.sample_rate as f32, channels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("mfc_sample_{tag}_{}_{nanos}.wav", std::process::id()))
    }

    fn write(path: &Path, channels: u16, bits: u16, float: bool, frames: &[f32]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: 44_100,
            bits_per_sample: bits,
            sample_format: if float { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for &v in frames {
            if float {
                w.write_sample(v).unwrap();
            } else {
                let max = ((1i64 << (bits - 1)) - 1) as f32;
                let s = (v * max).round() as i32;
                if bits == 8 {
                    w.write_sample(s as i8).unwrap();
                } else if bits == 16 {
                    w.write_sample(s as i16).unwrap();
                } else {
                    w.write_sample(s).unwrap();
                }
            }
        }
        w.finalize().unwrap();
    }

    #[test]
    fn loads_all_formats() {
        let data = [0.5f32, -0.5, 0.25, -0.25];
        for (bits, float) in [(8, false), (16, false), (24, false), (32, false), (32, true)] {
            for ch in [1u16, 2] {
                let p = temp_path(&format!("{bits}_{float}_{ch}"));
                write(&p, ch, bits, float, &data);
                let s = load_sample(&p).unwrap();
                let _ = std::fs::remove_file(&p);
                assert_eq!(s.channels.len(), ch as usize);
                assert_eq!(s.len(), data.len() / ch as usize);
                assert_eq!(s.sample_rate, 44_100.0);
                let tol = if bits == 8 { 0.02 } else { 1e-3 };
                assert!((s.channels[0][0] - 0.5).abs() < tol, "{bits} {float} {ch}: {}", s.channels[0][0]);
                if ch == 2 {
                    assert!((s.channels[1][0] + 0.5).abs() < tol);
                }
            }
        }
    }

    #[test]
    fn missing_file_is_error() {
        assert!(load_sample("/definitely/not/here.wav").is_err());
        let p = temp_path("garbage");
        std::fs::write(&p, b"not a wav").unwrap();
        assert!(load_sample(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
