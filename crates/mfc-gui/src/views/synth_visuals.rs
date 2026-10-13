//! Anteprime grafiche dei gruppi di parametri dei synth (editor generico dei plugin):
//! curva ADSR, forma d'onda dell'oscillatore, forma dell'LFO, risposta del filtro.
//!
//! Il riconoscimento è guidato dai dati: si guarda il suffisso dei nomi dei parametri
//! di un gruppo (`aenv_attack` → `attack`), così funziona con Nova e con i synth futuri
//! che seguono le stesse convenzioni. Il disegno usa solo poche polilinee per anteprima.

use std::f32::consts::TAU;

use egui::{pos2, vec2, Color32, Pos2, Sense, Shape, Stroke, Ui};
use mfc_plugins::ParamDesc;

use crate::theme::{self, pal};

/// Dimensione di un'anteprima.
pub const VIS_SIZE: egui::Vec2 = egui::Vec2::new(168.0, 64.0);
/// Punti per curva (compromesso nitidezza / costo).
const POINTS: usize = 96;

/// Anteprima riconosciuta in un gruppo; contiene i nomi completi dei parametri usati.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visual {
    Adsr {
        attack: &'static str,
        decay: &'static str,
        sustain: &'static str,
        release: &'static str,
    },
    Wave {
        wave: &'static str,
        pw: Option<&'static str>,
    },
    Lfo {
        shape: &'static str,
        rate: &'static str,
        amount: Option<&'static str>,
    },
    Filter {
        cutoff: &'static str,
        reso: &'static str,
        mode: Option<&'static str>,
        slope: Option<&'static str>,
    },
}

/// Ultima parola del nome (`flt_cutoff` → `cutoff`).
fn suffix(name: &str) -> &str {
    name.rsplit('_').next().unwrap_or(name)
}

fn find<'a>(params: &[&'a ParamDesc], keys: &[&str], choice: Option<bool>) -> Option<&'a ParamDesc> {
    params
        .iter()
        .copied()
        .find(|p| keys.contains(&suffix(p.name)) && choice.is_none_or(|c| c == !p.choices.is_empty()))
}

/// Anteprime applicabili a un gruppo di parametri.
pub fn detect(params: &[&ParamDesc]) -> Vec<Visual> {
    let mut out = Vec::new();
    let a = find(params, &["attack"], Some(false));
    let d = find(params, &["decay"], Some(false));
    let s = find(params, &["sustain"], Some(false));
    let r = find(params, &["release"], Some(false));
    if let (Some(a), Some(d), Some(s), Some(r)) = (a, d, s, r) {
        out.push(Visual::Adsr {
            attack: a.name,
            decay: d.name,
            sustain: s.name,
            release: r.name,
        });
    }
    // Oscillatore: scelta della forma d'onda, PW opzionale.
    if let Some(w) = find(params, &["wave", "waveform"], Some(true)) {
        out.push(Visual::Wave {
            wave: w.name,
            pw: find(params, &["pw", "width"], Some(false)).map(|p| p.name),
        });
    }
    // LFO: forma + velocità.
    if let (Some(shape), Some(rate)) = (
        find(params, &["shape"], Some(true)),
        find(params, &["rate"], Some(false)),
    ) {
        out.push(Visual::Lfo {
            shape: shape.name,
            rate: rate.name,
            amount: find(params, &["amount", "depth"], Some(false)).map(|p| p.name),
        });
    }
    // Filtro: cutoff in Hz + risonanza, tipo e pendenza opzionali.
    if let (Some(c), Some(q)) = (
        find(params, &["cutoff"], Some(false)).filter(|p| p.unit == "Hz"),
        find(params, &["reso", "resonance", "q"], Some(false)),
    ) {
        out.push(Visual::Filter {
            cutoff: c.name,
            reso: q.name,
            mode: find(params, &["mode", "type"], Some(true)).map(|p| p.name),
            slope: find(params, &["slope"], Some(true)).map(|p| p.name),
        });
    }
    out
}

/// Etichetta della scelta corrente di un parametro a scelta (minuscola).
fn choice_label(params: &[&ParamDesc], name: &str, value: f32) -> String {
    params
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.choices.get((value.round() - p.min).max(0.0) as usize))
        .map_or_else(String::new, |s| s.to_ascii_lowercase())
}

fn unit_of(params: &[&ParamDesc], name: &str) -> &'static str {
    params.iter().find(|p| p.name == name).map_or("", |p| p.unit)
}

/// Forme d'onda riconosciute dalle etichette delle scelte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape1 {
    Sine,
    Triangle,
    Saw,
    Square,
    Noise,
    SampleHold,
}

/// Forma dall'etichetta della scelta ("Saw", "Square", "S&H"...).
pub fn shape_from_label(label: &str) -> Shape1 {
    let l = label.to_ascii_lowercase();
    if l.contains("s&h") || l.contains("sample") || l.contains("random") {
        Shape1::SampleHold
    } else if l.contains("noise") || l.contains("rumore") {
        Shape1::Noise
    } else if l.contains("tri") {
        Shape1::Triangle
    } else if l.contains("saw") || l.contains("ramp") {
        Shape1::Saw
    } else if l.contains("square") || l.contains("pulse") || l.contains("quadr") {
        Shape1::Square
    } else {
        Shape1::Sine
    }
}

/// Valore -1..1 della forma alla fase `ph` (0..1); `pw` = duty cycle del quadrato.
pub fn shape_value(shape: Shape1, ph: f32, pw: f32) -> f32 {
    let ph = ph.rem_euclid(1.0);
    // Rumore deterministico (hash della fase quantizzata): l'anteprima non "balla".
    let hash = |k: u32| {
        let mut x = k.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
        x ^= x >> 15;
        x = x.wrapping_mul(0x2c1b_3c6d);
        x ^= x >> 12;
        (x & 0xffff) as f32 / 32767.5 - 1.0
    };
    match shape {
        Shape1::Sine => (ph * TAU).sin(),
        Shape1::Triangle => 1.0 - 4.0 * (ph - 0.25).rem_euclid(1.0).min(1.0 - (ph - 0.25).rem_euclid(1.0)),
        Shape1::Saw => 2.0 * ph - 1.0,
        Shape1::Square => {
            if ph < pw.clamp(0.02, 0.98) {
                1.0
            } else {
                -1.0
            }
        }
        Shape1::Noise => hash((ph * 64.0) as u32),
        Shape1::SampleHold => hash((ph * 4.0) as u32 + 7),
    }
}

/// Punti (x 0..1, y 0..1 dal basso) della curva ADSR. Tempi in secondi.
/// La scala del tempo è compressa (radice) così anche gli attacchi brevi si vedono.
pub fn adsr_points(a: f32, d: f32, s: f32, r: f32) -> Vec<(f32, f32)> {
    let w = |t: f32| t.max(0.0).sqrt().max(0.02);
    let (wa, wd, wr) = (w(a), w(d), w(r));
    let ws = ((wa + wd + wr) * 0.35).max(0.15);
    let total = wa + wd + ws + wr;
    let s = s.clamp(0.0, 1.0);
    let mut pts = Vec::with_capacity(4 + 3 * 16);
    let n = 16;
    pts.push((0.0, 0.0));
    // Attacco leggermente convesso.
    for i in 1..=n {
        let k = i as f32 / n as f32;
        pts.push((wa * k / total, 1.0 - (1.0 - k).powi(2)));
    }
    // Decadimento esponenziale verso il sustain.
    let x0 = wa / total;
    for i in 1..=n {
        let k = i as f32 / n as f32;
        let y = s + (1.0 - s) * (-(k * 4.0)).exp() * (1.0 - k);
        pts.push((x0 + wd * k / total, y));
    }
    let x1 = (wa + wd + ws) / total;
    pts.push((x1, s));
    // Rilascio esponenziale a zero.
    for i in 1..=n {
        let k = i as f32 / n as f32;
        let y = s * (-(k * 4.0)).exp() * (1.0 - k);
        pts.push((x1 + wr * k / total, y));
    }
    pts
}

/// Risposta in ampiezza approssimata (filtro a 2 poli, o 4 per "24 dB").
/// `mode`: 0 = passa basso, 1 = passa alto, 2 = passa banda, 3 = notch.
pub fn filter_gain(freq: f32, cutoff: f32, reso: f32, mode: u8, four_pole: bool) -> f32 {
    let x = (freq / cutoff.max(1.0)).max(1e-4);
    let q = 0.707 + reso.clamp(0.0, 1.0) * 8.0;
    let den = ((1.0 - x * x).powi(2) + (x / q).powi(2)).sqrt().max(1e-6);
    let g = match mode {
        1 => x * x / den,
        2 => (x / q) / den,
        3 => (1.0 - x * x).abs() / den,
        _ => 1.0 / den,
    };
    if four_pole {
        g * g
    } else {
        g
    }
}

/// Mappatura delle etichette dei tipi di filtro sull'indice di `filter_gain`.
fn filter_mode_from_label(l: &str) -> u8 {
    if l.contains("high") || l.contains("hp") || l.contains("alto") {
        1
    } else if l.contains("band") || l.contains("bp") {
        2
    } else if l.contains("notch") {
        3
    } else {
        0
    }
}

/// Disegna l'anteprima. `value(name)` restituisce il valore corrente (anche durante il drag).
pub fn show(ui: &mut Ui, visual: &Visual, params: &[&ParamDesc], value: &dyn Fn(&str) -> f32) {
    let (rect, _) = ui.allocate_exact_size(VIS_SIZE, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    p.rect_filled(rect, 5.0, pal::BG_INSET());
    p.rect_stroke(rect, 5.0, Stroke::new(1.0, pal::BORDER()), egui::StrokeKind::Inside);
    let inner = rect.shrink2(vec2(8.0, 7.0));
    let grid = Stroke::new(1.0, theme::mix(pal::BG_INSET(), pal::TEXT_FAINT(), 0.25));
    let line = Stroke::new(1.6, pal::ACCENT());
    let fill = pal::ACCENT().gamma_multiply(0.18);
    let to_screen = |x: f32, y: f32| pos2(inner.left() + x * inner.width(), inner.bottom() - y * inner.height());
    let caption = |text: &str| {
        p.text(
            rect.right_top() + vec2(-6.0, 4.0),
            egui::Align2::RIGHT_TOP,
            text,
            egui::FontId::proportional(9.0),
            pal::TEXT_FAINT(),
        );
    };
    match visual {
        Visual::Adsr {
            attack,
            decay,
            sustain,
            release,
        } => {
            let secs = |n: &str| {
                let v = value(n);
                if unit_of(params, n) == "ms" {
                    v / 1000.0
                } else {
                    v
                }
            };
            let pts: Vec<Pos2> = adsr_points(secs(attack), secs(decay), value(sustain), secs(release))
                .into_iter()
                .map(|(x, y)| to_screen(x, y * 0.92))
                .collect();
            let base = inner.bottom();
            p.line_segment([pos2(inner.left(), base), pos2(inner.right(), base)], grid);
            fill_under(p, &pts, base, fill);
            p.add(Shape::line(pts, line));
            caption("ADSR");
        }
        Visual::Wave { wave, pw } => {
            let shape = shape_from_label(&choice_label(params, wave, value(wave)));
            let duty = pw.map_or(0.5, value);
            let mid = inner.center().y;
            p.line_segment([pos2(inner.left(), mid), pos2(inner.right(), mid)], grid);
            let pts = sampled(shape, 2.0, duty, 0.9, |x, y| to_screen(x, 0.5 + 0.5 * y));
            p.add(Shape::line(pts, line));
            caption("OSC");
        }
        Visual::Lfo { shape, rate, amount } => {
            let s = shape_from_label(&choice_label(params, shape, value(shape)));
            // Finestra di un secondo: tanti cicli quanti gli Hz (limitati per leggibilità).
            let cycles = value(rate).clamp(0.5, 12.0);
            let depth = amount.map_or(0.9, |n| 0.15 + 0.75 * value(n).clamp(0.0, 1.0));
            let mid = inner.center().y;
            p.line_segment([pos2(inner.left(), mid), pos2(inner.right(), mid)], grid);
            let pts = sampled(s, cycles, 0.5, depth, |x, y| to_screen(x, 0.5 + 0.5 * y));
            p.add(Shape::line(pts, Stroke::new(1.6, pal::BLUE())));
            caption(&format!("LFO {:.2} Hz", value(rate)));
        }
        Visual::Filter {
            cutoff,
            reso,
            mode,
            slope,
        } => {
            let fc = value(cutoff);
            let q = value(reso);
            let m = mode.map_or(0, |n| filter_mode_from_label(&choice_label(params, n, value(n))));
            let four = slope.is_some_and(|n| choice_label(params, n, value(n)).contains("24"));
            // Asse log 20 Hz..20 kHz, ampiezza -36..+18 dB.
            let (db_lo, db_hi) = (-36.0f32, 18.0f32);
            let y_of = |db: f32| ((db - db_lo) / (db_hi - db_lo)).clamp(0.0, 1.0);
            for f in [100.0f32, 1000.0, 10_000.0] {
                let x = (f / 20.0).log10() / 3.0;
                p.line_segment([to_screen(x, 0.0), to_screen(x, 1.0)], grid);
            }
            p.line_segment([to_screen(0.0, y_of(0.0)), to_screen(1.0, y_of(0.0))], grid);
            let pts: Vec<Pos2> = (0..=POINTS)
                .map(|i| {
                    let x = i as f32 / POINTS as f32;
                    let f = 20.0 * 10f32.powf(3.0 * x);
                    let db = 20.0 * filter_gain(f, fc, q, m, four).max(1e-5).log10();
                    to_screen(x, y_of(db))
                })
                .collect();
            fill_under(p, &pts, inner.bottom(), fill);
            p.add(Shape::line(pts, line));
            let fx = ((fc.max(20.0) / 20.0).log10() / 3.0).clamp(0.0, 1.0);
            p.circle_filled(to_screen(fx, y_of(0.0)), 2.5, pal::TEXT_STRONG());
            caption("FILTRO");
        }
    }
}

/// Campiona una forma su `cycles` periodi nella larghezza dell'anteprima.
fn sampled(shape: Shape1, cycles: f32, pw: f32, amp: f32, map: impl Fn(f32, f32) -> Pos2) -> Vec<Pos2> {
    // Le forme con salti (quadra, saw, S&H) hanno bisogno di più punti per gli spigoli.
    let n = POINTS * 2;
    (0..=n)
        .map(|i| {
            let x = i as f32 / n as f32;
            map(x, shape_value(shape, x * cycles, pw) * amp)
        })
        .collect()
}

/// Riempimento sotto una polilinea (strisce verticali: funziona anche per curve non convesse).
fn fill_under(p: &egui::Painter, pts: &[Pos2], base: f32, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    // Vertici condivisi tra strisce adiacenti: niente cuciture visibili.
    let mut mesh = egui::Mesh::default();
    for (i, q) in pts.iter().enumerate() {
        mesh.colored_vertex(*q, color);
        mesh.colored_vertex(pos2(q.x, base), color);
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k, k + 1);
            mesh.add_triangle(k - 2, k + 1, k - 1);
        }
    }
    p.add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nova_group(prefix: &str) -> Vec<&'static ParamDesc> {
        let d = mfc_plugins::descriptor(mfc_plugins::ids::NOVA).expect("Nova registrato");
        d.params.iter().filter(|p| p.name.starts_with(prefix)).collect()
    }

    #[test]
    fn detects_nova_groups() {
        assert!(matches!(detect(&nova_group("aenv_"))[..], [Visual::Adsr { .. }]));
        assert!(matches!(detect(&nova_group("fenv_"))[..], [Visual::Adsr { .. }]));
        assert!(matches!(
            detect(&nova_group("osc1_"))[..],
            [Visual::Wave {
                pw: Some("osc1_pw"),
                ..
            }]
        ));
        assert!(matches!(detect(&nova_group("lfo2_"))[..], [Visual::Lfo { .. }]));
        assert!(matches!(
            detect(&nova_group("flt_"))[..],
            [Visual::Filter {
                mode: Some("flt_mode"),
                slope: Some("flt_slope"),
                ..
            }]
        ));
        assert!(detect(&nova_group("voice_")).is_empty());
    }

    #[test]
    fn detects_generic_adsr_and_filter() {
        // Synth con parametri senza prefisso (es. 3x Osc, Supersaw).
        let ps = [
            ParamDesc::new("attack", "A", 0.0, 10.0, 0.0, "s"),
            ParamDesc::new("decay", "D", 0.0, 10.0, 0.0, "s"),
            ParamDesc::new("sustain", "S", 0.0, 1.0, 0.0, ""),
            ParamDesc::new("release", "R", 0.0, 10.0, 0.0, "s"),
            ParamDesc::new("cutoff", "C", 20.0, 20_000.0, 1000.0, "Hz"),
            ParamDesc::new("resonance", "Q", 0.0, 1.0, 0.0, ""),
        ];
        let refs: Vec<&ParamDesc> = ps.iter().collect();
        let v = detect(&refs);
        assert_eq!(v.len(), 2);
        assert!(matches!(v[1], Visual::Filter { mode: None, .. }));
        // Senza sustain niente ADSR.
        assert!(detect(&[&ps[0], &ps[1], &ps[3]]).is_empty());
    }

    #[test]
    fn adsr_curve_shape() {
        let pts = adsr_points(0.01, 0.3, 0.6, 0.5);
        assert_eq!(pts.first(), Some(&(0.0, 0.0)));
        let last = pts.last().unwrap();
        assert!((last.0 - 1.0).abs() < 1e-4 && last.1.abs() < 1e-4);
        assert!(pts.windows(2).all(|w| w[1].0 >= w[0].0), "x crescente");
        let peak = pts.iter().map(|p| p.1).fold(0.0, f32::max);
        assert!((peak - 1.0).abs() < 1e-4);
        // Il plateau è al livello di sustain.
        assert!(pts.iter().any(|p| (p.1 - 0.6).abs() < 1e-4));
    }

    #[test]
    fn shapes_and_filter() {
        assert_eq!(shape_from_label("Saw"), Shape1::Saw);
        assert_eq!(shape_from_label("S&H"), Shape1::SampleHold);
        assert_eq!(shape_from_label("Triangle"), Shape1::Triangle);
        assert_eq!(shape_value(Shape1::Square, 0.2, 0.25), 1.0);
        assert_eq!(shape_value(Shape1::Square, 0.3, 0.25), -1.0);
        assert!((shape_value(Shape1::Triangle, 0.25, 0.5) - 1.0).abs() < 1e-5);
        assert!((shape_value(Shape1::Triangle, 0.75, 0.5) + 1.0).abs() < 1e-5);
        for ph in [0.0f32, 0.13, 0.5, 0.99] {
            for s in [Shape1::Sine, Shape1::Noise, Shape1::SampleHold, Shape1::Saw] {
                assert!(shape_value(s, ph, 0.5).abs() <= 1.0 + 1e-5);
            }
        }
        // Passa basso: piatto sotto il taglio, attenuato sopra; 24 dB attenua di più.
        assert!((filter_gain(20.0, 5000.0, 0.0, 0, false) - 1.0).abs() < 0.01);
        let g12 = filter_gain(20_000.0, 1000.0, 0.0, 0, false);
        let g24 = filter_gain(20_000.0, 1000.0, 0.0, 0, true);
        assert!(g12 < 0.01 && g24 < g12);
        // Risonanza: picco al taglio.
        assert!(filter_gain(1000.0, 1000.0, 1.0, 0, false) > 4.0);
        // Passa alto speculare.
        assert!(filter_gain(20.0, 5000.0, 0.0, 1, false) < 0.01);
        assert_eq!(filter_mode_from_label("high pass"), 1);
        assert_eq!(filter_mode_from_label("notch"), 3);
    }
}
