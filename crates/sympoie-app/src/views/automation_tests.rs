use super::*;
use crate::document::edit::Grid;

struct Harness {
    ctx: egui::Context,
    project: Project,
    state: ArrangementState,
    editor: Editor,
    time: f64,
}

impl Harness {
    fn new(points: Vec<AutomationPoint>) -> Self {
        let mut project = Project::new();
        let track = project.add_audio_track("Automated");
        if !points.is_empty() {
            project
                .set_track_automation(
                    track,
                    AutomationLane::new(AutomationTarget::TrackGain, points).unwrap(),
                )
                .unwrap();
        }
        let mut harness = Self {
            ctx: egui::Context::default(),
            project,
            state: ArrangementState {
                automation: true,
                ..Default::default()
            },
            editor: Editor::default(),
            time: 0.0,
        };
        harness.step(Vec::new());
        harness.step(Vec::new());
        harness
    }

    fn step(&mut self, events: Vec<egui::Event>) -> Vec<Action> {
        self.time += 1.0 / 30.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(520.0, 200.0))),
            time: Some(self.time),
            focused: true,
            events,
            ..Default::default()
        };
        let mut actions = Vec::new();
        let mut output = self.ctx.run_ui(input, |ui| {
            self.editor
                .keyboard(ui, &self.project, &self.state, &mut actions);
            lane(
                ui,
                Rect::from_min_max(pos2(10.0, 40.0), pos2(490.0, 140.0)),
                1,
                &self.state,
                &self.project,
                &Timeline {
                    left: 10.0,
                    right: 490.0,
                    scroll_tick: 0.0,
                    pixels_per_tick: 0.1,
                    grid: Grid::every(240),
                },
                &mut self.editor,
                &mut actions,
            );
            self.editor.finish(ui, &mut actions);
        });
        output.textures_delta.clear();
        for action in &actions {
            let id = self.project.track_ids().nth(1).unwrap();
            match action {
                Action::SetAutomation { lane, .. } => {
                    self.project.set_track_automation(id, lane.clone()).unwrap()
                }
                Action::RemoveAutomation { target, .. } => {
                    self.project.remove_track_automation(id, *target).unwrap()
                }
                _ => panic!("Automation emitted a non-envelope action"),
            }
        }
        actions
    }

    fn button(&mut self, pos: Pos2, pressed: bool) -> Vec<Action> {
        self.step(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }])
    }

    fn key(&mut self, key: Key) -> Vec<Action> {
        self.step(vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }])
    }

    fn click(&mut self, pos: Pos2) -> Vec<Action> {
        assert!(self.step(vec![egui::Event::PointerMoved(pos)]).is_empty());
        assert!(self.button(pos, true).is_empty());
        self.button(pos, false)
    }

    fn edit_value(&mut self, point: AutomationPoint) {
        let pos = pos2(
            10.0 + point.tick as f32 * 0.1,
            130.0 - point.value as f32 * 80.0,
        );
        assert!(self.step(vec![egui::Event::PointerMoved(pos)]).is_empty());
        for pressed in [true, false] {
            assert!(
                self.step(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }])
                .is_empty()
            );
        }
        assert!(self.step(Vec::new()).is_empty());
        assert!(self.step(Vec::new()).is_empty());
        assert!(
            self.ctx
                .read_response(Id::new("automation-point-value"))
                .is_some()
        );
    }

    fn enter_value(&mut self, value: &str) {
        let field = self
            .ctx
            .read_response(Id::new("automation-point-value"))
            .unwrap();
        field.request_focus();
        assert!(
            self.step(vec![
                egui::Event::ModifiersChanged(Modifiers::COMMAND),
                egui::Event::Key {
                    key: Key::A,
                    physical_key: Some(Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                },
                egui::Event::ModifiersChanged(Modifiers::NONE),
                egui::Event::Paste(value.to_owned()),
            ])
            .is_empty()
        );
    }

    fn points(&self) -> &[AutomationPoint] {
        self.project.tracks()[1].automation()[0].points()
    }
}

#[test]
fn clicking_an_empty_envelope_adds_a_snapped_point_at_the_manual_value() {
    let mut harness = Harness::new(Vec::new());
    let pos = pos2(113.0, 70.0);
    harness.step(vec![egui::Event::PointerMoved(pos)]);
    assert!(harness.button(pos, true).is_empty());
    let actions = harness.button(pos, false);
    assert!(matches!(
        actions.as_slice(),
        [Action::SetAutomation { track: 1, .. }]
    ));
    assert_eq!(
        harness.points(),
        &[AutomationPoint {
            tick: 960,
            value: 0.5
        }]
    );
}

#[test]
fn empty_envelopes_follow_the_selected_parameters_saved_manual_value() {
    for (target, expected) in [
        (AutomationTarget::TrackGain, 0.25),
        (AutomationTarget::TrackPan, 0.2),
    ] {
        let mut harness = Harness::new(Vec::new());
        let sympoie::track::TrackId::Audio(track) = harness.project.track_ids().nth(1).unwrap()
        else {
            panic!("the harness should have an audio track");
        };
        let mixer =
            sympoie::track::MixerSettings::new(0.5, -0.6, sympoie::track::AudioOutput::Master)
                .unwrap();
        harness.project.set_audio_track_mixer(track, mixer).unwrap();
        harness.state.automation_targets.insert(1, target);
        assert_eq!(harness.click(pos2(113.0, 55.0)).len(), 1);
        assert_eq!(harness.project.tracks()[1].automation()[0].target(), target);
        assert!((harness.points()[0].value - expected).abs() < 1e-7);
    }
}

#[test]
fn double_clicking_empty_space_keeps_the_new_breakpoint() {
    let mut harness = Harness::new(Vec::new());
    let pos = pos2(106.0, 70.0);
    harness.step(vec![egui::Event::PointerMoved(pos)]);
    harness.button(pos, true);
    harness.button(pos, false);
    harness.button(pos, true);
    assert!(harness.button(pos, false).is_empty());
    assert_eq!(
        harness.points(),
        &[AutomationPoint {
            tick: 960,
            value: 0.5
        }]
    );
}

#[test]
fn clicking_an_envelope_uses_its_value_at_the_snapped_tick() {
    let mut harness = Harness::new(vec![
        AutomationPoint {
            tick: 480,
            value: 0.2,
        },
        AutomationPoint {
            tick: 1920,
            value: 0.8,
        },
    ]);
    let actions = harness.click(pos2(113.0, 55.0));
    assert!(matches!(
        actions.as_slice(),
        [Action::SetAutomation { track: 1, .. }]
    ));
    assert_eq!(harness.points().len(), 3);
    assert_eq!(harness.points()[1].tick, 960);
    assert!((harness.points()[1].value - 0.4).abs() < 1e-12);
}

#[test]
fn clicks_outside_existing_points_hold_the_endpoint_values() {
    let mut harness = Harness::new(vec![
        AutomationPoint {
            tick: 480,
            value: 0.2,
        },
        AutomationPoint {
            tick: 1920,
            value: 0.8,
        },
    ]);
    assert_eq!(harness.click(pos2(12.0, 55.0)).len(), 1);
    assert_eq!(
        harness.points()[0],
        AutomationPoint {
            tick: 0,
            value: 0.2
        }
    );
    assert_eq!(harness.click(pos2(300.0, 120.0)).len(), 1);
    assert_eq!(
        *harness.points().last().unwrap(),
        AutomationPoint {
            tick: 2880,
            value: 0.8
        }
    );
}

#[test]
fn clicking_a_disabled_envelope_preserves_its_shape_and_disabled_state() {
    let mut harness = Harness::new(vec![
        AutomationPoint {
            tick: 480,
            value: 0.2,
        },
        AutomationPoint {
            tick: 1920,
            value: 0.8,
        },
    ]);
    let track = harness.project.track_ids().nth(1).unwrap();
    let lane = harness.project.tracks()[1].automation()[0]
        .clone()
        .with_enabled(false);
    harness.project.set_track_automation(track, lane).unwrap();
    assert_eq!(harness.click(pos2(113.0, 55.0)).len(), 1);
    assert!((harness.points()[1].value - 0.4).abs() < 1e-12);
    assert!(!harness.project.tracks()[1].automation()[0].enabled());
}

#[test]
fn clicking_a_snapped_existing_tick_selects_without_a_redundant_edit() {
    let mut harness = Harness::new(vec![AutomationPoint {
        tick: 960,
        value: 0.3,
    }]);
    assert!(harness.click(pos2(113.0, 55.0)).is_empty());
    assert_eq!(
        harness.points(),
        &[AutomationPoint {
            tick: 960,
            value: 0.3
        }]
    );
    assert_eq!(
        harness.editor.selected,
        Some(SelectedPoint {
            track: 1,
            target: AutomationTarget::TrackGain,
            tick: 960,
        })
    );
}

#[test]
fn entering_a_point_value_commits_once_and_preserves_the_other_points() {
    let original = [
        AutomationPoint {
            tick: 480,
            value: 0.5,
        },
        AutomationPoint {
            tick: 1920,
            value: 0.8,
        },
    ];
    let mut harness = Harness::new(original.to_vec());
    harness.edit_value(original[0]);
    harness.enter_value("-6 dB");
    assert_eq!(harness.points(), &original);
    assert!(matches!(
        harness.key(Key::Enter).as_slice(),
        [Action::SetAutomation { track: 1, .. }]
    ));
    assert_eq!(harness.points()[0].tick, original[0].tick);
    assert!((harness.points()[0].value - 10.0_f64.powf(-6.0 / 20.0) / 2.0).abs() < 1e-12);
    assert_eq!(harness.points()[1], original[1]);
    assert!(harness.step(Vec::new()).is_empty());
}

#[test]
fn applying_a_point_value_keeps_a_disabled_envelope_disabled() {
    let point = AutomationPoint {
        tick: 480,
        value: 0.5,
    };
    let mut harness = Harness::new(vec![point]);
    let track = harness.project.track_ids().nth(1).unwrap();
    let lane = harness.project.tracks()[1].automation()[0]
        .clone()
        .with_enabled(false);
    harness.project.set_track_automation(track, lane).unwrap();
    harness.edit_value(point);
    harness.enter_value("-12");
    let apply = harness
        .ctx
        .read_response(Id::new("automation-point-value-apply"))
        .unwrap()
        .rect;
    assert!(matches!(
        harness.click(apply.center()).as_slice(),
        [Action::SetAutomation { track: 1, .. }]
    ));
    assert!((harness.points()[0].value - 10.0_f64.powf(-12.0 / 20.0) / 2.0).abs() < 1e-12);
    assert!(!harness.project.tracks()[1].automation()[0].enabled());
    assert!(harness.step(Vec::new()).is_empty());
}

#[test]
fn escape_or_dismiss_cancels_the_uncommitted_point_value() {
    let point = AutomationPoint {
        tick: 480,
        value: 0.5,
    };
    let mut harness = Harness::new(vec![point]);
    harness.edit_value(point);
    harness.enter_value("-6");
    assert!(harness.key(Key::Escape).is_empty());
    assert!(harness.step(Vec::new()).is_empty());
    assert_eq!(harness.points(), &[point]);

    harness.edit_value(point);
    harness.enter_value("-12");
    assert!(harness.click(pos2(510.0, 190.0)).is_empty());
    assert!(harness.step(Vec::new()).is_empty());
    assert_eq!(harness.points(), &[point]);
}

#[test]
fn invalid_point_value_remains_editable_without_changing_the_envelope() {
    let point = AutomationPoint {
        tick: 480,
        value: 0.5,
    };
    let mut harness = Harness::new(vec![point]);
    harness.edit_value(point);
    harness.enter_value("not a value");
    assert!(harness.key(Key::Enter).is_empty());
    assert_eq!(harness.points(), &[point]);
    assert!(harness.step(Vec::new()).is_empty());
    assert!(
        harness
            .ctx
            .read_response(Id::new("automation-point-value"))
            .is_some()
    );
    harness.enter_value("-6");
    assert!(matches!(
        harness.key(Key::Enter).as_slice(),
        [Action::SetAutomation { track: 1, .. }]
    ));
    assert!((harness.points()[0].value - 10.0_f64.powf(-6.0 / 20.0) / 2.0).abs() < 1e-12);
}

#[test]
fn accepting_the_prefilled_value_does_not_round_or_edit_the_point() {
    let point = AutomationPoint {
        tick: 480,
        value: 0.123456789,
    };
    let mut harness = Harness::new(vec![point]);
    harness.edit_value(point);
    assert!(harness.key(Key::Enter).is_empty());
    assert!(harness.step(Vec::new()).is_empty());
    assert_eq!(harness.points(), &[point]);
}

#[test]
fn dragging_empty_space_still_creates_a_point_at_the_pointer_value() {
    let mut harness = Harness::new(Vec::new());
    let from = pos2(58.0, 90.0);
    let to = pos2(106.0, 70.0);
    harness.step(vec![egui::Event::PointerMoved(from)]);
    assert!(harness.button(from, true).is_empty());
    assert!(harness.step(vec![egui::Event::PointerMoved(to)]).is_empty());
    assert!(harness.project.tracks()[1].automation().is_empty());
    assert!(matches!(
        harness.button(to, false).as_slice(),
        [Action::SetAutomation { track: 1, .. }]
    ));
    assert_eq!(
        harness.points(),
        &[AutomationPoint {
            tick: 960,
            value: 0.75
        }]
    );
}

#[test]
fn point_drag_previews_without_edits_then_commits_once_and_clamps_to_neighbors() {
    let mut harness = Harness::new(vec![
        AutomationPoint {
            tick: 480,
            value: 0.5,
        },
        AutomationPoint {
            tick: 1920,
            value: 0.5,
        },
    ]);
    let from = pos2(58.0, 90.0);
    let to = pos2(400.0, 45.0);
    harness.step(vec![egui::Event::PointerMoved(from)]);
    assert!(harness.button(from, true).is_empty());
    assert!(harness.step(vec![egui::Event::PointerMoved(to)]).is_empty());
    assert_eq!(
        harness.points()[0],
        AutomationPoint {
            tick: 480,
            value: 0.5
        }
    );
    let actions = harness.button(to, false);
    assert!(matches!(
        actions.as_slice(),
        [Action::SetAutomation { track: 1, .. }]
    ));
    assert_eq!(
        harness.points()[0],
        AutomationPoint {
            tick: 1919,
            value: 1.0
        }
    );
    assert!(harness.step(Vec::new()).is_empty());
}

#[test]
fn escape_cancels_a_drag_and_delete_removes_only_the_selected_breakpoint() {
    let mut harness = Harness::new(vec![AutomationPoint {
        tick: 480,
        value: 0.5,
    }]);
    let from = pos2(58.0, 90.0);
    let to = pos2(200.0, 70.0);
    harness.step(vec![egui::Event::PointerMoved(from)]);
    harness.button(from, true);
    harness.step(vec![egui::Event::PointerMoved(to)]);
    assert!(harness.key(Key::Escape).is_empty());
    assert!(harness.button(to, false).is_empty());
    assert_eq!(
        harness.points(),
        &[AutomationPoint {
            tick: 480,
            value: 0.5
        }]
    );
    harness.step(vec![egui::Event::PointerMoved(from)]);
    harness.button(from, true);
    harness.button(from, false);
    assert!(matches!(
        harness.key(Key::Delete).as_slice(),
        [Action::RemoveAutomation { track: 1, .. }]
    ));
    assert!(harness.project.tracks()[1].automation().is_empty());
}

#[test]
fn hidden_parameter_selection_cannot_delete_a_point_from_another_envelope() {
    let mut harness = Harness::new(vec![AutomationPoint {
        tick: 480,
        value: 0.5,
    }]);
    harness.editor.selected = Some(SelectedPoint {
        track: 1,
        target: AutomationTarget::TrackGain,
        tick: 480,
    });
    harness
        .state
        .automation_targets
        .insert(1, AutomationTarget::TrackPan);
    assert!(harness.key(Key::Delete).is_empty());
    assert_eq!(harness.points().len(), 1);
}

struct SelectorHarness {
    ctx: egui::Context,
    project: Project,
    track: usize,
    state: ArrangementState,
    editor: Editor,
    time: f64,
    accessibility: Option<egui::accesskit::TreeUpdate>,
}

impl SelectorHarness {
    fn new(project: Project, track: usize) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut harness = Self {
            ctx,
            project,
            track,
            state: ArrangementState {
                automation: true,
                ..Default::default()
            },
            editor: Editor::default(),
            time: 0.0,
            accessibility: None,
        };
        harness.step(Vec::new());
        harness.step(Vec::new());
        harness
    }

    fn step(&mut self, events: Vec<egui::Event>) -> Vec<Action> {
        self.time += 1.0 / 30.0;
        let mut actions = Vec::new();
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 500.0))),
                time: Some(self.time),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| {
                selector(
                    ui,
                    Rect::from_min_max(pos2(10.0, 10.0), pos2(250.0, 100.0)),
                    0.0,
                    self.track,
                    &mut self.state,
                    &self.project,
                    &mut actions,
                );
                lane(
                    ui,
                    Rect::from_min_max(pos2(310.0, 40.0), pos2(890.0, 140.0)),
                    self.track,
                    &self.state,
                    &self.project,
                    &Timeline {
                        left: 310.0,
                        right: 890.0,
                        scroll_tick: 0.0,
                        pixels_per_tick: 0.1,
                        grid: Grid::every(240),
                    },
                    &mut self.editor,
                    &mut actions,
                );
                self.editor.finish(ui, &mut actions);
            },
        );
        self.accessibility = output.platform_output.accesskit_update.take();
        output.textures_delta.clear();
        for action in &actions {
            match action {
                Action::SetAutomation { track, lane } => {
                    let id = self.project.track_ids().nth(*track).unwrap();
                    self.project.set_track_automation(id, lane.clone()).unwrap();
                }
                Action::RemoveAutomation { track, target } => {
                    let id = self.project.track_ids().nth(*track).unwrap();
                    self.project.remove_track_automation(id, *target).unwrap();
                }
                _ => panic!("Unexpected target selector action: {action:?}"),
            }
        }
        actions
    }

    fn click(&mut self, pos: Pos2) -> Vec<Action> {
        self.click_button(pos, egui::PointerButton::Primary)
    }

    fn click_button(&mut self, pos: Pos2, button: egui::PointerButton) -> Vec<Action> {
        assert!(self.step(vec![egui::Event::PointerMoved(pos)]).is_empty());
        let mut actions = Vec::new();
        for pressed in [true, false] {
            actions.extend(self.step(vec![egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            }]));
        }
        actions
    }

    fn labelled(&mut self, label: &str) -> Option<Rect> {
        assert!(self.step(Vec::new()).is_empty());
        let update = self.accessibility.as_ref().expect("an accessibility tree");
        update
            .nodes
            .iter()
            .filter(|(_, node)| {
                node.label()
                    .is_some_and(|text| text.trim_end_matches(['⏵', ' ']) == label)
            })
            .find_map(|(_, node)| node.bounds())
            .map(|bounds| {
                Rect::from_min_max(
                    pos2(bounds.x0 as f32, bounds.y0 as f32),
                    pos2(bounds.x1 as f32, bounds.y1 as f32),
                )
            })
    }

    fn click_label(&mut self, label: &str) {
        let rect = self.labelled(label).unwrap_or_else(|| {
            let labels: Vec<_> = self
                .accessibility
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .filter_map(|(_, node)| node.label())
                .collect();
            panic!("no widget labelled {label:?} among {labels:?}")
        });
        assert!(self.click(rect.center()).is_empty());
        assert!(self.step(Vec::new()).is_empty());
    }

    fn open_group(&mut self, group: &str) {
        let selector = self
            .ctx
            .read_response(Id::new(("automation-target", self.track)))
            .expect("target selector");
        assert!(self.click(selector.rect.center()).is_empty());
        self.click_label(group);
    }

    fn assert_selected_and_add_point(&mut self, expected: AutomationTarget, value: f64) {
        assert!(!egui::Popup::is_id_open(
            &self.ctx,
            Id::new(("automation-target-menu", self.track)),
        ));
        assert_eq!(
            self.state.automation_targets.get(&self.track),
            Some(&expected)
        );
        assert!(self.project.tracks()[self.track].automation().is_empty());
        assert_eq!(self.click(pos2(600.0, 85.0)).len(), 1);
        let lane = &self.project.tracks()[self.track].automation()[0];
        assert_eq!(lane.target(), expected);
        assert_eq!(lane.points().len(), 1);
        assert!((lane.points()[0].value - value).abs() < 1e-7);
    }
}

#[test]
fn automation_target_menu_selects_untouched_mixer_parameters_on_every_track_kind() {
    let mut project = Project::new();
    project.add_audio_track("Audio");
    project.add_midi_track("MIDI");
    project.add_group_track("Group");
    for track in 0..project.tracks().len() {
        let mut harness = SelectorHarness::new(project.clone(), track);
        harness.open_group("Track");
        assert!(harness.labelled("Volume").is_some());
        harness.click_label("Pan");
        harness.assert_selected_and_add_point(AutomationTarget::TrackPan, 0.5);
    }
}

#[test]
fn automation_target_menu_selects_untouched_builtin_instrument_parameter() {
    use sympoie::automation::SubtractiveParameter;
    use sympoie::instrument::{Instrument, SubtractiveSettings};

    let mut project = Project::new();
    let track = project.add_midi_track("Synth");
    let settings = SubtractiveSettings::default();
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(settings)))
        .unwrap();
    let mut harness = SelectorHarness::new(project, 1);
    harness.open_group("Subtractive");
    for parameter in SubtractiveParameter::ALL {
        assert!(harness.labelled(parameter.label()).is_some());
    }
    harness.click_label("Cutoff");
    harness.assert_selected_and_add_point(
        AutomationTarget::Subtractive(SubtractiveParameter::Cutoff),
        SubtractiveParameter::Cutoff.normalized_value(&settings),
    );
}

fn target_menu_plugin(
    name: &str,
    kind: sympoie::plugin::PluginKind,
) -> sympoie::plugin::PluginInstance {
    use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginParameter};

    PluginInstance::new(
        PluginDescriptor {
            path: "/nonexistent-target-menu-fixture.vst3".into(),
            class_id: "0123456789ABCDEFFEDCBA9876543210".into(),
            name: name.into(),
            vendor: "Target menu tests".into(),
            kind,
        },
        vec![
            PluginParameter {
                id: 37,
                name: "Filter Freq".into(),
                unit: "Hz".into(),
                value: 0.37,
                steps: 0,
                read_only: false,
            },
            PluginParameter {
                id: 91,
                name: "Output Meter".into(),
                unit: "dB".into(),
                value: 0.0,
                steps: 0,
                read_only: true,
            },
        ],
        Vec::new(),
    )
}

#[test]
fn automation_target_menu_lists_untouched_plugin_parameters_and_excludes_read_only() {
    use sympoie::plugin::PluginKind;

    let mut project = Project::new();
    let track = project.add_midi_track("Synth");
    project
        .set_plugin_instrument(
            track,
            Some(target_menu_plugin("Synth", PluginKind::Instrument)),
        )
        .unwrap();
    let plugin = project
        .midi_track(track)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    let mut harness = SelectorHarness::new(project, 1);
    harness.open_group("Synth");
    assert!(harness.labelled("Output Meter").is_none());
    harness.click_label("Filter Freq");
    harness.assert_selected_and_add_point(
        AutomationTarget::PluginParameter {
            plugin,
            parameter: 37,
        },
        0.37,
    );
}

#[test]
fn automation_target_menu_distinguishes_repeated_plugin_instances() {
    use sympoie::plugin::PluginKind;

    let mut project = Project::new();
    let track = project.add_audio_track("Audio");
    let mut targets = Vec::new();
    for value in [0.2, 0.8] {
        let mut effect = target_menu_plugin("Echo", PluginKind::Effect);
        effect.parameters[0].value = value;
        let plugin = project.add_effect(track.into(), effect).unwrap();
        targets.push((plugin, value));
    }
    for (index, (plugin, value)) in targets.into_iter().enumerate() {
        let mut harness = SelectorHarness::new(project.clone(), 1);
        harness.open_group(&format!("Echo ({})", index + 1));
        harness.click_label("Filter Freq");
        harness.assert_selected_and_add_point(
            AutomationTarget::PluginParameter {
                plugin,
                parameter: 37,
            },
            value,
        );
    }
}

fn large_target_menu_project() -> (Project, u64) {
    use sympoie::plugin::{PluginKind, PluginParameter};

    let mut project = Project::new();
    let track = project.add_audio_track("Audio");
    let mut effect = target_menu_plugin("Multiband", PluginKind::Effect);
    let mut parameters: Vec<_> = (100..300)
        .map(|id| PluginParameter {
            id,
            name: format!("Band {id}"),
            unit: String::new(),
            value: 0.5,
            steps: 0,
            read_only: false,
        })
        .collect();
    parameters.append(&mut effect.parameters);
    effect.parameters = parameters;
    let plugin = project.add_effect(track.into(), effect).unwrap();
    (project, plugin)
}

#[test]
fn automation_target_menu_search_reaches_parameters_beyond_the_initial_scroll_window() {
    let (project, plugin) = large_target_menu_project();
    let mut harness = SelectorHarness::new(project, 1);
    harness.open_group("Multiband");
    let search = harness
        .ctx
        .read_response(Id::new(("automation-target-search", 1_usize, plugin)))
        .expect("search field");
    assert!(harness.click(search.rect.center()).is_empty());
    assert!(egui::Popup::is_id_open(
        &harness.ctx,
        Id::new(("automation-target-menu", 1_usize)),
    ));
    assert!(
        harness
            .step(vec![egui::Event::Text("fIlTeR".into())])
            .is_empty()
    );
    assert!(harness.labelled("Band 100").is_none());
    harness.click_label("Filter Freq");
    harness.assert_selected_and_add_point(
        AutomationTarget::PluginParameter {
            plugin,
            parameter: 37,
        },
        0.37,
    );
}

#[test]
fn automation_target_menu_scroll_reaches_and_selects_the_last_parameter() {
    let (project, plugin) = large_target_menu_project();
    let mut harness = SelectorHarness::new(project, 1);
    harness.open_group("Multiband");
    assert!(harness.labelled("Filter Freq").is_none());
    let first_row = harness.labelled("Band 100").expect("first parameter");
    assert!(
        harness
            .step(vec![
                egui::Event::PointerMoved(first_row.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, -10_000.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                },
            ])
            .is_empty()
    );
    for _ in 0..12 {
        assert!(harness.step(Vec::new()).is_empty());
    }
    harness.click_label("Filter Freq");
    harness.assert_selected_and_add_point(
        AutomationTarget::PluginParameter {
            plugin,
            parameter: 37,
        },
        0.37,
    );
}

#[test]
fn automation_target_menu_reopening_resets_the_parameter_search() {
    let (project, plugin) = large_target_menu_project();
    let mut harness = SelectorHarness::new(project, 1);
    harness.open_group("Multiband");
    let search = harness
        .ctx
        .read_response(Id::new(("automation-target-search", 1_usize, plugin)))
        .expect("search field");
    assert!(harness.click(search.rect.center()).is_empty());
    assert!(
        harness
            .step(vec![egui::Event::Text("filter".into())])
            .is_empty()
    );
    assert!(harness.labelled("Filter Freq").is_some());
    assert!(harness.labelled("Band 100").is_none());
    assert!(harness.click(pos2(800.0, 400.0)).is_empty());
    assert!(!egui::Popup::is_id_open(
        &harness.ctx,
        Id::new(("automation-target-menu", 1_usize)),
    ));
    harness.open_group("Multiband");
    assert!(harness.labelled("Band 100").is_some());
    assert!(harness.labelled("Filter Freq").is_none());
}

#[test]
fn automation_target_menu_marks_existing_envelopes_and_preserves_context_actions() {
    let mut project = Project::new();
    let track = project.add_audio_track("Audio");
    project
        .set_track_automation(
            track,
            AutomationLane::new(
                AutomationTarget::TrackPan,
                vec![AutomationPoint {
                    tick: 0,
                    value: 0.3,
                }],
            )
            .unwrap(),
        )
        .unwrap();
    let mut harness = SelectorHarness::new(project, 1);
    harness.open_group("Track");
    assert!(harness.labelled("Volume").is_some());
    harness.click_label("● Pan");
    let selector = harness
        .ctx
        .read_response(Id::new(("automation-target", 1_usize)))
        .expect("target selector");
    assert!(
        harness
            .click_button(selector.rect.center(), egui::PointerButton::Secondary)
            .is_empty()
    );
    let disable = harness
        .labelled("Disable Envelope")
        .expect("disable action");
    assert_eq!(harness.click(disable.center()).len(), 1);
    assert!(!harness.project.tracks()[1].automation()[0].enabled());
    assert!(
        harness
            .click_button(selector.rect.center(), egui::PointerButton::Secondary)
            .is_empty()
    );
    assert!(harness.labelled("Enable Envelope").is_some());
    let clear = harness.labelled("Clear Envelope").expect("clear action");
    assert_eq!(harness.click(clear.center()).len(), 1);
    assert!(harness.project.tracks()[1].automation().is_empty());
}

#[test]
fn automation_target_menu_exposes_effects_on_master_and_group_tracks() {
    use sympoie::plugin::PluginKind;

    let mut project = Project::new();
    let group = project.add_group_track("Group");
    for (index, track) in [project.master_track_id().into(), group.into()]
        .into_iter()
        .enumerate()
    {
        let plugin = project
            .add_effect(track, target_menu_plugin("Bus EQ", PluginKind::Effect))
            .unwrap();
        let mut harness = SelectorHarness::new(project.clone(), index);
        harness.open_group("Bus EQ");
        harness.click_label("Filter Freq");
        harness.assert_selected_and_add_point(
            AutomationTarget::PluginParameter {
                plugin,
                parameter: 37,
            },
            0.37,
        );
    }
}
