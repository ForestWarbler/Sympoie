//! What Calor shows an interface while it runs, shared lock-free with the audio thread.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::params::MAX_BANDS;

/// Meters, published by the processor at the end of every block while an interface
/// watches. Levels are `f32` dBFS stored as their bits.
#[derive(Debug)]
pub struct Telemetry {
    /// Peak input and output, each held and falling back over about 300 ms.
    pub input_db: AtomicU32,
    pub output_db: AtomicU32,
    /// Counts the blocks processed. The meters survive a reset (Sympoie resets its devices
    /// at every loop wrap), so a display that sees this stop advancing should let them
    /// fall.
    pub updates: AtomicU32,
    /// Peak output of each band, held and falling back like the others.
    pub band_db: [AtomicU32; MAX_BANDS],
    /// Where the modulation sources stand: the LFO from −1 to 1, the envelope from 0 to 1.
    pub lfo: AtomicU32,
    pub envelope: AtomicU32,
    /// Where the LFO is in its cycle, from 0 to 1.
    pub lfo_phase: AtomicU32,
}

impl Default for Telemetry {
    fn default() -> Self {
        let quiet = || AtomicU32::new((-120.0_f32).to_bits());
        Self {
            input_db: quiet(),
            output_db: quiet(),
            updates: AtomicU32::new(0),
            band_db: std::array::from_fn(|_| quiet()),
            lfo: AtomicU32::new(0),
            envelope: AtomicU32::new(0),
            lfo_phase: AtomicU32::new(0),
        }
    }
}

/// Reads one `f32` stored as bits.
pub fn get(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

pub(crate) fn set(cell: &AtomicU32, value: f32) {
    cell.store(value.to_bits(), Ordering::Relaxed);
}
