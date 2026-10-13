//! Lato helper: il processo `mfc-plugin-host` che ospita UNA istanza di plugin.
//!
//! Uso:
//! - `mfc-plugin-host --kind <kind> --sample-rate <hz> --max-block <n>`: crea l'istanza
//!   con i backend in-process e serve le richieste su stdin/stdout (vedi [`super::proto`]).
//! - `mfc-plugin-host --scan <percorso>`: scansiona un bundle/file e stampa il JSON
//!   `{"ok":[PluginInfo...]}` oppure `{"err":"messaggio"}`.
//!
//! Thread:
//! - main: crea l'istanza ed esegue tutte le richieste "UI" (parametri, stato, editor);
//!   quando c'è un editor aperto fa girare anche il loop degli eventi nativo (NSApplication
//!   su macOS, coda dei messaggi Win32 su Windows);
//! - lettore: legge stdin, applica subito cambi di parametro e reset (consentiti da
//!   qualunque thread) e smista blocchi audio e richieste;
//! - audio: elabora i blocchi in ordine e risponde.
//!
//! Lo stdout originale è riservato al protocollo: all'avvio viene duplicato e il
//! descrittore 1 viene rediretto su stderr, così le stampe dei plugin non corrompono i frame.

use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::proto::{self, from_child, to_child, Call, Dec, Enc, ProcessReq, Reply};
use super::window::{self, EditorWindow};
use crate::{parse_kind, EditorSize, ExternalInstance, FormatBackend, HostError, PluginFormat, PluginInfo};

/// Esito di `--scan` (JSON su stdout).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanOutput {
    Ok(Vec<PluginInfo>),
    Err(String),
}

/// Modalità d'uso dell'helper.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Scan(PathBuf),
    Serve {
        kind: String,
        sample_rate: f32,
        max_block: usize,
    },
}

const USAGE: &str =
    "uso: mfc-plugin-host --kind <kind> --sample-rate <hz> --max-block <n>\n     mfc-plugin-host --scan <percorso>";

fn parse_args(args: &[String]) -> Result<Mode, String> {
    let mut kind = None;
    let mut sample_rate = 48_000.0f32;
    let mut max_block = crate::DEFAULT_MAX_BLOCK;
    let mut scan = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("manca il valore di {a}"));
        match a.as_str() {
            "--scan" => scan = Some(PathBuf::from(value()?)),
            "--kind" => kind = Some(value()?),
            "--sample-rate" => sample_rate = value()?.parse().map_err(|_| "sample rate non valido".to_string())?,
            "--max-block" => max_block = value()?.parse().map_err(|_| "blocco massimo non valido".to_string())?,
            _ => return Err(format!("argomento sconosciuto: {a}")),
        }
    }
    if let Some(path) = scan {
        return Ok(Mode::Scan(path));
    }
    let kind = kind.ok_or("manca --kind")?;
    if !(sample_rate.is_finite() && sample_rate > 0.0) || max_block == 0 {
        return Err("sample rate o blocco massimo non validi".into());
    }
    Ok(Mode::Serve {
        kind,
        sample_rate,
        max_block,
    })
}

/// Punto d'ingresso del binario `mfc-plugin-host`; restituisce il codice di uscita.
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("{USAGE}");
        return 0;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        eprintln!("mfc-plugin-host {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let mode = match parse_args(&args) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("mfc-plugin-host: {e}\n{USAGE}");
            return 2;
        }
    };
    let out = match protocol_stdout() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("mfc-plugin-host: stdout non disponibile: {e}");
            return 2;
        }
    };
    match mode {
        Mode::Scan(path) => scan(&path, out),
        Mode::Serve {
            kind,
            sample_rate,
            max_block,
        } => serve(&kind, sample_rate, max_block, out),
    }
}

fn backends() -> [Box<dyn FormatBackend>; 2] {
    [
        Box::new(crate::vst3::Vst3Backend::new()),
        Box::new(crate::clap::ClapBackend::new()),
    ]
}

fn scan(path: &std::path::Path, mut out: File) -> i32 {
    let result = match backends().iter().find(|b| b.is_candidate(path)) {
        Some(b) => b.scan(path).map_err(|e| e.to_string()),
        None => Err(format!("formato non riconosciuto: {}", path.display())),
    };
    let code = if result.is_ok() { 0 } else { 1 };
    let output = match result {
        Ok(v) => ScanOutput::Ok(v),
        Err(e) => ScanOutput::Err(e),
    };
    let json = serde_json::to_vec(&output).unwrap_or_default();
    if out.write_all(&json).and_then(|_| out.flush()).is_err() {
        return 3;
    }
    code
}

/// Uscita del protocollo condivisa tra i thread.
type Out = Arc<Mutex<File>>;

fn send(out: &Out, frame: &[u8]) -> io::Result<()> {
    proto::write_frame(&mut *out.lock(), frame)
}

fn serve(kind: &str, sample_rate: f32, max_block: usize, out: File) -> i32 {
    let out: Out = Arc::new(Mutex::new(out));
    let mut enc = Enc::new();

    let created = match parse_kind(kind) {
        None => Err(HostError::BadKind(kind.to_string())),
        Some(parsed) => {
            let backend: Box<dyn FormatBackend> = match parsed.format {
                PluginFormat::Vst3 => Box::new(crate::vst3::Vst3Backend::new()),
                PluginFormat::Clap => Box::new(crate::clap::ClapBackend::new()),
            };
            backend.instantiate(&parsed, sample_rate, max_block)
        }
    };
    enc.begin(from_child::READY).u32(proto::PROTOCOL_VERSION);
    let inst = match created {
        Ok(i) => {
            enc.bool(true).info(i.info()).u32(i.latency());
            i
        }
        Err(e) => {
            enc.bool(false).error(&e);
            let _ = send(&out, enc.finish());
            return 1;
        }
    };
    if send(&out, enc.finish()).is_err() {
        return 3;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let (ctrl_tx, ctrl_rx) = crossbeam_channel::unbounded::<(u32, Call)>();
    let (proc_tx, proc_rx) = crossbeam_channel::unbounded::<ProcessReq>();

    let audio = {
        let inst = inst.clone();
        let out = out.clone();
        let stop = stop.clone();
        thread::Builder::new()
            .name("mfc-sandbox-audio".into())
            .spawn(move || audio_loop(inst, proc_rx, out, stop))
    };
    let Ok(audio) = audio else { return 3 };
    {
        // Il lettore resta bloccato su stdin fino alla chiusura: non viene atteso.
        let weak = Arc::downgrade(&inst);
        let stop = stop.clone();
        if thread::Builder::new()
            .name("mfc-sandbox-reader".into())
            .spawn(move || reader_loop(weak, ctrl_tx, proc_tx, stop))
            .is_err()
        {
            return 3;
        }
    }

    main_loop(inst.as_ref(), &ctrl_rx, &out);

    // Chiusura ordinata: l'istanza viene distrutta sul main thread, dopo il thread audio.
    stop.store(true, Ordering::SeqCst);
    let _ = audio.join();
    drop(inst);
    0
}

/// Editor aperto nella finestra dell'helper.
struct OpenEditor {
    window: EditorWindow,
    size: EditorSize,
}

fn main_loop(inst: &dyn ExternalInstance, ctrl_rx: &Receiver<(u32, Call)>, out: &Out) {
    let mut enc = Enc::new();
    let mut editor: Option<OpenEditor> = None;
    let mut last_idle = Instant::now();
    let mut latency = inst.latency();
    loop {
        let msg = if window::active() {
            // Loop degli eventi nativo attivo (finestre dell'editor): pochi ms per giro.
            window::pump(Duration::from_millis(2));
            match ctrl_rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => break,
            }
        } else {
            match ctrl_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };
        if let Some((id, call)) = msg {
            let shutdown = call == Call::Shutdown;
            let reply = handle(inst, call, &mut editor);
            enc.begin(from_child::REPLY).u32(id);
            reply.encode(&mut enc);
            if send(out, enc.finish()).is_err() || shutdown {
                break;
            }
        }
        if last_idle.elapsed() >= Duration::from_millis(15) {
            last_idle = Instant::now();
            inst.idle();
            if let Some(ed) = &mut editor {
                if !ed.window.is_open() {
                    // Chiusa dall'utente.
                    inst.close_editor();
                    ed.window.destroy();
                    editor = None;
                    enc.begin(from_child::EDITOR_CLOSED);
                    let _ = send(out, enc.finish());
                } else if let Some(size) = inst.take_editor_resize() {
                    ed.window.resize(size);
                    ed.size = size;
                }
            }
            let l = inst.latency();
            if l != latency {
                latency = l;
                enc.begin(from_child::LATENCY).u32(l);
                let _ = send(out, enc.finish());
            }
        }
    }
    if let Some(ed) = editor.take() {
        inst.close_editor();
        ed.window.destroy();
    }
}

fn unit(r: Result<(), HostError>) -> Reply {
    match r {
        Ok(()) => Reply::Unit,
        Err(e) => Reply::Err(e),
    }
}

fn handle(inst: &dyn ExternalInstance, call: Call, editor: &mut Option<OpenEditor>) -> Reply {
    match call {
        Call::Prepare { sample_rate, max_block } => unit(inst.prepare(sample_rate, max_block as usize)),
        Call::Params => Reply::Params(inst.params()),
        Call::ParamNormalized(id) => Reply::F64(inst.param_normalized(id)),
        Call::ParamDisplay(id, v) => Reply::Str(inst.param_display(id, v)),
        Call::SaveState => match inst.save_state() {
            Ok(b) => Reply::Bytes(b),
            Err(e) => Reply::Err(e),
        },
        Call::LoadState(data) => unit(inst.load_state(&data)),
        Call::Reset => {
            inst.reset();
            Reply::Unit
        }
        Call::HasEditor => Reply::Bool(window::SUPPORTED && inst.has_editor()),
        Call::OpenEditor(title) => open_editor(inst, &title, editor),
        Call::CloseEditor => {
            if let Some(ed) = editor.take() {
                inst.close_editor();
                ed.window.destroy();
            }
            Reply::Unit
        }
        Call::Idle => {
            inst.idle();
            Reply::Unit
        }
        Call::Latency => Reply::U32(inst.latency()),
        Call::Info => Reply::Info(Box::new(inst.info().clone())),
        Call::Shutdown => Reply::Unit,
    }
}

fn open_editor(inst: &dyn ExternalInstance, title: &str, editor: &mut Option<OpenEditor>) -> Reply {
    if let Some(ed) = editor {
        ed.window.show();
        return Reply::Size(ed.size);
    }
    if !window::SUPPORTED {
        return Reply::Err(HostError::Unsupported(
            "editor in un processo separato non supportato su questa piattaforma".into(),
        ));
    }
    if !inst.has_editor() {
        return Reply::Err(HostError::Unsupported("il plugin non ha un'interfaccia propria".into()));
    }
    let window = match EditorWindow::create(title) {
        Ok(w) => w,
        Err(e) => return Reply::Err(HostError::Plugin(e)),
    };
    match inst.open_editor(window.parent()) {
        Ok(size) => {
            window.resize(size);
            window.show();
            *editor = Some(OpenEditor { window, size });
            Reply::Size(size)
        }
        Err(e) => {
            window.destroy();
            Reply::Err(e)
        }
    }
}

/// Tentativi quando l'istanza è momentaneamente occupata dal main thread.
const BUSY_RETRIES: usize = 20;

fn audio_loop(inst: Arc<dyn ExternalInstance>, rx: Receiver<ProcessReq>, out: Out, stop: Arc<AtomicBool>) {
    let mut enc = Enc::new();
    let (mut l, mut r) = (Vec::new(), Vec::new());
    loop {
        let req = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(req) => req,
            Err(RecvTimeoutError::Timeout) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        l.clear();
        l.resize(req.frames, 0.0f32);
        r.clear();
        r.resize(req.frames, 0.0f32);
        let input = req.input.as_ref().map(|(a, b)| (a.as_slice(), b.as_slice()));
        let mut ok = false;
        for _ in 0..BUSY_RETRIES {
            if inst.process(&req.events, input, &mut l, &mut r) {
                ok = true;
                break;
            }
            thread::sleep(Duration::from_micros(200));
        }
        enc.begin(from_child::PROCESSED).u32(req.seq).bool(ok).f32s(&l).f32s(&r);
        if send(&out, enc.finish()).is_err() {
            break;
        }
    }
}

fn reader_loop(
    inst: Weak<dyn ExternalInstance>,
    ctrl_tx: Sender<(u32, Call)>,
    proc_tx: Sender<ProcessReq>,
    stop: Arc<AtomicBool>,
) {
    let stdin = io::stdin();
    let mut r = BufReader::with_capacity(1 << 16, stdin.lock());
    let mut buf = Vec::new();
    // Istanza per le operazioni consentite da qualunque thread (mai dopo l'arresto).
    let with_inst = |f: &dyn Fn(&dyn ExternalInstance)| {
        if !stop.load(Ordering::SeqCst) {
            if let Some(i) = inst.upgrade() {
                f(i.as_ref());
            }
        }
    };
    while let Ok(true) = proto::read_frame(&mut r, &mut buf) {
        let mut d = Dec::new(&buf);
        let res = (|| -> io::Result<bool> {
            match d.u8()? {
                to_child::CALL => {
                    let id = d.u32()?;
                    let call = Call::decode(&mut d)?;
                    Ok(ctrl_tx.send((id, call)).is_ok())
                }
                to_child::SET_PARAM => {
                    let (id, v) = (d.u32()?, d.f64()?);
                    with_inst(&|i| i.set_param_normalized(id, v));
                    Ok(true)
                }
                to_child::RESET => {
                    with_inst(&|i| i.reset());
                    Ok(true)
                }
                to_child::PROCESS => Ok(proc_tx.send(ProcessReq::decode(&mut d)?).is_ok()),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tipo di messaggio sconosciuto",
                )),
            }
        })();
        if !matches!(res, Ok(true)) {
            break;
        }
    }
    // Fine di stdin (host chiuso o terminato): i canali si chiudono e il main thread esce.
}

/// Riserva lo stdout originale al protocollo e redirige il descrittore 1 su stderr.
#[cfg(unix)]
fn protocol_stdout() -> io::Result<File> {
    use std::os::fd::FromRawFd;
    // SAFETY: operazioni sui descrittori standard del processo, all'avvio (nessun altro
    // thread li sta usando); il descrittore duplicato passa in proprietà al `File`.
    unsafe {
        let fd = libc::dup(1);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::dup2(2, 1);
        Ok(File::from_raw_fd(fd))
    }
}

/// Riserva lo stdout originale al protocollo e sostituisce l'handle standard con stderr.
#[cfg(windows)]
fn protocol_stdout() -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    // SAFETY: handle standard del processo, all'avvio; l'handle di stdout passa in
    // proprietà al `File` e non è più raggiungibile con GetStdHandle.
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(io::Error::other("handle di stdout non valido"));
        }
        SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_ERROR_HANDLE));
        Ok(File::from_raw_handle(h as _))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_arguments() {
        assert_eq!(
            parse_args(&args(&["--scan", "/a b/x.clap"])).unwrap(),
            Mode::Scan("/a b/x.clap".into())
        );
        assert_eq!(
            parse_args(&args(&[
                "--kind",
                "clap:/x.clap::id",
                "--sample-rate",
                "44100",
                "--max-block",
                "512"
            ]))
            .unwrap(),
            Mode::Serve {
                kind: "clap:/x.clap::id".into(),
                sample_rate: 44_100.0,
                max_block: 512,
            }
        );
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["--kind"])).is_err());
        assert!(parse_args(&args(&["--kind", "k", "--sample-rate", "x"])).is_err());
        assert!(parse_args(&args(&["--kind", "k", "--max-block", "0"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
    }

    #[test]
    fn scan_output_json() {
        let s = serde_json::to_string(&ScanOutput::Err("x".into())).unwrap();
        assert_eq!(s, r#"{"err":"x"}"#);
        let s = serde_json::to_string(&ScanOutput::Ok(vec![])).unwrap();
        assert_eq!(s, r#"{"ok":[]}"#);
    }
}
