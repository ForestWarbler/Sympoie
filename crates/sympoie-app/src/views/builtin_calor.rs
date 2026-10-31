//! Calor's interface in Sympoie. Calor draws its own graphite faceplate, the same one the
//! Calor plugin shows in other hosts (see the `sympoie-calor-ui` crate); this module only
//! connects it to the device's parameters, its power button and its telemetry.

use super::*;
use sympoie_calor_ui::Controls;

/// The device's parameters and power button, as the shared interface reads and sets them.
struct DeckControls<'a, 'b> {
    deck: &'a mut Deck<'b>,
}

impl Controls for DeckControls<'_, '_> {
    fn value(&self, id: u32) -> f64 {
        self.deck.value(id)
    }

    /// Sympoie folds a drag's edits into one undo step on its own.
    fn begin(&mut self, _id: u32) {}

    fn set(&mut self, id: u32, value: f64) {
        self.deck.set(id, value);
    }

    fn end(&mut self, _id: u32) {}

    fn bypassed(&self) -> bool {
        self.deck.plugin.bypassed
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.deck.actions.push(Action::SetVst3Bypassed {
            id: self.deck.plugin.id,
            bypassed,
        });
    }
}

/// Keeps one Calor's interface state (the eased level, the meters' age) apart from
/// another's.
fn salt(deck: &Deck) -> Id {
    Id::new(("calor", deck.plugin.id))
}

pub(super) fn compact(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    let salt = salt(deck);
    sympoie_calor_ui::card(
        ui,
        &mut DeckControls { deck },
        telemetry.map(|telemetry| telemetry.calor.as_ref()),
        salt,
    );
}

pub(super) fn editor(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    let salt = salt(deck);
    sympoie_calor_ui::panel(
        ui,
        &mut DeckControls { deck },
        telemetry.map(|telemetry| telemetry.calor.as_ref()),
        salt,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sympoie::builtin::calor::{AUTO_GAIN, DRIVE};

    /// Runs one frame of Calor's card (`panel` false) or full panel.
    fn frame(
        ctx: &egui::Context,
        instance: &PluginInstance,
        time: &mut f64,
        events: Vec<egui::Event>,
        actions: &mut Vec<Action>,
        panel: bool,
    ) {
        let size = if panel {
            vec2(900.0, 500.0)
        } else {
            vec2(444.0, 172.0)
        };
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), size)),
            time: Some(*time),
            focused: true,
            events,
            ..Default::default()
        };
        *time += 1.0 / 60.0;
        let mut output = ctx.run_ui(raw, |ui| {
            if panel {
                crate::views::builtin_devices::show(ui, instance, BuiltinKind::Calor, None, actions)
            } else {
                super::super::show(ui, instance, BuiltinKind::Calor, None, actions)
            }
        });
        output.textures_delta.clear();
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn dragging_drive_on_the_card_sets_only_drive_and_auto_gain_toggles() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::Calor.instance();
        instance.id = 71;
        let mut actions = Vec::new();
        let mut time = 0.0;
        frame(&ctx, &instance, &mut time, vec![], &mut actions, false);
        frame(&ctx, &instance, &mut time, vec![], &mut actions, false);
        let salt = Id::new(("calor", 71_u64));
        let start = ctx
            .read_response(salt.with(("knob", DRIVE)))
            .unwrap()
            .rect
            .center();
        frame(
            &ctx,
            &instance,
            &mut time,
            vec![egui::Event::PointerMoved(start), press(start, true)],
            &mut actions,
            false,
        );
        frame(
            &ctx,
            &instance,
            &mut time,
            vec![egui::Event::PointerMoved(start + vec2(0.0, -40.0))],
            &mut actions,
            false,
        );
        frame(
            &ctx,
            &instance,
            &mut time,
            vec![press(start + vec2(0.0, -40.0), false)],
            &mut actions,
            false,
        );
        let sets: Vec<(u32, f64)> = actions
            .iter()
            .filter_map(|action| match action {
                Action::SetVst3Parameter {
                    id: 71,
                    parameter,
                    value,
                } => Some((*parameter, *value)),
                _ => None,
            })
            .collect();
        assert!(!sets.is_empty());
        assert_eq!(sets.len(), actions.len());
        let default = BuiltinKind::Calor.instance().parameters[DRIVE as usize].value;
        assert!(
            sets.iter()
                .all(|&(id, value)| id == DRIVE && value > default)
        );

        actions.clear();
        let auto = ctx
            .read_response(salt.with("auto gain"))
            .unwrap()
            .rect
            .center();
        frame(
            &ctx,
            &instance,
            &mut time,
            vec![egui::Event::PointerMoved(auto), press(auto, true)],
            &mut actions,
            false,
        );
        frame(
            &ctx,
            &instance,
            &mut time,
            vec![press(auto, false)],
            &mut actions,
            false,
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::SetVst3Parameter {
                id: 71,
                parameter,
                value,
            } if *parameter == AUTO_GAIN && *value == 0.0
        )));
    }

    #[test]
    fn the_power_button_bypasses_the_device() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::Calor.instance();
        instance.id = 72;
        let mut actions = Vec::new();
        let mut time = 0.0;
        frame(&ctx, &instance, &mut time, vec![], &mut actions, true);
        frame(&ctx, &instance, &mut time, vec![], &mut actions, true);
        let power = ctx
            .read_response(Id::new(("calor", 72_u64)).with("power"))
            .unwrap()
            .rect
            .center();
        frame(
            &ctx,
            &instance,
            &mut time,
            vec![egui::Event::PointerMoved(power), press(power, true)],
            &mut actions,
            true,
        );
        frame(
            &ctx,
            &instance,
            &mut time,
            vec![press(power, false)],
            &mut actions,
            true,
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::SetVst3Bypassed {
                id: 72,
                bypassed: true
            }
        )));
    }
}
