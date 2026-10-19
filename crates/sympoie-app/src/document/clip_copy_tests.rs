use super::*;
use crate::document::Document;
use sympoie::clip::MidiNote;

#[derive(Debug, PartialEq)]
enum TrackClips {
    Audio(Vec<AudioClip>),
    Midi(Vec<MidiClip>),
    Group,
    Bus,
    Master,
}

fn clips(project: &Project) -> Vec<TrackClips> {
    project
        .tracks()
        .iter()
        .map(|track| match track {
            Track::Audio(track) => TrackClips::Audio(track.clips().to_vec()),
            Track::Midi(track) => TrackClips::Midi(track.clips().to_vec()),
            Track::Group(_) => TrackClips::Group,
            Track::Bus(_) => TrackClips::Bus,
            Track::Master(_) => TrackClips::Master,
        })
        .collect()
}

fn audio_project() -> (Project, ClipRef) {
    let mut project = Project::new();
    project.set_snap_grid(SnapGrid::Off);
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 48_000]).unwrap());
    let track = project.add_audio_track("Source");
    project.add_audio_track("Destination");
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 123,
                start_tick: 987,
                frame_count: 24_000,
                gain: -0.5,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    (project, ClipRef { track: 1, index: 0 })
}

fn midi_project() -> (Project, ClipRef) {
    let mut project = Project::new();
    project.set_snap_grid(SnapGrid::Off);
    let track = project.add_midi_track("Source");
    project.add_midi_track("Destination");
    project
        .add_midi_clip(
            track,
            MidiClip {
                start_tick: 987,
                length_ticks: 960,
                offset_ticks: 480,
                notes: [0, 600, 2000]
                    .into_iter()
                    .map(|start_tick| MidiNote {
                        start_tick,
                        duration_ticks: 120,
                        key: 67,
                        velocity: 91,
                        channel: 3,
                        slide_ticks: 0,
                    })
                    .collect(),
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    (project, ClipRef { track: 1, index: 0 })
}

#[test]
fn copying_audio_preserves_original_source_and_gain_on_same_or_other_track() {
    for to_track in [1, 2] {
        let (mut project, source) = audio_project();
        let original = (audio_clip(&project, source).unwrap()).clone();
        let asset = project.shared_audio_asset(original.asset).unwrap();
        project.set_snap_grid(SnapGrid::Bar);
        let copied = copy_clip_to(&mut project, source, to_track, 1237).unwrap();
        assert_eq!(copied.track, to_track);
        assert_eq!(copied.index, usize::from(to_track == source.track));
        assert_eq!(audio_clip(&project, source), Some(&original));
        assert_eq!(
            audio_clip(&project, copied),
            Some(&AudioClip {
                start_tick: 1237,
                ..original
            })
        );
        assert!(Arc::ptr_eq(
            &asset,
            &project
                .shared_audio_asset(audio_clip(&project, copied).unwrap().asset)
                .unwrap()
        ));
        assert_eq!(project.snap_grid(), SnapGrid::Bar);
    }
}

#[test]
fn copying_midi_preserves_original_offsets_and_hidden_notes() {
    for to_track in [1, 2] {
        let (mut project, source) = midi_project();
        let original = midi_clip(&project, source).unwrap().clone();
        project.set_snap_grid(SnapGrid::Bar);
        let copied = copy_clip_to(&mut project, source, to_track, 1237).unwrap();
        assert_eq!(copied.track, to_track);
        assert_eq!(copied.index, usize::from(to_track == source.track));
        assert_eq!(midi_clip(&project, source), Some(&original));
        assert_eq!(
            midi_clip(&project, copied),
            Some(&MidiClip {
                start_tick: 1237,
                ..original
            })
        );
        assert_eq!(midi_clip(&project, copied).unwrap().notes.len(), 3);
        assert_eq!(
            midi_clip(&project, copied).unwrap().audible_notes().count(),
            1
        );
        assert_eq!(project.snap_grid(), SnapGrid::Bar);
    }
}

#[test]
fn clip_copy_rejects_incompatible_targets_without_changing_document_or_history() {
    for (mut project, source) in [audio_project(), midi_project()] {
        let opposite = match &project.tracks()[source.track] {
            Track::Audio(_) => add_track(&mut project, TrackKind::Midi, None),
            _ => add_track(&mut project, TrackKind::Audio, None),
        };
        let group = add_track(&mut project, TrackKind::Group, None);
        project.set_snap_grid(SnapGrid::Bar);
        let content = copy_clip(&project, source).unwrap();
        let before = clips(&project);
        let mut document = Document::new(project, Some("copy.symp".into()));
        assert!(content.is_compatible(&document.project().tracks()[source.track]));
        for target in [0, opposite, group, usize::MAX] {
            if let Some(track) = document.project().tracks().get(target) {
                assert!(!content.is_compatible(track));
            }
            assert!(
                document
                    .edit_structural("Paste Clip", |project| paste_clip(
                        project, &content, target, 13
                    ))
                    .is_err()
            );
            assert_eq!(clips(document.project()), before);
            assert_eq!(document.project().snap_grid(), SnapGrid::Bar);
            assert!(document.undo_label().is_none());
            assert_eq!(document.revision(), 0);
            assert!(!document.is_dirty());
        }
        assert!(
            document
                .edit_structural("Paste Clip", |project| paste_clip(
                    project,
                    &content,
                    source.track,
                    u64::MAX
                ))
                .is_err()
        );
        assert_eq!(clips(document.project()), before);
        assert!(document.undo_label().is_none());
    }
}

#[test]
fn copying_a_clip_is_one_undoable_step() {
    for (project, source) in [audio_project(), midi_project()] {
        let before = clips(&project);
        let mut document = Document::new(project, Some("copy.symp".into()));
        document
            .edit_structural("Copy Clip", |project| copy_clip_to(project, source, 2, 13))
            .unwrap();
        let after = clips(document.project());
        assert_ne!(before, after);
        assert_eq!(document.undo_label(), Some("Copy Clip"));
        assert!(document.undo_invalidates_clip_refs());
        document.undo();
        assert_eq!(clips(document.project()), before);
        assert!(document.undo_label().is_none());
        assert!(!document.is_dirty());
        document.redo();
        assert_eq!(clips(document.project()), after);
        assert!(document.redo_label().is_none());
    }
}

#[test]
fn copied_audio_survives_undo_and_reuse_of_its_asset_id() {
    let mut project = Project::new();
    project.add_audio_track("Audio");
    let mut document = Document::new(project, Some("copy.symp".into()));
    let source = document
        .edit_structural("Import", |project| {
            place_audio(
                project,
                AudioBuffer::new(vec![[0.25; 2]; 100]).unwrap(),
                "Original",
                Some(1),
                0,
            )
        })
        .unwrap();
    let content = copy_clip(document.project(), source).unwrap();
    let ClipContent::Audio { clip, asset } = &content else {
        unreachable!()
    };
    let original_id = clip.asset;
    document.undo();
    document
        .edit_structural("Different import", |project| {
            place_audio(
                project,
                AudioBuffer::new(vec![[0.75; 2]; 100]).unwrap(),
                "Replacement",
                Some(1),
                0,
            )
        })
        .unwrap();
    assert_eq!(
        audio_clip(document.project(), source).unwrap().asset,
        original_id
    );
    for start in [100, 200] {
        let pasted = document
            .edit_structural("Paste Clip", |project| {
                paste_clip(project, &content, 1, start)
            })
            .unwrap();
        let pasted = audio_clip(document.project(), pasted).unwrap();
        assert_ne!(pasted.asset, original_id);
        assert_eq!(pasted.start_tick, start);
        let pasted_asset = document.project().shared_audio_asset(pasted.asset).unwrap();
        assert!(Arc::ptr_eq(asset, &pasted_asset));
        assert_eq!(pasted_asset.samples()[0], [0.25; 2]);
        if start == 200 {
            let first_copy =
                audio_clip(document.project(), ClipRef { track: 1, index: 1 }).unwrap();
            assert_eq!(first_copy.asset, pasted.asset);
        }
    }
    let mut other_project = Project::new();
    other_project.add_audio_track("New project");
    let pasted = paste_clip(&mut other_project, &content, 1, 300).unwrap();
    let pasted = audio_clip(&other_project, pasted).unwrap();
    assert!(Arc::ptr_eq(
        asset,
        &other_project.shared_audio_asset(pasted.asset).unwrap()
    ));
}
