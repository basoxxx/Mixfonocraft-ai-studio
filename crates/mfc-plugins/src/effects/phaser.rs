//! Phaser: catena di all-pass del primo ordine con frequenza modulata da LFO e feedback.

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use crate::sync::{lfo_rate, sanitize_bpm, DEFAULT_BPM, SYNC_CHOICES};
use mfc_dsp::{flush_denormal, Lfo, Smoother};

/// Numero di stadi selezionabili (valore del parametro `stages`).
pub const STAGES: &[&str] = &["2", "4", "6", "8", "10", "12"];
const MAX_STAGES: usize = 12;

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::PHASER,
    name: "Phaser",
    role: PluginRole::Effect,
    description: "Phaser stereo a 2-12 stadi all-pass con feedback e sfasamento dell'LFO tra i canali.",
    params: &[
        ParamDesc::choice("stages", "Stages", STAGES, 1),
        ParamDesc::new("rate", "Rate", 0.05, 10.0, 0.5, "Hz"),
        ParamDesc::choice("sync", "Sync", SYNC_CHOICES, 0),
        ParamDesc::new("depth", "Depth", 0.0, 1.0, 0.7, ""),
        ParamDesc::new("center", "Center", 100.0, 4_000.0, 800.0, "Hz"),
        ParamDesc::new("feedback", "Feedback", -0.9, 0.9, 0.4, ""),
        ParamDesc::new("mix", "Mix", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("stereo", "Stereo phase", 0.0, 1.0, 0.25, ""),
    ],
};

/// Escursione della modulazione a depth = 1 (ottave, ±).
const SWEEP_OCT: f32 = 2.5;

/// Phaser stereo.
pub struct Phaser {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    stages: usize,
    state: [[f32; MAX_STAGES]; 2],
    last: [f32; 2],
    lfo: [Lfo; 2],
    /// Rate libero (Hz).
    rate: f32,
    /// Divisione di sync (indice in [`SYNC_CHOICES`], 0 = Off).
    sync: f32,
    /// Tempo del progetto (BPM).
    bpm: f32,
    stereo: f32,
    depth: f32,
    center: f32,
    feedback: f32,
    mix: Smoother,
}

impl Default for Phaser {
    fn default() -> Self {
        Self::new()
    }
}

impl Phaser {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            stages: 4,
            state: [[0.0; MAX_STAGES]; 2],
            last: [0.0; 2],
            lfo: [Lfo::new(DEFAULT_SR); 2],
            rate: 0.5,
            sync: 0.0,
            bpm: DEFAULT_BPM,
            stereo: 0.25,
            depth: 0.7,
            center: 800.0,
            feedback: 0.4,
            mix: Smoother::new(0.02, DEFAULT_SR, 0.5),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    /// Rate libero o derivato dal tempo (sync attivo).
    fn apply_rate(&mut self) {
        let hz = lfo_rate(self.rate, self.sync, self.bpm);
        for l in &mut self.lfo {
            l.set_rate(hz);
        }
    }
}

impl Effect for Phaser {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        for l in &mut self.lfo {
            l.set_sample_rate(sample_rate);
        }
        self.apply_rate();
        self.mix.set_sample_rate(sample_rate);
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "stages" => self.stages = ((v as usize) + 1) * 2,
            "rate" => {
                self.rate = v;
                self.apply_rate();
            }
            "sync" => {
                self.sync = v;
                self.apply_rate();
            }
            "depth" => self.depth = v,
            "center" => self.center = v,
            "feedback" => self.feedback = v,
            "mix" => self.mix.set_target(v),
            "stereo" => self.stereo = v,
            _ => {}
        }
    }

    fn set_tempo(&mut self, bpm: f32) {
        self.bpm = sanitize_bpm(bpm);
        self.apply_rate();
    }

    fn reset(&mut self) {
        self.state = [[0.0; MAX_STAGES]; 2];
        self.last = [0.0; 2];
        self.lfo[0].reset(0.0);
        self.lfo[1].reset(self.stereo);
        self.mix.set_immediate(self.mix.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        let stages = self.stages.min(MAX_STAGES);
        let max_hz = 0.45 * self.sr;
        let k = std::f32::consts::PI / self.sr;
        for i in 0..n {
            let mix = self.mix.next_sample();
            for (ch, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                let hz =
                    (self.center * 2f32.powf(self.depth * SWEEP_OCT * self.lfo[ch].next_sample())).clamp(20.0, max_hz);
                let t = (k * hz).tan();
                let a = (t - 1.0) / (t + 1.0);
                let x = buf[i];
                let mut y = x + self.feedback * self.last[ch];
                for z in &mut self.state[ch][..stages] {
                    // All-pass del primo ordine: H(z) = (a + z^-1) / (1 + a z^-1).
                    let out = a * y + *z;
                    *z = flush_denormal(y - a * out);
                    y = out;
                }
                self.last[ch] = flush_denormal(y);
                buf[i] = x * (1.0 - mix) + y * mix;
            }
        }
    }
}
