//! Import semantico: [`FlpFile`] → [`Project`]. Ciò che non è mappabile genera un avviso,
//! mai un errore; gli eventi originali restano comunque disponibili nel [`FlpFile`].

use std::collections::{BTreeMap, HashMap};

use mfc_core::{
    AutomationClip, AutomationId, AutomationPoint, AutomationTarget, Channel, ChannelId, ChannelKind, ClipId,
    ClipSource, Color, EffectSlot, Note, Pattern, PatternId, PlaylistClip, PlaylistTrack, PluginInstance, Project,
    Route, Tick, TimeSignature, MIXER_INSERTS, PLAYLIST_TRACKS,
};

use crate::events::{decode_text, event_name, id, parse_version, FlpFile, RawEvent, TextEncoding};
use crate::plugin_map::{
    effect_kind, external_from_wrapper, generator_kind, FL_WRAPPER, FOREIGN_EFFECT, FOREIGN_GENERATOR,
};
use crate::structs::{
    insert_flag, insert_flags_parse, mixer_param, FlAutoTarget, FlAutomation, FlLevels, FlMixerParam, FlNote,
    FlPlaylistItem, FlRemoteController, FlTrackData, ITEM_FLAG_MUTED, ITEM_SIZE_NEW, ITEM_SIZE_OLD, NOTE_FLAG_SLIDE,
};
use crate::FlpError;

/// Risultato dell'import: progetto + avvisi sulle informazioni non mappate.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    pub project: Project,
    pub warnings: Vec<String>,
}

#[derive(Default)]
struct ChanAcc {
    iid: u16,
    ty: Option<u8>,
    name_old: Option<String>,
    name: Option<String>,
    internal_name: Option<String>,
    plugin_data: Option<Vec<u8>>,
    sample_path: Option<String>,
    levels: Option<FlLevels>,
    vol_word: Option<u16>,
    pan_word: Option<u16>,
    vol_byte: Option<u8>,
    pan_byte: Option<u8>,
    routed: Option<i8>,
    color: Option<[u8; 4]>,
    enabled: Option<bool>,
    root: Option<u32>,
    children: Vec<u16>,
    /// Punti di automazione (evento 234, solo canali di tipo 5).
    automation: Option<Vec<u8>>,
    /// Collegamento al bersaglio (evento 227 nel blocco del canale di automazione).
    remote: Option<Vec<u8>>,
}

/// Plugin esterno (VST3/CLAP) riconosciuto nei dati di un "Fruity Wrapper".
fn external_plugin(internal: &str, data: Option<&[u8]>) -> Option<PluginInstance> {
    if !internal.trim().eq_ignore_ascii_case(FL_WRAPPER) {
        return None;
    }
    let w = external_from_wrapper(data?)?;
    let mut pi = PluginInstance::new(w.plugin.kind());
    pi.opaque_state = w.state;
    pi.foreign_name = w.name;
    Some(pi)
}

/// Clip di automazione FL riconosciuta (punti + bersaglio noto).
struct FlAuto {
    iid: u16,
    name: String,
    points: FlAutomation,
    target: FlAutoTarget,
}

#[derive(Default)]
struct PatAcc {
    iid: u16,
    name: Option<String>,
    color: Option<[u8; 4]>,
    notes: Vec<FlNote>,
    has_controllers: bool,
}

#[derive(Default)]
struct SlotAcc {
    internal_name: Option<String>,
    data: Option<Vec<u8>>,
}

impl SlotAcc {
    fn is_empty(&self) -> bool {
        self.internal_name.as_deref().unwrap_or("").is_empty() && self.data.is_none()
    }
}

#[derive(Default)]
struct InsAcc {
    flags: Option<u32>,
    name: Option<String>,
    color: Option<[u8; 4]>,
    routing: Option<Vec<bool>>,
    slots: BTreeMap<u16, SlotAcc>,
    pending: SlotAcc,
    ended: bool,
}

#[derive(Default)]
struct ArrAcc {
    items: Option<Vec<u8>>,
    tracks: Vec<(Option<FlTrackData>, Option<String>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Global,
    Channel,
    Mixer,
    Arrangement,
}

fn color4(e: &RawEvent) -> Option<[u8; 4]> {
    e.as_u32().map(u32::to_le_bytes)
}

fn to_color(c: [u8; 4]) -> Color {
    Color(c[0], c[1], c[2])
}

/// Stato dell'analisi sequenziale degli eventi.
struct Collector {
    enc: TextEncoding,
    ctx: Ctx,
    channels: Vec<ChanAcc>,
    patterns: Vec<PatAcc>,
    cur_pattern: Option<usize>,
    inserts: Vec<InsAcc>,
    mixer_params: Vec<FlMixerParam>,
    arrangements: Vec<ArrAcc>,
    current_arrangement: Option<u16>,
    unmapped: BTreeMap<u8, usize>,
    // progetto
    version: Option<String>,
    title: Option<String>,
    comments: Option<String>,
    rtf_comments: Option<String>,
    url: Option<String>,
    genre: Option<String>,
    artists: Option<String>,
    tempo: Option<u32>,
    tempo_coarse: Option<u16>,
    tempo_fine: Option<u16>,
    ts_num: Option<u8>,
    ts_beat: Option<u8>,
    main_volume: Option<u8>,
    main_pitch: Option<i16>,
    swing: Option<u8>,
    current_pattern: Option<u16>,
}

impl Collector {
    fn text(&self, e: &RawEvent) -> Option<String> {
        e.as_data().map(|d| decode_text(d, self.enc))
    }

    fn channel(&mut self) -> Option<&mut ChanAcc> {
        if self.ctx == Ctx::Channel {
            self.channels.last_mut()
        } else {
            None
        }
    }

    /// Insert corrente; ne crea uno nuovo se il precedente è stato chiuso.
    fn insert(&mut self) -> &mut InsAcc {
        self.ctx = Ctx::Mixer;
        if self.inserts.last().is_none_or(|i| i.ended) {
            self.inserts.push(InsAcc::default());
        }
        let n = self.inserts.len() - 1;
        &mut self.inserts[n]
    }

    fn arrangement(&mut self) -> &mut ArrAcc {
        self.ctx = Ctx::Arrangement;
        if self.arrangements.is_empty() {
            self.arrangements.push(ArrAcc::default());
        }
        let n = self.arrangements.len() - 1;
        &mut self.arrangements[n]
    }

    fn pattern(&mut self) -> Option<&mut PatAcc> {
        self.cur_pattern.and_then(|i| self.patterns.get_mut(i))
    }

    /// Gestisce un evento; `false` se non mappato.
    fn handle(&mut self, e: &RawEvent) -> bool {
        let num = e.as_u32();
        match e.id {
            // --- progetto ---
            id::FL_VERSION => self.version = e.as_data().map(|d| decode_text(d, TextEncoding::Ascii)),
            id::FL_BUILD => {}
            id::TITLE => self.title = self.text(e),
            id::COMMENTS => self.comments = self.text(e),
            id::RTF_COMMENTS => self.rtf_comments = self.text(e),
            id::URL => self.url = self.text(e),
            id::GENRE => self.genre = self.text(e),
            id::ARTISTS => self.artists = self.text(e),
            id::TEMPO => self.tempo = num,
            id::TEMPO_COARSE => self.tempo_coarse = num.map(|v| v as u16),
            id::TEMPO_FINE => self.tempo_fine = num.map(|v| v as u16),
            id::TIME_SIG_NUM => self.ts_num = num.map(|v| v as u8),
            id::TIME_SIG_BEAT => self.ts_beat = num.map(|v| v as u8),
            id::MAIN_VOLUME => self.main_volume = num.map(|v| v as u8),
            id::MAIN_PITCH => self.main_pitch = num.map(|v| v as u16 as i16),
            id::SWING => self.swing = num.map(|v| v as u8),
            id::CURRENT_PATTERN => self.current_pattern = num.map(|v| v as u16),

            // --- canali ---
            id::CHAN_NEW => {
                self.ctx = Ctx::Channel;
                self.channels.push(ChanAcc {
                    iid: num.unwrap_or(0) as u16,
                    ..Default::default()
                });
            }
            id::CHAN_TYPE
            | id::CHAN_ROUTED_TO
            | id::CHAN_ENABLED
            | id::CHAN_SAMPLE_PATH
            | id::CHAN_NAME_OLD
            | id::CHAN_LEVELS
            | id::CHAN_VOL_WORD
            | id::CHAN_PAN_WORD
            | id::CHAN_VOL_BYTE
            | id::CHAN_PAN_BYTE
            | id::CHAN_ROOT_NOTE
            | id::CHAN_AUTOMATION
            | id::CHAN_CHILDREN => {
                let text = self.text(e);
                let Some(c) = self.channel() else { return false };
                match e.id {
                    id::CHAN_TYPE => c.ty = num.map(|v| v as u8),
                    id::CHAN_ROUTED_TO => c.routed = num.map(|v| v as u8 as i8),
                    id::CHAN_ENABLED => c.enabled = num.map(|v| v != 0),
                    id::CHAN_SAMPLE_PATH => c.sample_path = text,
                    id::CHAN_NAME_OLD => c.name_old = text,
                    id::CHAN_LEVELS => c.levels = e.as_data().map(FlLevels::parse),
                    id::CHAN_VOL_WORD => c.vol_word = num.map(|v| v as u16),
                    id::CHAN_PAN_WORD => c.pan_word = num.map(|v| v as u16),
                    id::CHAN_VOL_BYTE => c.vol_byte = num.map(|v| v as u8),
                    id::CHAN_PAN_BYTE => c.pan_byte = num.map(|v| v as u8),
                    id::CHAN_ROOT_NOTE => c.root = num,
                    id::CHAN_AUTOMATION => c.automation = e.as_data().map(<[u8]>::to_vec),
                    _ => c.children.extend(num.map(|v| v as u16)),
                }
            }
            // Collegamento dell'automazione: mappato solo dentro un canale di tipo 5.
            id::REMOTE_CONTROLLER => {
                let Some(c) = self.channel().filter(|c| c.ty == Some(5)) else {
                    return false;
                };
                c.remote = e.as_data().map(<[u8]>::to_vec);
            }

            // --- plugin (canale o slot del mixer a seconda del contesto) ---
            id::PLUGIN_INTERNAL_NAME | id::PLUGIN_NAME | id::PLUGIN_DATA | id::PLUGIN_COLOR => {
                let text = self.text(e);
                match self.ctx {
                    Ctx::Channel => {
                        let Some(c) = self.channels.last_mut() else {
                            return false;
                        };
                        match e.id {
                            id::PLUGIN_INTERNAL_NAME => c.internal_name = text,
                            id::PLUGIN_NAME => c.name = text,
                            id::PLUGIN_DATA => c.plugin_data = e.as_data().map(<[u8]>::to_vec),
                            _ => c.color = color4(e),
                        }
                    }
                    Ctx::Mixer => {
                        let ins = self.insert();
                        match e.id {
                            id::PLUGIN_INTERNAL_NAME => ins.pending.internal_name = text,
                            id::PLUGIN_DATA => ins.pending.data = e.as_data().map(<[u8]>::to_vec),
                            _ => {} // nome/colore dello slot: non rappresentati nel modello
                        }
                    }
                    _ => return false,
                }
            }

            // --- pattern ---
            id::PAT_NEW => {
                let iid = num.unwrap_or(0) as u16;
                let idx = match self.patterns.iter().position(|p| p.iid == iid) {
                    Some(i) => i,
                    None => {
                        self.patterns.push(PatAcc {
                            iid,
                            ..Default::default()
                        });
                        self.patterns.len() - 1
                    }
                };
                self.cur_pattern = Some(idx);
            }
            id::PAT_NAME => {
                let t = self.text(e);
                let Some(p) = self.pattern() else { return false };
                p.name = t;
            }
            id::PAT_COLOR => {
                let c = color4(e);
                let Some(p) = self.pattern() else { return false };
                p.color = c;
            }
            id::PAT_NOTES => {
                let notes = e.as_data().map(FlNote::parse_all).unwrap_or_default();
                let Some(p) = self.pattern() else { return false };
                p.notes.extend(notes);
            }
            id::PAT_CONTROLLERS => {
                let Some(p) = self.pattern() else { return false };
                p.has_controllers = true;
                return false;
            }

            // --- arrangement / playlist ---
            id::ARRANGEMENT_NEW => {
                self.ctx = Ctx::Arrangement;
                self.arrangements.push(ArrAcc::default());
            }
            id::ARRANGEMENT_NAME => {
                // Il nome dell'arrangement non ha corrispondente nel modello.
                self.arrangement();
            }
            id::PLAYLIST_ITEMS => self.arrangement().items = e.as_data().map(<[u8]>::to_vec),
            id::TRACK_DATA => {
                let d = e.as_data().map(FlTrackData::parse);
                self.arrangement().tracks.push((d, None));
            }
            id::TRACK_NAME => {
                let t = self.text(e);
                let a = self.arrangement();
                match a.tracks.last_mut() {
                    Some(last) if last.1.is_none() => last.1 = t,
                    _ => a.tracks.push((None, t)),
                }
            }
            id::ARRANGEMENT_CURRENT => self.current_arrangement = num.map(|v| v as u16),

            // --- mixer ---
            id::MIXER_PARAMS => {
                self.ctx = Ctx::Mixer;
                if let Some(d) = e.as_data() {
                    self.mixer_params.extend(FlMixerParam::parse_all(d));
                }
            }
            id::INSERT_FLAGS => {
                let has_flags = self.inserts.last().is_some_and(|i| !i.ended && i.flags.is_some());
                if has_flags {
                    self.insert().ended = true;
                }
                let f = e.as_data().and_then(insert_flags_parse);
                self.insert().flags = f;
            }
            id::INSERT_ROUTING => {
                let has_routing = self.inserts.last().is_some_and(|i| !i.ended && i.routing.is_some());
                if has_routing {
                    self.insert().ended = true;
                }
                let r = e.as_data().map(|d| d.iter().map(|b| *b != 0).collect());
                self.insert().routing = r;
            }
            id::INSERT_NAME => {
                let t = self.text(e);
                self.insert().name = t;
            }
            id::INSERT_COLOR => {
                let c = color4(e);
                self.insert().color = c;
            }
            id::INSERT_ICON | id::INSERT_INPUT => {
                self.insert();
            }
            id::INSERT_OUTPUT => self.insert().ended = true,
            id::SLOT_INDEX => {
                let ins = self.insert();
                let pending = std::mem::take(&mut ins.pending);
                if !pending.is_empty() {
                    ins.slots.insert(num.unwrap_or(0) as u16, pending);
                }
            }
            _ => return false,
        }
        true
    }
}

/// Converte un file FLP nel modello di progetto.
pub fn to_project(file: &FlpFile) -> Result<ImportResult, FlpError> {
    let mut warnings = Vec::new();
    let mut c = Collector {
        enc: file.text_encoding(),
        ctx: Ctx::Global,
        channels: Vec::new(),
        patterns: Vec::new(),
        cur_pattern: None,
        inserts: Vec::new(),
        mixer_params: Vec::new(),
        arrangements: Vec::new(),
        current_arrangement: None,
        unmapped: BTreeMap::new(),
        version: None,
        title: None,
        comments: None,
        rtf_comments: None,
        url: None,
        genre: None,
        artists: None,
        tempo: None,
        tempo_coarse: None,
        tempo_fine: None,
        ts_num: None,
        ts_beat: None,
        main_volume: None,
        main_pitch: None,
        swing: None,
        current_pattern: None,
    };
    for e in &file.events {
        if !c.handle(e) {
            *c.unmapped.entry(e.id).or_default() += 1;
        }
    }

    let version: Vec<u32> = c.version.as_deref().map(parse_version).unwrap_or_default();
    let ver_ge = |v: &[u32]| version.as_slice() >= v;
    if c.version.is_none() {
        warnings.push("versione di FL assente: si assume FL ≥ 12.9 e testo UTF-16".into());
    }

    let mut p = Project::new(c.title.clone().unwrap_or_default());
    p.patterns.clear();
    p.current_pattern = None;
    p.info.fl_version = c.version.clone();
    p.info.artist = c.artists.clone().unwrap_or_default();
    p.info.genre = c.genre.clone().unwrap_or_default();
    p.info.url = c.url.clone().unwrap_or_default();
    p.info.comments = match (&c.comments, &c.rtf_comments) {
        (Some(s), _) => s.clone(),
        (None, Some(rtf)) => {
            warnings.push("commenti in formato RTF importati come testo grezzo".into());
            rtf.clone()
        }
        _ => String::new(),
    };

    // Tempo e griglia.
    if let Some(t) = c.tempo {
        p.tempo = t as f64 / 1000.0;
    } else if let Some(t) = c.tempo_coarse {
        p.tempo = t as f64 + c.tempo_fine.unwrap_or(0) as f64 / 1000.0;
    }
    if !(1.0..=1000.0).contains(&p.tempo) {
        warnings.push(format!("tempo {} fuori range, uso 140", p.tempo));
        p.tempo = 140.0;
    }
    p.time.ppq = if file.header.ppq == 0 {
        warnings.push("PPQ 0 nell'header, uso 96".into());
        96
    } else {
        file.header.ppq
    };
    let ppq = p.time.ppq as Tick;
    if c.ts_num.is_some() || c.ts_beat.is_some() {
        p.time.signature = TimeSignature {
            numerator: c.ts_num.filter(|v| *v > 0).unwrap_or(4),
            denominator: c.ts_beat.filter(|v| *v > 0).unwrap_or(4),
        };
    }
    if let Some(v) = c.main_volume {
        p.master_volume = (v as f32 / 100.0).clamp(0.0, 1.25);
    }
    if let Some(v) = c.main_pitch {
        p.master_pitch = v as f32;
    }
    if let Some(v) = c.swing {
        p.swing = (v as f32 / 128.0).clamp(0.0, 1.0);
    }

    // Canali.
    let mut chan_ids: HashMap<u16, (ChannelId, ChannelKind)> = HashMap::new();
    let mut fl_autos: Vec<FlAuto> = Vec::new();
    for ch in &c.channels {
        // Canali di automazione con punti e bersaglio riconosciuti → clip di automazione.
        if ch.ty == Some(5) {
            let points = ch.automation.as_deref().and_then(FlAutomation::parse);
            let target = ch
                .remote
                .as_deref()
                .and_then(FlRemoteController::parse)
                .and_then(FlAutoTarget::from_remote);
            if let (Some(points), Some(target)) = (points, target) {
                let name = [&ch.name, &ch.name_old]
                    .into_iter()
                    .flatten()
                    .find(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("Automation {}", ch.iid as u32 + 1));
                fl_autos.push(FlAuto {
                    iid: ch.iid,
                    name,
                    points,
                    target,
                });
                continue;
            }
        }
        let cid = ChannelId(p.alloc_id());
        let internal = ch.internal_name.clone().filter(|s| !s.is_empty());
        let kind = match ch.ty {
            Some(0) | None => ChannelKind::Sampler,
            Some(2) if internal.is_none() => ChannelKind::AudioClip,
            Some(2) | Some(4) => ChannelKind::Generator,
            Some(3) => ChannelKind::Layer,
            Some(5) => ChannelKind::Automation,
            Some(t) => {
                warnings.push(format!(
                    "canale {}: tipo {t} sconosciuto, trattato come generatore",
                    ch.iid
                ));
                ChannelKind::Generator
            }
        };
        let name = [&ch.name, &ch.name_old, &internal]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| format!("Channel {}", ch.iid as u32 + 1));
        let plugin = match kind {
            ChannelKind::Sampler | ChannelKind::AudioClip => {
                let mut pi = PluginInstance::new(if kind == ChannelKind::Sampler {
                    "mfc.sampler"
                } else {
                    "mfc.audioclip"
                });
                pi.sample_path = ch.sample_path.clone().filter(|s| !s.is_empty());
                pi.opaque_state = ch.plugin_data.clone();
                pi
            }
            ChannelKind::Layer => PluginInstance::new(FOREIGN_GENERATOR),
            ChannelKind::Automation => {
                warnings.push(format!(
                    "canale '{name}': clip di automazione FL non importata (target non documentato)"
                ));
                PluginInstance::new(FOREIGN_GENERATOR)
            }
            ChannelKind::Generator => {
                let fl = internal.clone().unwrap_or_default();
                if let Some(pi) = external_plugin(&fl, ch.plugin_data.as_deref()) {
                    pi
                } else if let Some(k) = generator_kind(&fl) {
                    let mut pi = PluginInstance::new(k);
                    pi.opaque_state = ch.plugin_data.clone();
                    pi
                } else {
                    warnings.push(format!(
                        "canale '{name}': plugin '{fl}' non supportato, sostituito da {FOREIGN_GENERATOR} (stato preservato)"
                    ));
                    let mut pi = PluginInstance::new(FOREIGN_GENERATOR);
                    pi.foreign_name = Some(fl);
                    pi.opaque_state = ch.plugin_data.clone();
                    pi
                }
            }
        };
        let mut chan = Channel::new(cid, name, kind, plugin);
        let vol = ch
            .levels
            .and_then(|l| l.volume)
            .or(ch.vol_word.map(u32::from))
            .or(ch.vol_byte.map(|b| b as u32 * 100));
        if let Some(v) = vol {
            chan.volume = (v as f32 / 12800.0).clamp(0.0, 1.25);
        }
        let pan = ch
            .levels
            .and_then(|l| l.pan)
            .or(ch.pan_word.map(i32::from))
            .or(ch.pan_byte.map(|b| b as i32 * 100));
        if let Some(v) = pan {
            chan.pan = ((v as f32 - 6400.0) / 6400.0).clamp(-1.0, 1.0);
        }
        if let Some(ps) = ch.levels.and_then(|l| l.pitch_shift) {
            chan.pitch_cents = (ps as f32).clamp(-4800.0, 4800.0);
        }
        if let Some(col) = ch.color {
            chan.color = to_color(col);
        }
        chan.muted = ch.enabled == Some(false);
        if let Some(r) = ch.routed {
            if r < 0 || r as usize > MIXER_INSERTS {
                if r > 0 {
                    warnings.push(format!("canale '{}': insert {r} fuori range, uso master", chan.name));
                }
                chan.mixer_insert = 0;
            } else {
                chan.mixer_insert = r as u16;
            }
        }
        if let Some(root) = ch.root {
            chan.root_note = root.min(127) as u8;
        }
        if chan_ids.insert(ch.iid, (cid, kind)).is_some() {
            warnings.push(format!("iid canale {} duplicato", ch.iid));
        }
        p.channels.push(chan);
    }
    // Figli dei layer (risolti dopo aver creato tutti i canali).
    for (i, ch) in c.channels.iter().enumerate() {
        let kids: Vec<ChannelId> = ch
            .children
            .iter()
            .filter_map(|k| chan_ids.get(k).map(|v| v.0))
            .collect();
        if let Some(chan) = p.channels.get_mut(i) {
            chan.layer_children = kids;
        }
    }

    // Clip di automazione (risolte dopo i canali: il bersaglio può seguire la clip).
    let mut auto_ids: HashMap<u16, AutomationId> = HashMap::new();
    for fa in fl_autos {
        let ch = |iid: u16| chan_ids.get(&iid).map(|v| v.0);
        let target = match fa.target {
            FlAutoTarget::MasterVolume => Some(AutomationTarget::MasterVolume),
            FlAutoTarget::Tempo => Some(AutomationTarget::Tempo),
            FlAutoTarget::ChannelVolume(c) => ch(c).map(|channel| AutomationTarget::ChannelVolume { channel }),
            FlAutoTarget::ChannelPan(c) => ch(c).map(|channel| AutomationTarget::ChannelPan { channel }),
            FlAutoTarget::ChannelPitch(c) => ch(c).map(|channel| AutomationTarget::ChannelPitch { channel }),
            FlAutoTarget::MixerVolume(i) if i as usize <= MIXER_INSERTS => {
                Some(AutomationTarget::MixerVolume { insert: i })
            }
            FlAutoTarget::MixerPan(i) if i as usize <= MIXER_INSERTS => Some(AutomationTarget::MixerPan { insert: i }),
            _ => None,
        };
        let Some(target) = target else {
            warnings.push(format!(
                "automazione '{}': bersaglio {:?} inesistente nel progetto, non importata",
                fa.name, fa.target
            ));
            continue;
        };
        let (lo, hi) = fa.target.range();
        let mut points: Vec<AutomationPoint> = fa
            .points
            .points
            .iter()
            .map(|pt| AutomationPoint {
                position: if pt.position.is_finite() && pt.position > 0.0 {
                    pt.position.round() as Tick
                } else {
                    0
                },
                value: if pt.value.is_finite() {
                    (pt.value as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                },
                tension: if pt.tension.is_finite() {
                    pt.tension.clamp(-1.0, 1.0)
                } else {
                    0.0
                },
            })
            .collect();
        // Scarti negativi (file malformati) non devono rompere l'ordinamento.
        points.sort_by_key(|pt| pt.position);
        let aid = AutomationId(p.alloc_id());
        auto_ids.insert(fa.iid, aid);
        p.automations.push(AutomationClip {
            id: aid,
            name: fa.name,
            target,
            min: lo,
            max: hi,
            points,
        });
    }

    // Pattern e note.
    let mut pat_ids: HashMap<u16, PatternId> = HashMap::new();
    let mut lost_notes = 0usize;
    let mut big_keys = 0usize;
    let mut sorted_pats: Vec<&PatAcc> = c.patterns.iter().collect();
    sorted_pats.sort_by_key(|p| p.iid);
    for pa in sorted_pats {
        let pid = PatternId(p.alloc_id());
        pat_ids.insert(pa.iid, pid);
        let name = pa.name.clone().unwrap_or_else(|| format!("Pattern {}", pa.iid));
        let mut pat = Pattern::new(pid, name);
        if let Some(col) = pa.color {
            pat.color = to_color(col);
        }
        if pa.has_controllers {
            warnings.push(format!(
                "pattern '{}': controller (automazioni nel pattern) non importati",
                pat.name
            ));
        }
        for n in &pa.notes {
            let Some((cid, _)) = chan_ids.get(&n.rack_channel) else {
                lost_notes += 1;
                continue;
            };
            if n.key > 127 {
                big_keys += 1;
            }
            let note = Note {
                position: n.position as Tick,
                length: if n.length == 0 {
                    (ppq / 4).max(1)
                } else {
                    n.length as Tick
                },
                key: n.key.min(127) as u8,
                velocity: (n.velocity as f32 / 128.0).clamp(0.0, 1.0),
                pan: ((n.pan as f32 - 64.0) / 64.0).clamp(-1.0, 1.0),
                fine_pitch: (n.fine_pitch as f32 - 120.0).clamp(-120.0, 120.0),
                release: (n.release as f32 / 128.0).clamp(0.0, 1.0),
                slide: n.flags & NOTE_FLAG_SLIDE != 0,
                mod_x: (n.mod_x as f32 / 256.0).clamp(0.0, 1.0),
                mod_y: (n.mod_y as f32 / 256.0).clamp(0.0, 1.0),
            };
            pat.notes.entry(*cid).or_default().push(note);
        }
        pat.sort_notes();
        p.patterns.push(pat);
    }
    if lost_notes > 0 {
        warnings.push(format!("{lost_notes} note riferite a canali inesistenti ignorate"));
    }
    if big_keys > 0 {
        warnings.push(format!("{big_keys} note oltre il MIDI 127 limitate a 127"));
    }
    p.current_pattern = c.current_pattern.and_then(|i| pat_ids.get(&i).copied());
    if p.patterns.is_empty() {
        let pid = PatternId(p.alloc_id());
        p.patterns.push(Pattern::new(pid, "Pattern 1"));
    }
    if p.current_pattern.is_none() {
        p.current_pattern = p.patterns.first().map(|x| x.id);
    }

    // Arrangement / Playlist.
    if c.arrangements.len() > 1 {
        warnings.push(format!(
            "{} arrangement presenti: importato solo quello corrente",
            c.arrangements.len()
        ));
    }
    let arr_idx = c
        .current_arrangement
        .map(|i| i as usize)
        .filter(|i| *i < c.arrangements.len())
        .unwrap_or(0);
    if let Some(arr) = c.arrangements.get(arr_idx) {
        let max_idx: u16 = if ver_ge(&[12, 9, 1]) || c.version.is_none() {
            499
        } else {
            198
        };
        if let Some(d) = &arr.items {
            let preferred = if ver_ge(&[21]) { ITEM_SIZE_NEW } else { ITEM_SIZE_OLD };
            let size = if d.len() % preferred == 0 {
                preferred
            } else if d.len() % ITEM_SIZE_OLD == 0 {
                ITEM_SIZE_OLD
            } else if d.len() % ITEM_SIZE_NEW == 0 {
                ITEM_SIZE_NEW
            } else {
                warnings.push(format!("playlist: dimensione {} non multipla dei record noti", d.len()));
                preferred
            };
            let mut skipped = 0usize;
            for it in FlPlaylistItem::parse_all(d, size) {
                if it.track_rvidx > max_idx {
                    skipped += 1;
                    continue;
                }
                let track = max_idx - it.track_rvidx;
                if track as usize >= PLAYLIST_TRACKS {
                    skipped += 1;
                    continue;
                }
                let (source, offset) = if it.item_index > it.pattern_base {
                    let Some(pid) = pat_ids.get(&(it.item_index - it.pattern_base)) else {
                        skipped += 1;
                        continue;
                    };
                    let off = if it.start_offset.is_finite() && it.start_offset > 0.0 {
                        it.start_offset.round() as Tick
                    } else {
                        0
                    };
                    (ClipSource::Pattern(*pid), off)
                } else if let Some(aid) = auto_ids.get(&it.item_index) {
                    let off = if it.start_offset.is_finite() && it.start_offset > 0.0 {
                        (it.start_offset as f64 * ppq as f64).round() as Tick
                    } else {
                        0
                    };
                    (ClipSource::Automation(*aid), off)
                } else {
                    match chan_ids.get(&it.item_index) {
                        Some((cid, ChannelKind::AudioClip | ChannelKind::Sampler)) => {
                            let off = if it.start_offset.is_finite() && it.start_offset > 0.0 {
                                (it.start_offset as f64 * ppq as f64).round() as Tick
                            } else {
                                0
                            };
                            (ClipSource::Audio(*cid), off)
                        }
                        _ => {
                            skipped += 1;
                            continue;
                        }
                    }
                };
                let cid = ClipId(p.alloc_id());
                p.playlist.clips.push(PlaylistClip {
                    id: cid,
                    track,
                    position: it.position as Tick,
                    length: it.length as Tick,
                    offset,
                    source,
                    muted: it.flags & ITEM_FLAG_MUTED != 0,
                });
            }
            if skipped > 0 {
                warnings.push(format!(
                    "{skipped} elementi di playlist non importati (automazioni o riferimenti non validi)"
                ));
            }
        }
        for (i, (data, name)) in arr.tracks.iter().enumerate() {
            if i >= PLAYLIST_TRACKS {
                break;
            }
            let name = name.clone().unwrap_or_default();
            let muted = data.and_then(|d| d.enabled) == Some(false);
            if name.is_empty() && !muted {
                continue;
            }
            let mut t = PlaylistTrack {
                name,
                muted,
                ..Default::default()
            };
            if let Some(col) = data.and_then(|d| d.color) {
                t.color = to_color(col);
            }
            p.playlist.tracks.insert(i as u16, t);
        }
    }

    // Mixer.
    let mut slot_pos: HashMap<(usize, u16), usize> = HashMap::new();
    for (i, ins) in c.inserts.iter().enumerate() {
        if i > MIXER_INSERTS {
            if !ins.slots.is_empty() {
                warnings.push(format!("insert {i} oltre il limite del modello ignorato"));
            }
            continue;
        }
        let Some(mi) = p.mixer.inserts.get_mut(i) else { continue };
        if let Some(n) = ins.name.as_ref().filter(|n| !n.is_empty()) {
            mi.name = n.clone();
        }
        if let Some(col) = ins.color {
            mi.color = to_color(col);
        }
        if let Some(f) = ins.flags {
            mi.muted = f & insert_flag::ENABLED == 0;
            mi.solo = f & insert_flag::SOLO != 0;
            mi.phase_invert = f & insert_flag::POLARITY_REVERSED != 0;
            mi.swap_lr = f & insert_flag::SWAP_LEFT_RIGHT != 0;
        }
        if let Some(r) = &ins.routing {
            mi.routes = r
                .iter()
                .enumerate()
                .filter(|(j, b)| **b && *j != i && *j <= MIXER_INSERTS)
                .map(|(j, _)| Route {
                    target: j as u16,
                    gain: 1.0,
                })
                .collect();
        }
        for (slot, s) in &ins.slots {
            let fl = s.internal_name.clone().unwrap_or_default();
            if let Some(pi) = external_plugin(&fl, s.data.as_deref()) {
                slot_pos.insert((i, *slot), mi.effects.len());
                mi.effects.push(EffectSlot::new(pi));
                continue;
            }
            let slot_obj = match effect_kind(&fl) {
                Some(k) => {
                    let mut pi = PluginInstance::new(k);
                    pi.opaque_state = s.data.clone();
                    EffectSlot::new(pi)
                }
                None => {
                    warnings.push(format!(
                        "insert {i} slot {slot}: effetto '{fl}' non supportato, slot bypassato (stato preservato)"
                    ));
                    let mut pi = PluginInstance::new(FOREIGN_EFFECT);
                    pi.foreign_name = Some(fl);
                    pi.opaque_state = s.data.clone();
                    let mut es = EffectSlot::new(pi);
                    es.enabled = false;
                    es
                }
            };
            slot_pos.insert((i, *slot), mi.effects.len());
            mi.effects.push(slot_obj);
        }
    }
    for prm in &c.mixer_params {
        let i = prm.insert as usize;
        let Some(mi) = p.mixer.inserts.get_mut(i) else { continue };
        let m = prm.msg as f32;
        match prm.id {
            mixer_param::VOLUME => mi.volume = (m / 16000.0).clamp(0.0, 1.25),
            mixer_param::PAN => mi.pan = (m / 6400.0).clamp(-1.0, 1.0),
            mixer_param::STEREO_SEPARATION => mi.stereo_separation = (-m / 64.0).clamp(-1.0, 1.0),
            mixer_param::SLOT_ENABLED | mixer_param::SLOT_MIX => {
                if let Some(&k) = slot_pos.get(&(i, prm.slot)) {
                    let es = &mut mi.effects[k];
                    if prm.id == mixer_param::SLOT_ENABLED {
                        // Gli effetti non supportati restano bypassati.
                        if es.plugin.kind != FOREIGN_EFFECT {
                            es.enabled = prm.msg != 0;
                        }
                    } else {
                        es.mix = ((m + 6400.0) / 12800.0).clamp(0.0, 1.0);
                    }
                }
            }
            id if (mixer_param::ROUTE_VOL_START..mixer_param::VOLUME).contains(&id) => {
                let target = (id - mixer_param::ROUTE_VOL_START) as u16;
                if let Some(r) = mi.routes.iter_mut().find(|r| r.target == target) {
                    r.gain = (m / 12800.0).max(0.0);
                }
            }
            _ => {}
        }
    }

    // Eventi non mappati (restano nel FlpFile).
    if !c.unmapped.is_empty() {
        let list: Vec<String> = c
            .unmapped
            .iter()
            .map(|(id, n)| match event_name(*id) {
                Some(name) => format!("{name}({id})×{n}"),
                None => format!("{id}×{n}"),
            })
            .collect();
        warnings.push(format!(
            "eventi non mappati nel modello (conservati solo a livello eventi): {}",
            list.join(", ")
        ));
    }

    p.playlist
        .clips
        .sort_by(|a, b| a.track.cmp(&b.track).then(a.position.cmp(&b.position)));
    Ok(ImportResult { project: p, warnings })
}
