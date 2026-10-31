//! The small controls set into the panel: segmented switches, menus, toggles, a stepper,
//! and the power button.

use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use sympoie_calor::{PARAMETERS, choices};

use crate::Controls;
use crate::paint::{
    self, BODY_DARK, BODY_LIGHT, EMBER, FAINT, GOLD, HAIRLINE, LABEL, RAISED, TEXT, WELL, fade,
    sans, spaced,
};

/// Sets a parameter as one gesture.
pub(crate) fn set(controls: &mut dyn Controls, id: u32, value: f64) {
    controls.begin(id);
    controls.set(id, value);
    controls.end(id);
}

fn corner(rect: Rect, s: f32) -> f32 {
    (6.0 * s).min(rect.height() / 2.0)
}

/// A segmented switch set into the panel; returns the segment picked, if one was.
pub(crate) fn segmented(
    ui: &mut Ui,
    key: Id,
    rect: Rect,
    s: f32,
    options: &[&str],
    current: usize,
) -> Option<usize> {
    let radius = corner(rect, s);
    ui.painter().rect_filled(rect, radius, WELL);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, HAIRLINE), StrokeKind::Inside);
    let width = rect.width() / options.len() as f32;
    let mut picked = None;
    for (index, option) in options.iter().enumerate() {
        let segment = Rect::from_min_size(
            rect.min + vec2(width * index as f32, 0.0),
            vec2(width, rect.height()),
        );
        let response = ui.interact(segment, key.with(index), Sense::click());
        let painter = ui.painter();
        let on = index == current;
        let face = segment.shrink(2.0 * s);
        if on {
            paint::gradient_rect(
                painter,
                face,
                radius - 2.0 * s,
                RAISED,
                paint::mix(RAISED, Color32::BLACK, 0.18),
            );
        } else if response.hovered() {
            painter.rect_filled(face, radius - 2.0 * s, fade(Color32::WHITE, 0.04));
        } else if index > 0 && index != current + 1 {
            painter.vline(
                segment.left(),
                (segment.top() + 6.0 * s)..=(segment.bottom() - 6.0 * s),
                Stroke::new(1.0, HAIRLINE),
            );
        }
        spaced(
            painter,
            segment.center(),
            Align2::CENTER_CENTER,
            option,
            sans((rect.height() * 0.44).min(10.5 * s)),
            if on { TEXT } else { LABEL },
            0.2 * s,
        );
        if response.clicked() && !on {
            picked = Some(index);
        }
    }
    picked
}

/// A segmented switch for a choice parameter.
pub(crate) fn choice_segments(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    salt: Id,
    rect: Rect,
    s: f32,
    id: u32,
) {
    let spec = &PARAMETERS[id as usize];
    let Some(names) = choices(id) else {
        return;
    };
    let current = (controls.value(id) - spec.min).round().max(0.0) as usize;
    if let Some(index) = segmented(ui, salt.with(("segments", id)), rect, s, names, current) {
        set(controls, id, spec.min + index as f64);
    }
}

/// A menu button: a label, the current choice, and a list that opens on click. Returns
/// the item picked, if one was.
#[allow(clippy::too_many_arguments)]
pub(crate) fn menu(
    ui: &mut Ui,
    key: Id,
    rect: Rect,
    s: f32,
    label: &str,
    shown: &str,
    items: &[&str],
    current: Option<usize>,
) -> Option<usize> {
    let response = ui.interact(rect, key, Sense::click());
    let painter = ui.painter();
    let radius = corner(rect, s);
    painter.rect_filled(
        rect,
        radius,
        if response.hovered() {
            paint::mix(WELL, Color32::WHITE, 0.04)
        } else {
            WELL
        },
    );
    painter.rect_stroke(rect, radius, Stroke::new(1.0, HAIRLINE), StrokeKind::Inside);
    let text_size = (rect.height() * 0.44).min(10.5 * s);
    let mut x = rect.left() + 8.0 * s;
    if !label.is_empty() {
        let label = spaced(
            painter,
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            sans(text_size),
            LABEL,
            0.2 * s,
        );
        x = label.right() + 6.0 * s;
    }
    painter.text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        shown,
        sans(text_size),
        TEXT,
    );
    // A small chevron at the right.
    let chevron = pos2(rect.right() - 9.0 * s, rect.center().y);
    let size = 2.6 * s;
    painter.line_segment(
        [
            chevron + vec2(-size, -size * 0.5),
            chevron + vec2(0.0, size * 0.5),
        ],
        Stroke::new(1.2, LABEL),
    );
    painter.line_segment(
        [
            chevron + vec2(0.0, size * 0.5),
            chevron + vec2(size, -size * 0.5),
        ],
        Stroke::new(1.2, LABEL),
    );
    let mut picked = None;
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(rect.width().max(120.0));
            for (index, item) in items.iter().enumerate() {
                if ui.selectable_label(current == Some(index), *item).clicked() {
                    picked = Some(index);
                    ui.close();
                }
            }
        });
    picked
}

/// A menu button for a choice parameter.
pub(crate) fn choice_menu(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    salt: Id,
    rect: Rect,
    s: f32,
    id: u32,
    label: &str,
) {
    let spec = &PARAMETERS[id as usize];
    let Some(names) = choices(id) else {
        return;
    };
    let current = (controls.value(id) - spec.min).round().max(0.0) as usize;
    let shown = names.get(current).copied().unwrap_or_default();
    if let Some(index) = menu(
        ui,
        salt.with(("menu", id)),
        rect,
        s,
        label,
        shown,
        names,
        Some(current),
    ) {
        set(controls, id, spec.min + index as f64);
    }
}

/// A toggle for a switch parameter: a pill that lights up in `color` while it is on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn toggle(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    key: Id,
    rect: Rect,
    s: f32,
    id: u32,
    label: &str,
    color: Color32,
) -> egui::Response {
    let on = controls.value(id) >= 0.5;
    let response = ui.interact(rect, key, Sense::click());
    let painter = ui.painter();
    let radius = corner(rect, s);
    painter.rect_filled(rect, radius, if on { fade(color, 0.2) } else { WELL });
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, if on { fade(color, 0.55) } else { HAIRLINE }),
        StrokeKind::Inside,
    );
    spaced(
        painter,
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        sans((rect.height() * 0.44).min(10.5 * s)),
        if on {
            color
        } else if response.hovered() {
            TEXT
        } else {
            LABEL
        },
        0.2 * s,
    );
    if response.clicked() {
        set(controls, id, if on { 0.0 } else { 1.0 });
    }
    response
}

/// A stepper for a whole-number parameter: minus, the value, plus.
pub(crate) fn stepper(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    salt: Id,
    rect: Rect,
    s: f32,
    id: u32,
    unit: &str,
) {
    let spec = &PARAMETERS[id as usize];
    let value = controls.value(id).round();
    let radius = corner(rect, s);
    ui.painter().rect_filled(rect, radius, WELL);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, HAIRLINE), StrokeKind::Inside);
    let side = rect.height();
    let minus = Rect::from_min_size(rect.min, Vec2::splat(side));
    let plus = Rect::from_min_size(pos2(rect.right() - side, rect.top()), Vec2::splat(side));
    for (button, step, glyph) in [(minus, -1.0, "−"), (plus, 1.0, "+")] {
        let response = ui.interact(button, salt.with(("step", id, glyph)), Sense::click());
        let enabled = (spec.min..=spec.max).contains(&(value + step));
        if response.hovered() && enabled {
            ui.painter().rect_filled(
                button.shrink(2.0 * s),
                radius - 2.0 * s,
                fade(Color32::WHITE, 0.05),
            );
        }
        ui.painter().text(
            button.center(),
            Align2::CENTER_CENTER,
            glyph,
            sans(side * 0.6),
            if enabled { TEXT } else { FAINT },
        );
        if response.clicked() && enabled {
            set(controls, id, value + step);
        }
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{value:.0} {unit}"),
        sans((side * 0.44).min(10.5 * s)),
        TEXT,
    );
}

/// The power button: a small lit cap, its glyph copper while Calor runs and amber while it
/// is bypassed.
pub(crate) fn power(ui: &mut Ui, controls: &mut dyn Controls, salt: Id, centre: Pos2, radius: f32) {
    let rect = Rect::from_center_size(centre, Vec2::splat(2.0 * radius + 4.0));
    let response = ui.interact(rect, salt.with("power"), Sense::click());
    let on = !controls.bypassed();
    let painter = ui.painter();
    let color = if on { EMBER } else { GOLD };
    paint::shadow(painter, centre, radius, 0.45);
    let light = if response.hovered() {
        paint::mix(BODY_LIGHT, Color32::WHITE, 0.08)
    } else {
        BODY_LIGHT
    };
    paint::cap(painter, centre, radius, light, BODY_DARK);
    // The power glyph: an open ring with a bar through its gap.
    let glyph = radius * 0.45;
    painter.add(egui::Shape::line(
        paint::arc(centre, glyph, 0.6, std::f32::consts::TAU - 0.6),
        Stroke::new(1.5, color),
    ));
    painter.line_segment(
        [
            centre - vec2(0.0, glyph * 1.25),
            centre - vec2(0.0, glyph * 0.25),
        ],
        Stroke::new(1.5, color),
    );
    if response.clicked() {
        controls.set_bypassed(on);
    }
    response.on_hover_text(if on {
        "Bypass Calor"
    } else {
        "Turn Calor back on"
    });
}

/// "Bypassed", in amber, anchored at `anchor`.
pub(crate) fn bypassed_badge(painter: &egui::Painter, anchor: Pos2, align: Align2, s: f32) {
    spaced(
        painter,
        anchor,
        align,
        "Bypassed",
        sans(9.5 * s),
        GOLD,
        0.3 * s,
    );
}

/// A small heading set in spaced capitals.
pub(crate) fn heading(painter: &egui::Painter, pos: Pos2, text: &str, s: f32) -> Rect {
    spaced(
        painter,
        pos,
        Align2::LEFT_CENTER,
        &text.to_uppercase(),
        sans(8.4 * s),
        LABEL,
        1.4 * s,
    )
}

/// A thin horizontal meter from 0 to 1 in `color`.
pub(crate) fn bar(painter: &egui::Painter, rect: Rect, share: f32, color: Color32, s: f32) {
    painter.rect_filled(rect, 2.0 * s, WELL);
    let filled = Rect::from_min_max(
        rect.min,
        pos2(
            rect.left() + rect.width() * share.clamp(0.0, 1.0),
            rect.bottom(),
        ),
    );
    if filled.width() > 0.5 {
        painter.rect_filled(filled, 2.0 * s, color);
    }
}
