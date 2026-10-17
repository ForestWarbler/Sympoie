use sympoie::automation::{
    AutomationLane, AutomationPoint, AutomationTarget, MAX_AUTOMATION_POINTS, SubtractiveParameter,
};
use sympoie::instrument::{Instrument, SubtractiveSettings};
use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind, PluginParameter};
use sympoie::project::{Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::track::Track;

fn lane(target: AutomationTarget) -> AutomationLane {
    AutomationLane::new(
        target,
        vec![
            AutomationPoint {
                tick: 240,
                value: 0.2,
            },
            AutomationPoint {
                tick: 960,
                value: 0.8,
            },
        ],
    )
    .unwrap()
}

fn plugin(kind: PluginKind) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "missing.vst3".into(),
            class_id: "automation-test".into(),
            name: "Device".into(),
            vendor: "Test".into(),
            kind,
        },
        vec![
            PluginParameter {
                id: 37,
                name: "Amount".into(),
                unit: String::new(),
                value: 0.4,
                steps: 0,
                read_only: false,
            },
            PluginParameter {
                id: 38,
                name: "Meter".into(),
                unit: String::new(),
                value: 0.0,
                steps: 0,
                read_only: true,
            },
        ],
        Vec::new(),
    )
}

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

#[test]
fn envelopes_interpolate_and_hold_endpoints_without_changing_saved_values() {
    let envelope = lane(AutomationTarget::TrackGain);
    assert_eq!(envelope.value_at(0.0), Some(0.2));
    assert_eq!(envelope.value_at(240.0), Some(0.2));
    assert!((envelope.value_at(600.0).unwrap() - 0.5).abs() < 1e-12);
    assert_eq!(envelope.value_at(960.0), Some(0.8));
    assert_eq!(envelope.value_at(5000.0), Some(0.8));
    assert_eq!(envelope.value_at(f64::NAN), None);
    assert_eq!(envelope.with_enabled(false).value_at(600.0), None);
    assert_eq!(
        AutomationLane::new(AutomationTarget::TrackGain, vec![])
            .unwrap()
            .value_at(0.0),
        None
    );
}

#[test]
fn rejects_invalid_points_before_mutation() {
    for value in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
        assert_eq!(
            AutomationLane::new(
                AutomationTarget::TrackGain,
                vec![AutomationPoint { tick: 0, value }]
            ),
            Err(ProjectError::InvalidAutomationValue)
        );
    }
    for ticks in [[2, 2], [3, 2]] {
        assert_eq!(
            AutomationLane::new(
                AutomationTarget::TrackGain,
                ticks
                    .map(|tick| AutomationPoint { tick, value: 0.5 })
                    .to_vec()
            ),
            Err(ProjectError::InvalidAutomationOrder)
        );
    }
    assert_eq!(
        AutomationLane::new(
            AutomationTarget::TrackGain,
            vec![
                AutomationPoint {
                    tick: 0,
                    value: 0.5
                };
                MAX_AUTOMATION_POINTS + 1
            ]
        ),
        Err(ProjectError::TooManyAutomationPoints)
    );
    let mut project = Project::new();
    assert!(
        project
            .set_track_automation(
                project.master_track_id(),
                AutomationLane::new(
                    AutomationTarget::TrackGain,
                    vec![AutomationPoint {
                        tick: u64::MAX,
                        value: 0.5
                    }]
                )
                .unwrap()
            )
            .is_err()
    );
    assert!(project.master_track().automation().is_empty());
}

#[test]
fn model_replaces_targets_and_reordering_keeps_envelopes_on_their_track() {
    let mut project = Project::new();
    let first = project.add_audio_track("First");
    let second = project.add_audio_track("Second");
    project
        .set_track_automation(first, lane(AutomationTarget::TrackGain))
        .unwrap();
    project
        .set_track_automation(first, lane(AutomationTarget::TrackGain).with_enabled(false))
        .unwrap();
    project
        .set_track_automation(second, lane(AutomationTarget::TrackPan))
        .unwrap();
    assert_eq!(project.audio_track(first).unwrap().automation().len(), 1);
    project.reorder_tracks(&[0, 2, 1]).unwrap();
    assert_eq!(
        project.tracks()[1].automation()[0].target(),
        AutomationTarget::TrackPan
    );
    assert_eq!(
        project.tracks()[2].automation()[0].target(),
        AutomationTarget::TrackGain
    );
    assert!(!project.tracks()[2].automation()[0].enabled());
    let id = project.track_ids().nth(2).unwrap();
    project
        .remove_track_automation(id, AutomationTarget::TrackGain)
        .unwrap();
    assert!(project.tracks()[2].automation().is_empty());
}

#[test]
fn grouping_and_deleting_another_track_preserve_the_same_envelopes() {
    let mut project = Project::new();
    let removed = project.add_audio_track("Remove me");
    let audio = project.add_audio_track("Keep me");
    let group = project.add_group_track("Group");
    project
        .set_track_automation(audio, lane(AutomationTarget::TrackGain))
        .unwrap();
    project.set_track_group(audio, Some(group)).unwrap();
    assert_eq!(
        project.audio_track(audio).unwrap().automation(),
        &[lane(AutomationTarget::TrackGain)]
    );
    project.remove_track(removed).unwrap();
    assert_eq!(project.tracks()[1].name(), "Keep me");
    assert_eq!(
        project.tracks()[1].automation(),
        &[lane(AutomationTarget::TrackGain)]
    );
    let id = project.track_ids().nth(1).unwrap();
    project.set_track_group(id, None).unwrap();
    assert_eq!(
        project.tracks()[1].automation(),
        &[lane(AutomationTarget::TrackGain)]
    );
    let clone = project.clone();
    project
        .remove_track_automation(id, AutomationTarget::TrackGain)
        .unwrap();
    assert_eq!(
        clone.tracks()[1].automation(),
        &[lane(AutomationTarget::TrackGain)]
    );
}

#[test]
fn device_targets_validate_ownership_and_are_removed_when_devices_disappear() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Synth");
    let other = project.add_audio_track("Other");
    let cutoff = AutomationTarget::Subtractive(SubtractiveParameter::Cutoff);
    assert_eq!(
        project.set_track_automation(midi, lane(cutoff)),
        Err(ProjectError::InvalidAutomationTarget)
    );
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    project.set_track_automation(midi, lane(cutoff)).unwrap();
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings {
                gain: 0.6,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    assert_eq!(project.midi_track(midi).unwrap().automation().len(), 1);
    project
        .set_plugin_instrument(midi, Some(plugin(PluginKind::Instrument)))
        .unwrap();
    assert!(project.midi_track(midi).unwrap().automation().is_empty());
    let id = project
        .midi_track(midi)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    let parameter = AutomationTarget::PluginParameter {
        plugin: id,
        parameter: 37,
    };
    project.set_track_automation(midi, lane(parameter)).unwrap();
    assert_eq!(
        project.set_track_automation(other, lane(parameter)),
        Err(ProjectError::InvalidAutomationTarget)
    );
    assert_eq!(
        project.set_track_automation(
            midi,
            lane(AutomationTarget::PluginParameter {
                plugin: id,
                parameter: 38
            })
        ),
        Err(ProjectError::InvalidAutomationTarget)
    );
    project.set_plugin_instrument(midi, None).unwrap();
    assert!(project.midi_track(midi).unwrap().automation().is_empty());
    let effect = project
        .add_effect(other.into(), plugin(PluginKind::Effect))
        .unwrap();
    project
        .set_track_automation(
            other,
            lane(AutomationTarget::PluginParameter {
                plugin: effect,
                parameter: 37,
            }),
        )
        .unwrap();
    project.remove_effect(other.into(), 0).unwrap();
    assert!(project.audio_track(other).unwrap().automation().is_empty());
}

#[test]
fn normalized_subtractive_parameters_stay_valid_and_round_trip() {
    for parameter in SubtractiveParameter::ALL {
        for value in [0.0, 0.5, 1.0] {
            let mut settings = SubtractiveSettings::default();
            parameter.set_normalized(&mut settings, value);
            assert!(settings.validate().is_ok(), "{parameter:?}: {settings:?}");
            assert!((parameter.normalized_value(&settings) - value).abs() < 1e-6);
        }
    }
}

#[test]
fn version_eight_round_trips_every_target_and_disabled_lanes() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Synth");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    let effect = project
        .add_effect(midi.into(), plugin(PluginKind::Effect))
        .unwrap();
    project
        .set_track_automation(project.master_track_id(), lane(AutomationTarget::TrackGain))
        .unwrap();
    for target in [
        AutomationTarget::TrackPan,
        AutomationTarget::PluginParameter {
            plugin: effect,
            parameter: 37,
        },
    ] {
        project
            .set_track_automation(midi, lane(target).with_enabled(false))
            .unwrap();
    }
    for parameter in SubtractiveParameter::ALL {
        project
            .set_track_automation(midi, lane(AutomationTarget::Subtractive(parameter)))
            .unwrap();
    }
    let bytes = encoded(&project);
    assert_eq!(&bytes[8..12], &13_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    for (before, after) in project.tracks().iter().zip(restored.tracks()) {
        assert_eq!(before.automation(), after.automation());
        for target in AutomationTarget::available_for(after) {
            assert!(target.value(after).is_some());
        }
    }
}

#[test]
fn legacy_version_seven_loads_empty_envelopes() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    project
        .set_audio_track_mixer(
            audio,
            project
                .audio_track(audio)
                .unwrap()
                .mixer()
                .with_soloed(true),
        )
        .unwrap();
    let mut bytes = encoded(&project);
    bytes.truncate(bytes.len() - project.tracks().len() * 8);
    bytes.truncate(bytes.len() - 4 - project.tracks().len() * 8);
    let plugins = bytes.len() - 2 - 8 - 2 * 17;
    for offset in [plugins + 17 + 9, plugins + 9] {
        assert_eq!(&bytes[offset..offset + 8], &[0; 8]);
        bytes.drain(offset..offset + 8);
    }
    bytes[8..12].copy_from_slice(&7_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert!(
        restored
            .tracks()
            .iter()
            .all(|track| track.automation().is_empty())
    );
    assert!(restored.tracks()[1].mixer().soloed());
}

#[test]
fn malformed_automation_extensions_are_rejected() {
    let mut project = Project::new();
    project
        .set_track_automation(project.master_track_id(), lane(AutomationTarget::TrackGain))
        .unwrap();
    let bytes = encoded(&project);
    let start = bytes.len() - 8 - 50;
    for offset in start..bytes.len() {
        assert!(read_project(&bytes[..offset]).is_err());
    }
    for (offset, replacement) in [
        (start, u64::MAX.to_le_bytes().to_vec()),
        (start + 8, vec![255]),
        (start + 9, vec![2]),
        (start + 10, u64::MAX.to_le_bytes().to_vec()),
        (start + 26, f64::NAN.to_le_bytes().to_vec()),
        (start + 34, 240_u64.to_le_bytes().to_vec()),
    ] {
        let mut invalid = bytes.clone();
        invalid[offset..offset + replacement.len()].copy_from_slice(&replacement);
        assert!(read_project(invalid.as_slice()).is_err(), "offset {offset}");
    }
    let mut duplicate = bytes.clone();
    duplicate[start..start + 8].copy_from_slice(&2_u64.to_le_bytes());
    duplicate.splice(
        start + 50..start + 50,
        bytes[start + 8..start + 50].iter().copied(),
    );
    assert!(read_project(duplicate.as_slice()).is_err());
    let mut missing_instrument = bytes.clone();
    missing_instrument[start + 8] = 2;
    missing_instrument.insert(start + 9, 1);
    assert!(read_project(missing_instrument.as_slice()).is_err());
    missing_instrument[start + 9] = 255;
    assert!(read_project(missing_instrument.as_slice()).is_err());
    let mut missing_plugin = bytes;
    missing_plugin[start + 8] = 3;
    missing_plugin.splice(
        start + 9..start + 9,
        [
            1_u64.to_le_bytes().as_slice(),
            37_u32.to_le_bytes().as_slice(),
        ]
        .concat(),
    );
    assert!(read_project(missing_plugin.as_slice()).is_err());
}

#[test]
fn automated_release_extends_export_duration() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Synth");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings {
                release_seconds: 0.0,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    let mut clip = sympoie::clip::MidiClip::new(0, 960);
    clip.notes.push(sympoie::clip::MidiNote {
        start_tick: 0,
        duration_ticks: 960,
        key: 60,
        velocity: 100,
        channel: 0,
    });
    project.add_midi_clip(midi, clip).unwrap();
    let before = project.duration_frames().unwrap();
    project
        .set_track_automation(
            midi,
            lane(AutomationTarget::Subtractive(SubtractiveParameter::Release)),
        )
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), before + 8 * 48_000);
    assert!(matches!(project.tracks()[1], Track::Midi(_)));
}
