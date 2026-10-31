//! The modulation page: the LFO and the envelope follower, and the four slots that route
//! them to the controls they move.

use egui::{Align2, Color32, Id, Pos2, Rect, Stroke, Ui, pos2};
use sympoie_calor::{
    ENVELOPE_ATTACK, ENVELOPE_RELEASE, LFO_RATE, LFO_SHAPE, MOD_AMOUNT, MOD_BAND, MOD_SLOTS,
    MOD_SOURCE, MOD_TARGET, Telemetry, mod_parameter, telemetry,
};

use crate::graphs::{Levels, glass};
use crate::paint::{self, EMBER, FAINT, TEAL, TEXT, fade, sans};
use crate::{Controls, Place, knob, widgets};

/// The envelope's scope: how many readings it shows, and how often it takes one.
const HISTORY: usize = 150;
const HISTORY_STEP: f64 = 1.0 / 30.0;

/// The colours of the two sources, wherever they are drawn.
const LFO_COLOR: Color32 = EMBER;
const ENVELOPE_COLOR: Color32 = TEAL;

/// The LFO's value at `phase` (0 to 1) for shape `shape`, as the processor reads it.
fn lfo(shape: usize, phase: f64) -> f64 {
    match shape {
        0 => (std::f64::consts::TAU * phase).sin(),
        1 => 1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs(),
        2 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => 2.0 * (phase + 0.5).fract() - 1.0,
    }
}

/// Where the sources stand: the LFO's phase and value, and the envelope, from the
/// telemetry while the processor runs. The envelope falls with the meters once the
/// processor stops refreshing them.
fn sources(telemetry: Option<&Telemetry>, levels: &Levels) -> (f32, f32, f32) {
    telemetry.map_or((0.0, 0.0, 0.0), |telemetry| {
        (
            telemetry::get(&telemetry.lfo_phase),
            telemetry::get(&telemetry.lfo),
            telemetry::get(&telemetry.envelope) * (1.0 - levels.stale),
        )
    })
}

/// The envelope's last few seconds, oldest first, with `now` taken in as time passes.
fn history(ui: &Ui, salt: Id, now: f32) -> Vec<f32> {
    let key = salt.with("envelope history");
    let time = ui.input(|input| input.time);
    let (mut readings, mut taken) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(Vec<f32>, f64)>(key))
        .unwrap_or_else(|| (vec![0.0; HISTORY], time));
    let due = ((time - taken) / HISTORY_STEP).floor().max(0.0);
    if due >= 1.0 {
        let fresh = (due as usize).min(HISTORY);
        readings.drain(..fresh);
        readings.extend(std::iter::repeat_n(now, fresh));
        taken = if fresh == HISTORY {
            time
        } else {
            taken + due * HISTORY_STEP
        };
    }
    ui.ctx()
        .data_mut(|data| data.insert_temp(key, (readings.clone(), taken)));
    readings
}

/// Draws the page into the panel's space below its header.
pub(crate) fn page(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    telemetry: Option<&Telemetry>,
    levels: &Levels,
    salt: Id,
    place: Place,
) {
    let s = place.s;
    let painter = ui.painter().clone();
    let (phase, lfo_now, envelope_now) = sources(telemetry, levels);
    let look = knob::Look {
        radius: 20.0 * s,
        accent: LFO_COLOR,
        hero: false,
        text: s,
    };

    // The LFO: its rate, its shape, and one cycle of it with where it stands now.
    let lfo_box = place.rect(20.0, 56.0, 425.0, 150.0);
    glass(&painter, lfo_box, s);
    widgets::heading(&painter, place.at(32.0, 72.0), "LFO", s);
    knob::knob(
        ui,
        controls,
        salt,
        LFO_RATE,
        place.at(76.0, 146.0),
        look,
        "Rate",
        "How fast the LFO swings.",
    );
    widgets::choice_segments(
        ui,
        controls,
        salt,
        place.rect(140.0, 62.0, 290.0, 22.0),
        s,
        LFO_SHAPE,
    );
    let scope = place.rect(140.0, 96.0, 290.0, 96.0);
    painter.hline(
        scope.x_range(),
        scope.center().y,
        Stroke::new(1.0, fade(Color32::WHITE, 0.06)),
    );
    let shape = controls.value(LFO_SHAPE).round() as usize;
    let at = |phase: f64| {
        pos2(
            scope.left() + phase as f32 * scope.width(),
            scope.center().y - lfo(shape, phase) as f32 * scope.height() * 0.42,
        )
    };
    let points: Vec<Pos2> = (0..=160).map(|step| at(f64::from(step) / 160.0)).collect();
    paint::soft_line(&painter, points, LFO_COLOR, 1.6 * s);
    if telemetry.is_some() {
        let now = at(f64::from(phase));
        painter.vline(
            now.x,
            scope.y_range(),
            Stroke::new(1.0, fade(LFO_COLOR, 0.25)),
        );
        painter.circle_filled(now, 5.5 * s, fade(LFO_COLOR, 0.25));
        painter.circle_filled(now, 3.0 * s, LFO_COLOR);
    }

    // The envelope: its times, and how loud the input is to it.
    let envelope_box = place.rect(455.0, 56.0, 425.0, 150.0);
    glass(&painter, envelope_box, s);
    widgets::heading(&painter, place.at(467.0, 72.0), "Envelope", s);
    let look_envelope = knob::Look {
        accent: ENVELOPE_COLOR,
        ..look
    };
    knob::knob(
        ui,
        controls,
        salt,
        ENVELOPE_ATTACK,
        place.at(512.0, 146.0),
        look_envelope,
        "Attack",
        "How fast the envelope rises with the input.",
    );
    knob::knob(
        ui,
        controls,
        salt,
        ENVELOPE_RELEASE,
        place.at(588.0, 146.0),
        look_envelope,
        "Release",
        "How fast the envelope falls back.",
    );
    // The input's level as the envelope follows it, over the last few seconds.
    painter.text(
        place.at(866.0, 72.0),
        Align2::RIGHT_CENTER,
        "Follows the input's level",
        sans(9.0 * s),
        FAINT,
    );
    let scope = place.rect(640.0, 96.0, 222.0, 96.0);
    for share in [0.0, 0.5, 1.0] {
        painter.hline(
            scope.x_range(),
            scope.bottom() - share * scope.height(),
            Stroke::new(
                1.0,
                fade(Color32::WHITE, if share == 0.0 { 0.06 } else { 0.03 }),
            ),
        );
    }
    let readings = history(ui, salt, envelope_now);
    let last = readings.len().saturating_sub(1).max(1) as f32;
    let trace: Vec<Pos2> = readings
        .iter()
        .enumerate()
        .map(|(index, &reading)| {
            pos2(
                scope.left() + index as f32 / last * scope.width(),
                scope.bottom() - reading.clamp(0.0, 1.0) * scope.height(),
            )
        })
        .collect();
    paint::fill_to(&painter, &trace, scope.bottom(), |point| {
        let height = ((scope.bottom() - point.y) / scope.height()).clamp(0.0, 1.0);
        fade(ENVELOPE_COLOR, 0.08 + 0.22 * height)
    });
    let now = trace.last().copied().unwrap_or(scope.left_bottom());
    paint::soft_line(&painter, trace, ENVELOPE_COLOR, 1.4 * s);
    if telemetry.is_some() {
        painter.circle_filled(now, 3.0 * s, ENVELOPE_COLOR);
    }
    if telemetry.is_some() && levels.stale < 1.0 {
        ui.ctx().request_repaint();
    }

    // The slots.
    let routing = place.rect(20.0, 216.0, 860.0, 274.0);
    glass(&painter, routing, s);
    widgets::heading(&painter, place.at(32.0, 232.0), "Routing", s);
    for (label, x) in [
        ("Source", 56.0),
        ("Target", 286.0),
        ("Band", 470.0),
        ("Amount", 632.0),
        ("Now", 704.0),
    ] {
        painter.text(
            place.at(x, 252.0),
            Align2::LEFT_CENTER,
            label,
            sans(9.0 * s),
            FAINT,
        );
    }
    for slot in 0..MOD_SLOTS {
        let y = 286.0 + 54.0 * slot as f32;
        let source = controls.value(mod_parameter(slot, MOD_SOURCE)).round() as usize;
        let active = source != 0;
        let color = match source {
            1 => LFO_COLOR,
            2 => ENVELOPE_COLOR,
            _ => FAINT,
        };
        painter.circle_filled(
            place.at(40.0, y),
            9.0 * s,
            fade(color, if active { 0.9 } else { 0.3 }),
        );
        painter.text(
            place.at(40.0, y),
            Align2::CENTER_CENTER,
            format!("{}", slot + 1),
            sans(10.0 * s),
            if active {
                Color32::from_rgb(0x1c, 0x1b, 0x1e)
            } else {
                TEXT
            },
        );
        widgets::choice_segments(
            ui,
            controls,
            salt,
            place.rect(56.0, y - 11.0, 214.0, 22.0),
            s,
            mod_parameter(slot, MOD_SOURCE),
        );
        widgets::choice_menu(
            ui,
            controls,
            salt,
            place.rect(286.0, y - 11.0, 170.0, 22.0),
            s,
            mod_parameter(slot, MOD_TARGET),
            "",
        );
        widgets::choice_menu(
            ui,
            controls,
            salt,
            place.rect(470.0, y - 11.0, 140.0, 22.0),
            s,
            mod_parameter(slot, MOD_BAND),
            "",
        );
        knob::knob(
            ui,
            controls,
            salt,
            mod_parameter(slot, MOD_AMOUNT),
            place.at(650.0, y + 2.0),
            knob::Look {
                radius: 13.0 * s,
                accent: color,
                hero: false,
                text: 0.9 * s,
            },
            "",
            "How far the source moves the target: a share of its whole range.",
        );
        // What the slot is doing now, from the middle of the bar.
        let amount = (controls.value(mod_parameter(slot, MOD_AMOUNT)) / 100.0) as f32;
        let push = match source {
            1 => 0.5 * lfo_now * amount,
            2 => envelope_now * amount,
            _ => 0.0,
        };
        let bar = place.rect(704.0, y - 3.0, 160.0, 6.0);
        painter.rect_filled(bar, 2.0 * s, crate::paint::WELL);
        let middle = bar.center().x;
        let end = middle + push.clamp(-0.5, 0.5) * bar.width();
        if active && (end - middle).abs() > 0.5 {
            painter.rect_filled(
                Rect::from_x_y_ranges(middle.min(end)..=middle.max(end), bar.y_range()),
                2.0 * s,
                color,
            );
        }
        painter.vline(
            middle,
            (bar.top() - 3.0 * s)..=(bar.bottom() + 3.0 * s),
            Stroke::new(1.0, FAINT),
        );
        if slot + 1 < MOD_SLOTS {
            painter.hline(
                place.at(32.0, 0.0).x..=place.at(868.0, 0.0).x,
                place.at(0.0, y + 27.0).y,
                Stroke::new(1.0, fade(Color32::WHITE, 0.04)),
            );
        }
    }
}
