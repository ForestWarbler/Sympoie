//! Before/after scope for the Vari-Mu: the input and output waveforms on one screen with
//! the gain-reduction trace laid over them, the two spectra on another with the
//! difference shaded, and dynamics readouts computed over the visible window.

use super::*;
use std::sync::Arc;
use sympoie::builtin::{SCOPE_FRAMES_PER_COLUMN, ScopeColumn};

const BEFORE: Color32 = Color32::from_rgb(0xbf, 0xb8, 0xa2);
const AFTER: Color32 = AMBER;
const REDUCTION: Color32 = Color32::from_rgb(0xff, 0x5c, 0x3e);
const ADDED: Color32 = Color32::from_rgb(0x86, 0xd6, 0x8e);
const GLASS_TOP: Color32 = Color32::from_rgb(0x10, 0x12, 0x11);
const GLASS_BOTTOM: Color32 = Color32::from_rgb(0x06, 0x07, 0x07);

const WINDOWS: [f32; 3] = [0.5, 1.0, 2.0];
const SPECTRUM_FLOOR: f32 = -100.0;
const LOG_FLOOR: f32 = -48.0;
const GR_RANGE: f32 = 24.0;

/// What the screens draw: live telemetry, or a copy kept while held.
#[derive(Clone)]
struct Capture {
    columns: Arc<Vec<ScopeColumn>>,
    before: [f32; 128],
    after: [f32; 128],
}

impl Capture {
    fn read(ui: &Ui, key: Id, telemetry: Option<&Telemetry>) -> Self {
        match telemetry {
            Some(t) => Self {
                columns: Arc::new(t.scope.snapshot()),
                before: display_spectrum(
                    ui,
                    key.with("before"),
                    t,
                    &t.input_spectrum,
                    SPECTRUM_FLOOR,
                ),
                after: display_spectrum(ui, key.with("after"), t, &t.spectrum, SPECTRUM_FLOOR),
            },
            None => Self {
                columns: Arc::new(Vec::new()),
                before: [SPECTRUM_FLOOR; 128],
                after: [SPECTRUM_FLOOR; 128],
            },
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Settings {
    window: usize,
    log: bool,
    hold: bool,
}

pub(super) fn show(
    ui: &mut Ui,
    plugin: &PluginInstance,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    const DESIGN: Vec2 = vec2(960.0, 600.0);
    let available = ui.available_rect_before_wrap();
    ui.allocate_rect(available, Sense::hover());
    let s = (available.width() / DESIGN.x)
        .min(available.height() / DESIGN.y)
        .clamp(0.5, 2.0);
    let rect = Rect::from_center_size(available.center(), DESIGN * s);
    let rect = rect.translate(vec2(
        (available.left() - rect.left()).max(0.0),
        (available.top() - rect.top()).max(0.0),
    ));
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    let painter = ui.painter().clone();
    let window = ui.max_rect().expand(8.0).intersect(ui.clip_rect());
    plate(&painter, window, s);
    rack_ears(&painter, window, s);

    let key = Id::new(("varimu-scope", plugin.id));
    let mut settings = ui
        .ctx()
        .data_mut(|data| data.get_temp::<Settings>(key))
        .unwrap_or(Settings {
            window: 2,
            ..Settings::default()
        });

    silk(
        &painter,
        at(30.0, 16.0),
        Align2::LEFT_TOP,
        "Sympoie",
        7.5 * s,
        BRASS,
    );
    silk(
        &painter,
        at(30.0, 27.0),
        Align2::LEFT_TOP,
        "Vari-Mu scope",
        20.0 * s,
        SILK,
    );
    silk(
        &painter,
        at(30.0, 54.0),
        Align2::LEFT_TOP,
        "Before / after monitor",
        7.0 * s,
        SILK_DIM,
    );
    for (x, color, label) in [
        (250.0, BEFORE, "Before"),
        (318.0, AFTER, "After"),
        (380.0, REDUCTION, "Gain reduction"),
    ] {
        let swatch = Rect::from_min_size(at(x, 57.0), vec2(12.0, 4.0) * s);
        painter.rect_filled(swatch, 1.0, color);
        silk(
            &painter,
            at(x + 16.0, 59.0),
            Align2::LEFT_CENTER,
            label,
            6.5 * s,
            SILK,
        );
    }

    for (x, title, labels, selected) in [
        (
            560.0,
            "Window",
            &["0.5 s", "1 s", "2 s"][..],
            settings.window,
        ),
        (
            636.0,
            "Scale",
            &["Lin", "Log"][..],
            usize::from(settings.log),
        ),
        (
            702.0,
            "Display",
            &["Run", "Hold"][..],
            usize::from(settings.hold),
        ),
    ] {
        silk(
            &painter,
            at(x, 14.0),
            Align2::LEFT_TOP,
            title,
            6.0 * s,
            BRASS,
        );
        let picked = toggle(
            ui,
            key.with(title),
            Rect::from_min_size(at(x, 24.0), vec2(58.0, 40.0) * s),
            s,
            labels,
            selected,
        );
        match (title, picked) {
            ("Window", Some(index)) => settings.window = index,
            ("Scale", Some(index)) => settings.log = index == 1,
            ("Display", Some(index)) => settings.hold = index == 1,
            _ => {}
        }
    }
    let controls = plate_button(
        ui,
        Rect::from_min_size(at(790.0, 26.0), vec2(72.0, 18.0) * s),
        key.with("controls"),
        "Controls",
        s,
    );
    if controls
        .on_hover_text("Open the Vari-Mu's controls")
        .clicked()
    {
        actions.push(Action::OpenVst3Editor(plugin.id));
    }
    let bypassed = plugin.bypassed;
    jewel(
        &painter,
        at(900.0, 34.0),
        7.0 * s,
        if bypassed { 0.0 } else { 0.9 },
    );
    silk(
        &painter,
        at(900.0, 48.0),
        Align2::CENTER_TOP,
        if bypassed { "Bypassed" } else { "On" },
        6.0 * s,
        SILK,
    );
    rule(
        &painter,
        at(0.0, 74.0).y,
        at(30.0, 0.0).x..=at(930.0, 0.0).x,
        s,
    );

    let held = key.with("held");
    let capture = if settings.hold {
        let held_capture = ui.ctx().data_mut(|data| data.get_temp::<Capture>(held));
        held_capture.unwrap_or_else(|| {
            let capture = Capture::read(ui, key, telemetry);
            ui.ctx()
                .data_mut(|data| data.insert_temp(held, capture.clone()));
            capture
        })
    } else {
        ui.ctx().data_mut(|data| data.remove::<Capture>(held));
        Capture::read(ui, key, telemetry)
    };
    let span = ((WINDOWS[settings.window] * sympoie::SAMPLE_RATE as f32)
        / SCOPE_FRAMES_PER_COLUMN as f32) as usize;
    let visible = &capture.columns[capture.columns.len().saturating_sub(span)..];

    waveform(
        ui,
        Rect::from_min_max(at(30.0, 86.0), at(930.0, 336.0)),
        key.with("waveform"),
        visible,
        span,
        settings,
        s,
    );
    spectrum(
        ui,
        Rect::from_min_max(at(30.0, 354.0), at(700.0, 584.0)),
        key.with("spectrum"),
        &capture,
        settings.hold,
        s,
    );
    readouts(
        &painter,
        Rect::from_min_max(at(716.0, 354.0), at(930.0, 584.0)),
        visible,
        s,
    );
    ui.ctx().data_mut(|data| data.insert_temp(key, settings));
}

/// Dark phosphor screen with bezel; returns the plotting area.
fn screen(painter: &Painter, rect: Rect, s: f32) -> Rect {
    painter.rect_filled(
        rect.expand(3.0 * s),
        6.0 * s,
        Color32::from_black_alpha(210),
    );
    analog::gradient_rect(painter, rect, GLASS_TOP, GLASS_BOTTOM);
    let step = (3.0 * s).max(2.0);
    let mut y = rect.top();
    while y < rect.bottom() {
        painter.hline(
            rect.x_range(),
            y,
            Stroke::new(1.0, Color32::from_black_alpha(26)),
        );
        y += step;
    }
    rect.shrink2(vec2(40.0, 18.0) * s)
}

fn screen_glass(painter: &Painter, rect: Rect, s: f32) {
    analog::glass(painter, rect);
    painter.rect_stroke(
        rect,
        3.0 * s,
        Stroke::new(1.0, Color32::from_black_alpha(220)),
        egui::StrokeKind::Inside,
    );
}

fn graticule() -> Stroke {
    Stroke::new(1.0, AMBER.gamma_multiply(0.09))
}

fn scale_label(painter: &Painter, pos: Pos2, align: Align2, text: &str, s: f32, color: Color32) {
    painter.text(pos, align, text, theme::mono(7.5 * s), color);
}

/// Display height for a signed sample, either linear or on a −48 dB log scale.
fn amplitude_to_unit(sample: f32, log: bool) -> f32 {
    if !log {
        return sample.clamp(-1.2, 1.2);
    }
    let magnitude = sample.abs();
    if magnitude <= 0.0 {
        return 0.0;
    }
    let unit = (1.0 + 20.0 * magnitude.log10() / -LOG_FLOOR).clamp(0.0, 1.2);
    unit.copysign(sample)
}

fn peak_db(column: &ScopeColumn, before: bool) -> f32 {
    let peak = if before {
        column.input_min.abs().max(column.input_max.abs())
    } else {
        column.output_min.abs().max(column.output_max.abs())
    };
    20.0 * peak.max(1.0e-6).log10()
}

fn level(db: f32) -> String {
    if db <= -90.0 {
        "−∞".to_owned()
    } else {
        format!("{db:+.1}")
    }
}

/// Filled band between two traces, one quad per pair of neighbouring points.
fn band(painter: &Painter, x: &[f32], upper: &[f32], lower: &[f32], color: Color32) {
    if x.len() < 2 {
        return;
    }
    let mut mesh = Mesh::default();
    for i in 0..x.len() {
        mesh.colored_vertex(pos2(x[i], upper[i]), color);
        mesh.colored_vertex(pos2(x[i], lower[i]), color);
    }
    for i in 0..x.len() as u32 - 1 {
        let a = 2 * i;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(Shape::mesh(mesh));
}

fn waveform(
    ui: &mut Ui,
    outer: Rect,
    id: Id,
    columns: &[ScopeColumn],
    span: usize,
    settings: Settings,
    s: f32,
) {
    let response = ui.interact(outer, id, Sense::hover());
    let painter = ui.painter().with_clip_rect(outer.intersect(ui.clip_rect()));
    let plot = screen(&painter, outer, s);
    let mid = plot.center().y;
    let half = plot.height() / 2.0;
    let y = |sample: f32| mid - amplitude_to_unit(sample, settings.log) * half;
    let ticks: &[(f32, &str)] = if settings.log {
        &[(1.0, "0"), (0.25, "-12"), (0.063, "-24"), (0.0158, "-36")]
    } else {
        &[(1.0, "0"), (0.5, "-6"), (0.25, "-12")]
    };
    for &(value, label) in ticks {
        for sign in [1.0, -1.0] {
            painter.hline(plot.x_range(), y(value * sign), graticule());
        }
        scale_label(
            &painter,
            pos2(plot.left() - 6.0 * s, y(value)),
            Align2::RIGHT_CENTER,
            label,
            s,
            BEFORE.gamma_multiply(0.7),
        );
    }
    painter.hline(
        plot.x_range(),
        mid,
        Stroke::new(1.0, AMBER.gamma_multiply(0.2)),
    );
    scale_label(
        &painter,
        pos2(plot.left() - 6.0 * s, plot.top() - 10.0 * s),
        Align2::RIGHT_CENTER,
        "dBFS",
        s,
        SILK_DIM,
    );
    let seconds = span as f32 * SCOPE_FRAMES_PER_COLUMN as f32 / sympoie::SAMPLE_RATE as f32;
    for step in 0..=4 {
        let fraction = step as f32 / 4.0;
        let x = plot.left() + fraction * plot.width();
        painter.vline(x, plot.y_range(), graticule());
        let ago = seconds * (1.0 - fraction);
        scale_label(
            &painter,
            pos2(x, plot.bottom() + 9.0 * s),
            Align2::CENTER_CENTER,
            &if step == 4 {
                "now".to_owned()
            } else {
                format!("−{ago:.2} s")
            },
            s,
            SILK_DIM,
        );
    }
    let gr_y = |db: f32| plot.top() + (db / GR_RANGE).clamp(0.0, 1.0) * plot.height();
    for db in [0.0, 6.0, 12.0, 18.0, 24.0] {
        scale_label(
            &painter,
            pos2(plot.right() + 6.0 * s, gr_y(db)),
            Align2::LEFT_CENTER,
            &format!("{db:.0}"),
            s,
            REDUCTION.gamma_multiply(0.8),
        );
    }
    scale_label(
        &painter,
        pos2(plot.right() + 6.0 * s, plot.top() - 10.0 * s),
        Align2::LEFT_CENTER,
        "GR dB",
        s,
        REDUCTION.gamma_multiply(0.8),
    );

    if columns.is_empty() {
        silk(
            &painter,
            plot.center(),
            Align2::CENTER_CENTER,
            "No signal yet — play something through the Vari-Mu",
            8.0 * s,
            SILK_DIM,
        );
    } else {
        // The newest column sits at the right edge; a short history fills from the right.
        let x_of = |index: usize| {
            let age = (columns.len() - 1 - index) as f32;
            plot.right() - age / (span.max(2) - 1) as f32 * plot.width()
        };
        let x: Vec<f32> = (0..columns.len()).map(x_of).collect();
        let pick = |f: fn(&ScopeColumn) -> f32| -> Vec<f32> {
            columns.iter().map(|column| y(f(column))).collect()
        };
        let (in_top, in_bottom) = (pick(|c| c.input_max), pick(|c| c.input_min));
        let (out_top, out_bottom) = (pick(|c| c.output_max), pick(|c| c.output_min));
        band(
            &painter,
            &x,
            &in_top,
            &in_bottom,
            BEFORE.gamma_multiply(0.22),
        );
        for trace in [&in_top, &in_bottom] {
            painter.add(Shape::line(
                x.iter()
                    .zip(trace.iter())
                    .map(|(x, y)| pos2(*x, *y))
                    .collect(),
                Stroke::new(1.0, BEFORE.gamma_multiply(0.55)),
            ));
        }
        band(
            &painter,
            &x,
            &out_top,
            &out_bottom,
            AFTER.gamma_multiply(0.42),
        );
        for trace in [&out_top, &out_bottom] {
            painter.add(Shape::line(
                x.iter()
                    .zip(trace.iter())
                    .map(|(x, y)| pos2(*x, *y))
                    .collect(),
                Stroke::new(1.2 * s.max(0.7), AFTER),
            ));
        }
        let reduction: Vec<f32> = columns.iter().map(|c| gr_y(c.reduction_db)).collect();
        band(
            &painter,
            &x,
            &vec![plot.top(); x.len()],
            &reduction,
            REDUCTION.gamma_multiply(0.12),
        );
        painter.add(Shape::line(
            x.iter()
                .zip(&reduction)
                .map(|(x, y)| pos2(*x, *y))
                .collect(),
            Stroke::new(1.6 * s.max(0.7), REDUCTION),
        ));
        if let Some(pointer) = response.hover_pos()
            && plot.x_range().contains(pointer.x)
        {
            let index = x
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1 - pointer.x).abs().total_cmp(&(b.1 - pointer.x).abs()))
                .map(|(index, _)| index)
                .unwrap_or(0);
            let column = columns[index];
            painter.vline(
                x[index],
                plot.y_range(),
                Stroke::new(1.0, SILK.gamma_multiply(0.5)),
            );
            let ago = (columns.len() - 1 - index) as f32 * SCOPE_FRAMES_PER_COLUMN as f32
                / sympoie::SAMPLE_RATE as f32;
            cursor_readout(
                &painter,
                plot.left_top() + vec2(6.0, 4.0) * s,
                &format!(
                    "−{ago:.2} s   in {} dBFS   out {} dBFS   GR {:.1} dB",
                    level(peak_db(&column, true)),
                    level(peak_db(&column, false)),
                    column.reduction_db
                ),
                s,
            );
        }
    }
    if settings.hold {
        hold_badge(&painter, plot.right_top() + vec2(-4.0, 4.0) * s, s);
    }
    screen_glass(&painter, outer, s);
}

fn cursor_readout(painter: &Painter, pos: Pos2, text: &str, s: f32) {
    let galley = painter.layout_no_wrap(text.to_owned(), theme::mono(8.0 * s), SILK);
    let rect = Rect::from_min_size(pos, galley.size()).expand2(vec2(6.0, 3.0) * s);
    painter.rect_filled(rect, 2.0 * s, Color32::from_black_alpha(190));
    painter.rect_stroke(
        rect,
        2.0 * s,
        Stroke::new(1.0, AMBER.gamma_multiply(0.35)),
        egui::StrokeKind::Inside,
    );
    painter.galley(pos, galley, SILK);
}

fn hold_badge(painter: &Painter, top_right: Pos2, s: f32) {
    let rect = Rect::from_min_size(top_right - vec2(40.0 * s, 0.0), vec2(40.0, 14.0) * s);
    painter.rect_filled(rect, 2.0 * s, AMBER.gamma_multiply(0.85));
    silk(
        painter,
        rect.center(),
        Align2::CENTER_CENTER,
        "Hold",
        6.5 * s,
        PRINT,
    );
}

fn spectrum(ui: &mut Ui, outer: Rect, id: Id, capture: &Capture, hold: bool, s: f32) {
    let response = ui.interact(outer, id, Sense::hover());
    let painter = ui.painter().with_clip_rect(outer.intersect(ui.clip_rect()));
    let plot = screen(&painter, outer, s);
    let x_of = |band: usize| plot.left() + band as f32 / 127.0 * plot.width();
    let y_of =
        |db: f32| plot.top() + (db.clamp(SPECTRUM_FLOOR, 0.0) / SPECTRUM_FLOOR) * plot.height();
    let hz_x = |hz: f32| plot.left() + (hz / 20.0).ln() / 1000.0_f32.ln() * plot.width();
    for hz in [
        20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
    ] {
        let x = hz_x(hz);
        painter.vline(x, plot.y_range(), graticule());
        scale_label(
            &painter,
            pos2(x, plot.bottom() + 9.0 * s),
            Align2::CENTER_CENTER,
            &if hz >= 1000.0 {
                format!("{}k", hz / 1000.0)
            } else {
                format!("{hz:.0}")
            },
            s,
            SILK_DIM,
        );
    }
    for db in [0.0, -20.0, -40.0, -60.0, -80.0] {
        painter.hline(plot.x_range(), y_of(db), graticule());
        scale_label(
            &painter,
            pos2(plot.left() - 6.0 * s, y_of(db)),
            Align2::RIGHT_CENTER,
            &format!("{db:.0}"),
            s,
            SILK_DIM,
        );
    }
    scale_label(
        &painter,
        pos2(plot.left() - 6.0 * s, plot.top() - 10.0 * s),
        Align2::RIGHT_CENTER,
        "dB",
        s,
        SILK_DIM,
    );
    let x: Vec<f32> = (0..128).map(x_of).collect();
    let before: Vec<f32> = capture.before.iter().map(|db| y_of(*db)).collect();
    let after: Vec<f32> = capture.after.iter().map(|db| y_of(*db)).collect();
    let floor = vec![plot.bottom(); 128];
    band(&painter, &x, &before, &floor, BEFORE.gamma_multiply(0.1));
    // Shade what the unit changed: red where it took energy away, green where it added
    // some (makeup gain and the harmonics of the colour stage).
    // In screen space a quieter band sits lower, so "removed" fills from the before trace
    // down to the after trace and "added" the other way round; each is empty elsewhere.
    let mut removed = Mesh::default();
    let mut added = Mesh::default();
    for i in 0..127 {
        for (mesh, color, top, bottom) in [
            (
                &mut removed,
                REDUCTION.gamma_multiply(0.28),
                &before,
                &after,
            ),
            (&mut added, ADDED.gamma_multiply(0.3), &after, &before),
        ] {
            let base = mesh.vertices.len() as u32;
            for band in [i, i + 1] {
                mesh.colored_vertex(pos2(x[band], top[band]), color);
                mesh.colored_vertex(pos2(x[band], bottom[band].max(top[band])), color);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 3, base + 2);
        }
    }
    painter.add(Shape::mesh(removed));
    painter.add(Shape::mesh(added));
    painter.add(Shape::line(
        x.iter().zip(&before).map(|(x, y)| pos2(*x, *y)).collect(),
        Stroke::new(1.1 * s.max(0.7), BEFORE.gamma_multiply(0.75)),
    ));
    painter.add(Shape::line(
        x.iter().zip(&after).map(|(x, y)| pos2(*x, *y)).collect(),
        Stroke::new(1.6 * s.max(0.7), AFTER),
    ));
    for (offset, color, label) in [(0.0, REDUCTION, "Reduced"), (64.0, ADDED, "Added")] {
        let swatch = Rect::from_min_size(
            plot.right_top() + vec2(-128.0 + offset, 6.0) * s,
            vec2(9.0, 9.0) * s,
        );
        painter.rect_filled(swatch, 1.0, color.gamma_multiply(0.6));
        silk(
            &painter,
            swatch.right_center() + vec2(4.0 * s, 0.0),
            Align2::LEFT_CENTER,
            label,
            6.0 * s,
            SILK_DIM,
        );
    }
    if let Some(pointer) = response.hover_pos()
        && plot.x_range().contains(pointer.x)
    {
        let band = (((pointer.x - plot.left()) / plot.width() * 127.0).round() as usize).min(127);
        let hz = 20.0 * 1000.0_f32.powf(band as f32 / 127.0);
        painter.vline(
            x[band],
            plot.y_range(),
            Stroke::new(1.0, SILK.gamma_multiply(0.5)),
        );
        let (b, a) = (capture.before[band], capture.after[band]);
        cursor_readout(
            &painter,
            plot.left_top() + vec2(6.0, 4.0) * s,
            &format!(
                "{}   before {b:.1} dB   after {a:.1} dB   Δ {:+.1} dB",
                if hz >= 1000.0 {
                    format!("{:.2} kHz", hz / 1000.0)
                } else {
                    format!("{hz:.0} Hz")
                },
                a - b
            ),
            s,
        );
    }
    if hold {
        hold_badge(&painter, plot.left_bottom() + vec2(44.0, -18.0) * s, s);
    }
    screen_glass(&painter, outer, s);
}

/// Level statistics of the visible window, before and after.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Dynamics {
    peak: [f32; 2],
    rms: [f32; 2],
    reduction_now: f32,
    reduction_max: f32,
    reduction_mean: f32,
}

fn dynamics(columns: &[ScopeColumn]) -> Option<Dynamics> {
    if columns.is_empty() {
        return None;
    }
    let db = |value: f32| 20.0 * value.max(1.0e-6).log10();
    let count = columns.len() as f32;
    let peak = |before: bool| {
        columns
            .iter()
            .map(|c| peak_db(c, before))
            .fold(-120.0, f32::max)
    };
    let rms = |power: fn(&ScopeColumn) -> f32| {
        db((columns.iter().map(power).sum::<f32>() / count).sqrt())
    };
    Some(Dynamics {
        peak: [peak(true), peak(false)],
        rms: [rms(|c| c.input_power), rms(|c| c.output_power)],
        reduction_now: columns[columns.len() - 1].reduction_db,
        reduction_max: columns.iter().map(|c| c.reduction_db).fold(0.0, f32::max),
        reduction_mean: columns.iter().map(|c| c.reduction_db).sum::<f32>() / count,
    })
}

fn readouts(painter: &Painter, rect: Rect, columns: &[ScopeColumn], s: f32) {
    recess(painter, rect, s);
    silk(
        painter,
        rect.left_top() + vec2(12.0, 10.0) * s,
        Align2::LEFT_TOP,
        "Dynamics · window",
        6.5 * s,
        BRASS,
    );
    let headers = rect.left_top() + vec2(0.0, 28.0) * s;
    for (x, label, color) in [(104.0, "Before", BEFORE), (164.0, "After", AFTER)] {
        silk(
            painter,
            headers + vec2(x, 0.0) * s,
            Align2::CENTER_TOP,
            label,
            6.0 * s,
            color,
        );
    }
    let stats = dynamics(columns);
    let pair = |f: fn(&Dynamics) -> [f32; 2]| stats.map(|d| f(&d));
    let rows: [(&str, Option<[f32; 2]>, &str); 3] = [
        ("Peak", pair(|d| d.peak), "dBFS"),
        ("RMS", pair(|d| d.rms), "dBFS"),
        (
            "Crest",
            pair(|d| [d.peak[0] - d.rms[0], d.peak[1] - d.rms[1]]),
            "dB",
        ),
    ];
    for (row, (label, values, unit)) in rows.into_iter().enumerate() {
        let y = 44.0 + row as f32 * 28.0;
        silk(
            painter,
            rect.left_top() + vec2(12.0, y + 9.0) * s,
            Align2::LEFT_CENTER,
            label,
            6.5 * s,
            SILK,
        );
        for (column, x) in [76.0_f32, 136.0].into_iter().enumerate() {
            let cell = Rect::from_min_size(rect.left_top() + vec2(x, y) * s, vec2(56.0, 18.0) * s);
            painter.add(analog::readout(cell, false));
            let text = values.map_or("—".to_owned(), |values| {
                if unit == "dB" {
                    format!("{:.1}", values[column])
                } else {
                    level(values[column])
                }
            });
            painter.text(
                cell.center(),
                Align2::CENTER_CENTER,
                text,
                theme::mono(8.5 * s),
                if column == 0 { BEFORE } else { AFTER },
            );
        }
    }
    let lines = stats.map_or_else(
        || {
            [
                "Level change —".to_owned(),
                "Dynamics tamed —".to_owned(),
                "GR —".to_owned(),
            ]
        },
        |d| {
            [
                format!("Level change {:+.1} dB", d.rms[1] - d.rms[0]),
                format!(
                    "Dynamics tamed {:.1} dB",
                    (d.peak[0] - d.rms[0]) - (d.peak[1] - d.rms[1])
                ),
                format!(
                    "GR now {:.1} · max {:.1} · avg {:.1}",
                    d.reduction_now, d.reduction_max, d.reduction_mean
                ),
            ]
        },
    );
    for (row, line) in lines.iter().enumerate() {
        let y = 136.0 + row as f32 * 26.0;
        let cell = Rect::from_min_size(
            rect.left_top() + vec2(12.0, y) * s,
            vec2(rect.width() / s - 24.0, 20.0) * s,
        );
        painter.add(analog::readout(cell, false));
        painter.text(
            cell.left_center() + vec2(8.0 * s, 0.0),
            Align2::LEFT_CENTER,
            line,
            theme::mono(8.0 * s),
            if row == 2 { REDUCTION } else { SILK },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(input: f32, output: f32, reduction: f32) -> ScopeColumn {
        ScopeColumn {
            input_min: -input,
            input_max: input,
            input_power: input * input / 2.0,
            output_min: -output,
            output_max: output,
            output_power: output * output / 2.0,
            reduction_db: reduction,
        }
    }

    #[test]
    fn dynamics_report_level_crest_and_gain_reduction_of_the_window() {
        assert!(dynamics(&[]).is_none());
        let columns = [column(1.0, 0.5, 6.0), column(0.5, 0.5, 0.0)];
        let d = dynamics(&columns).unwrap();
        assert!(d.peak[0].abs() < 1.0e-4 && (d.peak[1] + 6.0206).abs() < 1.0e-3);
        let rms_in = 20.0 * ((1.0 / 2.0 + 0.25 / 2.0) / 2.0_f32).sqrt().log10();
        assert!((d.rms[0] - rms_in).abs() < 1.0e-3);
        assert_eq!(
            (d.reduction_now, d.reduction_max, d.reduction_mean),
            (0.0, 6.0, 3.0)
        );
    }

    #[test]
    fn log_scale_maps_full_scale_and_floor_symmetrically() {
        assert_eq!(amplitude_to_unit(1.0, true), 1.0);
        assert_eq!(amplitude_to_unit(-1.0, true), -1.0);
        assert!((amplitude_to_unit(10.0_f32.powf(-24.0 / 20.0), true) - 0.5).abs() < 1.0e-5);
        assert_eq!(amplitude_to_unit(1.0e-4, true), 0.0);
        assert_eq!(amplitude_to_unit(0.0, true), 0.0);
        assert_eq!(amplitude_to_unit(0.3, false), 0.3);
    }

    #[test]
    fn scope_window_fits_its_minimum_size_and_hold_freezes_the_display() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::VariMu.instance();
        instance.id = 3;
        let telemetry = Telemetry::default();
        let mut actions = Vec::new();
        let size = vec2(680.0, 440.0);
        let frame = |events: Vec<egui::Event>, actions: &mut Vec<Action>| {
            let mut width = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), size)),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::super::super::super::scope(
                        ui,
                        &instance,
                        BuiltinKind::VariMu,
                        Some(&telemetry),
                        actions,
                    );
                    width = ui.min_rect().width();
                },
            );
            output.textures_delta.clear();
            width
        };
        for _ in 0..2 {
            assert!(frame(vec![], &mut actions) <= size.x + 1.0);
        }
        assert!(actions.is_empty());
        telemetry.scope.push(column(0.5, 0.25, 6.0));
        let key = Id::new(("varimu-scope", 3_u64));
        let settings = |ctx: &egui::Context| ctx.data_mut(|data| data.get_temp::<Settings>(key));
        assert!(!settings(&ctx).unwrap().hold);
        let lever = ctx
            .read_response(key.with("Display"))
            .unwrap()
            .rect
            .left_center()
            + vec2(9.0, 0.0);
        frame(vec![egui::Event::PointerMoved(lever)], &mut actions);
        for pressed in [true, false] {
            frame(
                vec![egui::Event::PointerButton {
                    pos: lever,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                &mut actions,
            );
        }
        frame(vec![], &mut actions);
        assert!(settings(&ctx).unwrap().hold);
        telemetry.scope.push(column(0.9, 0.3, 9.0));
        frame(vec![], &mut actions);
        let held = ctx
            .data_mut(|data| data.get_temp::<Capture>(key.with("held")))
            .unwrap();
        assert_eq!(held.columns.len(), 1, "hold must not pick up new audio");
        assert!(actions.is_empty());
    }
}
