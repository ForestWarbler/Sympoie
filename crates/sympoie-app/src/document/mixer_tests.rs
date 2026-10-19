use std::collections::HashSet;

use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind, SidechainRoute, SidechainTap};
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::write_project;
use sympoie::track::{AudioOutput, MixerSettings, Track, TrackId};

use super::edit::{self, TrackDrop};
use super::{Document, rows};

fn plugin(kind: PluginKind) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/missing/Mixer-test.vst3".into(),
            class_id: "mixer-document-test".into(),
            name: "Mixer test".into(),
            vendor: "Test".into(),
            kind,
        },
        Vec::new(),
        vec![4, 5, 6],
    )
}

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn index(project: &Project, name: &str) -> usize {
    project
        .tracks()
        .iter()
        .position(|track| track.name() == name)
        .unwrap()
}

fn output_name<'a>(project: &'a Project, name: &str) -> Option<&'a str> {
    let output = project.tracks()[index(project, name)].mixer().output();
    let target = match output {
        AudioOutput::Master => return Some("Master"),
        AudioOutput::Group(id) => TrackId::Group(id),
        AudioOutput::Bus(id) => TrackId::Bus(id),
        AudioOutput::Output | AudioOutput::Disconnected => return None,
    };
    project
        .track_ids()
        .position(|id| id == target)
        .map(|index| project.tracks()[index].name())
}

fn rows_named<'a>(project: &'a Project, rows: &[rows::Row]) -> Vec<&'a str> {
    rows.iter()
        .map(|row| project.tracks()[row.index].name())
        .collect()
}

fn bus_project() -> Project {
    let mut project = Project::new();
    let bus = project.add_bus_track("Mix bus");
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    let group = project.add_group_track("Group");
    let bus_source = project.add_bus_track("Bus source");
    let destination = project.add_bus_track("Final bus");
    project
        .set_bus_track_mixer(
            bus,
            MixerSettings::new(0.7, 0.2, AudioOutput::Bus(destination)).unwrap(),
        )
        .unwrap();
    for id in [audio.into(), midi.into(), group.into(), bus_source.into()] {
        let index = project
            .track_ids()
            .position(|candidate| candidate == id)
            .unwrap();
        edit::set_mixer(
            &mut project,
            index,
            MixerSettings::new(0.8, -0.3, AudioOutput::Bus(bus)).unwrap(),
        )
        .unwrap();
    }
    let asset = project.add_asset(AudioBuffer::new(vec![[0.2, -0.1]; 32]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 2,
                start_tick: 37,
                frame_count: 20,
                gain: 0.4,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                start_tick: 90,
                length_ticks: 960,
                offset_ticks: 0,
                notes: vec![MidiNote {
                    start_tick: 24,
                    duration_ticks: 480,
                    key: 65,
                    velocity: 90,
                    channel: 2,
                    slide_ticks: 0,
                }],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    project
}

#[test]
fn deleting_bus_reroutes_sources_preserves_content_and_is_one_undoable_edit() {
    let project = bus_project();
    let Track::Audio(original_audio) = &project.tracks()[2] else {
        panic!()
    };
    let audio_clips = original_audio.clips().to_vec();
    let Track::Midi(original_midi) = &project.tracks()[3] else {
        panic!()
    };
    let midi_clips = original_midi.clips().to_vec();
    let before = encoded(&project);
    let mut document = Document::new(project, None);
    assert_eq!(
        document
            .edit_structural("Delete Bus", |project| edit::delete_track(project, 1))
            .unwrap(),
        1
    );
    assert_eq!(document.project().tracks().len(), 6);
    for name in ["Audio", "MIDI", "Group", "Bus source"] {
        assert_eq!(output_name(document.project(), name), Some("Final bus"));
        let mixer = document.project().tracks()[index(document.project(), name)].mixer();
        assert_eq!(mixer.gain(), 0.8);
        assert_eq!(mixer.pan(), -0.3);
    }
    let Track::Audio(audio) = &document.project().tracks()[1] else {
        panic!()
    };
    let Track::Midi(midi) = &document.project().tracks()[2] else {
        panic!()
    };
    assert_eq!(audio.clips(), audio_clips);
    assert_eq!(midi.clips(), midi_clips);
    let after = encoded(document.project());
    assert_eq!(document.undo().as_deref(), Some("Delete Bus"));
    assert_eq!(encoded(document.project()), before);
    assert!(document.undo().is_none());
    assert_eq!(document.redo().as_deref(), Some("Delete Bus"));
    assert_eq!(encoded(document.project()), after);
}

#[test]
fn deleting_bus_honors_disconnected_output_without_deleting_sources() {
    let mut project = bus_project();
    edit::set_mixer(
        &mut project,
        1,
        MixerSettings::default().with_output(AudioOutput::Disconnected),
    )
    .unwrap();
    edit::delete_track(&mut project, 1).unwrap();
    for name in ["Audio", "MIDI", "Group", "Bus source"] {
        assert_eq!(
            project.tracks()[index(&project, name)].mixer().output(),
            AudioOutput::Disconnected
        );
    }
}

#[test]
fn shared_bus_survives_grouping_ungrouping_and_exiting_one_member() {
    let project = bus_project();
    let original = encoded(&project);
    let mut document = Document::new(project, None);
    let group = document
        .edit_structural("Group", |project| edit::group_tracks(project, &[2, 3]))
        .unwrap();
    let group_name = document.project().tracks()[group].name().to_owned();
    assert_eq!(
        output_name(document.project(), &group_name),
        Some("Mix bus")
    );
    assert_eq!(
        output_name(document.project(), "Audio"),
        Some(group_name.as_str())
    );
    assert_eq!(
        output_name(document.project(), "MIDI"),
        Some(group_name.as_str())
    );
    let grouped = encoded(document.project());
    document
        .edit_structural("Ungroup", |project| edit::ungroup(project, group))
        .unwrap();
    assert_eq!(output_name(document.project(), "Audio"), Some("Mix bus"));
    assert_eq!(output_name(document.project(), "MIDI"), Some("Mix bus"));
    document.undo();
    assert_eq!(encoded(document.project()), grouped);
    document
        .edit_track_order("Exit Group", |project| {
            let audio = index(project, "Audio");
            edit::exit_group(project, audio)
        })
        .unwrap();
    assert_eq!(output_name(document.project(), "Audio"), Some("Mix bus"));
    assert_eq!(
        output_name(document.project(), "MIDI"),
        Some(group_name.as_str())
    );
    assert_eq!(
        output_name(document.project(), &group_name),
        Some("Mix bus")
    );
    assert_eq!(document.project().tracks().len(), 8);
    document.undo();
    assert_eq!(encoded(document.project()), grouped);
    document.undo();
    assert_eq!(encoded(document.project()), original);
}

#[test]
fn sends_follow_their_buses_across_grouping_track_moves_and_undo() {
    let mut project = Project::new();
    let first = project.add_audio_track("First");
    let reverb = project.add_bus_track("Reverb");
    let second = project.add_audio_track("Second");
    let delay = project.add_bus_track("Delay");
    project.set_track_send(first, reverb, 0.3).unwrap();
    project.set_track_send(first, delay, 0.7).unwrap();
    project.set_track_send(second, reverb, 0.5).unwrap();
    let original = encoded(&project);
    let check = |project: &Project| {
        for (name, expected) in [
            ("First", vec![("Reverb", 0.3), ("Delay", 0.7)]),
            ("Second", vec![("Reverb", 0.5)]),
        ] {
            let actual: Vec<_> = project.tracks()[index(project, name)]
                .sends()
                .iter()
                .map(|send| {
                    let bus = project
                        .track_ids()
                        .position(|id| id == TrackId::Bus(send.destination))
                        .unwrap();
                    (project.tracks()[bus].name(), send.gain)
                })
                .collect();
            assert_eq!(actual, expected);
        }
    };
    let mut document = Document::new(project, None);
    let group = document
        .edit_structural("Group", |project| edit::group_tracks(project, &[1, 3]))
        .unwrap();
    check(document.project());
    document
        .edit_track_order("Move", |project| {
            let first = index(project, "First");
            let second = index(project, "Second");
            edit::move_track(project, second, TrackDrop::Before(first))
        })
        .unwrap();
    check(document.project());
    document.undo();
    document
        .edit_structural("Ungroup", |project| edit::ungroup(project, group))
        .unwrap();
    check(document.project());
    assert_eq!(output_name(document.project(), "First"), Some("Master"));
    document.undo();
    document.undo();
    assert_eq!(encoded(document.project()), original);
}

#[test]
fn buses_are_mixer_only_and_do_not_hide_their_input_tracks() {
    let project = bus_project();
    let folded = HashSet::new();
    assert_eq!(
        rows_named(&project, &rows::display_rows(&project, &folded)),
        ["Audio", "MIDI", "Group"]
    );
    assert_eq!(
        rows_named(&project, &rows::mixer_rows(&project, &folded)),
        [
            "Audio",
            "MIDI",
            "Group",
            "Mix bus",
            "Bus source",
            "Final bus"
        ]
    );
    assert!(rows::parents(&project).iter().all(Option::is_none));
    let folded = HashSet::from([1]);
    assert_eq!(
        rows_named(&project, &rows::display_rows(&project, &folded)),
        ["Audio", "MIDI", "Group"]
    );
    assert_eq!(rows::subtree(&project, 1), [1]);
}

#[test]
fn arrangement_reorder_retains_hidden_buses_routes_sidechains_and_undo_mapping() {
    let mut project = bus_project();
    let audio = edit::track_id(&project, 2).unwrap();
    let device = project
        .add_effect(audio, plugin(PluginKind::Effect))
        .unwrap();
    let insert = project
        .add_mixer_effect(audio, plugin(PluginKind::Effect))
        .unwrap();
    let route = SidechainRoute {
        source_track: 3,
        bus_index: 1,
        gain: 0.5,
        tap: SidechainTap::PostFader,
    };
    for id in [device, insert] {
        project.set_plugin_sidechain(id, Some(route)).unwrap();
    }
    let names = project
        .tracks()
        .iter()
        .map(|track| track.name().to_owned())
        .collect::<Vec<_>>();
    let before = encoded(&project);
    let mut document = Document::new(project, None);
    let mapping = document
        .edit_track_order("Move Audio", |project| {
            edit::move_track(project, 2, TrackDrop::After(4))
        })
        .unwrap();
    assert_eq!(mapping.len(), names.len());
    for (old, &new) in mapping.iter().enumerate() {
        assert_eq!(document.project().tracks()[new].name(), names[old]);
    }
    assert_eq!(
        rows_named(
            document.project(),
            &rows::display_rows(document.project(), &HashSet::new())
        ),
        ["MIDI", "Group", "Audio"]
    );
    for name in ["Audio", "MIDI", "Group", "Bus source"] {
        assert_eq!(output_name(document.project(), name), Some("Mix bus"));
    }
    assert_eq!(
        output_name(document.project(), "Mix bus"),
        Some("Final bus")
    );
    let midi_index = index(document.project(), "MIDI");
    for id in [device, insert] {
        assert_eq!(
            document.project().plugin(id).unwrap().sidechain,
            Some(SidechainRoute {
                source_track: midi_index,
                ..route
            })
        );
    }
    assert!(document.undo_track_mapping().is_some());
    let after = encoded(document.project());
    document.undo();
    assert_eq!(encoded(document.project()), before);
    assert_eq!(document.redo_track_mapping(), Some(mapping.as_slice()));
    document.redo();
    assert_eq!(encoded(document.project()), after);
}

#[test]
fn hidden_buses_cannot_be_dragged_or_used_as_arrangement_group_targets() {
    let mut project = bus_project();
    let before = encoded(&project);
    for (source, target) in [
        (1, TrackDrop::After(2)),
        (2, TrackDrop::Before(1)),
        (2, TrackDrop::IntoGroup(1)),
    ] {
        assert!(!edit::can_drop_track(&project, source, target));
        assert!(edit::move_track(&mut project, source, target).is_err());
        assert_eq!(encoded(&project), before);
    }
    assert!(edit::group_tracks(&mut project, &[1, 5]).is_err());
    assert_eq!(encoded(&project), before);
}

#[test]
fn mixer_insert_validation_chain_separation_and_undo_preserve_device_identity() {
    let mut project = bus_project();
    let device = edit::insert_plugin(&mut project, 2, plugin(PluginKind::Effect), None).unwrap();
    let first =
        edit::insert_mixer_plugin(&mut project, 2, plugin(PluginKind::Effect), None).unwrap();
    let original = encoded(&project);
    let mut document = Document::new(project, None);
    let second = document
        .edit("Insert Mixer Effect", |project| {
            edit::insert_mixer_plugin(project, 2, plugin(PluginKind::Effect), Some(first))
        })
        .unwrap();
    let audio = &document.project().tracks()[2];
    assert_eq!(
        audio
            .effects()
            .iter()
            .map(|effect| effect.id)
            .collect::<Vec<_>>(),
        [device]
    );
    assert_eq!(
        audio
            .mixer_effects()
            .iter()
            .map(|effect| effect.id)
            .collect::<Vec<_>>(),
        [second, first]
    );
    let after = encoded(document.project());
    for (kind, before) in [
        (PluginKind::Instrument, None),
        (PluginKind::Effect, Some(device)),
        (PluginKind::Effect, Some(u64::MAX)),
    ] {
        assert!(
            document
                .edit("Invalid insert", |project| edit::insert_mixer_plugin(
                    project,
                    2,
                    plugin(kind),
                    before
                ))
                .is_err()
        );
        assert_eq!(encoded(document.project()), after);
    }
    assert_eq!(document.undo_label(), Some("Insert Mixer Effect"));
    document.undo();
    assert_eq!(encoded(document.project()), original);
    document.redo();
    assert_eq!(encoded(document.project()), after);
}

#[test]
fn bus_device_surface_rejects_plugins_while_mixer_accepts_effects() {
    let mut project = bus_project();
    let original = encoded(&project);
    assert!(edit::insert_plugin(&mut project, 1, plugin(PluginKind::Effect), None).is_err());
    assert!(
        edit::insert_mixer_plugin(&mut project, 1, plugin(PluginKind::Instrument), None).is_err()
    );
    assert_eq!(encoded(&project), original);
    let effect =
        edit::insert_mixer_plugin(&mut project, 1, plugin(PluginKind::Effect), None).unwrap();
    assert!(project.tracks()[1].effects().is_empty());
    assert_eq!(project.tracks()[1].mixer_effects()[0].id, effect);
}
