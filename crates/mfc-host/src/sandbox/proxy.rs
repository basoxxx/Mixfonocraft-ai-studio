//! Lato host: [`SandboxInstance`], un [`ExternalInstance`] che inoltra tutto al processo
//! `mfc-plugin-host`.
//!
//! Thread coinvolti:
//! - thread audio (motore): [`SandboxInstance::process`], solo operazioni atomiche sullo
//!   slot e copie in buffer preallocati;
//! - worker IPC (uno per istanza): invia il blocco richiesto e attende il risultato;
//! - lettore (uno per processo): legge stdout del figlio e smista risposte, blocchi
//!   elaborati ed eventi; a fine flusso segna l'istanza come morta;
//! - thread UI: richieste di controllo sincrone (con timeout).

use std::cell::UnsafeCell;
use std::io::{self, BufReader};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;

use super::proto::{self, from_child, to_child, Call, Dec, Enc, Reply};
use super::{describe_exit, SandboxConfig, HELPER_NAME};
use crate::{
    make_kind, EditorSize, ExternalInstance, ExternalParam, HostError, MidiEvent, ParentWindow, ParsedKind, PluginInfo,
    DEFAULT_MAX_BLOCK,
};

/// Eventi MIDI per blocco (in eccesso vengono scartati, come negli adattatori del motore).
const MAX_EVENTS: usize = 1024;
/// Blocco ipotizzato per la latenza prima del primo `process` (`mfc_engine::CHUNK`).
const ASSUMED_BLOCK: u32 = 64;
/// Cambi di parametro in coda tra due invii.
const PARAM_QUEUE: usize = 8192;

// Stati dello slot audio.
/// Nessuna richiesta in corso, nessun risultato: i dati sono del thread audio.
const IDLE: u8 = 0;
/// Richiesta consegnata: i dati sono del worker.
const REQUESTED: u8 = 1;
/// Risultato pronto: i dati tornano al thread audio.
const DONE: u8 = 2;

/// Buffer dello slot (richiesta + risultato), preallocati alla creazione.
struct SlotData {
    frames: usize,
    events: Vec<MidiEvent>,
    has_input: bool,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    ok: bool,
    out_frames: usize,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

/// Passaggio di un blocco a uno slot singolo tra thread audio e worker.
struct Slot {
    state: AtomicU8,
    data: UnsafeCell<SlotData>,
}

// SAFETY: l'accesso a `data` è regolato da `state`: il thread audio lo usa solo in
// IDLE/DONE (e passa a REQUESTED con Release), il worker solo in REQUESTED (e passa a
// DONE con Release); entrambi leggono `state` con Acquire. Il thread audio è unico
// (garantito dal `try_lock` di `Shared::audio`).
unsafe impl Sync for Slot {}

/// Stato privato del thread audio.
struct AudioLocal {
    /// Eventi di blocchi non consegnati (worker in ritardo), da inviare col successivo.
    carry: Vec<MidiEvent>,
}

/// Errore di una richiesta al processo.
enum IpcError {
    Io,
    Timeout,
    Disconnected,
}

impl From<io::Error> for IpcError {
    fn from(_: io::Error) -> Self {
        IpcError::Io
    }
}

/// Blocco elaborato dal figlio.
struct Processed {
    seq: u32,
    ok: bool,
    l: Vec<f32>,
    r: Vec<f32>,
}

/// Stato condiviso tra proxy, worker e lettore.
struct Shared {
    slot: Slot,
    audio: Mutex<AudioLocal>,
    capacity: usize,
    conn: Mutex<Option<Arc<Conn>>>,
    /// Generazione del processo corrente: la morte di un processo vecchio (chiuso o
    /// sostituito da `restart`) non segna l'istanza come morta.
    generation: AtomicU64,
    dead: AtomicBool,
    death: Mutex<Option<String>>,
    shutdown: AtomicBool,
    worker: OnceLock<Thread>,
    params_tx: Sender<(u32, f64)>,
    params_rx: Receiver<(u32, f64)>,
    reset_pending: AtomicBool,
    editor_open: AtomicBool,
    plugin_latency: AtomicU32,
    last_block: AtomicU32,
    sample_rate: AtomicU32,
    max_block: AtomicU32,
    process_wait: Option<Duration>,
    block_timeout: Duration,
}

impl Shared {
    fn mark_dead(&self, generation: u64, reason: String) {
        if self.generation.load(Ordering::SeqCst) == generation && !self.dead.swap(true, Ordering::SeqCst) {
            eprintln!("mfc-host: processo del plugin terminato: {reason}");
            *self.death.lock() = Some(reason);
            self.editor_open.store(false, Ordering::SeqCst);
        }
    }

    fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate.load(Ordering::Relaxed))
    }
}

/// Collegamento a un processo `mfc-plugin-host`.
struct Conn {
    generation: u64,
    pid: u32,
    child: Arc<Mutex<Child>>,
    writer: Mutex<(ChildStdin, Enc)>,
    /// Una sola richiesta di controllo in volo alla volta.
    ctrl: Mutex<()>,
    next_id: AtomicU32,
    replies: Receiver<(u32, Reply)>,
    audio: Receiver<Processed>,
}

impl Conn {
    /// Avvia il processo e attende la creazione dell'istanza.
    fn spawn(
        cfg: &SandboxConfig,
        kind: &str,
        sample_rate: f32,
        max_block: usize,
        shared: Weak<Shared>,
        generation: u64,
    ) -> Result<(Arc<Conn>, PluginInfo, u32), HostError> {
        let mut child = cfg
            .command()
            .arg("--kind")
            .arg(kind)
            .arg("--sample-rate")
            .arg(sample_rate.to_string())
            .arg("--max-block")
            .arg(max_block.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| HostError::Load {
                path: cfg.helper.clone(),
                reason: format!("avvio di {HELPER_NAME}: {e}"),
            })?;
        let stdin = child.stdin.take().expect("stdin in pipe");
        let stdout = child.stdout.take().expect("stdout in pipe");
        let pid = child.id();
        let child = Arc::new(Mutex::new(child));
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let (rep_tx, rep_rx) = crossbeam_channel::unbounded();
        let (aud_tx, aud_rx) = crossbeam_channel::unbounded();
        {
            let reader_child = child.clone();
            let spawned = thread::Builder::new()
                .name("mfc-sandbox-reader".into())
                .spawn(move || reader(stdout, ready_tx, rep_tx, aud_tx, shared, generation, reader_child));
            if let Err(e) = spawned {
                kill(&child);
                return Err(HostError::Io(e));
            }
        }
        let conn = Arc::new(Conn {
            generation,
            pid,
            child: child.clone(),
            writer: Mutex::new((stdin, Enc::new())),
            ctrl: Mutex::new(()),
            next_id: AtomicU32::new(1),
            replies: rep_rx,
            audio: aud_rx,
        });
        match ready_rx.recv_timeout(cfg.startup_timeout) {
            Ok(Ok((info, latency))) => Ok((conn, info, latency)),
            Ok(Err(e)) => {
                conn.shutdown();
                Err(e)
            }
            Err(RecvTimeoutError::Timeout) => {
                kill(&child);
                Err(HostError::Plugin(format!(
                    "{HELPER_NAME} non risponde durante il caricamento (processo terminato)"
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(HostError::Plugin(format!(
                "il processo del plugin si è chiuso in modo anomalo durante il caricamento ({})",
                exit_reason(&child)
            ))),
        }
    }

    /// Invia reset e cambi di parametro in coda (con il writer già bloccato).
    fn flush_locked(w: &mut (ChildStdin, Enc), shared: &Shared) -> io::Result<()> {
        let (stdin, enc) = w;
        if shared.reset_pending.swap(false, Ordering::AcqRel) {
            enc.begin(to_child::RESET);
            proto::write_frame(stdin, enc.finish())?;
        }
        while let Ok((id, v)) = shared.params_rx.try_recv() {
            enc.begin(to_child::SET_PARAM).u32(id).f64(v);
            proto::write_frame(stdin, enc.finish())?;
        }
        Ok(())
    }

    fn flush(&self, shared: &Shared) -> io::Result<()> {
        Self::flush_locked(&mut self.writer.lock(), shared)
    }

    /// Richiesta di controllo sincrona.
    fn call(&self, shared: &Shared, call: &Call, timeout: Duration, flush: bool) -> Result<Reply, IpcError> {
        let _one = self.ctrl.lock();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).max(1);
        {
            let mut w = self.writer.lock();
            if flush {
                Self::flush_locked(&mut w, shared)?;
            }
            let (stdin, enc) = &mut *w;
            enc.begin(to_child::CALL).u32(id);
            call.encode(enc);
            proto::write_frame(stdin, enc.finish())?;
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.replies.recv_timeout(left) {
                Ok((rid, reply)) if rid == id => return Ok(reply),
                // Risposta a una richiesta scaduta in precedenza.
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => return Err(IpcError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(IpcError::Disconnected),
            }
        }
    }

    /// Elabora il blocco dello slot (dal worker).
    fn process(&self, shared: &Shared, data: &mut SlotData, seq: u32) -> Result<(), IpcError> {
        {
            let mut w = self.writer.lock();
            Self::flush_locked(&mut w, shared)?;
            let (stdin, enc) = &mut *w;
            let input = data
                .has_input
                .then(|| (&data.in_l[..data.frames], &data.in_r[..data.frames]));
            proto::encode_process(enc, seq, data.frames, &data.events, input);
            proto::write_frame(stdin, enc.finish())?;
        }
        let deadline = Instant::now() + shared.block_timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.audio.recv_timeout(left) {
                Ok(p) if p.seq == seq => {
                    let n = p.l.len().min(p.r.len()).min(data.out_l.len());
                    data.out_l[..n].copy_from_slice(&p.l[..n]);
                    data.out_r[..n].copy_from_slice(&p.r[..n]);
                    data.out_frames = n;
                    data.ok = p.ok;
                    return Ok(());
                }
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => return Err(IpcError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(IpcError::Disconnected),
            }
        }
    }

    fn kill(&self) {
        kill(&self.child);
    }

    /// Chiusura ordinata: chiede al figlio di terminare e lo raccoglie in background
    /// (terminandolo se non esce entro pochi secondi).
    fn shutdown(&self) {
        {
            let mut w = self.writer.lock();
            let (stdin, enc) = &mut *w;
            enc.begin(to_child::CALL).u32(0);
            Call::Shutdown.encode(enc);
            let _ = proto::write_frame(stdin, enc.finish());
        }
        let child = self.child.clone();
        let _ = thread::Builder::new().name("mfc-sandbox-reaper".into()).spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match child.lock().try_wait() {
                    Ok(None) => {}
                    _ => return,
                }
                if Instant::now() >= deadline {
                    kill(&child);
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
    }
}

fn kill(child: &Mutex<Child>) {
    let mut c = child.lock();
    let _ = c.kill();
    let _ = c.wait();
}

/// Motivo dell'uscita del figlio (attende brevemente che il processo termini).
fn exit_reason(child: &Mutex<Child>) -> String {
    for _ in 0..50 {
        if let Ok(Some(st)) = child.lock().try_wait() {
            return describe_exit(st);
        }
        thread::sleep(Duration::from_millis(10));
    }
    "comunicazione interrotta".into()
}

type ReadyResult = Result<(PluginInfo, u32), HostError>;

/// Thread lettore di stdout del figlio.
fn reader(
    stdout: ChildStdout,
    ready_tx: Sender<ReadyResult>,
    rep_tx: Sender<(u32, Reply)>,
    aud_tx: Sender<Processed>,
    shared: Weak<Shared>,
    generation: u64,
    child: Arc<Mutex<Child>>,
) {
    let mut r = BufReader::with_capacity(1 << 16, stdout);
    let mut buf = Vec::new();
    let mut ready_tx = Some(ready_tx);
    while let Ok(true) = proto::read_frame(&mut r, &mut buf) {
        let mut d = Dec::new(&buf);
        let res = (|| -> io::Result<()> {
            match d.u8()? {
                from_child::READY => {
                    let version = d.u32()?;
                    let result = if version != proto::PROTOCOL_VERSION {
                        Err(HostError::Plugin(format!(
                            "{HELPER_NAME}: versione del protocollo {version} non compatibile"
                        )))
                    } else if d.bool()? {
                        Ok((d.info()?, d.u32()?))
                    } else {
                        Err(d.error()?)
                    };
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(result);
                    }
                }
                from_child::REPLY => {
                    let id = d.u32()?;
                    let _ = rep_tx.send((id, Reply::decode(&mut d)?));
                }
                from_child::PROCESSED => {
                    let seq = d.u32()?;
                    let ok = d.bool()?;
                    let (mut l, mut r) = (Vec::new(), Vec::new());
                    d.f32s_into(&mut l)?;
                    d.f32s_into(&mut r)?;
                    let _ = aud_tx.send(Processed { seq, ok, l, r });
                }
                from_child::EDITOR_CLOSED => {
                    if let Some(s) = shared.upgrade() {
                        s.editor_open.store(false, Ordering::SeqCst);
                    }
                }
                from_child::LATENCY => {
                    let l = d.u32()?;
                    if let Some(s) = shared.upgrade() {
                        s.plugin_latency.store(l, Ordering::Relaxed);
                    }
                }
                _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "tipo sconosciuto")),
            }
            Ok(())
        })();
        if res.is_err() {
            // Protocollo corrotto: il processo non è più affidabile.
            kill(&child);
            break;
        }
    }
    drop((ready_tx, rep_tx, aud_tx));
    let reason = exit_reason(&child);
    if let Some(s) = shared.upgrade() {
        s.mark_dead(generation, reason);
    }
}

/// Thread IPC dell'audio: elabora le richieste dello slot.
fn worker(shared: Arc<Shared>) {
    let mut seq = 0u32;
    while !shared.shutdown.load(Ordering::Acquire) {
        if shared.slot.state.load(Ordering::Acquire) != REQUESTED {
            thread::park_timeout(Duration::from_millis(100));
            continue;
        }
        // SAFETY: in REQUESTED i dati dello slot appartengono al worker (vedi `Slot`).
        let data = unsafe { &mut *shared.slot.data.get() };
        seq = seq.wrapping_add(1);
        let conn = shared.conn.lock().clone();
        let result = match &conn {
            Some(c) if !shared.dead.load(Ordering::SeqCst) => c.process(&shared, data, seq),
            _ => Err(IpcError::Disconnected),
        };
        if let Err(e) = result {
            data.ok = false;
            data.out_frames = 0;
            if let (IpcError::Timeout, Some(c)) = (e, &conn) {
                // Plugin bloccato: il processo viene terminato.
                c.kill();
                shared.mark_dead(c.generation, "il plugin non risponde (blocco audio)".into());
            }
        }
        shared.slot.state.store(DONE, Ordering::Release);
    }
}

fn dead_error() -> HostError {
    HostError::Plugin("il processo del plugin non è attivo".into())
}

fn unexpected() -> HostError {
    HostError::Plugin("risposta inattesa dal processo del plugin".into())
}

/// Istanza di plugin ospitata in un processo `mfc-plugin-host`.
pub struct SandboxInstance {
    info: PluginInfo,
    kind: String,
    config: SandboxConfig,
    shared: Arc<Shared>,
    /// Ultimo stato noto (creazione, salvataggi, caricamenti, istantanee periodiche):
    /// ripristinato da `restart`.
    last_state: Mutex<Option<Vec<u8>>>,
    last_snapshot: Mutex<Instant>,
}

impl SandboxInstance {
    /// Avvia il processo helper e vi crea l'istanza indicata.
    pub fn spawn(
        config: SandboxConfig,
        kind: &ParsedKind,
        sample_rate: f32,
        max_block: usize,
    ) -> Result<Self, HostError> {
        let kind = make_kind(kind.format, &kind.path, &kind.id);
        let capacity = max_block.max(DEFAULT_MAX_BLOCK);
        let (params_tx, params_rx) = crossbeam_channel::bounded(PARAM_QUEUE);
        let shared = Arc::new(Shared {
            slot: Slot {
                state: AtomicU8::new(IDLE),
                data: UnsafeCell::new(SlotData {
                    frames: 0,
                    events: Vec::with_capacity(MAX_EVENTS),
                    has_input: false,
                    in_l: vec![0.0; capacity],
                    in_r: vec![0.0; capacity],
                    ok: false,
                    out_frames: 0,
                    out_l: vec![0.0; capacity],
                    out_r: vec![0.0; capacity],
                }),
            },
            audio: Mutex::new(AudioLocal {
                carry: Vec::with_capacity(MAX_EVENTS),
            }),
            capacity,
            conn: Mutex::new(None),
            generation: AtomicU64::new(1),
            dead: AtomicBool::new(false),
            death: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            worker: OnceLock::new(),
            params_tx,
            params_rx,
            reset_pending: AtomicBool::new(false),
            editor_open: AtomicBool::new(false),
            plugin_latency: AtomicU32::new(0),
            last_block: AtomicU32::new(0),
            sample_rate: AtomicU32::new(sample_rate.to_bits()),
            max_block: AtomicU32::new(max_block as u32),
            process_wait: config.process_wait,
            block_timeout: config.block_timeout,
        });
        let (conn, info, latency) = Conn::spawn(&config, &kind, sample_rate, max_block, Arc::downgrade(&shared), 1)?;
        shared.plugin_latency.store(latency, Ordering::Relaxed);
        *shared.conn.lock() = Some(conn.clone());
        let worker = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("mfc-sandbox-ipc".into())
                .spawn(move || worker(shared))
        };
        let worker = match worker {
            Ok(w) => w,
            Err(e) => {
                conn.shutdown();
                return Err(HostError::Io(e));
            }
        };
        let _ = shared.worker.set(worker.thread().clone());
        let inst = Self {
            info,
            kind,
            config,
            shared,
            last_state: Mutex::new(None),
            last_snapshot: Mutex::new(Instant::now()),
        };
        // Stato alla creazione, per un eventuale riavvio.
        let _ = inst.save_state();
        Ok(inst)
    }

    /// Motivo della terminazione del processo, se morto.
    pub fn death_reason(&self) -> Option<String> {
        self.shared.death.lock().clone()
    }

    /// Ultimo stato noto del plugin (quello che `restart` ripristinerebbe).
    pub fn last_state(&self) -> Option<Vec<u8>> {
        self.last_state.lock().clone()
    }

    fn call_with(&self, call: Call, timeout: Duration) -> Result<Reply, HostError> {
        let s = &*self.shared;
        if s.dead.load(Ordering::SeqCst) {
            return Err(dead_error());
        }
        let conn = s.conn.lock().clone().ok_or_else(dead_error)?;
        match conn.call(s, &call, timeout, true) {
            Ok(Reply::Err(e)) => Err(e),
            Ok(r) => Ok(r),
            Err(IpcError::Timeout) => {
                conn.kill();
                s.mark_dead(conn.generation, "il plugin non risponde".into());
                Err(HostError::Plugin("il plugin non risponde (processo terminato)".into()))
            }
            Err(_) => Err(dead_error()),
        }
    }

    fn call(&self, call: Call) -> Result<Reply, HostError> {
        self.call_with(call, self.config.call_timeout)
    }

    fn current_conn(&self) -> Option<Arc<Conn>> {
        if self.shared.dead.load(Ordering::SeqCst) {
            return None;
        }
        self.shared.conn.lock().clone()
    }
}

impl ExternalInstance for SandboxInstance {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn prepare(&self, sample_rate: f32, max_block: usize) -> Result<(), HostError> {
        self.shared.sample_rate.store(sample_rate.to_bits(), Ordering::Relaxed);
        self.shared.max_block.store(max_block as u32, Ordering::Relaxed);
        match self.call(Call::Prepare {
            sample_rate,
            max_block: max_block as u32,
        })? {
            Reply::Unit => Ok(()),
            _ => Err(unexpected()),
        }
    }

    fn process(
        &self,
        events: &[MidiEvent],
        input: Option<(&[f32], &[f32])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> bool {
        let s = &*self.shared;
        if s.dead.load(Ordering::Relaxed) {
            return false;
        }
        let n = out_l.len().min(out_r.len());
        if n == 0 {
            return true;
        }
        if n > s.capacity {
            return false;
        }
        let Some(mut local) = s.audio.try_lock() else {
            return false;
        };
        s.last_block.store(n as u32, Ordering::Relaxed);

        let mut state = s.slot.state.load(Ordering::Acquire);
        if state == REQUESTED {
            // Attesa limitata del blocco precedente (senza lock né syscall bloccanti).
            let wait = s
                .process_wait
                .unwrap_or_else(|| Duration::from_secs_f64(n as f64 / s.sample_rate().max(1.0) as f64 * 0.5));
            let start = Instant::now();
            let mut spins = 0u32;
            loop {
                state = s.slot.state.load(Ordering::Acquire);
                if state != REQUESTED {
                    break;
                }
                if start.elapsed() >= wait {
                    // In ritardo: gli eventi passano al blocco successivo (offset azzerato).
                    for e in events {
                        if local.carry.len() < MAX_EVENTS {
                            local.carry.push(at_start(*e));
                        }
                    }
                    return false;
                }
                spins = spins.wrapping_add(1);
                if spins & 63 == 0 {
                    thread::yield_now();
                } else {
                    std::hint::spin_loop();
                }
            }
        }

        // SAFETY: stato IDLE o DONE: i dati dello slot appartengono al thread audio.
        let data = unsafe { &mut *s.slot.data.get() };
        let produced = if state == DONE {
            if data.ok {
                let m = data.out_frames.min(n);
                out_l[..m].copy_from_slice(&data.out_l[..m]);
                out_r[..m].copy_from_slice(&data.out_r[..m]);
                out_l[m..n].fill(0.0);
                out_r[m..n].fill(0.0);
                true
            } else {
                false
            }
        } else {
            // Primo blocco: nessun risultato precedente (un blocco di latenza) → silenzio.
            out_l[..n].fill(0.0);
            out_r[..n].fill(0.0);
            true
        };

        // Nuova richiesta.
        data.frames = n;
        data.events.clear();
        for e in local.carry.drain(..).chain(events.iter().copied()) {
            if data.events.len() < MAX_EVENTS {
                data.events.push(e);
            }
        }
        match input {
            Some((l, r)) => {
                let m = l.len().min(r.len()).min(n);
                data.in_l[..m].copy_from_slice(&l[..m]);
                data.in_r[..m].copy_from_slice(&r[..m]);
                data.in_l[m..n].fill(0.0);
                data.in_r[m..n].fill(0.0);
                data.has_input = true;
            }
            None => data.has_input = false,
        }
        s.slot.state.store(REQUESTED, Ordering::Release);
        if let Some(t) = s.worker.get() {
            t.unpark();
        }
        produced
    }

    fn set_param_normalized(&self, id: u32, value: f64) {
        // Da qualunque thread (anche audio): coda lock-free, inviata prima del blocco
        // o della richiesta successivi.
        let _ = self.shared.params_tx.try_send((id, value));
    }

    fn reset(&self) {
        // Può arrivare dal thread audio: inviato con il blocco successivo.
        self.shared.reset_pending.store(true, Ordering::Release);
    }

    fn params(&self) -> Vec<ExternalParam> {
        match self.call(Call::Params) {
            Ok(Reply::Params(v)) => v,
            _ => Vec::new(),
        }
    }

    fn param_normalized(&self, id: u32) -> f64 {
        match self.call(Call::ParamNormalized(id)) {
            Ok(Reply::F64(v)) => v,
            _ => 0.0,
        }
    }

    fn param_display(&self, id: u32, value: f64) -> String {
        match self.call(Call::ParamDisplay(id, value)) {
            Ok(Reply::Str(s)) => s,
            _ => format!("{value:.3}"),
        }
    }

    fn save_state(&self) -> Result<Vec<u8>, HostError> {
        match self.call(Call::SaveState)? {
            Reply::Bytes(b) => {
                *self.last_state.lock() = Some(b.clone());
                *self.last_snapshot.lock() = Instant::now();
                Ok(b)
            }
            _ => Err(unexpected()),
        }
    }

    fn load_state(&self, data: &[u8]) -> Result<(), HostError> {
        match self.call(Call::LoadState(data.to_vec()))? {
            Reply::Unit => {
                *self.last_state.lock() = Some(data.to_vec());
                *self.last_snapshot.lock() = Instant::now();
                Ok(())
            }
            _ => Err(unexpected()),
        }
    }

    fn has_editor(&self) -> bool {
        matches!(self.call(Call::HasEditor), Ok(Reply::Bool(true)))
    }

    fn open_editor(&self, _parent: ParentWindow) -> Result<EditorSize, HostError> {
        #[cfg(windows)]
        if let Some(c) = self.current_conn() {
            // Permette alla finestra del figlio di andare in primo piano.
            // SAFETY: semplice chiamata Win32 con un PID.
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(c.pid) };
        }
        match self.call(Call::OpenEditor(self.info.name.clone()))? {
            Reply::Size(size) => {
                self.shared.editor_open.store(true, Ordering::SeqCst);
                Ok(size)
            }
            _ => Err(unexpected()),
        }
    }

    fn close_editor(&self) {
        if self.shared.editor_open.swap(false, Ordering::SeqCst) {
            let _ = self.call(Call::CloseEditor);
        }
    }

    fn idle(&self) {
        let Some(conn) = self.current_conn() else { return };
        // Cambi di parametro fatti a motore fermo.
        let _ = conn.flush(&self.shared);
        if let Some(every) = self.config.state_snapshot_interval {
            let due = self.last_snapshot.lock().elapsed() >= every;
            if due {
                let _ = self.save_state();
                // Anche se fallisce, si riprova al prossimo intervallo.
                *self.last_snapshot.lock() = Instant::now();
            }
        }
    }

    fn latency(&self) -> u32 {
        let block = match self.shared.last_block.load(Ordering::Relaxed) {
            0 => ASSUMED_BLOCK,
            b => b,
        };
        self.shared.plugin_latency.load(Ordering::Relaxed) + block
    }

    fn is_alive(&self) -> bool {
        !self.shared.dead.load(Ordering::SeqCst)
    }

    fn crash_reason(&self) -> Option<String> {
        self.death_reason()
    }

    fn restart(&self) -> Result<(), HostError> {
        let s = &*self.shared;
        // Il processo corrente (se vivo) viene sostituito: la sua fine non è un crash.
        s.dead.store(true, Ordering::SeqCst);
        let generation = s.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(old) = s.conn.lock().take() {
            old.kill();
        }
        s.editor_open.store(false, Ordering::SeqCst);
        let fail = |e: HostError| {
            *s.death.lock() = Some(format!("riavvio non riuscito: {e}"));
            e
        };
        let max_block = s.max_block.load(Ordering::Relaxed) as usize;
        let (conn, _info, latency) = Conn::spawn(
            &self.config,
            &self.kind,
            s.sample_rate(),
            max_block,
            Arc::downgrade(&self.shared),
            generation,
        )
        .map_err(fail)?;
        if let Some(state) = self.last_state.lock().clone() {
            // Senza flush: i cambi in coda sono più recenti dello stato e vanno applicati dopo.
            let restored = match conn.call(s, &Call::LoadState(state), self.config.call_timeout, false) {
                Ok(Reply::Unit) => Ok(()),
                Ok(Reply::Err(e)) => Err(e),
                Ok(_) => Err(unexpected()),
                Err(_) => Err(dead_error()),
            };
            if let Err(e) = restored {
                conn.shutdown();
                return Err(fail(e));
            }
        }
        s.plugin_latency.store(latency, Ordering::Relaxed);
        // Il risultato rimasto nello slot appartiene al processo precedente (spesso un
        // blocco fallito): si riparte come al primo blocco. Solo lo stato cambia, i dati
        // non vengono toccati, quindi la transizione è sicura anche col thread audio attivo.
        let _ = s
            .slot
            .state
            .compare_exchange(DONE, IDLE, Ordering::AcqRel, Ordering::Acquire);
        *s.conn.lock() = Some(conn);
        *s.death.lock() = None;
        s.dead.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn process_id(&self) -> Option<u32> {
        self.shared.conn.lock().as_ref().map(|c| c.pid)
    }

    fn editor_is_external(&self) -> bool {
        true
    }

    fn editor_is_open(&self) -> bool {
        self.shared.editor_open.load(Ordering::SeqCst)
    }
}

impl Drop for SandboxInstance {
    fn drop(&mut self) {
        let s = &*self.shared;
        // La chiusura non è un crash.
        s.generation.fetch_add(1, Ordering::SeqCst);
        s.shutdown.store(true, Ordering::Release);
        if let Some(t) = s.worker.get() {
            t.unpark();
        }
        if let Some(conn) = s.conn.lock().take() {
            conn.shutdown();
        }
    }
}

/// Evento spostato all'inizio del blocco.
fn at_start(e: MidiEvent) -> MidiEvent {
    match e {
        MidiEvent::NoteOn {
            key, velocity, note_id, ..
        } => MidiEvent::NoteOn {
            offset: 0,
            key,
            velocity,
            note_id,
        },
        MidiEvent::NoteOff {
            key, velocity, note_id, ..
        } => MidiEvent::NoteOff {
            offset: 0,
            key,
            velocity,
            note_id,
        },
        MidiEvent::AllNotesOff { .. } => MidiEvent::AllNotesOff { offset: 0 },
    }
}
