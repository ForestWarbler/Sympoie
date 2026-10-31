//! Frame and hop sizes for a sample rate.

/// How Reson cuts the audio into frames at one sample rate. The frame and hop double with
/// the rate, so a bin stays 21–23 Hz wide and a hop 2.7–2.9 ms long at every rate, and
/// everything Reson measures in hertz or milliseconds means the same at 44.1 kHz as at
/// 192 kHz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// Sample rate in Hz.
    pub rate: f64,
    /// Samples per analysis frame, a power of two.
    pub frame: usize,
    /// Samples between frames.
    pub hop: usize,
}

/// Frame and hop at rates up to [`BASE_RATE_LIMIT`].
const BASE_FRAME: usize = 2048;
const BASE_HOP: usize = 128;
/// Rates up to this keep the base frame; each doubling above it doubles the frame.
const BASE_RATE_LIMIT: f64 = 50_000.0;
/// How far the frame may grow, reached above 200 kHz.
const MAX_SCALE: usize = 8;

impl Layout {
    /// The layout for `rate` Hz. A rate that is no positive number reads as 48 kHz.
    pub fn new(rate: f64) -> Self {
        let rate = if rate.is_finite() && rate > 0.0 {
            rate
        } else {
            48_000.0
        };
        let mut scale = 1;
        while scale < MAX_SCALE && rate > BASE_RATE_LIMIT * scale as f64 {
            scale *= 2;
        }
        Self {
            rate,
            frame: BASE_FRAME * scale,
            hop: BASE_HOP * scale,
        }
    }

    /// Bins from DC to Nyquist, both included.
    pub fn bins(&self) -> usize {
        self.frame / 2 + 1
    }

    /// Index of the Nyquist bin.
    pub fn half(&self) -> usize {
        self.frame / 2
    }

    /// Width of one bin in Hz.
    pub fn bin_hz(&self) -> f64 {
        self.rate / self.frame as f64
    }

    /// Frequency of bin `k` in Hz.
    pub fn hz(&self, k: usize) -> f64 {
        k as f64 * self.bin_hz()
    }

    /// Delay of the processed signal behind its input: the two hops each frame hands over.
    pub fn latency(&self) -> u32 {
        2 * self.hop as u32
    }

    /// Hops in one frame.
    pub fn hops_per_frame(&self) -> usize {
        self.frame / self.hop
    }

    /// Seconds between frames.
    pub fn hop_seconds(&self) -> f64 {
        self.hop as f64 / self.rate
    }

    /// Share of the way a one-pole smoother with a time constant of `milliseconds` moves
    /// towards its target in one hop.
    pub fn hop_rate(&self, milliseconds: f64) -> f32 {
        (1.0 - (-self.hop_seconds() / (milliseconds.max(0.01) / 1000.0)).exp()) as f32
    }

    /// The same per sample.
    pub fn sample_rate(&self, milliseconds: f64) -> f32 {
        (1.0 - (-1.0 / (milliseconds.max(0.01) / 1000.0 * self.rate)).exp()) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_grow_with_the_rate_so_bins_and_hops_keep_their_size() {
        for (rate, frame) in [
            (44_100.0, 2048),
            (48_000.0, 2048),
            (88_200.0, 4096),
            (96_000.0, 4096),
            (176_400.0, 8192),
            (192_000.0, 8192),
            (384_000.0, 16384),
        ] {
            let layout = Layout::new(rate);
            assert_eq!(layout.frame, frame, "{rate}");
            assert_eq!(layout.hops_per_frame(), 16);
            assert!((21.0..=23.5).contains(&layout.bin_hz()), "{rate}");
            assert!((2.6e-3..=2.95e-3).contains(&layout.hop_seconds()), "{rate}");
        }
        assert_eq!(Layout::new(48_000.0).latency(), 256);
        assert_eq!(Layout::new(f64::NAN), Layout::new(48_000.0));
    }
}
