//! Building blocks of the Ferro voice: antialiased oscillators, the IR3109 OTA filter, RC contour generators, oversampling decimators, drift, and the BBD chorus.

use std::f64::consts::{PI, TAU};

/// Deterministic xorshift generator; every reset reseeds it so renders repeat.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [-1, 1).
    pub(super) fn bipolar(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

// ---------------------------------------------------------------------------------------
// Oscillators
// ---------------------------------------------------------------------------------------

fn blep(phase: f64, step: f64) -> f64 {
    if phase < step {
        let t = phase / step;
        2.0 * t - t * t - 1.0
    } else if phase > 1.0 - step {
        let t = (phase - 1.0) / step;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

fn blamp(phase: f64, step: f64) -> f64 {
    if phase < step {
        (1.0 - phase / step).powi(3) / 3.0
    } else if phase > 1.0 - step {
        ((phase - 1.0) / step + 1.0).powi(3) / 3.0
    } else {
        0.0
    }
}

pub const WAVES: [&str; 6] = [
    "Triangle",
    "Shark",
    "Saw",
    "Square",
    "Wide pulse",
    "Narrow pulse",
];

fn triangle(phase: f64, step: f64) -> f64 {
    1.0 - 4.0 * (phase - 0.5).abs()
        + 4.0 * step * (blamp(phase, step) - blamp((phase + 0.5).fract(), step))
}

fn saw(phase: f64, step: f64) -> f64 {
    2.0 * phase - 1.0 - blep(phase, step)
}

fn pulse(phase: f64, step: f64, width: f64) -> f64 {
    let width = width.clamp(step.max(0.02), 1.0 - step.max(0.02));
    let raw = if phase < width { 1.0 } else { -1.0 };
    raw + blep(phase, step) - blep((phase - width).rem_euclid(1.0), step) - (2.0 * width - 1.0)
}

/// One free-running oscillator, modelled on the Juno's DCO: an integrator ramps a
/// capacitor and a switch resets it once per cycle, so the saw is a clean linear ramp and
/// the pulses come from a comparator watching that ramp. Like the hardware it never
/// resets its phase on a new note, so chords and repeated notes beat naturally against
/// each other.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Vco {
    pub(super) phase: f64,
}

impl Vco {
    /// Advances one sample at `step` cycles per sample and returns the wave. `width_mod`
    /// shifts the comparator threshold, and so the pulse width (−0.5..0.5).
    pub(super) fn tick(&mut self, wave: u32, step: f64, width_mod: f64) -> f64 {
        let step = step.clamp(0.0, 0.49);
        let phase = self.phase;
        let value = match wave {
            0 => triangle(phase, step),
            1 => {
                // The Minimoog's "shark tooth": a triangle whose rise bows like a saw.
                0.6 * triangle(phase, step) + 0.4 * saw(phase, step)
            }
            2 => saw(phase, step),
            3 => pulse(phase, step, 0.5 + width_mod),
            4 => pulse(phase, step, 0.3 + width_mod),
            _ => pulse(phase, step, 0.1 + width_mod),
        };
        self.phase += step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        value
    }
}

/// Corner of the coupling capacitor between each oscillator and the mixer. Being AC
/// coupled, a square's flat tops sag toward zero and a saw's ramp bows slightly, most
/// visibly on low notes.
pub(super) const COUPLING_HZ: f64 = 5.0;

/// The DCO's amplitude error at a pitch. The Juno scales the integrator's charge current
/// with the note so every pitch reaches the same height, but the compensating DAC is
/// coarse, so the level wobbles by a few percent from note to note. This is a smooth
/// value noise over pitch (one random point per semitone), so glides move through it
/// without stepping. Returns −1..1.
pub(super) fn compensation_error(seed: u64, semitones: f64) -> f64 {
    let point = |n: i64| {
        let mut rng = Rng::new(seed ^ (n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        rng.next_u64();
        rng.bipolar()
    };
    let base = semitones.floor();
    let t = semitones - base;
    let t = t * t * (3.0 - 2.0 * t);
    let a = point(base as i64);
    a + (point(base as i64 + 1) - a) * t
}

/// Juno-style sub-oscillator: a square one octave below, phase-locked to its parent.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Sub {
    phase: f64,
}

impl Sub {
    pub(super) fn tick(&mut self, parent_step: f64) -> f64 {
        let step = (parent_step * 0.5).clamp(0.0, 0.49);
        let value = pulse(self.phase, step, 0.5);
        self.phase += step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        value
    }
}

/// Paul Kellet's economy pink filter over white noise.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Pink {
    b: [f64; 3],
}

impl Pink {
    pub(super) fn tick(&mut self, white: f64) -> f64 {
        self.b[0] = 0.99765 * self.b[0] + white * 0.0990460;
        self.b[1] = 0.96300 * self.b[1] + white * 0.2965164;
        self.b[2] = 0.57000 * self.b[2] + white * 1.0526913;
        (self.b[0] + self.b[1] + self.b[2] + white * 0.1848) * 0.25
    }
}

// ---------------------------------------------------------------------------------------
// IR3109 OTA filter
// ---------------------------------------------------------------------------------------

/// tanh(x)/x, the secant gain of a tanh stage, as a Padé approximation (good to a few
/// parts in a thousand where it matters and cheap enough for realtime use).
fn tanh_xdx_fast(x: f64) -> f64 {
    let x = x.clamp(-4.0, 4.0);
    let x2 = x * x;
    ((x2 + 105.0) * x2 + 945.0) / ((15.0 * x2 + 420.0) * x2 + 945.0)
}

fn tanh_xdx_exact(x: f64) -> f64 {
    if x.abs() < 1.0e-6 { 1.0 } else { x.tanh() / x }
}

/// The Juno's IR3109: four OTA integrators in a cascade with resonance fed back from the
/// last stage to the first, in zero-delay-feedback form.
///
/// Each stage is an OTA charging a capacitor, `y' = ωc·tanh(u − y)`: its differential pair
/// sees the difference between its input and its own output, unlike a transistor ladder
/// where every signal is shaped on its own. Ordinary levels pass almost cleanly, loud
/// transients slew into a rounded overdrive, and the resonance swells into a smooth,
/// sine-like self-oscillation.
///
/// Each tanh is replaced by its secant gain at an estimate of its input, which turns the
/// loop into a linear system that is solved exactly per sample. Realtime mode takes the
/// estimate from the filter's state (one solve, as in Mystran's cheap nonlinear ZDF).
/// Offline mode then re-estimates from the solution and solves again until the outputs
/// settle, so the nonlinear loop is resolved rather than approximated.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Ir3109 {
    s: [f64; 4],
    last_input: f64,
}

/// Linear range of each OTA stage, in the voice's signal units.
const STAGE_HEADROOM: f64 = 2.0;
/// Level at which the resonance feedback saturates.
const FEEDBACK_LIMIT: f64 = 2.0;

pub(super) struct FilterSolve {
    pub(super) iterations: usize,
    pub(super) tolerance: f64,
    pub(super) exact: bool,
}

impl Ir3109 {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// `g` is the prewarped integrator gain tan(π·fc/fs); `k` is the feedback, where 4
    /// is the edge of self-oscillation.
    pub(super) fn tick(&mut self, input: f64, g: f64, k: f64, solve: &FilterSolve) -> f64 {
        let t = if solve.exact {
            tanh_xdx_exact
        } else {
            tanh_xdx_fast
        };
        // Averaging with the previous input is a half-sample delay that keeps resonance
        // tuning and amount steady across the cutoff range.
        let x = 0.5 * (input + self.last_input);
        self.last_input = input;
        let s = self.s;
        // A trapezoidal OTA stage with secant gain t is a linear one-pole with gain g·t:
        // y = G·u + S, where G = g·t / (1 + g·t) and S = s / (1 + g·t).
        let mut y = s;
        for pass in 0..solve.iterations.max(1) {
            // The resonance returns through its own limiting gain stage, which caps how
            // hard the feedback can push the first OTA and keeps self-oscillation in tune.
            let kf = k * t(k * y[3] / FEEDBACK_LIMIT);
            let u0 = x - kf * y[3];
            let differences = [u0 - y[0], y[0] - y[1], y[1] - y[2], y[2] - y[3]];
            let mut gains = [0.0; 4];
            let mut offsets = [0.0; 4];
            for i in 0..4 {
                let gt = g * t(differences[i] / STAGE_HEADROOM);
                gains[i] = gt / (1.0 + gt);
                offsets[i] = s[i] / (1.0 + gt);
            }
            let [g0, g1, g2, g3] = gains;
            let [o0, o1, o2, o3] = offsets;
            let through = g0 * g1 * g2 * g3;
            let rest = g3 * (g2 * (g1 * o0 + o1) + o2) + o3;
            let y3 = (through * x + rest) / (1.0 + kf * through);
            let y0 = g0 * (x - kf * y3) + o0;
            let y1 = g1 * y0 + o1;
            let y2 = g2 * y1 + o2;
            let next = [y0, y1, y2, y3];
            let settled = pass > 0
                && next
                    .iter()
                    .zip(&y)
                    .all(|(a, b)| (a - b).abs() <= solve.tolerance);
            y = next;
            if settled {
                break;
            }
        }
        for (state, output) in self.s.iter_mut().zip(y) {
            *state = 2.0 * output - *state;
            // Flush denormals, and recover from anything non-finite.
            if !state.is_finite() || state.abs() < 1.0e-20 {
                *state = 0.0;
            }
        }
        y[3]
    }
}

/// Prewarped integrator gain for a cutoff at `rate`.
pub(super) fn filter_gain(cutoff: f64, rate: f64) -> f64 {
    (PI * cutoff.clamp(5.0, rate * 0.45) / rate).tan()
}

// ---------------------------------------------------------------------------------------
// Contour generators
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

/// An RC contour like the hardware's: the attack charges toward a target above full
/// scale and stops at 1, decay and release fall exponentially. Retriggering starts from
/// wherever the level is, so fast repeats never click back to zero.
#[derive(Debug, Clone, Copy)]
pub(super) struct Contour {
    pub(super) level: f64,
    pub(super) stage: Stage,
}

impl Default for Contour {
    fn default() -> Self {
        Self {
            level: 0.0,
            stage: Stage::Idle,
        }
    }
}

const ATTACK_TARGET: f64 = 1.3;
/// Decay and release times are the time to fall by 60 dB.
const LN_1000: f64 = 6.907_755_278_982_137;

/// Per-sample coefficient for a one-pole with time constant `seconds / divisor`.
fn coefficient(seconds: f64, divisor: f64, rate: f64) -> f64 {
    let tau = (seconds / divisor).max(1.0e-5);
    1.0 - (-1.0 / (tau * rate)).exp()
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ContourRates {
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
}

impl ContourRates {
    pub(super) fn new(attack: f64, decay: f64, sustain: f64, release: f64, rate: f64) -> Self {
        // Time for the attack curve to reach 1 when charging toward ATTACK_TARGET.
        let attack_divisor = -(1.0 - 1.0 / ATTACK_TARGET).ln();
        Self {
            attack: coefficient(attack, attack_divisor, rate),
            decay: coefficient(decay, LN_1000, rate),
            sustain: sustain.clamp(0.0, 1.0),
            release: coefficient(release, LN_1000, rate),
        }
    }
}

impl Contour {
    pub(super) fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub(super) fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    pub(super) fn tick(&mut self, rates: &ContourRates) -> f64 {
        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                self.level += (ATTACK_TARGET - self.level) * rates.attack;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => self.level += (rates.sustain - self.level) * rates.decay,
            Stage::Release => {
                self.level -= self.level * rates.release;
                if self.level < 1.0e-5 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

// ---------------------------------------------------------------------------------------
// Glide
// ---------------------------------------------------------------------------------------

/// Pitch in semitones gliding toward a target. `curve` 0 is the RC lag of the hardware's
/// portamento circuit (fast then settling); 1 is a constant-time linear slide. Both run
/// per sample, so slides are smooth at any rate.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Glide {
    pub(super) pitch: f64,
    target: f64,
    start: f64,
    elapsed: f64,
    duration: f64,
    curve: f64,
}

impl Glide {
    pub(super) fn jump(&mut self, pitch: f64) {
        *self = Self {
            pitch,
            target: pitch,
            start: pitch,
            ..Self::default()
        };
    }

    pub(super) fn to(&mut self, target: f64, frames: f64, curve: f64) {
        if frames < 1.0 {
            self.jump(target);
            return;
        }
        self.start = self.pitch;
        self.target = target;
        self.elapsed = 0.0;
        self.duration = frames;
        self.curve = curve.clamp(0.0, 1.0);
    }

    pub(super) fn tick(&mut self) -> f64 {
        if self.elapsed < self.duration {
            self.elapsed += 1.0;
            let t = (self.elapsed / self.duration).min(1.0);
            // An RC charge that lands within 1% at `duration`, rescaled to finish exactly.
            let rc = (1.0 - (-4.6 * t).exp()) / (1.0 - (-4.6_f64).exp());
            let shape = rc + (t - rc) * self.curve;
            self.pitch = self.start + (self.target - self.start) * shape;
            if self.elapsed >= self.duration {
                self.pitch = self.target;
            }
        }
        self.pitch
    }
}

// ---------------------------------------------------------------------------------------
// Oversampling
// ---------------------------------------------------------------------------------------

/// A 2:1 half-band FIR decimator (Kaiser-windowed sinc). Half the taps are zero, so only
/// the odd ones and the centre are evaluated.
#[derive(Debug, Clone)]
pub(super) struct HalfBand {
    taps: Vec<f64>,
    history: Vec<f64>,
    position: usize,
    centre: f64,
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..32 {
        term *= half / k as f64;
        sum += term * term;
    }
    sum
}

impl HalfBand {
    /// `length` must be 4n+3 so the centre tap sits between non-zero odd taps.
    pub(super) fn new(length: usize, beta: f64) -> Self {
        assert!(length % 4 == 3);
        let middle = (length / 2) as isize;
        let mut odd = Vec::new();
        for n in 0..length as isize {
            let m = n - middle;
            if m % 2 == 0 {
                continue;
            }
            let x = m as f64 * PI / 2.0;
            let sinc = x.sin() / x;
            let ratio = m as f64 / middle as f64;
            let window = bessel_i0(beta * (1.0 - ratio * ratio).max(0.0).sqrt()) / bessel_i0(beta);
            odd.push(0.5 * sinc * window);
        }
        let sum: f64 = odd.iter().sum::<f64>() + 0.5;
        let odd: Vec<f64> = odd.into_iter().map(|tap| tap / sum).collect();
        Self {
            history: vec![0.0; length],
            taps: odd,
            position: 0,
            centre: 0.5 / sum,
        }
    }

    pub(super) fn reset(&mut self) {
        self.history.fill(0.0);
        self.position = 0;
    }

    fn push(&mut self, sample: f64) {
        self.history[self.position] = sample;
        self.position = (self.position + 1) % self.history.len();
    }

    /// Consumes two input samples and returns one output sample.
    pub(super) fn decimate(&mut self, a: f64, b: f64) -> f64 {
        self.push(a);
        self.push(b);
        let length = self.history.len();
        let oldest = self.position;
        let at = |index: usize| self.history[(oldest + index) % length];
        let mut sum = self.centre * at(length / 2);
        for (k, tap) in self.taps.iter().enumerate() {
            sum += tap * at(2 * k);
        }
        sum
    }
}

/// Cascaded half-band stages from `factor`× down to the base rate.
#[derive(Debug, Clone)]
pub(super) struct Decimator {
    stages: Vec<HalfBand>,
    scratch: Vec<f64>,
}

impl Decimator {
    pub(super) fn new(factor: usize, exact: bool) -> Self {
        let mut stages = Vec::new();
        let mut remaining = factor;
        let mut last = true;
        while remaining > 1 {
            // The last stage (nearest the base rate) needs the steepest transition; the
            // earlier ones only guard the far-away images.
            let length = match (last, exact) {
                (true, true) => 95,
                (true, false) => 31,
                (false, true) => 23,
                (false, false) => 11,
            };
            stages.push(HalfBand::new(length, if exact { 10.0 } else { 7.0 }));
            remaining /= 2;
            last = false;
        }
        stages.reverse();
        Self {
            stages,
            scratch: vec![0.0; factor],
        }
    }

    pub(super) fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }

    /// Reduces `factor` oversampled samples to one.
    pub(super) fn process(&mut self, input: &[f64]) -> f64 {
        if self.stages.is_empty() {
            return input[0];
        }
        let mut count = input.len();
        self.scratch[..count].copy_from_slice(input);
        for stage in &mut self.stages {
            for i in 0..count / 2 {
                self.scratch[i] = stage.decimate(self.scratch[2 * i], self.scratch[2 * i + 1]);
            }
            count /= 2;
        }
        self.scratch[0]
    }
}

// ---------------------------------------------------------------------------------------
// Simple filters
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct OnePole {
    state: f64,
}

impl OnePole {
    pub(super) fn lowpass(&mut self, input: f64, coefficient: f64) -> f64 {
        self.state += (input - self.state) * coefficient;
        self.state
    }

    pub(super) fn reset(&mut self) {
        self.state = 0.0;
    }
}

pub(super) fn one_pole_coefficient(hz: f64, rate: f64) -> f64 {
    1.0 - (-TAU * hz / rate).exp()
}

/// The Juno's fixed high-pass: 0 lifts the lows, 1 is flat, 2 and 3 thin the sound.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct JunoHpf {
    low: OnePole,
}

impl JunoHpf {
    pub(super) fn tick(&mut self, input: f64, position: u32, rate: f64) -> f64 {
        match position {
            0 => input + 0.8 * self.low.lowpass(input, one_pole_coefficient(110.0, rate)),
            1 => {
                self.low.lowpass(input, one_pole_coefficient(110.0, rate));
                input
            }
            2 => input - self.low.lowpass(input, one_pole_coefficient(240.0, rate)),
            _ => input - self.low.lowpass(input, one_pole_coefficient(720.0, rate)),
        }
    }

    pub(super) fn reset(&mut self) {
        self.low.reset();
    }
}

/// A two-pole state-variable low-pass, used for the BBD's anti-alias and reconstruction
/// filters.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Svf {
    ic1: f64,
    ic2: f64,
}

impl Svf {
    pub(super) fn lowpass(&mut self, input: f64, g: f64, damping: f64) -> f64 {
        let a1 = 1.0 / (1.0 + g * (g + damping));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------------------
// BBD chorus
// ---------------------------------------------------------------------------------------

pub const CHORUS_MODES: [&str; 4] = ["Off", "I", "II", "I+II"];

/// A model of the Juno's bucket-brigade stereo chorus: a mono voice feeds one delay
/// line tapped by two triangle-modulated readers in antiphase. Each tap goes through the
/// BBD's band-limiting filters and a little companding grit. The line adds hiss, which
/// the compander's expander pulls down with the signal, so it breathes with the music and
/// is gone in silence. Offline renders read the moving taps with cubic rather than linear
/// interpolation.
#[derive(Debug, Clone)]
pub(super) struct Chorus {
    buffer: Vec<f64>,
    write: usize,
    lfo: f64,
    pre: Svf,
    post: [Svf; 2],
    level: f64,
    /// The compander's envelope of the input, which sets how much hiss survives.
    envelope: f64,
    rng: Rng,
    /// The last mode that was on, so switching off fades out the sound that was playing.
    voiced: u32,
}

const CHORUS_BUFFER: usize = 1024;
/// BBD hiss relative to the signal envelope, about −50 dB under the music.
const CHORUS_HISS: f64 = 0.003;
const CHORUS_SEED: u64 = 0x6262_645f_6869_7373;

impl Chorus {
    pub(super) fn new() -> Self {
        Self {
            buffer: vec![0.0; CHORUS_BUFFER],
            write: 0,
            lfo: 0.0,
            pre: Svf::default(),
            post: [Svf::default(); 2],
            level: 0.0,
            envelope: 0.0,
            rng: Rng::new(CHORUS_SEED),
            voiced: 1,
        }
    }

    pub(super) fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
        self.lfo = 0.0;
        self.pre.reset();
        for filter in &mut self.post {
            filter.reset();
        }
        self.level = 0.0;
        self.envelope = 0.0;
        self.rng = Rng::new(CHORUS_SEED);
    }

    fn read(&self, delay: f64, exact: bool) -> f64 {
        let position = self.write as f64 - delay;
        let base = position.floor();
        let frac = position - base;
        let at = |offset: isize| {
            let index = (base as isize + offset).rem_euclid(CHORUS_BUFFER as isize) as usize;
            self.buffer[index]
        };
        if exact {
            // Cubic Hermite interpolation keeps the moving taps clean.
            let (y0, y1, y2, y3) = (at(-1), at(0), at(1), at(2));
            let c1 = 0.5 * (y2 - y0);
            let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
            ((c3 * frac + c2) * frac + c1) * frac + y1
        } else {
            at(0) + (at(1) - at(0)) * frac
        }
    }

    /// Returns the stereo output for one mono input sample.
    pub(super) fn tick(&mut self, input: f64, mode: u32, rate: f64, exact: bool) -> [f64; 2] {
        // Fade the wet path in and out so switching modes never clicks.
        let target = if mode == 0 { 0.0 } else { 1.0 };
        self.level += (target - self.level) * 0.002;
        if mode != 0 {
            self.voiced = mode;
        }
        let (hz, centre, depth) = match self.voiced {
            1 => (0.513, 3.5e-3, 1.85e-3),
            2 => (0.863, 3.5e-3, 1.85e-3),
            _ => (9.75, 3.3e-3, 0.3e-3),
        };
        let g = (PI * 9000.0 / rate).tan();
        let filtered = self.pre.lowpass(input, g, 1.2);
        // Mild BBD companding: the expander undoes most of the compressor, but not all.
        let filtered = filtered - 0.05 * filtered * filtered * filtered;
        let magnitude = filtered.abs();
        let speed = if magnitude > self.envelope {
            one_pole_coefficient(200.0, rate)
        } else {
            one_pole_coefficient(3.0, rate)
        };
        self.envelope += (magnitude - self.envelope) * speed;
        let hiss = self.rng.bipolar() * CHORUS_HISS * self.envelope;
        self.buffer[self.write] = filtered + hiss;
        self.lfo = (self.lfo + hz / rate).fract();
        let triangle = 1.0 - 4.0 * (self.lfo - 0.5).abs();
        let mut out = [0.0; 2];
        for (side, sign) in [(0, 1.0), (1, -1.0)] {
            let delay = (centre + sign * depth * triangle) * rate;
            let mut wet = self.read(delay, exact);
            wet = self.post[side].lowpass(wet, g, 1.2);
            out[side] = input + wet * self.level;
        }
        self.write = (self.write + 1) % CHORUS_BUFFER;
        let gain = 1.0 / (1.0 + 0.4 * self.level);
        [out[0] * gain, out[1] * gain]
    }
}

// ---------------------------------------------------------------------------------------
// Drift
// ---------------------------------------------------------------------------------------

/// Slow random-walk drift that wanders toward a new random target every `period` samples.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Drift {
    value: f64,
    target: f64,
    counter: u32,
}

impl Drift {
    pub(super) fn tick(&mut self, rng: &mut Rng, period: u32) -> f64 {
        if self.counter == 0 {
            self.counter = period;
            self.target = rng.bipolar();
        }
        self.counter -= 1;
        self.value += (self.target - self.value) * (2.0 / period as f64);
        self.value
    }

    pub(super) fn value(&self) -> f64 {
        self.value
    }
}
