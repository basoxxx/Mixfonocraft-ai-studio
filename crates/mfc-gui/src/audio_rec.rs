//! Registrazione audio (microfono/strumento) collegata al pulsante Record.
//!
//! Se l'ingresso audio è attivo, la registrazione scrive un WAV dall'ingresso scelto; allo
//! stop la ripresa diventa un canale Audio Clip posizionato in Playlist dove è iniziata.
//! Senza la feature `realtime` l'ingresso non esiste e queste funzioni non fanno nulla.

use std::path::PathBuf;

use mfc_core::{ChannelKind, ClipSource, Command, Outcome};

use crate::state::Studio;

/// Impostazioni e stato della registrazione audio.
#[derive(Default)]
pub struct AudioRec {
    /// Registra anche l'audio dall'ingresso quando si preme Record.
    pub enabled: bool,
    /// Dispositivo d'ingresso (`None` = predefinito).
    pub device: Option<String>,
    /// Ripresa in corso.
    #[cfg(feature = "realtime")]
    active: Option<(mfc_engine::backend::device::InputRecorder, f64)>,
    /// Numero progressivo delle riprese.
    #[cfg_attr(not(feature = "realtime"), allow(dead_code))]
    takes: u32,
}

impl AudioRec {
    /// `true` se una ripresa è in corso.
    pub fn is_active(&self) -> bool {
        #[cfg(feature = "realtime")]
        {
            self.active.is_some()
        }
        #[cfg(not(feature = "realtime"))]
        {
            false
        }
    }

    /// Livello di picco dell'ingresso dall'ultima lettura (0 senza ripresa).
    pub fn level(&self) -> f32 {
        #[cfg(feature = "realtime")]
        if let Some((r, _)) = &self.active {
            return r.level();
        }
        0.0
    }

    /// Dispositivi d'ingresso disponibili.
    pub fn devices() -> Vec<String> {
        #[cfg(feature = "realtime")]
        {
            mfc_engine::backend::device::input_devices()
        }
        #[cfg(not(feature = "realtime"))]
        {
            Vec::new()
        }
    }
}

/// Cartella delle riprese: accanto al progetto salvato, altrimenti nella cartella utente.
fn takes_dir(studio: &Studio) -> PathBuf {
    match &studio.path {
        Some(p) => {
            let stem = p
                .file_stem()
                .map_or("progetto".into(), |s| s.to_string_lossy().into_owned());
            p.with_file_name(format!("{stem} Registrazioni"))
        }
        None => crate::plugins::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("Registrazioni"),
    }
}

/// Nome del file della ripresa `n`.
pub fn take_file_name(n: u32, unix_secs: u64) -> String {
    format!("Ripresa {n:02} ({unix_secs}).wav")
}

impl Studio {
    /// Avvia la ripresa audio (se attiva nelle impostazioni). Errori → messaggio di stato.
    pub(crate) fn start_audio_take(&mut self) {
        if !self.audio_rec.enabled {
            return;
        }
        #[cfg(feature = "realtime")]
        {
            let dir = takes_dir(self);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                self.status = format!("Registrazione audio: {e}");
                return;
            }
            self.audio_rec.takes += 1;
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let path = dir.join(take_file_name(self.audio_rec.takes, secs));
            let start = self.engine.shared.position();
            match mfc_engine::backend::device::InputRecorder::start(self.audio_rec.device.as_deref(), &path) {
                Ok(r) => {
                    self.audio_rec.active = Some((r, start));
                    self.status = "Registrazione audio in corso…".into();
                }
                Err(e) => self.status = format!("Ingresso audio non disponibile: {e}"),
            }
        }
        #[cfg(not(feature = "realtime"))]
        {
            let _ = takes_dir(self);
            self.status = "Registrazione audio non disponibile in questa build".into();
        }
    }

    /// Chiude la ripresa in corso e la mette in Playlist come Audio Clip.
    pub(crate) fn finish_audio_take(&mut self) {
        #[cfg(feature = "realtime")]
        {
            let Some((rec, start_tick)) = self.audio_rec.active.take() else {
                return;
            };
            match rec.stop() {
                Ok(take) if take.frames > 0 => {
                    let place = mfc_engine::take_placement(
                        &self.project.time,
                        start_tick,
                        take.frames,
                        take.sample_rate,
                        self.project.tempo,
                    );
                    self.place_take(&take.path, place.position, place.length);
                }
                Ok(_) => self.status = "Ripresa audio vuota".into(),
                Err(e) => self.status = format!("Registrazione audio: {e}"),
            }
        }
    }

    /// Crea un canale Audio Clip per il file e lo posiziona in Playlist.
    pub fn place_take(&mut self, path: &std::path::Path, position: mfc_core::Tick, length: mfc_core::Tick) {
        let name = path
            .file_stem()
            .map_or_else(|| "Ripresa".to_string(), |s| s.to_string_lossy().into_owned());
        let Some(plugin) = mfc_plugins::default_instance(mfc_plugins::ids::AUDIO_CLIP) else {
            return;
        };
        let Some(Outcome::Channel(channel)) = self.exec(Command::AddChannel {
            name: name.clone(),
            kind: ChannelKind::AudioClip,
            plugin,
            mixer_insert: None,
        }) else {
            return;
        };
        self.exec(Command::SetChannelSample {
            channel,
            path: Some(path.display().to_string()),
        });
        let track = crate::views::browser::next_free_track(&self.project);
        self.exec(Command::PlaceClip {
            track,
            position,
            length: Some(length.max(1)),
            source: ClipSource::Audio(channel),
        });
        self.status = format!("Ripresa audio aggiunta in Playlist: {name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_names_are_ordered() {
        assert_eq!(take_file_name(3, 1700000000), "Ripresa 03 (1700000000).wav");
    }

    #[test]
    fn take_becomes_audio_clip_in_playlist() {
        let mut s = Studio::new(mfc_core::Project::new("t"));
        let clips = s.project.playlist.clips.len();
        s.place_take(std::path::Path::new("/tmp/Ripresa 01.wav"), 384, 768);
        let ch = s.project.channels.last().unwrap();
        assert_eq!(ch.kind, ChannelKind::AudioClip);
        assert_eq!(ch.plugin.sample_path.as_deref(), Some("/tmp/Ripresa 01.wav"));
        assert_eq!(s.project.playlist.clips.len(), clips + 1);
        let clip = s.project.playlist.clips.last().unwrap();
        assert_eq!((clip.position, clip.length), (384, 768));
        assert_eq!(clip.source, ClipSource::Audio(ch.id));
    }
}
