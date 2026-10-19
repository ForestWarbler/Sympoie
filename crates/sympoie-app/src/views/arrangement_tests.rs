use super::*;

struct TrackHarness {
    ctx: egui::Context,
    project: Project,
    state: ArrangementState,
    interaction: Interaction,
    input_catalog: InputCatalog,
    recording: bool,
    selection: Selection,
    modifiers: Modifiers,
    accessibility: Option<egui::accesskit::TreeUpdate>,
    time: f64,
}

impl TrackHarness {
    fn new() -> Self {
        let mut project = Project::new();
        project.add_audio_track("Lead");
        let band = project.add_group_track("Band");
        let bass = project.add_audio_track("Bass");
        let keys = project.add_midi_track("Keys");
        project.add_audio_track("Voice");
        project.set_track_group(bass, Some(band)).unwrap();
        project.set_track_group(keys, Some(band)).unwrap();
        let mut harness = Self {
            ctx: egui::Context::default(),
            project,
            state: ArrangementState {
                row_height: 60.0,
                ..Default::default()
            },
            interaction: Interaction::default(),
            input_catalog: InputCatalog {
                audio: vec![
                    crate::audio::record_input::AudioInputDevice {
                        id: "interface".into(),
                        name: "Test Interface".into(),
                        channels: 4,
                        is_default: true,
                    },
                    crate::audio::record_input::AudioInputDevice {
                        id: "mic".into(),
                        name: "Test Microphone".into(),
                        channels: 1,
                        is_default: false,
                    },
                ],
                midi: vec![crate::audio::record_input::MidiInputDevice {
                    id: "keyboard".into(),
                    name: "Test Keyboard".into(),
                }],
                error: None,
            },
            recording: false,
            selection: Selection::default(),
            modifiers: Modifiers::NONE,
            accessibility: None,
            time: 0.0,
        };
        harness.step(Vec::new());
        harness.step(Vec::new());
        harness
    }

    fn row_rects(&self) -> Vec<(Row, Rect)> {
        rows::display_rows(&self.project, &self.state.folded)
            .into_iter()
            .enumerate()
            .map(|(position, row)| {
                let top = 50.0 + position as f32 * 60.0;
                (
                    row,
                    Rect::from_min_max(pos2(10.0, top), pos2(290.0, top + 60.0)),
                )
            })
            .collect()
    }

    fn step(&mut self, mut events: Vec<egui::Event>) -> Vec<Action> {
        self.time += 1.0 / 60.0;
        events.insert(0, egui::Event::ModifiersChanged(self.modifiers));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(320.0, 440.0))),
            time: Some(self.time),
            focused: true,
            events,
            ..Default::default()
        };
        let status = EngineStatus {
            state: PlaybackState::Stopped,
            position_frame: 0,
            render_error: None,
        };
        let inputs = Inputs {
            project: &self.project,
            status: &status,
            selection: &self.selection,
            loop_region: LoopRegion::default(),
            armed_tracks: &BTreeSet::new(),
            input_catalog: &self.input_catalog,
            recording: self.recording,
            recording_start_tick: None,
        };
        let row_rects = self.row_rects();
        let mut actions = Vec::new();
        let mut output = self.ctx.run_ui(input, |ui| {
            for (row, rect) in &row_rects {
                track_header(
                    ui,
                    *rect,
                    *row,
                    &mut self.state,
                    &inputs,
                    None,
                    &mut self.interaction,
                    &mut actions,
                );
            }
            update_track_drag(
                ui,
                &self.project,
                &row_rects,
                Rect::from_min_max(pos2(10.0, 50.0), pos2(290.0, 400.0)),
                &mut self.interaction,
                &mut actions,
            );
        });
        if let Some(update) = output.platform_output.accesskit_update.take() {
            self.accessibility = Some(update);
        }
        output.textures_delta.clear();
        actions
    }

    fn button(&mut self, pos: Pos2, pressed: bool) -> Vec<Action> {
        self.step(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: self.modifiers,
        }])
    }

    fn begin_drag(&mut self, source: Pos2, destination: Pos2) {
        self.step(vec![egui::Event::PointerMoved(source)]);
        assert!(self.button(source, true).is_empty());
        let actions = self.step(vec![egui::Event::PointerMoved(destination)]);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, Action::MoveTrack { .. }))
        );
    }

    fn click(&mut self, point: Pos2) -> Vec<Action> {
        self.step(vec![egui::Event::PointerMoved(point)]);
        self.button(point, true);
        self.button(point, false)
    }

    fn open_inputs(&mut self, track: usize) {
        let point = self
            .ctx
            .read_response(Id::new(("track-record-input", track)))
            .expect("recording input selector")
            .rect
            .center();
        assert!(self.click(point).is_empty());
        self.step(Vec::new());
    }

    fn labelled(&mut self, label: &str) -> Rect {
        self.ctx.enable_accesskit();
        self.step(Vec::new());
        let update = self.accessibility.as_ref().expect("accessibility tree");
        let bounds = update
            .nodes
            .iter()
            .filter(|(_, node)| node.label() == Some(label))
            .find_map(|(_, node)| node.bounds())
            .unwrap_or_else(|| panic!("missing recording input menu label {label:?}"));
        Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        )
    }
}

#[test]
fn audio_header_selects_device_and_mono_or_stereo_channels() {
    use sympoie::track::{AudioInputChannels, AudioRecordingInput};
    let mut harness = TrackHarness::new();
    harness.open_inputs(1);
    let choice = harness.labelled("Mono 2");
    assert_eq!(
        harness.click(choice.center()),
        vec![Action::SetAudioRecordingInput {
            track: 1,
            input: AudioRecordingInput {
                device: None,
                channels: AudioInputChannels::Mono(1),
            },
        }]
    );
    harness.open_inputs(1);
    let choice = harness.labelled("Stereo 3/4");
    assert_eq!(
        harness.click(choice.center()),
        vec![Action::SetAudioRecordingInput {
            track: 1,
            input: AudioRecordingInput {
                device: None,
                channels: AudioInputChannels::Stereo(2, 3),
            },
        }]
    );
    harness.open_inputs(1);
    let choice = harness.labelled("Test Microphone");
    assert_eq!(
        harness.click(choice.center()),
        vec![Action::SetAudioRecordingInput {
            track: 1,
            input: AudioRecordingInput {
                device: Some("mic".into()),
                channels: AudioInputChannels::Mono(0),
            },
        }]
    );
}

#[test]
fn midi_header_selects_device_and_channel() {
    use sympoie::track::MidiRecordingInput;
    let mut harness = TrackHarness::new();
    harness.open_inputs(4);
    let choice = harness.labelled("Test Keyboard");
    assert_eq!(
        harness.click(choice.center()),
        vec![Action::SetMidiRecordingInput {
            track: 4,
            input: MidiRecordingInput {
                device: Some("keyboard".into()),
                channel: None,
            },
        }]
    );
    harness.open_inputs(4);
    let choice = harness.labelled("Channel 2");
    assert_eq!(
        harness.click(choice.center()),
        vec![Action::SetMidiRecordingInput {
            track: 4,
            input: MidiRecordingInput {
                device: None,
                channel: Some(1),
            },
        }]
    );
}

#[test]
fn recording_input_selector_refreshes_catalog_and_is_disabled_during_take() {
    let mut harness = TrackHarness::new();
    harness.open_inputs(1);
    let choice = harness.labelled("Refresh inputs");
    assert_eq!(
        harness.click(choice.center()),
        vec![Action::RefreshRecordingInputs]
    );
    harness.open_inputs(1);
    harness.recording = true;
    harness.step(Vec::new());
    let point = harness
        .ctx
        .read_response(Id::new(("track-record-input", 1_usize)))
        .unwrap()
        .rect
        .center();
    assert!(!harness.click(point).iter().any(|action| matches!(
        action,
        Action::SetAudioRecordingInput { .. } | Action::SetMidiRecordingInput { .. }
    )));
    assert!(!egui::Popup::is_any_open(&harness.ctx));
    assert!(
        harness
            .ctx
            .read_response(Id::new(("track-record-input", 2_usize)))
            .is_none()
    );
}

#[test]
fn unavailable_recording_source_is_shown_and_preserved_in_the_header() {
    use sympoie::track::{AudioInputChannels, AudioRecordingInput, TrackId};
    let mut harness = TrackHarness::new();
    let TrackId::Audio(track) = harness.project.track_ids().nth(1).unwrap() else {
        panic!("audio track");
    };
    let saved = AudioRecordingInput {
        device: Some("disconnected-interface".into()),
        channels: AudioInputChannels::Mono(3),
    };
    harness
        .project
        .set_audio_recording_input(track, saved.clone())
        .unwrap();
    harness.step(Vec::new());
    harness.open_inputs(1);
    harness.ctx.enable_accesskit();
    harness.step(Vec::new());
    assert!(
        harness
            .accessibility
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .any(|(_, node)| {
                node.value() == Some("Selected device unavailable")
                    || node.label() == Some("Selected device unavailable")
            })
    );
    assert_eq!(
        harness
            .project
            .audio_track(track)
            .unwrap()
            .recording_input(),
        &saved
    );
}

#[test]
fn audio_and_midi_track_headers_toggle_their_record_arm() {
    let mut harness = TrackHarness::new();
    for (index, y) in [(1, 59.0), (4, 239.0)] {
        let point = pos2(239.0, y);
        harness.step(vec![egui::Event::PointerMoved(point)]);
        harness.button(point, true);
        let actions = harness.button(point, false);
        assert_eq!(actions, vec![Action::ToggleTrackArm(index)]);
    }
}

#[test]
fn group_headers_have_no_record_arm() {
    let mut harness = TrackHarness::new();
    let point = pos2(239.0, 119.0);
    harness.step(vec![egui::Event::PointerMoved(point)]);
    harness.button(point, true);
    assert!(
        !harness
            .button(point, false)
            .iter()
            .any(|action| matches!(action, Action::ToggleTrackArm(_)))
    );
}

#[test]
fn track_header_drag_reorders_once_on_release() {
    let mut harness = TrackHarness::new();
    let destination = pos2(100.0, 346.0);
    harness.begin_drag(pos2(100.0, 59.0), destination);
    assert_eq!(harness.interaction.track_drag, Some(1));
    assert!(matches!(
        harness.button(destination, false).as_slice(),
        [Action::MoveTrack {
            track: 1,
            target: edit::TrackDrop::After(5)
        }]
    ));
    assert!(harness.step(Vec::new()).is_empty());
    assert_eq!(harness.interaction.track_drag, None);
}

#[test]
fn group_body_joins_but_edges_keep_the_original_group() {
    let harness = TrackHarness::new();
    let rects = harness.row_rects();
    assert_eq!(
        track_drop_target(&harness.project, 5, &rects, pos2(100.0, 119.0)),
        Some(edit::TrackDrop::IntoGroup(2))
    );
    assert_eq!(
        track_drop_target(&harness.project, 5, &rects, pos2(100.0, 111.0)),
        Some(edit::TrackDrop::Before(2))
    );
    assert_eq!(
        track_drop_target(&harness.project, 1, &rects, pos2(100.0, 168.0)),
        Some(edit::TrackDrop::After(2))
    );
    assert_eq!(
        track_drop_target(&harness.project, 1, &rects, pos2(100.0, 190.0)),
        Some(edit::TrackDrop::After(2))
    );
    assert_eq!(
        track_drop_target(&harness.project, 3, &rects, pos2(100.0, 346.0)),
        None
    );
    assert_eq!(
        track_drop_target(&harness.project, 0, &rects, pos2(100.0, 119.0)),
        None
    );
}

#[test]
fn track_drag_group_entry_and_escape_cancellation() {
    let destination = pos2(100.0, 119.0);
    let mut harness = TrackHarness::new();
    harness.begin_drag(pos2(100.0, 59.0), destination);
    assert!(matches!(
        harness.button(destination, false).as_slice(),
        [Action::MoveTrack {
            track: 1,
            target: edit::TrackDrop::IntoGroup(2)
        }]
    ));

    let mut harness = TrackHarness::new();
    harness.begin_drag(pos2(100.0, 59.0), destination);
    assert!(
        harness
            .step(vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }])
            .is_empty()
    );
    assert!(harness.button(destination, false).is_empty());
    assert_eq!(harness.interaction.track_drag, None);
}

#[test]
fn track_header_controls_do_not_begin_track_moves() {
    for origin in [pos2(278.0, 59.0), pos2(18.0, 119.0), pos2(77.0, 78.0)] {
        let mut harness = TrackHarness::new();
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        harness.button(origin, true);
        harness.step(vec![egui::Event::PointerMoved(origin + vec2(20.0, 25.0))]);
        assert_eq!(
            harness.interaction.track_drag, None,
            "control at {origin:?}"
        );
        assert!(
            !harness
                .button(origin + vec2(20.0, 25.0), false)
                .iter()
                .any(|action| matches!(action, Action::MoveTrack { .. }))
        );
    }
}

#[test]
fn double_click_track_title_or_background_opens_only_that_tracks_devices() {
    for (index, top) in [(1, 50.0), (2, 110.0), (4, 230.0)] {
        for point in [pos2(100.0, top + 9.0), pos2(20.0, top + 50.0)] {
            let mut harness = TrackHarness::new();
            assert_eq!(
                harness.click(point),
                vec![
                    Action::SelectClip(None),
                    Action::SelectTracks(BTreeSet::from([index]))
                ]
            );
            harness.modifiers = Modifiers::SHIFT;
            harness.selection.tracks = BTreeSet::from([1, 2, 4]);
            assert_eq!(
                harness.click(point),
                vec![
                    Action::SelectClip(None),
                    Action::SelectTracks(BTreeSet::from([index])),
                    Action::ShowDevices
                ]
            );
            assert!(harness.state.renaming.is_none());
            assert!(harness.interaction.track_drag.is_none());
        }
    }
}

#[test]
fn double_click_header_controls_does_not_open_devices() {
    for point in [
        pos2(259.0, 59.0),
        pos2(278.0, 59.0),
        pos2(239.0, 59.0),
        pos2(18.0, 119.0),
        pos2(77.0, 78.0),
    ] {
        let mut harness = TrackHarness::new();
        for _ in 0..2 {
            assert!(
                !harness
                    .click(point)
                    .iter()
                    .any(|action| matches!(action, Action::ShowDevices)),
                "control at {point:?}"
            );
        }
    }
}

#[test]
fn context_menu_rename_keeps_text_selection_separate_from_track_drag() {
    let mut harness = TrackHarness::new();
    let origin = pos2(100.0, 59.0);
    harness.step(vec![egui::Event::PointerMoved(origin)]);
    for pressed in [true, false] {
        harness.step(vec![egui::Event::PointerButton {
            pos: origin,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    let rename = harness.labelled("Rename");
    harness.click(rename.center());
    assert_eq!(harness.state.renaming, Some((1, "Lead".to_owned())));
    harness.step(Vec::new());
    harness.button(origin, true);
    harness.step(vec![egui::Event::PointerMoved(origin + vec2(30.0, 0.0))]);
    assert_eq!(harness.interaction.track_drag, None);
    assert!(
        !harness
            .button(origin + vec2(30.0, 0.0), false)
            .iter()
            .any(|action| matches!(action, Action::MoveTrack { .. }))
    );
}

fn timeline() -> Timeline {
    Timeline {
        left: 10.0,
        right: 490.0,
        scroll_tick: 0.0,
        pixels_per_tick: 0.1,
        grid: Grid::every(240),
    }
}

#[test]
fn visual_grid_thinning_does_not_change_playback_start_snapping() {
    let meter = TimeSignature::default();
    let timeline = Timeline {
        pixels_per_tick: 0.03,
        grid: Grid::every(60),
        ..timeline()
    };
    assert_eq!(timeline.visible_grid_interval(meter), 240);
    assert_eq!(timeline.snapped(timeline.x(60.0), true), 60);
    assert_eq!(timeline.snapped(timeline.x(190.0), true), 180);
}

#[test]
fn disabling_snap_keeps_visual_grid_and_allows_any_playback_start() {
    let meter = TimeSignature::default();
    let timeline = Timeline {
        grid: Grid::OFF,
        scroll_tick: 731.0,
        ..timeline()
    };
    assert_eq!(timeline.visible_grid_interval(meter), 240);
    assert_eq!(timeline.snapped(timeline.x(963.0), true), 963);
}

struct PlaygroundHarness {
    ctx: egui::Context,
    project: Project,
    state: ArrangementState,
    status: EngineStatus,
    time: f64,
}

impl PlaygroundHarness {
    fn new() -> Self {
        let mut harness = Self {
            ctx: egui::Context::default(),
            project: Project::new(),
            state: ArrangementState::default(),
            status: EngineStatus {
                state: PlaybackState::Stopped,
                position_frame: 0,
                render_error: None,
            },
            time: 0.0,
        };
        harness.step(Vec::new());
        harness.step(Vec::new());
        harness
    }

    fn step(&mut self, events: Vec<egui::Event>) -> Vec<Action> {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 500.0))),
            time: Some(self.time),
            focused: true,
            events,
            ..Default::default()
        };
        let inputs = Inputs {
            project: &self.project,
            status: &self.status,
            selection: &Selection::default(),
            loop_region: LoopRegion::default(),
            armed_tracks: &BTreeSet::new(),
            input_catalog: &InputCatalog::default(),
            recording: false,
            recording_start_tick: None,
        };
        let mut actions = Vec::new();
        let mut output = self.ctx.run_ui(input, |ui| {
            show(
                ui,
                &mut self.state,
                &inputs,
                &mut WaveformCache::default(),
                &mut [],
                &mut widgets::MeterState::default(),
                &mut actions,
            );
        });
        output.textures_delta.clear();
        actions
    }

    fn click(&mut self, point: Pos2) -> Vec<Action> {
        self.step(vec![egui::Event::PointerMoved(point)]);
        for pressed in [true, false] {
            let actions = self.step(vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
            if !pressed {
                return actions;
            }
        }
        unreachable!()
    }
}

#[test]
fn empty_arrangement_background_places_a_distant_playback_start() {
    let mut harness = PlaygroundHarness::new();
    harness.state.scroll_tick = 500_000.0;
    harness.state.pixels_per_tick = 0.1;
    harness.project.set_snap_grid(SnapGrid::Off);
    harness.state.grid = GridMode::Fixed;
    harness.step(Vec::new());
    let below = harness
        .ctx
        .read_response(Id::new("arrangement-below"))
        .unwrap()
        .rect;
    let point = below.left_top() + vec2(100.0, 20.0);
    assert!(matches!(
        harness.click(point).as_slice(),
        [Action::SelectClip(None), Action::Seek(501_000)]
    ));
}

#[test]
fn double_click_master_title_or_background_opens_master_devices() {
    for offset in [vec2(70.0, 9.0), vec2(20.0, 50.0)] {
        let mut harness = PlaygroundHarness::new();
        let header = harness
            .ctx
            .read_response(Id::new(("header", 0)))
            .unwrap()
            .rect;
        let point = header.min + offset;
        assert_eq!(
            harness.click(point),
            vec![Action::SelectTracks(BTreeSet::from([0]))]
        );
        assert_eq!(
            harness.click(point),
            vec![
                Action::SelectClip(None),
                Action::SelectTracks(BTreeSet::from([0])),
                Action::ShowDevices
            ]
        );
    }
}

#[test]
fn overview_includes_empty_playback_start_playhead_and_browsed_region() {
    let project = Project::new();
    let mut state = ArrangementState {
        follow: false,
        playback_start: 500_000,
        ..ArrangementState::default()
    };
    let total = overview_end_tick(&project, &state, 700_000.0);
    assert!(total > 700_000.0);
    assert_eq!(state.scroll_tick, 0.0);
    state.scroll_tick = 900_000.0;
    assert!(overview_end_tick(&project, &state, 700_000.0) >= 900_000.0 + state.visible_ticks());
    assert!(overview_end_tick(&project, &state, 0.0) > state.playback_start as f64);
}

#[test]
fn empty_playback_follows_into_the_distance_but_manual_pan_keeps_its_position() {
    let mut harness = PlaygroundHarness::new();
    harness.status.state = PlaybackState::Playing;
    harness.status.position_frame = 30_000_000;
    harness.step(Vec::new());
    let followed = harness.state.scroll_tick;
    assert!(followed > 500_000.0);
    harness.step(vec![
        egui::Event::PointerMoved(pos2(HEADER_WIDTH + 200.0, 100.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(-100.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        },
    ]);
    assert!(!harness.state.follow);
    assert!(harness.state.scroll_tick > followed);
    for _ in 0..120 {
        harness.step(Vec::new());
    }
    let browsed = harness.state.scroll_tick;
    harness.status.position_frame *= 2;
    harness.step(Vec::new());
    assert_eq!(harness.state.scroll_tick, browsed);
    harness.state.follow = true;
    harness.step(Vec::new());
    assert!(harness.state.scroll_tick > browsed);
}

#[test]
fn overview_pan_remains_under_manual_control_during_empty_playback() {
    let mut harness = PlaygroundHarness::new();
    harness.status.state = PlaybackState::Playing;
    harness.status.position_frame = 30_000_000;
    harness.step(Vec::new());
    let overview = harness
        .ctx
        .read_response(Id::new("arrangement-overview"))
        .unwrap()
        .rect;
    harness.click(overview.left_center() + vec2(20.0, 0.0));
    assert!(!harness.state.follow);
    let browsed = harness.state.scroll_tick;
    harness.status.position_frame *= 2;
    harness.step(Vec::new());
    assert_eq!(harness.state.scroll_tick, browsed);
}

struct ClipHarness {
    ctx: egui::Context,
    project: Project,
    timeline: Timeline,
    selection: Selection,
    interaction: Interaction,
    time: f64,
    shapes: Vec<egui::epaint::ClippedShape>,
    cursor: CursorIcon,
    modifiers: Modifiers,
}

impl ClipHarness {
    fn new() -> Self {
        let mut project = Project::new();
        let track = project.add_midi_track("Piano");
        project
            .add_midi_clip(
                track,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 480,
                    length_ticks: 3360,
                    notes: Vec::new(),
                    name: None,
                    pitch_bend: Vec::new(),
                },
            )
            .unwrap();
        Self::with_project(project)
    }

    fn with_project(project: Project) -> Self {
        let mut harness = Self {
            ctx: egui::Context::default(),
            project,
            timeline: timeline(),
            selection: Selection::default(),
            interaction: Interaction::default(),
            time: 0.0,
            shapes: Vec::new(),
            cursor: CursorIcon::Default,
            modifiers: Modifiers::NONE,
        };
        harness.step(Vec::new());
        harness.step(Vec::new());
        harness
    }

    fn step(&mut self, mut events: Vec<egui::Event>) -> Vec<Action> {
        self.time += 1.0 / 60.0;
        events.insert(0, egui::Event::ModifiersChanged(self.modifiers));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 220.0))),
            time: Some(self.time),
            focused: true,
            events,
            ..Default::default()
        };
        let status = EngineStatus {
            state: PlaybackState::Stopped,
            position_frame: 0,
            render_error: None,
        };
        let inputs = Inputs {
            project: &self.project,
            status: &status,
            selection: &self.selection,
            loop_region: LoopRegion::default(),
            armed_tracks: &BTreeSet::new(),
            input_catalog: &InputCatalog::default(),
            recording: false,
            recording_start_tick: None,
        };
        let display = rows::display_rows(&self.project, &Default::default());
        let row_rects: Vec<_> = display
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let top = 50.0 + index as f32 * 60.0;
                (
                    *row,
                    Rect::from_min_max(pos2(10.0, top), pos2(490.0, top + 60.0)),
                )
            })
            .collect();
        let mut actions = Vec::new();
        let mut output = self.ctx.run_ui(input, |ui| {
            for (row, lane) in &row_rects {
                lane_background(
                    ui,
                    *lane,
                    *row,
                    &display,
                    &self.timeline,
                    GridMode::Fixed,
                    0,
                    &inputs,
                    &mut self.interaction,
                    &mut actions,
                );
            }
            for (row, lane) in &row_rects {
                clips(
                    ui,
                    &ui.painter().clone(),
                    *lane,
                    *row,
                    &self.timeline,
                    &self.project,
                    &inputs,
                    &mut WaveformCache::default(),
                    &mut self.interaction,
                    true,
                    &mut actions,
                );
            }
            update_range_drag(
                ui,
                &self.timeline,
                &inputs,
                &row_rects,
                &mut self.interaction,
                &mut actions,
            );
        });
        output.textures_delta.clear();
        self.shapes = output.shapes;
        self.cursor = output.platform_output.cursor_icon;
        for action in &actions {
            match action {
                Action::SelectClip(Some(clip)) => self.selection.select_clip(*clip),
                Action::SelectClip(None) => {
                    self.selection.clip = None;
                    self.selection.range = None;
                }
                Action::SelectTracks(tracks) => {
                    self.selection.tracks = tracks.clone();
                    self.selection.clip = None;
                }
                Action::SetTimeSelection(range) => {
                    if let Some(range) = range {
                        self.selection.clip = None;
                        self.selection.tracks = range.tracks.clone();
                    }
                    self.selection.range = range.clone();
                }
                _ => {}
            }
        }
        actions
    }

    fn button(&mut self, pos: Pos2, pressed: bool) -> Vec<Action> {
        self.step(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: self.modifiers,
        }])
    }

    fn click(&mut self, pos: Pos2) -> Vec<Action> {
        self.step(vec![egui::Event::PointerMoved(pos)]);
        assert!(self.button(pos, true).is_empty());
        self.button(pos, false)
    }

    fn release_clip(&mut self, pos: Pos2) -> Vec<Action> {
        let mut actions = self.button(pos, false);
        if let Some((drag, pointer)) = self.interaction.released.take() {
            let row_rects: Vec<_> = rows::display_rows(&self.project, &Default::default())
                .into_iter()
                .enumerate()
                .map(|(index, row)| {
                    let top = 50.0 + index as f32 * 60.0;
                    (
                        row,
                        Rect::from_min_max(pos2(10.0, top), pos2(490.0, top + 60.0)),
                    )
                })
                .collect();
            if let Some(ghost) = ghost(
                &self.project,
                drag,
                &self.timeline,
                pointer,
                &row_rects,
                drag.snap_on(self.modifiers),
            ) {
                commit(drag, ghost, &mut actions);
            }
        }
        actions
    }
}

#[test]
fn clip_body_and_empty_lane_clicks_clear_selection_select_track_and_seek() {
    for tick in [240, 481, 1000, 1090, 3839, 4320] {
        let mut harness = ClipHarness::new();
        harness
            .selection
            .select_clip(ClipRef { track: 1, index: 0 });
        let pos = pos2(timeline().x(tick as f64), 90.0);
        let expected = timeline().snapped(pos.x, true);
        harness.step(vec![egui::Event::PointerMoved(pos)]);
        assert_eq!(harness.cursor, CursorIcon::Default);
        assert!(matches!(
            harness.click(pos).as_slice(),
            [Action::SelectClip(None), Action::SelectTracks(tracks), Action::Seek(tick)]
                if *tick == expected && *tracks == BTreeSet::from([1])
        ));
        assert_eq!(harness.selection.clip, None);
    }
}

#[test]
fn double_click_clip_body_opens_audio_or_midi_clip_without_inserting() {
    for audio in [false, true] {
        let mut harness = colored_clip(audio, 0xffffff);
        let point = pos2(harness.timeline.x(963.0), 90.0);
        assert!(matches!(
            harness.click(point).as_slice(),
            [
                Action::SelectClip(None),
                Action::SelectTracks(_),
                Action::Seek(_)
            ]
        ));
        let clip = ClipRef { track: 1, index: 0 };
        assert_eq!(
            harness.click(point),
            vec![
                Action::SelectClip(Some(clip)),
                if audio {
                    Action::ShowClipView
                } else {
                    Action::OpenPianoRoll(clip)
                }
            ]
        );
    }
}

#[test]
fn double_click_empty_midi_lane_still_creates_a_clip() {
    for tick in [479.0, 3840.0, 4320.0] {
        let mut harness = ClipHarness::new();
        let point = pos2(harness.timeline.x(tick), 90.0);
        harness.click(point);
        let actions = harness.click(point);
        assert_eq!(
            actions.last(),
            Some(&Action::InsertMidiClip {
                track: 1,
                start: SnapGrid::Bar.floor(tick as u64, harness.project.time_signature()),
            })
        );
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, Action::ShowClipView | Action::OpenPianoRoll(_)))
        );
    }
}

#[test]
fn clip_body_hit_testing_uses_unsnapped_bounds_and_the_topmost_clip() {
    let mut harness = ClipHarness::new();
    assert_eq!(clip_at_tick(&harness.project, 1, 479.99), None);
    assert_eq!(
        clip_at_tick(&harness.project, 1, 480.0),
        Some(ClipRef { track: 1, index: 0 })
    );
    assert_eq!(clip_at_tick(&harness.project, 1, 3840.0), None);
    let sympoie::track::TrackId::Midi(track) = harness.project.track_ids().nth(1).unwrap() else {
        unreachable!()
    };
    harness
        .project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 960,
                length_ticks: 960,
                notes: Vec::new(),
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(
        clip_at_tick(&harness.project, 1, 1000.0),
        Some(ClipRef { track: 1, index: 1 })
    );
}

#[test]
fn clip_header_selects_and_opens_at_any_grid_density() {
    for (zoom, offset) in [(0.1, 4.0), (0.03, 0.0), (0.03, 3.0)] {
        let mut harness = ClipHarness::new();
        harness.timeline.pixels_per_tick = zoom;
        harness.step(Vec::new());
        harness.step(Vec::new());
        let pos = pos2(harness.timeline.grid_line_x(960) + offset, 58.0);
        let clip = ClipRef { track: 1, index: 0 };
        harness.step(vec![egui::Event::PointerMoved(pos)]);
        assert_eq!(harness.cursor, CursorIcon::Grab);
        assert!(
            matches!(harness.click(pos).as_slice(), [Action::SelectClip(Some(selected))] if *selected == clip)
        );
        assert!(
            matches!(harness.click(pos).as_slice(), [Action::SelectClip(Some(selected)), Action::OpenPianoRoll(opened)] if *selected == clip && *opened == clip)
        );
        let body_pos = pos2(pos.x, 90.0);
        let expected = harness.timeline.snapped(body_pos.x, true);
        assert!(matches!(
            harness.click(body_pos).as_slice(),
            [Action::SelectClip(None), Action::SelectTracks(_), Action::Seek(tick)]
                if *tick == expected
        ));
    }
}

#[test]
fn clip_body_seek_respects_disabled_snap_and_modifier_bypass() {
    for (grid, modifiers) in [
        (Grid::OFF, Modifiers::NONE),
        (Grid::every(240), Modifiers::ALT),
        (Grid::every(240), Modifiers::COMMAND),
    ] {
        let mut harness = ClipHarness::new();
        harness.timeline.grid = grid;
        harness.modifiers = modifiers;
        let pos = pos2(timeline().x(1003.0), 90.0);
        assert!(matches!(
            harness.click(pos).as_slice(),
            [
                Action::SelectClip(None),
                Action::SelectTracks(_),
                Action::Seek(1003)
            ]
        ));
    }
}

#[test]
fn dragging_clip_header_preserves_move_and_edge_trim_when_crossing_into_body() {
    for (start_tick, offset, mode) in [
        (960, 0.0, ClipMode::Move),
        (480, 1.0, ClipMode::TrimStart),
        (3840, -2.0, ClipMode::TrimEnd),
    ] {
        let mut harness = ClipHarness::new();
        let origin = pos2(timeline().grid_line_x(start_tick) + offset, 58.0);
        let end = origin + vec2(40.0, 32.0);
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        assert!(harness.button(origin, true).is_empty());
        let actions = harness.step(vec![egui::Event::PointerMoved(end)]);
        assert!(matches!(actions.as_slice(), [Action::SelectClip(Some(_))]));
        assert_eq!(harness.interaction.clip_drag.unwrap().mode, mode);
        assert!(harness.button(end, false).is_empty());
        assert_eq!(harness.interaction.released.unwrap().0.mode, mode);
    }
}

#[test]
fn command_drag_copies_audio_and_midi_clips_across_tracks_once_on_release() {
    for audio in [false, true] {
        let mut harness = colored_clip(audio, 0x1a2f96);
        if audio {
            harness.project.add_audio_track("Destination");
        } else {
            harness.project.add_midi_track("Destination");
        }
        harness.step(Vec::new());
        harness.modifiers = Modifiers::COMMAND;
        let origin = pos2(timeline().x(960.0), 58.0);
        let destination = origin + vec2(35.0, 60.0);
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        assert!(harness.button(origin, true).is_empty());
        assert!(matches!(
            harness
                .step(vec![egui::Event::PointerMoved(destination)])
                .as_slice(),
            [Action::SelectClip(Some(ClipRef { track: 1, index: 0 }))]
        ));
        assert!(harness.interaction.clip_drag.unwrap().copy);
        assert_eq!(harness.cursor, CursorIcon::Copy);
        assert!(matches!(
            harness.release_clip(destination).as_slice(),
            [Action::CopyClipTo {
                clip: ClipRef { track: 1, index: 0 },
                to_track: 2,
                start: 240,
            }]
        ));
        assert!(harness.interaction.clip_drag.is_none());
        assert!(harness.step(Vec::new()).is_empty());
    }
}

#[test]
fn clip_copy_drag_bypasses_time_selection_and_follows_command_until_release() {
    let mut harness = ClipHarness::new();
    harness.selection.range = Some(TimeRange {
        start: 480,
        end: 1920,
        tracks: BTreeSet::from([1]),
    });
    harness.modifiers = Modifiers::COMMAND;
    let origin = pos2(timeline().x(960.0), 58.0);
    let destination = origin + vec2(48.0, 0.0);
    harness.step(vec![egui::Event::PointerMoved(origin)]);
    harness.button(origin, true);
    harness.step(vec![egui::Event::PointerMoved(destination)]);
    assert!(harness.interaction.range_move.is_none());
    assert!(harness.interaction.clip_drag.unwrap().copy);
    harness.modifiers = Modifiers::NONE;
    harness.step(Vec::new());
    assert!(!harness.interaction.clip_drag.unwrap().copy);
    assert_eq!(harness.cursor, CursorIcon::Grabbing);
    assert!(matches!(
        harness.release_clip(destination).as_slice(),
        [Action::MoveClip {
            clip: ClipRef { track: 1, index: 0 },
            to_track: 1,
            start: 960,
        }]
    ));
}

#[test]
fn pressing_command_mid_drag_copies_and_releasing_it_moves_again() {
    for release_before_drop in [false, true] {
        let mut harness = ClipHarness::new();
        let origin = pos2(timeline().x(960.0), 58.0);
        let destination = origin + vec2(48.0, 0.0);
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        harness.button(origin, true);
        harness.step(vec![egui::Event::PointerMoved(destination)]);
        assert!(!harness.interaction.clip_drag.unwrap().copy);
        assert_eq!(harness.cursor, CursorIcon::Grabbing);
        harness.modifiers = Modifiers::COMMAND;
        harness.step(Vec::new());
        assert!(harness.interaction.clip_drag.unwrap().copy);
        assert_eq!(harness.cursor, CursorIcon::Copy);
        if release_before_drop {
            harness.modifiers = Modifiers::NONE;
            harness.step(Vec::new());
        }
        let actions = harness.release_clip(destination);
        let clip = ClipRef { track: 1, index: 0 };
        if release_before_drop {
            assert!(matches!(
                actions.as_slice(),
                [Action::MoveClip { clip: moved, to_track: 1, start: 960 }] if *moved == clip
            ));
        } else {
            assert!(matches!(
                actions.as_slice(),
                [Action::CopyClipTo { clip: copied, to_track: 1, start: 960 }] if *copied == clip
            ));
        }
    }
}

#[test]
fn clip_copy_drag_keeps_grid_snapping_unless_alt_is_held() {
    for (modifiers, expected) in [
        (Modifiers::COMMAND, 720),
        (Modifiers::COMMAND | Modifiers::ALT, 830),
    ] {
        let mut harness = ClipHarness::new();
        harness.modifiers = modifiers;
        let origin = pos2(timeline().x(960.0), 58.0);
        let destination = origin + vec2(35.0, 0.0);
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        harness.button(origin, true);
        harness.step(vec![egui::Event::PointerMoved(destination)]);
        assert!(matches!(
            harness.release_clip(destination).as_slice(),
            [Action::CopyClipTo { start, .. }] if *start == expected
        ));
    }
}

#[test]
fn command_clip_edge_drags_still_trim_without_copying() {
    for (origin_tick, mode) in [(480, ClipMode::TrimStart), (3830, ClipMode::TrimEnd)] {
        let mut harness = ClipHarness::new();
        harness.modifiers = Modifiers::COMMAND;
        let origin = pos2(timeline().x(origin_tick as f64), 58.0);
        let destination = origin + vec2(35.0, 0.0);
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        harness.button(origin, true);
        harness.step(vec![egui::Event::PointerMoved(destination)]);
        let drag = harness.interaction.clip_drag.unwrap();
        assert_eq!(drag.mode, mode);
        assert!(!drag.copy);
        let actions = harness.release_clip(destination);
        assert!(matches!(
            actions.as_slice(),
            [Action::TrimClipStart { start: 830, .. }] | [Action::TrimClipEnd { end: 4180, .. }]
        ));
    }
}

#[test]
fn clip_copy_preview_keeps_the_original_header_opaque() {
    for copy in [false, true] {
        let mut harness = ClipHarness::new();
        let origin = pos2(timeline().x(960.0), 58.0);
        harness.modifiers = if copy {
            Modifiers::COMMAND
        } else {
            Modifiers::NONE
        };
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        harness.button(origin, true);
        harness.step(vec![egui::Event::PointerMoved(origin + vec2(48.0, 0.0))]);
        let header = Rect::from_min_max(pos2(58.0, 51.0), pos2(394.0, 64.0));
        let fill = harness
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect) if rect.rect == header => Some(rect.fill),
                _ => None,
            })
            .expect("original clip header must still be visible");
        let color = theme::tint(
            theme::track_color(&harness.project, 1),
            Color32::BLACK,
            0.12,
        );
        assert_eq!(fill, color.gamma_multiply(if copy { 1.0 } else { 0.45 }));
    }
}

#[test]
fn dragging_clip_body_selects_time_even_at_edges_or_inside_existing_selection() {
    for selected_range in [false, true] {
        for start_tick in [481, 960, 3839] {
            let mut harness = ClipHarness::new();
            if selected_range {
                harness.selection.range = Some(TimeRange {
                    start: 480,
                    end: 3840,
                    tracks: BTreeSet::from([1]),
                });
            }
            let origin = pos2(timeline().x(start_tick as f64), 90.0);
            let end = origin + vec2(-30.0, -32.0);
            let expected = TimeRange {
                start: timeline().snapped(end.x, true),
                end: timeline().snapped(origin.x, true),
                tracks: BTreeSet::from([1]),
            };
            harness.step(vec![egui::Event::PointerMoved(origin)]);
            assert!(harness.button(origin, true).is_empty());
            let actions = harness.step(vec![egui::Event::PointerMoved(end)]);
            assert!(matches!(
                actions.as_slice(),
                [Action::SelectClip(None), Action::SetTimeSelection(Some(range))]
                    if *range == expected
            ));
            assert_eq!(harness.selection.range, Some(expected.clone()));
            assert!(harness.interaction.clip_drag.is_none());
            assert!(harness.interaction.range_move.is_none());
            assert!(harness.interaction.range_drag.is_some());
            let actions = harness.button(end, false);
            assert!(matches!(actions.as_slice(), [Action::Seek(tick)] if *tick == expected.start));
            assert!(harness.interaction.range_drag.is_none());
            assert!(harness.interaction.released.is_none());
            assert!(harness.interaction.range_released.is_none());
        }
    }
}

#[test]
fn shift_click_extends_time_selection_over_clip_bodies_and_empty_lanes() {
    for tick in [1920, 4080] {
        let mut harness = ClipHarness::new();
        harness.selection.range = Some(TimeRange {
            start: 480,
            end: 960,
            tracks: BTreeSet::from([1]),
        });
        harness.modifiers = Modifiers::SHIFT;
        let pos = pos2(timeline().x(tick as f64), 90.0);
        assert!(matches!(
            harness.click(pos).as_slice(),
            [Action::SetTimeSelection(Some(range))]
                if range.start == 480 && range.end == tick && range.tracks == BTreeSet::from([1])
        ));
    }
}

#[test]
fn dragging_inside_audio_and_midi_clip_bodies_selects_only_the_requested_time() {
    for audio in [false, true] {
        let mut harness = colored_clip(audio, 0x1a2f96);
        let origin = pos2(timeline().x(960.0), 90.0);
        let end = pos2(timeline().x(1920.0), 90.0);
        harness.step(vec![egui::Event::PointerMoved(origin)]);
        assert!(harness.button(origin, true).is_empty());
        assert!(matches!(
            harness.step(vec![egui::Event::PointerMoved(end)]).as_slice(),
            [Action::SelectClip(None), Action::SetTimeSelection(Some(range))]
                if range.start == 960 && range.end == 1920 && range.tracks == BTreeSet::from([1])
        ));
        assert!(matches!(
            harness.button(end, false).as_slice(),
            [Action::Seek(960)]
        ));
        assert!(harness.interaction.clip_drag.is_none());
        assert!(harness.interaction.range_move.is_none());
    }
}

#[test]
fn time_selection_can_start_in_one_clip_body_and_finish_in_another_track() {
    let mut project = ClipHarness::new().project;
    let second = project.add_midi_track("Second piano");
    project
        .add_midi_clip(
            second,
            MidiClip {
                offset_ticks: 0,
                start_tick: 480,
                length_ticks: 3360,
                notes: Vec::new(),
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let mut harness = ClipHarness::with_project(project);
    let origin = pos2(timeline().x(960.0), 90.0);
    let end = pos2(timeline().x(1920.0), 150.0);
    harness.step(vec![egui::Event::PointerMoved(origin)]);
    assert!(harness.button(origin, true).is_empty());
    assert!(matches!(
        harness.step(vec![egui::Event::PointerMoved(end)]).as_slice(),
        [Action::SelectClip(None), Action::SetTimeSelection(Some(range))]
            if range.start == 960 && range.end == 1920 && range.tracks == BTreeSet::from([1, 2])
    ));
    assert!(matches!(
        harness.button(end, false).as_slice(),
        [Action::Seek(960)]
    ));
}

#[test]
fn dragging_clip_header_inside_time_selection_moves_the_selection() {
    let mut harness = ClipHarness::new();
    let range = TimeRange {
        start: 480,
        end: 1920,
        tracks: BTreeSet::from([1]),
    };
    harness.selection.range = Some(range.clone());
    let origin = pos2(timeline().x(960.0), 58.0);
    let end = origin + vec2(40.0, 32.0);
    harness.step(vec![egui::Event::PointerMoved(origin)]);
    assert!(harness.button(origin, true).is_empty());
    assert!(
        harness
            .step(vec![egui::Event::PointerMoved(end)])
            .is_empty()
    );
    assert_eq!(
        harness.interaction.range_move.as_ref().unwrap().range,
        range
    );
    assert!(harness.interaction.clip_drag.is_none());
    assert!(harness.button(end, false).is_empty());
    assert_eq!(harness.interaction.range_released.unwrap().0.range, range);
}

fn colored_clip(audio: bool, color: u32) -> ClipHarness {
    let mut project = Project::new();
    let track: sympoie::track::TrackId = if audio {
        let track = project.add_audio_track("Audio");
        let frames = (4560.0 * 3000.0 / project.bpm()) as usize;
        let asset = project
            .add_asset(sympoie::project::AudioBuffer::new(vec![[0.5, -0.5]; frames]).unwrap());
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: frames as u64,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        track.into()
    } else {
        let track = project.add_midi_track("MIDI");
        project
            .add_midi_clip(
                track,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 0,
                    length_ticks: 4560,
                    notes: vec![sympoie::clip::MidiNote {
                        start_tick: 240,
                        duration_ticks: 4000,
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
        track.into()
    };
    project.set_track_color(track, Some(color)).unwrap();
    ClipHarness::with_project(project)
}

#[test]
fn audio_clip_header_opens_clip_view_at_grid_line() {
    let mut harness = colored_clip(true, 0xffffff);
    let pos = pos2(harness.timeline.grid_line_x(960), 58.0);
    assert!(matches!(
        harness.click(pos).as_slice(),
        [Action::SelectClip(Some(_))]
    ));
    assert!(matches!(
        harness.click(pos).as_slice(),
        [Action::SelectClip(Some(_)), Action::ShowClipView]
    ));
}

#[test]
fn clip_grid_overlays_contents_with_contrast_and_stays_below_title() {
    for audio in [false, true] {
        for color in [0xffffff, 0x1a2f96] {
            for selected in [false, true] {
                let mut harness = colored_clip(audio, color);
                if selected {
                    harness
                        .selection
                        .select_clip(ClipRef { track: 1, index: 0 });
                }
                harness.step(Vec::new());
                let content_index = harness
                    .shapes
                    .iter()
                    .rposition(|shape| match &shape.shape {
                        Shape::Mesh(mesh) => audio && !mesh.is_empty(),
                        Shape::Rect(rect) => !audio && rect.rect.top() > 64.0,
                        _ => false,
                    })
                    .expect("the fixture must paint waveform or note contents");
                let mut strengths = Vec::new();
                for tick in [240, 960, 3840] {
                    let x = harness.timeline.grid_line_x(tick);
                    let (index, shape, stroke) = harness
                        .shapes
                        .iter()
                        .enumerate()
                        .find_map(|(index, shape)| match shape.shape {
                            Shape::LineSegment { points, stroke }
                                if points[0].x == x && points[1].x == x =>
                            {
                                assert_eq!(points[0].y, 64.0);
                                assert_eq!(points[1].y, 108.0);
                                Some((index, shape, stroke))
                            }
                            _ => None,
                        })
                        .expect("visible grid lines must be painted over clips");
                    assert!(index > content_index, "contents must not cover grid lines");
                    assert_eq!(shape.clip_rect.top(), 64.0);
                    assert!(shape.clip_rect.contains(pos2(x, 90.0)));
                    assert!(!shape.clip_rect.contains(pos2(x, 58.0)));
                    let expected_ink = Color32::WHITE;
                    let _ = (selected, color);
                    assert_eq!(stroke.color.to_opaque(), expected_ink);
                    strengths.push(stroke.color.a());
                }
                assert!(strengths[0] > 0);
                assert!(strengths[0] < strengths[1] && strengths[1] < strengths[2]);
            }
        }
    }
}
