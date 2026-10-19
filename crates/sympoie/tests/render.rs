use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::render::{Renderer, render_block};
use sympoie::time::TimeError;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings};

fn one_clip_project(samples: Vec<Frame>) -> (Project, AudioTrackId, AudioClip) {
    let frame_count = samples.len() as u64;
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    let clip = AudioClip {
        asset,
        source_start: 0,
        start_tick: 0,
        frame_count,
        gain: 1.0,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    (project, track, clip)
}

#[test]
fn mixes_stereo_with_offsets_trims_and_gain_without_clipping() {
    let (mut project, track, first) = one_clip_project(vec![
        [10.0, 10.0],
        [0.25, -0.5],
        [0.5, -0.25],
        [-0.25, 0.5],
        [10.0, 10.0],
    ]);
    project
        .add_audio_clip(
            track,
            AudioClip {
                source_start: 1,
                start_tick: 1,
                frame_count: 3,
                gain: 2.0,
                ..first
            },
        )
        .unwrap();
    let second_asset = project.add_asset(AudioBuffer::new(vec![[0.5, 1.0], [0.25, -0.5]]).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset: second_asset,
                source_start: 0,
                start_tick: 2,
                frame_count: 2,
                gain: 0.5,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();

    let mut output = [[99.0; 2]; 6];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(
        output,
        [
            [0.0, 0.0],
            [0.5, -1.0],
            [1.25, 0.0],
            [-0.375, 0.75],
            [0.0, 0.0],
            [0.0, 0.0],
        ]
    );
    assert_eq!(project.audio_duration_frames(), 4);
}

#[test]
fn silence_clears_reused_output_before_after_and_without_clips() {
    let (mut project, track, clip) = one_clip_project(vec![[0.5; 2]; 3]);
    let mut output = [[1.0; 2]; 4];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    assert_eq!(project.audio_duration_frames(), 0);

    project
        .add_audio_clip(
            track,
            AudioClip {
                start_tick: 10,
                ..clip
            },
        )
        .unwrap();
    output.fill([1.0; 2]);
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    output.fill([1.0; 2]);
    render_block(&project, 13, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    render_block(&project, 11, &mut []).unwrap();
}

#[test]
fn rendering_is_identical_across_block_sizes() {
    let samples = (0..641)
        .map(|index| [index as f32 / 100.0, -(index as f32) / 128.0])
        .collect();
    let (mut project, track, clip) = one_clip_project(samples);
    project
        .add_audio_clip(
            track,
            AudioClip {
                start_tick: 2,
                ..clip.clone()
            },
        )
        .unwrap();
    project
        .add_audio_clip(
            track,
            AudioClip {
                start_tick: 13,
                source_start: 5,
                frame_count: 427,
                gain: -0.375,
                ..clip.clone()
            },
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.7, -0.3, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let second_track = project.add_audio_track("Second");
    project
        .add_audio_clip(
            second_track,
            AudioClip {
                start_tick: 108,
                gain: 0.2,
                ..clip
            },
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            second_track,
            MixerSettings::new(0.6, 0.9, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let mut reference = vec![[0.0; 2]; project.audio_duration_frames() as usize + 7];
    render_block(&project, 0, &mut reference).unwrap();
    for block_size in [1, 2, 3, 7, 16, 32, 128, 255, 256, 257, 512, 1024] {
        let mut actual = vec![[99.0; 2]; reference.len()];
        for (block_index, block) in actual.chunks_mut(block_size).enumerate() {
            render_block(&project, (block_index * block_size) as u64, block).unwrap();
        }
        assert_eq!(actual, reference, "block size {block_size}");
    }
}

#[test]
fn default_tempo_places_a_quarter_note_at_half_a_second() {
    let mut project = Project::new();
    let track = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25, -0.5]; 3]).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 960,
                frame_count: 3,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();

    assert_eq!(project.audio_duration_frames(), 24_003);
    let mut output = [[99.0; 2]; 5];
    Renderer::new(&project)
        .unwrap()
        .render_block(23_999, &mut output);
    assert_eq!(
        output,
        [[0.0; 2], [0.25, -0.5], [0.25, -0.5], [0.25, -0.5], [0.0; 2]]
    );
}

#[test]
fn changing_bpm_moves_audio_without_stretching_or_resampling_its_source() {
    let mut project = Project::new();
    let track = project.add_audio_track("Audio");
    let asset = project.add_asset(
        AudioBuffer::new(vec![[9.0; 2], [0.25, -0.5], [0.5, 0.75], [-0.25, 0.0]]).unwrap(),
    );
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 1,
                start_tick: 960,
                frame_count: 3,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();

    for (bpm, start_frame) in [(120.0, 24_000), (60.0, 48_000), (137.0, 21_022)] {
        project.set_bpm(bpm).unwrap();
        assert_eq!(
            project.audio_track(track).unwrap().clips()[0].start_tick,
            960
        );
        assert_eq!(project.audio_duration_frames(), start_frame + 3);
        let mut renderer = Renderer::new(&project).unwrap();
        let mut output = [[99.0; 2]; 5];
        renderer.render_block(start_frame - 1, &mut output);
        assert_eq!(
            output,
            [[0.0; 2], [0.25, -0.5], [0.5, 0.75], [-0.25, 0.0], [0.0; 2]]
        );
        if bpm == 60.0 {
            renderer.render_block(24_000, &mut output);
            assert_eq!(output, [[0.0; 2]; 5]);
        }
    }
}

#[test]
fn fractional_frames_per_tick_have_identical_positions_across_block_sizes() {
    let mut project = Project::new();
    project.set_bpm(137.0).unwrap();
    let track = project.add_audio_track("Audio");
    let samples: Vec<_> = (0..641)
        .map(|index| [index as f32 / 1024.0, -(index as f32) / 512.0])
        .collect();
    let asset = project.add_asset(AudioBuffer::new(samples.clone()).unwrap());
    for (start_tick, gain) in [(13, 1.0), (37, -0.5)] {
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick,
                    frame_count: samples.len() as u64,
                    gain,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
    }
    assert_eq!(project.audio_duration_frames(), 1451);
    let mut expected = vec![[0.0; 2]; 1600];
    for (start, gain) in [(285, 1.0), (810, -0.5)] {
        for (frame, source) in expected[start..].iter_mut().zip(&samples) {
            frame[0] += source[0] * gain;
            frame[1] += source[1] * gain;
        }
    }
    for block_size in [1, 2, 7, 128, 255, 256, 257, 512, 1024] {
        let mut renderer = Renderer::new(&project).unwrap();
        let mut actual = vec![[99.0; 2]; expected.len()];
        for (index, block) in actual.chunks_mut(block_size).enumerate() {
            renderer.render_block((index * block_size) as u64, block);
        }
        assert_eq!(actual, expected, "block size {block_size}");
    }
}

#[test]
fn timeline_end_and_blocks_near_u64_max_do_not_wrap() {
    let mut samples = vec![[0.0; 2]; 6144];
    samples[6141..].copy_from_slice(&[[0.25; 2], [0.5; 2], [0.75; 2]]);
    let (mut project, track, clip) = one_clip_project(samples);
    project
        .add_audio_clip(
            track,
            AudioClip {
                start_tick: u64::MAX - 6143,
                source_start: 1,
                frame_count: 6143,
                ..clip
            },
        )
        .unwrap();
    assert_eq!(project.audio_duration_frames(), u64::MAX);
    let mut output = [[99.0; 2]; 6];
    render_block(&project, u64::MAX - 4, &mut output).unwrap();
    assert_eq!(
        output,
        [[0.0; 2], [0.25; 2], [0.5; 2], [0.75; 2], [0.0; 2], [0.0; 2]]
    );
    render_block(&project, u64::MAX, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 6]);

    let mut large_output = [[99.0; 2]; 600];
    render_block(&project, u64::MAX - 300, &mut large_output).unwrap();
    assert!(large_output[..297].iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(large_output[297..300], [[0.25; 2], [0.5; 2], [0.75; 2]]);
    assert!(large_output[300..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn accepts_empty_audio_and_rejects_nonfinite_samples() {
    let empty = AudioBuffer::new(vec![]).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    assert!(empty.samples().is_empty());
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(matches!(
            AudioBuffer::new(vec![[0.0; 2], [0.0, bad]]),
            Err(ProjectError::NonFiniteSample {
                frame: 1,
                channel: 1
            })
        ));
    }
}

#[test]
fn invalid_clips_do_not_mutate_the_project() {
    let (mut project, track, clip) = one_clip_project(vec![[1.0; 2]; 4]);
    let cases = [
        (
            AudioClip {
                frame_count: 0,
                ..clip.clone()
            },
            ProjectError::EmptyClip,
        ),
        (
            AudioClip {
                gain: f32::NAN,
                ..clip.clone()
            },
            ProjectError::NonFiniteGain,
        ),
        (
            AudioClip {
                gain: f32::INFINITY,
                ..clip.clone()
            },
            ProjectError::NonFiniteGain,
        ),
        (
            AudioClip {
                source_start: u64::MAX,
                frame_count: 1,
                ..clip.clone()
            },
            ProjectError::SourceRangeOverflow,
        ),
        (
            AudioClip {
                source_start: 3,
                frame_count: 2,
                ..clip.clone()
            },
            ProjectError::SourceRangeOutOfBounds {
                start: 3,
                frame_count: 2,
                asset_frames: 4,
            },
        ),
        (
            AudioClip {
                start_tick: u64::MAX,
                frame_count: 1,
                ..clip.clone()
            },
            ProjectError::Time(TimeError::ConversionOverflow),
        ),
    ];
    for (invalid, expected) in cases {
        assert_eq!(project.add_audio_clip(track, invalid), Err(expected));
        assert_eq!(project.audio_duration_frames(), 0);
        let mut output = [[99.0; 2]; 4];
        render_block(&project, 0, &mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 4]);
    }
    project.add_audio_clip(track, clip.clone()).unwrap();
    assert_eq!(project.audio_duration_frames(), 4);

    let mut other_project = Project::new();
    let other_track = other_project.add_audio_track("Other");
    assert_eq!(
        other_project.add_audio_clip(other_track, clip.clone()),
        Err(ProjectError::UnknownAsset { asset: clip.asset })
    );
}

#[test]
fn extreme_finite_arithmetic_is_left_for_output_validation() {
    let (mut project, track, clip) = one_clip_project(vec![[f32::MAX; 2]]);
    project
        .add_audio_clip(track, AudioClip { gain: 2.0, ..clip })
        .unwrap();
    let mut output = [[0.0; 2]; 1];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[f32::INFINITY; 2]]);
}

#[test]
fn tracks_apply_independent_gain_and_balance_before_master_mixing() {
    let (mut project, first_track, clip) = one_clip_project(vec![[1.0, 2.0]]);
    project.add_audio_clip(first_track, clip.clone()).unwrap();
    project
        .set_audio_track_mixer(
            first_track,
            MixerSettings::new(0.5, -0.25, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let second_track = project.add_audio_track("Second");
    let asset = project.add_asset(AudioBuffer::new(vec![[4.0, 2.0]]).unwrap());
    project
        .add_audio_clip(second_track, AudioClip { asset, ..clip })
        .unwrap();
    project
        .set_audio_track_mixer(
            second_track,
            MixerSettings::new(0.25, 0.5, AudioOutput::Master).unwrap(),
        )
        .unwrap();

    let mut output = [[99.0; 2]; 1];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[1.0, 1.25]]);
}

#[test]
fn master_mixer_applies_once_to_direct_and_grouped_tracks_without_clipping() {
    let (mut project, grouped, clip) = one_clip_project(vec![[1.0, 2.0]]);
    project
        .add_audio_clip(
            grouped,
            AudioClip {
                gain: 0.5,
                ..clip.clone()
            },
        )
        .unwrap();
    let group = project.add_group_track("Bus");
    project
        .set_audio_track_mixer(
            grouped,
            MixerSettings::new(0.5, -0.5, AudioOutput::Group(group)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(2.0, 0.5, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let direct = project.add_audio_track("Direct");
    project.add_audio_clip(direct, clip).unwrap();
    project
        .set_audio_track_mixer(
            direct,
            MixerSettings::new(0.25, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();

    for (gain, pan, expected) in [(0.5, -0.5, [0.25, 0.25]), (4.0, 0.0, [2.0, 4.0])] {
        project
            .set_master_track_mixer(MixerSettings::new(gain, pan, AudioOutput::Output).unwrap())
            .unwrap();
        let mut output = [[99.0; 2]];
        render_block(&project, 0, &mut output).unwrap();
        assert_eq!(output, [expected]);
    }
}

#[test]
fn master_sums_direct_and_grouped_tracks_before_applying_gain() {
    let (mut project, direct, clip) = one_clip_project(vec![[f32::MAX / 2.0; 2]]);
    project.add_audio_clip(direct, clip.clone()).unwrap();
    let inverted = project.add_audio_track("Inverted");
    project
        .add_audio_clip(inverted, AudioClip { gain: -1.0, ..clip })
        .unwrap();
    let group = project.add_group_track("Bus");
    project.set_track_group(inverted, Some(group)).unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(4.0, 0.0, AudioOutput::Output).unwrap())
        .unwrap();

    let mut output = [[99.0; 2]];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]]);
}

#[test]
fn silent_master_suppresses_all_overflowing_descendants() {
    let (mut project, direct, clip) = one_clip_project(vec![[f32::MAX; 2]]);
    project
        .add_audio_clip(
            direct,
            AudioClip {
                gain: 2.0,
                ..clip.clone()
            },
        )
        .unwrap();
    let grouped = project.add_audio_track("Grouped");
    project
        .add_audio_clip(grouped, AudioClip { gain: -2.0, ..clip })
        .unwrap();
    let inner = project.add_group_track("Inner");
    let outer = project.add_group_track("Outer");
    project.set_track_group(grouped, Some(inner)).unwrap();
    project.set_track_group(inner, Some(outer)).unwrap();

    for mixer in [
        MixerSettings::new(0.0, 0.0, AudioOutput::Output).unwrap(),
        MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
    ] {
        project.set_master_track_mixer(mixer).unwrap();
        let mut output = [[99.0; 2]; 600];
        Renderer::new(&project)
            .unwrap()
            .render_block(0, &mut output);
        assert_eq!(output, [[0.0; 2]; 600]);
    }
}

#[test]
fn master_balance_cuts_overflowing_channel_without_crossfeed() {
    let (mut project, track, clip) = one_clip_project(vec![[0.25, f32::MAX]]);
    project
        .add_audio_clip(track, AudioClip { gain: 2.0, ..clip })
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.5, -1.0, AudioOutput::Output).unwrap())
        .unwrap();
    let mut output = [[99.0; 2]];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.25, 0.0]]);
}

#[test]
fn balance_endpoints_cut_a_channel_without_crossfeed() {
    let (mut project, track, clip) = one_clip_project(vec![[0.25, 0.75]]);
    project.add_audio_clip(track, clip).unwrap();
    for (pan, expected) in [(-1.0, [0.25, 0.0]), (0.0, [0.25, 0.75]), (1.0, [0.0, 0.75])] {
        project
            .set_audio_track_mixer(
                track,
                MixerSettings::new(1.0, pan, AudioOutput::Master).unwrap(),
            )
            .unwrap();
        let mut output = [[99.0; 2]; 1];
        render_block(&project, 0, &mut output).unwrap();
        assert_eq!(output, [expected]);
    }
}

#[test]
fn track_gain_applies_after_summing_its_clips() {
    let (mut project, track, clip) = one_clip_project(vec![[f32::MAX / 2.0; 2]]);
    project.add_audio_clip(track, clip.clone()).unwrap();
    project
        .add_audio_clip(track, AudioClip { gain: -1.0, ..clip })
        .unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(4.0, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();

    let mut output = [[99.0; 2]; 1];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]]);
}

#[test]
fn disconnected_and_zero_gain_tracks_do_not_contaminate_master() {
    let (mut project, active, clip) = one_clip_project(vec![[0.25, 0.5]]);
    project.add_audio_clip(active, clip.clone()).unwrap();
    let loud_asset = project.add_asset(AudioBuffer::new(vec![[f32::MAX; 2]]).unwrap());
    for mixer in [
        MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        MixerSettings::new(0.0, 0.0, AudioOutput::Master).unwrap(),
    ] {
        let track = project.add_audio_track("Silent");
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset: loud_asset,
                    gain: 2.0,
                    ..clip.clone()
                },
            )
            .unwrap();
        project.set_audio_track_mixer(track, mixer).unwrap();
    }
    let mut output = [[99.0; 2]; 1];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.25, 0.5]]);
}

#[test]
fn completely_cut_balance_channel_stays_silent_after_overflow() {
    let (mut project, track, clip) = one_clip_project(vec![[0.25, f32::MAX]]);
    project
        .add_audio_clip(track, AudioClip { gain: 2.0, ..clip })
        .unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(1.0, -1.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let mut output = [[99.0; 2]; 1];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.5, 0.0]]);
}

#[test]
fn unassigned_midi_clips_are_silent_and_do_not_block_audio() {
    for notes in [
        vec![],
        vec![MidiNote {
            start_tick: 0,
            duration_ticks: 120,
            key: 60,
            velocity: 100,
            channel: 0,
            slide_ticks: 0,
        }],
    ] {
        for start_tick in [0, 1_000] {
            let (mut project, audio_track, clip) = one_clip_project(vec![[0.5; 2]]);
            project.add_audio_clip(audio_track, clip).unwrap();
            let midi_track = project.add_midi_track("MIDI");
            project
                .add_midi_clip(
                    midi_track,
                    MidiClip {
                        offset_ticks: 0,
                        start_tick,
                        length_ticks: 480,
                        notes: notes.clone(),
                        name: None,
                        pitch_bend: Vec::new(),
                    },
                )
                .unwrap();
            let mut output = [[99.0; 2]; 3];
            render_block(&project, 0, &mut output).unwrap();
            assert_eq!(output, [[0.5; 2], [0.0; 2], [0.0; 2]]);
            output.fill([99.0; 2]);
            render_block(&project, start_tick + 1, &mut output).unwrap();
            assert_eq!(output, [[0.0; 2]; 3]);
            render_block(&project, 0, &mut []).unwrap();
            assert_eq!(project.duration_frames().unwrap(), 1);
        }
    }
}

#[test]
fn empty_midi_tracks_do_not_prevent_audio_rendering() {
    let (mut project, audio_track, clip) = one_clip_project(vec![[0.25, -0.5]]);
    project.add_audio_clip(audio_track, clip).unwrap();
    project.add_midi_track("Empty MIDI");
    let mut output = [[99.0; 2]; 1];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.25, -0.5]]);
}

#[test]
fn group_sums_its_children_before_applying_its_gain() {
    let (mut project, first, clip) = one_clip_project(vec![[f32::MAX / 2.0; 2]]);
    project.add_audio_clip(first, clip.clone()).unwrap();
    let second = project.add_audio_track("Inverted");
    project
        .add_audio_clip(second, AudioClip { gain: -1.0, ..clip })
        .unwrap();
    let group = project.add_group_track("Cancellation bus");
    project.set_track_group(first, Some(group)).unwrap();
    project.set_track_group(second, Some(group)).unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(4.0, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();

    let mut output = [[99.0; 2]];
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]]);
}

#[test]
fn child_and_group_mixers_apply_once_without_extra_dry_output() {
    let (mut project, first, clip) = one_clip_project(vec![[1.0, 2.0]]);
    project.add_audio_clip(first, clip.clone()).unwrap();
    let group = project.add_group_track("Group");
    project
        .set_audio_track_mixer(
            first,
            MixerSettings::new(0.5, -0.25, AudioOutput::Group(group)).unwrap(),
        )
        .unwrap();
    let second = project.add_audio_track("Second");
    let asset = project.add_asset(AudioBuffer::new(vec![[4.0, 2.0]]).unwrap());
    project
        .add_audio_clip(
            second,
            AudioClip {
                asset,
                ..clip.clone()
            },
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            second,
            MixerSettings::new(0.25, 0.5, AudioOutput::Group(group)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(2.0, -0.5, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let independent = project.add_audio_track("Independent");
    project
        .add_audio_clip(independent, AudioClip { gain: 0.25, ..clip })
        .unwrap();

    let mut output = [[99.0; 2]];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[2.25, 1.75]]);
}

#[test]
fn nested_groups_render_independently_of_parent_creation_order() {
    let (mut project, first, clip) = one_clip_project(vec![[0.5; 2]]);
    project.add_audio_clip(first, clip.clone()).unwrap();
    let root = project.add_group_track("Root");
    let second = project.add_audio_track("Second");
    project
        .add_audio_clip(second, AudioClip { gain: 2.0, ..clip })
        .unwrap();
    let inner = project.add_group_track("Inner");
    project
        .set_audio_track_mixer(
            first,
            MixerSettings::new(2.0, 0.0, AudioOutput::Group(inner)).unwrap(),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            second,
            MixerSettings::new(0.5, 0.0, AudioOutput::Group(root)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            inner,
            MixerSettings::new(0.25, 0.0, AudioOutput::Group(root)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            root,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();

    let mut output = [[99.0; 2]];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.375; 2]]);
}

#[test]
fn group_siblings_are_summed_in_track_insertion_order() {
    let mut project = Project::new();
    let root = project.add_group_track("Root");
    let positive = project.add_group_track("Positive");
    let negative = project.add_audio_track("Negative");
    let small = project.add_audio_track("Small");
    let late_child = project.add_audio_track("Positive child");
    for (track, sample, parent) in [
        (late_child, 1.0e20, positive),
        (negative, -1.0e20, root),
        (small, 0.25, root),
    ] {
        let asset = project.add_asset(AudioBuffer::new(vec![[sample; 2]]).unwrap());
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 1,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        project.set_track_group(track, Some(parent)).unwrap();
    }
    project.set_track_group(positive, Some(root)).unwrap();
    let mut output = [[99.0; 2]];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.25; 2]]);
}

#[test]
fn silent_groups_suppress_overflowing_descendants() {
    let (mut project, audible, clip) = one_clip_project(vec![[0.25, 0.5]]);
    project.add_audio_clip(audible, clip.clone()).unwrap();
    let loud = project.add_asset(AudioBuffer::new(vec![[f32::MAX; 2]]).unwrap());
    for mixer in [
        MixerSettings::new(0.0, 0.0, AudioOutput::Master).unwrap(),
        MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
    ] {
        let outer = project.add_group_track("Silent root");
        let inner = project.add_group_track("Nested bus");
        let track = project.add_audio_track("Overflowing child");
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset: loud,
                    gain: 2.0,
                    ..clip.clone()
                },
            )
            .unwrap();
        project.set_track_group(track, Some(inner)).unwrap();
        project.set_track_group(inner, Some(outer)).unwrap();
        project.set_group_track_mixer(outer, mixer).unwrap();
    }
    let mut output = [[99.0; 2]];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.25, 0.5]]);
}

#[test]
fn group_balance_cuts_overflowing_channel_without_crossfeed() {
    let (mut project, track, clip) = one_clip_project(vec![[0.25, f32::MAX]]);
    project
        .add_audio_clip(track, AudioClip { gain: 2.0, ..clip })
        .unwrap();
    let group = project.add_group_track("Left only");
    project.set_track_group(track, Some(group)).unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(1.0, -1.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let mut output = [[99.0; 2]];
    render_block(&project, 0, &mut output).unwrap();
    assert_eq!(output, [[0.5, 0.0]]);
}

#[test]
fn rebuilding_after_reparenting_uses_the_new_bus_and_can_ungroup() {
    let (mut project, track, clip) = one_clip_project(vec![[1.0; 2]]);
    project.add_audio_clip(track, clip).unwrap();
    let quiet = project.add_group_track("Quiet");
    let loud = project.add_group_track("Loud");
    project
        .set_group_track_mixer(
            quiet,
            MixerSettings::new(0.25, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            loud,
            MixerSettings::new(2.0, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    for (parent, expected) in [(Some(quiet), 0.25), (Some(loud), 2.0), (None, 1.0)] {
        project.set_track_group(track, parent).unwrap();
        let mut renderer = Renderer::new(&project).unwrap();
        let mut output = [[99.0; 2]];
        renderer.render_block(0, &mut output);
        assert_eq!(output, [[expected; 2]]);
        renderer.render_block(1, &mut output);
        assert_eq!(output, [[0.0; 2]], "the reusable bus buffers must clear");
    }
}

#[test]
fn empty_groups_and_empty_grouped_midi_tracks_are_silent() {
    let mut project = Project::new();
    let outer = project.add_group_track("Outer");
    let inner = project.add_group_track("Empty");
    let midi = project.add_midi_track("Empty MIDI");
    project.set_track_group(inner, Some(outer)).unwrap();
    project.set_track_group(midi, Some(inner)).unwrap();
    let mut output = [[99.0; 2]; 600];
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]; 600]);
}

#[test]
fn instrument_audio_is_silenced_by_a_disconnected_or_zero_gain_group() {
    let mut project = Project::new();
    let group = project.add_group_track("Disconnected");
    let midi = project.add_midi_track("MIDI");
    project.set_track_group(midi, Some(group)).unwrap();
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 480,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 480,
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
    let mut output = [[0.0; 2]; 600];
    render_block(&project, 0, &mut output).unwrap();
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    for mixer in [
        MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        MixerSettings::new(0.0, 0.0, AudioOutput::Master).unwrap(),
    ] {
        project.set_group_track_mixer(group, mixer).unwrap();
        output.fill([99.0; 2]);
        Renderer::new(&project)
            .unwrap()
            .render_block(0, &mut output);
        assert_eq!(output, [[0.0; 2]; 600]);
    }
}

#[test]
fn instrument_audio_is_silenced_by_the_master_gain_or_output() {
    let mut project = Project::new();
    let midi = project.add_midi_track("MIDI");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 480,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 480,
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
    let mut output = [[0.0; 2]; 600];
    render_block(&project, 0, &mut output).unwrap();
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    for mixer in [
        MixerSettings::new(0.0, 0.0, AudioOutput::Output).unwrap(),
        MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
    ] {
        project.set_master_track_mixer(mixer).unwrap();
        output.fill([99.0; 2]);
        render_block(&project, 0, &mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 600]);
    }
}

#[test]
fn nested_groups_and_master_preserve_samples_across_block_sizes_and_timeline_end() {
    let samples = (0..6143)
        .map(|i| [i as f32 / 1024.0, -(i as f32) / 512.0])
        .collect();
    let (mut project, first, clip) = one_clip_project(samples);
    project
        .add_audio_clip(
            first,
            AudioClip {
                start_tick: u64::MAX - 6143,
                ..clip.clone()
            },
        )
        .unwrap();
    let outer = project.add_group_track("Outer");
    let inner = project.add_group_track("Inner");
    let second = project.add_audio_track("Second");
    project
        .add_audio_clip(
            second,
            AudioClip {
                start_tick: u64::MAX - 2047,
                source_start: 13,
                frame_count: 2000,
                gain: -0.375,
                ..clip
            },
        )
        .unwrap();
    project.set_track_group(first, Some(inner)).unwrap();
    project.set_track_group(second, Some(outer)).unwrap();
    project
        .set_group_track_mixer(
            inner,
            MixerSettings::new(0.7, -0.3, AudioOutput::Group(outer)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            outer,
            MixerSettings::new(0.6, 0.9, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.75, -0.2, AudioOutput::Output).unwrap())
        .unwrap();
    let start = u64::MAX - 607;
    let mut reference = [[99.0; 2]; 900];
    Renderer::new(&project)
        .unwrap()
        .render_block(start, &mut reference);
    assert!(reference[607..].iter().all(|frame| *frame == [0.0; 2]));
    for block_size in [1, 2, 7, 128, 255, 256, 257, 512, 1024] {
        let mut renderer = Renderer::new(&project).unwrap();
        let mut actual = [[0.0; 2]; 900];
        for (index, block) in actual.chunks_mut(block_size).enumerate() {
            if let Some(position) = start.checked_add((index * block_size) as u64) {
                renderer.render_block(position, block);
            }
        }
        assert_eq!(actual, reference, "block size {block_size}");
    }
}

#[test]
fn deeply_nested_groups_render_without_recursive_traversal() {
    let (mut project, track, clip) = one_clip_project(vec![[0.25, -0.5]]);
    project.add_audio_clip(track, clip).unwrap();
    let mut parent = project.add_group_track("First");
    project.set_track_group(track, Some(parent)).unwrap();
    for _ in 0..4096 {
        let outer = project.add_group_track("Outer");
        project.set_track_group(parent, Some(outer)).unwrap();
        parent = outer;
    }
    let mut output = [[99.0; 2]];
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.25, -0.5]]);
}

#[test]
fn reusable_group_and_master_buffers_support_seeking_and_changing_block_lengths() {
    let samples = (0..600)
        .map(|i| [i as f32 / 1024.0, -(i as f32) / 512.0])
        .collect();
    let (mut project, track, clip) = one_clip_project(samples);
    project
        .add_audio_clip(
            track,
            AudioClip {
                start_tick: 7,
                ..clip
            },
        )
        .unwrap();
    let group = project.add_group_track("Bus");
    project.set_track_group(track, Some(group)).unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(0.5, 0.5, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.5, -0.5, AudioOutput::Output).unwrap())
        .unwrap();
    let mut renderer = Renderer::new(&project).unwrap();
    for (position, length) in [
        (0, 700),
        (11, 3),
        (700, 513),
        (0, 259),
        (599, 1),
        (5, 0),
        (6, 600),
    ] {
        let mut output = vec![[99.0; 2]; length];
        renderer.render_block(position, &mut output);
        for (index, actual) in output.into_iter().enumerate() {
            let expected = match (position + index as u64).checked_sub(7) {
                Some(source) if source < 600 => [source as f32 / 8192.0, -(source as f32) / 4096.0],
                _ => [0.0; 2],
            };
            assert_eq!(
                actual, expected,
                "position {position}, length {length}, sample {index}"
            );
        }
    }
}
