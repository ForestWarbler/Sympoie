use std::f64::consts::{FRAC_PI_2, PI, TAU};

use crate::instrument::NoteEvent;
use crate::plugin::{PluginInstance, PluginProcessContext, PluginProcessor};
use crate::{Frame, SAMPLE_RATE};

use super::{ParameterSpec, Scale};

mod sampler;
pub use sampler::{
    SamplePreview, create_sampler, sample_preview, sample_state_fingerprint, sample_state_from_wav,
    sampler_note_tail_frames, sampler_note_tail_frames_with_automation,
};

pub const GAIN: u32 = 0;
pub const PAN: u32 = 1;
pub const VELOCITY: u32 = 2;
pub const ATTACK: u32 = 10;
pub const DECAY: u32 = 11;
pub const SUSTAIN: u32 = 12;
pub const RELEASE: u32 = 13;
pub const FILTER_CUTOFF: u32 = 20;
pub const FILTER_RESONANCE: u32 = 21;
pub const FILTER_TYPE: u32 = 22;
pub const FILTER_ENVELOPE: u32 = 23;
pub const FILTER_KEYTRACK: u32 = 24;
pub const FILTER_DRIVE: u32 = 25;
pub const FILTER_ATTACK: u32 = 30;
pub const FILTER_DECAY: u32 = 31;
pub const FILTER_SUSTAIN: u32 = 32;
pub const FILTER_RELEASE: u32 = 33;
pub const LFO_RATE: u32 = 40;
pub const LFO_PITCH: u32 = 41;
pub const LFO_FILTER: u32 = 42;
pub const LFO_SHAPE: u32 = 43;
pub const LFO_RETRIGGER: u32 = 44;
pub const OSC_BASE: u32 = 100;
pub const OSC_STRIDE: u32 = 10;
pub const OSC_SHAPE: u32 = 0;
pub const OSC_COARSE: u32 = 1;
pub const OSC_FINE: u32 = 2;
pub const OSC_LEVEL: u32 = 3;
pub const OSC_PAN: u32 = 4;
pub const OSC_PULSE_WIDTH: u32 = 5;

pub const MATRIX_BASE: u32 = 200;
pub const MATRIX_STRIDE: u32 = 3;
pub const MATRIX_ROUTES: usize = 6;
pub const MATRIX_SOURCE: u32 = 0;
pub const MATRIX_TARGET: u32 = 1;
pub const MATRIX_AMOUNT: u32 = 2;
pub const MATRIX_SOURCE_LABELS: [&str; 4] = ["Off", "Amp Env", "Mod Env", "LFO"];
pub const MATRIX_TARGET_LABELS: [&str; 17] = [
    "All pitch",
    "Osc 1 pitch",
    "Osc 2 pitch",
    "Osc 3 pitch",
    "Osc 1 level",
    "Osc 2 level",
    "Osc 3 level",
    "Voice pan",
    "Osc 1 pan",
    "Osc 2 pan",
    "Osc 3 pan",
    "Osc 1 width",
    "Osc 2 width",
    "Osc 3 width",
    "Cutoff",
    "Resonance",
    "Voice gain",
];

pub fn matrix_target_depth(target: u32) -> (f64, &'static str) {
    match target {
        0..=3 => (24.0, "st"),
        4..=6 | 16 => (24.0, "dB"),
        7..=10 => (1.0, "pan"),
        11..=13 => (0.45, "width"),
        14 => (8.0, "oct"),
        15 => (3.0, "oct"),
        _ => (0.0, ""),
    }
}

pub const SAMPLE_ROOT: u32 = 20;
pub const SAMPLE_TRANSPOSE: u32 = 21;
pub const SAMPLE_TUNE: u32 = 22;
pub const SAMPLE_MODE: u32 = 23;
pub const SAMPLE_REVERSE: u32 = 24;
pub const SAMPLE_START: u32 = 30;
pub const SAMPLE_END: u32 = 31;
pub const SAMPLE_LOOP_START: u32 = 32;
pub const SAMPLE_LOOP_END: u32 = 33;
pub const SAMPLE_CROSSFADE: u32 = 34;
pub const SAMPLE_CUTOFF: u32 = 40;
pub const SAMPLE_RESONANCE: u32 = 41;

const VOICES: usize = 32;
const PARAMETER_COUNT: usize = 256;
const SMOOTH_SAMPLES: usize = 240;

fn spec(
    id: u32,
    name: &'static str,
    unit: &'static str,
    range: [f64; 3],
    scale: Scale,
    steps: u32,
) -> ParameterSpec {
    ParameterSpec {
        id,
        name,
        unit,
        min: range[0],
        max: range[1],
        default: range[2],
        scale,
        steps,
    }
}

fn common_parameters(gain: f64) -> Vec<ParameterSpec> {
    vec![
        spec(GAIN, "Gain", "dB", [-60.0, 6.0, gain], Scale::Linear, 0),
        spec(PAN, "Pan", "", [-1.0, 1.0, 0.0], Scale::Linear, 0),
        spec(VELOCITY, "Velocity", "", [0.0, 1.0, 0.8], Scale::Linear, 0),
        spec(ATTACK, "Attack", "s", [0.0001, 10.0, 0.005], Scale::Log, 0),
        spec(DECAY, "Decay", "s", [0.001, 10.0, 0.25], Scale::Log, 0),
        spec(SUSTAIN, "Sustain", "", [0.0, 1.0, 0.75], Scale::Linear, 0),
        spec(RELEASE, "Release", "s", [0.001, 15.0, 0.25], Scale::Log, 0),
    ]
}

pub fn oscil_parameters() -> Vec<ParameterSpec> {
    let mut result = common_parameters(-12.0);
    result.extend([
        spec(
            FILTER_CUTOFF,
            "Cutoff",
            "Hz",
            [20.0, 20000.0, 6000.0],
            Scale::Log,
            0,
        ),
        spec(
            FILTER_RESONANCE,
            "Resonance",
            "Q",
            [0.5, 12.0, 0.707],
            Scale::Log,
            0,
        ),
        spec(
            FILTER_TYPE,
            "Filter type",
            "",
            [0.0, 2.0, 0.0],
            Scale::Linear,
            2,
        ),
        spec(
            FILTER_ENVELOPE,
            "Filter envelope",
            "oct",
            [-8.0, 8.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            FILTER_KEYTRACK,
            "Key tracking",
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            FILTER_DRIVE,
            "Drive",
            "dB",
            [0.0, 24.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            FILTER_ATTACK,
            "Filter attack",
            "s",
            [0.0001, 10.0, 0.005],
            Scale::Log,
            0,
        ),
        spec(
            FILTER_DECAY,
            "Filter decay",
            "s",
            [0.001, 10.0, 0.3],
            Scale::Log,
            0,
        ),
        spec(
            FILTER_SUSTAIN,
            "Filter sustain",
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            FILTER_RELEASE,
            "Filter release",
            "s",
            [0.001, 15.0, 0.25],
            Scale::Log,
            0,
        ),
        spec(LFO_RATE, "LFO rate", "Hz", [0.02, 30.0, 2.0], Scale::Log, 0),
        spec(
            LFO_PITCH,
            "LFO pitch",
            "cent",
            [0.0, 1200.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            LFO_FILTER,
            "LFO filter",
            "oct",
            [0.0, 6.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            LFO_SHAPE,
            "LFO shape",
            "",
            [0.0, 2.0, 0.0],
            Scale::Linear,
            2,
        ),
        spec(
            LFO_RETRIGGER,
            "LFO retrigger",
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            1,
        ),
    ]);
    const NAMES: [[&str; 6]; 3] = [
        [
            "Osc 1 shape",
            "Osc 1 coarse",
            "Osc 1 fine",
            "Osc 1 level",
            "Osc 1 pan",
            "Osc 1 width",
        ],
        [
            "Osc 2 shape",
            "Osc 2 coarse",
            "Osc 2 fine",
            "Osc 2 level",
            "Osc 2 pan",
            "Osc 2 width",
        ],
        [
            "Osc 3 shape",
            "Osc 3 coarse",
            "Osc 3 fine",
            "Osc 3 level",
            "Osc 3 pan",
            "Osc 3 width",
        ],
    ];
    for (osc, names) in NAMES.iter().enumerate() {
        let id = OSC_BASE + osc as u32 * OSC_STRIDE;
        result.extend([
            spec(id, names[0], "", [0.0, 3.0, 2.0], Scale::Linear, 3),
            spec(
                id + 1,
                names[1],
                "st",
                [-48.0, 48.0, 0.0],
                Scale::Linear,
                96,
            ),
            spec(
                id + 2,
                names[2],
                "cent",
                [-100.0, 100.0, if osc == 1 { 7.0 } else { 0.0 }],
                Scale::Linear,
                0,
            ),
            spec(
                id + 3,
                names[3],
                "dB",
                [-60.0, 0.0, if osc == 0 { 0.0 } else { -60.0 }],
                Scale::Linear,
                0,
            ),
            spec(id + 4, names[4], "", [-1.0, 1.0, 0.0], Scale::Linear, 0),
            spec(id + 5, names[5], "", [0.05, 0.95, 0.5], Scale::Linear, 0),
        ]);
    }
    const MATRIX_NAMES: [[&str; 3]; MATRIX_ROUTES] = [
        ["Mod 1 source", "Mod 1 target", "Mod 1 amount"],
        ["Mod 2 source", "Mod 2 target", "Mod 2 amount"],
        ["Mod 3 source", "Mod 3 target", "Mod 3 amount"],
        ["Mod 4 source", "Mod 4 target", "Mod 4 amount"],
        ["Mod 5 source", "Mod 5 target", "Mod 5 amount"],
        ["Mod 6 source", "Mod 6 target", "Mod 6 amount"],
    ];
    for (route, names) in MATRIX_NAMES.iter().enumerate() {
        let id = MATRIX_BASE + route as u32 * MATRIX_STRIDE;
        result.extend([
            spec(
                id + MATRIX_SOURCE,
                names[0],
                "",
                [0.0, 3.0, 0.0],
                Scale::Linear,
                3,
            ),
            spec(
                id + MATRIX_TARGET,
                names[1],
                "",
                [0.0, 16.0, 14.0],
                Scale::Linear,
                16,
            ),
            spec(
                id + MATRIX_AMOUNT,
                names[2],
                "",
                [-1.0, 1.0, 0.0],
                Scale::Linear,
                0,
            ),
        ]);
    }
    result
}

pub fn sampler_parameters() -> Vec<ParameterSpec> {
    let mut result = common_parameters(-6.0);
    result.iter_mut().find(|p| p.id == SUSTAIN).unwrap().default = 1.0;
    result.extend([
        spec(
            SAMPLE_ROOT,
            "Root key",
            "MIDI",
            [0.0, 127.0, 60.0],
            Scale::Linear,
            127,
        ),
        spec(
            SAMPLE_TRANSPOSE,
            "Transpose",
            "st",
            [-48.0, 48.0, 0.0],
            Scale::Linear,
            96,
        ),
        spec(
            SAMPLE_TUNE,
            "Tune",
            "cent",
            [-100.0, 100.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            SAMPLE_MODE,
            "Playback",
            "",
            [0.0, 2.0, 1.0],
            Scale::Linear,
            2,
        ),
        spec(
            SAMPLE_REVERSE,
            "Reverse",
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            1,
        ),
        spec(
            SAMPLE_START,
            "Sample start",
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            SAMPLE_END,
            "Sample end",
            "",
            [0.0, 1.0, 1.0],
            Scale::Linear,
            0,
        ),
        spec(
            SAMPLE_LOOP_START,
            "Loop start",
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            SAMPLE_LOOP_END,
            "Loop end",
            "",
            [0.0, 1.0, 1.0],
            Scale::Linear,
            0,
        ),
        spec(
            SAMPLE_CROSSFADE,
            "Loop crossfade",
            "ms",
            [0.0, 250.0, 5.0],
            Scale::Linear,
            0,
        ),
        spec(
            SAMPLE_CUTOFF,
            "Cutoff",
            "Hz",
            [20.0, 20000.0, 20000.0],
            Scale::Log,
            0,
        ),
        spec(
            SAMPLE_RESONANCE,
            "Resonance",
            "Q",
            [0.5, 12.0, 0.707],
            Scale::Log,
            0,
        ),
    ]);
    result
}

#[derive(Debug)]
struct Parameters {
    specs: Vec<ParameterSpec>,
    current: [f64; PARAMETER_COUNT],
    target: [f64; PARAMETER_COUNT],
    increments: [f64; PARAMETER_COUNT],
    remaining: [usize; PARAMETER_COUNT],
}

#[derive(Debug, Clone, Copy)]
struct ModeBlend<const N: usize> {
    weights: [f64; N],
    increments: [f64; N],
    target: usize,
    remaining: usize,
}

impl<const N: usize> ModeBlend<N> {
    fn new(mode: u32) -> Self {
        let target = (mode as usize).min(N - 1);
        let mut weights = [0.0; N];
        weights[target] = 1.0;
        Self {
            weights,
            increments: [0.0; N],
            target,
            remaining: 0,
        }
    }

    fn set(&mut self, mode: u32) {
        let target = (mode as usize).min(N - 1);
        if target == self.target {
            return;
        }
        self.target = target;
        self.remaining = SMOOTH_SAMPLES;
        for (index, increment) in self.increments.iter_mut().enumerate() {
            let target_weight = if index == target { 1.0 } else { 0.0 };
            *increment = (target_weight - self.weights[index]) / SMOOTH_SAMPLES as f64;
        }
    }

    fn advance(&mut self) {
        if self.remaining == 0 {
            return;
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            *self = Self::new(self.target as u32);
        } else {
            for (weight, increment) in self.weights.iter_mut().zip(self.increments) {
                *weight += increment;
            }
        }
    }
}

impl Parameters {
    fn new(instance: &PluginInstance, specs: Vec<ParameterSpec>) -> Self {
        let mut current = [0.0; PARAMETER_COUNT];
        for spec in &specs {
            let value = instance
                .parameters
                .iter()
                .find(|p| p.id == spec.id)
                .map_or(spec.default, |p| spec.denormalize(p.value));
            current[spec.id as usize] = value;
        }
        Self {
            specs,
            current,
            target: current,
            increments: [0.0; PARAMETER_COUNT],
            remaining: [0; PARAMETER_COUNT],
        }
    }

    fn set(&mut self, id: u32, value: f64, immediate: bool) {
        if !value.is_finite() {
            return;
        }
        let Some(spec) = self.specs.iter().find(|spec| spec.id == id) else {
            return;
        };
        let physical = spec.denormalize(value.clamp(0.0, 1.0));
        let i = id as usize;
        self.target[i] = physical;
        if immediate || spec.steps > 0 {
            self.current[i] = physical;
            self.remaining[i] = 0;
        } else {
            self.increments[i] = (physical - self.current[i]) / SMOOTH_SAMPLES as f64;
            self.remaining[i] = SMOOTH_SAMPLES;
        }
    }

    fn advance(&mut self) {
        for spec in &self.specs {
            let i = spec.id as usize;
            if self.remaining[i] > 0 {
                self.remaining[i] -= 1;
                self.current[i] = if self.remaining[i] == 0 {
                    self.target[i]
                } else {
                    self.current[i] + self.increments[i]
                };
            }
        }
    }

    fn finish(&mut self) {
        self.current = self.target;
        self.remaining.fill(0);
    }
    fn get(&self, id: u32) -> f64 {
        self.current[id as usize]
    }
    fn envelope(&self, base: u32) -> EnvelopeSettings {
        EnvelopeSettings {
            attack: self.get(base),
            decay: self.get(base + 1),
            sustain: self.get(base + 2),
            release: self.get(base + 3),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct EnvelopeSettings {
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Silent,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone, Copy)]
struct Envelope {
    stage: Stage,
    level: f64,
    release_step: f64,
}

impl Envelope {
    const SILENT: Self = Self {
        stage: Stage::Silent,
        level: 0.0,
        release_step: 0.0,
    };
    fn trigger() -> Self {
        Self {
            stage: Stage::Attack,
            ..Self::SILENT
        }
    }
    fn off(&mut self, seconds: f64) {
        if self.stage != Stage::Silent && self.stage != Stage::Release {
            self.stage = Stage::Release;
            self.release_step = self.level / (seconds * f64::from(SAMPLE_RATE)).max(1.0);
        }
    }
    fn next(&mut self, settings: EnvelopeSettings) -> f64 {
        match self.stage {
            Stage::Silent => return 0.0,
            Stage::Attack => {
                self.level += 1.0 / (settings.attack * f64::from(SAMPLE_RATE)).max(1.0);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level -=
                    (1.0 - settings.sustain) / (settings.decay * f64::from(SAMPLE_RATE)).max(1.0);
                if self.level <= settings.sustain {
                    self.level = settings.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = settings.sustain,
            Stage::Release => {
                self.level -= self.release_step;
                if self.level <= 1.0e-12 {
                    *self = Self::SILENT;
                }
            }
        }
        self.level
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Filter {
    ic1: [f64; 2],
    ic2: [f64; 2],
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
}

impl Filter {
    pub(super) const EMPTY: Self = Self {
        ic1: [0.0; 2],
        ic2: [0.0; 2],
        a1: 1.0,
        a2: 0.0,
        a3: 0.0,
        k: 1.0,
    };
    pub(super) fn coefficients(&mut self, hz: f64, q: f64) {
        let g = (PI * hz.clamp(10.0, 20000.0) / f64::from(SAMPLE_RATE)).tan();
        self.k = 1.0 / q.clamp(0.5, 12.0);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
    pub(super) fn process(&mut self, input: [f64; 2], mode: u32) -> [f64; 2] {
        let mut weights = [0.0; 3];
        weights[(mode as usize).min(2)] = 1.0;
        self.process_blended(input, weights)
    }

    fn process_blended(&mut self, input: [f64; 2], weights: [f64; 3]) -> [f64; 2] {
        let mut output = [0.0; 2];
        for channel in 0..2 {
            let v3 = input[channel] - self.ic2[channel];
            let v1 = self.a1 * self.ic1[channel] + self.a2 * v3;
            let v2 = self.ic2[channel] + self.a2 * self.ic1[channel] + self.a3 * v3;
            self.ic1[channel] = 2.0 * v1 - self.ic1[channel];
            self.ic2[channel] = 2.0 * v2 - self.ic2[channel];
            output[channel] = v2 * weights[0]
                + (input[channel] - self.k * v1 - v2) * weights[1]
                + self.k * v1 * weights[2];
            if self.ic1[channel].abs() < 1e-20 {
                self.ic1[channel] = 0.0;
            }
            if self.ic2[channel].abs() < 1e-20 {
                self.ic2[channel] = 0.0;
            }
        }
        output
    }
}

fn amplitude(db: f64) -> f64 {
    if db <= -60.0 {
        0.0
    } else {
        10.0_f64.powf(db / 20.0)
    }
}
fn pan(p: f64) -> [f64; 2] {
    let angle = (p.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_2 / 2.0;
    [angle.cos(), angle.sin()]
}
fn balance(p: f64) -> [f64; 2] {
    if p < 0.0 {
        [1.0, ((-p) * FRAC_PI_2).cos()]
    } else {
        [(p * FRAC_PI_2).cos(), 1.0]
    }
}
fn velocity_gain(velocity: u8, amount: f64) -> f64 {
    1.0 - amount + amount * f64::from(velocity) / 127.0
}
fn note_hz(key: u8) -> f64 {
    440.0 * ((f64::from(key) - 69.0) / 12.0).exp2()
}

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

pub(super) fn oscillator(shape: u32, phase: f64, step: f64, width: f64) -> f64 {
    match shape {
        0 => (TAU * phase).sin(),
        1 => {
            1.0 - 4.0 * (phase - 0.5).abs()
                + 4.0 * step * (blamp(phase, step) - blamp((phase + 0.5).fract(), step))
        }
        2 => 2.0 * phase - 1.0 - blep(phase, step),
        _ => {
            let width = width.clamp(step, 1.0 - step);
            let raw = if phase < width { 1.0 } else { -1.0 };
            raw + blep(phase, step)
                - blep((phase - width).rem_euclid(1.0), step)
                - (2.0 * width - 1.0)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SynthVoice {
    id: u64,
    key: u8,
    velocity: u8,
    frequency: f64,
    phases: [f64; 3],
    amp: Envelope,
    modulation: Envelope,
    filter: Filter,
}

impl SynthVoice {
    const SILENT: Self = Self {
        id: 0,
        key: 0,
        velocity: 0,
        frequency: 0.0,
        phases: [0.0; 3],
        amp: Envelope::SILENT,
        modulation: Envelope::SILENT,
        filter: Filter::EMPTY,
    };
}

fn matrix_displacements(
    params: &Parameters,
    sources: &[ModeBlend<4>; MATRIX_ROUTES],
    targets: &[ModeBlend<17>; MATRIX_ROUTES],
    amplitude_envelope: f64,
    modulation_envelope: f64,
    lfo: f64,
) -> [f64; 17] {
    let mut result = [0.0; 17];
    for route in 0..MATRIX_ROUTES {
        let amount = params.get(MATRIX_BASE + route as u32 * MATRIX_STRIDE + MATRIX_AMOUNT);
        if amount == 0.0 {
            continue;
        }
        let source = if sources[route].remaining == 0 {
            match sources[route].target {
                1 => amplitude_envelope,
                2 => modulation_envelope,
                3 => lfo,
                _ => continue,
            }
        } else {
            let weights = sources[route].weights;
            amplitude_envelope * weights[1] + modulation_envelope * weights[2] + lfo * weights[3]
        };
        let contribution = source * amount;
        if contribution == 0.0 {
            continue;
        }
        if targets[route].remaining == 0 {
            let target = targets[route].target;
            result[target] += contribution * matrix_target_depth(target as u32).0;
        } else {
            for (target, weight) in targets[route].weights.iter().enumerate() {
                if *weight > 0.0 {
                    result[target] += contribution * weight * matrix_target_depth(target as u32).0;
                }
            }
        }
    }
    result
}

#[derive(Debug)]
struct Oscil {
    params: Parameters,
    voices: [SynthVoice; VOICES],
    lfo_phase: f64,
    control_counter: u32,
    oscillator_shapes: [ModeBlend<4>; 3],
    filter_mode: ModeBlend<3>,
    lfo_shape: ModeBlend<3>,
    matrix_sources: [ModeBlend<4>; MATRIX_ROUTES],
    matrix_targets: [ModeBlend<17>; MATRIX_ROUTES],
}

pub fn create_oscil(instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
    instance.validate().map_err(|error| error.to_string())?;
    Ok(Box::new(Oscil::new(instance)))
}

impl Oscil {
    fn new(instance: &PluginInstance) -> Self {
        let params = Parameters::new(instance, oscil_parameters());
        let oscillator_shapes = std::array::from_fn(|osc| {
            ModeBlend::new(params.get(OSC_BASE + osc as u32 * OSC_STRIDE) as u32)
        });
        let filter_mode = ModeBlend::new(params.get(FILTER_TYPE) as u32);
        let lfo_shape = ModeBlend::new(params.get(LFO_SHAPE) as u32);
        let matrix_sources = std::array::from_fn(|route| {
            ModeBlend::new(
                params.get(MATRIX_BASE + route as u32 * MATRIX_STRIDE + MATRIX_SOURCE) as u32,
            )
        });
        let matrix_targets = std::array::from_fn(|route| {
            ModeBlend::new(
                params.get(MATRIX_BASE + route as u32 * MATRIX_STRIDE + MATRIX_TARGET) as u32,
            )
        });
        Self {
            params,
            voices: [SynthVoice::SILENT; VOICES],
            lfo_phase: 0.0,
            control_counter: 0,
            oscillator_shapes,
            filter_mode,
            lfo_shape,
            matrix_sources,
            matrix_targets,
        }
    }

    fn update_parameter(&mut self, id: u32, value: f64, immediate: bool) {
        self.params.set(id, value, immediate);
        if (MATRIX_BASE..MATRIX_BASE + MATRIX_ROUTES as u32 * MATRIX_STRIDE).contains(&id) {
            let route = ((id - MATRIX_BASE) / MATRIX_STRIDE) as usize;
            match (id - MATRIX_BASE) % MATRIX_STRIDE {
                MATRIX_SOURCE => self.matrix_sources[route].set(self.params.get(id) as u32),
                MATRIX_TARGET => self.matrix_targets[route].set(self.params.get(id) as u32),
                _ => {}
            }
        }
        match id {
            FILTER_TYPE => self.filter_mode.set(self.params.get(id) as u32),
            LFO_SHAPE => self.lfo_shape.set(self.params.get(id) as u32),
            100 | 110 | 120 => self.oscillator_shapes[((id - OSC_BASE) / OSC_STRIDE) as usize]
                .set(self.params.get(id) as u32),
            _ => {}
        }
    }
}

impl PluginProcessor for Oscil {
    fn reset(&mut self) {
        self.voices.fill(SynthVoice::SILENT);
        self.params.finish();
        self.lfo_phase = 0.0;
        self.control_counter = 0;
        for (osc, blend) in self.oscillator_shapes.iter_mut().enumerate() {
            *blend = ModeBlend::new(self.params.get(OSC_BASE + osc as u32 * OSC_STRIDE) as u32);
        }
        self.filter_mode = ModeBlend::new(self.params.get(FILTER_TYPE) as u32);
        self.lfo_shape = ModeBlend::new(self.params.get(LFO_SHAPE) as u32);
        for route in 0..MATRIX_ROUTES {
            let base = MATRIX_BASE + route as u32 * MATRIX_STRIDE;
            self.matrix_sources[route] =
                ModeBlend::new(self.params.get(base + MATRIX_SOURCE) as u32);
            self.matrix_targets[route] =
                ModeBlend::new(self.params.get(base + MATRIX_TARGET) as u32);
        }
    }
    fn set_parameter(&mut self, id: u32, value: f64) {
        self.update_parameter(id, value, false);
    }
    fn set_automation_parameter(&mut self, id: u32, value: f64) {
        self.update_parameter(id, value, true);
    }
    fn event(&mut self, event: NoteEvent) {
        match event {
            NoteEvent::PitchBend { .. } => {}
            NoteEvent::On {
                id, key, velocity, ..
            } => {
                let slot = self
                    .voices
                    .iter()
                    .position(|v| v.amp.stage != Stage::Silent && v.id == id)
                    .or_else(|| {
                        self.voices
                            .iter()
                            .position(|v| v.amp.stage == Stage::Silent)
                    })
                    .unwrap_or_else(|| {
                        self.voices
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| {
                                let a_level = a.amp.level
                                    + if a.amp.stage == Stage::Release {
                                        0.0
                                    } else {
                                        2.0
                                    };
                                let b_level = b.amp.level
                                    + if b.amp.stage == Stage::Release {
                                        0.0
                                    } else {
                                        2.0
                                    };
                                a_level.total_cmp(&b_level)
                            })
                            .unwrap()
                            .0
                    });
                self.voices[slot] = SynthVoice {
                    id,
                    key,
                    velocity,
                    frequency: note_hz(key),
                    amp: Envelope::trigger(),
                    modulation: Envelope::trigger(),
                    ..SynthVoice::SILENT
                };
                if self.params.get(LFO_RETRIGGER) > 0.5 {
                    self.lfo_phase = 0.0;
                }
                self.control_counter = 0;
            }
            NoteEvent::Off { id } => {
                for voice in &mut self.voices {
                    if voice.id == id {
                        voice.amp.off(self.params.envelope(ATTACK).release);
                        voice
                            .modulation
                            .off(self.params.envelope(FILTER_ATTACK).release);
                    }
                }
            }
        }
    }
    fn process(&mut self, frames: &mut [Frame], _context: PluginProcessContext) {
        for frame in frames {
            self.params.advance();
            let p = &self.params;
            let env = p.envelope(ATTACK);
            let filter_env = p.envelope(FILTER_ATTACK);
            let lfo = (TAU * self.lfo_phase).sin() * self.lfo_shape.weights[0]
                + (1.0 - 4.0 * (self.lfo_phase - 0.5).abs()) * self.lfo_shape.weights[1]
                + (if self.lfo_phase < 0.5 { 1.0 } else { -1.0 }) * self.lfo_shape.weights[2];
            self.lfo_phase = (self.lfo_phase + p.get(LFO_RATE) / f64::from(SAMPLE_RATE)).fract();
            let pitch_modulation = (lfo * p.get(LFO_PITCH) / 1200.0).exp2();
            let mut tuning = [0.0; 3];
            let mut levels = [[0.0; 2]; 3];
            for osc in 0..3 {
                let base = OSC_BASE + osc as u32 * OSC_STRIDE;
                tuning[osc] = ((p.get(base + OSC_COARSE) + p.get(base + OSC_FINE) / 100.0) / 12.0)
                    .exp2()
                    * pitch_modulation
                    / f64::from(SAMPLE_RATE);
                let position = pan(p.get(base + OSC_PAN));
                let level = amplitude(p.get(base + OSC_LEVEL));
                levels[osc] = [position[0] * level, position[1] * level];
            }
            let drive = amplitude(p.get(FILTER_DRIVE));
            let matrix_active = self
                .matrix_sources
                .iter()
                .enumerate()
                .any(|(route, source)| {
                    p.get(MATRIX_BASE + route as u32 * MATRIX_STRIDE + MATRIX_AMOUNT) != 0.0
                        && source.weights[1..].iter().any(|weight| *weight > 0.0)
                });
            let mut mixed = [0.0; 2];
            for voice in &mut self.voices {
                if voice.amp.stage == Stage::Silent {
                    continue;
                }
                let envelope = voice.amp.next(env);
                let modulation = voice.modulation.next(filter_env);
                let matrix = if matrix_active {
                    matrix_displacements(
                        p,
                        &self.matrix_sources,
                        &self.matrix_targets,
                        envelope,
                        modulation,
                        lfo,
                    )
                } else {
                    [0.0; 17]
                };
                let mut audio = [0.0; 2];
                for osc in 0..3 {
                    let base = OSC_BASE + osc as u32 * OSC_STRIDE;
                    let pitch = (matrix[0] + matrix[1 + osc]).clamp(-96.0, 96.0);
                    let pitch_ratio = if pitch == 0.0 {
                        1.0
                    } else {
                        (pitch / 12.0).exp2()
                    };
                    let raw_step = voice.frequency * tuning[osc] * pitch_ratio;
                    let step = raw_step.min(0.49);
                    let width =
                        (p.get(base + OSC_PULSE_WIDTH) + matrix[11 + osc]).clamp(0.05, 0.95);
                    let sample = if raw_step >= 0.49 {
                        0.0
                    } else {
                        self.oscillator_shapes[osc]
                            .weights
                            .iter()
                            .enumerate()
                            .filter(|(_, weight)| **weight > 0.0)
                            .map(|(shape, weight)| {
                                weight * oscillator(shape as u32, voice.phases[osc], step, width)
                            })
                            .sum::<f64>()
                            * ((0.49 - raw_step) / 0.07).clamp(0.0, 1.0)
                    };
                    voice.phases[osc] = (voice.phases[osc] + step).fract();
                    let osc_levels = if matrix[4 + osc] != 0.0 || matrix[8 + osc] != 0.0 {
                        let level = amplitude(
                            (p.get(base + OSC_LEVEL) + matrix[4 + osc]).clamp(-60.0, 0.0),
                        );
                        let position =
                            pan((p.get(base + OSC_PAN) + matrix[8 + osc]).clamp(-1.0, 1.0));
                        [position[0] * level, position[1] * level]
                    } else {
                        levels[osc]
                    };
                    audio[0] += sample * osc_levels[0];
                    audio[1] += sample * osc_levels[1];
                }
                if self.control_counter == 0 {
                    let octaves = modulation * p.get(FILTER_ENVELOPE)
                        + (f64::from(voice.key) - 60.0) / 12.0 * p.get(FILTER_KEYTRACK)
                        + lfo * p.get(LFO_FILTER)
                        + matrix[14];
                    let resonance = (p.get(FILTER_RESONANCE)
                        * matrix[15].clamp(-16.0, 16.0).exp2())
                    .clamp(0.5, 12.0);
                    voice.filter.coefficients(
                        p.get(FILTER_CUTOFF) * octaves.clamp(-16.0, 16.0).exp2(),
                        resonance,
                    );
                }
                let saturation_mix = (p.get(FILTER_DRIVE) / 6.0).clamp(0.0, 1.0);
                if saturation_mix > 0.0 {
                    for sample in &mut audio {
                        *sample += ((*sample * drive).tanh() - *sample) * saturation_mix;
                    }
                }
                let filtered = voice
                    .filter
                    .process_blended(audio, self.filter_mode.weights);
                let mut level = envelope * velocity_gain(voice.velocity, p.get(VELOCITY));
                let voice_pan = if matrix_active {
                    level *= amplitude((p.get(GAIN) + matrix[16]).clamp(-60.0, 6.0));
                    balance((p.get(PAN) + matrix[7]).clamp(-1.0, 1.0))
                } else {
                    [1.0; 2]
                };
                for channel in 0..2 {
                    mixed[channel] += filtered[channel] * level * voice_pan[channel];
                }
            }
            self.control_counter = (self.control_counter + 1) % 16;
            for shape in &mut self.oscillator_shapes {
                shape.advance();
            }
            self.filter_mode.advance();
            self.lfo_shape.advance();
            for source in &mut self.matrix_sources {
                source.advance();
            }
            for target in &mut self.matrix_targets {
                target.advance();
            }
            let output_gain = if matrix_active {
                1.0
            } else {
                amplitude(p.get(GAIN))
            };
            let output_pan = if matrix_active {
                [1.0; 2]
            } else {
                balance(p.get(PAN))
            };
            *frame = [
                (mixed[0] * output_gain * output_pan[0]) as f32,
                (mixed[1] * output_gain * output_pan[1]) as f32,
            ];
        }
    }
}

#[cfg(test)]
mod tests;
