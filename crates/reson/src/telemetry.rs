//! What Reson shows an interface while it runs, shared lock-free with the audio thread.

use std::sync::atomic::{AtomicU32, Ordering};

/// Bands of [`Telemetry::spectrum`] and [`Telemetry::input_spectrum`], log-spaced from 20 Hz
/// to 20 kHz.
pub const SPECTRUM_BANDS: usize = 128;
/// Bands of the cut curves, log-spaced from 20 Hz to 20 kHz.
pub const REDUCTION_BANDS: usize = 256;

/// Spectra, cuts, notes and meters, published by the processor every few hops while an
/// interface watches. Every value is an `f32` stored as its bits.
#[derive(Debug)]
pub struct Telemetry {
    /// Level of the output and of the input, in dB where a full-scale sine reads 0 dB.
    pub spectrum: [AtomicU32; SPECTRUM_BANDS],
    pub input_spectrum: [AtomicU32; SPECTRUM_BANDS],
    /// Cut the resonances are getting, in dB, zero or more.
    pub reduction_spectrum: [AtomicU32; REDUCTION_BANDS],
    /// Cut Clean is making on the noise floor, in dB, averaged over the band.
    pub cleaning_spectrum: [AtomicU32; REDUCTION_BANDS],
    /// How much each band belongs to the harmonics of a note Reson hears, 0 to 1.
    pub spared_spectrum: [AtomicU32; REDUCTION_BANDS],
    /// Fundamentals of the notes Reson hears, in Hz, zero in unused slots.
    pub notes: [AtomicU32; 4],
    /// Counts refreshes. The spectra survive a reset (Sympoie resets its devices at every
    /// loop wrap), so a display that sees this stop advancing should let them fade.
    pub spectrum_updates: AtomicU32,
    /// Peak input and output in dBFS, and the deepest cut in dB, each held and decaying
    /// over about 300 ms.
    pub input_db: AtomicU32,
    pub output_db: AtomicU32,
    pub reduction_db: AtomicU32,
}

impl Default for Telemetry {
    fn default() -> Self {
        let quiet = || AtomicU32::new((-120.0_f32).to_bits());
        let zero = || AtomicU32::new(0);
        Self {
            spectrum: std::array::from_fn(|_| quiet()),
            input_spectrum: std::array::from_fn(|_| quiet()),
            reduction_spectrum: std::array::from_fn(|_| zero()),
            cleaning_spectrum: std::array::from_fn(|_| zero()),
            spared_spectrum: std::array::from_fn(|_| zero()),
            notes: std::array::from_fn(|_| zero()),
            spectrum_updates: AtomicU32::new(0),
            input_db: quiet(),
            output_db: quiet(),
            reduction_db: zero(),
        }
    }
}

/// Reads a run of `f32` values stored as bits.
pub fn load<const N: usize>(cells: &[AtomicU32; N]) -> [f32; N] {
    std::array::from_fn(|index| f32::from_bits(cells[index].load(Ordering::Relaxed)))
}

/// Reads one `f32` stored as bits.
pub fn get(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

pub(crate) fn set(cell: &AtomicU32, value: f32) {
    cell.store(value.to_bits(), Ordering::Relaxed);
}
