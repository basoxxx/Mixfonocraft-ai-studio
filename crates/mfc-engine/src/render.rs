//! Render offline (bounce) su buffer e file WAV, con stem per insert del mixer.

use std::path::Path;

use mfc_core::{PatternId, Project};

use crate::engine::{Engine, PlayMode};

/// Formato del file WAV in uscita.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WavFormat {
    Int16,
    #[default]
    Int24,
    Float32,
}

/// Opzioni di render.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub sample_rate: u32,
    /// `None` = brano intero (Song), `Some(p)` = un singolo pattern.
    pub pattern: Option<PatternId>,
    /// Ripetizioni del pattern (solo modalità pattern).
    pub pattern_loops: u32,
    /// Coda dopo la fine (riverberi, release), in secondi.
    pub tail_seconds: f64,
    /// Durata massima di sicurezza in secondi.
    pub max_seconds: f64,
    /// Normalizza il picco a -0.3 dBFS.
    pub normalize: bool,
    /// Cattura gli stem post-fader per ogni insert con segnale.
    pub stems: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            sample_rate: 44_100,
            pattern: None,
            pattern_loops: 1,
            tail_seconds: 2.0,
            max_seconds: 60.0 * 30.0,
            normalize: false,
            stems: false,
        }
    }
}

/// Audio stereo renderizzato.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Audio {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.left.len()
    }
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }
    pub fn peak(&self) -> f32 {
        self.left.iter().chain(&self.right).fold(0.0, |m, x| m.max(x.abs()))
    }
    pub fn rms(&self) -> f32 {
        let n = (self.left.len() + self.right.len()).max(1);
        (self.left.iter().chain(&self.right).map(|x| x * x).sum::<f32>() / n as f32).sqrt()
    }
    fn scale(&mut self, g: f32) {
        self.left.iter_mut().chain(self.right.iter_mut()).for_each(|x| *x *= g);
    }
}

/// Stem di un insert.
#[derive(Debug, Clone)]
pub struct Stem {
    pub insert: u16,
    pub name: String,
    pub audio: Audio,
}

/// Risultato del render.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub mix: Audio,
    pub stems: Vec<Stem>,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("pattern {0} inesistente")]
    NoPattern(PatternId),
    #[error("errore WAV: {0}")]
    Wav(#[from] hound::Error),
}

/// Renderizza il progetto in memoria.
pub fn render(project: &Project, opts: &RenderOptions) -> Result<Rendered, RenderError> {
    let sr = opts.sample_rate.max(8000);
    let (mut engine, _handle) = Engine::new(project, sr as f32);
    if let Some(p) = opts.pattern {
        if project.pattern(p).is_none() {
            return Err(RenderError::NoPattern(p));
        }
        engine.set_mode(PlayMode::Pattern(p));
    }
    let n_ins = engine.insert_names().len();
    if opts.stems {
        engine.capture = Some(vec![(Vec::new(), Vec::new()); n_ins]);
    }
    let target = match opts.pattern {
        Some(_) => engine.sequence_length() * opts.pattern_loops.max(1) as f64,
        None => engine.sequence_length(),
    };
    engine.play();
    let max_frames = (opts.max_seconds * sr as f64) as usize;
    let block = 512;
    let mut mix = Audio { sample_rate: sr, left: Vec::new(), right: Vec::new() };
    let mut l = vec![0.0f32; block];
    let mut r = vec![0.0f32; block];
    // In modalità pattern il motore va in loop: si conta il tempo trascorso in tick.
    let mut elapsed_ticks = 0.0;
    let mut last_pos = engine.position();
    while elapsed_ticks < target && mix.frames() < max_frames {
        engine.process(&mut l, &mut r);
        mix.left.extend_from_slice(&l);
        mix.right.extend_from_slice(&r);
        let pos = engine.position();
        elapsed_ticks += if pos >= last_pos { pos - last_pos } else { engine.sequence_length() - last_pos + pos };
        last_pos = pos;
    }
    // Taglio preciso alla fine musicale.
    let raw_body = mix.frames();
    let exact = (project.time.ticks_to_seconds(target, project.tempo) * sr as f64).round() as usize;
    let has_tempo_auto = project
        .automations
        .iter()
        .any(|a| matches!(a.target, mfc_core::AutomationTarget::Tempo));
    if !has_tempo_auto && exact <= mix.frames() {
        mix.left.truncate(exact);
        mix.right.truncate(exact);
    }
    let body = mix.frames();
    // Coda: stop del sequencer, le voci terminano il release.
    engine.pause();
    let tail = (opts.tail_seconds.max(0.0) * sr as f64) as usize;
    let mut written = 0;
    while written < tail {
        let n = block.min(tail - written);
        engine.process(&mut l[..n], &mut r[..n]);
        mix.left.extend_from_slice(&l[..n]);
        mix.right.extend_from_slice(&r[..n]);
        written += n;
    }

    let mut stems = Vec::new();
    if let Some(cap) = engine.capture.take() {
        let total = mix.frames();
        let names = engine.insert_names().to_vec();
        for (i, (mut cl, mut cr)) in cap.into_iter().enumerate() {
            if i == mfc_core::MASTER as usize {
                continue;
            }
            let peak = cl.iter().chain(&cr).fold(0f32, |m, x| m.max(x.abs()));
            if peak <= 1e-6 {
                continue;
            }
            // Stesso taglio applicato al mix.
            if raw_body > body && cl.len() >= raw_body {
                cl.drain(body..raw_body);
                cr.drain(body..raw_body);
            }
            cl.resize(total, 0.0);
            cr.resize(total, 0.0);
            stems.push(Stem {
                insert: i as u16,
                name: names.get(i).cloned().unwrap_or_default(),
                audio: Audio { sample_rate: sr, left: cl, right: cr },
            });
        }
    }

    if opts.normalize {
        let peak = mix.peak();
        if peak > 1e-6 {
            let g = 10f32.powf(-0.3 / 20.0) / peak;
            mix.scale(g);
            for s in &mut stems {
                s.audio.scale(g);
            }
        }
    }
    Ok(Rendered { mix, stems })
}

/// Scrive audio stereo su WAV.
pub fn write_wav(path: impl AsRef<Path>, audio: &Audio, format: WavFormat) -> Result<(), RenderError> {
    let (bits, fmt) = match format {
        WavFormat::Int16 => (16, hound::SampleFormat::Int),
        WavFormat::Int24 => (24, hound::SampleFormat::Int),
        WavFormat::Float32 => (32, hound::SampleFormat::Float),
    };
    let spec = hound::WavSpec { channels: 2, sample_rate: audio.sample_rate, bits_per_sample: bits, sample_format: fmt };
    let mut w = hound::WavWriter::create(path, spec)?;
    for (l, r) in audio.left.iter().zip(&audio.right) {
        for s in [*l, *r] {
            match format {
                WavFormat::Float32 => w.write_sample(s)?,
                WavFormat::Int16 => w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?,
                WavFormat::Int24 => w.write_sample((s.clamp(-1.0, 1.0) * 8_388_607.0) as i32)?,
            }
        }
    }
    w.finalize()?;
    Ok(())
}

/// Renderizza e scrive il mix (e gli stem, se richiesti, come `<stem>_<n>_<nome>.wav` accanto).
pub fn render_to_file(
    project: &Project,
    path: impl AsRef<Path>,
    opts: &RenderOptions,
    format: WavFormat,
) -> Result<Rendered, RenderError> {
    let path = path.as_ref();
    let out = render(project, opts)?;
    write_wav(path, &out.mix, format)?;
    if opts.stems {
        let stem_base = path.with_extension("");
        for s in &out.stems {
            let safe: String = s.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
            let p = format!("{}_{:03}_{}.wav", stem_base.display(), s.insert, safe);
            write_wav(p, &s.audio, format)?;
        }
    }
    Ok(out)
}
