use std::path::PathBuf;

use sympoie::Frame;
use sympoie::clip::AudioClip;
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PLUGIN_TAIL_FRAMES, PluginDescriptor, PluginError, PluginInstance, PluginKind,
    PluginProcessContext, PluginProcessor, PluginSidechainInput, SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::track::{AudioOutput, MixerSettings};

fn effect() -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: PathBuf::from("/missing/Sidechain.vst3"),
            class_id: "0123456789ABCDEF0123456789ABCDEF".into(),
            name: "Sidechain effect".into(),
            vendor: "Test".into(),
            kind: PluginKind::Effect,
        },
        Vec::new(),
        vec![10, 20],
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
fn sidechain_dependencies_include_disconnected_sources_and_stable_sibling_order() {
    let mut project = Project::new();
    let target = project.add_audio_track("Target");
    project.add_audio_track("Sibling");
    let source = project.add_audio_track("Disconnected source");
    assert_eq!(project.processing_order().unwrap(), [1, 2, 3, 0]);
    project
        .set_audio_track_mixer(
            source,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    let id = project.add_effect(target.into(), effect()).unwrap();
    project.set_plugin_sidechain(id, Some(route(3))).unwrap();
    assert_eq!(project.processing_order().unwrap(), [2, 3, 1, 0]);
    project.set_plugin_sidechain(id, None).unwrap();
    assert_eq!(project.processing_order().unwrap(), [1, 2, 0, 3]);
}

#[test]
fn invalid_routes_and_feedback_edits_leave_the_project_unchanged() {
    let mut project = Project::new();
    let target = project.add_audio_track("Target");
    project.add_audio_track("Source");
    let id = project.add_effect(target.into(), effect()).unwrap();
    project.set_plugin_sidechain(id, Some(route(2))).unwrap();
    let before = encoded(&project);
    for invalid in [
        route(usize::MAX),
        route(1),
        route(0),
        SidechainRoute {
            bus_index: 0,
            ..route(2)
        },
        SidechainRoute {
            gain: -0.1,
            ..route(2)
        },
        SidechainRoute {
            gain: 4.01,
            ..route(2)
        },
        SidechainRoute {
            gain: f32::NAN,
            ..route(2)
        },
        SidechainRoute {
            gain: f32::INFINITY,
            ..route(2)
        },
    ] {
        assert!(project.set_plugin_sidechain(id, Some(invalid)).is_err());
        assert_eq!(encoded(&project), before);
    }
    let mut invalid_copy = effect();
    invalid_copy.sidechain = Some(route(1));
    assert!(project.add_effect(target.into(), invalid_copy).is_err());
    assert_eq!(
        encoded(&project),
        before,
        "failed insertion must not consume an ID"
    );

    project.set_plugin_bypassed(id, true).unwrap();
    let silent_self = SidechainRoute {
        gain: 0.0,
        ..route(1)
    };
    assert!(matches!(
        project.set_plugin_sidechain(id, Some(silent_self)),
        Err(ProjectError::SidechainRoutingCycle)
    ));
    let midi = project.add_midi_track("Instrument");
    let mut instrument = effect();
    instrument.descriptor.kind = PluginKind::Instrument;
    instrument.sidechain = Some(route(2));
    assert!(matches!(
        project.set_plugin_instrument(midi, Some(instrument)),
        Err(ProjectError::Plugin(PluginError::SidechainRequiresEffect))
    ));
}

#[test]
fn sidechain_and_group_routes_cannot_form_cycles_in_either_edit_direction() {
    let mut project = Project::new();
    let target = project.add_audio_track("Target");
    let source = project.add_audio_track("Source");
    let group = project.add_group_track("Bus");
    let target_effect = project.add_effect(target.into(), effect()).unwrap();
    let source_effect = project.add_effect(source.into(), effect()).unwrap();
    project
        .set_plugin_sidechain(target_effect, Some(route(2)))
        .unwrap();
    let before = encoded(&project);
    assert!(
        project
            .set_plugin_sidechain(source_effect, Some(route(1)))
            .is_err()
    );
    assert_eq!(encoded(&project), before);

    project
        .set_plugin_sidechain(target_effect, Some(route(3)))
        .unwrap();
    let before = encoded(&project);
    assert!(matches!(
        project.set_track_group(target, Some(group)),
        Err(ProjectError::SidechainRoutingCycle)
    ));
    assert_eq!(encoded(&project), before);
    project.set_plugin_sidechain(target_effect, None).unwrap();
    project.set_track_group(target, Some(group)).unwrap();
    let before = encoded(&project);
    assert!(
        project
            .set_plugin_sidechain(target_effect, Some(route(3)))
            .is_err()
    );
    assert_eq!(encoded(&project), before);
}

#[test]
fn deletion_remaps_routes_effect_moves_preserve_them_and_source_deletion_clears_them() {
    let mut project = Project::new();
    let preceding = project.add_audio_track("Preceding");
    let target = project.add_audio_track("Target");
    project.add_audio_track("Source");
    let group = project.add_group_track("Bus");
    project.set_track_group(target, Some(group)).unwrap();
    let first = project.add_effect(target.into(), effect()).unwrap();
    let second = project.add_effect(target.into(), effect()).unwrap();
    project.set_plugin_sidechain(first, Some(route(3))).unwrap();
    project.move_effect(target.into(), 0, 1).unwrap();
    assert_eq!(project.audio_track(target).unwrap().effects()[1].id, first);
    assert_eq!(project.plugin(first).unwrap().sidechain, Some(route(3)));

    let duplicate_target = project.add_audio_track("Copy target");
    let copy = project.plugin(first).unwrap().clone();
    let duplicate = project.add_effect(duplicate_target.into(), copy).unwrap();
    assert_ne!(first, duplicate);
    assert_eq!(project.plugin(duplicate).unwrap().sidechain, Some(route(3)));
    let snapshot = project.clone();
    project.remove_track(preceding).unwrap();
    assert_eq!(project.plugin(first).unwrap().sidechain, Some(route(2)));
    assert_eq!(project.plugin(duplicate).unwrap().sidechain, Some(route(2)));
    assert_eq!(snapshot.plugin(first).unwrap().sidechain, Some(route(3)));
    let source = project.track_ids().nth(2).unwrap();
    project.remove_track(source).unwrap();
    assert_eq!(project.plugin(first).unwrap().sidechain, None);
    assert_eq!(project.plugin(duplicate).unwrap().sidechain, None);
    assert_eq!(project.plugin(second).unwrap().sidechain, None);
    project.processing_order().unwrap();
}

#[test]
fn current_version_restores_forward_sources_and_all_taps_after_main_routing() {
    let mut project = Project::new();
    let target = project.add_audio_track("Disconnected target");
    let source = project.add_audio_track("Later source");
    project
        .set_audio_track_mixer(
            target,
            MixerSettings::new(0.5, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    let mut ids = Vec::new();
    for (index, tap) in [
        SidechainTap::PreEffects,
        SidechainTap::PostEffects,
        SidechainTap::PostFader,
    ]
    .into_iter()
    .enumerate()
    {
        let id = project.add_effect(target.into(), effect()).unwrap();
        project
            .set_plugin_sidechain(
                id,
                Some(SidechainRoute {
                    source_track: if index == 0 { 0 } else { 2 },
                    bus_index: index as u32 + 1,
                    gain: index as f32 * 1.5,
                    tap,
                }),
            )
            .unwrap();
        ids.push(id);
    }
    let bytes = encoded(&project);
    assert_eq!(&bytes[8..12], &16_u32.to_le_bytes());
    let mut loaded = read_project(bytes.as_slice()).unwrap();
    assert_eq!(encoded(&loaded), bytes);
    assert_eq!(loaded.audio_track(source).unwrap().name(), "Later source");
    for id in ids {
        assert_eq!(loaded.plugin(id), project.plugin(id));
        loaded
            .set_plugin_snapshot(id, Vec::new(), vec![30, 40])
            .unwrap();
        assert_eq!(
            loaded.plugin(id).unwrap().sidechain,
            project.plugin(id).unwrap().sidechain
        );
    }
}

#[test]
fn version_three_plugins_load_without_sidechains() {
    let mut project = Project::new();
    let id = project
        .add_effect(project.master_track_id().into(), effect())
        .unwrap();
    let mut bytes = encoded(&project);
    bytes.truncate(bytes.len() - project.tracks().len() * 8);
    bytes.truncate(bytes.len() - 8 - 1 - 8 - 8);
    for offset in [54 + 24 + 1, 54 + 24, 37] {
        assert_eq!(bytes.remove(offset), 0);
    }
    bytes[8..12].copy_from_slice(&3_u32.to_le_bytes());
    let loaded = read_project(bytes.as_slice()).unwrap();
    assert_eq!(loaded.plugin(id), project.plugin(id));
    assert_eq!(loaded.plugin(id).unwrap().sidechain, None);
}

#[test]
fn effect_tail_allowance_follows_sidechain_paths_and_the_selected_tap() {
    let mut project = Project::new();
    let source = project.add_audio_track("Source");
    let target = project.add_audio_track("Target");
    let asset = project.add_asset(AudioBuffer::new(vec![[1.0; 2]]).unwrap());
    project
        .add_audio_clip(
            source,
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
    project
        .set_audio_track_mixer(
            source,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    for _ in 0..2 {
        project.add_effect(source.into(), effect()).unwrap();
    }
    let receiving = project.add_effect(target.into(), effect()).unwrap();
    for _ in 0..2 {
        project.add_effect(target.into(), effect()).unwrap();
    }
    project
        .set_plugin_sidechain(receiving, Some(route(1)))
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 5 * PLUGIN_TAIL_FRAMES
    );
    project
        .set_plugin_sidechain(
            receiving,
            Some(SidechainRoute {
                tap: SidechainTap::PostEffects,
                ..route(1)
            }),
        )
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 5 * PLUGIN_TAIL_FRAMES
    );
    project
        .set_plugin_sidechain(
            receiving,
            Some(SidechainRoute {
                tap: SidechainTap::PreEffects,
                ..route(1)
            }),
        )
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        1 + 3 * PLUGIN_TAIL_FRAMES
    );
}

#[test]
fn malformed_saved_sidechain_references_values_and_duplicate_assignments_are_rejected() {
    let mut project = Project::new();
    let target = project.add_audio_track("Target");
    project.add_audio_track("Source");
    let id = project.add_effect(target.into(), effect()).unwrap();
    project.set_plugin_sidechain(id, Some(route(2))).unwrap();
    let bytes = encoded(&project);
    let record = bytes.len() - 8 - project.tracks().len() * 17 - 25;
    for (offset, replacement) in [
        (0, u64::MAX.to_le_bytes().to_vec()),
        (8, u64::MAX.to_le_bytes().to_vec()),
        (8, 1_u64.to_le_bytes().to_vec()),
        (16, 0_u32.to_le_bytes().to_vec()),
        (20, f32::NAN.to_le_bytes().to_vec()),
        (24, vec![3]),
    ] {
        let mut invalid = bytes.clone();
        invalid[record + offset..record + offset + replacement.len()].copy_from_slice(&replacement);
        assert!(
            read_project(invalid.as_slice()).is_err(),
            "accepted malformed route field at {offset}"
        );
    }
    let mut duplicate = bytes.clone();
    duplicate[record - 8..record].copy_from_slice(&2_u64.to_le_bytes());
    duplicate.splice(
        record + 25..record + 25,
        bytes[record..record + 25].iter().copied(),
    );
    assert!(read_project(duplicate.as_slice()).is_err());
    for end in record - 8..bytes.len() {
        assert!(read_project(&bytes[..end]).is_err());
    }
}

#[derive(Debug)]
struct OrdinaryProcessor;

impl PluginProcessor for OrdinaryProcessor {
    fn reset(&mut self) {}
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, _: u32, _: f64) {}
    fn process(&mut self, frames: &mut [Frame], _: PluginProcessContext) {
        frames.fill([0.25; 2]);
    }
}

#[test]
fn existing_processors_default_to_zero_latency_and_ordinary_processing() {
    let mut processor = OrdinaryProcessor;
    assert_eq!(processor.latency_samples(), 0);
    let mut output = [[0.0; 2]; 2];
    processor.process_with_sidechain(
        &mut output,
        Some(PluginSidechainInput {
            bus_index: 1,
            frames: &[[1.0; 2]; 2],
        }),
        PluginProcessContext {
            frame: 0,
            bpm: 120.0,
            playing: true,
            quality: sympoie::plugin::RenderQuality::Realtime,
        },
    );
    assert_eq!(output, [[0.25; 2]; 2]);
}
