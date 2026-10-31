//! The two glasses (the curve with the input's reach marked on it, and the harmonics a
//! sine at the input's level comes out with) and the level meters.

use std::sync::atomic::Ordering;

use egui::{
    Align2, Color32, CornerRadius, Id, Painter, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2,
    vec2,
};
use sympoie_calor::{HARMONICS, MAX_BANDS, REFERENCE_LEVEL, Telemetry, Transfer, telemetry};

use crate::format;
use crate::paint::{
    self, EMBER, FAINT, GLASS_BOTTOM, GLASS_TOP, HAIRLINE, LABEL, TEAL, TEXT, WELL, fade, mono,
    sans, spaced,
};

/// Grid lines on a glass: the axes, and the finer divisions.
const AXIS: f32 = 0.06;
const GRID: f32 = 0.03;

/// How long the meters hold once the processor stops refreshing them, and how long they
/// then take to fall, in seconds.
const STALE_HOLD: f64 = 0.4;
const STALE_FADE: f64 = 0.6;
/// Harmonic levels the bars span, in dB below the fundamental.
const BAR_RANGE: f64 = 80.0;
/// Below this input level, in dBFS, the harmonics are shown at the reference level.
const QUIET_DB: f32 = -60.0;
/// The meters' span, in dBFS.
const METER_FLOOR: f32 = -60.0;
const METER_CEILING: f32 = 6.0;

/// The meters' readings, in dBFS.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Levels {
    pub(crate) input_db: f32,
    pub(crate) output_db: f32,
    /// Each band's output.
    pub(crate) bands: [f32; MAX_BANDS],
    /// How far the readings have fallen since the processor stopped refreshing them, from
    /// 0 (fresh) to 1 (gone).
    pub(crate) stale: f32,
}

/// What the meters read, let fall to silence once the processor stops refreshing them (as
/// it does when playback stops).
pub(crate) fn levels(ui: &Ui, salt: Id, telemetry: Option<&Telemetry>) -> Levels {
    let Some(telemetry) = telemetry else {
        return Levels {
            input_db: -120.0,
            output_db: -120.0,
            bands: [-120.0; MAX_BANDS],
            stale: 1.0,
        };
    };
    let now = ui.input(|input| input.time);
    let updates = telemetry.updates.load(Ordering::Relaxed);
    let key = salt.with("age");
    let (seen, since) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(u32, f64)>(key))
        .unwrap_or((updates, f64::NEG_INFINITY));
    let since = if updates == seen { since } else { now };
    ui.ctx()
        .data_mut(|data| data.insert_temp(key, (updates, since)));
    let stale = (((now - since) - STALE_HOLD) / STALE_FADE).clamp(0.0, 1.0) as f32;
    let fall = |db: f32| db + (-120.0 - db) * stale;
    Levels {
        input_db: fall(telemetry::get(&telemetry.input_db)),
        output_db: fall(telemetry::get(&telemetry.output_db)),
        bands: std::array::from_fn(|band| fall(telemetry::get(&telemetry.band_db[band]))),
        stale,
    }
}

/// The level, as a sine's peak, the harmonics are shown at: the input's peak while it
/// plays, eased so the bars do not jitter, and the reference level in silence.
pub(crate) fn display_level(ui: &Ui, salt: Id, levels: &Levels) -> f64 {
    let reference = 20.0 * REFERENCE_LEVEL.log10() as f32;
    let target = if levels.input_db > QUIET_DB {
        levels.input_db.min(METER_CEILING)
    } else {
        reference
    };
    let key = salt.with("level");
    let shown = ui
        .ctx()
        .data_mut(|data| data.get_temp::<f32>(key))
        .unwrap_or(target);
    let next = shown + (target - shown) * 0.2;
    ui.ctx().data_mut(|data| data.insert_temp(key, next));
    if (next - target).abs() > 0.05 {
        ui.ctx().request_repaint();
    }
    10.0_f64.powf(f64::from(next) / 20.0)
}

pub(crate) fn glass(painter: &Painter, rect: Rect, s: f32) {
    paint::gradient_rect(painter, rect, 8.0 * s, GLASS_TOP, GLASS_BOTTOM);
    painter.rect_stroke(
        rect,
        8.0 * s,
        Stroke::new(1.0, HAIRLINE),
        StrokeKind::Inside,
    );
    painter.hline(
        (rect.left() + 10.0 * s)..=(rect.right() - 10.0 * s),
        rect.top() + 1.0,
        Stroke::new(1.0, fade(Color32::WHITE, 0.05)),
    );
}

/// The transfer glass: the curve over the full-scale input, scaled to fill the glass, with
/// the clean line dashed beside it. `reach` is the input's peak: the stretch of the curve
/// it plays over is shaded and where its peaks land is marked.
pub(crate) fn transfer(
    painter: &Painter,
    rect: Rect,
    transfer: &Transfer,
    reach: Option<f64>,
    live: bool,
    s: f32,
    compact: bool,
) {
    glass(painter, rect, s);
    let plot = rect.shrink(if compact { 7.0 } else { 18.0 } * s);
    let clip = painter.with_clip_rect(plot.expand(2.0 * s));
    const POINTS: usize = 161;
    let samples: Vec<(f64, f64)> = (0..POINTS)
        .map(|index| {
            let x = -1.0 + 2.0 * index as f64 / (POINTS - 1) as f64;
            (x, transfer.apply(x))
        })
        .collect();
    let top = samples
        .iter()
        .fold(1.0e-6_f64, |top, &(_, y)| top.max(y.abs()))
        * 1.1;
    let at = |x: f64, y: f64| {
        pos2(
            plot.center().x + x as f32 * plot.width() / 2.0,
            plot.center().y - (y / top) as f32 * plot.height() / 2.0,
        )
    };
    for fraction in [0.25, 0.75] {
        let grid = Stroke::new(1.0, fade(Color32::WHITE, GRID));
        clip.vline(plot.left() + fraction * plot.width(), plot.y_range(), grid);
        clip.hline(plot.x_range(), plot.top() + fraction * plot.height(), grid);
    }
    let axis = Stroke::new(1.0, fade(Color32::WHITE, AXIS));
    clip.vline(plot.center().x, plot.y_range(), axis);
    clip.hline(plot.x_range(), plot.center().y, axis);
    let reach = reach.map(|reach| reach.min(1.0));
    if let Some(reach) = reach {
        let band = Rect::from_x_y_ranges(at(-reach, 0.0).x..=at(reach, 0.0).x, plot.y_range());
        clip.rect_filled(band, 0.0, fade(EMBER, 0.05));
    }
    clip.extend(Shape::dashed_line(
        &[at(-1.0, -1.0), at(1.0, 1.0)],
        Stroke::new(1.0, FAINT),
        3.0 * s,
        3.0 * s,
    ));
    paint::soft_line(
        &clip,
        samples.iter().map(|&(x, y)| at(x, y)).collect(),
        if live { EMBER } else { FAINT },
        if compact { 1.3 } else { 1.8 } * s,
    );
    if let Some(reach) = reach.filter(|_| live) {
        for x in [-reach, reach] {
            let point = at(x, transfer.apply(x));
            clip.circle_filled(point, 2.8 * s, TEXT);
            clip.circle_stroke(point, 2.8 * s, Stroke::new(1.0, fade(Color32::BLACK, 0.5)));
        }
    }
    if !compact {
        spaced(
            painter,
            rect.left_top() + vec2(10.0, 8.0) * s,
            Align2::LEFT_TOP,
            "TRANSFER",
            sans(7.6 * s),
            LABEL,
            1.2 * s,
        );
        spaced(
            painter,
            rect.right_bottom() - vec2(9.0, 6.0) * s,
            Align2::RIGHT_BOTTOM,
            "IN",
            sans(7.0 * s),
            FAINT,
            1.2 * s,
        );
    }
}

/// The odd and the even harmonics' combined power, relative to the fundamental's.
pub(crate) fn sums(levels: &[f64; HARMONICS]) -> (f64, f64) {
    let fundamental = levels[0].max(1.0e-12);
    let (mut odd, mut even) = (0.0, 0.0);
    for (index, &level) in levels.iter().enumerate().skip(1) {
        let power = (level / fundamental).powi(2);
        if index % 2 == 0 {
            odd += power;
        } else {
            even += power;
        }
    }
    (odd, even)
}

/// A combined power ratio in dB, or a dash for none at all.
pub(crate) fn power_db(power: f64) -> String {
    if power < 1.0e-12 {
        "—".to_owned()
    } else {
        format!("{} dB", format::decibels(10.0 * power.log10(), 1))
    }
}

/// Labelled readouts in a row from `x`, centred on `y`.
pub(crate) fn readouts(
    painter: &Painter,
    mut x: f32,
    y: f32,
    items: &[(&str, String, Color32)],
    s: f32,
) {
    for (label, value, color) in items {
        let label = spaced(
            painter,
            pos2(x, y),
            Align2::LEFT_CENTER,
            label,
            sans(7.4 * s),
            LABEL,
            1.2 * s,
        );
        let value = painter.text(
            pos2(label.right() + 5.0 * s, y),
            Align2::LEFT_CENTER,
            value,
            mono(9.0 * s),
            *color,
        );
        x = value.right() + 14.0 * s;
    }
}

/// The harmonics glass: one bar per harmonic in dB below the fundamental, the fundamental
/// light, the odd harmonics ember and the even ones teal. Returns the harmonics' amplitudes
/// for the readouts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn harmonics(
    ui: &mut Ui,
    painter: &Painter,
    rect: Rect,
    transfer: &Transfer,
    amplitude: f64,
    live: bool,
    salt: Id,
    s: f32,
    compact: bool,
) -> [f64; HARMONICS] {
    glass(painter, rect, s);
    let levels = transfer.harmonics(amplitude);
    let fundamental = levels[0].max(1.0e-12);
    let relative = levels.map(|level| 20.0 * (level / fundamental).max(1.0e-12).log10());
    let count = if compact { 12 } else { HARMONICS };
    let plot = if compact {
        Rect::from_min_max(rect.min + vec2(8.0, 8.0) * s, rect.max - vec2(8.0, 6.0) * s)
    } else {
        Rect::from_min_max(
            rect.min + vec2(44.0, 42.0) * s,
            rect.max - vec2(18.0, 26.0) * s,
        )
    };
    let y_of = |db: f64| plot.top() + ((-db / BAR_RANGE).clamp(0.0, 1.0) as f32) * plot.height();
    for db in [0.0, -20.0, -40.0, -60.0, -80.0] {
        let y = y_of(db);
        let color = fade(Color32::WHITE, if db == 0.0 { AXIS } else { GRID });
        painter.hline(plot.x_range(), y, Stroke::new(1.0, color));
        if !compact {
            painter.text(
                pos2(plot.left() - 8.0 * s, y),
                Align2::RIGHT_CENTER,
                format::decibels(db, 0),
                mono(8.0 * s),
                FAINT,
            );
        }
    }
    let slot = plot.width() / count as f32;
    let width = slot * if compact { 0.5 } else { 0.56 };
    let corner = (2.0 * s).round().clamp(0.0, 255.0) as u8;
    for (index, &level) in relative.iter().enumerate().take(count) {
        let harmonic = index + 1;
        let color = if !live {
            FAINT
        } else if harmonic == 1 {
            TEXT
        } else if harmonic % 2 == 1 {
            EMBER
        } else {
            TEAL
        };
        let x = plot.left() + slot * (index as f32 + 0.5);
        if level > -BAR_RANGE {
            let top = y_of(level);
            let bar = Rect::from_min_max(
                pos2(x - width / 2.0, top),
                pos2(x + width / 2.0, plot.bottom()),
            );
            painter.rect_filled(
                bar,
                CornerRadius {
                    nw: corner,
                    ne: corner,
                    sw: 0,
                    se: 0,
                },
                fade(color, 0.88),
            );
            painter.hline(
                (bar.left() + 1.0)..=(bar.right() - 1.0),
                top + 0.5,
                Stroke::new(1.0, paint::mix(color, Color32::WHITE, 0.3)),
            );
        }
        if !compact {
            painter.text(
                pos2(x, plot.bottom() + 5.0 * s),
                Align2::CENTER_TOP,
                harmonic.to_string(),
                mono(8.0 * s),
                if harmonic == 1 { LABEL } else { FAINT },
            );
        }
    }
    let response = ui.interact(plot, salt.with("bars"), Sense::hover());
    if let Some(pointer) = response.hover_pos() {
        let index = (((pointer.x - plot.left()) / slot).max(0.0) as usize).min(count - 1);
        let harmonic = index + 1;
        let kind = match harmonic {
            1 => "the fundamental",
            harmonic if harmonic % 2 == 1 => "odd",
            _ => "even",
        };
        response.on_hover_text(format!(
            "Harmonic {harmonic}, {kind}: {} dB",
            format::decibels(relative[index], 1)
        ));
    }
    if !compact {
        spaced(
            painter,
            rect.left_top() + vec2(12.0, 8.0) * s,
            Align2::LEFT_TOP,
            "HARMONICS",
            sans(7.6 * s),
            LABEL,
            1.2 * s,
        );
        painter.text(
            rect.right_top() + vec2(-14.0, 8.0) * s,
            Align2::RIGHT_TOP,
            format!(
                "SINE AT {} dBFS",
                format::decibels(20.0 * amplitude.log10(), 1)
            ),
            mono(8.0 * s),
            FAINT,
        );
        let (odd, even) = sums(&levels);
        readouts(
            painter,
            rect.left() + 12.0 * s,
            rect.top() + 28.0 * s,
            &[
                ("THD", format!("{:.2}%", (odd + even).sqrt() * 100.0), TEXT),
                ("ODD", power_db(odd), if live { EMBER } else { FAINT }),
                ("EVEN", power_db(even), if live { TEAL } else { FAINT }),
            ],
            s,
        );
    }
    levels
}

/// A horizontal peak meter from −60 to +6 dBFS, copper past 0 dBFS, labelled on the left and
/// read out on the right.
pub(crate) fn meter(painter: &Painter, rect: Rect, db: f32, label: &str, s: f32) {
    spaced(
        painter,
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        sans(7.0 * s),
        LABEL,
        1.2 * s,
    );
    let track = Rect::from_min_max(
        pos2(rect.left() + 28.0 * s, rect.center().y - 2.0 * s),
        pos2(rect.right() - 42.0 * s, rect.center().y + 2.0 * s),
    );
    painter.rect_filled(track.expand(1.0), 3.0 * s, WELL);
    let x_of = |db: f32| {
        track.left()
            + ((db - METER_FLOOR) / (METER_CEILING - METER_FLOOR)).clamp(0.0, 1.0) * track.width()
    };
    let (level, zero) = (x_of(db), x_of(0.0));
    if level > track.left() {
        let clean = Rect::from_min_max(track.min, pos2(level.min(zero), track.max.y));
        painter.rect_filled(clean, 2.0 * s, paint::mix(LABEL, TEXT, 0.4));
        if level > zero {
            let over = Rect::from_min_max(pos2(zero, track.min.y), pos2(level, track.max.y));
            painter.rect_filled(over, 2.0 * s, EMBER);
        }
    }
    painter.vline(
        zero,
        (track.top() - 2.0 * s)..=(track.bottom() + 2.0 * s),
        Stroke::new(1.0, FAINT),
    );
    painter.text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        if db > -100.0 {
            format::decibels(f64::from(db), 1)
        } else {
            "−∞".to_owned()
        },
        mono(8.4 * s),
        if db > 0.0 { EMBER } else { TEXT },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sums_split_the_harmonics_by_parity() {
        let mut levels = [0.0; HARMONICS];
        levels[0] = 1.0;
        levels[1] = 0.1;
        levels[2] = 0.01;
        let (odd, even) = sums(&levels);
        assert!((odd - 1.0e-4).abs() < 1.0e-12);
        assert!((even - 1.0e-2).abs() < 1.0e-12);
        assert_eq!(power_db(even), "−20.0 dB");
        assert_eq!(power_db(0.0), "—");
    }
}
