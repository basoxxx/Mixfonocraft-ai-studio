//! Round-trip di plugin esterni (VST3/CLAP nel "Fruity Wrapper") e clip di automazione
//! (canali di tipo 5 con `ChanAutomation` + `RemoteController`).

use mfc_core::{
    AutomationClip, AutomationId, AutomationPoint, AutomationTarget, Channel, ChannelId, ChannelKind, ClipId,
    ClipSource, EffectSlot, PlaylistClip, PluginInstance, Project,
};
use mfc_flp::structs::{vst_record, FlWrapperData, WRAPPER_DATA_TYPE};
use mfc_flp::{from_project, from_project_report, id, parse, to_project, write, RawEvent, TextEncoding};

const VST3_KIND: &str = "vst3:/plugins/Big Synth.vst3::00112233445566778899AABBCCDDEEFF";
const CLAP_KIND: &str = "clap:/usr/lib/clap/Verb.clap::com.vendor.verb";

fn pt(position: u64, value: f32, tension: f32) -> AutomationPoint {
    AutomationPoint {
        position,
        value,
        tension,
    }
}

struct Fixture {
    project: Project,
    synth: ChannelId,
}

fn build() -> Fixture {
    let mut p = Project::new("Esterni e automazioni");
    p.tempo = 120.0;

    let kick = ChannelId(p.alloc_id());
    p.channels.push(Channel::new(
        kick,
        "Kick",
        ChannelKind::Sampler,
        PluginInstance::new("mfc.sampler"),
    ));
    let synth = ChannelId(p.alloc_id());
    let mut vst = PluginInstance::new(VST3_KIND);
    vst.foreign_name = Some("Big Synth".into());
    vst.opaque_state = Some((0..1500u32).map(|i| (i * 7 % 251) as u8).collect());
    vst.instance_id = Some("inst-1".into());
    let mut ch = Channel::new(synth, "Lead", ChannelKind::Generator, vst);
    ch.mixer_insert = 1;
    p.channels.push(ch);

    let mut clap = PluginInstance::new(CLAP_KIND);
    clap.opaque_state = Some(vec![0, 255, 1, 254, 2, 253]);
    let mut slot = EffectSlot::new(clap);
    slot.mix = 0.5;
    p.mixer.inserts[1].effects.push(slot);

    let autos = [
        (
            "Vol Lead",
            AutomationTarget::ChannelVolume { channel: synth },
            0.0,
            1.0,
            vec![pt(0, 0.2, 0.0), pt(96, 0.8, 0.5), pt(384, 0.5, -0.3)],
        ),
        (
            "Pan Lead",
            AutomationTarget::ChannelPan { channel: synth },
            -1.0,
            1.0,
            vec![pt(0, 0.0, 0.0), pt(192, 1.0, 0.0)],
        ),
        (
            "Pitch Lead",
            AutomationTarget::ChannelPitch { channel: synth },
            -1200.0,
            1200.0,
            vec![pt(0, 0.5, 0.0), pt(48, 0.75, 0.25)],
        ),
        (
            "Pan insert 1",
            AutomationTarget::MixerPan { insert: 1 },
            -1.0,
            1.0,
            vec![pt(0, 0.25, 0.0), pt(200, 0.75, 1.0)],
        ),
        (
            "Vol insert 1",
            AutomationTarget::MixerVolume { insert: 1 },
            0.0,
            1.0,
            vec![pt(10, 0.9, 0.0), pt(20, 0.1, 0.0)],
        ),
        (
            "Master",
            AutomationTarget::MasterVolume,
            0.0,
            1.0,
            vec![pt(0, 1.0, 0.0), pt(768, 0.0, 0.0)],
        ),
        (
            "Tempo",
            AutomationTarget::Tempo,
            60.0,
            180.0,
            vec![pt(0, 0.5, 0.0), pt(96, 0.25, -1.0), pt(960, 1.0, 0.0)],
        ),
        (
            "Cutoff",
            AutomationTarget::EffectParam {
                insert: 1,
                slot: 0,
                param: "cutoff".into(),
            },
            0.0,
            1.0,
            vec![pt(0, 0.0, 0.0)],
        ),
    ];
    for (name, target, min, max, points) in autos {
        let aid = AutomationId(p.alloc_id());
        p.automations.push(AutomationClip {
            id: aid,
            name: name.into(),
            target,
            min,
            max,
            points,
        });
    }
    let clips = [
        (0usize, 2u16, 0u64, 384u64, 0u64),
        (6, 3, 768, 384, 96),
        (7, 4, 0, 96, 0),
    ];
    for (ai, track, position, length, offset) in clips {
        let source = ClipSource::Automation(p.automations[ai].id);
        let id = ClipId(p.alloc_id());
        p.playlist.clips.push(PlaylistClip {
            id,
            track,
            position,
            length,
            offset,
            source,
            muted: false,
        });
    }
    Fixture { project: p, synth }
}

#[test]
fn external_plugins_and_automation_roundtrip() {
    let fx = build();
    let src = &fx.project;
    let rep = from_project_report(src);
    assert!(
        rep.warnings
            .iter()
            .any(|w| w.contains("Cutoff") && w.contains("non esportata")),
        "{:?}",
        rep.warnings
    );
    assert_eq!(rep.warnings.len(), 1, "{:?}", rep.warnings);
    let flp = rep.file;
    // 2 canali + 7 canali di automazione.
    assert_eq!(flp.header.n_channels, 9);

    // Livello eventi: round-trip byte-identico.
    let bytes = write(&flp);
    let parsed = parse(&bytes).unwrap();
    assert_eq!(parsed, flp);
    assert_eq!(write(&parsed), bytes);

    // Il canale VST3 è un "Fruity Wrapper" di tipo 4 con i record del wrapper.
    let ev = &parsed.events;
    let wrapper_names = ev
        .iter()
        .filter(|e| e.id == id::PLUGIN_INTERNAL_NAME)
        .filter(|e| mfc_flp::decode_text(e.as_data().unwrap(), TextEncoding::Utf16) == "Fruity Wrapper")
        .count();
    assert_eq!(wrapper_names, 2, "canale + slot del mixer");
    let datas: Vec<FlWrapperData> = ev
        .iter()
        .filter(|e| e.id == id::PLUGIN_DATA)
        .filter_map(|e| FlWrapperData::parse(e.as_data().unwrap()))
        .collect();
    assert_eq!(datas.len(), 2);
    let w = &datas[0];
    assert_eq!(w.kind, WRAPPER_DATA_TYPE);
    assert_eq!(w.text(vst_record::NAME).as_deref(), Some("Big Synth"));
    assert_eq!(
        w.text(vst_record::PLUGIN_PATH).as_deref(),
        Some("/plugins/Big Synth.vst3")
    );
    assert_eq!(w.record(vst_record::GUID).map(<[u8]>::len), Some(16));
    assert_eq!(
        w.record(vst_record::STATE),
        src.channels[1].plugin.opaque_state.as_deref()
    );
    assert_eq!(datas[1].text(vst_record::NAME).as_deref(), Some("Verb"));
    assert_eq!(ev.iter().filter(|e| e.id == id::CHAN_AUTOMATION).count(), 7);
    assert_eq!(ev.iter().filter(|e| e.id == id::REMOTE_CONTROLLER).count(), 7);

    // Import.
    let res = to_project(&parsed).unwrap();
    let p = &res.project;
    assert!(
        res.warnings
            .iter()
            .all(|w| !w.contains("non mappati") && !w.contains("automazion")),
        "{:?}",
        res.warnings
    );
    assert_eq!(p.channels.len(), 2, "i canali di automazione diventano clip");
    let lead = &p.channels[1];
    assert_eq!((lead.name.as_str(), lead.kind), ("Lead", ChannelKind::Generator));
    assert_eq!(lead.plugin.kind, VST3_KIND);
    assert!(lead.plugin.is_external());
    assert_eq!(lead.plugin.opaque_state, src.channels[1].plugin.opaque_state);
    assert_eq!(lead.plugin.foreign_name.as_deref(), Some("Big Synth"));
    assert_eq!(lead.mixer_insert, 1);
    let slot = &p.mixer.inserts[1].effects[0];
    assert_eq!(slot.plugin.kind, CLAP_KIND);
    assert_eq!(
        slot.plugin.opaque_state.as_deref(),
        Some(&[0u8, 255, 1, 254, 2, 253][..])
    );
    assert!(slot.enabled);
    assert!((slot.mix - 0.5).abs() < 1e-3);

    // Automazioni: bersagli, posizioni e valori reali entro tolleranza.
    assert_eq!(p.automations.len(), 7);
    let map_c = |c: ChannelId| if c == fx.synth { lead.id } else { panic!("canale {c:?}") };
    for a in src.automations.iter().take(7) {
        let b = p.automations.iter().find(|b| b.name == a.name).expect(&a.name);
        let want_target = match &a.target {
            AutomationTarget::ChannelVolume { channel } => AutomationTarget::ChannelVolume {
                channel: map_c(*channel),
            },
            AutomationTarget::ChannelPan { channel } => AutomationTarget::ChannelPan {
                channel: map_c(*channel),
            },
            AutomationTarget::ChannelPitch { channel } => AutomationTarget::ChannelPitch {
                channel: map_c(*channel),
            },
            t => t.clone(),
        };
        assert_eq!(b.target, want_target, "{}", a.name);
        assert_eq!(a.points.len(), b.points.len());
        let tol = (a.max - a.min).abs() * 1e-4;
        for (x, y) in a.points.iter().zip(&b.points) {
            assert_eq!(x.position, y.position, "{}", a.name);
            let rx = a.min + (a.max - a.min) * x.value;
            let ry = b.min + (b.max - b.min) * y.value;
            assert!((rx - ry).abs() <= tol, "{}: {rx} vs {ry}", a.name);
            assert!((x.tension - y.tension).abs() < 1e-6);
            let pos = x.position as f64;
            let (va, vb) = (a.real_value_at(pos).unwrap(), b.real_value_at(pos).unwrap());
            assert!((va - vb).abs() <= tol);
        }
    }

    // Playlist: le clip di automazione esportabili restano al loro posto.
    let mut got: Vec<_> = p
        .playlist
        .clips
        .iter()
        .map(|c| {
            let ClipSource::Automation(aid) = c.source else {
                panic!("sorgente inattesa {:?}", c.source)
            };
            let name = p.automation(aid).unwrap().name.clone();
            (name, c.track, c.position, c.length, c.offset, c.muted)
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("Tempo".to_string(), 3, 768, 384, 96, false),
            ("Vol Lead".to_string(), 2, 0, 384, 0, false),
        ]
    );

    // Seconda passata stabile.
    let again = to_project(&parse(&write(&from_project(p))).unwrap()).unwrap().project;
    assert_eq!(again.channels[1].plugin.kind, VST3_KIND);
    assert_eq!(again.channels[1].plugin.opaque_state, lead.plugin.opaque_state);
    assert_eq!(again.automations.len(), 7);
    assert_eq!(again.playlist.clips.len(), 2);
    for (a, b) in p.automations.iter().zip(&again.automations) {
        assert_eq!(a.name, b.name);
        assert_eq!((a.min, a.max), (b.min, b.max));
        for (x, y) in a.points.iter().zip(&b.points) {
            assert_eq!(x.position, y.position);
            assert!((x.value - y.value).abs() < 1e-6);
        }
    }
}

/// Wrapper VST2 (.dll) o con dati non strutturati: comportamento "foreign" invariato.
#[test]
fn vst2_wrapper_stays_foreign() {
    let mut p = Project::new("VST2");
    let cid = ChannelId(p.alloc_id());
    let mut pi = PluginInstance::new("mfc.silent");
    pi.foreign_name = Some("Fruity Wrapper".into());
    let blob = FlWrapperData {
        kind: 8,
        records: vec![
            (vst_record::NAME, b"Old Synth".to_vec()),
            (vst_record::PLUGIN_PATH, b"C:\\VST\\Old Synth.dll".to_vec()),
            (vst_record::FOURCC, b"OlDs".to_vec()),
            (vst_record::STATE, vec![1, 2, 3]),
        ],
    }
    .write();
    pi.opaque_state = Some(blob.clone());
    p.channels.push(Channel::new(cid, "Old", ChannelKind::Generator, pi));
    let res = to_project(&parse(&write(&from_project(&p))).unwrap()).unwrap();
    let ch = &res.project.channels[0];
    assert_eq!(ch.plugin.kind, "mfc.silent");
    assert_eq!(ch.plugin.opaque_state.as_deref(), Some(blob.as_slice()));
}

/// Kind esterno malformato: nessun panic, avviso e canale vuoto.
#[test]
fn invalid_external_kind_warns() {
    let mut p = Project::new("Bad");
    let cid = ChannelId(p.alloc_id());
    p.channels.push(Channel::new(
        cid,
        "Bad",
        ChannelKind::Generator,
        PluginInstance::new("vst3:nopath"),
    ));
    let rep = from_project_report(&p);
    assert!(rep.warnings.iter().any(|w| w.contains("vst3:nopath")));
    let res = to_project(&rep.file).unwrap();
    assert_eq!(res.project.channels[0].kind, ChannelKind::Sampler);
}

/// Canale di automazione senza collegamento riconosciuto (es. file FL reale):
/// resta un canale `Automation` con avviso, come prima.
#[test]
fn automation_without_known_target_keeps_legacy_behaviour() {
    let enc = TextEncoding::Utf16;
    let f = mfc_flp::FlpFile {
        header: mfc_flp::FlpHeader {
            format: 0,
            n_channels: 1,
            ppq: 96,
        },
        events: vec![
            RawEvent::text(id::FL_VERSION, "20.8.4", TextEncoding::Ascii),
            RawEvent::num(id::CHAN_NEW, 0),
            RawEvent::num(id::CHAN_TYPE, 5),
            RawEvent::text(id::PLUGIN_NAME, "Automation Clip", enc),
            RawEvent::data(
                id::CHAN_AUTOMATION,
                mfc_flp::structs::FlAutomation { points: Vec::new() }.write(),
            ),
        ],
        trailing: Vec::new(),
    };
    let res = to_project(&f).unwrap();
    assert_eq!(res.project.channels.len(), 1);
    assert_eq!(res.project.channels[0].kind, ChannelKind::Automation);
    assert!(res.project.automations.is_empty());
    assert!(res.warnings.iter().any(|w| w.contains("Automation Clip")));
}
