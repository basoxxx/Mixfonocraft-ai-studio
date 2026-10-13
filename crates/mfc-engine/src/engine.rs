//! Motore audio: transport, sequencer sample-accurate, mixer con mandate ed effetti.
//!
//! `Engine::process` è real-time safe: nessuna allocazione né lock. I comandi arrivano
//! da un [`EngineHandle`] tramite canale; i `Prepared` sostituiti tornano indietro su un
//! canale "garbage" per essere deallocati fuori dal thread audio.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use mfc_core::{ChannelId, PatternId, Project, Tick};
use mfc_plugins::NoteOn;

use crate::prepared::{AutoTarget, NoteEvent, Prepared};

/// Dimensione massima di un sotto-blocco interno (risoluzione di automazioni e tempo).
pub const CHUNK: usize = 64;
/// Massimo numero di note attive tracciate dal sequencer.
const MAX_ACTIVE: usize = 4096;
/// Bit che marca le voci di anteprima (non generate dal sequencer).
const PREVIEW_BIT: u64 = 1 << 63;

/// Modalità di riproduzione (pulsante PAT/SONG di FL Studio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Song,
    Pattern(PatternId),
}

/// Comandi verso il thread audio.
pub enum EngineCommand {
    Play,
    Stop,
    Pause,
    Seek(f64),
    SetMode(PlayMode),
    /// Loop in Song mode (tick inizio, tick fine). `None` = nessun loop.
    SetLoop(Option<(f64, f64)>),
    Install(Box<Prepared>),
    PreviewOn {
        channel: ChannelId,
        key: u8,
        velocity: f32,
    },
    PreviewOff {
        channel: ChannelId,
        key: u8,
    },
    AllNotesOff,
}

/// Stato condiviso letto da GUI/MCP (atomics, aggiornati dal thread audio).
#[derive(Debug)]
pub struct SharedState {
    playing: AtomicBool,
    position_bits: AtomicU64,
    tempo_bits: AtomicU64,
    /// Picchi (L,R) per insert, f32 in bit.
    meters: Vec<AtomicU32>,
    /// Picchi (L,R) dell'uscita finale.
    out_meter: [AtomicU32; 2],
    /// Carico CPU levigato (tempo di elaborazione / tempo del blocco), f32 in bit.
    cpu_bits: AtomicU32,
}

impl SharedState {
    fn new(n_inserts: usize) -> Self {
        Self {
            playing: AtomicBool::new(false),
            position_bits: AtomicU64::new(0f64.to_bits()),
            tempo_bits: AtomicU64::new(140f64.to_bits()),
            meters: (0..n_inserts * 2).map(|_| AtomicU32::new(0)).collect(),
            out_meter: [AtomicU32::new(0), AtomicU32::new(0)],
            cpu_bits: AtomicU32::new(0),
        }
    }
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
    /// Posizione corrente in tick.
    pub fn position(&self) -> f64 {
        f64::from_bits(self.position_bits.load(Ordering::Relaxed))
    }
    pub fn tempo(&self) -> f64 {
        f64::from_bits(self.tempo_bits.load(Ordering::Relaxed))
    }
    /// Picco (L,R) dell'insert e azzera (lettura "consuma" il picco).
    pub fn take_meter(&self, insert: usize) -> (f32, f32) {
        let get = |i: usize| {
            self.meters
                .get(i)
                .map_or(0.0, |a| f32::from_bits(a.swap(0, Ordering::Relaxed)))
        };
        (get(insert * 2), get(insert * 2 + 1))
    }
    pub fn take_output_meter(&self) -> (f32, f32) {
        (
            f32::from_bits(self.out_meter[0].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.out_meter[1].swap(0, Ordering::Relaxed)),
        )
    }
    pub fn insert_count(&self) -> usize {
        self.meters.len() / 2
    }
    /// Carico CPU stimato del thread audio (0..1, può superare 1 se il motore è in ritardo).
    pub fn cpu_load(&self) -> f32 {
        f32::from_bits(self.cpu_bits.load(Ordering::Relaxed))
    }
}

fn store_peak(a: &AtomicU32, v: f32) {
    let cur = f32::from_bits(a.load(Ordering::Relaxed));
    if v > cur {
        a.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// Lato controllo del motore (thread GUI/MCP).
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<EngineCommand>,
    garbage: Receiver<Box<Prepared>>,
    pub shared: Arc<SharedState>,
    sample_rate: f32,
}

impl EngineHandle {
    fn send(&self, cmd: EngineCommand) -> bool {
        self.collect_garbage();
        match self.tx.try_send(cmd) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }
    /// Dealloca i `Prepared` dismessi dal thread audio.
    pub fn collect_garbage(&self) {
        while self.garbage.try_recv().is_ok() {}
    }
    pub fn play(&self) -> bool {
        self.send(EngineCommand::Play)
    }
    pub fn stop(&self) -> bool {
        self.send(EngineCommand::Stop)
    }
    pub fn pause(&self) -> bool {
        self.send(EngineCommand::Pause)
    }
    pub fn seek(&self, tick: f64) -> bool {
        self.send(EngineCommand::Seek(tick))
    }
    pub fn set_mode(&self, mode: PlayMode) -> bool {
        self.send(EngineCommand::SetMode(mode))
    }
    pub fn set_loop(&self, range: Option<(f64, f64)>) -> bool {
        self.send(EngineCommand::SetLoop(range))
    }
    /// Ricompila il progetto (sul thread chiamante) e lo installa nel motore.
    /// I generatori/effetti strutturalmente invariati vengono riusati (le code non si interrompono).
    pub fn load_project(&self, project: &Project) -> bool {
        let prepared = Prepared::build(project, self.sample_rate);
        self.send(EngineCommand::Install(Box::new(prepared)))
    }
    pub fn preview_on(&self, channel: ChannelId, key: u8, velocity: f32) -> bool {
        self.send(EngineCommand::PreviewOn { channel, key, velocity })
    }
    pub fn preview_off(&self, channel: ChannelId, key: u8) -> bool {
        self.send(EngineCommand::PreviewOff { channel, key })
    }
    pub fn all_notes_off(&self) -> bool {
        self.send(EngineCommand::AllNotesOff)
    }
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
}

#[derive(Clone, Copy)]
struct Active {
    chan: usize,
    voice: u64,
    off: f64,
}

/// Il motore audio.
pub struct Engine {
    prep: Box<Prepared>,
    rx: Receiver<EngineCommand>,
    garbage: Sender<Box<Prepared>>,
    shared: Arc<SharedState>,
    playing: bool,
    mode: PlayMode,
    loop_range: Option<(f64, f64)>,
    /// Posizione in tick (song o locale al pattern).
    pos: f64,
    next_event: usize,
    active: Vec<Active>,
    voice_counter: u64,
    tempo: f64,
    master_gain: f32,
    // Buffer (CHUNK) per insert e scratch.
    ins_l: Vec<[f32; CHUNK]>,
    ins_r: Vec<[f32; CHUNK]>,
    ch_l: [f32; CHUNK],
    ch_r: [f32; CHUNK],
    dry_l: [f32; CHUNK],
    dry_r: [f32; CHUNK],
    /// Cattura post-fader per insert (solo render offline degli stem).
    pub(crate) capture: Option<Vec<(Vec<f32>, Vec<f32>)>>,
}

impl Engine {
    /// Crea motore e handle a partire da un progetto.
    pub fn new(project: &Project, sample_rate: f32) -> (Engine, EngineHandle) {
        let prep = Box::new(Prepared::build(project, sample_rate));
        let (tx, rx) = bounded(1024);
        let (gtx, grx) = bounded(64);
        let n = prep.inserts.len();
        let shared = Arc::new(SharedState::new(n));
        let engine = Engine {
            tempo: prep.tempo,
            master_gain: prep.master_gain,
            prep,
            rx,
            garbage: gtx,
            shared: shared.clone(),
            playing: false,
            mode: PlayMode::Song,
            loop_range: None,
            pos: 0.0,
            next_event: 0,
            active: Vec::with_capacity(MAX_ACTIVE),
            voice_counter: 0,
            ins_l: vec![[0.0; CHUNK]; n],
            ins_r: vec![[0.0; CHUNK]; n],
            ch_l: [0.0; CHUNK],
            ch_r: [0.0; CHUNK],
            dry_l: [0.0; CHUNK],
            dry_r: [0.0; CHUNK],
            capture: None,
        };
        shared.tempo_bits.store(engine.tempo.to_bits(), Ordering::Relaxed);
        (
            engine,
            EngineHandle {
                tx,
                garbage: grx,
                shared,
                sample_rate,
            },
        )
    }

    pub fn shared(&self) -> &Arc<SharedState> {
        &self.shared
    }
    pub fn sample_rate(&self) -> f32 {
        self.prep.sample_rate
    }
    pub fn position(&self) -> f64 {
        self.pos
    }
    pub fn is_playing(&self) -> bool {
        self.playing
    }
    pub fn mode(&self) -> PlayMode {
        self.mode
    }
    pub fn insert_names(&self) -> &[String] {
        &self.prep.insert_names
    }

    /// Lunghezza della sequenza corrente in tick.
    pub fn sequence_length(&self) -> f64 {
        self.prep.sequence(self.pattern_of_mode()).map_or(0.0, |s| s.length)
    }

    fn pattern_of_mode(&self) -> Option<PatternId> {
        match self.mode {
            PlayMode::Song => None,
            PlayMode::Pattern(p) => Some(p),
        }
    }

    // ---- API diretta (usata dal render offline e dai comandi) ----

    pub fn play(&mut self) {
        self.playing = true;
        self.relocate(self.pos);
    }
    pub fn stop(&mut self) {
        self.playing = false;
        self.release_all();
        self.pos = 0.0;
        self.next_event = 0;
    }
    pub fn pause(&mut self) {
        self.playing = false;
        self.release_all();
    }
    pub fn seek(&mut self, tick: f64) {
        self.release_all();
        self.relocate(tick.max(0.0));
    }
    pub fn set_mode(&mut self, mode: PlayMode) {
        self.release_all();
        self.mode = mode;
        self.relocate(0.0);
    }
    pub fn set_loop(&mut self, range: Option<(f64, f64)>) {
        self.loop_range = range.filter(|(a, b)| b > a);
    }

    /// Installa un nuovo stato compilato riusando generatori/effetti compatibili.
    pub fn install(&mut self, mut new: Box<Prepared>) {
        // Riuso dei generatori per chiave strutturale.
        for nc in new.channels.iter_mut() {
            if let Some(oc) = self.prep.channels.iter_mut().find(|c| c.key == nc.key) {
                std::mem::swap(&mut nc.gen, &mut oc.gen);
                for (k, v) in &nc.params {
                    nc.gen.set_param(k, *v);
                }
                nc.gen.set_pitch_offset(nc.pitch);
            }
        }
        for (ni, oi) in new.inserts.iter_mut().zip(self.prep.inserts.iter_mut()) {
            for (ns, os) in ni.slots.iter_mut().zip(oi.slots.iter_mut()) {
                if ns.key == os.key {
                    std::mem::swap(&mut ns.fx, &mut os.fx);
                    if let Some(fx) = ns.fx.as_mut() {
                        for (k, v) in &ns.params {
                            fx.set_param(k, *v);
                        }
                    }
                }
            }
        }
        // Le note attive puntano a indici di canale vecchi: chiudile.
        self.release_all();
        let old = std::mem::replace(&mut self.prep, new);
        if self.ins_l.len() != self.prep.inserts.len() {
            // Solo se cambia il numero di insert (raro: progetti importati). Alloca.
            self.ins_l.resize(self.prep.inserts.len(), [0.0; CHUNK]);
            self.ins_r.resize(self.prep.inserts.len(), [0.0; CHUNK]);
        }
        self.tempo = self.prep.tempo;
        self.master_gain = self.prep.master_gain;
        if let PlayMode::Pattern(p) = self.mode {
            if self.prep.sequence(Some(p)).is_none() {
                self.mode = PlayMode::Song;
            }
        }
        self.relocate(self.pos);
        if let Err(e) = self.garbage.try_send(old) {
            // Nessuno raccoglie: dealloca qui (accettabile nel render offline).
            drop(e.into_inner());
        }
    }

    fn relocate(&mut self, tick: f64) {
        self.pos = tick;
        let pat = self.pattern_of_mode();
        let idx = self
            .prep
            .sequence(pat)
            .map_or(0, |s| s.events.partition_point(|e| e.on < tick));
        self.next_event = idx;
    }

    fn release_all(&mut self) {
        for a in self.active.drain(..) {
            if let Some(c) = self.prep.channels.get_mut(a.chan) {
                c.gen.note_off(a.voice);
            }
        }
    }

    fn handle_commands(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                EngineCommand::Play => self.play(),
                EngineCommand::Stop => self.stop(),
                EngineCommand::Pause => self.pause(),
                EngineCommand::Seek(t) => self.seek(t),
                EngineCommand::SetMode(m) => self.set_mode(m),
                EngineCommand::SetLoop(r) => self.set_loop(r),
                EngineCommand::Install(p) => self.install(p),
                EngineCommand::PreviewOn { channel, key, velocity } => self.preview_on(channel, key, velocity),
                EngineCommand::PreviewOff { channel, key } => self.preview_off(channel, key),
                EngineCommand::AllNotesOff => {
                    self.release_all();
                    for c in self.prep.channels.iter_mut() {
                        c.gen.all_notes_off();
                    }
                }
            }
        }
    }

    /// Anteprima di una nota (click su step, tastiera del piano roll).
    pub fn preview_on(&mut self, channel: ChannelId, key: u8, velocity: f32) {
        if let Some(c) = self.prep.channels.iter_mut().find(|c| c.id == channel) {
            let mut n = NoteOn::simple(PREVIEW_BIT | key as u64, key, velocity.clamp(0.0, 1.0));
            n.root_key = c.root;
            c.gen.note_on(n);
        }
    }
    pub fn preview_off(&mut self, channel: ChannelId, key: u8) {
        if let Some(c) = self.prep.channels.iter_mut().find(|c| c.id == channel) {
            c.gen.note_off(PREVIEW_BIT | key as u64);
        }
    }

    fn samples_per_tick(&self) -> f64 {
        self.prep
            .time
            .samples_per_tick(self.tempo.max(1.0), self.prep.sample_rate as f64)
    }

    /// Elabora un blocco audio. Scrive (non somma) in `left`/`right`.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let started = Instant::now();
        self.handle_commands();
        let n = left.len().min(right.len());
        let mut done = 0;
        while done < n {
            let mut len = (n - done).min(CHUNK);
            if self.playing {
                self.apply_automation();
                self.fire_events();
                // Accorcia il sotto-blocco fino al prossimo evento per l'accuratezza al sample.
                let spt = self.samples_per_tick();
                if let Some(next) = self.next_boundary() {
                    let samples = ((next - self.pos) * spt).ceil();
                    if samples >= 1.0 && (samples as usize) < len {
                        len = samples as usize;
                    }
                }
            }
            self.render_chunk(&mut left[done..done + len], &mut right[done..done + len]);
            if self.playing {
                self.pos += len as f64 / self.samples_per_tick();
                self.wrap_loop();
            }
            done += len;
        }
        self.shared.playing.store(self.playing, Ordering::Relaxed);
        self.shared.position_bits.store(self.pos.to_bits(), Ordering::Relaxed);
        self.shared.tempo_bits.store(self.tempo.to_bits(), Ordering::Relaxed);
        self.update_cpu_load(n, started.elapsed());
    }

    /// Aggiorna il carico CPU: tempo impiegato diviso per la durata del blocco, levigato.
    fn update_cpu_load(&self, frames: usize, elapsed: Duration) {
        if frames == 0 {
            return;
        }
        let block_secs = frames as f32 / self.prep.sample_rate;
        let ratio = elapsed.as_secs_f32() / block_secs;
        if !ratio.is_finite() {
            return;
        }
        let prev = f32::from_bits(self.shared.cpu_bits.load(Ordering::Relaxed));
        let smoothed = prev * 0.9 + ratio * 0.1;
        self.shared.cpu_bits.store(smoothed.to_bits(), Ordering::Relaxed);
    }

    /// Prossimo istante (tick) in cui succede qualcosa: note on/off o fine loop.
    fn next_boundary(&self) -> Option<f64> {
        let seq = self.prep.sequence(self.pattern_of_mode())?;
        let mut next = seq.events.get(self.next_event).map(|e| e.on);
        for a in &self.active {
            next = Some(next.map_or(a.off, |x: f64| x.min(a.off)));
        }
        if let Some(end) = self.loop_end() {
            next = Some(next.map_or(end, |x| x.min(end)));
        }
        next.filter(|t| *t > self.pos)
    }

    fn loop_end(&self) -> Option<f64> {
        match self.mode {
            PlayMode::Pattern(_) => Some(self.sequence_length()).filter(|l| *l > 0.0),
            PlayMode::Song => self.loop_range.map(|r| r.1),
        }
    }

    fn wrap_loop(&mut self) {
        let Some(end) = self.loop_end() else { return };
        if self.pos + 1e-9 >= end {
            let start = match self.mode {
                PlayMode::Pattern(_) => 0.0,
                PlayMode::Song => self.loop_range.map_or(0.0, |r| r.0),
            };
            let overshoot = (self.pos - end).max(0.0);
            // Chiude le note che finiscono al confine del loop.
            self.fire_note_offs(end);
            self.relocate(start + overshoot);
        }
    }

    fn fire_note_offs(&mut self, upto: f64) {
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i].off <= upto + 1e-9 {
                let a = self.active.swap_remove(i);
                self.prep.channels[a.chan].gen.note_off(a.voice);
            } else {
                i += 1;
            }
        }
    }

    fn fire_events(&mut self) {
        // Prima le note off (permette il retrigger della stessa nota).
        self.fire_note_offs(self.pos);
        let spt = self.samples_per_tick();
        let pat = self.pattern_of_mode();
        loop {
            let ev: NoteEvent = {
                let Some(seq) = self.prep.sequence(pat) else { return };
                match seq.events.get(self.next_event) {
                    Some(e) if e.on <= self.pos + 1e-9 => *e,
                    _ => return,
                }
            };
            self.next_event += 1;
            let Some(ch) = self.prep.channels.get_mut(ev.chan) else {
                continue;
            };
            if ch.muted || ev.off <= self.pos {
                continue;
            }
            self.voice_counter = (self.voice_counter + 1) & !PREVIEW_BIT;
            let voice = self.voice_counter;
            let late = (self.pos - ev.on).max(0.0);
            ch.gen.note_on(NoteOn {
                voice_id: voice,
                key: ev.key,
                velocity: ev.velocity,
                pan: ev.pan,
                fine_pitch_cents: ev.fine_pitch,
                root_key: ch.root,
                mod_x: ev.mod_x,
                mod_y: ev.mod_y,
                slide: ev.slide,
                start_offset_samples: ((ev.start_offset + late) * spt) as u64,
            });
            if self.active.len() < MAX_ACTIVE {
                self.active.push(Active {
                    chan: ev.chan,
                    voice,
                    off: ev.off,
                });
            } else {
                ch.gen.note_off(voice);
            }
        }
    }

    fn apply_automation(&mut self) {
        let pos = self.pos;
        let in_song = self.mode == PlayMode::Song;
        let prep = &mut *self.prep;
        for a in prep.automations.iter() {
            if !in_song || pos < a.start || pos >= a.end {
                continue;
            }
            let Some(v) = a.clip.real_value_at(pos - a.start + a.offset) else {
                continue;
            };
            match &a.target {
                AutoTarget::MasterVolume => self.master_gain = mfc_core::fader_gain(v, mfc_core::MIXER_UNITY),
                AutoTarget::Tempo => self.tempo = (v as f64).clamp(10.0, 999.0),
                AutoTarget::ChannelVolume(c) => {
                    prep.channels[*c].gain = mfc_core::fader_gain(v, mfc_core::CHANNEL_UNITY)
                }
                AutoTarget::ChannelPan(c) => prep.channels[*c].pan = v.clamp(-1.0, 1.0),
                AutoTarget::ChannelPitch(c) => {
                    let ch = &mut prep.channels[*c];
                    ch.pitch = v + prep.master_pitch;
                    ch.gen.set_pitch_offset(ch.pitch);
                }
                AutoTarget::ChannelParam(c, name) => prep.channels[*c].gen.set_param(name, v),
                AutoTarget::MixerVolume(i) => prep.inserts[*i].gain = mfc_core::fader_gain(v, mfc_core::MIXER_UNITY),
                AutoTarget::MixerPan(i) => prep.inserts[*i].pan = v.clamp(-1.0, 1.0),
                AutoTarget::EffectParam(i, s, name) => {
                    if let Some(fx) = prep.inserts[*i].slots.get_mut(*s).and_then(|s| s.fx.as_mut()) {
                        fx.set_param(name, v);
                    }
                }
                AutoTarget::EffectMix(i, s) => {
                    if let Some(slot) = prep.inserts[*i].slots.get_mut(*s) {
                        slot.mix = v.clamp(0.0, 1.0);
                    }
                }
            }
        }
    }

    fn render_chunk(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len();
        for (l, r) in self.ins_l.iter_mut().zip(self.ins_r.iter_mut()) {
            l[..len].fill(0.0);
            r[..len].fill(0.0);
        }
        let prep = &mut *self.prep;
        // 1) Canali → input degli insert.
        for ch in prep.channels.iter_mut() {
            if ch.gen.active_voices() == 0 {
                continue;
            }
            let cl = &mut self.ch_l[..len];
            let cr = &mut self.ch_r[..len];
            cl.fill(0.0);
            cr.fill(0.0);
            ch.gen.process(cl, cr);
            if ch.muted {
                continue;
            }
            let (gl, gr) = balance(ch.pan);
            let (gl, gr) = (gl * ch.gain, gr * ch.gain);
            let il = &mut self.ins_l[ch.insert][..len];
            let ir = &mut self.ins_r[ch.insert][..len];
            for i in 0..len {
                il[i] += cl[i] * gl;
                ir[i] += cr[i] * gr;
            }
        }
        // 2) Insert in ordine topologico.
        for &idx in prep.order.iter() {
            let ins = &mut prep.inserts[idx];
            let bl = &mut self.ins_l[idx][..len];
            let br = &mut self.ins_r[idx][..len];
            for slot in ins.slots.iter_mut() {
                let Some(fx) = slot.fx.as_mut() else { continue };
                if !slot.enabled {
                    continue;
                }
                if slot.mix >= 1.0 {
                    fx.process(bl, br);
                } else {
                    let (dl, dr) = (&mut self.dry_l[..len], &mut self.dry_r[..len]);
                    dl.copy_from_slice(bl);
                    dr.copy_from_slice(br);
                    fx.process(bl, br);
                    let w = slot.mix.max(0.0);
                    for i in 0..len {
                        bl[i] = dl[i] + (bl[i] - dl[i]) * w;
                        br[i] = dr[i] + (br[i] - dr[i]) * w;
                    }
                }
            }
            let (pl, pr) = balance(ins.pan);
            let silent = ins.muted || !ins.audible;
            let g = if silent { 0.0 } else { ins.gain };
            let inv = if ins.phase_invert { -1.0 } else { 1.0 };
            let width = 1.0 + ins.separation;
            let (mut peak_l, mut peak_r) = (0f32, 0f32);
            for i in 0..len {
                let (mut l, mut r) = (bl[i] * inv, br[i] * inv);
                if ins.swap_lr {
                    std::mem::swap(&mut l, &mut r);
                }
                if ins.separation != 0.0 {
                    let m = 0.5 * (l + r);
                    let s = 0.5 * (l - r) * width;
                    l = m + s;
                    r = m - s;
                }
                l *= g * pl;
                r *= g * pr;
                bl[i] = l;
                br[i] = r;
                peak_l = peak_l.max(l.abs());
                peak_r = peak_r.max(r.abs());
            }
            store_peak(&self.shared.meters[idx * 2], peak_l);
            store_peak(&self.shared.meters[idx * 2 + 1], peak_r);
            if let Some(cap) = self.capture.as_mut() {
                if let Some((cl, cr)) = cap.get_mut(idx) {
                    cl.extend_from_slice(bl);
                    cr.extend_from_slice(br);
                }
            }
            // Mandate verso insert successivi (sempre a indice "dopo" in ordine topologico).
            for &(t, gain) in ins.routes.iter() {
                let (src_l, dst_l) = two_mut(&mut self.ins_l, idx, t);
                let (src_r, dst_r) = two_mut(&mut self.ins_r, idx, t);
                for i in 0..len {
                    dst_l[i] += src_l[i] * gain;
                    dst_r[i] += src_r[i] * gain;
                }
            }
        }
        // 3) Master → uscita.
        let ml = &self.ins_l[mfc_core::MASTER as usize];
        let mr = &self.ins_r[mfc_core::MASTER as usize];
        let (mut pk_l, mut pk_r) = (0f32, 0f32);
        for i in 0..len {
            let l = sanitize(ml[i] * self.master_gain);
            let r = sanitize(mr[i] * self.master_gain);
            out_l[i] = l;
            out_r[i] = r;
            pk_l = pk_l.max(l.abs());
            pk_r = pk_r.max(r.abs());
        }
        store_peak(&self.shared.out_meter[0], pk_l);
        store_peak(&self.shared.out_meter[1], pk_r);
    }

    /// Note attualmente suonate dal sequencer.
    pub fn active_notes(&self) -> usize {
        self.active.len()
    }

    /// Tick → secondi al tempo corrente.
    pub fn ticks_to_seconds(&self, ticks: Tick) -> f64 {
        self.prep.time.ticks_to_seconds(ticks as f64, self.tempo)
    }
}

/// Pan "balance" a potenza costante normalizzato (centro = 0 dB su entrambi i lati).
fn balance(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    let angle = (p + 1.0) * std::f32::consts::FRAC_PI_4;
    let s = std::f32::consts::SQRT_2;
    ((angle.cos() * s).min(1.0), (angle.sin() * s).min(1.0))
}

fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-8.0, 8.0)
    } else {
        0.0
    }
}

fn two_mut<T>(v: &mut [T], a: usize, b: usize) -> (&T, &mut T) {
    debug_assert_ne!(a, b);
    if a < b {
        let (x, y) = v.split_at_mut(b);
        (&x[a], &mut y[0])
    } else {
        let (x, y) = v.split_at_mut(a);
        (&y[0], &mut x[b])
    }
}

#[cfg(test)]
mod cpu_load_tests {
    use super::*;

    #[test]
    fn cpu_load_is_finite_and_non_negative_after_a_block() {
        let project = Project::new("cpu");
        let (mut engine, handle) = Engine::new(&project, 48_000.0);
        let mut l = vec![0.0f32; 512];
        let mut r = vec![0.0f32; 512];
        engine.process(&mut l, &mut r);
        let load = handle.shared.cpu_load();
        assert!(load.is_finite() && load >= 0.0, "carico CPU non valido: {load}");
    }
}
