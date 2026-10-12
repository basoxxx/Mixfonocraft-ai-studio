//! Backend "null": fa avanzare il motore in tempo reale scartando l'audio.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::engine::Engine;

/// Thread che pilota il motore senza dispositivo audio.
pub struct NullBackend {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Engine>>,
}

impl NullBackend {
    pub fn start(mut engine: Engine, block: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let block = block.clamp(32, 8192);
        let thread = std::thread::spawn(move || {
            let mut l = vec![0.0; block];
            let mut r = vec![0.0; block];
            let period = Duration::from_secs_f64(block as f64 / engine.sample_rate() as f64);
            let mut next = Instant::now();
            while !flag.load(Ordering::Relaxed) {
                engine.process(&mut l, &mut r);
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
            engine
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    /// Ferma il thread e restituisce il motore.
    pub fn stop(mut self) -> Option<Engine> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().and_then(|t| t.join().ok())
    }
}

impl Drop for NullBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
