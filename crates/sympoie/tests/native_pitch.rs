#![cfg(feature = "native-pitch")]

use std::f32::consts::TAU;
use std::sync::Arc;

use sympoie::Frame;
use sympoie::bounce::{BounceMode, BouncePlan, apply_bounce};
use sympoie::clip::AudioClip;
use sympoie::pitch::PitchAlgorithm;
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::track::{AudioTrackId, Track};
use sympoie::transport::Transport;

const ENGINES: [PitchAlgorithm; 2] = [PitchAlgorithm::Signalsmith, PitchAlgorithm::RubberBandR3];
const FRAMES: usize = 12_000;

fn fixture(
    algorithm: PitchAlgorithm,
    preserve_formants: bool,
) -> (Project, AudioTrackId, AudioClip) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let samples = (0..FRAMES)
        .map(|index| {
            let phase = TAU * 220.0 * index as f32 / 48_000.0;
            let left = 0.2 * phase.sin() + 0.1 * (phase * 3.0).sin();
            let right = 0.15 * (phase + 0.7).sin() + 0.06 * (phase * 5.0).sin();
            [left, right]
        })
        .collect();
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    let track = project.add_audio_track("Native pitch");
    let clip = AudioClip {
        asset,
        source_start: 0,
        start_tick: 0,
        frame_count: FRAMES as u64,
        gain: 0.75,
        pitch_semitones: 7.25,
        pitch_algorithm: algorithm,
        preserve_formants,
    };
    project.add_audio_clip(track, clip).unwrap();
    (project, track, clip)
}

fn render(project: &Project, frames: usize, block_size: usize) -> Vec<Frame> {
    let mut renderer = Renderer::new(project).unwrap();
    let mut output = vec![[0.0; 2]; frames];
    for (index, chunk) in output.chunks_mut(block_size).enumerate() {
        renderer.render_block((index * block_size) as u64, chunk);
    }
    output
}

#[test]
fn native_pitch_split_and_trim_remain_exact_windows_of_the_processed_source() {
    for algorithm in ENGINES {
        let (project, track, clip) = fixture(algorithm, true);
        let expected = render(&project, FRAMES + 16, FRAMES);
        assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.01));

        let mut split = project.clone();
        split.remove_audio_clip(track, 0).unwrap();
        for (start, count) in [(0, 2777), (2777, 5432), (8209, 3791)] {
            split
                .add_audio_clip(
                    track,
                    AudioClip {
                        source_start: start,
                        start_tick: start,
                        frame_count: count,
                        ..clip
                    },
                )
                .unwrap();
        }
        assert_eq!(
            render(&split, expected.len(), 137),
            expected,
            "{algorithm:?}"
        );

        let mut trimmed = project.clone();
        trimmed.remove_audio_clip(track, 0).unwrap();
        trimmed
            .add_audio_clip(
                track,
                AudioClip {
                    source_start: 2777,
                    start_tick: 100,
                    frame_count: 5432,
                    ..clip
                },
            )
            .unwrap();
        let heard = render(&trimmed, 5600, 113);
        assert!(heard[..100].iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(&heard[100..5532], &expected[2777..8209], "{algorithm:?}");
        assert!(heard[5532..].iter().all(|frame| *frame == [0.0; 2]));
    }
}

#[test]
fn native_pitch_playback_seeking_offline_render_and_bounce_are_identical() {
    for algorithm in ENGINES {
        let (project, _, _) = fixture(algorithm, true);
        let expected = render(&project, FRAMES, 4096);
        let mut transport = Transport::new(&project).unwrap();
        transport.play();
        let mut played = vec![[0.0; 2]; FRAMES];
        for chunk in played.chunks_mut(101) {
            transport.process_block(chunk);
        }
        assert_eq!(played, expected, "{algorithm:?}");
        let mut seek = [[0.0; 2]; 501];
        Renderer::new(&project)
            .unwrap()
            .render_block(3003, &mut seek);
        assert_eq!(seek.as_slice(), &expected[3003..3504], "{algorithm:?}");

        let plan = BouncePlan::new(&project, 1).unwrap();
        let bounced = render(&plan.project, plan.end_frame as usize, 211);
        assert_eq!(bounced, expected, "{algorithm:?}");
        let mut replaced = project.clone();
        apply_bounce(
            &mut replaced,
            &plan,
            AudioBuffer::new(bounced).unwrap(),
            BounceMode::InPlace,
        )
        .unwrap();
        let Track::Audio(audio) = &replaced.tracks()[1] else {
            panic!("expected bounced audio");
        };
        assert_eq!(audio.clips()[0].pitch_semitones, 0.0);
        assert_eq!(render(&replaced, FRAMES, 67), expected, "{algorithm:?}");
    }
}

#[test]
fn native_pitch_cache_distinguishes_engines_and_formants_and_pins_all_live_variants() {
    let (mut project, track, clip) = fixture(PitchAlgorithm::Signalsmith, false);
    for (algorithm, preserve_formants) in [
        (PitchAlgorithm::Signalsmith, true),
        (PitchAlgorithm::RubberBandR3, false),
        (PitchAlgorithm::RubberBandR3, true),
        (PitchAlgorithm::Signalsmith, false),
    ] {
        project
            .add_audio_clip(
                track,
                AudioClip {
                    pitch_algorithm: algorithm,
                    preserve_formants,
                    ..clip
                },
            )
            .unwrap();
    }
    assert!(project.cached_audio_pitch().is_none());
    let pins = project.prepare_audio_pitch(|| false).unwrap().unwrap();
    assert_eq!(pins.len(), 4);
    for (index, pin) in pins.iter().enumerate() {
        assert_eq!(pin.len(), FRAMES);
        for other in pins.iter().skip(index + 1) {
            assert!(!Arc::ptr_eq(pin, other));
        }
    }

    assert!(project.audio_pitch_is_prepared());
    for cached in [
        project.cached_audio_pitch().unwrap(),
        project.prepare_audio_pitch(|| false).unwrap().unwrap(),
    ] {
        assert_eq!(cached.len(), pins.len());
        assert!(pins.iter().zip(&cached).all(|(a, b)| Arc::ptr_eq(a, b)));
    }
    assert_eq!(render(&project, FRAMES, 1024), render(&project, FRAMES, 97));
}

#[test]
fn renderer_requires_rebuild_for_algorithm_and_formant_changes_at_the_same_pitch() {
    for algorithm in ENGINES {
        let (project, track, _) = fixture(algorithm, false);
        let other = if algorithm == PitchAlgorithm::Signalsmith {
            PitchAlgorithm::RubberBandR3
        } else {
            PitchAlgorithm::Signalsmith
        };
        let mut renderer = Renderer::new(&project).unwrap();
        for (next, preserve_formants) in [(other, false), (algorithm, true)] {
            let mut edited = project.clone();
            edited
                .set_audio_clip_pitch_options(track, 0, next, preserve_formants)
                .unwrap();
            assert!(
                !renderer.update_parameters(&edited),
                "{algorithm:?} -> {next:?}"
            );
            assert!(
                !renderer.update_instruments(&edited),
                "{algorithm:?} -> {next:?}"
            );
        }
    }
}
