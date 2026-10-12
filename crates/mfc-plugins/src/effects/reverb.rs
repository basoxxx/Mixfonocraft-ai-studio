//! Riverbero algoritmico stile Freeverb: 8 comb + 4 allpass per canale, spread stereo.

use super::{apply_defaults, ensure_delay, frames, param_value, sr_ok, DEFAULT_SR, MAX_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{flush_denormal, DelayLine, OnePole, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::REVERB,
    name: "Reverb",
    role: PluginRole::Effect,
    description: "Riverbero Freeverb (comb + allpass) con predelay, smorzamento e taglio delle alte.",
    params: &[
        ParamDesc::new("room_size", "Room size", 0.0, 1.0, 0.6, ""),
        ParamDesc::new("damping", "Damping", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("width", "Width", 0.0, 1.0, 1.0, ""),
        ParamDesc::new("wet", "Wet", 0.0, 1.0, 0.3, ""),
        ParamDesc::new("dry", "Dry", 0.0, 1.0, 1.0, ""),
        ParamDesc::new("predelay", "Predelay", 0.0, 200.0, 0.0, "ms"),
        ParamDesc::new("high_cut", "High cut", 1_000.0, 20_000.0, 12_000.0, "Hz"),
    ],
};

/// Lunghezze originali (campioni a 44.1 kHz).
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;
const FIXED_GAIN: f32 = 0.015;
const WET_SCALE: f32 = 3.0;
const MAX_PREDELAY_S: f32 = 0.2;

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
    store: f32,
}

impl Comb {
    fn new(cap: usize) -> Self {
        Self {
            buf: vec![0.0; cap.max(1)],
            len: 1,
            pos: 0,
            store: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.pos];
        self.store = flush_denormal(out * (1.0 - damp) + self.store * damp);
        self.buf[self.pos] = x + self.store * feedback;
        self.pos += 1;
        if self.pos >= self.len {
            self.pos = 0;
        }
        out
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
}

impl Allpass {
    fn new(cap: usize) -> Self {
        Self {
            buf: vec![0.0; cap.max(1)],
            len: 1,
            pos: 0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        self.buf[self.pos] = flush_denormal(x + b * 0.5);
        self.pos += 1;
        if self.pos >= self.len {
            self.pos = 0;
        }
        b - x
    }
}

/// Lunghezza scalata al sample rate (limitata alla capacità).
fn scaled(n: usize, sr: f32, cap: usize) -> usize {
    ((n as f32 * sr / 44_100.0).round() as usize).clamp(1, cap)
}

/// Riverbero stereo.
pub struct Reverb {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    predelay: [DelayLine; 2],
    predelay_samples: f32,
    predelay_ms: f32,
    high_cut: f32,
    lp: [OnePole; 2],
    feedback: f32,
    damp: f32,
    width: f32,
    wet: Smoother,
    dry: Smoother,
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Reverb {
    pub fn new() -> Self {
        let scale = MAX_SR / 44_100.0;
        let cap = |n: usize| ((n + SPREAD) as f32 * scale).ceil() as usize + 1;
        let combs = || COMBS.iter().map(|&n| Comb::new(cap(n))).collect::<Vec<_>>();
        let aps = || ALLPASSES.iter().map(|&n| Allpass::new(cap(n))).collect::<Vec<_>>();
        let pd = super::capacity(MAX_PREDELAY_S, MAX_SR);
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            combs: [combs(), combs()],
            allpasses: [aps(), aps()],
            predelay: [DelayLine::new(pd), DelayLine::new(pd)],
            predelay_samples: 0.0,
            predelay_ms: 0.0,
            high_cut: 12_000.0,
            lp: [OnePole::default(); 2],
            feedback: 0.84,
            damp: 0.2,
            width: 1.0,
            wet: Smoother::new(0.02, DEFAULT_SR, 0.3),
            dry: Smoother::new(0.02, DEFAULT_SR, 1.0),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.set_sample_rate(DEFAULT_SR);
        s
    }

    fn configure_lengths(&mut self) {
        let sr = self.sr;
        for (ch, extra) in [(0usize, 0usize), (1, SPREAD)] {
            for (c, &n) in self.combs[ch].iter_mut().zip(&COMBS) {
                if c.buf.len() < scaled(n + extra, sr, usize::MAX) {
                    // Sample rate oltre MAX_SR: rialloca (mai dal thread audio, vedi set_sample_rate).
                    c.buf = vec![0.0; scaled(n + extra, sr, usize::MAX)];
                }
                c.len = scaled(n + extra, sr, c.buf.len());
            }
            for (a, &n) in self.allpasses[ch].iter_mut().zip(&ALLPASSES) {
                if a.buf.len() < scaled(n + extra, sr, usize::MAX) {
                    a.buf = vec![0.0; scaled(n + extra, sr, usize::MAX)];
                }
                a.len = scaled(n + extra, sr, a.buf.len());
            }
        }
    }

    fn update_filters(&mut self) {
        for f in &mut self.lp {
            f.set_cutoff(self.high_cut, self.sr);
        }
        self.predelay_samples = self.predelay_ms * 0.001 * self.sr;
    }
}

impl Effect for Reverb {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        self.configure_lengths();
        for d in &mut self.predelay {
            ensure_delay(d, MAX_PREDELAY_S, sample_rate);
        }
        self.wet.set_sample_rate(sample_rate);
        self.dry.set_sample_rate(sample_rate);
        self.update_filters();
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "room_size" => self.feedback = 0.7 + 0.28 * v,
            "damping" => self.damp = 0.4 * v,
            "width" => self.width = v,
            "wet" => self.wet.set_target(v * WET_SCALE),
            "dry" => self.dry.set_target(v),
            "predelay" => {
                self.predelay_ms = v;
                self.update_filters();
            }
            "high_cut" => {
                self.high_cut = v;
                self.update_filters();
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            for c in &mut self.combs[ch] {
                c.buf.fill(0.0);
                c.pos = 0;
                c.store = 0.0;
            }
            for a in &mut self.allpasses[ch] {
                a.buf.fill(0.0);
                a.pos = 0;
            }
            self.predelay[ch].reset();
            self.lp[ch].reset();
        }
        self.wet.set_immediate(self.wet.target());
        self.dry.set_immediate(self.dry.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        let (fb, damp) = (self.feedback, self.damp);
        let wet1 = 0.5 * self.width + 0.5;
        let wet2 = 0.5 * (1.0 - self.width);
        let pd = self.predelay_samples;
        for i in 0..n {
            let (l, r) = (left[i], right[i]);
            let input = (l + r) * FIXED_GAIN;
            let mut out = [0.0f32; 2];
            for (ch, o) in out.iter_mut().enumerate() {
                let x = self.predelay[ch].process(input, pd);
                let mut acc = 0.0;
                for c in &mut self.combs[ch] {
                    acc += c.process(x, fb, damp);
                }
                for a in &mut self.allpasses[ch] {
                    acc = a.process(acc);
                }
                *o = self.lp[ch].lowpass(acc);
            }
            let wet = self.wet.next_sample();
            let dry = self.dry.next_sample();
            left[i] = l * dry + (out[0] * wet1 + out[1] * wet2) * wet;
            right[i] = r * dry + (out[1] * wet1 + out[0] * wet2) * wet;
        }
    }
}
