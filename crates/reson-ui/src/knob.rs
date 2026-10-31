//! The ring knob: a dark disc in a thin track, its value an arc of light.

use egui::{Align2, Color32, CursorIcon, Id, Pos2, Rect, Response, Sense, Stroke, Ui, vec2};
use sympoie_reson::PARAMETERS;

use crate::paint::{
    self, BODY_DARK, BODY_LIGHT, LABEL, TEXT, TRACK, arc, fade, mono, sans, spaced,
};
use crate::{Controls, format};

/// Half the knob's travel, from straight up, in radians.
const SWEEP: f32 = 2.36;
/// Share of the range one point of drag covers, and how much finer Shift makes it.
const DRAG: f32 = 0.0045;
const FINE: f32 = 0.15;

/// How a knob is drawn.
#[derive(Clone, Copy)]
pub(crate) struct Look {
    pub(crate) radius: f32,
    pub(crate) accent: Color32,
    /// The larger knobs of the Notes group get a halo while they are up.
    pub(crate) hero: bool,
    /// Scale of the label and value type.
    pub(crate) text: f32,
}

/// Draws a knob for parameter `id` around `centre`, with its label above and its value
/// below, and handles dragging, Shift for fine steps, scrolling, double-click to reset and
/// right-click to type a value. Every change goes through `controls` as one gesture.
#[allow(clippy::too_many_arguments)]
pub(crate) fn knob(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    salt: Id,
    id: u32,
    centre: Pos2,
    look: Look,
    label: &str,
    help: &str,
) -> Response {
    let spec = &PARAMETERS[id as usize];
    let radius = look.radius;
    let hit = Rect::from_center_size(centre, vec2(2.0 * radius + 12.0, 2.0 * radius + 8.0));
    let key = salt.with(("knob", id));
    let mut response = ui.interact(hit, key, Sense::click_and_drag());
    let value = controls.value(id);
    let mut position = spec.normalize(value) as f32;
    if response.drag_started() {
        controls.begin(id);
    }
    if response.dragged() {
        let delta = response.drag_delta();
        let speed = if ui.input(|input| input.modifiers.shift) {
            DRAG * FINE
        } else {
            DRAG
        };
        let next = (position + (delta.x - delta.y) * speed).clamp(0.0, 1.0);
        if next != position {
            position = next;
            controls.set(id, spec.denormalize(f64::from(position)));
            response.mark_changed();
        }
    }
    if response.drag_stopped() {
        controls.end(id);
    }
    if response.hovered() && !response.dragged() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll != 0.0 {
            ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
            let next = (position + scroll * 0.0015).clamp(0.0, 1.0);
            controls.begin(id);
            controls.set(id, spec.denormalize(f64::from(next)));
            controls.end(id);
            position = next;
        }
    }
    if response.double_clicked() {
        controls.begin(id);
        controls.set(id, spec.default);
        controls.end(id);
        position = spec.normalize(spec.default) as f32;
    }
    entry(controls, &response, id, key);

    if ui.is_rect_visible(hit.expand(radius)) {
        let painter = ui.painter();
        let hot = response.hovered() || response.dragged();
        // The groove and the value arc around the cap, from the bottom of the travel or,
        // for a value that can go either way, from the middle.
        let width = if look.hero { 3.0 } else { 2.4 };
        painter.add(egui::Shape::line(
            arc(centre, radius, -SWEEP, SWEEP),
            Stroke::new(width, TRACK),
        ));
        let angle = -SWEEP + 2.0 * SWEEP * position;
        let bipolar = spec.min < 0.0 && spec.max > 0.0;
        let start = if bipolar { 0.0 } else { -SWEEP };
        if (angle - start).abs() > 0.01 {
            let accent = if hot {
                paint::mix(look.accent, Color32::WHITE, 0.2)
            } else {
                look.accent
            };
            paint::line(
                painter,
                arc(centre, radius, start.min(angle), start.max(angle)),
                accent,
                width,
            );
        }
        // The cap, lit from above, on a soft shadow.
        let body = radius - if look.hero { 5.5 } else { 4.8 };
        paint::shadow(painter, centre, body, 0.5);
        let light = if hot {
            paint::mix(BODY_LIGHT, Color32::WHITE, 0.08)
        } else {
            BODY_LIGHT
        };
        paint::cap(painter, centre, body, light, BODY_DARK);
        // The pointer: a fine light line from the middle of the cap to its edge.
        let direction = vec2(angle.sin(), -angle.cos());
        painter.line_segment(
            [
                centre + direction * (body * 0.4),
                centre + direction * (body - 2.0),
            ],
            Stroke::new(1.6, if hot { Color32::WHITE } else { fade(TEXT, 0.9) }),
        );
        spaced(
            painter,
            centre - vec2(0.0, radius + 5.0 * look.text),
            Align2::CENTER_BOTTOM,
            label,
            sans(10.0 * look.text),
            if hot { TEXT } else { LABEL },
            0.2 * look.text,
        );
        painter.text(
            centre + vec2(0.0, radius + 4.0 * look.text),
            Align2::CENTER_TOP,
            format::value(spec, spec.denormalize(f64::from(position))),
            mono(9.3 * look.text),
            if hot { look.accent } else { TEXT },
        );
    }
    response
        .on_hover_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(format!(
            "{}: {}\n{help}\nDrag, Shift for fine steps, double-click to reset, right-click to type.",
            spec.name,
            format::value(spec, value)
        ))
}

/// The right-click box for typing a value.
pub(crate) fn entry(controls: &mut dyn Controls, response: &Response, id: u32, key: Id) {
    let spec = &PARAMETERS[id as usize];
    let text_key = key.with("entry");
    if response.secondary_clicked() {
        let text = format::value(spec, controls.value(id));
        response
            .ctx
            .data_mut(|data| data.insert_temp(text_key, text));
    }
    let mut commit = None;
    egui::Popup::context_menu(response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(egui::RichText::new(spec.name).small().color(LABEL));
            let mut text = ui
                .ctx()
                .data_mut(|data| data.get_temp::<String>(text_key))
                .unwrap_or_default();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(110.0)
                    .font(mono(11.0)),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            let parsed = format::parse(spec, &text);
            if edit.lost_focus() {
                if ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && let Some(parsed) = parsed
                {
                    commit = Some(parsed);
                }
                ui.close();
            } else {
                ui.label(
                    egui::RichText::new(if text.trim().is_empty() || parsed.is_some() {
                        "Enter to apply · Esc to cancel"
                    } else {
                        "Not a valid value"
                    })
                    .small()
                    .color(LABEL),
                );
            }
            ui.ctx().data_mut(|data| data.insert_temp(text_key, text));
        });
    if let Some(value) = commit {
        controls.begin(id);
        controls.set(id, value.clamp(spec.min, spec.max));
        controls.end(id);
    }
}
