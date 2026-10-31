//! The models a band can run besides the plain curve, and the feedback loop around it.
//!
//! - **Tube** leans its curve further as the band gets louder, the way a tube's operating
//!   point shifts under drive, so the even harmonics bloom on the loud notes and settle
//!   on the quiet ones.
//! - **Tape** follows the curve with a hysteresis loop (Bouc-Wen): the output takes a
//!   different path on the way up than on the way down, which softens the edges and adds
//!   the level-bound compression of tape, and a head bump lifts the low end.
//! - **Transformer** saturates the core's flux, the integral of the signal, and then
//!   differentiates it back, so the lows saturate first and the highs pass almost clean.
//! - **Fold** swaps the curve for a sine fold (see `curve`).
//!
//! All of them run at the curve's oversampled rate, and every one is transparent where the
//! curve is straight.

use std::f64::consts::TAU;
use std::sync::OnceLock;

use crate::curve::REFERENCE_LEVEL;

/// How hard Tape drives its hysteresis loop: the loop sees the curve's output this many
/// times over and its own output is scaled back by as much, so quiet signals still pass
/// at their level while ordinary ones already ride the loop.
pub(crate) const TAPE_DRIVE: f64 = 6.0;
/// How far Tube's operating point moves at full scale, in the curve's input units.
pub(crate) const BLOOM: f64 = 0.35;
/// Tape's head bump: a lift in the lows after the tape.
pub(crate) const BUMP_HZ: f64 = 90.0;
pub(crate) const BUMP_Q: f64 = 0.8;
pub(crate) const BUMP_DB: f64 = 2.0;
/// Below this the transformer's core passes the flux whole; above it, less and less.
const FLUX_CORNER: f64 = 150.0;
/// The feedback loop's gain at 100 %, and its ringing filter's Q. A little over unity, so
/// at the top of the range the band sings at the feedback frequency; the curve holds it.
pub(crate) const FEEDBACK_LOOP: f64 = 1.15;
pub(crate) const FEEDBACK_Q: f64 = 3.0;
/// Below this input level (about −50 dBFS) the feedback fades out, so a band stops ringing
/// once its input stops.
pub(crate) const FEEDBACK_GATE: f64 = 0.003;

/// Bouc-Wen hysteresis: `dz = du − β|du|z − γ du|z|`, which follows `u` closely for small
/// signals, saturates at `1/(β + γ)`, and swings back faster than it rose once the input
/// turns, which opens the loop.
const BETA: f64 = 0.5;
const GAMMA: f64 = 1.0 / 6.0;
const HYSTERESIS_LIMIT: f64 = 1.0 / (BETA + GAMMA);

/// The model a band runs: the index of its setting in `params::MODELS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Model {
    Curve,
    Tube,
    Tape,
    Transformer,
    Fold,
}

impl Model {
    pub(crate) fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Tube,
            2 => Self::Tape,
            3 => Self::Transformer,
            4 => Self::Fold,
            _ => Self::Curve,
        }
    }
}

/// What Auto gain multiplies Tape by: the loop's loss on the fundamental of a sine at the
/// level Auto gain matches the curve's output to. The loop depends on the size of its
/// input and not on its rate, so one sine, measured once, stands for all.
pub(crate) fn tape_makeup() -> f64 {
    static MAKEUP: OnceLock<f64> = OnceLock::new();
    *MAKEUP.get_or_init(|| {
        const POINTS: usize = 512;
        const CYCLES: usize = 6;
        let mut hysteresis = Hysteresis::default();
        let (mut re, mut im) = (0.0, 0.0);
        for index in 0..POINTS * CYCLES {
            let (sin, cos) = (TAU * index as f64 / POINTS as f64).sin_cos();
            let out = hysteresis.step(REFERENCE_LEVEL * TAPE_DRIVE * sin) / TAPE_DRIVE;
            if index >= POINTS * (CYCLES - 1) {
                re += out * cos;
                im += out * sin;
            }
        }
        REFERENCE_LEVEL / (2.0 * re.hypot(im) / POINTS as f64)
    })
}

/// Tape's hysteresis, one per channel and rate.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Hysteresis {
    previous: f64,
    z: f64,
}

impl Hysteresis {
    #[inline]
    pub(crate) fn step(&mut self, u: f64) -> f64 {
        let du = u - self.previous;
        self.previous = u;
        self.z += du - BETA * du.abs() * self.z - GAMMA * du * self.z.abs();
        self.z = self.z.clamp(-HYSTERESIS_LIMIT, HYSTERESIS_LIMIT);
        self.z
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The transformer's core: a one-pole integrator up to the flux, and its exact inverse back
/// down, so with nothing saturating between them the pair passes the signal unchanged.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Flux {
    pole: f64,
    flux: f64,
    previous: f64,
}

impl Flux {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            pole: (-std::f64::consts::TAU * FLUX_CORNER / rate).exp(),
            flux: 0.0,
            previous: 0.0,
        }
    }

    #[inline]
    pub(crate) fn integrate(&mut self, x: f64) -> f64 {
        self.flux = (1.0 - self.pole) * x + self.pole * self.flux;
        self.flux
    }

    #[inline]
    pub(crate) fn differentiate(&mut self, y: f64) -> f64 {
        let x = (y - self.pole * self.previous) / (1.0 - self.pole);
        self.previous = y;
        x
    }

    pub(crate) fn clear(&mut self) {
        self.flux = 0.0;
        self.previous = 0.0;
    }

    pub(crate) fn flush(&mut self) {
        if self.flux.abs() < 1.0e-25 {
            self.flux = 0.0;
        }
        if self.previous.abs() < 1.0e-25 {
            self.previous = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flux_pair_passes_the_signal_unchanged() {
        let mut flux = Flux::new(192_000.0);
        for i in 0..2000 {
            let x = (i as f64 * 0.013).sin() + 0.3 * (i as f64 * 0.41).cos();
            let integral = flux.integrate(x);
            let back = flux.differentiate(integral);
            assert!((back - x).abs() < 1.0e-9, "{i}: {back} vs {x}");
        }
    }

    #[test]
    fn the_hysteresis_follows_small_signals_saturates_and_opens_a_loop() {
        let mut loop_ = Hysteresis::default();
        // Small: it follows.
        for i in 0..1000 {
            let u = 0.01 * (i as f64 * 0.01).sin();
            let z = loop_.step(u);
            assert!((z - u).abs() < 2.0e-3, "{i}: {z} vs {u}");
        }
        // Large: it saturates below its limit, and passes zero input at different outputs
        // on the way up and on the way down.
        let mut loop_ = Hysteresis::default();
        let (mut rising, mut falling) = (None, None);
        for i in 0..4000 {
            let u = 3.0 * (std::f64::consts::TAU * i as f64 / 1000.0).sin();
            let z = loop_.step(u);
            assert!(z.abs() <= HYSTERESIS_LIMIT + 1.0e-12);
            if i == 3000 {
                rising = Some(z); // u = 0, rising
            }
            if i == 3500 {
                falling = Some(z); // u = 0, falling
            }
        }
        let (rising, falling) = (rising.unwrap(), falling.unwrap());
        assert!((rising - falling).abs() > 0.5, "{rising} {falling}");
        assert!((rising + falling).abs() < 1.0e-3, "the loop is symmetric");
    }
}
