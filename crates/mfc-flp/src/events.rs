//! Livello eventi (lossless) del formato FLP.
//!
//! Mappatura degli id derivata esclusivamente da documentazione pubblica della community
//! (PyFLP, Monad.FLParser, note "FLP format").

use serde::Serialize;

/// Inizio della classe di eventi a 1 byte.
pub const BYTE: u8 = 0;
/// Inizio della classe di eventi a 2 byte.
pub const WORD: u8 = 64;
/// Inizio della classe di eventi a 4 byte.
pub const DWORD: u8 = 128;
/// Inizio della classe di eventi testuali (lunghezza variabile).
pub const TEXT: u8 = 192;
/// Inizio della classe di eventi binari (lunghezza variabile).
pub const DATA: u8 = 208;

/// Id degli eventi noti (nomi come in PyFLP).
pub mod id {
    use super::{DATA, DWORD, TEXT, WORD};

    // --- progetto ---
    pub const LOOP_ACTIVE: u8 = 9;
    pub const SHOW_INFO: u8 = 10;
    pub const SWING: u8 = 11;
    pub const MAIN_VOLUME: u8 = 12;
    pub const FIT_TO_STEPS: u8 = 13;
    pub const TIME_SIG_NUM: u8 = 17;
    pub const TIME_SIG_BEAT: u8 = 18;
    pub const PAN_LAW: u8 = 23;
    pub const LICENSED: u8 = 28;
    pub const MIXER_APDC: u8 = 29;
    pub const PLAY_TRUNCATED_NOTES: u8 = 30;
    pub const TEMPO_COARSE: u8 = WORD + 2;
    pub const CURRENT_PATTERN: u8 = WORD + 3;
    pub const MAIN_PITCH: u8 = WORD + 16;
    pub const TEMPO_FINE: u8 = WORD + 29;
    pub const CUR_GROUP_ID: u8 = DWORD + 18;
    pub const RACK_WINDOW_HEIGHT: u8 = DWORD + 5;
    pub const TEMPO: u8 = DWORD + 28;
    pub const FL_BUILD: u8 = DWORD + 31;
    pub const TITLE: u8 = TEXT + 2;
    pub const COMMENTS: u8 = TEXT + 3;
    pub const URL: u8 = TEXT + 5;
    pub const RTF_COMMENTS: u8 = TEXT + 6;
    pub const FL_VERSION: u8 = TEXT + 7;
    pub const LICENSEE: u8 = TEXT + 8;
    pub const DATA_PATH: u8 = TEXT + 10;
    pub const GENRE: u8 = TEXT + 14;
    pub const ARTISTS: u8 = TEXT + 15;
    pub const TIMESTAMP: u8 = DATA + 29;

    // --- canali ---
    pub const CHAN_ENABLED: u8 = 0;
    pub const CHAN_VOL_BYTE: u8 = 2;
    pub const CHAN_PAN_BYTE: u8 = 3;
    pub const CHAN_ZIPPED: u8 = 15;
    pub const CHAN_PING_PONG_LOOP: u8 = 20;
    pub const CHAN_TYPE: u8 = 21;
    pub const CHAN_ROUTED_TO: u8 = 22;
    pub const CHAN_LOCKED: u8 = 32;
    pub const CHAN_NEW: u8 = WORD;
    pub const CHAN_FREQ_TILT: u8 = WORD + 5;
    pub const CHAN_FX_FLAGS: u8 = WORD + 6;
    pub const CHAN_CUTOFF: u8 = WORD + 7;
    pub const CHAN_VOL_WORD: u8 = WORD + 8;
    pub const CHAN_PAN_WORD: u8 = WORD + 9;
    pub const CHAN_PREAMP: u8 = WORD + 10;
    pub const CHAN_FADE_OUT: u8 = WORD + 11;
    pub const CHAN_FADE_IN: u8 = WORD + 12;
    pub const CHAN_RESONANCE: u8 = WORD + 19;
    pub const CHAN_STEREO_DELAY: u8 = WORD + 21;
    pub const CHAN_POGO: u8 = WORD + 22;
    pub const CHAN_TIME_SHIFT: u8 = WORD + 25;
    pub const CHAN_CHILDREN: u8 = WORD + 30;
    pub const CHAN_SWING: u8 = WORD + 33;
    pub const CHAN_RING_MOD: u8 = DWORD + 3;
    pub const CHAN_CUT_GROUP: u8 = DWORD + 4;
    pub const CHAN_ROOT_NOTE: u8 = DWORD + 7;
    pub const CHAN_DELAY_MOD_XY: u8 = DWORD + 10;
    pub const CHAN_REVERB: u8 = DWORD + 11;
    pub const CHAN_STRETCH_TIME: u8 = DWORD + 12;
    pub const CHAN_FINE_TUNE: u8 = DWORD + 14;
    pub const CHAN_SAMPLER_FLAGS: u8 = DWORD + 15;
    pub const CHAN_LAYER_FLAGS: u8 = DWORD + 16;
    pub const CHAN_GROUP_NUM: u8 = DWORD + 17;
    pub const CHAN_AU_SAMPLE_RATE: u8 = DWORD + 25;
    pub const CHAN_NAME_OLD: u8 = TEXT;
    pub const CHAN_SAMPLE_PATH: u8 = TEXT + 4;
    pub const CHAN_DELAY: u8 = DATA + 1;
    pub const CHAN_PARAMETERS: u8 = DATA + 7;
    pub const CHAN_ENVELOPE_LFO: u8 = DATA + 10;
    pub const CHAN_LEVELS: u8 = DATA + 11;
    pub const CHAN_POLYPHONY: u8 = DATA + 13;
    pub const CHAN_TRACKING: u8 = DATA + 20;
    pub const CHAN_LEVEL_ADJUSTS: u8 = DATA + 21;
    pub const CHAN_AUTOMATION: u8 = DATA + 26;
    pub const DISPLAY_GROUP_NAME: u8 = TEXT + 39;

    // --- plugin (condivisi da canali e slot del mixer) ---
    pub const PLUGIN_COLOR: u8 = DWORD;
    pub const PLUGIN_ICON: u8 = DWORD + 27;
    pub const PLUGIN_INTERNAL_NAME: u8 = TEXT + 9;
    pub const PLUGIN_NAME: u8 = TEXT + 11;
    pub const PLUGIN_WRAPPER: u8 = DATA + 4;
    pub const PLUGIN_DATA: u8 = DATA + 5;

    // --- pattern ---
    pub const PAT_LOOPED: u8 = 26;
    pub const PAT_NEW: u8 = WORD + 1;
    pub const PAT_COLOR: u8 = DWORD + 22;
    pub const PAT_CHANNEL_IID: u8 = DWORD + 32;
    pub const PAT_161: u8 = DWORD + 33;
    pub const PAT_162: u8 = DWORD + 34;
    pub const PAT_LENGTH: u8 = DWORD + 36;
    pub const PAT_NAME: u8 = TEXT + 1;
    pub const PAT_CONTROLLERS: u8 = DATA + 15;
    pub const PAT_NOTES: u8 = DATA + 16;

    // --- time marker ---
    pub const MARKER_NUMERATOR: u8 = 33;
    pub const MARKER_DENOMINATOR: u8 = 34;
    pub const MARKER_POSITION: u8 = DWORD + 20;
    pub const MARKER_NAME: u8 = TEXT + 13;

    // --- arrangement / playlist ---
    pub const ARRANGEMENT_NEW: u8 = WORD + 35;
    pub const ARRANGEMENT_CURRENT: u8 = WORD + 36;
    pub const LOOP_POS: u8 = DWORD + 24;
    pub const PL_SELECTION: u8 = DATA + 9;
    pub const PLAYLIST_ITEMS: u8 = DATA + 25;
    pub const TRACK_DATA: u8 = DATA + 30;
    pub const TRACK_NAME: u8 = TEXT + 47;
    pub const ARRANGEMENT_NAME: u8 = TEXT + 49;

    // --- mixer ---
    pub const INSERT_ICON: u8 = WORD + 31;
    pub const SLOT_INDEX: u8 = WORD + 34;
    pub const INSERT_OUTPUT: u8 = DWORD + 19;
    pub const INSERT_COLOR: u8 = DWORD + 21;
    pub const INSERT_INPUT: u8 = DWORD + 26;
    pub const INSERT_NAME: u8 = TEXT + 12;
    pub const MIXER_PARAMS: u8 = DATA + 17;
    pub const INSERT_ROUTING: u8 = DATA + 27;
    pub const INSERT_FLAGS: u8 = DATA + 28;

    // --- controller ---
    pub const MIDI_CONTROLLER: u8 = DATA + 18;
    pub const REMOTE_CONTROLLER: u8 = DATA + 19;
}

/// Nomi leggibili degli eventi noti.
const NAMES: &[(u8, &str)] = &[
    (id::CHAN_ENABLED, "ChanEnabled"),
    (id::CHAN_VOL_BYTE, "ChanVolByte"),
    (id::CHAN_PAN_BYTE, "ChanPanByte"),
    (id::LOOP_ACTIVE, "LoopActive"),
    (id::SHOW_INFO, "ShowInfo"),
    (id::SWING, "Swing"),
    (id::MAIN_VOLUME, "MainVolume"),
    (id::FIT_TO_STEPS, "FitToSteps"),
    (id::CHAN_ZIPPED, "ChanZipped"),
    (id::TIME_SIG_NUM, "TimeSigNum"),
    (id::TIME_SIG_BEAT, "TimeSigBeat"),
    (id::CHAN_PING_PONG_LOOP, "ChanPingPongLoop"),
    (id::CHAN_TYPE, "ChanType"),
    (id::CHAN_ROUTED_TO, "ChanRoutedTo"),
    (id::PAN_LAW, "PanLaw"),
    (id::PAT_LOOPED, "PatLooped"),
    (id::LICENSED, "Licensed"),
    (id::MIXER_APDC, "MixerAPDC"),
    (id::PLAY_TRUNCATED_NOTES, "PlayTruncatedNotes"),
    (id::CHAN_LOCKED, "ChanLocked"),
    (id::MARKER_NUMERATOR, "MarkerNumerator"),
    (id::MARKER_DENOMINATOR, "MarkerDenominator"),
    (id::CHAN_NEW, "ChanNew"),
    (id::PAT_NEW, "PatNew"),
    (id::TEMPO_COARSE, "TempoCoarse"),
    (id::CURRENT_PATTERN, "CurrentPattern"),
    (id::CHAN_FREQ_TILT, "ChanFreqTilt"),
    (id::CHAN_FX_FLAGS, "ChanFXFlags"),
    (id::CHAN_CUTOFF, "ChanCutoff"),
    (id::CHAN_VOL_WORD, "ChanVolWord"),
    (id::CHAN_PAN_WORD, "ChanPanWord"),
    (id::CHAN_PREAMP, "ChanPreamp"),
    (id::CHAN_FADE_OUT, "ChanFadeOut"),
    (id::CHAN_FADE_IN, "ChanFadeIn"),
    (id::MAIN_PITCH, "MainPitch"),
    (id::CHAN_RESONANCE, "ChanResonance"),
    (id::CHAN_STEREO_DELAY, "ChanStereoDelay"),
    (id::CHAN_POGO, "ChanPogo"),
    (id::CHAN_TIME_SHIFT, "ChanTimeShift"),
    (id::TEMPO_FINE, "TempoFine"),
    (id::CHAN_CHILDREN, "ChanChildren"),
    (id::INSERT_ICON, "InsertIcon"),
    (id::CHAN_SWING, "ChanSwing"),
    (id::SLOT_INDEX, "SlotIndex"),
    (id::ARRANGEMENT_NEW, "ArrangementNew"),
    (id::ARRANGEMENT_CURRENT, "ArrangementCurrent"),
    (id::PLUGIN_COLOR, "PluginColor"),
    (id::CHAN_RING_MOD, "ChanRingMod"),
    (id::CHAN_CUT_GROUP, "ChanCutGroup"),
    (id::RACK_WINDOW_HEIGHT, "RackWindowHeight"),
    (id::CHAN_ROOT_NOTE, "ChanRootNote"),
    (id::CHAN_DELAY_MOD_XY, "ChanDelayModXY"),
    (id::CHAN_REVERB, "ChanReverb"),
    (id::CHAN_STRETCH_TIME, "ChanStretchTime"),
    (id::CHAN_FINE_TUNE, "ChanFineTune"),
    (id::CHAN_SAMPLER_FLAGS, "ChanSamplerFlags"),
    (id::CHAN_LAYER_FLAGS, "ChanLayerFlags"),
    (id::CHAN_GROUP_NUM, "ChanGroupNum"),
    (id::CUR_GROUP_ID, "CurGroupId"),
    (id::INSERT_OUTPUT, "InsertOutput"),
    (id::MARKER_POSITION, "MarkerPosition"),
    (id::INSERT_COLOR, "InsertColor"),
    (id::PAT_COLOR, "PatColor"),
    (id::LOOP_POS, "LoopPos"),
    (id::CHAN_AU_SAMPLE_RATE, "ChanAUSampleRate"),
    (id::INSERT_INPUT, "InsertInput"),
    (id::PLUGIN_ICON, "PluginIcon"),
    (id::TEMPO, "Tempo"),
    (id::FL_BUILD, "FLBuild"),
    (id::PAT_CHANNEL_IID, "PatChannelIID"),
    (id::PAT_161, "Pat161"),
    (id::PAT_162, "Pat162"),
    (id::PAT_LENGTH, "PatLength"),
    (id::CHAN_NAME_OLD, "ChanNameOld"),
    (id::PAT_NAME, "PatName"),
    (id::TITLE, "Title"),
    (id::COMMENTS, "Comments"),
    (id::CHAN_SAMPLE_PATH, "ChanSamplePath"),
    (id::URL, "Url"),
    (id::RTF_COMMENTS, "RTFComments"),
    (id::FL_VERSION, "FLVersion"),
    (id::LICENSEE, "Licensee"),
    (id::PLUGIN_INTERNAL_NAME, "PluginInternalName"),
    (id::DATA_PATH, "DataPath"),
    (id::PLUGIN_NAME, "PluginName"),
    (id::INSERT_NAME, "InsertName"),
    (id::MARKER_NAME, "MarkerName"),
    (id::GENRE, "Genre"),
    (id::ARTISTS, "Artists"),
    (id::CHAN_DELAY, "ChanDelay"),
    (id::PLUGIN_WRAPPER, "PluginWrapper"),
    (id::PLUGIN_DATA, "PluginData"),
    (id::CHAN_PARAMETERS, "ChanParameters"),
    (id::PL_SELECTION, "PLSelection"),
    (id::CHAN_ENVELOPE_LFO, "ChanEnvelopeLFO"),
    (id::CHAN_LEVELS, "ChanLevels"),
    (id::CHAN_POLYPHONY, "ChanPolyphony"),
    (id::PAT_CONTROLLERS, "PatControllers"),
    (id::PAT_NOTES, "PatNotes"),
    (id::MIXER_PARAMS, "MixerParams"),
    (id::MIDI_CONTROLLER, "MIDIController"),
    (id::REMOTE_CONTROLLER, "RemoteController"),
    (id::CHAN_TRACKING, "ChanTracking"),
    (id::CHAN_LEVEL_ADJUSTS, "ChanLevelAdjusts"),
    (id::DISPLAY_GROUP_NAME, "DisplayGroupName"),
    (id::PLAYLIST_ITEMS, "PlaylistItems"),
    (id::CHAN_AUTOMATION, "ChanAutomation"),
    (id::INSERT_ROUTING, "InsertRouting"),
    (id::INSERT_FLAGS, "InsertFlags"),
    (id::TIMESTAMP, "Timestamp"),
    (id::TRACK_DATA, "TrackData"),
    (id::TRACK_NAME, "TrackName"),
    (id::ARRANGEMENT_NAME, "ArrangementName"),
];

/// Nome leggibile di un id evento noto.
pub fn event_name(id: u8) -> Option<&'static str> {
    NAMES.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
}

/// Classe di dimensione di un evento, determinata dall'id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Byte,
    Word,
    Dword,
    /// Lunghezza variabile, contenuto testuale.
    Text,
    /// Lunghezza variabile, contenuto binario.
    Data,
}

impl EventKind {
    pub fn of(id: u8) -> Self {
        match id {
            0..=63 => EventKind::Byte,
            64..=127 => EventKind::Word,
            128..=191 => EventKind::Dword,
            _ if is_text_id(id) => EventKind::Text,
            _ => EventKind::Data,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Byte => "byte",
            EventKind::Word => "word",
            EventKind::Dword => "dword",
            EventKind::Text => "text",
            EventKind::Data => "data",
        }
    }
}

/// Eventi testuali: 192..=207 più alcuni id "nuovi" (come in PyFLP).
pub fn is_text_id(id: u8) -> bool {
    (TEXT..DATA).contains(&id) || matches!(id, id::DISPLAY_GROUP_NAME | id::TRACK_NAME | id::ARRANGEMENT_NAME)
}

/// Valore grezzo di un evento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventValue {
    Byte(u8),
    Word(u16),
    Dword(u32),
    Data(Vec<u8>),
}

impl EventValue {
    /// Valore numerico (eventi a dimensione fissa).
    pub fn as_u32(&self) -> Option<u32> {
        match *self {
            EventValue::Byte(v) => Some(v as u32),
            EventValue::Word(v) => Some(v as u32),
            EventValue::Dword(v) => Some(v),
            EventValue::Data(_) => None,
        }
    }
    /// Payload (eventi a lunghezza variabile).
    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            EventValue::Data(d) => Some(d),
            _ => None,
        }
    }
    /// Dimensione del payload in byte.
    pub fn size(&self) -> usize {
        match self {
            EventValue::Byte(_) => 1,
            EventValue::Word(_) => 2,
            EventValue::Dword(_) => 4,
            EventValue::Data(d) => d.len(),
        }
    }
}

/// Evento grezzo. `varint_len` conserva la larghezza del prefisso di lunghezza quando
/// il file originale usava una codifica non minima (0 = minima), per round-trip esatto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvent {
    pub id: u8,
    pub value: EventValue,
    pub varint_len: u8,
}

impl RawEvent {
    /// Costruisce un evento a partire da un valore numerico, con la classe dedotta dall'id.
    /// Per gli id a lunghezza variabile il valore viene scritto come u32 little-endian.
    pub fn num(id: u8, v: u32) -> Self {
        let value = match EventKind::of(id) {
            EventKind::Byte => EventValue::Byte(v as u8),
            EventKind::Word => EventValue::Word(v as u16),
            EventKind::Dword => EventValue::Dword(v),
            _ => EventValue::Data(v.to_le_bytes().to_vec()),
        };
        Self {
            id,
            value,
            varint_len: 0,
        }
    }
    /// Evento a lunghezza variabile.
    pub fn data(id: u8, data: Vec<u8>) -> Self {
        Self {
            id,
            value: EventValue::Data(data),
            varint_len: 0,
        }
    }
    /// Evento testuale codificato con `enc`.
    pub fn text(id: u8, s: &str, enc: TextEncoding) -> Self {
        Self::data(id, encode_text(s, enc))
    }
    pub fn kind(&self) -> EventKind {
        EventKind::of(self.id)
    }
    pub fn as_u32(&self) -> Option<u32> {
        self.value.as_u32()
    }
    pub fn as_data(&self) -> Option<&[u8]> {
        self.value.as_data()
    }
}

/// Header del chunk `FLhd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlpHeader {
    /// Formato del file (0 = progetto).
    pub format: i16,
    /// Numero di canali nel Channel Rack.
    pub n_channels: u16,
    /// Risoluzione in tick per semiminima.
    pub ppq: u16,
}

impl Default for FlpHeader {
    fn default() -> Self {
        Self {
            format: 0,
            n_channels: 0,
            ppq: 96,
        }
    }
}

/// File FLP a livello di eventi.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlpFile {
    pub header: FlpHeader,
    pub events: Vec<RawEvent>,
    /// Byte eventualmente presenti dopo il chunk `FLdt` (conservati per round-trip).
    pub trailing: Vec<u8>,
}

impl FlpFile {
    /// Versione di FL Studio (evento 199, sempre ASCII).
    pub fn version(&self) -> Option<String> {
        self.events
            .iter()
            .find(|e| e.id == id::FL_VERSION)
            .and_then(|e| e.as_data())
            .map(|d| decode_text(d, TextEncoding::Ascii))
    }

    /// Codifica delle stringhe deducibile dalla versione (≥ 11.5 → UTF-16LE).
    pub fn text_encoding(&self) -> TextEncoding {
        match self.version() {
            Some(v) => encoding_for_version(&v),
            None => TextEncoding::Utf16,
        }
    }

    /// Primo evento con l'id dato.
    pub fn first(&self, id: u8) -> Option<&RawEvent> {
        self.events.iter().find(|e| e.id == id)
    }

    /// Testo decodificato del primo evento con l'id dato.
    pub fn text(&self, id: u8) -> Option<String> {
        let enc = if id == id::FL_VERSION {
            TextEncoding::Ascii
        } else {
            self.text_encoding()
        };
        self.first(id).and_then(|e| e.as_data()).map(|d| decode_text(d, enc))
    }
}

/// Codifica delle stringhe negli eventi testuali.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TextEncoding {
    /// ASCII/ANSI (FL < 11.5).
    Ascii,
    /// UTF-16LE (FL ≥ 11.5).
    Utf16,
}

/// Componenti numeriche di una stringa di versione "a.b.c.d".
pub fn parse_version(v: &str) -> Vec<u32> {
    v.trim_end_matches('\0')
        .split('.')
        .map(|p| p.trim().parse::<u32>().unwrap_or(0))
        .collect()
}

/// Codifica usata da una certa versione di FL (regola PyFLP: `[major, minor] >= [11, 5]`).
pub fn encoding_for_version(v: &str) -> TextEncoding {
    let parts = parse_version(v);
    let major = parts.first().copied().unwrap_or(0);
    let minor = parts.get(1).copied().unwrap_or(0);
    if (major, minor) >= (11, 5) {
        TextEncoding::Utf16
    } else {
        TextEncoding::Ascii
    }
}

/// Decodifica un payload testuale togliendo i NUL finali.
pub fn decode_text(data: &[u8], enc: TextEncoding) -> String {
    let s: String = match enc {
        // ANSI: approssimato come Latin-1.
        TextEncoding::Ascii => data.iter().map(|&b| b as char).collect(),
        TextEncoding::Utf16 => {
            let units = data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c));
            char::decode_utf16(units)
                .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect()
        }
    };
    s.trim_end_matches('\0').to_string()
}

/// Codifica una stringa con terminatore NUL.
pub fn encode_text(s: &str, enc: TextEncoding) -> Vec<u8> {
    match enc {
        TextEncoding::Ascii => {
            let mut v: Vec<u8> = s
                .chars()
                .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
                .collect();
            v.push(0);
            v
        }
        TextEncoding::Utf16 => {
            let mut v = Vec::with_capacity(s.len() * 2 + 2);
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v.extend_from_slice(&[0, 0]);
            v
        }
    }
}

/// Riassunto di un evento per ispezione (tool MCP `flp_inspect_events`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventSummary {
    pub index: usize,
    pub id: u8,
    pub name: Option<&'static str>,
    pub kind: &'static str,
    pub size: usize,
    pub preview: String,
}

const PREVIEW_HEX: usize = 16;
const PREVIEW_TEXT: usize = 64;

/// Elenco riassuntivo di tutti gli eventi.
pub fn inspect(file: &FlpFile) -> Vec<EventSummary> {
    let enc = file.text_encoding();
    file.events
        .iter()
        .enumerate()
        .map(|(index, e)| {
            let kind = e.kind();
            let preview = match &e.value {
                EventValue::Byte(v) => v.to_string(),
                EventValue::Word(v) => v.to_string(),
                EventValue::Dword(v) => format!("{v} (0x{v:08x})"),
                EventValue::Data(d) if kind == EventKind::Text => {
                    let enc = if e.id == id::FL_VERSION {
                        TextEncoding::Ascii
                    } else {
                        enc
                    };
                    let s = decode_text(d, enc);
                    let mut p: String = s.chars().take(PREVIEW_TEXT).collect();
                    if s.chars().count() > PREVIEW_TEXT {
                        p.push('…');
                    }
                    format!("{p:?}")
                }
                EventValue::Data(d) => {
                    let mut p = hex(&d[..d.len().min(PREVIEW_HEX)]);
                    if d.len() > PREVIEW_HEX {
                        p.push_str(" …");
                    }
                    p
                }
            };
            EventSummary {
                index,
                id: e.id,
                name: event_name(e.id),
                kind: kind.as_str(),
                size: e.value.size(),
                preview,
            }
        })
        .collect()
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(EventKind::of(0), EventKind::Byte);
        assert_eq!(EventKind::of(63), EventKind::Byte);
        assert_eq!(EventKind::of(64), EventKind::Word);
        assert_eq!(EventKind::of(128), EventKind::Dword);
        assert_eq!(EventKind::of(192), EventKind::Text);
        assert_eq!(EventKind::of(207), EventKind::Text);
        assert_eq!(EventKind::of(208), EventKind::Data);
        assert_eq!(EventKind::of(239), EventKind::Text);
        assert_eq!(EventKind::of(241), EventKind::Text);
        assert_eq!(EventKind::of(224), EventKind::Data);
    }

    #[test]
    fn names_unique_and_classes() {
        for (i, (a, _)) in NAMES.iter().enumerate() {
            assert!(NAMES[i + 1..].iter().all(|(b, _)| a != b), "id duplicato {a}");
        }
        assert_eq!(event_name(id::TEMPO), Some("Tempo"));
        assert_eq!(event_name(id::PAT_NOTES), Some("PatNotes"));
        assert_eq!(id::TEMPO, 156);
        assert_eq!(id::PLAYLIST_ITEMS, 233);
        assert_eq!(id::MIXER_PARAMS, 225);
        assert_eq!(id::INSERT_ROUTING, 235);
        assert_eq!(id::INSERT_FLAGS, 236);
        assert_eq!(id::CHAN_LEVELS, 219);
        assert_eq!(event_name(250), None);
    }

    #[test]
    fn text_by_version() {
        assert_eq!(encoding_for_version("20.8.4.2576"), TextEncoding::Utf16);
        assert_eq!(encoding_for_version("11.5.0"), TextEncoding::Utf16);
        assert_eq!(encoding_for_version("11.4.9"), TextEncoding::Ascii);
        assert_eq!(encoding_for_version("9.0.3"), TextEncoding::Ascii);
        let u = encode_text("Città ♪", TextEncoding::Utf16);
        assert_eq!(&u[u.len() - 2..], &[0, 0]);
        assert_eq!(decode_text(&u, TextEncoding::Utf16), "Città ♪");
        let a = encode_text("Hello", TextEncoding::Ascii);
        assert_eq!(a, b"Hello\0");
        assert_eq!(decode_text(&a, TextEncoding::Ascii), "Hello");
        // Byte dispari e surrogati spaiati non causano panic.
        assert_eq!(decode_text(&[0x41, 0x00, 0x42], TextEncoding::Utf16), "A");
        assert_eq!(decode_text(&[0x00, 0xd8], TextEncoding::Utf16), "\u{fffd}");
    }
}
