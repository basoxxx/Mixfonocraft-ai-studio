//! Misure offline di loudness su audio stereo: loudness integrata EBU R128 (ITU-R BS.1770-4),
//! RMS sulle sole parti attive e true peak con sovracampionamento 4×.
//!
//! Sono funzioni di analisi (allocano): da usare dopo un render, non nel thread audio.

/// Filtro biquad in f64 (forma diretta I) per la pesatura K.
#[derive(Clone, Copy)]
struct Bq {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Bq {
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b,
            a,
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// I due stadi della pesatura K (shelf acustico della testa + passa-alto RLB) per `sr`.
fn k_weighting(sr: f64) -> [Bq; 2] {
    use std::f64::consts::PI;
    // Stadio 1: shelf alto (+4 dB sopra ~1.5 kHz).
    let f0 = 1681.974450955533;
    let g = 3.999843853973347;
    let q = 0.7071752369554196;
    let k = (PI * f0 / sr).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Bq::new(
        [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    // Stadio 2: passa-alto a ~38 Hz.
    let f0 = 38.13547087602444;
    let q = 0.5003270373238773;
    let k = (PI * f0 / sr).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Bq::new([1.0, -2.0, 1.0], [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0]);
    [shelf, hp]
}

/// Loudness integrata in LUFS (blocchi di 400 ms con sovrapposizione del 75%, gate assoluto
/// a -70 LUFS e relativo a -10 LU). `None` se il segnale è silenzioso o più corto di un blocco.
pub fn integrated_lufs(left: &[f32], right: &[f32], sample_rate: f32) -> Option<f64> {
    let sr = sample_rate as f64;
    let n = left.len().min(right.len());
    let block = (0.4 * sr).round() as usize;
    let hop = (0.1 * sr).round().max(1.0) as usize;
    if block == 0 || n < block {
        return None;
    }
    // Energia pesata K per hop di 100 ms (somma dei due canali).
    let mut fl = k_weighting(sr);
    let mut fr = k_weighting(sr);
    let mut hops = Vec::with_capacity(n / hop + 1);
    let mut acc = 0.0;
    for i in 0..n {
        let l = fl[0].run(left[i] as f64);
        let l = fl[1].run(l);
        let r = fr[0].run(right[i] as f64);
        let r = fr[1].run(r);
        acc += l * l + r * r;
        if (i + 1) % hop == 0 {
            hops.push(acc);
            acc = 0.0;
        }
    }
    let per_block = block / hop;
    if hops.len() < per_block {
        return None;
    }
    let blocks: Vec<f64> = (0..=hops.len() - per_block)
        .map(|s| hops[s..s + per_block].iter().sum::<f64>() / (per_block * hop) as f64)
        .collect();
    let lufs = |z: f64| -0.691 + 10.0 * z.log10();
    let gated: Vec<f64> = blocks.iter().copied().filter(|&z| z > 0.0 && lufs(z) > -70.0).collect();
    if gated.is_empty() {
        return None;
    }
    let rel = lufs(gated.iter().sum::<f64>() / gated.len() as f64) - 10.0;
    let kept: Vec<f64> = gated.into_iter().filter(|&z| lufs(z) > rel).collect();
    (!kept.is_empty()).then(|| lufs(kept.iter().sum::<f64>() / kept.len() as f64))
}

/// RMS (lineare) calcolato solo sulle finestre di 50 ms il cui RMS supera `threshold_db` dBFS.
/// Restituisce 0 se nessuna finestra è attiva.
pub fn active_rms(left: &[f32], right: &[f32], sample_rate: f32, threshold_db: f32) -> f32 {
    let n = left.len().min(right.len());
    let win = ((0.05 * sample_rate as f64).round() as usize).max(1);
    let thr = 10f64.powf(threshold_db as f64 / 20.0);
    let thr2 = thr * thr;
    let (mut sum, mut count) = (0.0f64, 0usize);
    let mut start = 0;
    while start < n {
        let end = (start + win).min(n);
        let e: f64 = (start..end)
            .map(|i| (left[i] as f64).powi(2) + (right[i] as f64).powi(2))
            .sum();
        let frames = 2 * (end - start);
        if e / frames as f64 > thr2 {
            sum += e;
            count += frames;
        }
        start = end;
    }
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt() as f32
    }
}

/// Mezza lunghezza (per fase) del filtro interpolatore del true peak.
const TP_HALF: i64 = 8;

/// True peak (lineare): massimo assoluto del segnale sovracampionato 4× con un interpolatore
/// sinc finestrato (Hann), come raccomandato da BS.1770 per stimare i picchi tra i campioni.
pub fn true_peak(left: &[f32], right: &[f32]) -> f32 {
    // Coefficienti delle fasi 1/4, 2/4, 3/4 (la fase 0 è il campione stesso).
    let taps: Vec<[f64; (2 * TP_HALF) as usize]> = (1..4)
        .map(|k| {
            let frac = k as f64 / 4.0;
            let mut h = [0.0; (2 * TP_HALF) as usize];
            for (j, c) in h.iter_mut().enumerate() {
                let t = (j as i64 - TP_HALF + 1) as f64 - frac;
                let sinc = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
                };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * t / (TP_HALF as f64 + 1.0)).cos();
                *c = sinc * w;
            }
            h
        })
        .collect();
    let mut peak = 0f64;
    for x in [left, right] {
        let n = x.len() as i64;
        let at = |i: i64| if (0..n).contains(&i) { x[i as usize] as f64 } else { 0.0 };
        for i in 0..n {
            let (a, b) = (at(i).abs(), at(i + 1).abs());
            peak = peak.max(a);
            // Un picco tra due campioni molto sotto il massimo corrente non può superarlo
            // in pratica: si salta l'interpolazione (la misura resta esatta sui picchi).
            if a.max(b) < 0.5 * peak {
                continue;
            }
            for h in &taps {
                // Interpola tra i e i+1.
                let y: f64 = h
                    .iter()
                    .enumerate()
                    .map(|(j, c)| c * at(i + j as i64 - TP_HALF + 1))
                    .sum();
                peak = peak.max(y.abs());
            }
        }
    }
    peak as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, secs: f32, sr: f32) -> Vec<f32> {
        (0..(secs * sr) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn lufs_of_reference_sine() {
        // EBU Tech 3341: sinusoide a 1 kHz, -18 dBFS di picco, stereo → -18 LUFS (±0.1).
        let sr = 48_000.0;
        let x = sine(1000.0, 10f32.powf(-18.0 / 20.0), 5.0, sr);
        let l = integrated_lufs(&x, &x, sr).unwrap();
        assert!((l + 18.0).abs() < 0.15, "{l}");
        // Stessa misura a 44.1 kHz (coefficienti ricalcolati).
        let x = sine(1000.0, 10f32.powf(-18.0 / 20.0), 5.0, 44_100.0);
        let l = integrated_lufs(&x, &x, 44_100.0).unwrap();
        assert!((l + 18.0).abs() < 0.15, "{l}");
    }

    #[test]
    fn lufs_gates_silence() {
        let sr = 48_000.0;
        let mut x = sine(1000.0, 10f32.powf(-18.0 / 20.0), 3.0, sr);
        x.extend(std::iter::repeat_n(0.0, (sr * 6.0) as usize));
        let l = integrated_lufs(&x, &x, sr).unwrap();
        assert!((l + 18.0).abs() < 0.3, "il silenzio non deve abbassare la misura: {l}");
        assert!(integrated_lufs(&[0.0; 48_000], &[0.0; 48_000], sr).is_none());
    }

    #[test]
    fn active_rms_ignores_silence() {
        let sr = 48_000.0;
        let mut x = sine(440.0, 0.5, 1.0, sr);
        x.extend(std::iter::repeat_n(0.0, (sr * 3.0) as usize));
        let r = active_rms(&x, &x, sr, -60.0);
        assert!((r - 0.5 / std::f32::consts::SQRT_2).abs() < 0.01, "{r}");
        assert_eq!(active_rms(&[0.0; 100], &[0.0; 100], sr, -60.0), 0.0);
    }

    #[test]
    fn true_peak_finds_intersample_peaks() {
        // Sinusoide a fs/4 sfasata di 45°: i campioni valgono ±0.707, il picco vero è 1.
        let x: Vec<f32> = (0..4800)
            .map(|i| (std::f32::consts::FRAC_PI_2 * i as f32 + std::f32::consts::FRAC_PI_4).sin())
            .collect();
        let sample_peak = x.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(sample_peak < 0.72);
        let tp = true_peak(&x, &x);
        assert!(tp > 0.97 && tp < 1.05, "{tp}");
    }
}
