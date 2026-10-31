pub mod calor;
pub mod effects;
pub mod ferro;
pub mod instruments;
pub mod rain;
pub mod reson;
pub mod varimu;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::plugin::{
    MAX_PLUGIN_PARAMETERS, PluginDescriptor, PluginError, PluginInstance, PluginKind,
    PluginParameter, PluginProcessor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Oscil,
    Sampler,
    Slicer,
    Eq,
    Comp,
    VariMu,
    Rain,
    Ferro,
    Reson,
    Calor,
}

impl BuiltinKind {
    pub const ALL: [Self; 10] = [
        Self::Oscil,
        Self::Sampler,
        Self::Slicer,
        Self::Eq,
        Self::Comp,
        Self::VariMu,
        Self::Rain,
        Self::Ferro,
        Self::Reson,
        Self::Calor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Oscil => "Oscil",
            Self::Sampler => "Sampler",
            Self::Slicer => "Slicer",
            Self::Eq => "EQ",
            Self::Comp => "Comp",
            Self::VariMu => "Vari-Mu",
            Self::Rain => "Rain",
            Self::Ferro => "Ferro",
            Self::Reson => "Reson",
            Self::Calor => "Calor",
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
                Self::Oscil | Self::Sampler | Self::Slicer | Self::Rain | Self::Ferro => {
                    PluginKind::Instrument
                }
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
/// Oscil's oscillator shapes: the shared four plus white noise.
pub const OSCIL_SHAPES: [&str; 5] = ["Sine", "Triangle", "Saw", "Pulse", "Noise"];
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
        FILTER_TYPE, LFO_SHAPE, LFO2_SHAPE, MATRIX_BASE, MATRIX_ROUTES, MATRIX_SOURCE,
        MATRIX_SOURCE_LABELS, MATRIX_STRIDE, MATRIX_TARGET, MATRIX_TARGET_LABELS, OSC_BASE,
        OSC_SHAPE, OSC_STRIDE,
    };
    let spec = parameters(kind).into_iter().find(|spec| spec.id == id)?;
    let named: Option<&'static [&'static str]> = match kind {
        BuiltinKind::Oscil => {
            let matrix = (MATRIX_BASE..MATRIX_BASE + MATRIX_ROUTES as u32 * MATRIX_STRIDE)
                .contains(&id)
                .then(|| (id - MATRIX_BASE) % MATRIX_STRIDE);
            if id == FILTER_TYPE {
                Some(&FILTER_TYPES)
            } else if id == LFO_SHAPE || id == LFO2_SHAPE {
                Some(&LFO_SHAPES)
            } else if (OSC_BASE..OSC_BASE + 3 * OSC_STRIDE).contains(&id)
                && (id - OSC_BASE) % OSC_STRIDE == OSC_SHAPE
            {
                Some(&OSCIL_SHAPES)
            } else if matrix == Some(MATRIX_SOURCE) {
                Some(&MATRIX_SOURCE_LABELS)
            } else if matrix == Some(MATRIX_TARGET) {
                Some(&MATRIX_TARGET_LABELS)
            } else {
                None
            }
        }
        BuiltinKind::Sampler => (spec.name == "Playback").then_some(&PLAYBACK_MODES[..]),
        BuiltinKind::Slicer => match id {
            instruments::SLICE_MODE => Some(&PLAYBACK_MODES[..]),
            instruments::SLICE_DETECT => Some(&instruments::SLICE_DETECTORS[..]),
            _ => None,
        },
        BuiltinKind::Eq => (spec.name.ends_with(" Type")).then_some(&EQ_BAND_TYPES[..]),
        BuiltinKind::Comp => (spec.name == "Detector").then_some(&DETECTORS[..]),
        BuiltinKind::VariMu => match id {
            varimu::RATIO => Some(&varimu::RATIOS[..]),
            varimu::TIME_CONSTANT => Some(&varimu::TIME_CONSTANTS[..]),
            varimu::STEREO => Some(&varimu::STEREO_MODES[..]),
            _ => None,
        },
        BuiltinKind::Rain => match id {
            rain::ORDER => Some(&rain::ORDERS[..]),
            rain::DIVISION => Some(&rain::DIVISIONS[..]),
            rain::SHAPE => Some(&OSC_SHAPES[..]),
            _ => None,
        },
        BuiltinKind::Ferro => ferro::choices(id),
        BuiltinKind::Reson => reson::choices(id),
        BuiltinKind::Calor => calor::choices(id),
    };
    named.or_else(|| (spec.steps == 1 && spec.min == 0.0 && spec.max == 1.0).then_some(&SWITCH[..]))
}

pub fn parameters(kind: BuiltinKind) -> Vec<ParameterSpec> {
    match kind {
        BuiltinKind::Oscil => instruments::oscil_parameters(),
        BuiltinKind::Sampler => instruments::sampler_parameters(),
        BuiltinKind::Slicer => instruments::slicer_parameters(),
        BuiltinKind::Eq => effects::eq_parameters(),
        BuiltinKind::Comp => effects::comp_parameters(),
        BuiltinKind::VariMu => varimu::parameters(),
        BuiltinKind::Rain => rain::parameters(),
        BuiltinKind::Ferro => ferro::parameters(),
        BuiltinKind::Reson => reson::parameters(),
        BuiltinKind::Calor => calor::parameters(),
    }
}

/// Samples by which a built-in device delays its signal, for delay compensation.
pub fn latency_samples(kind: BuiltinKind) -> u32 {
    match kind {
        BuiltinKind::Reson => reson::LATENCY,
        BuiltinKind::Calor => calor::LATENCY,
        _ => 0,
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
    let mut changed = !missing.is_empty();
    for (parameter, old, new) in choice_upgrades(instance) {
        let stored = instance
            .parameters
            .iter_mut()
            .find(|stored| stored.id == parameter)
            .expect("upgrades only name stored parameters");
        stored.value = upgrade_choice(stored.value, old, new);
        stored.steps = new;
        changed = true;
    }
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

/// Stepped parameters stored with fewer steps than the device now has, as
/// `(id, stored steps, current steps)`. Choice lists only ever grow at the end, so the
/// stored option index stays the same while its normalized value is rescaled.
pub fn choice_upgrades(instance: &PluginInstance) -> Vec<(u32, u32, u32)> {
    let Some(kind) = BuiltinKind::from_descriptor(&instance.descriptor) else {
        return Vec::new();
    };
    let specs = parameters(kind);
    instance
        .parameters
        .iter()
        .filter_map(|stored| {
            let spec = specs.iter().find(|spec| spec.id == stored.id)?;
            (stored.steps > 0 && spec.steps > stored.steps).then_some((
                stored.id,
                stored.steps,
                spec.steps,
            ))
        })
        .collect()
}

/// Rescales a normalized choice value saved with `old` steps to one with `new` steps.
pub fn upgrade_choice(value: f64, old: u32, new: u32) -> f64 {
    ((value.clamp(0.0, 1.0) * f64::from(old)).round() / f64::from(new)).clamp(0.0, 1.0)
}

/// Names of the device's factory presets, empty for devices without any.
pub fn preset_names(kind: BuiltinKind) -> Vec<&'static str> {
    match kind {
        BuiltinKind::Rain => rain::PRESETS.iter().map(|preset| preset.name).collect(),
        BuiltinKind::Ferro => ferro::PRESETS.iter().map(|preset| preset.name).collect(),
        _ => Vec::new(),
    }
}

/// Every parameter of a factory preset as `(id, normalized value)`.
pub fn preset_parameters(kind: BuiltinKind, preset: usize) -> Option<Vec<(u32, f64)>> {
    match kind {
        BuiltinKind::Rain => rain::preset_parameters(preset),
        BuiltinKind::Ferro => ferro::preset_parameters(preset),
        _ => None,
    }
}

/// The factory preset an instance currently matches, if any.
pub fn matching_preset(instance: &PluginInstance) -> Option<usize> {
    match BuiltinKind::from_descriptor(&instance.descriptor)? {
        BuiltinKind::Rain => rain::matching_preset(instance),
        BuiltinKind::Ferro => ferro::matching_preset(instance),
        _ => None,
    }
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
    /// Spectrum of the device's output (dB per log-spaced band, 20 Hz–20 kHz).
    pub spectrum: [AtomicU32; 128],
    /// Spectrum of the device's input, for devices that compare before and after.
    pub input_spectrum: [AtomicU32; 128],
    /// Reson's spectra, cuts, notes and meters, which its own interface draws.
    pub reson: Arc<reson::ResonTelemetry>,
    /// Calor's meters, which its own interface draws.
    pub calor: Arc<calor::CalorTelemetry>,
    /// Counts spectrum refreshes. Spectra survive a device reset (the engine resets devices
    /// at every loop wrap), so a display that sees this stop advancing should let them fade.
    pub spectrum_updates: AtomicU32,
    pub input_db: AtomicU32,
    pub output_db: AtomicU32,
    pub reduction_db: AtomicU32,
    pub scope: Scope,
    /// Drops the rain drum has landed, for its pond display.
    pub rain_drops: rain::DropLog,
    /// Keys the rain drum is holding, one bit per MIDI key.
    pub rain_held: [AtomicU64; 2],
    /// Loudness contour level of each Ferro voice (f32 bits), 0 when the voice is free.
    pub ferro_voices: [AtomicU32; ferro::MAX_VOICES],
    /// The host's offline render of this instrument, for devices that can play one:
    /// a [`RenderState`] and the progress of a render in flight (f32 bits, 0..1).
    pub render_state: AtomicU32,
    pub render_progress: AtomicU32,
}

/// What the host is doing with an instrument's cached offline render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RenderState {
    /// Playing the realtime engine; no render is wanted.
    Live = 0,
    /// A render is queued or running; the realtime engine plays meanwhile.
    Rendering = 1,
    /// Playback comes from the finished render.
    Ready = 2,
    /// The last render failed; the realtime engine plays.
    Failed = 3,
}

impl Telemetry {
    pub fn render_state(&self) -> RenderState {
        match self.render_state.load(Ordering::Relaxed) {
            1 => RenderState::Rendering,
            2 => RenderState::Ready,
            3 => RenderState::Failed,
            _ => RenderState::Live,
        }
    }

    pub fn set_render_state(&self, state: RenderState, progress: f32) {
        self.render_progress
            .store(progress.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.render_state.store(state as u32, Ordering::Relaxed);
    }

    pub fn render_progress(&self) -> f32 {
        f32::from_bits(self.render_progress.load(Ordering::Relaxed))
    }
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            spectrum: std::array::from_fn(|_| AtomicU32::new((-120.0_f32).to_bits())),
            input_spectrum: std::array::from_fn(|_| AtomicU32::new((-120.0_f32).to_bits())),
            reson: Arc::default(),
            calor: Arc::default(),
            spectrum_updates: AtomicU32::new(0),
            input_db: AtomicU32::new((-120.0_f32).to_bits()),
            output_db: AtomicU32::new((-120.0_f32).to_bits()),
            reduction_db: AtomicU32::new(0.0_f32.to_bits()),
            scope: Scope::default(),
            rain_drops: rain::DropLog::default(),
            rain_held: [AtomicU64::new(0), AtomicU64::new(0)],
            ferro_voices: std::array::from_fn(|_| AtomicU32::new(0)),
            render_state: AtomicU32::new(0),
            render_progress: AtomicU32::new(0),
        }
    }
}

pub const SCOPE_COLUMNS: usize = 1024;
/// Frames summarised by one scope column; 1024 columns hold about two seconds.
pub const SCOPE_FRAMES_PER_COLUMN: usize = 96;

/// One summarised slice of before/after audio.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScopeColumn {
    pub input_min: f32,
    pub input_max: f32,
    pub input_power: f32,
    pub output_min: f32,
    pub output_max: f32,
    pub output_power: f32,
    pub reduction_db: f32,
}

/// Lock-free history of before/after waveform columns, written by the audio thread and
/// read by the interface. A reader racing the writer may see one half-updated column,
/// which only affects a single pixel column of the display.
#[derive(Debug)]
pub struct Scope {
    columns: Box<[[AtomicU32; 7]]>,
    written: AtomicU32,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            columns: (0..SCOPE_COLUMNS)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                .collect(),
            written: AtomicU32::new(0),
        }
    }
}

impl Scope {
    pub fn push(&self, column: ScopeColumn) {
        let written = self.written.load(Ordering::Relaxed);
        let slot = &self.columns[written as usize % SCOPE_COLUMNS];
        let values = [
            column.input_min,
            column.input_max,
            column.input_power,
            column.output_min,
            column.output_max,
            column.output_power,
            column.reduction_db,
        ];
        for (cell, value) in slot.iter().zip(values) {
            cell.store(value.to_bits(), Ordering::Relaxed);
        }
        self.written
            .store(written.wrapping_add(1), Ordering::Release);
    }

    /// Columns written so far; wraps after about three months of audio.
    pub fn written(&self) -> u32 {
        self.written.load(Ordering::Acquire)
    }

    /// The most recent columns, oldest first.
    pub fn snapshot(&self) -> Vec<ScopeColumn> {
        let written = self.written();
        let count = (written as usize).min(SCOPE_COLUMNS);
        (0..count)
            .map(|age| {
                let index = written.wrapping_sub((count - age) as u32) as usize % SCOPE_COLUMNS;
                let read =
                    |i: usize| f32::from_bits(self.columns[index][i].load(Ordering::Relaxed));
                ScopeColumn {
                    input_min: read(0),
                    input_max: read(1),
                    input_power: read(2),
                    output_min: read(3),
                    output_max: read(4),
                    output_power: read(5),
                    reduction_db: read(6),
                }
            })
            .collect()
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
        BuiltinKind::Slicer => instruments::create_slicer(instance),
        BuiltinKind::Eq => effects::create_eq(instance, telemetry),
        BuiltinKind::Comp => effects::create_comp(instance, telemetry),
        BuiltinKind::VariMu => varimu::create_varimu(instance, telemetry),
        BuiltinKind::Rain => rain::create_rain(instance, telemetry),
        BuiltinKind::Ferro => ferro::create_ferro(instance, telemetry),
        BuiltinKind::Reson => reson::create_reson(instance, telemetry),
        BuiltinKind::Calor => calor::create_calor(instance, telemetry),
    }
}

pub fn release_frames(
    instance: &PluginInstance,
    lanes: &[crate::automation::AutomationLane],
) -> Option<u64> {
    use crate::automation::AutomationTarget;
    let kind = BuiltinKind::from_descriptor(&instance.descriptor)?;
    if !matches!(
        kind,
        BuiltinKind::Oscil
            | BuiltinKind::Sampler
            | BuiltinKind::Slicer
            | BuiltinKind::Rain
            | BuiltinKind::Ferro
    ) {
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
        .flat_map(|lane| {
            let stored = instance.parameters.iter().find(|p| p.id == 13);
            lane.trimmed_values(stored.map_or(lane.anchor().unwrap_or(0.0), |p| p.value))
        })
        .map(|value| release.denormalize(value))
        .fold(value(instance, 13), f64::max);
    let seconds = if kind == BuiltinKind::Rain {
        seconds + rain::extra_tail_seconds(instance)
    } else {
        seconds
    };
    Some((seconds * crate::SAMPLE_RATE as f64).ceil() as u64)
}

#[cfg(test)]
mod choice_tests {
    use super::*;

    #[test]
    fn scope_history_wraps_and_reads_oldest_first() {
        let scope = Scope::default();
        assert!(scope.snapshot().is_empty());
        for index in 0..1500 {
            scope.push(ScopeColumn {
                reduction_db: index as f32,
                ..ScopeColumn::default()
            });
        }
        let columns = scope.snapshot();
        assert_eq!(columns.len(), SCOPE_COLUMNS);
        assert_eq!(columns[0].reduction_db, (1500 - SCOPE_COLUMNS) as f32);
        assert_eq!(columns[SCOPE_COLUMNS - 1].reduction_db, 1499.0);
        assert!(
            columns
                .windows(2)
                .all(|pair| pair[1].reduction_db == pair[0].reduction_db + 1.0)
        );
    }

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
        assert_eq!(choices(BuiltinKind::Oscil, 120), Some(&OSCIL_SHAPES[..]));
        assert_eq!(choices(BuiltinKind::Oscil, 53), Some(&LFO_SHAPES[..]));
        assert_eq!(choices(BuiltinKind::Oscil, 20), None);
    }
}
