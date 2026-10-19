use sympoie::clip::AudioClip;
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{read_project, write_project};
use sympoie::time::{SnapGrid, TimeSignature};
use sympoie::track::{AudioOutput, MixerSettings};
use sympoie::transport::{PlaybackState, Transport};

fn arranged_project() -> Project {
    let mut project = Project::new();
    project.set_bpm(137.0).unwrap();
    project.set_time_signature(TimeSignature::new(3, 4).unwrap());
    let asset = project.add_asset(
        AudioBuffer::new(
            (0..600)
                .map(|i| [i as f32 / 1024.0, -(i as f32) / 2048.0])
                .collect(),
        )
        .unwrap(),
    );
    let voice = project.add_audio_track("Voice / 人声");
    let group = project.add_group_track("Inner");
    let outer = project.add_group_track("Outer");
    project.set_track_group(voice, Some(group)).unwrap();
    project.set_track_group(group, Some(outer)).unwrap();
    project
        .set_group_track_mixer(
            outer,
            MixerSettings::new(0.75, 0.25, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.6, -0.5, AudioOutput::Output).unwrap())
        .unwrap();
    project
        .add_audio_clip(
            voice,
            AudioClip {
                asset,
                source_start: 13,
                start_tick: 5,
                frame_count: 501,
                gain: -0.8,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    let direct = project.add_audio_track("Direct");
    project
        .add_audio_clip(
            direct,
            AudioClip {
                asset,
                source_start: 23,
                start_tick: 12,
                frame_count: 333,
                gain: 0.1,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    project.set_snap_grid(SnapGrid::Bar);
    project
}

fn playback(project: &Project, block_size: usize) -> Vec<[f32; 2]> {
    let mut transport = Transport::new(project).unwrap();
    transport.seek_tick(2).unwrap();
    transport.set_loop_ticks(11, 29).unwrap();
    transport.play();
    let mut output = vec![[f32::NAN; 2]; 2101];
    for block in output.chunks_mut(block_size) {
        transport.process_block(block);
    }
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.stop();
    let stopped_position = transport.position_frame();
    let mut silence = [[f32::NAN; 2]; 7];
    transport.process_block(&mut silence);
    assert_eq!(silence, [[0.0; 2]; 7]);
    assert_eq!(transport.position_frame(), stopped_position);
    transport.clear_loop();
    transport.return_to_start();
    transport.play();
    let mut ending = vec![[0.0; 2]; project.audio_duration_frames() as usize + 10];
    transport.process_block(&mut ending);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert_eq!(transport.position_frame(), project.audio_duration_frames());
    output.extend(ending);
    output
}

#[test]
fn save_load_preserves_musical_positions_routes_and_looped_audio_across_blocks() {
    let project = arranged_project();
    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(restored.bpm(), 137.0);
    assert_eq!(restored.time_signature(), project.time_signature());
    assert_eq!(restored.snap_grid(), SnapGrid::Bar);
    assert_eq!(
        restored.audio_duration_frames(),
        project.audio_duration_frames()
    );
    let expected = playback(&project, 256);
    assert!(expected.iter().any(|frame| frame[0] != 0.0));
    for block_size in [1, 7, 256, 511, 4096] {
        assert_eq!(playback(&restored, block_size), expected);
    }
}

#[test]
fn saving_while_playing_does_not_persist_the_runtime_cursor_or_loop() {
    let project = arranged_project();
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(11, 29).unwrap();
    transport.seek_tick(12).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 17]);
    let position = transport.position_frame();
    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert_eq!(transport.position_frame(), position);
    let restored = read_project(bytes.as_slice()).unwrap();
    let reloaded_transport = Transport::new(&restored).unwrap();
    assert_eq!(reloaded_transport.state(), PlaybackState::Stopped);
    assert_eq!(reloaded_transport.position_frame(), 0);
    assert!(reloaded_transport.loop_range().is_none());
}
