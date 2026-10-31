//! Calor as one of the engine's built-in effects. The processor itself lives in the
//! `sympoie-calor` crate, which the Calor plugin for other hosts runs as well; this module
//! maps its parameter table onto the engine's, feeds it the engine's audio, and switches it
//! to its offline path for bounces and exports.

use std::sync::Arc;

use crate::Frame;
use crate::instrument::NoteEvent;
use crate::plugin::{PluginInstance, PluginProcessContext, PluginProcessor, RenderQuality};

use super::{ParameterSpec, Scale, Telemetry};

pub use sympoie_calor::params::{
    AUTO_GAIN, BANDS, BIAS, COUNT, CROSSOVER_BASE, CROSSOVER_SLOPE, DRIVE, EMPHASIS,
    EMPHASIS_FREQUENCY, ENVELOPE_ATTACK, ENVELOPE_RELEASE, EVEN, HARDNESS, LFO_RATE, LFO_SHAPE,
    MAX_BANDS, MIX, MOD_AMOUNT, MOD_BAND, MOD_SOURCE, MOD_TARGET, ODD, OUTPUT, Slot, TONE,
    band_parameter, band_slot, mod_parameter,
};
pub use sympoie_calor::{Curve, HARMONICS, REFERENCE_LEVEL, Telemetry as CalorTelemetry, Transfer};

/// Delay of the processed signal behind its input at the engine's sample rate, the same
/// in real time and offline.
pub const LATENCY: u32 = 39;

fn spec(calor: &sympoie_calor::Spec) -> ParameterSpec {
    ParameterSpec {
        id: calor.id,
        name: calor.name,
        unit: calor.unit,
        min: calor.min,
        max: calor.max,
        default: calor.default,
        scale: match calor.scale {
            sympoie_calor::Scale::Linear => Scale::Linear,
            sympoie_calor::Scale::Log => Scale::Log,
        },
        steps: calor.steps,
    }
}

pub fn parameters() -> Vec<ParameterSpec> {
    sympoie_calor::PARAMETERS.iter().map(spec).collect()
}

/// The names of a stepped parameter's settings: models, channel modes, slopes, band
/// counts and modulation routings.
pub fn choices(id: u32) -> Option<&'static [&'static str]> {
    sympoie_calor::choices(id)
}

/// Every parameter of a Calor instance in physical units, indexed by id.
pub fn values(instance: &PluginInstance) -> [f64; COUNT] {
    std::array::from_fn(|id| {
        let spec = &sympoie_calor::PARAMETERS[id];
        instance
            .parameters
            .iter()
            .find(|parameter| parameter.id == spec.id)
            .map_or(spec.default, |parameter| spec.denormalize(parameter.value))
    })
}

#[derive(Debug)]
struct CalorEffect {
    calor: sympoie_calor::Calor,
}

pub fn create_calor(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    let mut calor = sympoie_calor::Calor::new(
        f64::from(crate::SAMPLE_RATE),
        telemetry.map(|telemetry| telemetry.calor.clone()),
    );
    for (id, value) in values(instance).into_iter().enumerate() {
        calor.set_parameter(id as u32, value);
    }
    calor.reset();
    Ok(Box::new(CalorEffect { calor }))
}

impl PluginProcessor for CalorEffect {
    fn reset(&mut self) {
        self.calor.reset();
    }

    fn event(&mut self, _event: NoteEvent) {}

    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        self.calor.set_quality(match context.quality {
            RenderQuality::Realtime => sympoie_calor::Quality::Realtime,
            RenderQuality::Offline => sympoie_calor::Quality::Offline,
        });
        self.calor.process(frames);
    }

    fn set_parameter(&mut self, id: u32, normalized: f64) {
        if normalized.is_finite()
            && let Some(spec) = sympoie_calor::PARAMETERS.get(id as usize)
        {
            self.calor
                .set_parameter(id, spec.denormalize(normalized.clamp(0.0, 1.0)));
        }
    }

    fn latency_samples(&self) -> u32 {
        self.calor.latency()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::BuiltinKind;

    fn context(quality: RenderQuality) -> PluginProcessContext {
        PluginProcessContext {
            frame: 0,
            bpm: 120.0,
            playing: true,
            quality,
        }
    }

    #[test]
    fn the_engine_sees_calor_s_parameters_switch_and_latency() {
        let specs = parameters();
        assert_eq!(specs.len(), COUNT);
        for (id, spec) in specs.iter().enumerate() {
            assert_eq!(spec.id as usize, id);
            let calor = &sympoie_calor::PARAMETERS[id];
            for position in [0.0, 0.3, 1.0] {
                assert_eq!(spec.denormalize(position), calor.denormalize(position));
            }
        }
        assert_eq!(
            crate::builtin::choices(BuiltinKind::Calor, AUTO_GAIN).map(<[_]>::len),
            Some(2)
        );
        assert_eq!(crate::builtin::latency_samples(BuiltinKind::Calor), LATENCY);
        let instance = BuiltinKind::Calor.instance();
        let processor = create_calor(&instance, None).unwrap();
        assert_eq!(processor.latency_samples(), LATENCY);
        let defaults = sympoie_calor::params::defaults();
        for (value, default) in values(&instance).into_iter().zip(defaults) {
            assert!((value - default).abs() <= 1.0e-9 * default.abs().max(1.0));
        }
    }

    #[test]
    fn automation_reaches_the_processor_and_offline_keeps_the_latency() {
        let mut instance = BuiltinKind::Calor.instance();
        for parameter in &mut instance.parameters {
            match parameter.id {
                OUTPUT => parameter.value = 1.0,
                MIX => parameter.value = 0.0,
                _ => {}
            }
        }
        for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
            // Output at +24 dB and Mix at 0 from the instance: the input, delayed and
            // louder.
            let mut processor = create_calor(&instance, None).unwrap();
            processor.set_parameter(DRIVE, f64::NAN);
            let mut frames = vec![[0.25_f32, -0.25]; 4096];
            processor.process(&mut frames, context(quality));
            assert!(
                frames[..LATENCY as usize]
                    .iter()
                    .all(|frame| frame[0] == 0.0)
            );
            let gain = frames[LATENCY as usize][0] / 0.25;
            assert!((gain - 15.85).abs() < 0.01, "{quality:?}: {gain}");
            // Automation turns Output back down, gliding.
            processor.set_parameter(OUTPUT, 0.5);
            let mut frames = vec![[0.25_f32, -0.25]; 9600];
            processor.process(&mut frames, context(quality));
            let gain = frames[9000][0] / 0.25;
            assert!((gain - 1.0).abs() < 0.01, "{quality:?}: {gain}");
            assert_eq!(processor.latency_samples(), LATENCY);
        }
    }
}
