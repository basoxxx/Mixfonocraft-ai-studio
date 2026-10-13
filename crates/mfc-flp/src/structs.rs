//! Layout dei record binari contenuti negli eventi DATA (da PyFLP / Monad.FLParser).

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    u32_at(d, o).map(|v| v as i32)
}
fn f32_at(d: &[u8], o: usize) -> Option<f32> {
    u32_at(d, o).map(f32::from_bits)
}

/// Flag "slide" di una nota.
pub const NOTE_FLAG_SLIDE: u16 = 1 << 3;
/// Dimensione di un record nota (evento 224).
pub const NOTE_SIZE: usize = 24;

/// Nota di un pattern (evento `PatNotes`, 24 byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlNote {
    pub position: u32,
    pub flags: u16,
    /// IID del canale (valore di `ChanNew`).
    pub rack_channel: u16,
    pub length: u32,
    pub key: u16,
    pub group: u16,
    /// 0..=240, 120 = nessuna intonazione fine.
    pub fine_pitch: u8,
    pub u1: u8,
    /// 0..=128, default 64.
    pub release: u8,
    pub midi_channel: u8,
    /// 0..=128, 64 = centro.
    pub pan: u8,
    /// 0..=128, default 100.
    pub velocity: u8,
    pub mod_x: u8,
    pub mod_y: u8,
}

impl FlNote {
    pub fn parse_all(d: &[u8]) -> Vec<FlNote> {
        d.as_chunks::<NOTE_SIZE>()
            .0
            .iter()
            .filter_map(|c| Self::parse(c))
            .collect()
    }
    fn parse(c: &[u8]) -> Option<FlNote> {
        Some(FlNote {
            position: u32_at(c, 0)?,
            flags: u16_at(c, 4)?,
            rack_channel: u16_at(c, 6)?,
            length: u32_at(c, 8)?,
            key: u16_at(c, 12)?,
            group: u16_at(c, 14)?,
            fine_pitch: c[16],
            u1: c[17],
            release: c[18],
            midi_channel: c[19],
            pan: c[20],
            velocity: c[21],
            mod_x: c[22],
            mod_y: c[23],
        })
    }
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.position.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.rack_channel.to_le_bytes());
        out.extend_from_slice(&self.length.to_le_bytes());
        out.extend_from_slice(&self.key.to_le_bytes());
        out.extend_from_slice(&self.group.to_le_bytes());
        out.extend_from_slice(&[
            self.fine_pitch,
            self.u1,
            self.release,
            self.midi_channel,
            self.pan,
            self.velocity,
            self.mod_x,
            self.mod_y,
        ]);
    }
}

/// Base degli indici pattern negli item della Playlist.
pub const PATTERN_BASE: u16 = 20480;
/// Flag "muted" di un item della Playlist (Monad.FLParser).
pub const ITEM_FLAG_MUTED: u16 = 0x2000;
/// Record playlist fino a FL 20.
pub const ITEM_SIZE_OLD: usize = 32;
/// Record playlist da FL 21 (28 byte extra sconosciuti).
pub const ITEM_SIZE_NEW: usize = 60;

/// Item della Playlist (evento `PlaylistItems`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlPlaylistItem {
    pub position: u32,
    pub pattern_base: u16,
    pub item_index: u16,
    pub length: u32,
    /// Traccia invertita: `max_idx - traccia`.
    pub track_rvidx: u16,
    pub group: u16,
    pub flags: u16,
    pub start_offset: f32,
    pub end_offset: f32,
}

impl FlPlaylistItem {
    pub fn parse_all(d: &[u8], size: usize) -> Vec<FlPlaylistItem> {
        d.chunks_exact(size).filter_map(Self::parse).collect()
    }
    fn parse(c: &[u8]) -> Option<Self> {
        Some(Self {
            position: u32_at(c, 0)?,
            pattern_base: u16_at(c, 4)?,
            item_index: u16_at(c, 6)?,
            length: u32_at(c, 8)?,
            track_rvidx: u16_at(c, 12)?,
            group: u16_at(c, 14)?,
            flags: u16_at(c, 18)?,
            start_offset: f32_at(c, 24)?,
            end_offset: f32_at(c, 28)?,
        })
    }
    /// Scrive il record da 32 byte (formato FL 20), con i valori costanti documentati.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.position.to_le_bytes());
        out.extend_from_slice(&self.pattern_base.to_le_bytes());
        out.extend_from_slice(&self.item_index.to_le_bytes());
        out.extend_from_slice(&self.length.to_le_bytes());
        out.extend_from_slice(&self.track_rvidx.to_le_bytes());
        out.extend_from_slice(&self.group.to_le_bytes());
        out.extend_from_slice(&[120, 0]);
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&[64, 100, 128, 128]);
        out.extend_from_slice(&self.start_offset.to_le_bytes());
        out.extend_from_slice(&self.end_offset.to_le_bytes());
    }
}

/// Id dei parametri del mixer (evento `MixerParams`).
pub mod mixer_param {
    pub const SLOT_ENABLED: u8 = 0;
    pub const SLOT_MIX: u8 = 1;
    pub const ROUTE_VOL_START: u8 = 64;
    pub const VOLUME: u8 = 192;
    pub const PAN: u8 = 193;
    pub const STEREO_SEPARATION: u8 = 194;
}

/// Record del mixer (12 byte): `u32 ?, u8 id, u8 ?, u16 channel_data, i32 msg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlMixerParam {
    pub id: u8,
    pub insert: u16,
    pub slot: u16,
    pub msg: i32,
}

impl FlMixerParam {
    pub const SIZE: usize = 12;
    pub fn parse_all(d: &[u8]) -> Vec<Self> {
        d.as_chunks::<{ Self::SIZE }>()
            .0
            .iter()
            .filter_map(|c| {
                let cd = u16_at(c, 6)?;
                Some(Self {
                    id: c[4],
                    insert: (cd >> 6) & 0x7f,
                    slot: cd & 0x3f,
                    msg: i32_at(c, 8)?,
                })
            })
            .collect()
    }
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&0u32.to_le_bytes());
        out.push(self.id);
        out.push(0);
        let cd = ((self.insert & 0x7f) << 6) | (self.slot & 0x3f);
        out.extend_from_slice(&cd.to_le_bytes());
        out.extend_from_slice(&self.msg.to_le_bytes());
    }
}

/// Livelli del canale (evento `ChanLevels`): pan, volume, pitch, mod X/Y, tipo filtro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlLevels {
    /// 0..=12800, 6400 = centro.
    pub pan: Option<i32>,
    /// 0..=12800, 10000 = default.
    pub volume: Option<u32>,
    /// Cent, -4800..=4800.
    pub pitch_shift: Option<i32>,
}

impl FlLevels {
    pub fn parse(d: &[u8]) -> Self {
        Self {
            pan: i32_at(d, 0),
            volume: u32_at(d, 4),
            pitch_shift: i32_at(d, 8),
        }
    }
    pub fn write(pan: i32, volume: u32, pitch: i32) -> Vec<u8> {
        let mut v = Vec::with_capacity(24);
        v.extend_from_slice(&pan.to_le_bytes());
        v.extend_from_slice(&volume.to_le_bytes());
        v.extend_from_slice(&pitch.to_le_bytes());
        v.extend_from_slice(&256u32.to_le_bytes()); // filter mod X
        v.extend_from_slice(&0u32.to_le_bytes()); // filter mod Y
        v.extend_from_slice(&0u32.to_le_bytes()); // tipo filtro
        v
    }
}

/// Flag dell'insert (evento `InsertFlags`, u32 all'offset 4).
pub mod insert_flag {
    pub const POLARITY_REVERSED: u32 = 1 << 0;
    pub const SWAP_LEFT_RIGHT: u32 = 1 << 1;
    pub const ENABLE_EFFECTS: u32 = 1 << 2;
    pub const ENABLED: u32 = 1 << 3;
    pub const SOLO: u32 = 1 << 12;
}

pub fn insert_flags_parse(d: &[u8]) -> Option<u32> {
    u32_at(d, 4)
}

pub fn insert_flags_write(flags: u32) -> Vec<u8> {
    let mut v = vec![0u8; 12];
    v[4..8].copy_from_slice(&flags.to_le_bytes());
    v
}

/// Dati di una traccia della Playlist (evento `TrackData`): iid, colore, icona, abilitata, …
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlTrackData {
    pub iid: Option<u32>,
    pub color: Option<[u8; 4]>,
    pub enabled: Option<bool>,
}

impl FlTrackData {
    pub fn parse(d: &[u8]) -> Self {
        Self {
            iid: u32_at(d, 0),
            color: d.get(4..8).map(|c| [c[0], c[1], c[2], c[3]]),
            enabled: d.get(12).map(|b| *b != 0),
        }
    }
    /// Scrive i 48 byte documentati con i default di FL 20.
    pub fn write(iid: u32, color: [u8; 4], enabled: bool) -> Vec<u8> {
        let mut v = Vec::with_capacity(48);
        v.extend_from_slice(&iid.to_le_bytes());
        v.extend_from_slice(&color);
        v.extend_from_slice(&0u32.to_le_bytes()); // icona
        v.push(enabled as u8);
        v.extend_from_slice(&1.0f32.to_le_bytes()); // altezza
        v.extend_from_slice(&(-1i32).to_le_bytes()); // locked height
        v.push(0); // content locked
        v.extend_from_slice(&0u32.to_le_bytes()); // motion
        v.extend_from_slice(&0u32.to_le_bytes()); // press
        v.extend_from_slice(&5u32.to_le_bytes()); // trigger sync (4 beat)
        v.extend_from_slice(&0u32.to_le_bytes()); // queued
        v.extend_from_slice(&1u32.to_le_bytes()); // tolerant
        v.extend_from_slice(&0u32.to_le_bytes()); // position sync
        v.push(0); // grouped
        v.push(0); // locked
        v
    }
}

/// Id dei record dentro i dati del "Fruity Wrapper" (evento `PluginData` dei plugin VST),
/// come elencati da PyFLP (`_VSTPluginEventID`).
pub mod vst_record {
    pub const MIDI: u32 = 1;
    pub const FLAGS: u32 = 2;
    pub const IO: u32 = 30;
    pub const INPUTS: u32 = 31;
    pub const OUTPUTS: u32 = 32;
    pub const PLUGIN_INFO: u32 = 50;
    /// Codice a 4 caratteri (solo VST2).
    pub const FOURCC: u32 = 51;
    /// Class id (16 byte per i VST3).
    pub const GUID: u32 = 52;
    /// Blob di stato del plugin.
    pub const STATE: u32 = 53;
    /// Nome "di fabbrica" del plugin (UTF-8).
    pub const NAME: u32 = 54;
    /// Percorso assoluto del binario (UTF-8).
    pub const PLUGIN_PATH: u32 = 55;
    /// Produttore (UTF-8).
    pub const VENDOR: u32 = 56;
}

/// Marcatore iniziale dei dati del wrapper: PyFLP osserva 8 o 10 nei file reali.
pub const WRAPPER_DATA_TYPE: u32 = 10;

/// Dati del "Fruity Wrapper" (evento `PluginData` con nome interno `Fruity Wrapper`):
/// `u32 tipo`, poi record `{u32 id, u64 lunghezza, byte[lunghezza]}` fino alla fine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlWrapperData {
    pub kind: u32,
    pub records: Vec<(u32, Vec<u8>)>,
}

impl FlWrapperData {
    /// Analizza il blob; `None` se non rispetta esattamente la struttura a record
    /// (es. dati di un plugin nativo o blob opaco arbitrario).
    pub fn parse(d: &[u8]) -> Option<Self> {
        let kind = u32_at(d, 0)?;
        let mut records = Vec::new();
        let mut o = 4usize;
        while o < d.len() {
            let rid = u32_at(d, o)?;
            let len = d
                .get(o + 4..o + 12)
                .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))?;
            let len = usize::try_from(len).ok()?;
            let start = o + 12;
            let end = start.checked_add(len)?;
            records.push((rid, d.get(start..end)?.to_vec()));
            o = end;
        }
        Some(Self { kind, records })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(4 + self.records.iter().map(|r| 12 + r.1.len()).sum::<usize>());
        v.extend_from_slice(&self.kind.to_le_bytes());
        for (rid, data) in &self.records {
            v.extend_from_slice(&rid.to_le_bytes());
            v.extend_from_slice(&(data.len() as u64).to_le_bytes());
            v.extend_from_slice(data);
        }
        v
    }

    /// Primo record con l'id dato.
    pub fn record(&self, rid: u32) -> Option<&[u8]> {
        self.records.iter().find(|r| r.0 == rid).map(|r| r.1.as_slice())
    }

    /// Record testuale (UTF-8, eventuali NUL finali rimossi).
    pub fn text(&self, rid: u32) -> Option<String> {
        self.record(rid)
            .map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_string())
    }
}

/// Dimensione dell'intestazione dell'evento `ChanAutomation` prima del numero di punti.
pub const AUTOMATION_HEADER: usize = 17;
/// Dimensione di un punto di automazione.
pub const AUTOMATION_POINT_SIZE: usize = 24;

/// Punto di una clip di automazione FL.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlAutoPoint {
    /// Posizione assoluta (somma degli scarti), in tick (PyFLP: "PPQ dependant").
    pub position: f64,
    /// Valore normalizzato 0..=1 sul range del parametro FL.
    pub value: f64,
    pub tension: f32,
}

/// Clip di automazione (evento `ChanAutomation` 234, layout da PyFLP `AutomationEvent`):
/// 17 byte di intestazione (`u32` sempre 1, `i32` quantità LFO, 9 byte sconosciuti),
/// `u32` numero di punti, punti da 24 byte (`f64` scarto in X dal punto precedente,
/// `f64` valore, `f32` tensione, 4 byte sconosciuti), coda sconosciuta.
#[derive(Debug, Clone, PartialEq)]
pub struct FlAutomation {
    pub points: Vec<FlAutoPoint>,
}

impl FlAutomation {
    pub fn parse(d: &[u8]) -> Option<Self> {
        let n = u32_at(d, AUTOMATION_HEADER)? as usize;
        let base = AUTOMATION_HEADER + 4;
        let needed = n.checked_mul(AUTOMATION_POINT_SIZE)?.checked_add(base)?;
        if needed > d.len() {
            return None;
        }
        let mut points = Vec::with_capacity(n);
        let mut pos = 0.0f64;
        for i in 0..n {
            let o = base + i * AUTOMATION_POINT_SIZE;
            let delta = f64::from_bits(u64::from_le_bytes(d[o..o + 8].try_into().ok()?));
            let value = f64::from_bits(u64::from_le_bytes(d[o + 8..o + 16].try_into().ok()?));
            let tension = f32_at(d, o + 16)?;
            pos += delta;
            points.push(FlAutoPoint {
                position: pos,
                value,
                tension,
            });
        }
        Some(Self { points })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(AUTOMATION_HEADER + 4 + self.points.len() * AUTOMATION_POINT_SIZE);
        v.extend_from_slice(&1u32.to_le_bytes()); // "sempre 1" secondo PyFLP
        v.extend_from_slice(&0i32.to_le_bytes()); // quantità LFO
        v.extend_from_slice(&[0u8; 9]);
        v.extend_from_slice(&(self.points.len() as u32).to_le_bytes());
        let mut prev = 0.0f64;
        for p in &self.points {
            v.extend_from_slice(&(p.position - prev).to_le_bytes());
            v.extend_from_slice(&p.value.to_le_bytes());
            v.extend_from_slice(&p.tension.to_le_bytes());
            v.extend_from_slice(&[0u8; 4]);
            prev = p.position;
        }
        v
    }
}

/// Collegamento di un controller interno (evento `RemoteController` 227, 20 byte, PyFLP):
/// `2+1+1` byte sconosciuti, `u16 parameter_data` (bit 15 = parametro VST), `i16
/// destination_data`, 12 byte sconosciuti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlRemoteController {
    pub parameter: u16,
    pub destination: i16,
}

impl FlRemoteController {
    pub const SIZE: usize = 20;
    pub fn parse(d: &[u8]) -> Option<Self> {
        Some(Self {
            parameter: u16_at(d, 4)?,
            destination: u16_at(d, 6)? as i16,
        })
    }
    pub fn write(&self) -> Vec<u8> {
        let mut v = vec![0u8; Self::SIZE];
        v[4..6].copy_from_slice(&self.parameter.to_le_bytes());
        v[6..8].copy_from_slice(&self.destination.to_le_bytes());
        v
    }
}

/// Bersaglio di un'automazione in termini FL (convenzione best-effort, vedi
/// `docs/FLP_FORMAT.md`): canale per iid, insert del mixer per indice, parametri globali.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlAutoTarget {
    ChannelVolume(u16),
    ChannelPan(u16),
    ChannelPitch(u16),
    MixerVolume(u16),
    MixerPan(u16),
    MasterVolume,
    Tempo,
}

/// Id dei parametri usati in `FlRemoteController::parameter`.
pub mod auto_param {
    /// Canale: volume / pan / pitch (numerazione "REC_Chan_*" nota dalla community).
    pub const CHAN_VOLUME: u16 = 0;
    pub const CHAN_PAN: u16 = 1;
    pub const CHAN_PITCH: u16 = 4;
    /// Globali (destinazione −1): volume principale e tempo.
    pub const MAIN_VOLUME: u16 = 0;
    pub const TEMPO: u16 = 1;
}

/// Bit della destinazione che indica un insert del mixer (`0x2000 | insert << 6 | slot`).
pub const DEST_MIXER: i16 = 0x2000;
/// Slot "nessuno" (parametro dell'insert, non di un effetto).
pub const DEST_NO_SLOT: i16 = 0x3f;
/// Destinazione dei parametri globali del progetto.
pub const DEST_GLOBAL: i16 = -1;

impl FlAutoTarget {
    pub fn to_remote(self) -> FlRemoteController {
        use auto_param::*;
        let mixer = |i: u16| DEST_MIXER | (((i & 0x7f) as i16) << 6) | DEST_NO_SLOT;
        let (parameter, destination) = match self {
            Self::ChannelVolume(c) => (CHAN_VOLUME, (c & 0x1fff) as i16),
            Self::ChannelPan(c) => (CHAN_PAN, (c & 0x1fff) as i16),
            Self::ChannelPitch(c) => (CHAN_PITCH, (c & 0x1fff) as i16),
            Self::MixerVolume(i) => (mixer_param::VOLUME as u16, mixer(i)),
            Self::MixerPan(i) => (mixer_param::PAN as u16, mixer(i)),
            Self::MasterVolume => (MAIN_VOLUME, DEST_GLOBAL),
            Self::Tempo => (TEMPO, DEST_GLOBAL),
        };
        FlRemoteController { parameter, destination }
    }

    pub fn from_remote(r: FlRemoteController) -> Option<Self> {
        use auto_param::*;
        if r.parameter & 0x8000 != 0 {
            return None; // parametro di un plugin VST: non rappresentabile
        }
        let d = r.destination;
        if d == DEST_GLOBAL {
            return match r.parameter {
                MAIN_VOLUME => Some(Self::MasterVolume),
                TEMPO => Some(Self::Tempo),
                _ => None,
            };
        }
        if d < 0 {
            return None;
        }
        if d & DEST_MIXER != 0 {
            if d & DEST_NO_SLOT != DEST_NO_SLOT {
                return None; // parametro di uno slot effetto
            }
            let insert = ((d >> 6) & 0x7f) as u16;
            return match r.parameter {
                p if p == mixer_param::VOLUME as u16 => Some(Self::MixerVolume(insert)),
                p if p == mixer_param::PAN as u16 => Some(Self::MixerPan(insert)),
                _ => None,
            };
        }
        let c = d as u16;
        match r.parameter {
            CHAN_VOLUME => Some(Self::ChannelVolume(c)),
            CHAN_PAN => Some(Self::ChannelPan(c)),
            CHAN_PITCH => Some(Self::ChannelPitch(c)),
            _ => None,
        }
    }

    /// Range reale (unità del modello) corrispondente a 0..=1 nel parametro FL.
    pub fn range(self) -> (f32, f32) {
        match self {
            // Volume canale: raw 0..12800 → modello raw/12800.
            Self::ChannelVolume(_) => (0.0, 1.0),
            Self::ChannelPan(_) | Self::MixerPan(_) => (-1.0, 1.0),
            Self::ChannelPitch(_) => (-4800.0, 4800.0),
            // Volume insert: raw 0..16000 → modello raw/16000.
            Self::MixerVolume(_) => (0.0, 1.0),
            Self::MasterVolume => (0.0, 1.25),
            // Range del tempo in FL 20.
            Self::Tempo => (10.0, 522.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_and_automation_roundtrip() {
        let w = FlWrapperData {
            kind: WRAPPER_DATA_TYPE,
            records: vec![
                (vst_record::NAME, b"Synth".to_vec()),
                (vst_record::STATE, vec![0, 1, 2, 255]),
                (vst_record::PLUGIN_PATH, Vec::new()),
            ],
        };
        let b = w.write();
        assert_eq!(FlWrapperData::parse(&b), Some(w.clone()));
        assert_eq!(w.text(vst_record::NAME).as_deref(), Some("Synth"));
        // Blob arbitrari non vengono scambiati per dati del wrapper.
        assert_eq!(FlWrapperData::parse(&[1, 2, 3, 4, 5]), None);
        assert_eq!(FlWrapperData::parse(&[1, 2, 3]), None);
        let mut long = b.clone();
        long.truncate(b.len() - 1);
        assert_eq!(FlWrapperData::parse(&long), None);

        let a = FlAutomation {
            points: vec![
                FlAutoPoint {
                    position: 0.0,
                    value: 0.25,
                    tension: 0.0,
                },
                FlAutoPoint {
                    position: 96.0,
                    value: 1.0,
                    tension: -0.5,
                },
            ],
        };
        let b = a.write();
        assert_eq!(b.len(), AUTOMATION_HEADER + 4 + 2 * AUTOMATION_POINT_SIZE);
        assert_eq!(FlAutomation::parse(&b), Some(a));
        assert_eq!(FlAutomation::parse(&b[..b.len() - 1]), None);
        assert_eq!(FlAutomation::parse(&[0; 4]), None);

        for t in [
            FlAutoTarget::ChannelVolume(3),
            FlAutoTarget::ChannelPan(0),
            FlAutoTarget::ChannelPitch(7),
            FlAutoTarget::MixerVolume(0),
            FlAutoTarget::MixerPan(125),
            FlAutoTarget::MasterVolume,
            FlAutoTarget::Tempo,
        ] {
            let r = t.to_remote();
            assert_eq!(FlRemoteController::parse(&r.write()), Some(r));
            assert_eq!(FlAutoTarget::from_remote(r), Some(t));
        }
        let vst = FlRemoteController {
            parameter: 0x8001,
            destination: 2,
        };
        assert_eq!(FlAutoTarget::from_remote(vst), None);
    }

    #[test]
    fn note_roundtrip() {
        let n = FlNote {
            position: 96,
            flags: NOTE_FLAG_SLIDE,
            rack_channel: 2,
            length: 48,
            key: 60,
            group: 0,
            fine_pitch: 120,
            u1: 0,
            release: 64,
            midi_channel: 0,
            pan: 64,
            velocity: 100,
            mod_x: 128,
            mod_y: 128,
        };
        let mut b = Vec::new();
        n.write(&mut b);
        assert_eq!(b.len(), NOTE_SIZE);
        assert_eq!(FlNote::parse_all(&b), vec![n]);
    }

    #[test]
    fn item_and_params_roundtrip() {
        let it = FlPlaylistItem {
            position: 384,
            pattern_base: PATTERN_BASE,
            item_index: PATTERN_BASE + 1,
            length: 384,
            track_rvidx: 499,
            group: 0,
            flags: 64 | ITEM_FLAG_MUTED,
            start_offset: -1.0,
            end_offset: -1.0,
        };
        let mut b = Vec::new();
        it.write(&mut b);
        assert_eq!(b.len(), ITEM_SIZE_OLD);
        assert_eq!(FlPlaylistItem::parse_all(&b, ITEM_SIZE_OLD), vec![it]);

        let p = FlMixerParam {
            id: mixer_param::VOLUME,
            insert: 126,
            slot: 9,
            msg: -6400,
        };
        let mut b = Vec::new();
        p.write(&mut b);
        assert_eq!(FlMixerParam::parse_all(&b), vec![p]);
        assert_eq!(FlTrackData::write(1, [1, 2, 3, 0], true).len(), 48);
    }
}
