//! Sezione "Campioni" del browser: albero delle cartelle dell'utente con i file
//! audio supportati, ascolto con un clic (caricamento in background, riproduzione
//! sul master del motore) e trascinamento verso Channel Rack e Playlist.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align2, FontId, Id, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui};
use mfc_core::{ChannelKind, ClipSource, Command, Outcome, Tick, PLAYLIST_TRACKS};
use mfc_plugins::SharedSample;

use crate::state::Studio;
use crate::theme::{self, pal};
use crate::views::widgets::{self, Icon};

/// Estensioni elencate: solo quelle che `mfc_plugins::load_sample` sa leggere (WAV via hound).
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "wave"];
/// Massimo numero di voci mostrate per cartella (cartelle enormi).
const MAX_ENTRIES: usize = 2000;
const ROW_H: f32 = 20.0;

/// Payload del drag & drop di un campione dal browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleDrag {
    pub path: PathBuf,
}

/// `true` se il file ha un'estensione audio supportata.
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Voce di una cartella.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// Contenuto di una cartella: sottocartelle e file audio, nascosti esclusi,
/// cartelle prima e ordine alfabetico senza distinzione di maiuscole.
pub fn list_dir(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = e.path();
            let is_dir = path.is_dir();
            (is_dir || is_audio_file(&path)).then_some(Entry { path, name, is_dir })
        })
        .take(MAX_ENTRIES)
        .collect();
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// Durata in secondi di un WAV letta dall'intestazione RIFF (senza decodificare l'audio).
pub fn wav_duration_secs(path: &Path) -> Option<f64> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = f.read(&mut buf).ok()?;
    let len = std::fs::metadata(path).ok()?.len();
    wav_duration_from_header(&buf[..n], len)
}

fn wav_duration_from_header(b: &[u8], file_len: u64) -> Option<f64> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let u32_at = |i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let mut pos = 12;
    let mut byte_rate = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32_at(pos + 4)? as usize;
        let body = pos + 8;
        if id == b"fmt " {
            byte_rate = u32_at(body + 8).filter(|r| *r > 0);
        } else if id == b"data" {
            // Dimensione dichiarata, limitata a quanto c'è davvero nel file.
            let avail = file_len.saturating_sub(body as u64);
            let data = (size as u64).min(avail);
            return byte_rate.map(|r| data as f64 / r as f64);
        }
        pos = body + size + (size & 1);
    }
    None
}

/// Esito di un caricamento in background.
type LoadResult = Result<SharedSample, String>;

/// Stato runtime del browser dei campioni.
#[derive(Default)]
pub struct SampleBrowser {
    expanded: HashSet<PathBuf>,
    listings: HashMap<PathBuf, Vec<Entry>>,
    loading: Option<(PathBuf, Receiver<LoadResult>)>,
    /// Ultimo campione caricato (riascolto immediato).
    cache: Option<(PathBuf, SharedSample)>,
    /// Campione in ascolto e istante previsto di fine.
    playing: Option<(PathBuf, Instant)>,
}

impl SampleBrowser {
    fn listing(&mut self, dir: &Path) -> &[Entry] {
        self.listings
            .entry(dir.to_path_buf())
            .or_insert_with(|| list_dir(dir))
            .as_slice()
    }

    /// Apre una cartella nell'albero.
    pub fn expand(&mut self, dir: PathBuf) {
        self.expanded.insert(dir);
    }

    /// Dimentica gli elenchi letti (dopo "Aggiorna").
    pub fn refresh(&mut self) {
        self.listings.clear();
    }

    pub fn is_playing(&self, path: &Path) -> bool {
        self.playing.as_ref().is_some_and(|(p, _)| p == path)
    }

    pub fn is_loading(&self, path: &Path) -> bool {
        self.loading.as_ref().is_some_and(|(p, _)| p == path)
    }

    /// Campione già caricato per il percorso, se in cache.
    pub fn cached(&self, path: &Path) -> Option<SharedSample> {
        self.cache.as_ref().filter(|(p, _)| p == path).map(|(_, s)| s.clone())
    }
}

/// Ascolta un campione: dalla cache, oppure caricandolo in un thread separato.
/// Un secondo clic sul campione in ascolto lo ferma.
pub fn audition(studio: &mut Studio, path: &Path) {
    let sb = &mut studio.ui.samples;
    if sb.is_playing(path) {
        studio.engine.stop_audition();
        sb.playing = None;
        return;
    }
    if let Some(sample) = sb.cached(path) {
        start_playback(studio, path.to_path_buf(), sample);
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let p = path.to_path_buf();
    std::thread::spawn(move || {
        let res = mfc_plugins::load_sample(&p).map_err(|e| e.to_string());
        let _ = tx.send(res);
    });
    sb.loading = Some((path.to_path_buf(), rx));
}

fn start_playback(studio: &mut Studio, path: PathBuf, sample: SharedSample) {
    let secs = sample.len() as f64 / sample.sample_rate.max(1.0) as f64;
    studio.engine.audition(sample.clone());
    let sb = &mut studio.ui.samples;
    sb.playing = Some((path.clone(), Instant::now() + Duration::from_secs_f64(secs.min(3600.0))));
    sb.cache = Some((path, sample));
}

/// Da chiamare a ogni frame: completa i caricamenti, aggiorna lo stato di ascolto,
/// disegna l'etichetta del campione trascinato.
pub fn poll(ctx: &egui::Context, studio: &mut Studio) {
    if let Some((path, rx)) = studio.ui.samples.loading.take() {
        match rx.try_recv() {
            Ok(Ok(sample)) => start_playback(studio, path, sample),
            Ok(Err(e)) => studio.status = format!("Campione non leggibile: {e}"),
            Err(TryRecvError::Empty) => {
                studio.ui.samples.loading = Some((path, rx));
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            Err(TryRecvError::Disconnected) => {}
        }
    }
    if let Some((_, end)) = &studio.ui.samples.playing {
        let now = Instant::now();
        if now >= *end {
            studio.ui.samples.playing = None;
        } else {
            ctx.request_repaint_after((*end - now).min(Duration::from_millis(250)));
        }
    }
    drag_preview(ctx);
}

/// Etichetta che segue il puntatore durante il trascinamento di un campione.
fn drag_preview(ctx: &egui::Context) {
    let Some(drag) = egui::DragAndDrop::payload::<SampleDrag>(ctx) else {
        return;
    };
    let Some(pos) = ctx.pointer_latest_pos() else { return };
    let name = drag
        .path
        .file_name()
        .map_or_else(|| drag.path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("sample_drag")));
    let font = FontId::proportional(11.5);
    let galley = painter.layout_no_wrap(name, font, pal::TEXT());
    let r = Rect::from_min_size(pos + vec2(14.0, 8.0), galley.size() + vec2(30.0, 10.0));
    painter.rect_filled(r, 4.0, pal::BG_RAISED());
    painter.rect_stroke(r, 4.0, Stroke::new(1.0, pal::ACCENT()), StrokeKind::Inside);
    widgets::paint_icon(
        &painter,
        Rect::from_center_size(pos2(r.left() + 12.0, r.center().y), vec2(12.0, 12.0)),
        Icon::Wave,
        pal::ACCENT(),
    );
    painter.galley(pos2(r.left() + 24.0, r.top() + 5.0), galley, pal::TEXT());
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
}

/// Zona di rilascio per i campioni trascinati dal browser. Evidenzia `rect` mentre un
/// campione ci passa sopra e, al rilascio, restituisce percorso e posizione.
pub fn drop_target(ui: &Ui, rect: Rect, hint: &str) -> Option<(PathBuf, Pos2)> {
    let ctx = ui.ctx();
    egui::DragAndDrop::payload::<SampleDrag>(ctx)?;
    let pos = ctx.pointer_latest_pos()?;
    if !rect.contains(pos) || !ui.rect_contains_pointer(rect) {
        return None;
    }
    // Sopra al contenuto della vista, che può essere disegnato dopo.
    let p = ctx
        .layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("sample_drop")))
        .with_clip_rect(rect.intersect(ui.clip_rect()));
    p.rect_filled(rect.shrink(2.0), 6.0, pal::ACCENT().gamma_multiply(0.08));
    p.rect_stroke(
        rect.shrink(2.0),
        6.0,
        Stroke::new(2.0, pal::ACCENT()),
        StrokeKind::Inside,
    );
    p.text(
        pos2(rect.center().x, rect.bottom() - 22.0),
        Align2::CENTER_CENTER,
        hint,
        theme::bold_font(12.0),
        pal::ACCENT(),
    );
    if ctx.input(|i| i.pointer.any_released()) {
        let drag = egui::DragAndDrop::take_payload::<SampleDrag>(ctx)?;
        return Some((drag.path.clone(), pos));
    }
    None
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "Campione".to_string(), |n| n.to_string_lossy().into_owned())
}

/// Aggiunge un canale Sampler con il campione indicato (drop sul Channel Rack o sulla finestra).
pub fn add_sample_channel(studio: &mut Studio, path: &Path) {
    let name = display_name(path);
    let Some(plugin) = mfc_plugins::default_instance(mfc_plugins::ids::SAMPLER) else {
        studio.status = "Plugin sampler non disponibile".into();
        return;
    };
    let Some(Outcome::Channel(channel)) = studio.exec(Command::AddChannel {
        name: name.clone(),
        kind: ChannelKind::Sampler,
        plugin,
        mixer_insert: None,
    }) else {
        return;
    };
    let sample = Command::SetChannelSample {
        channel,
        path: Some(path.display().to_string()),
    };
    if studio.exec(sample).is_some() {
        studio.selected_channel = Some(channel);
        studio.status = format!("Campione aggiunto: {name}");
    }
}

/// Crea un canale Audio Clip con il campione e lo piazza in Playlist alla posizione data,
/// lungo quanto il campione al tempo corrente.
pub fn add_audio_clip(studio: &mut Studio, path: &Path, track: u16, position: Tick) {
    let name = display_name(path);
    let Some(plugin) = mfc_plugins::default_instance(mfc_plugins::ids::AUDIO_CLIP) else {
        studio.status = "Plugin audio clip non disponibile".into();
        return;
    };
    let Some(Outcome::Channel(channel)) = studio.exec(Command::AddChannel {
        name: name.clone(),
        kind: ChannelKind::AudioClip,
        plugin,
        mixer_insert: None,
    }) else {
        return;
    };
    studio.exec(Command::SetChannelSample {
        channel,
        path: Some(path.display().to_string()),
    });
    let secs = studio
        .ui
        .samples
        .cached(path)
        .map(|s| s.len() as f64 / s.sample_rate.max(1.0) as f64)
        .or_else(|| wav_duration_secs(path));
    let p = &studio.project;
    let length = secs.map(|s| (p.time.seconds_to_ticks(s, p.tempo).ceil() as Tick).max(1));
    let placed = studio.exec(Command::PlaceClip {
        track: track.min(PLAYLIST_TRACKS as u16 - 1),
        position,
        length,
        source: ClipSource::Audio(channel),
    });
    if placed.is_some() {
        studio.status = format!("Audio clip: {name} (traccia {})", track + 1);
    }
}

/// Pulsante per aprire/chiudere il browser (freccia «/»).
pub fn collapse_button(ui: &mut Ui, open: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 4.0, pal::BG_HOVER());
    }
    let c = if resp.hovered() { pal::ACCENT() } else { pal::TEXT_DIM() };
    let m = rect.center();
    let d = if open { -1.0 } else { 1.0 };
    for dx in [-2.5f32, 2.5] {
        let tip = m + vec2(dx + d * 2.5, 0.0);
        p.line_segment([tip - vec2(d * 4.0, 4.0), tip], Stroke::new(1.5, c));
        p.line_segment([tip - vec2(d * 4.0, -4.0), tip], Stroke::new(1.5, c));
    }
    resp.on_hover_text(if open {
        "Chiudi il browser (F8)"
    } else {
        "Apri il browser (F8)"
    })
}

/// Azioni raccolte durante il disegno dell'albero (applicate dopo, senza prestiti aperti).
enum Action {
    Toggle(PathBuf),
    Play(PathBuf),
    Remove(PathBuf),
    AddToRack(PathBuf),
}

/// Contenuto della sezione "Campioni" del browser.
pub fn section(ui: &mut Ui, studio: &mut Studio) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        if ui
            .small_button("Aggiungi cartella…")
            .on_hover_text("Aggiunge una cartella di campioni al browser")
            .clicked()
        {
            pick_folder(studio);
        }
        if ui
            .small_button("Aggiorna")
            .on_hover_text("Rilegge il contenuto delle cartelle")
            .clicked()
        {
            studio.ui.samples.refresh();
        }
    });
    let roots = studio.ui.settings.sample_folders.clone();
    if roots.is_empty() {
        ui.label(
            RichText::new("   Nessuna cartella: aggiungine una")
                .size(11.0)
                .color(pal::TEXT_FAINT()),
        );
    }
    let mut actions = Vec::new();
    for root in &roots {
        let name = root
            .file_name()
            .map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        let open = studio.ui.samples.expanded.contains(root);
        let resp = row(ui, 0, RowKind::Folder { open, root: true }, &name, false, false)
            .on_hover_text(root.display().to_string());
        if resp.clicked() {
            actions.push(Action::Toggle(root.clone()));
        }
        resp.context_menu(|ui| {
            if ui.button("Rimuovi dalla lista").clicked() {
                actions.push(Action::Remove(root.clone()));
                ui.close();
            }
        });
        if open {
            tree(ui, &mut studio.ui.samples, root, 1, &mut actions);
        }
    }
    for a in actions {
        match a {
            Action::Toggle(p) => {
                let ex = &mut studio.ui.samples.expanded;
                if !ex.remove(&p) {
                    ex.insert(p);
                }
            }
            Action::Play(p) => audition(studio, &p),
            Action::AddToRack(p) => add_sample_channel(studio, &p),
            Action::Remove(p) => {
                studio.ui.settings.sample_folders.retain(|x| *x != p);
                studio.ui.save();
            }
        }
    }
}

fn pick_folder(studio: &mut Studio) {
    let mut dlg = rfd::FileDialog::new().set_title("Aggiungi cartella di campioni");
    if let Some(first) = studio.ui.settings.sample_folders.first() {
        dlg = dlg.set_directory(first);
    }
    if let Some(dir) = dlg.pick_folder() {
        if studio.ui.settings.add_sample_folder(dir.clone()) {
            studio.ui.samples.expanded.insert(dir);
            studio.ui.save();
        }
    }
}

fn tree(ui: &mut Ui, sb: &mut SampleBrowser, dir: &Path, depth: usize, actions: &mut Vec<Action>) {
    let entries = sb.listing(dir).to_vec();
    if entries.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(14.0 + depth as f32 * 12.0);
            ui.label(RichText::new("vuota").size(10.5).italics().color(pal::TEXT_FAINT()));
        });
    }
    for e in entries {
        if e.is_dir {
            let open = sb.expanded.contains(&e.path);
            let resp = row(ui, depth, RowKind::Folder { open, root: false }, &e.name, false, false);
            if resp.clicked() {
                actions.push(Action::Toggle(e.path.clone()));
            }
            if open {
                tree(ui, sb, &e.path, depth + 1, actions);
            }
        } else {
            let playing = sb.is_playing(&e.path);
            let loading = sb.is_loading(&e.path);
            let resp = row(ui, depth, RowKind::File, &e.name, playing, loading).on_hover_text(
                "Clic: ascolta · trascina sul Channel Rack (nuovo Sampler) o sulla Playlist (audio clip)",
            );
            resp.dnd_set_drag_payload(SampleDrag { path: e.path.clone() });
            if resp.clicked() {
                actions.push(Action::Play(e.path.clone()));
            }
            resp.context_menu(|ui| {
                if ui.button("Ascolta").clicked() {
                    actions.push(Action::Play(e.path.clone()));
                    ui.close();
                }
                if ui.button("Nuovo canale Sampler").clicked() {
                    actions.push(Action::AddToRack(e.path.clone()));
                    ui.close();
                }
            });
        }
    }
}

#[derive(Clone, Copy)]
enum RowKind {
    Folder { open: bool, root: bool },
    File,
}

fn row(ui: &mut Ui, depth: usize, kind: RowKind, label: &str, playing: bool, loading: bool) -> Response {
    let sense = match kind {
        RowKind::File => Sense::click_and_drag(),
        RowKind::Folder { .. } => Sense::click(),
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
    let p = ui.painter();
    if playing {
        p.rect_filled(rect, 4.0, theme::mix(pal::BG_RAISED(), pal::ACCENT(), 0.14));
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, pal::BG_RAISED());
    }
    let x = rect.left() + 8.0 + depth as f32 * 12.0;
    let cy = rect.center().y;
    let dim = if resp.hovered() { pal::TEXT() } else { pal::TEXT_DIM() };
    match kind {
        RowKind::Folder { open, root } => {
            let a = pos2(x + 2.0, cy);
            let tri = if open {
                vec![a + vec2(-3.0, -1.5), a + vec2(3.0, -1.5), a + vec2(0.0, 2.5)]
            } else {
                vec![a + vec2(-1.5, -3.0), a + vec2(2.5, 0.0), a + vec2(-1.5, 3.0)]
            };
            p.add(egui::Shape::convex_polygon(tri, dim, Stroke::NONE));
            let col = if root {
                pal::ACCENT()
            } else {
                theme::mix(pal::YELLOW(), pal::TEXT_DIM(), 0.35)
            };
            widgets::paint_icon(
                p,
                Rect::from_center_size(pos2(x + 15.0, cy), vec2(12.0, 12.0)),
                Icon::Folder,
                col,
            );
        }
        RowKind::File => {
            let col = if playing { pal::ACCENT() } else { pal::BLUE() };
            widgets::paint_icon(
                p,
                Rect::from_center_size(pos2(x + 15.0, cy), vec2(12.0, 12.0)),
                if playing { Icon::Play } else { Icon::Wave },
                col,
            );
        }
    }
    let tc = if playing {
        pal::TEXT_STRONG()
    } else if resp.hovered() {
        pal::TEXT()
    } else {
        theme::mix(pal::TEXT(), pal::TEXT_DIM(), 0.4)
    };
    let clip = Rect::from_min_max(pos2(x + 24.0, rect.top()), rect.right_bottom() - vec2(4.0, 0.0));
    ui.painter().with_clip_rect(clip.intersect(ui.clip_rect())).text(
        pos2(x + 25.0, cy),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.5),
        tc,
    );
    if loading {
        ui.painter().text(
            pos2(rect.right() - 6.0, cy),
            Align2::RIGHT_CENTER,
            "…",
            FontId::proportional(11.0),
            pal::TEXT_FAINT(),
        );
        ui.ctx().request_repaint();
    }
    if matches!(kind, RowKind::File) && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let d = std::env::temp_dir().join(format!("mfc_samples_{tag}_{}_{nanos}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// WAV PCM 16 bit minimale.
    fn wav_bytes(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
        let block = channels as u32 * 2;
        let data = frames * block;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * block).to_le_bytes());
        b.extend_from_slice(&(block as u16).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        b.resize(b.len() + data as usize, 0);
        b
    }

    #[test]
    fn audio_extensions() {
        assert!(is_audio_file(Path::new("/a/Kick.WAV")));
        assert!(is_audio_file(Path::new("x.wav")));
        assert!(!is_audio_file(Path::new("x.mp3")), "il loader legge solo WAV");
        assert!(!is_audio_file(Path::new("x")));
    }

    #[test]
    fn listing_filters_and_sorts() {
        let d = temp_dir("list");
        std::fs::create_dir(d.join("zeta")).unwrap();
        std::fs::create_dir(d.join("Alfa")).unwrap();
        std::fs::create_dir(d.join(".nascosta")).unwrap();
        std::fs::write(d.join("b.wav"), b"").unwrap();
        std::fs::write(d.join("A.WAV"), b"").unwrap();
        std::fs::write(d.join("note.txt"), b"").unwrap();
        let names: Vec<String> = list_dir(&d).into_iter().map(|e| e.name).collect();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(names, ["Alfa", "zeta", "A.WAV", "b.wav"]);
        assert!(list_dir(Path::new("/non/esiste/proprio")).is_empty());
    }

    #[test]
    fn wav_header_duration() {
        let b = wav_bytes(48_000, 2, 24_000);
        let len = b.len() as u64;
        assert_eq!(wav_duration_from_header(&b, len), Some(0.5));
        // File troncato: si conta solo l'audio presente.
        assert_eq!(wav_duration_from_header(&b[..44 + 48_000], 44 + 48_000), Some(0.25));
        assert_eq!(wav_duration_from_header(b"not a wav at all", 16), None);
        let d = temp_dir("dur");
        let p = d.join("t.wav");
        std::fs::write(&p, wav_bytes(44_100, 1, 44_100)).unwrap();
        let secs = wav_duration_secs(&p);
        // Il loader vero deve accettare lo stesso file.
        let loaded = mfc_plugins::load_sample(&p).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(secs, Some(1.0));
        assert_eq!(loaded.len(), 44_100);
    }
}
