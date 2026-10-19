use super::*;
use std::time::Instant;
use sympoie::clip::AudioClip;
use sympoie::project::AudioBuffer;
use sympoie::track::AudioTrackId;

const DEADLINE: Duration = Duration::from_secs(2);

fn audio_project(frames: usize, pitch: f32) -> (Project, AudioTrackId) {
    let mut project = Project::new();
    let samples = (0..frames)
        .map(|index| {
            let value = 0.25 + (index % 512) as f32 / 4096.0;
            [value, -value]
        })
        .collect();
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    let track = project.add_audio_track("Pitch preparation");
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: frames as u64,
                gain: 1.0,
                pitch_semitones: pitch,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    (project, track)
}

fn snapshot(project: Project) -> ProjectSnapshot {
    ProjectSnapshot::new(project, &PluginRuntime::new())
}

fn controller() -> PitchPreparation {
    let (sender, _receiver) = mpsc::channel();
    let mut pitch = PitchPreparation::new(sender);
    pitch.worker.shutdown();
    pitch
}

fn synchronize(engine: &AudioEngine) {
    let (sender, receiver) = mpsc::channel();
    engine.sender.send(Message::Synchronize(sender)).unwrap();
    receiver
        .recv_timeout(DEADLINE)
        .expect("pitch preparation blocked the playback command queue");
}

fn drain(shared: &Shared, frames: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; frames];
    let mut filled = 0;
    let deadline = Instant::now() + DEADLINE;
    while filled < frames && Instant::now() < deadline {
        filled += shared.consume(&mut output[filled..]);
        if filled < frames {
            thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(filled, frames, "pitch preparation starved playback");
    output
}

fn expected_frames(start: usize, frames: usize) -> Vec<Frame> {
    (start..start + frames)
        .map(|index| {
            let value = 0.25 + (index % 512) as f32 / 4096.0;
            [value, -value]
        })
        .collect()
}

#[test]
fn pending_pitch_keeps_structural_rebuild_and_accepts_only_latest_completion() {
    let shared = Shared::new();
    let mut pitch = controller();
    let (mut project, track) = audio_project(32, 1.0);
    assert!(
        pitch
            .submit(snapshot(project.clone()), true, false, &shared)
            .is_none()
    );
    let obsolete = pitch.generation;
    project.set_audio_clip_pitch(track, 0, 2.0).unwrap();
    assert!(
        pitch
            .submit(snapshot(project), false, false, &shared)
            .is_none()
    );
    let latest = pitch.generation;
    assert!(pitch.finish(obsolete, Vec::new(), &shared).is_none());
    assert!(shared.preparing_pitch.load(Ordering::Relaxed));

    let audio: Arc<[Frame]> = vec![[0.25; 2]; 32].into();
    let ready = pitch
        .finish(latest, vec![Arc::clone(&audio)], &shared)
        .unwrap();
    assert!(ready.force_rebuild);
    assert!(!ready.replace);
    assert_eq!(
        ready.snapshot.project.audio_track(track).unwrap().clips()[0].pitch_semitones,
        2.0
    );
    assert!(Arc::ptr_eq(&ready.snapshot.pitch_pins[0], &audio));
    assert!(pitch.pending.is_none());
    assert!(!shared.preparing_pitch.load(Ordering::Relaxed));
    assert!(pitch.finish(latest, Vec::new(), &shared).is_none());
}

#[test]
fn replacement_discards_old_completion_and_keeps_replacement_intent_after_edits() {
    let shared = Shared::new();
    let mut pitch = controller();
    let (old_project, _) = audio_project(32, 1.0);
    assert!(
        pitch
            .submit(snapshot(old_project), false, false, &shared)
            .is_none()
    );
    let obsolete = pitch.generation;

    let (mut replacement, track) = audio_project(64, 7.0);
    assert!(
        pitch
            .submit(snapshot(replacement.clone()), true, true, &shared)
            .is_none()
    );
    let replaced = pitch.generation;
    replacement.set_audio_clip_pitch(track, 0, 8.0).unwrap();
    assert!(
        pitch
            .submit(snapshot(replacement), false, false, &shared)
            .is_none()
    );
    assert!(pitch.finish(obsolete, Vec::new(), &shared).is_none());
    assert!(pitch.finish(replaced, Vec::new(), &shared).is_none());
    let ready = pitch.finish(pitch.generation, Vec::new(), &shared).unwrap();
    assert!(ready.replace);
    assert!(ready.force_rebuild);
    let clip = ready.snapshot.project.audio_track(track).unwrap().clips()[0].clone();
    assert_eq!(clip.frame_count, 64);
    assert_eq!(clip.pitch_semitones, 8.0);
}

#[test]
fn reset_pitch_is_immediately_ready_and_obsolete_completion_cannot_restore_it() {
    let shared = Shared::new();
    let mut pitch = controller();
    let (mut project, track) = audio_project(32, -12.0);
    assert!(
        pitch
            .submit(snapshot(project.clone()), true, false, &shared)
            .is_none()
    );
    let obsolete = pitch.generation;
    project.set_audio_clip_pitch(track, 0, 0.0).unwrap();
    let ready = pitch
        .submit(snapshot(project), false, false, &shared)
        .unwrap();
    assert!(ready.force_rebuild);
    assert!(ready.snapshot.pitch_pins.is_empty());
    assert!(pitch.pending.is_none());
    assert!(!shared.preparing_pitch.load(Ordering::Relaxed));
    assert!(pitch.finish(obsolete, Vec::new(), &shared).is_none());
}

#[test]
fn long_source_pitch_keeps_audio_and_commands_running_and_shutdown_cancels_work() {
    for algorithm in [
        sympoie::pitch::PitchAlgorithm::Builtin,
        sympoie::pitch::PitchAlgorithm::Signalsmith,
        sympoie::pitch::PitchAlgorithm::RubberBandR3,
    ] {
        check_long_source_responsiveness(algorithm);
    }
}

fn check_long_source_responsiveness(algorithm: sympoie::pitch::PitchAlgorithm) {
    let (mut project, track) = audio_project(30 * sympoie::SAMPLE_RATE as usize, 0.0);
    project
        .set_audio_clip_pitch_options(track, 0, algorithm, true)
        .unwrap();
    let mut engine = AudioEngine::start(project.clone());
    let shared = engine.shared();
    engine.command(TransportCommand::Play);
    synchronize(&engine);
    assert_eq!(drain(&shared, 4096), expected_frames(0, 4096));

    let mut edited = project.clone();
    edited.set_audio_clip_pitch(track, 0, 7.25).unwrap();
    engine.update_project(edited.clone());
    synchronize(&engine);
    assert!(engine.is_preparing_pitch());
    assert_eq!(drain(&shared, 8192), expected_frames(4096, 8192));
    engine.command(TransportCommand::Pause);
    engine.command(TransportCommand::SeekFrame(24_000));
    synchronize(&engine);
    assert_eq!(engine.status().state, PlaybackState::Paused);
    assert_eq!(engine.status().position_frame, 24_000);

    for semitones in -12..=12 {
        edited
            .set_audio_clip_pitch(track, 0, semitones as f32 + 0.25)
            .unwrap();
        engine.update_project(edited.clone());
    }
    engine.update_project(project);
    synchronize(&engine);
    assert!(!engine.is_preparing_pitch());
    engine.command(TransportCommand::Play);
    synchronize(&engine);
    assert_eq!(drain(&shared, 1024), expected_frames(24_000, 1024));
    assert_eq!(engine.status().render_error, None);

    edited.set_audio_clip_pitch(track, 0, 19.5).unwrap();
    engine.update_project(edited);
    synchronize(&engine);
    assert!(engine.is_preparing_pitch());
    let closing = Instant::now();
    engine.shutdown();
    assert!(
        closing.elapsed() < DEADLINE,
        "closing waited for the entire source to be pitched"
    );
}

#[test]
fn completed_pitch_preparation_plays_the_same_audio_as_offline_rendering() {
    let (mut project, track) = audio_project(24_000, 0.0);
    let engine = AudioEngine::start(project.clone());
    synchronize(&engine);
    project.set_audio_clip_pitch(track, 0, 7.25).unwrap();
    engine.update_project(project.clone());
    synchronize(&engine);
    let deadline = Instant::now() + DEADLINE;
    while engine.is_preparing_pitch() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        !engine.is_preparing_pitch(),
        "pitch preparation never completed"
    );
    synchronize(&engine);

    let mut expected = vec![[0.0; 2]; 8192];
    sympoie::render::Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut expected);
    assert_ne!(expected, expected_frames(0, expected.len()));
    engine.command(TransportCommand::Play);
    synchronize(&engine);
    assert_eq!(drain(&engine.shared(), expected.len()), expected);
    assert_eq!(engine.status().render_error, None);
}

#[test]
fn replacing_while_pitch_prepares_resets_once_and_preserves_new_transport_commands() {
    let (project, track) = audio_project(30 * sympoie::SAMPLE_RATE as usize, 0.0);
    let engine = AudioEngine::start(project.clone());
    let shared = engine.shared();
    engine.command(TransportCommand::Play);
    synchronize(&engine);
    drain(&shared, 512);
    let mut edited = project;
    edited.set_audio_clip_pitch(track, 0, 5.0).unwrap();
    engine.update_project(edited);
    synchronize(&engine);
    assert!(engine.is_preparing_pitch());

    let (mut replacement, track) = audio_project(30 * sympoie::SAMPLE_RATE as usize, -7.0);
    engine.replace_project(replacement.clone());
    synchronize(&engine);
    assert!(engine.is_preparing_pitch());
    assert_eq!(engine.status().state, PlaybackState::Stopped);
    assert_eq!(engine.status().position_frame, 0);
    let mut silence = [[1.0; 2]; 512];
    assert_eq!(shared.consume(&mut silence), 0);
    assert_eq!(silence, [[0.0; 2]; 512]);

    engine
        .sender
        .send(Message::PitchReady {
            generation: 2,
            pins: Vec::new(),
        })
        .unwrap();
    engine.command(TransportCommand::SeekFrame(12_345));
    engine.command(TransportCommand::Pause);
    synchronize(&engine);
    assert!(engine.is_preparing_pitch());
    assert_eq!(engine.status().position_frame, 12_345);
    assert_eq!(engine.status().state, PlaybackState::Paused);

    replacement.set_audio_clip_pitch(track, 0, 0.0).unwrap();
    engine.update_project(replacement);
    synchronize(&engine);
    assert!(!engine.is_preparing_pitch());
    assert_eq!(engine.status().position_frame, 12_345);
    assert_eq!(engine.status().state, PlaybackState::Paused);
    engine.command(TransportCommand::Play);
    synchronize(&engine);
    assert_eq!(drain(&shared, 512), expected_frames(12_345, 512));
}

#[test]
fn native_engine_and_formant_switches_reach_playback_and_match_offline() {
    use sympoie::pitch::PitchAlgorithm;
    let (mut project, track) = audio_project(24_000, 0.0);
    let engine = AudioEngine::start(project.clone());
    synchronize(&engine);
    project.set_audio_clip_pitch(track, 0, 7.25).unwrap();
    let mut outputs = Vec::new();
    for algorithm in [PitchAlgorithm::Signalsmith, PitchAlgorithm::RubberBandR3] {
        for preserve in [false, true] {
            engine.command(TransportCommand::Pause);
            engine.command(TransportCommand::SeekFrame(4096));
            synchronize(&engine);
            project
                .set_audio_clip_pitch_options(track, 0, algorithm, preserve)
                .unwrap();
            engine.update_project(project.clone());
            synchronize(&engine);
            let deadline = Instant::now() + Duration::from_secs(10);
            while engine.is_preparing_pitch() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            assert!(!engine.is_preparing_pitch(), "{algorithm:?} did not finish");
            synchronize(&engine);
            assert_eq!(engine.status().render_error, None);
            assert_eq!(engine.status().position_frame, 4096);
            let mut expected = vec![[0.0; 2]; 8192];
            sympoie::render::Renderer::new(&project)
                .unwrap()
                .render_block(4096, &mut expected);
            engine.command(TransportCommand::Play);
            synchronize(&engine);
            assert_eq!(
                drain(&engine.shared(), expected.len()),
                expected,
                "{algorithm:?}, formants={preserve}"
            );
            assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.001));
            outputs.push(expected);
        }
    }
    assert_ne!(
        outputs[0], outputs[2],
        "switching engines must change the processed sound"
    );
    assert_ne!(
        outputs[0], outputs[1],
        "Signalsmith formant control must affect playback"
    );
    assert_ne!(
        outputs[2], outputs[3],
        "Rubber Band formant control must affect playback"
    );
}

#[test]
fn preparation_failure_is_visible_and_an_obsolete_failure_cannot_replace_it() {
    let shared = Shared::new();
    let mut pitch = controller();
    let (mut project, track) = audio_project(32, 1.0);
    pitch.submit(snapshot(project.clone()), true, false, &shared);
    let obsolete = pitch.generation;
    project.set_audio_clip_pitch(track, 0, 2.0).unwrap();
    pitch.submit(snapshot(project.clone()), true, false, &shared);
    pitch.fail(obsolete, "obsolete error".into(), &shared);
    assert!(shared.preparing_pitch.load(Ordering::Relaxed));
    assert!(shared.pitch_error.lock().unwrap().is_none());
    pitch.fail(pitch.generation, "engine failed".into(), &shared);
    assert!(!shared.preparing_pitch.load(Ordering::Relaxed));
    assert_eq!(
        shared.pitch_error.lock().unwrap().as_deref(),
        Some("engine failed")
    );
    project.set_audio_clip_pitch(track, 0, 0.0).unwrap();
    assert!(
        pitch
            .submit(snapshot(project), false, false, &shared)
            .is_some()
    );
    assert!(shared.pitch_error.lock().unwrap().is_none());
}
