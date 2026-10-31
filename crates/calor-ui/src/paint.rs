//! The graphite palette and the drawing helpers every part of the interface shares.

use egui::epaint::Vertex;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontFamily, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, vec2};

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The panel, from its top edge to its bottom edge: graphite, a breath warmer than grey.
pub(crate) const PANEL_TOP: Color32 = rgb(0x2d, 0x2c, 0x2f);
pub(crate) const PANEL_BOTTOM: Color32 = rgb(0x23, 0x22, 0x25);
/// The glasses, from their top edge to their bottom edge.
pub(crate) const GLASS_TOP: Color32 = rgb(0x1b, 0x1a, 0x1d);
pub(crate) const GLASS_BOTTOM: Color32 = rgb(0x13, 0x12, 0x15);
/// Hairline borders, and the rules and grid lines.
pub(crate) const HAIRLINE: Color32 = rgb(0x41, 0x3f, 0x44);
pub(crate) const RULE: Color32 = rgb(0x38, 0x36, 0x3b);
/// Type: values and headings, labels, and the faintest marks.
pub(crate) const TEXT: Color32 = rgb(0xed, 0xeb, 0xea);
pub(crate) const LABEL: Color32 = rgb(0xa8, 0xa4, 0xa2);
pub(crate) const FAINT: Color32 = rgb(0x70, 0x6c, 0x6b);
/// The curve and the odd harmonics; the even harmonics; a bypassed device. Clear colours,
/// but none at full saturation.
pub(crate) const EMBER: Color32 = rgb(0xe4, 0x97, 0x5e);
pub(crate) const TEAL: Color32 = rgb(0x64, 0xb8, 0xa8);
pub(crate) const GOLD: Color32 = rgb(0xdd, 0xb1, 0x5a);
/// Knob caps, lit from above, from their top to their foot, and the groove their arcs run
/// in.
pub(crate) const BODY_LIGHT: Color32 = rgb(0x53, 0x51, 0x56);
pub(crate) const BODY_DARK: Color32 = rgb(0x2f, 0x2e, 0x32);
pub(crate) const TRACK: Color32 = rgb(0x17, 0x16, 0x19);
/// Fills of switches and meters set into the panel, and of the selected segment.
pub(crate) const WELL: Color32 = rgb(0x1b, 0x1a, 0x1d);
pub(crate) const RAISED: Color32 = rgb(0x4c, 0x4a, 0x4f);
/// Each band's colour, lowest band first: muted, warm to cool, so neighbours stay apart.
pub(crate) const BAND_COLORS: [Color32; 6] = [
    rgb(0xe4, 0x97, 0x5e),
    rgb(0xd6, 0xb2, 0x62),
    rgb(0x9d, 0xbe, 0x7a),
    rgb(0x64, 0xb8, 0xa8),
    rgb(0x6f, 0xa3, 0xd3),
    rgb(0xb1, 0x91, 0xd0),
];

/// Fonts: the hosts install Inter as the proportional face and JetBrains Mono as the
/// monospace one.
pub(crate) fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub(crate) fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// `color` at `alpha` of its opacity.
pub(crate) fn fade(color: Color32, alpha: f32) -> Color32 {
    color.gamma_multiply(alpha.clamp(0.0, 1.0))
}

pub(crate) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Text with extra space between its letters, as small capitals are set.
pub(crate) fn spaced(
    painter: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    spacing: f32,
) -> Rect {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// The outline of a rounded rectangle, clockwise from the top-left corner.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let steps = 6;
    let corners = [
        (rect.left_top() + vec2(radius, radius), std::f32::consts::PI),
        (
            rect.right_top() + vec2(-radius, radius),
            1.5 * std::f32::consts::PI,
        ),
        (rect.right_bottom() + vec2(-radius, -radius), 0.0),
        (
            rect.left_bottom() + vec2(radius, -radius),
            0.5 * std::f32::consts::PI,
        ),
    ];
    let mut points = Vec::with_capacity(4 * (steps + 1));
    for (centre, start) in corners {
        for step in 0..=steps {
            let angle = start + 0.5 * std::f32::consts::PI * step as f32 / steps as f32;
            points.push(centre + radius * vec2(angle.cos(), angle.sin()));
        }
    }
    points
}

/// A rounded rectangle shaded from `top` at its top edge to `bottom` at its bottom edge.
pub(crate) fn gradient_rect(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    top: Color32,
    bottom: Color32,
) {
    let shade = |point: Pos2| mix(top, bottom, (point.y - rect.top()) / rect.height().max(1.0));
    let outline = rounded_outline(rect, radius);
    let mut mesh = Mesh::default();
    let centre = rect.center();
    mesh.vertices.push(Vertex {
        pos: centre,
        uv: egui::epaint::WHITE_UV,
        color: shade(centre),
    });
    for &point in &outline {
        mesh.colored_vertex(point, shade(point));
    }
    let count = outline.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % count);
    }
    painter.add(Shape::mesh(mesh));
}

/// A soft glow: an ellipse of `color` at its centre fading to nothing at its edge.
pub(crate) fn glow(painter: &Painter, centre: Pos2, radii: egui::Vec2, color: Color32) {
    let steps = 40;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(centre, color);
    for step in 0..steps {
        let angle = std::f32::consts::TAU * step as f32 / steps as f32;
        mesh.colored_vertex(
            centre + vec2(angle.cos() * radii.x, angle.sin() * radii.y),
            Color32::TRANSPARENT,
        );
    }
    for step in 0..steps as u32 {
        mesh.add_triangle(0, 1 + step, 1 + (step + 1) % steps as u32);
    }
    painter.add(Shape::mesh(mesh));
}

/// A crisp line.
pub(crate) fn line(painter: &Painter, points: Vec<Pos2>, color: Color32, width: f32) {
    if points.len() >= 2 {
        painter.add(Shape::line(points, Stroke::new(width, color)));
    }
}

/// A line over a soft, faint band of its own colour, for the one trace a display is about.
pub(crate) fn soft_line(painter: &Painter, points: Vec<Pos2>, color: Color32, width: f32) {
    if points.len() < 2 {
        return;
    }
    painter.add(Shape::line(
        points.clone(),
        Stroke::new(width * 3.0, fade(color, 0.07)),
    ));
    painter.add(Shape::line(points, Stroke::new(width, color)));
}

/// A soft shadow under something round at `centre`.
pub(crate) fn shadow(painter: &Painter, centre: Pos2, radius: f32, alpha: f32) {
    glow(
        painter,
        centre + vec2(0.0, radius * 0.12),
        egui::Vec2::splat(radius * 1.25),
        fade(Color32::BLACK, alpha),
    );
}

/// A round cap lit from above: lighter at its top edge, darker at its foot, with a fine
/// dark rim and a faint highlight along its upper edge.
pub(crate) fn cap(painter: &Painter, centre: Pos2, radius: f32, light: Color32, dark: Color32) {
    let steps = 40;
    let shade = |y: f32| mix(light, dark, 0.5 + 0.5 * y / radius);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(centre, shade(0.0));
    for step in 0..steps {
        let angle = std::f32::consts::TAU * step as f32 / steps as f32;
        let offset = radius * vec2(angle.sin(), -angle.cos());
        mesh.colored_vertex(centre + offset, shade(offset.y));
    }
    for step in 0..steps as u32 {
        mesh.add_triangle(0, 1 + step, 1 + (step + 1) % steps as u32);
    }
    painter.add(Shape::mesh(mesh));
    painter.circle_stroke(centre, radius, Stroke::new(1.0, fade(Color32::BLACK, 0.45)));
    painter.add(Shape::line(
        arc(centre, radius - 1.0, -1.1, 1.1),
        Stroke::new(1.0, fade(Color32::WHITE, 0.10)),
    ));
}

/// Points along a circular arc, angles measured clockwise from twelve o'clock.
pub(crate) fn arc(centre: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = ((to - from).abs() * radius / 2.5).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|step| {
            let angle = from + (to - from) * step as f32 / steps as f32;
            centre + radius * vec2(angle.sin(), -angle.cos())
        })
        .collect()
}

/// A filled area between a curve and a horizontal edge, as a strip of quads.
pub(crate) fn fill_to(
    painter: &Painter,
    curve: &[Pos2],
    edge: f32,
    color: impl Fn(Pos2) -> Color32,
) {
    let mut mesh = Mesh::default();
    for pair in curve.windows(2) {
        let base = mesh.vertices.len() as u32;
        for point in [
            pair[0],
            pair[1],
            egui::pos2(pair[1].x, edge),
            egui::pos2(pair[0].x, edge),
        ] {
            mesh.colored_vertex(point, color(point));
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
}
