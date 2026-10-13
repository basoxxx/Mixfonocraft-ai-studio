//! Transient shaper: esalta o attenua attacco e coda senza soglia (indipendente dal livello).
//!
//! Due coppie di inviluppi: per l'attacco, attacco veloce contro lento con la stessa release
//! (il rapporto supera 1 solo sui transienti); per la coda, release lunga contro breve con lo
//! stesso attacco (il rapporto supera 1 solo mentre il suono decade).

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{db_to_gain, time_coef, EnvelopeFollower, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::TRANSIENT,
    name: "Transient Shaper",
    role: PluginRole::Effect,
    description: "Attacco e sustain da -1 (attenua) a +1 (esalta), indipendenti dal livello: \
                  cassa più secca o più corposa, rullante più incisivo.",
    params: &[
        ParamDesc::new("attack", "Attack", -1.0, 1.0, 0.0, ""),
        ParamDesc::new("sustain", "Sustain", -1.0, 1.0, 0.0, ""),
        ParamDesc::new("output", "Output", -12.0, 12.0, 0.0, "dB"),
    ],
};

/// Guadagno massimo applicabile dallo shaping (±12 dB).
const MAX_GAIN: f32 = 4.0;

/// Transient shaper stereo-linked.
pub struct TransientShaper {
    primed: bool,
    sr: f32,
    att_fast: EnvelopeFollower,
    att_slow: EnvelopeFollower,
    sus_short: EnvelopeFollower,
    sus_long: EnvelopeFollower,
    attack: f32,
    sustain: f32,
    output: Smoother,
    g: f32,
    g_coef: f32,
}

impl Default for TransientShaper {
    fn default() -> Self {
        Self::new()
    }
}

impl TransientShaper {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            att_fast: EnvelopeFollower::new(0.0005, 0.1, DEFAULT_SR),
            att_slow: EnvelopeFollower::new(0.015, 0.1, DEFAULT_SR),
            sus_short: EnvelopeFollower::new(0.0005, 0.03, DEFAULT_SR),
            sus_long: EnvelopeFollower::new(0.0005, 0.3, DEFAULT_SR),
            attack: 0.0,
            sustain: 0.0,
            output: Smoother::new(0.01, DEFAULT_SR, 1.0),
            g: 1.0,
            g_coef: time_coef(0.001, DEFAULT_SR),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s
    }
}

impl TransientShaper {
    fn envelopes(&mut self) -> [&mut EnvelopeFollower; 4] {
        [
            &mut self.att_fast,
            &mut self.att_slow,
            &mut self.sus_short,
            &mut self.sus_long,
        ]
    }
}

impl Effect for TransientShaper {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if sr_ok(sample_rate) {
            self.sr = sample_rate;
            for e in self.envelopes() {
                e.set_sample_rate(sample_rate);
            }
            self.output.set_sample_rate(sample_rate);
            self.g_coef = time_coef(0.001, sample_rate);
            self.reset();
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "attack" => self.attack = v,
            "sustain" => self.sustain = v,
            "output" => self.output.set_target(db_to_gain(v)),
            _ => {}
        }
    }

    fn reset(&mut self) {
        for e in self.envelopes() {
            e.reset();
        }
        self.g = 1.0;
        self.output.set_immediate(self.output.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        let (ka, ks) = (2.0 * self.attack, 2.0 * self.sustain);
        for i in 0..n {
            let x = left[i].abs().max(right[i].abs());
            let af = self.att_fast.process(x).max(1e-6);
            let as_ = self.att_slow.process(x).max(1e-6);
            let ss = self.sus_short.process(x).max(1e-6);
            let sl = self.sus_long.process(x).max(1e-6);
            let target = if sl > 1e-5 {
                ((af / as_).powf(ka) * (sl / ss).powf(ks)).clamp(1.0 / MAX_GAIN, MAX_GAIN)
            } else {
                1.0
            };
            self.g = target + self.g_coef * (self.g - target);
            let g = self.g * self.output.next_sample();
            left[i] *= g;
            right[i] *= g;
        }
    }
}
