use std::sync::atomic::Ordering;

use super::*;
use crate::builtin::BuiltinKind;

const CONTEXT: PluginProcessContext = PluginProcessContext {
    frame: 0,
    bpm: 120.0,
    playing: true,
    quality: crate::plugin::RenderQuality::Realtime,
};

fn set(instance: &mut PluginInstance, id: u32, physical: f64) {
    let spec = PARAMETERS[id as usize];
    instance
        .parameters
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .value = spec.normalize(physical);
}

/// Clean (colourless) unit so gain-cell behaviour can be measured exactly.
fn varimu(settings: &[(u32, f64)]) -> PluginInstance {
    let mut instance = BuiltinKind::VariMu.instance();
    set(&mut instance, COLOR, 0.0);
    for &(id, value) in settings {
        set(&mut instance, id, value);
    }
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

fn sine(hz: f64, level: f64, count: usize) -> Vec<f64> {
    (0..count)
        .map(|i| level * (TAU * hz * i as f64 / RATE).sin())
        .collect()
}

fn magnitude(signal: &[f64], hz: f64) -> f64 {
    let (re, im) = signal
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (i, sample)| {
            let phase = TAU * hz * i as f64 / RATE;
            (re + sample * phase.cos(), im - sample * phase.sin())
        });
    2.0 * (re * re + im * im).sqrt() / signal.len() as f64
}

/// Gain reduction the unit currently applies, probed with a signal far below threshold.
fn probe_reduction(processor: &mut Box<dyn PluginProcessor>) -> f64 {
    let mut probe = [[1.0e-5; 2]; 1];
    processor.process(&mut probe, CONTEXT);
    -decibels(f64::from(probe[0][0]) / 1.0e-5)
}

#[test]
fn parameter_ids_index_the_spec_table() {
    for (index, spec) in PARAMETERS.iter().enumerate() {
        assert_eq!(spec.id as usize, index, "{}", spec.name);
    }
}

#[test]
fn static_curve_has_a_wide_knee_and_a_ratio_that_grows_with_depth() {
    for ratio in [0.0, 1.0, 2.0] {
        assert_eq!(static_output_db(-30.0, -18.0, ratio), -30.0);
        let edge = static_output_db(-18.0 + 1.0e-6, -18.0, ratio);
        assert!((edge + 18.0).abs() < 1.0e-5);
        let local = |level: f64| {
            1.0 / (static_output_db(level + 0.01, -18.0, ratio)
                - static_output_db(level, -18.0, ratio))
                * 0.01
        };
        let (near, deep) = (local(-17.0), local(6.0));
        assert!(near > 1.0 && near < deep, "{ratio}: {near} vs {deep}");
        let asymptote = 1.0 + CURVES[ratio as usize].exponent;
        assert!(deep < asymptote, "{ratio}: {deep} vs {asymptote}");
    }
    let reduction = |ratio| 0.0 - static_output_db(0.0, -18.0, ratio);
    assert!(reduction(0.0) < reduction(1.0) && reduction(1.0) < reduction(2.0));
}

#[test]
fn feedback_loop_settles_on_the_static_curve_and_links_stereo() {
    let instance = varimu(&[(THRESHOLD, -18.0), (TIME_CONSTANT, 1.0)]);
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = create_varimu(&instance, Some(telemetry.clone())).unwrap();
    let mut frames = alternating(48000, 0.5, 0.125);
    processor.process(&mut frames, CONTEXT);
    let expected = static_output_db(decibels(0.5), -18.0, 1.0);
    let measured = decibels(f64::from(frames[47999][0].abs()));
    assert!(
        (measured - expected).abs() < 0.05,
        "{measured} vs {expected}"
    );
    for frame in &frames[24000..] {
        assert!((frame[0] * 0.25 - frame[1]).abs() < 1.0e-7);
    }
    let reduction = f64::from(f32::from_bits(
        telemetry.reduction_db.load(Ordering::Relaxed),
    ));
    assert!((reduction - (decibels(0.5) - expected)).abs() < 0.05);
}

#[test]
fn dual_mode_compresses_each_side_on_its_own() {
    let quiet = 0.05_f32;
    for (mode, untouched) in [(0.0, false), (1.0, true)] {
        let instance = varimu(&[(STEREO, mode)]);
        let mut processor = create_varimu(&instance, None).unwrap();
        let mut frames = alternating(24000, 0.5, quiet);
        processor.process(&mut frames, CONTEXT);
        assert!(frames[23999][0].abs() < 0.3);
        assert_eq!(frames[23999][1].abs() == quiet, untouched);
    }
}

#[test]
fn mid_side_mode_leaves_a_mono_signal_mono() {
    let instance = BuiltinKind::VariMu.instance();
    let mut instance = instance;
    set(&mut instance, STEREO, 2.0);
    set(&mut instance, INPUT, 12.0);
    let mut processor = create_varimu(&instance, None).unwrap();
    let mut frames: Vec<Frame> = sine(220.0, 0.7, 24000)
        .into_iter()
        .map(|sample| [sample as f32; 2])
        .collect();
    processor.process(&mut frames, CONTEXT);
    assert!(frames.iter().all(|frame| frame[0] == frame[1]));
    assert!(frames.iter().any(|frame| frame[0].abs() > 0.05));
}

#[test]
fn signal_below_threshold_passes_untouched_with_zero_latency() {
    let instance = varimu(&[]);
    let mut processor = create_varimu(&instance, None).unwrap();
    assert_eq!(processor.latency_samples(), 0);
    let mut frames = vec![[0.0; 2]; 64];
    frames[0] = [0.1, -0.05];
    let original = frames.clone();
    processor.process(&mut frames, CONTEXT);
    assert_eq!(frames, original);
}

#[test]
fn dry_mix_is_bit_exact() {
    let mut instance = BuiltinKind::VariMu.instance();
    set(&mut instance, MIX, 0.0);
    set(&mut instance, INPUT, 18.0);
    let mut processor = create_varimu(&instance, None).unwrap();
    let mut frames: Vec<Frame> = sine(110.0, 0.8, 24000)
        .into_iter()
        .map(|sample| [sample as f32, -sample as f32 * 0.5])
        .collect();
    let original = frames.clone();
    processor.process(&mut frames, CONTEXT);
    assert_eq!(frames, original);
}

#[test]
fn fastest_limiting_attack_converges_without_ringing() {
    let instance = varimu(&[
        (RATIO, 2.0),
        (TIME_CONSTANT, 0.0),
        (INPUT, 24.0),
        (THRESHOLD, -40.0),
    ]);
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = create_varimu(&instance, Some(telemetry)).unwrap();
    let mut frames = alternating(2400, 1.0, 1.0);
    processor.process(&mut frames, CONTEXT);
    let levels: Vec<f64> = frames
        .iter()
        .map(|frame| f64::from(frame[0].abs()))
        .collect();
    let expected = static_output_db(24.0, -40.0, 2.0);
    assert!((decibels(levels[2399]) - expected).abs() < 0.05);
    // Release ripple between rectifier charges is allowed; over-compression is not.
    for pair in levels.windows(2) {
        assert!(pair[1] <= pair[0] * 1.001, "{pair:?}");
    }
    let deepest = levels.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(decibels(deepest) > expected - 0.05, "{deepest}");
    let mut hostile = vec![[f32::NAN, f32::INFINITY], [1.0e30, -1.0e30], [1.0e-40, 0.0]];
    processor.process(&mut hostile, CONTEXT);
    assert!(hostile.iter().flatten().all(|sample| sample.is_finite()));
}

#[test]
fn program_dependent_release_holds_longer_after_sustained_compression() {
    let residual = |burst: usize| {
        let instance = varimu(&[(TIME_CONSTANT, 5.0), (THRESHOLD, -24.0)]);
        let mut processor = create_varimu(&instance, None).unwrap();
        let mut loud = alternating(burst, 0.5, 0.5);
        processor.process(&mut loud, CONTEXT);
        let mut silence = vec![[0.0; 2]; 72000];
        processor.process(&mut silence, CONTEXT);
        probe_reduction(&mut processor)
    };
    let (short, long) = (residual(1440), residual(96000));
    assert!(short < 1.0, "{short}");
    assert!(long > 4.0 * short && long > 3.0, "{short} vs {long}");
}

#[test]
fn variable_time_constant_uses_the_release_control() {
    let residual = |release: f64| {
        let instance = varimu(&[
            (TIME_CONSTANT, VARIABLE_TIME as f64),
            (ATTACK, 1.0),
            (RELEASE, release),
        ]);
        let mut processor = create_varimu(&instance, None).unwrap();
        let mut loud = alternating(24000, 0.5, 0.5);
        processor.process(&mut loud, CONTEXT);
        let mut silence = vec![[0.0; 2]; 48000];
        processor.process(&mut silence, CONTEXT);
        probe_reduction(&mut processor)
    };
    assert!(residual(100.0) < 0.01);
    assert!(residual(2000.0) > 3.0);
}

#[test]
fn external_bus_keys_compression_and_absent_bus_disables_it() {
    let instance = varimu(&[(EXTERNAL, 1.0)]);
    let mut processor = create_varimu(&instance, None).unwrap();
    let key = alternating(24000, 1.0, 1.0);
    let mut audio = vec![[0.1, 0.05]; 24000];
    processor.process_with_sidechain(
        &mut audio,
        Some(PluginSidechainInput {
            bus_index: 1,
            frames: &key,
        }),
        CONTEXT,
    );
    assert!(audio[23999][0] < 0.05 && audio[23999][0] > 0.0);
    assert!((audio[23999][0] * 0.5 - audio[23999][1]).abs() < 1.0e-7);
    for bus in [
        None,
        Some(PluginSidechainInput {
            bus_index: 2,
            frames: &key,
        }),
    ] {
        processor.reset();
        audio.fill([0.1, 0.05]);
        processor.process_with_sidechain(&mut audio, bus, CONTEXT);
        assert!(audio.iter().all(|frame| *frame == [0.1, 0.05]));
    }
}

#[test]
fn colour_adds_second_harmonic_that_grows_with_gain_reduction() {
    let signal = sine(1000.0, 0.3, 48000);
    let second = |depth: f64, amount: f64| {
        let mut dc = HighPass::default();
        let shaped: Vec<f64> = signal
            .iter()
            .map(|sample| color(*sample, depth, amount, &mut dc))
            .collect();
        let settled = &shaped[4800..];
        decibels(magnitude(settled, 2000.0) / magnitude(settled, 1000.0))
    };
    assert!(second(0.0, 0.0) < -110.0);
    let (light, deep) = (second(0.0, 1.0), second(0.8, 1.0));
    assert!(light > -50.0 && light < -35.0, "{light}");
    assert!(deep > light + 8.0, "{light} vs {deep}");
    let mut dc = HighPass::default();
    let mean = signal
        .iter()
        .map(|sample| color(*sample, 0.8, 1.0, &mut dc))
        .skip(24000)
        .sum::<f64>()
        / 24000.0;
    assert!(mean.abs() < 1.0e-4, "{mean}");
}

#[test]
fn scope_and_spectra_compare_before_and_after() {
    let instance = varimu(&[(THRESHOLD, -24.0), (TIME_CONSTANT, 1.0)]);
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = create_varimu(&instance, Some(telemetry.clone())).unwrap();
    let mut frames: Vec<Frame> = sine(1000.0, 0.5, 48000)
        .into_iter()
        .map(|sample| [sample as f32, sample as f32 * 0.5])
        .collect();
    processor.process(&mut frames, CONTEXT);
    let columns = telemetry.scope.snapshot();
    assert_eq!(
        columns.len(),
        48000 / crate::builtin::SCOPE_FRAMES_PER_COLUMN
    );
    assert_eq!(telemetry.scope.written() as usize, columns.len());
    let newest = *columns.last().unwrap();
    assert!((newest.input_max - 0.5).abs() < 0.01 && (newest.input_min + 0.5).abs() < 0.01);
    let expected = static_output_db(decibels(0.5), -24.0, 1.0);
    assert!((decibels(f64::from(newest.output_max)) - expected).abs() < 0.1);
    assert!((f64::from(newest.reduction_db) - (decibels(0.5) - expected)).abs() < 0.1);
    let power = (0.25 + 0.0625) / 2.0 / 2.0;
    assert!((f64::from(newest.input_power) / power - 1.0).abs() < 0.05);
    assert!(newest.output_power < newest.input_power * 0.5);
    let read = |bands: &[std::sync::atomic::AtomicU32; 128], hz: f64| {
        let band = ((hz / 20.0).ln() / 1000.0_f64.ln() * 127.0).round() as usize;
        f32::from_bits(bands[band].load(Ordering::Relaxed))
    };
    let (before, after) = (
        read(&telemetry.input_spectrum, 1000.0),
        read(&telemetry.spectrum, 1000.0),
    );
    assert!(
        before > -20.0 && after < before - 6.0,
        "{before} vs {after}"
    );
    assert!(read(&telemetry.input_spectrum, 5000.0) < before - 60.0);
    // The engine resets devices at every loop wrap. The analysis must carry straight on:
    // a fresh FFT window would leave the spectrum blank for 170 ms after each wrap.
    let (written, updates) = (
        telemetry.scope.written(),
        telemetry.spectrum_updates.load(Ordering::Relaxed),
    );
    processor.reset();
    assert_eq!(read(&telemetry.spectrum, 1000.0), after);
    let mut wrap: Vec<Frame> = sine(1000.0, 0.5, 2400)
        .into_iter()
        .map(|sample| [sample as f32; 2])
        .collect();
    processor.process(&mut wrap, CONTEXT);
    assert!(telemetry.spectrum_updates.load(Ordering::Relaxed) > updates);
    assert!(read(&telemetry.input_spectrum, 1000.0) > -20.0);
    assert_eq!(telemetry.scope.written(), written + 25);
}
