use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{
    self, Event, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2,
};
use sympoie::Frame;
use sympoie::automation::{
    AutomationLane, AutomationPoint, AutomationTarget, SubtractiveParameter,
};
use sympoie::clip::{MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
use sympoie::pitch::PitchAlgorithm;
use sympoie::plugin::{
    PluginDescriptor, PluginInstance, PluginKind, PluginParameter, SidechainRoute, SidechainTap,
};
use sympoie::project::Project;
use sympoie::track::Track;
use sympoie::transport::PlaybackState;
use sympoie::wav::WavWriter;

use crate::actions::Action;
use crate::app::{Startup, SympoieApp};
use crate::document::edit::{ClipRef, TrackDrop};
use crate::state::{GridDensity, GridMode, TimeRange};

mod mixer;

mod sends;

mod builtin_devices;

mod rack_visibility;

struct Harness {
    ctx: egui::Context,
    app: SympoieApp,
    time: f64,
    pointer: Pos2,
    clipboard: String,
    accessibility: Option<egui::accesskit::TreeUpdate>,
    _files: TempDir,
}

struct TempDir(PathBuf);

#[test]
fn recording_arm_top_bar_and_q_toggle_without_key_repeat() {
    let mut harness = Harness::with_two_clips();
    let arm = harness.rect(Id::new(("track-arm", 1)));
    harness.click(arm.center());
    assert_eq!(harness.app.armed_tracks_for_tests(), &BTreeSet::from([1]));
    let record = harness.labelled("Record (Q)");
    harness.click(record.center());
    assert!(harness.app.recording_for_tests());
    let record = harness.labelled("Stop recording (Q)");
    harness.click(record.center());
    assert!(!harness.app.recording_for_tests());
    harness.step(vec![Event::Key {
        key: Key::Q,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(harness.app.recording_for_tests());
    harness.step(vec![Event::Key {
        key: Key::Q,
        physical_key: None,
        pressed: true,
        repeat: true,
        modifiers: Modifiers::NONE,
    }]);
    assert!(harness.app.recording_for_tests());
    harness.step(vec![Event::Key {
        key: Key::Q,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    harness.key(Key::Q, Modifiers::NONE);
    assert!(!harness.app.recording_for_tests());
    harness.key(Key::Q, Modifiers::NONE);
    assert!(harness.app.recording_for_tests());
    let record = harness.labelled("Stop recording (Q)");
    harness.click(record.center());
    assert!(!harness.app.recording_for_tests());
}

#[test]
fn q_does_not_record_while_typing_a_track_name() {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::ToggleTrackArm(1));
    harness.begin_track_rename(1);
    harness.steps(2);
    assert!(harness.ctx.egui_wants_keyboard_input());
    harness.key(Key::Q, Modifiers::NONE);
    assert!(!harness.app.recording_for_tests());
}

#[test]
fn audio_reverse_shortcut_and_context_menu_restore_the_original() {
    let mut harness = Harness::with_two_clips();
    let clip = ClipRef { track: 2, index: 0 };
    let original = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    let samples = harness
        .project()
        .audio_asset(original.asset)
        .unwrap()
        .samples()
        .to_vec();
    harness.dispatch(Action::SelectClip(Some(clip)));
    harness.key(Key::R, Modifiers::NONE);
    let reversed = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    assert_eq!(reversed.frame_count, original.frame_count);
    assert_eq!(
        harness
            .project()
            .audio_asset(reversed.asset)
            .unwrap()
            .samples(),
        samples.iter().copied().rev().collect::<Vec<_>>()
    );
    assert_eq!(
        harness.app.document_for_tests().undo_label(),
        Some("Reverse Clip")
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip),
        Some(&original)
    );
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    let title = harness.rect(Id::new(("clip-title", 2, 0)));
    harness.right_click(title.center());
    let item = harness.labelled("Reverse R");
    harness.click(item.center());
    let restored = crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    assert_eq!(
        harness
            .project()
            .audio_asset(restored.asset)
            .unwrap()
            .samples(),
        samples
    );
}

#[test]
fn audio_reverse_time_selection_changes_only_the_middle_and_undoes_once() {
    let mut harness = Harness::with_two_clips();
    let clip = ClipRef { track: 2, index: 0 };
    let original = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    let samples = harness
        .project()
        .audio_asset(original.asset)
        .unwrap()
        .samples()
        .to_vec();
    let (start, end) = (480, 960);
    let from = harness.project().tempo().ticks_to_frames(start).unwrap() as usize;
    let to = harness.project().tempo().ticks_to_frames(end).unwrap() as usize;
    harness.dispatch(Action::SetTimeSelection(Some(TimeRange {
        start,
        end,
        tracks: BTreeSet::from([2]),
    })));
    let lane = harness.rect(Id::new(("lane", 2)));
    harness.right_click(pos2(harness.lane_x(720), lane.center().y));
    let item = harness.labelled("Reverse Selection R");
    harness.click(item.center());
    let middle = harness.app.selection_for_tests().clip.unwrap();
    let reversed = crate::document::edit::audio_clip(harness.project(), middle).unwrap();
    assert_eq!(reversed.source_start, from as u64);
    assert_eq!(reversed.frame_count, (to - from) as u64);
    let mut expected = samples.clone();
    expected[from..to].reverse();
    assert_eq!(
        harness
            .project()
            .audio_asset(reversed.asset)
            .unwrap()
            .samples(),
        expected
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    let Track::Audio(track) = &harness.project().tracks()[2] else {
        panic!()
    };
    assert_eq!(track.clips(), [original]);
    for use_title_menu in [false, true] {
        harness.dispatch(Action::SetTimeSelection(Some(TimeRange {
            start,
            end,
            tracks: BTreeSet::from([2]),
        })));
        if use_title_menu {
            let title = harness.rect(Id::new(("clip-title", 2, 0)));
            harness.right_click(title.center());
            let item = harness.labelled("Reverse R");
            harness.click(item.center());
        } else {
            harness.key(Key::R, Modifiers::NONE);
        }
        let middle = harness.app.selection_for_tests().clip.unwrap();
        let reversed = crate::document::edit::audio_clip(harness.project(), middle).unwrap();
        assert_eq!(
            (reversed.source_start, reversed.frame_count),
            (from as u64, (to - from) as u64)
        );
        assert_eq!(
            harness
                .project()
                .audio_asset(reversed.asset)
                .unwrap()
                .samples(),
            expected
        );
        harness.dispatch(Action::Undo);
    }
}

#[test]
fn audio_pitch_control_sets_fractional_semitones_and_reset_is_undoable() {
    let mut harness = Harness::with_two_clips();
    let clip = ClipRef { track: 2, index: 0 };
    harness.dispatch(Action::SelectClip(Some(clip)));
    harness.dispatch(Action::ShowClipView);
    let control = harness.labelled("Pitch");
    harness.double_click(control.center());
    harness.key(Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Text("7.25".to_owned())]);
    harness.key(Key::Enter, Modifiers::NONE);
    harness.steps(2);
    let audio = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    assert_eq!(audio.pitch_semitones, 7.25);
    assert_eq!(audio.frame_count, 48_000);
    let reset = harness.labelled("Reset");
    harness.click(reset.center());
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip)
            .unwrap()
            .pitch_semitones,
        0.0
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip)
            .unwrap()
            .pitch_semitones,
        7.25
    );
}

#[test]
fn audio_pitch_algorithm_context_controls_support_ab_comparison_and_undo() {
    fn open_pitch_menu(harness: &mut Harness) {
        let title = harness.rect(Id::new(("clip-title", 2, 0)));
        harness.right_click(title.center());
        let pitch = harness.labelled("Pitch");
        harness.move_to(pitch.center());
        harness.steps(2);
    }

    let mut harness = Harness::with_two_clips();
    let clip = ClipRef { track: 2, index: 0 };
    let original = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    assert_eq!(original.pitch_algorithm, PitchAlgorithm::Signalsmith);
    assert!(original.preserve_formants);
    harness.dispatch(Action::SetClipPitch {
        clip,
        semitones: 7.25,
    });

    open_pitch_menu(&mut harness);
    let rubber_band = harness.labelled(PitchAlgorithm::RubberBandR3.label());
    harness.click(rubber_band.center());
    let changed = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    assert_eq!(changed.pitch_algorithm, PitchAlgorithm::RubberBandR3);
    assert_eq!(changed.pitch_semitones, 7.25);
    assert_eq!(changed.frame_count, original.frame_count);
    assert_eq!(changed.asset, original.asset);
    assert!(changed.preserve_formants);

    harness.key(Key::Escape, Modifiers::NONE);
    open_pitch_menu(&mut harness);
    let formants = harness.labelled("Preserve Formants");
    harness.click(formants.center());
    assert!(
        !crate::document::edit::audio_clip(harness.project(), clip)
            .unwrap()
            .preserve_formants
    );
    harness.key(Key::Escape, Modifiers::NONE);
    harness.dispatch(Action::Undo);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip),
        Some(&changed)
    );
    harness.dispatch(Action::Undo);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip)
            .unwrap()
            .pitch_algorithm,
        PitchAlgorithm::Signalsmith
    );
    harness.dispatch(Action::Redo);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip),
        Some(&changed)
    );

    harness.dispatch(Action::ResetClipPitch(clip));
    let reset = crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    assert_eq!(reset.pitch_semitones, 0.0);
    assert_eq!(reset.pitch_algorithm, PitchAlgorithm::RubberBandR3);
    assert!(reset.preserve_formants);
    harness.dispatch(Action::Undo);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip),
        Some(&changed)
    );
}

#[test]
fn audio_pitch_clip_view_dropdown_changes_algorithm_without_changing_pitch() {
    let mut harness = Harness::with_two_clips();
    let clip = ClipRef { track: 2, index: 0 };
    harness.dispatch(Action::SelectClip(Some(clip)));
    harness.dispatch(Action::ShowClipView);
    for algorithm in [PitchAlgorithm::Builtin, PitchAlgorithm::RubberBandR3] {
        let selector = harness.labelled("Algorithm");
        harness.click(selector.center());
        let choice = harness.labelled(algorithm.label());
        harness.click(choice.center());
        let audio = crate::document::edit::audio_clip(harness.project(), clip).unwrap();
        assert_eq!(audio.pitch_algorithm, algorithm);
        assert_eq!(audio.pitch_semitones, 0.0);
        assert!(
            audio.preserve_formants,
            "changing algorithms retains the formant preference"
        );
    }
    harness.dispatch(Action::Undo);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip)
            .unwrap()
            .pitch_algorithm,
        PitchAlgorithm::Builtin
    );
}

#[test]
fn zero_pitch_algorithm_action_is_saved_and_identical_options_do_not_add_history() {
    let mut harness = Harness::with_two_clips();
    let clip = ClipRef { track: 2, index: 0 };
    let original = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    let options = Action::SetClipPitchOptions {
        clip,
        algorithm: PitchAlgorithm::RubberBandR3,
        preserve_formants: false,
    };
    harness.dispatch(options.clone());
    let changed = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    assert_eq!(changed.pitch_algorithm, PitchAlgorithm::RubberBandR3);
    assert_eq!(changed.pitch_semitones, 0.0);
    assert!(!changed.preserve_formants);
    let revision = harness.app.document_for_tests().revision();
    harness.dispatch(options);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    harness.dispatch(Action::Undo);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip),
        Some(&original)
    );
    harness.dispatch(Action::Redo);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip),
        Some(&changed)
    );
}

#[test]
fn audio_reverse_shortcut_is_ignored_while_typing_or_without_audio() {
    let mut harness = Harness::with_two_clips();
    let clip = ClipRef { track: 2, index: 0 };
    harness.dispatch(Action::SelectClip(Some(clip)));
    let original = *crate::document::edit::audio_clip(harness.project(), clip).unwrap();
    harness.begin_track_rename(2);
    assert!(harness.ctx.egui_wants_keyboard_input());
    harness.key(Key::R, Modifiers::NONE);
    assert_eq!(
        crate::document::edit::audio_clip(harness.project(), clip),
        Some(&original)
    );
    harness.key(Key::Escape, Modifiers::NONE);
    harness.dispatch(Action::SelectClip(None));
    let revision = harness.app.document_for_tests().revision();
    harness.key(Key::R, Modifiers::NONE);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_wav(path: &std::path::Path, frames: &[Frame]) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = WavWriter::new(std::io::BufWriter::new(file), frames.len() as u64).unwrap();
    writer.write_frames(frames).unwrap();
    writer.finish().unwrap();
}

impl Harness {
    fn with_empty_project() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "sympoie-empty-ui-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let ctx = egui::Context::default();
        let app = SympoieApp::new(&ctx, Startup::default());
        let mut harness = Self {
            ctx,
            app,
            time: 0.0,
            pointer: pos2(10.0, 10.0),
            clipboard: String::new(),
            accessibility: None,
            _files: TempDir(directory),
        };
        harness.steps(3);
        harness
    }

    fn with_vst3_devices() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "sympoie-vst3-ui-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("plugins.symp");
        let mut project = Project::new();
        let midi = project.add_midi_track("Synth");
        project
            .set_plugin_instrument(midi, Some(fake_vst3(PluginKind::Instrument)))
            .unwrap();
        project
            .add_effect(midi.into(), fake_vst3(PluginKind::Effect))
            .unwrap();
        project
            .add_effect(midi.into(), fake_vst3(PluginKind::Effect))
            .unwrap();
        let audio = project.add_audio_track("Audio");
        let group = project.add_group_track("Group");
        for track in [audio.into(), group.into(), project.master_track_id().into()] {
            project
                .add_effect(track, fake_vst3(PluginKind::Effect))
                .unwrap();
        }
        sympoie::project_file::save_project_directory(&project, &path).unwrap();
        let ctx = egui::Context::default();
        let app = SympoieApp::new(
            &ctx,
            Startup {
                project: Some(path),
                audio: Vec::new(),
                use_audio_device: false,
                ..Startup::default()
            },
        );
        let mut harness = Self {
            ctx,
            app,
            time: 0.0,
            pointer: pos2(10.0, 10.0),
            clipboard: String::new(),
            accessibility: None,
            _files: TempDir(directory),
        };
        harness.steps(3);
        harness
    }

    fn with_instruments() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "sympoie-devices-ui-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("instruments.symp");
        let mut project = Project::new();
        for name in ["Lead", "Bass"] {
            let track = project.add_midi_track(name);
            project
                .set_midi_track_instrument(
                    track,
                    Some(Instrument::Subtractive(SubtractiveSettings::default())),
                )
                .unwrap();
        }
        project.add_audio_track("Audio");
        sympoie::project_file::save_project_directory(&project, &path).unwrap();
        let ctx = egui::Context::default();
        let app = SympoieApp::new(
            &ctx,
            Startup {
                project: Some(path),
                audio: Vec::new(),
                use_audio_device: false,
                ..Startup::default()
            },
        );
        let mut harness = Self {
            ctx,
            app,
            time: 0.0,
            pointer: pos2(10.0, 10.0),
            clipboard: String::new(),
            accessibility: None,
            _files: TempDir(directory),
        };
        harness.steps(3);
        harness
    }

    fn with_two_clips() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "sympoie-ui-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let kick = directory.join("kick.wav");
        let tone = directory.join("tone.wav");
        write_wav(&kick, &vec![[0.25, 0.25]; 48_000]);
        write_wav(
            &tone,
            &(0..48_000)
                .map(|i| [((i as f32) * 0.05).sin() * 0.5; 2])
                .collect::<Vec<_>>(),
        );
        let startup = Startup {
            project: None,
            audio: vec![kick, tone],
            use_audio_device: false,
            ..Startup::default()
        };
        let ctx = egui::Context::default();
        let app = SympoieApp::new(&ctx, startup);
        let mut harness = Self {
            ctx,
            app,
            time: 0.0,
            pointer: pos2(10.0, 10.0),
            clipboard: String::new(),
            accessibility: None,
            _files: TempDir(directory),
        };
        harness.steps(3);
        harness
    }

    fn with_midi_clip() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "sympoie-piano-ui-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("notes.symp");
        let mut project = Project::new();
        let track = project.add_midi_track("Piano");
        project
            .add_midi_clip(
                track,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 3840,
                    length_ticks: 7680,
                    notes: vec![
                        MidiNote {
                            start_tick: 0,
                            duration_ticks: 480,
                            key: 60,
                            velocity: 87,
                            channel: 3,
                        },
                        MidiNote {
                            start_tick: 960,
                            duration_ticks: 480,
                            key: 64,
                            velocity: 106,
                            channel: 5,
                        },
                        MidiNote {
                            start_tick: 1920,
                            duration_ticks: 480,
                            key: 67,
                            velocity: 52,
                            channel: 15,
                        },
                    ],
                },
            )
            .unwrap();
        sympoie::project_file::save_project_directory(&project, &path).unwrap();
        let ctx = egui::Context::default();
        let app = SympoieApp::new(
            &ctx,
            Startup {
                project: Some(path),
                audio: Vec::new(),
                use_audio_device: false,
                ..Startup::default()
            },
        );
        let mut harness = Self {
            ctx,
            app,
            time: 0.0,
            pointer: pos2(10.0, 10.0),
            clipboard: String::new(),
            accessibility: None,
            _files: TempDir(directory),
        };
        harness.steps(3);
        let clip = harness.clip_rect(1, 0);
        harness.double_click(clip_title(clip));
        harness.steps(2);
        harness
    }

    fn step(&mut self, events: Vec<Event>) {
        self.step_input(RawInput {
            events,
            ..RawInput::default()
        });
    }

    fn step_input(&mut self, input: RawInput) {
        self.step_input_with_focus(input, true);
    }

    fn step_input_with_focus(&mut self, mut input: RawInput, focused: bool) {
        input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(1880.0, 900.0)));
        input.time = Some(self.time);
        input.focused = focused;
        self.time += 1.0 / 60.0;
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| app.frame(ui));
        if let Some(update) = output.platform_output.accesskit_update.take() {
            self.accessibility = Some(update);
        }
        for command in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command {
                self.clipboard.clone_from(text);
            }
        }
        output.textures_delta.clear();
    }

    fn steps(&mut self, count: usize) {
        for _ in 0..count {
            self.step(Vec::new());
        }
    }

    fn rect(&self, id: Id) -> Rect {
        self.ctx
            .read_response(id)
            .unwrap_or_else(|| panic!("no widget {id:?} was shown"))
            .rect
    }

    fn assert_visible(&self, id: Id, visible: bool) {
        assert_eq!(
            self.ctx.read_response(id).is_some(),
            visible,
            "unexpected visibility for widget {id:?}"
        );
    }

    fn clip_rect(&self, track: usize, index: usize) -> Rect {
        self.rect(Id::new(("clip", track, index)))
    }

    fn select_track(&mut self, track: usize) {
        let header = self.rect(Id::new(("header", track)));
        self.click(header.left_bottom() + vec2(6.0, -4.0));
        self.steps(1);
        assert_eq!(
            self.app.selection_for_tests().tracks,
            BTreeSet::from([track])
        );
    }

    fn open_track_devices(&mut self, track: usize) {
        let header = self.rect(Id::new(("header", track)));
        self.double_click(header.left_bottom() + vec2(6.0, -4.0));
        self.steps(1);
        assert!(self.app.showing_devices_for_tests());
        assert_eq!(
            self.app.selection_for_tests().tracks,
            BTreeSet::from([track])
        );
    }

    fn begin_track_rename(&mut self, track: usize) {
        let header = self.rect(Id::new(("header", track)));
        self.right_click(header.left_top() + vec2(30.0, 14.0));
        let rename = self.labelled("Rename");
        self.click(rename.center());
        self.steps(2);
        assert!(self.ctx.egui_wants_keyboard_input());
    }

    fn move_to(&mut self, to: Pos2) {
        self.pointer = to;
        self.step(vec![Event::PointerMoved(to)]);
    }

    fn button(&mut self, pressed: bool, modifiers: Modifiers) {
        let pos = self.pointer;
        self.step(vec![
            Event::ModifiersChanged(modifiers),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            },
        ]);
        if !pressed && modifiers != Modifiers::NONE {
            self.step(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        }
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        self.move_to(from);
        self.button(true, modifiers);
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            self.move_to(from + (to - from) * t);
        }
        self.button(false, modifiers);
        self.steps(2);
    }

    fn click(&mut self, at: Pos2) {
        self.move_to(at);
        self.button(true, Modifiers::NONE);
        self.button(false, Modifiers::NONE);
        self.steps(1);
    }

    fn double_click(&mut self, at: Pos2) {
        self.time += 0.7;
        self.move_to(at);
        for _ in 0..2 {
            self.button(true, Modifiers::NONE);
            self.button(false, Modifiers::NONE);
            self.steps(1);
        }
        self.steps(1);
    }

    fn click_with(&mut self, at: Pos2, modifiers: Modifiers) {
        self.move_to(at);
        self.button(true, modifiers);
        self.button(false, modifiers);
        self.steps(1);
    }

    fn right_click(&mut self, at: Pos2) {
        self.move_to(at);
        for pressed in [true, false] {
            self.step(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        self.steps(2);
    }

    fn labelled(&mut self, label: &str) -> Rect {
        self.ctx.enable_accesskit();
        self.steps(1);
        let update = self.accessibility.as_ref().expect("an accessibility tree");
        let bounds = update
            .nodes
            .iter()
            .filter(|(_, node)| {
                node.label()
                    .is_some_and(|text| text.trim_end_matches(['⏵', ' ']) == label)
                    || node.labelled_by().iter().any(|label_id| {
                        update.nodes.iter().any(|(id, label_node)| {
                            id == label_id
                                && (label_node.value() == Some(label)
                                    || label_node.label() == Some(label))
                        })
                    })
            })
            .find_map(|(_, node)| node.bounds())
            .unwrap_or_else(|| {
                let labels: Vec<_> = update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| node.label())
                    .collect();
                panic!("no widget labelled {label:?} among {labels:?}")
            });
        Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        )
    }

    fn lane_x(&self, tick: u64) -> f32 {
        let view = self.app.arrangement_for_tests();
        let lanes = self.rect(Id::new(("lane", 1)));
        lanes.left() + ((tick as f64 - view.scroll_tick) * view.pixels_per_tick) as f32
    }

    fn fixed_grid(&mut self, grid: sympoie::time::SnapGrid) {
        self.dispatch(Action::SetSnap(grid));
        self.dispatch(Action::SetGridMode(GridMode::Fixed));
    }

    fn midi_clips(&self, track: usize) -> &[MidiClip] {
        let Track::Midi(midi) = &self.project().tracks()[track] else {
            panic!("not a MIDI track")
        };
        midi.clips()
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        let event = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.step(vec![event(true)]);
        self.step(vec![event(false)]);
    }

    fn project(&self) -> &sympoie::project::Project {
        self.app.document_for_tests().project()
    }

    fn dispatch(&mut self, action: Action) {
        self.app.apply_for_tests(&self.ctx, action);
        self.steps(2);
    }

    fn finish_bounce(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.app.bouncing_for_tests() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
            self.steps(1);
        }
        assert!(!self.app.bouncing_for_tests(), "bounce did not complete");
        self.steps(2);
    }

    fn instrument(&self, index: usize) -> Option<&Instrument> {
        let Track::Midi(track) = &self.project().tracks()[index] else {
            panic!("not a MIDI track")
        };
        track.instrument()
    }

    fn instrument_bypassed(&self, index: usize) -> bool {
        let Track::Midi(track) = &self.project().tracks()[index] else {
            panic!("not a MIDI track")
        };
        track.instrument_bypassed()
    }

    fn effect_id(&self, track: usize, index: usize) -> u64 {
        self.project().tracks()[track].effects()[index].id
    }

    fn plugin_parameter(&self, id: u64) -> f64 {
        self.project().plugin(id).unwrap().parameters[0].value
    }

    fn clip_start(&self, track: usize, index: usize) -> u64 {
        match &self.project().tracks()[track] {
            Track::Audio(audio) => audio.clips()[index].start_tick,
            _ => panic!("not an audio track"),
        }
    }

    fn midi_clip(&self) -> &MidiClip {
        let Track::Midi(track) = &self.project().tracks()[1] else {
            panic!("not a MIDI track")
        };
        assert_eq!(track.clips().len(), 1, "note editing retains its clip");
        &track.clips()[0]
    }

    fn note_rect(&self, index: usize) -> Rect {
        self.rect(Id::new(("piano-note", index)))
    }

    fn piano_scale(&self) -> (f32, f32) {
        let first = self.note_rect(0);
        let second = self.note_rect(1);
        (
            (second.left() - first.left()) / 960.0,
            (first.center().y - second.center().y) / 4.0,
        )
    }
}

fn fake_vst3(kind: PluginKind) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: PathBuf::from("/sympoie-ui-test-missing-plugin.vst3"),
            class_id: "0123456789ABCDEFFEDCBA9876543210".into(),
            name: "Example plugin".into(),
            vendor: "Test fixture".into(),
            kind,
        },
        vec![PluginParameter {
            id: 37,
            name: "Level".into(),
            unit: "dB".into(),
            value: 0.5,
            steps: 0,
            read_only: false,
        }],
        vec![1, 2, 3, 4],
    )
}

fn clip_title(rect: Rect) -> Pos2 {
    pos2(rect.center().x, rect.top() + 6.0)
}

#[test]
fn startup_imports_files_onto_named_tracks() {
    let harness = Harness::with_two_clips();
    let names: Vec<_> = harness.project().tracks().iter().map(Track::name).collect();
    assert_eq!(names, ["Master", "kick", "tone"]);
    let kick = harness.clip_rect(1, 0);
    let tone = harness.clip_rect(2, 0);
    assert!(kick.width() > 20.0, "clips are drawn at a visible size");
    assert!((kick.left() - tone.left()).abs() < 0.5);
    assert!(tone.top() > kick.top());
}

#[test]
fn every_track_kind_has_solo_to_the_right_of_mute_with_separate_history() {
    for track in [1, 2, 3, 0] {
        let mut harness = Harness::with_vst3_devices();
        let mute = harness.rect(Id::new(("track-mute", track)));
        let solo = harness.rect(Id::new(("track-solo", track)));
        assert!(solo.left() >= mute.right());
        assert!((solo.center().y - mute.center().y).abs() < 0.5);
        harness.click(solo.center());
        assert!(harness.project().tracks()[track].mixer().soloed());
        assert!(!harness.project().tracks()[track].mixer().muted());
        harness.click(solo.center());
        assert!(!harness.project().tracks()[track].mixer().soloed());
        harness.dispatch(Action::Undo);
        assert!(harness.project().tracks()[track].mixer().soloed());
        harness.dispatch(Action::Undo);
        assert!(!harness.project().tracks()[track].mixer().soloed());
        assert!(!harness.app.document_for_tests().is_dirty());
        harness.dispatch(Action::Redo);
        assert!(harness.project().tracks()[track].mixer().soloed());
        harness.dispatch(Action::Redo);
        assert!(!harness.project().tracks()[track].mixer().soloed());
    }
}

#[test]
fn additive_solo_keeps_mute_independent_and_follows_saved_track_order() {
    let mut harness = Harness::with_vst3_devices();
    for track in [1, 2] {
        let solo = harness.rect(Id::new(("track-solo", track)));
        harness.click(solo.center());
    }
    assert!(harness.project().tracks()[1].mixer().soloed());
    assert!(harness.project().tracks()[2].mixer().soloed());
    let mute = harness.rect(Id::new(("track-mute", 1_usize)));
    harness.click(mute.center());
    assert!(harness.project().tracks()[1].mixer().muted());
    assert!(harness.project().tracks()[1].mixer().soloed());
    assert!(!harness.project().tracks()[2].mixer().muted());

    harness.dispatch(Action::Undo);
    assert!(!harness.project().tracks()[1].mixer().muted());
    assert!(harness.project().tracks()[1].mixer().soloed());
    assert!(harness.project().tracks()[2].mixer().soloed());
    harness.dispatch(Action::Undo);
    assert!(harness.project().tracks()[1].mixer().soloed());
    assert!(!harness.project().tracks()[2].mixer().soloed());
    harness.dispatch(Action::Undo);
    assert!(!harness.project().tracks()[1].mixer().soloed());
    assert!(!harness.app.document_for_tests().is_dirty());
    for _ in 0..3 {
        harness.dispatch(Action::Redo);
    }
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::After(3),
    });
    assert_eq!(harness.project().tracks()[3].name(), "Synth");
    assert!(harness.project().tracks()[3].mixer().soloed());
    assert!(harness.project().tracks()[3].mixer().muted());
    assert_eq!(harness.project().tracks()[1].name(), "Audio");
    assert!(harness.project().tracks()[1].mixer().soloed());
    assert!(!harness.project().tracks()[2].mixer().soloed());
    harness.dispatch(Action::Save);
    let saved = Project::load(harness._files.0.join("plugins.symp")).unwrap();
    for (actual, expected) in saved.tracks().iter().zip(harness.project().tracks()) {
        assert_eq!(actual.name(), expected.name());
        assert_eq!(actual.mixer(), expected.mixer());
    }
}

#[test]
fn track_reorder_remaps_clip_selection_and_undo_redo() {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::SelectClip(Some(ClipRef { track: 1, index: 0 })));
    let before = harness.app.selection_for_tests().clone();
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::After(2),
    });
    assert_eq!(harness.project().tracks()[2].name(), "kick");
    assert_eq!(
        harness.app.selection_for_tests().clip,
        Some(ClipRef { track: 2, index: 0 })
    );
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([2])
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().tracks()[1].name(), "kick");
    assert_eq!(harness.app.selection_for_tests(), &before);
    harness.dispatch(Action::Redo);
    assert_eq!(harness.project().tracks()[2].name(), "kick");
    assert_eq!(
        harness.app.selection_for_tests().clip,
        Some(ClipRef { track: 2, index: 0 })
    );
    harness.dispatch(Action::SetClipGain {
        clip: harness.app.selection_for_tests().clip.unwrap(),
        gain: 0.375,
    });
    let Track::Audio(kick) = &harness.project().tracks()[2] else {
        panic!("the selected clip still belongs to the moved audio track")
    };
    assert_eq!(kick.clips()[0].gain, 0.375);
    let Track::Audio(tone) = &harness.project().tracks()[1] else {
        panic!("tone remains an audio track")
    };
    assert_eq!(tone.clips()[0].gain, 1.0);
}

#[test]
fn track_reorder_remaps_time_selection_and_instrument_windows_through_history() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::OpenPluginEditor(1));
    harness.dispatch(Action::OpenPluginEditor(2));
    harness.dispatch(Action::SetTimeSelection(Some(TimeRange {
        start: 240,
        end: 960,
        tracks: BTreeSet::from([1, 3]),
    })));
    let before = harness.app.selection_for_tests().clone();
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::After(3),
    });
    let after = harness.app.selection_for_tests().clone();
    assert_eq!(after.tracks, BTreeSet::from([2, 3]));
    assert_eq!(
        after.range,
        Some(TimeRange {
            start: 240,
            end: 960,
            tracks: BTreeSet::from([2, 3]),
        })
    );
    assert_eq!(
        harness.app.plugin_editors_for_tests(),
        &BTreeSet::from([1, 3])
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.app.selection_for_tests(), &before);
    assert_eq!(
        harness.app.plugin_editors_for_tests(),
        &BTreeSet::from([1, 2])
    );
    assert!(!harness.app.document_for_tests().is_dirty());
    harness.dispatch(Action::Redo);
    assert_eq!(harness.app.selection_for_tests(), &after);
    assert_eq!(
        harness.app.plugin_editors_for_tests(),
        &BTreeSet::from([1, 3])
    );
    harness.select_track(3);
    harness.assert_visible(Id::new(("close-plugin-editor", 3)), true);
    harness.assert_visible(Id::new(("close-plugin-editor", 1)), false);
    assert_eq!(harness.project().tracks()[3].name(), "Lead");
}

#[test]
fn track_reorder_keeps_the_piano_roll_editing_the_moved_clip_after_undo_redo() {
    let mut harness = Harness::with_midi_clip();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    let clip = harness.clip_rect(1, 0);
    harness.double_click(clip_title(clip));
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::After(2),
    });
    harness.dispatch(Action::Undo);
    harness.dispatch(Action::Redo);
    harness.click(harness.note_rect(0).center());
    harness.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(harness.project().tracks()[2].name(), "Piano");
    assert_eq!(harness.midi_clips(2)[0].notes[0].key, 61);
    assert!(harness.midi_clips(1).is_empty());
    assert_eq!(
        harness.app.selection_for_tests().clip,
        Some(ClipRef { track: 2, index: 0 })
    );
}

#[test]
fn track_reorder_preserves_sidechain_windows_routes_folded_groups_and_saved_order() {
    let mut harness = Harness::with_vst3_devices();
    let id = harness.effect_id(1, 0);
    let route = SidechainRoute {
        source_track: 2,
        bus_index: 1,
        tap: SidechainTap::PreEffects,
        gain: 0.65,
    };
    harness.dispatch(Action::SetVst3Sidechain {
        id,
        route: Some(route),
    });
    harness.dispatch(Action::OpenVst3Editor(id));
    harness.dispatch(Action::OpenVst3Sidechain(id));
    harness.dispatch(Action::ToggleFold(3));
    harness.dispatch(Action::MoveTrack {
        track: 3,
        target: TrackDrop::Before(1),
    });
    assert!(harness.app.arrangement_for_tests().folded.contains(&1));
    assert!(!harness.app.arrangement_for_tests().folded.contains(&3));
    let moved_route = SidechainRoute {
        source_track: 3,
        ..route
    };
    assert_eq!(
        harness.project().plugin(id).unwrap().sidechain,
        Some(moved_route)
    );
    assert_eq!(harness.app.vst3_editors_for_tests(), &BTreeSet::from([id]));
    assert_eq!(
        harness.app.sidechain_editors_for_tests(),
        &BTreeSet::from([id])
    );
    harness.dispatch(Action::Undo);
    assert!(harness.app.arrangement_for_tests().folded.contains(&3));
    assert_eq!(harness.project().plugin(id).unwrap().sidechain, Some(route));
    harness.dispatch(Action::Redo);
    harness.dispatch(Action::Save);
    let saved = Project::load(harness._files.0.join("plugins.symp")).unwrap();
    assert_eq!(
        saved.tracks().iter().map(Track::name).collect::<Vec<_>>(),
        ["Master", "Group", "Synth", "Audio"]
    );
    assert_eq!(saved.plugin(id).unwrap().sidechain, Some(moved_route));
    assert_eq!(harness.app.vst3_editors_for_tests(), &BTreeSet::from([id]));
    assert_eq!(
        harness.app.sidechain_editors_for_tests(),
        &BTreeSet::from([id])
    );
}

#[test]
fn ineffective_track_drops_do_not_dirty_a_saved_project_or_clear_redo() {
    let mut harness = Harness::with_instruments();
    for action in [
        Action::MoveTrack {
            track: 1,
            target: TrackDrop::Before(1),
        },
        Action::MoveTrack {
            track: 1,
            target: TrackDrop::Before(2),
        },
        Action::MoveTrack {
            track: 3,
            target: TrackDrop::After(2),
        },
        Action::MoveTrack {
            track: 0,
            target: TrackDrop::After(2),
        },
        Action::ExitGroup(1),
    ] {
        let revision = harness.app.document_for_tests().revision();
        harness.dispatch(action);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(harness.app.document_for_tests().undo_label().is_none());
        assert!(!harness.app.document_for_tests().is_dirty());
    }
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::After(2),
    });
    harness.dispatch(Action::Undo);
    let revision = harness.app.document_for_tests().revision();
    let redo_label = harness
        .app
        .document_for_tests()
        .redo_label()
        .unwrap()
        .to_owned();
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::Before(2),
    });
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert_eq!(
        harness.app.document_for_tests().redo_label(),
        Some(redo_label.as_str())
    );
    harness.dispatch(Action::Redo);
    assert_eq!(harness.project().tracks()[2].name(), "Lead");
}

#[test]
fn entering_and_exiting_a_group_are_undoable_even_when_exit_keeps_the_same_order() {
    let mut harness = Harness::with_vst3_devices();
    let effect = harness.effect_id(1, 0);
    harness.dispatch(Action::SetVst3Sidechain {
        id: effect,
        route: Some(SidechainRoute {
            source_track: 2,
            bus_index: 1,
            tap: SidechainTap::PostEffects,
            gain: 0.5,
        }),
    });
    harness.dispatch(Action::SelectTracks(BTreeSet::from([2])));
    let revision = harness.app.document_for_tests().revision();
    harness.dispatch(Action::MoveTrack {
        track: 2,
        target: TrackDrop::IntoGroup(3),
    });
    assert_eq!(harness.project().tracks()[3].name(), "Audio");
    assert_eq!(
        crate::document::rows::parents(harness.project())[3],
        Some(2)
    );
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([3])
    );
    assert_eq!(
        harness
            .project()
            .plugin(effect)
            .unwrap()
            .sidechain
            .unwrap()
            .source_track,
        3
    );
    harness.dispatch(Action::ExitGroup(3));
    assert_eq!(harness.project().tracks()[3].name(), "Audio");
    assert_eq!(crate::document::rows::parents(harness.project())[3], None);
    assert_eq!(harness.app.document_for_tests().revision(), revision + 2);
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([3])
    );

    harness.dispatch(Action::Undo);
    assert_eq!(
        crate::document::rows::parents(harness.project())[3],
        Some(2)
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().tracks()[2].name(), "Audio");
    assert_eq!(crate::document::rows::parents(harness.project())[2], None);
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([2])
    );
    assert_eq!(
        harness
            .project()
            .plugin(effect)
            .unwrap()
            .sidechain
            .unwrap()
            .source_track,
        2
    );
    harness.dispatch(Action::Redo);
    harness.dispatch(Action::Redo);
    assert_eq!(harness.project().tracks()[3].name(), "Audio");
    assert_eq!(crate::document::rows::parents(harness.project())[3], None);
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([3])
    );
    assert_eq!(
        harness
            .project()
            .plugin(effect)
            .unwrap()
            .sidechain
            .unwrap()
            .source_track,
        3
    );
}

#[test]
fn track_name_and_header_context_menus_exit_exactly_one_group_level() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Group));
    harness.dispatch(Action::RenameTrack(4, "Outer".into()));
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Group));
    harness.dispatch(Action::RenameTrack(5, "Inner".into()));
    harness.dispatch(Action::MoveTrack {
        track: 5,
        target: TrackDrop::IntoGroup(4),
    });
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::IntoGroup(5),
    });
    assert_eq!(harness.project().tracks()[5].name(), "Lead");
    assert_eq!(harness.project().tracks()[4].name(), "Inner");
    assert_eq!(harness.project().tracks()[3].name(), "Outer");
    let depth = |harness: &Harness| {
        crate::document::rows::display_rows(harness.project(), &Default::default())
            .into_iter()
            .find(|row| row.index == 5)
            .unwrap()
            .depth
    };

    for click_name in [true, false] {
        assert_eq!(depth(&harness), 2);
        let point = if click_name {
            harness.rect(Id::new(("track-name", 5))).center()
        } else {
            harness.rect(Id::new(("header", 5))).left_bottom() + vec2(30.0, -6.0)
        };
        harness.right_click(point);
        let exit = harness.labelled("Exit Group");
        harness.click(exit.center());
        assert_eq!(harness.project().tracks()[5].name(), "Lead");
        assert_eq!(
            crate::document::rows::parents(harness.project())[5],
            Some(3)
        );
        assert_eq!(depth(&harness), 1);
        harness.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(
            crate::document::rows::parents(harness.project())[5],
            Some(4)
        );
        assert_eq!(depth(&harness), 2);
    }
}

#[test]
fn undo_during_a_track_header_drag_cancels_the_pending_drop() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::After(3),
    });
    let from = harness.rect(Id::new(("track-name", 1))).center();
    let to = harness.rect(Id::new(("header", 3))).center_bottom() - vec2(0.0, 3.0);
    harness.move_to(from);
    harness.button(true, Modifiers::NONE);
    harness.move_to(to);
    harness.key(Key::Z, Modifiers::COMMAND);
    let revision = harness.app.document_for_tests().revision();
    harness.button(false, Modifiers::NONE);
    harness.steps(2);
    assert_eq!(
        harness
            .project()
            .tracks()
            .iter()
            .map(Track::name)
            .collect::<Vec<_>>(),
        ["Master", "Lead", "Bass", "Audio"]
    );
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(harness.app.document_for_tests().redo_label().is_some());
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn track_headers_show_independent_meters_and_reset_only_the_clicked_clip_light() {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::SetClipGain {
        clip: ClipRef { track: 1, index: 0 },
        gain: 16.0,
    });
    let _shared = harness.app.capture_audio_for_tests();
    let selection = harness.app.selection_for_tests().clone();
    let revision = harness.app.document_for_tests().revision();
    harness.key(Key::Space, Modifiers::NONE);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !harness.app.track_meter_for_tests(1).clipped && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
        harness.steps(1);
    }
    assert!(harness.app.track_meter_for_tests(1).clipped);
    assert!(harness.app.track_meter_for_tests(2).is_active());
    assert!(!harness.app.track_meter_for_tests(2).clipped);
    for track in 0..3 {
        let header = harness.rect(Id::new(("header", track)));
        let meter = harness.rect(Id::new(("track-meter", track)));
        assert!(header.contains_rect(meter));
        assert!(meter.width() > 150.0 && meter.height() >= 8.0);
    }

    harness.click(harness.rect(Id::new(("track-meter", 1))).center());
    assert!(!harness.app.track_meter_for_tests(1).clipped);
    assert!(harness.app.track_meter_for_tests(1).is_active());
    assert_eq!(harness.app.selection_for_tests(), &selection);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn midi_audio_group_and_main_headers_all_have_meters() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Group));
    harness.steps(1);
    for height in ["Compact", "Normal", "Large"] {
        let view = harness.labelled("View");
        harness.click(view.center());
        let item = harness.labelled(height);
        harness.click(item.center());
        harness.key(Key::Escape, Modifiers::NONE);
        harness.steps(2);
        for track in 0..harness.project().tracks().len() {
            let header = harness.rect(Id::new(("header", track)));
            let meter = harness.rect(Id::new(("track-meter", track)));
            assert!(header.contains_rect(meter));
            assert!(meter.left() >= header.left() + 44.0);
        }
    }
}

#[test]
fn bounce_in_place_from_the_track_menu_is_one_undoable_edit() {
    let mut harness = Harness::with_midi_clip();
    harness.dispatch(Action::SetInstrument {
        track: 1,
        instrument: Some(Instrument::Subtractive(SubtractiveSettings::default())),
    });
    let original = harness.midi_clip().clone();
    let revision = harness.app.document_for_tests().revision();
    let header = harness.rect(Id::new(("header", 1)));
    harness.right_click(header.left_bottom() + vec2(25.0, -5.0));
    let item = harness.labelled("Bounce Track in Place");
    harness.click(item.center());
    harness.finish_bounce();
    assert_eq!(harness.project().tracks().len(), 2);
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!(
            "bounce did not replace MIDI track: {:?}",
            harness.app.notice_for_tests()
        );
    };
    assert_eq!(track.clips().len(), 1);
    assert_eq!(track.clips()[0].start_tick, original.start_tick);
    assert!(track.effects().is_empty());
    assert_eq!(harness.app.document_for_tests().revision(), revision + 1);
    assert_eq!(
        harness.app.selection_for_tests().clip,
        Some(ClipRef { track: 1, index: 0 })
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clips(1), [original]);
    assert!(harness.instrument(1).is_some());
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(matches!(harness.project().tracks()[1], Track::Audio(_)));
}

#[test]
fn duplicate_bounce_mutes_the_original_and_survives_directory_save_and_reopen() {
    let mut harness = Harness::with_midi_clip();
    harness.dispatch(Action::SetInstrument {
        track: 1,
        instrument: Some(Instrument::Subtractive(SubtractiveSettings::default())),
    });
    let original = harness.midi_clip().clone();
    let header = harness.rect(Id::new(("header", 1)));
    harness.right_click(header.left_bottom() + vec2(25.0, -5.0));
    let item = harness.labelled("Duplicate and Bounce Track");
    harness.click(item.center());
    harness.finish_bounce();
    assert_eq!(harness.project().tracks().len(), 3);
    assert_eq!(harness.midi_clips(1), std::slice::from_ref(&original));
    assert!(harness.project().tracks()[1].mixer().muted());
    assert!(!harness.project().tracks()[2].mixer().muted());
    let Track::Audio(audio) = &harness.project().tracks()[2] else {
        panic!()
    };
    assert_eq!(audio.clips()[0].start_tick, original.start_tick);

    let folder = harness._files.0.join("notes.symp");
    harness.key(Key::S, Modifiers::COMMAND);
    assert!(folder.join("project.json").is_file());
    let samples: Vec<_> = std::fs::read_dir(folder.join("samples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(samples.len(), 1);
    let samples = sympoie::wav::read_wav(&samples[0]).unwrap();
    assert!(
        samples
            .samples()
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.01)
    );
    assert!(!harness.app.document_for_tests().is_dirty());
    harness.dispatch(Action::OpenPath(folder.join("project.json")));
    assert_eq!(
        harness.app.document_for_tests().path(),
        Some(folder.as_path())
    );
    assert_eq!(harness.midi_clips(1), [original]);
    assert!(harness.project().tracks()[1].mixer().muted());
    assert!(matches!(harness.project().tracks()[2], Track::Audio(_)));
    let mute = harness.rect(Id::new(("track-mute", 1)));
    harness.click(mute.center());
    assert!(!harness.project().tracks()[1].mixer().muted());
    harness.key(Key::Z, Modifiers::COMMAND);
    assert!(harness.project().tracks()[1].mixer().muted());
}

#[test]
fn bounce_does_not_replace_edits_made_while_the_worker_is_running() {
    let mut harness = Harness::with_two_clips();
    let tracks = harness.project().tracks().len();
    harness.app.apply_for_tests(
        &harness.ctx,
        Action::BounceTrack {
            track: 1,
            duplicate: true,
        },
    );
    harness
        .app
        .apply_for_tests(&harness.ctx, Action::RenameTrack(1, "Updated".into()));
    harness.finish_bounce();
    assert_eq!(harness.project().tracks().len(), tracks);
    assert_eq!(harness.project().tracks()[1].name(), "Updated");
    assert!(!harness.project().tracks()[1].mixer().muted());
    assert!(
        harness
            .app
            .notice_for_tests()
            .unwrap()
            .contains("project changed")
    );
}

#[test]
fn cancelling_a_bounce_keeps_the_source_track_and_history() {
    let mut harness = Harness::with_two_clips();
    let revision = harness.app.document_for_tests().revision();
    let tracks = harness.project().tracks().len();
    harness.app.apply_for_tests(
        &harness.ctx,
        Action::BounceTrack {
            track: 1,
            duplicate: true,
        },
    );
    harness.app.cancel_bounce_for_tests();
    harness.finish_bounce();
    assert_eq!(harness.project().tracks().len(), tracks);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.project().tracks()[1].mixer().muted());
}

#[test]
fn dragging_a_clip_moves_it_on_the_grid_and_undo_restores_it() {
    let mut harness = Harness::with_two_clips();
    let clip = harness.clip_rect(1, 0);
    let bar = harness.project().time_signature().ticks_per_bar();
    let pixels_per_tick = clip.width() as f64 / 1920.0;
    let from = clip_title(clip);
    let to = from + vec2((bar as f64 * pixels_per_tick) as f32 + 3.0, 0.0);
    harness.drag(from, to, Modifiers::NONE);
    assert_eq!(harness.clip_start(1, 0), bar);
    assert_eq!(
        harness.app.selection_for_tests().clip,
        Some(ClipRef { track: 1, index: 0 })
    );

    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.clip_start(1, 0), 0);
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(harness.clip_start(1, 0), bar);
}

#[test]
fn dragging_onto_another_track_moves_the_clip_there() {
    let mut harness = Harness::with_two_clips();
    let kick = harness.clip_rect(1, 0);
    let tone = harness.clip_rect(2, 0);
    let to = pos2(kick.center().x + kick.width() * 1.5, tone.center().y);
    harness.drag(clip_title(kick), to, Modifiers::NONE);
    let counts: Vec<usize> = harness
        .project()
        .tracks()
        .iter()
        .map(|track| match track {
            Track::Audio(audio) => audio.clips().len(),
            _ => 0,
        })
        .collect();
    assert_eq!(counts, [0, 0, 2]);
}

#[test]
fn dragging_the_right_edge_trims_the_clip() {
    let mut harness = Harness::with_two_clips();
    let clip = harness.clip_rect(1, 0);
    let from = pos2(clip.right() - 2.0, clip_title(clip).y);
    let to = pos2(clip.left() + clip.width() * 0.5, clip_title(clip).y);
    harness.drag(from, to, Modifiers::NONE);
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!()
    };
    assert_eq!(track.clips()[0].frame_count, 24_000);
}

#[test]
fn space_plays_and_pauses() {
    let mut harness = Harness::with_two_clips();
    harness.key(Key::Space, Modifiers::NONE);
    std::thread::sleep(Duration::from_millis(60));
    harness.steps(2);
    let status = harness.app.status_for_tests();
    assert_eq!(status.state, PlaybackState::Playing);
    std::thread::sleep(Duration::from_millis(60));
    harness.steps(1);
    assert!(harness.app.status_for_tests().position_frame > 0);
    harness.key(Key::Space, Modifiers::NONE);
    std::thread::sleep(Duration::from_millis(30));
    harness.steps(1);
    assert_eq!(harness.app.status_for_tests().state, PlaybackState::Paused);
}

#[test]
fn empty_project_space_and_play_button_start_at_a_distant_marker() {
    fn wait_for_state(harness: &mut Harness, state: PlaybackState) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            harness.steps(1);
            if harness.app.status_for_tests().state == state {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "transport never reached {state:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn drain_silence(shared: &crate::audio::engine::Shared) {
        let mut output = [[1.0; 2]; 4096];
        let mut filled = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while filled < output.len() && std::time::Instant::now() < deadline {
            filled += shared.consume(&mut output[filled..]);
            if filled < output.len() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(
            filled,
            output.len(),
            "empty playback must supply timed silence"
        );
        assert_eq!(output, [[0.0; 2]; 4096]);
    }

    let mut harness = Harness::with_empty_project();
    assert_eq!(harness.project().tracks().len(), 1);
    assert_eq!(harness.project().duration_frames().unwrap(), 0);
    let shared = harness.app.capture_audio_for_tests();
    let marker = 10_000 * harness.project().time_signature().ticks_per_bar();
    let start_frame = harness.project().tempo().ticks_to_frames(marker).unwrap();
    harness.dispatch(Action::Seek(marker));
    harness.key(Key::Space, Modifiers::NONE);
    wait_for_state(&mut harness, PlaybackState::Playing);
    drain_silence(&shared);
    harness.steps(1);
    assert_eq!(
        harness.app.status_for_tests().position_frame,
        start_frame + 4096
    );
    assert_eq!(harness.app.arrangement_for_tests().playback_start, marker);

    harness.key(Key::Space, Modifiers::NONE);
    wait_for_state(&mut harness, PlaybackState::Paused);
    assert_eq!(
        harness.app.status_for_tests().position_frame,
        start_frame + 4096
    );

    let play = harness.labelled("Play from start marker (Space)");
    harness.click(play.center());
    wait_for_state(&mut harness, PlaybackState::Playing);
    drain_silence(&shared);
    harness.steps(1);
    assert_eq!(
        harness.app.status_for_tests().position_frame,
        start_frame + 4096
    );
    drain_silence(&shared);
    harness.steps(1);
    assert_eq!(
        harness.app.status_for_tests().position_frame,
        start_frame + 8192
    );

    let stop = harness.labelled("Stop; press again to return to the start (Home)");
    harness.click(stop.center());
    wait_for_state(&mut harness, PlaybackState::Stopped);
    assert_eq!(
        harness.app.status_for_tests().position_frame,
        start_frame + 8192
    );
    assert_eq!(harness.app.arrangement_for_tests().playback_start, marker);
    harness.click(stop.center());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while harness.app.status_for_tests().position_frame != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "second Stop did not return to zero"
        );
        std::thread::sleep(Duration::from_millis(2));
        harness.steps(1);
    }
    assert_eq!(harness.app.arrangement_for_tests().playback_start, 0);
}

#[test]
fn positioning_playback_over_a_clip_uses_lane_selection_without_interrupting_audio() {
    fn drain(shared: &crate::audio::engine::Shared) {
        let mut output = [[0.0; 2]; 256];
        let mut filled = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while filled < output.len() && std::time::Instant::now() < deadline {
            filled += shared.consume(&mut output[filled..]);
            if filled < output.len() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(filled, output.len(), "playback must keep producing audio");
    }

    let mut harness = Harness::with_two_clips();
    let shared = harness.app.capture_audio_for_tests();
    let clip = harness.clip_rect(1, 0);
    let ruler = harness.rect(Id::new("ruler-seek"));
    let revision = harness.app.document_for_tests().revision();
    let selection = harness.app.selection_for_tests().clone();

    harness.click(pos2(clip.center().x, ruler.center().y));
    assert_eq!(harness.app.arrangement_for_tests().playback_start, 960);
    assert_eq!(harness.app.selection_for_tests(), &selection);
    assert_eq!(harness.app.status_for_tests().position_frame, 0);
    harness.key(Key::Space, Modifiers::NONE);
    drain(&shared);
    harness.steps(1);
    let playing_from = harness.app.status_for_tests().position_frame;
    assert!((24_000..25_000).contains(&playing_from));

    harness.click(pos2(clip.left() + 4.0, clip.center().y));
    assert_eq!(harness.app.arrangement_for_tests().playback_start, 0);
    assert_eq!(harness.app.selection_for_tests().clip, None);
    assert_eq!(harness.app.selection_for_tests().range, None);
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([1])
    );
    drain(&shared);
    harness.steps(1);
    assert!((harness.rect(Id::new("playback-start")).center().x - clip.left()).abs() < 1.0);
    assert_eq!(harness.app.status_for_tests().state, PlaybackState::Playing);
    assert!(harness.app.status_for_tests().position_frame > playing_from);

    harness.key(Key::Space, Modifiers::NONE);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while harness.app.status_for_tests().state != PlaybackState::Paused
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(2));
        harness.steps(1);
    }
    assert_eq!(harness.app.status_for_tests().state, PlaybackState::Paused);
    harness.key(Key::Space, Modifiers::NONE);
    drain(&shared);
    harness.steps(1);
    assert_eq!(harness.app.status_for_tests().state, PlaybackState::Playing);
    assert!(harness.app.status_for_tests().position_frame < 1000);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn dragging_in_the_loop_band_sets_and_enables_the_loop() {
    let mut harness = Harness::with_two_clips();
    let band = harness.rect(Id::new("loop-band"));
    let clip = harness.clip_rect(1, 0);
    let x = |tick: f32| clip.left() + tick * clip.width() / 1920.0;
    harness.drag(
        pos2(x(19_250.0), band.center().y),
        pos2(x(23_000.0), band.center().y),
        Modifiers::NONE,
    );
    let region = harness.app.loop_for_tests();
    assert!(region.enabled);
    assert_eq!((region.start, region.end), (19_200, 23_040));
    harness.drag(
        pos2(x(19_200.0), band.center().y),
        pos2(x(15_400.0), band.center().y),
        Modifiers::NONE,
    );
    let region = harness.app.loop_for_tests();
    assert_eq!((region.start, region.end), (15_360, 23_040));
}

#[test]
fn ctrl_l_loops_the_selected_clip_and_l_still_only_toggles() {
    let mut harness = Harness::with_two_clips();
    let selected = ClipRef { track: 1, index: 0 };
    harness.dispatch(Action::MoveClip {
        clip: selected,
        to_track: 1,
        start: 960,
    });
    harness.dispatch(Action::SelectClip(None));
    harness.steps(2);
    let clip = harness.clip_rect(1, 0);
    harness.click(clip.left_top() + vec2(1.0, 4.0));
    assert_eq!(harness.app.selection_for_tests().clip, Some(selected));
    let revision = harness.app.document_for_tests().revision();
    assert_eq!(harness.app.arrangement_for_tests().playback_start, 0);

    for modifiers in [Modifiers::CTRL, Modifiers::CTRL, Modifiers::COMMAND] {
        harness.key(Key::L, modifiers);
        assert_eq!(
            harness.app.loop_for_tests(),
            crate::state::LoopRegion {
                start: 960,
                end: 2880,
                enabled: true,
            }
        );
    }
    harness.key(Key::L, Modifiers::NONE);
    assert_eq!(
        harness.app.loop_for_tests(),
        crate::state::LoopRegion {
            start: 960,
            end: 2880,
            enabled: false,
        }
    );
    harness.key(Key::L, Modifiers::CTRL);
    assert!(harness.app.loop_for_tests().enabled);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn ctrl_l_loops_the_full_time_selection_across_tracks() {
    let mut harness = Harness::with_instruments();
    harness.fixed_grid(sympoie::time::SnapGrid::Quarter);
    let first = harness.rect(Id::new(("lane", 1)));
    let second = harness.rect(Id::new(("lane", 2)));
    harness.drag(
        pos2(harness.lane_x(960) + 1.0, first.center().y),
        pos2(harness.lane_x(6720) + 1.0, second.center().y),
        Modifiers::NONE,
    );
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 960,
            end: 6720,
            tracks: BTreeSet::from([1, 2]),
        })
    );
    harness.dispatch(Action::Seek(0));
    let revision = harness.app.document_for_tests().revision();
    for modifiers in [Modifiers::CTRL, Modifiers::COMMAND] {
        harness.key(Key::L, modifiers);
        assert_eq!(
            harness.app.loop_for_tests(),
            crate::state::LoopRegion {
                start: 960,
                end: 6720,
                enabled: true,
            }
        );
    }
    assert_eq!(harness.app.document_for_tests().revision(), revision);

    harness.key(Key::Escape, Modifiers::NONE);
    harness.key(Key::L, Modifiers::NONE);
    let previous = harness.app.loop_for_tests();
    harness.key(Key::L, Modifiers::CTRL);
    assert_eq!(harness.app.loop_for_tests(), previous);
}

#[test]
fn ctrl_l_uses_the_arrangement_span_of_a_midi_clip_while_editing_notes() {
    let mut harness = Harness::with_midi_clip();
    harness.key(Key::L, Modifiers::CTRL);
    assert_eq!(
        harness.app.loop_for_tests(),
        crate::state::LoopRegion {
            start: 3840,
            end: 11520,
            enabled: true,
        }
    );
}

#[test]
fn keyboard_shortcuts_add_and_group_tracks() {
    let mut harness = Harness::with_two_clips();
    harness.key(Key::T, Modifiers::COMMAND);
    harness.key(Key::T, Modifiers::COMMAND | Modifiers::SHIFT);
    let kinds: Vec<_> = harness
        .project()
        .tracks()
        .iter()
        .map(crate::document::rows::kind_label)
        .collect();
    assert_eq!(kinds, ["MASTER", "AUDIO", "AUDIO", "AUDIO", "MIDI"]);

    let first = harness.rect(Id::new(("header", 1)));
    let second = harness.rect(Id::new(("header", 2)));
    harness.click(first.left_bottom() + vec2(6.0, -4.0));
    harness.move_to(second.left_bottom() + vec2(6.0, -4.0));
    harness.button(true, Modifiers::SHIFT);
    harness.button(false, Modifiers::SHIFT);
    harness.key(Key::G, Modifiers::COMMAND);
    let parents = crate::document::rows::parents(harness.project());
    assert_eq!(parents[1], Some(5));
    assert_eq!(parents[2], Some(5));
    assert_eq!(harness.project().tracks()[5].name(), "Group 1");
}

#[test]
fn deleting_the_selected_clip_with_the_keyboard() {
    let mut harness = Harness::with_two_clips();
    let clip = harness.clip_rect(2, 0);
    harness.click(clip_title(clip));
    harness.key(Key::Delete, Modifiers::NONE);
    let Track::Audio(track) = &harness.project().tracks()[2] else {
        panic!()
    };
    assert!(track.clips().is_empty());
    assert!(harness.app.selection_for_tests().clip.is_none());
}

#[test]
fn symbols_used_in_the_interface_have_glyphs() {
    let harness = Harness::with_two_clips();
    let font = egui::FontId::proportional(13.0);
    for symbol in "→∞…·“”ø•×⌘".chars() {
        assert!(
            harness
                .ctx
                .fonts_mut(|fonts| fonts.has_glyph(&font, symbol)),
            "missing glyph {symbol:?}"
        );
    }
}

#[test]
fn track_context_menu_renames_in_one_undo_step() {
    let mut harness = Harness::with_two_clips();
    harness.begin_track_rename(1);
    harness.key(Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Text("Drums".to_owned())]);
    harness.key(Key::Enter, Modifiers::NONE);
    harness.steps(2);
    assert_eq!(harness.project().tracks()[1].name(), "Drums");
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.project().tracks()[1].name(), "kick");
}

#[test]
fn automation_button_and_a_toggle_view_without_editing_project() {
    let mut harness = Harness::with_instruments();
    let revision = harness.app.document_for_tests().revision();
    assert!(!harness.app.arrangement_for_tests().automation);
    harness.key(Key::A, Modifiers::NONE);
    harness.steps(2);
    assert!(harness.app.arrangement_for_tests().automation);
    assert!(
        harness
            .ctx
            .read_response(Id::new(("automation-lane", 1)))
            .is_some()
    );
    let button = harness.rect(Id::new("automation-toggle"));
    harness.click(button.center());
    assert!(!harness.app.arrangement_for_tests().automation);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn typing_a_track_name_does_not_toggle_automation() {
    let mut harness = Harness::with_instruments();
    harness.begin_track_rename(1);
    harness.key(Key::A, Modifiers::COMMAND);
    harness.key(Key::A, Modifiers::NONE);
    harness.step(vec![Event::Text("Analog".to_owned())]);
    harness.key(Key::Enter, Modifiers::NONE);
    assert_eq!(harness.project().tracks()[1].name(), "Analog");
    assert!(!harness.app.arrangement_for_tests().automation);
}

#[test]
fn automation_edit_undo_and_redo_leave_clip_content_intact() {
    let mut harness = Harness::with_two_clips();
    let clips_before = format!("{:?}", harness.project().tracks()[1]);
    harness.key(Key::A, Modifiers::NONE);
    harness.steps(2);
    let lane = harness.rect(Id::new(("automation-lane", 1)));
    harness.click(lane.left_center() + vec2(80.0, 0.0));
    assert_eq!(harness.project().tracks()[1].automation().len(), 1);
    let envelope = harness.project().tracks()[1].automation()[0].clone();
    harness.key(Key::Z, Modifiers::COMMAND);
    assert!(harness.project().tracks()[1].automation().is_empty());
    assert_eq!(format!("{:?}", harness.project().tracks()[1]), clips_before);
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(harness.project().tracks()[1].automation(), &[envelope]);
    harness.key(Key::Delete, Modifiers::NONE);
    assert_eq!(harness.project().tracks()[1].automation().len(), 1);
    harness.key(Key::A, Modifiers::NONE);
    assert!(!harness.app.arrangement_for_tests().automation);
    assert_eq!(harness.project().tracks()[1].automation().len(), 1);
}

#[test]
fn automation_line_insertion_and_numeric_entry_are_undoable_without_seeking() {
    let mut harness = Harness::with_two_clips();
    harness.fixed_grid(sympoie::time::SnapGrid::Quarter);
    let target = AutomationTarget::TrackGain;
    let original = AutomationLane::new(
        target,
        vec![
            AutomationPoint {
                tick: 0,
                value: 0.2,
            },
            AutomationPoint {
                tick: 1920,
                value: 0.8,
            },
        ],
    )
    .unwrap();
    harness.dispatch(Action::SetAutomation {
        track: 1,
        lane: original.clone(),
    });
    harness.dispatch(Action::ToggleAutomation);
    harness.dispatch(Action::Seek(3840));
    let lane = harness.rect(Id::new(("automation-lane", 1)));
    let view = harness.app.arrangement_for_tests();
    let x = lane.left() + ((1100.0 - view.scroll_tick) * view.pixels_per_tick) as f32;
    harness.click(pos2(x, lane.center().y + 4.0));
    let inserted = harness.project().tracks()[1].automation()[0].clone();
    assert_eq!(
        inserted.points()[1],
        AutomationPoint {
            tick: 960,
            value: 0.5
        }
    );
    assert_eq!(harness.app.arrangement_for_tests().playback_start, 3840);
    harness.dispatch(Action::Undo);
    assert_eq!(
        harness.project().tracks()[1].automation(),
        std::slice::from_ref(&original)
    );
    harness.dispatch(Action::Redo);
    assert_eq!(
        harness.project().tracks()[1].automation(),
        std::slice::from_ref(&inserted)
    );

    let point = harness.rect(Id::new(("automation-point", 1, target, 960_u64)));
    harness.right_click(point.center());
    assert!(harness.ctx.egui_wants_keyboard_input());
    let revision = harness.app.document_for_tests().revision();
    harness.key(Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Text("-6 dB".into())]);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    harness.key(Key::Enter, Modifiers::NONE);
    let edited = harness.project().tracks()[1].automation()[0].clone();
    assert_eq!(harness.app.document_for_tests().revision(), revision + 1);
    assert!((edited.points()[1].value - 10.0_f64.powf(-6.0 / 20.0) / 2.0).abs() < 1e-12);
    assert_eq!(edited.points()[1].tick, 960);
    assert_eq!(edited.points()[0], inserted.points()[0]);
    assert_eq!(edited.points()[2], inserted.points()[2]);
    assert_eq!(harness.app.arrangement_for_tests().playback_start, 3840);
    harness.dispatch(Action::Undo);
    assert_eq!(
        harness.project().tracks()[1].automation(),
        std::slice::from_ref(&inserted)
    );
    harness.dispatch(Action::Redo);
    assert_eq!(harness.project().tracks()[1].automation(), &[edited]);
}

#[test]
fn automation_escape_cancels_drag_before_global_shortcuts() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::ToggleAutomation);
    let lane = harness.rect(Id::new(("automation-lane", 1)));
    let from = lane.left_center() + vec2(96.0, 0.0);
    harness.click(from);
    let before = harness.project().tracks()[1].automation().to_vec();
    harness.time += 0.7;
    harness.move_to(from);
    harness.button(true, Modifiers::NONE);
    harness.move_to(from + vec2(80.0, -20.0));
    harness.key(Key::Escape, Modifiers::NONE);
    harness.button(false, Modifiers::NONE);
    assert_eq!(harness.project().tracks()[1].automation(), before);
}

#[test]
fn automation_target_follows_touched_parameter_and_track_order_history() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::ToggleAutomation);
    harness.dispatch(Action::SetInstrumentParameter {
        track: 1,
        instrument: Instrument::Subtractive(SubtractiveSettings {
            cutoff_hz: 1200.0,
            ..SubtractiveSettings::default()
        }),
        parameter: "cutoff",
    });
    let target = AutomationTarget::Subtractive(SubtractiveParameter::Cutoff);
    harness.dispatch(Action::SetAutomation {
        track: 1,
        lane: AutomationLane::new(
            target,
            vec![AutomationPoint {
                tick: 0,
                value: 0.4,
            }],
        )
        .unwrap(),
    });
    assert_eq!(
        harness
            .app
            .arrangement_for_tests()
            .automation_targets
            .get(&1),
        Some(&target)
    );
    harness.dispatch(Action::MoveTrack {
        track: 1,
        target: TrackDrop::After(2),
    });
    assert_eq!(harness.project().tracks()[2].name(), "Lead");
    assert_eq!(
        harness.project().tracks()[2].automation()[0].target(),
        target
    );
    assert_eq!(
        harness
            .app
            .arrangement_for_tests()
            .automation_targets
            .get(&2),
        Some(&target)
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().tracks()[1].name(), "Lead");
    assert_eq!(
        harness.project().tracks()[1].automation()[0].target(),
        target
    );
    assert_eq!(
        harness
            .app
            .arrangement_for_tests()
            .automation_targets
            .get(&1),
        Some(&target)
    );
}

#[test]
fn the_selected_clip_can_be_split_at_the_playhead() {
    let mut harness = Harness::with_two_clips();
    let clip = harness.clip_rect(1, 0);
    let ruler = harness.rect(Id::new("ruler-seek"));
    harness.click(pos2(clip.center().x, ruler.center().y));
    harness.click(clip_title(clip));
    harness.key(Key::E, Modifiers::COMMAND);
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!()
    };
    let lengths: Vec<u64> = track.clips().iter().map(|clip| clip.frame_count).collect();
    assert_eq!(lengths, [24_000, 24_000]);
}

#[test]
fn piano_roll_opens_and_creates_clip_relative_notes() {
    let mut harness = Harness::with_midi_clip();
    let grid = harness.rect(Id::new("piano-grid"));
    let first = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    let point = pos2(
        first.left() + 2900.0 * pixels_per_tick,
        first.center().y - 2.0 * key_height,
    );
    assert!(grid.contains(point));
    harness.double_click(point);

    let clip = harness.midi_clip();
    assert_eq!(clip.start_tick, 3840);
    assert_eq!(clip.length_ticks, 7680);
    assert_eq!(clip.notes.len(), 4);
    let note = clip.notes.last().unwrap();
    assert_eq!(note.start_tick, 2880, "note time is local to the clip");
    assert_eq!(note.duration_ticks, 240);
    assert_eq!(note.key, 62);
    assert!((1..=127).contains(&note.velocity));

    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes.len(), 3);
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(harness.midi_clip().notes.len(), 4);
}

#[test]
fn a_new_midi_clip_opens_an_editable_piano_roll() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::NewProject);
    harness.key(Key::T, Modifiers::COMMAND | Modifiers::SHIFT);
    let Track::Midi(track) = &harness.project().tracks()[1] else {
        panic!("new track is not MIDI")
    };
    assert!(track.instrument().is_none());
    assert!(track.plugin_instrument().is_none());
    assert!(track.effects().is_empty());
    assert!(track.clips().is_empty());
    let lane = harness.rect(Id::new(("lane", 1)));
    harness.double_click(lane.left_center() + vec2(40.0, 0.0));
    let grid = harness.rect(Id::new("piano-grid"));
    harness.double_click(grid.center());

    let Track::Midi(track) = &harness.project().tracks()[1] else {
        panic!("new track is not MIDI")
    };
    assert_eq!(track.clips().len(), 1);
    let clip = &track.clips()[0];
    assert_eq!(clip.notes.len(), 1);
    assert_eq!(clip.length_ticks, 3840);
    assert_eq!(clip.notes[0].duration_ticks, 240);
    assert!(clip.notes[0].start_tick + clip.notes[0].duration_ticks <= clip.length_ticks);
    assert!(track.instrument().is_none());
    let original = clip.clone();
    let start_frame = harness
        .project()
        .tempo()
        .ticks_to_frames(clip.start_tick + clip.notes[0].start_tick)
        .unwrap();
    let mut heard = [[0.0; 2]; 512];
    sympoie::render::render_block(harness.project(), start_frame, &mut heard).unwrap();
    assert_eq!(heard, [[0.0; 2]; 512]);

    harness.dispatch(Action::ShowDevices);
    let add = harness.rect(Id::new(("add-instrument", 1_usize)));
    harness.click(add.center());
    harness.steps(3);
    let Track::Midi(track) = &harness.project().tracks()[1] else {
        panic!("MIDI track")
    };
    assert!(track.instrument().is_none());
    assert_eq!(
        track
            .plugin_instrument()
            .and_then(|plugin| sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)),
        Some(sympoie::builtin::BuiltinKind::Oscil)
    );
    assert_eq!(harness.midi_clips(1), [original]);
    sympoie::render::render_block(harness.project(), start_frame, &mut heard).unwrap();
    assert!(heard.iter().any(|frame| frame[0].abs() > 0.0001));
}

#[test]
fn subtractive_power_keeps_notes_parameters_history_and_saved_state() {
    let mut harness = Harness::with_midi_clip();
    let notes = harness.midi_clip().clone();
    let original = SubtractiveSettings {
        cutoff_hz: 725.0,
        ..SubtractiveSettings::default()
    };
    harness.dispatch(Action::SetInstrument {
        track: 1,
        instrument: Some(Instrument::Subtractive(original)),
    });
    harness.dispatch(Action::ShowDevices);
    let power = harness.rect(Id::new(("device-instrument-activator", 1_usize)));
    harness.click(power.center());
    assert!(harness.instrument_bypassed(1));
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(original))
    );
    assert_eq!(harness.midi_clips(1), std::slice::from_ref(&notes));
    let mut output = [[1.0; 2]; 512];
    sympoie::render::render_block(harness.project(), 96_000, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 512]);

    let changed = SubtractiveSettings {
        cutoff_hz: 1300.0,
        ..original
    };
    harness.dispatch(Action::SetInstrumentParameter {
        track: 1,
        instrument: Instrument::Subtractive(changed),
        parameter: "cutoff",
    });
    assert!(harness.instrument_bypassed(1));
    harness.dispatch(Action::Undo);
    assert!(harness.instrument_bypassed(1));
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(original))
    );
    harness.dispatch(Action::Undo);
    assert!(!harness.instrument_bypassed(1));
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(original))
    );
    harness.dispatch(Action::Redo);
    harness.dispatch(Action::Redo);
    assert!(harness.instrument_bypassed(1));
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(changed))
    );

    let open = harness.rect(Id::new(("open-plugin-editor", 1_usize)));
    harness.click(open.center());
    let power = harness.rect(Id::new(("editor-instrument-activator", 1_usize)));
    harness.click(power.center());
    assert!(!harness.instrument_bypassed(1));
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(changed))
    );
    sympoie::render::render_block(harness.project(), 96_000, &mut output).unwrap();
    assert!(output.iter().any(|frame| frame[0].abs() > 0.0001));
    harness.dispatch(Action::Undo);
    assert!(harness.instrument_bypassed(1));
    assert_eq!(harness.midi_clips(1), std::slice::from_ref(&notes));
    harness.dispatch(Action::Save);
    let path = harness._files.0.join("notes.symp");
    let saved = Project::load(&path).unwrap();
    let Track::Midi(track) = &saved.tracks()[1] else {
        panic!("saved MIDI track was lost")
    };
    assert!(track.instrument_bypassed());
    assert_eq!(track.instrument(), Some(&Instrument::Subtractive(changed)));
    assert_eq!(track.clips(), std::slice::from_ref(&notes));
    harness.dispatch(Action::OpenPath(path));
    assert!(harness.instrument_bypassed(1));
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(changed))
    );
    assert_eq!(harness.midi_clips(1), [notes]);
}

#[test]
fn instruments_can_be_assigned_edited_undone_and_saved_without_changing_notes() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().clone();
    assert_eq!(harness.instrument(1), None);
    harness.dispatch(Action::SetInstrument {
        track: 1,
        instrument: Some(Instrument::Subtractive(SubtractiveSettings::default())),
    });

    let mut settings = SubtractiveSettings::default();
    for cutoff in [900.0, 1500.0] {
        settings.cutoff_hz = cutoff;
        harness.dispatch(Action::SetInstrumentParameter {
            track: 1,
            instrument: Instrument::Subtractive(settings),
            parameter: "cutoff",
        });
    }
    settings.waveform = Waveform::Square;
    harness.dispatch(Action::SetInstrumentParameter {
        track: 1,
        instrument: Instrument::Subtractive(settings),
        parameter: "waveform",
    });
    harness.key(Key::Z, Modifiers::COMMAND);
    settings.waveform = SubtractiveSettings::default().waveform;
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(settings))
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(SubtractiveSettings::default()))
    );
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(settings))
    );
    assert_eq!(*harness.midi_clip(), before);

    harness.key(Key::S, Modifiers::COMMAND);
    let loaded = Project::load(harness._files.0.join("notes.symp")).unwrap();
    let Track::Midi(track) = &loaded.tracks()[1] else {
        panic!("saved MIDI track was lost")
    };
    assert_eq!(track.instrument(), Some(&Instrument::Subtractive(settings)));
    assert_eq!(track.clips()[0], before);

    harness.dispatch(Action::SetInstrument {
        track: 1,
        instrument: None,
    });
    let mut output = [[1.0; 2]; 512];
    sympoie::render::render_block(harness.project(), 96_000, &mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 512]);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert!(harness.instrument(1).is_some());
}

#[test]
fn double_clicking_a_track_header_shows_its_complete_device_chain_in_the_rack() {
    let mut harness = Harness::with_instruments();
    for track in [1, 2, 3, 0] {
        harness.dispatch(Action::ShowMixer);
        harness.open_track_devices(track);
        assert!(harness.app.showing_devices_for_tests());
        let chain = harness.rect(Id::new(("device-chain", track)));
        let effects = harness.rect(Id::new(("device-empty-effects", track)));
        assert!(
            chain.left() > 900.0,
            "devices belong in the rack on the right"
        );
        assert!(chain.contains(effects.center()));
        assert!(
            harness
                .ctx
                .read_response(Id::new(("device-output", track)))
                .is_none()
        );
        if matches!(harness.project().tracks()[track], Track::Midi(_)) {
            let instrument = harness.rect(Id::new(("device-instrument", track)));
            assert!(chain.contains(instrument.center()));
            assert!(instrument.bottom() <= effects.top());
        }
    }
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn vst3_chain_cards_follow_instrument_effect_and_add_card_order_on_every_track_kind() {
    let mut harness = Harness::with_vst3_devices();
    for track in [1, 2, 3, 0] {
        harness.select_track(track);
        let mut ids = Vec::new();
        if let Track::Midi(midi) = &harness.project().tracks()[track] {
            ids.push(midi.plugin_instrument().unwrap().id);
        }
        ids.extend(
            harness.project().tracks()[track]
                .effects()
                .iter()
                .map(|plugin| plugin.id),
        );
        let chain = harness.rect(Id::new(("device-chain", track)));
        let cards: Vec<_> = ids
            .iter()
            .map(|id| harness.rect(Id::new(("device-vst3", *id as usize))))
            .collect();
        assert!(chain.left() > 900.0);
        assert!(chain.contains(cards[0].center()));
        for pair in cards.windows(2) {
            assert!(
                pair[0].bottom() <= pair[1].top(),
                "signal order matches the saved chain"
            );
        }
        let add = harness.rect(Id::new(("device-empty-effects", track)));
        assert!(cards.last().unwrap().bottom() <= add.top());
        assert!(
            harness
                .ctx
                .read_response(Id::new(("device-output", track)))
                .is_none()
        );
    }
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn vst3_parameter_gestures_and_bypass_are_undoable_and_persist_without_changing_ids() {
    let mut harness = Harness::with_vst3_devices();
    let id = harness.effect_id(1, 0);
    let untouched = harness.effect_id(1, 1);
    for value in [0.6, 0.7, 0.8] {
        harness.dispatch(Action::SetVst3Parameter {
            id,
            parameter: 37,
            value,
        });
    }
    harness.dispatch(Action::SetVst3Bypassed { id, bypassed: true });
    assert!(harness.project().plugin(id).unwrap().bypassed);
    assert_eq!(harness.plugin_parameter(id), 0.8);
    assert_eq!(harness.plugin_parameter(untouched), 0.5);
    harness.dispatch(Action::Undo);
    assert!(!harness.project().plugin(id).unwrap().bypassed);
    assert_eq!(harness.plugin_parameter(id), 0.8);
    harness.dispatch(Action::Undo);
    assert_eq!(
        harness.plugin_parameter(id),
        0.5,
        "the continuous gesture uses one undo step"
    );
    assert!(!harness.app.document_for_tests().is_dirty());
    harness.dispatch(Action::Redo);
    harness.dispatch(Action::Redo);
    harness.key(Key::S, Modifiers::COMMAND);
    let saved = Project::load(harness._files.0.join("plugins.symp")).unwrap();
    assert_eq!(saved.plugin(id), harness.project().plugin(id));
    assert_eq!(saved.plugin(untouched), harness.project().plugin(untouched));
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn vst3_sidechain_routes_and_gain_are_undoable_and_saved() {
    let mut harness = Harness::with_vst3_devices();
    let id = harness.effect_id(1, 0);
    let route = SidechainRoute {
        source_track: 2,
        bus_index: 1,
        tap: SidechainTap::PostFader,
        gain: 1.0,
    };
    harness.dispatch(Action::SetVst3Sidechain {
        id,
        route: Some(route),
    });
    for gain in [0.9, 0.7, 0.5] {
        harness.dispatch(Action::SetVst3SidechainGain { id, gain });
    }
    assert_eq!(
        harness
            .project()
            .plugin(id)
            .unwrap()
            .sidechain
            .unwrap()
            .gain,
        0.5
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().plugin(id).unwrap().sidechain, Some(route));
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().plugin(id).unwrap().sidechain, None);
    harness.dispatch(Action::Redo);
    harness.dispatch(Action::Redo);
    harness.key(Key::S, Modifiers::COMMAND);
    let saved = Project::load(harness._files.0.join("plugins.symp")).unwrap();
    assert_eq!(saved.plugin(id), harness.project().plugin(id));

    harness.select_track(1);
    harness.click(harness.rect(Id::new(("vst3-sidechain", id))).center());
    harness.click(harness.rect(Id::new(("sidechain-source", id))).center());
    harness.click(
        harness
            .rect(Id::new(("sidechain-source-none", id)))
            .center(),
    );
    assert_eq!(harness.project().plugin(id).unwrap().sidechain, None);
}

#[test]
fn vst3_sidechain_feedback_is_rejected_without_editing_the_project() {
    let mut harness = Harness::with_vst3_devices();
    let first = harness.effect_id(1, 0);
    let second = harness.effect_id(2, 0);
    harness.dispatch(Action::SetVst3Sidechain {
        id: first,
        route: Some(SidechainRoute {
            source_track: 2,
            bus_index: 1,
            tap: SidechainTap::PostEffects,
            gain: 1.0,
        }),
    });
    let before = harness.project().clone();
    let revision = harness.app.document_for_tests().revision();
    harness.dispatch(Action::SetVst3Sidechain {
        id: second,
        route: Some(SidechainRoute {
            source_track: 1,
            bus_index: 1,
            tap: SidechainTap::PostEffects,
            gain: 1.0,
        }),
    });
    assert_eq!(harness.project().plugin(first), before.plugin(first));
    assert_eq!(harness.project().plugin(second), before.plugin(second));
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn vst3_parameter_windows_restore_per_track_and_follow_ids_after_track_deletion() {
    let mut harness = Harness::with_vst3_devices();
    let removed = harness.effect_id(1, 0);
    let survivor = harness.effect_id(2, 0);
    let revision = harness.app.document_for_tests().revision();
    harness.dispatch(Action::OpenVst3Editor(removed));
    harness.dispatch(Action::OpenVst3Editor(survivor));
    harness.select_track(3);
    harness.dispatch(Action::ShowMixer);
    assert_eq!(
        harness.app.vst3_editors_for_tests(),
        &BTreeSet::from([removed, survivor])
    );
    for id in [removed, survivor] {
        harness.assert_visible(Id::new(("close-vst3-editor", id)), false);
    }
    harness.select_track(1);
    harness.assert_visible(Id::new(("close-vst3-editor", removed)), true);
    harness.assert_visible(Id::new(("close-vst3-editor", survivor)), false);
    harness.select_track(2);
    harness.assert_visible(Id::new(("close-vst3-editor", removed)), false);
    harness.assert_visible(Id::new(("close-vst3-editor", survivor)), true);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.app.document_for_tests().is_dirty());

    harness.dispatch(Action::DeleteTrack(1));
    assert_eq!(harness.project().tracks()[1].name(), "Audio");
    assert_eq!(harness.effect_id(1, 0), survivor);
    assert_eq!(
        harness.app.vst3_editors_for_tests(),
        &BTreeSet::from([survivor])
    );
    harness.select_track(1);
    harness.assert_visible(Id::new(("close-vst3-editor", survivor)), true);
    harness.assert_visible(Id::new(("close-vst3-editor", removed)), false);
    harness.dispatch(Action::SetVst3Parameter {
        id: survivor,
        parameter: 37,
        value: 0.2,
    });
    assert_eq!(harness.plugin_parameter(survivor), 0.2);
    harness.dispatch(Action::CloseVst3Editor(survivor));
    assert!(harness.app.vst3_editors_for_tests().is_empty());
    assert!(harness.project().plugin(survivor).is_some());
    harness.dispatch(Action::OpenVst3Editor(removed));
    harness.dispatch(Action::OpenVst3Editor(u64::MAX));
    assert!(harness.app.vst3_editors_for_tests().is_empty());
}

#[test]
fn vst3_parameter_slider_edits_only_the_selected_tracks_restored_window() {
    let mut harness = Harness::with_vst3_devices();
    let first = harness.effect_id(1, 0);
    let second = harness.effect_id(2, 0);
    harness.dispatch(Action::OpenVst3Editor(first));
    harness.dispatch(Action::OpenVst3Editor(second));
    harness.select_track(3);
    harness.assert_visible(Id::new(("vst3-parameter", first, 37_u32)), false);
    harness.assert_visible(Id::new(("vst3-parameter", second, 37_u32)), false);
    harness.select_track(2);
    harness.assert_visible(Id::new(("vst3-parameter", first, 37_u32)), false);
    let slider = harness.rect(Id::new(("vst3-parameter", second, 37_u32)));
    harness.drag(
        slider.left_center() + vec2(12.0, 0.0),
        slider.left_center() + vec2(60.0, 0.0),
        Modifiers::NONE,
    );
    assert_eq!(harness.plugin_parameter(first), 0.5);
    assert_ne!(harness.plugin_parameter(second), 0.5);
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([2])
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.plugin_parameter(second), 0.5);
    assert_eq!(
        harness.app.vst3_editors_for_tests(),
        &BTreeSet::from([first, second])
    );
    assert!(!harness.app.document_for_tests().is_dirty());
    let close = harness.rect(Id::new(("close-vst3-editor", second)));
    harness.click(close.center());
    assert_eq!(
        harness.app.vst3_editors_for_tests(),
        &BTreeSet::from([first])
    );
    harness.select_track(1);
    harness.assert_visible(Id::new(("close-vst3-editor", first)), true);
    harness.select_track(2);
    harness.assert_visible(Id::new(("close-vst3-editor", second)), false);
}

#[test]
fn clip_selection_restores_a_tracks_vst3_windows_and_ambiguous_selection_hides_all() {
    let mut harness = Harness::with_vst3_devices();
    harness.dispatch(Action::InsertMidiClip { track: 1, start: 0 });
    let first = harness.effect_id(1, 0);
    let sibling = harness.effect_id(1, 1);
    let other = harness.effect_id(2, 0);
    for id in [first, sibling, other] {
        harness.dispatch(Action::OpenVst3Editor(id));
    }
    for selection in [BTreeSet::from([1, 2]), BTreeSet::new()] {
        harness.dispatch(Action::SelectTracks(selection));
        for id in [first, sibling, other] {
            harness.assert_visible(Id::new(("close-vst3-editor", id)), false);
        }
        assert_eq!(
            harness.app.vst3_editors_for_tests(),
            &BTreeSet::from([first, sibling, other])
        );
    }
    harness.dispatch(Action::SelectClip(Some(ClipRef { track: 1, index: 0 })));
    for id in [first, sibling] {
        harness.assert_visible(Id::new(("close-vst3-editor", id)), true);
    }
    harness.assert_visible(Id::new(("close-vst3-editor", other)), false);
    harness.dispatch(Action::CloseVst3Editor(first));
    harness.select_track(2);
    harness.assert_visible(Id::new(("close-vst3-editor", other)), true);
    harness.select_track(1);
    harness.assert_visible(Id::new(("close-vst3-editor", first)), false);
    harness.assert_visible(Id::new(("close-vst3-editor", sibling)), true);
}

#[test]
fn viewport_close_events_for_hidden_editors_forget_their_reopen_requests() {
    let mut harness = Harness::with_vst3_devices();
    let generic = harness.effect_id(1, 0);
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    let builtin = harness.project().tracks().len() - 1;
    harness.dispatch(Action::SetInstrument {
        track: builtin,
        instrument: Some(Instrument::Subtractive(SubtractiveSettings::default())),
    });
    harness.dispatch(Action::OpenPluginEditor(builtin));
    harness.dispatch(Action::OpenVst3Editor(generic));
    harness.select_track(3);
    harness.assert_visible(Id::new(("close-plugin-editor", builtin)), false);
    harness.assert_visible(Id::new(("close-vst3-editor", generic)), false);
    assert!(harness.app.plugin_editors_for_tests().contains(&builtin));
    assert!(harness.app.vst3_editors_for_tests().contains(&generic));

    let mut input = RawInput::default();
    for viewport in [
        crate::views::devices::editor_viewport_id(builtin),
        crate::views::devices::vst3_editor_viewport_id(generic),
    ] {
        input.viewports.insert(
            viewport,
            egui::ViewportInfo {
                events: vec![egui::ViewportEvent::Close],
                ..egui::ViewportInfo::default()
            },
        );
    }
    harness.step_input(input);
    harness.steps(2);
    assert!(harness.app.plugin_editors_for_tests().is_empty());
    assert!(harness.app.vst3_editors_for_tests().is_empty());
    harness.select_track(builtin);
    harness.assert_visible(Id::new(("close-plugin-editor", builtin)), false);
    harness.select_track(1);
    harness.assert_visible(Id::new(("close-vst3-editor", generic)), false);
}

#[test]
fn sidechain_windows_restore_per_track_and_forget_closed_removed_or_replaced_devices() {
    let mut harness = Harness::with_vst3_devices();
    let first = harness.effect_id(1, 0);
    let second = harness.effect_id(2, 0);
    for id in [first, second] {
        harness.dispatch(Action::OpenVst3Sidechain(id));
    }
    harness.select_track(1);
    harness.assert_visible(Id::new(("sidechain-source", first)), true);
    harness.assert_visible(Id::new(("sidechain-source", second)), false);
    harness.select_track(2);
    harness.assert_visible(Id::new(("sidechain-source", first)), false);
    harness.assert_visible(Id::new(("sidechain-source", second)), true);
    for selection in [BTreeSet::from([1, 2]), BTreeSet::new()] {
        harness.dispatch(Action::SelectTracks(selection));
        for id in [first, second] {
            harness.assert_visible(Id::new(("sidechain-source", id)), false);
        }
        assert_eq!(
            harness.app.sidechain_editors_for_tests(),
            &BTreeSet::from([first, second])
        );
    }
    harness.select_track(1);
    harness.dispatch(Action::CloseVst3Sidechain(first));
    harness.select_track(2);
    harness.select_track(1);
    harness.assert_visible(Id::new(("sidechain-source", first)), false);
    harness.dispatch(Action::RemoveVst3 {
        track: 2,
        id: second,
    });
    assert!(harness.app.sidechain_editors_for_tests().is_empty());
    harness.dispatch(Action::Undo);
    harness.select_track(2);
    assert!(harness.project().plugin(second).is_some());
    harness.assert_visible(Id::new(("sidechain-source", second)), false);
    harness.dispatch(Action::OpenVst3Sidechain(second));
    harness.dispatch(Action::OpenPath(harness._files.0.join("plugins.symp")));
    assert!(harness.app.sidechain_editors_for_tests().is_empty());
    harness.select_track(2);
    harness.assert_visible(Id::new(("sidechain-source", second)), false);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn read_only_stepped_vst3_parameters_never_emit_edits_when_rendered_off_grid() {
    let mut project = Project::new();
    let mut plugin = fake_vst3(PluginKind::Effect);
    plugin.parameters[0].read_only = true;
    plugin.parameters[0].steps = 1;
    plugin.parameters[0].value = 0.37;
    let id = project
        .add_effect(project.master_track_id().into(), plugin)
        .unwrap();
    let document = crate::document::Document::new(project, Some(PathBuf::from("readonly.symp")));
    let revision = document.revision();
    let ctx = egui::Context::default();
    for frame in 0..5 {
        let mut actions = Vec::new();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
            time: Some(frame as f64 / 60.0),
            ..RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            crate::views::devices::show_vst3_editor(ui.ctx(), document.project(), id, &mut actions);
        });
        output.textures_delta.clear();
        assert!(
            ctx.read_response(Id::new(("vst3-parameter", id, 37_u32)))
                .is_some()
        );
        assert!(actions.is_empty(), "read-only display emitted {actions:?}");
    }
    assert_eq!(
        document.project().plugin(id).unwrap().parameters[0].value,
        0.37
    );
    assert_eq!(document.revision(), revision);
    assert!(!document.is_dirty());
}

#[test]
fn dragging_effect_titles_reorders_once_and_preserves_plugin_state_and_history() {
    let mut harness = Harness::with_vst3_devices();
    harness.select_track(1);
    let first = harness.effect_id(1, 0);
    let second = harness.effect_id(1, 1);
    let original = harness.project().tracks()[1].effects().to_vec();
    let handle = harness.rect(Id::new(("device-effect-drag", first)));
    let target = harness.rect(Id::new(("device-vst3", second as usize)));
    harness.drag(
        handle.center(),
        pos2(target.center().x, target.bottom() - 12.0),
        Modifiers::NONE,
    );
    assert_eq!(harness.effect_id(1, 0), second);
    assert_eq!(harness.effect_id(1, 1), first);
    assert_eq!(harness.project().plugin(first), Some(&original[0]));
    assert_eq!(harness.project().plugin(second), Some(&original[1]));
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.project().tracks()[1].effects(), original);
    assert!(!harness.app.document_for_tests().is_dirty());
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(harness.effect_id(1, 0), second);
    assert_eq!(harness.effect_id(1, 1), first);
    harness.steps(2);

    let handle = harness.rect(Id::new(("device-effect-drag", first)));
    let target = harness.rect(Id::new(("device-vst3", second as usize)));
    harness.drag(
        handle.center(),
        pos2(target.left() + 12.0, target.top() + 50.0),
        Modifiers::NONE,
    );
    assert_eq!(harness.project().tracks()[1].effects(), original);
    let revision = harness.app.document_for_tests().revision();
    let handle = harness.rect(Id::new(("device-effect-drag", first)));
    harness.drag(
        handle.center(),
        handle.center() + vec2(10.0, 0.0),
        Modifiers::NONE,
    );
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn vst3_effect_move_and_remove_preserve_identity_and_undo_restores_chain_content() {
    let mut harness = Harness::with_vst3_devices();
    harness.select_track(1);
    let first = harness.effect_id(1, 0);
    let second = harness.effect_id(1, 1);
    let first_saved = harness.project().plugin(first).unwrap().clone();
    harness.dispatch(Action::OpenVst3Editor(first));
    harness.dispatch(Action::OpenVst3Editor(second));
    harness.dispatch(Action::MoveEffect {
        track: 1,
        from: 0,
        to: 1,
    });
    assert_eq!(harness.effect_id(1, 0), second);
    assert_eq!(harness.effect_id(1, 1), first);
    assert_eq!(
        harness.app.vst3_editors_for_tests(),
        &BTreeSet::from([first, second])
    );
    let upper = harness.rect(Id::new(("device-vst3", second as usize)));
    let lower = harness.rect(Id::new(("device-vst3", first as usize)));
    assert!(upper.bottom() <= lower.top());
    harness.dispatch(Action::RemoveVst3 {
        track: 1,
        id: first,
    });
    assert!(harness.project().plugin(first).is_none());
    assert_eq!(
        harness.app.vst3_editors_for_tests(),
        &BTreeSet::from([second])
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().plugin(first), Some(&first_saved));
    assert_eq!(harness.effect_id(1, 1), first);
    harness.dispatch(Action::Undo);
    assert_eq!(harness.effect_id(1, 0), first);
    assert_eq!(harness.effect_id(1, 1), second);
    assert!(!harness.app.document_for_tests().is_dirty());

    let revision = harness.app.document_for_tests().revision();
    harness.dispatch(Action::RemoveVst3 {
        track: 2,
        id: first,
    });
    assert_eq!(harness.project().plugin(first), Some(&first_saved));
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn replacing_a_project_closes_vst3_windows_even_when_saved_ids_match() {
    let mut harness = Harness::with_vst3_devices();
    let id = harness.effect_id(1, 0);
    harness.dispatch(Action::OpenVst3Editor(id));
    harness.dispatch(Action::OpenPath(harness._files.0.join("plugins.symp")));
    assert!(harness.app.vst3_editors_for_tests().is_empty());
    assert!(harness.project().plugin(id).is_some());
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn single_click_selection_preserves_clip_mixer_and_hidden_detail_views() {
    for (mode, action) in [
        ("clip", Action::ShowClipView),
        ("mixer", Action::ShowMixer),
        ("hidden", Action::ToggleDetailView),
    ] {
        let mut harness = Harness::with_two_clips();
        harness.dispatch(Action::SelectClip(Some(ClipRef { track: 1, index: 0 })));
        harness.dispatch(action);
        let revision = harness.app.document_for_tests().revision();
        assert_eq!(harness.app.detail_view_for_tests(), mode);

        harness.select_track(2);
        assert_eq!(harness.app.detail_view_for_tests(), mode);
        harness.click(clip_title(harness.clip_rect(1, 0)));
        assert_eq!(harness.app.detail_view_for_tests(), mode);
        assert_eq!(
            harness.app.selection_for_tests().clip,
            Some(ClipRef { track: 1, index: 0 })
        );
        harness.click(harness.clip_rect(2, 0).center());
        assert_eq!(harness.app.detail_view_for_tests(), mode);
        let lane = harness.rect(Id::new(("lane", 1)));
        harness.click(lane.right_center() - vec2(20.0, 0.0));
        assert_eq!(harness.app.detail_view_for_tests(), mode);
        let below = harness.rect(Id::new("arrangement-below"));
        harness.click(below.center());
        assert_eq!(harness.app.detail_view_for_tests(), mode);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
    }
}

#[test]
fn single_click_selection_keeps_the_open_piano_roll_on_its_original_clip() {
    let mut harness = Harness::with_midi_clip();
    let clip = harness.midi_clip().clone();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    harness.dispatch(Action::InsertMidiClip { track: 2, start: 0 });
    harness.dispatch(Action::OpenPianoRoll(ClipRef { track: 1, index: 0 }));
    let revision = harness.app.document_for_tests().revision();

    harness.select_track(2);
    assert_eq!(harness.app.detail_view_for_tests(), "piano");
    harness.click(clip_title(harness.clip_rect(2, 0)));
    assert_eq!(harness.app.detail_view_for_tests(), "piano");
    assert_eq!(
        harness.app.selection_for_tests().clip,
        Some(ClipRef { track: 2, index: 0 })
    );
    harness.click(harness.clip_rect(2, 0).center());
    assert_eq!(harness.app.detail_view_for_tests(), "piano");
    let lane = harness.rect(Id::new(("lane", 2)));
    harness.click(lane.right_center() - vec2(20.0, 0.0));
    assert_eq!(harness.app.detail_view_for_tests(), "piano");
    harness.assert_visible(Id::new(("piano-note", 2)), true);
    assert_eq!(*harness.midi_clip(), clip);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn double_clicks_switch_between_track_devices_and_audio_clip_view() {
    let mut harness = Harness::with_two_clips();
    let revision = harness.app.document_for_tests().revision();
    for body in [false, true] {
        harness.dispatch(Action::ShowMixer);
        let header = harness.rect(Id::new(("header", 1)));
        harness.double_click(header.left_top() + vec2(30.0, 14.0));
        assert_eq!(harness.app.detail_view_for_tests(), "devices");
        harness.rect(Id::new(("device-chain", 1)));
        assert!(!harness.ctx.egui_wants_keyboard_input());
        assert_eq!(harness.project().tracks()[1].name(), "kick");

        let clip = harness.clip_rect(2, 0);
        harness.double_click(if body {
            clip.center()
        } else {
            clip_title(clip)
        });
        assert_eq!(harness.app.detail_view_for_tests(), "clip");
        assert_eq!(
            harness.app.selection_for_tests().clip,
            Some(ClipRef { track: 2, index: 0 })
        );
        harness.labelled("Pitch");
        harness.open_track_devices(2);
        harness.rect(Id::new(("device-chain", 2)));
    }
    assert_eq!(harness.app.document_for_tests().revision(), revision);
}

#[test]
fn double_clicks_switch_between_track_devices_and_midi_clip_editor() {
    let mut harness = Harness::with_midi_clip();
    let clip = harness.midi_clip().clone();
    for body in [false, true] {
        harness.open_track_devices(1);
        assert_eq!(harness.app.detail_view_for_tests(), "devices");
        let rect = harness.clip_rect(1, 0);
        harness.double_click(if body {
            rect.center()
        } else {
            clip_title(rect)
        });
        assert_eq!(harness.app.detail_view_for_tests(), "piano");
        harness.rect(Id::new("piano-grid"));
    }
    harness.dispatch(Action::ShowDevices);
    assert_eq!(harness.app.detail_view_for_tests(), "devices");
    harness.dispatch(Action::ShowClipView);
    assert_eq!(harness.app.detail_view_for_tests(), "piano");
    harness.dispatch(Action::ClosePianoRoll);
    assert_eq!(harness.app.detail_view_for_tests(), "hidden");
    assert_eq!(*harness.midi_clip(), clip);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn grouping_the_edited_midi_track_closes_the_stale_editor_without_entering_devices() {
    let mut harness = Harness::with_midi_clip();
    let clip = harness.midi_clip().clone();
    harness.dispatch(Action::SelectTracks(BTreeSet::from([1])));
    assert_eq!(harness.app.detail_view_for_tests(), "piano");
    harness.dispatch(Action::GroupSelected);
    assert_eq!(harness.app.detail_view_for_tests(), "clip");
    harness.assert_visible(Id::new("piano-grid"), false);
    let midi = harness
        .project()
        .tracks()
        .iter()
        .find_map(|track| match track {
            Track::Midi(midi) => Some(midi),
            _ => None,
        })
        .expect("grouped MIDI track");
    assert_eq!(midi.clips(), &[clip]);
}

#[test]
fn plugin_windows_restore_per_track_and_manual_close_leaves_the_project_clean() {
    let mut harness = Harness::with_instruments();
    let revision = harness.app.document_for_tests().revision();
    for track in [1, 2] {
        harness.select_track(track);
        let open = harness.rect(Id::new(("open-plugin-editor", track)));
        harness.click(open.center());
        harness.rect(Id::new(("close-plugin-editor", track)));
    }
    assert_eq!(
        harness.app.plugin_editors_for_tests(),
        &BTreeSet::from([1, 2])
    );
    harness.select_track(3);
    harness.dispatch(Action::ShowMixer);
    harness.dispatch(Action::ShowDevices);
    assert_eq!(
        harness.app.plugin_editors_for_tests(),
        &BTreeSet::from([1, 2]),
        "each track remembers its editor across selection and bottom-lane changes"
    );
    for track in [1, 2] {
        harness.assert_visible(Id::new(("close-plugin-editor", track)), false);
    }

    harness.select_track(2);
    let close = harness.rect(Id::new(("close-plugin-editor", 2)));
    harness.click(close.center());
    assert_eq!(harness.app.plugin_editors_for_tests(), &BTreeSet::from([1]));
    harness.select_track(1);
    harness.assert_visible(Id::new(("close-plugin-editor", 1)), true);
    harness.select_track(2);
    harness.assert_visible(Id::new(("close-plugin-editor", 2)), false);
    harness.dispatch(Action::ClosePluginEditor(1));
    assert!(harness.app.plugin_editors_for_tests().is_empty());
    for track in [1, 2] {
        assert_eq!(
            harness.instrument(track),
            Some(&Instrument::Subtractive(SubtractiveSettings::default())),
            "closing a window must leave its instrument assigned"
        );
    }
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn a_restored_plugin_window_edits_only_its_selected_tracks_instrument() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::OpenPluginEditor(1));
    harness.dispatch(Action::OpenPluginEditor(2));
    harness.select_track(3);
    for track in [1, 2] {
        harness.assert_visible(Id::new(("close-plugin-editor", track)), false);
    }
    harness.select_track(2);
    harness.assert_visible(Id::new(("close-plugin-editor", 1)), false);
    let cutoff = harness.rect(Id::new(("plugin-parameter", 2, "cutoff")));
    harness.drag(
        cutoff.left_center() + vec2(20.0, 0.0),
        cutoff.left_center() + vec2(70.0, 0.0),
        Modifiers::NONE,
    );
    assert_eq!(
        harness.instrument(1),
        Some(&Instrument::Subtractive(SubtractiveSettings::default()))
    );
    assert_ne!(
        harness.instrument(2),
        Some(&Instrument::Subtractive(SubtractiveSettings::default()))
    );
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([2])
    );
    assert_eq!(
        harness.app.plugin_editors_for_tests(),
        &BTreeSet::from([1, 2])
    );
    harness.dispatch(Action::Undo);
    assert_eq!(
        harness.instrument(2),
        Some(&Instrument::Subtractive(SubtractiveSettings::default()))
    );
    assert!(!harness.app.document_for_tests().is_dirty());
    assert_eq!(
        harness.app.plugin_editors_for_tests(),
        &BTreeSet::from([1, 2]),
        "parameter undo retains the editors"
    );
}

#[test]
fn builtin_editors_follow_clip_selection_and_hide_for_multiple_or_no_tracks() {
    let mut harness = Harness::with_instruments();
    for track in [1, 2] {
        harness.dispatch(Action::InsertMidiClip { track, start: 0 });
    }
    for track in [1, 2] {
        harness.dispatch(Action::OpenPluginEditor(track));
    }
    for selection in [BTreeSet::from([1, 2]), BTreeSet::new()] {
        harness.dispatch(Action::SelectTracks(selection));
        for track in [1, 2] {
            harness.assert_visible(Id::new(("close-plugin-editor", track)), false);
        }
        assert_eq!(
            harness.app.plugin_editors_for_tests(),
            &BTreeSet::from([1, 2])
        );
    }
    harness.dispatch(Action::SelectClip(Some(ClipRef { track: 1, index: 0 })));
    harness.assert_visible(Id::new(("close-plugin-editor", 1)), true);
    harness.assert_visible(Id::new(("close-plugin-editor", 2)), false);
    harness.dispatch(Action::OpenPianoRoll(ClipRef { track: 2, index: 0 }));
    harness.assert_visible(Id::new(("close-plugin-editor", 1)), false);
    harness.assert_visible(Id::new(("close-plugin-editor", 2)), true);
    harness.dispatch(Action::ShowMixer);
    harness.assert_visible(Id::new(("close-plugin-editor", 2)), true);
}

#[test]
fn removing_an_instrument_closes_only_its_editor_and_track_deletion_clears_stale_editors() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::OpenPluginEditor(1));
    harness.dispatch(Action::OpenPluginEditor(2));
    harness.dispatch(Action::SetInstrument {
        track: 1,
        instrument: None,
    });
    assert_eq!(harness.app.plugin_editors_for_tests(), &BTreeSet::from([2]));
    assert!(harness.instrument(1).is_none());
    harness.dispatch(Action::Undo);
    assert!(harness.instrument(1).is_some());
    assert_eq!(harness.app.plugin_editors_for_tests(), &BTreeSet::from([2]));

    harness.dispatch(Action::OpenPluginEditor(1));
    harness.dispatch(Action::DeleteTrack(1));
    assert!(harness.app.plugin_editors_for_tests().is_empty());
    assert_eq!(harness.project().tracks()[1].name(), "Bass");
    assert!(harness.instrument(1).is_some());
    harness.dispatch(Action::OpenPluginEditor(usize::MAX));
    harness.dispatch(Action::OpenPluginEditor(0));
    harness.dispatch(Action::OpenPluginEditor(2));
    assert!(
        harness.app.plugin_editors_for_tests().is_empty(),
        "missing, master, and audio tracks cannot open instrument editors"
    );
}

#[test]
fn replacing_the_document_and_structural_history_close_plugin_windows() {
    let mut harness = Harness::with_instruments();
    let saved = harness._files.0.join("instruments.symp");
    harness.dispatch(Action::OpenPluginEditor(1));
    harness.dispatch(Action::OpenPluginEditor(2));
    harness.dispatch(Action::OpenPath(saved.clone()));
    assert!(harness.app.plugin_editors_for_tests().is_empty());

    harness.dispatch(Action::OpenPluginEditor(1));
    harness.dispatch(Action::NewProject);
    assert!(harness.app.plugin_editors_for_tests().is_empty());
    assert_eq!(harness.project().tracks().len(), 1);
    harness.dispatch(Action::OpenPath(saved));
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    harness.dispatch(Action::OpenPluginEditor(1));
    harness.dispatch(Action::Undo);
    assert!(harness.app.plugin_editors_for_tests().is_empty());
    harness.dispatch(Action::OpenPluginEditor(2));
    harness.dispatch(Action::Redo);
    assert!(harness.app.plugin_editors_for_tests().is_empty());
}

#[test]
fn piano_mouse_previews_sustain_until_release_for_both_keys_and_notes() {
    for keyboard in [true, false] {
        let mut harness = Harness::with_midi_clip();
        harness.dispatch(Action::SetInstrument {
            track: 1,
            instrument: Some(Instrument::Subtractive(SubtractiveSettings {
                sustain: 1.0,
                release_seconds: 0.05,
                ..SubtractiveSettings::default()
            })),
        });
        let notes = harness.midi_clip().notes.clone();
        let shared = harness.app.capture_audio_for_tests();
        let target = if keyboard {
            harness.rect(Id::new(("piano-key", 60u8)))
        } else {
            harness.note_rect(0)
        };
        harness.move_to(target.center());
        harness.button(true, Modifiers::NONE);
        harness.steps(3);

        let held = read_preview_frames(&shared, 36_000);
        assert!(
            held[35_000..].iter().any(|frame| frame[0].abs() > 0.01),
            "a held mouse preview must keep sounding beyond its note duration"
        );
        assert_eq!(harness.midi_clip().notes, notes);
        assert_eq!(harness.app.status_for_tests().state, PlaybackState::Stopped);

        if keyboard {
            harness.move_to(pos2(10.0, 10.0));
        }
        harness.button(false, Modifiers::NONE);
        assert_preview_finishes(&shared);
        assert_eq!(harness.midi_clip().notes, notes);
    }
}

#[test]
fn piano_mouse_previews_release_when_closed_cancelled_or_unfocused() {
    for cancellation in ["close", "devices", "escape", "window focus"] {
        let mut harness = Harness::with_midi_clip();
        harness.dispatch(Action::SetInstrument {
            track: 1,
            instrument: Some(Instrument::Subtractive(SubtractiveSettings {
                sustain: 1.0,
                release_seconds: 0.05,
                ..SubtractiveSettings::default()
            })),
        });
        let notes = harness.midi_clip().notes.clone();
        let shared = harness.app.capture_audio_for_tests();
        harness.move_to(harness.note_rect(0).center());
        harness.button(true, Modifiers::NONE);
        assert!(
            read_preview_frames(&shared, 2_000)
                .iter()
                .any(|frame| frame[0].abs() > 0.01)
        );

        match cancellation {
            "close" => harness.dispatch(Action::ClosePianoRoll),
            "devices" => harness.dispatch(Action::ShowDevices),
            "escape" => harness.key(Key::Escape, Modifiers::NONE),
            "window focus" => harness.step_input_with_focus(
                RawInput {
                    events: vec![Event::WindowFocused(false)],
                    ..RawInput::default()
                },
                false,
            ),
            _ => unreachable!(),
        }
        assert_preview_finishes(&shared);
        assert_eq!(harness.midi_clip().notes, notes, "{cancellation}");
        harness.button(false, Modifiers::NONE);
    }
}

#[test]
fn piano_pitch_drag_commit_preserves_the_preview_release_envelope() {
    let mut harness = Harness::with_midi_clip();
    harness.dispatch(Action::SetInstrument {
        track: 1,
        instrument: Some(Instrument::Subtractive(SubtractiveSettings {
            waveform: Waveform::Sine,
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            release_seconds: 0.4,
            ..SubtractiveSettings::default()
        })),
    });
    let shared = harness.app.capture_audio_for_tests();
    let start = harness.note_rect(0).center();
    let (_, key_height) = harness.piano_scale();
    harness.move_to(start);
    harness.button(true, Modifiers::NONE);
    read_preview_frames(&shared, 2_000);
    harness.move_to(start - vec2(0.0, key_height));

    let held = read_preview_frames(&shared, 24_000);
    let held_peak = held[23_000..]
        .iter()
        .map(|frame| frame[0].abs())
        .fold(0.0_f32, f32::max);
    assert!(held_peak > 0.01);
    harness.button(false, Modifiers::NONE);
    assert_eq!(harness.midi_clip().notes[0].key, 61);

    let release = read_preview_frames(&shared, 12_000);
    let late_peak = release[10_000..]
        .iter()
        .map(|frame| frame[0].abs())
        .fold(0.0_f32, f32::max);
    assert!(late_peak > held_peak * 0.2, "the edit truncated release");
    assert!(
        late_peak < held_peak * 0.8,
        "mouse-up must release the preview rather than retrigger or sustain it"
    );
    assert_preview_finishes(&shared);
    assert_eq!(harness.app.status_for_tests().state, PlaybackState::Stopped);
}

fn read_preview_frames(shared: &crate::audio::engine::Shared, count: usize) -> Vec<Frame> {
    let mut frames = vec![[0.0; 2]; count];
    let mut filled = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while filled < count && std::time::Instant::now() < deadline {
        filled += shared.consume(&mut frames[filled..]);
        if filled < count {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(filled, count, "the expected preview audio ended early");
    frames
}

fn assert_preview_finishes(shared: &crate::audio::engine::Shared) {
    let mut output = [[0.0; 2]; 512];
    let mut rendered = 0;
    let mut quiet_since = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let count = shared.consume(&mut output);
        rendered += count;
        assert!(rendered < 12_000, "the cancelled preview kept sounding");
        if count == 0 {
            let quiet = quiet_since.get_or_insert_with(std::time::Instant::now);
            if quiet.elapsed() >= Duration::from_millis(50) {
                return;
            }
        } else {
            quiet_since = None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the mouse preview never finished its release");
}

#[test]
fn piano_pitch_shortcuts_keep_the_audition_after_the_project_rebuild() {
    for (key, modifiers, expected_key) in [
        (Key::ArrowUp, Modifiers::NONE, 61),
        (Key::ArrowDown, Modifiers::SHIFT, 48),
    ] {
        let mut harness = Harness::with_midi_clip();
        harness.dispatch(Action::SetInstrument {
            track: 1,
            instrument: Some(Instrument::Subtractive(SubtractiveSettings {
                release_seconds: 0.0,
                ..SubtractiveSettings::default()
            })),
        });
        harness.click(harness.note_rect(0).center());
        let shared = harness.app.capture_audio_for_tests();
        harness.key(key, modifiers);
        assert_eq!(harness.midi_clip().notes[0].key, expected_key);

        let mut output = vec![[0.0; 2]; 11_000];
        let mut filled = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while filled < output.len() && std::time::Instant::now() < deadline {
            filled += shared.consume(&mut output[filled..]);
            if filled < output.len() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(filled, output.len(), "the pitch preview ended early");
        assert!(
            output[10_000..].iter().any(|frame| frame[0].abs() > 0.01),
            "editing the note must not truncate its subsequent preview"
        );
        harness.steps(1);
        assert_eq!(harness.app.status_for_tests().state, PlaybackState::Stopped);
        assert_eq!(harness.app.status_for_tests().position_frame, 0);
    }
}

#[test]
fn piano_octave_shortcuts_preserve_chords_and_support_undo() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    harness.click(harness.note_rect(0).center());
    harness.click_with(harness.note_rect(1).center(), Modifiers::SHIFT);

    harness.key(Key::ArrowUp, Modifiers::SHIFT);
    let mut higher = before.clone();
    for note in &mut higher[..2] {
        note.key += 12;
    }
    assert_eq!(harness.midi_clip().notes, higher);
    harness.key(Key::ArrowDown, Modifiers::SHIFT);
    assert_eq!(harness.midi_clip().notes, before);
    harness.key(Key::ArrowDown, Modifiers::NONE);
    let mut lower = before.clone();
    for note in &mut lower[..2] {
        note.key -= 1;
    }
    assert_eq!(harness.midi_clip().notes, lower);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes, before);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes, higher);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes, before);
}

#[test]
fn piano_note_drag_is_one_undoable_edit_and_preserves_performance_data() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    let rect = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    harness.drag(
        rect.center(),
        rect.center() + vec2(240.0 * pixels_per_tick, -2.0 * key_height),
        Modifiers::NONE,
    );
    let mut moved = before[0];
    moved.start_tick = 240;
    moved.key = 62;
    assert_eq!(harness.midi_clip().notes[0], moved);
    assert_eq!(&harness.midi_clip().notes[1..], &before[1..]);

    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes, before);
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(harness.midi_clip().notes[0], moved);
}

#[test]
fn resizing_a_piano_note_retains_pitch_velocity_and_channel() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes[0];
    let rect = harness.note_rect(0);
    let (pixels_per_tick, _) = harness.piano_scale();
    let from = pos2(rect.right() - 2.0, rect.center().y);
    harness.drag(
        from,
        from + vec2(240.0 * pixels_per_tick, 0.0),
        Modifiers::NONE,
    );
    assert_eq!(
        harness.midi_clip().notes[0],
        MidiNote {
            duration_ticks: 720,
            ..before
        }
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes[0], before);
}

#[test]
fn piano_multi_selection_deletes_notes_without_deleting_the_clip() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    harness.click(harness.note_rect(0).center());
    harness.click_with(harness.note_rect(1).center(), Modifiers::SHIFT);
    harness.key(Key::Delete, Modifiers::NONE);
    assert_eq!(harness.midi_clip().notes, [before[2]]);
    assert_eq!(harness.midi_clip().start_tick, 3840);

    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes, before);
}

#[test]
fn a_piano_marquee_selects_only_the_notes_it_intersects() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    let first = harness.note_rect(0);
    let second = harness.note_rect(1);
    harness.drag(
        pos2(first.left() + 1.0, first.bottom() + 4.0),
        second.right_top() + vec2(4.0, -4.0),
        Modifiers::NONE,
    );
    harness.key(Key::Delete, Modifiers::NONE);
    assert_eq!(harness.midi_clip().notes, [before[2]]);
}

#[test]
fn dragging_piano_velocity_is_one_undoable_edit() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes[0];
    let note = harness.note_rect(0);
    let lane = harness.rect(Id::new("piano-velocity-lane"));
    let from = pos2(note.left() + 2.0, lane.center().y);
    harness.drag(from, from - vec2(0.0, 10.0), Modifiers::NONE);
    assert_eq!(
        harness.midi_clip().notes[0],
        MidiNote {
            velocity: before.velocity + (10.0 * 127.0 / (lane.height() - 12.0)).round() as u8,
            ..before
        }
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes[0], before);
}

#[test]
fn dragging_selected_piano_notes_keeps_their_time_and_pitch_intervals() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    let first = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    harness.click(first.center());
    harness.click_with(harness.note_rect(1).center(), Modifiers::SHIFT);
    harness.drag(
        first.center(),
        first.center() + vec2(480.0 * pixels_per_tick, -2.0 * key_height),
        Modifiers::NONE,
    );

    let mut expected = before.clone();
    for note in &mut expected[..2] {
        note.start_tick += 480;
        note.key += 2;
    }
    assert_eq!(harness.midi_clip().notes, expected);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes, before);
}

#[test]
fn delete_with_no_piano_notes_selected_does_not_delete_the_clip() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().clone();
    let first = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    harness.click(pos2(
        first.left() + 2900.0 * pixels_per_tick,
        first.center().y - 2.0 * key_height,
    ));
    harness.key(Key::Delete, Modifiers::NONE);
    assert_eq!(*harness.midi_clip(), before);
}

#[test]
fn clicking_the_arrangement_returns_delete_to_clip_editing() {
    let mut harness = Harness::with_midi_clip();
    harness.click(harness.note_rect(0).center());
    harness.click(clip_title(harness.clip_rect(1, 0)));
    harness.key(Key::Delete, Modifiers::NONE);
    let Track::Midi(track) = &harness.project().tracks()[1] else {
        panic!("MIDI track was deleted instead of its clip")
    };
    assert!(track.clips().is_empty());

    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes.len(), 3);
}

#[test]
fn undoing_clip_creation_cancels_an_uncommitted_note_drag() {
    let mut harness = Harness::with_midi_clip();
    let original = harness.midi_clip().clone();
    harness.click(clip_title(harness.clip_rect(1, 0)));
    harness.key(Key::D, Modifiers::COMMAND);
    harness.steps(2);
    harness.double_click(clip_title(harness.clip_rect(1, 1)));
    let first = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    harness.move_to(first.center());
    harness.button(true, Modifiers::NONE);
    harness.move_to(first.center() + vec2(240.0 * pixels_per_tick, -key_height));

    harness.key(Key::Z, Modifiers::COMMAND);
    harness.button(false, Modifiers::NONE);
    harness.steps(2);
    assert_eq!(*harness.midi_clip(), original);
}

#[test]
fn piano_clipboard_preserves_velocity_and_channel_and_edits_are_saved() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    let first = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    harness.click(first.center());
    harness.click_with(harness.note_rect(1).center(), Modifiers::SHIFT);
    harness.key(Key::C, Modifiers::COMMAND);
    harness.click(pos2(
        first.left() + 2900.0 * pixels_per_tick,
        first.center().y - 2.0 * key_height,
    ));
    harness.key(Key::V, Modifiers::COMMAND);
    let copied = MidiNote {
        start_tick: 2880,
        ..before[0]
    };
    assert_eq!(harness.midi_clip().notes.len(), 5);
    assert_eq!(harness.midi_clip().notes[3], copied);
    assert_eq!(
        harness.midi_clip().notes[4],
        MidiNote {
            start_tick: 3840,
            ..before[1]
        }
    );

    harness.key(Key::S, Modifiers::COMMAND);
    let loaded = Project::load(harness._files.0.join("notes.symp")).unwrap();
    let Track::Midi(track) = &loaded.tracks()[1] else {
        panic!("saved MIDI track was lost")
    };
    assert_eq!(track.clips()[0], *harness.midi_clip());
    harness.key(Key::Delete, Modifiers::NONE);
    assert_eq!(
        harness.midi_clip().notes,
        before,
        "pasted notes remain selected for the next edit"
    );
}

#[test]
fn clip_clipboard_keys_paste_midi_on_the_selected_track_and_undo_once() {
    for modifiers in [Modifiers::COMMAND, Modifiers::CTRL] {
        let mut harness = Harness::with_midi_clip();
        let original = harness.midi_clip().clone();
        harness.click(clip_title(harness.clip_rect(1, 0)));
        let revision = harness.app.document_for_tests().revision();
        harness.key(Key::C, modifiers);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(!harness.clipboard.is_empty());

        harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
        harness.dispatch(Action::SetTimeSelection(Some(TimeRange {
            start: 1920,
            end: 3840,
            tracks: BTreeSet::from([2]),
        })));
        harness.key(Key::V, modifiers);
        let expected = MidiClip {
            start_tick: 1920,
            ..original.clone()
        };
        assert_eq!(harness.midi_clips(1), std::slice::from_ref(&original));
        assert_eq!(harness.midi_clips(2), std::slice::from_ref(&expected));
        assert_eq!(
            harness.app.selection_for_tests().clip,
            Some(ClipRef { track: 2, index: 0 })
        );
        assert_eq!(
            harness.app.document_for_tests().undo_label(),
            Some("Paste Clip")
        );
        harness.dispatch(Action::Undo);
        assert!(harness.midi_clips(2).is_empty());
        assert_eq!(harness.midi_clips(1), std::slice::from_ref(&original));
        harness.dispatch(Action::Redo);
        assert_eq!(harness.midi_clips(2), [expected]);
    }
}

#[test]
fn native_clip_clipboard_preserves_deleted_audio_and_ignores_unrelated_text() {
    use crate::document::edit::audio_clip;

    let mut harness = Harness::with_two_clips();
    let source = ClipRef { track: 1, index: 0 };
    let original = *audio_clip(harness.project(), source).unwrap();
    harness.click(clip_title(harness.clip_rect(1, 0)));
    harness.step(vec![Event::Copy]);
    let text = harness.clipboard.clone();
    assert!(!text.is_empty());
    harness.dispatch(Action::SetClipGain {
        clip: source,
        gain: 0.2,
    });
    harness.dispatch(Action::DeleteClip(source));
    harness.select_track(2);
    harness.dispatch(Action::Seek(2880));
    let revision = harness.app.document_for_tests().revision();
    harness.step(vec![Event::Paste("ordinary text".into())]);
    assert_eq!(harness.app.document_for_tests().revision(), revision);

    harness.step(vec![Event::Paste(text.clone())]);
    let copy = ClipRef { track: 2, index: 1 };
    assert_eq!(
        *audio_clip(harness.project(), copy).unwrap(),
        sympoie::clip::AudioClip {
            start_tick: 2880,
            ..original
        }
    );
    let next = harness.app.arrangement_for_tests().playback_start;
    assert!(next > 2880);
    harness.step(vec![Event::Paste(text)]);
    assert_eq!(
        audio_clip(harness.project(), ClipRef { track: 2, index: 2 })
            .unwrap()
            .start_tick,
        next
    );
    harness.dispatch(Action::Undo);
    assert!(audio_clip(harness.project(), ClipRef { track: 2, index: 2 }).is_none());
    assert!(audio_clip(harness.project(), copy).is_some());
}

#[test]
fn clip_paste_ignores_incompatible_tracks_and_text_field_input() {
    let mut harness = Harness::with_two_clips();
    harness.click(clip_title(harness.clip_rect(1, 0)));
    harness.step(vec![Event::Copy]);
    let text = harness.clipboard.clone();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    harness.dispatch(Action::SetTimeSelection(Some(TimeRange {
        start: 1920,
        end: 3840,
        tracks: BTreeSet::from([1]),
    })));
    harness.dispatch(Action::SelectTracks(BTreeSet::from([3])));
    let revision = harness.app.document_for_tests().revision();
    harness.step(vec![Event::Paste(text.clone())]);
    assert!(harness.midi_clips(3).is_empty());
    assert_eq!(harness.app.document_for_tests().revision(), revision);

    harness.select_track(1);
    harness.begin_track_rename(1);
    assert!(harness.ctx.egui_wants_keyboard_input());
    harness.key(Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Paste(text.clone())]);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    harness.key(Key::Enter, Modifiers::NONE);
    assert_eq!(harness.project().tracks()[1].name(), text);
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!()
    };
    assert_eq!(track.clips().len(), 1);
}

#[test]
fn command_drag_copies_notes_and_clips_in_single_undo_steps() {
    let mut harness = Harness::with_midi_clip();
    let original = harness.midi_clip().clone();
    let first = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    harness.click(first.center());
    harness.click_with(harness.note_rect(1).center(), Modifiers::SHIFT);
    harness.drag(
        first.center(),
        first.center() + vec2(480.0 * pixels_per_tick, -2.0 * key_height),
        Modifiers::COMMAND,
    );
    assert_eq!(harness.midi_clip().notes.len(), 5);
    assert_eq!(&harness.midi_clip().notes[..3], &original.notes);
    for (copy, source) in harness.midi_clip().notes[3..].iter().zip(&original.notes) {
        assert_eq!(
            *copy,
            MidiNote {
                start_tick: source.start_tick + 480,
                key: source.key + 2,
                ..*source
            }
        );
    }
    harness.dispatch(Action::Undo);
    assert_eq!(*harness.midi_clip(), original);
    harness.dispatch(Action::Redo);
    assert_eq!(harness.midi_clip().notes.len(), 5);
    harness.dispatch(Action::Undo);
    harness.dispatch(Action::ShowDevices);

    let from = clip_title(harness.clip_rect(1, 0));
    let offset = 1920.0 * harness.app.arrangement_for_tests().pixels_per_tick as f32;
    harness.drag(from, from + vec2(offset, 0.0), Modifiers::COMMAND);
    assert_eq!(harness.midi_clips(1).len(), 2);
    assert_eq!(harness.midi_clips(1)[0], original);
    assert_eq!(harness.midi_clips(1)[1].notes, original.notes);
    assert_eq!(
        harness.app.document_for_tests().undo_label(),
        Some("Copy Clip")
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.midi_clips(1), std::slice::from_ref(&original));
    harness.dispatch(Action::Redo);
    assert_eq!(harness.midi_clips(1).len(), 2);
}

#[test]
fn native_clipboard_events_edit_the_selected_piano_notes() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    let first = harness.note_rect(0);
    let (pixels_per_tick, key_height) = harness.piano_scale();
    harness.click(first.center());
    harness.step(vec![Event::Copy]);
    assert!(
        !harness.clipboard.is_empty(),
        "native paste needs system clipboard content"
    );
    let copied_text = harness.clipboard.clone();
    harness.click(pos2(
        first.left() + 2900.0 * pixels_per_tick,
        first.center().y - 2.0 * key_height,
    ));
    harness.step(vec![Event::Paste("Unrelated clipboard text".to_owned())]);
    assert_eq!(harness.midi_clip().notes, before);
    harness.step(vec![Event::Paste(copied_text)]);
    harness.steps(1);
    assert_eq!(harness.midi_clip().notes.len(), 4);
    assert_eq!(
        harness.midi_clip().notes[3],
        MidiNote {
            start_tick: 2880,
            ..before[0]
        }
    );
    harness.step(vec![Event::Cut]);
    assert_eq!(harness.midi_clip().notes, before);
}

#[test]
fn piano_velocity_lane_resizes_without_editing_notes_and_keeps_both_lanes_visible() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().clone();
    let lane_id = Id::new("piano-velocity-lane");
    let divider_id = Id::new("piano-velocity-divider");
    let grid_id = Id::new("piano-grid");
    let lane = harness.rect(lane_id);
    let grid = harness.rect(grid_id);
    let divider = harness.rect(divider_id);
    harness.drag(
        divider.center(),
        divider.center() - vec2(0.0, 64.0),
        Modifiers::NONE,
    );
    let expanded = harness.rect(lane_id);
    assert!((expanded.height() - lane.height() - 64.0).abs() < 1.0);
    assert_eq!(expanded.left(), grid.left());
    assert_eq!(expanded.bottom(), lane.bottom());
    assert!((grid.height() - harness.rect(grid_id).height() - 64.0).abs() < 1.0);

    let divider = harness.rect(divider_id);
    harness.drag(
        divider.center(),
        divider.center() - vec2(0.0, 1000.0),
        Modifiers::NONE,
    );
    assert!(harness.rect(grid_id).height() >= 48.0);
    let divider = harness.rect(divider_id);
    harness.drag(
        divider.center(),
        divider.center() + vec2(0.0, 1000.0),
        Modifiers::NONE,
    );
    assert!((harness.rect(lane_id).height() - 48.0).abs() < 1.0);
    assert_eq!(*harness.midi_clip(), before);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn piano_velocity_drag_after_lane_resize_clamps_values_and_remains_undoable() {
    let mut harness = Harness::with_midi_clip();
    let before = harness.midi_clip().notes.clone();
    let note_x = harness.note_rect(0).left() + 2.0;
    let divider = harness.rect(Id::new("piano-velocity-divider"));
    harness.drag(
        divider.center(),
        divider.center() - vec2(0.0, 64.0),
        Modifiers::NONE,
    );
    let lane = harness.rect(Id::new("piano-velocity-lane"));
    let from = pos2(note_x, lane.center().y);
    harness.drag(from, from - vec2(0.0, lane.height()), Modifiers::NONE);
    assert_eq!(harness.midi_clip().notes[0].velocity, 127);
    assert_eq!(&harness.midi_clip().notes[1..], &before[1..]);
    harness.drag(from, from + vec2(0.0, lane.height()), Modifiers::NONE);
    assert_eq!(harness.midi_clip().notes[0].velocity, 1);
    assert_eq!(
        harness.midi_clip().notes[0],
        MidiNote {
            velocity: 1,
            ..before[0]
        }
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes[0].velocity, 127);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clip().notes, before);
}

#[test]
fn selecting_inside_an_audio_clip_can_loop_and_split_only_that_region() {
    let mut harness = Harness::with_two_clips();
    harness.fixed_grid(sympoie::time::SnapGrid::Eighth);
    let clip = harness.clip_rect(1, 0);
    harness.click(clip_title(clip));
    let revision = harness.app.document_for_tests().revision();
    harness.drag(
        pos2(harness.lane_x(480), clip.center().y),
        pos2(harness.lane_x(1440), clip.center().y),
        Modifiers::NONE,
    );
    assert_eq!(harness.app.selection_for_tests().clip, None);
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 480,
            end: 1440,
            tracks: BTreeSet::from([1])
        }),
    );
    assert_eq!(harness.app.arrangement_for_tests().playback_start, 480);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    harness.key(Key::L, Modifiers::CTRL);
    assert_eq!(
        harness.app.loop_for_tests(),
        crate::state::LoopRegion {
            start: 480,
            end: 1440,
            enabled: true
        },
    );
    harness.key(Key::E, Modifiers::COMMAND);
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!()
    };
    let spans: Vec<_> = track
        .clips()
        .iter()
        .map(|clip| (clip.start_tick, clip.source_start, clip.frame_count))
        .collect();
    assert_eq!(
        spans,
        [
            (0, 0, 12_000),
            (480, 12_000, 24_000),
            (1440, 36_000, 12_000)
        ]
    );
    harness.key(Key::Z, Modifiers::COMMAND);
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!()
    };
    assert_eq!(track.clips().len(), 1);
    assert_eq!(track.clips()[0].frame_count, 48_000);
}

#[test]
fn selecting_inside_a_midi_clip_deletes_only_the_time_selection() {
    let mut harness = Harness::with_midi_clip();
    harness.fixed_grid(sympoie::time::SnapGrid::Quarter);
    let original = harness.midi_clip().clone();
    let clip = harness.clip_rect(1, 0);
    harness.drag(
        pos2(harness.lane_x(4800), clip.center().y),
        pos2(harness.lane_x(5760), clip.center().y),
        Modifiers::NONE,
    );
    assert_eq!(harness.app.selection_for_tests().clip, None);
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 4800,
            end: 5760,
            tracks: BTreeSet::from([1])
        }),
    );
    harness.key(Key::Delete, Modifiers::NONE);
    let mut spans: Vec<_> = harness
        .midi_clips(1)
        .iter()
        .map(|clip| (clip.start_tick, clip.length_ticks, clip.offset_ticks))
        .collect();
    spans.sort_unstable();
    assert_eq!(spans, [(3840, 960, 0), (5760, 5760, 1920)]);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clips(1), [original]);
}

#[test]
fn clip_body_time_selection_crosses_tracks_and_shift_click_extends_it() {
    let mut harness = Harness::with_two_clips();
    harness.fixed_grid(sympoie::time::SnapGrid::Eighth);
    let first = harness.clip_rect(1, 0);
    let second = harness.clip_rect(2, 0);
    let revision = harness.app.document_for_tests().revision();
    harness.drag(
        pos2(harness.lane_x(960), second.center().y),
        pos2(harness.lane_x(480), first.center().y),
        Modifiers::NONE,
    );
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 480,
            end: 960,
            tracks: BTreeSet::from([1, 2])
        }),
    );
    harness.click_with(
        pos2(harness.lane_x(1440), second.center().y),
        Modifiers::SHIFT,
    );
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 480,
            end: 1440,
            tracks: BTreeSet::from([1, 2])
        }),
    );
    assert_eq!(harness.app.selection_for_tests().clip, None);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert_eq!((harness.clip_start(1, 0), harness.clip_start(2, 0)), (0, 0));
}

#[test]
fn dragging_empty_lanes_selects_time_and_ctrl_m_fills_it_with_a_midi_clip() {
    use sympoie::time::SnapGrid;
    let mut harness = Harness::with_instruments();
    harness.fixed_grid(SnapGrid::Quarter);
    let lane = harness.rect(Id::new(("lane", 1)));
    let y = lane.center().y;
    let from = pos2(harness.lane_x(960) + 1.0, y);
    let to = pos2(harness.lane_x(960 * 7) + 1.0, y);
    harness.drag(from, to, Modifiers::NONE);
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 960,
            end: 6720,
            tracks: BTreeSet::from([1]),
        })
    );
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([1]),
        "the selection's track is selected"
    );

    harness.key(Key::M, Modifiers::CTRL);
    let clips = harness.midi_clips(1);
    assert_eq!(clips.len(), 1);
    assert_eq!(
        (
            clips[0].start_tick,
            clips[0].length_ticks,
            clips[0].offset_ticks
        ),
        (960, 5760, 0)
    );
    assert!(clips[0].notes.is_empty());
    assert_eq!(
        harness.app.selection_for_tests().clip,
        Some(ClipRef { track: 1, index: 0 })
    );

    harness.key(Key::Z, Modifiers::COMMAND);
    assert!(harness.midi_clips(1).is_empty());
}

#[test]
fn insert_midi_clip_needs_an_empty_selection_on_one_midi_track() {
    use sympoie::time::SnapGrid;
    let mut harness = Harness::with_instruments();
    harness.fixed_grid(SnapGrid::Quarter);
    let first = harness.rect(Id::new(("lane", 1)));
    let second = harness.rect(Id::new(("lane", 2)));
    harness.drag(
        pos2(harness.lane_x(0) + 2.0, first.center().y),
        pos2(harness.lane_x(3840) + 1.0, second.center().y),
        Modifiers::NONE,
    );
    let range = harness.app.selection_for_tests().range.clone().unwrap();
    assert_eq!(range.tracks, BTreeSet::from([1, 2]));
    harness.key(Key::M, Modifiers::COMMAND);
    assert!(harness.midi_clips(1).is_empty() && harness.midi_clips(2).is_empty());
    assert!(
        harness
            .app
            .notice_for_tests()
            .is_some_and(|text| text.contains("single MIDI track"))
    );

    harness.dispatch(Action::InsertMidiClip { track: 1, start: 0 });
    harness.dispatch(Action::SetTimeSelection(Some(TimeRange {
        start: 1920,
        end: 7680,
        tracks: BTreeSet::from([1]),
    })));
    harness.key(Key::M, Modifiers::COMMAND);
    assert_eq!(harness.midi_clips(1).len(), 1);
}

#[test]
fn shift_click_extends_a_time_selection_across_tracks() {
    use sympoie::time::SnapGrid;
    let mut harness = Harness::with_instruments();
    harness.fixed_grid(SnapGrid::Quarter);
    let first = harness.rect(Id::new(("lane", 1)));
    let audio = harness.rect(Id::new(("lane", 3)));
    harness.drag(
        pos2(harness.lane_x(960) + 1.0, first.center().y),
        pos2(harness.lane_x(1920) + 1.0, first.center().y),
        Modifiers::NONE,
    );
    harness.click_with(
        pos2(harness.lane_x(4800) + 1.0, audio.center().y),
        Modifiers::SHIFT,
    );
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 960,
            end: 4800,
            tracks: BTreeSet::from([1, 2, 3]),
        })
    );
    harness.key(Key::Escape, Modifiers::NONE);
    assert_eq!(harness.app.selection_for_tests().range, None);
}

#[test]
fn moving_a_time_selection_cuts_a_midi_clip_and_both_parts_keep_every_note() {
    use sympoie::time::SnapGrid;
    let mut harness = Harness::with_midi_clip();
    harness.fixed_grid(SnapGrid::Quarter);
    let original = harness.midi_clip().clone();
    let lane = harness.rect(Id::new(("lane", 1)));
    let y = lane.center().y;
    harness.drag(
        pos2(harness.lane_x(1920) + 1.0, y),
        pos2(harness.lane_x(4800) + 1.0, y),
        Modifiers::NONE,
    );
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 1920,
            end: 4800,
            tracks: BTreeSet::from([1]),
        })
    );
    let y = clip_title(harness.clip_rect(1, 0)).y;
    let grab = pos2(harness.lane_x(4320), y);
    harness.drag(grab, pos2(harness.lane_x(4320 + 3840), y), Modifiers::NONE);

    let mut clips = harness.midi_clips(1).to_vec();
    clips.sort_by_key(|clip| clip.start_tick);
    let spans: Vec<_> = clips
        .iter()
        .map(|clip| (clip.start_tick, clip.length_ticks, clip.offset_ticks))
        .collect();
    assert_eq!(
        spans,
        [(4800, 960, 960), (7680, 960, 0), (8640, 2880, 4800)]
    );
    for clip in &clips {
        assert_eq!(clip.notes, original.notes, "every part keeps all notes");
    }
    let moved: Vec<_> = clips[1].audible_notes().collect();
    assert_eq!(moved, [original.notes[0]]);
    assert_eq!(
        harness.app.selection_for_tests().range,
        Some(TimeRange {
            start: 5760,
            end: 8640,
            tracks: BTreeSet::from([1]),
        }),
        "the selection follows the material"
    );

    let left = harness
        .midi_clips(1)
        .iter()
        .position(|clip| clip.start_tick == 4800)
        .unwrap();
    harness.dispatch(Action::TrimClipStart {
        clip: ClipRef {
            track: 1,
            index: left,
        },
        start: 3840,
    });
    let revealed: Vec<_> = harness
        .midi_clips(1)
        .iter()
        .find(|clip| clip.start_tick == 3840)
        .unwrap()
        .audible_notes()
        .collect();
    assert_eq!(revealed, original.notes[..2]);

    harness.key(Key::Z, Modifiers::COMMAND);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.midi_clips(1), [original]);
}

#[test]
fn track_headers_change_colour_from_their_context_menu() {
    let mut harness = Harness::with_instruments();
    let header = harness.rect(Id::new(("header", 2)));
    harness.right_click(header.left_bottom() + vec2(30.0, -6.0));
    let item = harness.labelled("Change Color");
    harness.move_to(item.center());
    harness.steps(2);
    let red_slot = 2 * crate::ui::theme::PALETTE_COLUMNS;
    let red = crate::ui::theme::PALETTE[red_slot];
    let swatch = harness.rect(Id::new(("track-color", red_slot)));
    harness.move_to(pos2(swatch.center().x, item.center().y));
    harness.click(swatch.center());
    assert_eq!(harness.project().tracks()[2].color(), Some(red));
    assert_eq!(harness.project().tracks()[1].color(), None);

    harness.select_track(1);
    let other = harness.rect(Id::new(("header", 3)));
    harness.click_with(other.left_bottom() + vec2(30.0, -6.0), Modifiers::COMMAND);
    harness.dispatch(Action::SetTrackColor {
        tracks: harness.app.selection_for_tests().tracks.clone(),
        color: Some(0x1a2f96),
    });
    let colors: Vec<_> = harness
        .project()
        .tracks()
        .iter()
        .map(Track::color)
        .collect();
    assert_eq!(colors, [None, Some(0x1a2f96), Some(red), Some(0x1a2f96)]);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(harness.project().tracks()[1].color(), None);
}

#[test]
fn grid_follows_zoom_and_steps_with_command_1_and_2() {
    let mut harness = Harness::with_instruments();
    let grid = |harness: &Harness| {
        harness
            .app
            .arrangement_for_tests()
            .grid(harness.project())
            .interval()
    };
    assert_eq!(
        harness.app.arrangement_for_tests().grid,
        GridMode::Adaptive(GridDensity::Medium)
    );
    let before = grid(&harness).unwrap();
    harness.dispatch(Action::Zoom(4.0));
    let zoomed = grid(&harness).unwrap();
    assert!(
        zoomed < before,
        "zooming in refines the grid ({zoomed} vs {before})"
    );

    harness.key(Key::Num1, Modifiers::COMMAND);
    assert_eq!(
        harness.app.arrangement_for_tests().grid,
        GridMode::Adaptive(GridDensity::Narrow)
    );
    assert!(grid(&harness).unwrap() <= zoomed);
    harness.key(Key::Num2, Modifiers::COMMAND);
    harness.key(Key::Num2, Modifiers::COMMAND);
    assert_eq!(
        harness.app.arrangement_for_tests().grid,
        GridMode::Adaptive(GridDensity::Wide)
    );
    assert!(grid(&harness).unwrap() >= zoomed);
}

#[test]
fn right_clicking_an_empty_lane_chooses_the_grid() {
    let mut harness = Harness::with_instruments();
    let lane = harness.rect(Id::new(("lane", 2)));
    harness.right_click(lane.center());
    let wide = harness.labelled("Widest Grid");
    harness.click(wide.center());
    assert_eq!(
        harness.app.arrangement_for_tests().grid,
        GridMode::Adaptive(GridDensity::Widest)
    );

    harness.right_click(lane.center());
    let fixed = harness.labelled("Fixed Grid");
    harness.move_to(fixed.center());
    harness.steps(2);
    let eighth = harness.labelled("1/8");
    harness.move_to(pos2(eighth.center().x, fixed.center().y));
    harness.click(eighth.center());
    assert_eq!(harness.app.arrangement_for_tests().grid, GridMode::Fixed);
    assert_eq!(
        harness.project().snap_grid(),
        sympoie::time::SnapGrid::Eighth
    );
    assert_eq!(
        harness
            .app
            .arrangement_for_tests()
            .grid(harness.project())
            .interval(),
        Some(480)
    );
}

mod track_height_tests {
    use super::*;

    fn height(harness: &Harness, track: usize) -> f32 {
        harness.rect(Id::new(("header", track))).height()
    }

    fn resize(harness: &mut Harness, track: usize, delta: f32) {
        let handle = harness.rect(Id::new(("track-height-resize", track)));
        harness.drag(
            handle.center(),
            handle.center() + vec2(0.0, delta),
            Modifiers::NONE,
        );
    }

    fn assert_height(harness: &Harness, track: usize, expected: f32) {
        assert!(
            (height(harness, track) - expected).abs() < 1.0,
            "track {track}: expected {expected}, got {}",
            height(harness, track)
        );
    }

    fn preset(harness: &mut Harness, label: &str) {
        let view = harness.labelled("View");
        harness.click(view.center());
        let item = harness.labelled(label);
        harness.click(item.center());
        harness.key(Key::Escape, Modifiers::NONE);
        harness.steps(2);
    }

    #[test]
    fn resizing_changes_only_one_track_without_selecting_reordering_or_editing() {
        let mut harness = Harness::with_instruments();
        harness.select_track(2);
        let selection = harness.app.selection_for_tests().clone();
        let revision = harness.app.document_for_tests().revision();
        let main = harness.rect(Id::new(("header", 0)));
        let names: Vec<_> = harness
            .project()
            .tracks()
            .iter()
            .map(|track| track.name().to_owned())
            .collect();
        let mixers: Vec<_> = harness
            .project()
            .tracks()
            .iter()
            .map(|track| *track.mixer())
            .collect();
        let first = height(&harness, 1);
        let second = height(&harness, 2);
        let third = height(&harness, 3);

        resize(&mut harness, 1, 84.0);
        assert_height(&harness, 1, first + 84.0);
        assert_height(&harness, 2, second);
        assert_height(&harness, 3, third);
        resize(&mut harness, 2, 36.0);
        assert_height(&harness, 1, first + 84.0);
        assert_height(&harness, 2, second + 36.0);
        assert_height(&harness, 3, third);
        assert_eq!(harness.rect(Id::new(("header", 0))), main);
        assert_eq!(harness.app.selection_for_tests(), &selection);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(harness.app.document_for_tests().undo_label().is_none());
        assert!(!harness.app.document_for_tests().is_dirty());
        assert_eq!(
            harness
                .project()
                .tracks()
                .iter()
                .map(Track::name)
                .collect::<Vec<_>>(),
            names
        );
        assert_eq!(
            harness
                .project()
                .tracks()
                .iter()
                .map(|track| *track.mixer())
                .collect::<Vec<_>>(),
            mixers
        );
    }

    #[test]
    fn reset_button_and_presets_clear_overrides_and_double_click_uses_current_default() {
        let mut harness = Harness::with_instruments();
        let revision = harness.app.document_for_tests().revision();
        let main = height(&harness, 0);
        resize(&mut harness, 1, 84.0);
        resize(&mut harness, 2, 36.0);
        let reset = harness.rect(Id::new("reset-track-heights"));
        harness.click(reset.center());
        harness.steps(2);
        assert!(harness.app.arrangement_for_tests().track_heights.is_empty());
        assert_eq!(harness.app.arrangement_for_tests().row_height, 68.0);
        for track in 1..=3 {
            assert_height(&harness, track, 68.0);
        }

        for (label, expected) in [("Compact", 46.0), ("Large", 104.0), ("Normal", 68.0)] {
            resize(&mut harness, 1, 35.0);
            preset(&mut harness, label);
            assert!(harness.app.arrangement_for_tests().track_heights.is_empty());
            for track in 1..=3 {
                assert_height(&harness, track, expected);
            }
            resize(&mut harness, 2, 48.0);
            let handle = harness.rect(Id::new(("track-height-resize", 2)));
            harness.double_click(handle.center());
            assert_height(&harness, 2, expected);
            assert!(
                !harness
                    .app
                    .arrangement_for_tests()
                    .track_heights
                    .contains_key(&2)
            );
        }
        assert_height(&harness, 0, main);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(harness.app.document_for_tests().undo_label().is_none());
        assert!(!harness.app.document_for_tests().is_dirty());
    }

    #[test]
    fn drag_limits_and_automation_minimum_keep_the_saved_normal_height() {
        let mut harness = Harness::with_instruments();
        resize(&mut harness, 1, -500.0);
        assert_height(&harness, 1, 46.0);
        let normal_height = harness.app.arrangement_for_tests().track_heights[&1];
        harness.dispatch(Action::ToggleAutomation);
        assert_height(&harness, 1, 92.0);
        assert_height(&harness, 2, 92.0);
        assert_eq!(
            harness.app.arrangement_for_tests().track_heights[&1],
            normal_height
        );
        harness.dispatch(Action::ToggleAutomation);
        assert_height(&harness, 1, 46.0);
        assert_height(&harness, 2, 68.0);
        harness.dispatch(Action::ToggleAutomation);
        resize(&mut harness, 1, -500.0);
        assert_height(&harness, 1, 92.0);
        harness.dispatch(Action::ToggleAutomation);
        resize(&mut harness, 1, 500.0);
        assert_height(&harness, 1, 320.0);
        assert_height(&harness, 2, 68.0);
        assert!(!harness.app.document_for_tests().is_dirty());
        assert!(harness.app.document_for_tests().undo_label().is_none());
    }

    #[test]
    fn heights_follow_track_reorder_and_undo_redo() {
        let mut harness = Harness::with_instruments();
        resize(&mut harness, 1, 45.0);
        resize(&mut harness, 2, 91.0);
        let lead = height(&harness, 1);
        let bass = height(&harness, 2);
        harness.dispatch(Action::MoveTrack {
            track: 1,
            target: TrackDrop::After(3),
        });
        assert_eq!(harness.project().tracks()[3].name(), "Lead");
        assert_height(&harness, 3, lead);
        assert_height(&harness, 1, bass);
        assert_height(&harness, 2, 68.0);
        harness.dispatch(Action::Undo);
        assert_eq!(harness.project().tracks()[1].name(), "Lead");
        assert_height(&harness, 1, lead);
        assert_height(&harness, 2, bass);
        assert_height(&harness, 3, 68.0);
        assert!(!harness.app.document_for_tests().is_dirty());
        harness.dispatch(Action::Redo);
        assert_height(&harness, 3, lead);
        assert_height(&harness, 1, bass);
        assert_height(&harness, 2, 68.0);
    }

    #[test]
    fn appending_preserves_heights_and_structural_history_or_replacement_clears_them() {
        let mut harness = Harness::with_instruments();
        resize(&mut harness, 1, 50.0);
        let lead = height(&harness, 1);
        harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Audio));
        assert_height(&harness, 1, lead);
        assert_height(&harness, 4, 68.0);
        harness.dispatch(Action::Undo);
        assert!(harness.app.arrangement_for_tests().track_heights.is_empty());
        assert_height(&harness, 1, 68.0);
        resize(&mut harness, 2, 40.0);
        harness.dispatch(Action::Redo);
        assert!(harness.app.arrangement_for_tests().track_heights.is_empty());
        assert_height(&harness, 2, 68.0);
        harness.dispatch(Action::Undo);

        resize(&mut harness, 1, 50.0);
        harness.dispatch(Action::OpenPath(harness._files.0.join("instruments.symp")));
        assert!(harness.app.arrangement_for_tests().track_heights.is_empty());
        assert_height(&harness, 1, 68.0);
        resize(&mut harness, 1, 50.0);
        harness.dispatch(Action::NewProject);
        assert!(harness.app.arrangement_for_tests().track_heights.is_empty());
        assert_eq!(harness.project().tracks().len(), 1);
    }

    #[test]
    fn escape_cancels_a_resize_and_restores_the_previous_override() {
        let mut harness = Harness::with_instruments();
        harness.select_track(3);
        let selection = harness.app.selection_for_tests().clone();
        resize(&mut harness, 1, 35.0);
        let revision = harness.app.document_for_tests().revision();
        for track in [1, 2] {
            let before = height(&harness, track);
            let previous = harness
                .app
                .arrangement_for_tests()
                .track_heights
                .get(&track)
                .copied();
            let handle = harness.rect(Id::new(("track-height-resize", track)));
            harness.move_to(handle.center());
            harness.button(true, Modifiers::NONE);
            harness.move_to(handle.center() + vec2(0.0, 60.0));
            harness.steps(2);
            assert_height(&harness, track, before + 60.0);
            harness.key(Key::Escape, Modifiers::NONE);
            harness.button(false, Modifiers::NONE);
            harness.steps(2);
            assert_height(&harness, track, before);
            assert_eq!(
                harness
                    .app
                    .arrangement_for_tests()
                    .track_heights
                    .get(&track)
                    .copied(),
                previous
            );
            assert_eq!(harness.app.selection_for_tests(), &selection);
        }
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(harness.app.document_for_tests().undo_label().is_none());
        assert!(!harness.app.document_for_tests().is_dirty());
    }

    #[test]
    fn escape_during_resize_preserves_the_focused_piano_editor_and_clip_selection() {
        let mut harness = Harness::with_midi_clip();
        harness.click(harness.note_rect(0).center());
        let selection = harness.app.selection_for_tests().clone();
        let clip = harness.midi_clip().clone();
        let revision = harness.app.document_for_tests().revision();
        let before = height(&harness, 1);
        let handle = harness.rect(Id::new(("track-height-resize", 1)));
        harness.move_to(handle.center());
        harness.button(true, Modifiers::NONE);
        harness.move_to(handle.center() + vec2(0.0, 60.0));
        harness.steps(2);
        assert_height(&harness, 1, before + 60.0);
        harness.key(Key::Escape, Modifiers::NONE);
        harness.button(false, Modifiers::NONE);
        harness.steps(2);
        assert_height(&harness, 1, before);
        assert_eq!(harness.app.detail_view_for_tests(), "piano");
        assert_eq!(harness.app.selection_for_tests(), &selection);
        assert_eq!(*harness.midi_clip(), clip);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(!harness.app.document_for_tests().is_dirty());
    }

    #[test]
    fn losing_window_focus_cancels_resize_without_committing_the_preview() {
        let mut harness = Harness::with_instruments();
        harness.select_track(2);
        let selection = harness.app.selection_for_tests().clone();
        resize(&mut harness, 1, 35.0);
        let before = height(&harness, 1);
        let previous = harness.app.arrangement_for_tests().track_heights[&1];
        let revision = harness.app.document_for_tests().revision();
        let handle = harness.rect(Id::new(("track-height-resize", 1)));
        harness.move_to(handle.center());
        harness.button(true, Modifiers::NONE);
        harness.move_to(handle.center() + vec2(0.0, 60.0));
        harness.steps(2);
        assert_height(&harness, 1, before + 60.0);
        harness.step_input_with_focus(RawInput::default(), false);
        harness.button(false, Modifiers::NONE);
        harness.steps(2);
        assert_height(&harness, 1, before);
        assert_eq!(
            harness.app.arrangement_for_tests().track_heights[&1],
            previous
        );
        assert_eq!(harness.app.selection_for_tests(), &selection);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(!harness.app.document_for_tests().is_dirty());
    }

    #[test]
    fn a_fast_resize_applies_movement_when_motion_and_release_share_a_frame() {
        let mut harness = Harness::with_instruments();
        harness.select_track(2);
        let selection = harness.app.selection_for_tests().clone();
        let revision = harness.app.document_for_tests().revision();
        let before = height(&harness, 1);
        let handle = harness.rect(Id::new(("track-height-resize", 1)));
        let end = handle.center() + vec2(0.0, 76.0);
        harness.move_to(handle.center());
        harness.button(true, Modifiers::NONE);
        harness.pointer = end;
        harness.step(vec![
            Event::PointerMoved(end),
            Event::PointerButton {
                pos: end,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]);
        harness.steps(3);
        assert_height(&harness, 1, before + 76.0);
        assert_height(&harness, 2, 68.0);
        assert_eq!(harness.app.selection_for_tests(), &selection);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(!harness.app.document_for_tests().is_dirty());
    }

    #[test]
    fn a_fast_resize_applies_a_complete_gesture_delivered_in_one_frame() {
        let mut harness = Harness::with_instruments();
        harness.select_track(2);
        let selection = harness.app.selection_for_tests().clone();
        let revision = harness.app.document_for_tests().revision();
        let before = height(&harness, 1);
        let handle = harness.rect(Id::new(("track-height-resize", 1)));
        let start = handle.center();
        let end = start + vec2(0.0, 76.0);
        harness.move_to(start);
        harness.pointer = end;
        harness.step(vec![
            Event::PointerButton {
                pos: start,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(end),
            Event::PointerButton {
                pos: end,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]);
        harness.steps(3);
        assert_height(&harness, 1, before + 76.0);
        assert_height(&harness, 2, 68.0);
        assert_eq!(harness.app.selection_for_tests(), &selection);
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(!harness.app.document_for_tests().is_dirty());
    }

    #[test]
    fn a_batched_drag_over_a_context_menu_does_not_resize_the_covered_track() {
        let mut harness = Harness::with_instruments();
        let before = height(&harness, 1);
        let revision = harness.app.document_for_tests().revision();
        let handle = harness
            .ctx
            .read_response(Id::new(("track-height-resize", 1)))
            .unwrap();
        let start = handle.rect.center();
        let header = harness.rect(Id::new(("header", 1)));
        harness.right_click(header.center_top() + vec2(0.0, 8.0));
        assert_ne!(
            harness.ctx.layer_id_at(start),
            Some(handle.layer_id),
            "the context menu must cover the resize handle for this regression"
        );
        let end = start + vec2(0.0, 76.0);
        harness.pointer = end;
        harness.step(vec![
            Event::PointerButton {
                pos: start,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(end),
            Event::PointerButton {
                pos: end,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]);
        harness.steps(3);
        assert_height(&harness, 1, before);
        assert!(harness.app.arrangement_for_tests().track_heights.is_empty());
        assert_eq!(harness.app.document_for_tests().revision(), revision);
        assert!(!harness.app.document_for_tests().is_dirty());
    }
}

#[test]
fn every_mixer_channel_has_a_level_meter_beside_its_fader() {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::ShowMixer);
    for index in 0..harness.project().tracks().len() {
        let fader = harness.rect(Id::new(("mixer-fader", index)));
        let meter = harness.rect(Id::new(("mixer-meter", index)));
        assert!(
            meter.left() >= fader.right(),
            "track {index}: meter beside the fader"
        );
        assert!(meter.height() > 40.0, "track {index}: a full-height meter");
    }
}

mod agent_mode {
    use super::*;
    use sympoie::project_json::{load_project_folder, save_project_folder};

    fn run_until(harness: &mut Harness, mut done: impl FnMut(&Harness) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done(harness) {
            assert!(std::time::Instant::now() < deadline, "timed out");
            std::thread::sleep(std::time::Duration::from_millis(20));
            harness.steps(1);
        }
    }

    fn track_file(folder: &std::path::Path, ending: &str) -> std::path::PathBuf {
        std::fs::read_dir(folder.join("tracks"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.to_string_lossy().ends_with(ending))
            .unwrap()
    }

    fn agent_project(harness: &mut Harness) -> std::path::PathBuf {
        let folder = harness._files.0.join("agent.symp");
        let mut project = Project::new();
        project.add_midi_track("Bass");
        save_project_folder(&project, &folder).unwrap();
        harness.dispatch(Action::OpenPath(folder.clone()));
        harness.dispatch(Action::ToggleAgentMode);
        assert!(harness.app.agent_mode_for_tests());
        folder
    }

    #[test]
    fn edits_are_saved_after_a_pause_and_outside_edits_load_as_one_undo_step() {
        let mut harness = Harness::with_empty_project();
        let folder = agent_project(&mut harness);

        harness.dispatch(Action::SetBpm(97.0));
        assert!(harness.app.document_for_tests().is_dirty());
        harness.steps(10);
        assert_eq!(load_project_folder(&folder).unwrap().bpm(), 120.0);
        harness.time += 1.5;
        run_until(&mut harness, |harness| {
            !harness.app.document_for_tests().is_dirty()
        });
        assert_eq!(load_project_folder(&folder).unwrap().bpm(), 97.0);

        let file = track_file(&folder, "-bass.json");
        let text = std::fs::read_to_string(&file).unwrap();
        let edited = text.replace("\"Bass\"", "\"Sub Bass\"").replace(
            "\"clips\": []",
            "\"clips\": [{\"start_tick\": 0, \"length_ticks\": 3840, \"notes\": \
             [{\"start\": 0, \"length\": 960, \"key\": 36}]}]",
        );
        assert_ne!(edited, text);
        std::fs::write(&file, edited).unwrap();
        run_until(&mut harness, |harness| {
            harness.project().tracks()[1].name() == "Sub Bass"
        });
        assert_eq!(harness.midi_clips(1)[0].notes[0].key, 36);
        assert!(!harness.app.document_for_tests().is_dirty());
        assert_eq!(
            harness.app.document_for_tests().undo_label(),
            Some("Agent Edit")
        );
        harness.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(harness.project().tracks()[1].name(), "Bass");
        harness.time += 1.5;
        run_until(&mut harness, |harness| {
            !harness.app.document_for_tests().is_dirty()
        });
        assert_eq!(
            load_project_folder(&folder).unwrap().tracks()[1].name(),
            "Bass"
        );
    }

    #[test]
    fn broken_files_pause_autosave_and_report_the_error() {
        let mut harness = Harness::with_empty_project();
        let folder = agent_project(&mut harness);
        let file = track_file(&folder, "-bass.json");
        std::fs::write(&file, "{\"type\": \"midi\", \"nmae\": \"x\"}").unwrap();
        run_until(&mut harness, |harness| {
            harness
                .app
                .notice_for_tests()
                .is_some_and(|notice| notice.contains("unknown field \"nmae\""))
        });
        harness.dispatch(Action::SetBpm(90.0));
        harness.time += 1.5;
        harness.steps(5);
        std::thread::sleep(std::time::Duration::from_millis(300));
        harness.steps(5);
        assert!(std::fs::read_to_string(&file).unwrap().contains("nmae"));
        std::fs::write(&file, "{\"type\": \"midi\", \"name\": \"Fixed\"}").unwrap();
        run_until(&mut harness, |harness| {
            harness.app.agent_conflict_for_tests()
        });
        let take = harness.labelled("Use the Agent's Version");
        harness.click(take.center());
        assert_eq!(harness.project().tracks()[1].name(), "Fixed");
        assert!(harness.app.agent_problem_for_tests().is_none());
    }

    #[test]
    fn simultaneous_changes_ask_which_version_to_keep() {
        let mut harness = Harness::with_empty_project();
        let folder = agent_project(&mut harness);
        harness.dispatch(Action::SetBpm(80.0));
        let manifest = folder.join(sympoie::project_json::MANIFEST);
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            text.replace("\"tempo\": 120.0", "\"tempo\": 140.0"),
        )
        .unwrap();
        run_until(&mut harness, |harness| {
            harness.app.agent_conflict_for_tests()
        });
        assert_eq!(harness.project().bpm(), 80.0);
        let keep = harness.labelled("Keep Mine");
        harness.click(keep.center());
        assert!(!harness.app.agent_conflict_for_tests());
        run_until(&mut harness, |_| {
            load_project_folder(&folder).unwrap().bpm() == 80.0
        });

        harness.dispatch(Action::SetBpm(85.0));
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            text.replace("\"tempo\": 80.0", "\"tempo\": 150.0"),
        )
        .unwrap();
        run_until(&mut harness, |harness| {
            harness.app.agent_conflict_for_tests()
        });
        let take = harness.labelled("Use the Agent's Version");
        harness.click(take.center());
        assert_eq!(harness.project().bpm(), 150.0);
        harness.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(harness.project().bpm(), 85.0);
    }

    #[test]
    fn agent_mode_is_remembered_per_project_and_off_ignores_the_files() {
        let mut harness = Harness::with_empty_project();
        let folder = agent_project(&mut harness);
        let other = harness._files.0.join("other.symp");
        save_project_folder(&Project::new(), &other).unwrap();
        harness.dispatch(Action::OpenPath(other.clone()));
        assert!(!harness.app.agent_mode_for_tests());
        harness.dispatch(Action::OpenPath(folder.clone()));
        assert!(harness.app.agent_mode_for_tests());

        harness.dispatch(Action::ToggleAgentMode);
        assert!(!harness.app.agent_mode_for_tests());
        let manifest = folder.join(sympoie::project_json::MANIFEST);
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            text.replace("\"tempo\": 120.0", "\"tempo\": 60.0"),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(900));
        harness.steps(3);
        assert_eq!(harness.project().bpm(), 120.0);
        harness.dispatch(Action::OpenPath(other));
        harness.dispatch(Action::OpenPath(folder));
        assert!(!harness.app.agent_mode_for_tests());
    }

    fn while_running<T: Send + 'static>(
        harness: &mut Harness,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let worker = std::thread::spawn(work);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !worker.is_finished() {
            assert!(std::time::Instant::now() < deadline, "timed out");
            std::thread::sleep(std::time::Duration::from_millis(10));
            harness.steps(1);
        }
        worker.join().unwrap()
    }

    #[test]
    fn analysis_only_loads_external_plugins_on_the_main_thread() {
        let mut harness = Harness::with_empty_project();
        let folder = harness._files.0.join("vst.symp");
        let path = harness._files.0.join("analysis-thread-check.vst3");
        let mut project = Project::new();
        let drums = project.add_midi_track("Drums");
        let descriptor = sympoie::plugin::PluginDescriptor {
            path: path.clone(),
            class_id: "00000000000000000000000000000000".into(),
            name: "External Drums".into(),
            vendor: String::new(),
            kind: sympoie::plugin::PluginKind::Instrument,
        };
        project
            .set_plugin_instrument(
                drums,
                Some(sympoie::plugin::PluginInstance::new(
                    descriptor,
                    Vec::new(),
                    Vec::new(),
                )),
            )
            .unwrap();
        save_project_folder(&project, &folder).unwrap();
        harness.dispatch(Action::OpenPath(folder.clone()));
        harness.dispatch(Action::ToggleAgentMode);
        run_until(&mut harness, |_| {
            folder.join(crate::mcp::DISCOVERY).is_file()
        });

        let call_folder = folder.clone();
        let analysis = while_running(&mut harness, move || {
            crate::mcp::server::call(
                &call_folder,
                "analyze_audio",
                &serde_json::json!({"start_tick": 0, "end_tick": 3840}),
            )
        });
        assert!(analysis.unwrap_err().contains("External Drums"));
        let main = std::thread::current().id();
        let loads: Vec<_> = crate::plugins::LOAD_THREADS
            .lock()
            .unwrap()
            .iter()
            .filter(|(loaded, _)| *loaded == path)
            .map(|&(_, thread)| thread)
            .collect();
        assert!(loads.len() >= 2, "the editor and the analysis both load it");
        assert!(loads.iter().all(|&thread| thread == main), "{loads:?}");
    }

    #[test]
    fn agents_get_a_guide_and_tools_that_reach_the_open_editor() {
        let mut harness = Harness::with_empty_project();
        let folder = harness._files.0.join("tools.symp");
        let mut project = Project::new();
        let synth = project.add_midi_track("Lead");
        project
            .set_plugin_instrument(synth, Some(sympoie::builtin::BuiltinKind::Oscil.instance()))
            .unwrap();
        project
            .add_midi_clip(
                synth,
                MidiClip {
                    notes: vec![MidiNote {
                        start_tick: 0,
                        duration_ticks: 1920,
                        key: 57,
                        velocity: 110,
                        channel: 0,
                    }],
                    ..MidiClip::new(0, 7680)
                },
            )
            .unwrap();
        project.add_audio_track("Empty");
        save_project_folder(&project, &folder).unwrap();
        harness.dispatch(Action::OpenPath(folder.clone()));
        harness.dispatch(Action::ToggleAgentMode);
        assert!(
            std::fs::read_to_string(folder.join("AGENTS.md"))
                .unwrap()
                .contains("960 per quarter note")
        );
        assert!(folder.join("CLAUDE.md").is_file());
        assert!(
            std::fs::read_to_string(folder.join(".mcp.json"))
                .unwrap()
                .contains("--mcp")
        );
        run_until(&mut harness, |_| {
            folder.join(crate::mcp::DISCOVERY).is_file()
        });
        harness.dispatch(Action::SelectTracks([1].into()));

        let bridge_folder = folder.clone();
        let output = while_running(&mut harness, move || {
            let input = concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",",
                "\"params\":{\"name\":\"get_context\",\"arguments\":{}}}\n"
            );
            let mut output = Vec::new();
            crate::mcp::bridge::run(Some(bridge_folder), input.as_bytes(), &mut output);
            String::from_utf8(output).unwrap()
        });
        let reply: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        let context = &reply["result"]["structuredContent"];
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        let lead = project.tracks()[1].uid();
        assert_eq!(context["selection"]["tracks"][0]["id"], lead);
        assert_eq!(
            context["selection"]["tracks"][0]["file"],
            format!("tracks/{lead}-lead.json")
        );
        assert_eq!(context["tracks"].as_array().unwrap().len(), 3);
        assert_eq!(context["ticks_per_bar"], 3840);
        assert_eq!(context["files_status"]["loads"], true);

        let call_folder = folder.clone();
        let done = while_running(&mut harness, move || {
            crate::mcp::server::call(
                &call_folder,
                "transport",
                &serde_json::json!({"action": "set_loop", "start_tick": 3840, "end_tick": 7680}),
            )
        });
        assert_eq!(done.unwrap()["done"], true);
        let region = harness.app.loop_for_tests();
        assert!(region.enabled && region.start == 3840 && region.end == 7680);

        let call_folder = folder.clone();
        let analysis = while_running(&mut harness, move || {
            crate::mcp::server::call(
                &call_folder,
                "analyze_audio",
                &serde_json::json!({"start_tick": 0, "end_tick": 7680, "tracks": [lead],
                                    "save_wav": "renders/lead.wav"}),
            )
        })
        .unwrap();
        assert_eq!(analysis["silent"], false, "{analysis}");
        assert_eq!(analysis["segments"].as_array().unwrap().len(), 2);
        let first = analysis["segments"][0]["rms_db"].as_f64().unwrap();
        let second = analysis["segments"][1]["rms_db"].as_f64().unwrap();
        assert!(first > second + 20.0, "{analysis}");
        let wav = sympoie::wav::read_wav(folder.join("renders/lead.wav")).unwrap();
        assert_eq!(wav.len(), 4 * 48_000);
        let call_folder = folder.clone();
        let refused = while_running(&mut harness, move || {
            crate::mcp::server::call(
                &call_folder,
                "analyze_audio",
                &serde_json::json!({"save_wav": "../outside.wav"}),
            )
        });
        assert!(refused.unwrap_err().contains("renders/NAME.wav"));

        harness.dispatch(Action::ToggleAgentMode);
        assert!(!folder.join(crate::mcp::DISCOVERY).exists());
        let error =
            crate::mcp::server::call(&folder, "get_context", &serde_json::json!({})).unwrap_err();
        assert!(error.contains("AGENT switch"));
    }
}
