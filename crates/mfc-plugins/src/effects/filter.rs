//! Filtro multimodo (SVF) con modulazione LFO del cutoff.

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use crate::sync::{lfo_rate, sanitize_bpm, DEFAULT_BPM, SYNC_CHOICES};
use mfc_dsp::{Lfo, Smoother, Svf, SvfMode};

/// Modi del filtro (valore del parametro `type`).
pub const TYPES: &[&str] = &["lp", "hp", "bp", "notch"];

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::FILTER,
    name: "Filter",
    role: PluginRole::Effect,
    description: "Filtro state-variable 12 dB/ott (LP/HP/BP/notch) con LFO sul cutoff.",
    params: &[
        ParamDesc::choice("type", "Type", TYPES, 0),
        ParamDesc::new("cutoff", "Cutoff", 20.0, 20_000.0, 1_000.0, "Hz"),
        ParamDesc::new("resonance", "Resonance", 0.0, 1.0, 0.2, ""),
        ParamDesc::new("lfo_amount", "LFO amount", 0.0, 4.0, 0.0, "oct"),
        ParamDesc::new("lfo_rate", "LFO rate", 0.01, 20.0, 1.0, "Hz"),
        ParamDesc::choice("lfo_sync", "LFO sync", SYNC_CHOICES, 0),
    ],
};

/// Filtro stereo modulabile.
pub struct Filter {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    svf: [Svf; 2],
    /// Cutoff in ottave (log2 Hz), smussato.
    log_cutoff: Smoother,
    lfo: Lfo,
    lfo_amount: f32,
    /// Rate libero dell'LFO (Hz).
    lfo_rate: f32,
    /// Divisione di sync (indice in [`SYNC_CHOICES`], 0 = Off).
    lfo_sync: f32,
    /// Tempo del progetto (BPM).
    bpm: f32,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            svf: [Svf::new(DEFAULT_SR); 2],
            log_cutoff: Smoother::new(0.02, DEFAULT_SR, 1000f32.log2()),
            lfo: Lfo::new(DEFAULT_SR),
            lfo_amount: 0.0,
            lfo_rate: 1.0,
            lfo_sync: 0.0,
            bpm: DEFAULT_BPM,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    /// Rate dell'LFO libero o derivato dal tempo (sync attivo).
    fn apply_rate(&mut self) {
        self.lfo.set_rate(lfo_rate(self.lfo_rate, self.lfo_sync, self.bpm));
    }
}

impl Effect for Filter {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        for f in &mut self.svf {
            f.set_sample_rate(sample_rate);
        }
        self.log_cutoff.set_sample_rate(sample_rate);
        self.lfo.set_sample_rate(sample_rate);
        self.apply_rate();
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "type" => {
                let mode = match v as i32 {
                    1 => SvfMode::Highpass,
                    2 => SvfMode::Bandpass,
                    3 => SvfMode::Notch,
                    _ => SvfMode::Lowpass,
                };
                for f in &mut self.svf {
                    f.set_mode(mode);
                }
            }
            "cutoff" => self.log_cutoff.set_target(v.log2()),
            "resonance" => {
                for f in &mut self.svf {
                    f.set_resonance(v);
                }
            }
            "lfo_amount" => self.lfo_amount = v,
            "lfo_rate" => {
                self.lfo_rate = v;
                self.apply_rate();
            }
            "lfo_sync" => {
                self.lfo_sync = v;
                self.apply_rate();
            }
            _ => {}
        }
    }

    fn set_tempo(&mut self, bpm: f32) {
        self.bpm = sanitize_bpm(bpm);
        self.apply_rate();
    }

    fn reset(&mut self) {
        for f in &mut self.svf {
            f.reset();
        }
        self.log_cutoff.set_immediate(self.log_cutoff.target());
        self.lfo.reset(0.0);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        let max_hz = 0.45 * self.sr;
        for i in 0..n {
            let oct = self.log_cutoff.next_sample() + self.lfo.next_sample() * self.lfo_amount;
            let hz = 2f32.powf(oct).clamp(20.0, max_hz);
            self.svf[0].set_cutoff(hz);
            self.svf[1].set_cutoff(hz);
            left[i] = self.svf[0].process(left[i]);
            right[i] = self.svf[1].process(right[i]);
        }
    }
}
