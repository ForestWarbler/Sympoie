//! The small filters around the curve: the emphasis shelves, the Tone tilt, the even
//! part's DC blocker, the shaper's treble compensation, and delays.

use std::f64::consts::{PI, SQRT_2, TAU};

/// Values that have decayed below this are let go to zero before they can turn
/// denormal.
const FLUSH: f64 = 1.0e-25;

fn flush(value: &mut f64) {
    if value.abs() < FLUSH {
        *value = 0.0;
    }
}

/// A biquad's coefficients, normalised so that `a0` is 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefficients {
    pub(crate) const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// A high shelf of `gain_db` above `frequency`, with a slope of 1 (the Audio EQ
    /// Cookbook's). Shelves of opposite gain at the same frequency undo each other
    /// exactly.
    pub(crate) fn high_shelf(rate: f64, frequency: f64, gain_db: f64) -> Self {
        let a = 10.0_f64.powf(gain_db / 40.0);
        let w0 = TAU * (frequency / rate).clamp(1.0e-5, 0.49);
        let (sin, cos) = w0.sin_cos();
        let root = 2.0 * a.sqrt() * sin / SQRT_2;
        let a0 = (a + 1.0) - (a - 1.0) * cos + root;
        Self {
            b0: a * ((a + 1.0) + (a - 1.0) * cos + root) / a0,
            b1: -2.0 * a * ((a - 1.0) + (a + 1.0) * cos) / a0,
            b2: a * ((a + 1.0) + (a - 1.0) * cos - root) / a0,
            a1: 2.0 * ((a - 1.0) - (a + 1.0) * cos) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cos - root) / a0,
        }
    }

    /// A first-order tilt through 0 dB at `pivot`: `gain_db` at the top of the band and
    /// its opposite at the bottom.
    pub(crate) fn tilt(rate: f64, pivot: f64, gain_db: f64) -> Self {
        let k = 10.0_f64.powf(gain_db / 20.0);
        let t = (PI * (pivot / rate).clamp(1.0e-5, 0.49)).tan();
        let a0 = 1.0 + k * t;
        Self {
            b0: (k + t) / a0,
            b1: (t - k) / a0,
            b2: 0.0,
            a1: (k * t - 1.0) / a0,
            a2: 0.0,
        }
    }

    /// A second-order section from the Audio EQ Cookbook: `kind` picks the response.
    fn cookbook(rate: f64, frequency: f64, q: f64, kind: Section) -> Self {
        let w0 = TAU * (frequency / rate).clamp(1.0e-5, 0.49);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Section::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            Section::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            Section::AllPass => (
                1.0 - alpha,
                -2.0 * cos,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            Section::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            Section::Peak(gain_db) => {
                let a = 10.0_f64.powf(gain_db / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cos,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos,
                    1.0 - alpha / a,
                )
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    pub(crate) fn lowpass(rate: f64, frequency: f64, q: f64) -> Self {
        Self::cookbook(rate, frequency, q, Section::LowPass)
    }

    pub(crate) fn highpass(rate: f64, frequency: f64, q: f64) -> Self {
        Self::cookbook(rate, frequency, q, Section::HighPass)
    }

    pub(crate) fn allpass(rate: f64, frequency: f64, q: f64) -> Self {
        Self::cookbook(rate, frequency, q, Section::AllPass)
    }

    /// A band-pass that peaks at 0 dB.
    pub(crate) fn bandpass(rate: f64, frequency: f64, q: f64) -> Self {
        Self::cookbook(rate, frequency, q, Section::BandPass)
    }

    pub(crate) fn peak(rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        Self::cookbook(rate, frequency, q, Section::Peak(gain_db))
    }

    /// First-order sections by the bilinear transform, warped to `frequency`.
    fn first_order(rate: f64, frequency: f64) -> (f64, f64) {
        let k = (PI * (frequency / rate).clamp(1.0e-5, 0.49)).tan();
        (k, (k - 1.0) / (k + 1.0))
    }

    pub(crate) fn lowpass1(rate: f64, frequency: f64) -> Self {
        let (k, a1) = Self::first_order(rate, frequency);
        Self {
            b0: k / (1.0 + k),
            b1: k / (1.0 + k),
            b2: 0.0,
            a1,
            a2: 0.0,
        }
    }

    pub(crate) fn highpass1(rate: f64, frequency: f64) -> Self {
        let (k, a1) = Self::first_order(rate, frequency);
        Self {
            b0: 1.0 / (1.0 + k),
            b1: -1.0 / (1.0 + k),
            b2: 0.0,
            a1,
            a2: 0.0,
        }
    }

    pub(crate) fn allpass1(rate: f64, frequency: f64) -> Self {
        let (_, a1) = Self::first_order(rate, frequency);
        Self {
            b0: a1,
            b1: 1.0,
            b2: 0.0,
            a1,
            a2: 0.0,
        }
    }

    /// The magnitude response at `hz`.
    #[cfg(test)]
    pub(crate) fn magnitude(&self, rate: f64, hz: f64) -> f64 {
        let w = TAU * hz / rate;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (
            self.b0 + self.b1 * z1.0 + self.b2 * z2.0,
            self.b1 * z1.1 + self.b2 * z2.1,
        );
        let den = (
            1.0 + self.a1 * z1.0 + self.a2 * z2.0,
            self.a1 * z1.1 + self.a2 * z2.1,
        );
        (num.0.hypot(num.1)) / (den.0.hypot(den.1))
    }
}

/// Which second-order response [`Coefficients::cookbook`] builds.
#[derive(Debug, Clone, Copy)]
enum Section {
    LowPass,
    HighPass,
    AllPass,
    BandPass,
    Peak(f64),
}

/// A biquad in transposed direct form II.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Biquad {
    coefficients: Coefficients,
    s1: f64,
    s2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            coefficients: Coefficients::IDENTITY,
            s1: 0.0,
            s2: 0.0,
        }
    }
}

impl Biquad {
    pub(crate) fn set(&mut self, coefficients: Coefficients) {
        self.coefficients = coefficients;
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f64) -> f64 {
        let c = &self.coefficients;
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }

    pub(crate) fn clear(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    pub(crate) fn flush(&mut self) {
        flush(&mut self.s1);
        flush(&mut self.s2);
    }
}

/// A one-pole high-pass that takes the offset off the even part.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DcBlocker {
    pole: f64,
    input: f64,
    output: f64,
}

impl DcBlocker {
    pub(crate) fn new(rate: f64, cutoff: f64) -> Self {
        Self {
            pole: (-TAU * cutoff / rate).exp(),
            input: 0.0,
            output: 0.0,
        }
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f64) -> f64 {
        let y = x - self.input + self.pole * self.output;
        self.input = x;
        self.output = y;
        y
    }

    pub(crate) fn clear(&mut self) {
        self.input = 0.0;
        self.output = 0.0;
    }

    pub(crate) fn flush(&mut self) {
        flush(&mut self.input);
        flush(&mut self.output);
    }
}

/// Follows the shaper. Averaging two samples, as the shaper does, takes a little treble
/// off (half a decibel at 20 kHz at 192 kHz) and delays by half a sample; this short
/// symmetric FIR, `(−1, 5, 5, −1)/8`, puts the treble back and adds a sample and a half,
/// so the two together delay by exactly two samples.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Compensation {
    history: [f64; 3],
}

impl Compensation {
    #[inline]
    pub(crate) fn process(&mut self, x: f64) -> f64 {
        let [one, two, three] = self.history;
        self.history = [x, one, two];
        0.625 * (one + two) - 0.125 * (x + three)
    }

    pub(crate) fn clear(&mut self) {
        self.history = [0.0; 3];
    }
}

/// A delay of a fixed number of samples.
#[derive(Debug, Clone)]
pub(crate) struct Delay {
    buffer: Box<[f32]>,
    at: usize,
}

impl Delay {
    pub(crate) fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length].into_boxed_slice(),
            at: 0,
        }
    }

    /// Takes a sample and hands back the one from the delay's length before.
    #[inline]
    pub(crate) fn push(&mut self, sample: f32) -> f32 {
        if self.buffer.is_empty() {
            return sample;
        }
        let out = std::mem::replace(&mut self.buffer[self.at], sample);
        self.at += 1;
        if self.at == self.buffer.len() {
            self.at = 0;
        }
        out
    }

    pub(crate) fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_shelves_cancel_and_the_tilt_pivots_at_unity() {
        let rate = 48_000.0;
        let up = Coefficients::high_shelf(rate, 2000.0, 9.0);
        let down = Coefficients::high_shelf(rate, 2000.0, -9.0);
        for hz in [30.0, 500.0, 2000.0, 9000.0, 20_000.0] {
            let product = up.magnitude(rate, hz) * down.magnitude(rate, hz);
            assert!((product - 1.0).abs() < 1.0e-9, "{hz}: {product}");
        }
        assert!((20.0 * up.magnitude(rate, 20_000.0).log10() - 9.0).abs() < 0.3);
        assert!(up.magnitude(rate, 30.0) < 1.01);
        let tilt = Coefficients::tilt(rate, 800.0, 6.0);
        assert!((tilt.magnitude(rate, 800.0) - 1.0).abs() < 1.0e-9);
        assert!(tilt.magnitude(rate, 20_000.0) > 1.6);
        assert!(tilt.magnitude(rate, 20.0) < 0.55);
        let flat = Coefficients::tilt(rate, 800.0, 0.0);
        let mut biquad = Biquad::default();
        biquad.set(flat);
        for x in [0.5, -0.25, 1.0, 0.0] {
            assert!((biquad.process(x) - x).abs() < 1.0e-12);
        }
    }

    #[test]
    fn the_compensation_restores_the_treble_the_average_takes() {
        // The average of two samples followed by the compensation, at 192 kHz.
        let rate: f64 = 192_000.0;
        for hz in [1000.0, 10_000.0, 20_000.0] {
            let w = TAU * hz / rate;
            let average = (w / 2.0).cos();
            let compensation = 1.25 * (w / 2.0).cos() - 0.25 * (1.5 * w).cos();
            let total = 20.0 * (average * compensation).log10();
            assert!(total.abs() < 0.1, "{hz}: {total} dB");
        }
    }
}
