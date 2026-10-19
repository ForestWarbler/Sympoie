use sympoie::clip::{MidiClip, MidiNote};
use sympoie::project::{Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::time::SnapGrid;
use sympoie::track::{MidiTrackId, TrackId};

fn note() -> MidiNote {
    MidiNote {
        start_tick: 17,
        duration_ticks: 319,
        key: 60,
        velocity: 100,
        channel: 7,
        slide_ticks: 0,
    }
}

fn fixture() -> (Project, MidiTrackId) {
    let mut project = Project::new();
    let track = project.add_midi_track("Keys");
    for start_tick in [11, 4001, 21] {
        project
            .add_midi_clip(
                track,
                MidiClip {
                    offset_ticks: 0,
                    start_tick,
                    length_ticks: 3840,
                    notes: vec![note()],
                    name: None,
                    pitch_bend: Vec::new(),
                },
            )
            .unwrap();
    }
    project.set_snap_grid(SnapGrid::Bar);
    (project, track)
}

#[test]
fn batch_replacement_preserves_local_timing_polyphony_channels_and_clip_identity() {
    let (mut project, track) = fixture();
    let before = project.midi_track(track).unwrap().clips().to_vec();
    let notes = vec![
        MidiNote {
            start_tick: 251,
            key: 64,
            channel: 15,
            ..note()
        },
        note(),
        MidiNote { key: 67, ..note() },
        note(),
        MidiNote {
            start_tick: 18,
            duration_ticks: 1,
            key: 0,
            velocity: 1,
            channel: 0,
            slide_ticks: 0,
        },
        MidiNote {
            start_tick: 3839,
            duration_ticks: 1,
            key: 127,
            velocity: 127,
            channel: 15,
            slide_ticks: 0,
        },
    ];

    project
        .set_midi_clip_notes(track, 1, notes.clone())
        .unwrap();

    let clips = project.midi_track(track).unwrap().clips();
    assert_eq!(clips.len(), before.len());
    assert_eq!(clips[0], before[0]);
    assert_eq!(clips[2], before[2]);
    assert_eq!(clips[1].start_tick, 4001);
    assert_eq!(clips[1].length_ticks, 3840);
    assert_eq!(clips[1].notes, notes);
    assert_eq!(project.snap_grid(), SnapGrid::Bar);
}

#[test]
fn every_invalid_note_rejects_the_whole_batch() {
    let (mut project, track) = fixture();
    let before = project.midi_track(track).unwrap().clips().to_vec();
    let cases = [
        (
            MidiNote { key: 128, ..note() },
            ProjectError::InvalidMidiKey { note: 1, key: 128 },
        ),
        (
            MidiNote {
                velocity: 0,
                ..note()
            },
            ProjectError::InvalidMidiVelocity {
                note: 1,
                velocity: 0,
            },
        ),
        (
            MidiNote {
                velocity: 128,
                ..note()
            },
            ProjectError::InvalidMidiVelocity {
                note: 1,
                velocity: 128,
            },
        ),
        (
            MidiNote {
                channel: 16,
                ..note()
            },
            ProjectError::InvalidMidiChannel {
                note: 1,
                channel: 16,
            },
        ),
        (
            MidiNote {
                duration_ticks: 0,
                ..note()
            },
            ProjectError::EmptyMidiNote { note: 1 },
        ),
        (
            MidiNote {
                start_tick: u64::MAX,
                duration_ticks: 1,
                ..note()
            },
            ProjectError::MidiNoteRangeOverflow { note: 1 },
        ),
    ];
    for (invalid, expected) in cases {
        let replacement = vec![MidiNote { key: 71, ..note() }, invalid];
        assert_eq!(
            project.set_midi_clip_notes(track, 1, replacement),
            Err(expected)
        );
        assert_eq!(project.midi_track(track).unwrap().clips(), before);
    }
}

#[test]
fn unknown_clip_or_track_cannot_change_existing_notes() {
    let (mut project, track) = fixture();
    let before = project.midi_track(track).unwrap().clips().to_vec();
    assert_eq!(
        project.set_midi_clip_notes(track, 3, vec![note()]),
        Err(ProjectError::UnknownMidiClip { track, index: 3 })
    );

    let mut other = Project::new();
    other.add_midi_track("Other keys");
    let missing_track = other.add_midi_track("Not in this project");
    assert_eq!(
        project.set_midi_clip_notes(missing_track, 0, vec![note()]),
        Err(ProjectError::UnknownMidiTrack {
            track: missing_track
        })
    );
    assert_eq!(project.midi_track(track).unwrap().clips(), before);
}

#[test]
fn clearing_notes_keeps_the_clip_and_does_not_mutate_a_snapshot() {
    let (mut project, track) = fixture();
    let snapshot = project.clone();
    project.set_midi_clip_notes(track, 1, Vec::new()).unwrap();
    let clips = project.midi_track(track).unwrap().clips();
    assert!(clips[1].notes.is_empty());
    assert_eq!(clips[1].start_tick, 4001);
    assert_eq!(clips[1].length_ticks, 3840);
    assert_eq!(clips.len(), 3);
    assert_eq!(
        snapshot.midi_track(track).unwrap().clips()[1].notes,
        [note()]
    );
}

#[test]
fn edited_notes_round_trip_and_can_be_edited_after_loading() {
    let (mut project, track) = fixture();
    let notes = vec![
        MidiNote {
            key: 77,
            channel: 14,
            ..note()
        },
        MidiNote {
            start_tick: 29,
            velocity: 1,
            ..note()
        },
    ];
    project.set_midi_clip_notes(track, 1, notes).unwrap();
    project.set_midi_clip_notes(track, 0, Vec::new()).unwrap();
    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    let mut restored = read_project(bytes.as_slice()).unwrap();
    let TrackId::Midi(restored_track) = restored.track_ids().nth(1).unwrap() else {
        panic!("expected MIDI track");
    };
    assert_eq!(
        restored.midi_track(restored_track).unwrap().clips(),
        project.midi_track(track).unwrap().clips()
    );
    let edited = vec![MidiNote {
        duration_ticks: 37,
        ..note()
    }];
    restored
        .set_midi_clip_notes(restored_track, 2, edited.clone())
        .unwrap();
    assert_eq!(
        restored.midi_track(restored_track).unwrap().clips()[2].notes,
        edited
    );
}

#[test]
fn notes_outside_the_shown_region_are_kept_but_silent() {
    use sympoie::instrument::{Instrument, SubtractiveSettings};
    use sympoie::render::render_block;

    let mut project = Project::new();
    let track = project.add_midi_track("Keys");
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    let clip = MidiClip {
        start_tick: 3840,
        length_ticks: 1920,
        offset_ticks: 960,
        notes: vec![
            MidiNote {
                start_tick: 0,
                duration_ticks: 960,
                ..note()
            },
            MidiNote {
                start_tick: 1920,
                duration_ticks: 1920,
                ..note()
            },
            MidiNote {
                start_tick: 4000,
                duration_ticks: 100,
                ..note()
            },
        ],
        name: None,
        pitch_bend: Vec::new(),
    };
    project.add_midi_clip(track, clip.clone()).unwrap();
    assert_eq!(
        clip.audible_notes().collect::<Vec<_>>(),
        [MidiNote {
            start_tick: 960,
            duration_ticks: 960,
            ..note()
        }]
    );
    assert_eq!(clip.content_end(), 4100);

    let frames = |tick: u64| tick * 25;
    let mut heard = [[0.0; 2]; 512];
    render_block(&project, frames(3840 + 960 + 10), &mut heard).unwrap();
    assert!(heard.iter().any(|frame| frame[0].abs() > 1e-4));
    let mut silent = [[1.0; 2]; 512];
    render_block(&project, frames(3840 + 3040), &mut silent).unwrap();
    assert_eq!(silent, [[0.0; 2]; 512]);
}
