use super::*;
use sympoie::clip::AudioClip;
use sympoie::project::AudioBuffer;

fn project(bpm: f64) -> Project {
    let mut project = Project::new();
    project.set_bpm(bpm).unwrap();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 32]).unwrap());
    let track = project.add_audio_track("Wait-state transport");
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 32,
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

fn pending(project: Project) -> (PitchPreparation, Sender<Message>, Receiver<Message>) {
    let (sender, receiver) = mpsc::channel();
    let mut preparation = PitchPreparation::new(sender.clone());
    preparation.worker.shutdown();
    preparation.generation = 7;
    preparation.pending = Some(PendingPitch {
        snapshot: ProjectSnapshot::new(project, &PluginRuntime::new()),
        force_rebuild: true,
        replace: true,
        generation: 7,
    });
    (preparation, sender, receiver)
}

fn finish_after_commands(project: Project, commands: Vec<Message>) -> Cursor {
    let (mut preparation, sender, receiver) = pending(project);
    for command in commands {
        sender.send(command).unwrap();
    }
    sender
        .send(Message::PitchReady {
            generation: 7,
            pins: Vec::new(),
        })
        .unwrap();
    let mut cursor = Cursor::default();
    assert!(matches!(
        wait_for_pitch(&mut cursor, &receiver, &Shared::new(), &mut preparation),
        Next::Update(_)
    ));
    cursor
}

#[test]
fn waiting_play_at_or_past_the_end_preserves_the_requested_position() {
    for frame in [32, 100] {
        let cursor = finish_after_commands(
            project(3000.0),
            vec![
                Message::Transport(TransportCommand::SeekFrame(frame)),
                Message::Transport(TransportCommand::Play),
            ],
        );
        assert_eq!(cursor.frame, frame);
        assert_eq!(cursor.state, PlaybackState::Playing);
    }
    let cursor = finish_after_commands(
        project(3000.0),
        vec![Message::PlayFromTick {
            tick: 100,
            toggle: false,
        }],
    );
    assert_eq!(cursor.frame, 100);
    assert_eq!(cursor.state, PlaybackState::Playing);
}

#[test]
fn waiting_play_preserves_the_cursor_when_a_loop_will_wrap_it() {
    let cursor = finish_after_commands(
        project(3000.0),
        vec![
            Message::Transport(TransportCommand::SetLoopTicks { start: 8, end: 24 }),
            Message::Transport(TransportCommand::SeekFrame(100)),
            Message::Transport(TransportCommand::Play),
        ],
    );
    assert_eq!(cursor.frame, 100);
    assert_eq!(cursor.loop_ticks, Some((8, 24)));
    assert_eq!(cursor.state, PlaybackState::Playing);
}

#[test]
fn invalid_waiting_loop_edits_preserve_the_last_valid_loop() {
    let cursor = finish_after_commands(
        project(120.0),
        vec![
            Message::Transport(TransportCommand::SetLoopTicks { start: 8, end: 24 }),
            Message::Transport(TransportCommand::SetLoopTicks { start: 24, end: 8 }),
            Message::Transport(TransportCommand::SetLoopTicks { start: 8, end: 8 }),
            Message::Transport(TransportCommand::SetLoopTicks {
                start: u64::MAX - 1,
                end: u64::MAX,
            }),
        ],
    );
    assert_eq!(cursor.loop_ticks, Some((8, 24)));
}

#[test]
fn tempo_changes_during_initial_preparation_preserve_seek_ticks() {
    let initial = project(120.0);
    let expected_tick = 960;
    let mut changed = initial.clone();
    changed.set_bpm(240.0).unwrap();
    let expected_frame = changed.tempo().ticks_to_frames(expected_tick).unwrap();
    let (mut preparation, sender, receiver) = pending(initial);
    sender
        .send(Message::Transport(TransportCommand::SeekTick(
            expected_tick,
        )))
        .unwrap();
    sender
        .send(Message::Transport(TransportCommand::Pause))
        .unwrap();
    sender
        .send(Message::Update {
            snapshot: ProjectSnapshot::new(changed, &PluginRuntime::new()),
            force_rebuild: false,
        })
        .unwrap();
    let mut cursor = Cursor::default();
    let Next::Update(snapshot) =
        wait_for_pitch(&mut cursor, &receiver, &Shared::new(), &mut preparation)
    else {
        panic!("the updated snapshot should be ready without DSP");
    };
    assert_eq!(snapshot.project.bpm(), 240.0);
    assert_eq!(cursor.frame, expected_frame);
    assert_eq!(cursor.state, PlaybackState::Paused);
}
