//! Inviluppi: ADSR (con hold opzionale) e decadimento esponenziale.

/// Livello sotto il quale un inviluppo è considerato concluso (-80 dB).
pub const ENV_END: f32 = 1e-4;

/// Coefficiente esponenziale che porta da 1 a [`ENV_END`] in `seconds`.
#[inline]
fn exp_coef(seconds: f32, sample_rate: f32) -> f32 {
    let n = seconds * sample_rate;
    if n < 1.0 || !n.is_finite() {
        0.0
    } else {
        (ENV_END.ln() / n).exp()
    }
}

#[inline]
fn sane_time(t: f32) -> f32 {
    if t.is_finite() {
        t.clamp(0.0, 600.0)
    } else {
        0.0
    }
}

/// Fase corrente dell'[`Adsr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

/// ADSR (+ hold): attacco lineare, decay e release esponenziali; tempi in secondi.
///
/// Il retrigger riparte dal livello corrente (niente click) e il release parte dal livello attuale.
/// Decay e release durano esattamente il tempo impostato (fino a -80 dB rispetto all'escursione).
#[derive(Debug, Clone, Copy)]
pub struct Adsr {
    stage: Stage,
    level: f32,
    sample_rate: f32,
    attack: f32,
    hold: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    attack_inc: f32,
    hold_samples: u32,
    hold_count: u32,
    decay_coef: f32,
    release_coef: f32,
}

impl Adsr {
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self {
            stage: Stage::Idle,
            level: 0.0,
            sample_rate: sample_rate.max(1.0),
            attack: 0.005,
            hold: 0.0,
            decay: 0.2,
            sustain: 0.7,
            release: 0.2,
            attack_inc: 0.0,
            hold_samples: 0,
            hold_count: 0,
            decay_coef: 0.0,
            release_coef: 0.0,
        };
        e.update();
        e
    }

    fn update(&mut self) {
        let sr = self.sample_rate;
        let a = self.attack * sr;
        self.attack_inc = if a < 1.0 { f32::INFINITY } else { 1.0 / a };
        self.hold_samples = (self.hold * sr) as u32;
        self.decay_coef = exp_coef(self.decay, sr);
        self.release_coef = exp_coef(self.release, sr);
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.update();
    }

    pub fn set_attack(&mut self, seconds: f32) {
        self.attack = sane_time(seconds);
        self.update();
    }

    pub fn set_hold(&mut self, seconds: f32) {
        self.hold = sane_time(seconds);
        self.update();
    }

    pub fn set_decay(&mut self, seconds: f32) {
        self.decay = sane_time(seconds);
        self.update();
    }

    /// Livello di sustain 0..=1.
    pub fn set_sustain(&mut self, level: f32) {
        self.sustain = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
    }

    pub fn set_release(&mut self, seconds: f32) {
        self.release = sane_time(seconds);
        self.update();
    }

    /// Imposta tutti i tempi (s) e il sustain in un colpo.
    pub fn set_adsr(&mut self, attack: f32, decay: f32, sustain: f32, release: f32) {
        self.attack = sane_time(attack);
        self.decay = sane_time(decay);
        self.release = sane_time(release);
        self.set_sustain(sustain);
        self.update();
    }

    /// Avvia (o riavvia dal livello corrente) l'inviluppo.
    pub fn note_on(&mut self) {
        self.hold_count = 0;
        self.stage = Stage::Attack;
    }

    /// Passa al release partendo dal livello corrente.
    pub fn note_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    /// Torna a riposo immediatamente.
    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// `true` in release o a riposo.
    pub fn is_released(&self) -> bool {
        matches!(self.stage, Stage::Release | Stage::Idle)
    }

    /// Avanza di un campione e restituisce il livello 0..=1.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => {
                self.level = 0.0;
            }
            Stage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = if self.hold_samples > 0 {
                        Stage::Hold
                    } else {
                        Stage::Decay
                    };
                }
            }
            Stage::Hold => {
                self.hold_count += 1;
                if self.hold_count >= self.hold_samples {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                let s = self.sustain;
                self.level = s + (self.level - s) * self.decay_coef;
                if self.level - s <= ENV_END * (1.0 - s) {
                    self.level = s;
                    self.stage = if s <= 0.0 { Stage::Idle } else { Stage::Sustain };
                }
            }
            Stage::Sustain => {
                self.level = self.sustain;
                if self.level <= 0.0 {
                    self.stage = Stage::Idle;
                }
            }
            Stage::Release => {
                self.level *= self.release_coef;
                if self.level <= ENV_END {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

impl Default for Adsr {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

/// Decadimento esponenziale one-shot (percussioni): da `amount` a -80 dB in `time` secondi.
#[derive(Debug, Clone, Copy)]
pub struct ExpDecay {
    level: f32,
    coef: f32,
    time: f32,
    sample_rate: f32,
}

impl ExpDecay {
    pub fn new(time: f32, sample_rate: f32) -> Self {
        let mut d = Self {
            level: 0.0,
            coef: 0.0,
            time: sane_time(time),
            sample_rate: sample_rate.max(1.0),
        };
        d.coef = exp_coef(d.time, d.sample_rate);
        d
    }

    pub fn set_time(&mut self, time: f32) {
        self.time = sane_time(time);
        self.coef = exp_coef(self.time, self.sample_rate);
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.coef = exp_coef(self.time, self.sample_rate);
    }

    pub fn trigger(&mut self, amount: f32) {
        self.level = if amount.is_finite() { amount.max(0.0) } else { 0.0 };
    }

    pub fn reset(&mut self) {
        self.level = 0.0;
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_active(&self) -> bool {
        self.level > ENV_END
    }

    /// Restituisce il livello corrente e avanza.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let l = self.level;
        if l > ENV_END {
            self.level *= self.coef;
        } else {
            self.level = 0.0;
        }
        l
    }
}

impl Default for ExpDecay {
    fn default() -> Self {
        Self::new(0.5, 48_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn run_until(env: &mut Adsr, stage: Stage, max: usize) -> usize {
        for i in 0..max {
            if env.stage() == stage {
                return i;
            }
            env.next_sample();
        }
        max
    }

    #[test]
    fn stage_timing() {
        let mut e = Adsr::new(SR);
        e.set_adsr(0.01, 0.1, 0.5, 0.2);
        e.note_on();
        let a = run_until(&mut e, Stage::Decay, 100_000);
        assert!((a as f32 - 0.01 * SR).abs() <= 2.0, "attack {a}");
        let d = run_until(&mut e, Stage::Sustain, 100_000);
        assert!((d as f32 - 0.1 * SR).abs() / (0.1 * SR) < 0.02, "decay {d}");
        for _ in 0..1000 {
            assert_eq!(e.next_sample(), 0.5);
        }
        e.note_off();
        let r = run_until(&mut e, Stage::Idle, 1_000_000);
        // Il release parte da 0.5: termina un po' prima del tempo nominale.
        assert!(r as f32 <= 0.2 * SR + 2.0 && r as f32 > 0.15 * SR, "release {r}");
        assert!(e.is_idle());
        assert_eq!(e.next_sample(), 0.0);
    }

    #[test]
    fn hold_and_zero_times() {
        let mut e = Adsr::new(SR);
        e.set_adsr(0.0, 0.0, 1.0, 0.0);
        e.set_hold(0.01);
        e.note_on();
        assert_eq!(e.next_sample(), 1.0);
        assert_eq!(e.stage(), Stage::Hold);
        e.note_off();
        e.next_sample();
        assert!(e.is_idle());
    }

    #[test]
    fn retrigger_from_current_level() {
        let mut e = Adsr::new(SR);
        e.set_adsr(0.1, 0.1, 0.8, 0.5);
        e.note_on();
        for _ in 0..24_000 {
            e.next_sample();
        }
        e.note_off();
        for _ in 0..2_400 {
            e.next_sample();
        }
        let before = e.level();
        e.note_on();
        let after = e.next_sample();
        assert!(after > before && after - before < 0.01);
    }

    #[test]
    fn zero_sustain_goes_idle() {
        let mut e = Adsr::new(SR);
        e.set_adsr(0.0, 0.05, 0.0, 0.1);
        e.note_on();
        let n = run_until(&mut e, Stage::Idle, 100_000);
        assert!((n as f32 - 0.05 * SR).abs() < 50.0);
    }

    #[test]
    fn exp_decay_time() {
        let mut d = ExpDecay::new(0.1, SR);
        d.trigger(1.0);
        let mut n = 0;
        while d.is_active() {
            d.next_sample();
            n += 1;
        }
        assert!((n as f32 - 0.1 * SR).abs() <= 2.0, "{n}");
    }
}
