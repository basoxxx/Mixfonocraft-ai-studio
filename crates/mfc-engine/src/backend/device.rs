//! Uscita audio realtime tramite cpal e registrazione dell'ingresso ([`InputRecorder`]).

use std::path::Path;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, SupportedBufferSize};

use crate::capture::{capture_channel, CaptureProducer, CaptureWriter, RecordedTake};
use crate::engine::Engine;
use crate::render::WavFormat;

/// Frequenze di campionamento proposte quando il dispositivo non dichiara le proprie.
pub const STANDARD_RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("nessun dispositivo di uscita disponibile")]
    NoDevice,
    #[error("configurazione dispositivo: {0}")]
    Config(String),
    #[error("stream: {0}")]
    Stream(String),
}

/// Scelta dell'utente per l'uscita audio. `None` = valore predefinito di sistema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceConfig {
    /// Nome del dispositivo di uscita.
    pub device: Option<String>,
    /// Frequenza di campionamento in Hz.
    pub sample_rate: Option<u32>,
    /// Dimensione del buffer in campioni (frame).
    pub buffer_size: Option<u32>,
}

/// Stream audio attivo; il motore vive nel callback.
pub struct DeviceBackend {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Nomi dei dispositivi di uscita disponibili sull'host di default.
pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(devices) => devices.filter_map(|d| device_name(&d)).collect(),
        Err(_) => Vec::new(),
    }
}

/// Frequenze di campionamento supportate dal dispositivo (`None` = default di sistema).
///
/// Se cpal non riesce a interrogare il dispositivo, restituisce [`STANDARD_RATES`].
pub fn supported_sample_rates(device: Option<&str>) -> Vec<u32> {
    let host = cpal::default_host();
    let Ok(dev) = find_device(&host, device) else {
        return STANDARD_RATES.to_vec();
    };
    let Ok(configs) = dev.supported_output_configs() else {
        return STANDARD_RATES.to_vec();
    };
    let ranges: Vec<(u32, u32)> = configs
        .filter(|c| c.sample_format() == SampleFormat::F32)
        .map(|c| (c.min_sample_rate(), c.max_sample_rate()))
        .collect();
    STANDARD_RATES
        .into_iter()
        .filter(|r| ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(r)))
        .collect()
}

/// Sample rate del dispositivo di default (per creare il motore con la frequenza corretta).
pub fn default_sample_rate() -> Result<u32, DeviceError> {
    resolved_sample_rate(&DeviceConfig::default())
}

/// Frequenza che [`DeviceBackend::start_with`] userà per questa configurazione:
/// il motore va creato con questo valore.
pub fn resolved_sample_rate(cfg: &DeviceConfig) -> Result<u32, DeviceError> {
    let host = cpal::default_host();
    let dev = find_device(&host, cfg.device.as_deref())?;
    Ok(choose_stream(&dev, cfg)?.sample_rate)
}

fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

fn find_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, DeviceError> {
    let Some(name) = name else {
        return host.default_output_device().ok_or(DeviceError::NoDevice);
    };
    let devices = host.output_devices().map_err(|e| DeviceError::Config(e.to_string()))?;
    devices
        .into_iter()
        .find(|d| device_name(d).as_deref() == Some(name))
        .ok_or_else(|| DeviceError::Config(format!("dispositivo \"{name}\" non trovato")))
}

/// Sceglie la configurazione di stream (f32, stessi canali del default, frequenza e buffer richiesti).
fn choose_stream(dev: &cpal::Device, cfg: &DeviceConfig) -> Result<cpal::StreamConfig, DeviceError> {
    let default = dev
        .default_output_config()
        .map_err(|e| DeviceError::Config(e.to_string()))?;
    let channels = default.channels();
    let want = cfg.sample_rate.unwrap_or(default.sample_rate());
    let ranges: Vec<_> = dev
        .supported_output_configs()
        .map(|it| it.collect())
        .unwrap_or_default();
    let range = ranges.iter().find(|r| {
        r.sample_format() == SampleFormat::F32
            && r.channels() == channels
            && (r.min_sample_rate()..=r.max_sample_rate()).contains(&want)
    });
    if range.is_none() && (want != default.sample_rate() || default.sample_format() != SampleFormat::F32) {
        return Err(DeviceError::Config(format!(
            "formato {:?} a {want} Hz non supportato",
            default.sample_format()
        )));
    }
    // Buffer richiesto solo se rientra nei limiti dichiarati dal dispositivo.
    let buffer_size = match (cfg.buffer_size, range.map(|r| *r.buffer_size())) {
        (Some(n), Some(SupportedBufferSize::Range { min, max })) if (min..=max).contains(&n) => BufferSize::Fixed(n),
        _ => BufferSize::Default,
    };
    Ok(cpal::StreamConfig {
        channels,
        sample_rate: want,
        buffer_size,
    })
}

impl DeviceBackend {
    /// Avvia lo stream sul dispositivo di default. Il motore deve essere creato con
    /// [`default_sample_rate`].
    pub fn start(engine: Engine) -> Result<Self, DeviceError> {
        Self::start_with(engine, &DeviceConfig::default())
    }

    /// Avvia lo stream con la configurazione scelta. Il motore deve essere creato con
    /// [`resolved_sample_rate`] per la stessa configurazione.
    pub fn start_with(mut engine: Engine, cfg: &DeviceConfig) -> Result<Self, DeviceError> {
        let host = cpal::default_host();
        let dev = find_device(&host, cfg.device.as_deref())?;
        let config = choose_stream(&dev, cfg)?;
        if config.sample_rate as f32 != engine.sample_rate() {
            return Err(DeviceError::Config(format!(
                "il motore usa {} Hz, il dispositivo {} Hz",
                engine.sample_rate(),
                config.sample_rate
            )));
        }
        let channels = config.channels;
        let sample_rate = config.sample_rate;
        let mut l = vec![0.0f32; 8192];
        let mut r = vec![0.0f32; 8192];
        let stream = dev
            .build_output_stream(
                config,
                move |data: &mut [f32], _| {
                    let ch = channels.max(1) as usize;
                    for frame_block in data.chunks_mut(8192 * ch) {
                        let frames = frame_block.len() / ch;
                        engine.process(&mut l[..frames], &mut r[..frames]);
                        for (i, f) in frame_block.chunks_mut(ch).enumerate() {
                            f[0] = l[i];
                            if ch > 1 {
                                f[1] = r[i];
                            }
                            for x in f.iter_mut().skip(2) {
                                *x = 0.0;
                            }
                        }
                    }
                },
                |e| eprintln!("errore stream audio: {e}"),
                None,
            )
            .map_err(|e| DeviceError::Stream(e.to_string()))?;
        stream.play().map_err(|e| DeviceError::Stream(e.to_string()))?;
        Ok(Self {
            _stream: stream,
            sample_rate,
            channels,
        })
    }
}

// ---------------------------------------------------------------------------------------
// Ingresso audio (registrazione)
// ---------------------------------------------------------------------------------------

/// Secondi di audio che il ring d'ingresso può contenere prima di scartare frame.
const INPUT_RING_SECONDS: u32 = 4;

/// Nomi dei dispositivi d'ingresso disponibili sull'host di default.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|d| device_name(&d)).collect(),
        Err(_) => Vec::new(),
    }
}

fn find_input_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, String> {
    let Some(name) = name else {
        return host
            .default_input_device()
            .ok_or_else(|| "nessun dispositivo d'ingresso disponibile".to_string());
    };
    let devices = host.input_devices().map_err(|e| e.to_string())?;
    devices
        .into_iter()
        .find(|d| device_name(d).as_deref() == Some(name))
        .ok_or_else(|| format!("dispositivo d'ingresso \"{name}\" non trovato"))
}

/// Registrazione in corso da un dispositivo d'ingresso su file WAV stereo.
///
/// La callback cpal converte i campioni in `f32` stereo (mono duplicato) e li accoda in
/// un ring lock-free ([`crate::capture`]); un thread li scrive su disco alla frequenza del
/// dispositivo. Nessun monitoraggio dell'ingresso: l'audio registrato non passa dal mixer.
/// Per posare la take nella Playlist vedi [`crate::capture::take_placement`].
pub struct InputRecorder {
    stream: cpal::Stream,
    writer: CaptureWriter,
    sample_rate: u32,
    channels: u16,
}

impl InputRecorder {
    /// Avvia la registrazione (WAV 24 bit) dal dispositivo `device` (`None` = default)
    /// con la sua configurazione predefinita.
    pub fn start(device: Option<&str>, path: &Path) -> Result<Self, String> {
        Self::start_with_format(device, path, WavFormat::Int24)
    }

    /// Come [`InputRecorder::start`], scegliendo il formato del file.
    pub fn start_with_format(device: Option<&str>, path: &Path, format: WavFormat) -> Result<Self, String> {
        let host = cpal::default_host();
        let dev = find_input_device(&host, device)?;
        let supported = dev
            .default_input_config()
            .map_err(|e| format!("configurazione d'ingresso: {e}"))?;
        let sample_format = supported.sample_format();
        let config = supported.config();
        let sample_rate = config.sample_rate;
        let channels = config.channels;
        let (producer, consumer) = capture_channel((sample_rate * INPUT_RING_SECONDS) as usize);
        // In caso d'errore più avanti il writer, lasciato cadere, chiude il suo thread.
        let writer = CaptureWriter::spawn(consumer, path, sample_rate, format)?;
        let stream = match sample_format {
            SampleFormat::I8 => build_input::<i8>(&dev, config, producer),
            SampleFormat::I16 => build_input::<i16>(&dev, config, producer),
            SampleFormat::I24 => build_input::<cpal::I24>(&dev, config, producer),
            SampleFormat::I32 => build_input::<i32>(&dev, config, producer),
            SampleFormat::I64 => build_input::<i64>(&dev, config, producer),
            SampleFormat::U8 => build_input::<u8>(&dev, config, producer),
            SampleFormat::U16 => build_input::<u16>(&dev, config, producer),
            SampleFormat::U24 => build_input::<cpal::U24>(&dev, config, producer),
            SampleFormat::U32 => build_input::<u32>(&dev, config, producer),
            SampleFormat::U64 => build_input::<u64>(&dev, config, producer),
            SampleFormat::F32 => build_input::<f32>(&dev, config, producer),
            SampleFormat::F64 => build_input::<f64>(&dev, config, producer),
            other => Err(format!("formato d'ingresso {other:?} non supportato")),
        }?;
        stream.play().map_err(|e| format!("avvio stream d'ingresso: {e}"))?;
        Ok(Self {
            stream,
            writer,
            sample_rate,
            channels,
        })
    }

    /// Picco d'ingresso (lineare) dall'ultima lettura; la lettura lo azzera.
    pub fn level(&self) -> f32 {
        self.writer.meter().take_peak()
    }

    /// Frequenza del dispositivo (e del file).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Canali del dispositivo (il file è sempre stereo).
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Ferma la stream, completa e chiude il WAV.
    pub fn stop(self) -> Result<RecordedTake, String> {
        let Self { stream, writer, .. } = self;
        let _ = stream.pause();
        // Chiudere la stream ferma la callback (il produttore) prima di finalizzare il file.
        drop(stream);
        writer.finish()
    }
}

fn build_input<T>(
    dev: &cpal::Device,
    config: cpal::StreamConfig,
    mut producer: CaptureProducer,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    dev.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            producer.push_interleaved(data, channels, |s| f32::from_sample_(s));
        },
        |e| eprintln!("errore stream d'ingresso: {e}"),
        None,
    )
    .map_err(|e| format!("stream d'ingresso: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Senza scheda audio non deve andare in panico: lista (anche vuota) o errore pulito.
    #[test]
    fn input_enumeration_and_missing_device_do_not_panic() {
        let _ = input_devices();
        let path = std::env::temp_dir().join(format!("mfc-input-{}.wav", std::process::id()));
        let res = InputRecorder::start(Some("dispositivo-inesistente-mfc"), &path);
        assert!(res.is_err());
        let _ = std::fs::remove_file(&path);
    }
}
