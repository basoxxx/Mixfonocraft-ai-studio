//! Formato dei messaggi tra l'host e il processo `mfc-plugin-host`.
//!
//! Ogni messaggio è un frame `[lunghezza u32 LE][payload]`; il primo byte del payload
//! è il tipo di messaggio. Tutti i numeri sono little endian; stringhe e blob sono
//! preceduti dalla lunghezza (u32). Il formato è privato: host e helper vengono sempre
//! distribuiti insieme, ma il saluto iniziale ([`FromChild::Ready`]) contiene comunque
//! la versione del protocollo.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::{EditorSize, ExternalParam, HostError, MidiEvent, PluginInfo};

/// Versione del protocollo (cambia a ogni modifica incompatibile).
pub const PROTOCOL_VERSION: u32 = 1;

/// Dimensione massima di un frame (stati dei plugin molto grandi compresi).
pub const MAX_FRAME: usize = 1 << 30;

/// Errore di decodifica.
fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("messaggio non valido: {what}"))
}

/// Codificatore di un frame. Riserva i 4 byte della lunghezza, completati da [`Enc::finish`].
pub struct Enc {
    buf: Vec<u8>,
}

impl Default for Enc {
    fn default() -> Self {
        Self::new()
    }
}

impl Enc {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(1024),
        }
    }

    /// Ricomincia un frame riusando il buffer.
    pub fn begin(&mut self, tag: u8) -> &mut Self {
        self.buf.clear();
        self.buf.extend_from_slice(&[0; 4]);
        self.buf.push(tag);
        self
    }

    /// Frame completo (lunghezza inclusa), pronto per `write_all`.
    pub fn finish(&mut self) -> &[u8] {
        let len = (self.buf.len() - 4) as u32;
        self.buf[..4].copy_from_slice(&len.to_le_bytes());
        &self.buf
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v);
        self
    }
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    pub fn f32s(&mut self, v: &[f32]) -> &mut Self {
        self.u32(v.len() as u32);
        self.buf.reserve(v.len() * 4);
        for x in v {
            self.buf.extend_from_slice(&x.to_le_bytes());
        }
        self
    }

    pub fn event(&mut self, e: &MidiEvent) -> &mut Self {
        match *e {
            MidiEvent::NoteOn {
                offset,
                key,
                velocity,
                note_id,
            } => self.u8(0).u32(offset).u8(key).f32(velocity).i32(note_id),
            MidiEvent::NoteOff {
                offset,
                key,
                velocity,
                note_id,
            } => self.u8(1).u32(offset).u8(key).f32(velocity).i32(note_id),
            MidiEvent::AllNotesOff { offset } => self.u8(2).u32(offset),
        }
    }

    pub fn error(&mut self, e: &HostError) -> &mut Self {
        match e {
            HostError::BadKind(s) => self.u8(0).str(s),
            HostError::Load { path, reason } => self.u8(1).str(&path.to_string_lossy()).str(reason),
            HostError::NotFound(s) => self.u8(2).str(s),
            HostError::Plugin(s) => self.u8(3).str(s),
            HostError::Unsupported(s) => self.u8(4).str(s),
            HostError::Io(e) => self.u8(5).str(&e.to_string()),
        }
    }

    pub fn info(&mut self, i: &PluginInfo) -> &mut Self {
        let json = serde_json::to_vec(i).unwrap_or_default();
        self.bytes(&json)
    }

    pub fn param(&mut self, p: &ExternalParam) -> &mut Self {
        self.u32(p.id)
            .str(&p.name)
            .str(&p.unit)
            .f64(p.default)
            .u32(p.steps)
            .bool(p.automatable)
            .bool(p.hidden)
    }
}

/// Lettore di un payload.
pub struct Dec<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Dec<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.buf.len() - self.pos < n {
            return Err(bad("troncato"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 byte")))
    }
    pub fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("4 byte")))
    }
    pub fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("4 byte")))
    }
    pub fn f64(&mut self) -> io::Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().expect("8 byte")))
    }
    pub fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn string(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }
    /// Legge un vettore di f32 nel buffer indicato (sostituendone il contenuto).
    pub fn f32s_into(&mut self, out: &mut Vec<f32>) -> io::Result<()> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(4).ok_or_else(|| bad("lunghezza"))?)?;
        out.clear();
        out.extend(
            raw.chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().expect("4 byte"))),
        );
        Ok(())
    }

    pub fn event(&mut self) -> io::Result<MidiEvent> {
        Ok(match self.u8()? {
            0 => MidiEvent::NoteOn {
                offset: self.u32()?,
                key: self.u8()?,
                velocity: self.f32()?,
                note_id: self.i32()?,
            },
            1 => MidiEvent::NoteOff {
                offset: self.u32()?,
                key: self.u8()?,
                velocity: self.f32()?,
                note_id: self.i32()?,
            },
            2 => MidiEvent::AllNotesOff { offset: self.u32()? },
            _ => return Err(bad("evento")),
        })
    }

    pub fn error(&mut self) -> io::Result<HostError> {
        Ok(match self.u8()? {
            0 => HostError::BadKind(self.string()?),
            1 => HostError::Load {
                path: PathBuf::from(self.string()?),
                reason: self.string()?,
            },
            2 => HostError::NotFound(self.string()?),
            3 => HostError::Plugin(self.string()?),
            4 => HostError::Unsupported(self.string()?),
            5 => HostError::Io(io::Error::other(self.string()?)),
            _ => return Err(bad("errore")),
        })
    }

    pub fn info(&mut self) -> io::Result<PluginInfo> {
        serde_json::from_slice(self.bytes()?).map_err(|_| bad("info"))
    }

    pub fn param(&mut self) -> io::Result<ExternalParam> {
        Ok(ExternalParam {
            id: self.u32()?,
            name: self.string()?,
            unit: self.string()?,
            default: self.f64()?,
            steps: self.u32()?,
            automatable: self.bool()?,
            hidden: self.bool()?,
        })
    }
}

/// Scrive un frame già completo.
pub fn write_frame(w: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    w.write_all(frame)?;
    w.flush()
}

/// Legge il payload del frame successivo in `buf`. `Ok(false)` a fine flusso.
pub fn read_frame(r: &mut impl Read, buf: &mut Vec<u8>) -> io::Result<bool> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(bad("lunghezza del frame"));
    }
    buf.resize(len, 0);
    r.read_exact(buf)?;
    Ok(true)
}

// ------------------------------------------------------------------ host → helper

/// Tipi dei messaggi host → helper.
pub mod to_child {
    /// Richiesta con risposta: id u32, poi [`super::Call`].
    pub const CALL: u8 = 1;
    /// Cambio di parametro senza risposta: id u32, valore f64.
    pub const SET_PARAM: u8 = 2;
    /// Reset senza risposta.
    pub const RESET: u8 = 3;
    /// Blocco audio: seq u32, campioni u32, eventi, ingresso opzionale.
    pub const PROCESS: u8 = 4;
}

/// Tipi dei messaggi helper → host.
pub mod from_child {
    /// Saluto iniziale: versione, poi esito della creazione dell'istanza.
    pub const READY: u8 = 1;
    /// Risposta a una [`super::Call`]: id u32, poi [`super::Reply`].
    pub const REPLY: u8 = 2;
    /// Blocco elaborato: seq u32, ok, uscite.
    pub const PROCESSED: u8 = 3;
    /// L'utente ha chiuso la finestra dell'editor.
    pub const EDITOR_CLOSED: u8 = 4;
    /// La latenza del plugin è cambiata: u32.
    pub const LATENCY: u8 = 5;
}

/// Richieste con risposta (rispecchiano [`crate::ExternalInstance`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Prepare {
        sample_rate: f32,
        max_block: u32,
    },
    Params,
    ParamNormalized(u32),
    ParamDisplay(u32, f64),
    SaveState,
    LoadState(Vec<u8>),
    Reset,
    HasEditor,
    /// Apre l'editor in una finestra dell'helper con il titolo indicato.
    OpenEditor(String),
    CloseEditor,
    Idle,
    Latency,
    Info,
    /// Chiude l'istanza e termina l'helper.
    Shutdown,
}

impl Call {
    pub fn encode(&self, e: &mut Enc) {
        match self {
            Call::Prepare { sample_rate, max_block } => e.u8(0).f32(*sample_rate).u32(*max_block),
            Call::Params => e.u8(1),
            Call::ParamNormalized(id) => e.u8(2).u32(*id),
            Call::ParamDisplay(id, v) => e.u8(3).u32(*id).f64(*v),
            Call::SaveState => e.u8(4),
            Call::LoadState(d) => e.u8(5).bytes(d),
            Call::Reset => e.u8(6),
            Call::HasEditor => e.u8(7),
            Call::OpenEditor(t) => e.u8(8).str(t),
            Call::CloseEditor => e.u8(9),
            Call::Idle => e.u8(10),
            Call::Latency => e.u8(11),
            Call::Info => e.u8(12),
            Call::Shutdown => e.u8(13),
        };
    }

    pub fn decode(d: &mut Dec) -> io::Result<Self> {
        Ok(match d.u8()? {
            0 => Call::Prepare {
                sample_rate: d.f32()?,
                max_block: d.u32()?,
            },
            1 => Call::Params,
            2 => Call::ParamNormalized(d.u32()?),
            3 => Call::ParamDisplay(d.u32()?, d.f64()?),
            4 => Call::SaveState,
            5 => Call::LoadState(d.bytes()?.to_vec()),
            6 => Call::Reset,
            7 => Call::HasEditor,
            8 => Call::OpenEditor(d.string()?),
            9 => Call::CloseEditor,
            10 => Call::Idle,
            11 => Call::Latency,
            12 => Call::Info,
            13 => Call::Shutdown,
            _ => return Err(bad("richiesta")),
        })
    }
}

/// Risposte.
#[derive(Debug)]
pub enum Reply {
    Unit,
    Err(HostError),
    Bool(bool),
    U32(u32),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    Params(Vec<ExternalParam>),
    Size(EditorSize),
    Info(Box<PluginInfo>),
}

impl Reply {
    pub fn encode(&self, e: &mut Enc) {
        match self {
            Reply::Unit => e.u8(0),
            Reply::Err(err) => e.u8(1).error(err),
            Reply::Bool(b) => e.u8(2).bool(*b),
            Reply::U32(v) => e.u8(3).u32(*v),
            Reply::F64(v) => e.u8(4).f64(*v),
            Reply::Str(s) => e.u8(5).str(s),
            Reply::Bytes(b) => e.u8(6).bytes(b),
            Reply::Params(ps) => {
                e.u8(7).u32(ps.len() as u32);
                for p in ps {
                    e.param(p);
                }
                e
            }
            Reply::Size(s) => e.u8(8).u32(s.width).u32(s.height),
            Reply::Info(i) => e.u8(9).info(i),
        };
    }

    pub fn decode(d: &mut Dec) -> io::Result<Self> {
        Ok(match d.u8()? {
            0 => Reply::Unit,
            1 => Reply::Err(d.error()?),
            2 => Reply::Bool(d.bool()?),
            3 => Reply::U32(d.u32()?),
            4 => Reply::F64(d.f64()?),
            5 => Reply::Str(d.string()?),
            6 => Reply::Bytes(d.bytes()?.to_vec()),
            7 => {
                let n = d.u32()? as usize;
                let mut v = Vec::with_capacity(n.min(65_536));
                for _ in 0..n {
                    v.push(d.param()?);
                }
                Reply::Params(v)
            }
            8 => Reply::Size(EditorSize {
                width: d.u32()?,
                height: d.u32()?,
            }),
            9 => Reply::Info(Box::new(d.info()?)),
            _ => return Err(bad("risposta")),
        })
    }
}

/// Codifica una richiesta di blocco audio.
pub fn encode_process(e: &mut Enc, seq: u32, frames: usize, events: &[MidiEvent], input: Option<(&[f32], &[f32])>) {
    e.begin(to_child::PROCESS)
        .u32(seq)
        .u32(frames as u32)
        .u32(events.len() as u32);
    for ev in events {
        e.event(ev);
    }
    match input {
        Some((l, r)) => {
            e.bool(true).f32s(&l[..frames]).f32s(&r[..frames]);
        }
        None => {
            e.bool(false);
        }
    }
}

/// Richiesta di blocco audio decodificata (lato helper).
#[derive(Debug, Default)]
pub struct ProcessReq {
    pub seq: u32,
    pub frames: usize,
    pub events: Vec<MidiEvent>,
    pub input: Option<(Vec<f32>, Vec<f32>)>,
}

impl ProcessReq {
    pub fn decode(d: &mut Dec) -> io::Result<Self> {
        let seq = d.u32()?;
        let frames = d.u32()? as usize;
        let n = d.u32()? as usize;
        let mut events = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            events.push(d.event()?);
        }
        let input = if d.bool()? {
            let (mut l, mut r) = (Vec::new(), Vec::new());
            d.f32s_into(&mut l)?;
            d.f32s_into(&mut r)?;
            if l.len() != frames || r.len() != frames {
                return Err(bad("ingresso"));
            }
            Some((l, r))
        } else {
            None
        };
        Ok(Self {
            seq,
            frames,
            events,
            input,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(e: &mut Enc) -> Vec<u8> {
        let f = e.finish().to_vec();
        let mut r = &f[..];
        let mut buf = Vec::new();
        assert!(read_frame(&mut r, &mut buf).unwrap());
        assert!(!read_frame(&mut r, &mut buf).unwrap(), "fine flusso");
        buf
    }

    #[test]
    fn calls_and_replies_roundtrip() {
        let calls = vec![
            Call::Prepare {
                sample_rate: 44_100.0,
                max_block: 512,
            },
            Call::Params,
            Call::ParamNormalized(7),
            Call::ParamDisplay(3, 0.25),
            Call::SaveState,
            Call::LoadState(vec![1, 2, 3]),
            Call::Reset,
            Call::HasEditor,
            Call::OpenEditor("Synth è".into()),
            Call::CloseEditor,
            Call::Idle,
            Call::Latency,
            Call::Info,
            Call::Shutdown,
        ];
        let mut e = Enc::new();
        for c in calls {
            e.begin(to_child::CALL).u32(9);
            c.encode(&mut e);
            let p = payload(&mut e);
            let mut d = Dec::new(&p);
            assert_eq!(d.u8().unwrap(), to_child::CALL);
            assert_eq!(d.u32().unwrap(), 9);
            assert_eq!(Call::decode(&mut d).unwrap(), c);
        }

        let param = ExternalParam {
            id: 4,
            name: "Gain".into(),
            unit: "dB".into(),
            default: 0.5,
            steps: 0,
            automatable: true,
            hidden: false,
        };
        e.begin(from_child::REPLY).u32(1);
        Reply::Params(vec![param.clone()]).encode(&mut e);
        let p = payload(&mut e);
        let mut d = Dec::new(&p);
        d.u8().unwrap();
        d.u32().unwrap();
        match Reply::decode(&mut d).unwrap() {
            Reply::Params(v) => assert_eq!(v, vec![param]),
            r => panic!("{r:?}"),
        }

        e.begin(from_child::REPLY).u32(1);
        Reply::Err(HostError::Load {
            path: "/x.clap".into(),
            reason: "no".into(),
        })
        .encode(&mut e);
        let p = payload(&mut e);
        let mut d = Dec::new(&p);
        d.u8().unwrap();
        d.u32().unwrap();
        match Reply::decode(&mut d).unwrap() {
            Reply::Err(HostError::Load { path, reason }) => {
                assert_eq!(path, PathBuf::from("/x.clap"));
                assert_eq!(reason, "no");
            }
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn process_roundtrip() {
        let events = [
            MidiEvent::NoteOn {
                offset: 3,
                key: 69,
                velocity: 0.5,
                note_id: -1,
            },
            MidiEvent::AllNotesOff { offset: 9 },
        ];
        let l = [0.1f32, 0.2, 0.3, 0.4];
        let r = [-0.1f32, -0.2, -0.3, -0.4];
        let mut e = Enc::new();
        encode_process(&mut e, 42, 3, &events, Some((&l, &r)));
        let p = payload(&mut e);
        let mut d = Dec::new(&p);
        assert_eq!(d.u8().unwrap(), to_child::PROCESS);
        let req = ProcessReq::decode(&mut d).unwrap();
        assert_eq!(req.seq, 42);
        assert_eq!(req.frames, 3);
        assert_eq!(req.events, events.to_vec());
        let (il, ir) = req.input.unwrap();
        assert_eq!(il, l[..3].to_vec());
        assert_eq!(ir, r[..3].to_vec());
    }

    #[test]
    fn rejects_garbage() {
        let mut r: &[u8] = &[0, 0, 0, 0];
        assert!(read_frame(&mut r, &mut Vec::new()).is_err());
        let mut r: &[u8] = &[10, 0, 0, 0, 1];
        assert!(read_frame(&mut r, &mut Vec::new()).is_err());
        let mut d = Dec::new(&[99]);
        assert!(Call::decode(&mut d).is_err());
        let mut d = Dec::new(&[]);
        assert!(d.u32().is_err());
    }
}
