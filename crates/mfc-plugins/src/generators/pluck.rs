//! Pluck: corda pizzicata con modello Karplus-Strong (rumore filtrato che ricircola in una
//! linea di ritardo lunga un periodo, smorzata da un passa-basso nel loop).

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, midi_to_hz, Rng};

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::PLUCK,
    name: "Pluck",
    role: PluginRole::Generator,
    description: "Corda pizzicata Karplus-Strong: decadimento, smorzamento delle alte, brillantezza \
                  dell'attacco e release al rilascio della nota.",
    params: &[
        ParamDesc::new("decay", "Decay", 0.1, 20.0, 3.0, "s"),
        ParamDesc::new("damping", "Damping", 0.0, 1.0, 0.4, ""),
        ParamDesc::new("brightness", "Brightness", 0.0, 1.0, 0.7, ""),
        ParamDesc::new("release", "Release", 0.01, 5.0, 0.15, "s"),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.6, ""),
    ],
};

const VOICES: usize = 12;
/// Frequenza minima riproducibile (dimensiona le linee di ritardo).
const MIN_HZ: f32 = 16.0;

#[derive(Debug, Clone)]
struct Voice {
    tag: VoiceTag,
    buf: Vec<f32>,
    write: usize,
    /// Ritardo in campioni (frazionario) per l'intonazione.
    delay: f32,
    hz: f32,
    last: f32,
    gl: f32,
    gr: f32,
    /// Inseguitore di picco per spegnere la voce quando è silenziosa.
    level: f32,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth a corde pizzicate polifonico.
pub struct Pluck {
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    rng: Rng,
    pitch_offset: f32,
    decay: f32,
    damping: f32,
    brightness: f32,
    release: f32,
    amp: f32,
}

impl Default for Pluck {
    fn default() -> Self {
        Self::new()
    }
}

impl Pluck {
    pub fn new() -> Self {
        let mut s = Self {
            sr: DEFAULT_SR,
            voices: Vec::new(),
            order: 0,
            rng: Rng::new(0x9_1C4),
            pitch_offset: 0.0,
            decay: 3.0,
            damping: 0.4,
            brightness: 0.7,
            release: 0.15,
            amp: 0.6,
        };
        s.allocate();
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s
    }

    /// Alloca le linee di ritardo per il sample rate corrente (fuori dal thread audio).
    fn allocate(&mut self) {
        let cap = (self.sr / MIN_HZ).ceil() as usize + 4;
        let voice = Voice {
            tag: VoiceTag::default(),
            buf: vec![0.0; cap],
            write: 0,
            delay: 100.0,
            hz: 440.0,
            last: 0.0,
            gl: 1.0,
            gr: 1.0,
            level: 0.0,
        };
        self.voices = vec![voice; VOICES];
    }

    /// Guadagno per giro del loop che porta a -60 dB in `seconds` alla frequenza `hz`.
    fn loop_gain(seconds: f32, hz: f32) -> f32 {
        10f32.powf(-3.0 / (seconds.max(1e-3) * hz.max(1.0)))
    }
}

impl Generator for Pluck {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        self.allocate();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        match name {
            "decay" => self.decay = v,
            "damping" => self.damping = v,
            "brightness" => self.brightness = v,
            "release" => self.release = v,
            "amp" => self.amp = v,
            _ => {}
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let fine = if note.fine_pitch_cents.is_finite() {
            note.fine_pitch_cents
        } else {
            0.0
        };
        let hz =
            midi_to_hz(note.key.min(127) as f32 + (fine + self.pitch_offset) / 100.0).clamp(MIN_HZ, 0.45 * self.sr);
        let (gl, gr) = balance_gains(note.pan);
        let vel = note.velocity.clamp(0.0, 1.0);
        // Eccitazione: rumore passa-basso (più scuro con brightness bassa e velocity bassa).
        let coef = (1.0 - self.brightness * (0.5 + 0.5 * vel)).clamp(0.0, 0.98);
        let v = &mut self.voices[idx];
        v.tag = VoiceTag {
            active: true,
            released: false,
            id: note.voice_id,
            order: self.order,
        };
        v.hz = hz;
        // Il passa-basso a due punti nel loop aggiunge mezzo campione di ritardo.
        v.delay = (self.sr / hz - 0.5).clamp(2.0, (v.buf.len() - 3) as f32);
        v.gl = gl;
        v.gr = gr;
        v.last = 0.0;
        v.level = 1.0;
        v.buf.fill(0.0);
        let len = v.delay.ceil() as usize + 1;
        let mut lp = 0.0;
        let mut mean = 0.0;
        for k in 0..len {
            let x = self.rng.next_bipolar();
            lp = x + coef * (lp - x);
            v.buf[k] = lp;
            mean += lp;
        }
        // Niente componente continua: eviterebbe un offset che decade lentamente.
        mean /= len as f32;
        let mut peak = 1e-6f32;
        for x in &mut v.buf[..len] {
            *x -= mean;
            peak = peak.max(x.abs());
        }
        let scale = vel / peak;
        for x in &mut v.buf[..len] {
            *x *= scale;
        }
        v.write = len % v.buf.len();
    }

    fn note_off(&mut self, voice_id: u64) {
        for v in &mut self.voices {
            if v.tag.active && !v.tag.released && v.tag.id == voice_id {
                v.tag.released = true;
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
            }
        }
    }

    fn set_pitch_offset(&mut self, cents: f32) {
        if cents.is_finite() {
            self.pitch_offset = cents.clamp(-9600.0, 9600.0);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = frames(left, right);
        // Smorzamento: media pesata tra campione corrente e precedente (0 = nessun filtro).
        let d = 0.5 * self.damping;
        let level_coef = (-1.0 / (0.01 * self.sr)).exp();
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let time = if v.tag.released {
                self.release.min(self.decay)
            } else {
                self.decay
            };
            let g = Self::loop_gain(time, v.hz);
            let cap = v.buf.len();
            let gain = self.amp;
            for i in 0..n {
                // Lettura frazionaria (interpolazione lineare) a `delay` campioni indietro.
                let pos = v.write as f32 + cap as f32 - v.delay;
                let i0 = pos.floor();
                let frac = pos - i0;
                let a = v.buf[i0 as usize % cap];
                let b = v.buf[(i0 as usize + 1) % cap];
                let y = a + (b - a) * frac;
                let s = (1.0 - d) * y + d * v.last;
                v.last = s;
                let out = s * g;
                v.buf[v.write] = out;
                v.write = (v.write + 1) % cap;
                left[i] += out * gain * v.gl;
                right[i] += out * gain * v.gr;
                v.level = out.abs().max(v.level * level_coef);
                if v.level < 1e-5 {
                    v.tag.active = false;
                    break;
                }
            }
        }
    }

    fn active_voices(&self) -> usize {
        count_active(&self.voices)
    }
}
