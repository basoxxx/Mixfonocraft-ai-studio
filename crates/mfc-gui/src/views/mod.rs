//! Viste dell'interfaccia.

pub mod browser;
pub mod channel_rack;
pub mod mixer;
pub mod piano_roll;
pub mod playlist;
pub mod plugin_editor;
pub mod samples;
pub mod settings;
pub mod synth_visuals;
pub mod update_ui;
pub mod widgets;

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Nome nota stile FL Studio (C5 = 60).
pub fn key_name(key: u8) -> String {
    format!("{}{}", NOTE_NAMES[(key % 12) as usize], key / 12)
}
