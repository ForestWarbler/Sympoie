use super::*;
use crate::actions::Action;
use crate::app::Startup;
use crate::document::Document;

#[test]
fn midi_capture_preserves_channels_velocity_sustain_and_held_notes() {
    let mut midi = MidiPerformance::default();
    midi.event(0, 1000, [0x92, 60, 99]);
    midi.event(0, 2000, [0xb2, 64, 127]);
    midi.event(0, 3000, [0x92, 60, 0]);
    midi.event(1, 4000, [0x92, 60, 75]);
    midi.event(0, 5000, [0xb2, 64, 0]);
    let notes = midi.finish(6000, Tempo::new(120.0).unwrap());
    assert_eq!(
        notes,
        vec![
            MidiNote {
                start_tick: 40,
                duration_ticks: 160,
                key: 60,
                velocity: 99,
                channel: 2
            },
            MidiNote {
                start_tick: 160,
                duration_ticks: 80,
                key: 60,
                velocity: 75,
                channel: 2
            },
        ]
    );
}

#[test]
fn repeated_notes_and_all_notes_off_cannot_leave_stuck_notes() {
    let mut midi = MidiPerformance::default();
    midi.event(0, 0, [0x90, 64, 100]);
    midi.event(0, 100, [0x90, 64, 80]);
    midi.event(0, 200, [0xb0, 123, 0]);
    midi.event(0, 300, [0x80, 64, 0]);
    midi.event(0, 500, [0x90, 67, 127]);
    let notes = midi.finish(400, Tempo::new(120.0).unwrap());
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].duration_ticks, 4);
    assert_eq!(notes[1].start_tick, 4);
    assert_eq!(notes[1].duration_ticks, 4);
}

#[test]
fn one_take_records_each_kind_without_snapping_and_undoes_atomically() {
    let mut project = Project::new();
    project.add_audio_track("Audio 1");
    project.add_midi_track("MIDI");
    project.add_audio_track("Audio 2");
    project.set_snap_grid(SnapGrid::Quarter);
    let mut document = Document::new(project, None);
    let samples = vec![[0.25, -0.125]; 9000];
    let notes = vec![MidiNote {
        start_tick: 13,
        duration_ticks: 39,
        key: 61,
        velocity: 77,
        channel: 4,
    }];
    let clips = document
        .edit_structural("Record", |project| {
            commit_take(
                project,
                37,
                360,
                BTreeMap::from([(1, samples.clone()), (3, vec![[0.5; 2]; 9000])]),
                BTreeMap::from([(2, notes.clone())]),
            )
        })
        .unwrap();
    assert_eq!(clips.len(), 3);
    let Track::Audio(first) = &document.project().tracks()[1] else {
        panic!()
    };
    let first = first.clips()[0];
    assert_eq!(first.start_tick, 37);
    assert_eq!(first.frame_count, 9000);
    assert_eq!(
        document
            .project()
            .audio_asset(first.asset)
            .unwrap()
            .samples(),
        samples
    );
    let Track::Audio(second) = &document.project().tracks()[3] else {
        panic!()
    };
    assert_ne!(second.clips()[0].asset, first.asset);
    assert_eq!(
        document
            .project()
            .audio_asset(second.clips()[0].asset)
            .unwrap()
            .samples(),
        vec![[0.5; 2]; 9000]
    );
    let Track::Midi(midi) = &document.project().tracks()[2] else {
        panic!()
    };
    assert_eq!(midi.clips()[0].start_tick, 37);
    assert_eq!(midi.clips()[0].notes, notes);
    assert_eq!(document.project().snap_grid(), SnapGrid::Quarter);
    assert_eq!(document.undo(), Some("Record".to_owned()));
    assert!(document.project().tracks().iter().all(|track| match track {
        Track::Audio(t) => t.clips().is_empty(),
        Track::Midi(t) => t.clips().is_empty(),
        _ => true,
    }));
    assert_eq!(document.redo(), Some("Record".to_owned()));
    let path = std::env::temp_dir().join(format!("sympoie-recording-{}.symp", std::process::id()));
    document.project().save(&path).unwrap();
    let restored = Project::load(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        restored.audio_asset(first.asset).unwrap().samples(),
        samples
    );
    let Track::Midi(midi) = &restored.tracks()[2] else {
        panic!()
    };
    assert_eq!(midi.clips()[0].notes, notes);
}

#[test]
fn record_requires_an_armed_audio_or_midi_track() {
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    app.toggle_recording();
    assert!(app.recording.is_none());
    app.toggle_track_arm(0);
    assert!(app.armed_tracks.is_empty());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .text
            .contains("Enable recording")
    );
}

#[test]
fn confirmation_dialog_cannot_start_a_new_take() {
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    app.apply(&ctx, Action::AddTrack(edit::TrackKind::Midi));
    app.toggle_track_arm(1);
    app.pending = Some(crate::app::Pending::Quit);
    app.toggle_recording();
    assert!(app.recording.is_none());
    assert!(app.notice.as_ref().unwrap().text.contains("confirmation"));
}

#[test]
fn captured_audio_and_midi_are_drained_before_stop_commits_the_take() {
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    app.apply(&ctx, Action::AddTrack(edit::TrackKind::Audio));
    app.apply(&ctx, Action::AddTrack(edit::TrackKind::Midi));
    app.toggle_track_arm(1);
    app.toggle_track_arm(2);
    app.toggle_recording();
    let until = Instant::now() + Duration::from_secs(2);
    while !app.recording.as_ref().unwrap().clock.is_started() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(app.recording.as_ref().unwrap().clock.is_started());
    let take = app.recording.as_mut().unwrap();
    take.input.inject(CaptureEvent::Audio {
        track: 1,
        offset: 30,
        frames: vec![[0.3, -0.2]; 100],
    });
    take.input.inject(CaptureEvent::Midi {
        track: 2,
        offset: 25,
        source: 0,
        message: [0x95, 60, 103],
    });
    take.input.inject(CaptureEvent::Midi {
        track: 2,
        offset: 125,
        source: 0,
        message: [0x85, 60, 0],
    });
    std::thread::sleep(Duration::from_millis(10));
    app.finish_recording();
    let Track::Audio(audio) = &app.document.project().tracks()[1] else {
        panic!()
    };
    let samples = app
        .document
        .project()
        .audio_asset(audio.clips()[0].asset)
        .unwrap()
        .samples();
    assert_eq!(&samples[..30], &[[0.0; 2]; 30]);
    assert_eq!(&samples[30..130], &[[0.3, -0.2]; 100]);
    let Track::Midi(midi) = &app.document.project().tracks()[2] else {
        panic!()
    };
    assert_eq!(
        midi.clips()[0].notes,
        [MidiNote {
            start_tick: 1,
            duration_ticks: 4,
            key: 60,
            velocity: 103,
            channel: 5,
        }]
    );
    assert_eq!(app.document.undo_label(), Some("Record"));
}

#[test]
fn recording_starts_on_empty_timeline_and_stop_commits_then_undo_removes_take() {
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    app.apply(&ctx, Action::AddTrack(edit::TrackKind::Midi));
    app.arrangement.playback_start = 73;
    app.toggle_track_arm(1);
    app.loop_region = LoopRegion {
        start: 0,
        end: 960,
        enabled: true,
    };
    app.toggle_recording();
    assert!(app.recording.is_some());
    assert!(!app.loop_region.enabled);
    let until = Instant::now() + Duration::from_secs(2);
    while !app.recording.as_ref().unwrap().clock.is_started() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(app.recording.as_ref().unwrap().start_tick(), Some(73));
    std::thread::sleep(Duration::from_millis(20));
    app.apply(&ctx, Action::Stop);
    assert!(app.recording.is_none());
    assert!(app.loop_region.enabled);
    let Track::Midi(track) = &app.document.project().tracks()[1] else {
        panic!()
    };
    assert_eq!(track.clips().len(), 1);
    assert_eq!(track.clips()[0].start_tick, 73);
    assert!(track.clips()[0].length_ticks > 0);
    assert_eq!(app.document.undo_label(), Some("Record"));
    app.apply(&ctx, Action::Undo);
    let Track::Midi(track) = &app.document.project().tracks()[1] else {
        panic!()
    };
    assert!(track.clips().is_empty());
}

#[test]
fn simultaneous_tracks_keep_their_own_audio_midi_and_input_meters() {
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    for kind in [
        edit::TrackKind::Audio,
        edit::TrackKind::Audio,
        edit::TrackKind::Midi,
        edit::TrackKind::Midi,
    ] {
        app.apply(&ctx, Action::AddTrack(kind));
    }
    for track in 1..=4 {
        app.toggle_track_arm(track);
    }
    app.toggle_recording();
    let until = Instant::now() + Duration::from_secs(2);
    while !app.recording.as_ref().unwrap().clock.is_started() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    let take = app.recording.as_mut().unwrap();
    assert!(take.clock.is_started());
    for (track, sample) in [(1, [0.2; 2]), (2, [0.4, -0.3]), (99, [1.0; 2])] {
        take.input.inject(CaptureEvent::Audio {
            track,
            offset: 0,
            frames: vec![sample; 100],
        });
    }
    for (track, source, key, channel) in [(3, 0, 60, 0), (4, 1, 67, 3), (99, 2, 90, 0)] {
        take.input.inject(CaptureEvent::Midi {
            track,
            offset: 0,
            source,
            message: [0x90 | channel, key, 100],
        });
        take.input.inject(CaptureEvent::Midi {
            track,
            offset: 100,
            source,
            message: [0x80 | channel, key, 0],
        });
    }
    assert_eq!(app.drain_recording(false), None);
    let take = app.recording.as_ref().unwrap();
    assert_eq!(take.input_peaks.get(&1), Some(&[0.2; 2]));
    assert_eq!(take.input_peaks.get(&2), Some(&[0.4, 0.3]));
    assert!(!take.input_peaks.contains_key(&99));
    std::thread::sleep(Duration::from_millis(10));
    app.finish_recording();
    for (track, sample) in [(1, [0.2; 2]), (2, [0.4, -0.3])] {
        let Track::Audio(audio) = &app.document.project().tracks()[track] else {
            panic!()
        };
        let samples = app
            .document
            .project()
            .audio_asset(audio.clips()[0].asset)
            .unwrap()
            .samples();
        assert_eq!(&samples[..100], vec![sample; 100]);
    }
    for (track, key, channel) in [(3, 60, 0), (4, 67, 3)] {
        let Track::Midi(midi) = &app.document.project().tracks()[track] else {
            panic!()
        };
        assert_eq!(midi.clips()[0].notes.len(), 1);
        assert_eq!(midi.clips()[0].notes[0].key, key);
        assert_eq!(midi.clips()[0].notes[0].channel, channel);
    }
}

#[test]
fn input_selection_is_an_undoable_track_edit() {
    use sympoie::track::{AudioInputChannels, AudioRecordingInput, MidiRecordingInput};
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    app.apply(&ctx, Action::AddTrack(edit::TrackKind::Audio));
    app.apply(&ctx, Action::AddTrack(edit::TrackKind::Midi));
    let input = AudioRecordingInput {
        device: Some("interface".into()),
        channels: AudioInputChannels::Mono(1),
    };
    app.apply(
        &ctx,
        Action::SetAudioRecordingInput {
            track: 1,
            input: input.clone(),
        },
    );
    let Track::Audio(audio) = &app.document.project().tracks()[1] else {
        panic!()
    };
    assert_eq!(audio.recording_input(), &input);
    app.apply(&ctx, Action::Undo);
    let Track::Audio(audio) = &app.document.project().tracks()[1] else {
        panic!()
    };
    assert_eq!(audio.recording_input(), &AudioRecordingInput::default());
    app.apply(&ctx, Action::Redo);
    let Track::Audio(audio) = &app.document.project().tracks()[1] else {
        panic!()
    };
    assert_eq!(audio.recording_input(), &input);
    let input = MidiRecordingInput {
        device: Some("keyboard".into()),
        channel: Some(4),
    };
    app.apply(
        &ctx,
        Action::SetMidiRecordingInput {
            track: 2,
            input: input.clone(),
        },
    );
    let Track::Midi(midi) = &app.document.project().tracks()[2] else {
        panic!()
    };
    assert_eq!(midi.recording_input(), &input);
}
