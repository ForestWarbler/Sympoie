use crate::instrument::{Instrument, SubtractiveSettings, Waveform};
use crate::plugin::PluginInstance;
use crate::project::ProjectError;
use crate::track::Track;

pub const MAX_AUTOMATION_LANES: usize = 4_096;
pub const MAX_AUTOMATION_POINTS: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubtractiveParameter {
    Waveform,
    Cutoff,
    Resonance,
    Attack,
    Decay,
    Sustain,
    Release,
    Gain,
}

impl SubtractiveParameter {
    pub const ALL: [Self; 8] = [
        Self::Waveform,
        Self::Cutoff,
        Self::Resonance,
        Self::Attack,
        Self::Decay,
        Self::Sustain,
        Self::Release,
        Self::Gain,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Waveform => "Waveform",
            Self::Cutoff => "Cutoff",
            Self::Resonance => "Resonance",
            Self::Attack => "Attack",
            Self::Decay => "Decay",
            Self::Sustain => "Sustain",
            Self::Release => "Release",
            Self::Gain => "Gain",
        }
    }

    pub fn normalized_value(self, settings: &SubtractiveSettings) -> f64 {
        match self {
            Self::Waveform => match settings.waveform {
                Waveform::Sine => 0.0,
                Waveform::Saw => 0.5,
                Waveform::Square => 1.0,
            },
            Self::Cutoff => (f64::from(settings.cutoff_hz) / 20.0).ln() / 1000.0_f64.ln(),
            Self::Resonance => (f64::from(settings.resonance) - 0.5) / 9.5,
            Self::Attack => f64::from(settings.attack_seconds) / 10.0,
            Self::Decay => f64::from(settings.decay_seconds) / 10.0,
            Self::Sustain => f64::from(settings.sustain),
            Self::Release => f64::from(settings.release_seconds) / 10.0,
            Self::Gain => f64::from(settings.gain),
        }
        .clamp(0.0, 1.0)
    }

    pub fn set_normalized(self, settings: &mut SubtractiveSettings, value: f64) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match self {
            Self::Waveform => {
                settings.waveform = match (value * 2.0).round() as u8 {
                    0 => Waveform::Sine,
                    1 => Waveform::Saw,
                    _ => Waveform::Square,
                }
            }
            Self::Cutoff => settings.cutoff_hz = (20.0 * 1000.0_f64.powf(value)) as f32,
            Self::Resonance => settings.resonance = (0.5 + value * 9.5) as f32,
            Self::Attack => settings.attack_seconds = (value * 10.0) as f32,
            Self::Decay => settings.decay_seconds = (value * 10.0) as f32,
            Self::Sustain => settings.sustain = value as f32,
            Self::Release => settings.release_seconds = (value * 10.0) as f32,
            Self::Gain => settings.gain = value as f32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutomationTarget {
    TrackGain,
    TrackPan,
    Subtractive(SubtractiveParameter),
    PluginParameter { plugin: u64, parameter: u32 },
}

fn plugins(track: &Track) -> impl Iterator<Item = &PluginInstance> {
    let instrument = match track {
        Track::Midi(midi) => midi.plugin_instrument(),
        _ => None,
    };
    instrument.into_iter().chain(track.all_effects())
}

impl AutomationTarget {
    pub fn available_for(track: &Track) -> Vec<Self> {
        let mut targets = vec![Self::TrackGain, Self::TrackPan];
        if let Track::Midi(midi) = track
            && midi.instrument().is_some()
        {
            targets.extend(SubtractiveParameter::ALL.map(Self::Subtractive));
        }
        for plugin in plugins(track) {
            targets.extend(plugin.parameters.iter().filter(|p| !p.read_only).map(|p| {
                Self::PluginParameter {
                    plugin: plugin.id,
                    parameter: p.id,
                }
            }));
        }
        targets
    }

    pub fn value(self, track: &Track) -> Option<f64> {
        match self {
            Self::TrackGain => Some((f64::from(track.mixer().gain()) / 2.0).clamp(0.0, 1.0)),
            Self::TrackPan => Some((f64::from(track.mixer().pan()) + 1.0) / 2.0),
            Self::Subtractive(parameter) => match track {
                Track::Midi(midi) => midi
                    .instrument()
                    .map(|Instrument::Subtractive(settings)| parameter.normalized_value(settings)),
                _ => None,
            },
            Self::PluginParameter { plugin, parameter } => plugins(track)
                .find(|p| p.id == plugin)?
                .parameters
                .iter()
                .find(|p| p.id == parameter && !p.read_only)
                .map(|p| p.value),
        }
    }

    pub fn label(self, track: &Track) -> String {
        match self {
            Self::TrackGain => "Mixer · Volume".into(),
            Self::TrackPan => "Mixer · Pan".into(),
            Self::Subtractive(parameter) => format!("Subtractive · {}", parameter.label()),
            Self::PluginParameter { plugin, parameter } => plugins(track)
                .find(|p| p.id == plugin)
                .and_then(|p| {
                    p.parameters
                        .iter()
                        .find(|parameter_info| parameter_info.id == parameter)
                        .map(|info| format!("{} · {}", p.descriptor.name, info.name))
                })
                .unwrap_or_else(|| "Missing parameter".into()),
        }
    }

    pub fn format_value(self, value: f64) -> String {
        let value = value.clamp(0.0, 1.0);
        match self {
            Self::TrackGain if value == 0.0 => "−∞ dB".into(),
            Self::TrackGain => format!("{:+.1} dB", 20.0 * (value * 2.0).log10()),
            Self::TrackPan if (value - 0.5).abs() < 0.005 => "C".into(),
            Self::TrackPan => format!(
                "{:.0} {}",
                (value * 2.0 - 1.0).abs() * 100.0,
                if value < 0.5 { "L" } else { "R" }
            ),
            Self::Subtractive(SubtractiveParameter::Waveform) => {
                match (value * 2.0).round() as u8 {
                    0 => "Sine",
                    1 => "Saw",
                    _ => "Square",
                }
                .into()
            }
            Self::Subtractive(SubtractiveParameter::Cutoff) => {
                format!("{:.0} Hz", 20.0 * 1000.0_f64.powf(value))
            }
            Self::Subtractive(SubtractiveParameter::Resonance) => {
                format!("{:.2} Q", 0.5 + value * 9.5)
            }
            Self::Subtractive(
                SubtractiveParameter::Attack
                | SubtractiveParameter::Decay
                | SubtractiveParameter::Release,
            ) => format!("{:.2} s", value * 10.0),
            _ => format!("{:.1}%", value * 100.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutomationPoint {
    pub tick: u64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationLane {
    target: AutomationTarget,
    points: Vec<AutomationPoint>,
    enabled: bool,
}

impl AutomationLane {
    pub fn new(
        target: AutomationTarget,
        points: Vec<AutomationPoint>,
    ) -> Result<Self, ProjectError> {
        if points.len() > MAX_AUTOMATION_POINTS {
            return Err(ProjectError::TooManyAutomationPoints);
        }
        if points
            .iter()
            .any(|p| !p.value.is_finite() || !(0.0..=1.0).contains(&p.value))
        {
            return Err(ProjectError::InvalidAutomationValue);
        }
        if points.windows(2).any(|pair| pair[0].tick >= pair[1].tick) {
            return Err(ProjectError::InvalidAutomationOrder);
        }
        Ok(Self {
            target,
            points,
            enabled: true,
        })
    }

    pub fn target(&self) -> AutomationTarget {
        self.target
    }
    pub fn points(&self) -> &[AutomationPoint] {
        &self.points
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn with_enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    pub fn value_at(&self, tick: f64) -> Option<f64> {
        if !self.enabled || self.points.is_empty() || !tick.is_finite() {
            return None;
        }
        let upper = self
            .points
            .partition_point(|point| point.tick as f64 <= tick);
        if upper == 0 {
            return Some(self.points[0].value);
        }
        let previous = self.points[upper - 1];
        let Some(next) = self.points.get(upper) else {
            return Some(previous.value);
        };
        let fraction = (tick - previous.tick as f64) / (next.tick - previous.tick) as f64;
        Some(previous.value + (next.value - previous.value) * fraction)
    }
}
