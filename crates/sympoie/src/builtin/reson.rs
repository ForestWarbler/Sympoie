//! Reson as one of the engine's built-in effects. The processor itself lives in the
//! `sympoie-reson` crate, which the Reson plugin for other hosts runs as well; this module
//! maps its parameter table onto the engine's and feeds it the engine's audio.

use std::sync::Arc;

use crate::Frame;
use crate::instrument::NoteEvent;
use crate::plugin::{PluginInstance, PluginProcessContext, PluginProcessor};

use super::{ParameterSpec, Scale, Telemetry};

pub use sympoie_reson::params::{
    ATTACK, BAND_BASE, BAND_FREQUENCY, BAND_GAIN, BAND_Q, BAND_SHAPES, BAND_STRIDE, BANDS,
    BandShape, CLEAN, COUNT, DELTA, DEPTH, FOCUS, HIGH_CUT, KNEE, LOW_CUT, MIX, OUTPUT, RELEASE,
    STEREO, STEREO_MODES, Sensitivity, THRESHOLD, WIDTH,
};
pub use sympoie_reson::{REDUCTION_BANDS, SPECTRUM_BANDS, Telemetry as ResonTelemetry};

/// Delay of the processed signal behind its input at the engine's sample rate.
pub const LATENCY: u32 = 256;

fn spec(reson: &sympoie_reson::Spec) -> ParameterSpec {
    ParameterSpec {
        id: reson.id,
        name: reson.name,
        unit: reson.unit,
        min: reson.min,
        max: reson.max,
        default: reson.default,
        scale: match reson.scale {
            sympoie_reson::Scale::Linear => Scale::Linear,
            sympoie_reson::Scale::Log => Scale::Log,
        },
        steps: reson.steps,
    }
}

pub fn parameters() -> Vec<ParameterSpec> {
    sympoie_reson::PARAMETERS.iter().map(spec).collect()
}

pub fn choices(id: u32) -> Option<&'static [&'static str]> {
    sympoie_reson::choices(id)
}

/// Every parameter of a Reson instance in physical units, indexed by id.
pub fn values(instance: &PluginInstance) -> [f64; COUNT] {
    std::array::from_fn(|id| {
        let spec = &sympoie_reson::PARAMETERS[id];
        instance
            .parameters
            .iter()
            .find(|parameter| parameter.id == spec.id)
            .map_or(spec.default, |parameter| spec.denormalize(parameter.value))
    })
}

/// The sensitivity curve of a Reson instance.
pub fn sensitivity(instance: &PluginInstance) -> Sensitivity {
    Sensitivity::from_values(&values(instance))
}

#[derive(Debug)]
struct ResonEffect {
    reson: sympoie_reson::Reson,
}

pub fn create_reson(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    let mut reson = sympoie_reson::Reson::new(
        f64::from(crate::SAMPLE_RATE),
        telemetry.map(|telemetry| telemetry.reson.clone()),
    );
    for (id, value) in values(instance).into_iter().enumerate() {
        reson.set_parameter(id as u32, value);
    }
    reson.reset();
    Ok(Box::new(ResonEffect { reson }))
}

impl PluginProcessor for ResonEffect {
    fn reset(&mut self) {
        self.reson.reset();
    }

    fn event(&mut self, _event: NoteEvent) {}

    fn process(&mut self, frames: &mut [Frame], _context: PluginProcessContext) {
        self.reson.process(frames);
    }

    fn set_parameter(&mut self, id: u32, normalized: f64) {
        if normalized.is_finite()
            && let Some(spec) = sympoie_reson::PARAMETERS.get(id as usize)
        {
            self.reson
                .set_parameter(id, spec.denormalize(normalized.clamp(0.0, 1.0)));
        }
    }

    fn latency_samples(&self) -> u32 {
        self.reson.latency()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::BuiltinKind;

    #[test]
    fn the_engine_sees_reson_s_parameters_choices_and_latency() {
        let specs = parameters();
        assert_eq!(specs.len(), COUNT);
        for (id, spec) in specs.iter().enumerate() {
            assert_eq!(spec.id as usize, id);
            let reson = &sympoie_reson::PARAMETERS[id];
            for position in [0.0, 0.3, 1.0] {
                assert_eq!(spec.denormalize(position), reson.denormalize(position));
            }
        }
        assert_eq!(
            crate::builtin::choices(BuiltinKind::Reson, DELTA).map(<[_]>::len),
            Some(2)
        );
        assert_eq!(crate::builtin::latency_samples(BuiltinKind::Reson), LATENCY);
        let instance = BuiltinKind::Reson.instance();
        let processor = create_reson(&instance, None).unwrap();
        assert_eq!(processor.latency_samples(), LATENCY);
        let defaults = sympoie_reson::params::defaults();
        for (value, default) in values(&instance).into_iter().zip(defaults) {
            assert!((value - default).abs() <= 1.0e-9 * default.abs().max(1.0));
        }
    }

    #[test]
    fn automation_reaches_the_processor_in_physical_units() {
        let mut instance = BuiltinKind::Reson.instance();
        for parameter in &mut instance.parameters {
            if parameter.id == OUTPUT {
                parameter.value = 1.0;
            }
        }
        let mut processor = create_reson(&instance, None).unwrap();
        // Output at +12 dB from the instance; Mix at 0 passes the input through.
        processor.set_parameter(MIX, 0.0);
        processor.set_parameter(DEPTH, f64::NAN);
        let mut frames = vec![[0.25_f32, -0.25]; 4096];
        let context = PluginProcessContext {
            frame: 0,
            bpm: 120.0,
            playing: true,
            quality: crate::plugin::RenderQuality::Realtime,
        };
        processor.process(&mut frames, context);
        let gain = frames[4000][0] / 0.25;
        assert!((gain - 3.981).abs() < 0.01, "{gain}");
    }
}
