//! Alternating bar bands behind timelines, so neighbouring bars read dark, bright,
//! dark, bright… in the arrangement and the piano roll alike.

use std::ops::RangeInclusive;

use eframe::egui::{Painter, Rect};

use crate::ui::theme;

/// Bands narrower than this merge in pairs, so a zoomed-out view alternates every
/// 2, 4, 8… bars instead of shimmering with one-pixel stripes.
const MIN_BAND_PIXELS: f64 = 12.0;

/// How many bars each band spans when a bar is `bar_pixels` wide.
fn bars_per_band(bar_pixels: f64) -> u64 {
    let mut bars = 1_u64;
    while (bars as f64) * bar_pixels < MIN_BAND_PIXELS && bars < 1 << 20 {
        bars *= 2;
    }
    bars
}

/// Brightens every other band of bars inside `rect`, counting bars from tick 0.
///
/// `x` maps a tick to a screen position; `ticks` is the visible span. The first
/// band stays dark, so bar 1 is dark, bar 2 bright and so on.
pub fn paint_bands(
    painter: &Painter,
    rect: Rect,
    ticks_per_bar: u64,
    pixels_per_tick: f64,
    ticks: RangeInclusive<f64>,
    x: impl Fn(u64) -> f32,
) {
    if ticks_per_bar == 0 || !pixels_per_tick.is_finite() || pixels_per_tick <= 0.0 {
        return;
    }
    let band = ticks_per_bar.saturating_mul(bars_per_band(ticks_per_bar as f64 * pixels_per_tick));
    let first = (ticks.start().max(0.0) as u64) / band;
    let last = (ticks.end().max(0.0) as u64) / band;
    let color = theme::bar_band();
    for index in first..=last {
        if index % 2 == 0 {
            continue;
        }
        let start = index.saturating_mul(band);
        let left = x(start).round();
        let right = x(start.saturating_add(band)).round();
        let shade = Rect::from_x_y_ranges(left..=right, rect.y_range()).intersect(rect);
        if shade.is_positive() {
            painter.rect_filled(shade, 0.0, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_follow_single_bars_until_they_get_too_thin() {
        assert_eq!(bars_per_band(80.0), 1);
        assert_eq!(bars_per_band(12.0), 1);
        assert_eq!(bars_per_band(11.9), 2);
        assert_eq!(bars_per_band(4.0), 4);
        assert_eq!(bars_per_band(0.5), 32);
        assert_eq!(bars_per_band(0.0), 1 << 20);
    }
}
