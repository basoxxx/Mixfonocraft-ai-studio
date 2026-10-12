//! Backend di uscita: `null` (thread a tempo reale senza audio, utile headless/MCP)
//! e `cpal` (scheda audio, feature `realtime`).

pub mod null;
#[cfg(feature = "realtime")]
pub mod device;
