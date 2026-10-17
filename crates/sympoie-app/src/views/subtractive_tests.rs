use super::*;
use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput};
use std::sync::Arc;

struct Harness {
    ctx: egui::Context,
    project: Project,
    runtime: Arc<PluginRuntime>,
    actions: Vec<Action>,
    editor: bool,
    time: f64,
}

impl Harness {
    fn new(editor: bool, instrument: bool, bypassed: bool) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut project = Project::new();
        let track = project.add_midi_track("Keys");
        if instrument {
            project
                .set_midi_track_instrument(
                    track,
                    Some(Instrument::Subtractive(SubtractiveSettings::default())),
                )
                .unwrap();
            project
                .set_midi_track_instrument_bypassed(track, bypassed)
                .unwrap();
        }
        let mut harness = Self {
            ctx,
            project,
            runtime: PluginRuntime::new(),
            actions: Vec::new(),
            editor,
            time: 0.0,
        };
        harness.step(vec![]);
        harness.step(vec![]);
        harness
    }

    fn step(&mut self, events: Vec<Event>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 500.0))),
            time: Some(self.time),
            focused: true,
            events,
            ..RawInput::default()
        };
        self.time += 1.0 / 60.0;
        let mut output = self.ctx.run_ui(raw, |ui| {
            if self.editor {
                let Track::Midi(track) = &self.project.tracks()[1] else {
                    unreachable!();
                };
                let Some(Instrument::Subtractive(settings)) = track.instrument() else {
                    unreachable!();
                };
                editor_contents(
                    ui,
                    1,
                    settings,
                    track.instrument_bypassed(),
                    &mut self.actions,
                );
            } else {
                show(
                    ui,
                    &self.project,
                    &Selection {
                        tracks: [1].into(),
                        ..Selection::default()
                    },
                    &self.runtime,
                    &mut self.actions,
                );
            }
        });
        output.textures_delta.clear();
    }

    fn click(&mut self, name: &'static str) {
        let position = self
            .ctx
            .read_response(Id::new((name, 1_usize)))
            .unwrap_or_else(|| panic!("missing {name}"))
            .rect
            .center();
        self.step(vec![Event::PointerMoved(position)]);
        for pressed in [true, false] {
            self.step(vec![Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
}

#[test]
fn card_and_editor_activators_toggle_in_both_directions() {
    for editor in [false, true] {
        for bypassed in [false, true] {
            let mut harness = Harness::new(editor, true, bypassed);
            harness.click(if editor {
                "editor-instrument-activator"
            } else {
                "device-instrument-activator"
            });
            assert!(matches!(
                harness.actions.as_slice(),
                [Action::SetInstrumentBypassed { track: 1, bypassed: next }]
                    if *next == !bypassed
            ));
            assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
        }
    }
}

#[test]
fn bypassed_subtractive_keeps_editor_and_removal_available() {
    for (name, editor) in [("open-plugin-editor", true), ("remove-plugin", false)] {
        let mut harness = Harness::new(false, true, true);
        harness.click(name);
        if editor {
            assert!(matches!(
                harness.actions.as_slice(),
                [Action::OpenPluginEditor(1)]
            ));
        } else {
            assert!(matches!(
                harness.actions.as_slice(),
                [Action::SetInstrument {
                    track: 1,
                    instrument: None
                }]
            ));
        }
    }
    let harness = Harness::new(true, true, true);
    for parameter in ["waveform", "cutoff", "resonance", "attack", "gain"] {
        assert!(
            harness
                .ctx
                .read_response(Id::new(("plugin-parameter", 1_usize, parameter)))
                .unwrap()
                .enabled()
        );
    }
}

#[test]
fn empty_instrument_slot_has_no_activator_and_can_add_oscil() {
    let mut harness = Harness::new(false, false, false);
    assert!(
        harness
            .ctx
            .read_response(Id::new(("device-instrument-activator", 1_usize)))
            .is_none()
    );
    harness.click("add-instrument");
    assert!(matches!(
        harness.actions.as_slice(),
        [Action::LoadVst3 { track: 1, plugin }] if BuiltinKind::from_descriptor(plugin) == Some(BuiltinKind::Oscil)
    ));
}
