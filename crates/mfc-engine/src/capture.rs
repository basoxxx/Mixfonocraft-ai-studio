//! Registrazione dell'ingresso audio (microfono/strumento): nucleo indipendente dal dispositivo.
//!
//! Il percorso dei dati è:
//!
//! ```text
//! callback audio ──► CaptureProducer ──(ring SPSC lock-free)──► CaptureConsumer ──► CaptureWriter (thread) ──► WAV
//! ```
//!
//! - [`capture_channel`] crea un ring buffer SPSC preallocato di frame stereo `f32`.
//!   Il lato [`CaptureProducer`] è real-time safe (nessuna allocazione né lock): converte
//!   i campioni interleaved in stereo (mono duplicato, canali oltre il secondo ignorati),
//!   aggiorna il picco e, se il ring è pieno, scarta i frame contandoli.
//! - [`CaptureWriter`] svuota il ring da un thread dedicato e scrive un WAV con `hound`
//!   alla frequenza del dispositivo; [`CaptureWriter::finish`] chiude il file e restituisce
//!   un [`RecordedTake`].
//! - [`LevelMeter`] legge da qualunque thread il picco d'ingresso (per il VU della GUI).
//! - [`take_placement`] calcola dove posare la take nella Playlist (tick d'inizio e lunghezza).
//!
//! Con la feature `realtime`, `backend::device::InputRecorder` collega una stream d'ingresso
//! cpal a questo nucleo. Integrazione tipica nella GUI:
//!
//! ```ignore
//! // Avvio: posizione del transport al momento dell'avvio della registrazione.
//! let start_tick = handle.shared.position();
//! let rec = InputRecorder::start(None, &path)?;       // dispositivo di default
//! // Ogni frame della GUI: VU d'ingresso.
//! let peak = rec.level();
//! // Stop: chiude il WAV e calcola la clip.
//! let take = rec.stop()?;
//! let place = take_placement(&project.time, start_tick, take.frames, take.sample_rate, project.tempo);
//! // → canale Audio Clip con `sample_path = take.path`, clip a `place.position` lunga `place.length`.
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use mfc_core::{Tick, TimeBase};

use crate::render::WavFormat;

/// Stato condiviso tra produttore e consumatore.
struct Shared {
    /// Frame stereo come coppie di bit `f32` (L, R) contigue.
    buf: Box<[AtomicU32]>,
    /// Capacità in frame.
    cap: usize,
    /// Frame scritti in totale (solo il produttore incrementa).
    head: AtomicUsize,
    /// Frame letti in totale (solo il consumatore incrementa).
    tail: AtomicUsize,
    /// Picco assoluto dall'ultima lettura (bit `f32`).
    peak: AtomicU32,
    /// Frame scartati perché il ring era pieno.
    dropped: AtomicU64,
}

/// Crea un ring buffer SPSC con spazio per almeno `capacity_frames` frame stereo
/// (arrotondati alla potenza di 2 successiva, minimo 64: gli indici restano coerenti al wrap).
/// Alloca: va chiamata fuori dal thread audio.
pub fn capture_channel(capacity_frames: usize) -> (CaptureProducer, CaptureConsumer) {
    let cap = capacity_frames.max(64).next_power_of_two();
    let shared = Arc::new(Shared {
        buf: (0..cap * 2).map(|_| AtomicU32::new(0)).collect(),
        cap,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        peak: AtomicU32::new(0),
        dropped: AtomicU64::new(0),
    });
    (CaptureProducer { shared: shared.clone() }, CaptureConsumer { shared })
}

/// Lato scrittura del ring (callback audio). Real-time safe.
pub struct CaptureProducer {
    shared: Arc<Shared>,
}

impl CaptureProducer {
    /// Frame che possono essere accodati senza perdite in questo momento.
    pub fn free(&self) -> usize {
        let s = &*self.shared;
        s.cap
            - s.head
                .load(Ordering::Relaxed)
                .wrapping_sub(s.tail.load(Ordering::Acquire))
    }

    /// Accoda frame stereo; restituisce quanti sono entrati (gli altri sono contati come scartati).
    pub fn push_frames<I: IntoIterator<Item = (f32, f32)>>(&mut self, frames: I) -> usize {
        let s = &*self.shared;
        let head = s.head.load(Ordering::Relaxed);
        let tail = s.tail.load(Ordering::Acquire);
        let free = s.cap - head.wrapping_sub(tail);
        let mut written = 0usize;
        let mut lost = 0u64;
        let mut peak = 0f32;
        for (l, r) in frames {
            let (l, r) = (finite(l), finite(r));
            peak = peak.max(l.abs()).max(r.abs());
            if written < free {
                let i = (head.wrapping_add(written) % s.cap) * 2;
                s.buf[i].store(l.to_bits(), Ordering::Relaxed);
                s.buf[i + 1].store(r.to_bits(), Ordering::Relaxed);
                written += 1;
            } else {
                lost += 1;
            }
        }
        s.head.store(head.wrapping_add(written), Ordering::Release);
        if lost > 0 {
            s.dropped.fetch_add(lost, Ordering::Relaxed);
        }
        let _ = s.peak.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
            (peak > f32::from_bits(cur)).then_some(peak.to_bits())
        });
        written
    }

    /// Accoda campioni interleaved a `channels` canali, convertiti con `to_f32`.
    /// Mono → duplicato su L/R; dal terzo canale in poi ignorati. Restituisce i frame accodati.
    pub fn push_interleaved<T: Copy>(&mut self, data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) -> usize {
        match channels {
            0 => 0,
            1 => self.push_frames(data.iter().map(|&x| {
                let v = to_f32(x);
                (v, v)
            })),
            ch => self.push_frames(data.chunks_exact(ch).map(|f| (to_f32(f[0]), to_f32(f[1])))),
        }
    }

    /// Lettore del livello d'ingresso condiviso con questo ring.
    pub fn meter(&self) -> LevelMeter {
        LevelMeter {
            shared: self.shared.clone(),
        }
    }
}

/// Lato lettura del ring (thread di scrittura su disco).
pub struct CaptureConsumer {
    shared: Arc<Shared>,
}

impl CaptureConsumer {
    /// Frame disponibili alla lettura.
    pub fn available(&self) -> usize {
        let s = &*self.shared;
        s.head
            .load(Ordering::Acquire)
            .wrapping_sub(s.tail.load(Ordering::Relaxed))
    }

    /// Estrae tutti i frame disponibili passandoli a `f`; restituisce quanti.
    pub fn pop_frames(&mut self, mut f: impl FnMut(f32, f32)) -> usize {
        let s = &*self.shared;
        let tail = s.tail.load(Ordering::Relaxed);
        let head = s.head.load(Ordering::Acquire);
        let n = head.wrapping_sub(tail);
        for k in 0..n {
            let i = (tail.wrapping_add(k) % s.cap) * 2;
            f(
                f32::from_bits(s.buf[i].load(Ordering::Relaxed)),
                f32::from_bits(s.buf[i + 1].load(Ordering::Relaxed)),
            );
        }
        s.tail.store(head, Ordering::Release);
        n
    }

    /// Lettore del livello d'ingresso condiviso con questo ring.
    pub fn meter(&self) -> LevelMeter {
        LevelMeter {
            shared: self.shared.clone(),
        }
    }
}

/// Picco d'ingresso e statistiche, leggibili da qualunque thread.
#[derive(Clone)]
pub struct LevelMeter {
    shared: Arc<Shared>,
}

impl LevelMeter {
    /// Picco assoluto (lineare, 0..) dall'ultima chiamata; la lettura lo azzera.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.shared.peak.swap(0, Ordering::Relaxed))
    }
    /// Frame scartati finora perché il disco non teneva il passo.
    pub fn dropped_frames(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// Take registrata e chiusa su disco.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedTake {
    pub path: PathBuf,
    /// Frequenza del file (= quella del dispositivo d'ingresso).
    pub sample_rate: u32,
    /// Frame stereo scritti.
    pub frames: u64,
    /// Frame persi per ring pieno (0 in condizioni normali).
    pub dropped_frames: u64,
}

type Wav = hound::WavWriter<std::io::BufWriter<std::fs::File>>;

/// Thread che svuota il ring in un file WAV stereo.
pub struct CaptureWriter {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<u64, String>>>,
    path: PathBuf,
    sample_rate: u32,
    meter: LevelMeter,
}

/// Pausa del thread di scrittura quando il ring è vuoto.
const POLL: Duration = Duration::from_millis(5);

impl CaptureWriter {
    /// Crea il file (errore immediato se il percorso non è scrivibile) e avvia il thread.
    pub fn spawn(consumer: CaptureConsumer, path: &Path, sample_rate: u32, format: WavFormat) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("frequenza di campionamento nulla".into());
        }
        let (bits, fmt) = match format {
            WavFormat::Int16 => (16, hound::SampleFormat::Int),
            WavFormat::Int24 => (24, hound::SampleFormat::Int),
            WavFormat::Float32 => (32, hound::SampleFormat::Float),
        };
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample: bits,
            sample_format: fmt,
        };
        let writer = hound::WavWriter::create(path, spec)
            .map_err(|e| format!("impossibile creare \"{}\": {e}", path.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let meter = consumer.meter();
        let thread = std::thread::Builder::new()
            .name("mfc-capture-writer".into())
            .spawn(move || write_loop(consumer, writer, format, &flag))
            .map_err(|e| format!("thread di scrittura: {e}"))?;
        Ok(Self {
            stop,
            thread: Some(thread),
            path: path.to_path_buf(),
            sample_rate,
            meter,
        })
    }

    /// Livello d'ingresso del ring collegato.
    pub fn meter(&self) -> &LevelMeter {
        &self.meter
    }

    /// Scrive i frame rimasti, finalizza il WAV e restituisce la take.
    /// Va chiamata DOPO aver fermato il produttore (stream chiusa).
    pub fn finish(mut self) -> Result<RecordedTake, String> {
        let frames = self.join()?;
        Ok(RecordedTake {
            path: self.path.clone(),
            sample_rate: self.sample_rate,
            frames,
            dropped_frames: self.meter.dropped_frames(),
        })
    }

    fn join(&mut self) -> Result<u64, String> {
        self.stop.store(true, Ordering::Release);
        match self.thread.take() {
            Some(t) => t
                .join()
                .map_err(|_| "thread di scrittura terminato con errore".to_string())?,
            None => Err("registrazione già chiusa".into()),
        }
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.join();
        }
    }
}

fn write_loop(
    mut consumer: CaptureConsumer,
    mut wav: Wav,
    format: WavFormat,
    stop: &AtomicBool,
) -> Result<u64, String> {
    let mut frames = 0u64;
    let mut error: Option<hound::Error> = None;
    loop {
        // Il flag va letto PRIMA dello svuotamento: dopo lo stop il produttore è fermo,
        // quindi un ultimo giro vuoto garantisce che tutto sia stato scritto.
        let stopping = stop.load(Ordering::Acquire);
        let n = consumer.pop_frames(|l, r| {
            if error.is_none() {
                if let Err(e) = write_frame(&mut wav, format, l, r) {
                    error = Some(e);
                }
            }
        });
        frames += n as u64;
        if n == 0 {
            if stopping {
                break;
            }
            std::thread::sleep(POLL);
        }
    }
    if let Some(e) = error {
        return Err(format!("scrittura WAV: {e}"));
    }
    wav.finalize().map_err(|e| format!("chiusura WAV: {e}"))?;
    Ok(frames)
}

fn write_frame(w: &mut Wav, format: WavFormat, l: f32, r: f32) -> Result<(), hound::Error> {
    for s in [l, r] {
        match format {
            WavFormat::Float32 => w.write_sample(s)?,
            WavFormat::Int16 => w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?,
            WavFormat::Int24 => w.write_sample((s.clamp(-1.0, 1.0) * 8_388_607.0) as i32)?,
        }
    }
    Ok(())
}

/// Posizione e lunghezza (tick) di una take nella Playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakePlacement {
    /// Inizio della clip (tick di song).
    pub position: Tick,
    /// Lunghezza della clip (tick, almeno 1 se la take non è vuota).
    pub length: Tick,
}

/// Dove posare una take: `start_tick` è la posizione del transport (in tick, es.
/// `SharedState::position()`) all'avvio della registrazione; la lunghezza deriva da
/// `frames` / `sample_rate` al tempo `bpm` (tempo costante). Funzione pura.
pub fn take_placement(time: &TimeBase, start_tick: f64, frames: u64, sample_rate: u32, bpm: f64) -> TakePlacement {
    let position = if start_tick.is_finite() {
        start_tick.max(0.0).round() as Tick
    } else {
        0
    };
    if frames == 0 || sample_rate == 0 || !(bpm.is_finite() && bpm > 0.0) {
        return TakePlacement { position, length: 0 };
    }
    let seconds = frames as f64 / sample_rate as f64;
    let length = (time.seconds_to_ticks(seconds, bpm).round() as Tick).max(1);
    TakePlacement { position, length }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mfc-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// Campione sintetico deterministico (rampa periodica in -0.5..0.5).
    fn synth(i: usize) -> f32 {
        ((i % 1000) as f32 / 1000.0) - 0.5
    }

    #[test]
    fn ring_preserves_order_and_counts_drops() {
        let (mut p, mut c) = capture_channel(64);
        assert_eq!(p.push_frames((0..50).map(|i| (i as f32, -(i as f32)))), 50);
        // Solo 14 posti liberi.
        assert_eq!(p.push_frames((50..70).map(|i| (i as f32, -(i as f32)))), 14);
        assert_eq!(p.meter().dropped_frames(), 6);
        let mut got = Vec::new();
        assert_eq!(c.pop_frames(|l, r| got.push((l, r))), 64);
        assert!(got
            .iter()
            .enumerate()
            .all(|(i, &(l, r))| l == i as f32 && r == -(i as f32)));
        // Dopo lo svuotamento c'è di nuovo posto (wrap-around).
        assert_eq!(p.push_frames((0..40).map(|i| (i as f32, 0.0))), 40);
        got.clear();
        c.pop_frames(|l, _| got.push((l, 0.0)));
        assert_eq!(got.len(), 40);
        assert_eq!(got[39].0, 39.0);
    }

    #[test]
    fn interleaved_mono_is_duplicated_and_peak_is_tracked() {
        let (mut p, mut c) = capture_channel(256);
        let meter = p.meter();
        let mono: [i16; 4] = [0, 16_384, -32_768, 8_192];
        assert_eq!(p.push_interleaved(&mono, 1, |s| s as f32 / 32_768.0), 4);
        let quad = [0.1f32, 0.2, 9.0, 9.0, 0.3, -0.4, 9.0, 9.0];
        assert_eq!(p.push_interleaved(&quad, 4, |s| s), 2);
        let mut got = Vec::new();
        c.pop_frames(|l, r| got.push((l, r)));
        assert_eq!(
            got,
            vec![
                (0.0, 0.0),
                (0.5, 0.5),
                (-1.0, -1.0),
                (0.25, 0.25),
                (0.1, 0.2),
                (0.3, -0.4)
            ]
        );
        assert_eq!(meter.take_peak(), 1.0);
        assert_eq!(meter.take_peak(), 0.0);
    }

    #[test]
    fn writer_thread_produces_expected_wav() {
        let path = tmp("float.wav");
        let total = 48_000usize;
        let (mut p, c) = capture_channel(1024);
        let w = CaptureWriter::spawn(c, &path, 48_000, WavFormat::Float32).unwrap();
        // Produttore su un altro thread, a blocchi da 128 come una callback audio;
        // attende spazio libero (qui non si vogliono perdite: il ring è piccolo apposta).
        let producer = std::thread::spawn(move || {
            let mut i = 0;
            while i < total {
                let n = 128.min(total - i);
                while p.free() < n {
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(p.push_frames((i..i + n).map(|k| (synth(k), -synth(k)))), n);
                i += n;
            }
            p
        });
        let p = producer.join().unwrap();
        assert_eq!(p.meter().dropped_frames(), 0, "frame persi");
        let take = w.finish().unwrap();
        assert_eq!(take.frames, total as u64);
        assert_eq!(take.sample_rate, 48_000);
        assert_eq!(take.dropped_frames, 0);
        let mut r = hound::WavReader::open(&take.path).unwrap();
        let spec = r.spec();
        assert_eq!((spec.channels, spec.sample_rate, spec.bits_per_sample), (2, 48_000, 32));
        let samples: Vec<f32> = r.samples::<f32>().map(Result::unwrap).collect();
        assert_eq!(samples.len(), total * 2);
        for (k, fr) in samples.chunks(2).enumerate() {
            assert_eq!(fr[0], synth(k), "frame {k}");
            assert_eq!(fr[1], -synth(k), "frame {k}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn writer_24_bit_roundtrip() {
        let path = tmp("int24.wav");
        let (mut p, c) = capture_channel(4096);
        let w = CaptureWriter::spawn(c, &path, 44_100, WavFormat::Int24).unwrap();
        p.push_frames((0..3000).map(|k| (synth(k), 0.25)));
        drop(p);
        let take = w.finish().unwrap();
        assert_eq!(take.frames, 3000);
        let mut r = hound::WavReader::open(&path).unwrap();
        assert_eq!(r.spec().bits_per_sample, 24);
        assert_eq!(r.spec().sample_rate, 44_100);
        let s: Vec<i32> = r.samples::<i32>().map(Result::unwrap).collect();
        assert_eq!(s.len(), 6000);
        for (k, fr) in s.chunks(2).enumerate() {
            let want = synth(k);
            assert!((fr[0] as f32 / 8_388_607.0 - want).abs() < 1e-6, "frame {k}");
            assert!((fr[1] as f32 / 8_388_607.0 - 0.25).abs() < 1e-6);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn writer_reports_bad_path() {
        let (_p, c) = capture_channel(64);
        let bad = Path::new("/nonexistent-dir-mfc/take.wav");
        assert!(CaptureWriter::spawn(c, bad, 48_000, WavFormat::Int24).is_err());
    }

    #[test]
    fn placement_from_transport_and_tempo() {
        let tb = TimeBase::default(); // 96 PPQ
                                      // 2 s a 120 BPM = 4 beat = 384 tick; inizio alla battuta 2 (tick 384).
        let p = take_placement(&tb, 384.0, 96_000, 48_000, 120.0);
        assert_eq!(
            p,
            TakePlacement {
                position: 384,
                length: 384
            }
        );
        // 1 s a 90 BPM = 1.5 beat = 144 tick; posizione frazionaria arrotondata.
        let p = take_placement(&tb, 100.6, 44_100, 44_100, 90.0);
        assert_eq!(
            p,
            TakePlacement {
                position: 101,
                length: 144
            }
        );
        // Take brevissima: almeno 1 tick. Take vuota o input non validi: lunghezza 0.
        assert_eq!(take_placement(&tb, 0.0, 1, 48_000, 120.0).length, 1);
        assert_eq!(
            take_placement(&tb, -5.0, 0, 48_000, 120.0),
            TakePlacement { position: 0, length: 0 }
        );
        assert_eq!(take_placement(&tb, f64::NAN, 10, 0, 120.0).position, 0);
    }
}
