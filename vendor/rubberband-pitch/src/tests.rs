use super::shift;
use std::cell::Cell;
use std::f32::consts::TAU;

fn tone(length: usize, sample_rate: u32, frequency: f32) -> Vec<[f32; 2]> {
    (0..length)
        .map(|i| {
            let value = (TAU * frequency * i as f32 / sample_rate as f32).sin() * 0.3;
            [value, value * 0.5]
        })
        .collect()
}

fn measured_frequency(samples: &[[f32; 2]], sample_rate: u32) -> f64 {
    let middle = &samples[samples.len() / 4..samples.len() * 3 / 4];
    let crossings: Vec<f64> = middle
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
        .map(|(i, pair)| i as f64 + f64::from(-pair[0][0]) / f64::from(pair[1][0] - pair[0][0]))
        .collect();
    (crossings.len() - 1) as f64 * f64::from(sample_rate)
        / (crossings.last().unwrap() - crossings[0])
}

#[test]
fn r3_pitch_changes_frequency_without_duration_change() {
    let source = tone(48_137, 48_000, 440.0);
    for semitones in [-12.0, 7.25, 12.0] {
        let output = shift(&source, 48_000, semitones, false, &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(output.len(), source.len());
        let frequency = measured_frequency(&output, 48_000);
        let expected = 440.0 * 2.0_f64.powf(f64::from(semitones) / 12.0);
        assert!(
            (frequency - expected).abs() < expected * 0.01,
            "{frequency} != {expected}"
        );
        assert!(output.iter().any(|frame| frame[0].abs() > 0.1));
    }
}

#[test]
fn supported_sample_rate_extremes_keep_duration_and_frequency() {
    for sample_rate in [8_000, 96_000, 192_000] {
        let source = tone(sample_rate as usize / 3, sample_rate, 220.0);
        for semitones in [-24.0, 24.0] {
            let output = shift(&source, sample_rate, semitones, false, &|| false)
                .unwrap()
                .unwrap();
            assert_eq!(output.len(), source.len());
            let frequency = measured_frequency(&output, sample_rate);
            let expected = 220.0 * 2.0_f64.powf(f64::from(semitones) / 12.0);
            assert!(
                (frequency - expected).abs() < expected * 0.02,
                "{sample_rate} Hz, {semitones}: {frequency} != {expected}"
            );
        }
    }
}

#[test]
fn linked_stereo_and_repeatability() {
    let source = tone(24_000, 48_000, 330.0);
    let first = shift(&source, 48_000, -5.5, true, &|| false)
        .unwrap()
        .unwrap();
    let second = shift(&source, 48_000, -5.5, true, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(first, second);
    let error: f64 = first
        .iter()
        .map(|frame| f64::from(frame[1] - frame[0] * 0.5).powi(2))
        .sum();
    let energy: f64 = first.iter().map(|frame| f64::from(frame[0]).powi(2)).sum();
    assert!(
        error < energy * 1e-5,
        "linked stereo error {error}, energy {energy}"
    );
}

#[test]
fn short_clips_and_silence_keep_exact_length_and_finite_output() {
    for length in [1, 17, 511, 1024, 2049] {
        for semitones in [-24.0, 7.0, 24.0] {
            let source = tone(length, 44_100, 440.0);
            let output = shift(&source, 44_100, semitones, true, &|| false)
                .unwrap()
                .unwrap();
            assert_eq!(output.len(), length);
            assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        }
    }
    let silence = vec![[0.0; 2]; 5000];
    let output = shift(&silence, 44_100, 7.0, true, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(output, silence);
}

#[test]
fn cancellation_before_start_and_during_processing_publishes_nothing() {
    let source = tone(48_000, 48_000, 440.0);
    assert!(
        shift(&source, 48_000, 7.0, true, &|| true)
            .unwrap()
            .is_none()
    );
    let polls = Cell::new(0);
    let result = shift(&source, 48_000, 7.0, true, &|| {
        polls.set(polls.get() + 1);
        polls.get() >= 60
    })
    .unwrap();
    assert!(result.is_none());
    assert_eq!(polls.get(), 60);
}

#[test]
fn zero_pitch_is_bit_exact_and_invalid_parameters_are_errors() {
    let source = tone(5000, 48_000, 440.0);
    assert_eq!(
        shift(&source, 48_000, 0.0, true, &|| false)
            .unwrap()
            .unwrap(),
        source
    );
    assert_eq!(
        shift(&[], 48_000, 7.0, true, &|| false).unwrap(),
        Some(Vec::new())
    );
    assert!(shift(&source, 0, 7.0, true, &|| false).is_err());
    assert!(shift(&source, 48_000, f32::NAN, true, &|| false).is_err());
    assert!(shift(&[[f32::INFINITY, 0.0]], 48_000, 7.0, true, &|| false).is_err());
}

fn harmonic_centroid(samples: &[[f32; 2]], sample_rate: u32, base: f64) -> f64 {
    let middle = &samples[samples.len() / 4..samples.len() * 3 / 4];
    let mut weighted = 0.0;
    let mut total = 0.0;
    for harmonic in 1..=30 {
        let frequency = base * f64::from(harmonic);
        let mut real = 0.0;
        let mut imaginary = 0.0;
        for (i, frame) in middle.iter().enumerate() {
            let phase = std::f64::consts::TAU * frequency * i as f64 / f64::from(sample_rate);
            let sample = f64::from(frame[0]);
            real += sample * phase.cos();
            imaginary += sample * phase.sin();
        }
        let energy = real * real + imaginary * imaginary;
        weighted += frequency * energy;
        total += energy;
    }
    weighted / total
}

#[test]
fn formant_preservation_keeps_synthetic_vowel_envelope() {
    let sample_rate = 48_000;
    let source: Vec<[f32; 2]> = (0..sample_rate)
        .map(|i| {
            let value: f64 = (1..=50)
                .map(|harmonic| {
                    let frequency = 100.0 * f64::from(harmonic);
                    let envelope = (-0.5 * ((frequency - 1500.0) / 300.0).powi(2)).exp();
                    envelope
                        * (std::f64::consts::TAU * frequency * f64::from(i)
                            / f64::from(sample_rate))
                        .sin()
                        * 0.05
                })
                .sum();
            [value as f32; 2]
        })
        .collect();
    let shifted = shift(&source, sample_rate, 12.0, false, &|| false)
        .unwrap()
        .unwrap();
    let preserved = shift(&source, sample_rate, 12.0, true, &|| false)
        .unwrap()
        .unwrap();
    let shifted_centroid = harmonic_centroid(&shifted, sample_rate, 200.0);
    let preserved_centroid = harmonic_centroid(&preserved, sample_rate, 200.0);
    assert!(
        (shifted_centroid - 3000.0).abs() < 300.0,
        "shifted envelope {shifted_centroid}"
    );
    assert!(
        (preserved_centroid - 1500.0).abs() < 400.0,
        "preserved envelope {preserved_centroid}"
    );
    assert!((preserved_centroid - 1500.0).abs() < (shifted_centroid - 1500.0).abs() * 0.5);
}
