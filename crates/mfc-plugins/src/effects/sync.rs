//! Orologio sincronizzato al tempo per gli effetti ritmici (Ducker, Auto Pan).

/// Divisioni ritmiche (etichette dei parametri a scelta) e durata in beat.
pub(crate) const DIVISIONS: &[&str] = &["2/1", "1/1", "1/2", "1/4", "1/8", "1/16", "1/32"];
const DIVISION_BEATS: [f64; 7] = [8.0, 4.0, 2.0, 1.0, 0.5, 0.25, 0.125];

/// Durata in beat della divisione `index` (vedi [`DIVISIONS`]).
pub(crate) fn division_beats(index: f32) -> f64 {
    DIVISION_BEATS[(index.round().max(0.0) as usize).min(DIVISION_BEATS.len() - 1)]
}

/// Posizione musicale che segue il transport quando suona e scorre libera al tempo
/// corrente quando è fermo.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BeatClock {
    pub beat: f64,
    pub tempo: f64,
}

impl Default for BeatClock {
    fn default() -> Self {
        Self {
            beat: 0.0,
            tempo: 120.0,
        }
    }
}

impl BeatClock {
    pub fn set_transport(&mut self, tempo: f64, beat: f64, playing: bool) {
        if tempo.is_finite() && tempo > 0.0 {
            self.tempo = tempo;
        }
        if playing && beat.is_finite() {
            self.beat = beat;
        }
    }

    /// Beat per campione al tempo corrente.
    pub fn step(&self, sample_rate: f32) -> f64 {
        self.tempo / 60.0 / sample_rate as f64
    }

    /// Fase 0..1 in un ciclo lungo `cycle` beat, con sfasamento `offset` (0..1).
    pub fn phase(&self, cycle: f64, offset: f32) -> f32 {
        ((self.beat / cycle + offset as f64).rem_euclid(1.0)) as f32
    }
}
