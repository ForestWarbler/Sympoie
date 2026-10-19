//! Rain: a random arpeggiator with its own soft synthesizer.
//!
//! Notes are triggered by a generator rather than played: it fires at a controllable rate,
//! free-running or locked to the tempo, from perfectly regular to Poisson-random. Every
//! trigger strikes one of the notes currently held — at random, shuffled so a note never
//! repeats, or cycling up or down — optionally spread over extra octaves. With a single
//! note held every trigger plays that note; with a chord it walks the chord.
//!
//! The voice is built for soft, dreamy analog and acoustic tones: an antialiased
//! oscillator pair with analog-style detune and slow pitch drift, an inharmonic overtone
//! partial for bell and kalimba colours, a short noise transient, a chirp at the onset
//! like a droplet in a bucket, a resonant low-pass filter swept by the amplitude envelope,
//! and a small plate-style reverb. Presets cover the range, and the default settings are
//! the rain drum the device is named after.
//!
//! Randomness comes from a seeded generator that restarts at every reset, so renders and
//! bounces are repeatable.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::Frame;
use crate::instrument::NoteEvent;
use crate::plugin::{PluginInstance, PluginProcessContext, PluginProcessor};

use super::effects::{RATE, parameter};
use super::instruments::{Filter, oscillator};
use super::{ParameterSpec, Scale, Telemetry};

pub const GAIN: u32 = 0;
pub const VELOCITY: u32 = 2;
pub const DROP_RATE: u32 = 10;
pub const JITTER: u32 = 11;
pub const SPREAD: u32 = 12;
/// Shares the id the host reads as an instrument's release tail.
pub const DECAY: u32 = 13;
pub const VARIATION: u32 = 14;
pub const ORDER: u32 = 15;
pub const OCTAVES: u32 = 16;
pub const SYNC: u32 = 17;
pub const DIVISION: u32 = 18;
pub const ATTACK: u32 = 20;
pub const SHAPE: u32 = 21;
pub const DETUNE: u32 = 22;
pub const OVERTONE: u32 = 23;
pub const NOISE: u32 = 24;
pub const CHIRP: u32 = 25;
pub const DRIFT: u32 = 26;
pub const TRANSPOSE: u32 = 27;
pub const CUTOFF: u32 = 30;
pub const RESONANCE: u32 = 31;
pub const FILTER_ENV: u32 = 32;
pub const SPACE: u32 = 40;
pub const TONE: u32 = 41;

pub const ORDERS: [&str; 4] = ["Random", "Shuffle", "Up", "Down"];
pub const DIVISIONS: [&str; 7] = ["1/1", "1/2", "1/4", "1/8", "1/16", "1/8T", "1/16T"];
/// Beats per step for each entry of `DIVISIONS`.
const DIVISION_BEATS: [f64; 7] = [4.0, 2.0, 1.0, 0.5, 0.25, 1.0 / 3.0, 1.0 / 6.0];

pub const PARAMETER_COUNT: usize = 24;

const PARAMETERS: [ParameterSpec; PARAMETER_COUNT] = [
    parameter(GAIN, "Gain", "dB", (-60.0, 6.0), -9.0, Scale::Linear, 0),
    parameter(VELOCITY, "Velocity", "", (0.0, 1.0), 0.5, Scale::Linear, 0),
    parameter(DROP_RATE, "Rate", "Hz", (0.2, 60.0), 5.0, Scale::Log, 0),
    parameter(JITTER, "Jitter", "%", (0.0, 100.0), 70.0, Scale::Linear, 0),
    parameter(SPREAD, "Spread", "%", (0.0, 100.0), 60.0, Scale::Linear, 0),
    parameter(DECAY, "Decay", "s", (0.02, 8.0), 1.2, Scale::Log, 0),
    parameter(
        VARIATION,
        "Variation",
        "%",
        (0.0, 100.0),
        35.0,
        Scale::Linear,
        0,
    ),
    parameter(ORDER, "Order", "", (0.0, 3.0), 0.0, Scale::Linear, 3),
    parameter(OCTAVES, "Octaves", "", (0.0, 3.0), 0.0, Scale::Linear, 3),
    parameter(SYNC, "Sync", "", (0.0, 1.0), 0.0, Scale::Linear, 1),
    parameter(DIVISION, "Division", "", (0.0, 6.0), 3.0, Scale::Linear, 6),
    parameter(ATTACK, "Attack", "s", (0.0005, 2.0), 0.012, Scale::Log, 0),
    parameter(SHAPE, "Shape", "", (0.0, 3.0), 1.0, Scale::Linear, 3),
    parameter(DETUNE, "Detune", "cent", (0.0, 50.0), 8.0, Scale::Linear, 0),
    parameter(
        OVERTONE,
        "Overtone",
        "%",
        (0.0, 100.0),
        20.0,
        Scale::Linear,
        0,
    ),
    parameter(NOISE, "Noise", "%", (0.0, 100.0), 12.0, Scale::Linear, 0),
    parameter(CHIRP, "Chirp", "st", (-12.0, 12.0), 2.0, Scale::Linear, 0),
    parameter(DRIFT, "Drift", "cent", (0.0, 50.0), 6.0, Scale::Linear, 0),
    parameter(
        TRANSPOSE,
        "Transpose",
        "st",
        (-24.0, 24.0),
        0.0,
        Scale::Linear,
        48,
    ),
    parameter(
        CUTOFF,
        "Cutoff",
        "Hz",
        (100.0, 16000.0),
        2600.0,
        Scale::Log,
        0,
    ),
    parameter(RESONANCE, "Resonance", "Q", (0.5, 8.0), 0.8, Scale::Log, 0),
    parameter(
        FILTER_ENV,
        "Filter envelope",
        "oct",
        (-6.0, 6.0),
        1.5,
        Scale::Linear,
        0,
    ),
    parameter(SPACE, "Space", "%", (0.0, 100.0), 35.0, Scale::Linear, 0),
    parameter(TONE, "Tone", "Hz", (200.0, 16000.0), 9000.0, Scale::Log, 0),
];

pub fn parameters() -> Vec<ParameterSpec> {
    PARAMETERS.to_vec()
}

fn index(id: u32) -> usize {
    PARAMETERS
        .iter()
        .position(|spec| spec.id == id)
        .expect("known rain parameter")
}

/// A named set of settings; values not listed keep their defaults, so the first preset,
/// the rain drum, is the device's default state.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub values: &'static [(u32, f64)],
}

pub const PRESETS: [Preset; 8] = [
    Preset {
        name: "Rain drum",
        values: &[],
    },
    Preset {
        name: "Drizzle",
        values: &[
            (GAIN, -12.0),
            (DROP_RATE, 14.0),
            (JITTER, 90.0),
            (DECAY, 0.5),
            (VARIATION, 60.0),
            (ATTACK, 0.004),
            (SHAPE, 0.0),
            (NOISE, 25.0),
            (CHIRP, 4.0),
            (CUTOFF, 5000.0),
            (SPACE, 45.0),
        ],
    },
    Preset {
        name: "Music box",
        values: &[
            (DROP_RATE, 4.0),
            (JITTER, 30.0),
            (ORDER, 1.0),
            (OCTAVES, 1.0),
            (ATTACK, 0.002),
            (DECAY, 1.0),
            (SHAPE, 0.0),
            (DETUNE, 3.0),
            (OVERTONE, 55.0),
            (NOISE, 8.0),
            (CHIRP, 0.0),
            (TRANSPOSE, 12.0),
            (CUTOFF, 8000.0),
            (FILTER_ENV, 0.0),
            (SPACE, 30.0),
        ],
    },
    Preset {
        name: "Kalimba",
        values: &[
            (ATTACK, 0.003),
            (DECAY, 0.7),
            (SHAPE, 1.0),
            (DETUNE, 2.0),
            (OVERTONE, 40.0),
            (NOISE, 18.0),
            (CHIRP, 1.0),
            (DRIFT, 3.0),
            (CUTOFF, 3500.0),
            (RESONANCE, 0.7),
            (FILTER_ENV, 2.0),
            (SPACE, 25.0),
        ],
    },
    Preset {
        name: "Felt piano",
        values: &[
            (DROP_RATE, 3.0),
            (JITTER, 50.0),
            (ORDER, 1.0),
            (ATTACK, 0.02),
            (DECAY, 1.8),
            (SHAPE, 1.0),
            (DETUNE, 4.0),
            (OVERTONE, 10.0),
            (NOISE, 6.0),
            (CHIRP, 0.0),
            (DRIFT, 4.0),
            (CUTOFF, 1800.0),
            (FILTER_ENV, 1.2),
            (SPACE, 40.0),
        ],
    },
    Preset {
        name: "Analog pad drops",
        values: &[
            (DROP_RATE, 1.5),
            (JITTER, 40.0),
            (OCTAVES, 1.0),
            (ATTACK, 0.25),
            (DECAY, 2.5),
            (SHAPE, 2.0),
            (DETUNE, 14.0),
            (OVERTONE, 0.0),
            (NOISE, 0.0),
            (CHIRP, 0.0),
            (DRIFT, 10.0),
            (CUTOFF, 900.0),
            (RESONANCE, 1.2),
            (FILTER_ENV, 2.5),
            (SPACE, 60.0),
        ],
    },
    Preset {
        name: "Glass bells",
        values: &[
            (DROP_RATE, 2.5),
            (JITTER, 80.0),
            (OCTAVES, 2.0),
            (ATTACK, 0.005),
            (DECAY, 2.5),
            (SHAPE, 0.0),
            (DETUNE, 5.0),
            (OVERTONE, 70.0),
            (NOISE, 4.0),
            (CHIRP, -2.0),
            (TRANSPOSE, 12.0),
            (CUTOFF, 12000.0),
            (FILTER_ENV, 0.0),
            (SPACE, 55.0),
        ],
    },
    Preset {
        name: "Synced pluck",
        values: &[
            (JITTER, 0.0),
            (ORDER, 2.0),
            (OCTAVES, 1.0),
            (SYNC, 1.0),
            (DIVISION, 4.0),
            (ATTACK, 0.001),
            (DECAY, 0.3),
            (SHAPE, 2.0),
            (DETUNE, 6.0),
            (OVERTONE, 0.0),
            (NOISE, 10.0),
            (CHIRP, 0.0),
            (DRIFT, 2.0),
            (CUTOFF, 2400.0),
            (RESONANCE, 1.5),
            (FILTER_ENV, 3.0),
            (SPACE, 20.0),
        ],
    },
];

/// Every parameter of a preset as `(id, normalized value)`.
pub fn preset_parameters(preset: usize) -> Option<Vec<(u32, f64)>> {
    let preset = PRESETS.get(preset)?;
    Some(
        PARAMETERS
            .iter()
            .map(|spec| {
                let physical = preset
                    .values
                    .iter()
                    .find(|(id, _)| *id == spec.id)
                    .map_or(spec.default, |(_, value)| *value);
                (spec.id, spec.normalize(physical))
            })
            .collect(),
    )
}

/// The preset whose settings the instance currently holds, if any.
pub fn matching_preset(instance: &PluginInstance) -> Option<usize> {
    (0..PRESETS.len()).find(|&preset| {
        preset_parameters(preset).is_some_and(|values| {
            values.iter().all(|(id, value)| {
                instance
                    .parameters
                    .iter()
                    .find(|parameter| parameter.id == *id)
                    .is_some_and(|parameter| (parameter.value - value).abs() < 1.0e-6)
            })
        })
    })
}

/// Seconds the host should add to the decay tail: the attack, and the reverb when it is on.
pub fn extra_tail_seconds(instance: &PluginInstance) -> f64 {
    let attack = super::value(instance, ATTACK);
    let space = super::value(instance, SPACE);
    attack + if space > 0.0 { REVERB_TAIL } else { 0.0 }
}

const VOICES: usize = 48;
const HELD: usize = 32;
/// Seconds the chirp takes to settle on the note.
const CHIRP_SECONDS: f64 = 0.018;
/// Seconds the noise transient takes to fall to −60 dB.
const NOISE_SECONDS: f64 = 0.045;
/// Ratio of the overtone partial to the fundamental.
const OVERTONE_RATIO: f64 = 2.32;
/// Seconds the reverb takes to fall to −60 dB at full Space.
const REVERB_TAIL: f64 = 2.5;
/// Samples between drift target changes.
const DRIFT_PERIOD: f64 = 2400.0;
/// Keys outside this range still sound but share the edges of the stereo field.
const LOWEST_KEY: f64 = 24.0;
const HIGHEST_KEY: f64 = 108.0;

#[derive(Debug, Clone, Copy)]
struct Rng(u64);

impl Rng {
    const SEED: u64 = 0x9e37_79b9_7f4a_7c15;
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in the half-open interval (0, 1].
    fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    }
    fn below(&mut self, count: usize) -> usize {
        (self.next_u64() % count as u64) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Held {
    id: u64,
    key: u8,
    velocity: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Attack,
    Decay,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    /// Fundamental in cycles per sample once the chirp has settled.
    step: f64,
    phases: [f64; 2],
    /// Frequency ratios of the two oscillators (one sharp, one flat).
    detune: [f64; 2],
    shape: u32,
    overtone_phase: f64,
    /// Remaining chirp, in samples, counting down.
    chirp: f64,
    chirp_depth: f64,
    stage: Stage,
    /// Amplitude envelope, 0 to 1.
    envelope: f64,
    attack_step: f64,
    decay_gain: f64,
    peak: f64,
    overtone: f64,
    overtone_gain: f64,
    noise: f64,
    noise_gain: f64,
    noise_state: f64,
    /// Current and target pitch drift, as frequency ratios.
    drift: f64,
    drift_target: f64,
    drift_countdown: f64,
    drift_cents: f64,
    filter: Filter,
    counter: u32,
    pan: [f64; 2],
}

impl Voice {
    const SILENT: Self = Self {
        step: 0.0,
        phases: [0.0; 2],
        detune: [1.0; 2],
        shape: 0,
        overtone_phase: 0.0,
        chirp: 0.0,
        chirp_depth: 1.0,
        stage: Stage::Decay,
        envelope: 0.0,
        attack_step: 0.0,
        decay_gain: 0.0,
        peak: 0.0,
        overtone: 0.0,
        overtone_gain: 0.0,
        noise: 0.0,
        noise_gain: 0.0,
        noise_state: 0.0,
        drift: 1.0,
        drift_target: 1.0,
        drift_countdown: 0.0,
        drift_cents: 0.0,
        filter: Filter::EMPTY,
        counter: 0,
        pan: [0.0; 2],
    };
    fn level(&self) -> f64 {
        self.envelope * self.peak + self.overtone + self.noise
    }
    fn silent(&self) -> bool {
        self.stage == Stage::Decay && self.level() < 1.0e-6
    }
}

/// Per-sample multiplier that reaches −60 dB after `seconds`.
fn decay_coefficient(seconds: f64) -> f64 {
    (-(1000.0_f64.ln()) / (seconds.max(1.0e-4) * RATE)).exp()
}

fn amplitude(db: f64) -> f64 {
    if db <= -60.0 {
        0.0
    } else {
        10.0_f64.powf(db / 20.0)
    }
}

fn cents(cents: f64) -> f64 {
    (cents / 1200.0).exp2()
}

/// Horizontal position of a key across the stereo field, 0 at the left.
pub fn key_position(key: u8) -> f64 {
    ((f64::from(key) - LOWEST_KEY) / (HIGHEST_KEY - LOWEST_KEY)).clamp(0.0, 1.0)
}

fn pan(position: f64) -> [f64; 2] {
    let angle = position.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2;
    [angle.cos(), angle.sin()]
}

#[derive(Debug, Clone)]
struct Comb {
    buffer: Vec<f64>,
    index: usize,
    store: f64,
}

impl Comb {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length],
            index: 0,
            store: 0.0,
        }
    }
    fn process(&mut self, input: f64, feedback: f64, damping: f64) -> f64 {
        let output = self.buffer[self.index];
        self.store += (output - self.store) * (1.0 - damping);
        self.buffer[self.index] = input + self.store * feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }
    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
        self.store = 0.0;
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buffer: Vec<f64>,
    index: usize,
}

impl Allpass {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length],
            index: 0,
        }
    }
    fn process(&mut self, input: f64) -> f64 {
        let delayed = self.buffer[self.index];
        self.buffer[self.index] = input + delayed * 0.5;
        self.index = (self.index + 1) % self.buffer.len();
        delayed - input
    }
    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
    }
}

/// A small plate-style reverb: four damped combs and two allpasses per channel, with the
/// right channel's delays offset for width.
#[derive(Debug, Clone)]
struct Reverb {
    combs: [[Comb; 4]; 2],
    allpasses: [[Allpass; 2]; 2],
}

impl Reverb {
    const COMBS: [usize; 4] = [1215, 1293, 1390, 1476];
    const ALLPASSES: [usize; 2] = [605, 480];
    const STEREO: usize = 23;
    fn new() -> Self {
        Self {
            combs: std::array::from_fn(|channel| {
                std::array::from_fn(|i| Comb::new(Self::COMBS[i] + channel * Self::STEREO))
            }),
            allpasses: std::array::from_fn(|channel| {
                std::array::from_fn(|i| Allpass::new(Self::ALLPASSES[i] + channel * Self::STEREO))
            }),
        }
    }
    fn process(&mut self, input: [f64; 2], feedback: f64) -> [f64; 2] {
        let mono = (input[0] + input[1]) * 0.5;
        std::array::from_fn(|channel| {
            let mut sum = 0.0;
            for comb in &mut self.combs[channel] {
                sum += comb.process(mono, feedback, 0.3);
            }
            let mut wet = sum * 0.25;
            for allpass in &mut self.allpasses[channel] {
                wet = allpass.process(wet);
            }
            wet
        })
    }
    fn clear(&mut self) {
        for channel in 0..2 {
            for comb in &mut self.combs[channel] {
                comb.clear();
            }
            for allpass in &mut self.allpasses[channel] {
                allpass.clear();
            }
        }
    }
}

#[derive(Debug)]
pub struct Rain {
    target: [f64; PARAMETER_COUNT],
    current: [f64; PARAMETER_COUNT],
    held: Vec<Held>,
    voices: [Voice; VOICES],
    rng: Rng,
    /// Samples until the next trigger, when free-running.
    countdown: f64,
    /// Position inside the current step when locked to the tempo grid.
    grid_phase: Option<f64>,
    /// Steps through the chord for the Up and Down orders.
    cursor: usize,
    /// The key the last trigger played, so Shuffle can avoid it.
    last_key: Option<u8>,
    reverb: Reverb,
    lowpass: [f64; 2],
    telemetry: Option<Arc<Telemetry>>,
}

pub fn create_rain(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    instance.validate().map_err(|error| error.to_string())?;
    Ok(Box::new(Rain::new(instance, telemetry)))
}

impl Rain {
    pub fn new(instance: &PluginInstance, telemetry: Option<Arc<Telemetry>>) -> Self {
        let target = super::effects::physical_values(instance, &PARAMETERS);
        let rain = Self {
            target,
            current: target,
            held: Vec::with_capacity(HELD),
            voices: [Voice::SILENT; VOICES],
            rng: Rng(Rng::SEED),
            countdown: 0.0,
            grid_phase: None,
            cursor: 0,
            last_key: None,
            reverb: Reverb::new(),
            lowpass: [0.0; 2],
            telemetry,
        };
        rain.publish_held();
        rain
    }

    fn get(&self, id: u32) -> f64 {
        self.current[index(id)]
    }

    fn publish_held(&self) {
        if let Some(telemetry) = &self.telemetry {
            let mut mask = [0_u64; 2];
            for held in &self.held {
                mask[usize::from(held.key / 64)] |= 1 << (held.key % 64);
            }
            for (slot, bits) in telemetry.rain_held.iter().zip(mask) {
                slot.store(bits, Ordering::Relaxed);
            }
        }
    }

    /// Samples per step before jitter.
    fn step_samples(&self, bpm: f64) -> f64 {
        if self.get(SYNC) >= 0.5 {
            let beats = DIVISION_BEATS[(self.get(DIVISION).round().max(0.0) as usize).min(6)];
            let bpm = if bpm.is_finite() && bpm > 1.0 {
                bpm
            } else {
                120.0
            };
            beats * 60.0 / bpm * RATE
        } else {
            RATE / self.get(DROP_RATE).max(0.01)
        }
    }

    fn interval(&mut self, bpm: f64) -> f64 {
        let jitter = (self.get(JITTER) / 100.0).clamp(0.0, 1.0);
        // Blends a metronome with a Poisson process: the exponential distribution has the
        // same mean, so the average rate holds at every jitter setting.
        let random = -self.rng.unit().ln();
        self.step_samples(bpm) * ((1.0 - jitter) + jitter * random)
    }

    /// Chooses the held note and octave the next trigger plays.
    fn pick(&mut self) -> Option<(Held, u8)> {
        if self.held.is_empty() {
            return None;
        }
        let octaves = self.get(OCTAVES).round().max(0.0) as u8;
        let order = self.get(ORDER).round().max(0.0) as usize;
        let choice = match order {
            2 | 3 => {
                let mut keys = self.held.clone();
                keys.sort_by_key(|held| held.key);
                if order == 3 {
                    keys.reverse();
                }
                let count = keys.len() * usize::from(octaves + 1);
                let cursor = self.cursor % count;
                self.cursor = (cursor + 1) % count;
                (keys[cursor % keys.len()], (cursor / keys.len()) as u8)
            }
            _ => {
                let count = self.held.len() * usize::from(octaves + 1);
                let mut choice = self.rng.below(count);
                if order == 1 && count > 1 {
                    let key_of = |choice: usize| {
                        self.held[choice % self.held.len()].key
                            + 12 * (choice / self.held.len()) as u8
                    };
                    for _ in 0..8 {
                        if self.last_key != Some(key_of(choice)) {
                            break;
                        }
                        choice = self.rng.below(count);
                    }
                }
                (
                    self.held[choice % self.held.len()],
                    (choice / self.held.len()) as u8,
                )
            }
        };
        Some(choice)
    }

    fn trigger(&mut self) {
        let Some((held, octave)) = self.pick() else {
            return;
        };
        let sounding = held.key.saturating_add(12 * octave).min(127);
        self.last_key = Some(sounding);
        let variation = (self.get(VARIATION) / 100.0).clamp(0.0, 1.0);
        let strength = 1.0 - variation * self.rng.unit() * 0.85;
        let velocity =
            1.0 - self.get(VELOCITY) + self.get(VELOCITY) * f64::from(held.velocity) / 127.0;
        let position = key_position(sounding) + variation * (self.rng.unit() - 0.5) * 0.08;
        let position = position.clamp(0.0, 1.0);
        let spread = (self.get(SPREAD) / 100.0).clamp(0.0, 1.0);
        let stereo = 0.5 + (position - 0.5) * spread;
        let hz = 440.0
            * ((f64::from(sounding) + self.get(TRANSPOSE) - 69.0) / 12.0).exp2()
            * cents(variation * (self.rng.unit() - 0.5) * 24.0);
        let decay = self.get(DECAY) * (1.0 - variation * self.rng.unit() * 0.5);
        let level = strength * velocity;
        let detune = self.get(DETUNE) * 0.5;
        let slot = self
            .voices
            .iter()
            .position(Voice::silent)
            .unwrap_or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.level().total_cmp(&b.level()))
                    .map_or(0, |(slot, _)| slot)
            });
        let drift = self.get(DRIFT);
        let mut voice = Voice {
            step: (hz / RATE).min(0.45),
            phases: [self.rng.unit(), self.rng.unit()],
            detune: [cents(detune), cents(-detune)],
            shape: self.get(SHAPE).round().max(0.0) as u32,
            chirp: CHIRP_SECONDS * RATE,
            chirp_depth: (-self.get(CHIRP) / 12.0).exp2(),
            stage: Stage::Attack,
            attack_step: 1.0 / (self.get(ATTACK) * RATE).max(1.0),
            decay_gain: decay_coefficient(decay),
            peak: level,
            overtone: level * self.get(OVERTONE) / 100.0 * 0.6,
            overtone_gain: decay_coefficient(decay * 0.35),
            noise: level * self.get(NOISE) / 100.0 * 0.5,
            noise_gain: decay_coefficient(NOISE_SECONDS),
            drift: cents(drift * (self.rng.unit() - 0.5) * 2.0),
            drift_target: cents(drift * (self.rng.unit() - 0.5) * 2.0),
            drift_countdown: DRIFT_PERIOD,
            drift_cents: drift,
            pan: pan(stereo),
            ..Voice::SILENT
        };
        voice
            .filter
            .coefficients(self.get(CUTOFF), self.get(RESONANCE));
        self.voices[slot] = voice;
        if let Some(telemetry) = &self.telemetry {
            telemetry.rain_drops.push(RainDrop {
                position: position as f32,
                key: held.key,
                intensity: level.clamp(0.0, 1.0) as f32,
            });
        }
    }
}

/// One trigger as reported to the interface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RainDrop {
    /// Where it landed, 0 at the left edge.
    pub position: f32,
    /// The held key it struck (before any octave shift).
    pub key: u8,
    pub intensity: f32,
}

pub const DROP_LOG: usize = 64;

/// Lock-free log of the most recent triggers, written by the audio thread and read by the
/// interface, which animates only the entries it has not yet seen.
#[derive(Debug)]
pub struct DropLog {
    entries: Box<[[std::sync::atomic::AtomicU32; 3]]>,
    written: std::sync::atomic::AtomicU32,
}

impl Default for DropLog {
    fn default() -> Self {
        Self {
            entries: (0..DROP_LOG)
                .map(|_| std::array::from_fn(|_| std::sync::atomic::AtomicU32::new(0)))
                .collect(),
            written: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

impl DropLog {
    pub fn push(&self, drop: RainDrop) {
        let written = self.written.load(Ordering::Relaxed);
        let slot = &self.entries[written as usize % DROP_LOG];
        slot[0].store(drop.position.to_bits(), Ordering::Relaxed);
        slot[1].store(u32::from(drop.key), Ordering::Relaxed);
        slot[2].store(drop.intensity.to_bits(), Ordering::Relaxed);
        self.written
            .store(written.wrapping_add(1), Ordering::Release);
    }

    pub fn written(&self) -> u32 {
        self.written.load(Ordering::Acquire)
    }

    /// Drops logged since `seen` entries had been written, oldest first; at most the log's
    /// length.
    pub fn since(&self, seen: u32) -> Vec<RainDrop> {
        let written = self.written();
        let count = (written.wrapping_sub(seen) as usize).min(DROP_LOG);
        (0..count)
            .map(|age| {
                let slot =
                    &self.entries[written.wrapping_sub((count - age) as u32) as usize % DROP_LOG];
                RainDrop {
                    position: f32::from_bits(slot[0].load(Ordering::Relaxed)),
                    key: slot[1].load(Ordering::Relaxed).min(127) as u8,
                    intensity: f32::from_bits(slot[2].load(Ordering::Relaxed)),
                }
            })
            .collect()
    }
}

impl PluginProcessor for Rain {
    fn reset(&mut self) {
        self.voices.fill(Voice::SILENT);
        self.held.clear();
        self.current = self.target;
        self.rng = Rng(Rng::SEED);
        self.countdown = 0.0;
        self.grid_phase = None;
        self.cursor = 0;
        self.last_key = None;
        self.reverb.clear();
        self.lowpass = [0.0; 2];
        self.publish_held();
    }

    fn set_parameter(&mut self, id: u32, value: f64) {
        if !value.is_finite() {
            return;
        }
        let Some(position) = PARAMETERS.iter().position(|spec| spec.id == id) else {
            return;
        };
        self.target[position] = PARAMETERS[position].denormalize(value.clamp(0.0, 1.0));
    }

    fn event(&mut self, event: NoteEvent) {
        match event {
            NoteEvent::PitchBend { .. } => {}
            NoteEvent::On {
                id, key, velocity, ..
            } => {
                self.held.retain(|held| held.id != id);
                if self.held.len() == HELD {
                    self.held.remove(0);
                }
                self.held.push(Held { id, key, velocity });
                // The first note of a phrase sounds at once rather than after a full step.
                if self.held.len() == 1 {
                    self.countdown = self.countdown.min(self.interval(120.0) * 0.25);
                    self.cursor = 0;
                }
            }
            NoteEvent::Off { id } => self.held.retain(|held| held.id != id),
        }
        self.publish_held();
    }

    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        let smooth = super::effects::SMOOTH;
        for (offset, frame) in frames.iter_mut().enumerate() {
            for ((current, target), spec) in
                self.current.iter_mut().zip(self.target).zip(&PARAMETERS)
            {
                if spec.steps > 0 {
                    *current = target;
                } else {
                    *current += (target - *current) * smooth;
                }
            }
            // Locked to the tempo without jitter, steps land on the transport grid; otherwise
            // the scheduler free-runs from the last trigger.
            let on_grid = self.get(SYNC) >= 0.5 && context.playing && self.get(JITTER) < 0.5;
            if on_grid {
                let step = self.step_samples(context.bpm);
                let phase = (context.frame + offset as u64) as f64 % step;
                if self.grid_phase.is_some_and(|previous| phase < previous) {
                    self.trigger();
                }
                self.grid_phase = Some(phase);
                self.countdown = step - phase;
            } else {
                self.grid_phase = None;
                self.countdown -= 1.0;
                while self.countdown <= 0.0 {
                    self.trigger();
                    self.countdown += self.interval(context.bpm);
                }
            }
            let filter_env = self.get(FILTER_ENV);
            let cutoff = self.get(CUTOFF);
            let resonance = self.get(RESONANCE);
            let mut mixed = [0.0_f64; 2];
            for voice in &mut self.voices {
                if voice.silent() {
                    continue;
                }
                match voice.stage {
                    Stage::Attack => {
                        voice.envelope += voice.attack_step;
                        if voice.envelope >= 1.0 {
                            voice.envelope = 1.0;
                            voice.stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => voice.envelope *= voice.decay_gain,
                }
                let glide = if voice.chirp > 0.0 {
                    voice.chirp -= 1.0;
                    let t = voice.chirp / (CHIRP_SECONDS * RATE);
                    voice.chirp_depth.powf(t * t)
                } else {
                    1.0
                };
                voice.drift_countdown -= 1.0;
                if voice.drift_countdown <= 0.0 {
                    voice.drift_countdown = DRIFT_PERIOD;
                    voice.drift_target = cents(voice.drift_cents * (voice.noise_state - 0.5) * 2.0);
                }
                voice.drift += (voice.drift_target - voice.drift) / DRIFT_PERIOD;
                let step = voice.step * glide * voice.drift;
                let mut tone = 0.0;
                for osc in 0..2 {
                    let osc_step = (step * voice.detune[osc]).min(0.49);
                    tone += oscillator(voice.shape, voice.phases[osc], osc_step, 0.5);
                    voice.phases[osc] = (voice.phases[osc] + osc_step).fract();
                }
                tone *= 0.5 * voice.envelope * voice.peak;
                voice.overtone_phase = (voice.overtone_phase + step * OVERTONE_RATIO).fract();
                tone += (TAU * voice.overtone_phase).sin() * voice.overtone;
                voice.overtone *= voice.overtone_gain;
                // Cheap uniform noise for the transient, independent of the scheduler.
                voice.noise_state = (voice.noise_state * 16807.0 + 0.7013).fract();
                tone += (voice.noise_state * 2.0 - 1.0) * voice.noise;
                voice.noise *= voice.noise_gain;
                if voice.counter == 0 {
                    let octaves = (voice.envelope * filter_env).clamp(-10.0, 10.0);
                    voice
                        .filter
                        .coefficients(cutoff * octaves.exp2(), resonance);
                }
                voice.counter = (voice.counter + 1) % 8;
                let filtered = voice.filter.process([tone, 0.0], 0)[0];
                mixed[0] += filtered * voice.pan[0];
                mixed[1] += filtered * voice.pan[1];
            }
            let space = (self.get(SPACE) / 100.0).clamp(0.0, 1.0);
            if space > 0.0 {
                let wet = self.reverb.process(mixed, 0.72 + 0.14 * space);
                for channel in 0..2 {
                    mixed[channel] += wet[channel] * space * 0.9;
                }
            }
            let tone = 1.0 - (-TAU * self.get(TONE) / RATE).exp();
            let gain = amplitude(self.get(GAIN));
            for channel in 0..2 {
                self.lowpass[channel] += (mixed[channel] - self.lowpass[channel]) * tone;
                frame[channel] = (self.lowpass[channel] * gain) as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::BuiltinKind;

    fn context() -> PluginProcessContext {
        PluginProcessContext {
            frame: 0,
            bpm: 120.0,
            playing: true,
            quality: crate::plugin::RenderQuality::Realtime,
        }
    }

    fn on(id: u64, key: u8) -> NoteEvent {
        NoteEvent::On {
            id,
            key,
            velocity: 100,
            channel: 0,
            slide_frames: 0,
        }
    }

    fn set(instance: &mut PluginInstance, id: u32, physical: f64) {
        let spec = PARAMETERS[index(id)];
        instance
            .parameters
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap()
            .value = spec.normalize(physical);
    }

    fn render(rain: &mut Rain, frames: usize) -> Vec<Frame> {
        let mut audio = vec![[0.0; 2]; frames];
        rain.process(&mut audio, context());
        audio
    }

    /// A plain sine voice with no reverb, chirp, noise or variation.
    fn clean() -> PluginInstance {
        let mut instance = BuiltinKind::Rain.instance();
        set(&mut instance, JITTER, 0.0);
        set(&mut instance, VARIATION, 0.0);
        set(&mut instance, SHAPE, 0.0);
        set(&mut instance, DETUNE, 0.0);
        set(&mut instance, NOISE, 0.0);
        set(&mut instance, OVERTONE, 0.0);
        set(&mut instance, CHIRP, 0.0);
        set(&mut instance, DRIFT, 0.0);
        set(&mut instance, SPACE, 0.0);
        set(&mut instance, FILTER_ENV, 0.0);
        set(&mut instance, CUTOFF, 16000.0);
        set(&mut instance, TONE, 16000.0);
        set(&mut instance, ATTACK, 0.0005);
        instance
    }

    #[test]
    fn silent_without_held_notes_and_sounds_once_a_note_is_held() {
        let instance = BuiltinKind::Rain.instance();
        let telemetry = Arc::new(Telemetry::default());
        let mut rain = Rain::new(&instance, Some(Arc::clone(&telemetry)));
        let audio = render(&mut rain, 48_000);
        assert!(audio.iter().all(|frame| frame == &[0.0, 0.0]));
        assert_eq!(telemetry.rain_drops.written(), 0);
        rain.event(on(1, 60));
        let audio = render(&mut rain, 48_000);
        assert!(audio.iter().any(|frame| frame[0].abs() > 0.05));
        assert!(
            audio
                .iter()
                .all(|frame| frame[0].is_finite() && frame[0].abs() < 1.5)
        );
        assert!(telemetry.rain_drops.written() > 0);
        rain.event(NoteEvent::Off { id: 1 });
        let written = telemetry.rain_drops.written();
        render(&mut rain, 48_000);
        assert_eq!(telemetry.rain_drops.written(), written);
    }

    #[test]
    fn regular_rain_lands_at_the_set_rate_and_jitter_keeps_the_mean() {
        for jitter in [0.0, 100.0] {
            let mut instance = BuiltinKind::Rain.instance();
            set(&mut instance, DROP_RATE, 10.0);
            set(&mut instance, JITTER, jitter);
            let telemetry = Arc::new(Telemetry::default());
            let mut rain = Rain::new(&instance, Some(Arc::clone(&telemetry)));
            rain.event(on(1, 60));
            render(&mut rain, 48_000 * 60);
            let drops = telemetry.rain_drops.written() as f64;
            let expected = 600.0;
            let tolerance = if jitter == 0.0 { 2.0 } else { 90.0 };
            assert!(
                (drops - expected).abs() <= tolerance,
                "jitter {jitter}: {drops} drops"
            );
        }
    }

    #[test]
    fn chords_spread_drops_over_every_held_note_and_single_notes_always_hit() {
        let mut instance = BuiltinKind::Rain.instance();
        set(&mut instance, DROP_RATE, 30.0);
        let telemetry = Arc::new(Telemetry::default());
        let mut rain = Rain::new(&instance, Some(Arc::clone(&telemetry)));
        rain.event(on(1, 60));
        render(&mut rain, 48_000);
        let drops = telemetry.rain_drops.since(0);
        assert!(drops.len() >= 20);
        assert!(drops.iter().all(|drop| drop.key == 60));
        rain.event(on(2, 64));
        rain.event(on(3, 67));
        let seen = telemetry.rain_drops.written();
        render(&mut rain, 48_000 * 2);
        let drops = telemetry.rain_drops.since(seen);
        for key in [60, 64, 67] {
            let count = drops.iter().filter(|drop| drop.key == key).count();
            assert!(count >= 5, "key {key} hit {count} times of {}", drops.len());
        }
        assert!(drops.iter().all(|drop| [60, 64, 67].contains(&drop.key)));
        assert_eq!(
            telemetry
                .rain_held
                .iter()
                .map(|m| m.load(Ordering::Relaxed))
                .collect::<Vec<_>>(),
            vec![1_u64 << 60, (1_u64 << 0) | (1_u64 << 3)]
        );
    }

    fn struck_keys(instance: &PluginInstance, keys: &[u8], triggers: usize) -> Vec<u8> {
        let mut rain = Rain::new(instance, None);
        for (index, &key) in keys.iter().enumerate() {
            rain.event(on(index as u64 + 1, key));
        }
        (0..triggers)
            .map(|_| {
                let (held, octave) = rain.pick().unwrap();
                rain.last_key = Some(held.key + 12 * octave);
                held.key + 12 * octave
            })
            .collect()
    }

    #[test]
    fn orders_walk_the_chord_up_down_shuffled_or_at_random_over_octaves() {
        let mut instance = BuiltinKind::Rain.instance();
        set(&mut instance, ORDER, 2.0);
        set(&mut instance, OCTAVES, 1.0);
        assert_eq!(
            struck_keys(&instance, &[67, 60, 64], 8),
            [60, 64, 67, 72, 76, 79, 60, 64]
        );
        set(&mut instance, ORDER, 3.0);
        set(&mut instance, OCTAVES, 0.0);
        assert_eq!(struck_keys(&instance, &[67, 60, 64], 4), [67, 64, 60, 67]);
        set(&mut instance, ORDER, 1.0);
        let shuffled = struck_keys(&instance, &[60, 64], 200);
        assert!(shuffled.windows(2).all(|pair| pair[0] != pair[1]));
        set(&mut instance, ORDER, 0.0);
        set(&mut instance, OCTAVES, 2.0);
        let random = struck_keys(&instance, &[60], 300);
        for key in [60, 72, 84] {
            assert!(random.iter().filter(|&&k| k == key).count() > 50);
        }
        assert!(random.windows(2).any(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn tempo_sync_follows_the_division_and_lands_on_the_grid_while_playing() {
        let mut instance = BuiltinKind::Rain.instance();
        set(&mut instance, SYNC, 1.0);
        set(&mut instance, DIVISION, 3.0);
        set(&mut instance, JITTER, 0.0);
        let telemetry = Arc::new(Telemetry::default());
        let mut rain = Rain::new(&instance, Some(Arc::clone(&telemetry)));
        rain.event(on(1, 60));
        // Eighth notes at 120 bpm: 4 per second, 12,000 samples apart.
        let mut audio = vec![[0.0; 2]; 4800];
        let mut onsets = Vec::new();
        for block in 0..50 {
            rain.process(
                &mut audio,
                PluginProcessContext {
                    frame: block * 4800,
                    bpm: 120.0,
                    playing: true,
                    quality: crate::plugin::RenderQuality::Realtime,
                },
            );
            let count = telemetry.rain_drops.written() as usize;
            while onsets.len() < count {
                onsets.push(block);
            }
        }
        assert_eq!(onsets.len(), 19, "{onsets:?}");
        // Each trigger falls in the block containing its grid line.
        for (index, block) in onsets.iter().enumerate() {
            assert_eq!(*block, (index as u64 + 1) * 12_000 / 4800, "{onsets:?}");
        }
        // Stopped, the same settings free-run at the division's period.
        let log = Arc::new(Telemetry::default());
        let mut free = Rain::new(&instance, Some(Arc::clone(&log)));
        free.event(on(1, 60));
        free.process(
            &mut vec![[0.0; 2]; 48_000 * 10],
            PluginProcessContext {
                frame: 0,
                bpm: 60.0,
                playing: false,
                quality: crate::plugin::RenderQuality::Realtime,
            },
        );
        assert!(
            (18..=21).contains(&log.rain_drops.written()),
            "{}",
            log.rain_drops.written()
        );
    }

    #[test]
    fn each_trigger_plays_the_struck_note() {
        let mut instance = clean();
        set(&mut instance, DROP_RATE, 0.5);
        set(&mut instance, DECAY, 8.0);
        let mut rain = Rain::new(&instance, None);
        rain.event(on(1, 69));
        let audio = render(&mut rain, 48_000);
        let first = audio
            .iter()
            .position(|frame| frame[0].abs() > 0.0)
            .expect("a trigger lands within the first second");
        let window = &audio[first + 100..first + 24_100];
        let crossings = window
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        assert!((crossings as i32 - 220).abs() <= 2, "{crossings} crossings");
    }

    #[test]
    fn stereo_position_follows_pitch_and_spread() {
        let energy = |key: u8, spread: f64| {
            let mut instance = clean();
            set(&mut instance, SPREAD, spread);
            let mut rain = Rain::new(&instance, None);
            rain.event(on(1, key));
            let audio = render(&mut rain, 48_000);
            audio.iter().fold([0.0_f64; 2], |acc, frame| {
                [
                    acc[0] + f64::from(frame[0]).powi(2),
                    acc[1] + f64::from(frame[1]).powi(2),
                ]
            })
        };
        let low = energy(36, 100.0);
        let high = energy(96, 100.0);
        assert!(low[0] > low[1] * 4.0, "{low:?}");
        assert!(high[1] > high[0] * 4.0, "{high:?}");
        let mono = energy(36, 0.0);
        assert!((mono[0] / mono[1] - 1.0).abs() < 0.01, "{mono:?}");
    }

    #[test]
    fn renders_are_repeatable_and_independent_of_block_size() {
        let mut instance = BuiltinKind::Rain.instance();
        set(&mut instance, DROP_RATE, 20.0);
        let mut whole = Rain::new(&instance, None);
        whole.event(on(1, 60));
        whole.event(on(2, 67));
        let reference = render(&mut whole, 4800);
        let mut split = Rain::new(&instance, None);
        split.event(on(1, 60));
        split.event(on(2, 67));
        let mut pieces = Vec::new();
        for size in [1, 7, 64, 500, 4228] {
            pieces.extend(render(&mut split, size));
        }
        assert_eq!(reference, pieces);
        whole.reset();
        whole.event(on(1, 60));
        whole.event(on(2, 67));
        assert_eq!(render(&mut whole, 4800), reference);
    }

    #[test]
    fn decay_sets_the_tail_and_the_host_reads_it_with_attack_and_reverb() {
        let mut instance = clean();
        set(&mut instance, DROP_RATE, 0.2);
        set(&mut instance, DECAY, 0.1);
        let mut rain = Rain::new(&instance, None);
        rain.event(on(1, 60));
        let audio = render(&mut rain, 48_000);
        let first = audio.iter().position(|frame| frame[0].abs() > 0.0).unwrap();
        let peak = audio[first..first + 4800]
            .iter()
            .map(|frame| frame[0].abs())
            .fold(0.0_f32, f32::max);
        let later = audio[first + 9600..first + 14_400]
            .iter()
            .map(|frame| frame[0].abs())
            .fold(0.0_f32, f32::max);
        assert!(
            later < peak * 0.002,
            "peak {peak}, after two decays {later}"
        );
        let tail = crate::builtin::release_frames(&instance, &[]).unwrap() as f64;
        assert!((tail - (0.1 + 0.0005) * RATE).abs() <= 2.0, "{tail}");
        set(&mut instance, SPACE, 50.0);
        let tail = crate::builtin::release_frames(&instance, &[]).unwrap() as f64;
        assert!(
            (tail - (0.1 + 0.0005 + REVERB_TAIL) * RATE).abs() <= 2.0,
            "{tail}"
        );
    }

    #[test]
    fn reverb_rings_out_within_its_declared_tail() {
        let mut instance = clean();
        set(&mut instance, SPACE, 100.0);
        set(&mut instance, DECAY, 0.05);
        set(&mut instance, DROP_RATE, 0.2);
        let mut rain = Rain::new(&instance, None);
        rain.event(on(1, 60));
        // The first trigger lands on the first sample; the note is released right after.
        let mut audio = render(&mut rain, 1);
        rain.event(NoteEvent::Off { id: 1 });
        audio.extend(render(&mut rain, (REVERB_TAIL * RATE) as usize + 48_000));
        let first = audio.iter().position(|frame| frame[0].abs() > 0.0).unwrap();
        let level = |from: usize, to: usize| {
            audio[first + from..first + to]
                .iter()
                .map(|f| f[0].abs())
                .fold(0.0_f32, f32::max)
        };
        let peak = level(0, 4800);
        // The dry voice is gone after four decay times; what remains is the room.
        let wet = level(9600, 14_400);
        assert!(wet > peak * 0.02, "the reverb rings: {wet} of {peak}");
        let rest = level(
            (REVERB_TAIL * RATE) as usize,
            (REVERB_TAIL * RATE) as usize + 24_000,
        );
        assert!(
            rest < peak * 0.002,
            "silent after the tail: {rest} of {peak}"
        );
    }

    #[test]
    fn filter_envelope_opens_the_tone() {
        let brightness = |envelope: f64| {
            let mut instance = clean();
            set(&mut instance, SHAPE, 2.0);
            set(&mut instance, CUTOFF, 300.0);
            set(&mut instance, FILTER_ENV, envelope);
            set(&mut instance, DROP_RATE, 0.2);
            let mut rain = Rain::new(&instance, None);
            rain.event(on(1, 48));
            let audio = render(&mut rain, 24_000);
            audio
                .windows(2)
                .map(|pair| f64::from((pair[1][0] - pair[0][0]).abs()))
                .sum::<f64>()
                / audio
                    .iter()
                    .map(|f| f64::from(f[0].abs()))
                    .sum::<f64>()
                    .max(1e-9)
        };
        // Mean slope over mean level rises as the saw's edges come through the filter.
        let (open, closed) = (brightness(4.0), brightness(0.0));
        assert!(open > closed * 1.3, "open {open} closed {closed}");
    }

    #[test]
    fn presets_cover_every_parameter_and_the_default_state_is_the_rain_drum() {
        let instance = BuiltinKind::Rain.instance();
        assert_eq!(matching_preset(&instance), Some(0));
        assert_eq!(PRESETS[0].name, "Rain drum");
        for (preset_index, preset) in PRESETS.iter().enumerate() {
            let values = preset_parameters(preset_index).unwrap();
            assert_eq!(values.len(), PARAMETER_COUNT);
            for (id, physical) in preset.values {
                let spec = PARAMETERS[index(*id)];
                assert!(
                    (spec.min..=spec.max).contains(physical),
                    "{} {} out of range",
                    preset.name,
                    spec.name
                );
            }
            let mut loaded = instance.clone();
            for (id, value) in values {
                loaded
                    .parameters
                    .iter_mut()
                    .find(|p| p.id == id)
                    .unwrap()
                    .value = value;
            }
            assert_eq!(
                matching_preset(&loaded),
                Some(preset_index),
                "{}",
                preset.name
            );
            let mut rain = Rain::new(&loaded, None);
            rain.event(on(1, 60));
            rain.event(on(2, 64));
            let audio = render(&mut rain, 48_000 * 2);
            assert!(audio.iter().all(|f| f[0].is_finite() && f[0].abs() < 2.0));
            assert!(audio.iter().any(|f| f[0].abs() > 0.01), "{}", preset.name);
        }
        assert!(preset_parameters(PRESETS.len()).is_none());
        let mut edited = instance.clone();
        edited.parameters[0].value = 0.123;
        assert_eq!(matching_preset(&edited), None);
    }

    #[test]
    fn drop_log_reports_only_unseen_entries_and_wraps() {
        let log = DropLog::default();
        assert!(log.since(0).is_empty());
        for index in 0..100 {
            log.push(RainDrop {
                position: index as f32 / 100.0,
                key: index as u8,
                intensity: 1.0,
            });
        }
        let recent = log.since(90);
        assert_eq!(recent.len(), 10);
        assert_eq!(recent[0].key, 90);
        assert_eq!(recent[9].key, 99);
        assert_eq!(log.since(0).len(), DROP_LOG);
        assert_eq!(log.since(100).len(), 0);
    }
}
