//! Calor measured as a host runs it: harmonics, latency, transparency, aliasing, glides.

use std::f64::consts::TAU;

use crate::params::{
    AUTO_GAIN, BIAS, DRIVE, EMPHASIS, EMPHASIS_FREQUENCY, EVEN, HARDNESS, MIX, ODD, OUTPUT,
    PARAMETERS, TONE, defaults,
};
use crate::{Calor, Curve, Quality, REFERENCE_LEVEL, Telemetry, Transfer};

const RATE: f64 = 48_000.0;
/// The measuring window: 0.1 s, a whole number of cycles of any multiple of 10 Hz.
const WINDOW: usize = 4800;

fn calor(settings: &[(u32, f64)]) -> Calor {
    let mut calor = Calor::new(RATE, None);
    for &(id, value) in settings {
        calor.set_parameter(id, value);
    }
    calor.reset();
    calor
}

fn values(settings: &[(u32, f64)]) -> [f64; crate::COUNT] {
    let mut values = defaults();
    for &(id, value) in settings {
        values[id as usize] = value;
    }
    values
}

/// Runs a mono signal through both channels in blocks of 128, and returns the left.
fn run(calor: &mut Calor, input: &[f32]) -> Vec<f32> {
    let mut frames: Vec<[f32; 2]> = input.iter().map(|&sample| [sample, sample]).collect();
    for block in frames.chunks_mut(128) {
        calor.process(block);
    }
    frames.iter().map(|frame| frame[0]).collect()
}

fn sine(hz: f64, amplitude: f64, length: usize) -> Vec<f32> {
    (0..length)
        .map(|i| (amplitude * (TAU * hz * i as f64 / RATE).sin()) as f32)
        .collect()
}

/// Amplitudes of the first `count` harmonics of `hz` over the last [`WINDOW`] samples of
/// `signal`, which hold a whole number of cycles of each.
fn harmonics(signal: &[f32], hz: f64, count: usize) -> Vec<f64> {
    let tail = &signal[signal.len() - WINDOW..];
    (1..=count)
        .map(|n| {
            let w = TAU * hz * n as f64 / RATE;
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &sample) in tail.iter().enumerate() {
                let (sin, cos) = (w * i as f64).sin_cos();
                re += f64::from(sample) * cos;
                im += f64::from(sample) * sin;
            }
            2.0 * (re * re + im * im).sqrt() / WINDOW as f64
        })
        .collect()
}

fn db(level: f64) -> f64 {
    20.0 * level.max(1.0e-15).log10()
}

#[test]
fn every_parameter_is_indexed_by_id_and_round_trips() {
    for (id, spec) in PARAMETERS.iter().enumerate() {
        assert_eq!(spec.id as usize, id);
        assert!(spec.min < spec.max, "{}", spec.name);
        assert!(
            (spec.min..=spec.max).contains(&spec.default),
            "{}",
            spec.name
        );
        if spec.steps == 0 {
            for position in [0.0, 0.25, 0.5, 1.0] {
                let back = spec.normalize(spec.denormalize(position));
                assert!((back - position).abs() < 1.0e-9, "{}", spec.name);
            }
        }
    }
    let mut calor = Calor::new(RATE, None);
    calor.set_parameter(DRIVE, 99.0);
    calor.set_parameter(EVEN, f64::NAN);
    calor.set_parameter(AUTO_GAIN, 0.3);
    calor.set_parameter(500, 1.0);
    assert_eq!(calor.value(DRIVE), 36.0);
    assert_eq!(calor.value(EVEN), 100.0);
    assert_eq!(calor.value(AUTO_GAIN), 0.0);
}

#[test]
fn the_curve_splits_exactly_into_odd_and_even_parts_with_their_antiderivatives() {
    for curve in [
        Curve::new(0.0, 0.0, 0.0),
        Curve::new(12.0, 30.0, 30.0),
        Curve::new(30.0, 100.0, 100.0),
    ] {
        assert_eq!(curve.shape(0.0), 0.0);
        for i in -40..=40 {
            let x = f64::from(i) / 20.0;
            let (odd, even) = curve.parts(x);
            let (odd_mirror, even_mirror) = curve.parts(-x);
            assert!((odd + odd_mirror).abs() < 1.0e-12);
            assert!((even - even_mirror).abs() < 1.0e-12);
            assert!((odd + even - curve.shape(x)).abs() < 1.0e-12);
            let h = 1.0e-6;
            let (below, above) = (curve.integrals(x - h), curve.integrals(x + h));
            let slope = |a: f64, b: f64| (b - a) / (2.0 * h);
            assert!((slope(below.0, above.0) - odd).abs() < 1.0e-5, "{x}");
            assert!((slope(below.1, above.1) - even).abs() < 1.0e-5, "{x}");
        }
    }
}

#[test]
fn odd_and_even_scale_their_own_harmonics_and_nothing_else() {
    let base = values(&[(DRIVE, 15.0), (BIAS, 60.0)]);
    let both = Transfer::from_values(&base).harmonics(REFERENCE_LEVEL);
    // Auto gain keeps the fundamental where it went in.
    assert!((db(both[0]) - db(REFERENCE_LEVEL)).abs() < 0.01);
    let mut half_odd = base;
    half_odd[ODD as usize] = 50.0;
    let half_odd = Transfer::from_values(&half_odd).harmonics(REFERENCE_LEVEL);
    let mut double_even = base;
    double_even[EVEN as usize] = 200.0;
    let double_even = Transfer::from_values(&double_even).harmonics(REFERENCE_LEVEL);
    for n in 1..crate::HARMONICS {
        let harmonic = n + 1;
        let (odd_factor, even_factor) = if harmonic % 2 == 1 {
            (0.5, 1.0)
        } else {
            (1.0, 2.0)
        };
        assert!(
            (half_odd[n] - odd_factor * both[n]).abs() < 1.0e-9,
            "H{harmonic}"
        );
        assert!(
            (double_even[n] - even_factor * both[n]).abs() < 1.0e-9,
            "H{harmonic}"
        );
    }
    assert!((half_odd[0] - both[0]).abs() < 1.0e-9);
    assert!(
        db(both[1] / both[0]) > -30.0,
        "the lean makes a second harmonic"
    );
    assert!(db(both[2] / both[0]) > -40.0, "the bend makes a third");
}

#[test]
fn auto_gain_matches_the_level_at_minus_12_dbfs_and_off_gives_the_curve_its_own() {
    for (drive, hardness, bias) in [(0.0, 30.0, 30.0), (18.0, 0.0, 80.0), (36.0, 100.0, 0.0)] {
        let settings = [(DRIVE, drive), (HARDNESS, hardness), (BIAS, bias)];
        let on = Transfer::from_values(&values(&settings));
        assert!((db(on.harmonics(REFERENCE_LEVEL)[0]) - db(REFERENCE_LEVEL)).abs() < 0.05);
        let mut off = values(&settings);
        off[AUTO_GAIN as usize] = 0.0;
        let off = Transfer::from_values(&off);
        let gain = off.harmonics(REFERENCE_LEVEL)[0] / REFERENCE_LEVEL;
        assert!((gain / on.reference - 1.0).abs() < 1.0e-3);
    }
}

#[test]
fn even_at_zero_leaves_only_odd_harmonics_and_odd_at_zero_only_even_ones() {
    let input = sine(1000.0, 0.5, 24_000);
    for quality in [Quality::Realtime, Quality::Offline] {
        let mut symmetric = calor(&[(DRIVE, 18.0), (BIAS, 70.0), (EVEN, 0.0)]);
        symmetric.set_quality(quality);
        let levels = harmonics(&run(&mut symmetric, &input), 1000.0, 12);
        for (index, &level) in levels.iter().enumerate().skip(1) {
            let relative = db(level / levels[0]);
            if index % 2 == 1 {
                assert!(
                    relative < -100.0,
                    "{quality:?} even only: H{} {relative}",
                    index + 1
                );
            }
        }
        assert!(db(levels[2] / levels[0]) > -30.0);

        let mut lean = calor(&[(DRIVE, 18.0), (BIAS, 70.0), (ODD, 0.0), (EVEN, 200.0)]);
        lean.set_quality(quality);
        let levels = harmonics(&run(&mut lean, &input), 1000.0, 12);
        for (index, &level) in levels.iter().enumerate().skip(1) {
            let relative = db(level / levels[0]);
            if index % 2 == 0 {
                assert!(
                    relative < -100.0,
                    "{quality:?} odd only: H{} {relative}",
                    index + 1
                );
            }
        }
        assert!(db(levels[1] / levels[0]) > -25.0);
    }
}

#[test]
fn the_processor_makes_the_harmonics_its_transfer_predicts() {
    let settings = [
        (DRIVE, 12.0),
        (HARDNESS, 50.0),
        (BIAS, 50.0),
        (ODD, 80.0),
        (EVEN, 150.0),
    ];
    let predicted = Transfer::from_values(&values(&settings)).harmonics(0.4);
    for quality in [Quality::Realtime, Quality::Offline] {
        let mut calor = calor(&settings);
        calor.set_quality(quality);
        let measured = harmonics(&run(&mut calor, &sine(1000.0, 0.4, 24_000)), 1000.0, 8);
        for n in 0..8 {
            if db(predicted[n] / predicted[0]) > -80.0 {
                let error = db(measured[n]) - db(predicted[n]);
                assert!(error.abs() < 0.1, "{quality:?} H{}: {error} dB", n + 1);
            }
        }
    }
}

#[test]
fn the_latency_is_whole_and_the_same_at_either_quality() {
    let mut calor = calor(&[(MIX, 0.0), (DRIVE, 30.0)]);
    assert_eq!(calor.latency(), 39);
    assert_eq!(calor.oversampling(Quality::Realtime), 4);
    assert_eq!(calor.oversampling(Quality::Offline), 16);
    let input: Vec<f32> = (0..2000)
        .map(|i| ((i * 37 % 101) as f32 / 101.0) - 0.5)
        .collect();
    for quality in [Quality::Realtime, Quality::Offline] {
        calor.set_quality(quality);
        calor.reset();
        let output = run(&mut calor, &input);
        // Mix at 0 plays the dry signal, delayed.
        assert!(output[..39].iter().all(|&sample| sample == 0.0));
        assert_eq!(&output[39..], &input[..input.len() - 39]);
        assert_eq!(calor.latency(), 39);
    }
    for (rate, realtime, offline) in [(44_100.0, 4, 16), (96_000.0, 2, 8), (192_000.0, 1, 4)] {
        let calor = Calor::new(rate, None);
        assert_eq!(calor.oversampling(Quality::Realtime), realtime);
        assert_eq!(calor.oversampling(Quality::Offline), offline);
        // The sharpest filter, at 44.1 kHz, takes 60 samples; higher rates need less.
        assert!(calor.latency() > 0 && f64::from(calor.latency()) / rate < 1.5e-3);
    }
}

/// A quiet signal well inside the passband, readable at any fractional time.
fn quiet(time: f64) -> f64 {
    0.05 * (TAU * 300.0 * time).sin()
        + 0.04 * (TAU * 4_100.0 * time).sin()
        + 0.03 * (TAU * 15_700.0 * time).sin()
}

#[test]
fn where_the_curve_is_straight_the_wet_signal_is_the_dry_one_in_line() {
    // At Hardness 100 % the curve is straight up to 0.97, and a quiet signal stays there,
    // emphasis and all.
    let input: Vec<f32> = (0..6000).map(|i| quiet(i as f64 / RATE) as f32).collect();
    for quality in [Quality::Realtime, Quality::Offline] {
        for settings in [
            vec![(MIX, 100.0)],
            vec![(MIX, 50.0)],
            vec![(EMPHASIS, 12.0), (EMPHASIS_FREQUENCY, 3000.0)],
            vec![(EMPHASIS, -12.0), (EMPHASIS_FREQUENCY, 500.0), (MIX, 70.0)],
        ] {
            let mut all = vec![(DRIVE, 0.0), (HARDNESS, 100.0), (BIAS, 0.0)];
            all.extend(settings.iter().copied());
            let mut calor = calor(&all);
            calor.set_quality(quality);
            let output = run(&mut calor, &input);
            for (index, &sample) in output.iter().enumerate().skip(1000) {
                let expected = quiet((index as f64 - 39.0) / RATE);
                assert!(
                    (f64::from(sample) - expected).abs() < 5.0e-4,
                    "{quality:?} {settings:?} at {index}: {sample} vs {expected}"
                );
            }
        }
    }
}

/// The share of a signal's power, over the last [`WINDOW`] samples, that is not at a
/// harmonic of `hz` below Nyquist, in dB relative to the fundamental.
fn inharmonic_db(signal: &[f32], hz: f64) -> f64 {
    let tail = &signal[signal.len() - WINDOW..];
    let total: f64 = tail.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / WINDOW as f64;
    let count = (RATE / 2.0 / hz) as usize;
    let levels = harmonics(signal, hz, count);
    let harmonic: f64 = levels.iter().map(|level| level * level / 2.0).sum();
    10.0 * ((total - harmonic).max(1.0e-30) / (levels[0] * levels[0] / 2.0)).log10()
}

#[test]
fn hard_clipping_folds_little_back_into_the_band() {
    // 7.01 kHz, a whole number of cycles in the window, clipped hard: its harmonics run
    // far past Nyquist.
    let settings = [(DRIVE, 24.0), (HARDNESS, 100.0), (EVEN, 0.0)];
    let input = sine(7010.0, 0.5, 24_000);
    let mut levels = Vec::new();
    for quality in [Quality::Realtime, Quality::Offline] {
        let mut calor = calor(&settings);
        calor.set_quality(quality);
        levels.push(inharmonic_db(&run(&mut calor, &input), 7010.0));
    }
    // Measured: about −75 dB in real time and −92 dB offline, where the last half-band's
    // 80 dB stopband sets the floor.
    assert!(levels[0] < -70.0, "real time: {} dB", levels[0]);
    assert!(levels[1] < -85.0, "offline: {} dB", levels[1]);
    assert!(levels[1] < levels[0] - 10.0);
}

#[test]
fn jumps_in_the_settings_glide_instead_of_clicking() {
    // A slow sine, so any step in the output is the processor's doing.
    let input = sine(100.0, 0.25, 24_000);
    let largest_step = |output: &[f32]| {
        output
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max)
    };
    for (id, from, to) in [(OUTPUT, 0.0, 24.0), (MIX, 0.0, 100.0), (DRIVE, 0.0, 36.0)] {
        let mut calor = calor(&[(id, from), (HARDNESS, 100.0)]);
        let mut output = run(&mut calor, &input[..12_000]);
        calor.set_parameter(id, to);
        output.extend(run(&mut calor, &input[12_000..]));
        // The slope of the sine at its loudest after the change, with room for the ramp.
        let settled = largest_step(&output[20_000..]);
        let around = largest_step(&output[11_000..14_000]);
        assert!(
            around < 1.5 * settled.max(0.004),
            "{id}: {around} vs {settled}"
        );
    }
}

#[test]
fn a_leaning_curve_leaves_no_offset() {
    let mut calor = calor(&[(DRIVE, 24.0), (BIAS, 100.0), (EVEN, 300.0), (ODD, 0.0)]);
    // 200 Hz: 240 samples a cycle, so the last half second holds whole cycles.
    let output = run(&mut calor, &sine(200.0, 0.8, 96_000));
    let tail = &output[72_000..];
    let mean = tail.iter().map(|&s| f64::from(s)).sum::<f64>() / tail.len() as f64;
    let peak = tail.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(
        mean.abs() < 1.0e-4 * f64::from(peak),
        "{mean} against a peak of {peak}"
    );
}

#[test]
fn silence_stays_silent_and_bad_input_is_ignored() {
    let mut calor = calor(&[(DRIVE, 36.0), (BIAS, 100.0), (EVEN, 300.0), (TONE, 6.0)]);
    let output = run(&mut calor, &vec![0.0; 4800]);
    assert!(output.iter().all(|&sample| sample == 0.0));
    let mut frames = vec![[f32::NAN, f32::INFINITY]; 64];
    calor.process(&mut frames);
    let output = run(&mut calor, &sine(440.0, 0.5, 4800));
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|&sample| sample != 0.0));
}

#[test]
fn reset_clears_the_signal_path_and_the_meters_follow_the_levels() {
    let telemetry = std::sync::Arc::new(Telemetry::default());
    let mut calor = Calor::new(RATE, Some(telemetry.clone()));
    calor.set_parameter(DRIVE, 20.0);
    run(&mut calor, &sine(440.0, 0.5, 4800));
    let input_db = crate::telemetry::get(&telemetry.input_db);
    assert!((input_db - -6.02).abs() < 0.1, "{input_db}");
    assert!(crate::telemetry::get(&telemetry.output_db) > -30.0);
    assert!(telemetry.updates.load(std::sync::atomic::Ordering::Relaxed) > 0);
    calor.reset();
    assert!(crate::telemetry::get(&telemetry.input_db) < -100.0);
    let output = run(&mut calor, &vec![0.0; 256]);
    assert!(output.iter().all(|&sample| sample == 0.0));
}

// ---------------------------------------------------------------------------------------
// Bands, models, dynamics, feedback and modulation
// ---------------------------------------------------------------------------------------

use crate::params::{
    BANDS, CROSSOVER_BASE, CROSSOVER_SLOPE, LFO_RATE, LFO_SHAPE, MOD_AMOUNT, MOD_SOURCE,
    MOD_TARGET, Slot, band_parameter, mod_parameter,
};

/// A band's control, by band (from 0) and slot.
fn band(index: usize, slot: Slot, value: f64) -> (u32, f64) {
    (band_parameter(index, slot), value)
}

/// The curve of band `index` kept straight: no drive, the knee at the top, no lean.
fn straight(index: usize) -> [(u32, f64); 3] {
    [
        band(index, Slot::Drive, 0.0),
        band(index, Slot::Hardness, 100.0),
        band(index, Slot::Bias, 0.0),
    ]
}

/// Runs a stereo signal in blocks of 128 and returns both channels.
fn run_stereo(calor: &mut Calor, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut frames: Vec<[f32; 2]> = left.iter().zip(right).map(|(&l, &r)| [l, r]).collect();
    for block in frames.chunks_mut(128) {
        calor.process(block);
    }
    frames.iter().map(|frame| (frame[0], frame[1])).unzip()
}

fn level(signal: &[f32], hz: f64) -> f64 {
    harmonics(signal, hz, 1)[0]
}

fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

#[test]
fn bands_add_back_up_flat_where_the_curves_are_straight() {
    for (bands, slope) in [(2.0, 0.0), (4.0, 1.0), (6.0, 2.0)] {
        let mut settings = vec![(BANDS, bands), (CROSSOVER_SLOPE, slope)];
        for index in 0..6 {
            settings.extend(straight(index));
        }
        for hz in [60.0, 400.0, 1500.0, 6000.0, 15_000.0] {
            let output = run(&mut calor(&settings), &sine(hz, 0.05, 24_000));
            let error = db(level(&output, hz)) - db(0.05);
            assert!(error.abs() < 0.1, "{bands} bands, {hz} Hz: {error} dB");
        }
        // The dry signal is split alongside, so Mix blends two signals that line up.
        let noise: Vec<f32> = (0..24_000)
            .map(|i| (0.04 * ((i * 7919 % 1013) as f64 / 506.5 - 1.0)) as f32)
            .collect();
        let wet = run(&mut calor(&settings), &noise);
        let mut dry_settings = settings.clone();
        dry_settings.push((MIX, 0.0));
        let dry = run(&mut calor(&dry_settings), &noise);
        let largest = wet[2000..]
            .iter()
            .zip(&dry[2000..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(largest < 2.0e-3, "{bands} bands: {largest}");
    }
    assert_eq!(calor(&[(BANDS, 6.0)]).latency(), 39);
}

#[test]
fn separate_bands_do_not_intermodulate() {
    // A bass note and a high tone, each driven hard; in one band the bass shakes the high
    // tone's clipping into sidebands, split apart it cannot.
    let input = add(&sine(100.0, 0.4, 24_000), &sine(3000.0, 0.4, 24_000));
    let heavy = |index: usize| {
        [
            band(index, Slot::Drive, 30.0),
            band(index, Slot::Hardness, 100.0),
            band(index, Slot::Bias, 0.0),
            band(index, Slot::Even, 0.0),
        ]
    };
    let mut one = calor(&heavy(0));
    let together = run(&mut one, &input);
    let mut settings = vec![
        (BANDS, 2.0),
        (CROSSOVER_BASE, 600.0),
        (CROSSOVER_SLOPE, 2.0),
    ];
    settings.extend(heavy(0));
    settings.extend(heavy(1));
    let apart = run(&mut calor(&settings), &input);
    // The curves are symmetric (no even harmonics), so the bass shakes the high tone's
    // clipping twice a cycle: sidebands two bass cycles either side.
    let sidebands = |signal: &[f32]| {
        let side = level(signal, 2800.0).max(level(signal, 3200.0));
        db(side / level(signal, 3000.0))
    };
    assert!(sidebands(&together) > -40.0, "{}", sidebands(&together));
    assert!(
        sidebands(&apart) < sidebands(&together) - 25.0,
        "{}",
        sidebands(&apart)
    );
    // Each band still saturates on its own.
    assert!(db(level(&apart, 9000.0) / level(&apart, 3000.0)) > -30.0);
}

#[test]
fn solo_and_mute_choose_the_bands_heard() {
    let input = add(&sine(60.0, 0.25, 24_000), &sine(2000.0, 0.25, 24_000));
    let base = [
        (BANDS, 3.0),
        (CROSSOVER_BASE, 150.0),
        (CROSSOVER_BASE + 1, 800.0),
        (CROSSOVER_SLOPE, 2.0),
    ];
    let render = |extra: &[(u32, f64)]| {
        let mut settings = base.to_vec();
        settings.extend_from_slice(extra);
        run(&mut calor(&settings), &input)
    };
    let all = render(&[]);
    let soloed = render(&[band(0, Slot::Solo, 1.0)]);
    let muted = render(&[band(0, Slot::Mute, 1.0)]);
    let relative = |signal: &[f32], hz: f64| db(level(signal, hz) / level(&all, hz));
    assert!(relative(&soloed, 60.0).abs() < 0.5);
    assert!(
        relative(&soloed, 2000.0) < -50.0,
        "{}",
        relative(&soloed, 2000.0)
    );
    assert!(relative(&muted, 60.0) < -50.0, "{}", relative(&muted, 60.0));
    assert!(relative(&muted, 2000.0).abs() < 0.5);
}

#[test]
fn mid_and_side_bands_work_on_one_part_and_leave_the_other_clean() {
    let mid = sine(200.0, 0.4, 24_000);
    let side = sine(1000.0, 0.2, 24_000);
    let left = add(&mid, &side);
    let right: Vec<f32> = mid.iter().zip(&side).map(|(m, s)| m - s).collect();
    let heavy = [
        band(0, Slot::Drive, 30.0),
        band(0, Slot::Hardness, 100.0),
        band(0, Slot::Bias, 0.0),
        band(0, Slot::Even, 0.0),
    ];
    for (mode, worked, passed, worked_hz, passed_hz) in
        [(1.0, 0, 1, 200.0, 1000.0), (2.0, 1, 0, 1000.0, 200.0)]
    {
        let mut settings = heavy.to_vec();
        settings.push(band(0, Slot::Channels, mode));
        let (l, r) = run_stereo(&mut calor(&settings), &left, &right);
        let parts: [Vec<f32>; 2] = [
            l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect(),
            l.iter().zip(&r).map(|(a, b)| 0.5 * (a - b)).collect(),
        ];
        let worked = &parts[worked];
        let passed = &parts[passed];
        assert!(db(level(worked, 3.0 * worked_hz) / level(worked, worked_hz)) > -40.0);
        assert!(db(level(passed, 3.0 * passed_hz) / level(passed, passed_hz)) < -100.0);
        let expected = if mode == 1.0 { 0.2 } else { 0.4 };
        assert!((db(level(passed, passed_hz)) - db(expected)).abs() < 0.01);
    }
}

#[test]
fn dynamics_compress_loud_bands_and_expand_quiet_ones() {
    let render = |dynamics: f64, amplitude: f64| {
        let mut settings = straight(0).to_vec();
        settings.push(band(0, Slot::Dynamics, dynamics));
        let output = run(&mut calor(&settings), &sine(1000.0, amplitude, 24_000));
        db(level(&output, 1000.0) / amplitude)
    };
    // 18 dB over the threshold at 8:1 comes down by about 15.75 dB.
    let compressed = render(100.0, 0.5);
    assert!((compressed + 15.75).abs() < 1.0, "{compressed}");
    assert!(render(100.0, 0.01).abs() < 0.5);
    assert!(render(-100.0, 0.0016) < -40.0, "{}", render(-100.0, 0.0016));
    assert!(render(-100.0, 0.5).abs() < 0.5);
}

#[test]
fn feedback_rings_at_its_frequency_and_stops_with_the_input() {
    let mut noise_state = 0x1234_5678_u32;
    let mut input: Vec<f32> = (0..24_000)
        .map(|_| {
            noise_state ^= noise_state << 13;
            noise_state ^= noise_state >> 17;
            noise_state ^= noise_state << 5;
            (0.1 * (f64::from(noise_state) / f64::from(u32::MAX) - 0.5)) as f32
        })
        .collect();
    input.extend(std::iter::repeat_n(0.0, 48_000));
    let render = |feedback: f64| {
        let settings = [
            band(0, Slot::Feedback, feedback),
            band(0, Slot::FeedbackFrequency, 1000.0),
        ];
        run(&mut calor(&settings), &input)
    };
    let dry = render(0.0);
    let rung = render(90.0);
    // Over the noise, the feedback frequency stands out.
    let near = |signal: &[f32]| level(&signal[..24_000], 1000.0);
    assert!(
        db(near(&rung) / near(&dry)) > 6.0,
        "{}",
        db(near(&rung) / near(&dry))
    );
    // Once the input stops, the ringing dies away.
    let tail = rung[60_000..]
        .iter()
        .fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(tail < 1.0e-4, "{tail}");
}

#[test]
fn every_model_is_clean_when_quiet_and_colours_when_driven() {
    let mut spectra = Vec::new();
    for model in 0..5 {
        // Quiet and undriven, it passes the tone at its level. (With Auto gain on, Tape's
        // makeup for its compression at −12 dBFS would lift a quiet tone, as a driven
        // curve's does.)
        let mut settings = straight(0).to_vec();
        settings.push(band(0, Slot::Model, f64::from(model)));
        settings.push((AUTO_GAIN, 0.0));
        let output = run(&mut calor(&settings), &sine(1000.0, 0.001, 24_000));
        let error = db(level(&output, 1000.0)) - db(0.001);
        assert!(error.abs() < 0.2, "model {model}: {error} dB");
        let quiet = harmonics(&output, 1000.0, 5);
        assert!(db(quiet[2] / quiet[0]) < -60.0, "model {model}");
        // Driven, it adds harmonics of its own.
        let settings = [
            band(0, Slot::Model, f64::from(model)),
            band(0, Slot::Drive, 18.0),
        ];
        let output = run(&mut calor(&settings), &sine(1000.0, 0.5, 24_000));
        assert!(output.iter().all(|sample| sample.is_finite()));
        let loud = harmonics(&output, 1000.0, 8);
        let distortion: f64 = loud[1..].iter().map(|h| h * h).sum::<f64>().sqrt() / loud[0];
        assert!(distortion > 0.01, "model {model}: {distortion}");
        spectra.push(loud.iter().map(|h| db(h / loud[0])).collect::<Vec<_>>());
    }
    for a in 0..spectra.len() {
        for b in a + 1..spectra.len() {
            let apart = spectra[a]
                .iter()
                .zip(&spectra[b])
                .map(|(x, y)| (x - y).abs())
                .fold(0.0, f64::max);
            assert!(apart > 1.0, "models {a} and {b} sound alike");
        }
    }
}

#[test]
fn the_transformer_saturates_the_lows_first() {
    let settings = [
        band(0, Slot::Model, 3.0),
        band(0, Slot::Drive, 12.0),
        band(0, Slot::Bias, 0.0),
        band(0, Slot::Even, 0.0),
    ];
    let distortion = |hz: f64| {
        let output = run(&mut calor(&settings), &sine(hz, 0.5, 24_000));
        let levels = harmonics(&output, hz, 6);
        levels[1..].iter().map(|h| h * h).sum::<f64>().sqrt() / levels[0]
    };
    let (low, high) = (distortion(50.0), distortion(2000.0));
    assert!(low > 4.0 * high, "{low} vs {high}");
}

#[test]
fn tube_blooms_into_more_even_harmonics_when_loud() {
    let second = |model: f64| {
        let settings = [band(0, Slot::Model, model), band(0, Slot::Drive, 12.0)];
        let output = run(&mut calor(&settings), &sine(1000.0, 0.5, 24_000));
        let levels = harmonics(&output, 1000.0, 2);
        db(levels[1] / levels[0])
    };
    let (tube, curve) = (second(1.0), second(0.0));
    assert!(tube > curve + 3.0, "tube {tube} vs curve {curve}");
}

#[test]
fn modulation_moves_its_target_and_rests_when_off() {
    let swing = |amount: f64| {
        let settings = [
            (mod_parameter(0, MOD_SOURCE), 1.0),
            (mod_parameter(0, MOD_TARGET), 12.0),
            (mod_parameter(0, MOD_AMOUNT), amount),
            (LFO_RATE, 4.0),
            (LFO_SHAPE, 0.0),
        ];
        let output = run(&mut calor(&settings), &sine(1000.0, 0.1, 48_000));
        let peaks: Vec<f32> = output[4800..]
            .chunks(480)
            .map(|chunk| chunk.iter().fold(0.0_f32, |peak, s| peak.max(s.abs())))
            .collect();
        let (low, high) = peaks
            .iter()
            .fold((f32::INFINITY, 0.0_f32), |(low, high), &p| {
                (low.min(p), high.max(p))
            });
        20.0 * (high / low).log10()
    };
    // Half the Output range (48 dB) swung by the LFO: about ±12 dB.
    let moved = swing(50.0);
    assert!(moved > 18.0, "{moved}");
    assert!(swing(0.0) < 0.1);
}

#[test]
fn every_style_plays_at_a_sensible_level() {
    for style in crate::STYLES {
        let settings: Vec<(u32, f64)> = style
            .settings
            .iter()
            .map(|&(slot, value)| band(0, slot, value))
            .collect();
        let output = run(&mut calor(&settings), &sine(440.0, 0.25, 24_000));
        assert!(
            output.iter().all(|sample| sample.is_finite()),
            "{}",
            style.name
        );
        let peak = output[4800..]
            .iter()
            .fold(0.0_f32, |peak, s| peak.max(s.abs()));
        let gain = 20.0 * (peak / 0.25).log10();
        assert!(gain.abs() < 12.0, "{}: {gain} dB", style.name);
    }
}

#[test]
fn auto_gain_levels_tape_at_the_reference_too() {
    for model in [0.0, 2.0] {
        let output = run(
            &mut calor(&[band(0, Slot::Model, model)]),
            &sine(1000.0, REFERENCE_LEVEL, 24_000),
        );
        let error = db(level(&output, 1000.0)) - db(REFERENCE_LEVEL);
        assert!(error.abs() < 0.2, "model {model}: {error} dB");
    }
}
