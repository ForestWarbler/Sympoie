use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Id, Mesh, Painter, Pos2, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, Ui, Vec2, pos2, text::LayoutJob, vec2,
};

use crate::ui::{analog, theme};

pub const PLATE_TOP: Color32 = Color32::from_rgb(0x32, 0x32, 0x32);
pub const PLATE_BOTTOM: Color32 = Color32::from_rgb(0x27, 0x27, 0x27);
pub const INK: Color32 = Color32::from_rgb(0x96, 0x96, 0x96);
pub const INK_BRIGHT: Color32 = Color32::from_rgb(0xe3, 0xe3, 0xe3);
pub const INK_FAINT: Color32 = Color32::from_rgb(0x62, 0x62, 0x62);

pub const ROW: f32 = 16.0;
pub const KNOB_CELL: Vec2 = vec2(40.0, 46.0);

pub fn legend_font() -> FontId {
    theme::prop(8.5)
}

pub fn value_font() -> FontId {
    theme::mono(8.5)
}

pub fn spaced(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    tracking: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: tracking,
            ..Default::default()
        },
    );
    painter.layout_job(job)
}

pub fn print(painter: &Painter, pos: Pos2, align: Align2, text: &str, color: Color32) -> Rect {
    let galley = spaced(painter, text, legend_font(), color, 0.6);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

fn gradient(rect: Rect, top: Color32, bottom: Color32) -> Shape {
    let mut mesh = Mesh::default();
    for (point, color) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), bottom),
        (rect.left_bottom(), bottom),
    ] {
        mesh.colored_vertex(point, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    Shape::mesh(mesh)
}

pub fn faceplate(rect: Rect, accent: Option<Color32>, dimmed: bool) -> Shape {
    let (top, bottom) = if dimmed {
        (
            PLATE_TOP.lerp_to_gamma(theme::BG_PANEL, 0.6),
            PLATE_BOTTOM.lerp_to_gamma(theme::BG_PANEL, 0.6),
        )
    } else {
        (PLATE_TOP, PLATE_BOTTOM)
    };
    let mut shapes = vec![
        Shape::rect_filled(rect, 3.0, bottom),
        gradient(rect.shrink(1.0), top, bottom),
        Shape::line_segment(
            [
                rect.left_top() + vec2(3.0, 1.5),
                rect.right_top() + vec2(-3.0, 1.5),
            ],
            Stroke::new(1.0, Color32::from_white_alpha(22)),
        ),
    ];
    if let Some(accent) = accent {
        let stripe = Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(rect.width() - 2.0, 2.0));
        let color = if dimmed {
            accent.gamma_multiply(0.35)
        } else {
            accent.gamma_multiply(0.85)
        };
        shapes.push(Shape::rect_filled(stripe, 1.0, color));
    }
    shapes.push(Shape::rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, Color32::from_black_alpha(230)),
        StrokeKind::Inside,
    ));
    Shape::Vec(shapes)
}

pub fn rule_vertical(painter: &Painter, x: f32, y: std::ops::RangeInclusive<f32>) {
    painter.vline(
        x,
        y.clone(),
        Stroke::new(1.0, Color32::from_black_alpha(130)),
    );
    painter.vline(x + 1.0, y, Stroke::new(1.0, Color32::from_white_alpha(14)));
}

pub fn section_label(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    let label = print(
        ui.painter(),
        rect.left_center(),
        Align2::LEFT_CENTER,
        text,
        INK_FAINT,
    );
    if label.right() + 6.0 < rect.right() {
        let y = rect.center().y.round() + 0.5;
        ui.painter().hline(
            (label.right() + 5.0)..=rect.right(),
            y,
            Stroke::new(1.0, Color32::from_black_alpha(120)),
        );
        ui.painter().hline(
            (label.right() + 5.0)..=rect.right(),
            y + 1.0,
            Stroke::new(1.0, Color32::from_white_alpha(12)),
        );
    }
}

const SWEEP: f32 = 2.36;

fn arc_points(center: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = ((to - from).abs() * 10.0).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|step| {
            let t = from + (to - from) * step as f32 / steps as f32;
            center + radius * vec2(t.sin(), -t.cos())
        })
        .collect()
}

pub fn knob(
    ui: &mut Ui,
    value: &mut f32,
    bipolar: bool,
    label: &str,
    readout: &str,
    accent: Color32,
) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(KNOB_CELL, Sense::click_and_drag());
    if response.dragged() {
        let speed = if ui.input(|input| input.modifiers.shift) {
            0.0012
        } else {
            0.0065
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
        let hot = response.hovered() || response.dragged();
        let galley = spaced(
            painter,
            label,
            theme::prop(8.0),
            if hot { INK_BRIGHT } else { INK },
            0.1,
        );
        if galley.size().x <= rect.width() + 2.0 {
            painter.galley(
                pos2(rect.center().x - galley.size().x / 2.0, rect.top()),
                galley,
                INK,
            );
        } else {
            crate::ui::widgets::fitted_text(
                painter,
                Rect::from_min_size(rect.min, vec2(rect.width(), 10.0)),
                &label.to_uppercase(),
                theme::prop(8.0),
                if hot { INK_BRIGHT } else { INK },
                Align2::CENTER_TOP,
            );
        }
        let center = pos2(rect.center().x, rect.top() + 22.5);
        let scale = 12.0;
        painter.add(Shape::line(
            arc_points(center, scale, -SWEEP, SWEEP),
            Stroke::new(1.0, Color32::from_black_alpha(150)),
        ));
        for tick in 0..=10 {
            let angle = -SWEEP + 2.0 * SWEEP * tick as f32 / 10.0;
            let direction = vec2(angle.sin(), -angle.cos());
            let major = tick == 0 || tick == 10 || (tick == 5 && bipolar);
            painter.line_segment(
                [
                    center + direction * (scale + 1.0),
                    center + direction * (scale + if major { 3.0 } else { 2.0 }),
                ],
                Stroke::new(1.0, if major { INK } else { INK_FAINT }),
            );
        }
        let angle = -SWEEP + value.clamp(0.0, 1.0) * 2.0 * SWEEP;
        let start = if bipolar { 0.0 } else { -SWEEP };
        if (angle - start).abs() > 0.02 {
            let points = arc_points(center, scale, angle.min(start), angle.max(start));
            painter.add(Shape::line(
                points.clone(),
                Stroke::new(3.0, accent.gamma_multiply(0.18)),
            ));
            painter.add(Shape::line(points, Stroke::new(1.5, accent)));
        }
        analog::knob_cap(painter, center, 8.0, angle, hot);
        crate::ui::widgets::fitted_text(
            painter,
            Rect::from_min_max(
                pos2(rect.left() - 2.0, rect.bottom() - 9.0),
                pos2(rect.right() + 2.0, rect.bottom()),
            ),
            readout,
            value_font(),
            if hot {
                accent.lerp_to_gamma(Color32::WHITE, 0.35)
            } else {
                INK_BRIGHT
            },
            Align2::CENTER_CENTER,
        );
    }
    response.on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
}

pub fn chooser(
    ui: &mut Ui,
    width: f32,
    options: &[&str],
    selected: usize,
    option_id: impl Fn(usize) -> Id,
) -> (Response, Option<usize>) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hot = response.hovered()
            || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
        painter.add(analog::readout(rect, hot));
        crate::ui::widgets::fitted_text(
            painter,
            Rect::from_min_max(rect.min + vec2(5.0, 0.0), rect.max - vec2(12.0, 0.0)),
            options.get(selected).copied().unwrap_or("—"),
            theme::prop(9.5),
            if hot { Color32::WHITE } else { INK_BRIGHT },
            Align2::LEFT_CENTER,
        );
        let c = pos2(rect.right() - 7.0, rect.center().y + 0.5);
        painter.add(Shape::convex_polygon(
            vec![
                c + vec2(-2.5, -1.5),
                c + vec2(2.5, -1.5),
                c + vec2(0.0, 1.5),
            ],
            if hot { INK_BRIGHT } else { INK },
            Stroke::NONE,
        ));
    }
    let mut picked = None;
    egui::Popup::menu(&response)
        .width(width.max(90.0))
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (index, text) in options.iter().enumerate() {
                let option = ui.selectable_label(index == selected, *text);
                ui.interact(option.rect, option_id(index), Sense::hover());
                if option.clicked() {
                    picked = Some(index);
                }
            }
        });
    (response.on_hover_cursor(CursorIcon::PointingHand), picked)
}

pub fn led(painter: &Painter, center: Pos2, radius: f32, color: Color32, lit: bool) {
    if lit {
        painter.circle_filled(center, radius + 2.0, color.gamma_multiply(0.18));
        painter.circle_filled(center, radius, color);
        painter.circle_filled(
            center + vec2(-0.4, -0.5),
            radius * 0.5,
            color.lerp_to_gamma(Color32::WHITE, 0.6),
        );
    } else {
        painter.circle_filled(center, radius, color.lerp_to_gamma(Color32::BLACK, 0.8));
    }
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(0.8, Color32::from_black_alpha(170)),
    );
}

pub fn led_switch(ui: &mut Ui, label: &str, on: bool, accent: Color32) -> Response {
    let galley = spaced(ui.painter(), label, legend_font(), INK, 0.4);
    let width = galley.size().x + 22.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let cap = analog::push_button(
            painter,
            rect.shrink2(vec2(0.5, 1.0)),
            2.0,
            None,
            response.hovered(),
            response.is_pointer_button_down_on(),
        );
        led(
            painter,
            pos2(cap.left() + 7.0, cap.center().y),
            2.3,
            accent,
            on,
        );
        let ink = if on || response.hovered() {
            INK_BRIGHT
        } else {
            INK
        };
        let galley = spaced(painter, label, legend_font(), ink, 0.4);
        painter.galley(
            pos2(cap.left() + 13.0, cap.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
    }
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    response
}

pub fn selector_bank(
    ui: &mut Ui,
    labels: &[&str],
    selected: usize,
    accent: impl Fn(usize) -> Color32,
    tab_id: impl Fn(usize) -> Id,
) -> usize {
    let painter = ui.painter().clone();
    let galleys: Vec<_> = labels
        .iter()
        .map(|label| spaced(&painter, label, legend_font(), INK, 0.5))
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 12.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 2.0;
    let (strip, _) = ui.allocate_exact_size(vec2(total, ROW), Sense::hover());
    analog::well(&painter, strip, 2.5);
    let mut chosen = selected;
    let mut x = strip.left() + 1.0;
    for (index, width) in widths.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(x, strip.top() + 1.0), vec2(*width, ROW - 2.0));
        x += width;
        let response = ui.interact(rect, tab_id(index), Sense::click());
        if response.clicked() {
            chosen = index;
        }
        let engaged = index == chosen;
        let accent = accent(index);
        if engaged {
            painter.rect_filled(rect, 2.0, Color32::from_gray(0x3c));
            analog::gradient_rect(
                &painter,
                rect.shrink(1.0),
                Color32::from_gray(0x44),
                Color32::from_gray(0x30),
            );
            painter.hline(
                (rect.left() + 4.0)..=(rect.right() - 4.0),
                rect.bottom() - 1.5,
                Stroke::new(1.5, accent),
            );
            painter.hline(
                (rect.left() + 2.0)..=(rect.right() - 2.0),
                rect.bottom() - 1.5,
                Stroke::new(3.5, accent.gamma_multiply(0.18)),
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 2.0, Color32::from_white_alpha(10));
        }
        if index > 0 && !engaged && index != chosen + 1 {
            painter.vline(
                rect.left(),
                (rect.top() + 3.0)..=(rect.bottom() - 3.0),
                Stroke::new(1.0, Color32::from_white_alpha(14)),
            );
        }
        let ink = if engaged {
            INK_BRIGHT
        } else if response.hovered() {
            Color32::from_gray(0xc8)
        } else {
            INK
        };
        let galley = spaced(&painter, labels[index], legend_font(), ink, 0.5);
        painter.galley(
            rect.center() - galley.size() / 2.0 - vec2(0.0, 0.5),
            galley,
            ink,
        );
    }
    chosen
}

pub fn screen(painter: &Painter, rect: Rect) -> Rect {
    analog::well(painter, rect, 3.0);
    painter.rect_filled(rect.shrink(1.0), 2.0, Color32::from_rgb(0x0c, 0x0c, 0x0c));
    rect.shrink(1.0)
}

pub fn screen_glass(painter: &Painter, rect: Rect) {
    analog::glass(painter, rect.shrink(1.0));
}

pub fn graticule() -> Stroke {
    Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xc0, 0xc0, 0xc0, 16))
}

pub fn trace(painter: &Painter, points: Vec<Pos2>, color: Color32) {
    painter.add(Shape::line(
        points.clone(),
        Stroke::new(3.5, color.gamma_multiply(0.16)),
    ));
    painter.add(Shape::line(points, Stroke::new(1.3, color)));
}

#[derive(Clone, Copy)]
pub enum Glyph {
    Up,
    Down,
    Close,
}

pub fn glyph_button(ui: &mut Ui, rect: Rect, id: Id, glyph: Glyph, enabled: bool) -> Response {
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let painter = ui.painter();
    let hot = enabled && response.hovered();
    if hot {
        painter.rect_filled(rect, 2.0, Color32::from_white_alpha(18));
    }
    let color = if !enabled {
        Color32::from_white_alpha(28)
    } else if hot {
        if matches!(glyph, Glyph::Close) {
            theme::ERROR
        } else {
            INK_BRIGHT
        }
    } else {
        INK
    };
    let c = rect.center();
    let stroke = Stroke::new(1.3, color);
    match glyph {
        Glyph::Up => {
            painter.line_segment([c + vec2(-3.5, 1.5), c + vec2(0.0, -2.0)], stroke);
            painter.line_segment([c + vec2(0.0, -2.0), c + vec2(3.5, 1.5)], stroke);
        }
        Glyph::Down => {
            painter.line_segment([c + vec2(-3.5, -1.5), c + vec2(0.0, 2.0)], stroke);
            painter.line_segment([c + vec2(0.0, 2.0), c + vec2(3.5, -1.5)], stroke);
        }
        Glyph::Close => {
            painter.line_segment([c + vec2(-3.0, -3.0), c + vec2(3.0, 3.0)], stroke);
            painter.line_segment([c + vec2(3.0, -3.0), c + vec2(-3.0, 3.0)], stroke);
        }
    }
    response
}

pub fn ladder(
    painter: &Painter,
    rect: Rect,
    fraction: f32,
    hanging: bool,
    color: impl Fn(f32) -> Color32,
) {
    analog::well(painter, rect, 1.5);
    let inner = rect.shrink2(vec2(1.5, 2.0));
    let pitch = 3.0;
    let count = ((inner.height() + 1.0) / pitch).floor().max(1.0) as usize;
    let mut glows = Mesh::default();
    let mut leds = Mesh::default();
    for index in 0..count {
        let position = (index as f32 + 0.5) / count as f32;
        let offset = index as f32 * pitch;
        let led = if hanging {
            Rect::from_min_size(
                pos2(inner.left(), inner.top() + offset),
                vec2(inner.width(), 2.0),
            )
        } else {
            Rect::from_min_max(
                pos2(inner.left(), inner.bottom() - offset - 2.0),
                pos2(inner.right(), inner.bottom() - offset),
            )
        };
        let lit = fraction > index as f32 / count as f32 && fraction > 0.0;
        let c = color(position);
        if lit {
            analog::led_glow(&mut glows, led, c);
        }
        analog::led(&mut leds, led, c, lit, false);
    }
    painter.add(Shape::mesh(glows));
    painter.add(Shape::mesh(leds));
    analog::glass(painter, inner);
}
