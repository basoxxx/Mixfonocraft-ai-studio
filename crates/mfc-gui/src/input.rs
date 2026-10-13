//! Ingresso live: tastiera MIDI (midir, feature `midi`), tastiera del computer usata come
//! pianoforte (stile "typing keyboard" di FL Studio) e registrazione nel pattern.
//!
//! Le note suonano subito tramite le note di anteprima del motore (anche dal thread MIDI,
//! senza passare dal frame della UI); la registrazione avviene sul thread UI in [`LiveInput::poll`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::Key;
use mfc_core::{ChannelId, Note, Tick, TimeBase};
use mfc_engine::EngineHandle;

/// Evento di nota live.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LiveEvent {
    On { key: u8, velocity: f32 },
    Off { key: u8 },
}

/// Mappa tastiera del computer → semitono relativo all'ottava base (layout FL Studio).
/// Fila inferiore: Z S X D C V G B H N J M , ; fila superiore: Q 2 W 3 E R 5 T 6 Y 7 U I 9 O 0 P.
pub fn key_to_semitone(key: Key) -> Option<i32> {
    use Key::*;
    Some(match key {
        Z => 0,
        S => 1,
        X => 2,
        D => 3,
        C => 4,
        V => 5,
        G => 6,
        B => 7,
        H => 8,
        N => 9,
        J => 10,
        M => 11,
        Comma => 12,
        Q => 12,
        Num2 => 13,
        W => 14,
        Num3 => 15,
        E => 16,
        R => 17,
        Num5 => 18,
        T => 19,
        Num6 => 20,
        Y => 21,
        Num7 => 22,
        U => 23,
        I => 24,
        Num9 => 25,
        O => 26,
        Num0 => 27,
        P => 28,
        _ => return None,
    })
}

/// Registratore: accoppia note on/off e produce note per il pattern.
#[derive(Debug, Default)]
pub struct Recorder {
    held: HashMap<u8, (f64, f32)>,
}

impl Recorder {
    /// Nota premuta alla posizione `pos` (tick relativi al pattern).
    pub fn on(&mut self, key: u8, velocity: f32, pos: f64) {
        self.held.insert(key, (pos, velocity));
    }

    /// Nota rilasciata: restituisce la nota da inserire. Gestisce il giro del loop del
    /// pattern (rilascio dopo il ritorno all'inizio). `snap` > 1 quantizza l'inizio.
    pub fn off(&mut self, key: u8, pos: f64, pattern_len: Tick, snap: Tick) -> Option<Note> {
        let (start, vel) = self.held.remove(&key)?;
        let len_total = pattern_len.max(1) as f64;
        let mut length = pos - start;
        if length < 0.0 {
            length += len_total;
        }
        let mut position = (start.max(0.0) as Tick) % pattern_len.max(1);
        if snap > 1 {
            position = TimeBase::quantize(position, snap) % pattern_len.max(1);
        }
        let length = (length.round() as Tick).max(1);
        Some(Note::new(position, length, key, vel.clamp(0.0, 1.0)))
    }

    /// Chiude tutte le note tenute (stop del transport) alla posizione data.
    pub fn flush(&mut self, pos: f64, pattern_len: Tick, snap: Tick) -> Vec<Note> {
        let keys: Vec<u8> = self.held.keys().copied().collect();
        keys.into_iter()
            .filter_map(|k| self.off(k, pos, pattern_len, snap))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

/// Stato dell'ingresso live.
pub struct LiveInput {
    engine: EngineHandle,
    /// Canale che riceve le note (0 = nessuno), condiviso con il thread MIDI.
    target: Arc<AtomicU32>,
    tx: Sender<LiveEvent>,
    rx: Receiver<LiveEvent>,
    /// Ottava base della tastiera del computer (C5 = 60 → ottava 5).
    pub octave: i32,
    /// Tastiera del computer attiva come pianoforte.
    pub typing_keyboard: bool,
    /// Velocity delle note da tastiera del computer.
    pub velocity: f32,
    pub recorder: Recorder,
    /// Tasti del computer premuti → nota suonata (per rilasciare la stessa nota anche se
    /// nel frattempo cambia l'ottava).
    pressed: HashMap<Key, u8>,
    #[cfg(feature = "midi")]
    midi: Option<midir::MidiInputConnection<()>>,
    /// Nome della porta MIDI collegata.
    pub midi_port: Option<String>,
}

impl LiveInput {
    pub fn new(engine: EngineHandle) -> Self {
        let (tx, rx) = unbounded();
        Self {
            engine,
            target: Arc::new(AtomicU32::new(0)),
            tx,
            rx,
            octave: 5,
            typing_keyboard: true,
            velocity: 0.8,
            recorder: Recorder::default(),
            pressed: HashMap::new(),
            #[cfg(feature = "midi")]
            midi: None,
            midi_port: None,
        }
    }

    /// Sostituisce l'handle del motore (dopo un riavvio dell'audio).
    pub fn set_engine(&mut self, engine: EngineHandle) {
        self.engine = engine;
    }

    /// Imposta il canale che suona le note live.
    pub fn set_target(&self, channel: Option<ChannelId>) {
        self.target.store(channel.map_or(0, |c| c.0), Ordering::Relaxed);
    }

    fn target_channel(&self) -> Option<ChannelId> {
        match self.target.load(Ordering::Relaxed) {
            0 => None,
            id => Some(ChannelId(id)),
        }
    }

    /// Suona e accoda un evento (usato da tastiera del computer e test).
    pub fn emit(&self, ev: LiveEvent) {
        if let Some(ch) = self.target_channel() {
            match ev {
                LiveEvent::On { key, velocity } => {
                    self.engine.preview_on(ch, key, velocity);
                }
                LiveEvent::Off { key } => {
                    self.engine.preview_off(ch, key);
                }
            }
        }
        let _ = self.tx.send(ev);
    }

    /// Legge gli eventi tastiera di egui. Va chiamato solo quando nessun campo di testo
    /// ha il focus. Restituisce `true` se ha consumato almeno un tasto.
    pub fn handle_keyboard(&mut self, ctx: &egui::Context) -> bool {
        if !self.typing_keyboard {
            return false;
        }
        let events = ctx.input(|i| i.events.clone());
        let mut used = false;
        for e in events {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = e
            else {
                continue;
            };
            if modifiers.command || modifiers.alt || modifiers.ctrl {
                continue;
            }
            if pressed && !repeat {
                // Cambio ottava con le frecce su/giù (come il selettore di FL).
                match key {
                    Key::ArrowUp if modifiers.shift => {
                        self.octave = (self.octave + 1).min(9);
                        used = true;
                        continue;
                    }
                    Key::ArrowDown if modifiers.shift => {
                        self.octave = (self.octave - 1).max(0);
                        used = true;
                        continue;
                    }
                    _ => {}
                }
            }
            let Some(semi) = key_to_semitone(key) else { continue };
            used = true;
            if pressed && !repeat && !self.pressed.contains_key(&key) {
                let note = (self.octave * 12 + semi).clamp(0, 127) as u8;
                self.pressed.insert(key, note);
                self.emit(LiveEvent::On {
                    key: note,
                    velocity: self.velocity,
                });
            } else if !pressed {
                if let Some(note) = self.pressed.remove(&key) {
                    self.emit(LiveEvent::Off { key: note });
                }
            }
        }
        used
    }

    /// Eventi in attesa (da MIDI e tastiera), svuotati.
    pub fn drain(&self) -> Vec<LiveEvent> {
        self.rx.try_iter().collect()
    }

    /// Porte MIDI di ingresso disponibili.
    pub fn midi_ports() -> Vec<String> {
        #[cfg(feature = "midi")]
        {
            if let Ok(input) = midir::MidiInput::new("Mixfonocraft") {
                return input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect();
            }
        }
        Vec::new()
    }

    /// Collega una porta MIDI per nome (`None` = scollega).
    pub fn connect_midi(&mut self, port: Option<&str>) -> Result<(), String> {
        #[cfg(feature = "midi")]
        {
            self.midi = None;
            self.midi_port = None;
            let Some(name) = port else { return Ok(()) };
            let input = midir::MidiInput::new("Mixfonocraft").map_err(|e| e.to_string())?;
            let ports = input.ports();
            let p = ports
                .iter()
                .find(|p| input.port_name(p).is_ok_and(|n| n == name))
                .ok_or_else(|| format!("porta MIDI non trovata: {name}"))?;
            let engine = self.engine.clone();
            let target = self.target.clone();
            let tx = self.tx.clone();
            let conn = input
                .connect(
                    p,
                    "mixfonocraft-in",
                    move |_ts, msg, _| {
                        if let Some(ev) = parse_midi(msg) {
                            let ch = target.load(Ordering::Relaxed);
                            if ch != 0 {
                                match ev {
                                    LiveEvent::On { key, velocity } => {
                                        engine.preview_on(ChannelId(ch), key, velocity);
                                    }
                                    LiveEvent::Off { key } => {
                                        engine.preview_off(ChannelId(ch), key);
                                    }
                                }
                            }
                            let _ = tx.send(ev);
                        }
                    },
                    (),
                )
                .map_err(|e| e.to_string())?;
            self.midi = Some(conn);
            self.midi_port = Some(name.to_string());
            Ok(())
        }
        #[cfg(not(feature = "midi"))]
        {
            match port {
                None => Ok(()),
                Some(_) => Err("MIDI non disponibile in questa build".into()),
            }
        }
    }
}

/// Interpreta un messaggio MIDI di canale (note on/off; note on con velocity 0 = off).
pub fn parse_midi(msg: &[u8]) -> Option<LiveEvent> {
    let (&status, rest) = msg.split_first()?;
    match (status & 0xf0, rest) {
        (0x90, [key, vel, ..]) if *vel > 0 => Some(LiveEvent::On {
            key: key & 0x7f,
            velocity: (*vel & 0x7f) as f32 / 127.0,
        }),
        (0x90, [key, _, ..]) | (0x80, [key, _, ..]) => Some(LiveEvent::Off { key: key & 0x7f }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_parsing() {
        assert_eq!(
            parse_midi(&[0x90, 60, 127]),
            Some(LiveEvent::On { key: 60, velocity: 1.0 })
        );
        assert_eq!(parse_midi(&[0x93, 61, 0]), Some(LiveEvent::Off { key: 61 }));
        assert_eq!(parse_midi(&[0x80, 62, 40]), Some(LiveEvent::Off { key: 62 }));
        assert_eq!(parse_midi(&[0xB0, 7, 100]), None);
        assert_eq!(parse_midi(&[0x90]), None);
    }

    #[test]
    fn keyboard_layout() {
        assert_eq!(key_to_semitone(Key::Z), Some(0));
        assert_eq!(key_to_semitone(Key::M), Some(11));
        assert_eq!(key_to_semitone(Key::Q), Some(12));
        assert_eq!(key_to_semitone(Key::P), Some(28));
        assert_eq!(key_to_semitone(Key::L), None);
    }

    #[test]
    fn recorder_pairs_and_wraps() {
        let mut r = Recorder::default();
        r.on(60, 0.9, 96.0);
        let n = r.off(60, 192.0, 384, 1).unwrap();
        assert_eq!((n.position, n.length, n.key), (96, 96, 60));
        // Rilascio dopo il giro del loop: 360 → 24 su un pattern da 384.
        r.on(62, 0.5, 360.0);
        let n = r.off(62, 24.0, 384, 1).unwrap();
        assert_eq!((n.position, n.length), (360, 48));
        // Quantizzazione dell'inizio a 1/16 (24 tick).
        r.on(64, 0.5, 37.0);
        let n = r.off(64, 80.0, 384, 24).unwrap();
        assert_eq!(n.position, 48);
        assert!(r.off(99, 0.0, 384, 1).is_none());
        r.on(65, 1.0, 10.0);
        assert_eq!(r.flush(20.0, 384, 1).len(), 1);
        assert!(r.is_empty());
    }

    #[test]
    fn typing_keyboard_plays_and_records() {
        let p = mfc_core::Project::new("t");
        let (_engine, handle) = mfc_engine::Engine::new(&p, 48_000.0);
        let mut live = LiveInput::new(handle);
        live.set_target(Some(ChannelId(7)));
        let ctx = egui::Context::default();
        let press = |k: Key, down: bool| egui::RawInput {
            events: vec![egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: down,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let mut out = ctx.run_ui(press(Key::Z, true), |ui| {
            live.handle_keyboard(ui.ctx());
        });
        out.textures_delta.clear();
        // Cambio ottava mentre il tasto è premuto: il rilascio spegne la nota originale.
        live.octave = 6;
        let mut out = ctx.run_ui(press(Key::Z, false), |ui| {
            live.handle_keyboard(ui.ctx());
        });
        out.textures_delta.clear();
        assert_eq!(
            live.drain(),
            vec![LiveEvent::On { key: 60, velocity: 0.8 }, LiveEvent::Off { key: 60 }]
        );
    }
}
