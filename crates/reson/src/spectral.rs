//! Spectral building blocks for Reson: a split-array complex FFT, the fast scalar maths
//! its per-bin loops lean on, and the minimum-phase reconstruction that turns a gain
//! curve into a causal filter.

use std::f64::consts::PI;

/// Radix-2 decimation-in-time FFT over separate real and imaginary arrays, so every
/// butterfly stage after the first two runs as contiguous slices the compiler can vectorise.
#[derive(Debug)]
pub(super) struct Fft {
    size: usize,
    /// Index pairs `(i, j)`, `i < j`, exchanged by the bit-reversal permutation.
    swaps: Box<[(u32, u32)]>,
    /// Twiddles of the stages after the first two, stage after stage: the stage that joins
    /// halves of `h` points stores `e^(−iπj/h)` for `j` in `0..h`.
    cos: Box<[f32]>,
    sin: Box<[f32]>,
}

impl Fft {
    pub(super) fn new(size: usize) -> Self {
        assert!(size.is_power_of_two() && size >= 4);
        let bits = size.trailing_zeros();
        let swaps = (0..size)
            .filter_map(|i| {
                let j = i.reverse_bits() >> (usize::BITS - bits);
                (i < j).then_some((i as u32, j as u32))
            })
            .collect();
        let mut cos = Vec::with_capacity(size);
        let mut sin = Vec::with_capacity(size);
        let mut half = 4;
        while half < size {
            for j in 0..half {
                let angle = -PI * j as f64 / half as f64;
                cos.push(angle.cos() as f32);
                sin.push(angle.sin() as f32);
            }
            half *= 2;
        }
        Self {
            size,
            swaps,
            cos: cos.into_boxed_slice(),
            sin: sin.into_boxed_slice(),
        }
    }

    /// In-place forward transform, `X[k] = Σ x[n]·e^(−2πikn/N)`.
    pub(super) fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let size = self.size;
        let (re, im) = (&mut re[..size], &mut im[..size]);
        for &(i, j) in self.swaps.iter() {
            re.swap(i as usize, j as usize);
            im.swap(i as usize, j as usize);
        }
        // The first two stages have trivial twiddles (1 and −i), so they run together as
        // one radix-4 pass without multiplications.
        for (re, im) in re
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(im.as_chunks_mut::<4>().0.iter_mut())
        {
            let (s0r, s0i) = (re[0] + re[1], im[0] + im[1]);
            let (d0r, d0i) = (re[0] - re[1], im[0] - im[1]);
            let (s1r, s1i) = (re[2] + re[3], im[2] + im[3]);
            let (d1r, d1i) = (re[2] - re[3], im[2] - im[3]);
            re[0] = s0r + s1r;
            im[0] = s0i + s1i;
            re[2] = s0r - s1r;
            im[2] = s0i - s1i;
            // d1 · (−i) = (d1.im, −d1.re)
            re[1] = d0r + d1i;
            im[1] = d0i - d1r;
            re[3] = d0r - d1i;
            im[3] = d0i + d1r;
        }
        let mut half = 4;
        let mut offset = 0;
        while half < size {
            let cos = &self.cos[offset..offset + half];
            let sin = &self.sin[offset..offset + half];
            for (re, im) in re
                .chunks_exact_mut(2 * half)
                .zip(im.chunks_exact_mut(2 * half))
            {
                let (low_re, high_re) = re.split_at_mut(half);
                let (low_im, high_im) = im.split_at_mut(half);
                for j in 0..half {
                    let (wr, wi) = (cos[j], sin[j]);
                    let vr = high_re[j] * wr - high_im[j] * wi;
                    let vi = high_re[j] * wi + high_im[j] * wr;
                    let (ur, ui) = (low_re[j], low_im[j]);
                    low_re[j] = ur + vr;
                    low_im[j] = ui + vi;
                    high_re[j] = ur - vr;
                    high_im[j] = ui - vi;
                }
            }
            offset += half;
            half *= 2;
        }
    }

    /// In-place inverse transform without the `1/N` scale: exchanging the real and
    /// imaginary parts on the way in and out turns the forward transform into its inverse.
    pub(super) fn inverse(&self, re: &mut [f32], im: &mut [f32]) {
        self.forward(im, re);
    }
}

/// Splits the spectrum `Z` of `a + i·b`, two real signals packed into one transform, into
/// their own spectra at bin `k` (`0 ≤ k ≤ N/2`).
#[inline]
pub(super) fn unpack(re: &[f32], im: &[f32], k: usize) -> ([f32; 2], [f32; 2]) {
    let size = re.len();
    let mirror = (size - k) & (size - 1);
    let (a, b) = (re[k], im[k]);
    let (c, d) = (re[mirror], im[mirror]);
    (
        [0.5 * (a + c), 0.5 * (b - d)],
        [0.5 * (b + d), 0.5 * (c - a)],
    )
}

/// Writes the spectrum of `a + i·b` at bins `k` and `N − k` from the spectra of the real
/// signals `a` and `b` at bin `k` (`0 ≤ k ≤ N/2`), the inverse of [`unpack`].
#[inline]
pub(super) fn pack(re: &mut [f32], im: &mut [f32], k: usize, a: [f32; 2], b: [f32; 2]) {
    let size = re.len();
    let mirror = (size - k) & (size - 1);
    re[k] = a[0] - b[1];
    im[k] = a[1] + b[0];
    if mirror != k {
        re[mirror] = a[0] + b[1];
        im[mirror] = b[0] - a[1];
    }
}

/// `log2(x)` to about 1e-6 for positive normal `x`; smaller inputs read as the smallest
/// normal number.
#[inline]
pub(super) fn log2(x: f32) -> f32 {
    let bits = x.max(f32::MIN_POSITIVE).to_bits();
    let mut exponent = (bits >> 23) as i32 - 127;
    let mut mantissa = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    if mantissa > std::f32::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }
    // log2(m) = (2/ln 2)·atanh(t) with t = (m − 1)/(m + 1), |t| < 0.172.
    let t = (mantissa - 1.0) / (mantissa + 1.0);
    let t2 = t * t;
    exponent as f32 + t * (2.885_39 + t2 * (0.961_796_7 + t2 * (0.577_078 + t2 * 0.412_198_6)))
}

/// `2^x` to about 3e-6 relative error, saturating outside ±126.
#[inline]
pub(super) fn exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 126.0);
    let whole = x.round();
    let f = x - whole;
    let fraction = 1.0
        + f * (std::f32::consts::LN_2
            + f * (0.240_226_5 + f * (0.055_504_1 + f * (0.009_618_1 + f * 0.001_333_6))));
    fraction * f32::from_bits(((whole as i32 + 127) as u32) << 23)
}

/// `(sin, cos)` of any finite angle to about 1e-6.
#[inline]
pub(super) fn sin_cos(angle: f32) -> (f32, f32) {
    let quarters = angle * std::f32::consts::FRAC_2_PI;
    let quadrant = quarters.round();
    let r = (quarters - quadrant) * std::f32::consts::FRAC_PI_2;
    let r2 = r * r;
    let sin = r * (1.0 + r2 * (-1.0 / 6.0 + r2 * (1.0 / 120.0 + r2 * (-1.0 / 5040.0))));
    let cos = 1.0 + r2 * (-0.5 + r2 * (1.0 / 24.0 + r2 * (-1.0 / 720.0 + r2 * (1.0 / 40320.0))));
    match (quadrant as i64) & 3 {
        0 => (sin, cos),
        1 => (cos, -sin),
        2 => (-sin, -cos),
        _ => (-cos, sin),
    }
}

/// Minimum-phase reconstruction through the real cepstrum. A gain curve given as
/// magnitudes from DC to Nyquist gets the phase that makes its impulse response causal
/// and front-loaded, so the curve can filter the newest samples of a frame without waiting
/// for the samples that follow them. Two curves share each pair of transforms.
#[derive(Debug)]
pub(super) struct MinimumPhase {
    re: Box<[f32]>,
    im: Box<[f32]>,
}

impl MinimumPhase {
    pub(super) fn new(size: usize) -> Self {
        Self {
            re: vec![0.0; size].into_boxed_slice(),
            im: vec![0.0; size].into_boxed_slice(),
        }
    }

    /// Replaces each natural-log magnitude curve (`N/2 + 1` bins, DC to Nyquist) with its
    /// minimum phase, in radians. `second` may be absent when only one curve needs it.
    pub(super) fn phases(&mut self, fft: &Fft, first: &mut [f32], mut second: Option<&mut [f32]>) {
        let size = self.re.len();
        let half = size / 2;
        let (re, im) = (&mut self.re[..], &mut self.im[..]);
        // Both log-magnitude curves are real and even, so their cepstra are real and come
        // out of one inverse transform as its real and imaginary parts.
        re[..=half].copy_from_slice(&first[..=half]);
        match second.as_deref() {
            Some(second) => im[..=half].copy_from_slice(&second[..=half]),
            None => im[..=half].fill(0.0),
        }
        for k in 1..half {
            re[size - k] = re[k];
            im[size - k] = im[k];
        }
        fft.inverse(re, im);
        // Fold the anticausal half of each cepstrum onto the causal half.
        let scale = 1.0 / size as f32;
        re[0] *= scale;
        im[0] *= scale;
        for n in 1..half {
            re[n] *= 2.0 * scale;
            im[n] *= 2.0 * scale;
        }
        re[half] *= scale;
        im[half] *= scale;
        re[half + 1..].fill(0.0);
        im[half + 1..].fill(0.0);
        fft.forward(re, im);
        // The phase is the imaginary part of each folded cepstrum's spectrum.
        for k in 0..=half {
            let (a, b) = unpack(re, im, k);
            first[k] = a[1];
            if let Some(second) = second.as_deref_mut() {
                second[k] = b[1];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(re: &[f32], im: &[f32]) -> (Vec<f64>, Vec<f64>) {
        let size = re.len();
        (0..size)
            .map(|k| {
                (0..size).fold((0.0, 0.0), |(sr, si), n| {
                    let angle = -2.0 * PI * (k * n % size) as f64 / size as f64;
                    let (s, c) = angle.sin_cos();
                    let (x, y) = (f64::from(re[n]), f64::from(im[n]));
                    (sr + x * c - y * s, si + x * s + y * c)
                })
            })
            .unzip()
    }

    fn signal(size: usize, seed: u32) -> (Vec<f32>, Vec<f32>) {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        let re = (0..size).map(|_| next()).collect();
        let im = (0..size).map(|_| next()).collect();
        (re, im)
    }

    #[test]
    fn forward_transform_matches_the_definition() {
        for size in [4, 8, 16, 64, 512] {
            let (mut re, mut im) = signal(size, size as u32);
            let (expected_re, expected_im) = naive(&re, &im);
            let fft = Fft::new(size);
            fft.forward(&mut re, &mut im);
            for k in 0..size {
                assert!(
                    (f64::from(re[k]) - expected_re[k]).abs() < 2e-4 * size as f64,
                    "{size} re {k}"
                );
                assert!(
                    (f64::from(im[k]) - expected_im[k]).abs() < 2e-4 * size as f64,
                    "{size} im {k}"
                );
            }
        }
    }

    #[test]
    fn inverse_undoes_forward_up_to_the_size() {
        let size = 2048;
        let fft = Fft::new(size);
        let (original_re, original_im) = signal(size, 7);
        let (mut re, mut im) = (original_re.clone(), original_im.clone());
        fft.forward(&mut re, &mut im);
        fft.inverse(&mut re, &mut im);
        for n in 0..size {
            assert!((re[n] / size as f32 - original_re[n]).abs() < 1e-5);
            assert!((im[n] / size as f32 - original_im[n]).abs() < 1e-5);
        }
    }

    #[test]
    fn packed_real_signals_unpack_and_repack() {
        let size = 64;
        let fft = Fft::new(size);
        let (a, b) = signal(size, 3);
        let (mut re, mut im) = (a.clone(), b.clone());
        fft.forward(&mut re, &mut im);
        let (mut a_re, mut a_im) = (a.clone(), vec![0.0; size]);
        fft.forward(&mut a_re, &mut a_im);
        let (mut b_re, mut b_im) = (b.clone(), vec![0.0; size]);
        fft.forward(&mut b_re, &mut b_im);
        let (mut packed_re, mut packed_im) = (vec![0.0; size], vec![0.0; size]);
        for k in 0..=size / 2 {
            let (x, y) = unpack(&re, &im, k);
            assert!((x[0] - a_re[k]).abs() < 1e-4 && (x[1] - a_im[k]).abs() < 1e-4);
            assert!((y[0] - b_re[k]).abs() < 1e-4 && (y[1] - b_im[k]).abs() < 1e-4);
            pack(&mut packed_re, &mut packed_im, k, x, y);
        }
        for k in 0..size {
            assert!((packed_re[k] - re[k]).abs() < 1e-4);
            assert!((packed_im[k] - im[k]).abs() < 1e-4);
        }
    }

    #[test]
    fn fast_maths_track_the_standard_library() {
        let mut x = 1.0e-30_f32;
        while x < 1.0e30 {
            assert!(
                (log2(x) - x.log2()).abs() < 2e-6 * x.log2().abs().max(1.0),
                "{x}"
            );
            x *= 1.37;
        }
        let mut y = -40.0_f32;
        while y < 40.0 {
            let exact = y.exp2();
            assert!(((exp2(y) - exact) / exact).abs() < 4e-6, "{y}");
            y += 0.0137;
        }
        let mut angle = -12.0_f32;
        while angle < 12.0 {
            let (sin, cos) = sin_cos(angle);
            assert!((sin - angle.sin()).abs() < 3e-6, "{angle}");
            assert!((cos - angle.cos()).abs() < 3e-6, "{angle}");
            angle += 0.0071;
        }
    }

    #[test]
    fn minimum_phase_keeps_the_magnitude_and_makes_the_response_causal() {
        let size = 512;
        let half = size / 2;
        let fft = Fft::new(size);
        // A 12 dB notch and a gentle shelf, as natural-log magnitudes.
        let curve = |k: usize, centre: f64| {
            let notch = -12.0 * (-((k as f64 - centre) / 6.0).powi(2)).exp();
            let shelf = -3.0 / (1.0 + (-(k as f64 - 200.0) / 10.0).exp());
            ((notch + shelf) / 20.0 * std::f64::consts::LN_10) as f32
        };
        let mut first: Vec<f32> = (0..=half).map(|k| curve(k, 40.0)).collect();
        let mut second: Vec<f32> = (0..=half).map(|k| curve(k, 120.0)).collect();
        let magnitudes = [first.clone(), second.clone()];
        let mut minimum = MinimumPhase::new(size);
        minimum.phases(&fft, &mut first, Some(&mut second));
        for (log_magnitude, phase) in magnitudes.iter().zip([&first, &second]) {
            let (mut re, mut im) = (vec![0.0; size], vec![0.0; size]);
            for k in 0..=half {
                let magnitude = log_magnitude[k].exp();
                let (sin, cos) = phase[k].sin_cos();
                re[k] = magnitude * cos;
                im[k] = magnitude * sin;
                if k > 0 && k < half {
                    re[size - k] = re[k];
                    im[size - k] = -im[k];
                }
            }
            fft.inverse(&mut re, &mut im);
            let energy = |range: std::ops::Range<usize>| -> f32 {
                re[range].iter().map(|x| x * x).sum::<f32>()
            };
            // Nearly all of the energy sits at the start; none of it wraps to the end.
            let total = energy(0..size);
            assert!(energy(0..half / 2) > 0.999 * total, "front-loaded");
            assert!(energy(size - half / 4..size) < 1e-6 * total, "causal");
            assert!(im.iter().all(|x| x.abs() < 1e-3 * size as f32), "real");
        }
    }
}
