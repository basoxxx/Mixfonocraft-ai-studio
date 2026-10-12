//! Tempo musicale: tick, PPQ, conversioni tick ↔ secondi ↔ sample.

use serde::{Deserialize, Serialize};

/// Posizione/durata musicale in tick (risoluzione = PPQ del progetto).
pub type Tick = u64;

/// PPQ di default (FL Studio usa 96 per i progetti nuovi).
pub const DEFAULT_PPQ: u16 = 96;

/// Indicazione di tempo (es. 4/4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

/// Griglia temporale di un progetto: PPQ + metrica.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeBase {
    pub ppq: u16,
    pub signature: TimeSignature,
}

impl Default for TimeBase {
    fn default() -> Self {
        Self {
            ppq: DEFAULT_PPQ,
            signature: TimeSignature::default(),
        }
    }
}

impl TimeBase {
    /// Tick per una semiminima (beat).
    pub fn ticks_per_beat(&self) -> Tick {
        self.ppq as Tick
    }

    /// Tick per uno step dello step sequencer (1/16 di nota, 4 step per beat).
    pub fn ticks_per_step(&self) -> Tick {
        (self.ppq as Tick / 4).max(1)
    }

    /// Tick per una battuta, tenendo conto della metrica.
    pub fn ticks_per_bar(&self) -> Tick {
        let beat_len = self.ppq as Tick * 4 / self.signature.denominator.max(1) as Tick;
        beat_len * self.signature.numerator.max(1) as Tick
    }

    /// Converte un numero di battute (0-based) in tick.
    pub fn bars_to_ticks(&self, bars: f64) -> Tick {
        (bars * self.ticks_per_bar() as f64).round().max(0.0) as Tick
    }

    /// Converte tick in secondi a tempo costante.
    pub fn ticks_to_seconds(&self, ticks: f64, bpm: f64) -> f64 {
        ticks / self.ppq as f64 * 60.0 / bpm
    }

    /// Converte secondi in tick a tempo costante.
    pub fn seconds_to_ticks(&self, seconds: f64, bpm: f64) -> f64 {
        seconds * bpm / 60.0 * self.ppq as f64
    }

    /// Quanti sample dura un tick a un dato tempo e sample rate.
    pub fn samples_per_tick(&self, bpm: f64, sample_rate: f64) -> f64 {
        sample_rate * 60.0 / (bpm * self.ppq as f64)
    }

    /// Riscala una posizione da un PPQ diverso a questo.
    pub fn rescale_from(&self, ticks: Tick, from_ppq: u16) -> Tick {
        if from_ppq == self.ppq || from_ppq == 0 {
            return ticks;
        }
        ((ticks as u128 * self.ppq as u128 + from_ppq as u128 / 2) / from_ppq as u128) as Tick
    }

    /// Quantizza `ticks` alla griglia `grid` (arrotondamento al più vicino).
    pub fn quantize(ticks: Tick, grid: Tick) -> Tick {
        if grid == 0 {
            return ticks;
        }
        (ticks + grid / 2) / grid * grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_sizes() {
        let tb = TimeBase::default();
        assert_eq!(tb.ticks_per_beat(), 96);
        assert_eq!(tb.ticks_per_step(), 24);
        assert_eq!(tb.ticks_per_bar(), 384);
        let tb34 = TimeBase {
            ppq: 96,
            signature: TimeSignature {
                numerator: 3,
                denominator: 4,
            },
        };
        assert_eq!(tb34.ticks_per_bar(), 288);
        let tb68 = TimeBase {
            ppq: 96,
            signature: TimeSignature {
                numerator: 6,
                denominator: 8,
            },
        };
        assert_eq!(tb68.ticks_per_bar(), 288);
    }

    #[test]
    fn conversions() {
        let tb = TimeBase::default();
        // 120 BPM: un beat = 0.5 s
        assert!((tb.ticks_to_seconds(96.0, 120.0) - 0.5).abs() < 1e-12);
        assert!((tb.seconds_to_ticks(0.5, 120.0) - 96.0).abs() < 1e-9);
        assert!((tb.samples_per_tick(120.0, 48000.0) * 96.0 - 24000.0).abs() < 1e-6);
        assert_eq!(tb.rescale_from(960, 960), 96);
        assert_eq!(tb.rescale_from(960, 960 * 2), 48);
        assert_eq!(tb.rescale_from(77, 96), 77);
        assert_eq!(TimeBase::quantize(37, 24), 48);
        assert_eq!(TimeBase::quantize(35, 24), 24);
    }
}
