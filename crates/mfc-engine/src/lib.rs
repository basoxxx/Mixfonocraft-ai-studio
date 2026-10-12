//! # mfc-engine
//!
//! Motore audio di Mixfonocraft: transport (Song/Pattern), sequencer sample-accurate,
//! Channel Rack → Mixer (125 insert + master, 10 slot effetto, mandate, solo/mute),
//! clip di automazione, render offline su WAV con stem, backend realtime.

pub mod backend;
pub mod engine;
pub mod prepared;
pub mod render;

pub use engine::{Engine, EngineCommand, EngineHandle, PlayMode, SharedState, CHUNK};
pub use prepared::Prepared;
pub use render::{render, render_to_file, write_wav, Audio, RenderError, RenderOptions, Rendered, Stem, WavFormat};
