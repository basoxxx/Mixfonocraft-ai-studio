//! EQ parametrico a 7 bande (stile "Parametric EQ 2").

use super::{apply_defaults, frames, param_value, sr_ok, DEFAULT_SR};
use crate::api::{Effect, ParamDesc, PluginDescriptor, PluginRole};
use crate::ids;
use mfc_dsp::{db_to_gain, Biquad, BiquadKind, Smoother};

/// Tipi di banda (valore dei parametri `bandN_type`).
pub const BAND_TYPES: &[&str] = &[
    "off",
    "low_cut",
    "low_shelf",
    "peak",
    "band_pass",
    "notch",
    "high_shelf",
    "high_cut",
];
pub const BANDS: usize = 7;

macro_rules! eq_params {
    ($( ($i:literal, $ty:expr, $freq:expr) ),*) => {
        &[
            $(
                ParamDesc::choice(concat!("band", $i, "_type"), concat!("Band ", $i, " type"), BAND_TYPES, $ty),
                ParamDesc::new(concat!("band", $i, "_freq"), concat!("Band ", $i, " frequency"), 20.0, 20_000.0, $freq, "Hz"),
                ParamDesc::new(concat!("band", $i, "_gain"), concat!("Band ", $i, " gain"), -18.0, 18.0, 0.0, "dB"),
                ParamDesc::new(concat!("band", $i, "_q"), concat!("Band ", $i, " Q"), 0.1, 18.0, 0.707, ""),
            )*
            ParamDesc::new("output", "Output gain", -18.0, 18.0, 0.0, "dB"),
        ]
    };
}

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::EQ,
    name: "Parametric EQ",
    role: PluginRole::Effect,
    description: "Equalizzatore parametrico a 7 bande (cut/shelf/peak/band pass/notch) con smoothing.",
    params: eq_params!(
        (1, 2, 60.0),
        (2, 3, 150.0),
        (3, 3, 400.0),
        (4, 3, 1_000.0),
        (5, 3, 2_500.0),
        (6, 3, 6_000.0),
        (7, 6, 12_000.0)
    ),
};

/// Campioni tra due ricalcoli dei coefficienti durante lo smoothing.
const CHUNK: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Band {
    kind: u8,
    /// Frequenza in log2(Hz), guadagno dB e Q: smussati a passi di CHUNK.
    freq: Smoother,
    gain: Smoother,
    q: Smoother,
    filters: [Biquad; 2],
}

impl Band {
    fn new(sr: f32) -> Self {
        let rate = sr / CHUNK as f32;
        Self {
            kind: 0,
            freq: Smoother::new(0.02, rate, 1000f32.log2()),
            gain: Smoother::new(0.02, rate, 0.0),
            q: Smoother::new(0.02, rate, 0.707),
            filters: [Biquad::new(); 2],
        }
    }

    fn update(&mut self, sr: f32) {
        let kind = match self.kind {
            1 => BiquadKind::Highpass,
            2 => BiquadKind::LowShelf,
            3 => BiquadKind::Peaking,
            4 => BiquadKind::Bandpass,
            5 => BiquadKind::Notch,
            6 => BiquadKind::HighShelf,
            7 => BiquadKind::Lowpass,
            _ => {
                for f in &mut self.filters {
                    f.set_identity();
                }
                return;
            }
        };
        let (hz, q, g) = (2f32.powf(self.freq.value()), self.q.value(), self.gain.value());
        for f in &mut self.filters {
            f.set(kind, hz, q, g, sr);
        }
    }

    fn smoothers(&mut self) -> [&mut Smoother; 3] {
        [&mut self.freq, &mut self.gain, &mut self.q]
    }
}

/// EQ stereo a 7 bande.
pub struct Eq {
    /// Falso finché il primo `process` non aggancia gli smoother ai parametri iniziali.
    primed: bool,
    sr: f32,
    bands: [Band; BANDS],
    output: Smoother,
}

impl Default for Eq {
    fn default() -> Self {
        Self::new()
    }
}

impl Eq {
    pub fn new() -> Self {
        let mut s = Self {
            primed: false,
            sr: DEFAULT_SR,
            bands: [Band::new(DEFAULT_SR); BANDS],
            output: Smoother::new(0.02, DEFAULT_SR, 1.0),
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s.reset();
        s
    }

    /// Risposta complessiva (dB) del canale sinistro alla frequenza data (per la GUI).
    pub fn magnitude_db_at(&self, freq: f32) -> f32 {
        let bands: f32 = self
            .bands
            .iter()
            .filter(|b| b.kind != 0)
            .map(|b| b.filters[0].magnitude_db_at(freq))
            .sum();
        bands + mfc_dsp::gain_to_db(self.output.target())
    }
}

impl Effect for Eq {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.primed = false;
        if !sr_ok(sample_rate) {
            return;
        }
        self.sr = sample_rate;
        for b in &mut self.bands {
            for s in b.smoothers() {
                s.set_sample_rate(sample_rate / CHUNK as f32);
            }
        }
        self.output.set_sample_rate(sample_rate);
        self.reset();
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        if name == "output" {
            self.output.set_target(db_to_gain(v));
            return;
        }
        let Some((idx, field)) = name.strip_prefix("band").and_then(|r| r.split_once('_')) else {
            return;
        };
        let Ok(i @ 1..=BANDS) = idx.parse::<usize>() else {
            return;
        };
        let band = &mut self.bands[i - 1];
        match field {
            "type" => {
                let kind = v as u8;
                if kind != band.kind {
                    band.kind = kind;
                    for f in &mut band.filters {
                        f.reset();
                    }
                    band.update(self.sr);
                }
            }
            "freq" => band.freq.set_target(v.log2()),
            "gain" => band.gain.set_target(v),
            "q" => band.q.set_target(v),
            _ => {}
        }
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            for s in b.smoothers() {
                s.set_immediate(s.target());
            }
            for f in &mut b.filters {
                f.reset();
            }
            b.update(self.sr);
        }
        self.output.set_immediate(self.output.target());
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.primed {
            self.reset();
            self.primed = true;
        }
        let n = frames(left, right);
        let mut start = 0;
        while start < n {
            let end = (start + CHUNK).min(n);
            for b in self.bands.iter_mut().filter(|b| b.kind != 0) {
                let mut moving = false;
                for s in b.smoothers() {
                    if !s.is_settled() {
                        s.next_sample();
                        moving = true;
                    }
                }
                if moving {
                    b.update(self.sr);
                }
                let [fl, fr] = &mut b.filters;
                for x in &mut left[start..end] {
                    *x = fl.process(*x);
                }
                for x in &mut right[start..end] {
                    *x = fr.process(*x);
                }
            }
            for i in start..end {
                let g = self.output.next_sample();
                left[i] *= g;
                right[i] *= g;
            }
            start = end;
        }
    }
}
