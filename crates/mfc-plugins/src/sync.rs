//! Sincronizzazione al tempo del progetto (LFO e tempi di delay).
//!
//! I parametri di sync sono scelte discrete che usano [`SYNC_CHOICES`]: l'indice 0 è
//! "Off" (vale il rate/tempo libero del plugin), gli altri sono divisioni musicali.
//! Le battute sono considerate di 4/4 (4 semiminime): il plugin conosce solo il BPM,
//! ricevuto dal motore con [`Generator::set_tempo`](crate::Generator::set_tempo) /
//! [`Effect::set_tempo`](crate::Effect::set_tempo).

/// Tempo assunto finché il motore non comunica quello del progetto.
pub const DEFAULT_BPM: f32 = 120.0;

/// Etichette delle divisioni (valori del parametro di sync).
pub const SYNC_CHOICES: &[&str] = &[
    "Off",
    "4 battute",
    "2 battute",
    "1 battuta",
    "1/2",
    "1/4",
    "1/4 puntato",
    "1/4 terzina",
    "1/8",
    "1/8 puntato",
    "1/8 terzina",
    "1/16",
    "1/16 terzina",
    "1/32",
];

/// Durata di ogni divisione in semiminime (beat); l'indice 0 (Off) non è usato.
const SYNC_BEATS: [f64; 14] = [
    0.0,
    16.0,
    8.0,
    4.0,
    2.0,
    1.0,
    1.5,
    2.0 / 3.0,
    0.5,
    0.75,
    1.0 / 3.0,
    0.25,
    1.0 / 6.0,
    0.125,
];

/// Indice di sync da valore di parametro (arrotondato). `None` = Off o fuori intervallo.
fn index(sync: f32) -> Option<usize> {
    if !sync.is_finite() {
        return None;
    }
    let i = sync.round();
    (i >= 1.0 && (i as usize) < SYNC_BEATS.len()).then_some(i as usize)
}

/// BPM utilizzabile (valori non validi → [`DEFAULT_BPM`]).
pub fn sanitize_bpm(bpm: f32) -> f32 {
    if bpm.is_finite() && bpm > 0.0 {
        bpm.clamp(1.0, 2000.0)
    } else {
        DEFAULT_BPM
    }
}

/// Durata della divisione in beat; `None` se il sync è Off.
pub fn sync_beats(sync: f32) -> Option<f64> {
    index(sync).map(|i| SYNC_BEATS[i])
}

/// Durata della divisione in secondi al tempo `bpm`; `None` se il sync è Off.
pub fn sync_seconds(sync: f32, bpm: f32) -> Option<f32> {
    sync_beats(sync).map(|b| (b * 60.0 / sanitize_bpm(bpm) as f64) as f32)
}

/// Frequenza (Hz) di un LFO con periodo pari alla divisione; `None` se il sync è Off.
pub fn sync_rate_hz(sync: f32, bpm: f32) -> Option<f32> {
    sync_beats(sync).map(|b| (sanitize_bpm(bpm) as f64 / 60.0 / b) as f32)
}

/// Rate effettivo di un LFO: la divisione sincronizzata se attiva, altrimenti `free_hz`.
pub fn lfo_rate(free_hz: f32, sync: f32, bpm: f32) -> f32 {
    sync_rate_hz(sync, bpm).unwrap_or(free_hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(label: &str) -> f32 {
        SYNC_CHOICES.iter().position(|c| *c == label).unwrap() as f32
    }

    #[test]
    fn tables_have_same_length() {
        assert_eq!(SYNC_CHOICES.len(), SYNC_BEATS.len());
    }

    #[test]
    fn quarter_at_120_is_2_hz() {
        assert!((sync_rate_hz(idx("1/4"), 120.0).unwrap() - 2.0).abs() < 1e-6);
        assert!((sync_seconds(idx("1/4"), 120.0).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn divisions() {
        let s = |l: &str| sync_seconds(idx(l), 120.0).unwrap();
        assert!((s("1 battuta") - 2.0).abs() < 1e-6);
        assert!((s("4 battute") - 8.0).abs() < 1e-6);
        assert!((s("1/8") - 0.25).abs() < 1e-6);
        assert!((s("1/8 puntato") - 0.375).abs() < 1e-6);
        assert!((s("1/8 terzina") - 1.0 / 6.0).abs() < 1e-6);
        assert!((s("1/4 terzina") - 1.0 / 3.0).abs() < 1e-6);
        assert!((s("1/16 terzina") - 1.0 / 12.0).abs() < 1e-6);
        assert!((s("1/32") - 0.0625).abs() < 1e-6);
    }

    #[test]
    fn off_uses_free_rate() {
        assert_eq!(sync_beats(0.0), None);
        assert_eq!(lfo_rate(3.5, 0.0, 128.0), 3.5);
        assert_eq!(sync_beats(99.0), None);
        assert_eq!(sync_beats(f32::NAN), None);
        assert_eq!(sanitize_bpm(f32::NAN), DEFAULT_BPM);
    }
}
