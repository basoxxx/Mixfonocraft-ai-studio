//! Backend di uscita: `null` (thread a tempo reale senza audio, utile headless/MCP)
//! e `cpal` (scheda audio, feature `realtime`).

#[cfg(feature = "realtime")]
pub mod device;
pub mod null;
