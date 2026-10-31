//! The analyser glass: what Reson hears and what it takes out, under the sensitivity curve
//! whose nodes shape where it works.

use std::sync::atomic::{AtomicU32, Ordering};

use egui::{
    Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use sympoie_reson::params::{
    BAND_BASE, BAND_FREQUENCY, BAND_GAIN, BAND_Q, BAND_STRIDE, BANDS, HIGH_CUT, LOW_CUT,
};
use sympoie_reson::{COUNT, PARAMETERS, REDUCTION_BANDS, SPECTRUM_BANDS, Sensitivity, Telemetry};

use crate::paint::{
    self, CORAL, CYAN, FAINT, GLASS_BOTTOM, GLASS_TOP, HAIRLINE, LABEL, PANEL_TOP, TEXT, VIOLET,
    fade, mono, sans, spaced,
};
use crate::{Controls, format, knob};

/// Sensitivity the curve spans, in dB either side of neutral.
const SENSITIVITY_RANGE: f64 = 12.0;
/// Cut, in dB, that reaches the bottom of the glass.
const CUT_RANGE: f32 = 30.0;
/// Levels the spectra span, in dB below full scale.
const LEVEL_RANGE: f32 = 90.0;
/// How long a spectrum that has stopped refreshing holds, then fades, in seconds.
const STALE_HOLD: f64 = 0.4;
const STALE_FADE: f64 = 0.6;
/// Cut, in dB, from which the edge of the resonance cuts is traced.
const AUDIBLE_CUT: f32 = 0.3;
/// How long a note stays named after it was last heard, in seconds.
const NOTE_HOLD: f64 = 0.3;

const NODES: usize = BANDS + 2;
const NODE_LABELS: [&str; NODES] = ["LC", "1", "2", "3", "4", "5", "HC"];
pub(crate) const NODE_NAMES: [&str; NODES] = [
    "Low cut",
    "Low shelf",
    "Bell 1",
    "Bell 2",
    "Bell 3",
    "High shelf",
    "High cut",
];

/// The parameters a node moves: its frequency, and for the bands also gain and Q.
pub(crate) fn node_parameters(node: usize) -> (u32, Option<u32>, Option<u32>) {
    match node {
        0 => (LOW_CUT, None, None),
        node if node <= BANDS => {
            let base = BAND_BASE + (node as u32 - 1) * BAND_STRIDE;
            (
                base + BAND_FREQUENCY,
                Some(base + BAND_GAIN),
                Some(base + BAND_Q),
            )
        }
        _ => (HIGH_CUT, None, None),
    }
}

pub(crate) fn selected_node(ui: &Ui, salt: Id) -> usize {
    ui.ctx()
        .data_mut(|data| data.get_temp::<usize>(salt.with("node")))
        .unwrap_or(1)
        .min(NODES - 1)
}

fn select_node(ui: &Ui, salt: Id, node: usize) {
    ui.ctx()
        .data_mut(|data| data.insert_temp(salt.with("node"), node));
}

fn x_of(graph: Rect, hz: f64) -> f32 {
    let position = ((hz / 20.0).max(1.0e-6).log10() / 3.0) as f32;
    graph.left() + position * graph.width()
}

fn hz_at(graph: Rect, x: f32) -> f64 {
    let position = ((x - graph.left()) / graph.width()).clamp(0.0, 1.0);
    20.0 * 1000.0_f64.powf(f64::from(position))
}

fn band_x(graph: Rect, band: usize, bands: usize) -> f32 {
    graph.left() + band as f32 / (bands - 1) as f32 * graph.width()
}

/// How far a spectrum that has stopped refreshing has faded, from 0 to 1.
fn staleness(ui: &Ui, salt: Id, telemetry: &Telemetry) -> f32 {
    let now = ui.input(|input| input.time);
    let updates = telemetry.spectrum_updates.load(Ordering::Relaxed);
    let key = salt.with("age");
    let (seen, since) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(u32, f64)>(key))
        .unwrap_or((updates, f64::NEG_INFINITY));
    let since = if updates == seen { since } else { now };
    ui.ctx()
        .data_mut(|data| data.insert_temp(key, (updates, since)));
    (((now - since) - STALE_HOLD) / STALE_FADE).clamp(0.0, 1.0) as f32
}

fn read<const N: usize>(cells: &[AtomicU32; N], stale: f32, floor: f32) -> [f32; N] {
    std::array::from_fn(|band| {
        let value = f32::from_bits(cells[band].load(Ordering::Relaxed)).max(floor);
        value + (floor - value) * stale
    })
}

/// Names of the notes Reson hears, lowest first, each held a moment after it was last
/// heard so a note that drops out of a frame or two does not blink.
pub(crate) fn heard_notes(ui: &Ui, salt: Id, telemetry: &Telemetry) -> Vec<String> {
    let now = ui.input(|input| input.time);
    let key = salt.with("notes");
    let updates = telemetry.spectrum_updates.load(Ordering::Relaxed);
    let (seen, mut held) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(u32, Vec<(u8, f64)>)>(key))
        .unwrap_or_default();
    if updates != seen {
        for cell in &telemetry.notes {
            let hz = f32::from_bits(cell.load(Ordering::Relaxed));
            if !(hz.is_finite() && hz > 0.0) {
                continue;
            }
            let note = (69.0 + 12.0 * (hz / 440.0).log2())
                .round()
                .clamp(0.0, 127.0) as u8;
            match held.iter_mut().find(|(held, _)| *held == note) {
                Some((_, heard)) => *heard = now,
                None => held.push((note, now)),
            }
        }
    }
    held.retain(|&(_, heard)| now - heard < NOTE_HOLD);
    held.sort_unstable_by_key(|&(note, _)| note);
    let names = held.iter().map(|&(note, _)| note_name(note)).collect();
    if !held.is_empty() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(NOTE_HOLD));
    }
    ui.ctx()
        .data_mut(|data| data.insert_temp(key, (updates, held)));
    names
}

fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(key % 12)],
        i32::from(key / 12) - 1
    )
}

/// Draws the glass in `rect` at scale `s`: grid, spectra, cuts, the notes, and the
/// sensitivity curve with its nodes. The full panel also floats the picked node's settings
/// over the glass.
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyser(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    telemetry: Option<&Telemetry>,
    salt: Id,
    rect: Rect,
    s: f32,
    compact: bool,
    live: bool,
) {
    let painter = ui.painter_at(rect.expand(2.0 * s));
    paint::gradient_rect(&painter, rect, 8.0 * s, GLASS_TOP, GLASS_BOTTOM);
    let graph = if compact {
        Rect::from_min_max(
            rect.min + vec2(8.0, 6.0) * s,
            rect.max - vec2(8.0, 13.0) * s,
        )
    } else {
        Rect::from_min_max(
            rect.min + vec2(14.0, 12.0) * s,
            rect.max - vec2(14.0, 22.0) * s,
        )
    };
    let inner = painter.with_clip_rect(rect.shrink(1.0));
    // Grid: decades and their twos and fives, labelled along the bottom.
    for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
        let x = x_of(graph, hz);
        let decade = matches!(hz as u32, 100 | 1000 | 10_000);
        inner.vline(
            x,
            graph.y_range(),
            Stroke::new(1.0, fade(Color32::WHITE, if decade { 0.055 } else { 0.03 })),
        );
        if !compact || decade {
            inner.text(
                pos2(x, graph.bottom() + 4.0 * s),
                Align2::CENTER_TOP,
                if hz >= 1000.0 {
                    format!("{}k", hz / 1000.0)
                } else {
                    format!("{hz:.0}")
                },
                mono((if compact { 7.5 } else { 8.5 }) * s),
                FAINT,
            );
        }
    }
    let gain_y = |db: f64| {
        let position = (db / SENSITIVITY_RANGE).clamp(-1.0, 1.0) as f32;
        graph.center().y - position * graph.height() / 2.0
    };
    inner.hline(
        graph.x_range(),
        gain_y(0.0),
        Stroke::new(1.0, fade(Color32::WHITE, 0.035)),
    );
    if let Some(telemetry) = telemetry {
        spectra(ui, &inner, graph, telemetry, salt, s, live);
    }
    let values: [f64; COUNT] = std::array::from_fn(|id| controls.value(id as u32));
    let sensitivity = Sensitivity::from_values(&values);
    let curve: Vec<Pos2> = (0..=240)
        .map(|step| {
            let hz = 20.0 * 1000.0_f64.powf(f64::from(step) / 240.0);
            pos2(x_of(graph, hz), gain_y(sensitivity.db(hz)))
        })
        .collect();
    paint::line(&inner, curve, fade(TEXT, 0.72), 1.4 * s);
    if let Some(telemetry) = telemetry
        && (controls.value(sympoie_reson::FOCUS) > 0.0
            || controls.value(sympoie_reson::CLEAN) > 0.0)
    {
        let notes = heard_notes(ui, salt, telemetry);
        let mut x = graph.left() + 2.0 * s;
        for name in notes {
            let width = (8.0 + 7.0 * name.len() as f32) * s;
            let pill = Rect::from_min_size(pos2(x, graph.top() + 2.0 * s), vec2(width, 15.0 * s));
            inner.rect_filled(pill, 7.5 * s, fade(CYAN, 0.12));
            inner.rect_stroke(
                pill,
                7.5 * s,
                Stroke::new(1.0, fade(CYAN, 0.35)),
                StrokeKind::Inside,
            );
            inner.text(
                pill.center(),
                Align2::CENTER_CENTER,
                name,
                mono(8.5 * s),
                CYAN,
            );
            x = pill.right() + 4.0 * s;
        }
    }
    nodes(
        ui,
        controls,
        &inner,
        graph,
        salt,
        s,
        compact,
        &sensitivity,
        gain_y,
    );
    painter.rect_stroke(
        rect,
        8.0 * s,
        Stroke::new(1.0, HAIRLINE),
        StrokeKind::Inside,
    );
    painter.hline(
        (rect.left() + 12.0 * s)..=(rect.right() - 12.0 * s),
        rect.top() + 1.0,
        Stroke::new(1.0, fade(Color32::WHITE, 0.05)),
    );
}

/// Input and output spectra, Clean's haze and the resonance cuts hanging from the top, and
/// the comb of spared harmonics along the bottom.
fn spectra(
    ui: &Ui,
    painter: &egui::Painter,
    graph: Rect,
    telemetry: &Telemetry,
    salt: Id,
    s: f32,
    live: bool,
) {
    let stale = staleness(ui, salt, telemetry);
    let level_y = |db: f32| {
        graph.bottom() - (db.clamp(-LEVEL_RANGE, 0.0) + LEVEL_RANGE) / LEVEL_RANGE * graph.height()
    };
    let depth_y = |db: f32| graph.top() + (db.max(0.0) / CUT_RANGE).min(1.0) * graph.height();
    let input = read(&telemetry.input_spectrum, stale, -120.0);
    let output = read(&telemetry.spectrum, stale, -120.0);
    // The spectra are drawn as curves through their bands, kept inside the glass where the
    // curve overshoots.
    let curve = |levels: [f32; SPECTRUM_BANDS]| -> Vec<Pos2> {
        let points: Vec<Pos2> = levels
            .iter()
            .enumerate()
            .map(|(band, &db)| pos2(band_x(graph, band, SPECTRUM_BANDS), level_y(db)))
            .collect();
        paint::smooth(&points, 4)
            .into_iter()
            .map(|point| pos2(point.x, point.y.clamp(graph.top(), graph.bottom())))
            .collect()
    };
    let input = curve(input);
    paint::fill_to(painter, &input, graph.bottom(), |point| {
        let height = ((graph.bottom() - point.y) / graph.height()).clamp(0.0, 1.0);
        fade(Color32::WHITE, 0.025 + 0.06 * height)
    });
    painter.add(egui::Shape::line(
        input,
        Stroke::new(1.0, fade(Color32::WHITE, 0.16)),
    ));
    let output = curve(output);
    paint::soft_line(
        painter,
        output,
        fade(CYAN, if live { 0.95 } else { 0.35 }),
        1.3 * s,
    );
    let strength = if live { 1.0 } else { 0.4 };
    let cleaned = read(&telemetry.cleaning_spectrum, stale, 0.0);
    if cleaned.iter().any(|&db| db > 0.05) {
        let haze: Vec<Pos2> = cleaned
            .iter()
            .enumerate()
            .map(|(band, &db)| pos2(band_x(graph, band, REDUCTION_BANDS), depth_y(db)))
            .collect();
        paint::fill_to(painter, &haze, graph.top(), |point| {
            let depth = ((point.y - graph.top()) / graph.height()).clamp(0.0, 1.0);
            fade(VIOLET, strength * (0.04 + 0.26 * depth))
        });
        paint::line(painter, haze, fade(VIOLET, 0.45 * strength), 1.0 * s);
    }
    let cut = read(&telemetry.reduction_spectrum, stale, 0.0);
    let edge: Vec<Pos2> = cut
        .iter()
        .enumerate()
        .map(|(band, &db)| pos2(band_x(graph, band, REDUCTION_BANDS), depth_y(db)))
        .collect();
    if cut.iter().any(|&db| db > 0.05) {
        paint::fill_to(painter, &edge, graph.top(), |point| {
            let depth = ((point.y - graph.top()) / graph.height()).clamp(0.0, 1.0);
            fade(CORAL, strength * (0.05 + 0.6 * depth))
        });
        let mut run: Vec<Pos2> = Vec::new();
        for (index, &point) in edge.iter().enumerate() {
            // Only where a cut is heard; a trace along the top edge would only flicker.
            let cutting = cut[index] > AUDIBLE_CUT
                || index > 0 && cut[index - 1] > AUDIBLE_CUT
                || index + 1 < cut.len() && cut[index + 1] > AUDIBLE_CUT;
            if cutting {
                run.push(point);
            } else if !run.is_empty() {
                paint::line(
                    painter,
                    std::mem::take(&mut run),
                    fade(CORAL, strength),
                    1.3 * s,
                );
            }
        }
        paint::line(painter, run, fade(CORAL, strength), 1.3 * s);
    }
    let spared = read(&telemetry.spared_spectrum, stale, 0.0);
    let step = graph.width() / (REDUCTION_BANDS - 1) as f32;
    let strip = 3.0 * s;
    for (band, &share) in spared.iter().enumerate() {
        if share < 0.02 {
            continue;
        }
        let x = band_x(graph, band, REDUCTION_BANDS);
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x - step / 2.0, graph.bottom() - strip),
                pos2(x + step / 2.0, graph.bottom()),
            ),
            0.0,
            fade(CYAN, 0.6 * share.min(1.0)),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn nodes(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    painter: &egui::Painter,
    graph: Rect,
    salt: Id,
    s: f32,
    compact: bool,
    sensitivity: &Sensitivity,
    gain_y: impl Fn(f64) -> f32,
) {
    let selected = selected_node(ui, salt);
    let radius = if compact { 6.0 } else { 8.0 } * s;
    let mut selected_point = None;
    for node in 0..NODES {
        let (frequency, gain, q) = node_parameters(node);
        let hz = controls.value(frequency);
        let point = pos2(
            x_of(graph, hz),
            match gain {
                Some(gain) => gain_y(controls.value(gain)),
                None => gain_y(sensitivity.db(hz)),
            },
        );
        let response = ui.interact(
            Rect::from_center_size(point, egui::Vec2::splat(2.0 * radius + 8.0 * s)),
            salt.with(("node", node)),
            Sense::click_and_drag(),
        );
        // A band at 0 dB and a cut at its end stop do nothing, and are drawn hollow.
        let active = match gain {
            Some(gain) => controls.value(gain) != 0.0,
            None => hz != PARAMETERS[frequency as usize].default,
        };
        let hot = response.hovered() || response.dragged();
        let picked = node == selected;
        paint::shadow(painter, point, radius, 0.4);
        painter.circle_filled(
            point,
            radius,
            if active {
                if hot {
                    paint::mix(CYAN, Color32::WHITE, 0.15)
                } else {
                    CYAN
                }
            } else {
                paint::BODY_DARK
            },
        );
        painter.circle_stroke(
            point,
            radius,
            Stroke::new(
                1.2 * s,
                if active {
                    fade(Color32::BLACK, 0.6)
                } else if picked {
                    CYAN
                } else {
                    fade(TEXT, 0.55)
                },
            ),
        );
        if picked {
            painter.circle_stroke(point, radius + 3.0 * s, Stroke::new(1.0, fade(CYAN, 0.7)));
        } else if hot {
            painter.circle_stroke(point, radius + 3.0 * s, Stroke::new(1.0, fade(TEXT, 0.3)));
        }
        painter.text(
            point + vec2(0.0, 0.3 * s),
            Align2::CENTER_CENTER,
            NODE_LABELS[node],
            sans((if compact { 6.8 } else { 8.0 }) * s),
            if active {
                Color32::from_rgb(0x06, 0x10, 0x16)
            } else {
                TEXT
            },
        );
        if response.clicked() || response.drag_started() {
            select_node(ui, salt, node);
        }
        if response.drag_started() {
            controls.begin(frequency);
            if let Some(gain) = gain {
                controls.begin(gain);
            }
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            controls.set(frequency, hz_at(graph, pointer.x));
            if let Some(gain) = gain {
                let position = f64::from((graph.center().y - pointer.y) / (graph.height() / 2.0));
                controls.set(gain, (position * SENSITIVITY_RANGE).clamp(-12.0, 12.0));
            }
        }
        if response.drag_stopped() {
            controls.end(frequency);
            if let Some(gain) = gain {
                controls.end(gain);
            }
        }
        if let Some(q) = q
            && response.hovered()
        {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                controls.begin(q);
                controls.set(
                    q,
                    controls.value(q) * 2.0_f64.powf(f64::from(scroll) / 120.0),
                );
                controls.end(q);
            }
        }
        if response.double_clicked() {
            for id in [Some(frequency), gain, q].into_iter().flatten() {
                controls.begin(id);
                controls.set(id, PARAMETERS[id as usize].default);
                controls.end(id);
            }
        }
        if picked {
            selected_point = Some(point);
        }
        response
            .on_hover_and_drag_cursor(CursorIcon::Grab)
            .on_hover_text(match gain {
                Some(_) => format!(
                    "{} · Drag to move, scroll to change Q, double-click to reset.",
                    NODE_NAMES[node]
                ),
                None => format!(
                    "{} · Drag sideways to move, double-click to switch it off.",
                    NODE_NAMES[node]
                ),
            });
    }
    if !compact && let Some(point) = selected_point {
        chip(ui, controls, painter, graph, salt, s, selected, point);
    }
}

/// The picked node's settings, floating over the glass beside it, each draggable.
#[allow(clippy::too_many_arguments)]
fn chip(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    painter: &egui::Painter,
    graph: Rect,
    salt: Id,
    s: f32,
    node: usize,
    point: Pos2,
) {
    let (frequency, gain, q) = node_parameters(node);
    let fields: Vec<u32> = [Some(frequency), gain, q].into_iter().flatten().collect();
    let width = (64.0 + 62.0 * fields.len() as f32) * s;
    let height = 22.0 * s;
    let above = point.y - 18.0 * s - height > graph.top();
    let top = if above {
        point.y - 16.0 * s - height
    } else {
        point.y + 16.0 * s
    };
    let left = (point.x - width / 2.0).clamp(graph.left(), graph.right() - width);
    let rect = Rect::from_min_size(pos2(left, top), vec2(width, height));
    painter.rect_filled(
        rect.translate(vec2(0.0, 2.0 * s)).expand(1.0 * s),
        7.0 * s,
        fade(Color32::BLACK, 0.3),
    );
    painter.rect_filled(rect, 6.0 * s, fade(PANEL_TOP, 0.97));
    painter.rect_stroke(
        rect,
        6.0 * s,
        Stroke::new(1.0, HAIRLINE),
        StrokeKind::Inside,
    );
    spaced(
        painter,
        pos2(rect.left() + 8.0 * s, rect.center().y),
        Align2::LEFT_CENTER,
        NODE_NAMES[node],
        sans(9.4 * s),
        LABEL,
        0.2 * s,
    );
    for (index, &id) in fields.iter().enumerate() {
        let field = Rect::from_min_size(
            pos2(
                rect.left() + (64.0 + 62.0 * index as f32) * s,
                rect.top() + 3.0 * s,
            ),
            vec2(58.0 * s, height - 6.0 * s),
        );
        value_field(ui, controls, painter, salt, id, field, s);
    }
}

/// A value that changes when dragged sideways, resets on double-click and can be typed in.
fn value_field(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    painter: &egui::Painter,
    salt: Id,
    id: u32,
    rect: Rect,
    s: f32,
) {
    let spec = &PARAMETERS[id as usize];
    let key = salt.with(("field", id));
    let response = ui.interact(rect, key, Sense::click_and_drag());
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
        painter.rect_filled(rect, 4.0 * s, fade(CYAN, 0.08));
    }
    let text = if spec.unit.is_empty() {
        format!("Q {}", format::value(spec, controls.value(id)))
    } else {
        format::value(spec, controls.value(id))
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        mono(9.5 * s),
        if hot { CYAN } else { TEXT },
    );
    response
        .on_hover_cursor(CursorIcon::ResizeHorizontal)
        .on_hover_text(format!(
            "{}: drag sideways, double-click to reset, right-click to type.",
            spec.name
        ));
}
