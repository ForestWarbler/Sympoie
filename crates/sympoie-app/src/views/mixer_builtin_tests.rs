use super::*;
use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

struct Harness {
    ctx: egui::Context,
    project: Project,
    selection: Selection,
    actions: Vec<Action>,
    time: f64,
    id: u64,
}

impl Harness {
    fn new(kind: BuiltinKind) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut project = Project::new();
        let track = project.add_audio_track("Audio");
        project.add_audio_track("Other");
        let id = project
            .add_mixer_effect(track.into(), kind.instance())
            .unwrap();
        let mut result = Self {
            ctx,
            project,
            selection: Selection::default(),
            actions: Vec::new(),
            time: 0.0,
            id,
        };
        result.settle();
        result
    }

    fn step(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 350.0))),
            time: Some(self.time),
            focused: true,
            events,
            ..RawInput::default()
        };
        self.time += 1.0 / 60.0;
        let start = self.actions.len();
        let mut output = self.ctx.run_ui(input, |ui| {
            let mut child = ui.new_child(
                UiBuilder::new().max_rect(Rect::from_min_max(pos2(8.0, 8.0), pos2(992.0, 342.0))),
            );
            show(
                &mut child,
                &self.project,
                &self.selection,
                &Default::default(),
                &mut MeterState::default(),
                &mut self.actions,
            );
        });
        output.textures_delta.clear();
        for action in &self.actions[start..] {
            match action {
                Action::SelectTracks(tracks) => self.selection.tracks = tracks.clone(),
                Action::SetVst3Bypassed { id, bypassed } => {
                    self.project.set_plugin_bypassed(*id, *bypassed).unwrap()
                }
                Action::SetVst3Parameter {
                    id,
                    parameter,
                    value,
                } => self
                    .project
                    .set_plugin_parameter(*id, *parameter, *value)
                    .unwrap(),
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
    fn click(&mut self, at: Pos2, button: PointerButton) {
        self.step(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.step(vec![Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
    fn open(&mut self) {
        let at = self.rect(Id::new(("mixer-effect", self.id))).center();
        self.click(at, PointerButton::Primary);
        self.click(at, PointerButton::Primary);
        self.settle();
    }
    fn assert_no_editor(&self) {
        assert!(!self.actions.iter().any(|action| matches!(
            action,
            Action::OpenNativeVst3Editor(_)
                | Action::OpenVst3Editor(_)
                | Action::OpenVst3Sidechain(_)
        )));
    }
}

#[test]
fn builtins_open_inside_mixer_and_leave_channel_faders_accessible() {
    for kind in [BuiltinKind::Eq, BuiltinKind::Comp] {
        let mut h = Harness::new(kind);
        h.open();
        h.assert_no_editor();
        assert_eq!(
            h.ctx.data(|data| data.get_temp::<u64>(inspector_id())),
            Some(h.id)
        );
        assert_eq!(h.selection.tracks, BTreeSet::from([1]));
        let panel = h.rect(Id::new("mixer-builtin-panel"));
        assert!(panel.height() <= crate::views::devices::PANEL_HEIGHT);
        for track in [0, 1] {
            let strip = h.rect(Id::new(("mixer-strip", track)));
            let fader = h.rect(Id::new(("mixer-fader", track)));
            assert!(strip.contains_rect(fader), "{fader:?} outside {strip:?}");
            assert!(!fader.intersects(panel));
        }
        h.click(
            h.rect(Id::new(("mixer-builtin-enabled", h.id))).center(),
            PointerButton::Primary,
        );
        assert!(h.project.tracks()[1].mixer_effects()[0].bypassed);
        h.click(
            h.rect(Id::new("mixer-builtin-close")).center(),
            PointerButton::Primary,
        );
        h.settle();
        assert_eq!(
            h.ctx.data(|data| data.get_temp::<u64>(inspector_id())),
            None
        );
        h.assert_no_editor();
    }
}

#[test]
fn context_controls_open_inline_without_native_editor_actions() {
    let mut h = Harness::new(BuiltinKind::Comp);
    h.click(
        h.rect(Id::new(("mixer-effect", h.id))).center(),
        PointerButton::Secondary,
    );
    h.settle();
    h.click(
        h.rect(Id::new(("mixer-effect-open", h.id))).center(),
        PointerButton::Primary,
    );
    h.settle();
    assert_eq!(
        h.ctx.data(|data| data.get_temp::<u64>(inspector_id())),
        Some(h.id)
    );
    h.rect(Id::new("mixer-builtin-panel"));
    h.assert_no_editor();
}

#[test]
fn inspector_obeys_track_selection_and_disappears_after_removal() {
    let mut h = Harness::new(BuiltinKind::Eq);
    h.open();
    h.selection.tracks = BTreeSet::from([2]);
    h.settle();
    assert!(
        h.ctx
            .read_response(Id::new("mixer-builtin-panel"))
            .is_none()
    );
    h.selection.tracks = BTreeSet::from([1, 2]);
    h.settle();
    assert!(
        h.ctx
            .read_response(Id::new("mixer-builtin-panel"))
            .is_none()
    );
    h.selection.tracks = BTreeSet::from([1]);
    h.settle();
    h.rect(Id::new("mixer-builtin-panel"));
    let track = h.project.track_ids().nth(1).unwrap();
    h.project.remove_mixer_effect(track, 0).unwrap();
    h.settle();
    assert_eq!(
        h.ctx.data(|data| data.get_temp::<u64>(inspector_id())),
        None
    );
    assert!(
        h.ctx
            .read_response(Id::new("mixer-builtin-panel"))
            .is_none()
    );
}
