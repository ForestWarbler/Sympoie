//! Half-band filters that take the signal up to the curve's rate and back down.
//!
//! Each stage doubles or halves the rate with a Kaiser-windowed half-band filter. Every
//! other tap of such a filter is zero, so on the way up one sample of each new pair is a
//! delayed copy of the input and the other a short symmetric FIR, and on the way down the
//! same filter runs on alternate samples. A trip up and back down a stage delays its signal
//! by exactly `2·half − 1` of the stage's lower-rate samples, so the chain's delay is a
//! known number of samples at the top rate, and the dry signal can be lined up with it.

use std::f64::consts::PI;

/// Stopband attenuation of every filter, in dB.
const ATTENUATION: f64 = 80.0;
/// Highest frequency the chain keeps flat, in Hz, at rates that leave room for it.
const PASSBAND: f64 = 20_000.0;
/// Most taps a filter may have.
const MAX_TAPS: usize = 255;

/// The modified Bessel function of the first kind, order zero.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..40 {
        term *= half / k as f64;
        sum += term * term;
    }
    sum
}

/// The non-zero side taps of a half-band filter with `half` of them on each side of its
/// centre (`4·half − 1` taps in all), oldest first. The centre tap, ½, is left out; the
/// side taps are scaled to add up to ½, so the filter passes DC at unity.
fn side_taps(half: usize) -> Vec<f64> {
    let centre = 2 * half - 1;
    let beta = 0.1102 * (ATTENUATION - 8.7);
    let mut taps: Vec<f64> = (0..2 * half)
        .map(|r| {
            let offset = (2 * r) as f64 - centre as f64;
            let x = offset * PI / 2.0;
            let ratio = offset / centre as f64;
            let window = bessel_i0(beta * (1.0 - ratio * ratio).max(0.0).sqrt()) / bessel_i0(beta);
            0.5 * x.sin() / x * window
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap *= 0.5 / sum;
    }
    taps
}

/// Side taps per side for the stage that doubles `base_rate · 2^stage`: enough for the
/// passband to stay flat and for everything that would fold into it, going up or coming
/// down, to be attenuated by [`ATTENUATION`] (Kaiser's estimate).
fn half_for(base_rate: f64, stage: usize) -> usize {
    let low = base_rate * (1_u64 << stage) as f64;
    let passband = PASSBAND.min(0.45 * base_rate);
    let transition = (low - 2.0 * passband) / (2.0 * low);
    let length = (ATTENUATION - 7.95) / (14.36 * transition) + 1.0;
    (((length + 1.0) / 4.0).ceil() as usize).clamp(2, MAX_TAPS.div_ceil(4))
}

fn dot(taps: &[f32], samples: &[f32]) -> f32 {
    taps.iter()
        .zip(samples)
        .map(|(tap, sample)| tap * sample)
        .sum()
}

/// The last `len` samples of a signal, readable oldest first as one slice: each sample is
/// written twice, half a buffer apart.
#[derive(Debug, Clone)]
struct History {
    buffer: Box<[f32]>,
    len: usize,
    at: usize,
}

impl History {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; 2 * len].into_boxed_slice(),
            len,
            at: 0,
        }
    }

    #[inline]
    fn push(&mut self, sample: f32) {
        self.buffer[self.at] = sample;
        self.buffer[self.at + self.len] = sample;
        self.at += 1;
        if self.at == self.len {
            self.at = 0;
        }
    }

    #[inline]
    fn samples(&self) -> &[f32] {
        &self.buffer[self.at..self.at + self.len]
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.at = 0;
    }
}

/// One 2:1 stage: its filter, and its state going up and coming down.
#[derive(Debug, Clone)]
struct Stage {
    half: usize,
    /// The side taps, and twice them for the way up, where the zeros stuffed between the
    /// samples halve the level.
    taps: Box<[f32]>,
    doubled: Box<[f32]>,
    /// The way up: the last `2·half` input samples.
    rising: History,
    /// The way down: the last `2·half` first samples of each pair, and the last `half + 1`
    /// second ones.
    first: History,
    second: History,
}

impl Stage {
    fn new(half: usize) -> Self {
        let taps = side_taps(half);
        Self {
            half,
            taps: taps.iter().map(|&tap| tap as f32).collect(),
            doubled: taps.iter().map(|&tap| (2.0 * tap) as f32).collect(),
            rising: History::new(2 * half),
            first: History::new(2 * half),
            second: History::new(half + 1),
        }
    }

    /// Delay of a trip up and back down, in samples at the stage's lower rate.
    fn delay(&self) -> usize {
        2 * self.half - 1
    }

    /// One sample in at the lower rate, two out at the higher.
    #[inline]
    fn up(&mut self, sample: f32) -> (f32, f32) {
        self.rising.push(sample);
        let samples = self.rising.samples();
        (dot(&self.doubled, samples), samples[self.half])
    }

    /// Two samples in at the higher rate, one out at the lower.
    #[inline]
    fn down(&mut self, first: f32, second: f32) -> f32 {
        self.first.push(first);
        self.second.push(second);
        0.5 * self.second.samples()[0] + dot(&self.taps, self.first.samples())
    }

    fn clear(&mut self) {
        self.rising.clear();
        self.first.clear();
        self.second.clear();
    }
}

/// Takes a signal up by a power of two and back down, one stage at a time.
#[derive(Debug, Clone)]
pub(crate) struct Oversampler {
    /// The stage nearest the base rate first.
    stages: Vec<Stage>,
    scratch: Box<[f32]>,
}

impl Oversampler {
    /// An oversampler by `factor`, a power of two, for a signal at `rate` Hz.
    pub(crate) fn new(rate: f64, factor: usize) -> Self {
        let count = factor.max(1).trailing_zeros() as usize;
        Self {
            stages: (0..count)
                .map(|stage| Stage::new(half_for(rate, stage)))
                .collect(),
            scratch: vec![0.0; factor.max(1)].into_boxed_slice(),
        }
    }

    pub(crate) fn factor(&self) -> usize {
        1 << self.stages.len()
    }

    /// Delay of a trip up and back down, in samples at the top rate.
    pub(crate) fn delay(&self) -> usize {
        let count = self.stages.len();
        self.stages
            .iter()
            .enumerate()
            .map(|(index, stage)| stage.delay() << (count - index))
            .sum()
    }

    /// Takes one sample up to [`Oversampler::factor`] samples at the top rate, into `out`.
    #[inline]
    pub(crate) fn up(&mut self, sample: f32, out: &mut [f32]) {
        out[0] = sample;
        let mut length = 1;
        for stage in &mut self.stages {
            self.scratch[..length].copy_from_slice(&out[..length]);
            for (index, &sample) in self.scratch[..length].iter().enumerate() {
                let (first, second) = stage.up(sample);
                out[2 * index] = first;
                out[2 * index + 1] = second;
            }
            length *= 2;
        }
    }

    /// Takes [`Oversampler::factor`] samples at the top rate back down to one. `samples`
    /// is used as scratch.
    #[inline]
    pub(crate) fn down(&mut self, samples: &mut [f32]) -> f32 {
        let mut length = self.factor();
        for stage in self.stages.iter_mut().rev() {
            length /= 2;
            for index in 0..length {
                samples[index] = stage.down(samples[2 * index], samples[2 * index + 1]);
            }
        }
        samples[0]
    }

    pub(crate) fn clear(&mut self) {
        for stage in &mut self.stages {
            stage.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_as_long_as_the_band_needs() {
        let taps = |rate: f64, stage: usize| 4 * half_for(rate, stage) - 1;
        assert_eq!(taps(48_000.0, 0), 63);
        assert_eq!(taps(48_000.0, 1), 19);
        assert_eq!(taps(48_000.0, 2), 15);
        assert!(taps(44_100.0, 0) > taps(48_000.0, 0));
        assert!(taps(96_000.0, 0) < taps(48_000.0, 0));
        for half in [2, 5, 16, 30] {
            let side: f64 = side_taps(half).iter().sum();
            assert!((side - 0.5).abs() < 1.0e-12);
        }
    }

    #[test]
    fn a_round_trip_is_a_pure_delay_of_the_stated_length() {
        // A band-limited signal: three sines well inside the passband, which can be read
        // at any fractional time.
        let signal = |time: f64| {
            let tau = std::f64::consts::TAU;
            0.3 * (tau * 440.0 * time).sin()
                + 0.2 * (tau * 5_100.0 * time).sin()
                + 0.1 * (tau * 17_300.0 * time).sin()
        };
        for factor in [1, 2, 4, 8, 16] {
            let mut oversampler = Oversampler::new(48_000.0, factor);
            assert_eq!(oversampler.factor(), factor);
            let delay = oversampler.delay() as f64 / factor as f64;
            let mut top = vec![0.0; factor];
            for index in 0..4000 {
                let input = signal(index as f64 / 48_000.0) as f32;
                oversampler.up(input, &mut top);
                let output = oversampler.down(&mut top);
                if index >= 300 {
                    let expected = signal((index as f64 - delay) / 48_000.0);
                    assert!(
                        (f64::from(output) - expected).abs() < 2.0e-4,
                        "{factor}× at {index}: {output} vs {expected}"
                    );
                }
            }
        }
    }
}
