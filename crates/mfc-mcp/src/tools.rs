//! Catalogo dei tool MCP. Ogni tool è un wrapper sottile su [`mfc_core::Command`]
//! o sui servizi (render, transport, import/export).

use std::collections::BTreeMap;

use mfc_core::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::session::Session;

pub type Handler = fn(&mut Session, &Value) -> Result<Value, String>;

/// Definizione di un tool.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    pub handler: Handler,
}

fn schema<T: JsonSchema>() -> Value {
    let mut v = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({"type": "object"}));
    if let Some(o) = v.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
        o.entry("type").or_insert(json!("object"));
        o.entry("properties").or_insert(json!({}));
    }
    v
}

fn parse<T: for<'de> Deserialize<'de>>(v: &Value) -> Result<T, String> {
    let v = if v.is_null() { json!({}) } else { v.clone() };
    serde_json::from_value(v).map_err(|e| format!("argomenti non validi: {e}"))
}

macro_rules! tool {
    ($name:literal, $desc:literal, $args:ty, $f:expr) => {
        Tool {
            name: $name,
            description: $desc,
            schema: schema::<$args>,
            handler: |s: &mut Session, v: &Value| -> Result<Value, String> {
                let a: $args = parse(v)?;
                let f: fn(&mut Session, $args) -> Result<Value, String> = $f;
                f(s, a)
            },
        }
    };
}

// ---------------------------------------------------------------- utilità

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Nome nota stile FL Studio (C5 = 60).
pub fn key_name(key: u8) -> String {
    format!("{}{}", NOTE_NAMES[(key % 12) as usize], key / 12)
}

/// Interpreta una nota: numero MIDI o nome (`C5`, `F#4`, `Bb3`; C5 = 60 come in FL Studio).
pub fn parse_key(v: &Value) -> Result<u8, String> {
    if let Some(n) = v.as_u64() {
        return u8::try_from(n)
            .ok()
            .filter(|k| *k <= 127)
            .ok_or_else(|| format!("nota {n} fuori range 0..=127"));
    }
    let s = v.as_str().ok_or("nota: numero o nome (es. \"C5\")")?.trim();
    let mut chars = s.chars();
    let letter = chars.next().ok_or("nota vuota")?.to_ascii_uppercase();
    let base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(format!("nota non valida: {s}")),
    };
    let rest: String = chars.collect();
    let (acc, oct) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest.as_str()),
    };
    let octave: i32 = oct.parse().map_err(|_| format!("ottava non valida in {s}"))?;
    let key = octave * 12 + base + acc;
    u8::try_from(key)
        .ok()
        .filter(|k| *k <= 127)
        .ok_or_else(|| format!("nota {s} fuori range"))
}

fn outcome_json(o: &Outcome) -> Value {
    serde_json::to_value(o).unwrap_or(Value::Null)
}

fn current_pattern(s: &Session, p: Option<u32>) -> Result<PatternId, String> {
    match p {
        Some(id) => Ok(PatternId(id)),
        None => s
            .project
            .current_pattern
            .or(s.project.patterns.first().map(|p| p.id))
            .ok_or("nessun pattern".into()),
    }
}

fn plugin_instance(
    kind: &str,
    params: Option<BTreeMap<String, f32>>,
    sample: Option<String>,
) -> Result<PluginInstance, String> {
    let mut inst =
        mfc_plugins::default_instance(kind).ok_or_else(|| format!("plugin sconosciuto: {kind} (vedi plugin_list)"))?;
    let desc = mfc_plugins::descriptor(kind).unwrap();
    for (k, v) in params.unwrap_or_default() {
        let p = desc
            .param(&k)
            .ok_or_else(|| format!("{kind}: parametro sconosciuto '{k}'"))?;
        inst.params.insert(k, p.clamp(v));
    }
    inst.sample_path = sample;
    Ok(inst)
}

fn channel_json(p: &Project, c: &Channel) -> Value {
    let notes: usize = p
        .patterns
        .iter()
        .filter_map(|pt| pt.notes.get(&c.id))
        .map(Vec::len)
        .sum();
    json!({
        "id": c.id, "name": c.name, "kind": c.kind, "plugin": c.plugin.kind,
        "foreign_plugin": c.plugin.foreign_name, "sample": c.plugin.sample_path,
        "volume": c.volume, "volume_db": fader_db(c.volume, CHANNEL_UNITY), "gain_db": db(c.gain()), "pan": c.pan, "pitch_cents": c.pitch_cents,
        "muted": c.muted, "insert": c.mixer_insert, "root_note": key_name(c.root_note), "notes_total": notes,
    })
}

fn db(g: f32) -> f32 {
    if g <= 1e-9 {
        -120.0
    } else {
        (20.0 * g.log10() * 10.0).round() / 10.0
    }
}

/// Livello (dB) di un fader dato posizione e posizione di 0 dB.
fn fader_db(pos: f32, unity: f32) -> f32 {
    db(fader_gain(pos, unity))
}

/// Posizione massima dei fader.
const FADER_MAX: f32 = 1.25;

/// Posizione di un fader da un livello in dB (≤ -120 dB = fader a zero).
fn fader_from_db(level_db: f32, unity: f32) -> Result<f32, String> {
    if level_db.is_nan() || level_db == f32::INFINITY {
        return Err(format!("volume_db non valido: {level_db}"));
    }
    if level_db <= -120.0 {
        return Ok(0.0);
    }
    let pos = unity * 10f32.powf(level_db / 20.0);
    let max_db = 20.0 * (FADER_MAX / unity).log10();
    if pos > FADER_MAX * 1.005 {
        return Err(format!(
            "volume_db {level_db} oltre il massimo del fader ({max_db:+.1} dB)"
        ));
    }
    Ok(pos.min(FADER_MAX))
}

/// Volume da `volume` (posizione) oppure `volume_db`; non entrambi.
fn volume_arg(volume: Option<f32>, volume_db: Option<f32>, unity: f32, what: &str) -> Result<Option<f32>, String> {
    match (volume, volume_db) {
        (Some(_), Some(_)) => Err(format!("specificare {what} oppure {what}_db, non entrambi")),
        (Some(v), None) => Ok(Some(v)),
        (None, Some(d)) => fader_from_db(d, unity).map(Some),
        (None, None) => Ok(None),
    }
}

/// Id creati da un esito (in ordine).
fn outcome_ids(o: &Outcome, out: &mut Vec<u32>) {
    match o {
        Outcome::Channel(id) => out.push(id.0),
        Outcome::Pattern(id) => out.push(id.0),
        Outcome::Clip(id) => out.push(id.0),
        Outcome::Automation(id) => out.push(id.0),
        Outcome::Many(v) => v.iter().for_each(|x| outcome_ids(x, out)),
        Outcome::None | Outcome::Count(_) => {}
    }
}

/// Verifica che un effetto dell'insert `insert` possa usare `value` come `sidechain_insert`:
/// -1 = interno; altrimenti un insert diverso dal master e da se stesso, senza creare cicli.
fn check_sidechain(p: &Project, insert: u16, value: f32) -> Result<(), String> {
    let src = value.round();
    if src < 0.0 {
        return Ok(());
    }
    if src < 1.0 {
        return Err(
            "sidechain_insert: il master (0) non può fare da chiave; usare -1 per il rilevamento interno".into(),
        );
    }
    let src = src as usize;
    if src >= p.mixer.inserts.len() {
        return Err(format!("sidechain_insert: insert {src} inesistente"));
    }
    if src == insert as usize {
        return Err("sidechain_insert: un insert non può fare da chiave a se stesso (usare -1)".into());
    }
    // La sorgente deve essere elaborata prima: non deve trovarsi a valle dell'insert.
    let sc_of = |e: &EffectSlot| e.plugin.param("sidechain_insert").map(f32::round).filter(|v| *v >= 1.0);
    let mut seen = vec![false; p.mixer.inserts.len()];
    let mut stack = vec![insert as usize];
    while let Some(i) = stack.pop() {
        if i == src {
            return Err(format!(
                "sidechain_insert: l'insert {src} riceve già il segnale dell'insert {insert} (ciclo)"
            ));
        }
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        stack.extend(
            p.mixer.inserts[i]
                .routes
                .iter()
                .map(|r| r.target as usize)
                .filter(|t| *t != MASTER as usize),
        );
        for (j, ins) in p.mixer.inserts.iter().enumerate() {
            if ins.effects.iter().any(|e| sc_of(e) == Some(i as f32)) {
                stack.push(j);
            }
        }
    }
    Ok(())
}

fn insert_json(index: usize, ins: &MixerInsert) -> Value {
    json!({
        "insert": index, "name": ins.name, "volume": ins.volume, "volume_db": fader_db(ins.volume, MIXER_UNITY),
        "gain_db": db(ins.gain()), "pan": ins.pan,
        "muted": ins.muted, "solo": ins.solo, "stereo_separation": ins.stereo_separation,
        "routes": ins.routes.iter().map(|r| json!({"to": r.target, "gain": r.gain})).collect::<Vec<_>>(),
        "effects": ins.effects.iter().enumerate().map(|(k, e)| effect_json(k, e)).collect::<Vec<_>>(),
    })
}

fn effect_json(slot: usize, e: &EffectSlot) -> Value {
    json!({
        "slot": slot, "plugin": e.plugin.kind, "foreign": e.plugin.foreign_name, "enabled": e.enabled, "mix": e.mix, "params": e.plugin.params,
    })
}

fn ticks_from(s: &Session, ticks: Option<u64>, bars: Option<f64>) -> Result<Tick, String> {
    match (ticks, bars) {
        (Some(t), _) => Ok(t),
        (None, Some(b)) if b >= 0.0 => Ok(s.project.time.bars_to_ticks(b)),
        (None, Some(_)) => Err("bars negativo".into()),
        (None, None) => Ok(0),
    }
}

// ---------------------------------------------------------------- argomenti

#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

#[derive(Deserialize, JsonSchema)]
struct ProjectNew {
    /// Titolo del progetto.
    title: Option<String>,
    /// Tempo in BPM (default 140).
    tempo: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
struct PathArg {
    /// Percorso del file (relativo alla cartella di lavoro del server o assoluto).
    path: String,
}

#[derive(Deserialize, JsonSchema)]
struct OptPath {
    /// Percorso; se omesso usa quello del progetto aperto. Estensione .mfc (nativo JSON) o .flp.
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ProjectSet {
    title: Option<String>,
    artist: Option<String>,
    genre: Option<String>,
    comments: Option<String>,
    /// BPM 10..=999.
    tempo: Option<f64>,
    /// Metrica, es. 3 per 3/4.
    numerator: Option<u8>,
    denominator: Option<u8>,
    /// Fader master 0..=1.25 (0.8 = 0 dB, lineare in ampiezza). Alternativa: `master_volume_db`.
    master_volume: Option<f32>,
    /// Volume master in dB (0 = unità, massimo +3.9; ≤ -120 = silenzio).
    master_volume_db: Option<f32>,
    /// Swing globale 0..=1.
    swing: Option<f32>,
}

#[derive(Deserialize, JsonSchema)]
struct ApplyCommands {
    /// Comandi grezzi di mfc-core (campo "op" in snake_case, es. {"op":"set_tempo","bpm":128}).
    /// Eseguiti atomicamente come un unico passo di undo.
    commands: Vec<Value>,
}

#[derive(Deserialize, JsonSchema)]
struct PluginList {
    /// Filtra per ruolo: "generator" o "effect".
    role: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ChannelAdd {
    name: String,
    /// Id plugin generatore (es. "mfc.3xosc", "mfc.kick", "mfc.sampler"). Vedi plugin_list.
    plugin: String,
    /// Insert del mixer di destinazione (0 = master).
    insert: Option<u16>,
    /// File WAV per sampler/audio clip.
    sample_path: Option<String>,
    /// Parametri del plugin (valori reali).
    params: Option<BTreeMap<String, f32>>,
}

#[derive(Deserialize, JsonSchema)]
struct ChannelRef {
    channel: u32,
}

#[derive(Deserialize, JsonSchema)]
struct ChannelSet {
    channel: u32,
    name: Option<String>,
    /// Fader 0..=1.25 (0.78 = 0 dB, lineare in ampiezza). Alternativa: `volume_db`.
    volume: Option<f32>,
    /// Volume in dB (0 = unità, massimo +4.1; ≤ -120 = silenzio).
    volume_db: Option<f32>,
    /// -1..=1
    pan: Option<f32>,
    pitch_cents: Option<f32>,
    muted: Option<bool>,
    insert: Option<u16>,
    /// Nota radice (numero o nome, es. "C5").
    root_note: Option<Value>,
    /// Parametri del plugin da impostare.
    params: Option<BTreeMap<String, f32>>,
    /// File campione (sampler).
    sample_path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct NameArg {
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct PatternRef {
    pattern: u32,
}

#[derive(Deserialize, JsonSchema)]
struct PatternSet {
    pattern: u32,
    name: Option<String>,
    /// Lunghezza in battute (0 = automatica).
    length_bars: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
struct StepsSet {
    /// Pattern (default: quello selezionato).
    pattern: Option<u32>,
    channel: u32,
    /// Una lettera per step da 1/16: 'x' = on, 'X' = accento, 'o' = soft, '.' o '-' = off. '|' ignorato.
    row: String,
}

#[derive(Deserialize, JsonSchema)]
struct NoteArg {
    /// Posizione in tick (alternativa a `step`).
    position: Option<u64>,
    /// Posizione in step da 1/16 (alternativa a `position`).
    step: Option<f64>,
    /// Durata in tick (default 1 step).
    length: Option<u64>,
    /// Numero MIDI o nome (C5 = 60).
    key: Value,
    /// 0..=1 (default 0.78)
    velocity: Option<f32>,
    pan: Option<f32>,
    slide: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct NotesAdd {
    pattern: Option<u32>,
    channel: u32,
    notes: Vec<NoteArg>,
    /// Se true, svuota prima le note del canale nel pattern.
    replace: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct NotesList {
    pattern: Option<u32>,
    channel: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct NotesRemove {
    pattern: Option<u32>,
    channel: u32,
    from: Option<u64>,
    to: Option<u64>,
    key: Option<Value>,
}

#[derive(Deserialize, JsonSchema)]
struct NotesQuantize {
    pattern: Option<u32>,
    channel: Option<u32>,
    /// Griglia in tick (24 = 1/16 a PPQ 96).
    grid: u64,
    quantize_length: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct NotesTranspose {
    pattern: Option<u32>,
    channel: Option<u32>,
    semitones: i32,
}

#[derive(Deserialize, JsonSchema)]
struct PlaylistPlace {
    /// Traccia 0..=499.
    track: u16,
    /// Posizione in tick (alternativa a `bar`).
    position: Option<u64>,
    /// Posizione in battute, 0-based (alternativa a `position`).
    bar: Option<f64>,
    /// Durata in tick (default: lunghezza della sorgente).
    length: Option<u64>,
    /// Durata in battute.
    length_bars: Option<f64>,
    pattern: Option<u32>,
    audio_channel: Option<u32>,
    automation: Option<u32>,
    /// Ripete la clip N volte consecutive (default 1).
    repeat: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct ClipRef {
    clip: u32,
}

#[derive(Deserialize, JsonSchema)]
struct ClipMove {
    clip: u32,
    track: Option<u16>,
    position: Option<u64>,
    length: Option<u64>,
    muted: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct TrackSet {
    track: u16,
    name: Option<String>,
    muted: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct MixerList {
    /// Mostra anche gli insert inutilizzati (default false).
    all: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct MixerSet {
    insert: u16,
    name: Option<String>,
    /// Fader 0..=1.25 (0.8 = 0 dB, lineare in ampiezza). Alternativa: `volume_db`.
    volume: Option<f32>,
    /// Volume in dB (0 = unità, massimo +3.9; ≤ -120 = silenzio).
    volume_db: Option<f32>,
    pan: Option<f32>,
    stereo_separation: Option<f32>,
    muted: Option<bool>,
    solo: Option<bool>,
    phase_invert: Option<bool>,
    swap_lr: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct MixerRoute {
    from: u16,
    to: u16,
    /// Guadagno della mandata 0..=2; null/assente con remove=true per eliminarla.
    gain: Option<f32>,
    remove: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct EffectAdd {
    insert: u16,
    /// Id plugin effetto (es. "mfc.reverb"). Vedi plugin_list.
    plugin: String,
    params: Option<BTreeMap<String, f32>>,
    slot: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
struct EffectRef {
    insert: u16,
    slot: u16,
}

#[derive(Deserialize, JsonSchema)]
struct EffectSet {
    insert: u16,
    slot: u16,
    enabled: Option<bool>,
    /// Dry/wet 0..=1.
    mix: Option<f32>,
    params: Option<BTreeMap<String, f32>>,
}

#[derive(Deserialize, JsonSchema)]
struct PointArg {
    position: u64,
    /// 0..=1 (mappato su min..max)
    value: f32,
    tension: Option<f32>,
}

#[derive(Deserialize, JsonSchema)]
struct AutomationAdd {
    name: String,
    /// Bersaglio, es. {"type":"master_volume"}, {"type":"tempo"}, {"type":"channel_volume","channel":3},
    /// {"type":"channel_param","channel":3,"param":"cutoff"}, {"type":"mixer_volume","insert":1},
    /// {"type":"effect_param","insert":1,"slot":0,"param":"wet"}.
    target: Value,
    /// Valore reale per value=0.
    min: f32,
    /// Valore reale per value=1.
    max: f32,
    points: Vec<PointArg>,
    /// Se presente, piazza la clip in Playlist su questa traccia.
    track: Option<u16>,
    /// Posizione della clip in tick.
    position: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
struct RenderArgs {
    /// File WAV di uscita.
    path: String,
    /// Renderizza solo questo pattern (default: brano intero).
    pattern: Option<u32>,
    /// Ripetizioni del pattern.
    loops: Option<u32>,
    sample_rate: Option<u32>,
    tail_seconds: Option<f64>,
    normalize: Option<bool>,
    /// Esporta anche gli stem per insert.
    stems: Option<bool>,
    /// "int16", "int24" (default) o "float32".
    format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct TransportArgs {
    /// "play", "stop", "pause", "seek", "status".
    action: String,
    /// Per seek: posizione in tick.
    position: Option<f64>,
    /// "song" o "pattern".
    mode: Option<String>,
    pattern: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct PreviewArgs {
    channel: u32,
    key: Value,
    velocity: Option<f32>,
    /// Durata (ms) prima del note-off (default 300).
    duration_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
struct MidiImportArgs {
    path: String,
    pattern_name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct MidiExportArgs {
    path: String,
    /// Pattern da esportare; se omesso esporta l'intero brano.
    pattern: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct FlpInspectArgs {
    path: String,
    offset: Option<usize>,
    /// Numero massimo di eventi (default 200).
    limit: Option<usize>,
    /// Filtra per id evento.
    id: Option<u8>,
}

// ---------------------------------------------------------------- catalogo

pub fn all() -> Vec<Tool> {
    let mut v = vec![
        // ---- Progetto
        tool!("project_new", "Crea un nuovo progetto vuoto (un pattern, mixer con 125 insert).", ProjectNew, |s, a| {
            let mut p = Project::new(a.title.unwrap_or_else(|| "Untitled".into()));
            if let Some(t) = a.tempo {
                Command::SetTempo { bpm: t }.apply(&mut p).map_err(|e| e.to_string())?;
            }
            s.replace_project(p, None);
            project_info(s)
        }),
        tool!("project_open", "Apre un progetto: .mfc (nativo), .flp (FL Studio) o .mid (MIDI → nuovo progetto).", PathArg, |s, a| {
            let path = s.resolve(&a.path);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let mut warnings = Vec::new();
            let project = match ext.as_str() {
                "flp" => {
                    let r = crate::flp::import(&path)?;
                    warnings = r.1;
                    r.0
                }
                "mid" | "midi" => {
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    let mut p = Project::new(path.file_stem().and_then(|x| x.to_str()).unwrap_or("MIDI"));
                    let imp = mfc_midi::parse_midi(&bytes, p.time.ppq).map_err(|e| e.to_string())?;
                    let (pat, _) = mfc_midi::import_into_project(&mut p, &imp, "MIDI").map_err(|e| e.to_string())?;
                    let len = p.pattern(pat).map(|x| x.effective_length(&p.time));
                    Command::PlaceClip { track: 0, position: 0, length: len, source: ClipSource::Pattern(pat) }
                        .apply(&mut p)
                        .map_err(|e| e.to_string())?;
                    p
                }
                _ => {
                    let txt = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    Project::from_json(&txt).map_err(|e| format!("progetto .mfc non valido: {e}"))?
                }
            };
            let save_path = (ext == "mfc").then(|| path.clone());
            s.replace_project(project, save_path);
            let mut info = project_info(s)?;
            info["warnings"] = json!(warnings);
            Ok(info)
        }),
        tool!("project_save", "Salva il progetto (.mfc nativo, oppure .flp per FL Studio).", OptPath, |s, a| {
            let path = match a.path {
                Some(p) => s.resolve(&p),
                None => s.path.clone().ok_or("nessun percorso: specificare 'path'")?,
            };
            if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("flp")) {
                crate::flp::export(&path, &s.project)?;
            } else {
                let txt = s.project.to_json().map_err(|e| e.to_string())?;
                std::fs::write(&path, txt).map_err(|e| format!("{}: {e}", path.display()))?;
                s.path = Some(path.clone());
            }
            Ok(json!({"saved": path.display().to_string()}))
        }),
        tool!("project_info", "Riepilogo del progetto: tempo, metrica, canali, pattern, durata, insert usati.", NoArgs, |s, _| {
            project_info(s)
        }),
        tool!("project_set", "Modifica metadati, tempo, metrica, volume master e swing.", ProjectSet, |s, a| {
            let mut cmds = Vec::new();
            if a.title.is_some() || a.artist.is_some() || a.genre.is_some() || a.comments.is_some() {
                cmds.push(Command::SetInfo { title: a.title, artist: a.artist, genre: a.genre, comments: a.comments });
            }
            if let Some(bpm) = a.tempo {
                cmds.push(Command::SetTempo { bpm });
            }
            if a.numerator.is_some() || a.denominator.is_some() {
                let sig = s.project.time.signature;
                cmds.push(Command::SetTimeSignature {
                    numerator: a.numerator.unwrap_or(sig.numerator),
                    denominator: a.denominator.unwrap_or(sig.denominator),
                });
            }
            if let Some(volume) = volume_arg(a.master_volume, a.master_volume_db, MIXER_UNITY, "master_volume")? {
                cmds.push(Command::SetMasterVolume { volume });
            }
            if let Some(swing) = a.swing {
                cmds.push(Command::SetSwing { swing });
            }
            s.exec(Command::Batch { commands: cmds })?;
            project_info(s)
        }),
        tool!("undo", "Annulla l'ultima modifica.", NoArgs, |s, _| Ok(json!({"undone": s.undo()}))),
        tool!("redo", "Ripristina l'ultima modifica annullata.", NoArgs, |s, _| Ok(json!({"redone": s.redo()}))),
        tool!("history", "Elenco delle modifiche annullabili (più recente prima).", NoArgs, |s, _| {
            Ok(json!({"undo": s.history.undo_labels().take(50).collect::<Vec<_>>(), "can_redo": s.history.can_redo()}))
        }),
        tool!(
            "apply_commands",
            "Esegue comandi grezzi del modello (avanzato, atomico). Operazioni: set_tempo, add_channel, set_channel, add_notes, \
             remove_notes, set_step_row, place_clip, move_clip, set_insert, set_route, add_effect, set_effect_param, \
             add_automation, batch, ... (stessi nomi di mfc_core::Command in snake_case).",
            ApplyCommands,
            |s, a| {
                let commands = a
                    .commands
                    .into_iter()
                    .map(|c| serde_json::from_value::<Command>(c).map_err(|e| format!("comando non valido: {e}")))
                    .collect::<Result<Vec<_>, _>>()?;
                let out = s.exec(Command::Batch { commands })?;
                let mut ids = Vec::new();
                outcome_ids(&out, &mut ids);
                Ok(json!({"ids": ids, "outcome": outcome_json(&out)}))
            }
        ),
        // ---- Plugin e Channel Rack
        tool!("plugin_list", "Elenca generatori ed effetti nativi con i loro parametri (nome, range, default, unità).", PluginList, |_, a| {
            let role = a.role.as_deref();
            let list: Vec<Value> = mfc_plugins::registry()
                .iter()
                .filter(|d| match role {
                    Some("generator") => d.role == mfc_plugins::PluginRole::Generator,
                    Some("effect") => d.role == mfc_plugins::PluginRole::Effect,
                    _ => true,
                })
                .map(|d| {
                    json!({
                        "id": d.id, "name": d.name, "description": d.description,
                        "role": if d.role == mfc_plugins::PluginRole::Generator { "generator" } else { "effect" },
                        "params": d.params.iter().map(|p| {
                            let mut v = json!({"name": p.name, "label": p.label, "min": p.min, "max": p.max, "default": p.default, "unit": p.unit});
                            if !p.choices.is_empty() { v["choices"] = json!(p.choices); }
                            v
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({"plugins": list}))
        }),
        tool!("channel_add", "Aggiunge un canale al Channel Rack con un generatore.", ChannelAdd, |s, a| {
            let inst = plugin_instance(&a.plugin, a.params, a.sample_path)?;
            let desc = mfc_plugins::descriptor(&a.plugin).unwrap();
            if desc.role != mfc_plugins::PluginRole::Generator {
                return Err(format!("{} è un effetto: usare mixer_add_effect", a.plugin));
            }
            let kind = match a.plugin.as_str() {
                mfc_plugins::ids::SAMPLER => ChannelKind::Sampler,
                mfc_plugins::ids::AUDIO_CLIP => ChannelKind::AudioClip,
                _ => ChannelKind::Generator,
            };
            let out = s.exec(Command::AddChannel { name: a.name, kind, plugin: inst, mixer_insert: a.insert })?;
            let Outcome::Channel(id) = out else { unreachable!() };
            Ok(channel_json(&s.project, s.project.channel(id).unwrap()))
        }),
        tool!("channel_list", "Elenca i canali del Channel Rack.", NoArgs, |s, _| {
            Ok(json!({"channels": s.project.channels.iter().map(|c| channel_json(&s.project, c)).collect::<Vec<_>>()}))
        }),
        tool!("channel_set", "Modifica un canale: nome, volume, pan, pitch, mute, insert, nota radice, parametri, campione.", ChannelSet, |s, a| {
            let ch = ChannelId(a.channel);
            let kind = s.project.channel(ch).ok_or_else(|| format!("canale {ch} inesistente"))?.plugin.kind.clone();
            let volume = volume_arg(a.volume, a.volume_db, CHANNEL_UNITY, "volume")?;
            let mut cmds = vec![Command::SetChannel {
                channel: ch,
                name: a.name,
                volume,
                pan: a.pan,
                pitch_cents: a.pitch_cents,
                muted: a.muted,
                mixer_insert: a.insert,
                root_note: a.root_note.as_ref().map(parse_key).transpose()?,
                color: None,
            }];
            let desc = mfc_plugins::descriptor(&kind);
            for (param, value) in a.params.unwrap_or_default() {
                let value = match desc.and_then(|d| d.param(&param)) {
                    Some(p) => p.clamp(value),
                    None if desc.is_some() => return Err(format!("{kind}: parametro sconosciuto '{param}'")),
                    None => value,
                };
                cmds.push(Command::SetChannelParam { channel: ch, param, value });
            }
            if let Some(path) = a.sample_path {
                cmds.push(Command::SetChannelSample { channel: ch, path: Some(path) });
            }
            s.exec(Command::Batch { commands: cmds })?;
            Ok(channel_json(&s.project, s.project.channel(ch).unwrap()))
        }),
        tool!("channel_remove", "Rimuove un canale (e le sue note/clip).", ChannelRef, |s, a| {
            s.exec(Command::RemoveChannel { channel: ChannelId(a.channel) })?;
            Ok(json!({"removed": a.channel}))
        }),
        tool!("channel_clone", "Duplica un canale.", ChannelRef, |s, a| {
            let Outcome::Channel(id) = s.exec(Command::CloneChannel { channel: ChannelId(a.channel) })? else { unreachable!() };
            Ok(channel_json(&s.project, s.project.channel(id).unwrap()))
        }),
        // ---- Pattern
        tool!("pattern_add", "Crea un pattern vuoto e lo seleziona.", NameArg, |s, a| {
            let Outcome::Pattern(id) = s.exec(Command::AddPattern { name: a.name })? else { unreachable!() };
            s.exec(Command::SelectPattern { pattern: id })?;
            Ok(pattern_json(s, id))
        }),
        tool!("pattern_list", "Elenca i pattern con lunghezza e numero di note per canale.", NoArgs, |s, _| {
            Ok(json!({"current": s.project.current_pattern, "patterns": s.project.patterns.iter().map(|p| pattern_json(s, p.id)).collect::<Vec<_>>()}))
        }),
        tool!("pattern_select", "Seleziona il pattern corrente.", PatternRef, |s, a| {
            s.exec(Command::SelectPattern { pattern: PatternId(a.pattern) })?;
            Ok(json!({"current": a.pattern}))
        }),
        tool!("pattern_set", "Rinomina un pattern o ne imposta la lunghezza in battute.", PatternSet, |s, a| {
            let length = a.length_bars.map(|b| if b <= 0.0 { None } else { Some(s.project.time.bars_to_ticks(b)) });
            s.exec(Command::SetPattern { pattern: PatternId(a.pattern), name: a.name, length, color: None })?;
            Ok(json!({"ok": true}))
        }),
        tool!("pattern_clone", "Duplica un pattern.", PatternRef, |s, a| {
            let Outcome::Pattern(id) = s.exec(Command::ClonePattern { pattern: PatternId(a.pattern) })? else { unreachable!() };
            Ok(pattern_json(s, id))
        }),
        tool!("pattern_remove", "Elimina un pattern (e le sue clip in Playlist).", PatternRef, |s, a| {
            s.exec(Command::RemovePattern { pattern: PatternId(a.pattern) })?;
            Ok(json!({"removed": a.pattern}))
        }),
        tool!("steps_set", "Step sequencer: imposta una riga di step da 1/16, es. \"x...x...x...x...\".", StepsSet, |s, a| {
            let pattern = current_pattern(s, a.pattern)?;
            s.exec(Command::SetStepRow { pattern, channel: ChannelId(a.channel), row: a.row })?;
            Ok(json!({"pattern": pattern, "channel": a.channel}))
        }),
        tool!("notes_add", "Piano roll: aggiunge note (posizione in tick o step, durata in tick, nota numero o nome C5=60).", NotesAdd, |s, a| {
            let pattern = current_pattern(s, a.pattern)?;
            let step = s.project.time.ticks_per_step();
            let notes = a
                .notes
                .iter()
                .map(|n| {
                    let pos = match (n.position, n.step) {
                        (Some(p), _) => p,
                        (None, Some(st)) if st >= 0.0 => (st * step as f64).round() as u64,
                        _ => return Err("ogni nota richiede 'position' o 'step'".to_string()),
                    };
                    let mut note = Note::new(pos, n.length.unwrap_or(step).max(1), parse_key(&n.key)?, n.velocity.unwrap_or(0.78));
                    note.pan = n.pan.unwrap_or(0.0);
                    note.slide = n.slide.unwrap_or(false);
                    Ok(note)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let channel = ChannelId(a.channel);
            let mut cmds = Vec::new();
            if a.replace.unwrap_or(false) {
                cmds.push(Command::RemoveNotes { pattern, channel, filter: NoteFilter::default() });
            }
            let n = notes.len();
            cmds.push(Command::AddNotes { pattern, channel, notes });
            s.exec(Command::Batch { commands: cmds })?;
            Ok(json!({"pattern": pattern, "channel": a.channel, "added": n}))
        }),
        tool!("notes_list", "Elenca le note di un pattern (opzionalmente di un solo canale).", NotesList, |s, a| {
            let pattern = current_pattern(s, a.pattern)?;
            let p = s.project.pattern(pattern).ok_or("pattern inesistente")?;
            let mut out = BTreeMap::new();
            for (c, notes) in &p.notes {
                if a.channel.is_some_and(|x| x != c.0) {
                    continue;
                }
                out.insert(c.to_string(), notes.iter().map(|n| json!({
                    "position": n.position, "length": n.length, "key": n.key, "name": key_name(n.key),
                    "velocity": n.velocity, "pan": n.pan, "slide": n.slide,
                })).collect::<Vec<_>>());
            }
            Ok(json!({"pattern": pattern, "notes": out}))
        }),
        tool!("notes_remove", "Rimuove note di un canale (filtri opzionali: from/to in tick, key).", NotesRemove, |s, a| {
            let pattern = current_pattern(s, a.pattern)?;
            let filter = NoteFilter { from: a.from, to: a.to, key: a.key.as_ref().map(parse_key).transpose()? };
            let out = s.exec(Command::RemoveNotes { pattern, channel: ChannelId(a.channel), filter })?;
            Ok(json!({"removed": outcome_json(&out)}))
        }),
        tool!("notes_quantize", "Quantizza le note alla griglia (tick).", NotesQuantize, |s, a| {
            let pattern = current_pattern(s, a.pattern)?;
            s.exec(Command::QuantizeNotes {
                pattern,
                channel: a.channel.map(ChannelId),
                grid: a.grid,
                quantize_length: a.quantize_length.unwrap_or(false),
            })?;
            Ok(json!({"ok": true}))
        }),
        tool!("notes_transpose", "Traspone le note di N semitoni.", NotesTranspose, |s, a| {
            let pattern = current_pattern(s, a.pattern)?;
            s.exec(Command::TransposeNotes { pattern, channel: a.channel.map(ChannelId), semitones: a.semitones })?;
            Ok(json!({"ok": true}))
        }),
        // ---- Playlist
        tool!("playlist_place", "Piazza in Playlist un pattern, un'audio clip o un'automazione (posizione in tick o battute).", PlaylistPlace, |s, a| {
            let source = match (a.pattern, a.audio_channel, a.automation) {
                (Some(p), None, None) => ClipSource::Pattern(PatternId(p)),
                (None, Some(c), None) => ClipSource::Audio(ChannelId(c)),
                (None, None, Some(x)) => ClipSource::Automation(AutomationId(x)),
                _ => return Err("specificare esattamente uno tra pattern, audio_channel, automation".into()),
            };
            let pos = ticks_from(s, a.position, a.bar)?;
            let length = match (a.length, a.length_bars) {
                (Some(l), _) => Some(l),
                (None, Some(b)) => Some(s.project.time.bars_to_ticks(b)),
                _ => None,
            };
            let mut ids = Vec::new();
            let mut at = pos;
            for _ in 0..a.repeat.unwrap_or(1).clamp(1, 1000) {
                let Outcome::Clip(id) = s.exec(Command::PlaceClip { track: a.track, position: at, length, source })? else {
                    unreachable!()
                };
                let clip = s.project.playlist.clips.iter().find(|c| c.id == id).unwrap();
                at = clip.end();
                ids.push(id);
            }
            Ok(json!({"id": ids[0], "clips": ids, "end": at}))
        }),
        tool!("playlist_list", "Elenca le clip della Playlist.", NoArgs, |s, _| {
            let bar = s.project.time.ticks_per_bar() as f64;
            let clips: Vec<Value> = s.project.playlist.clips.iter().map(|c| json!({
                "id": c.id, "track": c.track, "position": c.position, "bar": c.position as f64 / bar,
                "length": c.length, "offset": c.offset, "source": c.source, "muted": c.muted,
            })).collect();
            Ok(json!({"clips": clips, "song_length_ticks": s.project.song_length(), "song_seconds": s.project.song_seconds()}))
        }),
        tool!("playlist_remove", "Rimuove una clip dalla Playlist.", ClipRef, |s, a| {
            s.exec(Command::RemoveClip { clip: ClipId(a.clip) })?;
            Ok(json!({"removed": a.clip}))
        }),
        tool!("playlist_move", "Sposta/ridimensiona/muta una clip.", ClipMove, |s, a| {
            let mut cmds = vec![Command::MoveClip { clip: ClipId(a.clip), track: a.track, position: a.position, length: a.length, offset: None }];
            if let Some(m) = a.muted {
                cmds.push(Command::SetClipMuted { clip: ClipId(a.clip), muted: m });
            }
            s.exec(Command::Batch { commands: cmds })?;
            Ok(json!({"ok": true}))
        }),
        tool!("playlist_track", "Rinomina o muta una traccia della Playlist.", TrackSet, |s, a| {
            s.exec(Command::SetPlaylistTrack { track: a.track, name: a.name, muted: a.muted, color: None })?;
            Ok(json!({"ok": true}))
        }),
        // ---- Mixer
        tool!("mixer_list", "Stato del mixer: insert usati (o tutti) con volume, pan, effetti, mandate.", MixerList, |s, a| {
            let used: Vec<u16> = s.project.channels.iter().map(|c| c.mixer_insert).collect();
            let list: Vec<Value> = s.project.mixer.inserts.iter().enumerate().filter(|(i, ins)| {
                a.all.unwrap_or(false) || *i == 0 || used.contains(&(*i as u16)) || !ins.effects.is_empty()
                    || ins.routes.iter().any(|r| r.target != MASTER) || ins.muted || ins.solo
            }).map(|(i, ins)| insert_json(i, ins)).collect();
            Ok(json!({"inserts": list}))
        }),
        tool!("mixer_set", "Modifica un insert del mixer (0 = master).", MixerSet, |s, a| {
            let volume = volume_arg(a.volume, a.volume_db, MIXER_UNITY, "volume")?;
            s.exec(Command::SetInsert {
                insert: a.insert, name: a.name, volume, pan: a.pan, stereo_separation: a.stereo_separation,
                muted: a.muted, solo: a.solo, phase_invert: a.phase_invert, swap_lr: a.swap_lr,
            })?;
            Ok(insert_json(a.insert as usize, s.project.insert(a.insert).unwrap()))
        }),
        tool!("mixer_route", "Crea/aggiorna/rimuove una mandata tra insert (i cicli sono rifiutati).", MixerRoute, |s, a| {
            let gain = if a.remove.unwrap_or(false) { None } else { Some(a.gain.unwrap_or(1.0)) };
            s.exec(Command::SetRoute { from: a.from, to: a.to, gain })?;
            Ok(json!({"ok": true}))
        }),
        tool!("mixer_add_effect", "Aggiunge un effetto in uno slot dell'insert (max 10).", EffectAdd, |s, a| {
            let inst = plugin_instance(&a.plugin, a.params, None)?;
            if mfc_plugins::descriptor(&a.plugin).unwrap().role != mfc_plugins::PluginRole::Effect {
                return Err(format!("{} è un generatore: usare channel_add", a.plugin));
            }
            if let Some(v) = inst.param("sidechain_insert") {
                check_sidechain(&s.project, a.insert, v)?;
            }
            let Outcome::Count(slot) = s.exec(Command::AddEffect { insert: a.insert, plugin: inst, slot: a.slot })? else { unreachable!() };
            let e = &s.project.insert(a.insert).unwrap().effects[slot];
            let mut v = effect_json(slot, e);
            v["id"] = json!(slot);
            v["insert"] = json!(a.insert);
            Ok(v)
        }),
        tool!("mixer_remove_effect", "Rimuove l'effetto da uno slot.", EffectRef, |s, a| {
            s.exec(Command::RemoveEffect { insert: a.insert, slot: a.slot })?;
            Ok(json!({"ok": true}))
        }),
        tool!("mixer_set_effect", "Abilita/disabilita, mix dry/wet e parametri di un effetto.", EffectSet, |s, a| {
            let kind = s
                .project
                .insert(a.insert)
                .and_then(|i| i.effects.get(a.slot as usize))
                .map(|e| e.plugin.kind.clone())
                .ok_or("slot inesistente")?;
            let desc = mfc_plugins::descriptor(&kind);
            let mut cmds = vec![Command::SetEffect { insert: a.insert, slot: a.slot, enabled: a.enabled, mix: a.mix }];
            for (param, value) in a.params.unwrap_or_default() {
                let value = match desc.and_then(|d| d.param(&param)) {
                    Some(p) => p.clamp(value),
                    None if desc.is_some() => return Err(format!("{kind}: parametro sconosciuto '{param}'")),
                    None => value,
                };
                if param == "sidechain_insert" {
                    check_sidechain(&s.project, a.insert, value)?;
                }
                cmds.push(Command::SetEffectParam { insert: a.insert, slot: a.slot, param, value });
            }
            s.exec(Command::Batch { commands: cmds })?;
            let e = &s.project.insert(a.insert).unwrap().effects[a.slot as usize];
            let mut v = effect_json(a.slot as usize, e);
            v["insert"] = json!(a.insert);
            Ok(v)
        }),
        // ---- Automazione
        tool!("automation_add", "Crea una clip di automazione (e opzionalmente la piazza in Playlist).", AutomationAdd, |s, a| {
            let target: AutomationTarget = serde_json::from_value(a.target).map_err(|e| format!("target non valido: {e}"))?;
            let points = a.points.iter().map(|p| AutomationPoint { position: p.position, value: p.value, tension: p.tension.unwrap_or(0.0) }).collect();
            let mut cmds = vec![Command::AddAutomation { name: a.name, target, min: a.min, max: a.max, points }];
            // L'id della nuova automazione è il prossimo id allocato.
            let next = AutomationId(s.project.next_id);
            if let Some(track) = a.track {
                cmds.push(Command::PlaceClip { track, position: a.position.unwrap_or(0), length: None, source: ClipSource::Automation(next) });
            }
            let out = s.exec(Command::Batch { commands: cmds })?;
            let mut ids = Vec::new();
            outcome_ids(&out, &mut ids);
            Ok(json!({"id": ids.first(), "clip": ids.get(1)}))
        }),
        // ---- Render e transport
        tool!("render_wav", "Esporta il brano (o un pattern) in WAV, con stem opzionali. Restituisce durata, picco, true peak, RMS, RMS attivo (solo parti sopra -60 dBFS), LUFS integrati e riduzione massima sul master.", RenderArgs, |s, a| {
            let format = match a.format.as_deref().unwrap_or("int24") {
                "int16" => mfc_engine::WavFormat::Int16,
                "int24" => mfc_engine::WavFormat::Int24,
                "float32" => mfc_engine::WavFormat::Float32,
                f => return Err(format!("formato sconosciuto: {f}")),
            };
            let opts = mfc_engine::RenderOptions {
                sample_rate: a.sample_rate.unwrap_or(44_100),
                pattern: a.pattern.map(PatternId),
                pattern_loops: a.loops.unwrap_or(1),
                tail_seconds: a.tail_seconds.unwrap_or(2.0),
                normalize: a.normalize.unwrap_or(false),
                stems: a.stems.unwrap_or(false),
                ..Default::default()
            };
            if opts.pattern.is_none() && s.project.playlist.clips.is_empty() {
                return Err("la Playlist è vuota: piazzare dei pattern (playlist_place) o specificare 'pattern'".into());
            }
            let path = s.resolve(&a.path);
            let out = mfc_engine::render_to_file(&s.project, &path, &opts, format).map_err(|e| e.to_string())?;
            let mut v = analysis_json(&out.mix.analyze());
            v["path"] = json!(path.display().to_string());
            v["seconds"] = json!(out.mix.seconds());
            v["clipping"] = json!(out.mix.peak() > 1.0);
            v["gain_reduction_max_db"] = json!((out.master_gain_reduction_db * 10.0).round() / 10.0);
            v["stems"] = out.stems.iter().map(|st| {
                let a = &st.audio;
                let mut x = json!({
                    "peak_db": db(a.peak()), "rms_db": db(a.rms()),
                    "rms_active_db": db(a.rms_active(mfc_engine::ACTIVE_THRESHOLD_DB)),
                    "lufs_integrated": a.lufs_integrated().map(|l| (l * 10.0).round() / 10.0),
                });
                x["insert"] = json!(st.insert);
                x["name"] = json!(st.name);
                x
            }).collect();
            Ok(v)
        }),
        tool!("transport", "Transport live: play/stop/pause/seek/status, modalità song o pattern.", TransportArgs, |s, a| {
            if a.action == "status" && s.transport.is_none() {
                return Ok(json!({"running": false}));
            }
            let h = s.ensure_transport()?.clone();
            match a.mode.as_deref() {
                Some("song") => {
                    h.set_mode(mfc_engine::PlayMode::Song);
                }
                Some("pattern") => {
                    let p = current_pattern(s, a.pattern)?;
                    h.set_mode(mfc_engine::PlayMode::Pattern(p));
                }
                Some(m) => return Err(format!("modalità sconosciuta: {m}")),
                None => {}
            }
            match a.action.as_str() {
                "play" => h.play(),
                "stop" => h.stop(),
                "pause" => h.pause(),
                "seek" => h.seek(a.position.unwrap_or(0.0)),
                "status" => true,
                x => return Err(format!("azione sconosciuta: {x}")),
            };
            let audible = s.transport.as_ref().is_some_and(|t| t.audible);
            Ok(json!({
                "running": true, "audible": audible,
                "playing": h.shared.is_playing(), "position_ticks": h.shared.position(), "tempo": h.shared.tempo(),
            }))
        }),
        tool!("preview_note", "Suona una nota di anteprima su un canale (richiede il transport live).", PreviewArgs, |s, a| {
            let key = parse_key(&a.key)?;
            let ch = ChannelId(a.channel);
            s.project.channel(ch).ok_or("canale inesistente")?;
            let h = s.ensure_transport()?.clone();
            h.preview_on(ch, key, a.velocity.unwrap_or(0.8));
            std::thread::sleep(std::time::Duration::from_millis(a.duration_ms.unwrap_or(300).min(5000)));
            h.preview_off(ch, key);
            Ok(json!({"played": key_name(key)}))
        }),
        // ---- MIDI
        tool!("midi_import", "Importa un file MIDI in un nuovo pattern (un canale per traccia).", MidiImportArgs, |s, a| {
            let path = s.resolve(&a.path);
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let imp = mfc_midi::parse_midi(&bytes, s.project.time.ppq).map_err(|e| e.to_string())?;
            let before = s.project.clone();
            let (pat, chans) =
                mfc_midi::import_into_project(&mut s.project, &imp, a.pattern_name.as_deref().unwrap_or("MIDI"))
                    .map_err(|e| e.to_string())?;
            s.history.record("midi_import", before);
            s.sync();
            Ok(json!({"id": pat, "pattern": pat, "channels": chans, "tempo": imp.tempo_bpm}))
        }),
        tool!("midi_export", "Esporta un pattern o l'intero brano in MIDI.", MidiExportArgs, |s, a| {
            let bytes = match a.pattern {
                Some(p) => mfc_midi::export_pattern(&s.project, PatternId(p)),
                None => mfc_midi::export_song(&s.project),
            }
            .map_err(|e| e.to_string())?;
            let path = s.resolve(&a.path);
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(json!({"path": path.display().to_string(), "bytes": bytes.len()}))
        }),
        // ---- FL Studio
        tool!("flp_import", "Importa un progetto FL Studio (.flp) sostituendo quello aperto.", PathArg, |s, a| {
            let path = s.resolve(&a.path);
            let (p, warnings) = crate::flp::import(&path)?;
            s.replace_project(p, None);
            let mut info = project_info(s)?;
            info["warnings"] = json!(warnings);
            Ok(info)
        }),
        tool!("flp_export", "Esporta il progetto in formato FL Studio (.flp).", PathArg, |s, a| {
            let path = s.resolve(&a.path);
            crate::flp::export(&path, &s.project)?;
            Ok(json!({"path": path.display().to_string()}))
        }),
        tool!("flp_inspect", "Elenca gli eventi grezzi di un file .flp (id, nome, tipo, dimensione, anteprima).", FlpInspectArgs, |s, a| {
            let path = s.resolve(&a.path);
            crate::flp::inspect(&path, a.offset.unwrap_or(0), a.limit.unwrap_or(200), a.id)
        }),
    ];
    v.sort_by_key(|t| t.name);
    v
}

/// Misure di livello in dB: picco, true peak, RMS (tutto il brano e sole parti attive), LUFS.
fn analysis_json(a: &mfc_engine::Analysis) -> Value {
    json!({
        "peak_db": db(a.peak),
        "true_peak_db": db(a.true_peak),
        "rms_db": db(a.rms),
        "rms_active_db": db(a.rms_active),
        "lufs_integrated": a.lufs_integrated.map(|l| (l * 10.0).round() / 10.0),
    })
}

fn pattern_json(s: &Session, id: PatternId) -> Value {
    let tb = s.project.time;
    let Some(p) = s.project.pattern(id) else {
        return json!({"id": id});
    };
    json!({
        "id": p.id, "pattern": p.id, "name": p.name,
        "length_ticks": p.effective_length(&tb),
        "length_bars": p.effective_length(&tb) as f64 / tb.ticks_per_bar() as f64,
        "notes": p.notes.iter().map(|(c, n)| (c.to_string(), n.len())).collect::<BTreeMap<_, _>>(),
    })
}

fn project_info(s: &mut Session) -> Result<Value, String> {
    let p = &s.project;
    let tb = p.time;
    Ok(json!({
        "title": p.info.title, "artist": p.info.artist, "genre": p.info.genre,
        "fl_version": p.info.fl_version,
        "tempo": p.tempo, "ppq": tb.ppq,
        "time_signature": format!("{}/{}", tb.signature.numerator, tb.signature.denominator),
        "ticks_per_step": tb.ticks_per_step(), "ticks_per_bar": tb.ticks_per_bar(),
        "master_volume": p.master_volume, "master_volume_db": fader_db(p.master_volume, MIXER_UNITY), "swing": p.swing,
        "channels": p.channels.iter().map(|c| json!({"id": c.id, "name": c.name, "plugin": c.plugin.kind, "insert": c.mixer_insert})).collect::<Vec<_>>(),
        "patterns": p.patterns.iter().map(|x| json!({"id": x.id, "name": x.name})).collect::<Vec<_>>(),
        "current_pattern": p.current_pattern,
        "clips": p.playlist.clips.len(),
        "automations": p.automations.len(),
        "song_length_bars": p.song_length() as f64 / tb.ticks_per_bar() as f64,
        "song_seconds": p.song_seconds(),
        "path": s.path.as_ref().map(|x| x.display().to_string()),
    }))
}
