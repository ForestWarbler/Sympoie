//! Reson: a dynamic resonance suppressor.
//!
//! Reson listens for frequencies that ring out above the rest of the spectrum — harsh
//! upper mids, whistling sibilance, a boxy room mode, a snare's ring — and turns each one
//! down only while it rings, leaving the rest of the sound alone.
//!
//! Every hop it takes an FFT of the most recent audio and
//! 1. measures how far each frequency bin stands out from its spectral neighbourhood and
//!    sets a target cut from Depth, Threshold, Knee, Width and the sensitivity curve (see
//!    the `detector` module);
//! 2. eases each bin's cut towards its target with the Attack and Release times;
//! 3. with Clean up, adds a cut for whatever sits on the noise floor (see the `clean`
//!    module);
//! 4. turns the cuts into a minimum-phase filter and applies it to the same frame.
//!
//! The frame and hop grow with the sample rate (see [`Layout`]), so the analysis resolves
//! the same hertz and milliseconds at 44.1 kHz as at 192 kHz.
//!
//! The minimum-phase filter is what keeps the delay short: its impulse response is causal
//! and front-loaded, so the newest samples of a frame can be filtered without waiting for
//! the ones after them. Each frame hands its last two hops to the output through a Hann
//! crossfade, so the result trails the input by exactly [`Reson::latency`] samples (about
//! 5.5 ms), which a host compensates. Frames with nothing to cut skip the filter and pass
//! through untouched.
//!
//! Mix blends the cuts towards unity before the filter is built, so the dry and processed
//! signals cannot comb against each other; Delta plays only what Reson removes. Stereo
//! signals are analysed linked, per channel, or as mid and side, and every pair of
//! channels shares one complex FFT.
//!
//! Focus is what sets Reson apart from a plain resonance suppressor: it leaves the notes
//! alone. Reson finds the fundamentals of up to four notes in every frame (see the `pitch`
//! module) and, as Focus rises from 0 % to 100 %, takes its cuts off their harmonics. A
//! harmonic still gives way where it stands above nearly all of its neighbouring
//! harmonics, because a resonance in pitched material shows exactly as a harmonic that is
//! too loud; everything that is not a harmonic of a note (noise, rings, hum, feedback) is
//! cut as before. The notes are found afresh in every frame, so however long one is held
//! it stays a note.
//!
//! Clean uses the same notes the other way round: it turns down the steady noise around
//! them (hiss, room tone, the breathy haze between a voice's harmonics) while their
//! harmonics, and anything that rises well above the noise, pass untouched. A plain
//! denoiser cannot tell a long-held note from noise and dulls the quiet top harmonics
//! along with the hiss around them; Clean knows where the notes are.
//!
//! The crate has no dependencies. Sympoie runs it as a built-in effect, and the Reson
//! plugin runs the same processor inside other hosts.

mod clean;
mod detector;
mod display;
mod layout;
pub mod params;
mod pitch;
mod processor;
mod spectral;
pub mod telemetry;
#[cfg(test)]
mod tests;

pub use layout::Layout;
pub use params::{
    ATTACK, BAND_BASE, BAND_FREQUENCY, BAND_GAIN, BAND_Q, BAND_SHAPES, BAND_STRIDE, BANDS,
    BandShape, CLEAN, COUNT, DELTA, DEPTH, FOCUS, HIGH_CUT, KNEE, LOW_CUT, MIX, OUTPUT, PARAMETERS,
    RELEASE, STEREO, STEREO_MODES, Scale, Sensitivity, Spec, THRESHOLD, WIDTH, choices,
};
pub use processor::Reson;
pub use telemetry::{REDUCTION_BANDS, SPECTRUM_BANDS, Telemetry};
