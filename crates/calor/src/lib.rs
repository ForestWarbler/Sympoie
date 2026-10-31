//! Calor: a multiband harmonic saturator with separate control of the odd and even
//! harmonics.
//!
//! **The curve.** Each band runs the signal through one smooth curve (see the `curve`
//! module): linear up to a knee that Hardness sets, then bending into a ceiling, driven by
//! Drive and moved off centre by Bias the way a tube's grid bias leans its curve. A
//! symmetric curve can only add odd harmonics; the lean is what adds the even ones. Calor
//! splits the curve exactly into its odd and even parts and sets them apart:
//!
//! - Odd blends the odd part with the clean signal, level with it at −12 dBFS, so it
//!   turns the odd harmonics (and the squash that comes with them) up and down without
//!   moving the fundamental;
//! - Even scales the even part, from none (a perfectly symmetric curve) through the
//!   curve's own (100 %) to three times that.
//!
//! At 100 % each, the result is the curve itself. The even part's offset is blocked at
//! 5 Hz, so leaning the curve never leaves DC behind. Around the curve, Emphasis lifts or
//! lowers the treble above its frequency on the way in and does the opposite on the way
//! out, so the band it lifts saturates first while the tonal balance stays where it was.
//!
//! **Models.** Besides the plain curve a band can run Tube (the lean grows with the band's
//! level, so the even harmonics bloom on loud notes), Tape (a hysteresis loop after the
//! curve, and a head bump), Transformer (the curve works on the core's flux, so the lows
//! saturate first) or Fold (a wavefolder in place of the curve); see the `models` module.
//! The style library (see [`STYLES`]) holds starting points that set a band's model and
//! controls together.
//!
//! **Bands.** Up to six bands, split by Linkwitz-Riley crossovers of 12, 24 or 48 dB per
//! octave that add back up to a flat all-pass (see the `crossover` module). Each band has
//! its own curve, model and emphasis; Dynamics, which compresses the band when turned
//! right and expands it towards a gate when turned left; Feedback, which feeds the band's
//! output back through a ringing band-pass at its own frequency and fades out when the
//! input stops; Channels, to work on the stereo pair, only the middle, or only the
//! sides; its own Mix and Level; and Solo and Mute.
//!
//! **Modulation.** An LFO and an envelope follower on the input reach any of a band's
//! main controls, or Mix, Tone and Output, through four slots (see the `modulation`
//! module).
//!
//! **After the bands**, Tone tilts their sum, Auto gain divides each band by its curve's
//! gain at −12 dBFS so a setting can be judged at equal loudness, and Mix blends in the
//! dry signal, split and delayed alongside the bands so the two line up.
//!
//! **Quality.** The curves run oversampled (see the `oversampling` module): four times at
//! 44.1 and 48 kHz in real time, sixteen times when rendering offline, fewer at higher
//! rates. Antiderivative anti-aliasing on top takes the harmonics that would still fold
//! back down sharply. Half-band filters and the shaper delay the signal by a whole number
//! of samples, padded to the same [`Calor::latency`] at either quality and for any number
//! of bands (39 samples, about 0.8 ms, at 48 kHz), which a host compensates.
//!
//! **Compatibility.** Parameters are only ever added (see the `params` module), and every
//! one added since the single-band Calor starts where it leaves that sound as it was:
//! with one band and the new controls at their defaults, a band runs exactly the
//! operations the single band did, so sets saved with it sound the same.
//!
//! The crate has no dependencies. Sympoie runs it as a built-in effect, and the Calor
//! plugin runs the same processor inside other hosts.

mod band;
mod crossover;
mod curve;
mod dynamics;
mod filters;
mod models;
mod modulation;
mod oversampling;
pub mod params;
mod processor;
mod styles;
pub mod telemetry;
#[cfg(test)]
mod tests;

pub use crossover::band_response;
pub use curve::{Curve, HARMONICS, REFERENCE_LEVEL, Transfer};
pub use params::{
    AUTO_GAIN, BANDS, BIAS, CHANNEL_MODES, COUNT, CROSSOVER_BASE, CROSSOVER_DEFAULTS,
    CROSSOVER_SLOPE, CROSSOVERS, DRIVE, EMPHASIS, EMPHASIS_FREQUENCY, ENVELOPE_ATTACK,
    ENVELOPE_RELEASE, EVEN, HARDNESS, LFO_RATE, LFO_SHAPE, LFO_SHAPES, MAX_BANDS, MIX, MOD_AMOUNT,
    MOD_BAND, MOD_BANDS, MOD_SLOTS, MOD_SOURCE, MOD_SOURCES, MOD_TARGET, MOD_TARGETS, MODELS, ODD,
    OUTPUT, PARAMETERS, SLOPES, Scale, Slot, Spec, TONE, band_parameter, band_slot, choices,
    mod_parameter,
};
pub use processor::{Calor, Quality};
pub use styles::{STYLES, Style, matching as matching_style};
pub use telemetry::Telemetry;
