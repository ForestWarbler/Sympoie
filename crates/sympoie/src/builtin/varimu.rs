//! Variable-mu tube compressor.
//!
//! Models a remote-cutoff tube gain cell in a feedback loop, after the classic broadcast
//! limiters: the detector listens to the gain cell's own output, so the ratio grows with
//! the amount of compression instead of being fixed, and the knee is wide by nature. The
//! control voltage charges an RC network through a rectifier — a fast capacitor for the
//! attack/release and, on the program-dependent time constants, a slow capacitor that
//! fills up only under sustained compression. As the tube is biased towards cutoff its
//! curvature grows, so the even-harmonic colour rises with gain reduction.
//!
//! Everything runs sample by sample with no lookahead and no oversampling: the plugin
//! reports zero latency and stays phase-coherent with its dry signal.

use std::f64::consts::TAU;
use std::sync::Arc;

use crate::Frame;
use crate::instrument::NoteEvent;
use crate::plugin::{PluginInstance, PluginProcessContext, PluginProcessor, PluginSidechainInput};

use super::effects::{
    Analyzer, Meters, RATE, amplitude, clean, decibels, output_sample, parameter, physical_values,
    smooth,
};
use super::{ParameterSpec, SCOPE_FRAMES_PER_COLUMN, Scale, Scope, ScopeColumn, Telemetry};

pub const INPUT: u32 = 0;
pub const THRESHOLD: u32 = 1;
pub const RATIO: u32 = 2;
pub const TIME_CONSTANT: u32 = 3;
pub const ATTACK: u32 = 4;
pub const RELEASE: u32 = 5;
pub const DETECTOR_HP: u32 = 6;
pub const STEREO: u32 = 7;
pub const COLOR: u32 = 8;
pub const MIX: u32 = 9;
pub const OUTPUT: u32 = 10;
pub const EXTERNAL: u32 = 11;

pub const RATIOS: [&str; 3] = ["Gentle", "Compress", "Limit"];
pub const TIME_CONSTANTS: [&str; 7] = ["1", "2", "3", "4", "5", "6", "Var"];
pub const STEREO_MODES: [&str; 3] = ["Link", "Dual", "M/S"];
/// Position of the time-constant selector that uses the Attack and Release controls.
pub const VARIABLE_TIME: usize = 6;

const PARAMETERS: [ParameterSpec; 12] = [
    parameter(INPUT, "Input", "dB", (-12.0, 24.0), 0.0, Scale::Linear, 0),
    parameter(
        THRESHOLD,
        "Threshold",
        "dB",
        (-40.0, 0.0),
        -18.0,
        Scale::Linear,
        0,
    ),
    parameter(RATIO, "Ratio", "", (0.0, 2.0), 1.0, Scale::Linear, 2),
    parameter(
        TIME_CONSTANT,
        "Time Constant",
        "",
        (0.0, 6.0),
        1.0,
        Scale::Linear,
        6,
    ),
    parameter(ATTACK, "Attack", "ms", (0.2, 80.0), 2.0, Scale::Log, 0),
    parameter(
        RELEASE,
        "Release",
        "ms",
        (50.0, 5000.0),
        400.0,
        Scale::Log,
        0,
    ),
    parameter(
        DETECTOR_HP,
        "Detector HP",
        "Hz",
        (20.0, 500.0),
        20.0,
        Scale::Log,
        0,
    ),
    parameter(STEREO, "Stereo", "", (0.0, 2.0), 0.0, Scale::Linear, 2),
    parameter(COLOR, "Color", "%", (0.0, 100.0), 35.0, Scale::Linear, 0),
    parameter(MIX, "Mix", "%", (0.0, 100.0), 100.0, Scale::Linear, 0),
    parameter(OUTPUT, "Output", "dB", (-24.0, 12.0), 0.0, Scale::Linear, 0),
    parameter(
        EXTERNAL,
        "External Sidechain",
        "",
        (0.0, 1.0),
        0.0,
        Scale::Linear,
        1,
    ),
];
pub fn parameters() -> Vec<ParameterSpec> {
    PARAMETERS.to_vec()
}

/// Remote-cutoff gain law, `gain = (1 + slope · bias)^-exponent`. Inside the feedback loop
/// the ratio starts near 1:1 at threshold and approaches `1 + exponent` deep into
/// compression.
#[derive(Debug, Clone, Copy)]
struct Curve {
    exponent: f64,
    slope: f64,
}

const CURVES: [Curve; 3] = [
    Curve {
        exponent: 1.0,
        slope: 0.5,
    },
    Curve {
        exponent: 3.0,
        slope: 0.5,
    },
    Curve {
        exponent: 8.0,
        slope: 0.7,
    },
];

impl Curve {
    fn of(ratio: f64) -> Self {
        CURVES[(ratio.round().max(0.0) as usize).min(CURVES.len() - 1)]
    }

    fn gain(self, bias: f64) -> f64 {
        (1.0 + self.slope * bias.max(0.0)).powf(-self.exponent)
    }
}

/// Steady-state output level of the feedback loop for a signal whose peak sits at
/// `level_db`, both measured at the gain cell (after Input).
pub fn static_output_db(level_db: f64, threshold_db: f64, ratio: f64) -> f64 {
    let curve = Curve::of(ratio);
    let over = amplitude(level_db - threshold_db);
    if !over.is_finite() || over <= 1.0 {
        return level_db;
    }
    // The loop settles where output · gain(output − threshold)⁻¹ equals the input, which
    // is monotonic in the output level between threshold and the uncompressed level.
    let (mut low, mut high) = (1.0_f64, over);
    for _ in 0..64 {
        let mid = 0.5 * (low + high);
        if mid / curve.gain(mid - 1.0) < over {
            low = mid;
        } else {
            high = mid;
        }
    }
    threshold_db + decibels(0.5 * (low + high))
}

/// Seconds for (attack, release, program release, program charge); a zero program
/// release means the position has a single release.
const TIMINGS: [(f64, f64, f64, f64); 6] = [
    (0.0002, 0.3, 0.0, 0.0),
    (0.0002, 0.8, 0.0, 0.0),
    (0.0004, 2.0, 0.0, 0.0),
    (0.0004, 5.0, 0.0, 0.0),
    (0.0004, 2.0, 10.0, 0.6),
    (0.0002, 0.3, 12.0, 0.5),
];

fn coefficient(seconds: f64) -> f64 {
    1.0 - (-1.0 / (seconds.max(1.0e-6) * RATE)).exp()
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Ballistics {
    attack: f64,
    release: f64,
    program_release: f64,
    program_charge: f64,
}

impl Ballistics {
    fn new(values: &[f64; 12]) -> Self {
        let position = (values[TIME_CONSTANT as usize].round().max(0.0) as usize).min(6);
        let (attack, release, program_release, program_charge) = if position == VARIABLE_TIME {
            (
                values[ATTACK as usize] / 1000.0,
                values[RELEASE as usize] / 1000.0,
                0.0,
                0.0,
            )
        } else {
            TIMINGS[position]
        };
        Self {
            attack: coefficient(attack),
            release: coefficient(release),
            program_release: if program_release > 0.0 {
                coefficient(program_release)
            } else {
                0.0
            },
            program_charge: if program_charge > 0.0 {
                coefficient(program_charge)
            } else {
                0.0
            },
        }
    }
}

/// Rectifier and timing network for one control voltage.
#[derive(Debug, Default, Clone, Copy)]
struct Bias {
    fast: f64,
    slow: f64,
}

impl Bias {
    fn level(self) -> f64 {
        self.fast.max(self.slow)
    }

    /// Advances the network by one sample. `key` is the detector level relative to
    /// threshold; with `feedback` it is measured before the gain cell and the cell's own
    /// gain is applied inside the loop.
    fn charge(&mut self, key: f64, feedback: Option<Curve>, timing: Ballistics) {
        let excess = |bias: f64| match feedback {
            Some(curve) => key * curve.gain(bias) - 1.0,
            None => key - 1.0,
        };
        // The rectifier only conducts while the detector exceeds the capacitor voltage;
        // otherwise the capacitor bleeds towards zero through its release resistor.
        let now = excess(self.level());
        if now > self.fast {
            let explicit = self.fast + timing.attack * (now - self.fast);
            self.fast = match feedback {
                Some(curve) if explicit > self.slow => {
                    settle(curve, key, self.fast, timing.attack, explicit)
                }
                _ => explicit,
            };
        } else {
            self.fast -= timing.release * self.fast;
        }
        let excess = excess(self.level());
        // The program capacitor sits on the same rectifier through a larger resistor, so
        // a lone transient barely fills it while sustained compression does.
        if timing.program_release > 0.0 {
            if excess > self.slow {
                self.slow += timing.program_charge * (excess - self.slow);
            } else {
                self.slow -= timing.program_release * self.slow;
            }
        } else {
            self.slow = 0.0;
        }
        if self.fast < 1.0e-12 {
            self.fast = 0.0;
        }
        if self.slow < 1.0e-12 {
            self.slow = 0.0;
        }
    }
}

/// Solves one backward-Euler step of the feedback loop,
/// `bias = previous + attack · (key · gain(bias) − 1 − previous)`, so the detector hears
/// the gain it is producing in the same sample. An explicit step would overshoot by tens of
/// dB when a loud signal hits a deep limiting curve; the implicit one cannot.
fn settle(curve: Curve, key: f64, previous: f64, attack: f64, upper: f64) -> f64 {
    // In u = ln(1 + slope · bias) the gain is exp(−exponent · u) and the residual is
    // strictly increasing, bracketed by the previous bias and the explicit step.
    let residual = |u: f64| {
        let pull = attack * curve.exponent * key * (-curve.exponent * u).exp();
        let value = u.exp_m1() / curve.slope
            - previous
            - attack * (key * (-curve.exponent * u).exp() - 1.0 - previous);
        (value, u.exp() / curve.slope + pull)
    };
    let mut low = (curve.slope * previous).ln_1p();
    let mut high = (curve.slope * upper).ln_1p();
    let mut u = high;
    for _ in 0..32 {
        let (value, slope) = residual(u);
        if value > 0.0 {
            high = u;
        } else {
            low = u;
        }
        let next = u - value / slope;
        let next = if next > low && next < high {
            next
        } else {
            0.5 * (low + high)
        };
        if (next - u).abs() <= 1.0e-12 * (1.0 + u) {
            u = next;
            break;
        }
        u = next;
    }
    (u.exp_m1() / curve.slope).clamp(previous, upper)
}

#[derive(Debug, Default, Clone, Copy)]
struct HighPass {
    input: f64,
    output: f64,
}

impl HighPass {
    fn process(&mut self, sample: f64, pole: f64) -> f64 {
        let output = pole * (self.output + sample - self.input);
        self.input = sample;
        self.output = if output.abs() < 1.0e-25 { 0.0 } else { output };
        self.output
    }
}

/// Smoothed controls, kept as linear gains so the per-sample path avoids `powf`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Controls {
    drive: f64,
    threshold: f64,
    color: f64,
    mix: f64,
    output: f64,
}

impl Controls {
    fn new(values: &[f64; 12]) -> Self {
        Self {
            drive: amplitude(values[INPUT as usize]),
            threshold: amplitude(values[THRESHOLD as usize]),
            color: values[COLOR as usize] / 100.0,
            mix: values[MIX as usize] / 100.0,
            output: amplitude(values[OUTPUT as usize]),
        }
    }

    fn approach(&mut self, target: Self) {
        smooth(&mut self.drive, target.drive);
        smooth(&mut self.threshold, target.threshold);
        smooth(&mut self.color, target.color);
        smooth(&mut self.mix, target.mix);
        smooth(&mut self.output, target.output);
    }
}

/// Tube and transformer headroom, in full-scale units, used by the colour stage.
const HEADROOM: f64 = 2.0;
const DC_POLE: f64 = 1.0 - TAU * 8.0 / RATE;

fn color(sample: f64, depth: f64, amount: f64, dc: &mut HighPass) -> f64 {
    if amount <= 0.0 {
        return sample;
    }
    let saturated = sample + amount * (HEADROOM * (sample / HEADROOM).tanh() - sample);
    // Second-harmonic curvature grows as the bias pushes the tube towards cutoff; only
    // that term is DC-blocked so the fundamental keeps its phase.
    let even = amount * (0.04 + 0.16 * depth) * saturated * saturated;
    saturated + dc.process(even, DC_POLE)
}

/// Summarises before/after audio into scope columns.
#[derive(Debug, Default)]
struct ScopeWriter {
    column: ScopeColumn,
    frames: usize,
}

impl ScopeWriter {
    fn frame(&mut self, input: [f64; 2], output: [f64; 2], reduction: f64, scope: &Scope) {
        let column = &mut self.column;
        if self.frames == 0 {
            *column = ScopeColumn {
                input_min: f32::INFINITY,
                input_max: f32::NEG_INFINITY,
                output_min: f32::INFINITY,
                output_max: f32::NEG_INFINITY,
                ..ScopeColumn::default()
            };
        }
        for channel in 0..2 {
            let (before, after) = (input[channel] as f32, output[channel] as f32);
            column.input_min = column.input_min.min(before);
            column.input_max = column.input_max.max(before);
            column.output_min = column.output_min.min(after);
            column.output_max = column.output_max.max(after);
            column.input_power += before * before;
            column.output_power += after * after;
        }
        column.reduction_db = column.reduction_db.max(reduction as f32);
        self.frames += 1;
        if self.frames == SCOPE_FRAMES_PER_COLUMN {
            let samples = (2 * SCOPE_FRAMES_PER_COLUMN) as f32;
            column.input_power /= samples;
            column.output_power /= samples;
            scope.push(*column);
            self.frames = 0;
        }
    }
}

/// Before/after analysis, only built when an interface is listening.
#[derive(Debug)]
struct Analysis {
    before: Analyzer,
    after: Analyzer,
    scope: ScopeWriter,
}

#[derive(Debug)]
struct VariMuProcessor {
    target: [f64; 12],
    controls: Controls,
    bias: [Bias; 2],
    detector: [HighPass; 2],
    dc: [HighPass; 2],
    telemetry: Option<Arc<Telemetry>>,
    analysis: Option<Box<Analysis>>,
    meters: Meters,
}

pub fn create_varimu(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    let target = physical_values(instance, &PARAMETERS);
    let analysis = telemetry.as_ref().map(|_| {
        Box::new(Analysis {
            before: Analyzer::new(),
            after: Analyzer::new(),
            scope: ScopeWriter::default(),
        })
    });
    Ok(Box::new(VariMuProcessor {
        target,
        controls: Controls::new(&target),
        bias: [Bias::default(); 2],
        detector: [HighPass::default(); 2],
        dc: [HighPass::default(); 2],
        telemetry,
        analysis,
        meters: Meters::default(),
    }))
}

impl VariMuProcessor {
    fn run(&mut self, frames: &mut [Frame], sidechain: Option<PluginSidechainInput<'_>>) {
        let external = (self.target[EXTERNAL as usize] >= 0.5)
            .then(|| sidechain.filter(|input| input.bus_index == 1));
        let curve = Curve::of(self.target[RATIO as usize]);
        let timing = Ballistics::new(&self.target);
        let feedback = external.is_none().then_some(curve);
        let mode = self.target[STEREO as usize].round() as u32;
        let linked = mode == 0;
        let mid_side = mode == 2;
        let encode = |pair: [f64; 2]| {
            if mid_side {
                [0.5 * (pair[0] + pair[1]), 0.5 * (pair[0] - pair[1])]
            } else {
                pair
            }
        };
        let detector_pole = (-TAU * self.target[DETECTOR_HP as usize] / RATE).exp();
        let target = Controls::new(&self.target);
        for (index, frame) in frames.iter_mut().enumerate() {
            self.controls.approach(target);
            let controls = self.controls;
            let input = [clean(f64::from(frame[0])), clean(f64::from(frame[1]))];
            let driven = encode(input.map(|sample| sample * controls.drive));
            // In feedback the detector follows the gain cell's output; the gain varies
            // slowly next to the detector high-pass, so it is applied inside the loop.
            let detected = match external {
                Some(bus) => encode(
                    bus.and_then(|bus| bus.frames.get(index))
                        .copied()
                        .unwrap_or([0.0; 2])
                        .map(|sample| clean(f64::from(sample))),
                ),
                None => driven,
            };
            let key = [0, 1].map(|channel| {
                self.detector[channel]
                    .process(detected[channel], detector_pole)
                    .abs()
                    / controls.threshold
            });
            if linked {
                self.bias[0].charge(key[0].max(key[1]), feedback, timing);
                self.bias[1] = self.bias[0];
            } else {
                for (bias, key) in self.bias.iter_mut().zip(key) {
                    bias.charge(key, feedback, timing);
                }
            }
            let gain = [0, 1].map(|channel| curve.gain(self.bias[channel].level()));
            let mut wet = [0, 1].map(|channel| {
                color(
                    driven[channel] * gain[channel],
                    1.0 - gain[channel],
                    controls.color,
                    &mut self.dc[channel],
                )
            });
            if mid_side {
                wet = [wet[0] + wet[1], wet[0] - wet[1]];
            }
            let output = [0, 1].map(|channel| {
                clean(
                    (input[channel] * (1.0 - controls.mix) + wet[channel] * controls.mix)
                        * controls.output,
                )
            });
            *frame = output.map(output_sample);
            if let Some(telemetry) = &self.telemetry {
                let reduction = -decibels(gain[0].min(gain[1]));
                self.meters.frame(input, output, reduction);
                if let Some(analysis) = &mut self.analysis {
                    analysis
                        .before
                        .push(input.map(|sample| sample as f32), &telemetry.input_spectrum);
                    if analysis.after.push(*frame, &telemetry.spectrum) {
                        telemetry
                            .spectrum_updates
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    analysis
                        .scope
                        .frame(input, output, reduction, &telemetry.scope);
                }
            }
        }
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }
}

impl PluginProcessor for VariMuProcessor {
    fn reset(&mut self) {
        self.controls = Controls::new(&self.target);
        self.bias = [Bias::default(); 2];
        self.detector = [HighPass::default(); 2];
        self.dc = [HighPass::default(); 2];
        self.meters = Meters::default();
        // The before/after analysis is display state: it keeps running through the reset the
        // engine performs at every loop wrap, so the scope stays continuous.
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }
    fn event(&mut self, _event: NoteEvent) {}
    fn process(&mut self, frames: &mut [Frame], _context: PluginProcessContext) {
        self.run(frames, None);
    }
    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        _context: PluginProcessContext,
    ) {
        self.run(frames, sidechain);
    }
    fn set_parameter(&mut self, id: u32, normalized: f64) {
        if normalized.is_finite()
            && let Some(spec) = PARAMETERS.get(id as usize)
        {
            self.target[id as usize] = spec.denormalize(normalized.clamp(0.0, 1.0));
        }
    }
}

#[cfg(test)]
#[path = "varimu_tests.rs"]
mod tests;
