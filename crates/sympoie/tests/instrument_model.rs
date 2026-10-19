use sympoie::SAMPLE_RATE;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{ProjectFileError, read_project, write_project};
use sympoie::time::TimeError;
use sympoie::track::{AudioOutput, MixerSettings, Track};

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn settings() -> SubtractiveSettings {
    SubtractiveSettings {
        waveform: Waveform::Square,
        cutoff_hz: 1234.5,
        resonance: 1.25,
        attack_seconds: 0.125,
        decay_seconds: 0.375,
        sustain: 0.625,
        release_seconds: 0.25,
        gain: 0.375,
    }
}

fn note(duration_ticks: u64) -> MidiNote {
    MidiNote {
        start_tick: 0,
        duration_ticks,
        key: 64,
        velocity: 89,
        channel: 11,
        slide_ticks: 0,
    }
}

#[test]
fn assignment_removal_and_snapshots_preserve_notes_and_routing() {
    let mut project = Project::new();
    let track = project.add_midi_track("Keys");
    let group = project.add_group_track("Instruments");
    project.set_track_group(track, Some(group)).unwrap();
    let clip = MidiClip {
        offset_ticks: 0,
        start_tick: 17,
        length_ticks: 960,
        notes: vec![note(480)],
        name: None,
        pitch_bend: Vec::new(),
    };
    project.add_midi_clip(track, clip.clone()).unwrap();
    assert_eq!(project.midi_track(track).unwrap().instrument(), None);

    let synth = Instrument::Subtractive(settings());
    project
        .set_midi_track_instrument(track, Some(synth))
        .unwrap();
    let snapshot = project.clone();
    project.set_midi_track_instrument(track, None).unwrap();
    assert_eq!(project.midi_track(track).unwrap().instrument(), None);
    assert_eq!(
        snapshot.midi_track(track).unwrap().instrument(),
        Some(&synth)
    );
    assert_eq!(project.midi_track(track).unwrap().clips(), [clip]);
    assert_eq!(
        project.midi_track(track).unwrap().mixer().output(),
        AudioOutput::Group(group)
    );
}

#[test]
fn invalid_assignments_leave_previous_instrument_and_project_intact() {
    let mut project = Project::new();
    let track = project.add_midi_track("Keys");
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(settings())))
        .unwrap();
    let original = encoded(&project);
    for invalid in [
        SubtractiveSettings {
            cutoff_hz: f32::NAN,
            ..settings()
        },
        SubtractiveSettings {
            cutoff_hz: 0.0,
            ..settings()
        },
        SubtractiveSettings {
            resonance: 0.0,
            ..settings()
        },
        SubtractiveSettings {
            attack_seconds: -1.0,
            ..settings()
        },
        SubtractiveSettings {
            decay_seconds: -1.0,
            ..settings()
        },
        SubtractiveSettings {
            sustain: 1.01,
            ..settings()
        },
        SubtractiveSettings {
            release_seconds: -1.0,
            ..settings()
        },
        SubtractiveSettings {
            gain: f32::INFINITY,
            ..settings()
        },
    ] {
        assert!(matches!(
            project.set_midi_track_instrument(track, Some(Instrument::Subtractive(invalid))),
            Err(ProjectError::Instrument(_))
        ));
        assert_eq!(encoded(&project), original);
    }
    let mut other = Project::new();
    other.add_midi_track("Other");
    let unknown = other.add_midi_track("Unknown");
    assert_eq!(
        project.set_midi_track_instrument(unknown, None),
        Err(ProjectError::UnknownMidiTrack { track: unknown })
    );
    assert_eq!(encoded(&project), original);
}

#[test]
fn every_waveform_and_parameter_round_trips_in_current_version() {
    let mut project = Project::new();
    for waveform in [Waveform::Sine, Waveform::Saw, Waveform::Square] {
        let track = project.add_midi_track(format!("{waveform:?}"));
        let instrument = Instrument::Subtractive(SubtractiveSettings {
            waveform,
            ..settings()
        });
        project
            .set_midi_track_instrument(track, Some(instrument))
            .unwrap();
        project
            .add_midi_clip(
                track,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 17,
                    length_ticks: 960,
                    notes: vec![note(480)],
                    name: None,
                    pitch_bend: Vec::new(),
                },
            )
            .unwrap();
    }
    project.add_midi_track("Unassigned");
    let bytes = encoded(&project);
    assert_eq!(&bytes[8..12], &16_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(encoded(&restored), bytes);
    for (before, after) in project.tracks().iter().zip(restored.tracks()) {
        if let (Track::Midi(before), Track::Midi(after)) = (before, after) {
            assert_eq!(before.instrument(), after.instrument());
            assert_eq!(before.clips(), after.clips());
        }
    }
}

#[test]
fn independent_version_one_fixture_loads_notes_without_an_instrument() {
    let mut bytes = b"SYMPROJ\0".to_vec();
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&120.0_f64.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&2_u64.to_le_bytes());
    for (kind, name, output) in [(3, "Master", 2), (1, "Keys", 0)] {
        bytes.push(kind);
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.push(output);
    }
    for value in [1_u64, 17, 960, 1, 0, 480] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&[64, 89, 11]);
    let restored = read_project(bytes.as_slice()).unwrap();
    let Track::Midi(track) = &restored.tracks()[1] else {
        panic!("expected MIDI track")
    };
    assert_eq!(track.instrument(), None);
    assert_eq!(track.clips()[0].start_tick, 17);
    assert_eq!(track.clips()[0].notes, [note(480)]);
    assert_eq!(restored.duration_frames().unwrap(), 0);
    assert_eq!(&encoded(&restored)[8..12], &16_u32.to_le_bytes());
}

#[test]
fn invalid_or_truncated_instrument_records_are_rejected() {
    let mut project = Project::new();
    let track = project.add_midi_track("x");
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(settings())))
        .unwrap();
    let original = encoded(&project);
    for (offset, replacement) in [(101, 255), (102, 255)] {
        let mut bytes = original.clone();
        bytes[offset] = replacement;
        assert!(matches!(
            read_project(bytes.as_slice()),
            Err(ProjectFileError::InvalidFormat(_))
        ));
    }
    for parameter in 0..7 {
        let offset = 103 + parameter * 4;
        let mut bytes = original.clone();
        bytes[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(matches!(
            read_project(bytes.as_slice()),
            Err(ProjectFileError::Project(ProjectError::Instrument(_)))
        ));
    }
    for end in 0..original.len() {
        assert!(
            read_project(&original[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
}

#[test]
fn duration_includes_musical_positions_release_tails_and_audio() {
    let mut project = Project::new();
    let track = project.add_midi_track("Keys");
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 960,
                length_ticks: 960,
                notes: vec![note(960)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), 0);
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(settings())))
        .unwrap();
    assert_eq!(project.audio_duration_frames(), 0);
    assert_eq!(project.duration_frames().unwrap(), 48_000 + 12_000);
    project.set_bpm(60.0).unwrap();
    assert_eq!(project.duration_frames().unwrap(), 96_000 + 12_000);
    project
        .set_midi_track_mixer(
            track,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), 108_000);

    let audio = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.0; 2]; 100]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 2400,
                frame_count: 100,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    assert_eq!(project.audio_duration_frames(), 120_100);
    assert_eq!(project.duration_frames().unwrap(), 120_100);
}

#[test]
fn empty_assigned_clips_and_silence_after_early_notes_retain_their_extent() {
    let mut project = Project::new();
    let track = project.add_midi_track("Keys");
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(settings())))
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 3840,
                notes: vec![note(480)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        2 * u64::from(SAMPLE_RATE)
    );
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 3840,
                length_ticks: 3840,
                notes: vec![],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(
        project.duration_frames().unwrap(),
        4 * u64::from(SAMPLE_RATE)
    );
}

#[test]
fn duration_reports_conversion_and_tail_overflow_without_restricting_saved_ticks() {
    let mut project = Project::new();
    let track = project.add_midi_track("Keys");
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: u64::MAX - 4095,
                notes: vec![note(u64::MAX - 4095)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), 0);
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(settings())))
        .unwrap();
    assert_eq!(
        project.duration_frames(),
        Err(TimeError::ConversionOverflow)
    );
    project.set_bpm(3000.0).unwrap();
    let converted_end = project.tempo().ticks_to_frames(u64::MAX - 4095).unwrap();
    assert_eq!(
        project.duration_frames(),
        Err(TimeError::ConversionOverflow)
    );
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings {
                release_seconds: 0.0,
                ..settings()
            })),
        )
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), converted_end);
}
