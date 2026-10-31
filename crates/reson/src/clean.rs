//! Noise cleaning for Clean: turns down whatever sits on the steady noise floor, and
//! leaves the notes, and anything that rises well above the floor, alone.
//!
//! Every bin keeps an estimate of the noise floor under it. The bin's power, averaged with
//! two neighbours either side and smoothed over about 40 ms, is followed down at once and
//! back up only slowly, a couple of dB a second, so the floor settles on the quiet moments
//! between sounds while anything that comes and goes rides above it. The harmonics of the
//! notes the `pitch` module finds never move the floor, however long a note is held: under
//! a harmonic the floor is taken from the bins beside it. Nor may any bin's floor stand
//! more than 3 dB above the floor about 140 Hz to either side, so a steady tone that is no
//! note (a sine, a flute's soft note the search misses, a drone) can never pass for noise:
//! noise is broad, and only broad sound lifts the floor. Frames that reach into digital
//! silence teach nothing, so a clip that starts from silence cannot drag the floor down.
//!
//! Each bin is then turned down by how close it sits to its floor, as spectral subtraction
//! would: a bin at up to three times the noise's power is cut by the full Clean depth, one
//! 6 dB above that by about 3 dB, and one 20 dB above it hardly at all. Harmonics of notes
//! that stand 8 dB or more clear of the noise are never cut; one buried in it cannot be
//! told from it, and the search for a note's harmonics, which takes the nearest peak, will
//! pick a peak of the noise for it as readily.
//!
//! The cuts are rounded across neighbouring bins so single bins do not flicker, let go
//! within a couple of milliseconds when a sound arrives, and come back over about 60 ms,
//! so a sound's decay fades into the floor instead of being chopped off. An attack lets
//! every cut go at once: the analysis window weighs its newest samples least and would
//! hear the attack only after its first milliseconds had passed through the cuts.

use crate::detector::{self, MAX_REDUCTION_DB, POWER_FLOOR};
use crate::layout::Layout;
use crate::spectral::log2;

/// Bins either side averaged into a bin's level.
const REACH: usize = 2;
/// Smoothing of the level the floor follows, in ms.
const LEVEL_MS: f64 = 40.0;
/// How fast the floor may rise, in dB per second.
const RISE_DB_PER_SECOND: f64 = 2.0;
/// Mean noise power over the floor that follows its dips.
const BIAS: f32 = 1.5;
/// Bins to either side whose floors cap a bin's, and how far above the lower of them, as a
/// power ratio, the bin's may stand. Past the reach of a tone's own level.
const SIDE: usize = 6;
const CAP: f32 = 2.0;
/// Power over the noise, as a multiple of it, below which a bin is cut in full.
const OVERSUBTRACT: f32 = 3.0;
/// How far over the noise, as a power ratio, the loudest bin of a note's harmonic must
/// stand for the harmonic to be left alone: further than the peaks of noise itself reach
/// but for one bin in a hundred or so.
const CLEAR: f32 = 6.0;
/// Bins either side of a harmonic's peak that make up its main lobe.
const LOBE: usize = 2;
/// Share of a spared harmonic's protection each bin out keeps: four tenths of the bin
/// before it.
const FADE: [f32; 5] = [1.0, 0.4, 0.16, 0.064, 0.0256];
/// How fast a cut lets go when a sound arrives and comes back when it fades, in ms.
const OPEN_MS: f64 = 2.0;
const CLOSE_MS: f64 = 60.0;
/// How long the floor learns before Clean works at full depth, in seconds.
const LEARN_SECONDS: f64 = 1.0;
const DB_PER_LOG2: f32 = 3.010_3;

#[derive(Debug)]
pub(super) struct Cleaner {
    /// Each bin's level, averaged over its neighbours and smoothed over time, and the noise
    /// floor under it, in the detector's raw power units.
    level: Box<[f32]>,
    floor: Box<[f32]>,
    /// The mean noise power each bin's floor stands for, capped by the floors beside it.
    noise: Box<[f32]>,
    /// This frame's power of each bin averaged over its neighbours, and scratch.
    near: Box<[f32]>,
    scratch: Box<[f32]>,
    /// Target and current cut of each bin, in dB.
    target: Box<[f32]>,
    pub(super) state: Box<[f32]>,
    /// Whether any cut is left to let go of.
    cutting: bool,
    /// Hops the floor has learned from, up to `learn_hops`.
    learned: u32,
    learn_hops: u32,
    level_rate: f32,
    rise: f32,
    open_rate: f32,
    close_rate: f32,
    bins: usize,
}

fn buffer(length: usize, value: f32) -> Box<[f32]> {
    vec![value; length].into_boxed_slice()
}

impl Cleaner {
    pub(super) fn new(layout: &Layout) -> Self {
        let hop_seconds = layout.hop_seconds();
        let bins = layout.bins();
        let buffer = |value: f32| buffer(bins, value);
        Self {
            level: buffer(0.0),
            floor: buffer(f32::INFINITY),
            noise: buffer(f32::INFINITY),
            near: buffer(0.0),
            scratch: buffer(0.0),
            target: buffer(0.0),
            state: buffer(0.0),
            cutting: false,
            learned: 0,
            learn_hops: (LEARN_SECONDS / hop_seconds).round() as u32,
            level_rate: layout.hop_rate(LEVEL_MS),
            rise: 10.0_f64.powf(RISE_DB_PER_SECOND * hop_seconds / 10.0) as f32,
            open_rate: layout.hop_rate(OPEN_MS),
            close_rate: layout.hop_rate(CLOSE_MS),
            bins,
        }
    }

    /// Drops every cut at once. The floor stays: it belongs to the material, not to the
    /// moment, and survives the resets the engine performs at every loop wrap.
    pub(super) fn clear(&mut self) {
        self.state.fill(0.0);
        self.cutting = false;
    }

    /// Takes over another channel's floor and cuts, as when its channels stop sharing one.
    pub(super) fn copy_from(&mut self, other: &Self) {
        self.level.copy_from_slice(&other.level);
        self.floor.copy_from_slice(&other.floor);
        self.noise.copy_from_slice(&other.noise);
        self.state.copy_from_slice(&other.state);
        self.cutting = other.cutting;
        self.learned = other.learned;
    }

    #[cfg(test)]
    pub(super) fn noise(&self) -> &[f32] {
        &self.noise
    }

    pub(super) fn cutting(&self) -> bool {
        self.cutting
    }

    /// Lets every cut go, as when Clean is off or the input is silent.
    pub(super) fn release(&mut self) {
        if self.cutting {
            self.target.fill(0.0);
            self.cutting =
                detector::follow(&mut self.state, &self.target, 1.0, self.open_rate) > 0.0;
        }
    }

    /// Learns the floor from one frame's power, unless `learn` is false, and moves each
    /// bin's cut towards how close it sits to the floor, or lets every cut go during an
    /// `attack`. `allowed` is finite at the notes' harmonics, and `weights` scale the
    /// depth, in dB, at each bin.
    pub(super) fn update(
        &mut self,
        power: &[f32],
        allowed: &[f32],
        weights: &[f32],
        depth: f32,
        learn: bool,
        attack: bool,
    ) {
        let bins = self.bins;
        let (power, allowed, weights) = (&power[..bins], &allowed[..bins], &weights[..bins]);
        let near = &mut self.near[..bins];
        for (k, near) in near.iter_mut().enumerate() {
            // Summed afresh, not as a running sum: a loud peak leaving a running sum would
            // leave rounding behind that drowns the quiet bins after it.
            let span = &power[k.saturating_sub(REACH)..(k + REACH + 1).min(bins)];
            *near = (span.iter().sum::<f32>() / span.len() as f32).max(POWER_FLOOR);
        }
        if learn {
            let fresh = self.learned == 0;
            for k in 0..bins {
                let level = &mut self.level[k];
                if fresh {
                    *level = near[k];
                } else {
                    *level += self.level_rate * (near[k] - *level);
                }
                if allowed[k].is_finite() {
                    continue;
                }
                let floor = &mut self.floor[k];
                *floor = if *level < *floor {
                    *level
                } else {
                    *floor * self.rise
                };
            }
            fill_under_notes(&mut self.floor, allowed, &mut self.scratch);
            for k in 0..bins {
                let below = k.checked_sub(SIDE).map_or(f32::INFINITY, |k| self.floor[k]);
                let above = self.floor.get(k + SIDE).copied().unwrap_or(f32::INFINITY);
                self.noise[k] = BIAS * self.floor[k].min(CAP * below.min(above));
            }
            self.learned = (self.learned + 1).min(self.learn_hops);
        }
        if attack {
            self.clear();
            return;
        }
        let depth = depth * self.learned as f32 / self.learn_hops as f32;
        // A note's harmonic that stands clear of the noise is spared: marked 1 in `taper`.
        let taper = &mut self.scratch[..bins];
        let mut any = false;
        for k in 0..bins {
            let spared = allowed[k].is_finite() && {
                let lobe = &power[k.saturating_sub(LOBE)..(k + LOBE + 1).min(bins)];
                lobe.iter().fold(0.0_f32, |peak, &power| peak.max(power)) >= CLEAR * self.noise[k]
            };
            any |= spared;
            taper[k] = if spared { 1.0 } else { 0.0 };
            let deepest = (depth * weights[k]).min(MAX_REDUCTION_DB);
            self.target[k] = if spared || deepest <= 0.0 {
                0.0
            } else {
                let kept = 1.0 - OVERSUBTRACT * self.noise[k] / near[k];
                if kept <= 0.0 {
                    deepest
                } else {
                    (-DB_PER_LOG2 * log2(kept)).min(deepest)
                }
            };
        }
        let target = &mut self.target[..bins];
        for _ in 0..2 {
            let mut previous = target[0];
            for k in 1..bins - 1 {
                let current = target[k];
                target[k] = 0.25 * (previous + target[k + 1]) + 0.5 * current;
                previous = current;
            }
        }
        // Cuts fade in beside a spared harmonic rather than starting at full depth: a step
        // from no cut to a deep one makes the filter ripple between bins and lifts a
        // harmonic that falls between two of them.
        if any {
            for k in 0..bins {
                if taper[k] < 1.0 {
                    continue;
                }
                for (distance, &share) in FADE.iter().enumerate().skip(1) {
                    if let Some(below) = k.checked_sub(distance) {
                        taper[below] = taper[below].max(share);
                    }
                    if let Some(above) = taper.get_mut(k + distance) {
                        *above = above.max(share);
                    }
                }
            }
            for (target, &taper) in target.iter_mut().zip(taper.iter()) {
                *target *= 1.0 - taper;
            }
        }
        self.cutting = detector::follow(
            &mut self.state,
            &self.target,
            self.close_rate,
            self.open_rate,
        ) > 0.0;
    }
}

/// Gives every bin under a note's harmonic the floor of the free bins either side of it
/// (their mean, or the one there is), unless its own is lower already.
fn fill_under_notes(floor: &mut [f32], allowed: &[f32], below: &mut [f32]) {
    let bins = floor.len();
    let (allowed, below) = (&allowed[..bins], &mut below[..bins]);
    let mut last = f32::INFINITY;
    for k in 0..bins {
        if allowed[k].is_finite() {
            below[k] = last;
        } else {
            last = floor[k];
        }
    }
    let mut next = f32::INFINITY;
    for k in (0..bins).rev() {
        if allowed[k].is_finite() {
            let side = match (below[k].is_finite(), next.is_finite()) {
                (true, true) => 0.5 * (below[k] + next),
                (true, false) => below[k],
                _ => next,
            };
            floor[k] = floor[k].min(side);
        } else {
            next = floor[k];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINS: usize = 1025;

    #[test]
    fn under_a_harmonic_the_floor_comes_from_either_side() {
        let mut floor = vec![f32::INFINITY; BINS];
        let mut allowed = vec![f32::INFINITY; BINS];
        floor[98] = 2.0;
        floor[99] = 2.0;
        floor[103] = 4.0;
        allowed[100..=102].fill(0.0);
        floor[101] = 1.0;
        fill_under_notes(&mut floor, &allowed, &mut vec![0.0; BINS]);
        assert_eq!(&floor[99..=103], &[2.0, 3.0, 1.0, 3.0, 4.0]);
    }
}
