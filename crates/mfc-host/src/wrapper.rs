//! Adattatori tra [`ExternalInstance`] e i trait `Generator` / `Effect` del motore.
//!
//! Nessuna allocazione nel thread audio: buffer ed elenco eventi sono preallocati.

use std::sync::Arc;

use mfc_plugins::{Effect, Generator, NoteOn, PluginDescriptor, PluginRole};

use crate::{parse_param_key, ExternalInstance, MidiEvent, DEFAULT_MAX_BLOCK};

/// Descrittore generico dei generatori esterni (i parametri reali sono dinamici).
pub static EXTERNAL_GENERATOR: PluginDescriptor = PluginDescriptor {
    id: "external.generator",
    name: "Plugin esterno",
    role: PluginRole::Generator,
    description: "Strumento VST3/CLAP ospitato",
    params: &[],
};

/// Descrittore generico degli effetti esterni.
pub static EXTERNAL_EFFECT: PluginDescriptor = PluginDescriptor {
    id: "external.effect",
    name: "Effetto esterno",
    role: PluginRole::Effect,
    description: "Effetto VST3/CLAP ospitato",
    params: &[],
};

const MAX_EVENTS: usize = 1024;
const MAX_HELD: usize = 512;

/// Strumento esterno come `Generator`.
pub struct ExternalGenerator {
    inst: Arc<dyn ExternalInstance>,
    events: Vec<MidiEvent>,
    /// Voci in corso: (voice_id del motore, tasto, note_id).
    held: Vec<(u64, u8, i32)>,
    next_note_id: i32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
}

impl ExternalGenerator {
    pub fn new(inst: Arc<dyn ExternalInstance>) -> Self {
        Self {
            inst,
            events: Vec::with_capacity(MAX_EVENTS),
            held: Vec::with_capacity(MAX_HELD),
            next_note_id: 0,
            buf_l: vec![0.0; DEFAULT_MAX_BLOCK],
            buf_r: vec![0.0; DEFAULT_MAX_BLOCK],
        }
    }

    pub fn instance(&self) -> &Arc<dyn ExternalInstance> {
        &self.inst
    }

    fn push(&mut self, ev: MidiEvent) {
        if self.events.len() < MAX_EVENTS {
            self.events.push(ev);
        }
    }
}

impl Generator for ExternalGenerator {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &EXTERNAL_GENERATOR
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {
        // Il sample rate è impostato dall'host con `prepare` alla creazione.
    }

    fn set_param(&mut self, name: &str, value: f32) {
        if let Some(id) = parse_param_key(name) {
            self.inst.set_param_normalized(id, value.clamp(0.0, 1.0) as f64);
        }
    }

    fn note_on(&mut self, note: NoteOn) {
        let key = (note.key as i32 + (note.fine_pitch_cents / 100.0).round() as i32).clamp(0, 127) as u8;
        let note_id = self.next_note_id;
        self.next_note_id = self.next_note_id.wrapping_add(1) & 0x7fff_ffff;
        if self.held.len() < MAX_HELD {
            self.held.push((note.voice_id, key, note_id));
        }
        self.push(MidiEvent::NoteOn {
            offset: 0,
            key,
            velocity: note.velocity.clamp(0.0, 1.0),
            note_id,
        });
    }

    fn note_off(&mut self, voice_id: u64) {
        if let Some(i) = self.held.iter().position(|h| h.0 == voice_id) {
            let (_, key, note_id) = self.held.swap_remove(i);
            self.push(MidiEvent::NoteOff {
                offset: 0,
                key,
                velocity: 0.0,
                note_id,
            });
        }
    }

    fn all_notes_off(&mut self) {
        while let Some((_, key, note_id)) = self.held.pop() {
            self.push(MidiEvent::NoteOff {
                offset: 0,
                key,
                velocity: 0.0,
                note_id,
            });
        }
        self.push(MidiEvent::AllNotesOff { offset: 0 });
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut done = 0;
        while done < n {
            let len = (n - done).min(self.buf_l.len());
            let (bl, br) = (&mut self.buf_l[..len], &mut self.buf_r[..len]);
            bl.fill(0.0);
            br.fill(0.0);
            // Gli eventi vanno solo nel primo sotto-blocco.
            let events: &[MidiEvent] = if done == 0 { &self.events } else { &[] };
            if self.inst.process(events, None, bl, br) {
                for i in 0..len {
                    left[done + i] += sanitize(bl[i]);
                    right[done + i] += sanitize(br[i]);
                }
            }
            done += len;
        }
        self.events.clear();
    }

    fn active_voices(&self) -> usize {
        // Le code (release, riverberi interni) non sono note: elaboriamo sempre.
        1
    }
}

/// Effetto esterno come `Effect`.
pub struct ExternalEffect {
    inst: Arc<dyn ExternalInstance>,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
}

impl ExternalEffect {
    pub fn new(inst: Arc<dyn ExternalInstance>) -> Self {
        Self {
            inst,
            in_l: vec![0.0; DEFAULT_MAX_BLOCK],
            in_r: vec![0.0; DEFAULT_MAX_BLOCK],
        }
    }

    pub fn instance(&self) -> &Arc<dyn ExternalInstance> {
        &self.inst
    }
}

impl Effect for ExternalEffect {
    fn descriptor(&self) -> &'static PluginDescriptor {
        &EXTERNAL_EFFECT
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn set_param(&mut self, name: &str, value: f32) {
        if let Some(id) = parse_param_key(name) {
            self.inst.set_param_normalized(id, value.clamp(0.0, 1.0) as f64);
        }
    }

    fn reset(&mut self) {
        self.inst.reset();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut done = 0;
        while done < n {
            let len = (n - done).min(self.in_l.len());
            self.in_l[..len].copy_from_slice(&left[done..done + len]);
            self.in_r[..len].copy_from_slice(&right[done..done + len]);
            let ok = self.inst.process(
                &[],
                Some((&self.in_l[..len], &self.in_r[..len])),
                &mut left[done..done + len],
                &mut right[done..done + len],
            );
            if ok {
                for x in left[done..done + len]
                    .iter_mut()
                    .chain(right[done..done + len].iter_mut())
                {
                    *x = sanitize(*x);
                }
            } else {
                // Plugin occupato: passa il segnale dry.
                left[done..done + len].copy_from_slice(&self.in_l[..len]);
                right[done..done + len].copy_from_slice(&self.in_r[..len]);
            }
            done += len;
        }
    }

    fn latency(&self) -> usize {
        self.inst.latency() as usize
    }
}

fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-8.0, 8.0)
    } else {
        0.0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::*;
    use parking_lot::Mutex;

    /// Istanza finta: sinusoide quando c'è una nota, guadagno = parametro 0.
    pub struct FakeInstance {
        pub info: PluginInfo,
        pub state: Mutex<(bool, f64, f32)>, // (nota attiva, gain, fase)
        pub busy: Mutex<()>,
    }

    impl FakeInstance {
        pub fn new() -> Self {
            Self {
                info: PluginInfo {
                    kind: "vst3:/fake.vst3::00".into(),
                    format: PluginFormat::Vst3,
                    path: "/fake.vst3".into(),
                    id: "00".into(),
                    name: "Fake".into(),
                    vendor: "Test".into(),
                    version: "1".into(),
                    category: "Instrument".into(),
                    is_instrument: true,
                    is_effect: false,
                },
                state: Mutex::new((false, 1.0, 0.0)),
                busy: Mutex::new(()),
            }
        }
    }

    impl ExternalInstance for FakeInstance {
        fn info(&self) -> &PluginInfo {
            &self.info
        }
        fn prepare(&self, _: f32, _: usize) -> Result<(), HostError> {
            Ok(())
        }
        fn process(
            &self,
            events: &[MidiEvent],
            input: Option<(&[f32], &[f32])>,
            out_l: &mut [f32],
            out_r: &mut [f32],
        ) -> bool {
            let Some(_g) = self.busy.try_lock() else { return false };
            let mut s = self.state.lock();
            for e in events {
                match e {
                    MidiEvent::NoteOn { .. } => s.0 = true,
                    MidiEvent::NoteOff { .. } | MidiEvent::AllNotesOff { .. } => s.0 = false,
                }
            }
            for i in 0..out_l.len() {
                let v = match input {
                    Some((l, _)) => l[i] * s.1 as f32,
                    None if s.0 => {
                        s.2 += 0.05;
                        s.2.sin() * s.1 as f32
                    }
                    None => 0.0,
                };
                out_l[i] = v;
                out_r[i] = input.map_or(v, |(_, r)| r[i] * s.1 as f32);
            }
            true
        }
        fn set_param_normalized(&self, _id: u32, value: f64) {
            self.state.lock().1 = value;
        }
        fn reset(&self) {
            self.state.lock().0 = false;
        }
        fn params(&self) -> Vec<ExternalParam> {
            vec![ExternalParam {
                id: 0,
                name: "Gain".into(),
                unit: "".into(),
                default: 1.0,
                steps: 0,
                automatable: true,
                hidden: false,
            }]
        }
        fn param_normalized(&self, _: u32) -> f64 {
            self.state.lock().1
        }
        fn param_display(&self, _: u32, v: f64) -> String {
            format!("{v:.2}")
        }
        fn save_state(&self) -> Result<Vec<u8>, HostError> {
            Ok(self.state.lock().1.to_le_bytes().to_vec())
        }
        fn load_state(&self, data: &[u8]) -> Result<(), HostError> {
            let b: [u8; 8] = data.try_into().map_err(|_| HostError::Plugin("stato".into()))?;
            self.state.lock().1 = f64::from_le_bytes(b);
            Ok(())
        }
        fn has_editor(&self) -> bool {
            false
        }
        fn open_editor(&self, _: ParentWindow) -> Result<EditorSize, HostError> {
            Err(HostError::Unsupported("editor".into()))
        }
        fn close_editor(&self) {}
    }

    #[test]
    fn generator_routes_notes_and_params() {
        let inst = Arc::new(FakeInstance::new());
        let mut g = ExternalGenerator::new(inst.clone());
        let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
        g.process(&mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.0));
        g.note_on(NoteOn::simple(7, 60, 1.0));
        g.process(&mut l, &mut r);
        assert!(l.iter().any(|x| x.abs() > 0.1));
        // Parametro: guadagno a zero → silenzio.
        g.set_param("p0", 0.0);
        let (mut l2, mut r2) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        g.process(&mut l2, &mut r2);
        assert!(l2.iter().all(|x| *x == 0.0));
        g.note_off(7);
        g.set_param("p0", 1.0);
        let (mut l3, mut r3) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        g.process(&mut l3, &mut r3);
        assert!(l3.iter().all(|x| *x == 0.0), "note off non arrivato");
    }

    #[test]
    fn effect_passes_dry_when_busy() {
        let inst = Arc::new(FakeInstance::new());
        let mut fx = ExternalEffect::new(inst.clone());
        fx.set_param("p0", 0.5);
        let (mut l, mut r) = (vec![1.0f32; 32], vec![1.0f32; 32]);
        fx.process(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-6);
        let _guard = inst.busy.lock();
        let (mut l, mut r) = (vec![1.0f32; 32], vec![1.0f32; 32]);
        fx.process(&mut l, &mut r);
        assert_eq!(l[0], 1.0);
    }
}
