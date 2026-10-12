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

#[cfg(test)]
mod tests {
    use super::*;

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
