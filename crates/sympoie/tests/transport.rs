use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
use sympoie::project::{AudioBuffer, Project};
use sympoie::time::{SnapGrid, TimeError};
use sympoie::track::{AudioOutput, MixerSettings};
use sympoie::transport::{PlaybackState, Transport, TransportCommand, TransportError};

fn project_with_samples(frame_count: usize) -> Project {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let samples = (0..frame_count)
        .map(|index| [(index + 1) as f32, -((index + 1) as f32)])
        .collect();
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    let track = project.add_audio_track("Sequence");
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: frame_count as u64,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    project
}

fn frames(values: &[f32]) -> Vec<Frame> {
    values.iter().map(|&value| [value, -value]).collect()
}

#[test]
fn free_play_starts_in_an_empty_project_and_can_seek_far_into_empty_space() {
    let project = Project::new();
    let mut transport = Transport::new(&project).unwrap();
    assert!(!transport.play_past_end());
    transport.set_play_past_end(true);
    transport.play();
    let mut output = [[99.0; 2]; 257];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 257]);
    assert_eq!(transport.position_frame(), 257);
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert_eq!(transport.duration_frames(), 0);

    transport.seek_frame(48_000 * 60 * 60);
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 257]);
    assert_eq!(transport.position_frame(), 48_000 * 60 * 60 + 257);
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert!(!transport.is_recording());
}

#[test]
fn free_play_keeps_clip_audio_and_continues_after_its_exact_end() {
    let project = project_with_samples(3);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_play_past_end(true);
    transport.play();
    let mut output = [[99.0; 2]; 3];
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[1.0, 2.0, 3.0]));
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 3]);
    assert_eq!(transport.position_frame(), 6);
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert_eq!(transport.duration_frames(), 3);
}

#[test]
fn free_play_renders_instrument_release_then_continues_through_silence() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_midi_track("Release");
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings {
                waveform: Waveform::Square,
                attack_seconds: 0.0,
                decay_seconds: 0.0,
                sustain: 1.0,
                release_seconds: 0.01,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 1000,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 1000,
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
    let mut finite = Transport::new(&project).unwrap();
    let duration = finite.duration_frames() as usize;
    let mut expected = vec![[99.0; 2]; duration + 99];
    finite.play();
    finite.process_block(&mut expected);
    assert!(expected[1000..duration].iter().any(|frame| frame[0] != 0.0));

    let mut transport = Transport::new(&project).unwrap();
    transport.set_play_past_end(true);
    transport.play();
    let mut actual = vec![[99.0; 2]; expected.len()];
    transport.process_block(&mut actual);
    assert_eq!(actual, expected);
    assert_eq!(transport.position_frame(), actual.len() as u64);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn free_play_respects_loops_in_empty_space_and_continues_when_loop_is_cleared() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let mut transport = Transport::new(&project).unwrap();
    transport.set_play_past_end(true);
    transport.set_loop_ticks(1_000_000, 1_000_003).unwrap();
    transport.seek_frame(u64::MAX);
    transport.play();
    let mut output = [[99.0; 2]; 8];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 8]);
    assert_eq!(transport.position_frame(), 1_000_002);
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.clear_loop();
    transport.process_block(&mut output);
    assert_eq!(transport.position_frame(), 1_000_010);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn free_play_survives_pause_stop_and_recording_finish() {
    let project = Project::new();
    let mut transport = Transport::new(&project).unwrap();
    transport.set_play_past_end(true);
    transport.set_recording(true);
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 4]);
    transport.pause();
    let mut output = [[99.0; 2]; 4];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 4]);
    assert_eq!(transport.position_frame(), 4);
    assert_eq!(transport.state(), PlaybackState::Paused);
    transport.set_recording(false);
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(transport.position_frame(), 8);
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.set_recording(true);
    transport.stop();
    assert!(transport.play_past_end());
    assert!(!transport.is_recording());
    transport.process_block(&mut output);
    assert_eq!(transport.position_frame(), 8);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(transport.position_frame(), 12);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn disabling_free_play_restores_finite_playback_on_the_next_nonempty_block() {
    let project = project_with_samples(3);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_play_past_end(true);
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 7]);
    transport.set_play_past_end(false);
    transport.process_block(&mut []);
    assert_eq!(transport.state(), PlaybackState::Playing);
    let mut output = [[99.0; 2]; 2];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 2]);
    assert_eq!(transport.position_frame(), 7);
    assert_eq!(transport.state(), PlaybackState::Stopped);
}

#[test]
fn free_play_and_recording_stop_at_the_frame_limit_without_wrapping() {
    let project = Project::new();
    for recording in [false, true] {
        for start in [u64::MAX - 3, u64::MAX] {
            let mut transport = Transport::new(&project).unwrap();
            transport.set_play_past_end(!recording);
            transport.set_recording(recording);
            transport.seek_frame(start);
            transport.play();
            let mut output = [[99.0; 2]; 8];
            transport.process_block(&mut output);
            assert_eq!(output, [[0.0; 2]; 8]);
            assert_eq!(transport.position_frame(), u64::MAX);
            assert_eq!(transport.state(), PlaybackState::Stopped);
        }
    }
}

#[test]
fn recording_rolls_through_empty_timeline_and_stops_normally_when_disabled() {
    let project = Project::new();
    let mut transport = Transport::new(&project).unwrap();
    transport.seek_frame(7000);
    transport.set_recording(true);
    transport.play();
    let mut output = [[99.0; 2]; 257];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 257]);
    assert_eq!(transport.position_frame(), 7257);
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.set_recording(false);
    transport.process_block(&mut output);
    assert_eq!(transport.position_frame(), 7257);
    assert_eq!(transport.state(), PlaybackState::Stopped);
}

#[test]
fn recording_keeps_backing_track_audio_and_advances_beyond_its_end() {
    let project = project_with_samples(3);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_recording(true);
    transport.play();
    let mut output = [[99.0; 2]; 7];
    transport.process_block(&mut output);
    assert_eq!(&output[..3], frames(&[1.0, 2.0, 3.0]));
    assert_eq!(&output[3..], &[[0.0; 2]; 4]);
    assert_eq!(transport.position_frame(), 7);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn starts_stopped_and_clears_reused_output_without_advancing() {
    let project = project_with_samples(8);
    let mut transport = Transport::new(&project).unwrap();
    let mut output = [[99.0; 2]; 4];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 4]);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert_eq!(transport.position_frame(), 0);
    assert_eq!(transport.position_tick().unwrap(), 0);
    assert_eq!(transport.loop_range(), None);
}

#[test]
fn pause_and_stop_retain_cursor_and_play_resumes_from_it() {
    let project = project_with_samples(10);
    let mut transport = Transport::new(&project).unwrap();
    let mut output = [[99.0; 2]; 2];
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[1.0, 2.0]));
    transport.pause();
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 2]);
    assert_eq!(transport.state(), PlaybackState::Paused);
    assert_eq!(transport.position_frame(), 2);
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[3.0, 4.0]));
    transport.stop();
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 2]);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert_eq!(transport.position_frame(), 4);
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[5.0, 6.0]));
}

#[test]
fn frame_and_tick_seeks_are_independent_of_editing_grid() {
    let mut project = project_with_samples(100);
    project.set_bpm(127.0).unwrap();
    project.set_snap_grid(SnapGrid::Bar);
    let mut transport = Transport::new(&project).unwrap();
    transport.seek_tick(1).unwrap();
    assert_eq!(transport.position_frame(), 24);
    assert_eq!(transport.position_tick().unwrap(), 1);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    transport.play();
    let mut output = [[0.0; 2]; 1];
    transport.process_block(&mut output);
    assert_eq!(output, [[25.0, -25.0]]);
    transport.seek_frame(7);
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.process_block(&mut output);
    assert_eq!(output, [[8.0, -8.0]]);
    transport.return_to_start();
    assert_eq!(transport.position_frame(), 0);
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.process_block(&mut output);
    assert_eq!(output, [[1.0, -1.0]]);
}

#[test]
fn failed_seek_keeps_cursor_and_playback_state() {
    let project = project_with_samples(10);
    let mut transport = Transport::new(&project).unwrap();
    transport.seek_frame(3);
    transport.play();
    assert_eq!(
        transport.seek_tick(u64::MAX),
        Err(TimeError::ConversionOverflow)
    );
    assert_eq!(transport.position_frame(), 3);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn frame_clock_does_not_accumulate_rounding_of_tick_positions() {
    let mut project = project_with_samples(10_000);
    project.set_bpm(127.0).unwrap();
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    for _ in 0..1000 {
        transport.process_block(&mut [[0.0; 2]; 7]);
    }
    assert_eq!(transport.position_frame(), 7000);
    assert_eq!(
        transport.position_tick().unwrap(),
        project.tempo().frames_to_ticks(7000).unwrap()
    );
}

#[test]
fn stops_at_audio_end_and_clears_the_rest_of_the_block() {
    let project = project_with_samples(3);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut output = [[99.0; 2]; 5];
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[1.0, 2.0, 3.0, 0.0, 0.0]));
    assert_eq!(transport.position_frame(), 3);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 5]);
    assert_eq!(transport.position_frame(), 3);
    assert_eq!(transport.state(), PlaybackState::Stopped);
}

#[test]
fn exact_end_boundary_stops_without_requiring_an_extra_block() {
    let project = project_with_samples(3);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 3]);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert_eq!(transport.position_frame(), 3);
}

#[test]
fn empty_project_and_seek_past_end_stop_without_overflow() {
    let project = Project::new();
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut output = [[99.0; 2]; 2];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 2]);
    assert_eq!(transport.position_frame(), 0);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    transport.seek_frame(u64::MAX);
    transport.play();
    output.fill([99.0; 2]);
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 2]);
    assert_eq!(transport.position_frame(), u64::MAX);
    assert_eq!(transport.state(), PlaybackState::Stopped);
}

#[test]
fn empty_blocks_do_not_stop_or_wrap_or_advance() {
    let project = project_with_samples(3);
    let mut transport = Transport::new(&project).unwrap();
    transport.seek_frame(u64::MAX);
    transport.play();
    transport.process_block(&mut []);
    assert_eq!(transport.position_frame(), u64::MAX);
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.set_loop_ticks(1, 3).unwrap();
    transport.process_block(&mut []);
    assert_eq!(transport.position_frame(), u64::MAX);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn loop_plays_intro_once_and_wraps_multiple_times_with_no_missing_frame() {
    let project = project_with_samples(10);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(2, 5).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 12];
    transport.process_block(&mut output);
    assert_eq!(
        output.as_slice(),
        frames(&[1.0, 2.0, 3.0, 4.0, 5.0, 3.0, 4.0, 5.0, 3.0, 4.0, 5.0, 3.0])
    );
    assert_eq!(transport.position_frame(), 3);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn exact_loop_boundary_reports_start_and_next_block_begins_there() {
    let project = project_with_samples(6);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(2, 5).unwrap();
    transport.seek_tick(2).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 3];
    for _ in 0..3 {
        transport.process_block(&mut output);
        assert_eq!(output.as_slice(), frames(&[3.0, 4.0, 5.0]));
        assert_eq!(transport.position_frame(), 2);
    }
}

#[test]
fn looping_from_end_or_beyond_wraps_only_when_playing_output_is_requested() {
    let project = project_with_samples(10);
    let mut transport = Transport::new(&project).unwrap();
    transport.seek_frame(u64::MAX);
    transport.set_loop_ticks(3, 7).unwrap();
    assert_eq!(transport.position_frame(), u64::MAX);
    let mut output = [[99.0; 2]; 2];
    transport.process_block(&mut output);
    assert_eq!(transport.position_frame(), u64::MAX);
    assert_eq!(output, [[0.0; 2]; 2]);
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[4.0, 5.0]));
    transport.seek_frame(7);
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[4.0, 5.0]));
}

#[test]
fn loop_output_is_identical_across_arbitrary_block_sizes() {
    let project = project_with_samples(1100);
    let mut whole = Transport::new(&project).unwrap();
    whole.set_loop_ticks(91, 1003).unwrap();
    whole.play();
    let mut expected = vec![[0.0; 2]; 6037];
    whole.process_block(&mut expected);
    for block_size in [1, 7, 255, 256, 257, 512, 911, 912, 913, 2048] {
        let mut transport = Transport::new(&project).unwrap();
        transport.set_loop_ticks(91, 1003).unwrap();
        transport.play();
        let mut actual = vec![[99.0; 2]; expected.len()];
        for block in actual.chunks_mut(block_size) {
            transport.process_block(block);
        }
        assert_eq!(actual, expected, "block size {block_size}");
        assert_eq!(transport.position_frame(), whole.position_frame());
        assert_eq!(transport.state(), PlaybackState::Playing);
    }
}

#[test]
fn nonintegral_tick_boundaries_repeat_the_same_absolute_sample_range() {
    let mut project = project_with_samples(100);
    project.set_bpm(127.0).unwrap();
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(1, 3).unwrap();
    let range = transport.loop_range().unwrap();
    assert_eq!((range.start_tick(), range.end_tick()), (1, 3));
    assert_eq!((range.start_frame(), range.end_frame()), (24, 71));
    transport.seek_tick(1).unwrap();
    transport.play();
    let mut output = vec![[99.0; 2]; 47_000];
    for block in output.chunks_mut(511) {
        transport.process_block(block);
    }
    for (index, frame) in output.iter().enumerate() {
        let value = (25 + index % 47) as f32;
        assert_eq!(*frame, [value, -value]);
    }
    assert_eq!(transport.position_frame(), 24);
}

#[test]
fn loops_can_include_silence_beyond_the_audio_end() {
    let project = project_with_samples(4);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(2, 7).unwrap();
    transport.seek_tick(2).unwrap();
    transport.play();
    let mut output = [[99.0; 2]; 12];
    transport.process_block(&mut output);
    assert_eq!(
        output.as_slice(),
        frames(&[3.0, 4.0, 0.0, 0.0, 0.0, 3.0, 4.0, 0.0, 0.0, 0.0, 3.0, 4.0])
    );
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert_eq!(transport.position_frame(), 4);
}

#[test]
fn a_loop_in_an_empty_project_keeps_advancing_through_silence() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(5, 6).unwrap();
    transport.seek_frame(5);
    transport.play();
    let mut output = [[99.0; 2]; 19];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 19]);
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert_eq!(transport.position_frame(), 5);
}

#[test]
fn invalid_loop_edits_keep_the_old_loop_cursor_and_state() {
    let project = project_with_samples(10);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(2, 5).unwrap();
    let old_loop = transport.loop_range();
    transport.seek_frame(3);
    transport.play();
    for (start, end) in [(3, 3), (5, 2)] {
        assert_eq!(
            transport.set_loop_ticks(start, end),
            Err(TransportError::InvalidLoopRange)
        );
    }
    assert_eq!(
        transport.set_loop_ticks(0, u64::MAX),
        Err(TransportError::Time(TimeError::ConversionOverflow))
    );
    assert_eq!(transport.loop_range(), old_loop);
    assert_eq!(transport.position_frame(), 3);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn sub_frame_loop_is_rejected_before_replacing_a_valid_loop() {
    let mut project = project_with_samples(10);
    project.set_bpm(12_000.0).unwrap();
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(0, 8).unwrap();
    let old_loop = transport.loop_range();
    assert_eq!(
        transport.set_loop_ticks(0, 1),
        Err(TransportError::LoopShorterThanFrame)
    );
    assert_eq!(transport.loop_range(), old_loop);
}

#[test]
fn clearing_loop_preserves_cursor_and_allows_playback_to_reach_project_end() {
    let project = project_with_samples(6);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(1, 3).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 4]);
    assert_eq!(transport.position_frame(), 2);
    transport.clear_loop();
    assert_eq!(transport.loop_range(), None);
    assert_eq!(transport.position_frame(), 2);
    let mut output = [[99.0; 2]; 5];
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[3.0, 4.0, 5.0, 6.0, 0.0]));
    assert_eq!(transport.state(), PlaybackState::Stopped);
}

#[test]
fn silent_master_does_not_shorten_playback_timeline() {
    for mixer in [
        MixerSettings::new(0.0, 0.0, AudioOutput::Output).unwrap(),
        MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
    ] {
        let mut project = project_with_samples(4);
        project.set_master_track_mixer(mixer).unwrap();
        let mut transport = Transport::new(&project).unwrap();
        transport.play();
        let mut output = [[99.0; 2]; 3];
        transport.process_block(&mut output);
        assert_eq!(output, [[0.0; 2]; 3]);
        assert_eq!(transport.position_frame(), 3);
        assert_eq!(transport.state(), PlaybackState::Playing);
        transport.process_block(&mut output);
        assert_eq!(transport.position_frame(), 4);
        assert_eq!(transport.state(), PlaybackState::Stopped);
    }
}

#[test]
fn unassigned_midi_does_not_block_audio_playback_or_extend_its_duration() {
    let mut project = project_with_samples(3);
    let midi = project.add_midi_track("MIDI");
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
    let mut transport = Transport::new(&project).unwrap();
    let mut output = [[99.0; 2]; 5];
    transport.play();
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[1.0, 2.0, 3.0, 0.0, 0.0]));
    assert_eq!(transport.position_frame(), 3);
    assert_eq!(transport.state(), PlaybackState::Stopped);
}

#[test]
fn shared_commands_control_the_same_cursor_and_loop() {
    let project = project_with_samples(10);
    let mut transport = Transport::new(&project).unwrap();
    for command in [
        TransportCommand::SeekTick(2),
        TransportCommand::SetLoopTicks { start: 2, end: 4 },
        TransportCommand::Play,
    ] {
        transport.apply(command).unwrap();
    }
    let mut output = [[0.0; 2]; 3];
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[3.0, 4.0, 3.0]));
    transport.apply(TransportCommand::Pause).unwrap();
    assert_eq!(transport.state(), PlaybackState::Paused);
    transport.apply(TransportCommand::SeekFrame(7)).unwrap();
    transport.apply(TransportCommand::ClearLoop).unwrap();
    transport.apply(TransportCommand::Play).unwrap();
    transport.process_block(&mut output);
    assert_eq!(output.as_slice(), frames(&[8.0, 9.0, 10.0]));
    transport.apply(TransportCommand::ReturnToStart).unwrap();
    transport.apply(TransportCommand::Stop).unwrap();
    assert_eq!(transport.position_frame(), 0);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert_eq!(transport.loop_range(), None);
}
