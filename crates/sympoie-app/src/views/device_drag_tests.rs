use super::*;
use crate::views::device_drag::BrowserDevice;
use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput};
use std::path::PathBuf;
use std::sync::Arc;
use sympoie::plugin::PluginDescriptor;

struct Harness {
    ctx: egui::Context,
    project: Project,
    runtime: Arc<PluginRuntime>,
    selection: Selection,
    actions: Vec<Action>,
    time: f64,
    height: f32,
}

fn descriptor(kind: PluginKind) -> PluginDescriptor {
    PluginDescriptor {
        path: PathBuf::from("/missing-test-plugin.vst3"),
        class_id: "0123456789ABCDEF".to_owned(),
        name: "Same name".to_owned(),
        vendor: "Tests".to_owned(),
        kind,
    }
}

impl Harness {
    fn new(count: usize) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut project = Project::new();
        let midi = project.add_midi_track("Keys");
        project.add_audio_track("Audio");
        project
            .set_plugin_instrument(
                midi,
                Some(PluginInstance::new(
                    descriptor(PluginKind::Instrument),
                    vec![],
                    vec![],
                )),
            )
            .unwrap();
        for _ in 0..count {
            project
                .add_effect(
                    midi.into(),
                    PluginInstance::new(descriptor(PluginKind::Effect), vec![], vec![]),
                )
                .unwrap();
        }
        let mut harness = Self {
            ctx,
            project,
            runtime: PluginRuntime::new(),
            selection: Selection {
                tracks: [1].into(),
                ..Selection::default()
            },
            actions: Vec::new(),
            time: 0.0,
            height: 900.0,
        };
        harness.settle();
        harness
    }

    fn step(&mut self, events: Vec<Event>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, self.height))),
            time: Some(self.time),
            focused: true,
            events,
            ..RawInput::default()
        };
        self.time += 1.0 / 60.0;
        let mut output = self.ctx.run_ui(raw, |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
                pos2(10.0, 10.0),
                pos2(10.0 + super::RACK_WIDTH, self.height - 10.0),
            )));
            show(
                &mut child,
                &self.project,
                &self.selection,
                &self.runtime,
                &mut self.actions,
            );
        });
        output.textures_delta.clear();
    }

    fn settle(&mut self) {
        self.step(vec![]);
        self.step(vec![]);
    }

    fn rect(&self, id: Id) -> Rect {
        self.ctx
            .read_response(id)
            .unwrap_or_else(|| panic!("missing widget {id:?}"))
            .rect
    }

    fn id(&self, position: usize) -> u64 {
        self.project.tracks()[1].effects()[position].id
    }

    fn card(&self, position: usize) -> Rect {
        self.rect(Id::new(("device-vst3", self.id(position) as usize)))
    }

    fn pointer_button(&mut self, position: Pos2, pressed: bool) {
        self.step(vec![Event::PointerButton {
            pos: position,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }

    fn start_drag(&mut self, position: usize) -> Pos2 {
        let source = self
            .rect(Id::new(("device-effect-drag", self.id(position))))
            .center();
        self.step(vec![Event::PointerMoved(source)]);
        self.pointer_button(source, true);
        self.step(vec![Event::PointerMoved(source + vec2(0.0, 12.0))]);
        assert!(egui::DragAndDrop::has_payload_of_type::<DeviceDrag>(
            &self.ctx
        ));
        source
    }

    fn finish_drag(&mut self, target: Pos2) {
        self.step(vec![Event::PointerMoved(target)]);
        self.pointer_button(target, false);
        self.settle();
    }

    fn drop_payload(&mut self, payload: DeviceDrag, target: Pos2) {
        let outside = pos2(2.0, 2.0);
        self.step(vec![Event::PointerMoved(outside)]);
        self.pointer_button(outside, true);
        egui::DragAndDrop::set_payload(&self.ctx, payload);
        self.finish_drag(target);
    }
}

#[test]
fn title_drag_reorders_effects_in_both_directions_and_to_the_tail() {
    for (from, target, to) in [(0, 2, 2), (2, 0, 0), (1, 3, 2)] {
        let mut harness = Harness::new(3);
        let target = if target == 3 {
            harness
                .rect(Id::new(("device-empty-effects", 1_usize)))
                .center()
        } else {
            let rect = harness.card(target);
            pos2(
                rect.center().x,
                if from < target {
                    rect.bottom() - 12.0
                } else {
                    rect.top() + 12.0
                },
            )
        };
        harness.start_drag(from);
        harness.finish_drag(target);
        assert!(
            matches!(harness.actions.as_slice(), [Action::MoveEffect { track: 1, from: actual_from, to: actual_to }] if *actual_from == from && *actual_to == to),
            "{:?}",
            harness.actions
        );
        assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
    }
}

#[test]
fn browser_effect_drop_uses_card_halves_for_first_middle_and_last_gaps() {
    for (card, after, expected) in [
        (0, false, Some(0)),
        (0, true, Some(1)),
        (1, false, Some(1)),
        (2, true, None),
    ] {
        let mut harness = Harness::new(3);
        let rect = harness.card(card);
        let target = pos2(
            rect.center().x,
            if after {
                rect.bottom() - 15.0
            } else {
                rect.top() + 15.0
            },
        );
        let plugin = descriptor(PluginKind::Effect);
        harness.drop_payload(
            DeviceDrag::Browser(BrowserDevice::Plugin(plugin.clone())),
            target,
        );
        match (expected, harness.actions.as_slice()) {
            (
                Some(position),
                [
                    Action::InsertVst3Before {
                        track: 1,
                        plugin: actual,
                        before,
                    },
                ],
            ) => {
                assert_eq!(*before, harness.id(position));
                assert_eq!(actual, &plugin);
            }
            (
                None,
                [
                    Action::LoadVst3 {
                        track: 1,
                        plugin: actual,
                    },
                ],
            ) => assert_eq!(actual, &plugin),
            _ => panic!("unexpected insertion: {:?}", harness.actions),
        }
    }
}

#[test]
fn empty_chain_accepts_effects_and_has_no_add_effect_button() {
    let mut harness = Harness::new(0);
    let tail = harness
        .rect(Id::new(("device-empty-effects", 1_usize)))
        .center();
    harness.drop_payload(
        DeviceDrag::Browser(BrowserDevice::Plugin(descriptor(PluginKind::Effect))),
        tail,
    );
    assert!(matches!(
        harness.actions.as_slice(),
        [Action::LoadVst3 { track: 1, .. }]
    ));
    assert!(
        harness
            .ctx
            .read_response(Id::new(("browse-effects", 1_usize)))
            .is_none()
    );
}

#[test]
fn instruments_replace_on_midi_even_over_effects_but_reject_other_tracks() {
    for track in [1, 2, 0] {
        let mut harness = Harness::new(2);
        harness.selection.tracks = [track].into();
        harness.settle();
        let tail = harness
            .rect(Id::new(("device-empty-effects", track)))
            .center();
        harness.drop_payload(
            DeviceDrag::Browser(BrowserDevice::Plugin(descriptor(PluginKind::Instrument))),
            tail,
        );
        if track == 1 {
            assert!(
                matches!(harness.actions.as_slice(), [Action::LoadVst3 { track: 1, plugin }] if plugin.kind == PluginKind::Instrument)
            );
        } else {
            assert!(harness.actions.is_empty());
        }
    }
    let mut harness = Harness::new(2);
    let target = harness.card(1).center();
    harness.drop_payload(
        DeviceDrag::Browser(BrowserDevice::Plugin(BuiltinKind::Oscil.descriptor())),
        target,
    );
    assert!(matches!(
        harness.actions.as_slice(),
        [Action::LoadVst3 { track: 1, plugin }] if BuiltinKind::from_descriptor(plugin) == Some(BuiltinKind::Oscil)
    ));
}

#[test]
fn no_op_cross_track_missing_and_cancelled_drags_leave_the_chain_alone() {
    for after in [false, true] {
        let mut harness = Harness::new(3);
        let rect = harness.card(1);
        let target = pos2(
            rect.center().x,
            if after {
                rect.bottom() - 10.0
            } else {
                rect.top() + 10.0
            },
        );
        harness.start_drag(1);
        harness.finish_drag(target);
        assert!(harness.actions.is_empty());
    }
    for (track, id) in [(2, 2), (1, 999)] {
        let mut harness = Harness::new(3);
        let target = harness.card(1).center();
        harness.drop_payload(DeviceDrag::Effect { track, id }, target);
        assert!(harness.actions.is_empty());
    }
    let mut harness = Harness::new(3);
    harness.start_drag(0);
    harness.step(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    let target = harness.card(2).center_bottom() - vec2(0.0, 10.0);
    harness.finish_drag(target);
    assert!(harness.actions.is_empty());
}

#[test]
fn drag_outside_the_visible_chain_is_cancelled_and_the_instrument_is_fixed() {
    let mut harness = Harness::new(3);
    let Track::Midi(midi) = &harness.project.tracks()[1] else {
        unreachable!()
    };
    let instrument = midi.plugin_instrument().unwrap().id;
    assert!(
        harness
            .ctx
            .read_response(Id::new(("device-effect-drag", instrument)))
            .is_none()
    );
    harness.start_drag(0);
    harness.finish_drag(pos2(2.0, 400.0));
    assert!(harness.actions.is_empty());
    assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
}

#[test]
fn title_click_and_control_gestures_do_not_reorder_effects() {
    let mut harness = Harness::new(3);
    let title = harness
        .rect(Id::new(("device-effect-drag", harness.id(0))))
        .center();
    harness.step(vec![Event::PointerMoved(title)]);
    harness.pointer_button(title, true);
    harness.pointer_button(title, false);
    assert!(harness.actions.is_empty());

    let card = harness.card(0);
    for start in [card.min + vec2(11.0, 11.0), card.min + vec2(40.0, 35.0)] {
        harness.step(vec![Event::PointerMoved(start)]);
        harness.pointer_button(start, true);
        harness.step(vec![Event::PointerMoved(start + vec2(0.0, 15.0))]);
        assert!(!egui::DragAndDrop::has_payload_of_type::<DeviceDrag>(
            &harness.ctx
        ));
        harness.finish_drag(harness.card(2).center_bottom() - vec2(0.0, 10.0));
        assert!(
            !harness
                .actions
                .iter()
                .any(|action| matches!(action, Action::MoveEffect { .. }))
        );
    }
}

#[test]
fn holding_a_drag_at_the_edge_scrolls_a_long_chain() {
    let mut harness = Harness::new(8);
    harness.height = 480.0;
    harness.settle();
    let before = harness.card(0).top();
    harness.start_drag(0);
    let chain = harness.rect(Id::new(("device-chain", 1_usize)));
    harness.step(vec![Event::PointerMoved(pos2(
        chain.center().x,
        chain.bottom() - 6.0,
    ))]);
    for _ in 0..90 {
        harness.step(vec![]);
    }
    assert!(
        harness.card(0).top() < before - 200.0,
        "drag should reveal later effects"
    );
    assert!(egui::DragAndDrop::has_payload_of_type::<DeviceDrag>(
        &harness.ctx
    ));
    assert!(harness.actions.is_empty());
}

#[test]
fn builtin_modules_fit_the_rack_width_and_focus_reveals_appended_effect() {
    for kind in BuiltinKind::ALL {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut project = Project::new();
        let track = project.add_midi_track("Built-ins");
        let id = if kind.descriptor().kind == PluginKind::Instrument {
            project
                .set_plugin_instrument(track, Some(kind.instance()))
                .unwrap();
            let Track::Midi(midi) = &project.tracks()[1] else {
                unreachable!()
            };
            midi.plugin_instrument().unwrap().id
        } else {
            for _ in 0..3 {
                project
                    .add_effect(track.into(), BuiltinKind::Eq.instance())
                    .unwrap();
            }
            project.add_effect(track.into(), kind.instance()).unwrap()
        };
        let runtime = PluginRuntime::new();
        let mut actions = Vec::new();
        ctx.data_mut(|data| data.insert_temp(Id::new("builtin-chain-focus"), id));
        for frame in 0..25 {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        Pos2::ZERO,
                        vec2(super::RACK_WIDTH, 600.0),
                    )),
                    time: Some(frame as f64 / 60.0),
                    ..Default::default()
                },
                |ui| {
                    show(
                        ui,
                        &project,
                        &Selection {
                            tracks: [1].into(),
                            ..Default::default()
                        },
                        &runtime,
                        &mut actions,
                    );
                },
            );
            output.textures_delta.clear();
        }
        let rect = ctx
            .read_response(Id::new(("device-vst3", id as usize)))
            .unwrap()
            .rect;
        assert!(
            rect.height() <= PANEL_HEIGHT - 6.0,
            "{} module grows beyond its height budget: {rect:?}",
            kind.name()
        );
        assert!(
            rect.left() >= 0.0 && rect.right() <= super::RACK_WIDTH,
            "{} module is wider than the rack: {rect:?}",
            kind.name()
        );
        assert!(
            rect.top() >= 0.0 && rect.bottom() <= 600.0,
            "focused {} module remains offscreen: {rect:?}",
            kind.name()
        );
        assert!(
            ctx.data_mut(|data| data.get_temp::<u64>(Id::new("builtin-chain-focus")))
                .is_none()
        );
        assert!(actions.is_empty());
    }
}

#[test]
fn compact_chain_reserves_each_card_before_the_drop_zone() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let mut project = Project::new();
    let track = project.add_midi_track("Compact chain");
    project
        .set_plugin_instrument(track, Some(BuiltinKind::Oscil.instance()))
        .unwrap();
    project
        .add_effect(track.into(), BuiltinKind::Eq.instance())
        .unwrap();
    let comp = project
        .add_effect(track.into(), BuiltinKind::Comp.instance())
        .unwrap();
    let runtime = PluginRuntime::new();
    ctx.data_mut(|data| data.insert_temp(Id::new("builtin-chain-focus"), comp));
    for frame in 0..25 {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    vec2(super::RACK_WIDTH, 900.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            },
            |ui| {
                show(
                    ui,
                    &project,
                    &Selection {
                        tracks: [1].into(),
                        ..Default::default()
                    },
                    &runtime,
                    &mut Vec::new(),
                )
            },
        );
        output.textures_delta.clear();
    }
    let card = ctx
        .read_response(Id::new(("device-vst3", comp as usize)))
        .unwrap()
        .rect;
    let drop = ctx
        .read_response(Id::new(("device-empty-effects", 1_usize)))
        .unwrap()
        .rect;
    assert!(card.width() >= builtin_devices::card_width(BuiltinKind::Comp));
    assert!(
        drop.top() >= card.bottom(),
        "drop zone {drop:?} overlaps Comp {card:?}"
    );
    for parameter in 0..=10 {
        let control = ctx
            .read_response(Id::new(("builtin-inline-param", comp, parameter)))
            .unwrap()
            .rect;
        assert!(
            card.contains_rect(control),
            "parameter {parameter}: {control:?} outside {card:?}"
        );
    }
}
