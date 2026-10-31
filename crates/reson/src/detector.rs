//! Resonance detection: turns the power spectrum of one analysis frame into the reduction
//! each frequency bin should get.
//!
//! A resonance is a frequency that rings louder than its spectral neighbourhood, so every
//! bin is compared with the mean power of the bins on either side of it, leaving out a
//! guard band where the bin's own peak spreads — the cell-averaging test radar uses to
//! pick targets out of clutter. The bin must stand out from the louder of its two sides,
//! which makes it a true local peak: the slope of a spectral tilt or the flank of a broad
//! hump is quieter than one side and never reads as a resonance, and a quiet side (say,
//! sub-bass below a high-pass) cannot make an ordinary bin look prominent. Measured this
//! way the detector does not care how loud the signal is, only how far a peak stands out,
//! and a peak's own energy never raises its reference.
//!
//! What stands out by more than the Threshold is turned down by up to Depth (scaled by the
//! sensitivity curve), the cut growing smoothly over the Knee: the further past the
//! threshold a peak must rise for the full cut, the gentler. Width sets how far each
//! reduction spreads around its peak, in octaves, and with it how wide a guard band the
//! reference skips, so wide settings catch broad humps with wide dips and narrow ones pick
//! single harmonics out with narrow notches.
//!
//! With Focus, the harmonics of the notes the `pitch` module finds may only be cut as far
//! as they stand above their neighbouring harmonics.

use crate::layout::Layout;
use crate::spectral::log2;

/// Power treated as silence, in the detector's raw units where a full-scale sine centred on
/// a bin reads `(frame/4)²` — about −140 dB below it.
pub(super) const POWER_FLOOR: f32 = 1.0e-9;
/// Decibels per octave of power ratio.
const DB_PER_LOG2: f32 = 3.010_3;
/// The deepest cut any bin can receive.
pub(super) const MAX_REDUCTION_DB: f32 = 40.0;
/// Reductions smaller than this are dropped, so idle bins hold exact zeros.
const NEGLIGIBLE_DB: f32 = 1.0e-4;
/// Lowest bin the detector can judge. Lower bins have no room for a reference below their
/// guard band; cuts found above still spread down into them.
const FIRST_BIN: usize = 3;
/// Training bins each side of a reference needs before its bin is judged.
const MIN_SIDE: usize = 2;
/// The training cells reach at least this many bins (375 Hz) each way. A low voice's
/// harmonics sit further apart than half an octave of bins, and each would otherwise be
/// judged against the gaps between them and read as a resonance.
const MIN_REACH: usize = 16;

/// Per-frame detector settings, in physical units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Settings {
    /// Reduction for a peak far above threshold, in dB.
    pub(super) depth: f32,
    /// How far a bin must stand out above its neighbourhood before it is reduced, in dB.
    pub(super) threshold: f32,
    /// How far past the threshold, in dB, a bin must stand out for the full reduction;
    /// zero cuts in full the moment it is crossed.
    pub(super) knee: f32,
}

/// Reference of one bin: training bins `lo..guard_lo` below it and `guard_hi + 1..=hi`
/// above it. An empty window marks a bin the detector does not judge.
#[derive(Debug, Clone, Copy, Default)]
struct Window {
    lo: u16,
    hi: u16,
    guard_lo: u16,
    guard_hi: u16,
    inverse_below: f32,
    inverse_above: f32,
}

/// Frequency layout for one Width setting.
#[derive(Debug)]
pub(super) struct Shape {
    width: f32,
    bins: usize,
    half: usize,
    windows: Box<[Window]>,
    /// Factor a reduction keeps when it spreads from bin `k − 1` into bin `k` (and back).
    steps: Box<[f32]>,
}

/// Narrowest and widest spread of a reduction, in octaves.
const WIDTHS: (f32, f32) = (1.0 / 64.0, 0.25);

impl Shape {
    /// The layout for a reduction spreading `width` octaves around its peak: the distance
    /// over which it falls by a factor of e.
    pub(super) fn new(layout: &Layout, width: f32) -> Self {
        let bins = layout.bins();
        let mut shape = Self {
            width,
            bins,
            half: layout.half(),
            windows: vec![Window::default(); bins].into_boxed_slice(),
            steps: vec![0.0; bins].into_boxed_slice(),
        };
        shape.rebuild(width);
        shape
    }

    /// Lays the tables out for a new Width, in place so automation never allocates.
    pub(super) fn rebuild(&mut self, width: f32) {
        self.width = width;
        let spread = width.clamp(WIDTHS.0, WIDTHS.1);
        // The guard covers the peak's own width; the training cells reach half an octave
        // further each way, so the reference tracks the local level and tilt.
        let guard_octaves = 1.5 * spread;
        let train_octaves = guard_octaves + 0.5;
        let (guard_down, guard_up) = ((-guard_octaves).exp2(), guard_octaves.exp2());
        let (train_down, train_up) = ((-train_octaves).exp2(), train_octaves.exp2());
        let half = self.half;
        let last = half - 1;
        for (k, window) in self.windows.iter_mut().enumerate() {
            if !(FIRST_BIN..half).contains(&k) {
                *window = Window::default();
                continue;
            }
            let position = k as f32;
            let guard_lo = ((position * guard_down).floor() as usize).clamp(1, k - 2);
            let guard_hi = ((position * guard_up).ceil() as usize).max(k + 2).min(last);
            let lo = ((position * train_down).floor() as usize)
                .min(guard_lo.saturating_sub(4))
                .min(k.saturating_sub(MIN_REACH))
                .max(1);
            let hi = ((position * train_up).ceil() as usize)
                .max(guard_hi + 4)
                .max(k + MIN_REACH)
                .min(last);
            let (below, above) = (guard_lo - lo, hi - guard_hi);
            *window = if below < MIN_SIDE || above < MIN_SIDE {
                Window::default()
            } else {
                Window {
                    lo: lo as u16,
                    hi: hi as u16,
                    guard_lo: guard_lo as u16,
                    guard_hi: guard_hi as u16,
                    inverse_below: 1.0 / below as f32,
                    inverse_above: 1.0 / above as f32,
                }
            };
        }
        for (k, step) in self.steps.iter_mut().enumerate() {
            *step = if k < 2 {
                0.0
            } else {
                (-((k as f32) / (k as f32 - 1.0)).log2() / spread).exp()
            };
        }
    }

    pub(super) fn width(&self) -> f32 {
        self.width
    }
}

/// Reusable working memory for [`reductions`].
#[derive(Debug)]
pub(super) struct Scratch {
    prefix: Box<[f64]>,
}

impl Scratch {
    pub(super) fn new(layout: &Layout) -> Self {
        Self {
            prefix: vec![0.0; layout.bins() + 1].into_boxed_slice(),
        }
    }
}

/// How much Focus spares the notes: the cut each bin allows, in dB (infinite away from
/// the notes), and the Focus amount from 0 to 1.
#[derive(Debug, Clone, Copy)]
pub(super) struct Spare<'a> {
    pub(super) allowed: &'a [f32],
    pub(super) focus: f32,
}

impl Spare<'_> {
    /// Takes the share Focus asks for off every cut deeper than its bin allows.
    fn apply(self, target: &mut [f32]) {
        for (target, &allowed) in target.iter_mut().zip(self.allowed) {
            if *target > allowed {
                *target -= self.focus * (*target - allowed);
            }
        }
    }
}

/// Writes the target reduction of every bin, in dB, for one frame's power spectrum.
pub(super) fn reductions(
    power: &[f32],
    weights: &[f32],
    shape: &Shape,
    settings: Settings,
    spare: Option<Spare<'_>>,
    scratch: &mut Scratch,
    target: &mut [f32],
) {
    let bins = shape.bins;
    let (power, weights, target) = (&power[..bins], &weights[..bins], &mut target[..bins]);
    let prefix = &mut scratch.prefix[..bins + 1];
    let mut sum = 0.0_f64;
    prefix[0] = 0.0;
    for (k, &value) in power.iter().enumerate() {
        sum += f64::from(value.max(POWER_FLOOR));
        prefix[k + 1] = sum;
    }
    target.fill(0.0);
    if settings.depth > 0.0 {
        for k in FIRST_BIN..shape.half {
            let weight = weights[k];
            let window = shape.windows[k];
            if weight <= 0.0 || window.hi == 0 {
                continue;
            }
            let (lo, hi) = (usize::from(window.lo), usize::from(window.hi));
            let (guard_lo, guard_hi) = (usize::from(window.guard_lo), usize::from(window.guard_hi));
            let below = (prefix[guard_lo] - prefix[lo]) as f32 * window.inverse_below;
            let above = (prefix[hi + 1] - prefix[guard_hi + 1]) as f32 * window.inverse_above;
            let reference = below.max(above).max(POWER_FLOOR);
            let excess = DB_PER_LOG2 * log2(power[k].max(POWER_FLOOR) / reference);
            let over = excess - settings.threshold;
            if over <= 0.0 {
                continue;
            }
            let amount = if over >= settings.knee {
                1.0
            } else {
                let t = over / settings.knee;
                t * t * (3.0 - 2.0 * t)
            };
            target[k] = (settings.depth * weight * amount).min(MAX_REDUCTION_DB);
        }
    }
    // Spared before spreading, so a note's harmonics start no cuts around them, and after,
    // so no other cut spreads over them.
    if let Some(spare) = spare {
        spare.apply(target);
    }
    spread(target, &shape.steps);
    if let Some(spare) = spare {
        spare.apply(target);
    }
}

/// Spreads every reduction over its neighbours, keeping the deepest cut where spreads
/// overlap, then rounds the result with two binomial passes so no cut is narrower than
/// the analysis can resolve.
fn spread(target: &mut [f32], steps: &[f32]) {
    let bins = target.len();
    let steps = &steps[..bins];
    for k in 1..bins {
        target[k] = target[k].max(target[k - 1] * steps[k]);
    }
    for k in (0..bins - 1).rev() {
        target[k] = target[k].max(target[k + 1] * steps[k + 1]);
    }
    for _ in 0..2 {
        let mut previous = target[0];
        for k in 1..bins - 1 {
            let current = target[k];
            target[k] = 0.25 * (previous + target[k + 1]) + 0.5 * current;
            previous = current;
        }
    }
}

/// One-pole attack/release smoothing of each bin's reduction towards its target.
pub(super) fn follow(state: &mut [f32], target: &[f32], attack: f32, release: f32) -> f32 {
    let mut deepest: f32 = 0.0;
    for (state, &target) in state.iter_mut().zip(target) {
        let rate = if target > *state { attack } else { release };
        let next = *state + rate * (target - *state);
        *state = if next < NEGLIGIBLE_DB { 0.0 } else { next };
        deepest = deepest.max(*state);
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINS: usize = 1025;
    const HALF: usize = 1024;

    fn layout() -> Layout {
        Layout::new(48_000.0)
    }

    fn detect(power: &[f32], width: f32, settings: Settings) -> Vec<f32> {
        let shape = Shape::new(&layout(), width);
        let weights = vec![1.0; BINS];
        let mut target = vec![0.0; BINS];
        reductions(
            power,
            &weights,
            &shape,
            settings,
            None,
            &mut Scratch::new(&layout()),
            &mut target,
        );
        target
    }

    const SETTINGS: Settings = Settings {
        depth: 10.0,
        threshold: 5.0,
        knee: 12.0,
    };

    #[test]
    fn windows_stay_inside_the_spectrum_and_skip_their_guard() {
        for width in [WIDTHS.1, 1.0 / 12.0, WIDTHS.0] {
            let shape = Shape::new(&layout(), width);
            let judged: Vec<usize> = (0..BINS).filter(|&k| shape.windows[k].hi != 0).collect();
            // Everything from about 120 Hz is judged,
            assert!(judged[0] <= 5, "{width}: first judged bin {}", judged[0]);
            // and up to 16 kHz even with the widest guard band.
            assert!(*judged.last().unwrap() >= 683, "{width}");
            assert_eq!(judged.len(), judged.last().unwrap() - judged[0] + 1);
            for &k in &judged {
                let window = shape.windows[k];
                let (lo, hi) = (usize::from(window.lo), usize::from(window.hi));
                let (guard_lo, guard_hi) =
                    (usize::from(window.guard_lo), usize::from(window.guard_hi));
                assert!(lo >= 1 && hi < HALF, "{width} {k}");
                assert!(guard_lo + 2 <= k && k + 2 <= guard_hi, "{width} {k}");
                assert!(guard_lo - lo >= MIN_SIDE && hi - guard_hi >= MIN_SIDE);
            }
        }
    }

    #[test]
    fn flat_spectrum_needs_no_reduction() {
        let target = detect(&vec![1.0; BINS], 1.0 / 12.0, SETTINGS);
        assert!(target.iter().all(|&r| r == 0.0));
    }

    #[test]
    fn isolated_peak_is_cut_by_up_to_depth_and_spreads_with_width() {
        let mut power = vec![1.0; BINS];
        power[400] = 1.0e4;
        power[399] = 2.5e3;
        power[401] = 2.5e3;
        let narrow = detect(&power, WIDTHS.0, SETTINGS);
        let wide = detect(&power, WIDTHS.1, SETTINGS);
        for target in [&narrow, &wide] {
            assert!(target[400] > 8.0 && target[400] <= 10.0, "{}", target[400]);
            assert!(target[100] < 0.2 && target[900] < 0.2);
        }
        let width = |target: &[f32]| target.iter().filter(|&&r| r > 1.0).count();
        assert!(
            width(&wide) > 3 * width(&narrow),
            "{} vs {}",
            width(&wide),
            width(&narrow)
        );
        // A threshold above the peak's prominence leaves it alone.
        let selective = detect(
            &power,
            1.0 / 12.0,
            Settings {
                threshold: 60.0,
                ..SETTINGS
            },
        );
        assert!(selective.iter().all(|&r| r == 0.0));
    }

    #[test]
    fn the_knee_sets_how_far_past_the_threshold_the_full_cut_comes() {
        // A peak 9 dB past the threshold.
        let mut power = vec![1.0; BINS];
        power[400] = 10.0_f32.powf((5.0 + 9.0) / 10.0);
        let cut = |knee: f32| {
            let target = detect(&power, WIDTHS.0, Settings { knee, ..SETTINGS });
            target[400]
        };
        let full = cut(0.0);
        assert!(full > 8.0, "{full}");
        // A knee narrower than the peak's excess still cuts it in full,
        assert!((cut(6.0) - full).abs() < 1.0e-4);
        // halfway through a knee is half the cut,
        assert!((cut(18.0) / full - 0.5).abs() < 1.0e-3, "{}", cut(18.0));
        // and the cut eases in over a wide knee.
        let gentle = cut(24.0) / full;
        assert!(gentle > 0.3 && gentle < 0.33, "{gentle}");
    }

    #[test]
    fn reference_follows_a_spectral_tilt() {
        // A steep −6 dB/octave slope with no peaks on it.
        let power: Vec<f32> = (0..BINS)
            .map(|k| 1.0e6 / (k.max(1) as f32).powi(2))
            .collect();
        let target = detect(&power, 1.0 / 12.0, SETTINGS);
        assert!(
            target.iter().all(|&r| r < 0.5),
            "{:?}",
            target.iter().cloned().fold(0.0, f32::max)
        );
    }

    #[test]
    fn follower_attacks_and_releases_at_their_own_rates() {
        let mut state = vec![0.0; BINS];
        let mut target = vec![0.0; BINS];
        target[10] = 10.0;
        let deepest = follow(&mut state, &target, 0.5, 0.1);
        assert_eq!(deepest, 5.0);
        target[10] = 0.0;
        follow(&mut state, &target, 0.5, 0.1);
        assert!((state[10] - 4.5).abs() < 1e-6);
    }
}
