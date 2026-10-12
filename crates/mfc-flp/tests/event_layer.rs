//! Test del livello eventi: round-trip byte-identico, input troncati/casuali senza panic.

use mfc_flp::{id, inspect, parse, parse_events, write, EventValue, FlpError, RawEvent, TextEncoding};

/// Generatore xorshift64 deterministico.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn varint(mut v: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

fn file_with(events: &[u8], ppq: u16) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(b"FLhd");
    f.extend_from_slice(&6u32.to_le_bytes());
    f.extend_from_slice(&0i16.to_le_bytes());
    f.extend_from_slice(&3u16.to_le_bytes());
    f.extend_from_slice(&ppq.to_le_bytes());
    f.extend_from_slice(b"FLdt");
    f.extend_from_slice(&(events.len() as u32).to_le_bytes());
    f.extend_from_slice(events);
    f
}

/// Fixture costruita a mano con tutte le classi di dimensione.
fn fixture_events() -> Vec<u8> {
    let mut e = Vec::new();
    // Testo ASCII versione.
    e.push(id::FL_VERSION);
    let v = b"20.8.4.2576\0";
    e.extend(varint(v.len()));
    e.extend_from_slice(v);
    // Byte, word, dword.
    e.extend_from_slice(&[id::LOOP_ACTIVE, 1]);
    e.extend_from_slice(&[id::CURRENT_PATTERN, 0x34, 0x12]);
    e.push(id::TEMPO);
    e.extend_from_slice(&128_000u32.to_le_bytes());
    // Testo UTF-16.
    e.push(id::TITLE);
    let t: Vec<u8> = "Prova ♪\0".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    e.extend(varint(t.len()));
    e.extend_from_slice(&t);
    // Dati vuoti.
    e.extend_from_slice(&[id::PLUGIN_DATA, 0]);
    // Dati con varint a 2 byte (200) e a 3 byte (20000).
    for len in [127usize, 128, 200, 16383, 16384, 20000] {
        e.push(id::PAT_NOTES);
        e.extend(varint(len));
        e.extend((0..len).map(|i| (i * 7 % 251) as u8));
    }
    // Varint non minimo: lunghezza 5 codificata su 2 byte.
    e.extend_from_slice(&[id::CHAN_LEVELS, 0x85, 0x00, 1, 2, 3, 4, 5]);
    // Varint non minimo per lunghezza 0 su 3 byte.
    e.extend_from_slice(&[id::PLAYLIST_ITEMS, 0x80, 0x80, 0x00]);
    // Id sconosciuti in ogni classe.
    e.extend_from_slice(&[63, 0xff, 127, 1, 2, 191, 1, 2, 3, 4, 255, 2, 0xaa, 0xbb]);
    e
}

#[test]
fn fixture_roundtrip_byte_identical() {
    let bytes = file_with(&fixture_events(), 96);
    let f = parse(&bytes).unwrap();
    assert_eq!(f.header.ppq, 96);
    assert_eq!(f.header.n_channels, 3);
    assert_eq!(f.version().as_deref(), Some("20.8.4.2576"));
    assert_eq!(f.text_encoding(), TextEncoding::Utf16);
    assert_eq!(f.text(id::TITLE).as_deref(), Some("Prova ♪"));
    assert_eq!(f.first(id::LOOP_ACTIVE).unwrap().value, EventValue::Byte(1));
    assert_eq!(f.first(id::CURRENT_PATTERN).unwrap().value, EventValue::Word(0x1234));
    assert_eq!(f.first(id::TEMPO).unwrap().value, EventValue::Dword(128_000));
    let sizes: Vec<usize> = f
        .events
        .iter()
        .filter(|e| e.id == id::PAT_NOTES)
        .map(|e| e.value.size())
        .collect();
    assert_eq!(sizes, vec![127, 128, 200, 16383, 16384, 20000]);
    let lv = f.first(id::CHAN_LEVELS).unwrap();
    assert_eq!(lv.varint_len, 2);
    assert_eq!(lv.as_data(), Some(&[1u8, 2, 3, 4, 5][..]));
    assert_eq!(f.first(id::PLAYLIST_ITEMS).unwrap().varint_len, 3);
    assert_eq!(write(&f), bytes);
}

#[test]
fn trailing_bytes_preserved() {
    let mut bytes = file_with(&fixture_events(), 960);
    bytes.extend_from_slice(b"extra");
    let f = parse(&bytes).unwrap();
    assert_eq!(f.trailing, b"extra");
    assert_eq!(write(&f), bytes);
}

#[test]
fn every_truncation_is_an_error() {
    let bytes = file_with(&fixture_events(), 96);
    for cut in 0..bytes.len() {
        assert!(parse(&bytes[..cut]).is_err(), "prefisso {cut} accettato");
    }
}

#[test]
fn truncated_event_stream_never_panics() {
    let ev = fixture_events();
    let mut ok = 0;
    for cut in 0..=ev.len() {
        // Il chunk dichiara esattamente `cut` byte: può terminare a metà evento.
        let bytes = file_with(&ev[..cut], 96);
        match parse(&bytes) {
            Ok(f) => {
                ok += 1;
                assert_eq!(write(&f), bytes);
            }
            Err(FlpError::TruncatedEvent { .. }) | Err(FlpError::BadVarint(_)) => {}
            Err(e) => panic!("errore inatteso a {cut}: {e}"),
        }
    }
    // Solo i tagli ai confini degli eventi sono validi.
    assert_eq!(ok, parse_events(&ev).unwrap().len() + 1);
}

#[test]
fn bad_headers() {
    let good = file_with(&[], 96);
    let mut b = good.clone();
    b[0] = b'X';
    assert!(matches!(parse(&b), Err(FlpError::BadMagic("FLhd"))));
    let mut b = good.clone();
    b[4] = 7;
    assert!(matches!(parse(&b), Err(FlpError::BadHeader(_))));
    let mut b = good.clone();
    b[14] = b'X';
    assert!(matches!(parse(&b), Err(FlpError::BadMagic("FLdt"))));
    // Lunghezza dati enorme.
    let mut b = good.clone();
    b[18..22].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse(&b).is_err());
    // Varint oltre 5 byte.
    let b = file_with(&[208, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 96);
    assert!(matches!(parse(&b), Err(FlpError::BadVarint(_))));
    // Varint > u32::MAX.
    let b = file_with(&[208, 0xff, 0xff, 0xff, 0xff, 0x7f], 96);
    assert!(matches!(parse(&b), Err(FlpError::BadVarint(_))));
    // Lunghezza dichiarata enorme ma dati assenti: nessuna allocazione, solo errore.
    let b = file_with(&[208, 0xff, 0xff, 0xff, 0xff, 0x0f], 96);
    assert!(matches!(
        parse(&b),
        Err(FlpError::TruncatedEvent { offset: 0, id: 208 })
    ));
    // File vuoto valido.
    assert_eq!(write(&parse(&good).unwrap()), good);
}

#[test]
fn fuzz_random_buffers() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let fixture = file_with(&fixture_events(), 96);
    for i in 0..2000 {
        let buf: Vec<u8> = match i % 4 {
            // Puro rumore.
            0 => (0..rng.below(300)).map(|_| rng.next() as u8).collect(),
            // Header valido + rumore come eventi.
            1 => {
                let ev: Vec<u8> = (0..rng.below(400)).map(|_| rng.next() as u8).collect();
                file_with(&ev, 96)
            }
            // Fixture con byte mutati.
            2 => {
                let mut b = fixture.clone();
                for _ in 0..1 + rng.below(8) {
                    let p = rng.below(b.len());
                    b[p] ^= 1 << rng.below(8);
                }
                b
            }
            // Fixture troncata/estesa casualmente.
            _ => {
                let mut b = fixture[..rng.below(fixture.len())].to_vec();
                b.extend((0..rng.below(16)).map(|_| rng.next() as u8));
                b
            }
        };
        if let Ok(f) = parse(&buf) {
            // Ogni input accettato deve tornare identico.
            assert_eq!(write(&f), buf, "round-trip fallito al caso {i}");
            let _ = inspect(&f);
            let _ = mfc_flp::to_project(&f).unwrap();
        }
    }
}

#[test]
fn inspect_summaries() {
    let f = parse(&file_with(&fixture_events(), 96)).unwrap();
    let s = inspect(&f);
    assert_eq!(s.len(), f.events.len());
    assert_eq!(s[0].name, Some("FLVersion"));
    assert_eq!(s[0].kind, "text");
    assert_eq!(s[0].preview, "\"20.8.4.2576\"");
    let title = s.iter().find(|x| x.id == id::TITLE).unwrap();
    assert_eq!(title.preview, "\"Prova ♪\"");
    let tempo = s.iter().find(|x| x.id == id::TEMPO).unwrap();
    assert_eq!((tempo.kind, tempo.size), ("dword", 4));
    assert!(tempo.preview.starts_with("128000"));
    let big = s.iter().find(|x| x.size == 20000).unwrap();
    assert!(big.preview.ends_with('…'));
    assert_eq!(big.kind, "data");
}

#[test]
fn mismatched_value_classes_are_coerced() {
    let f = mfc_flp::FlpFile {
        events: vec![
            RawEvent {
                id: 5,
                value: EventValue::Dword(0x1_02),
                varint_len: 0,
            },
            RawEvent {
                id: 70,
                value: EventValue::Data(vec![1, 2, 3]),
                varint_len: 0,
            },
            RawEvent {
                id: 210,
                value: EventValue::Word(0x0304),
                varint_len: 0,
            },
        ],
        ..Default::default()
    };
    let back = parse(&write(&f)).unwrap();
    assert_eq!(back.events[0].value, EventValue::Byte(2));
    assert_eq!(back.events[1].value, EventValue::Word(0x0201));
    assert_eq!(back.events[2].value, EventValue::Data(vec![4, 3]));
}

/// Sequenze casuali di eventi con id noti: l'import semantico non deve mai andare in panic.
#[test]
fn fuzz_semantic_import() {
    let ids: Vec<u8> = (0..=255u8).filter(|i| mfc_flp::event_name(*i).is_some()).collect();
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    for _ in 0..600 {
        let mut events = Vec::new();
        if rng.below(2) == 0 {
            let v = ["20.8.4.2576", "21.0.3", "9.1", "", "x.y"][rng.below(5)];
            events.push(RawEvent::text(id::FL_VERSION, v, TextEncoding::Ascii));
        }
        for _ in 0..rng.below(60) {
            let id = ids[rng.below(ids.len())];
            let ev = match mfc_flp::EventKind::of(id) {
                mfc_flp::EventKind::Text | mfc_flp::EventKind::Data => {
                    // Lunghezze vicine ai multipli dei record (12/24/32/60) e casuali.
                    let len = [0, 1, 11, 12, 24, 25, 32, 48, 60, 64, 127, 240][rng.below(12)] * (1 + rng.below(3));
                    RawEvent::data(id, (0..len).map(|_| rng.next() as u8).collect())
                }
                _ => RawEvent::num(id, rng.next() as u32),
            };
            events.push(ev);
        }
        let f = mfc_flp::FlpFile {
            header: mfc_flp::FlpHeader {
                format: 0,
                n_channels: 0,
                ppq: rng.next() as u16,
            },
            events,
            trailing: Vec::new(),
        };
        let res = mfc_flp::to_project(&f).unwrap();
        // Anche il re-export del risultato deve funzionare.
        let back = parse(&write(&mfc_flp::from_project(&res.project))).unwrap();
        let _ = mfc_flp::to_project(&back).unwrap();
    }
}
