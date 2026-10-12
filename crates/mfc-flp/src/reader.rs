//! Parser del livello eventi: `FLhd` + `FLdt`. Non va mai in panic su input malformato.

use crate::events::{EventKind, EventValue, FlpFile, FlpHeader, RawEvent};
use crate::FlpError;

/// Cursore su slice con letture controllate.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], FlpError> {
        let end = self.pos.checked_add(n).ok_or(FlpError::Truncated(what))?;
        let s = self.buf.get(self.pos..end).ok_or(FlpError::Truncated(what))?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, FlpError> {
        Ok(self.take(1, what)?[0])
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, FlpError> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, FlpError> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Numero minimo di byte per codificare `v` come varint a 7 bit.
pub(crate) fn varint_min_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// Legge un varint LEB128 (max 5 byte, valore ≤ u32::MAX). Ritorna (valore, byte letti).
fn read_varint(c: &mut Cursor<'_>) -> Result<(u32, usize), FlpError> {
    let start = c.pos;
    let mut value: u64 = 0;
    for i in 0..5 {
        let b = c.u8("lunghezza varint")?;
        value |= ((b & 0x7f) as u64) << (7 * i);
        if b & 0x80 == 0 {
            if value > u32::MAX as u64 {
                return Err(FlpError::BadVarint(start));
            }
            return Ok((value as u32, i + 1));
        }
    }
    Err(FlpError::BadVarint(start))
}

/// Analizza un file FLP completo.
pub fn parse(bytes: &[u8]) -> Result<FlpFile, FlpError> {
    let mut c = Cursor { buf: bytes, pos: 0 };
    if c.take(4, "magic FLhd")? != b"FLhd" {
        return Err(FlpError::BadMagic("FLhd"));
    }
    let hlen = c.u32("lunghezza header")?;
    if hlen != 6 {
        return Err(FlpError::BadHeader(format!("lunghezza header {hlen}, attesa 6")));
    }
    let format = c.u16("formato")? as i16;
    let n_channels = c.u16("numero canali")?;
    let ppq = c.u16("ppq")?;
    if c.take(4, "magic FLdt")? != b"FLdt" {
        return Err(FlpError::BadMagic("FLdt"));
    }
    let dlen = c.u32("lunghezza dati")? as usize;
    let data = c.take(dlen, "chunk FLdt")?;
    let trailing = bytes[c.pos..].to_vec();
    let events = parse_events(data)?;
    Ok(FlpFile {
        header: FlpHeader {
            format,
            n_channels,
            ppq,
        },
        events,
        trailing,
    })
}

/// Analizza il solo flusso di eventi (contenuto di `FLdt`).
pub fn parse_events(data: &[u8]) -> Result<Vec<RawEvent>, FlpError> {
    let mut c = Cursor { buf: data, pos: 0 };
    let mut events = Vec::new();
    while c.pos < data.len() {
        let offset = c.pos;
        let id = c.u8("id evento")?;
        let trunc = |_| FlpError::TruncatedEvent { offset, id };
        let (value, varint_len) = match EventKind::of(id) {
            EventKind::Byte => (EventValue::Byte(c.u8("").map_err(trunc)?), 0),
            EventKind::Word => (EventValue::Word(c.u16("").map_err(trunc)?), 0),
            EventKind::Dword => (EventValue::Dword(c.u32("").map_err(trunc)?), 0),
            EventKind::Text | EventKind::Data => {
                let (len, nbytes) = read_varint(&mut c).map_err(|e| match e {
                    FlpError::Truncated(_) => FlpError::TruncatedEvent { offset, id },
                    e => e,
                })?;
                let payload = c.take(len as usize, "").map_err(trunc)?;
                let minimal = varint_min_len(len as u64);
                let vl = if nbytes == minimal { 0 } else { nbytes as u8 };
                (EventValue::Data(payload.to_vec()), vl)
            }
        };
        events.push(RawEvent { id, value, varint_len });
    }
    Ok(events)
}
