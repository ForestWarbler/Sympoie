//! The curve at Calor's heart, and how its odd and even halves are pulled apart.
//!
//! The curve is linear up to a knee and then bends, as an algebraic sigmoid, into a
//! ceiling at 1. Hardness moves the knee from 0 (the sigmoid all the way, soft from the
//! first decibel) towards 1 (all but a hard clip). Drive scales the signal into the curve,
//! and Bias moves its operating point off centre the way a tube's grid bias does, so the
//! two half-waves bend differently:
//!
//! `f(x) = clip(g·x + b) − clip(b)`
//!
//! Any curve splits exactly into an odd part, `(f(x) − f(−x)) / 2`, which can only make
//! the fundamental and odd harmonics, and an even part, `(f(x) + f(−x)) / 2`, which can
//! only make even harmonics and DC. Calor sets the two apart: Odd blends the odd part
//! with the clean signal, Even scales the even part, and at 100 % each the result is the
//! curve itself (see [`Transfer`]).
//!
//! Both parts have closed-form antiderivatives, which the processor's shaper uses to keep
//! the harmonics the curve makes above the top of its oversampled band from folding back
//! as aliases.

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::sync::OnceLock;

use crate::params::{AUTO_GAIN, COUNT, MODELS, Slot, amplitude, band_parameter};

/// The knee Hardness reaches at 100 %: the last 3 % of the range bends into the ceiling.
const KNEE_MAX: f64 = 0.97;
/// The operating point Bias sets at 0 % and at 100 %, in the curve's input units. Even at
/// 0 % the curve leans a little, so Even always has something to scale.
const BIAS_MIN: f64 = 0.04;
const BIAS_MAX: f64 = 0.8;
/// Level at which Auto gain matches the curve to the clean signal: the peak of a sine at
/// −12 dBFS.
pub const REFERENCE_LEVEL: f64 = 0.251_188_643_150_958;
/// Harmonics [`Transfer::harmonics`] measures, the fundamental included.
pub const HARMONICS: usize = 16;

/// The bend: linear up to `knee`, then `knee + (1 − knee)·σ(z)`, where `σ(z) = z/√(1 + z²)`
/// and `z` is how far past the knee the input is, in units of the room left above it.
pub(crate) fn clip(u: f64, knee: f64) -> f64 {
    let magnitude = u.abs();
    if magnitude <= knee {
        return u;
    }
    let room = 1.0 - knee;
    let z = (magnitude - knee) / room;
    (knee + room * z / (1.0 + z * z).sqrt()).copysign(u)
}

/// The antiderivative of [`clip`], zero at zero.
pub(crate) fn clip_integral(u: f64, knee: f64) -> f64 {
    let magnitude = u.abs();
    if magnitude <= knee {
        return 0.5 * u * u;
    }
    let room = 1.0 - knee;
    let z = (magnitude - knee) / room;
    // `√(1 + z²) − 1`, written so that a small `z` does not cancel.
    let rise = z * z / ((1.0 + z * z).sqrt() + 1.0);
    0.5 * knee * knee + knee * (magnitude - knee) + room * room * rise
}

/// The fold: `sin(u)`, which leaves quiet signals as they are like the bend does, rises to
/// a ceiling of 1 at `u = π/2`, and then folds back down instead of flattening, so driving
/// it harder keeps adding harmonics.
fn fold(u: f64) -> f64 {
    u.sin()
}

/// The antiderivative of [`fold`], zero at zero.
fn fold_integral(u: f64) -> f64 {
    // `1 − cos u = 2 sin²(u/2)`, written so that a small `u` does not cancel.
    let half = (0.5 * u).sin();
    2.0 * half * half
}

/// The knee a Hardness setting, in %, puts the bend at.
pub(crate) fn knee(hardness: f64) -> f64 {
    KNEE_MAX * (hardness / 100.0).clamp(0.0, 1.0)
}

/// The operating point a Bias setting, in %, moves the curve to.
pub(crate) fn bias_point(bias: f64) -> f64 {
    BIAS_MIN + (BIAS_MAX - BIAS_MIN) * (bias / 100.0).clamp(0.0, 1.0)
}

/// The curve for one setting of Drive, Hardness and Bias: the bend, or for the Fold model
/// the fold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    gain: f64,
    bias: f64,
    knee: f64,
    folds: bool,
    /// The output at rest, which the curve subtracts so it passes through zero.
    rest: f64,
}

impl Curve {
    /// The curve for Drive in dB and Hardness and Bias in %.
    pub fn new(drive_db: f64, hardness: f64, bias: f64) -> Self {
        Self::raw(amplitude(drive_db), bias_point(bias), knee(hardness))
    }

    /// The first band's curve for a set of parameter values in physical units, indexed by
    /// id.
    pub fn from_values(values: &[f64; COUNT]) -> Self {
        Self::for_band(values, 0)
    }

    /// The curve of band `band` (from 0) for a set of parameter values in physical units.
    pub fn for_band(values: &[f64; COUNT], band: usize) -> Self {
        let value = |slot: Slot| values[band_parameter(band, slot) as usize];
        let curve = Self::new(value(Slot::Drive), value(Slot::Hardness), value(Slot::Bias));
        if MODELS[(value(Slot::Model).round() as usize).min(MODELS.len() - 1)] == "Fold" {
            curve.folding()
        } else {
            curve
        }
    }

    /// The curve for a linear drive gain, an operating point and a knee.
    pub(crate) fn raw(gain: f64, bias: f64, knee: f64) -> Self {
        Self {
            gain: gain.max(1.0e-6),
            bias,
            knee,
            folds: false,
            rest: clip(bias, knee),
        }
    }

    /// The same drive and operating point on the fold.
    pub(crate) fn folding(self) -> Self {
        Self {
            folds: true,
            rest: fold(self.bias),
            ..self
        }
    }

    /// Whether this is the fold.
    pub fn folds(&self) -> bool {
        self.folds
    }

    #[inline]
    fn bend(&self, u: f64) -> f64 {
        if self.folds {
            fold(u)
        } else {
            clip(u, self.knee)
        }
    }

    #[inline]
    fn bend_integral(&self, u: f64) -> f64 {
        if self.folds {
            fold_integral(u)
        } else {
            clip_integral(u, self.knee)
        }
    }

    /// Drive as a linear gain.
    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// The operating point Bias sets, in the curve's input units.
    pub fn bias(&self) -> f64 {
        self.bias
    }

    /// Where the bend begins, in the curve's input units.
    pub fn knee(&self) -> f64 {
        self.knee
    }

    /// The curve itself: zero at zero, with a ceiling of `1 − clip(bias)` above and
    /// `−1 − clip(bias)` below.
    pub fn shape(&self, x: f64) -> f64 {
        self.bend(self.gain * x + self.bias) - self.rest
    }

    /// The odd and even parts at `x`; they add up to [`Curve::shape`].
    pub fn parts(&self, x: f64) -> (f64, f64) {
        let rising = self.bend(self.gain * x + self.bias);
        let falling = self.bend(self.bias - self.gain * x);
        (
            0.5 * (rising - falling),
            0.5 * (rising + falling) - self.rest,
        )
    }

    /// Antiderivatives of the odd and even parts at `x`.
    pub(crate) fn integrals(&self, x: f64) -> (f64, f64) {
        let rising = self.bend_integral(self.gain * x + self.bias);
        let falling = self.bend_integral(self.bias - self.gain * x);
        (
            0.5 * (rising + falling) / self.gain,
            0.5 * (rising - falling) / self.gain - self.rest * x,
        )
    }

    /// The odd part's gain on the fundamental of a sine peaking at `amplitude`: its first
    /// Fourier coefficient over the amplitude, by Simpson's rule over a quarter period,
    /// which the odd part's symmetries repeat for the rest.
    pub fn fundamental_gain(&self, amplitude: f64) -> f64 {
        const STEPS: usize = 64;
        let amplitude = amplitude.max(1.0e-9);
        let step = FRAC_PI_2 / STEPS as f64;
        let mut sum = 0.0;
        for index in 0..=STEPS {
            let sin = (index as f64 * step).sin();
            let weight = match index {
                0 | STEPS => 1.0,
                index if index % 2 == 1 => 4.0,
                _ => 2.0,
            };
            sum += weight * self.parts(amplitude * sin).0 * sin;
        }
        4.0 / PI * (sum * step / 3.0) / amplitude
    }

    /// The gain Auto gain divides by: the odd part's at [`REFERENCE_LEVEL`].
    pub fn reference_gain(&self) -> f64 {
        self.fundamental_gain(REFERENCE_LEVEL).max(1.0e-6)
    }
}

/// What Calor's wet path does to a steady signal, leaving out its filters. Odd blends the
/// curve's odd part with the clean signal, Even scales its even part, and both parts are
/// divided by the odd part's gain at the reference level, so that the blend is level with
/// the clean signal there: with Auto gain on, a sine at −12 dBFS keeps its fundamental at
/// −12 dBFS whatever Drive does. With Auto gain off, that gain is put back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transfer {
    pub curve: Curve,
    /// Odd and Even as factors: 0–1 and 0–3.
    pub odd: f64,
    pub even: f64,
    /// The odd part's gain at the reference level.
    pub reference: f64,
    pub auto_gain: bool,
}

impl Transfer {
    /// The first band's transfer for a set of parameter values in physical units, indexed
    /// by id.
    pub fn from_values(values: &[f64; COUNT]) -> Self {
        Self::for_band(values, 0)
    }

    /// The transfer of band `band` (from 0): its curve, or its fold, as the steady part of
    /// whichever model it runs.
    pub fn for_band(values: &[f64; COUNT], band: usize) -> Self {
        let value = |slot: Slot| values[band_parameter(band, slot) as usize];
        let curve = Curve::for_band(values, band);
        Self {
            curve,
            odd: (value(Slot::Odd) / 100.0).clamp(0.0, 1.0),
            even: (value(Slot::Even) / 100.0).max(0.0),
            reference: curve.reference_gain(),
            auto_gain: values[AUTO_GAIN as usize] >= 0.5,
        }
    }

    /// The output for a steady input `x`, with the even part's offset, which the
    /// processor blocks, left in.
    pub fn apply(&self, x: f64) -> f64 {
        let (odd, even) = self.curve.parts(x);
        let blend = (1.0 - self.odd) * x + (self.odd * odd + self.even * even) / self.reference;
        if self.auto_gain {
            blend
        } else {
            blend * self.reference
        }
    }

    /// Amplitudes of the first [`HARMONICS`] harmonics of a sine peaking at `amplitude`
    /// after the transfer, the fundamental first.
    pub fn harmonics(&self, amplitude: f64) -> [f64; HARMONICS] {
        let table = cycle();
        let samples: Vec<f64> = table
            .iter()
            .map(|&(sin, _)| self.apply(amplitude * sin))
            .collect();
        std::array::from_fn(|harmonic| {
            let n = harmonic + 1;
            let (mut re, mut im) = (0.0, 0.0);
            for (index, &sample) in samples.iter().enumerate() {
                let (sin, cos) = table[(n * index) % CYCLE];
                re += sample * cos;
                im += sample * sin;
            }
            2.0 * (re * re + im * im).sqrt() / CYCLE as f64
        })
    }
}

/// Points per cycle [`Transfer::harmonics`] samples the transfer at.
const CYCLE: usize = 512;

/// The sine and cosine at each of [`CYCLE`] points around a cycle.
fn cycle() -> &'static [(f64, f64); CYCLE] {
    static TABLE: OnceLock<[(f64, f64); CYCLE]> = OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|index| (TAU * index as f64 / CYCLE as f64).sin_cos()))
}
