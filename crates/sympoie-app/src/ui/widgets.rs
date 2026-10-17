use eframe::egui::{
    self, Align2, Color32, CursorIcon, Id, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind,
    Ui, Vec2, pos2, vec2,
};

use crate::ui::{analog, format, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Stop,
    Record,
    Loop,
    Follow,
}

pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.height().min(rect.width()) * 0.5;
    let stroke = Stroke::new(1.5, color);
    match icon {
        Icon::Play => {
            let points = vec![
                pos2(c.x - s * 0.5, c.y - s * 0.7),
                pos2(c.x + s * 0.6, c.y),
                pos2(c.x - s * 0.5, c.y + s * 0.7),
            ];
            painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 1.2)), 0.0, color);
        }
        Icon::Record => {
            painter.circle_filled(c, s * 0.65, color);
        }
        Icon::Loop => {
            let body = Rect::from_center_size(c, vec2(s * 1.7, s * 1.1));
            painter.line_segment([body.left_top(), body.right_top()], stroke);
            painter.line_segment([body.left_top(), pos2(body.left(), c.y)], stroke);
            painter.line_segment([body.right_top(), pos2(body.right(), c.y)], stroke);
            painter.line_segment([body.left_bottom(), body.right_bottom()], stroke);
            painter.line_segment([pos2(body.left(), c.y), body.left_bottom()], stroke);
            painter.line_segment([pos2(body.right(), c.y), body.right_bottom()], stroke);
            painter.rect_filled(
                Rect::from_center_size(c, vec2(s * 0.9, s * 0.35)),
                0.0,
                color,
            );
        }
        Icon::Follow => {
            let page = Rect::from_center_size(c, vec2(s * 1.5, s * 1.3));
            painter.rect_stroke(page, 0.0, stroke, StrokeKind::Inside);
            let points = vec![
                pos2(c.x - s * 0.25, c.y - s * 0.4),
                pos2(c.x + s * 0.35, c.y),
                pos2(c.x - s * 0.25, c.y + s * 0.4),
            ];
            painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
        }
    }
}

pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    active: bool,
    accent: Color32,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 20.0), Sense::click());
    if ui.is_rect_visible(rect) {
        paint_transport_cap(ui, rect, &response, icon, active.then_some(accent));
    }
    response.on_hover_text(tooltip)
}

pub fn transport_button(ui: &mut Ui, icon: Icon, lit: Option<Color32>, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 20.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            lit.is_some(),
            tooltip,
        )
    });
    if ui.is_rect_visible(rect) {
        paint_transport_cap(ui, rect, &response, icon, lit);
    }
    response.on_hover_text(tooltip)
}

fn paint_transport_cap(ui: &Ui, rect: Rect, response: &Response, icon: Icon, lit: Option<Color32>) {
    let painter = ui.painter();
    let cap = analog::push_button(
        painter,
        rect.shrink2(vec2(1.0, 1.5)),
        2.5,
        lit,
        response.hovered(),
        response.is_pointer_button_down_on(),
    );
    let glyph = cap.shrink2(vec2(5.0, 3.0));
    if lit.is_none() {
        paint_icon(
            painter,
            glyph.translate(vec2(0.0, -0.6)),
            icon,
            Color32::from_black_alpha(180),
        );
    }
    let color = if lit.is_some() {
        theme::TEXT_ON_COLOR
    } else if icon == Icon::Record {
        theme::ERROR
    } else if response.hovered() {
        Color32::WHITE
    } else {
        theme::TEXT
    };
    paint_icon(painter, glyph, icon, color);
}

pub fn toggle(ui: &mut Ui, text: &str, on: bool, color: Color32, size: Vec2) -> Response {
    let font = theme::prop(10.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), theme::TEXT);
    let size = vec2(size.x.max(galley.size().x + 12.0), size.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, text)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let cap = analog::push_button(
            painter,
            rect.shrink2(vec2(1.0, 1.5)),
            2.5,
            on.then_some(color),
            response.hovered(),
            response.is_pointer_button_down_on(),
        );
        analog::legend(painter, cap, text, font, on);
    }
    response
}

pub fn value_box(ui: &mut Ui, text: &str, width: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        ui.painter().add(analog::readout(
            rect,
            response.hovered() || response.dragged(),
        ));
        fitted_text(
            ui.painter(),
            rect.shrink2(vec2(4.0, 0.0)),
            text,
            theme::mono(11.5),
            color,
            Align2::CENTER_CENTER,
        );
    }
    response.on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
}

pub fn activator(ui: &mut Ui, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
    if ui.is_rect_visible(rect) {
        analog::round_switch(
            ui.painter(),
            rect.center(),
            5.0,
            on.then_some(theme::ACCENT),
            response.hovered(),
            response.is_pointer_button_down_on(),
        );
    }
    response.on_hover_text(if on {
        "Device on. Click to bypass."
    } else {
        "Device bypassed. Click to switch it on."
    })
}

pub fn color_bar(painter: &egui::Painter, rect: Rect, color: Color32, text: &str) {
    analog::gradient_rect(
        painter,
        rect,
        Color32::from_rgb(0x36, 0x36, 0x36),
        Color32::from_rgb(0x27, 0x27, 0x27),
    );
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(170)),
    );
    let tab = Rect::from_min_size(
        rect.min + vec2(0.0, 1.0),
        vec2(4.0, (rect.height() - 2.0).max(0.0)),
    );
    painter.rect_filled(tab, 0.0, color);
    fitted_text(
        painter,
        Rect::from_min_max(
            pos2(tab.right() + 6.0, rect.top()),
            rect.max - vec2(5.0, 0.0),
        ),
        text,
        theme::prop(10.5),
        theme::TEXT,
        Align2::LEFT_CENTER,
    );
}

fn arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, stroke: Stroke) {
    let steps = 24;
    let points: Vec<Pos2> = (0..=steps)
        .map(|step| {
            let t = from + (to - from) * step as f32 / steps as f32;
            center + radius * vec2(t.sin(), -t.cos())
        })
        .collect();
    painter.add(Shape::line(points, stroke));
}

const KNOB_RANGE: f32 = 2.36;

fn paint_knob(painter: &egui::Painter, center: Pos2, radius: f32, value: f32, bipolar: bool) {
    arc(
        painter,
        center,
        radius,
        -KNOB_RANGE,
        KNOB_RANGE,
        Stroke::new(1.0, Color32::from_black_alpha(150)),
    );
    let angle = -KNOB_RANGE + value.clamp(0.0, 1.0) * 2.0 * KNOB_RANGE;
    let origin = if bipolar { 0.0 } else { -KNOB_RANGE };
    if (angle - origin).abs() > 0.01 {
        let (from, to) = (angle.min(origin), angle.max(origin));
        arc(
            painter,
            center,
            radius,
            from,
            to,
            Stroke::new(3.0, theme::ACCENT.gamma_multiply(0.18)),
        );
        arc(
            painter,
            center,
            radius,
            from,
            to,
            Stroke::new(1.5, theme::ACCENT),
        );
    }
    if bipolar {
        let tick = center + vec2(0.0, -(radius + 1.0));
        painter.line_segment(
            [tick, tick - vec2(0.0, 2.0)],
            Stroke::new(1.0, theme::TEXT_DIM),
        );
    }
    analog::knob_cap(painter, center, (radius - 3.0).max(3.0), angle, false);
}

pub fn pan_knob(ui: &mut Ui, value: &mut f32, diameter: f32) -> Response {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::splat(diameter), Sense::click_and_drag());
    if response.dragged() {
        let delta = response.drag_delta();
        let speed = if ui.input(|i| i.modifiers.shift) {
            0.002
        } else {
            0.01
        };
        let next = (*value + (delta.x - delta.y) * speed).clamp(-1.0, 1.0);
        if next != *value {
            *value = next;
            response.mark_changed();
        }
    }
    if response.double_clicked() && *value != 0.0 {
        *value = 0.0;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        paint_knob(
            ui.painter(),
            rect.center(),
            diameter * 0.5 - 2.0,
            (*value + 1.0) * 0.5,
            true,
        );
    }
    let hint = format!("Pan {}", format::pan(*value));
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(hint)
}

pub fn knob(ui: &mut Ui, value: &mut f32, label: &str, display: &str) -> Response {
    let size = vec2(48.0, 58.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if response.dragged() {
        let speed = if ui.input(|i| i.modifiers.shift) {
            0.001
        } else {
            0.006
        };
        let delta = response.drag_delta();
        let next = (*value + (delta.x - delta.y) * speed).clamp(0.0, 1.0);
        if next != *value {
            *value = next;
            response.mark_changed();
        }
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        fitted_text(
            painter,
            Rect::from_min_size(rect.min, vec2(rect.width(), 13.0)),
            label,
            theme::prop(10.0),
            theme::TEXT_DIM,
            Align2::CENTER_CENTER,
        );
        let center = pos2(rect.center().x, rect.top() + 29.0);
        paint_knob(painter, center, 12.5, *value, false);
        let color = if response.hovered() || response.dragged() {
            Color32::WHITE
        } else {
            theme::TEXT
        };
        fitted_text(
            painter,
            Rect::from_min_max(pos2(rect.left(), rect.bottom() - 13.0), rect.max),
            display,
            theme::prop(10.0),
            color,
            Align2::CENTER_CENTER,
        );
    }
    response.on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
}

pub fn gain_fader(ui: &mut Ui, gain: &mut f32, size: Vec2, color: Color32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let center_x = rect.right() - 15.0;
    let track = Rect::from_center_size(
        pos2(center_x, rect.center().y),
        vec2(7.0, (rect.height() - 22.0).max(1.0)),
    );
    let to_y = |position: f32| egui::lerp(track.bottom()..=track.top(), position);
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let delta = -response.drag_delta().y / track.height() * if fine { 0.2 } else { 1.0 };
        let position = (format::gain_to_fader(*gain) + delta).clamp(0.0, 1.0);
        let next = format::fader_to_gain(position);
        if next != *gain {
            *gain = next;
            response.mark_changed();
        }
    }
    if response.double_clicked() && *gain != 1.0 {
        *gain = 1.0;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        for (db, label) in [
            (6.0, "+6"),
            (0.0, "0"),
            (-6.0, "−6"),
            (-12.0, "−12"),
            (-24.0, "−24"),
            (-48.0, "−48"),
        ] {
            if track.height() < 100.0 && matches!(db, 6.0 | -6.0 | -24.0) {
                continue;
            }
            let y = to_y(format::gain_to_fader(format::db_to_gain(db)));
            let ink = if db == 0.0 {
                theme::TEXT
            } else {
                theme::TEXT_FAINT
            };
            let width = if db == 0.0 { 6.0 } else { 4.0 };
            painter.line_segment(
                [
                    pos2(track.left() - width - 5.0, y),
                    pos2(track.left() - 5.0, y),
                ],
                Stroke::new(1.0, ink),
            );
            if rect.width() >= 50.0 {
                painter.text(
                    pos2(track.left() - 13.0, y),
                    Align2::RIGHT_CENTER,
                    label,
                    theme::mono(8.0),
                    ink,
                );
            }
        }
        analog::well(painter, track.expand2(vec2(1.0, 2.0)), 3.0);
        painter.vline(
            center_x + 1.0,
            track.y_range(),
            Stroke::new(1.0, Color32::from_gray(24)),
        );
        if rect.width() >= 50.0 && track.height() >= 100.0 {
            painter.text(
                pos2(track.left() - 13.0, track.bottom()),
                Align2::RIGHT_CENTER,
                "−∞",
                theme::mono(8.0),
                theme::TEXT_FAINT,
            );
        }
        let position = format::gain_to_fader(*gain);
        let y = to_y(position);
        let hot = if response.hovered() || response.dragged() {
            Color32::from_gray(0xf4)
        } else {
            Color32::from_gray(0xd0)
        };
        let cap = Rect::from_center_size(pos2(center_x, y), vec2(24.0, 18.0));
        painter.rect_filled(
            cap.translate(vec2(0.0, 2.0)).expand(1.0),
            2.0,
            Color32::from_black_alpha(160),
        );
        painter.rect_filled(cap, 2.0, Color32::from_gray(0x45));
        let face = cap.shrink(1.0);
        analog::gradient_rect(painter, face, hot, Color32::from_gray(0x79));
        painter.rect_stroke(
            cap,
            2.0,
            Stroke::new(1.0, Color32::from_gray(0x12)),
            StrokeKind::Inside,
        );
        painter.hline(
            face.x_range(),
            face.top(),
            Stroke::new(1.0, Color32::from_white_alpha(160)),
        );
        for offset in [-5.0, 5.0] {
            let range = (cap.left() + 4.0)..=(cap.right() - 4.0);
            painter.hline(
                range.clone(),
                y + offset,
                Stroke::new(1.0, Color32::from_black_alpha(100)),
            );
            painter.hline(
                range,
                y + offset + 1.0,
                Stroke::new(1.0, Color32::from_white_alpha(110)),
            );
        }
        painter.hline(
            face.x_range(),
            y,
            Stroke::new(3.0, Color32::from_gray(0x26)),
        );
        painter.hline(
            (cap.left() + 3.0)..=(cap.right() - 3.0),
            y,
            Stroke::new(1.0, color),
        );
    }
    let hint = format!(
        "Volume {}\nShift: fine adjustment · Double-click: 0 dB",
        format::db(*gain)
    );
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(hint)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MeterState {
    level: [f32; 2],
    hold: [f32; 2],
    hold_age: [f32; 2],
    pub clipped: bool,
}

impl MeterState {
    pub fn update(&mut self, peaks: [f32; 2], dt: f32) {
        for (channel, peak) in peaks.into_iter().enumerate() {
            let fallen = self.level[channel] * 10_f32.powf(-1.3 * dt);
            self.level[channel] = peak.max(fallen);
            self.hold_age[channel] += dt;
            if peak >= self.hold[channel] || self.hold_age[channel] > 1.5 {
                self.hold[channel] = peak.max(fallen);
                self.hold_age[channel] = 0.0;
            }
            if peak > 1.0 {
                self.clipped = true;
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.level
            .iter()
            .chain(&self.hold)
            .any(|level| *level > 1e-4)
    }
}

const METER_FLOOR_DB: f32 = -60.0;
const METER_CEIL_DB: f32 = 6.0;

fn meter_fraction(level: f32) -> f32 {
    let db = format::gain_to_db(level).max(METER_FLOOR_DB);
    ((db - METER_FLOOR_DB) / (METER_CEIL_DB - METER_FLOOR_DB)).clamp(0.0, 1.0)
}

fn meter_color(fraction: f32) -> Color32 {
    let zero = meter_fraction(1.0);
    let minus_six = meter_fraction(0.5);
    if fraction > zero {
        theme::ERROR
    } else if fraction > minus_six {
        theme::WARN
    } else {
        theme::OK
    }
}

pub fn meter(ui: &mut Ui, state: &mut MeterState, size: Vec2, vertical: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    paint_meter(ui, state, rect, response, vertical)
}

pub fn meter_at(
    ui: &mut Ui,
    state: &mut MeterState,
    rect: Rect,
    id: Id,
    vertical: bool,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    paint_meter(ui, state, rect, response, vertical)
}

fn paint_meter(
    ui: &mut Ui,
    state: &mut MeterState,
    rect: Rect,
    response: Response,
    vertical: bool,
) -> Response {
    if response.clicked() {
        state.clipped = false;
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let light = 5.0;
    let gap = 1.0;
    let (bars_rect, light_rect) = if vertical {
        (
            Rect::from_min_max(pos2(rect.left(), rect.top() + light + gap), rect.max),
            Rect::from_min_size(rect.min, vec2(rect.width(), light)),
        )
    } else {
        (
            Rect::from_min_max(rect.min, pos2(rect.right() - light - gap, rect.bottom())),
            Rect::from_min_size(
                pos2(rect.right() - light, rect.top()),
                vec2(light, rect.height()),
            ),
        )
    };
    analog::lamp(painter, light_rect.shrink(0.5), theme::ERROR, state.clipped);
    analog::well(painter, bars_rect, 1.5);

    let inner = bars_rect.shrink(1.0);
    let length = if vertical {
        inner.height()
    } else {
        inner.width()
    };
    let size = if length >= 120.0 { 3.0 } else { 2.0 };
    let pitch = size + 1.0;
    let count = ((length + 1.0) / pitch).floor().max(1.0) as usize;
    let margin = ((length - (count as f32 * pitch - 1.0)) / 2.0).max(0.0);
    let mut glows = egui::Mesh::default();
    let mut leds = egui::Mesh::default();
    for channel in 0..2 {
        let lane = if vertical {
            let width = (inner.width() - gap) / 2.0;
            Rect::from_min_size(
                pos2(inner.left() + channel as f32 * (width + gap), inner.top()),
                vec2(width, inner.height()),
            )
        } else {
            let height = (inner.height() - gap) / 2.0;
            Rect::from_min_size(
                pos2(inner.left(), inner.top() + channel as f32 * (height + gap)),
                vec2(inner.width(), height),
            )
        };
        let level = state.level[channel];
        let fraction = meter_fraction(level);
        let hold = (state.hold[channel] > 1e-5).then(|| {
            ((meter_fraction(state.hold[channel]) * count as f32) as usize).min(count - 1)
        });
        for index in 0..count {
            let from = index as f32 / count as f32;
            let start = margin + index as f32 * pitch;
            let led = if vertical {
                Rect::from_min_max(
                    pos2(lane.left(), lane.bottom() - start - size),
                    pos2(lane.right(), lane.bottom() - start),
                )
            } else {
                Rect::from_min_max(
                    pos2(lane.left() + start, lane.top()),
                    pos2(lane.left() + start + size, lane.bottom()),
                )
            };
            let color = meter_color((index as f32 + 0.5) / count as f32);
            let lit = (level > 1e-5 && fraction > from) || hold == Some(index);
            if lit {
                analog::led_glow(&mut glows, led, color);
            }
            analog::led(&mut leds, led, color, lit, vertical);
        }
    }
    painter.add(Shape::mesh(glows));
    painter.add(Shape::mesh(leds));
    analog::glass(painter, inner);
    response.on_hover_text(format!(
        "Peak level — L: {}, R: {}\nClick to reset the clip light.",
        format::db(state.level[0]),
        format::db(state.level[1]),
    ))
}

pub fn inside<R>(
    ui: &mut Ui,
    rect: Rect,
    layout: egui::Layout,
    contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    contents(&mut child)
}

pub fn fitted_text(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: egui::FontId,
    color: Color32,
    align: Align2,
) {
    let Some(galley) = fit_galley(painter, rect.width(), text, &font, color) else {
        return;
    };
    let position = align
        .anchor_size(align.pos_in_rect(&rect), galley.size())
        .min;
    painter.galley(position, galley, color);
}

/// Lays out `text`, eliding it with "…" until it is at most `width` wide.
///
/// This runs for every visible clip label on every frame, so it must stay
/// cheap even when thousands of labels are too wide: it bails out before any
/// layout when there is no room, and finds the longest fitting prefix with a
/// binary search instead of trying every length.
fn fit_galley(
    painter: &egui::Painter,
    width: f32,
    text: &str,
    font: &egui::FontId,
    color: Color32,
) -> Option<std::sync::Arc<egui::Galley>> {
    if width.is_nan() || width <= 0.0 {
        return None;
    }
    let layout = |shown: String| painter.layout_no_wrap(shown, font.clone(), color);
    let full = layout(text.to_owned());
    if full.size().x <= width {
        return Some(full);
    }
    let count = text.chars().count();
    if count <= 1 {
        return None;
    }
    let elided = |keep: usize| {
        let end = text
            .char_indices()
            .nth(keep)
            .map_or(text.len(), |(index, _)| index);
        layout(format!("{}…", text[..end].trim_end()))
    };
    let mut best = elided(1);
    if best.size().x > width {
        return None;
    }
    let (mut low, mut high) = (1, count - 1);
    while low < high {
        let middle = (low + high).div_ceil(2);
        let candidate = elided(middle);
        if candidate.size().x <= width {
            low = middle;
            best = candidate;
        } else {
            high = middle - 1;
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The previous linear elision, kept as the reference behaviour.
    fn linear_fit(painter: &egui::Painter, width: f32, text: &str) -> Option<String> {
        let font = theme::prop(10.0);
        let mut shown = text.to_owned();
        let mut galley = painter.layout_no_wrap(shown.clone(), font.clone(), Color32::WHITE);
        while galley.size().x > width && shown.chars().count() > 1 {
            shown.pop();
            while shown.ends_with(char::is_whitespace) {
                shown.pop();
            }
            galley = painter.layout_no_wrap(format!("{shown}…"), font.clone(), Color32::WHITE);
        }
        (galley.size().x <= width).then(|| galley.text().to_owned())
    }

    #[test]
    fn fitted_labels_fit_and_appear_exactly_when_the_linear_search_found_room() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let painter = ui.painter();
            let font = theme::prop(10.0);
            for text in [
                "NT Closed Hat",
                "Bass  Phase Current",
                "a b",
                "x",
                "Ünïcødé ✓",
            ] {
                let mut width = -8.0;
                while width < 160.0 {
                    let fitted = fit_galley(painter, width, text, &font, Color32::WHITE);
                    let reference = linear_fit(painter, width, text);
                    assert_eq!(fitted.is_some(), reference.is_some(), "{text:?} at {width}");
                    if let Some(galley) = fitted {
                        assert!(galley.size().x <= width, "{text:?} at {width}");
                    }
                    width += 0.5;
                }
            }
            assert_eq!(
                fit_galley(painter, 1000.0, "NT Closed Hat", &font, Color32::WHITE)
                    .map(|galley| galley.text().to_owned()),
                Some("NT Closed Hat".to_owned())
            );
            assert!(fit_galley(painter, -4.0, "NT Closed Hat", &font, Color32::WHITE).is_none());
            assert!(
                fit_galley(painter, f32::NAN, "NT Closed Hat", &font, Color32::WHITE).is_none()
            );
        });
        output.textures_delta.clear();
    }
}
