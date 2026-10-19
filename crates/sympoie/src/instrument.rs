use std::error::Error;
use std::f64::consts::TAU;
use std::fmt;

use crate::{Frame, SAMPLE_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Saw,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubtractiveSettings {
    pub waveform: Waveform,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub attack_seconds: f32,
    pub decay_seconds: f32,
    pub sustain: f32,
    pub release_seconds: f32,
    pub gain: f32,
}

impl Default for SubtractiveSettings {
    fn default() -> Self {
        Self {
            waveform: Waveform::Saw,
            cutoff_hz: 4_000.0,
            resonance: std::f32::consts::FRAC_1_SQRT_2,
            attack_seconds: 0.005,
            decay_seconds: 0.12,
            sustain: 0.65,
            release_seconds: 0.18,
            gain: 0.2,
        }
    }
}

impl SubtractiveSettings {
    pub fn validate(&self) -> Result<(), InstrumentError> {
        for (parameter, value, min, max) in [
            ("cutoff_hz", self.cutoff_hz, 20.0, 20_000.0),
            ("resonance", self.resonance, 0.5, 10.0),
            ("attack_seconds", self.attack_seconds, 0.0, 10.0),
            ("decay_seconds", self.decay_seconds, 0.0, 10.0),
            ("sustain", self.sustain, 0.0, 1.0),
            ("release_seconds", self.release_seconds, 0.0, 10.0),
            ("gain", self.gain, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(InstrumentError::InvalidParameter(parameter));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstrumentError {
    InvalidParameter(&'static str),
}

impl fmt::Display for InstrumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::InvalidParameter(parameter) = self;
        let range = match *parameter {
            "cutoff_hz" => "20–20000 Hz",
            "resonance" => "0.5–10 Q",
            "attack_seconds" | "decay_seconds" | "release_seconds" => "0–10 seconds",
            _ => "0–1",
        };
        write!(
            formatter,
            "instrument {parameter} must be finite and in {range}"
        )
    }
}

impl Error for InstrumentError {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Instrument {
    Subtractive(SubtractiveSettings),
}

impl Instrument {
    pub fn validate(&self) -> Result<(), InstrumentError> {
        match self {
            Self::Subtractive(settings) => settings.validate(),
        }
    }

    pub fn release_frames(&self) -> u64 {
        match self {
            Self::Subtractive(settings) => seconds_to_frames(settings.release_seconds),
        }
    }

    pub fn create_processor(&self) -> Result<Box<dyn InstrumentProcessor>, InstrumentError> {
        self.validate()?;
        Ok(match self {
            Self::Subtractive(settings) => Box::new(SubtractiveProcessor::new(*settings)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteEvent {
    On {
        id: u64,
        key: u8,
        velocity: u8,
        channel: u8,
        /// Portamento time into this note from the previous pitch; 0 means no slide.
        slide_frames: u32,
    },
    Off {
        id: u64,
    },
    /// Ramp the channel's bend linearly to `value` (-1..=1 of the bend range) over
    /// `ramp_frames`; 0 jumps (instruments may still smooth it).
    PitchBend {
        channel: u8,
        value: f32,
        ramp_frames: u32,
    },
}

pub trait InstrumentProcessor: fmt::Debug + Send {
    fn reset(&mut self);
    fn event(&mut self, event: NoteEvent);
    fn process(&mut self, output: &mut [Frame]);
    fn set_parameters(&mut self, instrument: &Instrument) -> bool;
    fn set_automation_parameters(&mut self, instrument: &Instrument) -> bool {
        self.set_parameters(instrument)
    }
}

const VOICE_COUNT: usize = 32;

const PARAMETER_GLIDE_FRAMES: u32 = 960;

#[derive(Debug)]
struct SubtractiveProcessor {
    settings: SubtractiveSettings,
    previous_waveform: Waveform,
    waveform_mix: Glide,
    gain: Glide,
    sustain: Glide,
    cutoff_octaves: Glide,
    resonance: Glide,
    envelope: EnvelopeSettings,
    filter: LowPass,
    voices: [Voice; VOICE_COUNT],
    steal_cursor: usize,
}

impl SubtractiveProcessor {
    fn new(settings: SubtractiveSettings) -> Self {
        Self {
            settings,
            previous_waveform: settings.waveform,
            waveform_mix: Glide::new(1.0),
            gain: Glide::new(f64::from(settings.gain)),
            sustain: Glide::new(f64::from(settings.sustain)),
            cutoff_octaves: Glide::new(f64::from(settings.cutoff_hz).log2()),
            resonance: Glide::new(f64::from(settings.resonance)),
            envelope: EnvelopeSettings::new(&settings),
            filter: LowPass::new(settings.cutoff_hz, settings.resonance),
            voices: [Voice::SILENT; VOICE_COUNT],
            steal_cursor: 0,
        }
    }

    fn finish_glides(&mut self) {
        for glide in [
            &mut self.waveform_mix,
            &mut self.gain,
            &mut self.sustain,
            &mut self.cutoff_octaves,
            &mut self.resonance,
        ] {
            glide.finish();
        }
        self.filter = LowPass::new(self.settings.cutoff_hz, self.settings.resonance);
    }

    fn glide_filter(&mut self) {
        let octaves = self.cutoff_octaves.next();
        let resonance = self.resonance.next();
        self.filter = if self.cutoff_octaves.is_gliding() || self.resonance.is_gliding() {
            LowPass::from_f64(octaves.exp2(), resonance)
        } else {
            LowPass::new(self.settings.cutoff_hz, self.settings.resonance)
        };
    }
}

impl InstrumentProcessor for SubtractiveProcessor {
    fn reset(&mut self) {
        self.voices.fill(Voice::SILENT);
        self.steal_cursor = 0;
        self.finish_glides();
    }

    fn set_parameters(&mut self, instrument: &Instrument) -> bool {
        let Instrument::Subtractive(settings) = *instrument;
        if settings.validate().is_err() {
            return false;
        }
        if settings.waveform != self.settings.waveform {
            if self.waveform_mix.value() >= 0.5 {
                self.previous_waveform = self.settings.waveform;
            }
            self.waveform_mix = Glide::new(0.0);
            self.waveform_mix.set(1.0);
        }
        self.gain.set(f64::from(settings.gain));
        self.sustain.set(f64::from(settings.sustain));
        self.cutoff_octaves
            .set(f64::from(settings.cutoff_hz).log2());
        self.resonance.set(f64::from(settings.resonance));
        let envelope = EnvelopeSettings::new(&settings);
        if envelope != self.envelope {
            for voice in &mut self.voices {
                voice.envelope.retime(envelope);
            }
            self.envelope = envelope;
        }
        self.settings = settings;
        true
    }

    fn set_automation_parameters(&mut self, instrument: &Instrument) -> bool {
        if !self.set_parameters(instrument) {
            return false;
        }
        self.finish_glides();
        true
    }

    fn event(&mut self, event: NoteEvent) {
        match event {
            NoteEvent::PitchBend { .. } => {}
            NoteEvent::On {
                id, key, velocity, ..
            } => {
                let index = self
                    .voices
                    .iter()
                    .position(|voice| voice.active() && voice.id == id)
                    .or_else(|| self.voices.iter().position(|voice| !voice.active()))
                    .unwrap_or_else(|| {
                        let index = self.steal_cursor;
                        self.steal_cursor = (self.steal_cursor + 1) % VOICE_COUNT;
                        index
                    });
                self.voices[index] = Voice {
                    id,
                    phase: 0.0,
                    increment: 440.0 * 2.0_f64.powf((f64::from(key) - 69.0) / 12.0)
                        / f64::from(SAMPLE_RATE),
                    velocity: f64::from(velocity) / 127.0,
                    envelope: Envelope::new(self.envelope, self.sustain.value()),
                    filter: FilterState::default(),
                };
            }
            NoteEvent::Off { id } => {
                if let Some(voice) = self
                    .voices
                    .iter_mut()
                    .find(|voice| voice.active() && voice.id == id)
                {
                    voice.envelope.release(self.envelope.release);
                }
            }
        }
    }

    fn process(&mut self, output: &mut [Frame]) {
        for frame in output {
            if self.cutoff_octaves.is_gliding() || self.resonance.is_gliding() {
                self.glide_filter();
            }
            let sustain = self.sustain.next();
            let crossfade = self
                .waveform_mix
                .is_gliding()
                .then(|| self.waveform_mix.next());
            let mut sample = 0.0;
            for voice in &mut self.voices {
                if !voice.active() {
                    continue;
                }
                let mut oscillator =
                    oscillator(self.settings.waveform, voice.phase, voice.increment);
                if let Some(mix) = crossfade {
                    let previous =
                        self::oscillator(self.previous_waveform, voice.phase, voice.increment);
                    oscillator = previous + (oscillator - previous) * mix;
                }
                voice.phase += voice.increment;
                voice.phase -= voice.phase.floor();
                let filtered = voice.filter.process(oscillator, self.filter);
                sample += filtered * voice.envelope.next(self.envelope, sustain) * voice.velocity;
            }
            let sample = (sample * self.gain.next()) as f32;
            *frame = [sample; 2];
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Glide {
    value: f64,
    target: f64,
    step: f64,
    remaining: u32,
}

impl Glide {
    fn new(value: f64) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    fn set(&mut self, target: f64) {
        if target == self.target {
            return;
        }
        self.target = target;
        self.remaining = PARAMETER_GLIDE_FRAMES;
        self.step = (target - self.value) / f64::from(PARAMETER_GLIDE_FRAMES);
    }

    fn finish(&mut self) {
        self.value = self.target;
        self.remaining = 0;
    }

    fn is_gliding(&self) -> bool {
        self.remaining != 0
    }

    fn value(&self) -> f64 {
        self.value
    }

    fn next(&mut self) -> f64 {
        if self.remaining != 0 {
            self.remaining -= 1;
            self.value = if self.remaining == 0 {
                self.target
            } else {
                self.value + self.step
            };
        }
        self.value
    }
}

fn seconds_to_frames(seconds: f32) -> u64 {
    (f64::from(seconds) * f64::from(SAMPLE_RATE)).ceil() as u64
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    id: u64,
    phase: f64,
    increment: f64,
    velocity: f64,
    envelope: Envelope,
    filter: FilterState,
}

impl Voice {
    const SILENT: Self = Self {
        id: 0,
        phase: 0.0,
        increment: 0.0,
        velocity: 0.0,
        envelope: Envelope {
            stage: Stage::Silent,
            level: 0.0,
            start: 0.0,
            position: 0.0,
            length: 0,
        },
        filter: FilterState { z1: 0.0, z2: 0.0 },
    };

    fn active(&self) -> bool {
        self.envelope.stage != Stage::Silent
    }
}

fn oscillator(waveform: Waveform, phase: f64, increment: f64) -> f64 {
    match waveform {
        Waveform::Sine => (TAU * phase).sin(),
        Waveform::Saw => 2.0 * phase - 1.0 - poly_blep(phase, increment),
        Waveform::Square => {
            let square = if phase < 0.5 { 1.0 } else { -1.0 };
            square + poly_blep(phase, increment) - poly_blep((phase + 0.5).fract(), increment)
        }
    }
}

fn poly_blep(phase: f64, increment: f64) -> f64 {
    if phase < increment {
        let distance = 1.0 - phase / increment;
        -distance * distance
    } else if phase > 1.0 - increment {
        let distance = (phase - 1.0) / increment + 1.0;
        distance * distance
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Copy)]
struct LowPass {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl LowPass {
    fn new(cutoff_hz: f32, q: f32) -> Self {
        Self::from_f64(f64::from(cutoff_hz), f64::from(q))
    }

    fn from_f64(cutoff_hz: f64, q: f64) -> Self {
        let omega = TAU * cutoff_hz / f64::from(SAMPLE_RATE);
        let cosine = omega.cos();
        let alpha = omega.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cosine) / (2.0 * a0),
            b1: (1.0 - cosine) / a0,
            b2: (1.0 - cosine) / (2.0 * a0),
            a1: -2.0 * cosine / a0,
            a2: (1.0 - alpha) / a0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct FilterState {
    z1: f64,
    z2: f64,
}

impl FilterState {
    fn process(&mut self, input: f64, filter: LowPass) -> f64 {
        let output = filter.b0 * input + self.z1;
        self.z1 = filter.b1 * input - filter.a1 * output + self.z2;
        self.z2 = filter.b2 * input - filter.a2 * output;
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnvelopeSettings {
    attack: u64,
    decay: u64,
    release: u64,
}

impl EnvelopeSettings {
    fn new(settings: &SubtractiveSettings) -> Self {
        Self {
            attack: seconds_to_frames(settings.attack_seconds),
            decay: seconds_to_frames(settings.decay_seconds),
            release: seconds_to_frames(settings.release_seconds),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
    Silent,
}

#[derive(Debug, Clone, Copy)]
struct Envelope {
    stage: Stage,
    level: f64,
    start: f64,
    position: f64,
    length: u64,
}

impl Envelope {
    fn new(settings: EnvelopeSettings, sustain: f64) -> Self {
        let mut envelope = Self {
            stage: Stage::Attack,
            level: 0.0,
            start: 0.0,
            position: 0.0,
            length: settings.attack,
        };
        if settings.attack == 0 {
            envelope.level = 1.0;
            envelope.begin_decay(settings);
            if envelope.stage == Stage::Sustain {
                envelope.level = sustain;
            }
        }
        envelope
    }

    fn begin_decay(&mut self, settings: EnvelopeSettings) {
        self.position = 0.0;
        if settings.decay == 0 {
            self.stage = Stage::Sustain;
        } else {
            self.stage = Stage::Decay;
            self.start = 1.0;
            self.length = settings.decay;
        }
    }

    fn release(&mut self, duration: u64) {
        if self.stage == Stage::Release {
            return;
        }
        self.start = self.level;
        self.position = 0.0;
        self.length = duration;
        self.stage = if duration == 0 {
            Stage::Silent
        } else {
            Stage::Release
        };
    }

    fn retime(&mut self, settings: EnvelopeSettings) {
        let length = match self.stage {
            Stage::Attack => settings.attack,
            Stage::Decay => settings.decay,
            Stage::Release => settings.release,
            Stage::Sustain | Stage::Silent => return,
        };
        if length == self.length {
            return;
        }
        let progress = if self.length == 0 {
            1.0
        } else {
            self.position / self.length as f64
        };
        self.position = progress * length as f64;
        self.length = length;
    }

    fn advance(&mut self) -> (f64, bool) {
        self.position += 1.0;
        let length = self.length as f64;
        if self.position >= length {
            (1.0, true)
        } else {
            (self.position / length, false)
        }
    }

    fn next(&mut self, settings: EnvelopeSettings, sustain: f64) -> f64 {
        match self.stage {
            Stage::Attack => {
                let (fraction, finished) = self.advance();
                self.level = self.start + (1.0 - self.start) * fraction;
                if finished {
                    self.begin_decay(settings);
                }
            }
            Stage::Decay => {
                let (fraction, finished) = self.advance();
                self.level = self.start + (sustain - self.start) * fraction;
                if finished {
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = sustain,
            Stage::Release => {
                let (fraction, finished) = self.advance();
                self.level = self.start * (1.0 - fraction);
                if finished {
                    self.stage = Stage::Silent;
                }
            }
            Stage::Silent => self.level = 0.0,
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady_settings() -> SubtractiveSettings {
        SubtractiveSettings {
            waveform: Waveform::Sine,
            cutoff_hz: 20_000.0,
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            gain: 1.0,
            ..SubtractiveSettings::default()
        }
    }

    fn note_on(processor: &mut dyn InstrumentProcessor, id: u64, key: u8, velocity: u8) {
        processor.event(NoteEvent::On {
            id,
            key,
            velocity,
            channel: 0,
            slide_frames: 0,
        });
    }

    fn render(settings: SubtractiveSettings, key: u8, velocity: u8, frames: usize) -> Vec<Frame> {
        let mut processor = Instrument::Subtractive(settings)
            .create_processor()
            .unwrap();
        note_on(processor.as_mut(), 0, key, velocity);
        let mut output = vec![[0.0; 2]; frames];
        processor.process(&mut output);
        output
    }

    fn rms(frames: &[Frame]) -> f64 {
        (frames
            .iter()
            .map(|frame| f64::from(frame[0]).powi(2))
            .sum::<f64>()
            / frames.len() as f64)
            .sqrt()
    }

    #[test]
    fn default_parameters_are_valid_and_invalid_parameters_are_rejected() {
        assert!(SubtractiveSettings::default().validate().is_ok());
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 10.1] {
            assert!(
                SubtractiveSettings {
                    attack_seconds: invalid,
                    ..steady_settings()
                }
                .validate()
                .is_err()
            );
        }
        for settings in [
            SubtractiveSettings {
                cutoff_hz: 19.0,
                ..steady_settings()
            },
            SubtractiveSettings {
                cutoff_hz: 20_001.0,
                ..steady_settings()
            },
            SubtractiveSettings {
                resonance: 0.49,
                ..steady_settings()
            },
            SubtractiveSettings {
                resonance: 10.1,
                ..steady_settings()
            },
            SubtractiveSettings {
                gain: 1.1,
                ..steady_settings()
            },
            SubtractiveSettings {
                sustain: -0.1,
                ..steady_settings()
            },
        ] {
            assert!(settings.validate().is_err());
        }
    }

    #[test]
    fn a4_oscillates_at_440_hz_and_duplicates_to_stereo() {
        let output = render(steady_settings(), 69, 127, SAMPLE_RATE as usize + 1);
        let crossings = output
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        assert_eq!(crossings, 440);
        assert!(output.iter().all(|frame| frame[0] == frame[1]));
        assert!((rms(&output) - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.002);
    }

    #[test]
    fn low_pass_removes_high_frequency_energy_and_q_controls_resonance() {
        let bright = render(steady_settings(), 93, 127, 24_000);
        let dark = render(
            SubtractiveSettings {
                cutoff_hz: 200.0,
                ..steady_settings()
            },
            93,
            127,
            24_000,
        );
        assert!(rms(&dark[12_000..]) < rms(&bright[12_000..]) * 0.02);
        let low_q = render(
            SubtractiveSettings {
                cutoff_hz: 440.0,
                resonance: 0.5,
                ..steady_settings()
            },
            69,
            127,
            24_000,
        );
        let high_q = render(
            SubtractiveSettings {
                cutoff_hz: 440.0,
                resonance: 2.0,
                ..steady_settings()
            },
            69,
            127,
            24_000,
        );
        assert!(rms(&high_q[12_000..]) > 3.9 * rms(&low_q[12_000..]));
    }

    #[test]
    fn attack_decay_sustain_and_early_release_have_exact_sample_lengths() {
        let settings = EnvelopeSettings {
            attack: 4,
            decay: 2,
            release: 4,
        };
        let sustain = 0.5;
        let mut envelope = Envelope::new(settings, sustain);
        let levels: Vec<_> = (0..8).map(|_| envelope.next(settings, sustain)).collect();
        assert_eq!(levels, [0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.5, 0.5]);
        envelope.release(settings.release);
        assert_eq!(
            (0..5)
                .map(|_| envelope.next(settings, sustain))
                .collect::<Vec<_>>(),
            [0.375, 0.25, 0.125, 0.0, 0.0]
        );
        let mut envelope = Envelope::new(settings, sustain);
        assert_eq!(envelope.next(settings, sustain), 0.25);
        envelope.release(settings.release);
        assert_eq!(
            (0..4)
                .map(|_| envelope.next(settings, sustain))
                .collect::<Vec<_>>(),
            [0.1875, 0.125, 0.0625, 0.0]
        );
    }

    #[test]
    fn release_tail_is_finite_and_reset_clears_filter_and_voices() {
        let instrument = Instrument::Subtractive(SubtractiveSettings {
            release_seconds: 0.001,
            ..steady_settings()
        });
        let mut processor = instrument.create_processor().unwrap();
        note_on(processor.as_mut(), 12, 69, 127);
        processor.process(&mut [[0.0; 2]; 120]);
        processor.event(NoteEvent::Off { id: 12 });
        let mut tail = vec![[1.0; 2]; instrument.release_frames() as usize + 8];
        processor.process(&mut tail);
        assert!(tail[..tail.len() - 8].iter().any(|frame| frame[0] != 0.0));
        assert!(
            tail[instrument.release_frames() as usize - 1..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        note_on(processor.as_mut(), 13, 72, 127);
        processor.reset();
        processor.process(&mut tail);
        assert!(tail.iter().all(|frame| *frame == [0.0; 2]));
        note_on(processor.as_mut(), 0, 69, 127);
        processor.process(&mut tail);
        assert_eq!(
            tail,
            render(
                SubtractiveSettings {
                    release_seconds: 0.001,
                    ..steady_settings()
                },
                69,
                127,
                tail.len()
            )
        );
    }

    #[test]
    fn zero_release_stops_immediately() {
        let mut processor = Instrument::Subtractive(SubtractiveSettings {
            release_seconds: 0.0,
            ..steady_settings()
        })
        .create_processor()
        .unwrap();
        note_on(processor.as_mut(), 0, 69, 127);
        processor.process(&mut [[0.0; 2]; 100]);
        processor.event(NoteEvent::Off { id: 0 });
        let mut output = [[1.0; 2]; 10];
        processor.process(&mut output);
        assert_eq!(output, [[0.0; 2]; 10]);
    }

    #[test]
    fn velocity_scales_amplitude() {
        let loud = render(steady_settings(), 69, 127, 4_000);
        let soft = render(steady_settings(), 69, 64, 4_000);
        assert!((rms(&soft) / rms(&loud) - 64.0 / 127.0).abs() < 1e-7);
    }

    #[test]
    fn overlapping_identical_notes_have_independent_lifetimes() {
        let settings = SubtractiveSettings {
            release_seconds: 0.0,
            ..steady_settings()
        };
        let mut both = Instrument::Subtractive(settings)
            .create_processor()
            .unwrap();
        let mut reference = Instrument::Subtractive(settings)
            .create_processor()
            .unwrap();
        note_on(both.as_mut(), 1, 60, 127);
        note_on(both.as_mut(), 2, 60, 127);
        note_on(reference.as_mut(), 2, 60, 127);
        both.process(&mut [[0.0; 2]; 90]);
        reference.process(&mut [[0.0; 2]; 90]);
        both.event(NoteEvent::Off { id: 1 });
        let mut actual = [[0.0; 2]; 100];
        let mut expected = actual;
        both.process(&mut actual);
        reference.process(&mut expected);
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|frame| frame[0] != 0.0));
    }

    #[test]
    fn stealing_is_bounded_and_stale_off_does_not_stop_replacement() {
        let settings = SubtractiveSettings {
            release_seconds: 0.0,
            ..steady_settings()
        };
        let mut processor = SubtractiveProcessor::new(settings);
        for id in 0..33 {
            note_on(&mut processor, id, 60, 127);
        }
        assert_eq!(
            processor
                .voices
                .iter()
                .filter(|voice| voice.active())
                .count(),
            32
        );
        assert_eq!(processor.voices[0].id, 32);
        processor.event(NoteEvent::Off { id: 0 });
        assert!(processor.voices[0].active());
        for id in 1..33 {
            processor.event(NoteEvent::Off { id });
        }
        assert!(processor.voices.iter().all(|voice| !voice.active()));
    }

    #[test]
    fn all_waveforms_and_boundary_filter_settings_are_finite() {
        for waveform in [Waveform::Sine, Waveform::Saw, Waveform::Square] {
            for cutoff_hz in [20.0, 20_000.0] {
                for resonance in [0.5, 10.0] {
                    let settings = SubtractiveSettings {
                        waveform,
                        cutoff_hz,
                        resonance,
                        ..steady_settings()
                    };
                    for key in [0, 127] {
                        let output = render(settings, key, 127, 12_000);
                        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
                    }
                }
            }
        }
    }

    #[test]
    fn processing_is_independent_of_block_partitioning() {
        for waveform in [Waveform::Sine, Waveform::Saw, Waveform::Square] {
            let instrument = Instrument::Subtractive(SubtractiveSettings {
                waveform,
                ..SubtractiveSettings::default()
            });
            let mut whole = instrument.create_processor().unwrap();
            let mut chunked = instrument.create_processor().unwrap();
            note_on(whole.as_mut(), 1, 72, 85);
            note_on(chunked.as_mut(), 1, 72, 85);
            let mut expected = vec![[0.0; 2]; 2_047];
            let mut actual = expected.clone();
            whole.process(&mut expected);
            for chunk in actual.chunks_mut(37) {
                chunked.process(chunk);
            }
            assert_eq!(actual, expected);
            whole.event(NoteEvent::Off { id: 1 });
            chunked.event(NoteEvent::Off { id: 1 });
            whole.process(&mut expected);
            for chunk in actual.chunks_mut(53) {
                chunked.process(chunk);
            }
            assert_eq!(actual, expected);
        }
    }

    fn largest_step(frames: &[Frame]) -> f32 {
        frames
            .windows(2)
            .map(|pair| (pair[1][0] - pair[0][0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn parameter_changes_glide_from_the_old_sound_into_exactly_the_new_one() {
        let before = SubtractiveSettings {
            cutoff_hz: 2_000.0,
            gain: 0.8,
            ..steady_settings()
        };
        for after in [
            SubtractiveSettings {
                gain: 0.1,
                ..before
            },
            SubtractiveSettings {
                sustain: 0.3,
                ..before
            },
            SubtractiveSettings {
                cutoff_hz: 40.0,
                ..before
            },
            SubtractiveSettings {
                cutoff_hz: 300.0,
                resonance: 4.0,
                ..before
            },
            SubtractiveSettings {
                waveform: Waveform::Saw,
                ..before
            },
            SubtractiveSettings {
                waveform: Waveform::Square,
                cutoff_hz: 20_000.0,
                gain: 1.0,
                ..before
            },
        ] {
            let mut edited = SubtractiveProcessor::new(before);
            let mut reference = SubtractiveProcessor::new(after);
            note_on(&mut edited, 0, 12, 127);
            note_on(&mut reference, 0, 12, 127);
            let mut played = vec![[0.0; 2]; 7_000];
            edited.process(&mut played);
            reference.process(&mut vec![[0.0; 2]; played.len()]);

            assert!(edited.set_parameters(&Instrument::Subtractive(after)));
            let mut changed = vec![[0.0; 2]; 24_000];
            edited.process(&mut changed);
            let mut expected = vec![[0.0; 2]; changed.len()];
            reference.process(&mut expected);

            let steady = largest_step(&played[1_000..]);
            let mut joint = vec![*played.last().unwrap()];
            joint.extend_from_slice(&changed[..100]);
            assert!(
                largest_step(&joint) < 2.0 * steady + 1e-3,
                "{after:?}: step {} vs {steady}",
                largest_step(&joint)
            );
            for (actual, expected) in changed[20_000..].iter().zip(&expected[20_000..]) {
                assert!((actual[0] - expected[0]).abs() < 1e-6, "{after:?}");
            }
        }
    }

    #[test]
    fn retiming_an_envelope_keeps_its_level_and_adopts_the_new_rate() {
        let sustain = 0.5;
        let slow = EnvelopeSettings {
            attack: 1_000,
            decay: 1_000,
            release: 1_000,
        };
        let fast = EnvelopeSettings {
            attack: 100,
            decay: 100,
            release: 100,
        };
        let close = |actual: f64, expected: f64| (actual - expected).abs() < 1e-12;

        let mut envelope = Envelope::new(slow, sustain);
        for _ in 0..500 {
            envelope.next(slow, sustain);
        }
        assert!(close(envelope.level, 0.5));
        envelope.retime(fast);
        assert!(close(envelope.next(fast, sustain), 0.51));
        let rest: Vec<_> = (0..49).map(|_| envelope.next(fast, sustain)).collect();
        assert_eq!(rest.last(), Some(&1.0));
        assert_eq!(envelope.stage, Stage::Decay);

        for _ in 0..50 {
            envelope.next(fast, sustain);
        }
        assert!(close(envelope.level, 0.75));
        envelope.retime(slow);
        assert!(close(envelope.next(slow, sustain), 0.75 - 0.5 / 1_000.0));
        for _ in 0..498 {
            envelope.next(slow, sustain);
        }
        assert_eq!(envelope.stage, Stage::Decay);
        assert_eq!(envelope.next(slow, sustain), sustain);
        assert_eq!(envelope.stage, Stage::Sustain);
        envelope.retime(fast);
        assert_eq!(envelope.next(fast, sustain), sustain);

        envelope.release(fast.release);
        for _ in 0..25 {
            envelope.next(fast, sustain);
        }
        assert!(close(envelope.level, 0.375));
        envelope.retime(slow);
        let tail: Vec<_> = (0..750).map(|_| envelope.next(slow, sustain)).collect();
        assert!(close(tail[0], 0.375 - 0.5 / 1_000.0));
        assert!(tail.windows(2).all(|pair| pair[1] < pair[0]));
        assert_eq!(tail.last(), Some(&0.0));
        assert_eq!(envelope.stage, Stage::Silent);
    }

    #[test]
    fn rejected_parameters_change_nothing_and_reset_lands_on_the_new_settings() {
        let before = steady_settings();
        let after = SubtractiveSettings {
            waveform: Waveform::Square,
            cutoff_hz: 900.0,
            resonance: 3.0,
            sustain: 0.4,
            gain: 0.3,
            ..before
        };
        let mut processor = SubtractiveProcessor::new(before);
        note_on(&mut processor, 0, 69, 127);
        assert!(
            !processor.set_parameters(&Instrument::Subtractive(SubtractiveSettings {
                cutoff_hz: 5.0,
                ..after
            }))
        );
        let mut actual = vec![[0.0; 2]; 500];
        processor.process(&mut actual);
        assert_eq!(actual, render(before, 69, 127, 500));

        assert!(processor.set_parameters(&Instrument::Subtractive(after)));
        processor.process(&mut actual[..10]);
        processor.reset();
        note_on(&mut processor, 0, 69, 127);
        processor.process(&mut actual);
        assert_eq!(actual, render(after, 69, 127, 500));
    }
}
