//! Drumkit: drum machine sintetica con kit selezionabili (808, 909, Acoustic, Lo-Fi, Electro, Trap).
//!
//! I pad seguono la mappa General MIDI: 36 kick, 37 rim, 38 snare, 39 clap, 41/45/48 tom,
//! 42 hi-hat chiuso, 46 hi-hat aperto (soffocato dal chiuso), 49 crash, 51 ride,
//! 54 shaker/tamburello, 56 cowbell. Alcuni alias GM (35, 40, 43, 44, 47, 50...) puntano
//! al pad equivalente; gli altri tasti al pad più vicino.
//!
//! Ogni pad è sintetizzato per kit combinando un corpo tonale con inviluppo di pitch, sei
//! oscillatori quadri "metallici", rumore filtrato (anche a colpi multipli per il clap) e un
//! click d'attacco. Controlli per gruppo (kick, snare, hat, tom, cym): livello, intonazione e
//! decadimento; drive e volume generali. Polifonico (i pad si sovrappongono), nessuna
//! allocazione nel thread audio.

use super::{apply_defaults, count_active, frames, param_value, pick_voice, PolyVoice, VoiceTag, DEFAULT_SR};
use crate::api::{Generator, NoteOn, ParamDesc, PluginDescriptor, PluginRole, Preset};
use crate::ids;
use mfc_dsp::{pan_gains, soft_clip, time_coef, ExpDecay, Oscillator, Rng, Svf, SvfMode, Waveform};

/// Kit disponibili (valore del parametro `kit`).
pub const KIT_NAMES: &[&str] = &["808", "909", "Acoustic", "Lo-Fi", "Electro", "Trap"];

/// Tasti GM dei pad, nell'ordine interno.
pub const PAD_KEYS: [u8; PADS] = [36, 37, 38, 39, 41, 42, 45, 46, 48, 49, 51, 54, 56];
/// Nomi dei pad, nello stesso ordine di [`PAD_KEYS`].
pub const PAD_NAMES: [&str; PADS] = [
    "Kick",
    "Rim",
    "Snare",
    "Clap",
    "Tom basso",
    "Hi-hat chiuso",
    "Tom medio",
    "Hi-hat aperto",
    "Tom alto",
    "Crash",
    "Ride",
    "Shaker",
    "Cowbell",
];
const PADS: usize = 13;
const KICK: usize = 0;
const CLOSED_HAT: usize = 5;
const OPEN_HAT: usize = 7;

/// Gruppi di controllo.
const G_KICK: u8 = 0;
const G_SNARE: u8 = 1;
const G_HAT: u8 = 2;
const G_TOM: u8 = 3;
const G_CYM: u8 = 4;

pub static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
    id: ids::DRUMKIT,
    name: "Drumkit",
    role: PluginRole::Generator,
    description: "Drum machine sintetica con kit 808, 909, Acoustic, Lo-Fi, Electro e Trap su mappa General MIDI: \
                  kick, snare, clap, hi-hat con choke, tom, crash, ride, rim, cowbell, shaker.",
    params: &[
        ParamDesc::choice("kit", "Kit", KIT_NAMES, 0),
        ParamDesc::new("kick_level", "Kick livello", 0.0, 2.0, 1.0, ""),
        ParamDesc::new("kick_tune", "Kick intonazione", -12.0, 12.0, 0.0, "st"),
        ParamDesc::new("kick_decay", "Kick decay", 0.25, 4.0, 1.0, "×"),
        ParamDesc::new("snare_level", "Snare livello", 0.0, 2.0, 1.0, ""),
        ParamDesc::new("snare_tune", "Snare intonazione", -12.0, 12.0, 0.0, "st"),
        ParamDesc::new("snare_decay", "Snare decay", 0.25, 4.0, 1.0, "×"),
        ParamDesc::new("hat_level", "Hat livello", 0.0, 2.0, 1.0, ""),
        ParamDesc::new("hat_tune", "Hat intonazione", -12.0, 12.0, 0.0, "st"),
        ParamDesc::new("hat_decay", "Hat decay", 0.25, 4.0, 1.0, "×"),
        ParamDesc::new("tom_level", "Tom livello", 0.0, 2.0, 1.0, ""),
        ParamDesc::new("tom_tune", "Tom intonazione", -12.0, 12.0, 0.0, "st"),
        ParamDesc::new("tom_decay", "Tom decay", 0.25, 4.0, 1.0, "×"),
        ParamDesc::new("cym_level", "Piatti livello", 0.0, 2.0, 1.0, ""),
        ParamDesc::new("cym_tune", "Piatti intonazione", -12.0, 12.0, 0.0, "st"),
        ParamDesc::new("cym_decay", "Piatti decay", 0.25, 4.0, 1.0, "×"),
        ParamDesc::new("drive", "Drive", 0.0, 1.0, 0.0, ""),
        ParamDesc::new("amp", "Volume", 0.0, 2.0, 0.8, ""),
    ],
};

/// Preset di fabbrica: i kit e alcune varianti.
pub static PRESETS: &[Preset] = &[
    Preset {
        name: "808",
        category: "Kit",
        values: &[("kit", 0.0)],
    },
    Preset {
        name: "909",
        category: "Kit",
        values: &[("kit", 1.0)],
    },
    Preset {
        name: "Acoustic",
        category: "Kit",
        values: &[("kit", 2.0)],
    },
    Preset {
        name: "Lo-Fi",
        category: "Kit",
        values: &[("kit", 3.0)],
    },
    Preset {
        name: "Electro",
        category: "Kit",
        values: &[("kit", 4.0)],
    },
    Preset {
        name: "Trap",
        category: "Kit",
        values: &[("kit", 5.0)],
    },
    Preset {
        name: "808 Boom",
        category: "Variante",
        values: &[
            ("kit", 0.0),
            ("kick_decay", 2.5),
            ("kick_tune", -2.0),
            ("kick_level", 1.2),
            ("drive", 0.3),
            ("amp", 0.65),
        ],
    },
    Preset {
        name: "909 Hard",
        category: "Variante",
        values: &[
            ("kit", 1.0),
            ("drive", 0.5),
            ("snare_tune", 2.0),
            ("hat_decay", 0.7),
            ("amp", 0.65),
        ],
    },
    Preset {
        name: "Acoustic Tight",
        category: "Variante",
        values: &[
            ("kit", 2.0),
            ("snare_decay", 0.6),
            ("tom_decay", 0.6),
            ("cym_decay", 0.6),
            ("hat_decay", 0.7),
        ],
    },
    Preset {
        name: "Trap Dark",
        category: "Variante",
        values: &[
            ("kit", 5.0),
            ("kick_tune", -3.0),
            ("kick_decay", 1.5),
            ("hat_tune", -4.0),
            ("snare_tune", -3.0),
        ],
    },
];

/// Modo del filtro del rumore.
const N_LP: u8 = 0;
const N_HP: u8 = 1;
const N_BP: u8 = 2;

/// Ricetta di sintesi di un pad.
#[derive(Debug, Clone, Copy)]
struct Pad {
    group: u8,
    /// Corpo tonale: frequenza (0 = assente), forma, livello e decadimento (s).
    tone_hz: f32,
    tone_wave: Waveform,
    tone_level: f32,
    tone_decay: f32,
    /// Secondo oscillatore del corpo (rapporto, 0 = assente) e suo livello.
    tone2_ratio: f32,
    tone2_level: f32,
    /// Inviluppo di pitch: ottave sopra la frequenza base all'attacco, costante di tempo (s).
    sweep: f32,
    sweep_time: f32,
    /// Passa-banda sul corpo (Hz, 0 = assente).
    tone_bp: f32,
    /// Sei quadre metalliche: livello, scala di frequenza, passa-alto (Hz), decadimento (s).
    metal_level: f32,
    metal_scale: f32,
    metal_hp: f32,
    metal_decay: f32,
    /// Rumore filtrato.
    noise_level: f32,
    noise_decay: f32,
    noise_mode: u8,
    noise_hz: f32,
    noise_q: f32,
    /// Colpi ripetuti del rumore prima della coda (clap).
    bursts: u8,
    /// Click d'attacco (rumore, 3 ms).
    click: f32,
    /// Saturazione del pad.
    drive: f32,
    pan: f32,
    gain: f32,
}

const BASE: Pad = Pad {
    group: G_KICK,
    tone_hz: 0.0,
    tone_wave: Waveform::Sine,
    tone_level: 0.0,
    tone_decay: 0.2,
    tone2_ratio: 0.0,
    tone2_level: 0.0,
    sweep: 0.0,
    sweep_time: 0.02,
    tone_bp: 0.0,
    metal_level: 0.0,
    metal_scale: 1.0,
    metal_hp: 7_000.0,
    metal_decay: 0.1,
    noise_level: 0.0,
    noise_decay: 0.1,
    noise_mode: N_HP,
    noise_hz: 5_000.0,
    noise_q: 0.7,
    bursts: 0,
    click: 0.0,
    drive: 0.0,
    pan: 0.0,
    gain: 1.0,
};

const fn kick(hz: f32, sweep: f32, sweep_time: f32, decay: f32, click: f32, drive: f32) -> Pad {
    Pad {
        group: G_KICK,
        tone_hz: hz,
        tone_level: 1.0,
        tone_decay: decay,
        sweep,
        sweep_time,
        click,
        drive,
        gain: 1.3,
        ..BASE
    }
}

const fn snare(hz: f32, decay: f32, noise_mode: u8, noise_hz: f32, noise_decay: f32, noise_level: f32) -> Pad {
    Pad {
        group: G_SNARE,
        tone_hz: hz,
        tone_wave: Waveform::Triangle,
        tone_level: 0.7,
        tone_decay: decay,
        tone2_ratio: 1.6,
        tone2_level: 0.5,
        sweep: 0.3,
        sweep_time: 0.015,
        noise_level,
        noise_decay,
        noise_mode,
        noise_hz,
        click: 0.2,
        gain: 0.6,
        ..BASE
    }
}

const fn clap(hz: f32, decay: f32, bursts: u8) -> Pad {
    Pad {
        group: G_SNARE,
        noise_level: 3.6,
        noise_decay: decay,
        noise_mode: N_BP,
        noise_hz: hz,
        noise_q: 1.6,
        bursts,
        pan: 0.1,
        ..BASE
    }
}

const fn hat(scale: f32, decay: f32, metal: f32, noise: f32, hp: f32) -> Pad {
    Pad {
        group: G_HAT,
        metal_level: metal,
        metal_scale: scale,
        metal_hp: hp,
        metal_decay: decay,
        noise_level: noise,
        noise_decay: decay,
        noise_mode: N_HP,
        noise_hz: hp,
        pan: 0.25,
        ..BASE
    }
}

const fn tom(hz: f32, decay: f32, sweep: f32, noise: f32, pan: f32) -> Pad {
    Pad {
        group: G_TOM,
        tone_hz: hz,
        tone_level: 1.0,
        tone_decay: decay,
        sweep,
        sweep_time: 0.05,
        noise_level: noise,
        noise_decay: 0.05,
        noise_mode: N_LP,
        noise_hz: 3_000.0,
        click: 0.15,
        pan,
        gain: 0.7,
        ..BASE
    }
}

const fn cym(scale: f32, decay: f32, metal: f32, noise: f32, hp: f32, pan: f32) -> Pad {
    Pad {
        group: G_CYM,
        metal_level: metal,
        metal_scale: scale,
        metal_hp: hp,
        metal_decay: decay,
        noise_level: noise,
        noise_decay: decay * 0.8,
        noise_mode: N_HP,
        noise_hz: hp,
        pan,
        gain: 0.8,
        ..BASE
    }
}

const fn rim(hz: f32, decay: f32) -> Pad {
    Pad {
        group: G_SNARE,
        tone_hz: hz,
        tone_level: 0.6,
        tone_decay: decay,
        noise_level: 0.8,
        noise_decay: 0.012,
        noise_mode: N_BP,
        noise_hz: 2_500.0,
        noise_q: 2.5,
        pan: -0.15,
        ..BASE
    }
}

const fn cowbell(hz: f32, decay: f32) -> Pad {
    Pad {
        group: G_TOM,
        tone_hz: hz,
        tone_wave: Waveform::Square,
        tone_level: 0.25,
        tone_decay: decay,
        tone2_ratio: 1.48,
        tone2_level: 1.0,
        tone_bp: 2.0,
        pan: -0.3,
        ..BASE
    }
}

const fn shaker(decay: f32, hz: f32, metal: f32) -> Pad {
    Pad {
        group: G_HAT,
        metal_level: metal,
        metal_scale: 3.0,
        metal_hp: hz,
        metal_decay: decay,
        noise_level: 0.8,
        noise_decay: decay,
        noise_mode: N_HP,
        noise_hz: hz,
        pan: 0.35,
        gain: 0.7,
        ..BASE
    }
}

/// Kit: pad e degrado globale (bit, campioni tenuti) per il Lo-Fi.
#[derive(Debug, Clone, Copy)]
struct Kit {
    pads: [Pad; PADS],
    bits: f32,
    hold: u32,
}

static KITS: [Kit; 6] = [
    // 808: kick lungo e morbido, snare a due toni, hat metallici, tom profondi.
    Kit {
        pads: [
            kick(49.0, 1.4, 0.04, 0.9, 0.05, 0.1),
            rim(1_700.0, 0.03),
            snare(238.0, 0.12, N_HP, 1_800.0, 0.18, 0.7),
            clap(1_100.0, 0.25, 3),
            tom(80.0, 0.6, 0.6, 0.0, -0.4),
            hat(1.0, 0.05, 1.0, 0.2, 7_000.0),
            tom(110.0, 0.5, 0.6, 0.0, 0.0),
            hat(1.0, 0.8, 1.0, 0.2, 7_000.0),
            tom(150.0, 0.45, 0.6, 0.0, 0.4),
            cym(1.3, 1.6, 0.8, 0.5, 5_000.0, -0.3),
            cym(1.8, 2.0, 0.9, 0.2, 6_500.0, 0.3),
            shaker(0.07, 6_000.0, 0.0),
            cowbell(540.0, 0.35),
        ],
        bits: 0.0,
        hold: 1,
    },
    // 909: kick incisivo con click, snare rumoroso, hat più brillanti.
    Kit {
        pads: [
            kick(55.0, 2.0, 0.022, 0.45, 0.45, 0.25),
            rim(1_900.0, 0.025),
            snare(190.0, 0.1, N_HP, 1_200.0, 0.25, 0.95),
            clap(1_300.0, 0.3, 3),
            tom(100.0, 0.35, 0.8, 0.25, -0.4),
            hat(1.25, 0.045, 0.6, 0.6, 8_000.0),
            tom(140.0, 0.3, 0.8, 0.25, 0.0),
            hat(1.25, 0.6, 0.6, 0.6, 8_000.0),
            tom(190.0, 0.28, 0.8, 0.25, 0.4),
            cym(1.5, 1.4, 0.6, 0.8, 5_500.0, -0.3),
            cym(2.1, 1.8, 0.8, 0.4, 7_000.0, 0.3),
            shaker(0.06, 7_000.0, 0.2),
            cowbell(560.0, 0.3),
        ],
        bits: 0.0,
        hold: 1,
    },
    // Acoustic: kick con battente, rullante con cordiera, piatti ricchi di rumore.
    Kit {
        pads: [
            Pad {
                noise_level: 0.35,
                noise_decay: 0.04,
                noise_mode: N_LP,
                noise_hz: 600.0,
                ..kick(62.0, 0.9, 0.012, 0.3, 0.55, 0.0)
            },
            rim(1_500.0, 0.02),
            Pad {
                noise_q: 0.5,
                ..snare(200.0, 0.12, N_BP, 3_500.0, 0.3, 1.1)
            },
            clap(1_200.0, 0.2, 2),
            tom(95.0, 0.45, 0.3, 0.3, -0.4),
            hat(2.2, 0.05, 0.3, 0.6, 6_000.0),
            tom(135.0, 0.4, 0.3, 0.3, 0.0),
            hat(2.2, 0.8, 0.3, 0.6, 6_000.0),
            tom(185.0, 0.35, 0.3, 0.3, 0.4),
            cym(2.5, 2.2, 0.4, 1.0, 4_000.0, -0.35),
            cym(2.8, 2.5, 0.7, 0.4, 5_500.0, 0.35),
            // Tamburello: sonagli metallici e rumore.
            shaker(0.18, 7_500.0, 0.7),
            cowbell(600.0, 0.25),
        ],
        bits: 0.0,
        hold: 1,
    },
    // Lo-Fi: suoni morbidi, degradati a 8 bit e sottocampionati.
    Kit {
        pads: [
            kick(52.0, 1.6, 0.03, 0.5, 0.2, 0.4),
            rim(1_400.0, 0.03),
            snare(210.0, 0.12, N_LP, 4_000.0, 0.2, 1.2),
            clap(1_000.0, 0.22, 3),
            tom(90.0, 0.4, 0.5, 0.1, -0.4),
            Pad {
                noise_mode: N_BP,
                ..hat(0.9, 0.06, 0.5, 0.6, 5_000.0)
            },
            tom(120.0, 0.35, 0.5, 0.1, 0.0),
            Pad {
                noise_mode: N_BP,
                ..hat(0.9, 0.5, 0.5, 0.6, 5_000.0)
            },
            tom(160.0, 0.3, 0.5, 0.1, 0.4),
            cym(1.2, 1.0, 0.5, 0.6, 4_000.0, -0.3),
            cym(1.6, 1.3, 0.6, 0.3, 4_500.0, 0.3),
            shaker(0.08, 4_500.0, 0.0),
            cowbell(520.0, 0.3),
        ],
        bits: 8.0,
        hold: 3,
    },
    // Electro: sweep rapidi e saturati, snare e tom "laser".
    Kit {
        pads: [
            kick(48.0, 3.0, 0.012, 0.35, 0.3, 0.6),
            rim(2_200.0, 0.02),
            Pad {
                sweep: 0.8,
                sweep_time: 0.02,
                ..snare(260.0, 0.1, N_HP, 3_000.0, 0.12, 0.8)
            },
            clap(1_500.0, 0.18, 4),
            tom(110.0, 0.3, 1.8, 0.0, -0.5),
            hat(1.6, 0.035, 0.9, 0.3, 9_000.0),
            tom(150.0, 0.28, 1.8, 0.0, 0.0),
            hat(1.6, 0.45, 0.9, 0.3, 9_000.0),
            tom(210.0, 0.25, 1.8, 0.0, 0.5),
            cym(1.7, 1.0, 0.9, 0.5, 6_000.0, -0.3),
            cym(2.3, 1.2, 1.0, 0.2, 7_500.0, 0.3),
            shaker(0.05, 8_000.0, 0.3),
            cowbell(640.0, 0.2),
        ],
        bits: 0.0,
        hold: 1,
    },
    // Trap: 808 lunghissimo e saturato, snare secco, hat stretti.
    Kit {
        pads: [
            kick(42.0, 1.2, 0.06, 1.8, 0.15, 0.5),
            rim(1_800.0, 0.025),
            snare(220.0, 0.08, N_HP, 2_500.0, 0.22, 0.9),
            clap(1_250.0, 0.28, 3),
            tom(70.0, 0.9, 1.0, 0.0, -0.4),
            hat(1.4, 0.03, 0.8, 0.4, 8_500.0),
            tom(95.0, 0.8, 1.0, 0.0, 0.0),
            hat(1.4, 0.5, 0.8, 0.4, 8_500.0),
            tom(125.0, 0.7, 1.0, 0.0, 0.4),
            cym(1.5, 1.5, 0.7, 0.6, 5_500.0, -0.3),
            cym(2.0, 1.8, 0.9, 0.3, 7_000.0, 0.3),
            shaker(0.06, 7_000.0, 0.0),
            cowbell(560.0, 0.3),
        ],
        bits: 0.0,
        hold: 1,
    },
];

/// Frequenze dei sei oscillatori metallici (stile 808).
const METAL: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
const VOICES: usize = 24;
/// Durata minima di una voce (s), per non chiudere prima del click.
const MIN_LEN: f32 = 0.02;
/// Guadagno di uscita per voce.
const VOICE_GAIN: f32 = 0.4;

/// Pad per un tasto MIDI: alias GM, altrimenti il pad più vicino.
pub fn pad_for_key(key: u8) -> usize {
    let alias = match key {
        35 => Some(36),
        40 => Some(38),
        43 => Some(41),
        44 => Some(42),
        47 => Some(45),
        50 => Some(48),
        52 | 55 | 57 => Some(49),
        53 | 59 => Some(51),
        69 | 70 => Some(54),
        _ => None,
    };
    let k = alias.unwrap_or(key);
    let mut best = 0;
    let mut dist = u8::MAX;
    for (i, pk) in PAD_KEYS.iter().enumerate() {
        let d = pk.abs_diff(k);
        if d < dist {
            dist = d;
            best = i;
        }
    }
    best
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    tag: VoiceTag,
    pad: Pad,
    pad_idx: usize,
    t: u32,
    ratio: f32,
    sweep_k: f32,
    penv: f32,
    pcoef: f32,
    tone: [Oscillator; 2],
    metal: [Oscillator; 6],
    tone_env: ExpDecay,
    metal_env: ExpDecay,
    noise_env: ExpDecay,
    burst_env: ExpDecay,
    click_env: ExpDecay,
    bursts: u8,
    burst_timer: u32,
    burst_len: u32,
    rng: Rng,
    noise_f: Svf,
    noise_norm: f32,
    tone_f: Svf,
    metal_f: [Svf; 2],
    drive_gain: f32,
    drive_norm: f32,
    gl: f32,
    gr: f32,
    bits: f32,
    hold: u32,
    held: f32,
    hold_count: u32,
}

impl PolyVoice for Voice {
    fn tag(&self) -> &VoiceTag {
        &self.tag
    }
}

impl Voice {
    fn new(sr: f32, seed: u32) -> Self {
        Self {
            tag: VoiceTag::default(),
            pad: BASE,
            pad_idx: 0,
            t: 0,
            ratio: 1.0,
            sweep_k: 0.0,
            penv: 0.0,
            pcoef: 0.0,
            tone: [Oscillator::new(sr); 2],
            metal: [Oscillator::new(sr); 6],
            tone_env: ExpDecay::new(0.2, sr),
            metal_env: ExpDecay::new(0.1, sr),
            noise_env: ExpDecay::new(0.1, sr),
            burst_env: ExpDecay::new(0.01, sr),
            click_env: ExpDecay::new(0.003, sr),
            bursts: 0,
            burst_timer: 0,
            burst_len: 1,
            rng: Rng::new(seed),
            noise_f: Svf::new(sr),
            noise_norm: 1.0,
            tone_f: Svf::new(sr),
            metal_f: [Svf::new(sr); 2],
            drive_gain: 1.0,
            drive_norm: 1.0,
            gl: 1.0,
            gr: 1.0,
            bits: 0.0,
            hold: 1,
            held: 0.0,
            hold_count: 0,
        }
    }

    fn set_sample_rate(&mut self, sr: f32) {
        for o in self.tone.iter_mut().chain(self.metal.iter_mut()) {
            o.set_sample_rate(sr);
        }
        for e in [
            &mut self.tone_env,
            &mut self.metal_env,
            &mut self.noise_env,
            &mut self.burst_env,
            &mut self.click_env,
        ] {
            e.set_sample_rate(sr);
        }
        let [m0, m1] = &mut self.metal_f;
        for f in [&mut self.noise_f, &mut self.tone_f, m0, m1] {
            f.set_sample_rate(sr);
        }
    }

    /// Prepara la voce per il pad (intonazione `ratio`, decadimento × `decay`).
    fn setup(&mut self, sr: f32, ratio: f32, decay: f32, drive: f32) {
        let p = self.pad;
        let nyq = 0.45 * sr;
        self.t = 0;
        self.ratio = ratio;
        self.sweep_k = p.sweep.exp2() - 1.0;
        self.penv = 1.0;
        self.pcoef = time_coef(p.sweep_time, sr);
        for (o, r) in self.tone.iter_mut().zip([1.0, p.tone2_ratio]) {
            o.set_waveform(p.tone_wave);
            o.set_frequency(p.tone_hz * r * ratio);
            o.reset(0.0);
        }
        for (o, f) in self.metal.iter_mut().zip(METAL) {
            o.set_waveform(Waveform::Square);
            o.set_frequency(f * p.metal_scale * ratio);
            o.reset(0.0);
        }
        self.tone_env.set_time(p.tone_decay * decay);
        self.tone_env.trigger(if p.tone_hz > 0.0 { p.tone_level } else { 0.0 });
        self.metal_env.set_time(p.metal_decay * decay);
        self.metal_env.trigger(p.metal_level);
        self.burst_env.set_time(0.011);
        self.noise_env.set_time(p.noise_decay * decay);
        self.bursts = if p.noise_level > 0.0 { p.bursts } else { 0 };
        self.burst_len = ((0.01 * sr) as u32).max(1);
        self.burst_timer = self.burst_len;
        self.burst_env.reset();
        self.noise_env.reset();
        if p.noise_level > 0.0 {
            if p.bursts > 0 {
                self.burst_env.trigger(1.0);
            } else {
                self.noise_env.trigger(1.0);
            }
        }
        self.click_env.set_time(0.003);
        self.click_env.trigger(p.click);
        let mode = match p.noise_mode {
            N_LP => SvfMode::Lowpass,
            N_BP => SvfMode::Bandpass,
            _ => SvfMode::Highpass,
        };
        self.noise_f.set_mode(mode);
        self.noise_f.set((p.noise_hz * ratio).min(nyq), p.noise_q);
        self.noise_norm = if mode == SvfMode::Bandpass {
            1.0 / p.noise_q
        } else {
            1.0
        };
        self.noise_f.reset();
        self.tone_f.set_mode(SvfMode::Bandpass);
        self.tone_f
            .set((p.tone_hz * 1.5 * ratio).clamp(20.0, nyq), p.tone_bp.max(0.5));
        self.tone_f.reset();
        for f in &mut self.metal_f {
            f.set_mode(SvfMode::Highpass);
            f.set((p.metal_hp * ratio.sqrt()).min(nyq), 0.7);
            f.reset();
        }
        let d = p.drive + drive;
        self.drive_gain = 1.0 + d * 4.0;
        self.drive_norm = 1.0 / soft_clip(self.drive_gain);
        self.held = 0.0;
        self.hold_count = 0;
    }

    /// Accorcia il decadimento (choke).
    fn choke(&mut self) {
        for e in [
            &mut self.tone_env,
            &mut self.metal_env,
            &mut self.noise_env,
            &mut self.burst_env,
        ] {
            e.set_time(0.01);
        }
        self.bursts = 0;
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        let p = self.pad;
        let noise = self.rng.next_bipolar();
        let mut y = 0.0;
        if self.tone_env.is_active() {
            let bend = 1.0 + self.sweep_k * self.penv;
            self.penv *= self.pcoef;
            let f = p.tone_hz * self.ratio * bend;
            self.tone[0].set_frequency(f);
            let mut s = self.tone[0].next_sample();
            if p.tone2_ratio > 0.0 {
                self.tone[1].set_frequency(f * p.tone2_ratio);
                s += self.tone[1].next_sample() * p.tone2_level;
            }
            if p.tone_bp > 0.0 {
                // Il passa-banda ha guadagno Q al centro.
                s = self.tone_f.process(s) * (1.5 / p.tone_bp);
            }
            y += s * self.tone_env.next_sample();
        }
        if self.metal_env.is_active() {
            let mut m = 0.0;
            for o in &mut self.metal {
                m += o.next_sample();
            }
            let hp1 = self.metal_f[0].process(m * (1.0 / 6.0));
            m = self.metal_f[1].process(hp1);
            y += m * self.metal_env.next_sample() * 1.5;
        }
        if self.bursts > 0 {
            self.burst_timer = self.burst_timer.saturating_sub(1);
            if self.burst_timer == 0 {
                self.bursts -= 1;
                self.burst_timer = self.burst_len;
                if self.bursts > 0 {
                    self.burst_env.trigger(1.0);
                } else {
                    self.noise_env.trigger(0.8);
                }
            }
        }
        if p.noise_level > 0.0 && (self.noise_env.is_active() || self.burst_env.is_active()) {
            let e = self.noise_env.next_sample() + self.burst_env.next_sample();
            y += self.noise_f.process(noise) * self.noise_norm * e * p.noise_level;
        }
        if self.click_env.is_active() {
            y += noise * self.click_env.next_sample();
        }
        if self.drive_gain > 1.0 {
            y = soft_clip(y * self.drive_gain) * self.drive_norm;
        }
        if self.bits > 0.0 {
            // Sample & hold: nuovo valore ogni `hold` campioni.
            if self.hold_count == 0 {
                self.held = (y * self.bits).round() / self.bits;
                self.hold_count = self.hold;
            }
            self.hold_count -= 1;
            y = self.held;
        }
        self.t = self.t.saturating_add(1);
        y
    }

    fn alive(&self, min_len: u32) -> bool {
        self.t < min_len
            || self.bursts > 0
            || self.tone_env.is_active()
            || self.metal_env.is_active()
            || self.noise_env.is_active()
            || self.burst_env.is_active()
            || self.click_env.is_active()
    }
}

/// Controlli di un gruppo: livello, intonazione (st), decadimento (×).
#[derive(Debug, Clone, Copy)]
struct Group {
    level: f32,
    tune: f32,
    decay: f32,
}

/// Drum machine Drumkit.
pub struct Drumkit {
    sr: f32,
    voices: Vec<Voice>,
    order: u64,
    pitch_offset: f32,
    kit: usize,
    groups: [Group; 5],
    drive: f32,
    amp: f32,
}

impl Default for Drumkit {
    fn default() -> Self {
        Self::new()
    }
}

impl Drumkit {
    pub fn new() -> Self {
        let voices = (0..VOICES)
            .map(|i| Voice::new(DEFAULT_SR, 0xD4_0000 + i as u32 * 7919))
            .collect();
        let mut s = Self {
            sr: DEFAULT_SR,
            voices,
            order: 0,
            pitch_offset: 0.0,
            kit: 0,
            groups: [Group {
                level: 1.0,
                tune: 0.0,
                decay: 1.0,
            }; 5],
            drive: 0.0,
            amp: 0.8,
        };
        apply_defaults(&DESCRIPTOR, |n, v| s.set_param(n, v));
        s
    }
}

impl Generator for Drumkit {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &DESCRIPTOR
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return;
        }
        self.sr = sample_rate;
        for v in &mut self.voices {
            v.tag.active = false;
            v.set_sample_rate(sample_rate);
        }
    }

    fn set_param(&mut self, name: &str, value: f32) {
        let Some(v) = param_value(&DESCRIPTOR, name, value) else {
            return;
        };
        if let Some((pre, field)) = name.split_once('_') {
            let g = match pre {
                "kick" => Some(G_KICK),
                "snare" => Some(G_SNARE),
                "hat" => Some(G_HAT),
                "tom" => Some(G_TOM),
                "cym" => Some(G_CYM),
                _ => None,
            };
            if let Some(g) = g {
                let grp = &mut self.groups[g as usize];
                match field {
                    "level" => grp.level = v,
                    "tune" => grp.tune = v,
                    "decay" => grp.decay = v,
                    _ => {}
                }
                return;
            }
        }
        match name {
            "kit" => self.kit = (v as usize).min(KITS.len() - 1),
            "drive" => self.drive = v,
            "amp" => self.amp = v,
            _ => {}
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let pad_idx = pad_for_key(note.key);
        if pad_idx == CLOSED_HAT {
            for v in &mut self.voices {
                if v.tag.active && v.pad_idx == OPEN_HAT {
                    v.choke();
                }
            }
        }
        let kit = &KITS[self.kit];
        let pad = kit.pads[pad_idx];
        let grp = self.groups[pad.group as usize];
        let fine = if note.fine_pitch_cents.is_finite() {
            note.fine_pitch_cents
        } else {
            0.0
        };
        let st = (grp.tune + (fine + self.pitch_offset) / 100.0).clamp(-48.0, 48.0);
        let vel = note.velocity.clamp(0.0, 1.0);
        let gain = (0.25 + 0.75 * vel) * grp.level * pad.gain;
        let pan = (pad.pan + note.pan.clamp(-1.0, 1.0)).clamp(-1.0, 1.0);
        let (pl, pr) = pan_gains(pan);
        let idx = pick_voice(&self.voices);
        self.order += 1;
        let v = &mut self.voices[idx];
        v.tag = VoiceTag {
            active: true,
            released: false,
            id: note.voice_id,
            order: self.order,
        };
        v.pad = pad;
        v.pad_idx = pad_idx;
        v.bits = if kit.bits > 0.0 { kit.bits.exp2() * 0.5 } else { 0.0 };
        v.hold = kit.hold.max(1);
        v.gl = gain * pl * std::f32::consts::SQRT_2;
        v.gr = gain * pr * std::f32::consts::SQRT_2;
        v.setup(self.sr, (st / 12.0).exp2(), grp.decay, self.drive);
        if pad_idx == KICK {
            // Il kick riparte in fase per un attacco ripetibile.
            v.tone[0].reset(0.0);
        }
    }

    /// I pad sono one-shot: il rilascio del tasto non li ferma.
    fn note_off(&mut self, _voice_id: u64) {}

    fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.tag.active {
                v.tag.released = true;
                v.choke();
                v.click_env.reset();
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
        let min_len = (MIN_LEN * self.sr) as u32;
        let master = self.amp * VOICE_GAIN;
        for v in self.voices.iter_mut().filter(|v| v.tag.active) {
            let (gl, gr) = (v.gl * master, v.gr * master);
            for i in 0..n {
                let y = v.tick();
                left[i] += y * gl;
                right[i] += y * gr;
                if !v.alive(min_len) {
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
