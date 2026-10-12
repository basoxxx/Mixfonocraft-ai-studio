//! Serializzazione del livello eventi. Inverso esatto di [`crate::parse`].

use crate::events::{EventKind, EventValue, FlpFile, RawEvent};
use crate::reader::varint_min_len;

/// Serializza un file FLP completo.
pub fn write(file: &FlpFile) -> Vec<u8> {
    let body = write_events(&file.events);
    let mut out = Vec::with_capacity(22 + body.len() + file.trailing.len());
    out.extend_from_slice(b"FLhd");
    out.extend_from_slice(&6u32.to_le_bytes());
    out.extend_from_slice(&file.header.format.to_le_bytes());
    out.extend_from_slice(&file.header.n_channels.to_le_bytes());
    out.extend_from_slice(&file.header.ppq.to_le_bytes());
    out.extend_from_slice(b"FLdt");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&file.trailing);
    out
}

/// Serializza il flusso di eventi.
pub fn write_events(events: &[RawEvent]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in events {
        write_event(&mut out, e);
    }
    out
}

/// Scrive un evento. Se il valore non corrisponde alla classe dell'id viene convertito
/// (numeri troncati/estesi, payload numerici in little-endian).
fn write_event(out: &mut Vec<u8>, e: &RawEvent) {
    out.push(e.id);
    let num = || match &e.value {
        EventValue::Data(d) => {
            let mut b = [0u8; 4];
            for (i, x) in d.iter().take(4).enumerate() {
                b[i] = *x;
            }
            u32::from_le_bytes(b)
        }
        v => v.as_u32().unwrap_or(0),
    };
    match EventKind::of(e.id) {
        EventKind::Byte => out.push(num() as u8),
        EventKind::Word => out.extend_from_slice(&(num() as u16).to_le_bytes()),
        EventKind::Dword => out.extend_from_slice(&num().to_le_bytes()),
        EventKind::Text | EventKind::Data => {
            let owned;
            let payload: &[u8] = match &e.value {
                EventValue::Data(d) => d,
                EventValue::Byte(v) => {
                    owned = vec![*v];
                    &owned
                }
                EventValue::Word(v) => {
                    owned = v.to_le_bytes().to_vec();
                    &owned
                }
                EventValue::Dword(v) => {
                    owned = v.to_le_bytes().to_vec();
                    &owned
                }
            };
            write_varint(out, payload.len() as u64, e.varint_len as usize);
            out.extend_from_slice(payload);
        }
    }
}

/// Varint LEB128; `width` > minimo forza una codifica non minima (round-trip esatto).
fn write_varint(out: &mut Vec<u8>, v: u64, width: usize) {
    let n = width.clamp(varint_min_len(v), 10);
    for i in 0..n {
        let mut b = ((v >> (7 * i)) & 0x7f) as u8;
        if i + 1 < n {
            b |= 0x80;
        }
        out.push(b);
    }
}
