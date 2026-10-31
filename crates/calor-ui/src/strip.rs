//! The band strip: the spectrum split into bands, each drawn as the shape its crossovers
//! give it, with the crossovers to drag, a band to pick, and each band's solo and mute.

use egui::{Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use sympoie_calor::{
    BANDS, CROSSOVER_BASE, CROSSOVER_DEFAULTS, CROSSOVER_SLOPE, CROSSOVERS, MAX_BANDS, MODELS,
    Slot, band_parameter, band_response,
};

use crate::Controls;
use crate::graphs::glass;
use crate::paint::{self, BAND_COLORS, FAINT, GOLD, LABEL, TEXT, fade, mono, sans};
use crate::widgets;
use crate::{format, widgets::set};

/// The muted red of a band's Mute.
const MUTE: Color32 = Color32::from_rgb(0xd8, 0x6a, 0x60);
/// How close a crossover may come to its neighbours, as a frequency ratio.
const GAP: f64 = 1.25;

/// The band whose controls the interface shows, from 0.
pub(crate) fn selected(ui: &Ui, salt: Id, count: usize) -> usize {
    ui.ctx()
        .data_mut(|data| data.get_temp::<usize>(salt.with("band")))
        .unwrap_or(0)
        .min(count.saturating_sub(1))
}

pub(crate) fn select(ui: &Ui, salt: Id, band: usize) {
    ui.ctx()
        .data_mut(|data| data.insert_temp(salt.with("band"), band));
}

/// Bands in use.
pub(crate) fn count(controls: &dyn Controls) -> usize {
    (controls.value(BANDS).round() as usize).clamp(1, MAX_BANDS)
}

/// The crossover frequencies, lowest first.
pub(crate) fn crossovers(controls: &dyn Controls) -> [f64; CROSSOVERS] {
    std::array::from_fn(|index| controls.value(CROSSOVER_BASE + index as u32))
}

fn x_of(plot: Rect, hz: f64) -> f32 {
    let position = ((hz / 20.0).max(1.0e-6).log10() / 3.0) as f32;
    plot.left() + position.clamp(0.0, 1.0) * plot.width()
}

fn hz_at(plot: Rect, x: f32) -> f64 {
    let position = ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
    20.0 * 1000.0_f64.powf(f64::from(position))
}

/// The frequency range band `band` of `count` covers, for a readout.
pub(crate) fn range(controls: &dyn Controls, band: usize, count: usize) -> String {
    let crossovers = crossovers(controls);
    let low = if band == 0 {
        "20 Hz".to_owned()
    } else {
        format::hertz(crossovers[band - 1])
    };
    let high = if band + 1 >= count {
        "20 kHz".to_owned()
    } else {
        format::hertz(crossovers[band])
    };
    format!("{low} – {high}")
}

/// Draws the strip in `rect` and handles picking a band, dragging the crossovers, and,
/// on the full panel, each band's solo and mute.
pub(crate) fn strip(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    salt: Id,
    rect: Rect,
    s: f32,
    compact: bool,
    live: bool,
) {
    let painter = ui.painter_at(rect.expand(2.0 * s));
    glass(&painter, rect, s);
    let count = count(controls);
    let selected = selected(ui, salt, count);
    let crossovers = crossovers(controls);
    let slope = controls.value(CROSSOVER_SLOPE).round() as usize;
    let plot = if compact {
        Rect::from_min_max(rect.min + vec2(6.0, 5.0) * s, rect.max - vec2(6.0, 3.0) * s)
    } else {
        Rect::from_min_max(
            rect.min + vec2(12.0, 30.0) * s,
            rect.max - vec2(12.0, 16.0) * s,
        )
    };
    let inner = painter.with_clip_rect(plot.expand(1.0));
    // Decades.
    for hz in [100.0, 1000.0, 10_000.0] {
        let x = x_of(plot, hz);
        inner.vline(
            x,
            plot.y_range(),
            Stroke::new(1.0, fade(Color32::WHITE, 0.035)),
        );
        if !compact {
            painter.text(
                pos2(x, plot.bottom() + 3.0 * s),
                Align2::CENTER_TOP,
                if hz >= 1000.0 {
                    format!("{}k", hz / 1000.0)
                } else {
                    format!("{hz:.0}")
                },
                mono(8.0 * s),
                FAINT,
            );
        }
    }
    // Each band's shape, the picked one brightest. Silence sits just below the plot, so
    // the tails tuck out of sight instead of running along its edge.
    const POINTS: usize = 180;
    let floor = plot.bottom() + 2.5 * s;
    for (band, &color) in BAND_COLORS.iter().enumerate().take(count) {
        let color = if live { color } else { FAINT };
        let picked = band == selected;
        let curve: Vec<Pos2> = (0..=POINTS)
            .map(|step| {
                let hz = 20.0 * 1000.0_f64.powf(step as f64 / POINTS as f64);
                let level = band_response(&crossovers, slope, count, band, hz) as f32;
                pos2(x_of(plot, hz), floor - level * plot.height() * 0.86)
            })
            .collect();
        let alpha = if picked { 0.30 } else { 0.12 };
        paint::fill_to(&inner, &curve, floor, |point| {
            let height = ((plot.bottom() - point.y) / plot.height()).clamp(0.0, 1.0);
            fade(color, alpha * (0.35 + 0.65 * height))
        });
        paint::line(
            &inner,
            curve,
            fade(color, if picked { 0.95 } else { 0.5 }),
            if picked { 1.5 } else { 1.0 } * s,
        );
    }
    // Picking a band: a click anywhere over it.
    for band in 0..count {
        let left = if band == 0 {
            plot.left()
        } else {
            x_of(plot, crossovers[band - 1])
        };
        let right = if band + 1 >= count {
            plot.right()
        } else {
            x_of(plot, crossovers[band])
        };
        let region = Rect::from_x_y_ranges(left..=right, plot.y_range());
        let response = ui.interact(region, salt.with(("band", band)), Sense::click());
        if response.clicked() {
            select(ui, salt, band);
        }
        let label_at = pos2(left + 6.0 * s, plot.top() + 9.0 * s);
        let picked = band == selected;
        let color = if live { BAND_COLORS[band] } else { FAINT };
        let chip = Rect::from_center_size(
            label_at + vec2(5.0 * s, 0.0),
            vec2(14.0, 14.0) * if compact { 0.85 * s } else { s },
        );
        if region.width() > 22.0 * s {
            painter.circle_filled(
                chip.center(),
                chip.width() / 2.0,
                fade(color, if picked { 0.95 } else { 0.35 }),
            );
            painter.text(
                chip.center(),
                Align2::CENTER_CENTER,
                format!("{}", band + 1),
                sans(chip.height() * 0.62),
                if picked {
                    Color32::from_rgb(0x1c, 0x1b, 0x1e)
                } else {
                    TEXT
                },
            );
        }
        if !compact && region.width() > 90.0 * s {
            let model = controls.value(band_parameter(band, Slot::Model)).round() as usize;
            painter.text(
                chip.right_center() + vec2(6.0 * s, 0.0),
                Align2::LEFT_CENTER,
                MODELS[model.min(MODELS.len() - 1)],
                sans(9.5 * s),
                if picked { TEXT } else { LABEL },
            );
        }
        if !compact && region.width() > 48.0 * s {
            let size = vec2(16.0, 14.0) * s;
            let solo =
                Rect::from_min_size(pos2(left + 6.0 * s, plot.bottom() - size.y - 4.0 * s), size);
            let mute = solo.translate(vec2(size.x + 3.0 * s, 0.0));
            widgets::toggle(
                ui,
                controls,
                salt.with(("solo", band)),
                solo,
                s,
                band_parameter(band, Slot::Solo),
                "S",
                GOLD,
            )
            .on_hover_text("Solo: hear this band alone");
            widgets::toggle(
                ui,
                controls,
                salt.with(("mute", band)),
                mute,
                s,
                band_parameter(band, Slot::Mute),
                "M",
                MUTE,
            )
            .on_hover_text("Mute: silence this band");
        }
    }
    // The crossovers, to drag.
    for index in 0..count.saturating_sub(1) {
        let id = CROSSOVER_BASE + index as u32;
        let x = x_of(plot, crossovers[index]);
        let hit = Rect::from_x_y_ranges((x - 5.0 * s)..=(x + 5.0 * s), plot.y_range());
        let response = ui.interact(
            hit,
            salt.with(("crossover", index)),
            Sense::click_and_drag(),
        );
        let hot = response.hovered() || response.dragged();
        painter.vline(
            x,
            plot.y_range(),
            Stroke::new(
                if hot { 1.5 } else { 1.0 },
                fade(TEXT, if hot { 0.8 } else { 0.35 }),
            ),
        );
        let grip = Rect::from_center_size(pos2(x, plot.top() + 2.0 * s), vec2(8.0, 14.0) * s);
        painter.rect_filled(grip, 3.0 * s, if hot { TEXT } else { fade(TEXT, 0.6) });
        if response.drag_started() {
            controls.begin(id);
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let below = if index == 0 {
                20.0
            } else {
                crossovers[index - 1] * GAP
            };
            let above = if index + 2 >= count {
                20_000.0
            } else {
                crossovers[index + 1] / GAP
            };
            controls.set(id, hz_at(plot, pointer.x).clamp(below, above.max(below)));
        }
        if response.drag_stopped() {
            controls.end(id);
        }
        if response.double_clicked() {
            set(controls, id, CROSSOVER_DEFAULTS[index]);
        }
        if hot {
            painter.text(
                pos2(x, plot.bottom() - 4.0 * s),
                Align2::CENTER_BOTTOM,
                format::hertz(crossovers[index]),
                mono(8.6 * s),
                TEXT,
            );
        }
        response
            .on_hover_cursor(CursorIcon::ResizeHorizontal)
            .on_hover_text(format!(
                "Crossover {}: {}. Drag to move it, double-click to reset.",
                index + 1,
                format::hertz(crossovers[index])
            ));
    }
    if !compact {
        // The strip's own row: how many bands, and how steep their edges.
        let row = rect.top() + 15.0 * s;
        widgets::heading(&painter, pos2(rect.left() + 12.0 * s, row), "Bands", s);
        widgets::stepper(
            ui,
            controls,
            salt,
            Rect::from_min_size(
                pos2(rect.left() + 62.0 * s, row - 9.0 * s),
                vec2(96.0, 18.0) * s,
            ),
            s,
            BANDS,
            if count == 1 { "band" } else { "bands" },
        );
        widgets::heading(&painter, pos2(rect.right() - 236.0 * s, row), "Slope", s);
        widgets::choice_segments(
            ui,
            controls,
            salt,
            Rect::from_min_size(
                pos2(rect.right() - 188.0 * s, row - 9.0 * s),
                vec2(176.0, 18.0) * s,
            ),
            s,
            CROSSOVER_SLOPE,
        );
    }
}
