//! The processor: the parameters gliding to their settings, the crossovers that split the
//! signal into bands, each band's chain and its dynamics, and the blend of the bands' sum
//! with the dry signal.

use std::sync::Arc;

use crate::band::{BandFrame, Lane, SHAPER_DELAY};
use crate::crossover::{Crossover, Slope};
use crate::curve::Curve;
use crate::dynamics::{self, BAND_ATTACK_MS, BAND_RELEASE_MS, Follower};
use crate::filters::{Biquad, Coefficients, Delay};
use crate::models::{BLOOM, FEEDBACK_GATE, FEEDBACK_LOOP, Model, tape_makeup};
use crate::modulation::{self, Sources};
use crate::oversampling::Oversampler;
use crate::params::{
    AUTO_GAIN, BANDS, COUNT, CROSSOVER_BASE, CROSSOVER_SLOPE, CROSSOVERS, ENVELOPE_ATTACK,
    ENVELOPE_RELEASE, LFO_RATE, LFO_SHAPE, MAX_BANDS, MIX, MOD_AMOUNT, MOD_BAND, MOD_SLOTS,
    MOD_SOURCE, MOD_TARGET, OUTPUT, PARAMETERS, Slot, TONE, amplitude, band_parameter, decibels,
    mod_parameter,
};
use crate::telemetry::{self, Telemetry};

/// How hard Calor may work: in real time, or rendering offline with no deadline, where
/// it runs the curve at a higher rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    #[default]
    Realtime,
    Offline,
}

/// The lowest rate the curve runs at in real time and offline, in Hz.
const REALTIME_RATE: f64 = 176_000.0;
const OFFLINE_RATE: f64 = 705_000.0;
const MAX_FACTOR: usize = 32;
/// Inputs are clamped far above full scale so the arithmetic stays finite.
const INPUT_LIMIT: f32 = 1.0e4;
/// Where Tone pivots, in Hz.
const TONE_PIVOT: f64 = 800.0;
/// How long the parameters take to glide to a new setting, and how long the meters hold
/// their peaks, in seconds.
const SMOOTH_SECONDS: f64 = 0.015;
const METER_SECONDS: f64 = 0.300;
/// Samples between updates of the filters' coefficients.
const FILTER_STRIDE: usize = 16;
/// Samples between updates of what the modulation does.
const CONTROL_STRIDE: usize = 16;
/// A gliding value this close to its target snaps to it.
const SNAP: f64 = 1.0e-9;
/// The narrowest a band may be: each crossover sits at least this far above the one below.
const CROSSOVER_GAP: f64 = 1.25;

/// A band's gliding values, by slot.
const GAIN: usize = 0;
const BIAS_POINT: usize = 1;
const KNEE: usize = 2;
const ODD_SHARE: usize = 3;
const EVEN_SHARE: usize = 4;
/// The odd part's gain at the reference level, which Auto gain divides by.
const REFERENCE: usize = 5;
const EMPHASIS_DB: usize = 6;
/// Frequencies glide as their natural logarithms, so evenly in pitch.
const EMPHASIS_PITCH: usize = 7;
const MIX_SHARE: usize = 8;
const LEVEL_GAIN: usize = 9;
const DYNAMICS: usize = 10;
const FEEDBACK: usize = 11;
const FEEDBACK_PITCH: usize = 12;
/// 1 while the band is heard, 0 while it is muted or another band is soloed.
const AUDIBLE: usize = 13;
const BAND_SLOTS: usize = 14;

/// The global gliding values, by slot.
const AUTO: usize = 0;
const GLOBAL_MIX: usize = 1;
const OUTPUT_GAIN: usize = 2;
const TONE_DB: usize = 3;
const CROSSOVER_PITCH: usize = 4;
const GLOBAL_SLOTS: usize = 4 + CROSSOVERS;

/// How a band treats the two channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channels {
    Stereo,
    Mid,
    Side,
}

/// The smallest power-of-two factor that takes `rate` to at least `target`.
fn factor_for(rate: f64, target: f64) -> usize {
    let mut factor = 1;
    while factor < MAX_FACTOR && rate * (factor as f64) < target {
        factor *= 2;
    }
    factor
}

/// Moves `current` one sample towards `targets`.
#[inline]
fn glide(current: &mut [f64], targets: &[f64], share: f64) {
    for (value, &target) in current.iter_mut().zip(targets) {
        let distance = target - *value;
        *value = if distance.abs() <= SNAP * target.abs().max(1.0) {
            target
        } else {
            *value + share * distance
        };
    }
}

/// A band's wet signal through its dynamics, its Mix with its own dry signal, its level
/// and its mute, each only where it does something.
#[inline]
fn finish(wet: f64, dry: f64, dynamics: f64, mix: f64, level: f64, audible: f64) -> f64 {
    let mut x = wet;
    if dynamics != 1.0 {
        x *= dynamics;
    }
    if mix != 1.0 {
        x = dry + mix * (x - dry);
    }
    if level != 1.0 {
        x *= level;
    }
    if audible != 1.0 {
        x *= audible;
    }
    x
}

/// One band's settings: where its gliding values are heading and where they are, and its
/// stepped settings.
#[derive(Debug, Clone)]
struct Band {
    targets: [f64; BAND_SLOTS],
    current: [f64; BAND_SLOTS],
    model: Model,
    channels: Channels,
    /// The curve the reference gain was last measured on, and that gain.
    measured: Option<(Curve, f64)>,
    /// The emphasis and feedback settings the filters were last built for.
    built: [f64; 3],
}

impl Band {
    fn new() -> Self {
        Self {
            targets: [0.0; BAND_SLOTS],
            current: [0.0; BAND_SLOTS],
            model: Model::Curve,
            channels: Channels::Stereo,
            measured: None,
            built: [f64::NAN; 3],
        }
    }
}

/// Peak input, output and band levels, held and falling back slowly.
#[derive(Debug, Default)]
struct Meters {
    input: f64,
    output: f64,
    bands: [f64; MAX_BANDS],
}

impl Meters {
    fn frame(&mut self, decay: f64, input: [f32; 2], output: [f32; 2]) {
        let peak = |frame: [f32; 2]| f64::from(frame[0].abs().max(frame[1].abs()));
        self.input = (self.input * decay).max(peak(input));
        self.output = (self.output * decay).max(peak(output));
    }

    fn band(&mut self, band: usize, decay: f64, level: f64) {
        self.bands[band] = (self.bands[band] * decay).max(level);
    }

    fn publish(&self, telemetry: &Telemetry) {
        telemetry::set(&telemetry.input_db, decibels(self.input) as f32);
        telemetry::set(&telemetry.output_db, decibels(self.output) as f32);
        for (cell, &level) in telemetry.band_db.iter().zip(&self.bands) {
            telemetry::set(cell, decibels(level) as f32);
        }
    }
}

/// Calor running at one sample rate on a stereo signal.
#[derive(Debug)]
pub struct Calor {
    rate: f64,
    /// The parameters as set, and as the modulation moves them.
    values: [f64; COUNT],
    effective: [f64; COUNT],
    bands: [Band; MAX_BANDS],
    global_targets: [f64; GLOBAL_SLOTS],
    global_current: [f64; GLOBAL_SLOTS],
    /// Bands in use.
    count: usize,
    slope: Slope,
    targets_stale: bool,
    /// Whether any modulation slot is at work.
    modulated: bool,
    /// The Tone and crossover settings the filters were last built for.
    tone_built: f64,
    crossovers_built: Option<([f64; CROSSOVERS], Slope)>,
    filter_countdown: usize,
    control_countdown: usize,
    quality: Quality,
    factors: [usize; 2],
    latency: usize,
    /// `lanes[band][channel]`.
    lanes: [[Lane; 2]; MAX_BANDS],
    crossovers: [Crossover; 2],
    tone: [Biquad; 2],
    /// The dry signal, delayed by Calor's latency.
    dry: [Delay; 2],
    /// Each band's envelope, for its dynamics, Tube's bloom and the feedback's gate.
    followers: [Follower; MAX_BANDS],
    sources: Sources,
    /// Per-sample share of the way a gliding value moves, and of the way a meter falls.
    smooth: f64,
    meter_decay: f64,
    telemetry: Option<Arc<Telemetry>>,
    meters: Meters,
}

impl Calor {
    /// Calor at `sample_rate` Hz with every parameter at its default, processing in real
    /// time. With `telemetry`, it publishes its meters there for an interface to draw.
    pub fn new(sample_rate: f64, telemetry: Option<Arc<Telemetry>>) -> Self {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let factors = [
            factor_for(rate, REALTIME_RATE),
            factor_for(rate, OFFLINE_RATE),
        ];
        let oversamplers = factors.map(|factor| Oversampler::new(rate, factor));
        // Both paths are padded to the same whole number of samples, so the latency a host
        // compensates holds whichever one runs, and the dry signal lines up with either.
        let latency = oversamplers
            .iter()
            .map(|oversampler| {
                (oversampler.delay() + SHAPER_DELAY) as f64 / oversampler.factor() as f64
            })
            .fold(0.0, f64::max)
            .ceil() as usize;
        let mut calor = Self {
            rate,
            values: crate::params::defaults(),
            effective: crate::params::defaults(),
            bands: std::array::from_fn(|_| Band::new()),
            global_targets: [0.0; GLOBAL_SLOTS],
            global_current: [0.0; GLOBAL_SLOTS],
            count: 1,
            slope: Slope::TwentyFour,
            targets_stale: true,
            modulated: false,
            tone_built: f64::NAN,
            crossovers_built: None,
            filter_countdown: 0,
            control_countdown: CONTROL_STRIDE,
            quality: Quality::Realtime,
            factors,
            latency,
            lanes: std::array::from_fn(|_| {
                std::array::from_fn(|_| Lane::new(rate, &oversamplers, latency))
            }),
            crossovers: [Crossover::new(rate), Crossover::new(rate)],
            tone: [Biquad::default(); 2],
            dry: [Delay::new(latency), Delay::new(latency)],
            followers: [Follower::new(rate, BAND_ATTACK_MS, BAND_RELEASE_MS); MAX_BANDS],
            sources: Sources::new(rate),
            smooth: 1.0 - (-1.0 / (rate * SMOOTH_SECONDS)).exp(),
            meter_decay: 1.0 - 1.0 / (rate * METER_SECONDS),
            telemetry,
            meters: Meters::default(),
        };
        calor.reset();
        calor
    }

    /// Delay of the output behind the input, in samples. It is the same at either quality
    /// and for any number of bands.
    pub fn latency(&self) -> u32 {
        self.latency as u32
    }

    pub fn sample_rate(&self) -> f64 {
        self.rate
    }

    /// Bands in use.
    pub fn bands(&self) -> usize {
        self.count
    }

    /// A parameter's value in physical units, as set.
    pub fn value(&self, id: u32) -> f64 {
        self.values[id as usize]
    }

    /// Sets a parameter in physical units. Values outside its range are clamped, stepped
    /// parameters snap to their nearest step, and anything that is no number or no
    /// parameter is ignored. The sound glides to the new setting.
    pub fn set_parameter(&mut self, id: u32, value: f64) {
        let Some(spec) = PARAMETERS.get(id as usize) else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        let value = if spec.steps > 0 {
            spec.denormalize(spec.normalize(value))
        } else {
            value.clamp(spec.min, spec.max)
        };
        if self.values[id as usize] != value {
            self.values[id as usize] = value;
            self.targets_stale = true;
        }
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Switches between the real-time and the offline path. The latency stays the same;
    /// the path switched to starts from silence.
    pub fn set_quality(&mut self, quality: Quality) {
        if quality != self.quality {
            self.quality = quality;
            for lane in self.lanes.iter_mut().flatten() {
                lane.clear_path(quality as usize);
            }
        }
    }

    /// How many times the sample rate the curve runs at, at `quality`.
    pub fn oversampling(&self, quality: Quality) -> usize {
        self.factors[quality as usize]
    }

    /// Clears the signal path, the modulation and the meters. The parameters jump to their
    /// settings.
    pub fn reset(&mut self) {
        self.sources.reset();
        self.update_targets();
        for band in &mut self.bands {
            band.current = band.targets;
            band.built = [f64::NAN; 3];
        }
        self.global_current = self.global_targets;
        self.tone_built = f64::NAN;
        self.crossovers_built = None;
        self.update_filters();
        self.filter_countdown = FILTER_STRIDE;
        self.control_countdown = CONTROL_STRIDE;
        for lane in self.lanes.iter_mut().flatten() {
            lane.clear();
        }
        for crossover in &mut self.crossovers {
            crossover.clear();
        }
        for filter in &mut self.tone {
            filter.clear();
        }
        for delay in &mut self.dry {
            delay.clear();
        }
        for follower in &mut self.followers {
            follower.clear();
        }
        self.meters = Meters::default();
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }

    /// Processes interleaved stereo frames in place.
    pub fn process(&mut self, frames: &mut [[f32; 2]]) {
        self.begin_block();
        for frame in frames {
            *frame = self.step(*frame);
        }
        self.end_block();
    }

    /// Processes a stereo signal held as one slice per channel, in place. The slices must
    /// be the same length.
    pub fn process_channels(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.begin_block();
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            [*left, *right] = self.step([*left, *right]);
        }
        self.end_block();
    }

    fn begin_block(&mut self) {
        if self.targets_stale {
            self.update_targets();
        }
    }

    fn end_block(&mut self) {
        for lane in self.lanes.iter_mut().flatten() {
            lane.flush();
        }
        for crossover in &mut self.crossovers {
            crossover.flush();
        }
        for filter in &mut self.tone {
            filter.flush();
        }
        for follower in &mut self.followers {
            follower.flush();
        }
        self.sources.envelope.flush();
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
            let shape = self.values[LFO_SHAPE as usize].round() as usize;
            telemetry::set(&telemetry.lfo, self.sources.lfo.value(shape) as f32);
            telemetry::set(&telemetry.lfo_phase, self.sources.lfo.phase() as f32);
            telemetry::set(&telemetry.envelope, self.sources.envelope_value() as f32);
            telemetry
                .updates
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Works out the effective parameters (the settings, moved by any modulation) and
    /// where the gliding values are heading.
    fn update_targets(&mut self) {
        self.modulated = (0..MOD_SLOTS).any(|slot| {
            self.values[mod_parameter(slot, MOD_SOURCE) as usize].round() as usize != 0
                && self.values[mod_parameter(slot, MOD_AMOUNT) as usize] != 0.0
        });
        let attack = self.values[ENVELOPE_ATTACK as usize];
        let release = self.values[ENVELOPE_RELEASE as usize];
        self.sources.envelope.set_times(self.rate, attack, release);
        self.effective = self.values;
        if self.modulated {
            self.modulate();
        }
        self.refresh_targets();
        self.targets_stale = false;
    }

    /// Moves the effective parameters by every modulation slot at work.
    fn modulate(&mut self) {
        let lfo = self
            .sources
            .lfo
            .value(self.values[LFO_SHAPE as usize].round() as usize);
        let envelope = self.sources.envelope_value();
        let bands = (self.values[BANDS as usize].round() as usize).clamp(1, MAX_BANDS);
        for slot in 0..MOD_SLOTS {
            let field = |field: u32| self.values[mod_parameter(slot, field) as usize];
            let amount = field(MOD_AMOUNT) / 100.0;
            let offset = match field(MOD_SOURCE).round() as usize {
                1 => 0.5 * lfo * amount,
                2 => envelope * amount,
                _ => continue,
            };
            if offset == 0.0 {
                continue;
            }
            let target = field(MOD_TARGET).round() as usize;
            let band = field(MOD_BAND).round() as usize;
            let effective = &mut self.effective;
            modulation::each_target(target, band, bands, |id| {
                let spec = &PARAMETERS[id as usize];
                let position = spec.normalize(effective[id as usize]) + offset;
                effective[id as usize] = spec.denormalize(position);
            });
        }
    }

    /// Where the gliding values are heading, from the effective parameters.
    fn refresh_targets(&mut self) {
        let values = &self.effective;
        let count = (values[BANDS as usize].round() as usize).clamp(1, MAX_BANDS);
        if count > self.count {
            // Bands coming into use start from silence and their settings.
            for band in self.count..count {
                for lane in &mut self.lanes[band] {
                    lane.clear();
                }
                self.followers[band].clear();
                self.bands[band].built = [f64::NAN; 3];
            }
        }
        let jump = count > self.count;
        let previous = self.count;
        self.count = count;
        let solo = (0..count).any(|band| values[band_parameter(band, Slot::Solo) as usize] >= 0.5);
        for (index, band) in self.bands.iter_mut().enumerate() {
            let value = |slot: Slot| values[band_parameter(index, slot) as usize];
            let model = Model::from_index(value(Slot::Model).round() as usize);
            let curve = Curve::new(value(Slot::Drive), value(Slot::Hardness), value(Slot::Bias));
            let curve = if model == Model::Fold {
                curve.folding()
            } else {
                curve
            };
            let reference = match band.measured {
                Some((measured, gain)) if measured == curve => gain,
                _ => {
                    let gain = curve.reference_gain();
                    band.measured = Some((curve, gain));
                    gain
                }
            };
            let audible = if solo {
                value(Slot::Solo) >= 0.5
            } else {
                value(Slot::Mute) < 0.5
            };
            let targets = &mut band.targets;
            targets[GAIN] = curve.gain();
            targets[BIAS_POINT] = curve.bias();
            targets[KNEE] = curve.knee();
            targets[ODD_SHARE] = (value(Slot::Odd) / 100.0).clamp(0.0, 1.0);
            targets[EVEN_SHARE] = (value(Slot::Even) / 100.0).max(0.0);
            targets[REFERENCE] = reference;
            targets[EMPHASIS_DB] = value(Slot::Emphasis);
            targets[EMPHASIS_PITCH] = value(Slot::EmphasisFrequency).ln();
            targets[MIX_SHARE] = (value(Slot::Mix) / 100.0).clamp(0.0, 1.0);
            targets[LEVEL_GAIN] = amplitude(value(Slot::Level));
            targets[DYNAMICS] = (value(Slot::Dynamics) / 100.0).clamp(-1.0, 1.0);
            targets[FEEDBACK] = (value(Slot::Feedback) / 100.0).clamp(0.0, 1.0);
            targets[FEEDBACK_PITCH] = value(Slot::FeedbackFrequency).ln();
            targets[AUDIBLE] = if audible { 1.0 } else { 0.0 };
            band.model = model;
            band.channels = match value(Slot::Channels).round() as usize {
                1 => Channels::Mid,
                2 => Channels::Side,
                _ => Channels::Stereo,
            };
            if jump && index >= previous && index < count {
                band.current = band.targets;
            }
        }
        let globals = &mut self.global_targets;
        globals[AUTO] = if values[AUTO_GAIN as usize] >= 0.5 {
            1.0
        } else {
            0.0
        };
        globals[GLOBAL_MIX] = (values[MIX as usize] / 100.0).clamp(0.0, 1.0);
        globals[OUTPUT_GAIN] = amplitude(values[OUTPUT as usize]);
        globals[TONE_DB] = values[TONE as usize];
        // The crossovers in order, each a little above the one below it.
        let top = 0.45 * self.rate;
        let mut floor = 0.0_f64;
        for index in 0..CROSSOVERS {
            let frequency = values[(CROSSOVER_BASE + index as u32) as usize]
                .max(floor * CROSSOVER_GAP)
                .min(top);
            globals[CROSSOVER_PITCH + index] = frequency.ln();
            floor = frequency;
        }
        self.slope = Slope::from_index(values[CROSSOVER_SLOPE as usize].round() as usize);
    }

    /// Rebuilds the filters whose settings moved: each band's emphasis and feedback, Tone,
    /// and the crossovers.
    fn update_filters(&mut self) {
        for (band, lanes) in self.bands.iter_mut().zip(&mut self.lanes).take(self.count) {
            let settings = [
                band.current[EMPHASIS_DB],
                band.current[EMPHASIS_PITCH],
                band.current[FEEDBACK_PITCH],
            ];
            if settings[..2] != band.built[..2] {
                let frequency = settings[1].exp();
                let emphasis = Coefficients::high_shelf(self.rate, frequency, settings[0]);
                let deemphasis = Coefficients::high_shelf(self.rate, frequency, -settings[0]);
                for lane in lanes.iter_mut() {
                    lane.set_emphasis(emphasis, deemphasis);
                }
            }
            if settings[2] != band.built[2] {
                for lane in lanes.iter_mut() {
                    lane.set_feedback_frequency(settings[2].exp());
                }
            }
            band.built = settings;
        }
        let tone = self.global_current[TONE_DB];
        if tone != self.tone_built {
            self.tone_built = tone;
            let coefficients = Coefficients::tilt(self.rate, TONE_PIVOT, tone);
            for filter in &mut self.tone {
                filter.set(coefficients);
            }
        }
        if self.count > 1 {
            let frequencies: [f64; CROSSOVERS] =
                std::array::from_fn(|index| self.global_current[CROSSOVER_PITCH + index].exp());
            let settings = (frequencies, self.slope);
            if self.crossovers_built != Some(settings) {
                for crossover in &mut self.crossovers {
                    crossover.tune(&frequencies, self.slope);
                }
                self.crossovers_built = Some(settings);
            }
        }
    }

    /// Takes one input frame and hands back the output frame a latency behind it.
    #[inline]
    fn step(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let input = frame.map(|sample| {
            if sample.is_finite() {
                sample.clamp(-INPUT_LIMIT, INPUT_LIMIT)
            } else {
                0.0
            }
        });
        self.sources
            .envelope
            .follow(f64::from(input[0].abs().max(input[1].abs())));
        if self.modulated {
            self.control_countdown -= 1;
            if self.control_countdown == 0 {
                self.control_countdown = CONTROL_STRIDE;
                let hz = self.values[LFO_RATE as usize];
                self.sources.lfo.advance(hz, self.rate, CONTROL_STRIDE);
                self.effective = self.values;
                self.modulate();
                self.refresh_targets();
            }
        }
        let share = self.smooth;
        for band in &mut self.bands[..self.count] {
            glide(&mut band.current, &band.targets, share);
        }
        glide(&mut self.global_current, &self.global_targets, share);
        self.filter_countdown -= 1;
        if self.filter_countdown == 0 {
            self.filter_countdown = FILTER_STRIDE;
            self.update_filters();
        }
        let count = self.count;
        let path = self.quality as usize;
        let mut split = [[0.0_f64; MAX_BANDS]; 2];
        for (channel, crossover) in self.crossovers.iter_mut().enumerate() {
            crossover.split(f64::from(input[channel]), count, &mut split[channel]);
        }
        let auto = self.global_current[AUTO];
        let mut dry = [0.0_f32; 2];
        for (channel, delay) in self.dry.iter_mut().enumerate() {
            let value = if count == 1 && self.bands[0].current[AUDIBLE] == 1.0 {
                input[channel]
            } else {
                (0..count)
                    .map(|band| split[channel][band] * self.bands[band].current[AUDIBLE])
                    .sum::<f64>() as f32
            };
            dry[channel] = delay.push(value);
        }
        let mut wet: [Option<f64>; 2] = [None, None];
        // The index walks the bands' settings, envelopes, lanes and split signals together.
        #[allow(clippy::needless_range_loop)]
        for index in 0..count {
            let band = &self.bands[index];
            let current = &band.current;
            let level =
                self.followers[index].follow(split[0][index].abs().max(split[1][index].abs()));
            let bias = if band.model == Model::Tube {
                current[BIAS_POINT] + BLOOM * level.min(1.0)
            } else {
                current[BIAS_POINT]
            };
            let curve = Curve::raw(current[GAIN], bias, current[KNEE]);
            let reference = current[REFERENCE];
            let frame = BandFrame {
                curve: if band.model == Model::Fold {
                    curve.folding()
                } else {
                    curve
                },
                odd: current[ODD_SHARE],
                even: current[EVEN_SHARE],
                inverse: 1.0 / reference,
                makeup: if band.model == Model::Tape {
                    // Auto gain also makes up what the tape loop takes at its level.
                    (auto + (1.0 - auto) * reference) * (1.0 + auto * (tape_makeup() - 1.0))
                } else {
                    auto + (1.0 - auto) * reference
                },
                model: band.model,
                feedback: if current[FEEDBACK] != 0.0 {
                    current[FEEDBACK] * FEEDBACK_LOOP * (level / FEEDBACK_GATE).min(1.0)
                } else {
                    0.0
                },
            };
            let gain = if current[DYNAMICS] != 0.0 {
                dynamics::gain(current[DYNAMICS], level)
            } else {
                1.0
            };
            let (mix, level_gain, audible) =
                (current[MIX_SHARE], current[LEVEL_GAIN], current[AUDIBLE]);
            let [first, second] = &mut self.lanes[index];
            let out = match band.channels {
                Channels::Stereo => {
                    let (wet_left, dry_left) = first.step(split[0][index], path, &frame);
                    let (wet_right, dry_right) = second.step(split[1][index], path, &frame);
                    [
                        finish(wet_left, dry_left, gain, mix, level_gain, audible),
                        finish(wet_right, dry_right, gain, mix, level_gain, audible),
                    ]
                }
                Channels::Mid | Channels::Side => {
                    let mid = 0.5 * (split[0][index] + split[1][index]);
                    let side = 0.5 * (split[0][index] - split[1][index]);
                    let (worked, passed) = if band.channels == Channels::Mid {
                        (mid, side)
                    } else {
                        (side, mid)
                    };
                    let (wet, own_dry) = first.step(worked, path, &frame);
                    let worked = finish(wet, own_dry, gain, mix, level_gain, audible);
                    let passed = second.pass(passed) * audible;
                    let (mid, side) = if band.channels == Channels::Mid {
                        (worked, passed)
                    } else {
                        (passed, worked)
                    };
                    [mid + side, mid - side]
                }
            };
            for (sum, value) in wet.iter_mut().zip(out) {
                *sum = Some(sum.map_or(value, |sum| sum + value));
            }
            if self.telemetry.is_some() {
                let peak = out[0].abs().max(out[1].abs());
                self.meters.band(index, self.meter_decay, peak);
            }
        }
        let (mix, output) = (
            self.global_current[GLOBAL_MIX],
            self.global_current[OUTPUT_GAIN],
        );
        let mut result = [0.0_f32; 2];
        for channel in 0..2 {
            let wet = self.tone[channel].process(wet[channel].unwrap_or(0.0));
            let dry = f64::from(dry[channel]);
            let out = ((dry + mix * (wet - dry)) * output) as f32;
            result[channel] = if out.is_finite() { out } else { 0.0 };
        }
        if self.telemetry.is_some() {
            self.meters.frame(self.meter_decay, input, result);
        }
        result
    }
}
