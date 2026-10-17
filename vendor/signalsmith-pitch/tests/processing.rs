use std::cell::Cell;
use std::f32::consts::TAU;
use sympoie_signalsmith_pitch::shift;

fn sine(length: usize, sample_rate: u32, frequency: f32) -> Vec<[f32; 2]> {
    (0..length)
        .map(|i| {
            let sample = (i as f32 * TAU * frequency / sample_rate as f32).sin() * 0.25;
            [sample, sample]
        })
        .collect()
}

fn spectral_power(samples: &[[f32; 2]], sample_rate: u32, frequency: f32) -> f64 {
    let (mut real, mut imag) = (0.0_f64, 0.0_f64);
    for (i, frame) in samples.iter().enumerate() {
        let phase = f64::from(TAU) * f64::from(frequency) * i as f64 / f64::from(sample_rate);
        let window = 0.5 - 0.5 * (f64::from(TAU) * i as f64 / samples.len() as f64).cos();
        real += f64::from(frame[0]) * window * phase.cos();
        imag += f64::from(frame[0]) * window * phase.sin();
    }
    real * real + imag * imag
}

#[test]
fn transposes_both_directions_and_fractional_pitch_without_changing_length() {
    let sample_rate = 16_000;
    let source = sine(sample_rate as usize, sample_rate, 440.0);
    for semitones in [-12.0_f32, 7.25, 12.0] {
        let output = shift(&source, sample_rate, semitones, false, &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(output.len(), source.len());
        let middle = &output[4000..12000];
        let target = 440.0 * 2.0_f32.powf(semitones / 12.0);
        let wanted = spectral_power(middle, sample_rate, target);
        let old = spectral_power(middle, sample_rate, 440.0);
        assert!(wanted > 10_000.0, "no pitched signal at {target}: {wanted}");
        assert!(
            wanted > old * 100.0,
            "old pitch remains at {semitones}: {wanted}/{old}"
        );
    }
}

#[test]
fn stereo_is_coherent_and_repeated_processing_is_deterministic() {
    let mut source = sine(16_000, 16_000, 220.0);
    for frame in &mut source {
        frame[1] = -frame[0];
    }
    let a = shift(&source, 16_000, 5.0, true, &|| false)
        .unwrap()
        .unwrap();
    let b = shift(&source, 16_000, 5.0, true, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(a, b);
    let max = a
        .iter()
        .map(|frame| (frame[0] + frame[1]).abs())
        .fold(0.0_f32, f32::max);
    let rms = (a
        .iter()
        .map(|frame| (frame[0] + frame[1]).powi(2))
        .sum::<f32>()
        / a.len() as f32)
        .sqrt();
    assert!(
        max < 0.001 && rms < 0.0001,
        "stereo residual max {max}, rms {rms}"
    );
}

#[test]
fn empty_zero_pitch_and_silence_are_exact() {
    assert!(
        shift(&[], 48_000, 7.0, true, &|| false)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    let source = sine(1000, 48_000, 440.0);
    assert_eq!(
        shift(&source, 48_000, 0.0, true, &|| false)
            .unwrap()
            .unwrap(),
        source
    );
    let silence = vec![[0.0; 2]; 10_000];
    assert_eq!(
        shift(&silence, 48_000, 12.0, true, &|| false)
            .unwrap()
            .unwrap(),
        silence
    );
}

#[test]
fn clips_shorter_than_the_analysis_window_are_not_lost() {
    for length in [1, 7, 64, 320, 1000, 3000] {
        let mut source = sine(length, 48_000, 440.0);
        source[0] = [0.5; 2];
        let output = shift(&source, 48_000, 7.0, true, &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(output.len(), length);
        let power: f32 = output.iter().map(|frame| frame[0] * frame[0]).sum();
        assert!(
            power > 1e-10,
            "short clip of {length} frames became silent: {power}"
        );
    }
}

#[test]
fn cancellation_before_and_during_processing_does_not_publish_partial_audio() {
    let source = sine(48_000, 48_000, 220.0);
    assert!(
        shift(&source, 48_000, 12.0, true, &|| true)
            .unwrap()
            .is_none()
    );
    let checks = Cell::new(0);
    let cancel = || {
        checks.set(checks.get() + 1);
        checks.get() > 55
    };
    assert!(
        shift(&source, 48_000, 12.0, true, &cancel)
            .unwrap()
            .is_none()
    );
    assert_eq!(checks.get(), 56);
}

#[test]
fn onset_and_tail_are_aligned_after_latency_compensation() {
    let sample_rate = 16_000;
    let mut source = sine(16_000, sample_rate, 440.0);
    for (i, frame) in source.iter_mut().enumerate() {
        let amplitude = if !(1600..14_400).contains(&i) {
            1.0
        } else if (7200..8800).contains(&i) {
            let phase = (i - 7200) as f32 / 1600.0 * std::f32::consts::PI;
            phase.sin().powi(2)
        } else {
            0.0
        };
        frame[0] *= amplitude;
        frame[1] *= amplitude;
    }
    let output = shift(&source, sample_rate, 7.0, false, &|| false)
        .unwrap()
        .unwrap();
    let power = |slice: &[[f32; 2]]| slice.iter().map(|f| f[0] * f[0]).sum::<f32>();
    assert!(power(&output[..1600]) > power(&source[..1600]) * 0.2);
    assert!(power(&output[14_400..]) > power(&source[14_400..]) * 0.2);
    let centroid = |samples: &[[f32; 2]]| {
        let segment = &samples[6000..10_000];
        segment
            .iter()
            .enumerate()
            .map(|(i, f)| i as f32 * f[0] * f[0])
            .sum::<f32>()
            / power(segment)
    };
    assert!((centroid(&source) - centroid(&output)).abs() < 100.0);
}

#[test]
fn invalid_inputs_return_errors() {
    assert!(shift(&[[0.0; 2]], 0, 12.0, false, &|| false).is_err());
    assert!(shift(&[[0.0; 2]], 48_000, f32::NAN, false, &|| false).is_err());
    assert!(shift(&[[f32::INFINITY; 2]], 48_000, 12.0, false, &|| false).is_err());
}

#[test]
fn formant_preservation_keeps_the_spectral_envelope_nearer_the_original() {
    let sample_rate = 16_000;
    let source: Vec<_> = (0..sample_rate)
        .map(|i| {
            let sample = (1..40)
                .map(|harmonic| {
                    let frequency = harmonic as f32 * 100.0;
                    let amplitude = (-0.5 * ((frequency - 800.0) / 200.0).powi(2)).exp();
                    amplitude * (TAU * frequency * i as f32 / sample_rate as f32).sin()
                })
                .sum::<f32>()
                * 0.05;
            [sample, sample]
        })
        .collect();
    let moved = shift(&source, sample_rate, 12.0, false, &|| false)
        .unwrap()
        .unwrap();
    let preserved = shift(&source, sample_rate, 12.0, true, &|| false)
        .unwrap()
        .unwrap();
    let ratio = |samples: &[[f32; 2]]| {
        let mid = &samples[4000..12000];
        let original: f64 = [600.0, 800.0, 1000.0]
            .into_iter()
            .map(|f| spectral_power(mid, sample_rate, f))
            .sum();
        let moved: f64 = [1400.0, 1600.0, 1800.0]
            .into_iter()
            .map(|f| spectral_power(mid, sample_rate, f))
            .sum();
        original / moved.max(1e-15)
    };
    assert!(
        ratio(&preserved) > ratio(&moved) * 3.0,
        "formant compensation ineffective: preserved {}, moved {}",
        ratio(&preserved),
        ratio(&moved)
    );
}
