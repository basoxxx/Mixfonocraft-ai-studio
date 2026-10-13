//! Registrazione delle automazioni dai movimenti dei controlli durante la riproduzione.
//!
//! [`AutomationRecorder`] è puro (nessun I/O, nessun orologio): riceve bersaglio, valore
//! normalizzato e posizione in tick, accumula i punti per bersaglio sfoltendoli e, alla fine,
//! produce un [`Command::RecordAutomation`] per bersaglio. Il comando crea la clip di
//! automazione (e la piazza in Playlist) oppure estende quella esistente dello stesso
//! bersaglio, sostituendo i punti nell'intervallo registrato.

use crate::command::{Command, Outcome};
use crate::project::*;
use crate::time::Tick;
use crate::CoreError;

/// Regole di sfoltimento dei punti registrati.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thinning {
    /// Variazioni di valore (normalizzato) più piccole di questa vengono ignorate.
    pub min_delta: f32,
    /// Distanza minima in tick tra due punti...
    pub min_ticks: Tick,
    /// ...a meno che il valore non salti almeno di tanto.
    pub jump: f32,
    /// Dopo una pausa più lunga di questa (in tick) il valore precedente viene tenuto fino
    /// al nuovo punto (gradino) invece di interpolare lungo tutta la pausa.
    pub hold_gap: Tick,
}

impl Default for Thinning {
    fn default() -> Self {
        Self {
            min_delta: 0.004,
            min_ticks: 4,
            jump: 0.15,
            hold_gap: 48,
        }
    }
}

/// Registrazione in corso di un bersaglio.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    pub target: AutomationTarget,
    /// Nome della clip se va creata.
    pub name: String,
    /// Range reale mappato su 0..=1 dai valori dei punti.
    pub min: f32,
    pub max: f32,
    /// Punti accettati, in tick assoluti, ordinati.
    pub points: Vec<AutomationPoint>,
    /// Ultimo campione scartato perché troppo vicino: aggiunto alla fine se serve.
    tail: Option<AutomationPoint>,
}

impl Take {
    fn new(target: AutomationTarget, name: String, min: f32, max: f32) -> Self {
        Self {
            target,
            name,
            min,
            max,
            points: Vec::new(),
            tail: None,
        }
    }

    fn push(&mut self, th: &Thinning, value: f32, pos: Tick) {
        let pt = point(pos, value);
        // Salto all'indietro (loop, riposizionamento): il nuovo passaggio sovrascrive.
        if self.points.last().is_some_and(|l| pos < l.position) {
            self.points.retain(|p| p.position < pos);
            self.tail = None;
        }
        let Some(last) = self.points.last().copied() else {
            self.points.push(pt);
            return;
        };
        if pos == last.position {
            if let Some(l) = self.points.last_mut() {
                l.value = value;
            }
            self.tail = None;
            return;
        }
        let dv = (value - last.value).abs();
        let reference = self.tail.unwrap_or(last);
        if dv < th.min_delta && (value - reference.value).abs() < th.min_delta {
            return;
        }
        if pos - last.position < th.min_ticks && dv < th.jump {
            self.tail = Some(pt);
            return;
        }
        if pos - reference.position > th.hold_gap && (reference.value - value).abs() >= th.min_delta {
            // Valore fermo durante la pausa: tenuto fino al nuovo punto.
            if reference.position > last.position {
                self.points.push(reference);
            }
            self.points.push(point(pos, reference.value));
        }
        self.tail = None;
        self.points.push(pt);
    }

    /// Punti finali (con l'eventuale ultimo campione scartato).
    fn finish(mut self, th: &Thinning) -> Vec<AutomationPoint> {
        if let (Some(t), Some(l)) = (self.tail.take(), self.points.last()) {
            if t.position > l.position && (t.value - l.value).abs() >= th.min_delta {
                self.points.push(t);
            }
        }
        self.points
    }
}

fn point(position: Tick, value: f32) -> AutomationPoint {
    AutomationPoint {
        position,
        value: if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 },
        tension: 0.0,
    }
}

/// Accumula i movimenti dei controlli durante la registrazione.
#[derive(Debug, Clone, Default)]
pub struct AutomationRecorder {
    pub thinning: Thinning,
    takes: Vec<Take>,
}

impl AutomationRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_thinning(thinning: Thinning) -> Self {
        Self {
            thinning,
            takes: Vec::new(),
        }
    }

    /// Registra `value` (normalizzato 0..=1 su `min..max`) per `target` alla posizione `pos`.
    /// `name`, `min` e `max` valgono alla prima registrazione del bersaglio.
    pub fn record(&mut self, target: &AutomationTarget, name: &str, min: f32, max: f32, value: f32, pos: Tick) {
        let th = self.thinning;
        let take = match self.takes.iter().position(|t| &t.target == target) {
            Some(i) => &mut self.takes[i],
            None => {
                self.takes.push(Take::new(target.clone(), name.to_string(), min, max));
                self.takes.last_mut().expect("appena inserito")
            }
        };
        take.push(&th, value, pos);
    }

    /// Come [`AutomationRecorder::record`] con un valore reale (mappato su `min..max`).
    pub fn record_real(&mut self, target: &AutomationTarget, name: &str, min: f32, max: f32, real: f32, pos: Tick) {
        self.record(target, name, min, max, normalize(real, min, max), pos);
    }

    pub fn is_empty(&self) -> bool {
        self.takes.iter().all(|t| t.points.is_empty())
    }

    /// Registrazioni in corso.
    pub fn takes(&self) -> &[Take] {
        &self.takes
    }

    /// Scarta tutto.
    pub fn clear(&mut self) {
        self.takes.clear();
    }

    /// Chiude la registrazione: un [`Command::RecordAutomation`] per bersaglio (da eseguire
    /// insieme in un `Batch`, un solo passo di undo). Il registratore si svuota.
    pub fn finish(&mut self) -> Vec<Command> {
        let th = self.thinning;
        std::mem::take(&mut self.takes)
            .into_iter()
            .filter_map(|t| {
                let (target, name, min, max) = (t.target.clone(), t.name.clone(), t.min, t.max);
                let points = t.finish(&th);
                (!points.is_empty()).then_some(Command::RecordAutomation {
                    target,
                    name: Some(name),
                    min,
                    max,
                    points,
                    track: None,
                })
            })
            .collect()
    }
}

/// Valore reale → normalizzato su `min..max` (limitato a 0..=1).
pub fn normalize(real: f32, min: f32, max: f32) -> f32 {
    let range = max - min;
    if range.abs() < f32::EPSILON || !real.is_finite() {
        return 0.0;
    }
    ((real - min) / range).clamp(0.0, 1.0)
}

/// Nome predefinito di una clip di automazione per il bersaglio.
pub fn default_name(p: &Project, target: &AutomationTarget) -> String {
    let ch = |c: &ChannelId| {
        p.channel(*c)
            .map(|x| x.name.clone())
            .unwrap_or_else(|| format!("canale {c}"))
    };
    let ins = |i: &u16| {
        p.insert(*i)
            .map(|x| x.name.clone())
            .unwrap_or_else(|| format!("insert {i}"))
    };
    match target {
        AutomationTarget::MasterVolume => "Volume master".into(),
        AutomationTarget::Tempo => "Tempo".into(),
        AutomationTarget::ChannelVolume { channel } => format!("{} · volume", ch(channel)),
        AutomationTarget::ChannelPan { channel } => format!("{} · pan", ch(channel)),
        AutomationTarget::ChannelPitch { channel } => format!("{} · pitch", ch(channel)),
        AutomationTarget::ChannelParam { channel, param } => format!("{} · {param}", ch(channel)),
        AutomationTarget::MixerVolume { insert } => format!("{} · volume", ins(insert)),
        AutomationTarget::MixerPan { insert } => format!("{} · pan", ins(insert)),
        AutomationTarget::EffectParam { insert, slot, param } => {
            format!("{} · FX {} · {param}", ins(insert), slot + 1)
        }
        AutomationTarget::EffectMix { insert, slot } => format!("{} · FX {} · mix", ins(insert), slot + 1),
    }
}

/// Applica [`Command::RecordAutomation`]: `points` in tick assoluti del brano, valori
/// normalizzati su `min..max`.
pub(crate) fn apply_record(
    p: &mut Project,
    target: &AutomationTarget,
    name: Option<&str>,
    (min, max): (f32, f32),
    points: &[AutomationPoint],
    track: Option<u16>,
) -> Result<Outcome, CoreError> {
    if points.is_empty() {
        return Err(CoreError::Invalid("registrazione di automazione senza punti".into()));
    }
    if !min.is_finite() || !max.is_finite() {
        return Err(CoreError::Invalid("range di automazione non valido".into()));
    }
    if track.is_some_and(|t| t as usize >= PLAYLIST_TRACKS) {
        return Err(CoreError::Invalid(format!("traccia >= {PLAYLIST_TRACKS}")));
    }
    let mut rec: Vec<AutomationPoint> = points.iter().map(|x| point(x.position, x.value)).collect();
    rec.sort_by_key(|x| x.position);
    let from = rec[0].position;
    let to = rec[rec.len() - 1].position;

    // Clip esistente dello stesso bersaglio che copre (o tocca) l'intervallo registrato.
    let existing = p.playlist.clips.iter().find_map(|c| {
        let ClipSource::Automation(aid) = c.source else {
            return None;
        };
        let a = p.automation(aid)?;
        (a.target == *target && c.position <= to && c.end() >= from).then_some((c.id, aid))
    });

    let Some((clip_id, aid)) = existing else {
        let id = AutomationId(p.alloc_id());
        let name = name.map(str::to_string).unwrap_or_else(|| default_name(p, target));
        p.automations.push(AutomationClip {
            id,
            name,
            target: target.clone(),
            min,
            max,
            points: rec
                .iter()
                .map(|x| AutomationPoint {
                    position: x.position - from,
                    ..*x
                })
                .collect(),
        });
        let track = match track {
            Some(t) => t,
            None => free_track(p, target, from, to.max(from + 1)),
        };
        let cid = ClipId(p.alloc_id());
        p.playlist.clips.push(PlaylistClip {
            id: cid,
            track,
            position: from,
            length: (to - from).max(1),
            offset: 0,
            source: ClipSource::Automation(id),
            muted: false,
        });
        return Ok(Outcome::Automation(id));
    };

    let clip = *p.playlist.clips.iter().find(|c| c.id == clip_id).expect("clip trovata");
    let a = p.automation(aid).expect("automazione trovata").clone();
    // Punti registrati nel range della clip esistente.
    for x in rec.iter_mut() {
        x.value = normalize(min + (max - min) * x.value, a.min, a.max);
    }
    let origin_old = clip.position as i64 - clip.offset as i64;
    let abs = |x: &AutomationPoint| origin_old + x.position as i64;
    let old_at = |t: Tick| a.value_at((t as i64 - origin_old) as f64).unwrap_or(0.0);
    let (from_i, to_i) = (from as i64, to as i64);

    let mut merged: Vec<(i64, AutomationPoint)> = Vec::new();
    let before: Vec<_> = a.points.iter().filter(|x| abs(x) < from_i).collect();
    let after: Vec<_> = a.points.iter().filter(|x| abs(x) > to_i).collect();
    for x in &before {
        merged.push((abs(x), **x));
    }
    if !before.is_empty() {
        merged.push((from_i, point(0, old_at(from))));
    }
    for x in &rec {
        merged.push((x.position as i64, *x));
    }
    if !after.is_empty() {
        merged.push((to_i, point(0, old_at(to))));
    }
    for x in &after {
        merged.push((abs(x), **x));
    }
    merged.dedup_by(|b, a| a.0 == b.0 && (a.1.value - b.1.value).abs() < 1e-6);

    let new_pos = clip.position.min(from);
    let new_end = clip.end().max(to).max(new_pos + 1);
    let origin_new = merged.iter().map(|m| m.0).min().unwrap_or(from_i).min(new_pos as i64);
    let pts = merged
        .into_iter()
        .map(|(t, x)| AutomationPoint {
            position: (t - origin_new) as Tick,
            ..x
        })
        .collect();
    p.automation_mut(aid).expect("automazione trovata").points = pts;
    let c = p
        .playlist
        .clips
        .iter_mut()
        .find(|c| c.id == clip_id)
        .expect("clip trovata");
    c.position = new_pos;
    c.length = new_end - new_pos;
    c.offset = (new_pos as i64 - origin_new) as Tick;
    Ok(Outcome::Automation(aid))
}

/// Traccia per una nuova clip in `[from, to)`: quella di un'altra clip dello stesso
/// bersaglio se libera, altrimenti la prima traccia libera.
fn free_track(p: &Project, target: &AutomationTarget, from: Tick, to: Tick) -> u16 {
    let busy = |t: u16| {
        p.playlist
            .clips
            .iter()
            .any(|c| c.track == t && c.position < to && c.end() > from)
    };
    let same_target = p.playlist.clips.iter().find_map(|c| match c.source {
        ClipSource::Automation(aid) if p.automation(aid).is_some_and(|a| a.target == *target) => Some(c.track),
        _ => None,
    });
    if let Some(t) = same_target.filter(|t| !busy(*t)) {
        return t;
    }
    (0..PLAYLIST_TRACKS as u16).find(|t| !busy(*t)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;

    fn vol() -> AutomationTarget {
        AutomationTarget::MixerVolume { insert: 1 }
    }

    fn values(a: &AutomationClip) -> Vec<(Tick, f32)> {
        a.points
            .iter()
            .map(|p| (p.position, (p.value * 1000.0).round() / 1000.0))
            .collect()
    }

    #[test]
    fn thinning_drops_small_and_close_changes() {
        let mut r = AutomationRecorder::new();
        let t = vol();
        r.record(&t, "v", 0.0, 1.0, 0.5, 100);
        // Variazione minima: ignorata.
        r.record(&t, "v", 0.0, 1.0, 0.501, 104);
        // Troppo vicino e senza salto: scartato (ma ricordato come coda).
        r.record(&t, "v", 0.0, 1.0, 0.55, 102);
        assert_eq!(r.takes()[0].points.len(), 1);
        // Troppo vicino ma con un salto: tenuto.
        r.record(&t, "v", 0.0, 1.0, 0.9, 103);
        assert_eq!(r.takes()[0].points.len(), 2);
        // Ritmo normale.
        for (i, v) in [0.85f32, 0.8, 0.75].iter().enumerate() {
            r.record(&t, "v", 0.0, 1.0, *v, 110 + i as Tick * 6);
        }
        // Campione vicino all'ultimo: scartato, poi recuperato alla fine.
        r.record(&t, "v", 0.0, 1.0, 0.7, 124);
        let cmds = r.finish();
        assert!(r.is_empty());
        let [Command::RecordAutomation { points, min, max, .. }] = cmds.as_slice() else {
            panic!("{cmds:?}")
        };
        assert_eq!((*min, *max), (0.0, 1.0));
        let got: Vec<_> = points.iter().map(|p| (p.position, p.value)).collect();
        assert_eq!(
            got,
            vec![(100, 0.5), (103, 0.9), (110, 0.85), (116, 0.8), (122, 0.75), (124, 0.7)]
        );
    }

    #[test]
    fn pause_holds_value_and_loop_overwrites() {
        let mut r = AutomationRecorder::new();
        let t = vol();
        r.record(&t, "v", 0.0, 1.0, 0.2, 0);
        // Nessun movimento per 2 battute, poi un cambio: gradino, non rampa.
        r.record(&t, "v", 0.0, 1.0, 0.8, 768);
        let pts: Vec<_> = r.takes()[0].points.iter().map(|p| (p.position, p.value)).collect();
        assert_eq!(pts, vec![(0, 0.2), (768, 0.2), (768, 0.8)]);
        // Il loop torna indietro: il nuovo passaggio sostituisce i punti successivi.
        r.record(&t, "v", 0.0, 1.0, 0.4, 500);
        let pts: Vec<_> = r.takes()[0].points.iter().map(|p| (p.position, p.value)).collect();
        assert_eq!(pts, vec![(0, 0.2), (500, 0.2), (500, 0.4)]);
        // Valori reali normalizzati sul range.
        let pan = AutomationTarget::MixerPan { insert: 1 };
        r.record_real(&pan, "p", -1.0, 1.0, 0.0, 10);
        assert_eq!(r.takes()[1].points[0].value, 0.5);
        assert_eq!(r.finish().len(), 2);
    }

    #[test]
    fn new_clip_is_placed_at_recorded_position_on_free_track() {
        let mut p = Project::new("t");
        let pat = p.patterns[0].id;
        Command::PlaceClip {
            track: 0,
            position: 0,
            length: Some(4000),
            source: ClipSource::Pattern(pat),
        }
        .apply(&mut p)
        .unwrap();
        let mut r = AutomationRecorder::new();
        r.record(&vol(), "Vol", 0.0, 1.25, 0.2, 384);
        r.record(&vol(), "Vol", 0.0, 1.25, 0.6, 480);
        let mut h = History::default();
        let out = h.execute(&mut p, &Command::Batch { commands: r.finish() }).unwrap();
        let Outcome::Many(o) = out else { panic!() };
        let Outcome::Automation(aid) = o[0] else { panic!() };
        let a = p.automation(aid).unwrap();
        assert_eq!(a.name, "Vol");
        assert_eq!((a.min, a.max), (0.0, 1.25));
        // Due cambi lontani (es. due rilasci di una manopola): gradino, non rampa.
        assert_eq!(values(a), vec![(0, 0.2), (96, 0.2), (96, 0.6)]);
        let c = p
            .playlist
            .clips
            .iter()
            .find(|c| c.source == ClipSource::Automation(aid))
            .unwrap();
        assert_eq!((c.track, c.position, c.length, c.offset), (1, 384, 96, 0));
        // Un solo passo di undo.
        h.undo(&mut p);
        assert!(p.automations.is_empty());
        assert_eq!(p.playlist.clips.len(), 1);
    }

    #[test]
    fn recording_over_existing_clip_replaces_range_and_extends() {
        let mut p = Project::new("t");
        let t = vol();
        // Clip esistente da 100 a 500: rampa 0 → 1 (range 0..2).
        let rec = |pts: &[(Tick, f32)], min: f32, max: f32| Command::RecordAutomation {
            target: t.clone(),
            name: None,
            min,
            max,
            points: pts.iter().map(|(t, v)| point(*t, *v)).collect(),
            track: None,
        };
        let Outcome::Automation(aid) = rec(&[(100, 0.0), (300, 0.5), (500, 1.0)], 0.0, 2.0)
            .apply(&mut p)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(p.automation(aid).unwrap().name, "Insert 1 · volume");
        // Nuova passata 200..250 con range diverso (0..1): valori riportati su 0..2.
        let out = rec(&[(200, 1.0), (250, 0.0)], 0.0, 1.0).apply(&mut p).unwrap();
        assert_eq!(out, Outcome::Automation(aid));
        assert_eq!(p.automations.len(), 1);
        let a = p.automation(aid).unwrap();
        assert_eq!(
            values(a),
            vec![
                (0, 0.0),
                (100, 0.25), // valore vecchio tenuto fino all'inizio della passata
                (100, 0.5),
                (150, 0.0),
                (150, 0.375), // ...e ripreso alla fine
                (200, 0.5),
                (400, 1.0)
            ]
        );
        // Passata oltre la fine: la clip si allunga.
        rec(&[(450, 0.5), (700, 0.5)], 0.0, 2.0).apply(&mut p).unwrap();
        let c = p.playlist.clips[0];
        assert_eq!((c.position, c.length, c.offset), (100, 600, 0));
        let a = p.automation(aid).unwrap();
        assert_eq!(a.points.last().unwrap().position, 600);
        assert!((a.value_at(400.0).unwrap() - 0.5).abs() < 1e-6);
        // Passata prima dell'inizio ma sovrapposta: la clip parte prima.
        rec(&[(50, 1.0), (120, 1.0)], 0.0, 2.0).apply(&mut p).unwrap();
        let c = p.playlist.clips[0];
        assert_eq!((c.position, c.length, c.offset), (50, 650, 0));
        let a = p.automation(aid).unwrap();
        assert_eq!(a.value_at(0.0), Some(1.0));
        // Dopo il punto registrato (120) torna la curva vecchia.
        assert!((a.value_at(70.0 + 1.0).unwrap() - 0.0).abs() < 0.2);
        // Passata lontana: nuova clip, stessa traccia.
        rec(&[(5000, 0.3)], 0.0, 2.0).apply(&mut p).unwrap();
        assert_eq!(p.automations.len(), 2);
        assert_eq!(p.playlist.clips[1].track, p.playlist.clips[0].track);
        assert_eq!(p.playlist.clips[1].position, 5000);
    }

    #[test]
    fn existing_clip_with_offset_keeps_alignment() {
        let mut p = Project::new("t");
        let t = vol();
        let Outcome::Automation(aid) = Command::AddAutomation {
            name: "a".into(),
            target: t.clone(),
            min: 0.0,
            max: 1.0,
            points: vec![point(0, 0.0), point(1000, 1.0)],
        }
        .apply(&mut p)
        .unwrap() else {
            panic!()
        };
        // Clip a 2000 che mostra la sorgente da 500 (offset) per 400 tick.
        Command::PlaceClip {
            track: 3,
            position: 2000,
            length: Some(400),
            source: ClipSource::Automation(aid),
        }
        .apply(&mut p)
        .unwrap();
        let cid = p.playlist.clips[0].id;
        Command::MoveClip {
            clip: cid,
            track: None,
            position: None,
            length: None,
            offset: Some(500),
        }
        .apply(&mut p)
        .unwrap();
        let before = |p: &Project, song: f64| {
            let c = p.playlist.clips[0];
            p.automation(aid)
                .unwrap()
                .value_at(song - c.position as f64 + c.offset as f64)
                .unwrap()
        };
        let v2100 = before(&p, 2100.0);
        Command::RecordAutomation {
            target: t,
            name: None,
            min: 0.0,
            max: 1.0,
            points: vec![point(2200, 0.0), point(2300, 0.0)],
            track: None,
        }
        .apply(&mut p)
        .unwrap();
        let c = p.playlist.clips[0];
        assert_eq!((c.position, c.length, c.track), (2000, 400, 3));
        // Fuori dall'intervallo la curva non cambia, dentro vale il registrato.
        assert!((before(&p, 2100.0) - v2100).abs() < 1e-6);
        assert_eq!(before(&p, 2250.0), 0.0);
        assert!(before(&p, 2350.0) > 0.8);
    }

    #[test]
    fn invalid_record_is_rejected() {
        let mut p = Project::new("t");
        let cmd = Command::RecordAutomation {
            target: vol(),
            name: None,
            min: 0.0,
            max: 1.0,
            points: vec![],
            track: None,
        };
        assert!(cmd.apply(&mut p).is_err());
        let json = serde_json::to_string(&Command::RecordAutomation {
            target: vol(),
            name: None,
            min: 0.0,
            max: 1.0,
            points: vec![point(1, 0.5)],
            track: Some(2),
        })
        .unwrap();
        assert!(json.contains("\"op\":\"record_automation\""));
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(back.label(), "record_automation");
    }
}
