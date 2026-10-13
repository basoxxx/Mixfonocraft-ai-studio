//! Uscita audio realtime tramite cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, SupportedBufferSize};

use crate::engine::Engine;

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
