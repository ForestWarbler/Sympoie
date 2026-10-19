use sympoie::clip::AudioClip;
use sympoie::plugin::{
    PLUGIN_TAIL_FRAMES, PluginDescriptor, PluginInstance, PluginKind, SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::track::{AudioOutput, MixerSettings, TrackId, TrackSend};

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn effect() -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/missing/Send-test.vst3".into(),
            class_id: "send-test-effect".into(),
            name: "Send effect".into(),
            vendor: "Test".into(),
            kind: PluginKind::Effect,
        },
        Vec::new(),
        vec![1, 2],
    )
}

fn sidechain(source_track: usize) -> SidechainRoute {
    SidechainRoute {
        source_track,
        bus_index: 1,
        gain: 1.0,
        tap: SidechainTap::PostFader,
    }
}

#[test]
fn multiple_bus_sends_have_independent_gains_and_preserve_the_main_output() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let group = project.add_group_track("Group");
    let reverb = project.add_bus_track("Reverb");
    let delay = project.add_bus_track("Delay");
    let mixer = MixerSettings::new(0.8, -0.3, AudioOutput::Group(group))
        .unwrap()
        .with_soloed(true);
    project.set_audio_track_mixer(audio, mixer).unwrap();
    project.set_track_send(audio, reverb, 0.25).unwrap();
    project.set_track_send(audio, delay, 0.75).unwrap();
    assert_eq!(*project.audio_track(audio).unwrap().mixer(), mixer);
    assert_eq!(
        project.audio_track(audio).unwrap().sends(),
        [
            TrackSend {
                destination: reverb,
                gain: 0.25
            },
            TrackSend {
                destination: delay,
                gain: 0.75
            }
        ]
    );
    project.set_track_send(audio, reverb, 1.25).unwrap();
    assert_eq!(
        project.audio_track(audio).unwrap().sends(),
        [
            TrackSend {
                destination: reverb,
                gain: 1.25
            },
            TrackSend {
                destination: delay,
                gain: 0.75
            }
        ]
    );
    project.remove_track_send(audio, reverb).unwrap();
    project.remove_track_send(audio, reverb).unwrap();
    assert_eq!(
        project.audio_track(audio).unwrap().sends(),
        [TrackSend {
            destination: delay,
            gain: 0.75
        }]
    );
    assert_eq!(*project.audio_track(audio).unwrap().mixer(), mixer);
}

#[test]
fn all_nonmaster_tracks_can_send_and_invalid_edits_are_atomic() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    let group = project.add_group_track("Group");
    let bus = project.add_bus_track("Source bus");
    let target = project.add_bus_track("Target bus");
    for source in [TrackId::from(audio), midi.into(), group.into(), bus.into()] {
        project.set_track_send(source, target, 0.5).unwrap();
    }
    let before = encoded(&project);
    for gain in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.001] {
        assert!(matches!(
            project.set_track_send(audio, target, gain),
            Err(ProjectError::InvalidSendGain)
        ));
        assert_eq!(encoded(&project), before);
    }
    assert!(matches!(
        project.set_track_send(project.master_track_id(), target, 0.0),
        Err(ProjectError::MasterSend)
    ));
    assert_eq!(encoded(&project), before);
    let mut foreign = Project::new();
    let not_a_bus_here = foreign.add_bus_track("Foreign bus");
    assert!(matches!(
        project.set_track_send(audio, not_a_bus_here, 0.2),
        Err(ProjectError::UnknownBusTrack { .. })
    ));
    assert_eq!(encoded(&project), before);
}

#[test]
fn zero_gain_send_main_output_and_sidechain_cycles_are_all_rejected() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let bus = project.add_bus_track("Bus");
    let downstream = project.add_bus_track("Downstream");
    project.set_track_send(audio, bus, 0.0).unwrap();
    project.set_track_send(bus, downstream, 0.0).unwrap();
    let effect = project.add_mixer_effect(audio.into(), effect()).unwrap();
    let before = encoded(&project);
    for destination in [bus, downstream] {
        assert!(
            project
                .set_track_send(downstream, destination, 0.0)
                .is_err()
        );
        assert_eq!(encoded(&project), before);
    }
    assert!(
        project
            .set_bus_track_mixer(
                downstream,
                MixerSettings::default().with_output(AudioOutput::Bus(bus))
            )
            .is_err()
    );
    assert!(
        project
            .set_plugin_sidechain(effect, Some(sidechain(2)))
            .is_err()
    );
    assert_eq!(encoded(&project), before);
    project.remove_track_send(audio, bus).unwrap();
    project
        .set_plugin_sidechain(effect, Some(sidechain(2)))
        .unwrap();
    let before = encoded(&project);
    assert!(project.set_track_send(audio, bus, 0.0).is_err());
    assert_eq!(encoded(&project), before);
    project.set_plugin_sidechain(effect, None).unwrap();
    project
        .set_bus_track_mixer(
            bus,
            MixerSettings::default().with_output(AudioOutput::Bus(downstream)),
        )
        .unwrap();
    let before = encoded(&project);
    assert!(project.set_track_send(downstream, bus, 0.5).is_err());
    assert_eq!(encoded(&project), before);
}

#[test]
fn send_edges_define_dependency_order_without_turning_into_group_membership() {
    let mut project = Project::new();
    let bus = project.add_bus_track("Early bus");
    let group = project.add_group_track("Group");
    let audio = project.add_audio_track("Late audio");
    project.set_track_group(audio, Some(group)).unwrap();
    project.set_track_send(group, bus, 0.5).unwrap();
    assert_eq!(project.processing_order().unwrap(), [3, 2, 1, 0]);
    assert_eq!(
        project.group_members(group).unwrap().collect::<Vec<_>>(),
        [audio.into()]
    );
    assert_eq!(
        project.audio_track(audio).unwrap().mixer().output(),
        AudioOutput::Group(group)
    );
    let receiving = project.add_mixer_effect(group.into(), effect()).unwrap();
    let before = encoded(&project);
    assert!(
        project
            .set_plugin_sidechain(receiving, Some(sidechain(1)))
            .is_err()
    );
    assert_eq!(encoded(&project), before);
}

#[test]
fn reordering_and_removing_tracks_remaps_send_destinations_and_clears_removed_bus_sends() {
    let mut project = Project::new();
    let preceding = project.add_audio_track("Remove");
    let audio = project.add_audio_track("Audio");
    let first = project.add_bus_track("First");
    let second = project.add_bus_track("Second");
    project.set_track_send(audio, first, 0.25).unwrap();
    project.set_track_send(audio, second, 0.75).unwrap();
    project.set_track_send(first, second, 0.5).unwrap();
    project.remove_track(preceding).unwrap();
    let TrackId::Audio(audio) = project.track_ids().nth(1).unwrap() else {
        panic!()
    };
    let TrackId::Bus(first) = project.track_ids().nth(2).unwrap() else {
        panic!()
    };
    let TrackId::Bus(second) = project.track_ids().nth(3).unwrap() else {
        panic!()
    };
    assert_eq!(
        project.audio_track(audio).unwrap().sends(),
        [
            TrackSend {
                destination: first,
                gain: 0.25
            },
            TrackSend {
                destination: second,
                gain: 0.75
            }
        ]
    );
    project.reorder_tracks(&[0, 3, 1, 2]).unwrap();
    let TrackId::Bus(second) = project.track_ids().nth(1).unwrap() else {
        panic!()
    };
    let TrackId::Audio(audio) = project.track_ids().nth(2).unwrap() else {
        panic!()
    };
    let TrackId::Bus(first) = project.track_ids().nth(3).unwrap() else {
        panic!()
    };
    assert_eq!(
        project.audio_track(audio).unwrap().sends(),
        [
            TrackSend {
                destination: first,
                gain: 0.25
            },
            TrackSend {
                destination: second,
                gain: 0.75
            }
        ]
    );
    assert_eq!(
        project.bus_track(first).unwrap().sends(),
        [TrackSend {
            destination: second,
            gain: 0.5
        }]
    );
    project.remove_track(second).unwrap();
    let TrackId::Audio(audio) = project.track_ids().nth(1).unwrap() else {
        panic!()
    };
    let TrackId::Bus(first) = project.track_ids().nth(2).unwrap() else {
        panic!()
    };
    assert_eq!(
        project.audio_track(audio).unwrap().sends(),
        [TrackSend {
            destination: first,
            gain: 0.25
        }]
    );
    assert!(project.bus_track(first).unwrap().sends().is_empty());
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Bus(first)),
        )
        .unwrap();
    let before = encoded(&project);
    assert!(matches!(
        project.remove_track(first),
        Err(ProjectError::BusNotEmpty { .. })
    ));
    assert_eq!(encoded(&project), before);
    project
        .set_audio_track_mixer(audio, MixerSettings::default())
        .unwrap();
    project.remove_track(first).unwrap();
    assert!(project.tracks()[1].sends().is_empty());
}

#[test]
fn send_chains_extend_effect_tails_even_when_original_output_is_disconnected() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let first = project.add_bus_track("First");
    let second = project.add_bus_track("Second");
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
                name: None,
            },
        )
        .unwrap();
    project.add_effect(audio.into(), effect()).unwrap();
    for id in [
        audio.into(),
        first.into(),
        second.into(),
        project.master_track_id().into(),
    ] {
        project.add_mixer_effect(id, effect()).unwrap();
    }
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Disconnected),
        )
        .unwrap();
    project.set_track_send(audio, first, 0.5).unwrap();
    project.set_track_send(first, second, 0.5).unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 5 * PLUGIN_TAIL_FRAMES
    );
    project.remove_track_send(first, second).unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 4 * PLUGIN_TAIL_FRAMES
    );
}

#[test]
fn sends_round_trip_with_ordinary_outputs_and_mixer_effects() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    let group = project.add_group_track("Group");
    let first = project.add_bus_track("First");
    let second = project.add_bus_track("Second");
    project.set_track_group(audio, Some(group)).unwrap();
    for (source, gain) in [
        (TrackId::from(audio), 0.0),
        (midi.into(), 0.25),
        (group.into(), 0.75),
        (first.into(), 1.5),
    ] {
        project.set_track_send(source, second, gain).unwrap();
    }
    project.set_track_send(audio, first, 0.5).unwrap();
    project.add_mixer_effect(first.into(), effect()).unwrap();
    let bytes = encoded(&project);
    assert_eq!(&bytes[8..12], &16_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(encoded(&restored), bytes);
    for (before, after) in project.tracks().iter().zip(restored.tracks()) {
        assert_eq!(before.sends(), after.sends());
        assert_eq!(before.mixer(), after.mixer());
        assert_eq!(before.mixer_effects(), after.mixer_effects());
    }
    for end in bytes.len() - (project.tracks().len() * 8 + 5 * 12)..bytes.len() {
        assert!(
            read_project(&bytes[..end]).is_err(),
            "accepted truncated send extension at {end}"
        );
    }
}

#[test]
fn version_twelve_buses_load_with_empty_sends_and_preserve_mixer_chains() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let bus = project.add_bus_track("Bus");
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    project.add_effect(audio.into(), effect()).unwrap();
    project.add_mixer_effect(bus.into(), effect()).unwrap();
    let expected = encoded(&project);
    let mut legacy = expected.clone();
    let end = legacy.len() - project.tracks().len() * 8;
    assert!(legacy[end..].iter().all(|byte| *byte == 0));
    legacy.truncate(end);
    legacy[8..12].copy_from_slice(&12_u32.to_le_bytes());
    let restored = read_project(legacy.as_slice()).unwrap();
    assert!(
        restored
            .tracks()
            .iter()
            .all(|track| track.sends().is_empty())
    );
    assert_eq!(encoded(&restored), expected);
}

#[test]
fn malformed_send_extensions_reject_duplicates_invalid_gain_targets_master_and_cycles() {
    let mut project = Project::new();
    project.add_audio_track("Audio");
    project.add_bus_track("First");
    project.add_bus_track("Second");
    let mut prefix = encoded(&project);
    prefix.truncate(prefix.len() - project.tracks().len() * 8);
    let empty: Vec<(u64, f32)> = Vec::new();
    let cases = [
        [
            empty.clone(),
            vec![(2, 0.2), (2, 0.3)],
            empty.clone(),
            empty.clone(),
        ],
        [empty.clone(), vec![(0, 0.2)], empty.clone(), empty.clone()],
        [empty.clone(), vec![(1, 0.2)], empty.clone(), empty.clone()],
        [
            empty.clone(),
            vec![(u64::MAX, 0.2)],
            empty.clone(),
            empty.clone(),
        ],
        [
            empty.clone(),
            vec![(2, f32::NAN)],
            empty.clone(),
            empty.clone(),
        ],
        [
            empty.clone(),
            vec![(2, f32::INFINITY)],
            empty.clone(),
            empty.clone(),
        ],
        [empty.clone(), vec![(2, -0.1)], empty.clone(), empty.clone()],
        [vec![(2, 0.0)], empty.clone(), empty.clone(), empty.clone()],
        [empty.clone(), empty.clone(), vec![(2, 0.0)], empty.clone()],
        [empty.clone(), empty.clone(), vec![(3, 0.0)], vec![(2, 0.0)]],
    ];
    for sends in cases {
        let mut bytes = prefix.clone();
        for sends in sends {
            bytes.extend_from_slice(&(sends.len() as u64).to_le_bytes());
            for (destination, gain) in sends {
                bytes.extend_from_slice(&destination.to_le_bytes());
                bytes.extend_from_slice(&gain.to_le_bytes());
            }
        }
        assert!(read_project(bytes.as_slice()).is_err());
    }
    let mut huge = prefix;
    huge.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(read_project(huge.as_slice()).is_err());
}
