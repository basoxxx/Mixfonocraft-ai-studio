//! Coerenza di registro e descrittori.

use mfc_plugins::{create_effect, create_generator, default_instance, descriptor, ids, registry, PluginRole};
use std::collections::HashSet;

const ALL_IDS: &[&str] = &[
    ids::SAMPLER,
    ids::AUDIO_CLIP,
    ids::OSC3,
    ids::BOOBASS,
    ids::KICK,
    ids::DRUMSYNTH,
    ids::SUPERSAW,
    ids::SILENT,
    ids::FM,
    ids::PLUCK,
    ids::WAVETABLE,
    ids::EQ,
    ids::COMPRESSOR,
    ids::LIMITER,
    ids::GATE,
    ids::REVERB,
    ids::DELAY,
    ids::CHORUS,
    ids::FLANGER,
    ids::PHASER,
    ids::DISTORTION,
    ids::BITCRUSHER,
    ids::FILTER,
    ids::STEREO,
    ids::GAIN,
    ids::DUCKER,
    ids::AUTOPAN,
    ids::TRANSIENT,
];

#[test]
fn registry_contains_every_id_once() {
    let reg: Vec<&str> = registry().iter().map(|d| d.id).collect();
    let unique: HashSet<&str> = reg.iter().copied().collect();
    assert_eq!(unique.len(), reg.len(), "id duplicati");
    for id in ALL_IDS {
        assert!(unique.contains(id), "manca {id}");
        assert_eq!(descriptor(id).map(|d| d.id), Some(*id));
    }
    assert_eq!(reg.len(), ALL_IDS.len());
}

#[test]
fn params_are_consistent() {
    for d in registry() {
        let mut names = HashSet::new();
        for p in d.params {
            assert!(names.insert(p.name), "{}: parametro duplicato {}", d.id, p.name);
            assert!(p.min <= p.max, "{}.{}", d.id, p.name);
            assert!(
                p.default >= p.min && p.default <= p.max,
                "{}.{} default fuori range",
                d.id,
                p.name
            );
            assert!(!p.label.is_empty());
            assert!(
                p.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{}",
                p.name
            );
            if !p.choices.is_empty() {
                assert_eq!(p.max as usize, p.choices.len() - 1);
                assert_eq!(p.default.fract(), 0.0);
            }
        }
        let inst = default_instance(d.id).expect("default_instance");
        assert_eq!(inst.kind, d.id);
        assert_eq!(inst.params.len(), d.params.len());
    }
}

#[test]
fn every_plugin_can_be_instantiated_and_survives_odd_params() {
    for d in registry() {
        let inst = default_instance(d.id).unwrap();
        match d.role {
            PluginRole::Generator => {
                let mut g = create_generator(&inst, 44_100.0);
                assert_eq!(g.descriptor().id, d.id);
                for p in d.params {
                    for v in [f32::NAN, f32::INFINITY, -1e9, 1e9, p.min, p.max] {
                        g.set_param(p.name, v);
                    }
                }
                g.set_param("does_not_exist", 1.0);
                g.set_sample_rate(f32::NAN);
                g.set_sample_rate(0.0);
                let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
                g.process(&mut l, &mut r);
                assert!(l.iter().chain(&r).all(|x| x.is_finite()));
            }
            PluginRole::Effect => {
                let mut fx = create_effect(&inst, 44_100.0).expect("effetto");
                assert_eq!(fx.descriptor().id, d.id);
                for p in d.params {
                    for v in [f32::NAN, f32::NEG_INFINITY, -1e9, 1e9, p.min, p.max] {
                        fx.set_param(p.name, v);
                    }
                }
                fx.set_sample_rate(-5.0);
                let (mut l, mut r) = (vec![0.5; 64], vec![-0.5; 64]);
                fx.process(&mut l, &mut r);
                assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{}", d.id);
            }
        }
    }
}

#[test]
fn unknown_plugins() {
    let inst = mfc_core::PluginInstance::new("vst.something");
    let g = create_generator(&inst, 48_000.0);
    assert_eq!(g.descriptor().id, ids::SILENT);
    assert!(create_effect(&inst, 48_000.0).is_none());
    assert!(descriptor("nope").is_none());
    assert!(default_instance("nope").is_none());
}
