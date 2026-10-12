//! BooBass: basso monofonico con controlli di tono bass/mid/treble (stile FL "BooBass").

use super::{apply_defaults, frames, param_value, DEFAULT_SR};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, midi_to_hz, Adsr, Biquad, BiquadKind, ExpDecay, Oscillator, Svf, Waveform};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::BOOBASS,
    name: "BooBass",
    role: PluginRole::Generator,
    description: "Basso monofonico semplice con equalizzatore a tre bande (bass/mid/treble).",
    params: &[
        ParamDesc::new("bass", "Bass", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("mid", "Mid", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("treble", "Treble", 0.0, 1.0, 0.5, ""),
        ParamDesc::new("release", "Release", 0.005, 2.0, 0.08, "s"),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.8, ""),
    ],
};

/// Escursione delle manopole di tono (±dB).
const TONE_DB: f32 = 12.0;

/// Basso monofonico: una nuova nota riprende la voce dal livello corrente (legato senza click).
pub struct BooBass {
    sr: f32,
    active: bool,
    voice_id: u64,
    key: u8,
    fine: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    pitch_offset: f32,
    saw: Oscillator,
    sub: Oscillator,
    env: Adsr,
    filter_env: ExpDecay,
    svf: Svf,
    tone: [Biquad; 3],
    bass: f32,
    mid: f32,
    treble: f32,
    release: f32,
    amp: f32,
}

impl Default for BooBass {
    fn default() -> Self {
        Self::new()
    }
}

impl BooBass {
    pub fn new() -> Self {
        let mut saw = Oscillator::new(DEFAULT_SR);
        saw.set_waveform(Waveform::Saw);
        let mut s = Self {
            sr: DEFAULT_SR,
            active: false,
            voice_id: 0,
            key: 36,
            fine: 0.0,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            pitch_offset: 0.0,
            saw,
            sub: Oscillator::new(DEFAULT_SR),
            env: Adsr::new(DEFAULT_SR),
            filter_env: ExpDecay::new(0.35, DEFAULT_SR),
            svf: Svf::new(DEFAULT_SR),
            tone: [Biquad::new(); 3],
            bass: 0.5,
            mid: 0.5,
            treble: 0.5,
            release: 0.08,
            amp: 0.8,
        };
        s.svf.set_q(0.9);
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.update_tone();
        s
    }

    fn update_tone(&mut self) {
        let db = |v: f32| (v - 0.5) * 2.0 * TONE_DB;
        self.tone[0].set(BiquadKind::LowShelf, 120.0, 0.707, db(self.bass), self.sr);
        self.tone[1].set(BiquadKind::Peaking, 700.0, 0.8, db(self.mid), self.sr);
        self.tone[2].set(BiquadKind::HighShelf, 2500.0, 0.707, db(self.treble), self.sr);
        self.env.set_adsr(0.003, 0.0, 1.0, self.release);
    }
}

impl Generator for BooBass {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        self.saw.set_sample_rate(sample_rate);
        self.sub.set_sample_rate(sample_rate);
        self.env.set_sample_rate(sample_rate);
        self.filter_env.set_sample_rate(sample_rate);
        self.svf.set_sample_rate(sample_rate);
        self.update_tone();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else { return };
        match name {
            "bass" => self.bass = v,
            "mid" => self.mid = v,
            "treble" => self.treble = v,
            "release" => self.release = v,
            "amp" => {
                self.amp = v;
                return;
            }
            _ => return,
        }
        self.update_tone();
    }

    fn note_on(&mut self, note: NoteOn) {
        if !self.active {
            self.saw.reset(0.0);
            self.sub.reset(0.0);
            self.svf.reset();
            for b in &mut self.tone {
                b.reset();
            }
        }
        self.active = true;
        self.voice_id = note.voice_id;
        self.key = note.key.min(127);
        self.fine = if note.fine_pitch_cents.is_finite() { note.fine_pitch_cents } else { 0.0 };
        self.vel = note.velocity.clamp(0.0, 1.0);
        (self.gl, self.gr) = balance_gains(note.pan);
        self.env.note_on();
        self.filter_env.trigger(1.0);
    }

    fn note_off(&mut self, voice_id: u64) {
        if self.active && voice_id == self.voice_id {
            self.env.note_off();
        }
    }

    fn all_notes_off(&mut self) {
        self.env.note_off();
    }

    fn set_pitch_offset(&mut self, cents: f32) {
        if cents.is_finite() {
            self.pitch_offset = cents.clamp(-9600.0, 9600.0);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.active {
            return;
        }
        let n = frames(left, right);
        let hz = midi_to_hz(self.key as f32 + (self.fine + self.pitch_offset) / 100.0);
        self.saw.set_frequency(hz);
        self.sub.set_frequency(hz);
        let gain = self.amp * (0.3 + 0.7 * self.vel);
        let brightness = 0.4 + 0.6 * self.vel;
        for i in 0..n {
            let raw = 0.55 * self.saw.next() + 0.6 * self.sub.next();
            let fe = self.filter_env.next();
            self.svf.set_cutoff(hz * 1.5 + 3500.0 * fe * brightness + 150.0);
            let mut x = self.svf.process(raw);
            for b in &mut self.tone {
                x = b.process(x);
            }
            let y = x * self.env.next() * gain;
            left[i] += y * self.gl;
            right[i] += y * self.gr;
            if self.env.is_idle() {
                self.active = false;
                break;
            }
        }
    }

    fn active_voices(&self) -> usize {
        usize::from(self.active)
    }
}
