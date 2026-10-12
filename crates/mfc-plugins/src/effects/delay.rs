//! Delay stereo con feedback filtrato, ping-pong e offset stereo (stile "Fruity Delay 3").

use super::{apply_defaults, ensure_delay, frames, param_value, sr_ok, DEFAULT_SR, MAX_SR, OFF_ON};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{flush_denormal, DelayLine, OnePole, Smoother};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::DELAY,
    name: "Delay",
    role: PluginRole::Effect,
    description: "Delay con feedback filtrato passa-basso, ping-pong e sfasamento tra i canali.",
    params: &[
        ParamDesc::new("time", "Time", 1.0, 2000.0, 375.0, "ms"),
        ParamDesc::new("feedback", "Feedback", 0.0, 0.95, 0.4, ""),
        ParamDesc::choice("ping_pong", "Ping-pong", OFF_ON, 0),
        ParamDesc::new("cutoff", "Feedback cutoff", 200.0, 20_000.0, 8_000.0, "Hz"),
        ParamDesc::new("wet", "Wet", 0.0, 1.0, 0.35, ""),
        ParamDesc::new("dry", "Dry", 0.0, 1.0, 1.0, ""),
        ParamDesc::new("stereo_offset", "Stereo offset", -50.0, 50.0, 0.0, "ms"),
    ],
};

/// Ritardo massimo (s): tempo + offset.
const MAX_DELAY_S: f32 = 2.06;

/// Delay stereo.
pub struct Delay {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    lines: [DelayLine; 2],
    lp: [OnePole; 2],
    time_ms: f32,
    offset_ms: f32,
    cutoff: f32,
    ping_pong: bool,
    /// Ritardo in campioni per canale (smussato: effetto "nastro" sulle variazioni).
    delay: [Smoother; 2],
    feedback: Smoother,
    wet: Smoother,
    dry: Smoother,
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl Delay {
    pub fn new() -> Self {
        let cap = super::capacity(MAX_DELAY_S, MAX_SR);
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            lines: [DelayLine::new(cap), DelayLine::new(cap)],
            lp: [OnePole::default(); 2],
            time_ms: 375.0,
            offset_ms: 0.0,
            cutoff: 8_000.0,
            ping_pong: false,
            delay: [Smoother::new(0.05, DEFAULT_SR, 1.0); 2],
            feedback: Smoother::new(0.02, DEFAULT_SR, 0.4),
            wet: Smoother::new(0.02, DEFAULT_SR, 0.35),
            dry: Smoother::new(0.02, DEFAULT_SR, 1.0),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.set_sample_rate(DEFAULT_SR);
        s
    }

    fn update(&mut self) {
        let base = self.time_ms;
        let l = base + (-self.offset_ms).max(0.0);
        let r = base + self.offset_ms.max(0.0);
        self.delay[0].set_target((l * 0.001 * self.sr).max(1.0));
        self.delay[1].set_target((r * 0.001 * self.sr).max(1.0));
        for f in &mut self.lp {
            f.set_cutoff(self.cutoff, self.sr);
        }
    }
}

impl Effect for Delay {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        for d in &mut self.lines {
            ensure_delay(d, MAX_DELAY_S, sample_rate);
        }
        let [d0, d1] = &mut self.delay;
        for s in [d0, d1, &mut self.feedback, &mut self.wet, &mut self.dry] {
            s.set_sample_rate(sample_rate);
        }
        self.update();
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "time" => self.time_ms = v,
            "stereo_offset" => self.offset_ms = v,
            "cutoff" => self.cutoff = v,
            "ping_pong" => self.ping_pong = v >= 0.5,
            "feedback" => self.feedback.set_target(v),
            "wet" => self.wet.set_target(v),
            "dry" => self.dry.set_target(v),
            _ => return,
        }
        self.update();
    }

    fn reset(&mut self) {
        for d in &mut self.lines {
            d.reset();
        }
        for f in &mut self.lp {
            f.reset();
        }
        let [d0, d1] = &mut self.delay;
        for s in [d0, d1, &mut self.feedback, &mut self.wet, &mut self.dry] {
            s.set_immediate(s.target());
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        for i in 0..n {
            let (l, r) = (left[i], right[i]);
            // Lettura prima della scrittura: ritardo d → read(d - 1).
            let yl = self.lines[0].read_cubic(self.delay[0].next_sample() - 1.0);
            let yr = self.lines[1].read_cubic(self.delay[1].next_sample() - 1.0);
            let fb = self.feedback.next_sample();
            let fl = self.lp[0].lowpass(yl) * fb;
            let fr = self.lp[1].lowpass(yr) * fb;
            let (in_l, in_r) = if self.ping_pong {
                (0.5 * (l + r) + fr, fl)
            } else {
                (l + fl, r + fr)
            };
            self.lines[0].write(flush_denormal(in_l));
            self.lines[1].write(flush_denormal(in_r));
            let wet = self.wet.next_sample();
            let dry = self.dry.next_sample();
            left[i] = l * dry + yl * wet;
            right[i] = r * dry + yr * wet;
        }
    }
}
