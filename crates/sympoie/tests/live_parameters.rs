use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings};
use sympoie::transport::{PlaybackState, Transport};

fn audio_fixture() -> (Project, AudioTrackId) {
    let mut project = Project::new();
    let track = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[1.0; 2]; 10_000]).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 10_000,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    (project, track)
}

fn assert_small_steps(frames: &[Frame]) {
    for pair in frames.windows(2) {
        for channel in 0..2 {
            assert!(
                (pair[1][channel] - pair[0][channel]).abs() < 0.015,
                "discontinuous parameter change: {pair:?}"
            );
        }
    }
}

#[test]
fn mixer_and_clip_edits_ramp_without_resetting_transport() {
    let (project, track) = audio_fixture();
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(0, 960).unwrap();
    transport.play();
    let mut before = [[0.0; 2]; 20];
    transport.process_block(&mut before);
    assert_eq!(before, [[1.0; 2]; 20]);
    let loop_range = transport.loop_range();

    let mut edited = project.clone();
    edited.set_audio_clip_gain(track, 0, -0.5).unwrap();
    edited
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.5, -1.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    edited
        .set_master_track_mixer(MixerSettings::new(0.5, 0.0, AudioOutput::Output).unwrap())
        .unwrap();
    assert!(transport.update_parameters(&edited));
    assert_eq!(transport.position_frame(), 20);
    assert_eq!(transport.loop_range(), loop_range);
    assert_eq!(transport.state(), PlaybackState::Playing);

    let mut heard = vec![[1.0; 2]; 501];
    transport.process_block(&mut heard[1..]);
    assert_small_steps(&heard);
    assert_eq!(heard[500], [-0.125, 0.0]);
    assert_eq!(transport.position_frame(), 520);

    assert!(transport.update_parameters(&project));
    transport.process_block(&mut heard[1..]);
    assert_eq!(heard[500], [1.0; 2]);
}

#[test]
fn repeated_parameter_snapshots_and_block_sizes_do_not_restart_ramps() {
    let (project, track) = audio_fixture();
    let mut edited = project.clone();
    edited.set_audio_clip_gain(track, 0, -1.0).unwrap();
    edited
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.5, 0.7, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let mut single = Renderer::new(&project).unwrap();
    assert!(single.update_parameters(&edited));
    let mut expected = [[0.0; 2]; 600];
    single.render_block(0, &mut expected);

    let mut fragmented = Renderer::new(&project).unwrap();
    let mut actual = [[0.0; 2]; 600];
    for (index, block) in actual.chunks_mut(17).enumerate() {
        assert!(fragmented.update_parameters(&edited));
        fragmented.render_block((index * 17) as u64, block);
    }
    assert_eq!(actual, expected);
    assert_eq!(actual[599], [-0.15, -0.5]);
}

#[test]
fn unmuting_a_group_preserves_held_instrument_phase() {
    let mut project = Project::new();
    let track = project.add_midi_track("Sine");
    let group = project.add_group_track("Bus");
    project.set_track_group(track, Some(group)).unwrap();
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings {
                waveform: Waveform::Sine,
                attack_seconds: 0.01,
                cutoff_hz: 20_000.0,
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
                length_ticks: 10_000,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 10_000,
                    key: 12,
                    velocity: 127,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    let audible = project.clone();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(0.0, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let mut muted = Transport::new(&project).unwrap();
    let mut reference = Transport::new(&audible).unwrap();
    muted.play();
    reference.play();
    let mut scratch = [[0.0; 2]; 6_000];
    muted.process_block(&mut scratch);
    assert_eq!(scratch, [[0.0; 2]; 6_000]);
    reference.process_block(&mut scratch);

    assert!(muted.update_parameters(&audible));
    let mut actual = [[0.0; 2]; 600];
    let mut expected = [[0.0; 2]; 600];
    muted.process_block(&mut actual);
    reference.process_block(&mut expected);
    assert_small_steps(&actual);
    assert_eq!(actual[240..], expected[240..]);
    assert!(actual[240..].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn structural_edits_are_rejected_without_partial_parameter_changes() {
    let (project, track) = audio_fixture();
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut edited = project.clone();
    edited.set_audio_clip_gain(track, 0, 0.0).unwrap();
    edited
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.0, 1.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    assert!(!transport.update_parameters(&edited));
    let mut actual = [[0.0; 2]; 600];
    transport.process_block(&mut actual);
    assert_eq!(actual, [[1.0; 2]; 600]);
}
