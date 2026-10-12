//! 3x Osc: tre oscillatori sommati con inviluppo, filtro e glide (stile FL "3x Osc").

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{balance_gains, midi_to_hz, pan_gains, Adsr, Oscillator, Rng, Svf, Waveform};
use std::f32::consts::TAU;

/// Forme d'onda disponibili per ciascun oscillatore.
pub const SHAPES: &[&str] = &["sine", "triangle", "square", "saw", "rounded_saw", "noise"];

macro_rules! osc3_params {
    ($( ($i:literal, $vol:expr, $coarse:expr) ),*) => {
        &[
            $(
                ParamDesc::choice(concat!("osc", $i, "_shape"), concat!("Osc ", $i, " shape"), SHAPES, 0),
                ParamDesc::new(concat!("osc", $i, "_coarse"), concat!("Osc ", $i, " coarse"), -24.0, 24.0, $coarse, "st"),
                ParamDesc::new(concat!("osc", $i, "_fine"), concat!("Osc ", $i, " fine"), -100.0, 100.0, 0.0, "cents"),
                ParamDesc::new(concat!("osc", $i, "_volume"), concat!("Osc ", $i, " volume"), 0.0, 1.0, $vol, ""),
                ParamDesc::new(concat!("osc", $i, "_pan"), concat!("Osc ", $i, " pan"), -1.0, 1.0, 0.0, ""),
                ParamDesc::new(concat!("osc", $i, "_phase"), concat!("Osc ", $i, " phase"), 0.0, 1.0, 0.0, ""),
                ParamDesc::new(concat!("osc", $i, "_detune"), concat!("Osc ", $i, " stereo detune"), 0.0, 50.0, 0.0, "cents"),
            )*
            ParamDesc::new("attack", "Attack", 0.0, 10.0, 0.005, "s"),
            ParamDesc::new("decay", "Decay", 0.0, 10.0, 0.3, "s"),
            ParamDesc::new("sustain", "Sustain", 0.0, 1.0, 0.8, ""),
            ParamDesc::new("release", "Release", 0.0, 10.0, 0.2, "s"),
            ParamDesc::new("cutoff", "Filter cutoff", 20.0, 20_000.0, 20_000.0, "Hz"),
            ParamDesc::new("resonance", "Filter resonance", 0.0, 1.0, 0.0, ""),
            ParamDesc::new("glide", "Glide time", 0.0, 2.0, 0.1, "s"),
            ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.6, ""),
        ]
    };
}

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::OSC3,
    name: "3x Osc",
    role: PluginRole::Generator,
    description: "Tre oscillatori (sine/triangle/square/saw/rounded saw/noise) con ADSR, filtro e glide.",
    params: osc3_params!((1, 1.0, 0.0), (2, 0.5, -12.0), (3, 0.25, -24.0)),
};

const VOICES: usize = 16;
/// Campioni tra un aggiornamento di frequenza e il successivo.
const CHUNK: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Basic(Waveform),
    RoundedSaw,
    Noise,
}

fn shape_from(v: f32) -> Shape {
    match v as i32 {
        1 => Shape::Basic(Waveform::Triangle),
        2 => Shape::Basic(Waveform::Square),
        3 => Shape::Basic(Waveform::Saw),
        4 => Shape::RoundedSaw,
        5 => Shape::Noise,
        _ => Shape::Basic(Waveform::Sine),
    }
}

/// Dente di sega "arrotondato": prime 6 armoniche (ricorrenza seno/coseno, un solo sin_cos).
#[inline]
fn rounded_saw(phase: f32) -> f32 {
    let (s1, c1) = (TAU * phase).sin_cos();
    let (mut s, mut c) = (s1, c1);
    let mut acc = 0.0;
    for k in 1..=6 {
        acc += s / k as f32;
        let ns = s * c1 + c * s1;
        c = c * c1 - s * s1;
        s = ns;
    }
    -0.55 * acc
}

#[derive(Debug, Clone, Copy)]
struct OscParams {
    shape: Shape,
    coarse: f32,
    fine: f32,
    volume: f32,
    pan: f32,
    phase: f32,
    detune: f32,
    gl: f32,
    gr: f32,
}

impl Default for OscParams {
    fn default() -> Self {
        Self {
            shape: Shape::Basic(Waveform::Sine),
            coarse: 0.0,
            fine: 0.0,
            volume: 0.0,
            pan: 0.0,
            phase: 0.0,
            detune: 0.0,
            gl: 0.0,
            gr: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    key: u8,
    fine: f32,
    vel: f32,
    gl: f32,
    gr: f32,
    /// Altezza corrente e obiettivo (semitoni MIDI) per il glide.
    pitch: f32,
    target: f32,
    glide_rate: f32,
    /// [oscillatore][canale]
    osc: [[Oscillator; 2]; 3],
    rng: Rng,
    env: Adsr,
    svf: [Svf; 2],
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

/// Synth 3x Osc polifonico.
pub struct Osc3 {
    sr: f32,
    voices: Vec<Voice>,
    oscs: [OscParams; 3],
    order: u64,
    pitch_offset: f32,
    last_pitch: Option<f32>,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
    resonance: f32,
    glide: f32,
    amp: f32,
}

impl Default for Osc3 {
    fn default() -> Self {
        Self::new()
    }
}

impl Osc3 {
    pub fn new() -> Self {
        let voice = Voice {
            tag: VoiceTag::default(),
            key: 60,
            fine: 0.0,
            vel: 1.0,
            gl: 1.0,
            gr: 1.0,
            pitch: 60.0,
            target: 60.0,
            glide_rate: 0.0,
            osc: [[Oscillator::new(DEFAULT_SR); 2]; 3],
            rng: Rng::new(1),
            env: Adsr::new(DEFAULT_SR),
            svf: [Svf::new(DEFAULT_SR); 2],
        };
        let mut voices = vec![voice; VOICES];
        for (i, v) in voices.iter_mut().enumerate() {
            v.rng = Rng::new(0x3_0000 + i as u32);
        }
        let mut s = Self {
            sr: DEFAULT_SR,
            voices,
            oscs: [OscParams::default(); 3],
            order: 0,
            pitch_offset: 0.0,
            last_pitch: None,
            attack: 0.005,
            decay: 0.3,
            sustain: 0.8,
            release: 0.2,
            cutoff: 20_000.0,
            resonance: 0.0,
            glide: 0.1,
            amp: 0.6,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s
    }

    fn configure_voices(&mut self) {
        for v in &mut self.voices {
            v.env.set_adsr(self.attack, self.decay, self.sustain, self.release);
            for f in &mut v.svf {
                f.set_cutoff(self.cutoff);
                f.set_resonance(self.resonance);
            }
            for (o, p) in v.osc.iter_mut().zip(&self.oscs) {
                if let Shape::Basic(w) = p.shape {
                    o[0].set_waveform(w);
                    o[1].set_waveform(w);
                }
            }
        }
    }

    fn update_freqs(v: &mut Voice, oscs: &[OscParams; 3], offset: f32) {
        for (o, p) in v.osc.iter_mut().zip(oscs) {
            let base = v.pitch + p.coarse + (p.fine + v.fine + offset) / 100.0;
            let half = p.detune / 200.0;
            o[0].set_frequency(midi_to_hz(base - half));
            o[1].set_frequency(midi_to_hz(base + half));
        }
    }

    fn set_osc_param(&mut self, idx: usize, field: &str, v: f32) {
        let p = &mut self.oscs[idx];
        match field {
            "shape" => p.shape = shape_from(v),
            "coarse" => p.coarse = v,
            "fine" => p.fine = v,
            "volume" => p.volume = v,
            "pan" => p.pan = v,
            "phase" => p.phase = v,
            "detune" => p.detune = v,
            _ => return,
        }
        let (l, r) = pan_gains(p.pan);
        p.gl = l * p.volume;
        p.gr = r * p.volume;
    }
}

impl Generator for Osc3 {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        for v in &mut self.voices {
            v.env.set_sample_rate(sample_rate);
            for f in &mut v.svf {
                f.set_sample_rate(sample_rate);
            }
            for o in v.osc.iter_mut().flatten() {
                o.set_sample_rate(sample_rate);
            }
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        if let Some(rest) = name.strip_prefix("osc") {
            if let Some((i, field)) = rest.split_once('_') {
                if let Ok(i @ 1..=3) = i.parse::<usize>() {
                    self.set_osc_param(i - 1, field, v);
                    if field == "shape" {
                        self.configure_voices();
                    }
                }
            }
            return;
        }
        match name {
            "attack" => self.attack = v,
            "decay" => self.decay = v,
            "sustain" => self.sustain = v,
            "release" => self.release = v,
            "cutoff" => self.cutoff = v,
            "resonance" => self.resonance = v,
            "glide" => self.glide = v,
            "amp" => self.amp = v,
            _ => return,
        }
        if !matches!(name, "glide" | "amp") {
            self.configure_voices();
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = note.key.min(127);
        let target = key as f32;
        let fine = if note.fine_pitch_cents.is_finite() {
            note.fine_pitch_cents
        } else {
            0.0
        };
        let glide_samples = (self.glide * self.sr).max(1.0);
        // Nota slide: fa scivolare la voce tenuta più recente senza crearne una nuova.
        if note.slide {
            let newest = self
                .voices
                .iter_mut()
                .filter(|v| v.tag.active && !v.tag.released)
                .max_by_key(|v| v.tag.order);
            if let Some(v) = newest {
                v.target = target;
                v.key = key;
                v.fine = fine;
                v.glide_rate = (target - v.pitch).abs() / glide_samples;
                self.last_pitch = Some(target);
                return;
            }
        }
        let start_pitch = match (note.slide, self.last_pitch) {
            (true, Some(p)) => p,
            _ => target,
        };
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let (gl, gr) = balance_gains(note.pan);
        let v = &mut self.voices[idx];
        v.tag = VoiceTag {
            active: true,
            released: false,
            id: note.voice_id,
            order: self.order,
        };
        v.key = key;
        v.fine = fine;
        v.vel = note.velocity.clamp(0.0, 1.0);
        v.gl = gl;
        v.gr = gr;
        v.pitch = start_pitch;
        v.target = target;
        v.glide_rate = (target - start_pitch).abs() / glide_samples;
        for (o, p) in v.osc.iter_mut().zip(&self.oscs) {
            o[0].reset(p.phase);
            o[1].reset(p.phase);
        }
        v.env.note_on();
        for f in &mut v.svf {
            f.reset();
        }
        self.last_pitch = Some(target);
    }

    fn note_off(&mut self, voice_id: u64) {
        for v in &mut self.voices {
            if v.tag.active && !v.tag.released && v.tag.id == voice_id {
                v.tag.released = true;
                v.env.note_off();
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
                v.env.note_off();
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
        let filter_on = self.cutoff < 19_999.0;
        let oscs = self.oscs;
        let offset = self.pitch_offset;
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let gain = v.vel * self.amp;
            let mut start = 0;
            while start < n && v.tag.active {
                let end = (start + CHUNK).min(n);
                // Glide a passi di CHUNK campioni.
                if v.pitch != v.target {
                    let step = v.glide_rate * (end - start) as f32;
                    let diff = v.target - v.pitch;
                    v.pitch = if diff.abs() <= step {
                        v.target
                    } else {
                        v.pitch + step.copysign(diff)
                    };
                }
                Self::update_freqs(v, &oscs, offset);
                for i in start..end {
                    let (mut l, mut r) = (0.0, 0.0);
                    for (o, p) in v.osc.iter_mut().zip(&oscs) {
                        let (a, b) = match p.shape {
                            Shape::Basic(_) => (o[0].next_sample(), o[1].next_sample()),
                            Shape::RoundedSaw => (rounded_saw(o[0].advance()), rounded_saw(o[1].advance())),
                            Shape::Noise => {
                                let x = v.rng.next_bipolar() * 0.7;
                                (x, x)
                            }
                        };
                        l += a * p.gl;
                        r += b * p.gr;
                    }
                    if filter_on {
                        l = v.svf[0].process(l);
                        r = v.svf[1].process(r);
                    }
                    let e = v.env.next_sample() * gain;
                    left[i] += l * e * v.gl;
                    right[i] += r * e * v.gr;
                    if v.env.is_idle() {
                        v.tag.active = false;
                        break;
                    }
                }
                start = end;
            }
        }
    }

    fn active_voices(&self) -> usize {
        count_active(&self.voices)
    }
}
