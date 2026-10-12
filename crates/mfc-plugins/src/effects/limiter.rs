//! Limiter brickwall con lookahead (5 ms): il picco d'uscita non supera mai il ceiling.
//!
//! Schema: guadagno richiesto per campione → minimo su finestra W → release → media mobile di
//! lunghezza W; l'audio è ritardato di W-1 campioni, così il guadagno è già sceso quando arriva il picco.

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR, MAX_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{db_to_gain, time_coef, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::LIMITER,
    name: "Limiter",
    role: PluginRole::Effect,
    description: "Limiter brickwall con lookahead di 5 ms (latenza riportata) e release regolabile.",
    params: &[
        ParamDesc::new("ceiling", "Ceiling", -24.0, 0.0, -0.3, "dB"),
        ParamDesc::new("gain", "Input gain", 0.0, 24.0, 0.0, "dB"),
        ParamDesc::new("release", "Release", 1.0, 1000.0, 50.0, "ms"),
    ],
};

const LOOKAHEAD_S: f32 = 0.005;

/// Limiter stereo-linked con lookahead.
pub struct Limiter {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    /// Lunghezza della finestra (campioni); latenza = w - 1.
    w: usize,
    delay: [Vec<f32>; 2],
    dpos: usize,
    // Deque monotona (minimo su finestra scorrevole) in un ring preallocato.
    dq_val: Vec<f32>,
    dq_idx: Vec<u64>,
    dq_head: usize,
    dq_len: usize,
    n: u64,
    // Media mobile.
    box_buf: Vec<f32>,
    box_pos: usize,
    box_sum: f64,
    rel_state: f32,
    rel_coef: f32,
    release_ms: f32,
    ceiling: f32,
    input: Smoother,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            w: 1,
            delay: [Vec::new(), Vec::new()],
            dpos: 0,
            dq_val: Vec::new(),
            dq_idx: Vec::new(),
            dq_head: 0,
            dq_len: 0,
            n: 0,
            box_buf: Vec::new(),
            box_pos: 0,
            box_sum: 0.0,
            rel_state: 1.0,
            rel_coef: 0.0,
            release_ms: 50.0,
            ceiling: 1.0,
            input: Smoother::new(0.01, DEFAULT_SR, 1.0),
        };
        s.allocate(MAX_SR);
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.set_sample_rate(DEFAULT_SR);
        s
    }

    fn allocate(&mut self, sr: f32) {
        let cap = (LOOKAHEAD_S * sr).ceil() as usize + 2;
        if self.box_buf.len() < cap {
            self.delay = [vec![0.0; cap], vec![0.0; cap]];
            self.box_buf = vec![1.0; cap];
            self.dq_val = vec![1.0; cap];
            self.dq_idx = vec![0; cap];
        }
    }

    /// Ceiling lineare.
    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }
}

impl Effect for Limiter {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        self.allocate(sample_rate);
        self.w = ((LOOKAHEAD_S * sample_rate).round() as usize).clamp(1, self.box_buf.len() - 1);
        self.rel_coef = time_coef(self.release_ms * 0.001, sample_rate);
        self.input.set_sample_rate(sample_rate);
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "ceiling" => self.ceiling = db_to_gain(v),
            "gain" => self.input.set_target(db_to_gain(v)),
            "release" => {
                self.release_ms = v;
                self.rel_coef = time_coef(v * 0.001, self.sr);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.dpos = 0;
        self.dq_head = 0;
        self.dq_len = 0;
        self.n = 0;
        self.box_buf.fill(1.0);
        self.box_pos = 0;
        self.box_sum = self.w as f64;
        self.rel_state = 1.0;
        self.input.set_immediate(self.input.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let count = frames(left, right);
        let w = self.w;
        let cap = self.dq_val.len();
        let ceil = self.ceiling;
        for i in 0..count {
            let g_in = self.input.next_sample();
            let xl = left[i] * g_in;
            let xr = right[i] * g_in;
            let peak = xl.abs().max(xr.abs());
            let req = if peak > ceil && peak.is_finite() {
                ceil / peak
            } else {
                1.0
            };
            // Minimo sulla finestra [n-w+1, n].
            while self.dq_len > 0 {
                let back = (self.dq_head + self.dq_len - 1) % cap;
                if self.dq_val[back] >= req {
                    self.dq_len -= 1;
                } else {
                    break;
                }
            }
            let slot = (self.dq_head + self.dq_len) % cap;
            self.dq_val[slot] = req;
            self.dq_idx[slot] = self.n;
            self.dq_len += 1;
            while self.dq_idx[self.dq_head] + (w as u64) <= self.n {
                self.dq_head = (self.dq_head + 1) % cap;
                self.dq_len -= 1;
            }
            let hmin = self.dq_val[self.dq_head];
            // Attacco istantaneo (già anticipato dal lookahead), release esponenziale.
            self.rel_state = if hmin < self.rel_state {
                hmin
            } else {
                hmin + self.rel_coef * (self.rel_state - hmin)
            };
            self.box_sum += (self.rel_state - self.box_buf[self.box_pos]) as f64;
            self.box_buf[self.box_pos] = self.rel_state;
            self.box_pos = (self.box_pos + 1) % w;
            let g = (self.box_sum / w as f64) as f32;
            // Ritardo di w-1 campioni.
            self.delay[0][self.dpos] = xl;
            self.delay[1][self.dpos] = xr;
            let rd = (self.dpos + 1) % w;
            let (dl, dr) = (self.delay[0][rd], self.delay[1][rd]);
            self.dpos = rd;
            left[i] = (dl * g).clamp(-ceil, ceil);
            right[i] = (dr * g).clamp(-ceil, ceil);
            self.n += 1;
        }
    }

    fn latency(&self) -> usize {
        self.w - 1
    }
}
