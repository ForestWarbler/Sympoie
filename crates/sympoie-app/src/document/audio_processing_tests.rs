use std::collections::BTreeSet;

use super::{Document, edit};
use edit::ClipRef;
use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::pitch::PitchAlgorithm;
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::render_block;
use sympoie::time::SnapGrid;
use sympoie::track::Track;

fn source_samples() -> Vec<Frame> {
    (0..12)
        .map(|index| [index as f32 / 16.0, -(index as f32 + 1.0) / 32.0])
        .collect()
}

fn mixed_project() -> (Project, ClipRef) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let asset = project.add_asset(AudioBuffer::new(source_samples()).unwrap());
    let audio = project.add_audio_track("Audio");
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 2,
                start_tick: 10,
                frame_count: 8,
                gain: 0.5,
                pitch_semitones: 0.0,
                pitch_algorithm: PitchAlgorithm::Builtin,
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    let midi = project.add_midi_track("MIDI");
    project
        .add_midi_clip(
            midi,
            MidiClip {
                start_tick: 9,
                length_ticks: 10,
                offset_ticks: 3,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 20,
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
    project.add_group_track("Group");
    (project, ClipRef { track: 1, index: 0 })
}

fn audio_clips(project: &Project) -> Vec<AudioClip> {
    let Track::Audio(track) = &project.tracks()[1] else {
        panic!("expected audio track");
    };
    track.clips().to_vec()
}

fn midi_clips(project: &Project) -> Vec<MidiClip> {
    let Track::Midi(track) = &project.tracks()[2] else {
        panic!("expected MIDI track");
    };
    track.clips().to_vec()
}

fn render_audio(project: &Project) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; 10];
    render_block(project, 9, &mut output).unwrap();
    output
}

#[test]
fn reverse_range_isolates_exact_audio_frames_and_preserves_midi_and_hidden_data() {
    let (mut project, source) = mixed_project();
    let original = (edit::audio_clip(&project, source).unwrap()).clone();
    let original_midi = midi_clips(&project);
    let source_data = source_samples();
    project.set_snap_grid(SnapGrid::Bar);
    let tracks = BTreeSet::from([0, 1, 2, 3]);

    let changed = edit::reverse_range(&mut project, &tracks, 12, 16).unwrap();

    assert_eq!(changed.len(), 1);
    assert_eq!(project.snap_grid(), SnapGrid::Bar);
    assert_eq!(midi_clips(&project), original_midi);
    let mut clips = audio_clips(&project);
    clips.sort_by_key(|clip| clip.start_tick);
    assert_eq!(
        clips
            .iter()
            .map(|clip| (clip.start_tick, clip.source_start, clip.frame_count))
            .collect::<Vec<_>>(),
        [(10, 2, 2), (12, 4, 4), (16, 8, 2)]
    );
    assert_eq!(edit::audio_clip(&project, changed[0]), Some(&clips[1]));
    assert_eq!(clips[0].asset, original.asset);
    assert_eq!(clips[2].asset, original.asset);
    assert_ne!(clips[1].asset, original.asset);
    let mut reversed_source = source_data.clone();
    reversed_source[4..8].reverse();
    assert_eq!(
        project.audio_asset(clips[1].asset).unwrap().samples(),
        reversed_source
    );
    assert_eq!(
        project.audio_asset(original.asset).unwrap().samples(),
        source_data
    );
    let audible = render_audio(&project);
    assert_eq!(audible[0], [0.0; 2]);
    assert_eq!(audible[9], [0.0; 2]);
    for (frame, index) in audible[1..9].iter().zip([2, 3, 7, 6, 5, 4, 8, 9]) {
        assert_eq!(
            *frame,
            [source_data[index][0] * 0.5, source_data[index][1] * 0.5]
        );
    }

    let expanded = edit::trim_start(&mut project, changed[0], 8).unwrap();
    edit::trim_end(&mut project, expanded, 20).unwrap();
    let expanded = edit::audio_clip(&project, expanded).unwrap();
    assert_eq!((expanded.source_start, expanded.frame_count), (0, 12));
    assert_eq!(
        project.audio_asset(expanded.asset).unwrap().samples(),
        reversed_source
    );
}

#[test]
fn reversing_a_range_is_one_undoable_step_and_repeating_restores_audio() {
    let (project, _) = mixed_project();
    let original_clips = audio_clips(&project);
    let original_audio = render_audio(&project);
    let mut document = Document::new(project, Some("reverse.symp".into()));
    let tracks = BTreeSet::from([1, 2]);
    document
        .edit_structural("Reverse", |project| {
            edit::reverse_range(project, &tracks, 12, 16)
        })
        .unwrap();
    let reversed_clips = audio_clips(document.project());
    let reversed_audio = render_audio(document.project());
    assert_ne!(reversed_audio, original_audio);
    assert_eq!(document.undo_label(), Some("Reverse"));
    assert!(document.undo_invalidates_clip_refs());

    assert_eq!(document.undo().as_deref(), Some("Reverse"));
    assert_eq!(audio_clips(document.project()), original_clips);
    assert_eq!(render_audio(document.project()), original_audio);
    assert!(!document.is_dirty());
    assert!(document.undo_label().is_none());
    assert!(document.redo_invalidates_clip_refs());

    assert_eq!(document.redo().as_deref(), Some("Reverse"));
    assert_eq!(audio_clips(document.project()), reversed_clips);
    assert_eq!(render_audio(document.project()), reversed_audio);
    document
        .edit_structural("Reverse Again", |project| {
            edit::reverse_range(project, &tracks, 12, 16)
        })
        .unwrap();
    assert_eq!(audio_clips(document.project()).len(), 3);
    assert_eq!(render_audio(document.project()), original_audio);
}

#[test]
fn audio_processing_rejects_empty_or_midi_only_ranges_without_an_undo_step() {
    let (project, _) = mixed_project();
    let original_clips = audio_clips(&project);
    let original_midi = midi_clips(&project);
    let mut document = Document::new(project, Some("reverse.symp".into()));
    for (tracks, start, end) in [
        (BTreeSet::from([0, 2, 3]), 12, 16),
        (BTreeSet::from([1, 2]), 20, 24),
        (BTreeSet::from([1]), 12, 12),
        (BTreeSet::from([1]), 16, 12),
    ] {
        assert!(!edit::range_has_audio(
            document.project(),
            &tracks,
            start,
            end
        ));
        assert!(
            document
                .edit_structural("Reverse", |project| {
                    edit::reverse_range(project, &tracks, start, end)
                })
                .is_err()
        );
        assert_eq!(audio_clips(document.project()), original_clips);
        assert_eq!(midi_clips(document.project()), original_midi);
        assert!(document.undo_label().is_none());
        assert_eq!(document.revision(), 0);
        assert!(!document.is_dirty());
    }
}

#[test]
fn clip_pitch_survives_copy_split_trim_reverse_and_document_history() {
    let (project, source) = mixed_project();
    let original = (edit::audio_clip(&project, source).unwrap()).clone();
    let mut document = Document::new(project, Some("pitch.symp".into()));
    assert!(
        document
            .edit("Pitch", |project| edit::set_clip_pitch(
                project,
                source,
                f32::NAN
            ))
            .is_err()
    );
    assert_eq!(
        edit::audio_clip(document.project(), source),
        Some(&original)
    );
    assert!(document.undo_label().is_none());

    document
        .edit("Pitch", |project| {
            edit::set_clip_pitch(project, source, 7.5)?;
            edit::set_clip_pitch_options(project, source, PitchAlgorithm::RubberBandR3, true)
        })
        .unwrap();
    let pitched = AudioClip {
        pitch_semitones: 7.5,
        pitch_algorithm: PitchAlgorithm::RubberBandR3,
        preserve_formants: true,
        ..original.clone()
    };
    assert_eq!(edit::audio_clip(document.project(), source), Some(&pitched));
    let edited = document
        .edit_structural("Copy and Trim", |project| {
            let copied = edit::copy_clip_to(project, source, source.track, 30)?;
            let right = edit::split_clip(project, copied, 34)?;
            let trimmed = edit::trim_start(project, right, 35)?;
            edit::trim_end(project, trimmed, 37)?;
            edit::reverse_clip(project, trimmed)?;
            Ok(trimmed)
        })
        .unwrap();
    let clips_after = audio_clips(document.project());
    assert_eq!(clips_after.len(), 3);
    assert!(clips_after.iter().all(|clip| clip.pitch_semitones == 7.5));
    assert!(clips_after.iter().all(|clip| {
        clip.pitch_algorithm == PitchAlgorithm::RubberBandR3 && clip.preserve_formants
    }));
    assert_eq!(edit::audio_clip(document.project(), source), Some(&pitched));
    let trimmed = edit::audio_clip(document.project(), edited).unwrap();
    assert_eq!(
        (
            trimmed.start_tick,
            trimmed.source_start,
            trimmed.frame_count,
            trimmed.gain
        ),
        (35, 7, 2, 0.5)
    );

    document.undo();
    assert_eq!(
        audio_clips(document.project()),
        std::slice::from_ref(&pitched)
    );
    document.undo();
    assert_eq!(audio_clips(document.project()), [original]);
    assert!(!document.is_dirty());
    document.redo();
    assert_eq!(audio_clips(document.project()), [pitched]);
    document.redo();
    assert_eq!(audio_clips(document.project()), clips_after);
}

#[test]
fn pitch_options_are_undoable_at_zero_pitch_without_changing_source_or_duration() {
    let (project, source) = mixed_project();
    let original = (edit::audio_clip(&project, source).unwrap()).clone();
    let mut document = Document::new(project, Some("options.symp".into()));
    for (algorithm, preserve_formants) in [
        (PitchAlgorithm::Signalsmith, true),
        (PitchAlgorithm::RubberBandR3, false),
    ] {
        document
            .edit("Pitch Options", |project| {
                edit::set_clip_pitch_options(project, source, algorithm, preserve_formants)
            })
            .unwrap();
        let expected = AudioClip {
            pitch_algorithm: algorithm,
            preserve_formants,
            ..original.clone()
        };
        assert_eq!(
            edit::audio_clip(document.project(), source),
            Some(&expected)
        );
        assert!(document.is_dirty());
        assert!(!document.undo_invalidates_clip_refs());
        document.undo();
        assert_eq!(
            edit::audio_clip(document.project(), source),
            Some(&original)
        );
        assert!(!document.is_dirty());
        document.redo();
        assert_eq!(
            edit::audio_clip(document.project(), source),
            Some(&expected)
        );
        document.undo();
    }
    assert!(
        document
            .edit("Invalid Options", |project| {
                edit::set_clip_pitch_options(
                    project,
                    ClipRef { track: 2, index: 0 },
                    PitchAlgorithm::Signalsmith,
                    true,
                )
            })
            .is_err()
    );
    assert!(document.undo_label().is_none());
}
