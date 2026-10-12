//! Uscita audio realtime tramite cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::engine::Engine;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("nessun dispositivo di uscita disponibile")]
    NoDevice,
    #[error("configurazione dispositivo: {0}")]
    Config(String),
    #[error("stream: {0}")]
    Stream(String),
}

/// Stream audio attivo; il motore vive nel callback.
pub struct DeviceBackend {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Sample rate del dispositivo di default (per creare il motore con la frequenza corretta).
pub fn default_sample_rate() -> Result<u32, DeviceError> {
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or(DeviceError::NoDevice)?;
    let cfg = dev
        .default_output_config()
        .map_err(|e| DeviceError::Config(e.to_string()))?;
    Ok(cfg.sample_rate())
}

impl DeviceBackend {
    /// Avvia lo stream sul dispositivo di default. Il motore deve essere creato con
    /// [`default_sample_rate`].
    pub fn start(mut engine: Engine) -> Result<Self, DeviceError> {
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or(DeviceError::NoDevice)?;
        let supported = dev
            .default_output_config()
            .map_err(|e| DeviceError::Config(e.to_string()))?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(DeviceError::Config(format!(
                "formato {:?} non supportato",
                supported.sample_format()
            )));
        }
        let config: cpal::StreamConfig = supported.into();
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
