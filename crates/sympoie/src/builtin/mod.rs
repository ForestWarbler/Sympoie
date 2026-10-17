pub mod effects;
pub mod instruments;

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::plugin::{
    MAX_PLUGIN_PARAMETERS, PluginDescriptor, PluginError, PluginInstance, PluginKind,
    PluginParameter, PluginProcessor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Oscil,
    Sampler,
    Eq,
    Comp,
}

impl BuiltinKind {
    pub const ALL: [Self; 4] = [Self::Oscil, Self::Sampler, Self::Eq, Self::Comp];

    pub fn name(self) -> &'static str {
        match self {
            Self::Oscil => "Oscil",
            Self::Sampler => "Sampler",
            Self::Eq => "EQ",
            Self::Comp => "Comp",
        }
    }

    pub fn descriptor(self) -> PluginDescriptor {
        let key = self.name().to_ascii_lowercase();
        PluginDescriptor {
            path: format!("sympoie:builtin/{key}").into(),
            class_id: format!("sympoie.builtin.{key}.v1"),
            name: self.name().to_owned(),
            vendor: "Sympoie".to_owned(),
            kind: match self {
                Self::Oscil | Self::Sampler => PluginKind::Instrument,
                _ => PluginKind::Effect,
            },
        }
    }

    pub fn from_descriptor(descriptor: &PluginDescriptor) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| {
            let expected = kind.descriptor();
            expected.path == descriptor.path
                && expected.class_id == descriptor.class_id
                && expected.kind == descriptor.kind
        })
    }

    pub fn instance(self) -> PluginInstance {
        let parameters = parameters(self)
            .into_iter()
            .map(|spec| PluginParameter {
                id: spec.id,
                name: spec.name.to_owned(),
                unit: spec.unit.to_owned(),
                value: spec.normalize(spec.default),
                steps: spec.steps,
                read_only: false,
            })
            .collect();
        PluginInstance::new(self.descriptor(), parameters, Vec::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Linear,
    Log,
}

#[derive(Debug, Clone, Copy)]
pub struct ParameterSpec {
    pub id: u32,
    pub name: &'static str,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub scale: Scale,
    pub steps: u32,
}

impl ParameterSpec {
    pub fn normalize(self, physical: f64) -> f64 {
        let physical = if physical.is_finite() {
            physical.clamp(self.min, self.max)
        } else {
            self.default
        };
        let value = match self.scale {
            Scale::Linear => (physical - self.min) / (self.max - self.min),
            Scale::Log => (physical / self.min).ln() / (self.max / self.min).ln(),
        };
        self.quantize(value)
    }

    pub fn denormalize(self, normalized: f64) -> f64 {
        if !normalized.is_finite() {
            return self.default;
        }
        let value = self.quantize(normalized);
        match self.scale {
            Scale::Linear => self.min + value * (self.max - self.min),
            Scale::Log => self.min * (self.max / self.min).powf(value),
        }
    }

    fn quantize(self, value: f64) -> f64 {
        let value = value.clamp(0.0, 1.0);
        if self.steps == 0 {
            value
        } else {
            (value * self.steps as f64).round() / self.steps as f64
        }
    }
}

pub const OSC_SHAPES: [&str; 4] = ["Sine", "Triangle", "Saw", "Pulse"];
pub const FILTER_TYPES: [&str; 3] = ["Low-pass", "High-pass", "Band-pass"];
pub const LFO_SHAPES: [&str; 3] = ["Sine", "Triangle", "Square"];
pub const PLAYBACK_MODES: [&str; 3] = ["One shot", "Gate", "Loop"];
pub const EQ_BAND_TYPES: [&str; 6] = [
    "Bell",
    "Low shelf",
    "High shelf",
    "High-pass",
    "Low-pass",
    "Notch",
];
pub const DETECTORS: [&str; 2] = ["Peak", "RMS"];
const SWITCH: [&str; 2] = ["Off", "On"];

pub fn choices(kind: BuiltinKind, id: u32) -> Option<&'static [&'static str]> {
    use instruments::{
        FILTER_TYPE, LFO_SHAPE, MATRIX_BASE, MATRIX_ROUTES, MATRIX_SOURCE, MATRIX_SOURCE_LABELS,
        MATRIX_STRIDE, MATRIX_TARGET, MATRIX_TARGET_LABELS, OSC_BASE, OSC_SHAPE, OSC_STRIDE,
    };
    let spec = parameters(kind).into_iter().find(|spec| spec.id == id)?;
    let named: Option<&'static [&'static str]> = match kind {
        BuiltinKind::Oscil => {
            let matrix = (MATRIX_BASE..MATRIX_BASE + MATRIX_ROUTES as u32 * MATRIX_STRIDE)
                .contains(&id)
                .then(|| (id - MATRIX_BASE) % MATRIX_STRIDE);
            if id == FILTER_TYPE {
                Some(&FILTER_TYPES)
            } else if id == LFO_SHAPE {
                Some(&LFO_SHAPES)
            } else if (OSC_BASE..OSC_BASE + 3 * OSC_STRIDE).contains(&id)
                && (id - OSC_BASE) % OSC_STRIDE == OSC_SHAPE
            {
                Some(&OSC_SHAPES)
            } else if matrix == Some(MATRIX_SOURCE) {
                Some(&MATRIX_SOURCE_LABELS)
            } else if matrix == Some(MATRIX_TARGET) {
                Some(&MATRIX_TARGET_LABELS)
            } else {
                None
            }
        }
        BuiltinKind::Sampler => (spec.name == "Playback").then_some(&PLAYBACK_MODES[..]),
        BuiltinKind::Eq => (spec.name.ends_with(" Type")).then_some(&EQ_BAND_TYPES[..]),
        BuiltinKind::Comp => (spec.name == "Detector").then_some(&DETECTORS[..]),
    };
    named.or_else(|| (spec.steps == 1 && spec.min == 0.0 && spec.max == 1.0).then_some(&SWITCH[..]))
}

pub fn parameters(kind: BuiltinKind) -> Vec<ParameterSpec> {
    match kind {
        BuiltinKind::Oscil => instruments::oscil_parameters(),
        BuiltinKind::Sampler => instruments::sampler_parameters(),
        BuiltinKind::Eq => effects::eq_parameters(),
        BuiltinKind::Comp => effects::comp_parameters(),
    }
}

pub fn complete_parameters(instance: &mut PluginInstance) -> Result<bool, PluginError> {
    instance.validate()?;
    let Some(kind) = BuiltinKind::from_descriptor(&instance.descriptor) else {
        return Ok(false);
    };
    let missing: Vec<_> = parameters(kind)
        .into_iter()
        .filter(|spec| {
            !instance
                .parameters
                .iter()
                .any(|parameter| parameter.id == spec.id)
        })
        .collect();
    if instance.parameters.len() + missing.len() > MAX_PLUGIN_PARAMETERS {
        return Err(PluginError::TooManyParameters);
    }
    let changed = !missing.is_empty();
    instance
        .parameters
        .extend(missing.into_iter().map(|spec| PluginParameter {
            id: spec.id,
            name: spec.name.to_owned(),
            unit: spec.unit.to_owned(),
            value: spec.normalize(spec.default),
            steps: spec.steps,
            read_only: false,
        }));
    Ok(changed)
}

pub fn value(instance: &PluginInstance, id: u32) -> f64 {
    let Some(kind) = BuiltinKind::from_descriptor(&instance.descriptor) else {
        return 0.0;
    };
    let Some(spec) = parameters(kind).into_iter().find(|spec| spec.id == id) else {
        return 0.0;
    };
    instance
        .parameters
        .iter()
        .find(|p| p.id == id)
        .map_or(spec.default, |p| spec.denormalize(p.value))
}

#[derive(Debug)]
pub struct Telemetry {
    pub spectrum: [AtomicU32; 128],
    pub input_db: AtomicU32,
    pub output_db: AtomicU32,
    pub reduction_db: AtomicU32,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            spectrum: std::array::from_fn(|_| AtomicU32::new((-120.0_f32).to_bits())),
            input_db: AtomicU32::new((-120.0_f32).to_bits()),
            output_db: AtomicU32::new((-120.0_f32).to_bits()),
            reduction_db: AtomicU32::new(0.0_f32.to_bits()),
        }
    }
}

pub fn create_processor(instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
    create_processor_with_telemetry(instance, None)
}

pub fn create_processor_with_telemetry(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    instance.validate().map_err(|e| e.to_string())?;
    match BuiltinKind::from_descriptor(&instance.descriptor).ok_or("Unknown built-in device")? {
        BuiltinKind::Oscil => instruments::create_oscil(instance),
        BuiltinKind::Sampler => instruments::create_sampler(instance),
        BuiltinKind::Eq => effects::create_eq(instance, telemetry),
        BuiltinKind::Comp => effects::create_comp(instance, telemetry),
    }
}

pub fn release_frames(
    instance: &PluginInstance,
    lanes: &[crate::automation::AutomationLane],
) -> Option<u64> {
    use crate::automation::AutomationTarget;
    let kind = BuiltinKind::from_descriptor(&instance.descriptor)?;
    if !matches!(kind, BuiltinKind::Oscil | BuiltinKind::Sampler) {
        return Some(0);
    }
    let release = parameters(kind).into_iter().find(|p| p.id == 13)?;
    let seconds = lanes
        .iter()
        .filter(|lane| {
            lane.enabled()
                && lane.target()
                    == AutomationTarget::PluginParameter {
                        plugin: instance.id,
                        parameter: 13,
                    }
        })
        .flat_map(|lane| lane.points())
        .map(|point| release.denormalize(point.value))
        .fold(value(instance, 13), f64::max);
    Some((seconds * crate::SAMPLE_RATE as f64).ceil() as u64)
}

#[cfg(test)]
mod choice_tests {
    use super::*;

    #[test]
    fn every_stepped_choice_list_matches_its_steps() {
        let mut named = 0;
        for kind in BuiltinKind::ALL {
            for spec in parameters(kind) {
                if let Some(labels) = choices(kind, spec.id) {
                    assert_eq!(
                        labels.len() as u32,
                        spec.steps + 1,
                        "{} {}",
                        kind.name(),
                        spec.name
                    );
                    named += 1;
                }
            }
        }
        assert!(named > 20);
        assert_eq!(choices(BuiltinKind::Oscil, 22), Some(&FILTER_TYPES[..]));
        assert_eq!(choices(BuiltinKind::Oscil, 120), Some(&OSC_SHAPES[..]));
        assert_eq!(choices(BuiltinKind::Oscil, 20), None);
    }
}
