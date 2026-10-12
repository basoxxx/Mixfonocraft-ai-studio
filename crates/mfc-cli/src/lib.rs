//! # mfc-cli
//!
//! Logica della CLI `mixfonocraft`: caricamento e salvataggio dei progetti in base
//! all'estensione (`.mfc` nativo, `.flp` FL Studio, `.mid` MIDI), riepiloghi, render WAV,
//! ispezione degli eventi FLP, catalogo dei plugin e progetto dimostrativo.
//!
//! Tutto ciò che il binario fa sta qui, così è testabile; `src/main.rs` si limita a
//! interpretare gli argomenti e a stampare i risultati.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use mfc_core::{
    AutomationPoint, AutomationTarget, ChannelId, ChannelKind, ClipSource, Command, Note, Outcome, PatternId,
    PluginInstance, Project, TimeBase, MASTER, MIXER_UNITY,
};
use mfc_engine::{RenderOptions, WavFormat};
use serde_json::{json, Value};

// ---------------------------------------------------------------- formati e I/O

/// Formato di un progetto, riconosciuto dall'estensione del file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Progetto nativo (JSON).
    Mfc,
    /// Progetto FL Studio.
    Flp,
    /// File MIDI standard (SMF).
    Midi,
}

impl Format {
    /// Riconosce il formato dall'estensione (case-insensitive).
    pub fn from_path(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "mfc" => Ok(Format::Mfc),
            "flp" => Ok(Format::Flp),
            "mid" | "midi" => Ok(Format::Midi),
            _ => bail!(
                "formato non riconosciuto per {}: usare un'estensione .mfc, .flp o .mid",
                path.display()
            ),
        }
    }
}

/// Progetto caricato, con gli avvisi di mappatura prodotti dall'import (solo `.flp`).
#[derive(Debug, Clone)]
pub struct Loaded {
    pub project: Project,
    pub warnings: Vec<String>,
}

/// Carica un progetto da `.mfc`, `.flp` o `.mid`.
///
/// Un `.mid` diventa un nuovo progetto con le tracce MIDI come pattern, piazzato a battuta 0.
pub fn load_project(path: &Path) -> Result<Loaded> {
    let loaded = match Format::from_path(path)? {
        Format::Mfc => {
            let text = fs::read_to_string(path).with_context(|| format!("lettura di {}", path.display()))?;
            let project =
                Project::from_json(&text).with_context(|| format!("progetto .mfc non valido: {}", path.display()))?;
            Loaded {
                project,
                warnings: Vec::new(),
            }
        }
        Format::Flp => {
            let (_, res) =
                mfc_flp::read_file(path).with_context(|| format!("import FL Studio di {}", path.display()))?;
            Loaded {
                project: res.project,
                warnings: res.warnings,
            }
        }
        Format::Midi => {
            let bytes = fs::read(path).with_context(|| format!("lettura di {}", path.display()))?;
            let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("MIDI");
            Loaded {
                project: project_from_midi(&bytes, title)?,
                warnings: Vec::new(),
            }
        }
    };
    Ok(loaded)
}

/// Crea un progetto nuovo dai byte di un file MIDI: il pattern importato viene piazzato
/// a battuta 0 della traccia 0 e il pattern vuoto di default viene rimosso.
pub fn project_from_midi(bytes: &[u8], title: &str) -> Result<Project> {
    let mut project = Project::new(title);
    let imp = mfc_midi::parse_midi(bytes, project.time.ppq).context("file MIDI non valido")?;
    let (pattern, _) = mfc_midi::import_into_project(&mut project, &imp, title).context("import MIDI")?;
    let default = project.patterns[0].id;
    if default != pattern {
        apply(&mut project, Command::RemovePattern { pattern: default })?;
    }
    let length = project.pattern(pattern).map(|p| p.effective_length(&project.time));
    apply(
        &mut project,
        Command::PlaceClip {
            track: 0,
            position: 0,
            length,
            source: ClipSource::Pattern(pattern),
        },
    )?;
    Ok(project)
}

/// Salva un progetto nel formato indicato dall'estensione di `path`.
pub fn save_project(project: &Project, path: &Path) -> Result<()> {
    match Format::from_path(path)? {
        Format::Mfc => {
            let text = project.to_json().context("serializzazione .mfc")?;
            fs::write(path, text).with_context(|| format!("scrittura di {}", path.display()))?;
        }
        Format::Flp => {
            mfc_flp::write_file(path, project).with_context(|| format!("export FL Studio di {}", path.display()))?;
        }
        Format::Midi => {
            let bytes = mfc_midi::export_song(project).context("export MIDI del brano")?;
            fs::write(path, bytes).with_context(|| format!("scrittura di {}", path.display()))?;
        }
    }
    Ok(())
}

/// Progetto vuoto con titolo e, se indicato, tempo.
pub fn new_project(title: &str, tempo: Option<f64>) -> Result<Project> {
    let mut project = Project::new(title);
    if let Some(bpm) = tempo {
        apply(&mut project, Command::SetTempo { bpm })?;
    }
    Ok(project)
}

// ---------------------------------------------------------------- riepilogo

/// Conversione di un guadagno lineare in dB, con pavimento a -120 dB (valori JSON finiti).
pub fn to_db(linear: f32) -> f64 {
    if linear <= 1e-6 {
        -120.0
    } else {
        (20.0 * f64::from(linear).log10() * 10.0).round() / 10.0
    }
}

/// Riepilogo leggibile di un progetto.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub title: String,
    pub artist: String,
    pub genre: String,
    pub fl_version: Option<String>,
    pub tempo: f64,
    pub ppq: u16,
    pub time_signature: String,
    pub channels: Vec<ChannelSummary>,
    pub patterns: Vec<PatternSummary>,
    pub clips: Vec<ClipSummary>,
    pub automations: Vec<AutomationSummary>,
    /// Master più gli insert in uso (con effetti, mandate o parametri non di default).
    pub inserts: Vec<InsertSummary>,
    pub length_bars: f64,
    pub length_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSummary {
    pub id: u32,
    pub name: String,
    pub plugin: String,
    /// Nome originale se il plugin è stato importato da FL e non è supportato.
    pub foreign_plugin: Option<String>,
    pub insert: u16,
    pub notes: usize,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternSummary {
    pub id: u32,
    pub name: String,
    pub bars: f64,
    pub notes: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipSummary {
    pub id: u32,
    pub track: u16,
    pub start_bar: f64,
    pub bars: f64,
    /// Sorgente leggibile, es. `"pattern 1"`, `"audio 3"`, `"automazione 2"`.
    pub source: String,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationSummary {
    pub id: u32,
    pub name: String,
    pub target: String,
    pub points: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertSummary {
    pub index: u16,
    pub name: String,
    pub gain_db: f64,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    /// Effetti in ordine di slot: `"mfc.reverb"` (o `"mfc.reverb (spento)"`).
    pub effects: Vec<String>,
    /// Insert di destinazione delle mandate in uscita.
    pub routes: Vec<u16>,
}

impl Summary {
    /// Costruisce il riepilogo di un progetto.
    pub fn from_project(p: &Project) -> Self {
        let tb = p.time;
        let bar = tb.ticks_per_bar() as f64;
        let notes_of_channel =
            |c: ChannelId| -> usize { p.patterns.iter().filter_map(|pt| pt.notes.get(&c)).map(Vec::len).sum() };
        let channels = p
            .channels
            .iter()
            .map(|c| ChannelSummary {
                id: c.id.0,
                name: c.name.clone(),
                plugin: c.plugin.kind.clone(),
                foreign_plugin: c.plugin.foreign_name.clone(),
                insert: c.mixer_insert,
                notes: notes_of_channel(c.id),
                muted: c.muted,
            })
            .collect();
        let patterns = p
            .patterns
            .iter()
            .map(|pt| PatternSummary {
                id: pt.id.0,
                name: pt.name.clone(),
                bars: pt.effective_length(&tb) as f64 / bar,
                notes: pt.notes.values().map(Vec::len).sum(),
            })
            .collect();
        let clips = p
            .playlist
            .clips
            .iter()
            .map(|c| ClipSummary {
                id: c.id.0,
                track: c.track,
                start_bar: c.position as f64 / bar,
                bars: c.length as f64 / bar,
                source: match c.source {
                    ClipSource::Pattern(id) => format!("pattern {}", id.0),
                    ClipSource::Audio(id) => format!("audio {}", id.0),
                    ClipSource::Automation(id) => format!("automazione {}", id.0),
                },
                muted: c.muted,
            })
            .collect();
        let automations = p
            .automations
            .iter()
            .map(|a| AutomationSummary {
                id: a.id.0,
                name: a.name.clone(),
                target: target_label(&a.target),
                points: a.points.len(),
            })
            .collect();
        let inserts = p
            .mixer
            .inserts
            .iter()
            .enumerate()
            .filter(|(i, ins)| *i == MASTER as usize || insert_in_use(p, *i as u16, ins))
            .map(|(i, ins)| InsertSummary {
                index: i as u16,
                name: ins.name.clone(),
                gain_db: to_db(ins.gain()),
                pan: ins.pan,
                muted: ins.muted,
                solo: ins.solo,
                effects: ins
                    .effects
                    .iter()
                    .map(|e| {
                        if e.enabled {
                            e.plugin.kind.clone()
                        } else {
                            format!("{} (spento)", e.plugin.kind)
                        }
                    })
                    .collect(),
                routes: ins.routes.iter().map(|r| r.target).collect(),
            })
            .collect();
        Self {
            title: p.info.title.clone(),
            artist: p.info.artist.clone(),
            genre: p.info.genre.clone(),
            fl_version: p.info.fl_version.clone(),
            tempo: p.tempo,
            ppq: tb.ppq,
            time_signature: format!("{}/{}", tb.signature.numerator, tb.signature.denominator),
            channels,
            patterns,
            clips,
            automations,
            inserts,
            length_bars: p.song_length() as f64 / bar,
            length_seconds: p.song_seconds(),
        }
    }

    /// Rappresentazione JSON (per `info --json`).
    pub fn to_json(&self) -> Value {
        json!({
            "title": self.title,
            "artist": self.artist,
            "genre": self.genre,
            "fl_version": self.fl_version,
            "tempo": self.tempo,
            "ppq": self.ppq,
            "time_signature": self.time_signature,
            "length_bars": self.length_bars,
            "length_seconds": self.length_seconds,
            "channels": self.channels.iter().map(|c| json!({
                "id": c.id, "name": c.name, "plugin": c.plugin,
                "foreign_plugin": c.foreign_plugin, "insert": c.insert,
                "notes": c.notes, "muted": c.muted,
            })).collect::<Vec<_>>(),
            "patterns": self.patterns.iter().map(|p| json!({
                "id": p.id, "name": p.name, "bars": p.bars, "notes": p.notes,
            })).collect::<Vec<_>>(),
            "clips": self.clips.iter().map(|c| json!({
                "id": c.id, "track": c.track, "start_bar": c.start_bar,
                "bars": c.bars, "source": c.source, "muted": c.muted,
            })).collect::<Vec<_>>(),
            "automations": self.automations.iter().map(|a| json!({
                "id": a.id, "name": a.name, "target": a.target, "points": a.points,
            })).collect::<Vec<_>>(),
            "inserts": self.inserts.iter().map(|i| json!({
                "index": i.index, "name": i.name, "gain_db": i.gain_db, "pan": i.pan,
                "muted": i.muted, "solo": i.solo, "effects": i.effects, "routes": i.routes,
            })).collect::<Vec<_>>(),
        })
    }

    /// Testo per la console, in italiano.
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "Titolo:   {}", self.title);
        if !self.artist.is_empty() {
            let _ = writeln!(s, "Artista:  {}", self.artist);
        }
        if !self.genre.is_empty() {
            let _ = writeln!(s, "Genere:   {}", self.genre);
        }
        if let Some(v) = &self.fl_version {
            let _ = writeln!(s, "FL:       {v}");
        }
        let _ = writeln!(
            s,
            "Tempo:    {:.1} BPM · metrica {} · PPQ {}",
            self.tempo, self.time_signature, self.ppq
        );
        let _ = writeln!(
            s,
            "Durata:   {:.2} battute · {:.2} s",
            self.length_bars, self.length_seconds
        );
        let _ = writeln!(s, "\nCanali ({}):", self.channels.len());
        for c in &self.channels {
            let plugin = match &c.foreign_plugin {
                Some(f) => format!("{} [{f}]", c.plugin),
                None => c.plugin.clone(),
            };
            let _ = writeln!(
                s,
                "  [{:>3}] {:<14} {:<26} insert {:<3} note {}{}",
                c.id,
                c.name,
                plugin,
                c.insert,
                c.notes,
                if c.muted { " (muto)" } else { "" }
            );
        }
        let _ = writeln!(s, "\nPattern ({}):", self.patterns.len());
        for p in &self.patterns {
            let _ = writeln!(
                s,
                "  [{:>3}] {:<14} {:>6.2} battute · {} note",
                p.id, p.name, p.bars, p.notes
            );
        }
        let _ = writeln!(s, "\nPlaylist: {} clip", self.clips.len());
        for c in &self.clips {
            let _ = writeln!(
                s,
                "  [{:>3}] traccia {:<3} da battuta {:>6.2} per {:>6.2} battute · {}{}",
                c.id,
                c.track,
                c.start_bar,
                c.bars,
                c.source,
                if c.muted { " (muta)" } else { "" }
            );
        }
        if !self.automations.is_empty() {
            let _ = writeln!(s, "\nAutomazioni ({}):", self.automations.len());
            for a in &self.automations {
                let _ = writeln!(s, "  [{:>3}] {:<14} {} · {} punti", a.id, a.name, a.target, a.points);
            }
        }
        let _ = writeln!(s, "\nMixer (insert in uso):");
        for i in &self.inserts {
            let fx = if i.effects.is_empty() {
                "nessun effetto".to_string()
            } else {
                i.effects.join(", ")
            };
            let routes = if i.routes.is_empty() {
                String::new()
            } else {
                format!(
                    " → {}",
                    i.routes.iter().map(u16::to_string).collect::<Vec<_>>().join(",")
                )
            };
            let _ = writeln!(
                s,
                "  [{:>3}] {:<12} {:>7.1} dB pan {:+.2}{}{}  {}{}",
                i.index,
                i.name,
                i.gain_db,
                i.pan,
                if i.muted { " muto" } else { "" },
                if i.solo { " solo" } else { "" },
                fx,
                routes
            );
        }
        s
    }
}

/// Un insert è "in uso" se ha canali assegnati, effetti, mandate non standard o parametri non di default.
fn insert_in_use(p: &Project, index: u16, ins: &mfc_core::MixerInsert) -> bool {
    let assigned = p.channels.iter().any(|c| c.mixer_insert == index);
    let default_route = ins.routes.len() == 1 && ins.routes[0].target == MASTER && ins.routes[0].gain == 1.0;
    assigned
        || !ins.effects.is_empty()
        || (!default_route && !ins.routes.is_empty())
        || ins.muted
        || ins.solo
        || ins.phase_invert
        || ins.swap_lr
        || (ins.volume - MIXER_UNITY).abs() > 1e-6
        || ins.pan.abs() > 1e-6
}

/// Descrizione leggibile di un bersaglio di automazione.
pub fn target_label(t: &AutomationTarget) -> String {
    match t {
        AutomationTarget::MasterVolume => "volume master".into(),
        AutomationTarget::Tempo => "tempo".into(),
        AutomationTarget::ChannelVolume { channel } => format!("volume canale {channel}"),
        AutomationTarget::ChannelPan { channel } => format!("pan canale {channel}"),
        AutomationTarget::ChannelPitch { channel } => format!("pitch canale {channel}"),
        AutomationTarget::ChannelParam { channel, param } => format!("canale {channel} · {param}"),
        AutomationTarget::MixerVolume { insert } => format!("volume insert {insert}"),
        AutomationTarget::MixerPan { insert } => format!("pan insert {insert}"),
        AutomationTarget::EffectParam { insert, slot, param } => {
            format!("insert {insert} · slot {slot} · {param}")
        }
        AutomationTarget::EffectMix { insert, slot } => format!("insert {insert} · slot {slot} · mix"),
    }
}

// ---------------------------------------------------------------- render

/// Opzioni della CLI per `render`.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    pub sample_rate: u32,
    /// `None` = brano intero dalla Playlist.
    pub pattern: Option<PatternId>,
    pub loops: u32,
    pub tail_seconds: f64,
    pub normalize: bool,
    pub stems: bool,
    pub format: WavFormat,
}

impl Default for RenderRequest {
    fn default() -> Self {
        Self {
            sample_rate: 44_100,
            pattern: None,
            loops: 1,
            tail_seconds: 2.0,
            normalize: false,
            stems: false,
            format: WavFormat::Int24,
        }
    }
}

/// Esito di un render: durata, livelli e stem.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderReport {
    pub path: PathBuf,
    pub seconds: f64,
    pub peak_db: f64,
    pub rms_db: f64,
    pub clipping: bool,
    pub stems: Vec<StemReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StemReport {
    pub insert: u16,
    pub name: String,
    pub peak_db: f64,
}

/// Parsa il formato WAV dell'opzione `--format`.
pub fn parse_format(s: &str) -> Result<WavFormat> {
    match s {
        "int16" => Ok(WavFormat::Int16),
        "int24" => Ok(WavFormat::Int24),
        "float32" => Ok(WavFormat::Float32),
        other => bail!("formato WAV sconosciuto '{other}' (attesi int16, int24, float32)"),
    }
}

/// Renderizza il progetto su `out` (e gli stem accanto, se richiesti).
pub fn render_wav(project: &Project, out: &Path, req: &RenderRequest) -> Result<RenderReport> {
    if req.pattern.is_none() && project.playlist.clips.is_empty() {
        bail!("la Playlist è vuota: piazzare dei pattern oppure indicare un pattern da renderizzare");
    }
    let opts = RenderOptions {
        sample_rate: req.sample_rate,
        pattern: req.pattern,
        pattern_loops: req.loops.max(1),
        tail_seconds: req.tail_seconds.max(0.0),
        normalize: req.normalize,
        stems: req.stems,
        ..Default::default()
    };
    let rendered = mfc_engine::render_to_file(project, out, &opts, req.format)
        .with_context(|| format!("render di {}", out.display()))?;
    let mix = &rendered.mix;
    Ok(RenderReport {
        path: out.to_path_buf(),
        seconds: mix.seconds(),
        peak_db: to_db(mix.peak()),
        rms_db: to_db(mix.rms()),
        clipping: mix.peak() > 1.0,
        stems: rendered
            .stems
            .iter()
            .map(|s| StemReport {
                insert: s.insert,
                name: s.name.clone(),
                peak_db: to_db(s.audio.peak()),
            })
            .collect(),
    })
}

// ---------------------------------------------------------------- FLP

/// Una riga di `flp-inspect`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlpRow {
    pub index: usize,
    pub id: u8,
    pub name: String,
    pub kind: &'static str,
    pub size: usize,
    pub preview: String,
}

/// Esito di `flp-inspect`: header, conteggi e pagina di eventi.
#[derive(Debug, Clone, PartialEq)]
pub struct FlpReport {
    pub format: i16,
    pub channels: u16,
    pub ppq: u16,
    pub total: usize,
    pub matching: usize,
    pub offset: usize,
    pub rows: Vec<FlpRow>,
}

/// Elenca gli eventi grezzi di un `.flp`, con filtro per id e paginazione.
pub fn flp_inspect(path: &Path, offset: usize, limit: usize, id: Option<u8>) -> Result<FlpReport> {
    let bytes = fs::read(path).with_context(|| format!("lettura di {}", path.display()))?;
    let file = mfc_flp::parse(&bytes).with_context(|| format!("FLP non valido: {}", path.display()))?;
    let all = mfc_flp::inspect(&file);
    let matching: Vec<_> = all.iter().filter(|e| id.is_none_or(|x| e.id == x)).collect();
    let rows = matching
        .iter()
        .skip(offset)
        .take(limit)
        .map(|e| FlpRow {
            index: e.index,
            id: e.id,
            name: e.name.unwrap_or("-").to_string(),
            kind: e.kind,
            size: e.size,
            preview: e.preview.clone(),
        })
        .collect();
    Ok(FlpReport {
        format: file.header.format,
        channels: file.header.n_channels,
        ppq: file.header.ppq,
        total: all.len(),
        matching: matching.len(),
        offset,
        rows,
    })
}

// ---------------------------------------------------------------- plugin

/// Elenco JSON dei plugin nativi con i parametri.
pub fn plugins_json() -> Value {
    let list: Vec<Value> = mfc_plugins::registry()
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.name,
                "role": role_label(d.role),
                "description": d.description,
                "params": d.params.iter().map(|p| json!({
                    "name": p.name, "label": p.label, "min": p.min, "max": p.max,
                    "default": p.default, "unit": p.unit, "choices": p.choices,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "plugins": list })
}

/// Elenco testuale dei plugin nativi con i parametri.
pub fn plugins_text() -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for d in mfc_plugins::registry() {
        let _ = writeln!(s, "{}  {} [{}]", d.id, d.name, role_label(d.role));
        let _ = writeln!(s, "    {}", d.description);
        for p in d.params {
            let range = if p.choices.is_empty() {
                format!("{}..{} {}", p.min, p.max, p.unit)
            } else {
                p.choices.join(" | ")
            };
            let _ = writeln!(
                s,
                "      {:<16} {:<28} {:<24} default {}",
                p.name, p.label, range, p.default
            );
        }
    }
    s
}

fn role_label(role: mfc_plugins::PluginRole) -> &'static str {
    match role {
        mfc_plugins::PluginRole::Generator => "generatore",
        mfc_plugins::PluginRole::Effect => "effetto",
    }
}

// ---------------------------------------------------------------- demo

/// Tempo del progetto dimostrativo.
pub const DEMO_BPM: f64 = 128.0;
/// Lunghezza del brano dimostrativo in battute.
pub const DEMO_BARS: f64 = 8.0;
/// Fader del master del progetto dimostrativo (0.4 = -6 dB rispetto allo 0 dB di FL).
const DEMO_MASTER: f32 = 0.4;
/// Lunghezza di ciascun pattern dimostrativo in battute.
const PATTERN_BARS: f64 = 4.0;
/// Step per battuta (griglia 1/16).
const STEPS_PER_BAR: u64 = 16;
/// Radici del basso per battuta: Am – F – C – G (A2, F2, C3, G2; C5 = 60).
const BASS_ROOTS: [u8; 4] = [33, 29, 36, 31];
/// Melodia del lead per battuta: (step di inizio nella battuta, durata in step, nota).
const LEAD: [[(u64, u64, u8); 4]; 4] = [
    [(0, 6, 64), (6, 2, 62), (8, 4, 60), (12, 4, 57)],
    [(0, 4, 57), (4, 4, 60), (8, 4, 65), (12, 4, 64)],
    [(0, 6, 67), (6, 2, 64), (8, 4, 60), (12, 4, 62)],
    [(0, 4, 71), (4, 4, 62), (8, 4, 67), (12, 4, 64)],
];

fn apply(p: &mut Project, cmd: Command) -> Result<Outcome> {
    let label = cmd.label();
    cmd.apply(p).with_context(|| format!("comando '{label}'"))
}

/// Costruisce un plugin con i parametri indicati, verificando che esistano nel descrittore.
pub fn plugin(id: &str, params: &[(&str, f32)]) -> Result<PluginInstance> {
    let mut inst = mfc_plugins::default_instance(id).ok_or_else(|| anyhow!("plugin sconosciuto: {id}"))?;
    let desc = mfc_plugins::descriptor(id).ok_or_else(|| anyhow!("plugin sconosciuto: {id}"))?;
    for (name, value) in params {
        let desc_param = desc
            .param(name)
            .ok_or_else(|| anyhow!("{id}: parametro '{name}' inesistente"))?;
        inst.params.insert((*name).to_string(), desc_param.clamp(*value));
    }
    Ok(inst)
}

fn add_channel(p: &mut Project, name: &str, id: &str, params: &[(&str, f32)], insert: u16) -> Result<ChannelId> {
    let out = apply(
        p,
        Command::AddChannel {
            name: name.to_string(),
            kind: ChannelKind::Generator,
            plugin: plugin(id, params)?,
            mixer_insert: Some(insert),
        },
    )?;
    match out {
        Outcome::Channel(c) => Ok(c),
        other => bail!("esito inatteso da AddChannel: {other:?}"),
    }
}

fn set_pan(p: &mut Project, channel: ChannelId, pan: f32) -> Result<()> {
    apply(
        p,
        Command::SetChannel {
            channel,
            name: None,
            volume: None,
            pan: Some(pan),
            pitch_cents: None,
            muted: None,
            mixer_insert: None,
            root_note: None,
            color: None,
        },
    )?;
    Ok(())
}

fn name_insert(p: &mut Project, insert: u16, name: &str) -> Result<()> {
    apply(
        p,
        Command::SetInsert {
            insert,
            name: Some(name.to_string()),
            volume: None,
            pan: None,
            stereo_separation: None,
            muted: None,
            solo: None,
            phase_invert: None,
            swap_lr: None,
        },
    )?;
    Ok(())
}

/// Nota che inizia allo step `start` (in 1/16) e dura `len` step.
fn note_at(tb: &TimeBase, start: u64, len: u64, key: u8, velocity: f32) -> Note {
    let step = tb.ticks_per_step();
    Note::new(start * step, len * step, key, velocity)
}

/// Linea di basso: per ogni battuta root (6 step), ottava sopra (2 step), root (4 step).
fn bass_notes(tb: &TimeBase) -> Vec<Note> {
    let mut out = Vec::new();
    for (bar, root) in BASS_ROOTS.iter().enumerate() {
        let base = bar as u64 * STEPS_PER_BAR;
        out.push(note_at(tb, base, 6, *root, 0.85));
        out.push(note_at(tb, base + 10, 2, root + 12, 0.7));
        out.push(note_at(tb, base + 12, 4, *root, 0.8));
    }
    out
}

/// Melodia del lead su quattro battute.
fn lead_notes(tb: &TimeBase) -> Vec<Note> {
    let mut out = Vec::new();
    for (bar, phrase) in LEAD.iter().enumerate() {
        let base = bar as u64 * STEPS_PER_BAR;
        for (start, len, key) in phrase {
            out.push(note_at(tb, base + start, *len, *key, 0.8));
        }
    }
    out
}

/// Progetto dimostrativo: A minor a 128 BPM, cinque canali (kick, clap, hats, basso, lead),
/// due pattern di quattro battute ("Groove" e "Full") in Playlist, effetti sul lead e una
/// dissolvenza del master sulle ultime due battute.
pub fn build_demo() -> Result<Project> {
    let mut p = Project::new("Mixfonocraft Demo");
    apply(&mut p, Command::SetTempo { bpm: DEMO_BPM })?;
    apply(
        &mut p,
        Command::SetInfo {
            title: None,
            artist: Some("Mixfonocraft AI Studio".into()),
            genre: Some("Electro".into()),
            comments: Some("Progetto dimostrativo generato da `mixfonocraft demo`.".into()),
        },
    )?;
    let tb = p.time;

    // Master a -6 dB (0.4 sul fader, 0.8 = 0 dB) per lasciare headroom al mix.
    apply(&mut p, Command::SetMasterVolume { volume: DEMO_MASTER })?;

    // Channel Rack: un insert del mixer per strumento.
    let kick = add_channel(&mut p, "Kick", "mfc.kick", &[], 1)?;
    let clap = add_channel(&mut p, "Clap", "mfc.drumsynth", &[("type", 2.0)], 2)?;
    let hats = add_channel(&mut p, "Hats", "mfc.drumsynth", &[("type", 3.0), ("decay", 0.3)], 3)?;
    let bass = add_channel(
        &mut p,
        "Bass",
        "mfc.3xosc",
        &[
            ("cutoff", 900.0),
            ("resonance", 0.25),
            ("decay", 0.25),
            ("sustain", 0.6),
            ("release", 0.12),
        ],
        4,
    )?;
    let lead = add_channel(
        &mut p,
        "Lead",
        "mfc.supersaw",
        &[
            ("cutoff", 7000.0),
            ("resonance", 0.15),
            ("attack", 0.02),
            ("release", 0.4),
            ("amp", 0.45),
        ],
        5,
    )?;
    set_pan(&mut p, hats, 0.2)?;
    set_pan(&mut p, lead, -0.15)?;

    for (insert, name) in [(1, "Kick"), (2, "Clap"), (3, "Hats"), (4, "Bass"), (5, "Lead")] {
        name_insert(&mut p, insert, name)?;
    }
    apply(
        &mut p,
        Command::AddEffect {
            insert: 5,
            plugin: plugin("mfc.reverb", &[("room_size", 0.7), ("wet", 0.22), ("predelay", 12.0)])?,
            slot: Some(0),
        },
    )?;
    apply(
        &mut p,
        Command::AddEffect {
            insert: 5,
            plugin: plugin("mfc.delay", &[("time", 352.0), ("feedback", 0.35), ("wet", 0.18)])?,
            slot: Some(1),
        },
    )?;

    // Pattern: "Groove" (batteria + basso) e "Full" (anche il lead).
    let groove = p.patterns[0].id;
    apply(
        &mut p,
        Command::SetPattern {
            pattern: groove,
            name: Some("Groove".into()),
            length: Some(Some(tb.bars_to_ticks(PATTERN_BARS))),
            color: None,
        },
    )?;
    let full = match apply(&mut p, Command::AddPattern { name: "Full".into() })? {
        Outcome::Pattern(id) => id,
        other => bail!("esito inatteso da AddPattern: {other:?}"),
    };
    apply(
        &mut p,
        Command::SetPattern {
            pattern: full,
            name: None,
            length: Some(Some(tb.bars_to_ticks(PATTERN_BARS))),
            color: None,
        },
    )?;

    let bars = PATTERN_BARS as usize;
    for pattern in [groove, full] {
        for (channel, row) in [
            (kick, "x...x...x...x..."),
            (clap, "....x.......x..."),
            (hats, "..x...x...x...x."),
        ] {
            apply(
                &mut p,
                Command::SetStepRow {
                    pattern,
                    channel,
                    row: row.repeat(bars),
                },
            )?;
        }
        apply(
            &mut p,
            Command::AddNotes {
                pattern,
                channel: bass,
                notes: bass_notes(&tb),
            },
        )?;
    }
    apply(
        &mut p,
        Command::AddNotes {
            pattern: full,
            channel: lead,
            notes: lead_notes(&tb),
        },
    )?;

    // Playlist: traccia 0 per i pattern, traccia 1 per l'automazione.
    apply(
        &mut p,
        Command::SetPlaylistTrack {
            track: 0,
            name: Some("Beat".into()),
            muted: None,
            color: None,
        },
    )?;
    apply(
        &mut p,
        Command::SetPlaylistTrack {
            track: 1,
            name: Some("Master fade".into()),
            muted: None,
            color: None,
        },
    )?;
    apply(
        &mut p,
        Command::PlaceClip {
            track: 0,
            position: 0,
            length: None,
            source: ClipSource::Pattern(groove),
        },
    )?;
    apply(
        &mut p,
        Command::PlaceClip {
            track: 0,
            position: tb.bars_to_ticks(PATTERN_BARS),
            length: None,
            source: ClipSource::Pattern(full),
        },
    )?;

    // Dissolvenza del master sulle ultime due battute (valore 1 = 0 dB, 0 = silenzio).
    let fade = match apply(
        &mut p,
        Command::AddAutomation {
            name: "Fade out master".into(),
            target: AutomationTarget::MasterVolume,
            min: 0.0,
            max: DEMO_MASTER,
            points: vec![
                AutomationPoint {
                    position: 0,
                    value: 1.0,
                    tension: 0.0,
                },
                AutomationPoint {
                    position: tb.bars_to_ticks(2.0),
                    value: 0.0,
                    tension: 0.0,
                },
            ],
        },
    )? {
        Outcome::Automation(id) => id,
        other => bail!("esito inatteso da AddAutomation: {other:?}"),
    };
    apply(
        &mut p,
        Command::PlaceClip {
            track: 1,
            position: tb.bars_to_ticks(DEMO_BARS - 2.0),
            length: None,
            source: ClipSource::Automation(fade),
        },
    )?;
    Ok(p)
}
