//! Reson's interface, drawn in egui and shared by Sympoie's built-in Reson and the Reson
//! plugin for other hosts.
//!
//! Everything is drawn here, with no host styling, in the graphite look Reson shares with
//! Calor: a charcoal panel a breath cooler than grey, an analyser glass where the output is
//! traced in lagoon blue over the input, the resonance cuts hang soft coral from the top
//! and Clean's cut hangs behind them as a lavender haze, and below it four groups of knobs
//! with lit caps. The sensitivity curve runs across the glass with a node for each band: drag a
//! node to move it, scroll on it to change its Q, double-click it to reset it. The full
//! panel floats the picked node's settings beside it to drag or type; the compact card,
//! made for Sympoie's rack, reads them out under the glass.
//!
//! Both layouts are drawn at a design size and scaled to fit, so they stay crisp and in
//! proportion at any window size. A host connects the interface to Reson through
//! [`Controls`], and hands over Reson's [`Telemetry`] for the glass to draw.

mod analyser;
mod format;
mod knob;
mod paint;

use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
pub use sympoie_reson::Telemetry;
use sympoie_reson::params::{
    ATTACK, CLEAN, DELTA, DEPTH, FOCUS, KNEE, MIX, OUTPUT, RELEASE, STEREO, STEREO_MODES,
    THRESHOLD, WIDTH,
};

use paint::{
    AMBER, BODY_DARK, BODY_LIGHT, CORAL, CYAN, HAIRLINE, LABEL, PANEL_BOTTOM, PANEL_TOP, RAISED,
    RULE, TEXT, VIOLET, WELL, fade, mono, sans, spaced,
};

/// How the interface reads and changes Reson's parameters. Values are in physical units
/// (dB, Hz, ms, %), as `sympoie_reson::PARAMETERS` describes them.
pub trait Controls {
    fn value(&self, id: u32) -> f64;
    /// A change begins: a drag starts, or a single edit is about to be made.
    fn begin(&mut self, id: u32);
    fn set(&mut self, id: u32, value: f64);
    /// The change that `begin` opened is complete.
    fn end(&mut self, id: u32);
    fn bypassed(&self) -> bool;
    fn set_bypassed(&mut self, bypassed: bool);
}

/// A parameter's value as the interface writes it, for a host's own displays.
pub fn format_value(spec: &sympoie_reson::Spec, value: f64) -> String {
    format::value(spec, value)
}

/// Reads a value typed for a parameter: a number, optionally with its unit or a `k`, or a
/// fraction for Width. Anything else is `None`.
pub fn parse_value(spec: &sympoie_reson::Spec, text: &str) -> Option<f64> {
    format::parse(spec, text)
}

/// The full panel's design size; it scales to whatever space it is given.
pub const PANEL_SIZE: Vec2 = vec2(900.0, 500.0);
/// The compact card's design size, for Sympoie's rack.
pub const CARD_SIZE: Vec2 = vec2(444.0, 172.0);

/// A knob in one of the deck's groups: parameter, label, and what it does.
type Knob = (u32, &'static str, &'static str);

const DETECT: [Knob; 4] = [
    (DEPTH, "Depth", "How far Reson turns a resonance down."),
    (
        THRESHOLD,
        "Thresh",
        "How far a peak must stand out from its surroundings before it is cut.",
    ),
    (
        KNEE,
        "Knee",
        "How far past the threshold a peak must rise for the full cut.",
    ),
    (WIDTH, "Width", "How far each cut spreads around its peak."),
];
const NOTES: [Knob; 2] = [
    (
        FOCUS,
        "Focus",
        "Leaves the harmonics of the notes Reson hears alone.",
    ),
    (
        CLEAN,
        "Clean",
        "Turns down the steady noise around the notes.",
    ),
];
const TIME: [Knob; 2] = [
    (ATTACK, "Attack", "How fast a cut comes in."),
    (RELEASE, "Release", "How fast a cut lets go."),
];
const LEVEL: [Knob; 2] = [
    (MIX, "Mix", "How much of the processed sound you hear."),
    (OUTPUT, "Output", "Level after Reson."),
];

/// The accent of a knob's arc: Clean's violet for Clean, cyan for the rest.
fn accent(id: u32) -> Color32 {
    if id == CLEAN { VIOLET } else { CYAN }
}

/// Maps design coordinates into the space the layout was given.
#[derive(Clone, Copy)]
struct Place {
    origin: Pos2,
    s: f32,
}

impl Place {
    fn fit(space: Rect, design: Vec2) -> Self {
        let s = (space.width() / design.x)
            .min(space.height() / design.y)
            .max(0.1);
        let size = design * s;
        Self {
            origin: space.center() - size / 2.0,
            s,
        }
    }

    fn at(self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x, y) * self.s
    }

    fn rect(self, x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect::from_min_size(self.at(x, y), vec2(width, height) * self.s)
    }
}

/// The full panel, filling the space `ui` offers: Sympoie's Reson window, or the plugin's.
/// `salt` keeps this panel's interface state apart from any other Reson's.
pub fn panel(ui: &mut Ui, controls: &mut dyn Controls, telemetry: Option<&Telemetry>, salt: Id) {
    let space = ui.available_rect_before_wrap();
    ui.allocate_rect(space, Sense::hover());
    let painter = ui.painter_at(space);
    paint::gradient_rect(&painter, space, 0.0, PANEL_TOP, PANEL_BOTTOM);
    let place = Place::fit(space, PANEL_SIZE);
    let s = place.s;
    let live = !controls.bypassed();

    // Header: the name, then stereo, Delta and power on the right.
    let mark = spaced(
        &painter,
        place.at(28.0, 30.0),
        Align2::LEFT_CENTER,
        "Reson",
        sans(20.0 * s),
        TEXT,
        0.3 * s,
    );
    spaced(
        &painter,
        pos2(mark.right() + 12.0 * s, mark.center().y + 1.5 * s),
        Align2::LEFT_CENTER,
        "Resonance Suppressor",
        sans(10.5 * s),
        LABEL,
        0.2 * s,
    );
    stereo(ui, controls, salt, place.rect(596.0, 18.0, 168.0, 24.0), s);
    delta(ui, controls, salt, place.rect(776.0, 18.0, 64.0, 24.0), s);
    power(ui, controls, salt, place.at(866.0, 30.0), 12.0 * s);

    analyser::analyser(
        ui,
        controls,
        telemetry,
        salt,
        place.rect(20.0, 58.0, 860.0, 300.0),
        s,
        false,
        live,
    );

    // The deck: four groups of knobs, Notes in the middle with the two larger ones.
    let normal = knob::Look {
        radius: 20.0 * s,
        accent: CYAN,
        hero: false,
        text: s,
    };
    let hero = knob::Look {
        radius: 26.0 * s,
        hero: true,
        ..normal
    };
    let groups: [(&str, f32, f32, &[Knob], f32); 4] = [
        ("Detect", 24.0, 316.0, &DETECT, 72.0),
        ("Notes", 340.0, 556.0, &NOTES, 100.0),
        ("Time", 580.0, 716.0, &TIME, 72.0),
        ("Output", 740.0, 876.0, &LEVEL, 72.0),
    ];
    for (title, left, right, knobs, spacing) in groups {
        let heading = spaced(
            &painter,
            place.at(left, 382.0),
            Align2::LEFT_CENTER,
            &title.to_uppercase(),
            sans(8.4 * s),
            LABEL,
            1.4 * s,
        );
        painter.hline(
            (heading.right() + 10.0 * s)..=place.at(right, 0.0).x,
            heading.center().y,
            Stroke::new(1.0, RULE),
        );
        let first = left + (right - left - spacing * (knobs.len() - 1) as f32) / 2.0;
        for (index, &(id, label, help)) in knobs.iter().enumerate() {
            let look = if title == "Notes" { hero } else { normal };
            let look = knob::Look {
                accent: accent(id),
                ..look
            };
            let centre = place.at(first + spacing * index as f32, 440.0);
            knob::knob(ui, controls, salt, id, centre, look, label, help);
        }
    }
    if !live {
        bypassed_badge(&painter, place.at(870.0, 66.0), s);
    }
    if telemetry.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// The compact card for Sympoie's rack: the glass, the picked node and the stereo and Delta
/// switches in a line, and all ten knobs in a row.
pub fn card(ui: &mut Ui, controls: &mut dyn Controls, telemetry: Option<&Telemetry>, salt: Id) {
    let width = ui.available_width();
    let s = (width / CARD_SIZE.x).clamp(0.5, 1.5);
    let (space, _) = ui.allocate_exact_size(CARD_SIZE * s, Sense::hover());
    let painter = ui.painter_at(space);
    paint::gradient_rect(&painter, space, 8.0 * s, PANEL_TOP, PANEL_BOTTOM);
    painter.rect_stroke(
        space,
        8.0 * s,
        Stroke::new(1.0, HAIRLINE),
        StrokeKind::Inside,
    );
    let place = Place {
        origin: space.min,
        s,
    };
    let live = !controls.bypassed();
    analyser::analyser(
        ui,
        controls,
        telemetry,
        salt,
        place.rect(4.0, 4.0, 436.0, 92.0),
        s,
        true,
        live,
    );
    node_line(
        ui,
        controls,
        &painter,
        salt,
        place.rect(8.0, 100.0, 260.0, 16.0),
        s,
    );
    stereo(ui, controls, salt, place.rect(290.0, 100.0, 102.0, 16.0), s);
    delta(ui, controls, salt, place.rect(398.0, 100.0, 42.0, 16.0), s);
    let look = knob::Look {
        radius: 11.0 * s,
        accent: CYAN,
        hero: false,
        text: 0.82 * s,
    };
    let rows: [(&[Knob], [f32; 4]); 4] = [
        (&DETECT, [26.0, 68.0, 110.0, 152.0]),
        (&NOTES, [200.0, 246.0, 0.0, 0.0]),
        (&TIME, [294.0, 336.0, 0.0, 0.0]),
        (&LEVEL, [382.0, 422.0, 0.0, 0.0]),
    ];
    for (knobs, xs) in rows {
        for (&(id, label, help), &x) in knobs.iter().zip(&xs) {
            let look = if id == FOCUS || id == CLEAN {
                knob::Look {
                    radius: 12.5 * s,
                    hero: true,
                    accent: accent(id),
                    ..look
                }
            } else {
                look
            };
            knob::knob(
                ui,
                controls,
                salt,
                id,
                place.at(x, 144.0),
                look,
                label,
                help,
            );
        }
    }
    for x in [176.0, 270.0, 359.0] {
        painter.vline(
            place.at(x, 0.0).x,
            place.at(0.0, 130.0).y..=place.at(0.0, 166.0).y,
            Stroke::new(1.0, RULE),
        );
    }
    if !live {
        bypassed_badge(&painter, place.at(434.0, 12.0), s);
    }
    if telemetry.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// The picked node's name and settings in a line, each setting draggable.
fn node_line(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    painter: &egui::Painter,
    salt: Id,
    rect: Rect,
    s: f32,
) {
    let node = analyser::selected_node(ui, salt);
    let name = spaced(
        painter,
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        analyser::NODE_NAMES[node],
        sans(9.2 * s),
        LABEL,
        0.2 * s,
    );
    let (frequency, gain, q) = analyser::node_parameters(node);
    let mut x = name.right() + 8.0 * s;
    for id in [Some(frequency), gain, q].into_iter().flatten() {
        let spec = &sympoie_reson::PARAMETERS[id as usize];
        let text = if spec.unit.is_empty() {
            format!("Q {}", format::value(spec, controls.value(id)))
        } else {
            format::value(spec, controls.value(id))
        };
        let width = (12.0 + 6.0 * text.len() as f32) * s;
        let field = Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height()));
        let key = salt.with(("line", id));
        let response = ui.interact(field, key, Sense::click_and_drag());
        if response.drag_started() {
            controls.begin(id);
        }
        if response.dragged() {
            let fine = if ui.input(|input| input.modifiers.shift) {
                0.15
            } else {
                1.0
            };
            let position = spec.normalize(controls.value(id)) as f32;
            let next = (position + response.drag_delta().x * 0.003 * fine).clamp(0.0, 1.0);
            controls.set(id, spec.denormalize(f64::from(next)));
        }
        if response.drag_stopped() {
            controls.end(id);
        }
        if response.double_clicked() {
            controls.begin(id);
            controls.set(id, spec.default);
            controls.end(id);
        }
        knob::entry(controls, &response, id, key);
        let hot = response.hovered() || response.dragged();
        if hot {
            painter.rect_filled(field, 3.0 * s, fade(CYAN, 0.08));
        }
        painter.text(
            field.center(),
            Align2::CENTER_CENTER,
            text,
            mono(8.6 * s),
            if hot { CYAN } else { TEXT },
        );
        response
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text(format!(
                "{}: drag sideways, double-click to reset, right-click to type.",
                spec.name
            ));
        x = field.right() + 2.0 * s;
    }
}

/// The Link · Dual · M/S switch, a segmented control set into the panel.
fn stereo(ui: &mut Ui, controls: &mut dyn Controls, salt: Id, rect: Rect, s: f32) {
    let painter = ui.painter();
    let radius = (6.0 * s).min(rect.height() / 2.0);
    painter.rect_filled(rect, radius, WELL);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, HAIRLINE), StrokeKind::Inside);
    let current = controls.value(STEREO).round() as usize;
    let width = rect.width() / STEREO_MODES.len() as f32;
    for (index, mode) in STEREO_MODES.iter().enumerate() {
        let segment = Rect::from_min_size(
            rect.min + vec2(width * index as f32, 0.0),
            vec2(width, rect.height()),
        );
        let response = ui.interact(segment, salt.with(("stereo", index)), Sense::click());
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
            mode,
            sans(rect.height() * 0.44),
            if on { TEXT } else { LABEL },
            0.2 * s,
        );
        if response.clicked() && !on {
            controls.begin(STEREO);
            controls.set(STEREO, index as f64);
            controls.end(STEREO);
        }
        response.on_hover_text(match index {
            0 => "Link: both channels share one set of cuts",
            1 => "Dual: each channel gets its own cuts",
            _ => "M/S: the middle and the sides get their own cuts",
        });
    }
}

/// Delta: hear only what Reson takes out.
fn delta(ui: &mut Ui, controls: &mut dyn Controls, salt: Id, rect: Rect, s: f32) {
    let on = controls.value(DELTA) >= 0.5;
    let response = ui.interact(rect, salt.with("delta"), Sense::click());
    let painter = ui.painter();
    let radius = (6.0 * s).min(rect.height() / 2.0);
    painter.rect_filled(rect, radius, if on { fade(CORAL, 0.2) } else { WELL });
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, if on { fade(CORAL, 0.55) } else { HAIRLINE }),
        StrokeKind::Inside,
    );
    spaced(
        painter,
        rect.center(),
        Align2::CENTER_CENTER,
        "Delta",
        sans(rect.height() * 0.44),
        if on {
            CORAL
        } else if response.hovered() {
            TEXT
        } else {
            LABEL
        },
        0.2 * s,
    );
    if response.clicked() {
        controls.begin(DELTA);
        controls.set(DELTA, if on { 0.0 } else { 1.0 });
        controls.end(DELTA);
    }
    response.on_hover_text("Delta: hear only what Reson takes out");
}

/// The power button: a small lit cap, its glyph lagoon while Reson runs and amber while it
/// is bypassed.
fn power(ui: &mut Ui, controls: &mut dyn Controls, salt: Id, centre: Pos2, radius: f32) {
    let rect = Rect::from_center_size(centre, Vec2::splat(2.0 * radius + 4.0));
    let response = ui.interact(rect, salt.with("power"), Sense::click());
    let on = !controls.bypassed();
    let painter = ui.painter();
    let color = if on { CYAN } else { AMBER };
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
        "Bypass Reson"
    } else {
        "Turn Reson back on"
    });
}

fn bypassed_badge(painter: &egui::Painter, top_right: Pos2, s: f32) {
    spaced(
        painter,
        top_right,
        Align2::RIGHT_TOP,
        "Bypassed",
        sans(9.5 * s),
        AMBER,
        0.3 * s,
    );
}
