//! Linkwitz-Riley crossovers that split the signal into up to six bands which add back up
//! to an all-pass of the input: flat in level, turned only in phase.
//!
//! The bands are split off from the bottom up: the first crossover takes the lowest band
//! off the input, the second takes the next band off what is left, and so on, and the last
//! band is whatever passes them all. A Linkwitz-Riley pair adds up to an all-pass, so each
//! band below a later crossover also goes through that crossover's all-pass, which keeps
//! every band in phase with the others and their sum flat.
//!
//! The slopes are 12 dB/octave (a squared first-order pair, whose high side is inverted),
//! 24 (a squared second-order Butterworth pair) and 48 (a squared fourth-order one).

use crate::filters::{Biquad, Coefficients};
use crate::params::{CROSSOVERS, MAX_BANDS};

/// The Butterworth Qs of a fourth-order filter's two sections.
const FOURTH_ORDER_Q: [f64; 2] = [0.541_196_100_146_197, 1.306_562_964_876_376_7];
const SECOND_ORDER_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// A crossover's steepness: the index of its setting in `params::SLOPES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slope {
    Twelve,
    TwentyFour,
    FortyEight,
}

impl Slope {
    pub(crate) fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Twelve,
            1 => Self::TwentyFour,
            _ => Self::FortyEight,
        }
    }

    /// Order of the Butterworth filter that is squared.
    fn order(self) -> i32 {
        match self {
            Self::Twelve => 1,
            Self::TwentyFour => 2,
            Self::FortyEight => 4,
        }
    }
}

/// One crossover: its low and high sides, and the all-pass they add up to.
#[derive(Debug, Clone)]
struct Split {
    low: [Biquad; 4],
    high: [Biquad; 4],
    sections: usize,
    invert_high: bool,
}

impl Split {
    fn new() -> Self {
        Self {
            low: [Biquad::default(); 4],
            high: [Biquad::default(); 4],
            sections: 0,
            invert_high: false,
        }
    }

    fn tune(&mut self, rate: f64, frequency: f64, slope: Slope) {
        let (low, high): (Vec<Coefficients>, Vec<Coefficients>) = match slope {
            Slope::Twelve => (
                vec![Coefficients::lowpass1(rate, frequency); 2],
                vec![Coefficients::highpass1(rate, frequency); 2],
            ),
            Slope::TwentyFour => (
                vec![Coefficients::lowpass(rate, frequency, SECOND_ORDER_Q); 2],
                vec![Coefficients::highpass(rate, frequency, SECOND_ORDER_Q); 2],
            ),
            Slope::FortyEight => (
                FOURTH_ORDER_Q
                    .iter()
                    .chain(FOURTH_ORDER_Q.iter())
                    .map(|&q| Coefficients::lowpass(rate, frequency, q))
                    .collect(),
                FOURTH_ORDER_Q
                    .iter()
                    .chain(FOURTH_ORDER_Q.iter())
                    .map(|&q| Coefficients::highpass(rate, frequency, q))
                    .collect(),
            ),
        };
        self.sections = low.len();
        self.invert_high = slope == Slope::Twelve;
        for (filter, coefficients) in self.low.iter_mut().zip(low) {
            filter.set(coefficients);
        }
        for (filter, coefficients) in self.high.iter_mut().zip(high) {
            filter.set(coefficients);
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> (f64, f64) {
        let (mut low, mut high) = (x, x);
        for filter in &mut self.low[..self.sections] {
            low = filter.process(low);
        }
        for filter in &mut self.high[..self.sections] {
            high = filter.process(high);
        }
        (low, if self.invert_high { -high } else { high })
    }

    fn clear(&mut self) {
        for filter in self.low.iter_mut().chain(self.high.iter_mut()) {
            filter.clear();
        }
    }

    fn flush(&mut self) {
        for filter in self.low.iter_mut().chain(self.high.iter_mut()) {
            filter.flush();
        }
    }
}

/// The all-pass a crossover's two sides add up to.
#[derive(Debug, Clone)]
struct AllPass {
    filters: [Biquad; 2],
    sections: usize,
}

impl AllPass {
    fn new() -> Self {
        Self {
            filters: [Biquad::default(); 2],
            sections: 0,
        }
    }

    fn tune(&mut self, rate: f64, frequency: f64, slope: Slope) {
        let coefficients: Vec<Coefficients> = match slope {
            Slope::Twelve => vec![Coefficients::allpass1(rate, frequency)],
            Slope::TwentyFour => vec![Coefficients::allpass(rate, frequency, SECOND_ORDER_Q)],
            Slope::FortyEight => FOURTH_ORDER_Q
                .iter()
                .map(|&q| Coefficients::allpass(rate, frequency, q))
                .collect(),
        };
        self.sections = coefficients.len();
        for (filter, coefficients) in self.filters.iter_mut().zip(coefficients) {
            filter.set(coefficients);
        }
    }

    #[inline]
    fn process(&mut self, mut x: f64) -> f64 {
        for filter in &mut self.filters[..self.sections] {
            x = filter.process(x);
        }
        x
    }

    fn clear(&mut self) {
        for filter in &mut self.filters {
            filter.clear();
        }
    }

    fn flush(&mut self) {
        for filter in &mut self.filters {
            filter.flush();
        }
    }
}

/// One channel's crossovers.
#[derive(Debug, Clone)]
pub(crate) struct Crossover {
    rate: f64,
    splits: [Split; CROSSOVERS],
    /// `compensation[band][split]` is the all-pass of a later split that `band` goes
    /// through.
    compensation: [[AllPass; CROSSOVERS]; CROSSOVERS],
}

impl Crossover {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            rate,
            splits: std::array::from_fn(|_| Split::new()),
            compensation: std::array::from_fn(|_| std::array::from_fn(|_| AllPass::new())),
        }
    }

    /// Sets the crossover frequencies, lowest first, and their slope.
    pub(crate) fn tune(&mut self, frequencies: &[f64; CROSSOVERS], slope: Slope) {
        for (index, split) in self.splits.iter_mut().enumerate() {
            split.tune(self.rate, frequencies[index], slope);
        }
        for band in &mut self.compensation {
            for (index, allpass) in band.iter_mut().enumerate() {
                allpass.tune(self.rate, frequencies[index], slope);
            }
        }
    }

    /// Splits `x` into `count` bands, lowest first, into `bands`. One band is the input
    /// itself.
    #[inline]
    pub(crate) fn split(&mut self, x: f64, count: usize, bands: &mut [f64; MAX_BANDS]) {
        if count <= 1 {
            bands[0] = x;
            return;
        }
        let mut rest = x;
        for (index, split) in self.splits[..count - 1].iter_mut().enumerate() {
            let (low, high) = split.process(rest);
            bands[index] = low;
            rest = high;
        }
        bands[count - 1] = rest;
        for (band, value) in bands[..count - 2].iter_mut().enumerate() {
            for allpass in &mut self.compensation[band][band + 1..count - 1] {
                *value = allpass.process(*value);
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        for split in &mut self.splits {
            split.clear();
        }
        for allpass in self.compensation.iter_mut().flatten() {
            allpass.clear();
        }
    }

    pub(crate) fn flush(&mut self) {
        for split in &mut self.splits {
            split.flush();
        }
        for allpass in self.compensation.iter_mut().flatten() {
            allpass.flush();
        }
    }
}

/// The level of band `band` of `count` at `hz`, from the analog responses of the
/// crossovers below and above it, for an interface to draw.
pub fn band_response(frequencies: &[f64], slope: usize, count: usize, band: usize, hz: f64) -> f64 {
    if count <= 1 {
        return 1.0;
    }
    let order = Slope::from_index(slope).order();
    let low = |crossover: f64| 1.0 / (1.0 + (hz / crossover).powi(2 * order));
    let high = |crossover: f64| 1.0 - low(crossover);
    let mut level = 1.0;
    for &crossover in &frequencies[..band.min(count - 1)] {
        level *= high(crossover);
    }
    if band < count - 1 {
        level *= low(frequencies[band]);
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The amplitude of a sine at `hz` after `count` bands are split off and added back,
    /// and of each band on its own.
    fn levels(count: usize, slope: Slope, hz: f64) -> (f64, Vec<f64>) {
        let rate = 48_000.0;
        let mut crossover = Crossover::new(rate);
        crossover.tune(&[150.0, 800.0, 3000.0, 7000.0, 13_000.0], slope);
        let mut bands = [0.0; MAX_BANDS];
        // Each amplitude is read off its correlation with the tone over the second half,
        // a whole number of cycles.
        let mut sum = (0.0_f64, 0.0_f64);
        let mut each = vec![(0.0_f64, 0.0_f64); count];
        let w = std::f64::consts::TAU * hz / rate;
        for i in 0..96_000 {
            crossover.split((w * i as f64).sin(), count, &mut bands);
            if i >= 48_000 {
                let (sin, cos) = (w * i as f64).sin_cos();
                let total: f64 = bands[..count].iter().sum();
                sum = (sum.0 + total * sin, sum.1 + total * cos);
                for (pair, band) in each.iter_mut().zip(&bands) {
                    *pair = (pair.0 + band * sin, pair.1 + band * cos);
                }
            }
        }
        let amplitude = |(sin, cos): (f64, f64)| 2.0 * sin.hypot(cos) / 48_000.0;
        (amplitude(sum), each.into_iter().map(amplitude).collect())
    }

    #[test]
    fn the_bands_add_back_up_to_the_input_s_level() {
        for slope in [Slope::Twelve, Slope::TwentyFour, Slope::FortyEight] {
            for count in 1..=MAX_BANDS {
                for hz in [60.0, 150.0, 440.0, 800.0, 2000.0, 3000.0, 7000.0, 12_000.0] {
                    let (sum, _) = levels(count, slope, hz);
                    assert!(
                        (sum - 1.0).abs() < 1.0e-6,
                        "{slope:?}, {count} bands, {hz} Hz: {sum}"
                    );
                }
            }
        }
    }

    #[test]
    fn each_band_holds_its_own_range() {
        // A tone in the middle of the third of four bands comes out of it alone.
        let (_, bands) = levels(4, Slope::FortyEight, 1500.0);
        assert!(bands[2] > 0.95, "{bands:?}");
        for band in [0, 1, 3] {
            assert!(bands[band] < 0.1, "{band}: {bands:?}");
        }
        // At a crossover, the bands either side share it, −6 dB each.
        let (_, bands) = levels(2, Slope::TwentyFour, 150.0);
        assert!(
            (bands[0] - 0.5).abs() < 0.02 && (bands[1] - 0.5).abs() < 0.02,
            "{bands:?}"
        );
        let analog = band_response(&[150.0], 1, 2, 0, 150.0);
        assert!((analog - 0.5).abs() < 1.0e-12);
        assert_eq!(band_response(&[], 1, 1, 0, 1000.0), 1.0);
        let total: f64 = (0..4)
            .map(|band| band_response(&[150.0, 800.0, 3000.0], 2, 4, band, 400.0))
            .sum();
        assert!((total - 1.0).abs() < 1.0e-12, "{total}");
    }
}
