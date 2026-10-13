//! # mfc-engine
//!
//! Motore audio di Mixfonocraft: transport (Song/Pattern), sequencer sample-accurate,
//! Channel Rack → Mixer (125 insert + master, 10 slot effetto, mandate, solo/mute),
//! clip di automazione, render offline su WAV con stem, backend realtime, registrazione
//! dell'ingresso audio ([`capture`]).

pub mod backend;
pub mod capture;
pub mod engine;
pub mod prepared;
pub mod render;

pub use capture::{
    capture_channel, take_placement, CaptureConsumer, CaptureProducer, CaptureWriter, LevelMeter, RecordedTake,
    TakePlacement,
};
pub use engine::{Engine, EngineCommand, EngineHandle, PlayMode, SharedState, CHUNK};
pub use prepared::Prepared;
pub use render::{render, render_to_file, write_wav, Audio, RenderError, RenderOptions, Rendered, Stem, WavFormat};
