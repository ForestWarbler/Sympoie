use super::*;
use crate::builtin::BuiltinKind;

const CONTEXT: PluginProcessContext = PluginProcessContext {
    frame: 0,
    bpm: 120.0,
    playing: true,
};

fn set(instance: &mut PluginInstance, id: u32, physical: f64) {
    let specs = match BuiltinKind::from_descriptor(&instance.descriptor).unwrap() {
        BuiltinKind::Eq => eq_parameters(),
        BuiltinKind::Comp => comp_parameters(),
        _ => unreachable!(),
    };
    let spec = specs.iter().find(|spec| spec.id == id).unwrap();
    instance
        .parameters
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .value = spec.normalize(physical);
}

fn sine(hz: f64, amplitude: f64, count: usize) -> Vec<Frame> {
    (0..count)
        .map(|i| {
            let sample = (amplitude * (TAU * hz * i as f64 / RATE).sin()) as f32;
            [sample, sample]
        })
        .collect()
}

fn rms(frames: &[Frame], channel: usize) -> f64 {
    (frames
        .iter()
        .map(|frame| f64::from(frame[channel]).powi(2))
        .sum::<f64>()
        / frames.len() as f64)
        .sqrt()
}

fn meter(field: &std::sync::atomic::AtomicU32) -> f64 {
    f64::from(f32::from_bits(field.load(Ordering::Relaxed)))
}

#[test]
fn all_six_filter_shapes_have_expected_frequency_response() {
    let response = |kind, hz| {
        decibels(
            band_coefficients(&[1.0, kind, 1000.0, 12.0, std::f64::consts::FRAC_1_SQRT_2])
                .response(hz)
                .power()
                .sqrt(),
        )
    };
    assert!((response(0.0, 1000.0) - 12.0).abs() < 1.0e-8);
    assert!((response(1.0, 0.0) - 12.0).abs() < 1.0e-8);
    assert!((response(1.0, 24000.0)).abs() < 1.0e-8);
    assert!((response(2.0, 24000.0) - 12.0).abs() < 1.0e-8);
    assert!((response(2.0, 0.0)).abs() < 1.0e-8);
    assert!((response(3.0, 1000.0) + 3.0103).abs() < 0.001);
    assert!(response(3.0, 100.0) < -39.0);
    assert!((response(4.0, 1000.0) + 3.0103).abs() < 0.001);
    assert!(response(4.0, 10000.0) < -39.0);
    assert!(response(5.0, 1000.0) < -100.0);
    assert!(response(5.0, 100.0) > -0.1);
}

#[test]
fn eq_audio_matches_curve_and_only_modifies_its_channel() {
    let mut instance = BuiltinKind::Eq.instance();
    set(&mut instance, 1, 0.0);
    set(&mut instance, 2, 1000.0);
    set(&mut instance, 3, 12.0);
    let mut processor = create_eq(&instance, None).unwrap();
    let mut frames = sine(1000.0, 0.1, 24000);
    for frame in &mut frames {
        frame[1] = 0.0;
    }
    let input_rms = rms(&frames[12000..], 0);
    processor.process(&mut frames, CONTEXT);
    let measured = decibels(rms(&frames[12000..], 0) / input_rms);
    assert!((measured - 12.0).abs() < 0.001, "measured {measured}");
    assert!((measured - eq_response_db(&instance, 1000.0)).abs() < 0.001);
    assert_eq!(rms(&frames, 1), 0.0);
}

#[test]
fn eq_mix_includes_filter_phase_and_disabled_bands_are_flat() {
    let mut instance = BuiltinKind::Eq.instance();
    set(&mut instance, 1, 3.0);
    set(&mut instance, 2, 1000.0);
    set(&mut instance, 31, 50.0);
    let mut processor = create_eq(&instance, None).unwrap();
    let mut frames = sine(1000.0, 0.1, 24000);
    let input = rms(&frames[12000..], 0);
    processor.process(&mut frames, CONTEXT);
    assert!(
        (decibels(rms(&frames[12000..], 0) / input) - eq_response_db(&instance, 1000.0)).abs()
            < 0.001
    );
    set(&mut instance, 0, 0.0);
    for hz in [20.0, 1000.0, 20000.0] {
        assert!(eq_response_db(&instance, hz).abs() < 1.0e-7);
    }
}

#[test]
fn eq_filters_stay_finite_at_parameter_extremes_and_after_automation() {
    let mut instance = BuiltinKind::Eq.instance();
    for band in 0..6 {
        set(&mut instance, band * 5 + 1, f64::from(band));
        set(
            &mut instance,
            band * 5 + 2,
            if band % 2 == 0 { 20.0 } else { 20000.0 },
        );
        set(&mut instance, band * 5 + 3, 24.0);
        set(&mut instance, band * 5 + 4, 18.0);
    }
    let mut processor = create_eq(&instance, None).unwrap();
    let mut block = [[0.001; 2]; 256];
    for i in 0..300 {
        let band = i % 6;
        processor.set_parameter(band * 5 + 2, if i % 2 == 0 { 0.0 } else { 1.0 });
        processor.set_parameter(band * 5 + 3, if i % 3 == 0 { 0.0 } else { 1.0 });
        block.fill([0.001; 2]);
        processor.process(&mut block, CONTEXT);
        assert!(block.iter().flatten().all(|sample| sample.is_finite()));
    }
    processor.reset();
    block.fill([0.0; 2]);
    processor.process(&mut block, CONTEXT);
    assert!(block.iter().flatten().all(|sample| *sample == 0.0));
}

#[test]
fn fft_reports_actual_post_eq_spectrum_without_stereo_cancellation() {
    let mut instance = BuiltinKind::Eq.instance();
    set(&mut instance, 30, -6.0);
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = create_eq(&instance, Some(telemetry.clone())).unwrap();
    let frequency = 187.0 * RATE / FFT_SIZE as f64;
    let mut frames = sine(frequency, 0.4, 48000);
    for frame in &mut frames {
        frame[1] = -frame[0];
    }
    for block in frames.chunks_mut(127) {
        processor.process(block, CONTEXT);
    }
    let spectrum: Vec<f64> = telemetry.spectrum.iter().map(meter).collect();
    let (bin, db) = spectrum
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    let bin_frequency = 20.0 * 1000.0_f64.powf(bin as f64 / 127.0);
    assert!(
        (bin_frequency / frequency).ln().abs() < 0.06,
        "{bin_frequency} vs {frequency}"
    );
    assert!((*db - (decibels(0.4) - 6.0)).abs() < 0.1, "{db}");
    assert!((meter(&telemetry.input_db) - decibels(0.4)).abs() < 0.1);
    assert!((meter(&telemetry.output_db) - (decibels(0.4) - 6.0)).abs() < 0.1);
    assert_eq!(meter(&telemetry.reduction_db), 0.0);
    processor.reset();
    assert_eq!(meter(&telemetry.output_db), -120.0);
    assert!(telemetry.spectrum.iter().all(|bin| meter(bin) == -120.0));
}

#[test]
fn compressor_curve_has_correct_ratio_and_continuous_soft_knee() {
    assert_eq!(reduction_db(-30.0, -18.0, 4.0, 0.0), 0.0);
    assert_eq!(reduction_db(-6.0, -18.0, 4.0, 0.0), 9.0);
    assert_eq!(reduction_db(-18.0, -18.0, 4.0, 6.0), 0.5625);
    for edge in [-21.0, -15.0] {
        assert!(
            (reduction_db(edge - 1.0e-6, -18.0, 4.0, 6.0)
                - reduction_db(edge + 1.0e-6, -18.0, 4.0, 6.0))
            .abs()
                < 2.0e-6
        );
    }
    assert_eq!(reduction_db(12.0, -60.0, 1.0, 24.0), 0.0);
}

fn compressor() -> PluginInstance {
    let mut instance = BuiltinKind::Comp.instance();
    set(&mut instance, 2, 0.0);
    set(&mut instance, 3, 0.05);
    instance
}

fn alternating(count: usize, left: f32, right: f32) -> Vec<Frame> {
    (0..count)
        .map(|i| {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            [sign * left, sign * right]
        })
        .collect()
}

#[test]
fn compressor_hits_threshold_ratio_and_links_stereo() {
    let instance = compressor();
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = create_comp(&instance, Some(telemetry.clone())).unwrap();
    let mut frames = alternating(24000, 1.0, 0.25);
    processor.process(&mut frames, CONTEXT);
    let attenuation = -decibels(rms(&frames[12000..], 0));
    assert!((attenuation - 13.5).abs() < 0.03, "{attenuation}");
    for frame in &frames[12000..] {
        assert!((frame[0] * 0.25 - frame[1]).abs() < 1.0e-7);
    }
    assert!((meter(&telemetry.reduction_db) - attenuation).abs() < 0.03);
    assert!((meter(&telemetry.output_db) + attenuation).abs() < 0.03);
}

#[test]
fn external_bus_controls_compression_and_absent_bus_is_silent() {
    let mut instance = compressor();
    set(&mut instance, 10, 1.0);
    let mut processor = create_comp(&instance, None).unwrap();
    let sidechain = alternating(24000, 1.0, 1.0);
    let mut audio = vec![[0.1, 0.05]; 24000];
    processor.process_with_sidechain(
        &mut audio,
        Some(PluginSidechainInput {
            bus_index: 1,
            frames: &sidechain,
        }),
        CONTEXT,
    );
    assert!((audio[23999][0] - 0.02116).abs() < 0.0001);
    for bus in [
        None,
        Some(PluginSidechainInput {
            bus_index: 2,
            frames: &sidechain,
        }),
    ] {
        processor.reset();
        audio.fill([0.1, 0.05]);
        processor.process_with_sidechain(&mut audio, bus, CONTEXT);
        assert!(audio.iter().all(|frame| *frame == [0.1, 0.05]));
    }
}

#[test]
fn compressor_rms_detector_and_high_pass_change_only_detection() {
    let mut instance = compressor();
    set(&mut instance, 9, 1.0);
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = create_comp(&instance, Some(telemetry.clone())).unwrap();
    let mut frames = sine(1000.0, 1.0, 48000);
    for (i, frame) in frames.iter_mut().enumerate() {
        frame[1] = (TAU * 1000.0 * i as f64 / RATE).cos() as f32;
    }
    processor.process(&mut frames, CONTEXT);
    assert!((meter(&telemetry.reduction_db) - (18.0 - 3.0103) * 0.75).abs() < 0.15);

    set(&mut instance, 8, 1000.0);
    set(&mut instance, 1, 1.0);
    let mut processor = create_comp(&instance, None).unwrap();
    let mut bass = sine(50.0, 0.5, 24000);
    let original = bass.clone();
    processor.process(&mut bass, CONTEXT);
    assert_eq!(
        bass, original,
        "detector high pass must not remove bass from audio"
    );
}

#[test]
fn compressor_dry_mix_and_unity_ratio_keep_original_signal() {
    for (id, physical) in [(6, 0.0), (1, 1.0)] {
        let mut instance = compressor();
        set(&mut instance, id, physical);
        let mut processor = create_comp(&instance, None).unwrap();
        let mut frames = sine(110.0, 0.8, 24000);
        let original = frames.clone();
        processor.process(&mut frames, CONTEXT);
        assert_eq!(frames, original);
    }
}

#[test]
fn compressor_attack_and_release_follow_requested_time_constants() {
    let mut instance = compressor();
    set(&mut instance, 3, 20.0);
    set(&mut instance, 4, 100.0);
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = create_comp(&instance, Some(telemetry.clone())).unwrap();
    let mut attack = alternating(960, 1.0, 1.0);
    processor.process(&mut attack, CONTEXT);
    let initial = meter(&telemetry.reduction_db);
    assert!(
        (initial - 13.49 * (1.0 - (-1.0_f64).exp())).abs() < 0.1,
        "{initial}"
    );
    let mut silence = [[0.0; 2]; 4800];
    processor.process(&mut silence, CONTEXT);
    let mut probe = [[1.0e-5; 2]; 1];
    processor.process(&mut probe, CONTEXT);
    let residual = -decibels(f64::from(probe[0][0]) / 1.0e-5);
    assert!(
        (residual - initial / std::f64::consts::E).abs() < 0.2,
        "{residual}"
    );
}
