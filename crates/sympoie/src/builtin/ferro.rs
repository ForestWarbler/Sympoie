//! Ferro: an analog-modelled synthesizer after the classic hardware.
//!
//! Its layout follows the big American monosynths: three free-running oscillators with
//! wide footage ranges, a noise source and a sub-oscillator in the mixer, a four-pole
//! low-pass with its own contour, a loudness contour, and portamento. Its circuits are
//! the Juno's: saw-core DCOs with comparator pulses and coupling capacitors, and an
//! IR3109 OTA filter.
//! A voice switch makes it monophonic (with last-note priority and legato), or a small
//! polysynth like the Japanese classics, which also lend it a fixed high-pass filter and
//! a bucket-brigade stereo chorus.
//!
//! Two engines share one model:
//! - **Realtime** runs the voice at 2× oversampling, solves the filter once per sample
//!   from its state, and updates oscillator pitch and filter coefficients every
//!   [`CONTROL_FRAMES`] samples.
//! - **Offline** (bounce, export, and the cached render the device can play instead of
//!   the realtime engine) runs at 8× with steeper decimation, resolves the filter's
//!   nonlinear loop until it settles, and moves pitch, glide and cutoff continuously at
//!   the oversampled rate, so slides and sweeps are perfectly smooth.
//!
//! Per-note slides (`NoteEvent::On::slide_frames`) glide from the previous pitch and
//! continue the sounding contour instead of retriggering it, in every voice mode. Pitch
//! bend ramps arrive as `NoteEvent::PitchBend` and are followed sample by sample.
//!
//! Analog behaviour is deterministic: component tolerances are fixed per voice and the
//! random walks that drift pitch and cutoff restart from the same seed at every reset.

#[path = "ferro/dsp.rs"]
mod dsp;
#[cfg(test)]
#[path = "ferro/tests.rs"]
mod tests;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::Frame;
use crate::instrument::NoteEvent;
use crate::plugin::{PluginInstance, PluginProcessContext, PluginProcessor, RenderQuality};

use super::effects::{RATE, parameter};
use super::{ParameterSpec, Scale, Telemetry};
use dsp::{
    COUPLING_HZ, Chorus, Contour, ContourRates, Decimator, Drift, FilterSolve, Glide, Ir3109,
    JunoHpf, OnePole, Pink, Rng, Stage, Sub, Vco, compensation_error, filter_gain,
    one_pole_coefficient,
};

pub use dsp::{CHORUS_MODES, WAVES};

pub const GAIN: u32 = 0;
pub const TUNE: u32 = 1;
pub const VELOCITY: u32 = 2;
pub const VOICE_MODE: u32 = 3;
pub const VOICES: u32 = 4;
pub const DRIFT: u32 = 5;
pub const BEND_RANGE: u32 = 6;
pub const GLIDE: u32 = 7;
pub const GLIDE_MODE: u32 = 8;
pub const GLIDE_CURVE: u32 = 9;
pub const AMP_ATTACK: u32 = 10;
pub const AMP_DECAY: u32 = 11;
pub const AMP_SUSTAIN: u32 = 12;
/// Shares the id the host reads as an instrument's release tail.
pub const AMP_RELEASE: u32 = 13;
pub const CUTOFF: u32 = 20;
pub const EMPHASIS: u32 = 21;
pub const CONTOUR: u32 = 22;
pub const KEY_TRACK: u32 = 23;
pub const DRIVE: u32 = 24;
pub const HPF: u32 = 25;
pub const FILTER_VELOCITY: u32 = 26;
pub const FILTER_ATTACK: u32 = 30;
pub const FILTER_DECAY: u32 = 31;
pub const FILTER_SUSTAIN: u32 = 32;
pub const FILTER_RELEASE: u32 = 33;
pub const LFO_RATE: u32 = 40;
pub const LFO_SHAPE: u32 = 41;
pub const MOD_PITCH: u32 = 42;
pub const MOD_FILTER: u32 = 43;
pub const MOD_PWM: u32 = 44;
pub const LFO_DELAY: u32 = 45;
pub const MOD_SOURCE: u32 = 46;
pub const NOISE: u32 = 50;
pub const NOISE_COLOR: u32 = 51;
pub const FEEDBACK: u32 = 52;
pub const SUB: u32 = 53;
pub const CHORUS: u32 = 60;
/// Live plays the realtime engine; Render asks the host to play a cached offline render.
pub const ENGINE: u32 = 70;
pub const OSC_BASE: u32 = 100;
pub const OSC_STRIDE: u32 = 10;
pub const OSC_ON: u32 = 0;
pub const OSC_RANGE: u32 = 1;
pub const OSC_WAVE: u32 = 2;
pub const OSC_TUNE: u32 = 3;
pub const OSC_LEVEL: u32 = 4;
/// Oscillator 3 following the keyboard; off turns it into a drone or a modulator.
pub const OSC3_KEYBOARD: u32 = OSC_BASE + 2 * OSC_STRIDE + 6;

pub const VOICE_MODES: [&str; 3] = ["Poly", "Mono", "Legato"];
pub const GLIDE_MODES: [&str; 3] = ["Off", "Always", "Legato"];
pub const KEY_TRACKS: [&str; 4] = ["Off", "1/3", "2/3", "Full"];
pub const HPF_POSITIONS: [&str; 4] = ["0", "1", "2", "3"];
pub const LFO_SHAPES: [&str; 4] = ["Triangle", "Square", "Saw", "S&H"];
pub const MOD_SOURCES: [&str; 3] = ["LFO", "Osc 3", "Noise"];
pub const NOISE_COLORS: [&str; 2] = ["White", "Pink"];
pub const RANGES: [&str; 6] = ["Lo", "32'", "16'", "8'", "4'", "2'"];
pub const ENGINES: [&str; 2] = ["Live", "Render"];
/// Semitones each range position adds to the key.
const RANGE_SEMITONES: [f64; 6] = [-84.0, -24.0, -12.0, 0.0, 12.0, 24.0];

pub const MAX_VOICES: usize = 8;
/// Realtime mode recomputes pitch and cutoff coefficients every this many samples.
pub const CONTROL_FRAMES: u64 = 4;
const REALTIME_OVERSAMPLING: usize = 2;
const OFFLINE_OVERSAMPLING: usize = 8;
const HELD_NOTES: usize = 32;
/// Parameter smoothing time constant.
const SMOOTH_SECONDS: f64 = 0.004;
/// Bend jumps (live wheels, seeks) are smoothed this much; scheduled ramps are exact.
const BEND_SMOOTH_SECONDS: f64 = 0.002;
const PARAMETER_SLOTS: usize = 256;

const fn osc(index: u32, offset: u32) -> u32 {
    OSC_BASE + index * OSC_STRIDE + offset
}

pub const PARAMETER_COUNT: usize = 54;

const PARAMETERS: [ParameterSpec; PARAMETER_COUNT] = [
    parameter(GAIN, "Volume", "dB", (-60.0, 6.0), -8.0, Scale::Linear, 0),
    parameter(TUNE, "Tune", "cent", (-100.0, 100.0), 0.0, Scale::Linear, 0),
    parameter(VELOCITY, "Velocity", "", (0.0, 1.0), 0.3, Scale::Linear, 0),
    parameter(
        VOICE_MODE,
        "Voice mode",
        "",
        (0.0, 2.0),
        0.0,
        Scale::Linear,
        2,
    ),
    parameter(VOICES, "Voices", "", (1.0, 8.0), 6.0, Scale::Linear, 7),
    parameter(DRIFT, "Drift", "%", (0.0, 100.0), 35.0, Scale::Linear, 0),
    parameter(
        BEND_RANGE,
        "Bend range",
        "st",
        (0.0, 12.0),
        2.0,
        Scale::Linear,
        12,
    ),
    parameter(GLIDE, "Glide", "s", (0.005, 5.0), 0.08, Scale::Log, 0),
    parameter(
        GLIDE_MODE,
        "Glide mode",
        "",
        (0.0, 2.0),
        0.0,
        Scale::Linear,
        2,
    ),
    parameter(
        GLIDE_CURVE,
        "Glide curve",
        "",
        (0.0, 1.0),
        0.0,
        Scale::Linear,
        0,
    ),
    parameter(
        AMP_ATTACK,
        "Attack",
        "s",
        (0.001, 10.0),
        0.004,
        Scale::Log,
        0,
    ),
    parameter(AMP_DECAY, "Decay", "s", (0.005, 10.0), 0.6, Scale::Log, 0),
    parameter(
        AMP_SUSTAIN,
        "Sustain",
        "",
        (0.0, 1.0),
        0.85,
        Scale::Linear,
        0,
    ),
    parameter(
        AMP_RELEASE,
        "Release",
        "s",
        (0.005, 10.0),
        0.25,
        Scale::Log,
        0,
    ),
    parameter(
        CUTOFF,
        "Cutoff",
        "Hz",
        (20.0, 20_000.0),
        1800.0,
        Scale::Log,
        0,
    ),
    parameter(EMPHASIS, "Emphasis", "", (0.0, 10.0), 2.5, Scale::Linear, 0),
    parameter(CONTOUR, "Contour", "", (0.0, 10.0), 4.0, Scale::Linear, 0),
    parameter(KEY_TRACK, "Keyboard", "", (0.0, 3.0), 1.0, Scale::Linear, 3),
    parameter(DRIVE, "Drive", "", (0.0, 10.0), 3.0, Scale::Linear, 0),
    parameter(HPF, "HPF", "", (0.0, 3.0), 1.0, Scale::Linear, 3),
    parameter(
        FILTER_VELOCITY,
        "Filter velocity",
        "",
        (0.0, 1.0),
        0.2,
        Scale::Linear,
        0,
    ),
    parameter(
        FILTER_ATTACK,
        "Filter attack",
        "s",
        (0.001, 10.0),
        0.004,
        Scale::Log,
        0,
    ),
    parameter(
        FILTER_DECAY,
        "Filter decay",
        "s",
        (0.005, 10.0),
        0.45,
        Scale::Log,
        0,
    ),
    parameter(
        FILTER_SUSTAIN,
        "Filter sustain",
        "",
        (0.0, 1.0),
        0.25,
        Scale::Linear,
        0,
    ),
    parameter(
        FILTER_RELEASE,
        "Filter release",
        "s",
        (0.005, 10.0),
        0.3,
        Scale::Log,
        0,
    ),
    parameter(LFO_RATE, "LFO rate", "Hz", (0.05, 50.0), 5.0, Scale::Log, 0),
    parameter(
        LFO_SHAPE,
        "LFO shape",
        "",
        (0.0, 3.0),
        0.0,
        Scale::Linear,
        3,
    ),
    parameter(
        MOD_PITCH,
        "Mod pitch",
        "st",
        (0.0, 12.0),
        0.0,
        Scale::Linear,
        0,
    ),
    parameter(
        MOD_FILTER,
        "Mod filter",
        "oct",
        (0.0, 4.0),
        0.0,
        Scale::Linear,
        0,
    ),
    parameter(MOD_PWM, "Mod PWM", "", (0.0, 1.0), 0.0, Scale::Linear, 0),
    parameter(
        LFO_DELAY,
        "LFO delay",
        "s",
        (0.0, 5.0),
        0.0,
        Scale::Linear,
        0,
    ),
    parameter(
        MOD_SOURCE,
        "Mod source",
        "",
        (0.0, 2.0),
        0.0,
        Scale::Linear,
        2,
    ),
    parameter(NOISE, "Noise", "", (0.0, 10.0), 0.0, Scale::Linear, 0),
    parameter(
        NOISE_COLOR,
        "Noise color",
        "",
        (0.0, 1.0),
        0.0,
        Scale::Linear,
        1,
    ),
    parameter(FEEDBACK, "Feedback", "", (0.0, 10.0), 0.0, Scale::Linear, 0),
    parameter(SUB, "Sub", "", (0.0, 10.0), 0.0, Scale::Linear, 0),
    parameter(CHORUS, "Chorus", "", (0.0, 3.0), 0.0, Scale::Linear, 3),
    parameter(ENGINE, "Engine", "", (0.0, 1.0), 0.0, Scale::Linear, 1),
    parameter(
        osc(0, OSC_ON),
        "Osc 1",
        "",
        (0.0, 1.0),
        1.0,
        Scale::Linear,
        1,
    ),
    parameter(
        osc(0, OSC_RANGE),
        "Osc 1 range",
        "",
        (0.0, 5.0),
        3.0,
        Scale::Linear,
        5,
    ),
    parameter(
        osc(0, OSC_WAVE),
        "Osc 1 wave",
        "",
        (0.0, 5.0),
        2.0,
        Scale::Linear,
        5,
    ),
    parameter(
        osc(0, OSC_TUNE),
        "Osc 1 tune",
        "st",
        (-12.0, 12.0),
        0.0,
        Scale::Linear,
        0,
    ),
    parameter(
        osc(0, OSC_LEVEL),
        "Osc 1 level",
        "",
        (0.0, 10.0),
        10.0,
        Scale::Linear,
        0,
    ),
    parameter(
        osc(1, OSC_ON),
        "Osc 2",
        "",
        (0.0, 1.0),
        1.0,
        Scale::Linear,
        1,
    ),
    parameter(
        osc(1, OSC_RANGE),
        "Osc 2 range",
        "",
        (0.0, 5.0),
        3.0,
        Scale::Linear,
        5,
    ),
    parameter(
        osc(1, OSC_WAVE),
        "Osc 2 wave",
        "",
        (0.0, 5.0),
        2.0,
        Scale::Linear,
        5,
    ),
    parameter(
        osc(1, OSC_TUNE),
        "Osc 2 tune",
        "st",
        (-12.0, 12.0),
        0.07,
        Scale::Linear,
        0,
    ),
    parameter(
        osc(1, OSC_LEVEL),
        "Osc 2 level",
        "",
        (0.0, 10.0),
        8.0,
        Scale::Linear,
        0,
    ),
    parameter(
        osc(2, OSC_ON),
        "Osc 3",
        "",
        (0.0, 1.0),
        0.0,
        Scale::Linear,
        1,
    ),
    parameter(
        osc(2, OSC_RANGE),
        "Osc 3 range",
        "",
        (0.0, 5.0),
        2.0,
        Scale::Linear,
        5,
    ),
    parameter(
        osc(2, OSC_WAVE),
        "Osc 3 wave",
        "",
        (0.0, 5.0),
        3.0,
        Scale::Linear,
        5,
    ),
    parameter(
        osc(2, OSC_TUNE),
        "Osc 3 tune",
        "st",
        (-12.0, 12.0),
        -0.05,
        Scale::Linear,
        0,
    ),
    parameter(
        osc(2, OSC_LEVEL),
        "Osc 3 level",
        "",
        (0.0, 10.0),
        6.0,
        Scale::Linear,
        0,
    ),
    parameter(
        OSC3_KEYBOARD,
        "Osc 3 keyboard",
        "",
        (0.0, 1.0),
        1.0,
        Scale::Linear,
        1,
    ),
];

pub fn parameters() -> Vec<ParameterSpec> {
    PARAMETERS.to_vec()
}

pub fn choices(id: u32) -> Option<&'static [&'static str]> {
    let oscillator = (OSC_BASE..OSC_BASE + 3 * OSC_STRIDE)
        .contains(&id)
        .then(|| (id - OSC_BASE) % OSC_STRIDE);
    Some(match id {
        VOICE_MODE => &VOICE_MODES,
        GLIDE_MODE => &GLIDE_MODES,
        KEY_TRACK => &KEY_TRACKS,
        HPF => &HPF_POSITIONS,
        LFO_SHAPE => &LFO_SHAPES,
        MOD_SOURCE => &MOD_SOURCES,
        NOISE_COLOR => &NOISE_COLORS,
        CHORUS => &CHORUS_MODES,
        ENGINE => &ENGINES,
        _ if oscillator == Some(OSC_RANGE) => &RANGES,
        _ if oscillator == Some(OSC_WAVE) => &WAVES,
        _ => return None,
    })
}

/// Whether an instance asks the host to play its cached offline render.
pub fn renders_offline(instance: &PluginInstance) -> bool {
    super::value(instance, ENGINE) >= 0.5
}

/// A named set of settings; values not listed keep their defaults, so the first preset is
/// the device's default state.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub values: &'static [(u32, f64)],
}

pub const PRESETS: [Preset; 8] = [
    Preset {
        name: "Ferro init",
        values: &[],
    },
    Preset {
        name: "Taurus bass",
        values: &[
            (GAIN, -5.0),
            (VOICE_MODE, 1.0),
            (osc(0, OSC_RANGE), 1.0),
            (osc(1, OSC_RANGE), 2.0),
            (osc(1, OSC_WAVE), 3.0),
            (osc(1, OSC_TUNE), 0.05),
            (SUB, 4.0),
            (CUTOFF, 320.0),
            (EMPHASIS, 3.0),
            (CONTOUR, 5.5),
            (FILTER_DECAY, 0.35),
            (FILTER_SUSTAIN, 0.15),
            (DRIVE, 5.0),
            (AMP_SUSTAIN, 1.0),
            (AMP_RELEASE, 0.12),
            (GLIDE, 0.06),
            (GLIDE_MODE, 2.0),
        ],
    },
    Preset {
        name: "Lead one",
        values: &[
            (GAIN, -4.0),
            (VOICE_MODE, 2.0),
            (osc(1, OSC_TUNE), 0.12),
            (osc(2, OSC_ON), 1.0),
            (osc(2, OSC_RANGE), 3.0),
            (osc(2, OSC_TUNE), 7.0),
            (osc(2, OSC_LEVEL), 4.0),
            (CUTOFF, 1400.0),
            (EMPHASIS, 4.5),
            (CONTOUR, 4.5),
            (FEEDBACK, 2.5),
            (DRIVE, 6.0),
            (GLIDE, 0.12),
            (GLIDE_MODE, 2.0),
            (MOD_PITCH, 0.25),
            (LFO_RATE, 5.5),
            (LFO_DELAY, 0.6),
        ],
    },
    Preset {
        name: "Brass",
        values: &[
            (osc(1, OSC_TUNE), 0.1),
            (CUTOFF, 700.0),
            (EMPHASIS, 1.5),
            (CONTOUR, 6.0),
            (FILTER_ATTACK, 0.09),
            (FILTER_DECAY, 0.8),
            (FILTER_SUSTAIN, 0.55),
            (AMP_ATTACK, 0.03),
            (AMP_RELEASE, 0.35),
            (CHORUS, 1.0),
        ],
    },
    Preset {
        name: "Poly strings",
        values: &[
            (osc(0, OSC_WAVE), 4.0),
            (osc(1, OSC_WAVE), 2.0),
            (osc(1, OSC_TUNE), 0.15),
            (osc(1, OSC_RANGE), 4.0),
            (osc(1, OSC_LEVEL), 5.0),
            (MOD_PWM, 0.6),
            (LFO_RATE, 0.7),
            (CUTOFF, 3800.0),
            (EMPHASIS, 0.8),
            (CONTOUR, 1.0),
            (HPF, 2.0),
            (AMP_ATTACK, 0.45),
            (AMP_RELEASE, 1.6),
            (CHORUS, 2.0),
        ],
    },
    Preset {
        name: "Dream pad",
        values: &[
            (osc(0, OSC_WAVE), 3.0),
            (osc(1, OSC_WAVE), 2.0),
            (osc(1, OSC_TUNE), -0.18),
            (SUB, 3.0),
            (MOD_PWM, 0.45),
            (MOD_FILTER, 0.6),
            (LFO_RATE, 0.25),
            (CUTOFF, 1200.0),
            (EMPHASIS, 3.5),
            (CONTOUR, 3.0),
            (FILTER_ATTACK, 1.8),
            (FILTER_DECAY, 3.0),
            (FILTER_SUSTAIN, 0.4),
            (AMP_ATTACK, 1.2),
            (AMP_RELEASE, 3.0),
            (DRIFT, 60.0),
            (CHORUS, 3.0),
        ],
    },
    Preset {
        name: "Acid slide",
        values: &[
            (GAIN, 0.0),
            (VOICE_MODE, 2.0),
            (osc(0, OSC_RANGE), 2.0),
            (osc(1, OSC_ON), 0.0),
            (CUTOFF, 420.0),
            (EMPHASIS, 7.8),
            (CONTOUR, 6.5),
            (FILTER_DECAY, 0.22),
            (FILTER_SUSTAIN, 0.0),
            (DRIVE, 6.5),
            (AMP_DECAY, 0.4),
            (AMP_SUSTAIN, 0.7),
            (AMP_RELEASE, 0.04),
            (GLIDE, 0.07),
            (GLIDE_CURVE, 0.4),
            (VELOCITY, 0.6),
            (FILTER_VELOCITY, 0.5),
        ],
    },
    Preset {
        name: "Drone",
        values: &[
            (GAIN, -2.0),
            (VOICE_MODE, 1.0),
            (osc(0, OSC_RANGE), 1.0),
            (osc(0, OSC_WAVE), 1.0),
            (osc(1, OSC_RANGE), 2.0),
            (osc(1, OSC_WAVE), 1.0),
            (osc(1, OSC_TUNE), 0.2),
            (osc(2, OSC_ON), 1.0),
            (osc(2, OSC_RANGE), 0.0),
            (osc(2, OSC_WAVE), 0.0),
            (osc(2, OSC_LEVEL), 0.0),
            (OSC3_KEYBOARD, 0.0),
            (MOD_SOURCE, 1.0),
            (MOD_FILTER, 1.5),
            (NOISE, 1.5),
            (NOISE_COLOR, 1.0),
            (CUTOFF, 600.0),
            (EMPHASIS, 6.0),
            (CONTOUR, 2.0),
            (FEEDBACK, 4.0),
            (AMP_ATTACK, 2.0),
            (AMP_RELEASE, 4.0),
            (DRIFT, 80.0),
        ],
    },
];

/// Every parameter of a preset as `(id, normalized value)`. The engine switch is not part
/// of a sound, so presets keep whatever the instance has.
pub fn preset_parameters(preset: usize) -> Option<Vec<(u32, f64)>> {
    let preset = PRESETS.get(preset)?;
    Some(
        PARAMETERS
            .iter()
            .filter(|spec| spec.id != ENGINE)
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

pub fn create_ferro(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    Ok(Box::new(Ferro::new(instance, telemetry)))
}

fn spec(id: u32) -> Option<&'static ParameterSpec> {
    PARAMETERS.iter().find(|spec| spec.id == id)
}

/// Fixed per-voice component tolerances, as if each voice card were built from slightly
/// different parts.
#[derive(Debug, Clone, Copy)]
struct Tolerance {
    /// Oscillator detune in cents, per VCO.
    detune: [f64; 3],
    /// Cutoff offset in octaves.
    cutoff: f64,
    /// Contour time scale.
    time: f64,
    /// Oscillator level error (integrator component spread), per VCO.
    level: [f64; 3],
    /// Pulse comparator offset, per VCO.
    width: [f64; 3],
    /// Seeds for each VCO's amplitude-compensation error across the keyboard.
    compensation: [u64; 3],
}

impl Tolerance {
    fn for_voice(index: usize) -> Self {
        let mut rng =
            Rng::new(0x51f1_7e3d_a5a5_0001 ^ (index as u64 + 1).wrapping_mul(0x9e37_79b9));
        for _ in 0..4 {
            rng.next_u64();
        }
        Self {
            detune: [rng.bipolar(), rng.bipolar(), rng.bipolar()],
            cutoff: rng.bipolar(),
            time: rng.bipolar(),
            level: [rng.bipolar(), rng.bipolar(), rng.bipolar()],
            width: [rng.bipolar(), rng.bipolar(), rng.bipolar()],
            compensation: [rng.next_u64(), rng.next_u64(), rng.next_u64()],
        }
    }
}

#[derive(Debug, Clone)]
struct Voice {
    id: Option<u64>,
    key: u8,
    velocity: f64,
    gate: bool,
    age: u64,
    glide: Glide,
    vco: [Vco; 3],
    /// Each VCO's coupling capacitor into the mixer.
    coupling: [OnePole; 3],
    sub: Sub,
    pink: Pink,
    vcf: Ir3109,
    amp: Contour,
    filter: Contour,
    /// Drift shared by the voice's oscillators (a common supply), plus each VCO's own
    /// slow wander and fast jitter, so stacked oscillators keep shifting against each
    /// other.
    pitch_drift: Drift,
    osc_drift: [Drift; 3],
    jitter: [Drift; 3],
    cutoff_drift: Drift,
    tolerance: Tolerance,
    /// 0→1 fade of the LFO after a note starts, for the LFO delay.
    lfo_fade: f64,
    feedback: f64,
    osc3: f64,
    /// Oscillator steps (cycles per oversampled sample) and filter gain at the last
    /// control update; offline mode interpolates from these to the new values.
    steps: [f64; 4],
    gain: f64,
    /// Per-VCO level including tolerance and compensation error, from the last control
    /// update.
    amplitude: [f64; 3],
    previous_steps: [f64; 4],
    previous_gain: f64,
    dirty: bool,
}

impl Voice {
    fn new(index: usize) -> Self {
        Self {
            id: None,
            key: 60,
            velocity: 0.0,
            gate: false,
            age: 0,
            glide: Glide::default(),
            vco: [Vco::default(); 3],
            coupling: [OnePole::default(); 3],
            sub: Sub::default(),
            pink: Pink::default(),
            vcf: Ir3109::default(),
            amp: Contour::default(),
            filter: Contour::default(),
            pitch_drift: Drift::default(),
            osc_drift: [Drift::default(); 3],
            jitter: [Drift::default(); 3],
            cutoff_drift: Drift::default(),
            tolerance: Tolerance::for_voice(index),
            lfo_fade: 0.0,
            feedback: 0.0,
            osc3: 0.0,
            steps: [0.0; 4],
            gain: 0.0,
            amplitude: [1.0; 3],
            previous_steps: [0.0; 4],
            previous_gain: 0.0,
            dirty: true,
        }
    }

    fn sounding(&self) -> bool {
        self.amp.stage != Stage::Idle
    }
}

#[derive(Debug, Clone, Copy)]
struct Held {
    id: u64,
    key: u8,
    velocity: u8,
}

#[derive(Debug, Clone, Copy, Default)]
struct Bend {
    /// Scheduled value, ramping linearly toward `target`.
    value: f64,
    target: f64,
    remaining: u32,
    /// Output after the jump smoother.
    smoothed: f64,
}

#[derive(Debug)]
pub struct Ferro {
    current: [f64; PARAMETER_SLOTS],
    target: [f64; PARAMETER_SLOTS],
    voices: Vec<Voice>,
    held: Vec<Held>,
    last_pitch: Option<f64>,
    bend: Bend,
    lfo_phase: f64,
    lfo_hold: f64,
    lfo_value: f64,
    rng: Rng,
    chorus: Chorus,
    hpf: JunoHpf,
    decimator: Decimator,
    quality: RenderQuality,
    clock: u64,
    age: u64,
    scratch: Vec<f64>,
    telemetry: Option<Arc<Telemetry>>,
}

const SEED: u64 = 0xfe44_0c0d_e5a1_7e01;

impl Ferro {
    pub fn new(instance: &PluginInstance, telemetry: Option<Arc<Telemetry>>) -> Self {
        let mut current = [0.0; PARAMETER_SLOTS];
        for spec in &PARAMETERS {
            current[spec.id as usize] = instance
                .parameters
                .iter()
                .find(|p| p.id == spec.id)
                .map_or(spec.default, |p| spec.denormalize(p.value));
        }
        Self {
            current,
            target: current,
            voices: (0..MAX_VOICES).map(Voice::new).collect(),
            held: Vec::with_capacity(HELD_NOTES),
            last_pitch: None,
            bend: Bend::default(),
            lfo_phase: 0.0,
            lfo_hold: 0.0,
            lfo_value: 0.0,
            rng: Rng::new(SEED),
            chorus: Chorus::new(),
            hpf: JunoHpf::default(),
            decimator: Decimator::new(REALTIME_OVERSAMPLING, false),
            quality: RenderQuality::Realtime,
            clock: 0,
            age: 0,
            scratch: vec![0.0; OFFLINE_OVERSAMPLING],
            telemetry,
        }
    }

    fn get(&self, id: u32) -> f64 {
        self.current[id as usize]
    }

    fn choice(&self, id: u32) -> u32 {
        self.get(id).round().max(0.0) as u32
    }

    fn oversampling(&self) -> usize {
        match self.quality {
            RenderQuality::Realtime => REALTIME_OVERSAMPLING,
            RenderQuality::Offline => OFFLINE_OVERSAMPLING,
        }
    }

    fn exact(&self) -> bool {
        self.quality == RenderQuality::Offline
    }

    fn set_quality(&mut self, quality: RenderQuality) {
        if quality != self.quality {
            self.quality = quality;
            self.decimator = Decimator::new(self.oversampling(), self.exact());
            for voice in &mut self.voices {
                voice.dirty = true;
            }
        }
    }

    fn voice_limit(&self) -> usize {
        if self.choice(VOICE_MODE) == 0 {
            (self.choice(VOICES) as usize).clamp(1, MAX_VOICES)
        } else {
            1
        }
    }

    fn glide_frames(&self) -> f64 {
        self.get(GLIDE) * RATE
    }

    fn contour_rates(&self, base: u32, skew: f64) -> ContourRates {
        let scale = 1.0 + 0.06 * skew * self.get(DRIFT) / 100.0;
        ContourRates::new(
            self.get(base) * scale,
            self.get(base + 1) * scale,
            self.get(base + 2),
            self.get(base + 3) * scale,
            RATE,
        )
    }

    fn start_voice(&mut self, index: usize, held: Held, from: Option<f64>, frames: f64) {
        self.age += 1;
        let curve = self.get(GLIDE_CURVE);
        let fresh = !self.voices[index].sounding();
        let phases = [
            (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64,
            (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64,
            (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64,
        ];
        let voice = &mut self.voices[index];
        if fresh {
            // A silent voice's oscillators have been running unheard; pick up mid-cycle.
            for (vco, phase) in voice.vco.iter_mut().zip(phases) {
                vco.phase = phase;
            }
            voice.lfo_fade = 0.0;
            voice.feedback = 0.0;
            voice.vcf.reset();
            for coupling in &mut voice.coupling {
                coupling.reset();
            }
        }
        voice.id = Some(held.id);
        voice.key = held.key;
        voice.velocity = f64::from(held.velocity) / 127.0;
        voice.gate = true;
        voice.age = self.age;
        let target = f64::from(held.key);
        match from {
            Some(pitch) if frames >= 1.0 => {
                if fresh {
                    voice.glide.jump(pitch);
                }
                voice.glide.to(target, frames, curve);
            }
            _ => voice.glide.jump(target),
        }
        voice.amp.gate_on();
        voice.filter.gate_on();
        voice.dirty = true;
    }

    /// Moves a sounding voice to a new note without retriggering its contours.
    fn slide_voice(&mut self, index: usize, held: Held, frames: f64) {
        self.age += 1;
        let curve = self.get(GLIDE_CURVE);
        let voice = &mut self.voices[index];
        voice.id = Some(held.id);
        voice.key = held.key;
        voice.gate = true;
        voice.age = self.age;
        voice.glide.to(f64::from(held.key), frames, curve);
        for contour in [&mut voice.amp, &mut voice.filter] {
            match contour.stage {
                Stage::Idle => contour.gate_on(),
                Stage::Release => contour.stage = Stage::Decay,
                Stage::Attack | Stage::Decay => {}
            }
        }
        voice.dirty = true;
    }

    fn allocate(&self, id: u64) -> usize {
        let limit = self.voice_limit();
        let voices = &self.voices[..limit];
        if let Some(index) = voices.iter().position(|voice| voice.id == Some(id)) {
            return index;
        }
        let oldest = |filter: &dyn Fn(&Voice) -> bool| {
            voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| filter(voice))
                .min_by_key(|(_, voice)| voice.age)
                .map(|(index, _)| index)
        };
        oldest(&|voice| !voice.sounding())
            .or_else(|| oldest(&|voice| !voice.gate))
            .or_else(|| oldest(&|_| true))
            .unwrap_or(0)
    }

    fn note_on(&mut self, id: u64, key: u8, velocity: u8, slide_frames: u32) {
        let held = Held { id, key, velocity };
        let glide_mode = self.choice(GLIDE_MODE);
        let slide = f64::from(slide_frames);
        if self.choice(VOICE_MODE) == 0 {
            let overlapping = self.voices.iter().any(|voice| voice.gate);
            if slide_frames > 0
                && let Some(index) = (0..self.voice_limit())
                    .filter(|&index| self.voices[index].sounding())
                    .max_by_key(|&index| self.voices[index].age)
            {
                self.slide_voice(index, held, slide);
            } else {
                let index = self.allocate(id);
                let frames = match glide_mode {
                    1 => self.glide_frames(),
                    2 if overlapping => self.glide_frames(),
                    _ => 0.0,
                };
                self.start_voice(index, held, self.last_pitch, frames);
            }
        } else {
            self.held.retain(|note| note.id != id);
            let overlapping = !self.held.is_empty();
            if self.held.len() == HELD_NOTES {
                self.held.remove(0);
            }
            self.held.push(held);
            let sounding = self.voices[0].sounding();
            let legato_mode = self.choice(VOICE_MODE) == 2;
            let frames = if slide_frames > 0 {
                slide
            } else {
                match glide_mode {
                    1 if sounding || self.last_pitch.is_some() => self.glide_frames(),
                    2 if overlapping => self.glide_frames(),
                    _ => 0.0,
                }
            };
            let retrigger = slide_frames == 0 && !(legato_mode && overlapping);
            if retrigger {
                let from = sounding
                    .then_some(self.voices[0].glide.pitch)
                    .or(self.last_pitch);
                self.start_voice(0, held, from, frames);
            } else {
                self.slide_voice(0, held, frames);
                self.voices[0].velocity = f64::from(velocity) / 127.0;
            }
        }
        self.last_pitch = Some(f64::from(key));
    }

    fn note_off(&mut self, id: u64) {
        if self.choice(VOICE_MODE) == 0 {
            for voice in self.voices.iter_mut().filter(|voice| voice.id == Some(id)) {
                if voice.gate {
                    voice.gate = false;
                    voice.amp.gate_off();
                    voice.filter.gate_off();
                }
            }
            return;
        }
        let Some(position) = self.held.iter().position(|note| note.id == id) else {
            return;
        };
        self.held.remove(position);
        if self.voices[0].id != Some(id) {
            return;
        }
        if let Some(&top) = self.held.last() {
            // Last-note priority: fall back to the newest key still held, legato.
            let frames = match self.choice(GLIDE_MODE) {
                1 | 2 => self.glide_frames(),
                _ => 0.0,
            };
            self.slide_voice(0, top, frames);
            self.last_pitch = Some(f64::from(top.key));
        } else {
            let voice = &mut self.voices[0];
            voice.gate = false;
            voice.amp.gate_off();
            voice.filter.gate_off();
        }
    }

    fn advance_parameters(&mut self) {
        let k = 1.0 - (-1.0 / (SMOOTH_SECONDS * RATE)).exp();
        for spec in &PARAMETERS {
            if spec.steps == 0 {
                let i = spec.id as usize;
                let difference = self.target[i] - self.current[i];
                if difference != 0.0 {
                    self.current[i] = if difference.abs() < 1.0e-9 * (spec.max - spec.min) {
                        self.target[i]
                    } else {
                        self.current[i] + difference * k
                    };
                }
            }
        }
    }

    fn advance_bend(&mut self) -> f64 {
        let bend = &mut self.bend;
        if bend.remaining > 0 {
            bend.value += (bend.target - bend.value) / f64::from(bend.remaining);
            bend.remaining -= 1;
            // Scheduled ramps are followed exactly.
            bend.smoothed = bend.value;
        } else {
            bend.value = bend.target;
            let k = 1.0 - (-1.0 / (BEND_SMOOTH_SECONDS * RATE)).exp();
            bend.smoothed += (bend.value - bend.smoothed) * k;
        }
        bend.smoothed * self.get(BEND_RANGE)
    }

    fn advance_lfo(&mut self) -> f64 {
        let step = self.get(LFO_RATE) / RATE;
        let previous = self.lfo_phase;
        self.lfo_phase = (self.lfo_phase + step).fract();
        let wrapped = self.lfo_phase < previous;
        let raw = match self.choice(LFO_SHAPE) {
            0 => 1.0 - 4.0 * (self.lfo_phase - 0.5).abs(),
            1 => {
                if self.lfo_phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            2 => 1.0 - 2.0 * self.lfo_phase,
            _ => {
                if wrapped {
                    self.lfo_hold = self.rng.bipolar();
                }
                self.lfo_hold
            }
        };
        // Soften the square and sample-and-hold edges a touch, like an RC on the LFO out.
        self.lfo_value += (raw - self.lfo_value) * 0.05;
        self.lfo_value
    }

    /// Pitch (as oscillator steps), filter gain and oscillator levels for a voice at this
    /// instant.
    fn controls(&self, voice: &Voice, bend: f64, lfo: f64) -> ([f64; 4], f64, [f64; 3]) {
        let os_rate = RATE * self.oversampling() as f64;
        let drift = self.get(DRIFT) / 100.0;
        let fade = voice.lfo_fade;
        let source = self.choice(MOD_SOURCE);
        let modulation = match source {
            0 => lfo,
            1 => voice.osc3,
            _ => self.lfo_hold,
        } * fade;
        let pitch_mod = self.get(MOD_PITCH) * modulation;
        let shared = voice.pitch_drift.value() * 2.0 * drift;
        let keyboard = voice.glide.pitch + bend + self.get(TUNE) / 100.0;
        let mut steps = [0.0; 4];
        let mut amplitude = [1.0; 3];
        for (index, step) in steps.iter_mut().take(3).enumerate() {
            let range = RANGE_SEMITONES[self.choice(osc(index as u32, OSC_RANGE)) as usize % 6];
            let tune = self.get(osc(index as u32, OSC_TUNE));
            let cents = (voice.tolerance.detune[index] * 4.0
                + voice.osc_drift[index].value() * 5.0
                + voice.jitter[index].value() * 0.6)
                * drift
                + shared;
            let cents = cents / 100.0;
            let base = if index == 2 && self.choice(OSC3_KEYBOARD) == 0 {
                60.0
            } else {
                keyboard
            };
            let modulated = if index == 2 && source == 1 {
                0.0
            } else {
                pitch_mod
            };
            let semitones = base + range + tune + cents + modulated;
            *step = 440.0 * ((semitones - 69.0) / 12.0).exp2() / os_rate;
            let compensation = compensation_error(voice.tolerance.compensation[index], semitones);
            amplitude[index] =
                1.0 + (0.03 * voice.tolerance.level[index] + 0.05 * compensation) * drift;
        }
        // The sub follows oscillator 1, an octave down.
        steps[3] = steps[0];
        let tracking = [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0][self.choice(KEY_TRACK) as usize % 4];
        let octaves = self.get(CONTOUR) * 0.8 * voice.filter.level
            + tracking * (voice.glide.pitch - 60.0) / 12.0
            + self.get(MOD_FILTER) * modulation
            + self.get(FILTER_VELOCITY) * 2.0 * (voice.velocity - 0.5)
            + voice.tolerance.cutoff * 0.08 * drift
            + voice.cutoff_drift.value() * 0.05 * drift;
        let cutoff = self.get(CUTOFF) * octaves.exp2();
        (steps, filter_gain(cutoff, os_rate), amplitude)
    }

    fn render_sample(&mut self) -> Frame {
        self.advance_parameters();
        let bend = self.advance_bend();
        let lfo = self.advance_lfo();
        let factor = self.oversampling();
        let exact = self.exact();
        let control = exact || self.clock.is_multiple_of(CONTROL_FRAMES);
        self.clock += 1;
        self.scratch[..factor].fill(0.0);

        let waves = [0, 1, 2].map(|index| self.choice(osc(index, OSC_WAVE)));
        let levels = [0, 1, 2].map(|index| {
            if self.choice(osc(index, OSC_ON)) == 1 {
                self.get(osc(index, OSC_LEVEL)) / 10.0
            } else {
                0.0
            }
        });
        let sub_level = self.get(SUB) / 10.0;
        let noise_level = self.get(NOISE) / 10.0;
        let pink = self.choice(NOISE_COLOR) == 1;
        let drive = 0.35 * (self.get(DRIVE) / 10.0 * 3.0).exp2();
        let makeup = 1.0 / drive.powf(0.6);
        let emphasis = self.get(EMPHASIS) / 10.0;
        let k = 4.3 * emphasis.powf(1.2);
        let feedback = self.get(FEEDBACK) / 10.0 * 1.5;
        let velocity_amount = self.get(VELOCITY);
        let pwm = self.get(MOD_PWM) * 0.4 * lfo;
        let lfo_fade_step = {
            let delay = self.get(LFO_DELAY);
            if delay <= 0.0 {
                1.0
            } else {
                1.0 / (delay * RATE)
            }
        };
        let solve = FilterSolve {
            iterations: if exact { 8 } else { 1 },
            tolerance: 1.0e-7,
            exact,
        };
        let limit = self.voice_limit();
        let drift_amount = self.get(DRIFT);
        let drift = drift_amount / 100.0;
        let coupling = one_pole_coefficient(COUPLING_HZ, RATE * factor as f64);

        for index in 0..MAX_VOICES {
            if !self.voices[index].sounding() {
                continue;
            }
            if index >= limit && self.voices[index].gate {
                // The voice count shrank: let voices beyond it release.
                let voice = &mut self.voices[index];
                voice.gate = false;
                voice.amp.gate_off();
                voice.filter.gate_off();
            }
            let amp_rates = self.contour_rates(AMP_ATTACK, self.voices[index].tolerance.time);
            let filter_rates =
                self.contour_rates(FILTER_ATTACK, -self.voices[index].tolerance.time);
            let rng_drift = drift_amount > 0.0;
            {
                let voice = &mut self.voices[index];
                voice.glide.tick();
                voice.filter.tick(&filter_rates);
                voice.lfo_fade = (voice.lfo_fade + lfo_fade_step).min(1.0);
                if rng_drift {
                    voice.pitch_drift.tick(&mut self.rng, 9600);
                    voice.cutoff_drift.tick(&mut self.rng, 14_400);
                    for (index, period) in [7900, 9100, 10_300].into_iter().enumerate() {
                        voice.osc_drift[index].tick(&mut self.rng, period);
                        voice.jitter[index].tick(&mut self.rng, 96);
                    }
                }
            }
            let amp_before = self.voices[index].amp.level;
            let amp = self.voices[index].amp.tick(&amp_rates);
            if control || self.voices[index].dirty {
                let (steps, gain, amplitude) = self.controls(&self.voices[index], bend, lfo);
                let voice = &mut self.voices[index];
                if voice.dirty {
                    voice.previous_steps = steps;
                    voice.previous_gain = gain;
                } else {
                    voice.previous_steps = voice.steps;
                    voice.previous_gain = voice.gain;
                }
                voice.steps = steps;
                voice.gain = gain;
                voice.amplitude = amplitude;
                voice.dirty = false;
            }
            let voice = &mut self.voices[index];
            let velocity_gain = 1.0 - velocity_amount + velocity_amount * voice.velocity;
            for sub_index in 0..factor {
                // Offline mode moves pitch, cutoff and level continuously across the
                // oversampled sub-samples; realtime holds them per control period.
                let (steps, gain, level) = if exact {
                    let t = (sub_index + 1) as f64 / factor as f64;
                    let lerp = |a: f64, b: f64| a + (b - a) * t;
                    (
                        [0, 1, 2, 3].map(|i| lerp(voice.previous_steps[i], voice.steps[i])),
                        lerp(voice.previous_gain, voice.gain),
                        lerp(amp_before, amp),
                    )
                } else {
                    (voice.steps, voice.gain, amp)
                };
                let mut osc = [0.0; 3];
                for (i, out) in osc.iter_mut().enumerate() {
                    let width = pwm + 0.02 * voice.tolerance.width[i] * drift;
                    let raw = voice.vco[i].tick(waves[i], steps[i], width);
                    if i == 2 {
                        // Modulation taps oscillator 3 before its coupling capacitor, so
                        // slow shapes reach pitch and cutoff undistorted.
                        voice.osc3 = raw;
                    }
                    let coupled = raw - voice.coupling[i].lowpass(raw, coupling);
                    *out = coupled * voice.amplitude[i];
                }
                let [o1, o2, o3] = osc;
                let sub = voice.sub.tick(steps[3]);
                let white = self.rng.bipolar();
                let noise = if pink {
                    voice.pink.tick(white) * 2.0
                } else {
                    white
                };
                let mix = o1 * levels[0]
                    + o2 * levels[1]
                    + o3 * levels[2]
                    + sub * sub_level
                    + noise * noise_level * 0.7;
                let input =
                    mix * 0.5 * drive * (1.0 + 0.25 * k) + feedback * (2.0 * voice.feedback).tanh();
                let filtered = voice.vcf.tick(input, gain, k, &solve);
                voice.feedback = filtered;
                self.scratch[sub_index] += filtered * makeup * level * velocity_gain * 0.9;
            }
            if !voice.sounding() {
                voice.id = None;
                voice.gate = false;
            }
        }

        let mono = self.decimator.process(&self.scratch[..factor]);
        let mono = self.hpf.tick(mono, self.choice(HPF), RATE);
        let [left, right] = self.chorus.tick(mono, self.choice(CHORUS), RATE, exact);
        let gain = super::effects::amplitude(self.get(GAIN));
        [(left * gain) as f32, (right * gain) as f32]
    }

    fn publish(&self) {
        if let Some(telemetry) = &self.telemetry {
            for (cell, voice) in telemetry.ferro_voices.iter().zip(&self.voices) {
                let level = if voice.sounding() {
                    voice.amp.level as f32
                } else {
                    0.0
                };
                cell.store(level.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

impl PluginProcessor for Ferro {
    fn reset(&mut self) {
        self.voices = (0..MAX_VOICES).map(Voice::new).collect();
        self.held.clear();
        self.last_pitch = None;
        self.bend = Bend::default();
        self.lfo_phase = 0.0;
        self.lfo_hold = 0.0;
        self.lfo_value = 0.0;
        self.rng = Rng::new(SEED);
        self.chorus.reset();
        self.hpf.reset();
        self.decimator.reset();
        self.clock = 0;
        self.age = 0;
        self.current = self.target;
        self.publish();
    }

    fn event(&mut self, event: NoteEvent) {
        match event {
            NoteEvent::On {
                id,
                key,
                velocity,
                slide_frames,
                ..
            } => self.note_on(id, key.min(127), velocity.clamp(1, 127), slide_frames),
            NoteEvent::Off { id } => self.note_off(id),
            NoteEvent::PitchBend {
                value, ramp_frames, ..
            } => {
                let value = f64::from(value).clamp(-1.0, 1.0);
                self.bend.target = value;
                self.bend.remaining = ramp_frames;
                if ramp_frames == 0 && !self.voices.iter().any(Voice::sounding) {
                    // Nothing is sounding, so there is no glitch to smooth.
                    self.bend.value = value;
                    self.bend.smoothed = value;
                }
            }
        }
    }

    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        self.set_quality(context.quality);
        for frame in frames.iter_mut() {
            *frame = self.render_sample();
        }
        self.publish();
    }

    fn set_parameter(&mut self, id: u32, value: f64) {
        if !value.is_finite() || id as usize >= PARAMETER_SLOTS {
            return;
        }
        let Some(spec) = spec(id) else {
            return;
        };
        let physical = spec.denormalize(value.clamp(0.0, 1.0));
        self.target[id as usize] = physical;
        if spec.steps > 0 {
            self.current[id as usize] = physical;
            for voice in &mut self.voices {
                voice.dirty = true;
            }
        }
    }
}
