use std::f32::consts::TAU;

use sympoie::Frame;
use sympoie::bounce::{BounceMode, BouncePlan, apply_bounce};
use sympoie::clip::AudioClip;
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::render::Renderer;
use sympoie::track::{AudioTrackId, Track};
use sympoie::transport::Transport;

fn fixture(pitch_semitones: f32) -> (Project, AudioTrackId, AudioClip) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let samples = (0..16_384)
        .map(|index| {
            let sample = (TAU * 440.0 * index as f32 / 48_000.0).sin() * 0.3;
            [sample, sample * -0.5]
        })
        .collect();
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    let track = project.add_audio_track("Pitch");
    let clip = AudioClip {
        asset,
        source_start: 0,
        start_tick: 0,
        frame_count: 16_384,
        gain: 0.75,
        pitch_semitones,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
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
fn pitch_validation_is_atomic_and_keeps_the_source_and_window() {
    let (mut project, track, original) = fixture(0.0);
    let asset = project.shared_audio_asset(original.asset).unwrap();
    for semitones in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -24.1, 24.1] {
        assert!(matches!(
            project.set_audio_clip_pitch(track, 0, semitones),
            Err(ProjectError::InvalidClipPitch)
        ));
        assert!(matches!(
            project.add_audio_clip(
                track,
                AudioClip {
                    pitch_semitones: semitones,
                    ..original
                }
            ),
            Err(ProjectError::InvalidClipPitch)
        ));
        assert_eq!(project.audio_track(track).unwrap().clips(), &[original]);
    }
    for semitones in [-24.0, 0.25, 24.0] {
        project.set_audio_clip_pitch(track, 0, semitones).unwrap();
        assert_eq!(
            project.audio_track(track).unwrap().clips(),
            &[AudioClip {
                pitch_semitones: semitones,
                ..original
            }]
        );
        assert_eq!(project.audio_duration_frames(), original.frame_count);
        assert!(std::sync::Arc::ptr_eq(
            &asset,
            &project.shared_audio_asset(original.asset).unwrap()
        ));
    }
}

#[test]
fn pitched_split_and_trim_are_exact_windows_of_the_processed_source() {
    let (project, track, clip) = fixture(7.5);
    let expected = render(&project, 16_400, 16_400);

    let mut split = project.clone();
    split.remove_audio_clip(track, 0).unwrap();
    for (start, frames) in [(0, 4057), (4057, 6682), (10_739, 5645)] {
        split
            .add_audio_clip(
                track,
                AudioClip {
                    source_start: start,
                    start_tick: start,
                    frame_count: frames,
                    ..clip
                },
            )
            .unwrap();
    }
    assert_eq!(render(&split, expected.len(), 137), expected);

    let mut trimmed = project.clone();
    trimmed.remove_audio_clip(track, 0).unwrap();
    trimmed
        .add_audio_clip(
            track,
            AudioClip {
                source_start: 4057,
                start_tick: 100,
                frame_count: 6682,
                ..clip
            },
        )
        .unwrap();
    let heard = render(&trimmed, 7000, 113);
    assert_eq!(&heard[..100], &[[0.0; 2]; 100]);
    assert_eq!(&heard[100..6782], &expected[4057..10_739]);
    assert!(heard[6782..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn pitch_is_shared_by_playback_seeking_offline_render_and_bounce() {
    let (project, track, clip) = fixture(-12.0);
    let expected = render(&project, clip.frame_count as usize, 4096);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut played = vec![[0.0; 2]; expected.len()];
    for chunk in played.chunks_mut(101) {
        transport.process_block(chunk);
    }
    assert_eq!(played, expected);
    let mut seek = [[0.0; 2]; 501];
    Renderer::new(&project)
        .unwrap()
        .render_block(3003, &mut seek);
    assert_eq!(seek.as_slice(), &expected[3003..3504]);

    let plan = BouncePlan::new(&project, 1).unwrap();
    let bounced = render(&plan.project, plan.end_frame as usize, 211);
    assert_eq!(bounced, expected);
    let mut replaced = project.clone();
    apply_bounce(
        &mut replaced,
        &plan,
        AudioBuffer::new(bounced).unwrap(),
        BounceMode::InPlace,
    )
    .unwrap();
    let Track::Audio(audio) = &replaced.tracks()[1] else {
        panic!("expected audio")
    };
    assert_eq!(audio.clips()[0].pitch_semitones, 0.0);
    assert_eq!(render(&replaced, expected.len(), 67), expected);

    let mut edited = project.clone();
    edited.set_audio_clip_pitch(track, 0, 12.0).unwrap();
    let mut renderer = Renderer::new(&project).unwrap();
    assert!(!renderer.update_parameters(&edited));
    assert!(!renderer.update_instruments(&edited));
}

#[test]
fn background_preparation_pins_every_distinct_pitch_and_skips_zero_pitch() {
    let (mut project, track, clip) = fixture(0.0);
    assert!(project.audio_pitch_is_prepared());
    assert!(project.cached_audio_pitch().unwrap().is_empty());
    assert!(
        project
            .prepare_audio_pitch(|| false)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(project.prepare_audio_pitch(|| true).unwrap().is_none());

    for pitch_semitones in [1.0, 2.0, 3.0, 1.0] {
        project
            .add_audio_clip(
                track,
                AudioClip {
                    pitch_semitones,
                    ..clip
                },
            )
            .unwrap();
    }
    assert!(!project.audio_pitch_is_prepared());
    assert!(project.cached_audio_pitch().is_none());
    assert!(project.prepare_audio_pitch(|| true).unwrap().is_none());
    assert!(!project.audio_pitch_is_prepared());

    let pins = project.prepare_audio_pitch(|| false).unwrap().unwrap();
    assert_eq!(pins.len(), 3);
    assert!(project.audio_pitch_is_prepared());
    let cached = project.cached_audio_pitch().unwrap();
    assert!(
        pins.iter()
            .zip(&cached)
            .all(|(first, second)| std::sync::Arc::ptr_eq(first, second))
    );
    let again = project.prepare_audio_pitch(|| false).unwrap().unwrap();
    assert!(
        pins.iter()
            .zip(&again)
            .all(|(first, second)| std::sync::Arc::ptr_eq(first, second))
    );
    let expected = render(&project, clip.frame_count as usize, 1024);
    assert_eq!(expected, render(&project, clip.frame_count as usize, 97));
}

#[cfg(not(feature = "native-pitch"))]
#[test]
fn builds_without_native_engines_report_unavailable_processing_without_fallback() {
    use sympoie::pitch::PitchAlgorithm;
    for algorithm in [PitchAlgorithm::Signalsmith, PitchAlgorithm::RubberBandR3] {
        let (mut project, track, _) = fixture(7.0);
        project
            .set_audio_clip_pitch_options(track, 0, algorithm, true)
            .unwrap();
        let error = project.prepare_audio_pitch(|| false).unwrap_err();
        assert!(error.to_string().contains(algorithm.label()));
        assert!(error.to_string().contains("native-pitch"));
        assert!(Renderer::new(&project).is_err());
        assert!(project.cached_audio_pitch().is_none());
        project.set_audio_clip_pitch(track, 0, 0.0).unwrap();
        assert!(Renderer::new(&project).is_ok());
    }
}
