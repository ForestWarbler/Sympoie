use eframe::egui::{
    Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};

fn rounded_rect_points(rect: Rect, radius: f32) -> Vec<Pos2> {
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let steps = 4;
    let corners = [
        (pos2(rect.left() + radius, rect.top() + radius), 180.0_f32),
        (pos2(rect.right() - radius, rect.top() + radius), 270.0),
        (pos2(rect.right() - radius, rect.bottom() - radius), 0.0),
        (pos2(rect.left() + radius, rect.bottom() - radius), 90.0),
    ];
    let mut points = Vec::with_capacity(4 * (steps + 1));
    for (center, start) in corners {
        for step in 0..=steps {
            let angle = (start + 90.0 * step as f32 / steps as f32).to_radians();
            points.push(center + radius * vec2(angle.cos(), angle.sin()));
        }
    }
    points
}

fn circle_points(center: Pos2, radius: f32) -> Vec<Pos2> {
    let steps = 28;
    (0..steps)
        .map(|step| {
            let angle = std::f32::consts::TAU * step as f32 / steps as f32;
            center + radius * vec2(angle.cos(), angle.sin())
        })
        .collect()
}

fn fan(painter: &Painter, outline: &[Pos2], hub: Pos2, shade: impl Fn(Pos2) -> Color32) {
    if outline.len() < 3 {
        return;
    }
    let mut mesh = Mesh::default();
    mesh.colored_vertex(hub, shade(hub));
    for point in outline {
        mesh.colored_vertex(*point, shade(*point));
    }
    let count = outline.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % count);
    }
    painter.add(Shape::mesh(mesh));
}

fn vertical(rect: Rect, top: Color32, bottom: Color32) -> impl Fn(Pos2) -> Color32 {
    move |point| {
        let t = ((point.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
        top.lerp_to_gamma(bottom, t)
    }
}

pub fn gradient_rect(painter: &Painter, rect: Rect, top: Color32, bottom: Color32) {
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
    painter.add(Shape::mesh(mesh));
}

fn lens_rect(mesh: &mut Mesh, rect: Rect, edge: Color32, middle: Color32, vertical: bool) {
    let (a, b) = if vertical {
        let mid = rect.center().x;
        (
            [rect.left_top(), pos2(mid, rect.top()), rect.right_top()],
            [
                rect.left_bottom(),
                pos2(mid, rect.bottom()),
                rect.right_bottom(),
            ],
        )
    } else {
        let mid = rect.center().y;
        (
            [rect.left_top(), pos2(rect.left(), mid), rect.left_bottom()],
            [
                rect.right_top(),
                pos2(rect.right(), mid),
                rect.right_bottom(),
            ],
        )
    };
    let first = mesh.vertices.len() as u32;
    for (index, color) in [edge, middle, edge].into_iter().enumerate() {
        mesh.colored_vertex(a[index], color);
        mesh.colored_vertex(b[index], color);
    }
    for stripe in 0..2 {
        let base = first + stripe * 2;
        mesh.add_triangle(base, base + 1, base + 3);
        mesh.add_triangle(base, base + 3, base + 2);
    }
}

pub fn well(painter: &Painter, rect: Rect, radius: f32) {
    let outline = rounded_rect_points(rect, radius);
    fan(
        painter,
        &outline,
        rect.center(),
        vertical(rect, Color32::from_gray(0x03), Color32::from_gray(0x12)),
    );
    painter.rect_stroke(
        rect.expand(1.0),
        radius + 1.0,
        Stroke::new(1.0, Color32::from_white_alpha(18)),
        StrokeKind::Inside,
    );
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(200)),
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, Color32::from_black_alpha(220)),
        StrokeKind::Inside,
    );
}

pub fn glass(painter: &Painter, rect: Rect) {
    let upper = Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y));
    gradient_rect(
        painter,
        upper.shrink2(vec2(1.0, 0.0)),
        Color32::from_white_alpha(26),
        Color32::from_white_alpha(0),
    );
}

pub fn readout(rect: Rect, highlighted: bool) -> Shape {
    let inner = rect.shrink(1.0);
    let mut glass = Mesh::default();
    for (point, color) in [
        (inner.left_top(), Color32::from_gray(0x24)),
        (inner.right_top(), Color32::from_gray(0x24)),
        (inner.right_bottom(), Color32::from_gray(0x0d)),
        (inner.left_bottom(), Color32::from_gray(0x0d)),
    ] {
        glass.colored_vertex(point, color);
    }
    glass.add_triangle(0, 1, 2);
    glass.add_triangle(0, 2, 3);
    Shape::Vec(vec![
        Shape::rect_stroke(
            rect.translate(vec2(0.0, 0.7)),
            2.0,
            Stroke::new(
                1.0,
                Color32::from_white_alpha(if highlighted { 52 } else { 28 }),
            ),
            StrokeKind::Outside,
        ),
        Shape::rect_filled(rect, 2.0, Color32::from_gray(0x07)),
        Shape::mesh(glass),
        Shape::line_segment(
            [inner.left_top(), inner.right_top()],
            Stroke::new(1.0, Color32::from_black_alpha(160)),
        ),
        Shape::line_segment(
            [inner.left_bottom(), inner.right_bottom()],
            Stroke::new(1.0, Color32::from_white_alpha(12)),
        ),
        Shape::rect_stroke(
            rect,
            2.0,
            Stroke::new(1.0, Color32::from_black_alpha(210)),
            StrokeKind::Inside,
        ),
    ])
}

pub fn led_glow(mesh: &mut Mesh, rect: Rect, color: Color32) {
    mesh.add_colored_rect(rect.expand(1.0), color.gamma_multiply(0.2));
}

pub fn led(mesh: &mut Mesh, rect: Rect, color: Color32, lit: bool, vertical: bool) {
    if lit {
        let hot = color.lerp_to_gamma(Color32::WHITE, 0.45);
        let edge = color.lerp_to_gamma(Color32::BLACK, 0.25);
        lens_rect(mesh, rect, edge, hot, vertical);
    } else {
        let dim = color.lerp_to_gamma(Color32::BLACK, 0.84);
        lens_rect(
            mesh,
            rect,
            dim.lerp_to_gamma(Color32::BLACK, 0.4),
            dim,
            vertical,
        );
    }
}

pub fn lamp(painter: &Painter, rect: Rect, color: Color32, lit: bool) {
    if lit {
        painter.rect_filled(rect.expand(1.5), 2.0, color.gamma_multiply(0.30));
        fan(
            painter,
            &rounded_rect_points(rect, 1.0),
            rect.center(),
            |point| {
                let distance = (point - rect.center()).length() / (rect.size().length() / 2.0);
                color
                    .lerp_to_gamma(Color32::WHITE, 0.55)
                    .lerp_to_gamma(color.lerp_to_gamma(Color32::BLACK, 0.2), distance)
            },
        );
    } else {
        let dark = color.lerp_to_gamma(Color32::BLACK, 0.78);
        fan(
            painter,
            &rounded_rect_points(rect, 1.0),
            rect.center() - vec2(0.0, rect.height() * 0.2),
            |point| {
                let distance = (point - rect.center()).length() / (rect.size().length() / 2.0);
                dark.lerp_to_gamma(Color32::from_gray(0x08), distance)
            },
        );
    }
    painter.rect_stroke(
        rect,
        1.0,
        Stroke::new(1.0, Color32::from_black_alpha(160)),
        StrokeKind::Outside,
    );
}

pub fn round_switch(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    light: Option<Color32>,
    hovered: bool,
    pressed: bool,
) {
    let bezel = radius + 1.0;
    let cap = radius - 0.8;
    let center_cap = center + if pressed { vec2(0.0, 0.5) } else { Vec2::ZERO };

    if let Some(light) = light {
        for (grow, alpha) in [(3.0, 0.10), (2.0, 0.18), (1.0, 0.26)] {
            painter.circle_filled(center, bezel + grow, light.gamma_multiply(alpha));
        }
    } else {
        painter.circle_filled(
            center + vec2(0.0, 1.0),
            bezel + 0.6,
            Color32::from_black_alpha(90),
        );
    }

    let ring = Rect::from_center_size(center, Vec2::splat(bezel * 2.0));
    let (top, bottom) = if hovered {
        (Color32::from_gray(0xb4), Color32::from_gray(0x3a))
    } else {
        (Color32::from_gray(0x96), Color32::from_gray(0x2a))
    };
    fan(
        painter,
        &circle_points(center, bezel),
        center,
        vertical(ring, top, bottom),
    );

    let hub = center_cap + vec2(-cap * 0.3, -cap * 0.4);
    let (hot, rim) = match light {
        Some(light) if pressed => (
            light.lerp_to_gamma(Color32::WHITE, 0.3),
            light.lerp_to_gamma(Color32::BLACK, 0.45),
        ),
        Some(light) => (
            light.lerp_to_gamma(Color32::WHITE, 0.6),
            light.lerp_to_gamma(Color32::BLACK, 0.35),
        ),
        None if pressed => (Color32::from_gray(0x3a), Color32::from_gray(0x0e)),
        None => (Color32::from_gray(0x5c), Color32::from_gray(0x16)),
    };
    let span = cap * 1.8;
    fan(painter, &circle_points(center_cap, cap), hub, |point| {
        let distance = ((point - hub).length() / span).clamp(0.0, 1.0);
        hot.lerp_to_gamma(rim, distance)
    });
    painter.circle_stroke(
        center_cap,
        cap,
        Stroke::new(1.0, Color32::from_black_alpha(150)),
    );
    painter.circle_filled(
        hub + vec2(-0.2, -0.2),
        (cap * 0.28).max(1.0),
        Color32::from_white_alpha(if light.is_some() { 120 } else { 55 }),
    );
    painter.circle_stroke(
        center,
        bezel,
        Stroke::new(1.0, Color32::from_black_alpha(190)),
    );
}

pub fn push_button(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    light: Option<Color32>,
    hovered: bool,
    pressed: bool,
) -> Rect {
    let sunk = light.is_some() || pressed;
    let cap = if sunk {
        rect.translate(vec2(0.0, 0.5))
    } else {
        rect
    };

    if let Some(light) = light {
        painter.rect_filled(rect.expand(2.0), radius + 2.0, light.gamma_multiply(0.18));
    }
    painter.rect_filled(
        rect.expand(1.0),
        radius + 1.0,
        Color32::from_black_alpha(120),
    );
    if !sunk {
        painter.rect_filled(
            rect.translate(vec2(0.0, 1.0)),
            radius,
            Color32::from_black_alpha(110),
        );
    }

    let outline = rounded_rect_points(cap, radius);
    match light {
        Some(light) => {
            let hot = light.lerp_to_gamma(Color32::WHITE, if hovered { 0.62 } else { 0.5 });
            let edge = light.lerp_to_gamma(Color32::BLACK, 0.28);
            let half = cap.size().length() / 2.0;
            fan(painter, &outline, cap.center(), |point| {
                let distance = ((point - cap.center()).length() / half).clamp(0.0, 1.0);
                hot.lerp_to_gamma(edge, distance * distance)
            });
            painter.hline(
                (cap.left() + radius)..=(cap.right() - radius),
                cap.top() + 0.5,
                Stroke::new(1.0, Color32::from_black_alpha(90)),
            );
        }
        None => {
            let (top, bottom) = match (hovered, pressed) {
                (_, true) => (Color32::from_gray(0x3a), Color32::from_gray(0x2a)),
                (true, false) => (Color32::from_gray(0x76), Color32::from_gray(0x40)),
                (false, false) => (Color32::from_gray(0x62), Color32::from_gray(0x34)),
            };
            fan(painter, &outline, cap.center(), vertical(cap, top, bottom));
            if !pressed {
                painter.hline(
                    (cap.left() + radius)..=(cap.right() - radius),
                    cap.top() + 0.5,
                    Stroke::new(1.0, Color32::from_white_alpha(70)),
                );
                painter.hline(
                    (cap.left() + radius)..=(cap.right() - radius),
                    cap.bottom() - 0.5,
                    Stroke::new(1.0, Color32::from_black_alpha(90)),
                );
            }
        }
    }
    painter.rect_stroke(
        cap,
        radius,
        Stroke::new(1.0, Color32::from_black_alpha(200)),
        StrokeKind::Inside,
    );
    cap
}

pub fn knob_cap(painter: &Painter, center: Pos2, radius: f32, angle: f32, hot: bool) {
    painter.circle_filled(
        center + vec2(0.0, 1.2),
        radius + 0.8,
        Color32::from_black_alpha(110),
    );
    let skirt = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
    let (top, bottom) = if hot {
        (Color32::from_gray(0x6c), Color32::from_gray(0x1a))
    } else {
        (Color32::from_gray(0x58), Color32::from_gray(0x14))
    };
    fan(
        painter,
        &circle_points(center, radius),
        center,
        vertical(skirt, top, bottom),
    );
    let face = radius * 0.72;
    let hub = center + vec2(-face * 0.35, -face * 0.45);
    let (light, rim) = if hot {
        (Color32::from_gray(0x5a), Color32::from_gray(0x24))
    } else {
        (Color32::from_gray(0x4a), Color32::from_gray(0x1e))
    };
    fan(painter, &circle_points(center, face), hub, |point| {
        let distance = ((point - hub).length() / (face * 1.8)).clamp(0.0, 1.0);
        light.lerp_to_gamma(rim, distance)
    });
    painter.circle_stroke(
        center,
        face,
        Stroke::new(0.8, Color32::from_black_alpha(120)),
    );
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(1.0, Color32::from_black_alpha(200)),
    );
    let direction = vec2(angle.sin(), -angle.cos());
    painter.line_segment(
        [
            center + direction * radius * 0.25,
            center + direction * (radius - 0.6),
        ],
        Stroke::new(1.6, Color32::from_rgb(0xe6, 0xe6, 0xe6)),
    );
}

pub fn legend(painter: &Painter, rect: Rect, text: &str, font: eframe::egui::FontId, lit: bool) {
    let align = eframe::egui::Align2::CENTER_CENTER;
    if lit {
        painter.text(rect.center(), align, text, font, Color32::from_gray(0x1a));
    } else {
        painter.text(
            rect.center() + vec2(0.0, -0.6),
            align,
            text,
            font.clone(),
            Color32::from_black_alpha(180),
        );
        painter.text(rect.center(), align, text, font, Color32::from_gray(0xd6));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_outlines_stay_inside_their_rectangle() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(18.0, 14.0));
        for radius in [0.0, 2.5, 50.0] {
            let points = rounded_rect_points(rect, radius);
            assert_eq!(points.len(), 20);
            for point in points {
                assert!(
                    rect.expand(1e-3).contains(point),
                    "{point:?} outside {rect:?}"
                );
            }
        }
    }
}
