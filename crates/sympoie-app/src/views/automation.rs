use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Id, Key, Layout, Modifiers, Pos2, Rect, RichText,
    Sense, Shape, Stroke, Ui, pos2, vec2,
};
use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::project::Project;

use super::{Timeline, ignore_grid};
use crate::actions::Action;
use crate::state::ArrangementState;
use crate::ui::{analog, theme, widgets};

const RED: Color32 = Color32::from_rgb(238, 91, 94);

#[path = "automation_targets.rs"]
mod targets;
#[path = "automation_value.rs"]
mod value_edit;

#[derive(Debug, Clone, Copy, PartialEq)]
struct SelectedPoint {
    track: usize,
    target: AutomationTarget,
    tick: u64,
}

#[derive(Debug, Clone)]
struct Drag {
    track: usize,
    target: AutomationTarget,
    index: usize,
    original: Vec<AutomationPoint>,
    points: Vec<AutomationPoint>,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct PointValueEdit {
    selected: SelectedPoint,
    original: f64,
    original_text: String,
    text: String,
    focus: bool,
    invalid: bool,
}

impl PointValueEdit {
    fn new(selected: SelectedPoint, value: f64) -> Self {
        let text = value_edit::format(selected.target, value);
        Self {
            selected,
            original: value,
            original_text: text.clone(),
            text,
            focus: true,
            invalid: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Editor {
    drag: Option<Drag>,
    selected: Option<SelectedPoint>,
    value_edit: Option<PointValueEdit>,
}

impl Editor {
    pub(super) fn keyboard(
        &mut self,
        ui: &Ui,
        project: &Project,
        state: &ArrangementState,
        actions: &mut Vec<Action>,
    ) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.drag = None;
            self.selected = None;
        }
        let delete = ui.input_mut(|i| {
            let delete = i.consume_key(Modifiers::NONE, Key::Delete);
            i.consume_key(Modifiers::NONE, Key::Backspace) || delete
        });
        if delete && let Some(selected) = self.selected.take() {
            self.drag = None;
            if target_for(state, project, selected.track) == selected.target {
                delete_point(project, selected, actions);
            }
        }
    }

    pub(super) fn finish(&mut self, ui: &Ui, actions: &mut Vec<Action>) {
        if ui.input(|i| i.pointer.primary_released())
            && let Some(drag) = self.drag.take()
        {
            if drag.points != drag.original {
                set_lane(drag.track, drag.target, drag.points, drag.enabled, actions);
            }
        } else if !ui.input(|i| i.pointer.primary_down()) {
            self.drag = None;
        }
    }
}

pub(super) fn toggle(ui: &mut Ui, enabled: bool, actions: &mut Vec<Action>) {
    let (rect, _) = ui.allocate_exact_size(vec2(138.0, 24.0), Sense::hover());
    let response = ui.interact(rect, Id::new("automation-toggle"), Sense::click());
    let cap = analog::push_button(
        ui.painter(),
        rect.shrink(1.0),
        2.0,
        enabled.then_some(RED),
        response.hovered(),
        response.is_pointer_button_down_on(),
    );
    let ink = if enabled {
        theme::TEXT_ON_COLOR
    } else {
        theme::TEXT
    };
    ui.painter().text(
        cap.left_center() + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        "Automation",
        theme::prop(11.0),
        ink,
    );
    ui.painter().text(
        cap.right_center() - vec2(10.0, 0.0),
        Align2::RIGHT_CENTER,
        "A",
        theme::mono(10.0),
        ink.gamma_multiply(0.7),
    );
    response.clone().on_hover_text(
        "Show or hide automation envelopes · A\nClick to add points; drag to shape an envelope.",
    );
    if response.clicked() {
        actions.push(Action::ToggleAutomation);
    }
}

fn target_for(state: &ArrangementState, project: &Project, index: usize) -> AutomationTarget {
    let Some(track) = project.tracks().get(index) else {
        return AutomationTarget::TrackGain;
    };
    state
        .automation_targets
        .get(&index)
        .copied()
        .filter(|target| target.value(track).is_some())
        .or_else(|| track.automation().first().map(AutomationLane::target))
        .unwrap_or(AutomationTarget::TrackGain)
}

pub(super) fn selector(
    ui: &mut Ui,
    header: Rect,
    indent: f32,
    index: usize,
    state: &mut ArrangementState,
    project: &Project,
    actions: &mut Vec<Action>,
) {
    let track = &project.tracks()[index];
    let mut target = target_for(state, project, index);
    let area = Rect::from_min_max(
        pos2(header.left() + indent + 6.0, header.top() + 43.0),
        pos2(header.right() - 6.0, header.top() + 64.0),
    );
    widgets::inside(ui, area, Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (lamp, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
        let response = ui.interact(lamp, Id::new(("automation-enable", index)), Sense::click());
        let existing = track
            .automation()
            .iter()
            .find(|lane| lane.target() == target);
        analog::round_switch(
            ui.painter(),
            lamp.center(),
            4.5,
            existing
                .filter(|lane| lane.enabled() && !lane.points().is_empty())
                .map(|_| RED),
            response.hovered(),
            response.is_pointer_button_down_on(),
        );
        if response.clicked()
            && let Some(lane) = existing
        {
            actions.push(Action::SetAutomation {
                track: index,
                lane: lane.clone().with_enabled(!lane.enabled()),
            });
        }
        response.on_hover_text(
            "Enable or disable this envelope\nAn envelope takes control when it has points.",
        );
        targets::selector(
            ui,
            (area.width() - 26.0).max(80.0),
            index,
            track,
            &mut target,
        )
        .context_menu(|ui| envelope_menu(ui, index, target, existing, actions));
        state.automation_targets.insert(index, target);
    });
}

fn plot(lane: Rect) -> Rect {
    lane.shrink2(vec2(0.0, 10.0))
}

fn point_position(point: AutomationPoint, plot: Rect, timeline: &Timeline) -> Pos2 {
    pos2(
        timeline.x(point.tick as f64),
        plot.bottom() - point.value as f32 * plot.height(),
    )
}

fn pointer_value(pointer: Pos2, plot: Rect) -> f64 {
    f64::from(((plot.bottom() - pointer.y) / plot.height().max(1.0)).clamp(0.0, 1.0))
}

fn curve_value(points: &[AutomationPoint], tick: u64, manual: f64) -> f64 {
    let upper = points.partition_point(|point| point.tick <= tick);
    if upper == 0 {
        return points.first().map_or(manual, |point| point.value);
    }
    let previous = points[upper - 1];
    let Some(next) = points.get(upper) else {
        return previous.value;
    };
    let fraction = (tick - previous.tick) as f64 / (next.tick - previous.tick) as f64;
    previous.value + (next.value - previous.value) * fraction
}

fn nearest(
    points: &[AutomationPoint],
    pointer: Pos2,
    plot: Rect,
    timeline: &Timeline,
) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| {
            let distance = point_position(*point, plot, timeline).distance_sq(pointer);
            (distance <= 64.0).then_some((index, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

fn insert_point(points: &mut Vec<AutomationPoint>, point: AutomationPoint) -> usize {
    match points.binary_search_by_key(&point.tick, |p| p.tick) {
        Ok(index) => {
            points[index] = point;
            index
        }
        Err(index) => {
            points.insert(index, point);
            index
        }
    }
}

fn move_point(points: &mut [AutomationPoint], index: usize, tick: u64, value: f64) {
    let minimum = index
        .checked_sub(1)
        .map_or(0, |i| points[i].tick.saturating_add(1));
    let maximum = points
        .get(index + 1)
        .map_or(u64::MAX, |p| p.tick.saturating_sub(1));
    points[index] = AutomationPoint {
        tick: tick.clamp(minimum, maximum),
        value,
    };
}

fn set_lane(
    track: usize,
    target: AutomationTarget,
    points: Vec<AutomationPoint>,
    enabled: bool,
    actions: &mut Vec<Action>,
) {
    if points.is_empty() {
        actions.push(Action::RemoveAutomation { track, target });
    } else if let Ok(lane) = AutomationLane::new(target, points) {
        actions.push(Action::SetAutomation {
            track,
            lane: lane.with_enabled(enabled),
        });
    }
}

fn delete_point(project: &Project, selected: SelectedPoint, actions: &mut Vec<Action>) {
    if let Some(lane) = project.tracks().get(selected.track).and_then(|track| {
        track
            .automation()
            .iter()
            .find(|lane| lane.target() == selected.target)
    }) {
        let points: Vec<_> = lane
            .points()
            .iter()
            .copied()
            .filter(|point| point.tick != selected.tick)
            .collect();
        if points.len() != lane.points().len() {
            set_lane(
                selected.track,
                selected.target,
                points,
                lane.enabled(),
                actions,
            );
        }
    }
}

fn envelope_menu(
    ui: &mut Ui,
    track: usize,
    target: AutomationTarget,
    lane: Option<&AutomationLane>,
    actions: &mut Vec<Action>,
) {
    if let Some(lane) = lane {
        if ui
            .button(if lane.enabled() {
                "Disable Envelope"
            } else {
                "Enable Envelope"
            })
            .clicked()
        {
            actions.push(Action::SetAutomation {
                track,
                lane: lane.clone().with_enabled(!lane.enabled()),
            });
            ui.close();
        }
        if ui.button("Clear Envelope").clicked() {
            actions.push(Action::RemoveAutomation { track, target });
            ui.close();
        }
    } else {
        ui.label(RichText::new("Click the lane to add a breakpoint").color(theme::TEXT_DIM));
    }
}

fn point_value_menu(
    ui: &mut Ui,
    edit: &mut PointValueEdit,
    lane: &AutomationLane,
    actions: &mut Vec<Action>,
) {
    use egui::text::{CCursor, CCursorRange};

    let cancelled = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    if cancelled {
        ui.close();
        return;
    }
    let enter = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
    ui.label("Breakpoint Value");
    let mut field = egui::TextEdit::singleline(&mut edit.text)
        .id(Id::new("automation-point-value"))
        .desired_width(180.0)
        .hint_text(value_edit::hint(edit.selected.target))
        .show(ui);
    if edit.focus {
        field.response.request_focus();
        field.state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(0),
            CCursor::new(edit.text.chars().count()),
        )));
        field.state.clear_undoer();
        field.state.store(ui.ctx(), field.response.id);
        edit.focus = false;
    }
    if field.response.changed() {
        edit.invalid = false;
    }
    ui.label(RichText::new(value_edit::hint(edit.selected.target)).small());
    let apply = ui.button("Apply");
    ui.interact(
        apply.rect,
        Id::new("automation-point-value-apply"),
        Sense::hover(),
    );
    if enter || apply.clicked() {
        let value = if edit.text == edit.original_text {
            Some(edit.original)
        } else {
            value_edit::parse(edit.selected.target, &edit.text)
        };
        if let Some(value) = value {
            let mut points = lane.points().to_vec();
            if let Ok(index) = points.binary_search_by_key(&edit.selected.tick, |point| point.tick)
                && points[index].value == edit.original
                && points[index].value != value
            {
                points[index].value = value;
                set_lane(
                    edit.selected.track,
                    edit.selected.target,
                    points,
                    lane.enabled(),
                    actions,
                );
            }
            field.response.surrender_focus();
            ui.close();
        } else {
            edit.invalid = true;
            field.response.request_focus();
        }
    }
    if edit.invalid {
        ui.colored_label(RED, "Enter a valid value in the range shown above.");
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn lane(
    ui: &mut Ui,
    lane: Rect,
    index: usize,
    state: &ArrangementState,
    project: &Project,
    timeline: &Timeline,
    editor: &mut Editor,
    actions: &mut Vec<Action>,
) {
    let track = &project.tracks()[index];
    let target = target_for(state, project, index);
    let existing = track
        .automation()
        .iter()
        .find(|envelope| envelope.target() == target);
    let base_points = existing.map_or(&[][..], AutomationLane::points);
    let enabled = existing.is_none_or(AutomationLane::enabled);
    let manual = target.value(track).unwrap_or(0.5);
    let plot = plot(lane);
    let response = ui.interact(
        lane,
        Id::new(("automation-lane", index)),
        Sense::click_and_drag(),
    );
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| ui.input(|i| i.pointer.hover_pos()));
    let snapped = |pointer: Pos2| {
        timeline.snapped(pointer.x.clamp(lane.left(), lane.right()), !ignore_grid(ui))
    };
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        let mut points = base_points.to_vec();
        let point = nearest(&points, origin, plot, timeline).unwrap_or_else(|| {
            insert_point(
                &mut points,
                AutomationPoint {
                    tick: snapped(origin),
                    value: pointer_value(origin, plot),
                },
            )
        });
        editor.drag = Some(Drag {
            track: index,
            target,
            index: point,
            original: base_points.to_vec(),
            points,
            enabled,
        });
    }
    if let Some(drag) = editor
        .drag
        .as_mut()
        .filter(|drag| drag.track == index && drag.target == target)
        && let Some(pointer) = pointer
    {
        move_point(
            &mut drag.points,
            drag.index,
            snapped(pointer),
            pointer_value(pointer, plot),
        );
        editor.selected = Some(SelectedPoint {
            track: index,
            target,
            tick: drag.points[drag.index].tick,
        });
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    if response.clicked()
        && let Some(pointer) = pointer
    {
        if let Some(point) = nearest(base_points, pointer, plot, timeline) {
            let selected = SelectedPoint {
                track: index,
                target,
                tick: base_points[point].tick,
            };
            editor.selected = Some(selected);
        } else {
            let tick = snapped(pointer);
            let point = AutomationPoint {
                tick,
                value: curve_value(base_points, tick, manual),
            };
            let mut points = base_points.to_vec();
            insert_point(&mut points, point);
            if points != base_points {
                set_lane(index, target, points, enabled, actions);
            }
            editor.selected = Some(SelectedPoint {
                track: index,
                target,
                tick: point.tick,
            });
        }
    }
    if response.secondary_clicked() {
        editor.drag = None;
        editor.value_edit = pointer
            .and_then(|pointer| nearest(base_points, pointer, plot, timeline))
            .map(|point| {
                PointValueEdit::new(
                    SelectedPoint {
                        track: index,
                        target,
                        tick: base_points[point].tick,
                    },
                    base_points[point].value,
                )
            });
        if let Some(edit) = &editor.value_edit {
            editor.selected = Some(edit.selected);
        }
    }
    egui::Popup::context_menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            if let Some(edit) = editor
                .value_edit
                .as_mut()
                .filter(|edit| edit.selected.track == index && edit.selected.target == target)
                && let Some(existing) = existing
                && existing
                    .points()
                    .iter()
                    .any(|point| point.tick == edit.selected.tick)
            {
                point_value_menu(ui, edit, existing, actions);
                ui.separator();
                if ui.button("Delete Breakpoint").clicked() {
                    delete_point(project, edit.selected, actions);
                    editor.selected = None;
                    ui.close();
                }
                ui.separator();
            }
            envelope_menu(ui, index, target, existing, actions);
        });

    let points = editor
        .drag
        .as_ref()
        .filter(|drag| drag.track == index && drag.target == target)
        .map_or(base_points, |drag| drag.points.as_slice());
    let painter = ui.painter_at(lane);
    let color = if enabled && !points.is_empty() {
        RED
    } else {
        RED.gamma_multiply(0.45)
    };
    for value in [0.0, 0.5, 1.0] {
        let y = plot.bottom() - plot.height() * value;
        painter.hline(
            lane.x_range(),
            y,
            Stroke::new(1.0, Color32::from_white_alpha(12)),
        );
    }
    if points.is_empty() {
        let y = plot.bottom() - manual as f32 * plot.height();
        let mut x = lane.left();
        while x < lane.right() {
            painter.line_segment(
                [pos2(x, y), pos2((x + 5.0).min(lane.right()), y)],
                Stroke::new(1.0, color),
            );
            x += 9.0;
        }
        painter.text(
            lane.left_top() + vec2(8.0, 3.0),
            Align2::LEFT_TOP,
            format!("{}  ·  Click to add points", target.format_value(manual)),
            theme::prop(9.5),
            theme::TEXT_FAINT,
        );
    } else {
        let (from, to) = timeline.visible();
        let first = points
            .partition_point(|point| (point.tick as f64) < from)
            .saturating_sub(1);
        let last = (points.partition_point(|point| point.tick as f64 <= to) + 1).min(points.len());
        let visible = &points[first..last];
        let mut path = Vec::with_capacity(visible.len() + 2);
        if first == 0 {
            path.push(pos2(
                lane.left(),
                point_position(points[0], plot, timeline).y,
            ));
        }
        path.extend(
            visible
                .iter()
                .map(|point| point_position(*point, plot, timeline)),
        );
        if last == points.len() {
            path.push(pos2(
                lane.right(),
                point_position(points[points.len() - 1], plot, timeline).y,
            ));
        }
        painter.add(Shape::line(path, Stroke::new(1.5, color)));
        for point in visible {
            let pos = point_position(*point, plot, timeline);
            if !lane.contains(pos) {
                continue;
            }
            let selected = editor.selected
                == Some(SelectedPoint {
                    track: index,
                    target,
                    tick: point.tick,
                });
            let hover = ui.interact(
                Rect::from_center_size(pos, vec2(12.0, 12.0)),
                Id::new(("automation-point", index, target, point.tick)),
                Sense::hover(),
            );
            painter.circle_filled(
                pos,
                if selected || hover.hovered() {
                    4.0
                } else {
                    3.0
                },
                if selected { Color32::WHITE } else { color },
            );
            painter.circle_stroke(pos, 3.5, Stroke::new(1.0, theme::BG_DEEP));
            hover.on_hover_text(format!(
                "{} · {}\nDrag to move · Right-click to enter a value or delete",
                crate::ui::format::bar_beat(project.time_signature(), point.tick),
                target.format_value(point.value)
            ));
            if selected {
                let label = target.format_value(point.value);
                let right = pos.x < lane.right() - 90.0;
                let below = pos.y < lane.top() + 24.0;
                painter.text(
                    pos + vec2(
                        if right { 8.0 } else { -8.0 },
                        if below { 6.0 } else { -5.0 },
                    ),
                    match (right, below) {
                        (true, true) => Align2::LEFT_TOP,
                        (true, false) => Align2::LEFT_BOTTOM,
                        (false, true) => Align2::RIGHT_TOP,
                        (false, false) => Align2::RIGHT_BOTTOM,
                    },
                    label,
                    theme::mono(10.0),
                    Color32::WHITE,
                );
            }
        }
    }
    if response.hovered() && editor.drag.is_none() {
        let is_point =
            pointer.is_some_and(|pointer| nearest(points, pointer, plot, timeline).is_some());
        ui.ctx().set_cursor_icon(if is_point {
            CursorIcon::Grab
        } else {
            CursorIcon::Crosshair
        });
    }
}

#[cfg(test)]
#[path = "automation_tests.rs"]
mod tests;
