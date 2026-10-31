//! One band of one channel: its emphasis, its trip up to the curve's rate and back, the
//! model and the feedback loop at that rate, and its own dry signal for the band's Mix.
//!
//! With the plain curve and no feedback, a band does exactly what Calor's single band did
//! before there were bands, operation for operation, so sets saved then sound the same.

use crate::curve::Curve;
use crate::filters::{Biquad, Coefficients, Compensation, DcBlocker, Delay};
use crate::models::{BUMP_DB, BUMP_HZ, BUMP_Q, FEEDBACK_Q, Flux, Hysteresis, Model, TAPE_DRIVE};
use crate::oversampling::Oversampler;

/// Samples, at the curve's rate, by which the shaper and its compensation delay the
/// signal.
pub(crate) const SHAPER_DELAY: usize = 2;
/// Where the even part's offset is blocked, in Hz.
const DC_CUTOFF: f64 = 5.0;
/// Two successive samples closer than this are treated as one: the shaper evaluates the
/// curve between them instead of dividing by their difference.
const ADAA_EPSILON: f64 = 1.0e-7;

/// The curve with antiderivative anti-aliasing: each output is the curve's mean between
/// the last input and this one, `(F(x) − F(x₋₁)) / (x − x₋₁)` for its antiderivative `F`,
/// which takes the steepest harmonics, the ones that would alias, down sharply. The odd
/// and even parts each get their own, and the clean signal gets the same averaging, so
/// all three stay half a sample behind together.
#[derive(Debug, Clone)]
struct Shaper {
    previous: f64,
    /// The antiderivatives at `previous`, for `curve`.
    integrals: (f64, f64),
    curve: Option<Curve>,
}

impl Shaper {
    fn new() -> Self {
        Self {
            previous: 0.0,
            integrals: (0.0, 0.0),
            curve: None,
        }
    }

    /// The clean signal, odd part and even part for input `x`.
    #[inline]
    fn step(&mut self, x: f64, curve: &Curve) -> (f64, f64, f64) {
        if self.curve.as_ref() != Some(curve) {
            // The curve moved: measure the last input against the new one.
            self.integrals = curve.integrals(self.previous);
            self.curve = Some(*curve);
        }
        let integrals = curve.integrals(x);
        let difference = x - self.previous;
        let (odd, even) = if difference.abs() > ADAA_EPSILON {
            (
                (integrals.0 - self.integrals.0) / difference,
                (integrals.1 - self.integrals.1) / difference,
            )
        } else {
            curve.parts(0.5 * (x + self.previous))
        };
        let clean = 0.5 * (x + self.previous);
        self.previous = x;
        self.integrals = integrals;
        (clean, odd, even)
    }
}

/// The feedback loop's memory: the last output, rung through a band-pass at the feedback
/// frequency before it goes back in.
#[derive(Debug, Clone, Copy, Default)]
struct Feedback {
    resonator: Biquad,
    last: f64,
}

/// One band of one channel at one quality: the trip up and back, and everything at the
/// curve's rate.
#[derive(Debug, Clone)]
struct Path {
    rate: f64,
    oversampler: Oversampler,
    /// The samples at the curve's rate.
    top: Box<[f32]>,
    shaper: Shaper,
    dc: DcBlocker,
    compensation: Compensation,
    /// Pads the path's delay up to Calor's latency.
    pad: Delay,
    feedback: Feedback,
    hysteresis: Hysteresis,
    flux: Flux,
}

impl Path {
    fn new(rate: f64, oversampler: Oversampler, latency: usize) -> Self {
        let factor = oversampler.factor();
        let natural = oversampler.delay() + SHAPER_DELAY;
        let top_rate = rate * factor as f64;
        Self {
            rate: top_rate,
            top: vec![0.0; factor].into_boxed_slice(),
            shaper: Shaper::new(),
            dc: DcBlocker::new(top_rate, DC_CUTOFF),
            compensation: Compensation::default(),
            pad: Delay::new(latency * factor - natural),
            feedback: Feedback::default(),
            hysteresis: Hysteresis::default(),
            flux: Flux::new(top_rate),
            oversampler,
        }
    }

    fn clear(&mut self) {
        self.oversampler.clear();
        self.shaper = Shaper::new();
        self.dc.clear();
        self.compensation.clear();
        self.pad.clear();
        self.feedback.resonator.clear();
        self.feedback.last = 0.0;
        self.hysteresis.clear();
        self.flux.clear();
    }
}

/// What a band processes one sample with, shared by its channels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BandFrame {
    pub(crate) curve: Curve,
    pub(crate) odd: f64,
    pub(crate) even: f64,
    /// One over the reference gain.
    pub(crate) inverse: f64,
    /// The reference gain with Auto gain off, 1 with it on.
    pub(crate) makeup: f64,
    pub(crate) model: Model,
    /// The feedback loop's gain, 0 with no feedback.
    pub(crate) feedback: f64,
}

/// One band of one channel.
#[derive(Debug, Clone)]
pub(crate) struct Lane {
    emphasis: Biquad,
    deemphasis: Biquad,
    /// Tape's head bump.
    bump: Biquad,
    /// The band's input, delayed by Calor's latency, for the band's Mix.
    dry: Delay,
    /// The real-time path, then the offline one.
    paths: [Path; 2],
}

impl Lane {
    pub(crate) fn new(rate: f64, oversamplers: &[Oversampler; 2], latency: usize) -> Self {
        let mut bump = Biquad::default();
        bump.set(Coefficients::peak(rate, BUMP_HZ, BUMP_Q, BUMP_DB));
        Self {
            emphasis: Biquad::default(),
            deemphasis: Biquad::default(),
            bump,
            dry: Delay::new(latency),
            paths: oversamplers
                .clone()
                .map(|oversampler| Path::new(rate, oversampler, latency)),
        }
    }

    pub(crate) fn set_emphasis(&mut self, emphasis: Coefficients, deemphasis: Coefficients) {
        self.emphasis.set(emphasis);
        self.deemphasis.set(deemphasis);
    }

    /// Tunes the feedback's ringing to `frequency` on both paths.
    pub(crate) fn set_feedback_frequency(&mut self, frequency: f64) {
        for path in &mut self.paths {
            let coefficients = Coefficients::bandpass(path.rate, frequency, FEEDBACK_Q);
            path.feedback.resonator.set(coefficients);
        }
    }

    /// Takes one sample of the band and hands back its wet signal (before dynamics, the
    /// band's Mix and its level) and its own dry signal, both a latency behind.
    #[inline]
    pub(crate) fn step(&mut self, input: f64, path: usize, frame: &BandFrame) -> (f64, f64) {
        let dry = f64::from(self.dry.push(input as f32));
        let emphasised = self.emphasis.process(input) as f32;
        let Path {
            oversampler,
            top,
            shaper,
            dc,
            compensation,
            pad,
            feedback,
            hysteresis,
            flux,
            ..
        } = &mut self.paths[path];
        oversampler.up(emphasised, top);
        for sample in top.iter_mut() {
            let mut x = f64::from(*sample);
            if frame.feedback != 0.0 {
                x += frame.feedback * feedback.resonator.process(feedback.last);
            }
            if frame.model == Model::Transformer {
                x = flux.integrate(x);
            }
            let (clean, odd, even) = shaper.step(x, &frame.curve);
            let even = dc.process(frame.even * even);
            let mut blend = (1.0 - frame.odd) * clean + (frame.odd * odd + even) * frame.inverse;
            match frame.model {
                Model::Tape => blend = hysteresis.step(blend * TAPE_DRIVE) / TAPE_DRIVE,
                Model::Transformer => blend = flux.differentiate(blend),
                _ => {}
            }
            feedback.last = blend;
            *sample = pad.push(compensation.process(blend) as f32);
        }
        let wet = f64::from(oversampler.down(top));
        let wet = self.deemphasis.process(wet) * frame.makeup;
        let wet = if frame.model == Model::Tape {
            self.bump.process(wet)
        } else {
            wet
        };
        (wet, dry)
    }

    /// Lets the band's input through untouched, a latency behind, for the channel of a
    /// band that works on the other one (Mid or Side).
    #[inline]
    pub(crate) fn pass(&mut self, input: f64) -> f64 {
        f64::from(self.dry.push(input as f32))
    }

    /// Clears one path, for a switch of quality.
    pub(crate) fn clear_path(&mut self, path: usize) {
        self.paths[path].clear();
    }

    pub(crate) fn clear(&mut self) {
        self.emphasis.clear();
        self.deemphasis.clear();
        self.bump.clear();
        self.dry.clear();
        for path in &mut self.paths {
            path.clear();
        }
    }

    pub(crate) fn flush(&mut self) {
        self.emphasis.flush();
        self.deemphasis.flush();
        self.bump.flush();
        for path in &mut self.paths {
            path.dc.flush();
            path.feedback.resonator.flush();
            path.flux.flush();
        }
    }
}
