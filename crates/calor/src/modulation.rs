//! Calor's modulation: an LFO, and an envelope follower on the input, routed through four
//! slots to the controls they move.
//!
//! Each slot has a source (the LFO, swinging from −1 to 1, or the envelope, from 0 to 1),
//! a target (one of a band's controls, for one band or all of them, or Mix, Tone or
//! Output) and an amount, a share of the target's travel: at 100 % the LFO sweeps the
//! target's whole range around its setting, and the envelope pushes it up by the whole
//! range at full level. Slots on the same target add up.

use std::f64::consts::TAU;

use crate::dynamics::Follower;
use crate::params::{MIX, MOD_TARGETS, OUTPUT, Slot, TONE, band_parameter, decibels};

/// The envelope's span: silence below −60 dBFS reads 0, full scale reads 1.
const ENVELOPE_FLOOR_DB: f64 = -60.0;

/// The band control each of the first ten targets moves, in `MOD_TARGETS` order.
const BAND_TARGETS: [Slot; 10] = [
    Slot::Drive,
    Slot::Hardness,
    Slot::Bias,
    Slot::Odd,
    Slot::Even,
    Slot::Emphasis,
    Slot::Dynamics,
    Slot::Feedback,
    Slot::Mix,
    Slot::Level,
];

/// The LFO: a phase, and a shape read off it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Lfo {
    phase: f64,
}

impl Lfo {
    pub(crate) fn advance(&mut self, hz: f64, rate: f64, samples: usize) {
        self.phase = (self.phase + hz * samples as f64 / rate).fract();
    }

    /// The LFO's value for shape `shape` (an index into `params::LFO_SHAPES`): each starts
    /// at 0 and rises, like the sine.
    pub(crate) fn value(&self, shape: usize) -> f64 {
        let phase = self.phase;
        match shape {
            0 => (TAU * phase).sin(),
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

    /// Where the LFO is in its cycle, from 0 to 1.
    pub(crate) fn phase(&self) -> f64 {
        self.phase
    }

    pub(crate) fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// The two sources and where they stand.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sources {
    pub(crate) lfo: Lfo,
    pub(crate) envelope: Follower,
}

impl Sources {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            lfo: Lfo::default(),
            envelope: Follower::new(rate, 10.0, 150.0),
        }
    }

    /// The envelope from 0 to 1.
    pub(crate) fn envelope_value(&self) -> f64 {
        ((decibels(self.envelope.level()) - ENVELOPE_FLOOR_DB) / -ENVELOPE_FLOOR_DB).clamp(0.0, 1.0)
    }

    pub(crate) fn reset(&mut self) {
        self.lfo.reset();
        self.envelope.clear();
    }
}

/// Calls `apply` with every parameter a slot's `target` and `band` (an index into
/// `params::MOD_BANDS`, 0 for all) reach, when `bands` bands are in use.
pub(crate) fn each_target(target: usize, band: usize, bands: usize, mut apply: impl FnMut(u32)) {
    match target {
        target if target < BAND_TARGETS.len() => {
            let slot = BAND_TARGETS[target];
            if band == 0 {
                for band in 0..bands {
                    apply(band_parameter(band, slot));
                }
            } else if band <= bands {
                apply(band_parameter(band - 1, slot));
            }
        }
        target if target < MOD_TARGETS.len() => {
            apply([MIX, TONE, OUTPUT][target - BAND_TARGETS.len()])
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{DRIVE, MAX_BANDS};

    #[test]
    fn every_shape_starts_at_zero_rising_and_spans_minus_one_to_one() {
        for shape in 0..4 {
            let mut lfo = Lfo::default();
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            let first = lfo.value(shape);
            for _ in 0..1000 {
                lfo.advance(1.0, 1000.0, 1);
                let value = lfo.value(shape);
                low = low.min(value);
                high = high.max(value);
            }
            assert!(first.abs() < 1.0e-9 || shape == 2, "{shape}: {first}");
            assert!(low <= -0.99 && high >= 0.99, "{shape}: {low} {high}");
        }
    }

    #[test]
    fn targets_reach_one_band_every_band_in_use_or_a_global_control() {
        let collect = |target, band, bands| {
            let mut ids = Vec::new();
            each_target(target, band, bands, |id| ids.push(id));
            ids
        };
        assert_eq!(collect(0, 1, 3), vec![DRIVE]);
        assert_eq!(
            collect(0, 0, 3),
            (0..3)
                .map(|band| band_parameter(band, Slot::Drive))
                .collect::<Vec<_>>()
        );
        // A band not in use is not touched.
        assert!(collect(0, 5, 3).is_empty());
        assert_eq!(collect(10, 4, MAX_BANDS), vec![MIX]);
        assert_eq!(collect(12, 0, 1), vec![OUTPUT]);
        let mut sources = Sources::new(1000.0);
        for _ in 0..1000 {
            sources.envelope.follow(1.0);
        }
        assert!(sources.envelope_value() > 0.99);
        sources.reset();
        assert_eq!(sources.envelope_value(), 0.0);
    }
}
