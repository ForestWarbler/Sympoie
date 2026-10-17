use std::borrow::Cow;
use std::f64::consts::{PI, TAU};

use crate::Frame;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PitchAlgorithm {
    #[default]
    Builtin,
    Signalsmith,
    RubberBandR3,
}

impl PitchAlgorithm {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Builtin => "Built-in",
            Self::Signalsmith => "Signalsmith Stretch",
            Self::RubberBandR3 => "Rubber Band R3",
        }
    }

    pub const fn supports_formants(self) -> bool {
        !matches!(self, Self::Builtin)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PitchError(pub String);

impl std::fmt::Display for PitchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PitchError {}

pub fn process_cancellable(
    samples: &[Frame],
    semitones: f32,
    algorithm: PitchAlgorithm,
    preserve_formants: bool,
    cancelled: impl Fn() -> bool,
) -> Result<Option<Cow<'_, [Frame]>>, PitchError> {
    if cancelled() {
        return Ok(None);
    }
    if !valid_semitones(semitones) {
        return Err(PitchError(
            "pitch must be finite and in -24..=24 semitones".into(),
        ));
    }
    if semitones == 0.0 || samples.is_empty() {
        return Ok(Some(Cow::Borrowed(samples)));
    }
    if algorithm == PitchAlgorithm::Builtin {
        return Ok(shift_cancellable(samples, semitones, cancelled));
    }
    #[cfg(feature = "native-pitch")]
    {
        let processed = match algorithm {
            PitchAlgorithm::Signalsmith => sympoie_signalsmith_pitch::shift(
                samples,
                crate::SAMPLE_RATE,
                semitones,
                preserve_formants,
                &cancelled,
            ),
            PitchAlgorithm::RubberBandR3 => sympoie_rubberband_pitch::shift(
                samples,
                crate::SAMPLE_RATE,
                semitones,
                preserve_formants,
                &cancelled,
            ),
            PitchAlgorithm::Builtin => unreachable!(),
        }
        .map_err(|error| PitchError(format!("{}: {error}", algorithm.label())))?;
        let Some(output) = processed else {
            return Ok(None);
        };
        if cancelled() {
            return Ok(None);
        }
        if output.len() != samples.len() {
            return Err(PitchError(format!(
                "{} returned an incorrect audio length",
                algorithm.label()
            )));
        }
        for chunk in output.chunks(4096) {
            if cancelled() {
                return Ok(None);
            }
            if chunk.iter().flatten().any(|value| !value.is_finite()) {
                return Err(PitchError(format!(
                    "{} produced non-finite audio",
                    algorithm.label()
                )));
            }
        }
        Ok(Some(Cow::Owned(output)))
    }
    #[cfg(not(feature = "native-pitch"))]
    {
        let _ = preserve_formants;
        Err(PitchError(format!(
            "{} requires the native-pitch build feature",
            algorithm.label()
        )))
    }
}

const FFT_SIZE: usize = 4096;
const HOP: usize = FFT_SIZE / 8;
const BINS: usize = FFT_SIZE / 2 + 1;

pub fn valid_semitones(semitones: f32) -> bool {
    semitones.is_finite() && (-24.0..=24.0).contains(&semitones)
}

pub fn shift(samples: &[Frame], semitones: f32) -> Cow<'_, [Frame]> {
    shift_cancellable(samples, semitones, || false).expect("uncancelled pitch preparation")
}

pub fn shift_cancellable(
    samples: &[Frame],
    semitones: f32,
    cancelled: impl Fn() -> bool,
) -> Option<Cow<'_, [Frame]>> {
    if cancelled() {
        return None;
    }
    if semitones == 0.0 || !valid_semitones(semitones) || samples.is_empty() {
        return Some(Cow::Borrowed(samples));
    }
    let mut silent = true;
    for chunk in samples.chunks(FFT_SIZE) {
        if cancelled() {
            return None;
        }
        if chunk.iter().any(|frame| *frame != [0.0; 2]) {
            silent = false;
            break;
        }
    }
    if cancelled() {
        return None;
    }
    let mut output = Vec::with_capacity(samples.len());
    while output.len() < samples.len() {
        if cancelled() {
            return None;
        }
        output.resize((output.len() + FFT_SIZE).min(samples.len()), [0.0; 2]);
    }
    if silent {
        return (!cancelled()).then_some(Cow::Owned(output));
    }

    let ratio = 2.0_f64.powf(f64::from(semitones) / 12.0);
    let window: Vec<_> = (0..FFT_SIZE)
        .map(|index| 0.5 - 0.5 * (TAU * index as f64 / FFT_SIZE as f64).cos())
        .collect();
    let fft = Fft::new(FFT_SIZE);
    let mut analysis = [
        vec![Complex::default(); FFT_SIZE],
        vec![Complex::default(); FFT_SIZE],
    ];
    let mut previous = [
        vec![Complex::default(); BINS],
        vec![Complex::default(); BINS],
    ];
    let mut synthesis = analysis.clone();
    let mut energy = vec![0.0; BINS];
    let mut rotations = vec![0.0; BINS];
    let mut frequencies = vec![0.0; BINS];
    let mut peaks = Vec::with_capacity(BINS);
    let mut overlap = vec![[0.0_f64; 2]; FFT_SIZE];
    let mut weights = [0.0; HOP];
    for (index, &weight) in window.iter().enumerate() {
        weights[index % HOP] += weight * weight;
    }

    let half = FFT_SIZE as i64 / 2;
    let mut center = -half + HOP as i64;
    while center - half < samples.len() as i64 {
        if cancelled() {
            return None;
        }
        for (channel, spectrum) in analysis.iter_mut().enumerate() {
            for (index, value) in spectrum.iter_mut().enumerate() {
                let source = center - half + index as i64;
                value.re = if source >= 0 && source < samples.len() as i64 {
                    f64::from(samples[source as usize][channel]) * window[index]
                } else {
                    0.0
                };
                value.im = 0.0;
            }
            fft.transform(spectrum, false);
            for value in spectrum.iter_mut().skip(1).step_by(2) {
                value.re = -value.re;
                value.im = -value.im;
            }
        }

        for bin in 0..BINS {
            energy[bin] = analysis[0][bin].norm_sqr() + analysis[1][bin].norm_sqr();
            let cross = analysis[0][bin]
                .mul(previous[0][bin].conj())
                .add(analysis[1][bin].mul(previous[1][bin].conj()));
            let expected = TAU * bin as f64 * HOP as f64 / FFT_SIZE as f64;
            let delta = if cross.norm_sqr() > 0.0 {
                wrap_phase(cross.im.atan2(cross.re) - expected)
            } else {
                0.0
            };
            frequencies[bin] = ((expected + delta) / HOP as f64).clamp(0.0, PI);
            rotations[bin] =
                wrap_phase(rotations[bin] + (ratio - 1.0) * frequencies[bin] * HOP as f64);
        }

        peaks.clear();
        for bin in 0..BINS {
            if energy[bin] > 0.0
                && (bin == 0 || energy[bin] > energy[bin - 1])
                && (bin + 1 == BINS || energy[bin] >= energy[bin + 1])
            {
                peaks.push(bin);
            }
        }
        for spectrum in &mut synthesis {
            spectrum.fill(Complex::default());
        }
        let min_energy = energy.iter().copied().fold(f64::INFINITY, f64::min);
        let max_energy = energy.iter().copied().fold(0.0, f64::max);
        let flat_transient = max_energy > 0.0 && min_energy > max_energy * 0.95;
        let mut region_start = 0;
        for (index, &peak) in peaks.iter().enumerate() {
            let region_end = peaks
                .get(index + 1)
                .map_or(BINS, |&next| (peak + next) / 2 + 1);
            let displacement = (ratio - 1.0) * frequencies[peak] * FFT_SIZE as f64 / TAU;
            let rotation = rotations[peak];
            let (sin, cos) = rotation.sin_cos();
            let phasor = Complex { re: cos, im: sin };
            for (bin, bin_rotation) in rotations
                .iter_mut()
                .enumerate()
                .take(region_end)
                .skip(region_start)
            {
                *bin_rotation = rotation;
                if frequencies[peak] * ratio > PI {
                    continue;
                }
                let destination = bin as f64 + displacement;
                let lower = destination.floor() as i64;
                let fraction = destination - lower as f64;
                for channel in 0..2 {
                    let shifted = analysis[channel][bin].mul(phasor);
                    for (target, weight) in [(lower, 1.0 - fraction), (lower + 1, fraction)] {
                        if target.unsigned_abs() >= BINS as u64 {
                            continue;
                        }
                        let shifted = if target < 0 { shifted.conj() } else { shifted };
                        let target = target.unsigned_abs() as usize;
                        let source_scale = if bin == 0 || bin + 1 == BINS {
                            1.0
                        } else {
                            2.0
                        };
                        let target_scale = if target == 0 || target + 1 == BINS {
                            1.0
                        } else {
                            2.0
                        };
                        let weight = weight * source_scale / target_scale;
                        let value = &mut synthesis[channel][target];
                        value.re += shifted.re * weight;
                        value.im += shifted.im * weight;
                    }
                }
            }
            region_start = region_end;
        }

        for channel in 0..2 {
            previous[channel].copy_from_slice(&analysis[channel][..BINS]);
            if flat_transient {
                synthesis[channel][..BINS].copy_from_slice(&analysis[channel][..BINS]);
                rotations.fill(0.0);
            }
            synthesis[channel][0].im = 0.0;
            synthesis[channel][FFT_SIZE / 2].im = 0.0;
            for bin in 1..FFT_SIZE / 2 {
                synthesis[channel][FFT_SIZE - bin] = synthesis[channel][bin].conj();
            }
            for value in synthesis[channel].iter_mut().skip(1).step_by(2) {
                value.re = -value.re;
                value.im = -value.im;
            }
            fft.transform(&mut synthesis[channel], true);
        }
        for (index, &weight) in window.iter().enumerate() {
            let destination = center - half + index as i64;
            if destination < 0 || destination >= samples.len() as i64 {
                continue;
            }
            let destination = destination as usize % FFT_SIZE;
            for channel in 0..2 {
                overlap[destination][channel] += synthesis[channel][index].re * weight;
            }
        }
        let start = (center - half).max(0) as usize;
        let end = (center - half + HOP as i64).max(0) as usize;
        for (destination, frame) in output.iter_mut().enumerate().take(end).skip(start) {
            let accumulated = &mut overlap[destination % FFT_SIZE];
            *frame = accumulated.map(|sample| {
                (sample / weights[destination % HOP])
                    .clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
            });
            *accumulated = [0.0; 2];
        }
        center += HOP as i64;
    }

    (!cancelled()).then_some(Cow::Owned(output))
}

fn wrap_phase(phase: f64) -> f64 {
    (phase + PI).rem_euclid(TAU) - PI
}

#[derive(Debug, Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn add(self, other: Self) -> Self {
        Self {
            re: self.re + other.re,
            im: self.im + other.im,
        }
    }

    fn mul(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }

    fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

struct Fft {
    reversed: Vec<usize>,
    roots: Vec<Complex>,
}

impl Fft {
    fn new(size: usize) -> Self {
        let bits = size.trailing_zeros();
        Self {
            reversed: (0..size)
                .map(|index| index.reverse_bits() >> (usize::BITS - bits))
                .collect(),
            roots: (0..size / 2)
                .map(|index| {
                    let (sin, cos) = (-TAU * index as f64 / size as f64).sin_cos();
                    Complex { re: cos, im: sin }
                })
                .collect(),
        }
    }

    fn transform(&self, values: &mut [Complex], inverse: bool) {
        let size = values.len();
        for (index, &reversed) in self.reversed.iter().enumerate() {
            if index < reversed {
                values.swap(index, reversed);
            }
        }
        let mut width = 2;
        while width <= size {
            let half = width / 2;
            for chunk in values.chunks_exact_mut(width) {
                for index in 0..half {
                    let root = self.roots[index * size / width];
                    let root = if inverse { root.conj() } else { root };
                    let odd = chunk[index + half].mul(root);
                    let even = chunk[index];
                    chunk[index] = even.add(odd);
                    chunk[index + half] = Complex {
                        re: even.re - odd.re,
                        im: even.im - odd.im,
                    };
                }
            }
            width *= 2;
        }
        if inverse {
            for value in values {
                value.re /= size as f64;
                value.im /= size as f64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SAMPLE_RATE;
    use std::cell::Cell;

    fn tone(frequency: f64, frames: usize) -> Vec<Frame> {
        (0..frames)
            .map(|index| {
                let sample = (TAU * frequency * index as f64 / f64::from(SAMPLE_RATE)).sin() as f32;
                [sample * 0.5, sample * -0.25]
            })
            .collect()
    }

    fn frequency(samples: &[Frame], channel: usize) -> f64 {
        let crossings: Vec<_> = samples
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| {
                let a = f64::from(pair[0][channel]);
                let b = f64::from(pair[1][channel]);
                (a <= 0.0 && b > 0.0).then(|| index as f64 - a / (b - a))
            })
            .collect();
        assert!(crossings.len() > 10);
        (crossings.len() - 1) as f64 * f64::from(SAMPLE_RATE)
            / (crossings[crossings.len() - 1] - crossings[0])
    }

    #[test]
    fn octave_and_fractional_shifts_preserve_duration_and_stereo_phase() {
        let input = tone(440.0, 24_000);
        for semitones in [-24.0, -12.0, -0.5, 0.5, 7.0, 12.0, 24.0] {
            let shifted = shift(&input, semitones);
            assert_eq!(shifted.len(), input.len());
            let heard = &shifted[4096..shifted.len() - 4096];
            let expected = 440.0 * 2.0_f64.powf(f64::from(semitones) / 12.0);
            let actual = frequency(heard, 0);
            assert!(
                (actual - expected).abs() < expected * 0.005,
                "{semitones} semitones: {actual} Hz, expected {expected} Hz"
            );
            let rms = (heard
                .iter()
                .map(|frame| f64::from(frame[0]).powi(2))
                .sum::<f64>()
                / heard.len() as f64)
                .sqrt();
            assert!(
                rms > 0.2 && rms < 0.6,
                "unexpected level {rms} at {semitones}"
            );
            assert!(shifted.iter().all(|frame| frame[1] == frame[0] * -0.5));
        }
    }

    #[test]
    fn distinct_stereo_tones_are_transposed_in_their_original_channels() {
        let mut input = tone(330.0, 24_000);
        for (index, frame) in input.iter_mut().enumerate() {
            frame[1] = (TAU * 770.0 * index as f64 / f64::from(SAMPLE_RATE)).sin() as f32 * 0.4;
        }
        let output = shift(&input, 12.0);
        let heard = &output[4096..output.len() - 4096];
        assert!((frequency(heard, 0) - 660.0).abs() < 2.0);
        assert!((frequency(heard, 1) - 1540.0).abs() < 2.0);
    }

    #[test]
    fn zero_pitch_borrows_source_and_preserves_sample_bits() {
        let input = vec![[0.0, -0.0], [f32::MIN_POSITIVE, f32::MAX], [-0.125, 0.5]];
        for zero in [0.0, -0.0] {
            let output = shift(&input, zero);
            assert!(matches!(output, Cow::Borrowed(_)));
            assert!(std::ptr::eq(output.as_ptr(), input.as_ptr()));
            assert!(
                output
                    .iter()
                    .flatten()
                    .zip(input.iter().flatten())
                    .all(|(actual, expected)| actual.to_bits() == expected.to_bits())
            );
        }
    }

    #[test]
    fn cancellation_stops_before_work_and_during_long_silence_scan() {
        let input = vec![[0.0; 2]; FFT_SIZE * 32];
        let calls = Cell::new(0);
        assert!(
            shift_cancellable(&input, 12.0, || {
                calls.set(calls.get() + 1);
                true
            })
            .is_none()
        );
        assert_eq!(calls.get(), 1);

        calls.set(0);
        assert!(
            shift_cancellable(&input, 12.0, || {
                calls.set(calls.get() + 1);
                calls.get() == 4
            })
            .is_none()
        );
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn cancellation_stops_initialization_and_in_progress_fft_hops() {
        let input = vec![[0.25, -0.125]; FFT_SIZE * 4];
        for stop_at in [5, 3 + input.len().div_ceil(FFT_SIZE) + 3] {
            let calls = Cell::new(0);
            assert!(
                shift_cancellable(&input, 12.0, || {
                    calls.set(calls.get() + 1);
                    calls.get() == stop_at
                })
                .is_none()
            );
            assert_eq!(calls.get(), stop_at);
        }
    }

    #[test]
    fn uncancelled_preparation_matches_the_offline_processor() {
        let input = tone(440.0, 4096);
        assert_eq!(
            shift_cancellable(&input, 7.5, || false).unwrap(),
            shift(&input, 7.5)
        );
    }

    #[test]
    fn short_clips_extreme_finite_samples_and_silence_remain_finite() {
        for frames in [0, 1, 17, 255, 2047] {
            for semitones in [-24.0, 0.5, 24.0] {
                let silence = vec![[0.0; 2]; frames];
                assert_eq!(&*shift(&silence, semitones), silence);
                let input = vec![[f32::MAX, -f32::MAX]; frames];
                let output = shift(&input, semitones);
                assert_eq!(output.len(), frames);
                assert!(output.iter().flatten().all(|sample| sample.is_finite()));
            }
        }
    }

    #[test]
    fn shifted_ultrasonic_content_does_not_alias_into_audible_output() {
        let input = tone(15_000.0, 24_000);
        let output = shift(&input, 12.0);
        assert!(
            output[4096..output.len() - 4096]
                .iter()
                .all(|frame| frame[0].abs() < 0.001)
        );
    }

    #[test]
    fn low_bass_remains_in_tune_instead_of_becoming_a_dc_peak() {
        for frequency in [20.0, 27.5, 35.0] {
            let input: Vec<Frame> = (0..48_000)
                .map(|index| [(TAU * frequency * index as f64 / 48_000.0).cos() as f32 * 0.5; 2])
                .collect();
            for semitones in [12.0, 24.0] {
                let output = shift(&input, semitones);
                let expected = frequency * 2.0_f64.powf(f64::from(semitones) / 12.0);
                let mut correlation = Complex::default();
                let from = 8192;
                let to = output.len() - 8192;
                for (index, frame) in output.iter().enumerate().take(to).skip(from) {
                    let phase = TAU * expected * index as f64 / 48_000.0;
                    correlation.re += f64::from(frame[0]) * phase.cos();
                    correlation.im += f64::from(frame[0]) * phase.sin();
                }
                let amplitude = 2.0 * correlation.norm_sqr().sqrt() / (to - from) as f64;
                assert!(
                    amplitude > 0.35,
                    "{frequency} Hz at {semitones}: amplitude {amplitude}"
                );
            }
        }
    }

    #[test]
    fn isolated_transient_keeps_its_timing_at_every_hop_offset() {
        for position in [8191, 8192, 8193, 8448] {
            let mut input = vec![[0.0; 2]; 16_384];
            input[position] = [1.0, -0.5];
            for semitones in [-12.0, 12.0] {
                let output = shift(&input, semitones);
                assert!(
                    output
                        .iter()
                        .zip(&input)
                        .all(|(actual, expected)| (actual[0] - expected[0]).abs() < 1e-6
                            && (actual[1] - expected[1]).abs() < 1e-6)
                );
            }
        }
    }

    #[test]
    fn transposing_nyquist_preserves_real_spectrum_endpoint_level() {
        let input: Vec<Frame> = (0..24_000)
            .map(|index| [if index % 2 == 0 { 0.5 } else { -0.5 }; 2])
            .collect();
        for semitones in [-12.0, -24.0] {
            let output = shift(&input, semitones);
            let peak = output[8192..16_000]
                .iter()
                .map(|frame| frame[0].abs())
                .fold(0.0_f32, f32::max);
            assert!(peak < 0.6, "Nyquist endpoint was amplified to {peak}");
            assert!(peak > 0.3, "Nyquist endpoint disappeared: {peak}");
        }
    }

    #[test]
    fn source_cache_retains_live_pitches_and_clones_are_independent() {
        let source = crate::project::AudioBuffer::new(vec![[0.0; 2]; 16]).unwrap();
        let first = source.pitched_samples(0.5);
        assert!(std::sync::Arc::ptr_eq(&first, &source.pitched_samples(0.5)));
        let cloned = source.clone();
        let cloned_pitch = cloned.pitched_samples(0.5);
        assert!(!std::sync::Arc::ptr_eq(&first, &cloned_pitch));
        source.pitched_samples(1.0);
        source.pitched_samples(2.0);
        assert!(std::sync::Arc::ptr_eq(&first, &source.pitched_samples(0.5)));
        assert!(std::sync::Arc::ptr_eq(
            &cloned_pitch,
            &cloned.pitched_samples(0.5)
        ));
    }
}
