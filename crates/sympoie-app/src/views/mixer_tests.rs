use super::*;
use crate::document::edit;
use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput};
use std::collections::HashSet;
use std::path::PathBuf;
use sympoie::plugin::PluginDescriptor;
use sympoie::track::{BusTrackId, TrackId};

struct Harness {
    ctx: egui::Context,
    project: Project,
    selection: Selection,
    folded: HashSet<usize>,
    actions: Vec<Action>,
    meter: MeterState,
    time: f64,
    height: f32,
    cursor: egui::CursorIcon,
}

fn descriptor(kind: PluginKind) -> PluginDescriptor {
    PluginDescriptor {
        path: PathBuf::from("/missing-mixer-test.vst3"),
        class_id: "0123456789ABCDEF".to_owned(),
        name: "Channel EQ".to_owned(),
        vendor: "Tests".to_owned(),
        kind,
    }
}

impl Harness {
    fn new(effects: usize) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut project = Project::new();
        let audio = project.add_audio_track("Audio");
        project.add_midi_track("Keys");
        project.add_group_track("Group");
        project.add_bus_track("Mix bus");
        for _ in 0..effects {
            project
                .add_mixer_effect(
                    audio.into(),
                    PluginInstance::new(descriptor(PluginKind::Effect), vec![], vec![]),
                )
                .unwrap();
        }
        let mut harness = Self {
            ctx,
            project,
            selection: Selection::default(),
            folded: HashSet::new(),
            actions: Vec::new(),
            meter: MeterState::default(),
            time: 0.0,
            height: 430.0,
            cursor: egui::CursorIcon::Default,
        };
        harness.settle();
        harness
    }

    fn step(&mut self, events: Vec<Event>) {
        self.step_modifiers(events, Modifiers::NONE);
    }

    fn step_modifiers(&mut self, mut events: Vec<Event>, modifiers: Modifiers) {
        events.insert(0, Event::ModifiersChanged(modifiers));
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, self.height))),
            time: Some(self.time),
            focused: true,
            events,
            ..RawInput::default()
        };
        self.time += 1.0 / 60.0;
        let previous = self.actions.len();
        let mut output = self.ctx.run_ui(input, |ui| {
            let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(
                pos2(8.0, 8.0),
                pos2(892.0, self.height - 8.0),
            )));
            show(
                &mut child,
                &self.project,
                &self.selection,
                &self.folded,
                &mut self.meter,
                &mut self.actions,
            );
        });
        self.cursor = output.platform_output.cursor_icon;
        output.textures_delta.clear();
        for action in &self.actions[previous..] {
            match action {
                Action::SetMixer { track, mixer } => {
                    edit::set_mixer(&mut self.project, *track, *mixer).unwrap();
                }
                Action::SetTrackSend { track, bus, gain } => {
                    let source = self.project.track_ids().nth(*track).unwrap();
                    let bus = bus_id(&self.project, *bus);
                    self.project.set_track_send(source, bus, *gain).unwrap();
                }
                Action::RemoveTrackSend { track, bus } => {
                    let source = self.project.track_ids().nth(*track).unwrap();
                    let bus = bus_id(&self.project, *bus);
                    self.project.remove_track_send(source, bus).unwrap();
                }
                _ => {}
            }
        }
    }

    fn settle(&mut self) {
        self.step(vec![]);
        self.step(vec![]);
    }

    fn rect(&self, id: Id) -> Rect {
        self.ctx
            .read_response(id)
            .unwrap_or_else(|| panic!("missing {id:?}"))
            .rect
    }

    fn center(&self, kind: &'static str, track: usize) -> Pos2 {
        self.rect(Id::new((kind, track))).center()
    }

    fn pointer(&mut self, pos: Pos2, pressed: bool) {
        self.step(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }

    fn click(&mut self, pos: Pos2) {
        self.step(vec![Event::PointerMoved(pos)]);
        self.pointer(pos, true);
        self.pointer(pos, false);
    }

    fn secondary_click(&mut self, pos: Pos2) {
        self.step(vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.step(vec![Event::PointerButton {
                pos,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        self.settle();
    }

    fn key(&mut self, key: egui::Key, modifiers: Modifiers) {
        for pressed in [true, false] {
            self.step_modifiers(
                vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
            );
        }
    }

    fn effect(&self, position: usize) -> u64 {
        self.project.tracks()[1].mixer_effects()[position].id
    }

    fn enter_gain(&mut self, track: usize, text: &str) {
        self.time += 1.0;
        let value = self.center("mixer-gain", track);
        self.click(value);
        self.click(value);
        self.key(egui::Key::A, Modifiers::COMMAND);
        self.key(egui::Key::Backspace, Modifiers::NONE);
        self.step(vec![Event::Text(text.to_owned())]);
        self.key(egui::Key::Enter, Modifiers::NONE);
        self.settle();
    }

    fn add_send(&mut self, source: usize, bus: usize) {
        self.click(self.center("mixer-send-add", source));
        self.settle();
        self.click(
            self.rect(Id::new(("mixer-send-target", source, bus)))
                .center(),
        );
        self.settle();
    }

    fn send_gain(&self, source: usize, bus: usize) -> Option<f32> {
        self.project.tracks()[source]
            .sends()
            .iter()
            .find(|send| send.destination == bus_id(&self.project, bus))
            .map(|send| send.gain)
    }

    fn drop_browser(&mut self, device: BrowserDevice, destination: Pos2) {
        let outside = pos2(2.0, 2.0);
        self.step(vec![Event::PointerMoved(outside)]);
        self.pointer(outside, true);
        egui::DragAndDrop::set_payload(&self.ctx, DeviceDrag::Browser(device));
        self.step(vec![Event::PointerMoved(destination)]);
        self.pointer(destination, false);
        self.settle();
    }
}

fn bus_id(project: &Project, index: usize) -> BusTrackId {
    let TrackId::Bus(bus) = project.track_ids().nth(index).expect("track exists") else {
        panic!("not a bus");
    };
    bus
}

#[test]
fn every_channel_has_inserts_and_the_fader_fits_the_minimum_panel_height() {
    let mut harness = Harness::new(6);
    harness.height = 330.0;
    harness.settle();
    for track in 0..harness.project.tracks().len() {
        let strip = harness.rect(Id::new(("mixer-strip", track)));
        let fader = harness.rect(Id::new(("mixer-fader", track)));
        let plus = harness.rect(Id::new(("mixer-effect-plus", track)));
        assert!(
            strip.contains_rect(fader),
            "fader {fader:?} outside {strip:?}"
        );
        assert!(strip.contains_rect(plus));
        assert!(fader.height() >= 42.0);
    }
    assert!(
        !rows::display_rows(&harness.project, &HashSet::new())
            .iter()
            .any(|row| row.index == 4)
    );
}

#[test]
fn add_bus_and_effect_buttons_address_the_clicked_channel() {
    let mut harness = Harness::new(0);
    harness.click(harness.rect(Id::new("mixer-add-bus")).center());
    assert!(
        harness
            .actions
            .iter()
            .any(|action| matches!(action, Action::AddBusTrack))
    );
    for track in [0, 1, 2, 3, 4] {
        harness.actions.clear();
        harness.click(harness.center("mixer-effect-add", track));
        assert!(harness.actions.iter().any(|action| matches!(action, Action::BrowseMixerEffects { track: actual } if *actual == track)));
    }
}

#[test]
fn insert_click_opens_its_editor_and_activator_only_toggles_that_insert() {
    let mut harness = Harness::new(2);
    let id = harness.effect(1);
    harness.click(harness.rect(Id::new(("mixer-effect", id))).center());
    assert!(
        harness
            .actions
            .iter()
            .any(|action| matches!(action, Action::OpenNativeVst3Editor(actual) if *actual == id))
    );
    harness.actions.clear();
    harness.click(harness.rect(Id::new(("mixer-effect-bypass", id))).center());
    assert!(
        matches!(harness.actions.as_slice(), [Action::SetVst3Bypassed { id: actual, bypassed: true }] if *actual == id)
    );
}

#[test]
fn mixer_effect_drag_reorders_within_the_chain_and_rejects_other_channels() {
    for (from, to) in [(0, 2), (2, 0)] {
        let mut harness = Harness::new(3);
        harness.height = 560.0;
        harness.settle();
        let source = harness
            .rect(Id::new(("mixer-effect", harness.effect(from))))
            .center();
        let destination = if to == 0 {
            let rect = harness.rect(Id::new(("mixer-effect", harness.effect(0))));
            pos2(rect.center().x, rect.top() + 2.0)
        } else {
            harness.center("mixer-effect-add", 1)
        };
        harness.step(vec![Event::PointerMoved(source)]);
        harness.pointer(source, true);
        harness.step(vec![Event::PointerMoved(source + vec2(8.0, 0.0))]);
        assert!(egui::DragAndDrop::has_payload_of_type::<MixerEffectDrag>(
            &harness.ctx
        ));
        harness.step(vec![Event::PointerMoved(destination)]);
        harness.pointer(destination, false);
        assert!(harness.actions.iter().any(|action| matches!(action, Action::MoveMixerEffect {track: 1, from: actual_from, to: actual_to} if *actual_from == from && *actual_to == to)));
        assert!(
            !harness
                .actions
                .iter()
                .any(|action| matches!(action, Action::OpenNativeVst3Editor(_)))
        );
    }
    let mut harness = Harness::new(1);
    let outside = pos2(2.0, 2.0);
    harness.pointer(outside, true);
    egui::DragAndDrop::set_payload(
        &harness.ctx,
        MixerEffectDrag {
            track: 1,
            id: harness.effect(0),
        },
    );
    let target = harness.center("mixer-effect-add", 4);
    harness.step(vec![Event::PointerMoved(target)]);
    assert_eq!(harness.cursor, egui::CursorIcon::NotAllowed);
    harness.pointer(target, false);
    assert!(
        !harness
            .actions
            .iter()
            .any(|action| matches!(action, Action::MoveMixerEffect { .. }))
    );
}

#[test]
fn browser_effect_drops_target_mixer_slots_and_instruments_are_rejected() {
    for track in [0, 1, 2, 3, 4] {
        let mut harness = Harness::new(1);
        let destination = if track == 1 {
            let slot = harness.rect(Id::new(("mixer-effect", harness.effect(0))));
            pos2(slot.center().x, slot.top() + 2.0)
        } else {
            harness.center("mixer-effect-add", track)
        };
        harness.drop_browser(
            BrowserDevice::Plugin(descriptor(PluginKind::Effect)),
            destination,
        );
        assert!(harness.actions.iter().any(|action| matches!(action, Action::LoadMixerVst3 { track: actual, before, .. } if *actual == track && *before == (track == 1).then(|| harness.effect(0)))));
    }
    for device in [
        BrowserDevice::Plugin(sympoie::builtin::BuiltinKind::Oscil.descriptor()),
        BrowserDevice::Plugin(descriptor(PluginKind::Instrument)),
    ] {
        let mut harness = Harness::new(0);
        harness.drop_browser(device, harness.center("mixer-effect-add", 2));
        assert!(!harness.actions.iter().any(|action| matches!(
            action,
            Action::LoadMixerVst3 { .. } | Action::LoadVst3 { .. } | Action::SetInstrument { .. }
        )));
    }
}

#[test]
fn mute_and_solo_preserve_channel_volume_pan_and_bus_routing() {
    let mut harness = Harness::new(0);
    let bus = harness.project.track_ids().nth(4).unwrap();
    let sympoie::track::TrackId::Bus(bus) = bus else {
        panic!("bus")
    };
    let mixer = MixerSettings::new(0.6, -0.3, sympoie::track::AudioOutput::Bus(bus)).unwrap();
    edit::set_mixer(&mut harness.project, 1, mixer).unwrap();
    harness.settle();
    harness.click(harness.center("mixer-mute", 1));
    harness.click(harness.center("mixer-solo", 1));
    let current = harness.project.tracks()[1].mixer();
    assert!(current.muted() && current.soloed());
    assert_eq!(current.gain(), mixer.gain());
    assert_eq!(current.pan(), mixer.pan());
    assert_eq!(current.output(), mixer.output());
}

#[test]
fn channel_gain_entry_preserves_other_settings_and_channels() {
    let mut harness = Harness::new(1);
    let bus = bus_id(&harness.project, 4);
    let mixer = MixerSettings::new(0.6, -0.3, sympoie::track::AudioOutput::Bus(bus))
        .unwrap()
        .with_muted(true)
        .with_soloed(true);
    edit::set_mixer(&mut harness.project, 1, mixer).unwrap();
    harness.settle();
    let unchanged: Vec<_> = harness
        .project
        .tracks()
        .iter()
        .map(|track| *track.mixer())
        .collect();
    let first_action = harness.actions.len();
    harness.enter_gain(1, "-12.5 dB");
    let current = *harness.project.tracks()[1].mixer();
    assert!((current.gain() - format::db_to_gain(-12.5)).abs() < 1e-6);
    assert_eq!(current.pan(), mixer.pan());
    assert_eq!(current.output(), mixer.output());
    assert!(current.muted() && current.soloed());
    assert_eq!(harness.project.tracks()[1].mixer_effects().len(), 1);
    for (index, mixer) in unchanged.into_iter().enumerate() {
        if index != 1 {
            assert_eq!(*harness.project.tracks()[index].mixer(), mixer);
        }
    }
    assert!(harness.actions[first_action..].iter().any(|action| {
        matches!(action, Action::SetMixer { track: 1, mixer } if *mixer == current)
    }));

    harness.enter_gain(0, "-3");
    assert!((harness.project.tracks()[0].mixer().gain() - format::db_to_gain(-3.0)).abs() < 1e-6);
    assert_eq!(*harness.project.tracks()[1].mixer(), current);
}

#[test]
fn channel_gain_entry_clamps_and_rejects_non_finite_or_invalid_values() {
    let mut harness = Harness::new(0);
    for (text, expected) in [
        ("+6 dB", format::db_to_gain(6.0)),
        ("999", format::MAX_FADER_GAIN),
        ("-999", 0.0),
        ("−∞ dB", 0.0),
        ("-inf", 0.0),
        ("-infinity", 0.0),
        ("0", 1.0),
    ] {
        harness.enter_gain(1, text);
        let actual = harness.project.tracks()[1].mixer().gain();
        assert!(
            (actual - expected).abs() < 1e-6,
            "{text}: {actual} != {expected}"
        );
    }
    for text in ["NaN", "inf", "+inf", "1e999", "not a level", ""] {
        let first_action = harness.actions.len();
        harness.enter_gain(1, text);
        assert_eq!(harness.project.tracks()[1].mixer().gain(), 1.0, "{text}");
        assert!(
            !harness.actions[first_action..]
                .iter()
                .any(|action| matches!(action, Action::SetMixer { .. })),
            "invalid value {text} emitted a mixer edit"
        );
    }
}

#[test]
fn channel_gain_readout_drag_changes_only_its_channel() {
    let mut harness = Harness::new(0);
    let other = *harness.project.tracks()[2].mixer();
    let center = harness.center("mixer-gain", 1);
    harness.step(vec![Event::PointerMoved(center)]);
    harness.pointer(center, true);
    let lower = center - vec2(25.0, 0.0);
    harness.step(vec![Event::PointerMoved(lower)]);
    harness.pointer(lower, false);
    assert!(harness.project.tracks()[1].mixer().gain() < 1.0);
    assert_eq!(*harness.project.tracks()[2].mixer(), other);
}

#[test]
fn channel_gain_entry_waits_for_commit_and_escape_cancels() {
    let mut harness = Harness::new(0);
    let value = harness.center("mixer-gain", 1);
    harness.click(value);
    harness.click(value);
    harness.key(egui::Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Text("-24".to_owned())]);
    assert_eq!(harness.project.tracks()[1].mixer().gain(), 1.0);
    harness.key(egui::Key::Escape, Modifiers::NONE);
    harness.settle();
    assert_eq!(harness.project.tracks()[1].mixer().gain(), 1.0);
    assert!(
        !harness
            .actions
            .iter()
            .any(|action| matches!(action, Action::SetMixer { .. }))
    );
}

#[test]
fn list_frames_leave_padding_around_insert_and_send_controls() {
    let mut harness = Harness::new(1);
    let source = harness.project.track_ids().nth(1).unwrap();
    let bus = bus_id(&harness.project, 4);
    harness
        .project
        .set_track_send(source, bus, format::db_to_gain(-69.9))
        .unwrap();
    harness.settle();
    let inserts = harness.rect(Id::new(("mixer-insert-list", 1_usize)));
    let slot = harness.rect(Id::new(("mixer-effect", harness.effect(0))));
    let add = harness.rect(Id::new(("mixer-effect-add", 1_usize)));
    let sends = harness.rect(Id::new(("mixer-sends", 1_usize)));
    let level = harness.rect(Id::new(("mixer-send-level", 1_usize, 4_usize)));
    let remove = harness.rect(Id::new(("mixer-send-remove", 1_usize, 4_usize)));
    for (frame, content) in [
        (inserts, slot),
        (inserts, add),
        (sends, level),
        (sends, remove),
    ] {
        assert!(
            frame.shrink(LIST_PADDING - 0.5).contains_rect(content),
            "{content:?} reaches the list frame {frame:?}"
        );
    }
    assert!(
        level.right() <= remove.left(),
        "negative send value overlaps its remove control"
    );
}

#[test]
fn analog_fader_drag_keeps_other_settings_and_double_click_restores_unity() {
    let mut harness = Harness::new(0);
    let initial = *harness.project.tracks()[1].mixer();
    edit::set_mixer(
        &mut harness.project,
        1,
        initial.with_muted(true).with_soloed(true),
    )
    .unwrap();
    let center = harness.center("mixer-fader", 1);
    harness.step(vec![Event::PointerMoved(center)]);
    harness.pointer(center, true);
    let lower = center + vec2(0.0, 30.0);
    harness.step(vec![Event::PointerMoved(lower)]);
    harness.pointer(lower, false);
    let changed = *harness.project.tracks()[1].mixer();
    assert!(changed.gain() < 1.0);
    assert!(changed.muted() && changed.soloed());
    assert_eq!(changed.output(), initial.output());
    harness.time += 1.0;
    harness.click(center);
    harness.click(center);
    assert_eq!(harness.project.tracks()[1].mixer().gain(), 1.0);
}

#[test]
fn analog_fader_supports_fine_dragging_and_clamps_its_range() {
    let drag = |distance: f32, modifiers| {
        let mut harness = Harness::new(0);
        let center = harness.center("mixer-fader", 1);
        harness.step(vec![Event::PointerMoved(center)]);
        harness.pointer(center, true);
        let destination = center + vec2(0.0, distance);
        harness.step_modifiers(vec![Event::PointerMoved(destination)], modifiers);
        harness.pointer(destination, false);
        harness.project.tracks()[1].mixer().gain()
    };
    assert!(drag(25.0, Modifiers::SHIFT) > drag(25.0, Modifiers::NONE));
    assert_eq!(drag(1000.0, Modifiers::NONE), 0.0);
    assert_eq!(drag(-1000.0, Modifiers::NONE), format::MAX_FADER_GAIN);
}

#[test]
fn bus_channel_context_menu_allows_typing_a_name_and_deleting_the_channel() {
    let mut harness = Harness::new(0);
    harness.secondary_click(harness.center("mixer-title", 4));
    harness.click(
        harness
            .rect(Id::new(("mixer-channel-name", 4_usize)))
            .center(),
    );
    assert!(egui::Popup::is_id_open(
        &harness.ctx,
        Id::new(("mixer-channel-menu", 4_usize))
    ));
    harness.key(egui::Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Text("Drum bus".to_owned())]);
    harness.key(egui::Key::Enter, Modifiers::NONE);
    assert!(
        harness
            .actions
            .iter()
            .any(|action| matches!(action, Action::RenameTrack(4, name) if name == "Drum bus"))
    );
    harness.time += 1.0;
    harness.secondary_click(harness.center("mixer-title", 4));
    harness.click(harness.center("mixer-delete-channel", 4));
    assert!(
        harness
            .actions
            .iter()
            .any(|action| matches!(action, Action::DeleteTrack(4)))
    );
}

#[test]
fn sends_add_independent_bus_levels_without_changing_the_main_output_or_inserts() {
    let mut harness = Harness::new(1);
    harness.project.add_bus_track("Delay bus");
    harness.settle();
    let original = *harness.project.tracks()[1].mixer();
    harness.add_send(1, 4);
    harness.add_send(1, 5);
    assert_eq!(harness.send_gain(1, 4), Some(0.0));
    assert_eq!(harness.send_gain(1, 5), Some(0.0));
    let level = harness
        .rect(Id::new(("mixer-send-level", 1_usize, 4_usize)))
        .center();
    let field = harness.rect(Id::new(("mixer-send-level", 1_usize, 4_usize)));
    let remove = harness.rect(Id::new(("mixer-send-remove", 1_usize, 4_usize)));
    assert!(
        field.right() <= remove.left(),
        "send field {field:?} overlaps remove {remove:?}"
    );
    harness.time += 1.0;
    harness.click(level);
    harness.click(level);
    harness.key(egui::Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Text("-6".to_owned())]);
    harness.key(egui::Key::Enter, Modifiers::NONE);
    assert!((harness.send_gain(1, 4).unwrap() - format::db_to_gain(-6.0)).abs() < 1e-6);
    assert_eq!(harness.send_gain(1, 5), Some(0.0));
    assert_eq!(harness.project.tracks()[1].mixer(), &original);
    assert_eq!(harness.project.tracks()[1].mixer_effects().len(), 1);
    harness.click(
        harness
            .rect(Id::new(("mixer-send-remove", 1_usize, 4_usize)))
            .center(),
    );
    assert_eq!(harness.send_gain(1, 4), None);
    assert_eq!(harness.send_gain(1, 5), Some(0.0));
    assert!(
        harness
            .ctx
            .read_response(Id::new(("mixer-send-add", 0_usize)))
            .is_none()
    );
}

#[test]
fn send_menu_excludes_self_existing_sends_and_feedback_paths() {
    let mut harness = Harness::new(0);
    let downstream = harness.project.add_bus_track("Downstream");
    harness.project.add_bus_track("Safe destination");
    let upstream = bus_id(&harness.project, 4);
    harness
        .project
        .set_track_send(upstream, downstream, 0.0)
        .unwrap();
    harness.settle();
    harness.click(harness.center("mixer-send-add", 5));
    harness.settle();
    for forbidden in [4_usize, 5] {
        assert!(
            harness
                .ctx
                .read_response(Id::new(("mixer-send-target", 5_usize, forbidden)))
                .is_none()
        );
    }
    assert!(
        harness
            .ctx
            .read_response(Id::new(("mixer-send-target", 5_usize, 6_usize)))
            .is_some()
    );
    harness.key(egui::Key::Escape, Modifiers::NONE);
    harness.click(harness.center("mixer-send-add", 4));
    harness.settle();
    assert!(
        harness
            .ctx
            .read_response(Id::new(("mixer-send-target", 4_usize, 5_usize)))
            .is_none()
    );
}

#[test]
fn sends_and_inserts_scroll_independently_without_pushing_the_fader_outside_a_small_panel() {
    let mut harness = Harness::new(6);
    let source = harness.project.track_ids().nth(1).unwrap();
    for name in ["Reverb", "Delay", "Parallel compression"] {
        let bus = harness.project.add_bus_track(name);
        harness.project.set_track_send(source, bus, 0.5).unwrap();
    }
    harness.height = 330.0;
    harness.settle();
    let strip = harness.rect(Id::new(("mixer-strip", 1_usize)));
    let sends = harness.rect(Id::new(("mixer-sends", 1_usize)));
    let fader = harness.rect(Id::new(("mixer-fader", 1_usize)));
    assert!(strip.contains_rect(sends));
    assert!(
        strip.contains_rect(fader),
        "fader {fader:?} outside {strip:?}"
    );
    let insert_before = harness.rect(Id::new(("mixer-effect", harness.effect(0))));
    harness.step(vec![
        Event::PointerMoved(sends.center()),
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: vec2(0.0, -500.0),
            modifiers: Modifiers::NONE,
        },
    ]);
    for _ in 0..20 {
        harness.step(vec![]);
    }
    let last_send = harness.rect(Id::new(("mixer-send-level", 1_usize, 7_usize)));
    assert!(sends.intersects(last_send));
    assert_eq!(
        insert_before,
        harness.rect(Id::new(("mixer-effect", harness.effect(0))))
    );
    assert_eq!(fader, harness.rect(Id::new(("mixer-fader", 1_usize))));
}
