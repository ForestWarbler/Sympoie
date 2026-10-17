use std::error::Error;
use std::fmt;

use crate::{SAMPLE_RATE, TICKS_PER_QUARTER};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    bpm: f64,
}

impl Tempo {
    pub fn new(bpm: f64) -> Result<Self, TimeError> {
        if !bpm.is_finite() || bpm <= 0.0 {
            return Err(TimeError::InvalidTempo);
        }
        Ok(Self { bpm })
    }

    pub fn bpm(self) -> f64 {
        self.bpm
    }

    pub fn ticks_to_seconds(self, ticks: u64) -> Result<f64, TimeError> {
        let seconds = (ticks as f64 * (60.0 / TICKS_PER_QUARTER as f64)) / self.bpm;
        if !seconds.is_finite() {
            return Err(TimeError::ConversionOverflow);
        }
        Ok(seconds)
    }

    pub fn seconds_to_ticks(self, seconds: f64) -> Result<u64, TimeError> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(TimeError::InvalidSeconds);
        }
        rounded_position(seconds * self.bpm * (TICKS_PER_QUARTER as f64 / 60.0))
    }

    pub fn ticks_to_frames(self, ticks: u64) -> Result<u64, TimeError> {
        let scale = SAMPLE_RATE as f64 * 60.0 / TICKS_PER_QUARTER as f64;
        rounded_position(ticks as f64 * scale / self.bpm)
    }

    pub fn frames_to_ticks(self, frames: u64) -> Result<u64, TimeError> {
        let scale = SAMPLE_RATE as f64 * 60.0 / TICKS_PER_QUARTER as f64;
        rounded_position(frames as f64 * self.bpm / scale)
    }
}

impl Default for Tempo {
    fn default() -> Self {
        Self { bpm: 120.0 }
    }
}

fn rounded_position(value: f64) -> Result<u64, TimeError> {
    let rounded = value.round();
    if !rounded.is_finite() || rounded >= u64::MAX as f64 {
        return Err(TimeError::ConversionOverflow);
    }
    Ok(rounded as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    numerator: u16,
    denominator: u16,
}

impl TimeSignature {
    pub fn new(numerator: u16, denominator: u16) -> Result<Self, TimeError> {
        if numerator == 0
            || !denominator.is_power_of_two()
            || !(TICKS_PER_QUARTER * 4).is_multiple_of(u64::from(denominator))
        {
            return Err(TimeError::InvalidTimeSignature);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub fn numerator(self) -> u16 {
        self.numerator
    }

    pub fn denominator(self) -> u16 {
        self.denominator
    }

    pub fn ticks_per_beat(self) -> u64 {
        TICKS_PER_QUARTER * 4 / u64::from(self.denominator)
    }

    pub fn ticks_per_bar(self) -> u64 {
        self.ticks_per_beat() * u64::from(self.numerator)
    }

    pub fn position_at(self, tick: u64) -> MusicalPosition {
        let tick_in_bar = tick % self.ticks_per_bar();
        MusicalPosition {
            bar: tick / self.ticks_per_bar() + 1,
            beat: (tick_in_bar / self.ticks_per_beat()) as u16 + 1,
            tick: tick_in_bar % self.ticks_per_beat(),
        }
    }

    pub fn tick_at(self, position: MusicalPosition) -> Result<u64, TimeError> {
        if position.bar == 0
            || position.beat == 0
            || position.beat > self.numerator
            || position.tick >= self.ticks_per_beat()
        {
            return Err(TimeError::InvalidMusicalPosition);
        }
        let offset = u64::from(position.beat - 1) * self.ticks_per_beat() + position.tick;
        (position.bar - 1)
            .checked_mul(self.ticks_per_bar())
            .and_then(|bar_start| bar_start.checked_add(offset))
            .ok_or(TimeError::ConversionOverflow)
    }
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicalPosition {
    pub bar: u64,
    pub beat: u16,
    pub tick: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SnapGrid {
    #[default]
    Off,
    Bar,
    Quarter,
    Eighth,
    Sixteenth,
}

impl SnapGrid {
    pub fn interval(self, meter: TimeSignature) -> Option<u64> {
        match self {
            Self::Off => None,
            Self::Bar => Some(meter.ticks_per_bar()),
            Self::Quarter => Some(TICKS_PER_QUARTER),
            Self::Eighth => Some(TICKS_PER_QUARTER / 2),
            Self::Sixteenth => Some(TICKS_PER_QUARTER / 4),
        }
    }

    pub fn floor(self, tick: u64, meter: TimeSignature) -> u64 {
        self.interval(meter)
            .map_or(tick, |interval| tick - tick % interval)
    }

    pub fn snap(self, tick: u64, meter: TimeSignature) -> Result<u64, TimeError> {
        let Some(interval) = self.interval(meter) else {
            return Ok(tick);
        };
        let remainder = tick % interval;
        if remainder < interval - remainder {
            Ok(tick - remainder)
        } else {
            tick.checked_add(interval - remainder)
                .ok_or(TimeError::ConversionOverflow)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeError {
    InvalidTempo,
    InvalidSeconds,
    InvalidTimeSignature,
    InvalidMusicalPosition,
    ConversionOverflow,
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidTempo => {
                "tempo must be a positive, finite number of quarter notes per minute"
            }
            Self::InvalidSeconds => "time in seconds must be finite and nonnegative",
            Self::InvalidTimeSignature => {
                "time signature requires a positive numerator and a power-of-two denominator from 1 to 256"
            }
            Self::InvalidMusicalPosition => {
                "musical position requires a positive bar, a beat within the bar, and a tick offset within the beat"
            }
            Self::ConversionOverflow => "time cannot be represented in the destination unit",
        };
        f.write_str(message)
    }
}

impl Error for TimeError {}
