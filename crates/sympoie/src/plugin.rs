use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use crate::Frame;
use crate::instrument::NoteEvent;

pub const MAX_PLUGIN_STRING_BYTES: usize = 65_536;
pub const MAX_PLUGIN_PARAMETERS: usize = 65_536;
pub const MAX_PLUGIN_STATE_BYTES: usize = 64 * 1024 * 1024;
pub const PLUGIN_TAIL_FRAMES: u64 = crate::SAMPLE_RATE as u64 * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    Instrument,
    Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidechainTap {
    PreEffects,
    PostEffects,
    PostFader,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidechainRoute {
    pub source_track: usize,
    pub bus_index: u32,
    pub gain: f32,
    pub tap: SidechainTap,
}

impl SidechainRoute {
    pub fn validate(&self) -> Result<(), PluginError> {
        if self.bus_index == 0 {
            return Err(PluginError::InvalidSidechainBus);
        }
        if !self.gain.is_finite() || !(0.0..=4.0).contains(&self.gain) {
            return Err(PluginError::InvalidSidechainGain);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDescriptor {
    pub path: PathBuf,
    pub class_id: String,
    pub name: String,
    pub vendor: String,
    pub kind: PluginKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginParameter {
    pub id: u32,
    pub name: String,
    pub unit: String,
    pub value: f64,
    pub steps: u32,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginInstance {
    pub id: u64,
    pub descriptor: PluginDescriptor,
    pub parameters: Vec<PluginParameter>,
    pub state: Vec<u8>,
    pub bypassed: bool,
    pub sidechain: Option<SidechainRoute>,
}

impl PluginInstance {
    pub fn new(
        descriptor: PluginDescriptor,
        parameters: Vec<PluginParameter>,
        state: Vec<u8>,
    ) -> Self {
        Self {
            id: 0,
            descriptor,
            parameters,
            state,
            bypassed: false,
            sidechain: None,
        }
    }

    pub fn validate(&self) -> Result<(), PluginError> {
        if let Some(route) = self.sidechain {
            if self.descriptor.kind != PluginKind::Effect {
                return Err(PluginError::SidechainRequiresEffect);
            }
            route.validate()?;
        }
        let descriptor = &self.descriptor;
        let path = descriptor
            .path
            .to_str()
            .ok_or(PluginError::InvalidDescriptor)?;
        for text in [path, descriptor.class_id.as_str(), descriptor.name.as_str()] {
            if text.trim().is_empty() || !valid_text(text) {
                return Err(PluginError::InvalidDescriptor);
            }
        }
        if !valid_text(&descriptor.vendor) {
            return Err(PluginError::InvalidDescriptor);
        }
        if self.parameters.len() > MAX_PLUGIN_PARAMETERS {
            return Err(PluginError::TooManyParameters);
        }
        if self.state.len() > MAX_PLUGIN_STATE_BYTES {
            return Err(PluginError::StateTooLarge);
        }
        let mut ids = HashSet::with_capacity(self.parameters.len());
        for parameter in &self.parameters {
            if !valid_text(&parameter.name) || !valid_text(&parameter.unit) {
                return Err(PluginError::InvalidParameterMetadata);
            }
            if !ids.insert(parameter.id) {
                return Err(PluginError::DuplicateParameter(parameter.id));
            }
            if !parameter.value.is_finite() || !(0.0..=1.0).contains(&parameter.value) {
                return Err(PluginError::InvalidParameterValue);
            }
        }
        Ok(())
    }
}

fn valid_text(text: &str) -> bool {
    text.len() <= MAX_PLUGIN_STRING_BYTES && !text.contains('\0')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    InvalidDescriptor,
    InvalidParameterMetadata,
    InvalidParameterValue,
    DuplicateParameter(u32),
    TooManyParameters,
    StateTooLarge,
    WrongKind,
    InvalidInstanceId,
    UnknownInstance(u64),
    UnknownParameter(u32),
    ReadOnlyParameter(u32),
    InvalidSidechainBus,
    InvalidSidechainGain,
    SidechainRequiresEffect,
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDescriptor => write!(f, "plugin path, class ID, or name is invalid"),
            Self::InvalidParameterMetadata => write!(f, "plugin parameter metadata is invalid"),
            Self::InvalidParameterValue => {
                write!(f, "plugin parameter must be finite and in 0..=1")
            }
            Self::DuplicateParameter(id) => write!(f, "duplicate plugin parameter {id}"),
            Self::TooManyParameters => write!(f, "plugin has too many parameters"),
            Self::StateTooLarge => write!(f, "plugin state exceeds the size limit"),
            Self::WrongKind => write!(f, "plugin kind does not match the device slot"),
            Self::InvalidInstanceId => write!(f, "plugin instance ID is invalid or already used"),
            Self::UnknownInstance(id) => write!(f, "unknown plugin instance {id}"),
            Self::UnknownParameter(id) => write!(f, "unknown plugin parameter {id}"),
            Self::ReadOnlyParameter(id) => write!(f, "plugin parameter {id} is read-only"),
            Self::InvalidSidechainBus => write!(f, "sidechain must select an auxiliary input bus"),
            Self::InvalidSidechainGain => write!(f, "sidechain gain must be finite and in 0..=4"),
            Self::SidechainRequiresEffect => {
                write!(f, "sidechains are available on effect plugins")
            }
        }
    }
}

impl Error for PluginError {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginProcessContext {
    pub frame: u64,
    pub bpm: f64,
    pub playing: bool,
    /// How hard the device may work. Offline renders (bounce, export, cached renders)
    /// have no deadline, so devices may trade speed for accuracy.
    pub quality: RenderQuality,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RenderQuality {
    #[default]
    Realtime,
    Offline,
}

#[derive(Debug, Clone, Copy)]
pub struct PluginSidechainInput<'a> {
    pub bus_index: u32,
    pub frames: &'a [Frame],
}

pub trait PluginProcessor: fmt::Debug + Send {
    fn reset(&mut self);
    fn event(&mut self, event: NoteEvent);
    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext);
    fn set_parameter(&mut self, id: u32, value: f64);
    fn set_automation_parameter(&mut self, id: u32, value: f64) {
        self.set_parameter(id, value);
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        _sidechain: Option<PluginSidechainInput<'_>>,
        context: PluginProcessContext,
    ) {
        self.process(frames, context);
    }
}

pub trait PluginHost {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String>;
    fn load_builtin(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        crate::builtin::create_processor(instance)
    }
}
