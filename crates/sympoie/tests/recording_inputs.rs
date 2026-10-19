use std::sync::Arc;

use sympoie::bounce::{BounceMode, BouncePlan, apply_bounce};
use sympoie::clip::AudioClip;
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{read_project, write_project};
use sympoie::track::{AudioInputChannels, AudioRecordingInput, MidiRecordingInput, Track, TrackId};
use sympoie::transport::{PlaybackState, Transport};

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn audio_input(channels: AudioInputChannels) -> AudioRecordingInput {
    AudioRecordingInput {
        device: Some("CoreAudio:话筒:stable-key".into()),
        channels,
    }
}

#[test]
fn recording_input_selections_round_trip_and_follow_tracks_after_reordering() {
    let mut project = Project::new();
    let mono = project.add_audio_track("Mono");
    let stereo = project.add_audio_track("Stereo");
    let midi = project.add_midi_track("Keys");
    project.add_group_track("Group");
    let mono_input = audio_input(AudioInputChannels::Mono(4));
    let stereo_input = audio_input(AudioInputChannels::Stereo(2, 3));
    let midi_input = MidiRecordingInput {
        device: Some("CoreMIDI:keyboard:unique-port-id".into()),
        channel: Some(15),
    };
    project
        .set_audio_recording_input(mono, mono_input.clone())
        .unwrap();
    project
        .set_audio_recording_input(stereo, stereo_input.clone())
        .unwrap();
    project
        .set_midi_recording_input(midi, midi_input.clone())
        .unwrap();
    let snapshot = project.clone();
    project.reorder_tracks(&[0, 3, 4, 2, 1]).unwrap();

    let bytes = encoded(&project);
    assert_eq!(&bytes[8..12], &16_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(bytes, encoded(&restored));
    let [
        Track::Master(_),
        Track::Midi(keys),
        Track::Group(_),
        Track::Audio(stereo),
        Track::Audio(mono),
    ] = restored.tracks()
    else {
        panic!("track order changed");
    };
    assert_eq!(mono.recording_input(), &mono_input);
    assert_eq!(stereo.recording_input(), &stereo_input);
    assert_eq!(keys.recording_input(), &midi_input);
    assert_eq!(snapshot.tracks()[1].name(), "Mono");
}

#[test]
fn invalid_input_settings_leave_previous_selection_untouched() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    for channels in [
        AudioInputChannels::Mono(u16::MAX),
        AudioInputChannels::Stereo(0, u16::MAX),
        AudioInputChannels::Stereo(1, 1),
    ] {
        assert!(
            project
                .set_audio_recording_input(audio, audio_input(channels))
                .is_err()
        );
    }
    assert!(
        project
            .set_midi_recording_input(
                midi,
                MidiRecordingInput {
                    channel: Some(16),
                    ..Default::default()
                }
            )
            .is_err()
    );
    for device in [String::new(), "a\0b".into(), "x".repeat(4097)] {
        assert!(
            project
                .set_audio_recording_input(
                    audio,
                    AudioRecordingInput {
                        device: Some(device.clone()),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert!(
            project
                .set_midi_recording_input(
                    midi,
                    MidiRecordingInput {
                        device: Some(device),
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
    assert_eq!(
        project.audio_track(audio).unwrap().recording_input(),
        &AudioRecordingInput::default()
    );
    assert_eq!(
        project.midi_track(midi).unwrap().recording_input(),
        &MidiRecordingInput::default()
    );
}

fn legacy_project(version: u32) -> Vec<u8> {
    let mut bytes = b"SYMPROJ\0".to_vec();
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&120.0_f64.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.push(0);
    if version >= 6 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&4_u64.to_le_bytes());
    for (tag, name) in [(3, "Master"), (0, "Audio"), (1, "MIDI"), (2, "Group")] {
        bytes.push(tag);
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.push(if tag == 3 { 2 } else { 0 });
        if version >= 5 {
            bytes.push(0);
        }
        if version >= 6 {
            bytes.push(0);
        }
        if tag == 1 && version >= 2 {
            bytes.push(0);
        }
        if tag < 2 {
            bytes.extend_from_slice(&0_u64.to_le_bytes());
        }
    }
    if version >= 3 {
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        for _ in 0..4 {
            bytes.push(0);
            bytes.extend_from_slice(&0_u64.to_le_bytes());
        }
    }
    if version >= 4 {
        bytes.extend_from_slice(&0_u64.to_le_bytes());
    }
    if version >= 7 {
        bytes.extend_from_slice(&[0; 5]);
    }
    if version >= 8 {
        bytes.extend_from_slice(&[0; 32]);
    }
    bytes
}

#[test]
fn all_previous_file_versions_use_centered_default_audio_and_all_midi_inputs() {
    for version in 1..=10 {
        let loaded = read_project(legacy_project(version).as_slice()).unwrap();
        let [_, Track::Audio(audio), Track::Midi(midi), _] = loaded.tracks() else {
            panic!("historical track kind changed");
        };
        assert_eq!(audio.recording_input(), &AudioRecordingInput::default());
        assert_eq!(
            audio.recording_input().channels,
            AudioInputChannels::Mono(0)
        );
        assert_eq!(midi.recording_input(), &MidiRecordingInput::default());
        assert_eq!(
            encoded(&loaded),
            encoded(&read_project(encoded(&loaded).as_slice()).unwrap())
        );
    }
}

#[test]
fn malformed_recording_extensions_are_rejected() {
    let mut prefix = legacy_project(10);
    prefix[8..12].copy_from_slice(&11_u32.to_le_bytes());
    let valid = [0, 0, 0, 0, 0, 0];
    for length in 0..valid.len() {
        let mut bytes = prefix.clone();
        bytes.extend_from_slice(&valid[..length]);
        assert!(read_project(bytes.as_slice()).is_err());
    }
    for extension in [
        vec![2, 0, 0, 0, 0, 0],
        vec![0, 2, 0, 0, 0, 0],
        vec![0, 0, 255, 255, 0, 0],
        vec![0, 1, 2, 0, 2, 0, 0, 0],
        vec![0, 0, 0, 0, 2, 0],
        vec![0, 0, 0, 0, 0, 2],
        vec![0, 0, 0, 0, 0, 1, 16],
    ] {
        let mut bytes = prefix.clone();
        bytes.extend_from_slice(&extension);
        assert!(
            read_project(bytes.as_slice()).is_err(),
            "accepted {extension:?}"
        );
    }
    for key in [vec![], vec![0], vec![255], vec![b'x'; 4097]] {
        let mut bytes = prefix.clone();
        bytes.push(1);
        bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&valid[1..]);
        assert!(read_project(bytes.as_slice()).is_err());
    }
    let mut huge_key = prefix;
    huge_key.push(1);
    huge_key.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(read_project(huge_key.as_slice()).is_err());
}

#[test]
fn recording_routing_edits_preserve_pitch_cache_and_running_transport() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 4096]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 4096,
                gain: 1.0,
                pitch_semitones: 3.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    let pins = project.prepare_audio_pitch(|| false).unwrap().unwrap();
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 64]);
    let mut changed = project.clone();
    changed
        .set_audio_recording_input(audio, audio_input(AudioInputChannels::Mono(1)))
        .unwrap();
    changed
        .set_midi_recording_input(
            midi,
            MidiRecordingInput {
                channel: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(changed.audio_pitch_is_prepared());
    assert!(Arc::ptr_eq(
        &pins[0],
        &changed.cached_audio_pitch().unwrap()[0]
    ));
    assert!(transport.update_parameters(&changed));
    assert_eq!(transport.position_frame(), 64);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn audio_bounce_preserves_current_recording_input_selection() {
    let mut source = Project::new();
    let audio = source.add_audio_track("Audio");
    let samples = AudioBuffer::new(vec![[0.25; 2]; 64]).unwrap();
    let asset = source.add_asset(samples.clone());
    source
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 64,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    let plan = BouncePlan::new(&source, 1).unwrap();
    let selection = audio_input(AudioInputChannels::Stereo(4, 5));
    source
        .set_audio_recording_input(audio, selection.clone())
        .unwrap();
    for mode in [BounceMode::InPlace, BounceMode::Duplicate] {
        let mut result = source.clone();
        let index = apply_bounce(&mut result, &plan, samples.clone(), mode).unwrap();
        let TrackId::Audio(id) = result.track_ids().nth(index).unwrap() else {
            panic!("expected audio");
        };
        assert_eq!(
            result.audio_track(id).unwrap().recording_input(),
            &selection
        );
    }
}
