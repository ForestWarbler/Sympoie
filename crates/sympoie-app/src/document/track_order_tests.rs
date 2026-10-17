use std::collections::HashSet;

use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind, SidechainRoute, SidechainTap};
use sympoie::project::Project;
use sympoie::track::Track;

use super::edit::{self, TrackDrop};
use super::rows;

fn nested_project() -> Project {
    let mut project = Project::new();
    let band = project.add_group_track("Band");
    let drums = project.add_group_track("Drums");
    let kick = project.add_audio_track("Kick");
    let snare = project.add_audio_track("Snare");
    let bass = project.add_midi_track("Bass");
    let other = project.add_group_track("Other");
    let other_child = project.add_audio_track("Other child");
    project.add_audio_track("Outside");
    project.set_track_group(drums, Some(band)).unwrap();
    project.set_track_group(kick, Some(drums)).unwrap();
    project.set_track_group(snare, Some(drums)).unwrap();
    project.set_track_group(bass, Some(band)).unwrap();
    project.set_track_group(other_child, Some(other)).unwrap();
    project
}

fn index(project: &Project, name: &str) -> usize {
    project
        .tracks()
        .iter()
        .position(|track| track.name() == name)
        .unwrap()
}

fn display_names(project: &Project) -> Vec<&str> {
    rows::display_rows(project, &HashSet::new())
        .iter()
        .map(|row| project.tracks()[row.index].name())
        .collect()
}

fn parent_name<'a>(project: &'a Project, track: &str) -> Option<&'a str> {
    rows::parents(project)[index(project, track)].map(|parent| project.tracks()[parent].name())
}

#[test]
fn moving_a_group_moves_its_subtree_and_remaps_every_old_index() {
    let mut project = nested_project();
    let mapping = edit::move_track(&mut project, 2, TrackDrop::After(5)).unwrap();
    assert_eq!(mapping, [0, 1, 3, 4, 5, 2, 6, 7, 8]);
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Bass",
            "Drums",
            "Kick",
            "Snare",
            "Other",
            "Other child",
            "Outside"
        ]
    );
    assert_eq!(parent_name(&project, "Drums"), Some("Band"));
    assert_eq!(parent_name(&project, "Kick"), Some("Drums"));
    assert_eq!(parent_name(&project, "Snare"), Some("Drums"));
    assert_eq!(parent_name(&project, "Bass"), Some("Band"));
    assert_eq!(parent_name(&project, "Other child"), Some("Other"));
}

#[test]
fn ordinary_track_moves_cannot_change_group_membership() {
    let mut project = nested_project();
    let before = format!("{:?}", project.tracks());
    for (track, target) in [
        (3, TrackDrop::Before(5)),
        (3, TrackDrop::After(8)),
        (8, TrackDrop::Before(3)),
        (2, TrackDrop::After(6)),
    ] {
        assert!(!edit::can_drop_track(&project, track, target));
        assert!(edit::move_track(&mut project, track, target).is_err());
        assert_eq!(format!("{:?}", project.tracks()), before);
    }
    assert!(edit::can_drop_track(&project, 4, TrackDrop::Before(3)));
    edit::move_track(&mut project, 4, TrackDrop::Before(3)).unwrap();
    assert_eq!(parent_name(&project, "Kick"), Some("Drums"));
    assert_eq!(parent_name(&project, "Snare"), Some("Drums"));
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Drums",
            "Snare",
            "Kick",
            "Bass",
            "Other",
            "Other child",
            "Outside"
        ]
    );
}

#[test]
fn entering_groups_appends_after_all_existing_children_and_respects_the_current_parent() {
    let mut project = nested_project();
    assert!(edit::can_drop_track(&project, 8, TrackDrop::IntoGroup(2)));
    edit::move_track(&mut project, 8, TrackDrop::IntoGroup(2)).unwrap();
    assert_eq!(parent_name(&project, "Outside"), Some("Drums"));
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Drums",
            "Kick",
            "Snare",
            "Outside",
            "Bass",
            "Other",
            "Other child"
        ]
    );

    let outside = index(&project, "Outside");
    for destination in ["Band", "Other"] {
        let target = TrackDrop::IntoGroup(index(&project, destination));
        assert!(!edit::can_drop_track(&project, outside, target));
        assert!(edit::move_track(&mut project, outside, target).is_err());
        assert_eq!(parent_name(&project, "Outside"), Some("Drums"));
    }

    let bass = index(&project, "Bass");
    let target = TrackDrop::IntoGroup(index(&project, "Drums"));
    assert!(edit::can_drop_track(&project, bass, target));
    edit::move_track(&mut project, bass, target).unwrap();
    assert_eq!(parent_name(&project, "Bass"), Some("Drums"));
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Drums",
            "Kick",
            "Snare",
            "Outside",
            "Bass",
            "Other",
            "Other child"
        ]
    );
}

#[test]
fn dropping_onto_the_current_parent_moves_the_track_to_its_last_child() {
    let mut project = nested_project();
    assert!(edit::can_drop_track(&project, 2, TrackDrop::IntoGroup(1)));
    edit::move_track(&mut project, 2, TrackDrop::IntoGroup(1)).unwrap();
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Bass",
            "Drums",
            "Kick",
            "Snare",
            "Other",
            "Other child",
            "Outside"
        ]
    );
    assert_eq!(parent_name(&project, "Drums"), Some("Band"));
    assert_eq!(parent_name(&project, "Kick"), Some("Drums"));
}

#[test]
fn exiting_a_group_moves_exactly_one_level_after_the_old_parent_subtree() {
    let mut project = nested_project();
    edit::exit_group(&mut project, 3).unwrap();
    assert_eq!(parent_name(&project, "Kick"), Some("Band"));
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Drums",
            "Snare",
            "Kick",
            "Bass",
            "Other",
            "Other child",
            "Outside"
        ]
    );
    let kick = index(&project, "Kick");
    edit::exit_group(&mut project, kick).unwrap();
    assert_eq!(parent_name(&project, "Kick"), None);
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Drums",
            "Snare",
            "Bass",
            "Kick",
            "Other",
            "Other child",
            "Outside"
        ]
    );
}

#[test]
fn exiting_a_nested_group_preserves_and_moves_all_of_its_descendants() {
    let mut project = nested_project();
    let mapping = edit::exit_group(&mut project, 2).unwrap();
    assert_eq!(mapping, [0, 1, 3, 4, 5, 2, 6, 7, 8]);
    assert_eq!(parent_name(&project, "Drums"), None);
    assert_eq!(parent_name(&project, "Kick"), Some("Drums"));
    assert_eq!(parent_name(&project, "Snare"), Some("Drums"));
    assert_eq!(
        display_names(&project),
        [
            "Band",
            "Bass",
            "Drums",
            "Kick",
            "Snare",
            "Other",
            "Other child",
            "Outside"
        ]
    );
}

#[test]
fn rejected_group_drops_and_noops_leave_the_project_untouched() {
    let mut project = nested_project();
    let before = format!("{:?}", project.tracks());
    for (track, target) in [
        (0, TrackDrop::IntoGroup(1)),
        (1, TrackDrop::IntoGroup(1)),
        (1, TrackDrop::IntoGroup(2)),
        (2, TrackDrop::IntoGroup(6)),
        (3, TrackDrop::IntoGroup(4)),
        (8, TrackDrop::Before(0)),
        (usize::MAX, TrackDrop::Before(1)),
        (8, TrackDrop::IntoGroup(usize::MAX)),
    ] {
        assert!(!edit::can_drop_track(&project, track, target));
        assert!(edit::move_track(&mut project, track, target).is_err());
        assert_eq!(format!("{:?}", project.tracks()), before);
    }
    for (track, target) in [
        (3, TrackDrop::Before(3)),
        (3, TrackDrop::Before(4)),
        (4, TrackDrop::After(3)),
        (4, TrackDrop::IntoGroup(2)),
    ] {
        assert!(!edit::can_drop_track(&project, track, target));
        assert_eq!(
            edit::move_track(&mut project, track, target).unwrap(),
            (0..project.tracks().len()).collect::<Vec<_>>()
        );
        assert_eq!(format!("{:?}", project.tracks()), before);
    }
    for track in [0, 1, 8, usize::MAX] {
        assert!(edit::exit_group(&mut project, track).is_err());
        assert_eq!(format!("{:?}", project.tracks()), before);
    }
}

#[test]
fn reordering_an_old_interleaved_project_uses_visible_sibling_order() {
    let mut project = Project::new();
    let kick = project.add_audio_track("Kick");
    let band = project.add_group_track("Band");
    project.add_audio_track("Outside");
    let drums = project.add_group_track("Drums");
    let snare = project.add_audio_track("Snare");
    let bass = project.add_midi_track("Bass");
    project.set_track_group(kick, Some(drums)).unwrap();
    project.set_track_group(snare, Some(drums)).unwrap();
    project.set_track_group(drums, Some(band)).unwrap();
    project.set_track_group(bass, Some(band)).unwrap();
    let mapping = edit::move_track(&mut project, 4, TrackDrop::After(6)).unwrap();
    assert_eq!(mapping, [0, 4, 1, 6, 3, 5, 2]);
    assert_eq!(
        project.tracks().iter().map(Track::name).collect::<Vec<_>>(),
        [
            "Master", "Band", "Bass", "Drums", "Kick", "Snare", "Outside"
        ]
    );
    assert_eq!(parent_name(&project, "Kick"), Some("Drums"));
    assert_eq!(parent_name(&project, "Snare"), Some("Drums"));
    assert_eq!(parent_name(&project, "Drums"), Some("Band"));
}

#[test]
fn group_admission_that_would_create_sidechain_feedback_is_atomic() {
    let mut project = nested_project();
    let outside = edit::track_id(&project, 8).unwrap();
    let effect = project
        .add_effect(
            outside,
            PluginInstance::new(
                PluginDescriptor {
                    path: "/missing/feedback-test.vst3".into(),
                    class_id: "feedback-test".into(),
                    name: "Feedback test".into(),
                    vendor: "Tests".into(),
                    kind: PluginKind::Effect,
                },
                Vec::new(),
                Vec::new(),
            ),
        )
        .unwrap();
    let route = SidechainRoute {
        source_track: 2,
        bus_index: 1,
        tap: SidechainTap::PostFader,
        gain: 0.75,
    };
    project.set_plugin_sidechain(effect, Some(route)).unwrap();
    let before = format!("{:?}", project.tracks());
    assert!(edit::move_track(&mut project, 8, TrackDrop::IntoGroup(2)).is_err());
    assert_eq!(format!("{:?}", project.tracks()), before);
    assert_eq!(project.plugin(effect).unwrap().sidechain, Some(route));
    assert_eq!(parent_name(&project, "Outside"), None);
}
