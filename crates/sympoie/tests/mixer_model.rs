use std::path::PathBuf;

use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::clip::AudioClip;
use sympoie::plugin::{
    PLUGIN_TAIL_FRAMES, PluginDescriptor, PluginInstance, PluginKind, PluginParameter,
    SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::track::{AudioOutput, MixerSettings, Track, TrackId};

fn effect() -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: PathBuf::from("/missing/Mixer.vst3"),
            class_id: "mixer-test-effect".into(),
            name: "Mix effect".into(),
            vendor: "Test".into(),
            kind: PluginKind::Effect,
        },
        vec![PluginParameter {
            id: 7,
            name: "Gain".into(),
            unit: "dB".into(),
            value: 0.5,
            steps: 0,
            read_only: false,
        }],
        vec![1, 4, 9],
    )
}

fn route(source_track: usize) -> SidechainRoute {
    SidechainRoute {
        source_track,
        bus_index: 1,
        gain: 1.0,
        tap: SidechainTap::PostFader,
    }
}

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

#[test]
fn buses_sum_routes_without_becoming_arrangement_groups() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    let group = project.add_group_track("Group");
    let bus = project.add_bus_track("Mix bus");
    let next = project.add_bus_track("Final bus");
    project.set_track_group(audio, Some(group)).unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    project
        .set_bus_track_mixer(
            bus,
            MixerSettings::default().with_output(AudioOutput::Bus(next)),
        )
        .unwrap();
    assert_eq!(
        project.group_members(group).unwrap().collect::<Vec<_>>(),
        [audio.into()]
    );
    assert_eq!(project.processing_order().unwrap(), [1, 2, 3, 4, 5, 0]);
    let before = encoded(&project);
    assert!(matches!(
        project.set_track_group(bus, Some(group)),
        Err(ProjectError::InvalidBusOutput { .. })
    ));
    assert!(matches!(
        project.set_bus_track_mixer(
            bus,
            MixerSettings::default().with_output(AudioOutput::Output)
        ),
        Err(ProjectError::MasterBypass { .. })
    ));
    assert!(matches!(
        project.set_master_track_mixer(MixerSettings::default().with_output(AudioOutput::Bus(bus))),
        Err(ProjectError::InvalidMasterOutput { .. })
    ));
    assert_eq!(encoded(&project), before);
}

#[test]
fn bus_and_mixer_sidechain_feedback_is_rejected_atomically() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let bus = project.add_bus_track("Bus");
    let downstream = project.add_bus_track("Downstream");
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    project
        .set_bus_track_mixer(
            bus,
            MixerSettings::default().with_output(AudioOutput::Bus(downstream)),
        )
        .unwrap();
    let id = project.add_mixer_effect(audio.into(), effect()).unwrap();
    let before = encoded(&project);
    for destination in [bus, downstream] {
        assert!(
            project
                .set_bus_track_mixer(
                    downstream,
                    MixerSettings::default().with_output(AudioOutput::Bus(destination))
                )
                .is_err()
        );
        assert_eq!(encoded(&project), before);
    }
    assert!(project.set_plugin_sidechain(id, Some(route(2))).is_err());
    assert_eq!(encoded(&project), before);
    let mut invalid = effect();
    invalid.sidechain = Some(route(3));
    assert!(project.add_mixer_effect(audio.into(), invalid).is_err());
    assert_eq!(
        encoded(&project),
        before,
        "rejected inserts cannot consume IDs"
    );

    let sidechain = project.add_mixer_effect(bus.into(), effect()).unwrap();
    project
        .set_plugin_sidechain(sidechain, Some(route(1)))
        .unwrap();
    let before = encoded(&project);
    assert!(project.set_plugin_sidechain(id, Some(route(2))).is_err());
    assert_eq!(encoded(&project), before);
}

#[test]
fn remove_and_reorder_preserve_bus_destinations_and_both_chains_sidechains() {
    let mut project = Project::new();
    let preceding = project.add_audio_track("Delete me");
    let audio = project.add_audio_track("Audio");
    project.add_audio_track("Sidechain");
    let bus = project.add_bus_track("Bus");
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    let device = project.add_effect(audio.into(), effect()).unwrap();
    let insert = project.add_mixer_effect(bus.into(), effect()).unwrap();
    for id in [device, insert] {
        project.set_plugin_sidechain(id, Some(route(3))).unwrap();
    }
    assert!(matches!(
        project.remove_track(bus),
        Err(ProjectError::BusNotEmpty { .. })
    ));
    project.remove_track(preceding).unwrap();
    assert_eq!(project.plugin(device).unwrap().sidechain, Some(route(2)));
    assert_eq!(project.plugin(insert).unwrap().sidechain, Some(route(2)));
    let TrackId::Bus(bus) = project.track_ids().nth(3).unwrap() else {
        panic!()
    };
    assert_eq!(project.tracks()[1].mixer().output(), AudioOutput::Bus(bus));
    assert_eq!(project.reorder_tracks(&[0, 3, 1, 2]).unwrap(), [0, 2, 3, 1]);
    let TrackId::Bus(bus) = project.track_ids().nth(1).unwrap() else {
        panic!()
    };
    assert_eq!(project.tracks()[2].mixer().output(), AudioOutput::Bus(bus));
    assert_eq!(project.plugin(insert).unwrap().sidechain, Some(route(3)));
    let source = project.track_ids().nth(3).unwrap();
    project.remove_track(source).unwrap();
    assert_eq!(project.plugin(device).unwrap().sidechain, None);
    assert_eq!(project.plugin(insert).unwrap().sidechain, None);
}

#[test]
fn every_track_has_independent_device_and_mixer_chains_with_shared_plugin_controls() {
    let mut project = Project::new();
    project.add_audio_track("Audio");
    project.add_midi_track("MIDI");
    project.add_group_track("Group");
    project.add_bus_track("Bus");
    let tracks = project.track_ids().collect::<Vec<_>>();
    for (index, track) in tracks.into_iter().enumerate() {
        let device = project.add_effect(track, effect()).unwrap();
        let first = project.add_mixer_effect(track, effect()).unwrap();
        let second = project.add_mixer_effect(track, effect()).unwrap();
        assert_ne!(device, first);
        project.move_mixer_effect(track, 0, 1).unwrap();
        assert_eq!(
            project.tracks()[index]
                .all_effects()
                .map(|effect| effect.id)
                .collect::<Vec<_>>(),
            [device, second, first]
        );
        project.set_plugin_parameter(first, 7, 0.75).unwrap();
        project.set_plugin_bypassed(first, true).unwrap();
        project.set_plugin_state(first, vec![6, 2]).unwrap();
        let saved = project.plugin(first).unwrap();
        assert_eq!(saved.parameters[0].value, 0.75);
        assert!(saved.bypassed);
        assert_eq!(saved.state, [6, 2]);
        assert_eq!(project.tracks()[index].effects()[0].id, device);
        assert_eq!(project.remove_mixer_effect(track, 0).unwrap().id, second);
        assert!(project.plugin(second).is_none());
        let before = encoded(&project);
        assert!(project.move_mixer_effect(track, 0, 2).is_err());
        assert!(project.remove_mixer_effect(track, 1).is_err());
        let mut instrument = effect();
        instrument.descriptor.kind = PluginKind::Instrument;
        assert!(project.add_mixer_effect(track, instrument).is_err());
        assert_eq!(encoded(&project), before);
    }
}

#[test]
fn mixer_automation_is_discoverable_editable_and_pruned_only_with_its_device() {
    let mut project = Project::new();
    let bus = project.add_bus_track("Bus");
    let device = project.add_effect(bus.into(), effect()).unwrap();
    let insert = project.add_mixer_effect(bus.into(), effect()).unwrap();
    let target = AutomationTarget::PluginParameter {
        plugin: insert,
        parameter: 7,
    };
    assert!(AutomationTarget::available_for(&project.tracks()[1]).contains(&target));
    assert_eq!(target.value(&project.tracks()[1]), Some(0.5));
    project
        .set_track_automation(
            bus,
            AutomationLane::new(
                target,
                vec![AutomationPoint {
                    tick: 0,
                    value: 0.8,
                }],
            )
            .unwrap(),
        )
        .unwrap();
    project.remove_effect(bus.into(), 0).unwrap();
    assert!(project.plugin(device).is_none());
    assert_eq!(project.tracks()[1].automation().len(), 1);
    let mut parameters = project.plugin(insert).unwrap().parameters.clone();
    parameters[0].value = 0.6;
    project
        .set_plugin_snapshot(insert, parameters, vec![7])
        .unwrap();
    assert_eq!(target.value(&project.tracks()[1]), Some(0.6));
    project.remove_mixer_effect(bus.into(), 0).unwrap();
    assert!(project.tracks()[1].automation().is_empty());
}

#[test]
fn persistence_preserves_buses_both_effect_chains_sidechains_and_automation() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let bus = project.add_bus_track("Bus");
    project.set_track_color(bus, Some(0x123456)).unwrap();
    project.rename_track(bus, "Mix bus").unwrap();
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    let device = project.add_effect(bus.into(), effect()).unwrap();
    let insert = project.add_mixer_effect(bus.into(), effect()).unwrap();
    project
        .set_plugin_sidechain(insert, Some(route(1)))
        .unwrap();
    project.set_plugin_bypassed(insert, true).unwrap();
    project
        .set_track_automation(
            bus,
            AutomationLane::new(
                AutomationTarget::PluginParameter {
                    plugin: insert,
                    parameter: 7,
                },
                vec![AutomationPoint {
                    tick: 99,
                    value: 0.2,
                }],
            )
            .unwrap(),
        )
        .unwrap();
    let bytes = encoded(&project);
    let mut restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(encoded(&restored), bytes);
    assert!(matches!(restored.tracks()[2], Track::Bus(_)));
    assert_eq!(restored.tracks()[2].effects()[0].id, device);
    assert_eq!(restored.tracks()[2].mixer_effects()[0].id, insert);
    assert!(restored.add_mixer_effect(bus.into(), effect()).unwrap() > insert);
    for end in 0..bytes.len() {
        assert!(
            read_project(&bytes[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
    let path = b"/missing/Mixer.vst3";
    let offsets = bytes
        .windows(path.len())
        .enumerate()
        .filter(|(_, value)| *value == path)
        .map(|(index, _)| index - 16)
        .collect::<Vec<_>>();
    let mut duplicate = bytes;
    duplicate[offsets[1]..offsets[1] + 8].copy_from_slice(&device.to_le_bytes());
    assert!(read_project(duplicate.as_slice()).is_err());
}

#[test]
fn mixer_effect_tails_follow_bus_and_sidechain_dependency_paths() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let bus = project.add_bus_track("Bus");
    let asset = project.add_asset(AudioBuffer::new(vec![[1.0; 2]]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 1,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    project.add_effect(audio.into(), effect()).unwrap();
    project.add_mixer_effect(audio.into(), effect()).unwrap();
    let bus_effect = project.add_mixer_effect(bus.into(), effect()).unwrap();
    project
        .add_mixer_effect(project.master_track_id().into(), effect())
        .unwrap();
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 4 * PLUGIN_TAIL_FRAMES
    );
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Disconnected),
        )
        .unwrap();
    project
        .set_plugin_sidechain(bus_effect, Some(route(1)))
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 4 * PLUGIN_TAIL_FRAMES
    );
    project
        .set_plugin_sidechain(
            bus_effect,
            Some(SidechainRoute {
                tap: SidechainTap::PreEffects,
                ..route(1)
            }),
        )
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 2 * PLUGIN_TAIL_FRAMES
    );
}

#[test]
fn version_eleven_devices_load_into_sound_design_chain_with_automation_intact() {
    fn string(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    let mut bytes = b"SYMPROJ\0".to_vec();
    bytes.extend_from_slice(&11_u32.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&120.0_f64.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.push(3);
    string(&mut bytes, "Master");
    bytes.extend_from_slice(&1.0_f32.to_le_bytes());
    bytes.extend_from_slice(&0.0_f32.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 0]);
    bytes.extend_from_slice(&9_u64.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&8_u64.to_le_bytes());
    for value in ["/missing/Legacy.vst3", "legacy-mixer", "Legacy", "Test"] {
        string(&mut bytes, value);
    }
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&7_u32.to_le_bytes());
    string(&mut bytes, "Gain");
    string(&mut bytes, "dB");
    bytes.extend_from_slice(&0.5_f64.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&3_u64.to_le_bytes());
    bytes.extend_from_slice(&[1, 4, 9]);
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.push(3);
    bytes.extend_from_slice(&8_u64.to_le_bytes());
    bytes.extend_from_slice(&7_u32.to_le_bytes());
    bytes.push(1);
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&0.75_f64.to_le_bytes());
    let mut project = read_project(bytes.as_slice()).unwrap();
    assert_eq!(project.tracks()[0].effects()[0].id, 8);
    assert!(project.tracks()[0].mixer_effects().is_empty());
    assert_eq!(
        project.tracks()[0].automation()[0].target(),
        AutomationTarget::PluginParameter {
            plugin: 8,
            parameter: 7
        }
    );
    assert_eq!(
        project
            .add_mixer_effect(project.master_track_id().into(), effect())
            .unwrap(),
        9
    );
    let upgraded = encoded(&project);
    assert_eq!(
        encoded(&read_project(upgraded.as_slice()).unwrap()),
        upgraded
    );
}
