use std::path::PathBuf;

use sympoie::SAMPLE_RATE;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings};
use sympoie::plugin::{
    MAX_PLUGIN_STRING_BYTES, PluginDescriptor, PluginError, PluginInstance, PluginKind,
    PluginParameter,
};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{read_project, write_project};

fn device(kind: PluginKind) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: PathBuf::from("/missing/plugins/Example.vst3"),
            class_id: "0123456789ABCDEFFEDCBA9876543210".into(),
            name: "Example device".into(),
            vendor: "Example vendor".into(),
            kind,
        },
        vec![PluginParameter {
            id: 11,
            name: "Level".into(),
            unit: "dB".into(),
            value: 0.375,
            steps: 0,
            read_only: false,
        }],
        vec![0, 1, 255, 2],
    )
}

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

#[test]
fn instrument_replacement_and_effect_order_keep_distinct_project_assigned_ids() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Instrument");
    project
        .set_plugin_instrument(midi, Some(device(PluginKind::Instrument)))
        .unwrap();
    let instrument_id = project
        .midi_track(midi)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    let mut copy = device(PluginKind::Effect);
    copy.id = instrument_id;
    let first = project.add_effect(midi.into(), copy.clone()).unwrap();
    let second = project.add_effect(midi.into(), copy).unwrap();
    assert_ne!(instrument_id, first);
    assert_ne!(first, second);
    project.move_effect(midi.into(), 0, 1).unwrap();
    assert_eq!(project.midi_track(midi).unwrap().effects()[0].id, second);
    assert_eq!(project.remove_effect(midi.into(), 0).unwrap().id, second);
    assert!(project.plugin(second).is_none());
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    assert!(
        project
            .midi_track(midi)
            .unwrap()
            .plugin_instrument()
            .is_none()
    );
    project
        .set_plugin_instrument(midi, Some(device(PluginKind::Instrument)))
        .unwrap();
    assert!(project.midi_track(midi).unwrap().instrument().is_none());
    assert!(
        project
            .midi_track(midi)
            .unwrap()
            .plugin_instrument()
            .unwrap()
            .id
            > second
    );
}

#[test]
fn every_track_kind_supports_effects_and_invalid_edits_are_atomic() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    let group = project.add_group_track("Group");
    let ids = [
        project.master_track_id().into(),
        audio.into(),
        midi.into(),
        group.into(),
    ];
    for id in ids {
        project.add_effect(id, device(PluginKind::Effect)).unwrap();
    }
    let before = encoded(&project);
    assert!(
        project
            .add_effect(audio.into(), device(PluginKind::Instrument))
            .is_err()
    );
    assert!(
        project
            .set_plugin_instrument(midi, Some(device(PluginKind::Effect)))
            .is_err()
    );
    assert!(project.move_effect(group.into(), 0, 1).is_err());
    assert!(project.remove_effect(group.into(), 1).is_err());
    assert_eq!(encoded(&project), before);
    assert!(
        project
            .tracks()
            .iter()
            .all(|track| track.effects().len() == 1)
    );
}

#[test]
fn parameter_and_bypass_changes_preserve_identity_and_reject_invalid_values() {
    let mut project = Project::new();
    let master = project.master_track_id().into();
    let mut effect = device(PluginKind::Effect);
    effect.parameters.push(PluginParameter {
        id: 12,
        read_only: true,
        ..effect.parameters[0].clone()
    });
    let id = project.add_effect(master, effect).unwrap();
    project.set_plugin_parameter(id, 11, 0.8).unwrap();
    project.set_plugin_bypassed(id, true).unwrap();
    let before = encoded(&project);
    for value in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
        assert!(matches!(
            project.set_plugin_parameter(id, 11, value),
            Err(ProjectError::Plugin(PluginError::InvalidParameterValue))
        ));
    }
    assert!(project.set_plugin_parameter(id, 12, 0.2).is_err());
    assert!(project.set_plugin_parameter(id, 99, 0.2).is_err());
    assert!(project.set_plugin_bypassed(id + 1, false).is_err());
    assert_eq!(encoded(&project), before);
    let effect = project.plugin(id).unwrap();
    assert!(effect.bypassed);
    assert_eq!(effect.parameters[0].value, 0.8);
}

#[test]
fn invalid_metadata_and_duplicate_parameter_ids_are_rejected() {
    for field in ["path", "class", "name", "value", "duplicate", "nul"] {
        let mut effect = device(PluginKind::Effect);
        match field {
            "path" => effect.descriptor.path = PathBuf::new(),
            "class" => effect.descriptor.class_id.clear(),
            "name" => effect.descriptor.name.clear(),
            "value" => effect.parameters[0].value = f64::NAN,
            "duplicate" => effect.parameters.push(effect.parameters[0].clone()),
            "nul" => effect.descriptor.path = PathBuf::from("bad\0path"),
            _ => unreachable!(),
        }
        let mut project = Project::new();
        assert!(
            project
                .add_effect(project.master_track_id().into(), effect)
                .is_err(),
            "{field}"
        );
        assert!(project.master_track().effects().is_empty());
    }
}

#[test]
fn current_version_round_trip_preserves_missing_devices_state_parameters_order_and_ids() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Synth");
    let audio = project.add_audio_track("Audio");
    let group = project.add_group_track("Bus");
    project
        .set_plugin_instrument(midi, Some(device(PluginKind::Instrument)))
        .unwrap();
    for id in [
        project.master_track_id().into(),
        midi.into(),
        audio.into(),
        group.into(),
    ] {
        project.add_effect(id, device(PluginKind::Effect)).unwrap();
        let effect = project.add_effect(id, device(PluginKind::Effect)).unwrap();
        project.set_plugin_bypassed(effect, true).unwrap();
        project.set_plugin_parameter(effect, 11, 0.9).unwrap();
    }
    let bytes = encoded(&project);
    assert_eq!(&bytes[8..12], &16_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(encoded(&restored), bytes);
    assert_eq!(
        restored.midi_track(midi).unwrap().plugin_instrument(),
        project.midi_track(midi).unwrap().plugin_instrument()
    );
    for (a, b) in restored.tracks().iter().zip(project.tracks()) {
        assert_eq!(a.effects(), b.effects());
    }
}

#[test]
fn deleted_device_ids_are_not_reused_after_save_and_load() {
    let mut project = Project::new();
    let master = project.master_track_id().into();
    let first = project
        .add_effect(master, device(PluginKind::Effect))
        .unwrap();
    project.remove_effect(master, 0).unwrap();
    let mut restored = read_project(encoded(&project).as_slice()).unwrap();
    let next = restored
        .add_effect(master, device(PluginKind::Effect))
        .unwrap();
    assert!(next > first);
}

#[test]
fn snapshot_capture_is_validated_and_does_not_mutate_other_clones() {
    let mut project = Project::new();
    let id = project
        .add_effect(project.master_track_id().into(), device(PluginKind::Effect))
        .unwrap();
    let mut saved = project.clone();
    let mut parameters = saved.plugin(id).unwrap().parameters.clone();
    parameters[0].read_only = true;
    parameters[0].value = 0.875;
    saved
        .set_plugin_snapshot(id, parameters, vec![9, 8, 7])
        .unwrap();
    assert_eq!(project.plugin(id).unwrap().state, [0, 1, 255, 2]);
    assert_eq!(saved.plugin(id).unwrap().state, [9, 8, 7]);
    assert_eq!(saved.plugin(id).unwrap().parameters[0].value, 0.875);
    let before = encoded(&saved);
    let mut bad = saved.plugin(id).unwrap().parameters.clone();
    bad[0].value = f64::NAN;
    assert!(saved.set_plugin_snapshot(id, bad, vec![]).is_err());
    assert_eq!(encoded(&saved), before);
}

#[test]
fn external_instrument_and_routed_effect_chains_contribute_release_tails() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Synth");
    project
        .set_plugin_instrument(midi, Some(device(PluginKind::Instrument)))
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 960,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 960,
                    key: 60,
                    velocity: 100,
                    channel: 0,
                    slide_ticks: 0,
                }],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let group = project.add_group_track("Bus");
    project.set_track_group(midi, Some(group)).unwrap();
    for track in [midi.into(), group.into(), project.master_track_id().into()] {
        project
            .add_effect(track, device(PluginKind::Effect))
            .unwrap();
    }
    assert_eq!(
        project.duration_frames().unwrap(),
        SAMPLE_RATE as u64 / 2 + SAMPLE_RATE as u64 * 8
    );
}

#[test]
fn audio_effects_get_a_tail_but_empty_projects_remain_empty() {
    let mut project = Project::new();
    project
        .add_effect(project.master_track_id().into(), device(PluginKind::Effect))
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), 0);
    let audio = project.add_audio_track("Audio");
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
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + SAMPLE_RATE as u64 * 2
    );
}

#[test]
fn version_two_projects_still_load_without_external_devices() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Legacy synth");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    let mut bytes = encoded(&project);
    bytes.truncate(bytes.len() - project.tracks().len() * 8);
    let v7_flags = project.tracks().len() + 1;
    bytes.truncate(bytes.len() - 2 - v7_flags - 16 - 25 * project.tracks().len());
    for flag in [
        54 + 24 + 2 + 30 + 1,
        54 + 24 + 2 + 30,
        54 + 24 + 1,
        54 + 24,
        37,
    ] {
        assert_eq!(bytes.remove(flag), 0);
    }
    bytes[8..12].copy_from_slice(&2_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(
        restored.midi_track(midi).unwrap().instrument(),
        project.midi_track(midi).unwrap().instrument()
    );
    assert!(
        restored
            .midi_track(midi)
            .unwrap()
            .plugin_instrument()
            .is_none()
    );
    assert!(
        restored
            .tracks()
            .iter()
            .all(|track| track.effects().is_empty())
    );
}

#[test]
fn truncated_plugin_sections_and_invalid_flags_are_rejected() {
    let mut project = Project::new();
    project
        .add_effect(project.master_track_id().into(), device(PluginKind::Effect))
        .unwrap();
    let bytes = encoded(&project);
    for length in 0..bytes.len() {
        assert!(
            read_project(&bytes[..length]).is_err(),
            "accepted truncation at {length}"
        );
    }
    let mut bad = bytes;
    bad[88] = 2;
    assert!(read_project(bad.as_slice()).is_err());
}

#[test]
fn duplicate_saved_instance_ids_and_oversized_strings_are_rejected() {
    let mut project = Project::new();
    let master = project.master_track_id().into();
    project
        .add_effect(master, device(PluginKind::Effect))
        .unwrap();
    project
        .add_effect(master, device(PluginKind::Effect))
        .unwrap();
    let mut bytes = encoded(&project);
    let path = b"/missing/plugins/Example.vst3";
    let starts = bytes
        .windows(path.len())
        .enumerate()
        .filter(|(_, value)| *value == path)
        .map(|(i, _)| i - 16)
        .collect::<Vec<_>>();
    bytes[starts[1]..starts[1] + 8].copy_from_slice(&1_u64.to_le_bytes());
    assert!(read_project(bytes.as_slice()).is_err());

    let mut effect = device(PluginKind::Effect);
    effect.descriptor.name = "x".repeat(MAX_PLUGIN_STRING_BYTES + 1);
    assert!(effect.validate().is_err());
    let mut bytes = encoded(&project);
    bytes[starts[0] + 8..starts[0] + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(read_project(bytes.as_slice()).is_err());
}
