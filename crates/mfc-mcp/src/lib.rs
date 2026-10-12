//! # mfc-mcp
//!
//! Server [Model Context Protocol](https://modelcontextprotocol.io) per Mixfonocraft:
//! espone Channel Rack, step sequencer, piano roll, Playlist, mixer, automazioni,
//! render, transport e import/export FLP/MIDI come tool richiamabili da agenti AI.

mod flp;
pub mod protocol;
pub mod session;
pub mod tools;

pub use protocol::Server;
pub use session::Session;
