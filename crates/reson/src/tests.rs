use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::*;
use crate::params::defaults;

type Frame = [f32; 2];

const RATE: f64 = 48_000.0;
/// The frame, hop and latency at 48 kHz.
const FRAME: usize = 2048;
const HOP: usize = 128;
const BINS: usize = FRAME / 2 + 1;
const LAG: usize = 256;

/// Reson at 48 kHz with some parameters set, in physical units.
fn processor(settings: &[(u32, f64)]) -> Reson {
    watched(settings, None)
}

fn watched(settings: &[(u32, f64)], telemetry: Option<Arc<Telemetry>>) -> Reson {
    let mut reson = Reson::new(RATE, telemetry);
    for &(id, value) in settings {
        reson.set_parameter(id, value);
    }
    reson.reset();
    reson
}

/// The parameter values a set of settings makes, for the sensitivity curve.
fn values(settings: &[(u32, f64)]) -> [f64; COUNT] {
    let mut values = defaults();
    for &(id, value) in settings {
        values[id as usize] = value;
    }
    values
}

/// Runs a signal through in uneven blocks, as a host would.
fn run(processor: &mut Reson, input: &[Frame]) -> Vec<Frame> {
    let mut output = input.to_vec();
    let mut start = 0;
    let mut block = 1;
    while start < output.len() {
        let end = (start + block).min(output.len());
        processor.process(&mut output[start..end]);
        start = end;
        block = block * 7 % 509 + 1;
    }
    output
}

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(self.0 >> 8) / f64::from(1_u32 << 24) * 2.0 - 1.0
    }
}

fn tone(hz: f64, level: f64, i: usize) -> f64 {
    level * (TAU * hz * i as f64 / RATE).sin()
}

/// Stereo signal from a per-sample generator.
fn signal(count: usize, mut sample: impl FnMut(usize) -> [f64; 2]) -> Vec<Frame> {
    (0..count)
        .map(|i| sample(i).map(|value| value as f32))
        .collect()
}

/// Amplitude of the `hz` component of one channel over `range`.
fn magnitude(frames: &[Frame], channel: usize, hz: f64, range: std::ops::Range<usize>) -> f64 {
    let length = range.len() as f64;
    let (re, im) = frames[range]
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (i, frame)| {
            let phase = TAU * hz * i as f64 / RATE;
            let window = 0.5 - 0.5 * (TAU * i as f64 / length).cos();
            let sample = f64::from(frame[channel]) * window;
            (re + sample * phase.cos(), im - sample * phase.sin())
        });
    4.0 * (re * re + im * im).sqrt() / length
}

/// Power of one channel between two frequencies over `range`, from a plain DFT.
fn band_power(
    frames: &[Frame],
    channel: usize,
    (low, high): (f64, f64),
    range: std::ops::Range<usize>,
) -> f64 {
    let length = range.len();
    let first = (low * length as f64 / RATE).ceil() as usize;
    let last = (high * length as f64 / RATE).floor() as usize;
    let samples: Vec<f64> = frames[range]
        .iter()
        .map(|f| f64::from(f[channel]))
        .collect();
    (first..=last)
        .map(|k| {
            let (re, im) = samples
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(re, im), (n, x)| {
                    let phase = TAU * (k * n % length) as f64 / length as f64;
                    (re + x * phase.cos(), im - x * phase.sin())
                });
            re * re + im * im
        })
        .sum()
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// The cut a steady tone gets, comparing the output with the input it came from.
fn tone_cut(settings: &[(u32, f64)], hz: f64) -> f64 {
    let mut noise = Noise(9);
    let input = signal(48_000, |i| {
        let x = tone(hz, 0.25, i) + 0.001 * noise.next();
        [x, x]
    });
    let output = run(&mut processor(settings), &input);
    let range = 24_000..48_000 - LAG;
    let delayed = (24_000 + LAG)..48_000;
    db(magnitude(&input, 0, hz, range) / magnitude(&output, 0, hz, delayed))
}

#[test]
fn parameter_ids_index_the_spec_table() {
    for (index, spec) in PARAMETERS.iter().enumerate() {
        assert_eq!(spec.id as usize, index, "{}", spec.name);
        assert!(spec.min < spec.max && (spec.min..=spec.max).contains(&spec.default));
    }
    assert_eq!(choices(KNEE), None);
    assert_eq!(choices(STEREO), Some(&STEREO_MODES[..]));
    assert_eq!(PARAMETERS.len(), CLEAN as usize + 1);
    // Positions round-trip through physical values, and steps snap.
    for spec in PARAMETERS {
        for position in [0.0, 0.25, 0.5, 1.0] {
            let physical = spec.denormalize(position);
            assert!((spec.denormalize(spec.normalize(physical)) - physical).abs() < 1.0e-9);
        }
    }
    assert_eq!(PARAMETERS[STEREO as usize].denormalize(0.6), 1.0);
}

#[test]
fn reports_its_latency() {
    assert_eq!(processor(&[]).latency(), LAG as u32);
    assert_eq!(Reson::new(44_100.0, None).latency(), 256);
    assert_eq!(Reson::new(96_000.0, None).latency(), 512);
}

#[test]
fn with_nothing_to_cut_the_output_is_the_input_delayed() {
    let mut noise = Noise(1);
    let input = signal(20_000, |_| [0.5 * noise.next(), 0.3 * noise.next()]);
    for settings in [&[(DEPTH, 0.0)][..], &[(MIX, 0.0)][..]] {
        let output = run(&mut processor(settings), &input);
        assert!(output[..LAG].iter().all(|frame| *frame == [0.0; 2]));
        for (out, original) in output[LAG..].iter().zip(&input) {
            for channel in 0..2 {
                assert!((out[channel] - original[channel]).abs() < 1.0e-6);
            }
        }
    }
}

#[test]
fn an_impulse_comes_out_at_the_reported_latency() {
    let mut input = vec![[0.0; 2]; 4096];
    input[1000] = [1.0, -0.5];
    let output = run(&mut processor(&[]), &input);
    let peak = output
        .iter()
        .enumerate()
        .max_by(|a, b| a.1[0].abs().total_cmp(&b.1[0].abs()))
        .unwrap()
        .0;
    assert_eq!(peak, 1000 + LAG);
    assert!((output[1000 + LAG][0] - 1.0).abs() < 1.0e-5);
    assert!((output[1000 + LAG][1] + 0.5).abs() < 1.0e-5);
}

#[test]
fn a_ringing_tone_is_turned_down_by_depth() {
    for depth in [3.0, 6.0, 12.0] {
        let cut = tone_cut(&[(DEPTH, depth)], 1000.0);
        assert!(
            cut > depth - 1.0 && cut < depth + 0.5,
            "depth {depth}: cut {cut:.2} dB"
        );
    }
    assert!(tone_cut(&[(DEPTH, 0.0)], 1000.0).abs() < 0.01);
}

#[test]
fn a_resonance_on_noise_is_cut_and_the_rest_survives() {
    let mut noise = Noise(5);
    let input = signal(48_000, |i| {
        let x = tone(3000.0, 0.1, i) + 0.05 * noise.next();
        [x, x]
    });
    let output = run(&mut processor(&[(DEPTH, 10.0)]), &input);
    let (before, after) = (24_000..32_000, (24_000 + LAG)..(32_000 + LAG));
    let cut =
        db(magnitude(&input, 0, 3000.0, before.clone())
            / magnitude(&output, 0, 3000.0, after.clone()));
    assert!(cut > 6.0, "resonance cut {cut:.2} dB");
    // An octave away the noise keeps its level.
    let kept = 10.0
        * (band_power(&output, 0, (700.0, 1400.0), after)
            / band_power(&input, 0, (700.0, 1400.0), before))
        .log10();
    assert!(kept.abs() < 0.5, "noise changed by {kept:.2} dB");
}

#[test]
fn plain_noise_is_left_nearly_alone() {
    let mut noise = Noise(11);
    let input = signal(48_000, |_| {
        let x = 0.2 * noise.next();
        [x, x]
    });
    let output = run(&mut processor(&[]), &input);
    let power = |frames: &[Frame]| frames.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>();
    let change =
        10.0 * (power(&output[24_000 + LAG..]) / power(&input[24_000..48_000 - LAG])).log10();
    assert!(change > -1.0, "noise lost {change:.2} dB");
}

#[test]
fn the_threshold_spares_peaks_that_do_not_stand_out_enough() {
    // A tone about 12 dB above the noise in its bins stands out over a low threshold but
    // not over one above its prominence.
    let mut noise = Noise(3);
    let input = signal(48_000, |i| {
        let x = tone(2000.0, 0.0025, i) + 0.02 * noise.next();
        [x, x]
    });
    let measure = |threshold: f64| {
        let output = run(
            &mut processor(&[(THRESHOLD, threshold), (DEPTH, 12.0)]),
            &input,
        );
        db(magnitude(&input, 0, 2000.0, 24_000..40_000)
            / magnitude(&output, 0, 2000.0, (24_000 + LAG)..(40_000 + LAG)))
    };
    let (loose, strict) = (measure(1.5), measure(15.0));
    assert!(loose > 4.0, "loose {loose:.2}");
    assert!(strict < 1.0, "strict {strict:.2}");
}

#[test]
fn sensitivity_bands_and_cuts_shape_where_reson_works() {
    let plain = tone_cut(&[], 5000.0);
    let guarded = tone_cut(&[(HIGH_CUT, 1500.0)], 5000.0);
    let boosted = tone_cut(
        &[
            (BAND_BASE + 3 * BAND_STRIDE + BAND_FREQUENCY, 5000.0),
            (BAND_BASE + 3 * BAND_STRIDE + BAND_GAIN, 12.0),
        ],
        5000.0,
    );
    assert!(guarded < 0.2, "high cut leaves {guarded:.2} dB");
    assert!(boosted > plain + 6.0, "boost {boosted:.2} vs {plain:.2}");
    let low = tone_cut(&[(LOW_CUT, 1500.0)], 300.0);
    assert!(low < 0.2, "low cut leaves {low:.2} dB");
}

#[test]
fn sensitivity_curve_reads_its_bands_and_cuts() {
    let curve = Sensitivity::from_values(&values(&[
        (LOW_CUT, 100.0),
        (HIGH_CUT, 8000.0),
        (BAND_BASE + BAND_GAIN, 0.0),
        (BAND_BASE + BAND_STRIDE + BAND_FREQUENCY, 1000.0),
        (BAND_BASE + BAND_STRIDE + BAND_GAIN, 6.0),
    ]));
    assert!((curve.bands_db(1000.0) - 6.0).abs() < 1.0e-9);
    assert!((curve.db(100.0) + 6.02).abs() < 0.05, "{}", curve.db(100.0));
    assert!(
        (curve.db(8000.0) + 6.02).abs() < 0.05,
        "{}",
        curve.db(8000.0)
    );
    assert!(curve.db(25.0) < -40.0 && curve.db(19_000.0) < -20.0);
    // The default curve leans out of the low end and is flat above it.
    let defaults = Sensitivity::from_values(&defaults());
    assert!(defaults.db(60.0) < -4.0);
    assert!(defaults.db(3000.0).abs() < 0.05);
}

#[test]
fn attack_and_release_set_how_fast_cuts_come_and_go() {
    let input = signal(48_000, |i| {
        let x = if i < 24_000 {
            tone(1500.0, 0.3, i)
        } else {
            0.0
        };
        [x, x]
    });
    let deepest_after = |settings: &[(u32, f64)], samples: usize| {
        let mut processor = processor(settings);
        let mut frames = input[..samples].to_vec();
        processor.process(&mut frames);
        processor.deepest
    };
    // 30 ms in, a fast attack has nearly reached depth and a slow one has not.
    let fast = deepest_after(&[(ATTACK, 1.0)], 1440);
    let slow = deepest_after(&[(ATTACK, 100.0)], 1440);
    assert!(fast > 5.0 && slow < 3.0, "fast {fast:.2}, slow {slow:.2}");
    // 100 ms after the tone stops a short release is done and a long one is not.
    let short = deepest_after(&[(RELEASE, 10.0)], 28_800);
    let long = deepest_after(&[(RELEASE, 1000.0)], 28_800);
    assert!(
        short < 0.01 && long > 3.0,
        "short {short:.3}, long {long:.2}"
    );
}

#[test]
fn delta_plays_exactly_what_is_removed() {
    let mut noise = Noise(21);
    let input = signal(30_000, |i| {
        let x = tone(800.0, 0.2, i) + tone(4100.0, 0.1, i) + 0.02 * noise.next();
        [x, 0.5 * x]
    });
    let processed = run(&mut processor(&[(DEPTH, 9.0)]), &input);
    let removed = run(&mut processor(&[(DEPTH, 9.0), (DELTA, 1.0)]), &input);
    let mut energy = 0.0;
    for i in LAG..input.len() {
        for channel in 0..2 {
            let sum = processed[i][channel] + removed[i][channel];
            assert!((sum - input[i - LAG][channel]).abs() < 1.0e-4, "{i}");
            energy += f64::from(removed[i][channel]).powi(2);
        }
    }
    assert!(energy > 1.0, "delta should carry the cuts");
}

#[test]
fn mix_blends_the_cut_and_output_trims_the_level() {
    let full = tone_cut(&[(DEPTH, 12.0)], 1000.0);
    let half = tone_cut(&[(DEPTH, 12.0), (MIX, 50.0)], 1000.0);
    let expected = -db(0.5 + 0.5 * 10.0_f64.powf(-full / 20.0));
    assert!(
        (half - expected).abs() < 0.3,
        "half mix {half:.2} vs {expected:.2}"
    );
    let trimmed = tone_cut(&[(DEPTH, 0.0), (OUTPUT, 6.0)], 1000.0);
    assert!((trimmed + 6.0).abs() < 0.01, "{trimmed:.3}");
}

#[test]
fn stereo_modes_decide_which_channels_share_cuts() {
    let mut noise = Noise(8);
    // A tone only on the left; noise on both.
    let input = signal(48_000, |i| {
        let (a, b) = (0.03 * noise.next(), 0.03 * noise.next());
        [a + tone(2500.0, 0.3, i), b]
    });
    let right_band = |stereo: f64| {
        let output = run(&mut processor(&[(STEREO, stereo), (DEPTH, 12.0)]), &input);
        let range = (24_000 + LAG)..(40_000 + LAG);
        10.0 * (band_power(&output, 1, (2400.0, 2600.0), range)
            / band_power(&input, 1, (2400.0, 2600.0), 24_000..40_000))
        .log10()
    };
    let linked = right_band(0.0);
    let dual = right_band(1.0);
    assert!(linked < -4.0, "linked right channel {linked:.2} dB");
    assert!(dual.abs() < 0.5, "dual right channel {dual:.2} dB");
}

#[test]
fn mid_side_keeps_a_mono_signal_mono() {
    let mut noise = Noise(13);
    let input = signal(24_000, |i| {
        let x = tone(1200.0, 0.3, i) + 0.02 * noise.next();
        [x, x]
    });
    let output = run(&mut processor(&[(STEREO, 2.0), (DEPTH, 12.0)]), &input);
    assert!(
        output
            .iter()
            .all(|frame| (frame[0] - frame[1]).abs() < 1.0e-5)
    );
    let cut = db(magnitude(&input, 0, 1200.0, 12_000..20_000)
        / magnitude(&output, 0, 1200.0, (12_000 + LAG)..(20_000 + LAG)));
    assert!(cut > 8.0, "mid cut {cut:.2}");
}

#[test]
fn silence_stays_silent_and_bad_input_is_ignored() {
    let mut processor = processor(&[(DEPTH, 20.0)]);
    let silent = run(&mut processor, &vec![[0.0; 2]; 10_000]);
    assert!(silent.iter().all(|frame| *frame == [0.0; 2]));
    let mut bad = vec![[f32::NAN, f32::INFINITY]; 3000];
    bad.extend(vec![[f32::MAX, -f32::MAX]; 3000]);
    bad.extend(vec![[0.5, -0.5]; 3000]);
    let output = run(&mut processor, &bad);
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
}

#[test]
fn block_size_does_not_change_the_result() {
    let mut noise = Noise(4);
    let input = signal(12_000, |i| {
        let x = tone(700.0, 0.4, i) + 0.05 * noise.next();
        [x, -x]
    });
    let reference = {
        let mut frames = input.clone();
        processor(&[]).process(&mut frames);
        frames
    };
    for block in [1, 3, 128, 1000] {
        let mut processor = processor(&[]);
        let mut frames = input.clone();
        for chunk in frames.chunks_mut(block) {
            processor.process(chunk);
        }
        assert_eq!(frames, reference, "block {block}");
    }
}

#[test]
fn reset_clears_the_signal_path() {
    let mut processor = processor(&[]);
    let input = signal(8000, |i| [tone(440.0, 0.5, i); 2]);
    run(&mut processor, &input);
    processor.reset();
    let after = run(&mut processor, &vec![[0.0; 2]; 2000]);
    assert!(after.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(processor.deepest, 0.0);
}

#[test]
fn parameters_follow_automation_and_ignore_bad_values() {
    let mut processor = processor(&[]);
    processor.set_parameter(DEPTH, f64::NAN);
    assert_eq!(processor.value(DEPTH), 6.0);
    processor.set_parameter(DEPTH, 1.0e9);
    assert_eq!(processor.value(DEPTH), 20.0);
    processor.set_parameter(STEREO, 1.4);
    assert_eq!(processor.value(STEREO), 1.0);
    processor.set_parameter(WIDTH, 0.0);
    processor.set_parameter(BAND_BASE + BAND_GAIN, 12.0);
    assert!(processor.weights_stale);
    processor.refresh_tables();
    assert_eq!(processor.shape.width(), 1.0 / 64.0);
    assert!(!processor.weights_stale);
    processor.set_parameter(999, 0.5);
}

#[test]
fn telemetry_draws_the_cut_where_it_happens() {
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = watched(&[(DEPTH, 10.0)], Some(telemetry.clone()));
    let input = signal(24_000, |i| [tone(2000.0, 0.3, i); 2]);
    run(&mut processor, &input);
    let band = |hz: f64| {
        let position = (hz / 20.0).ln() / 1000.0_f64.ln() * (REDUCTION_BANDS - 1) as f64;
        f32::from_bits(
            telemetry.reduction_spectrum[position.round() as usize].load(Ordering::Relaxed),
        )
    };
    assert!(band(2000.0) > 6.0, "{}", band(2000.0));
    assert!(
        band(200.0) < 0.1 && band(12_000.0) < 0.1,
        "{} {}",
        band(200.0),
        band(12_000.0)
    );
    let reduction = f32::from_bits(telemetry.reduction_db.load(Ordering::Relaxed));
    assert!(reduction > 6.0);
    assert!(telemetry.spectrum_updates.load(Ordering::Relaxed) > 10);
    let level = |cells: &[std::sync::atomic::AtomicU32; 128], hz: f64| {
        let position = (hz / 20.0).ln() / 1000.0_f64.ln() * 127.0;
        f32::from_bits(cells[position.round() as usize].load(Ordering::Relaxed))
    };
    // A −10.5 dBFS tone reads about there before Reson and lower after it.
    let before = level(&telemetry.input_spectrum, 2000.0);
    assert!((before + 10.5).abs() < 2.0, "{before}");
    assert!(level(&telemetry.spectrum, 2000.0) < before - 6.0);
}

/// Real-time cost on this machine, on noise and on a melody that keeps Focus busy:
/// `cargo test --release -p sympoie-reson tests::cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn cost() {
    let seconds = 60;
    let mut noise = Noise(17);
    let tones = signal(48_000 * seconds, |i| {
        let x = 0.1 * noise.next() + tone(1800.0, 0.2, i) + tone(5300.0, 0.05, i);
        [x, 0.8 * x]
    });
    let melody = melody_through_body(seconds as f64, 2400.0);
    for (name, input) in [("noise", &tones), ("melody", &melody)] {
        for (stereo, focus, clean) in [
            (0.0, 0.0, 0.0),
            (1.0, 0.0, 0.0),
            (0.0, 100.0, 0.0),
            (1.0, 100.0, 0.0),
            (0.0, 0.0, 12.0),
            (1.0, 100.0, 12.0),
        ] {
            let mut processor = processor(&[(STEREO, stereo), (FOCUS, focus), (CLEAN, clean)]);
            let mut frames = input.clone();
            let started = std::time::Instant::now();
            for chunk in frames.chunks_mut(256) {
                processor.process(chunk);
            }
            let elapsed = started.elapsed().as_secs_f64();
            println!(
                "{name}, stereo mode {stereo}, Focus {focus}%, Clean {clean} dB: \
                 {:.2}% of one core",
                100.0 * elapsed / seconds as f64
            );
        }
    }
}

/// A melody of harmonic notes that changes every 300 ms, played through a body with one
/// fixed resonance at `resonance` Hz.
fn melody_through_body(seconds: f64, resonance: f64) -> Vec<Frame> {
    let notes = [
        165.0, 247.0, 196.0, 330.0, 220.0, 294.0, 185.0, 392.0, 262.0, 349.0, 208.0, 440.0,
    ];
    let mut phase = 0.0_f64;
    let mut noise = Noise(31);
    signal((seconds * RATE) as usize, |i| {
        let time = i as f64 / RATE;
        let pitch = notes[(time / 0.3) as usize % notes.len()];
        phase = (phase + pitch / RATE).fract();
        let mut x = 0.0;
        let mut harmonic = 1;
        while harmonic as f64 * pitch < 10_000.0 {
            let hz = harmonic as f64 * pitch;
            let octaves = (hz / resonance).log2() / 0.05;
            let body = 10.0_f64.powf(14.0 * (-0.5 * octaves * octaves).exp() / 20.0);
            x += body * (TAU * phase * harmonic as f64).sin() / harmonic as f64;
            harmonic += 1;
        }
        let x = 0.1 * x + 0.0003 * noise.next();
        [x, x]
    })
}

/// Each bin's cut, in dB, averaged over the hops after the first `from` samples.
fn mean_cuts(processor: &mut Reson, input: &[Frame], from: usize) -> Vec<f64> {
    let mut sums = vec![0.0; BINS];
    let mut count = 0;
    for (index, chunk) in input.chunks(HOP).enumerate() {
        let mut frames = chunk.to_vec();
        processor.process(&mut frames);
        if index * HOP >= from {
            for (sum, &cut) in sums.iter_mut().zip(processor.reduction[0].iter()) {
                *sum += f64::from(cut);
            }
            count += 1;
        }
    }
    sums.iter().map(|sum| sum / f64::from(count)).collect()
}

fn region(cuts: &[f64], hz: (f64, f64)) -> f64 {
    let bin = |hz: f64| (hz * FRAME as f64 / RATE).round() as usize;
    let bins = bin(hz.0)..=bin(hz.1);
    let count = bins.clone().count() as f64;
    cuts[bins].iter().sum::<f64>() / count
}

#[test]
fn focus_spares_moving_harmonics_and_keeps_cutting_a_fixed_resonance() {
    let input = melody_through_body(10.0, 2400.0);
    let measure = |focus: f64| {
        let mut processor = processor(&[(DEPTH, 10.0), (THRESHOLD, 2.25), (FOCUS, focus)]);
        let cuts = mean_cuts(&mut processor, &input, 48_000 * 4);
        let moving = region(&cuts, (300.0, 2100.0)) + region(&cuts, (2700.0, 6000.0));
        (region(&cuts, (2300.0, 2500.0)), moving)
    };
    let (fixed_all, moving_all) = measure(0.0);
    let (fixed_focused, moving_focused) = measure(100.0);
    let (fixed_half, moving_half) = measure(50.0);
    assert!(
        fixed_all > 5.0,
        "the resonance is cut without Focus: {fixed_all:.2}"
    );
    assert!(
        moving_all > 1.5,
        "the moving harmonics are cut without Focus: {moving_all:.2}"
    );
    // The harmonic the resonance lifts may be brought down to a little above its
    // neighbours; that margin, which keeps a note's natural unevenness, is all it gives up.
    assert!(
        fixed_focused > 0.8 * fixed_all,
        "Focus keeps the resonance cut: {fixed_focused:.2} vs {fixed_all:.2}"
    );
    assert!(
        moving_focused < 0.2 * moving_all,
        "Focus spares the moving harmonics: {moving_focused:.2} vs {moving_all:.2}"
    );
    assert!(
        fixed_half > 0.85 * fixed_all && moving_half < moving_all && moving_half > moving_focused,
        "half Focus: {fixed_half:.2} vs {fixed_all:.2}, {moving_half:.3} between {moving_all:.3} \
         and {moving_focused:.3}"
    );
}

/// A held note at `f0` with harmonics falling as `1/h`, and an optional extra sine.
fn held_note(seconds: f64, f0: f64, extra: Option<(f64, f64)>) -> Vec<Frame> {
    let mut noise = Noise(51);
    signal((seconds * RATE) as usize, |i| {
        let t = i as f64 / RATE;
        let mut x = 0.0;
        let mut harmonic = 1.0;
        while harmonic * f0 < 12_000.0 {
            x += (TAU * harmonic * f0 * t + harmonic).sin() / harmonic;
            harmonic += 1.0;
        }
        let ring = extra.map_or(0.0, |(hz, level)| level * (TAU * hz * t).sin());
        let x = 0.1 * x + ring + 0.0003 * noise.next();
        [x, x]
    })
}

/// Mean of `cuts` over the bins within one bin of each harmonic of `f0` up to `limit` Hz.
fn harmonic_cut(cuts: &[f64], f0: f64, limit: f64) -> f64 {
    let mut total = 0.0;
    let mut count = 0.0;
    let mut harmonic = 1.0;
    while harmonic * f0 < limit {
        let bin = (harmonic * f0 * FRAME as f64 / RATE).round() as usize;
        total += cuts[bin - 1..=bin + 1].iter().sum::<f64>();
        count += 3.0;
        harmonic += 1.0;
    }
    total / count
}

#[test]
fn focus_leaves_a_long_note_alone_however_long_it_is_held() {
    let input = held_note(8.0, 220.0, None);
    let measure = |focus: f64| {
        let mut processor = processor(&[(DEPTH, 10.0), (THRESHOLD, 1.5), (FOCUS, focus)]);
        let late = mean_cuts(&mut processor, &input, 48_000 * 6);
        harmonic_cut(&late, 220.0, 8000.0)
    };
    let (unfocused, focused) = (measure(0.0), measure(100.0));
    assert!(
        unfocused > 1.0,
        "without Focus the harmonics are cut: {unfocused:.2}"
    );
    assert!(
        focused < 0.05,
        "with Focus a held note is left alone: {focused:.3}"
    );
}

#[test]
fn focus_leaves_a_hollow_tone_alone_too() {
    // A clarinet-like tone: odd harmonics falling as 1/h, even ones 20 dB below them, so
    // every loud harmonic stands well above the two either side of it.
    let f0 = 220.0;
    let mut noise = Noise(71);
    let input = signal(48_000 * 4, |i| {
        let t = i as f64 / RATE;
        let x: f64 = (1..=54_u32)
            .map(|harmonic| {
                let level = if harmonic % 2 == 1 { 1.0 } else { 0.1 };
                let h = f64::from(harmonic);
                level * (TAU * h * f0 * t + h).sin() / h
            })
            .sum();
        let x = 0.1 * x + 0.0003 * noise.next();
        [x, x]
    });
    let measure = |focus: f64| {
        let mut processor = processor(&[(DEPTH, 10.0), (THRESHOLD, 1.5), (FOCUS, focus)]);
        let cuts = mean_cuts(&mut processor, &input, 48_000 * 2);
        harmonic_cut(&cuts, f0, 8000.0)
    };
    let (unfocused, focused) = (measure(0.0), measure(100.0));
    assert!(
        unfocused > 1.0,
        "without Focus the loud harmonics are cut: {unfocused:.2}"
    );
    assert!(
        focused < 0.05,
        "with Focus the tone is left alone: {focused:.3}"
    );
}

#[test]
fn focus_still_cuts_what_is_not_a_harmonic() {
    // A ring between the 7th and 8th harmonics of the held note, well above them.
    let input = held_note(4.0, 220.0, Some((1650.0, 0.1)));
    let ring = |focus: f64| {
        let mut processor = processor(&[(DEPTH, 10.0), (FOCUS, focus)]);
        let cuts = mean_cuts(&mut processor, &input, 48_000);
        region(&cuts, (1630.0, 1670.0))
    };
    let (unfocused, focused) = (ring(0.0), ring(100.0));
    assert!(unfocused > 5.0, "{unfocused:.2}");
    assert!(focused > 0.9 * unfocused, "{focused:.2} vs {unfocused:.2}");
}

#[test]
fn without_notes_focus_changes_next_to_nothing() {
    let mut noise = Noise(61);
    let input = signal(48_000, |i| {
        let x = 0.1 * noise.next() + tone(2300.0, 0.05, i);
        [x, x]
    });
    let plain = run(&mut processor(&[(DEPTH, 10.0)]), &input);
    let focused = run(&mut processor(&[(DEPTH, 10.0), (FOCUS, 100.0)]), &input);
    // Noise peaks only rarely line up like a note's partials, so the two barely differ,
    // and the whistle is cut just the same.
    let energy = |frames: &[Frame]| frames.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>();
    let difference: Vec<Frame> = plain
        .iter()
        .zip(&focused)
        .map(|(a, b)| [a[0] - b[0], a[1] - b[1]])
        .collect();
    let ratio = 10.0 * (energy(&difference) / energy(&plain)).log10();
    assert!(ratio < -40.0, "Focus changed noise by {ratio:.1} dB");
    let whistle = |frames: &[Frame]| magnitude(frames, 0, 2300.0, (24_000 + LAG)..(40_000 + LAG));
    assert!((db(whistle(&plain) / whistle(&focused))).abs() < 0.2);
}

#[test]
fn telemetry_reports_the_notes_and_the_harmonics_it_spares() {
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = watched(&[(FOCUS, 100.0)], Some(telemetry.clone()));
    run(&mut processor, &held_note(1.0, 220.0, None));
    let note = f32::from_bits(telemetry.notes[0].load(Ordering::Relaxed));
    assert!((note / 220.0 - 1.0).abs() < 0.005, "{note}");
    assert_eq!(
        f32::from_bits(telemetry.notes[1].load(Ordering::Relaxed)),
        0.0
    );
    let band = |hz: f64| {
        let position = (hz / 20.0).ln() / 1000.0_f64.ln() * (REDUCTION_BANDS - 1) as f64;
        f32::from_bits(telemetry.spared_spectrum[position.round() as usize].load(Ordering::Relaxed))
    };
    assert!(band(660.0) > 0.99, "{}", band(660.0));
    assert!(band(550.0) < 0.01, "{}", band(550.0));
}

/// Spread over time of each bin's level in dB, averaged over the bins from 500 Hz to
/// 12 kHz: steady noise reads about 5.6 dB, noise that flickers in and out reads more.
fn flicker(frames: &[Frame], range: std::ops::Range<usize>) -> f64 {
    let size = 1024;
    let fft = spectral::Fft::new(size);
    let bins = (500 * size / 48_000)..(12_000 * size / 48_000);
    let mut levels: Vec<Vec<f64>> = vec![Vec::new(); bins.len()];
    let mut start = range.start;
    while start + size <= range.end {
        let mut re: Vec<f32> = (0..size)
            .map(|n| {
                let window = 0.5 - 0.5 * (TAU * n as f64 / size as f64).cos();
                (f64::from(frames[start + n][0]) * window) as f32
            })
            .collect();
        let mut im = vec![0.0; size];
        fft.forward(&mut re, &mut im);
        for (levels, k) in levels.iter_mut().zip(bins.clone()) {
            let power = f64::from(re[k]).powi(2) + f64::from(im[k]).powi(2);
            levels.push(10.0 * power.max(1e-30).log10());
        }
        start += size;
    }
    let spread = |values: &[f64]| {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
    };
    levels.iter().map(|values| spread(values)).sum::<f64>() / levels.len() as f64
}

fn energy(frames: &[Frame], range: std::ops::Range<usize>) -> f64 {
    frames[range].iter().map(|f| f64::from(f[0]).powi(2)).sum()
}

/// Change in level, in dB, from `input` to `output` over the same span of audio.
fn level_change(input: &[Frame], output: &[Frame], range: std::ops::Range<usize>) -> f64 {
    let delayed = (range.start + LAG)..(range.end + LAG);
    10.0 * (energy(output, delayed) / energy(input, range)).log10()
}

#[test]
fn clean_turns_steady_noise_down_by_about_its_depth_without_flicker() {
    for (stereo, clean, least, most_flicker) in [
        (0.0, 12.0, 10.5, 1.02),
        (1.0, 12.0, 10.0, 1.03),
        (0.0, 24.0, 20.0, 1.05),
    ] {
        let mut noise = Noise(91);
        let input = signal(48_000 * 4, |_| [0.05 * noise.next(), 0.05 * noise.next()]);
        let mut processor = processor(&[(DEPTH, 0.0), (CLEAN, clean), (STEREO, stereo)]);
        let output = run(&mut processor, &input);
        let late = 48_000 * 2..48_000 * 4 - LAG;
        let change = level_change(&input, &output, late.clone());
        assert!(
            change < -least && change > -clean - 0.5,
            "Clean {clean} in stereo mode {stereo}: {change:.1} dB"
        );
        let delayed = (late.start + LAG)..(late.end + LAG);
        let (before, after) = (flicker(&input, late), flicker(&output, delayed));
        assert!(
            after < most_flicker * before,
            "Clean {clean} in stereo mode {stereo} flickers: {after:.2} vs {before:.2}"
        );
        // The floor stands for the noise's mean power, give or take 2 dB.
        let mean = processor.power[0][100..900].iter().sum::<f32>() / 800.0;
        let floor = processor.cleaners[0].noise()[100..900].iter().sum::<f32>() / 800.0;
        assert!(
            (10.0 * (floor / mean).log10()).abs() < 2.0,
            "{floor} vs {mean}"
        );
    }
}

#[test]
fn clean_leaves_a_held_note_alone_and_cleans_between_its_harmonics() {
    let mut noise = Noise(95);
    let input: Vec<Frame> = held_note(4.0, 220.0, None)
        .iter()
        .map(|frame| {
            let hiss = 0.01 * noise.next() as f32;
            [frame[0] + hiss, frame[1] + hiss]
        })
        .collect();
    let output = run(&mut processor(&[(DEPTH, 0.0), (CLEAN, 12.0)]), &input);
    let (range, delayed) = (48_000 * 2..48_000 * 4 - LAG, 48_000 * 2 + LAG..48_000 * 4);
    let change = |hz: f64| {
        db(magnitude(&output, 0, hz, delayed.clone()) / magnitude(&input, 0, hz, range.clone()))
    };
    for harmonic in [1.0, 2.0, 5.0, 10.0, 20.0, 40.0] {
        let hz = harmonic * 220.0;
        assert!(
            change(hz).abs() < 0.5,
            "harmonic at {hz} Hz: {:.2}",
            change(hz)
        );
    }
    for hz in [330.0, 1210.0, 4510.0] {
        assert!(
            change(hz) < -8.0,
            "between harmonics at {hz} Hz: {:.1}",
            change(hz)
        );
    }
}

#[test]
fn clean_lets_attacks_through_and_decays_fade_on_their_own() {
    // A bright pluck every half second over quiet hiss.
    let mut noise = Noise(97);
    let input = signal(48_000 * 4, |i| {
        let t = (i % 24_000) as f64 / RATE;
        let pluck: f64 = (1..12)
            .map(|h| (TAU * 330.0 * f64::from(h) * t).sin() / f64::from(h))
            .sum();
        let x = 0.3 * pluck * (-t * 8.0).exp() + 0.003 * noise.next();
        [x, x]
    });
    let output = run(&mut processor(&[(DEPTH, 0.0), (CLEAN, 12.0)]), &input);
    for pluck in 2..8 {
        let start = pluck * 24_000;
        let attack = level_change(&input, &output, start..start + 240);
        assert!(
            attack.abs() < 0.2,
            "the first 5 ms of pluck {pluck}: {attack:.2} dB"
        );
        let decay = level_change(&input, &output, start + 9_600..start + 14_400);
        assert!(
            decay.abs() < 0.5,
            "pluck {pluck} 200–300 ms on: {decay:.2} dB"
        );
    }
}

#[test]
fn clean_never_takes_a_steady_tone_for_noise() {
    // Sines from the very first sample, over noise whose bins read 0.05² · 768 ≈ 1.9:
    // 20 and 30 dB above it.
    for (amplitude, most) in [(0.027, 0.5), (0.0856, 0.1)] {
        let mut noise = Noise(93);
        let input = signal(48_000 * 8, |i| {
            let x = 0.05 * noise.next() + tone(3000.0, amplitude, i);
            [x, x]
        });
        let output = run(&mut processor(&[(DEPTH, 0.0), (CLEAN, 12.0)]), &input);
        let late = 48_000 * 6..48_000 * 8 - LAG;
        let change = db(
            magnitude(&output, 0, 3000.0, (late.start + LAG)..(late.end + LAG))
                / magnitude(&input, 0, 3000.0, late),
        );
        assert!(change.abs() < most, "a sine of {amplitude}: {change:.2} dB");
    }
}

#[test]
fn a_clip_starting_from_silence_does_not_fool_the_floor() {
    // A second of digital silence, then hiss: the frames that straddle the start would
    // read the hiss far quieter than it is.
    let mut noise = Noise(99);
    let input = signal(48_000 * 4, |i| {
        let x = if i < 48_000 { 0.0 } else { 0.05 * noise.next() };
        [x, x]
    });
    let output = run(&mut processor(&[(DEPTH, 0.0), (CLEAN, 12.0)]), &input);
    let change = level_change(&input, &output, 48_000 * 3..48_000 * 4 - LAG);
    assert!(change < -10.5, "{change:.1} dB");
}

#[test]
fn reset_drops_the_cuts_but_keeps_the_floor() {
    let mut noise = Noise(101);
    let input = signal(48_000 * 3, |_| {
        let x = 0.05 * noise.next();
        [x, x]
    });
    let mut processor = processor(&[(DEPTH, 0.0), (CLEAN, 12.0)]);
    run(&mut processor, &input[..48_000 * 2]);
    processor.reset();
    assert!(processor.cleaners[0].state.iter().all(|&cut| cut == 0.0));
    // Straight after the reset Clean works at full depth, with no second learning.
    let output = run(&mut processor, &input[48_000 * 2..]);
    let change = level_change(&input[48_000 * 2..], &output, 4_800..24_000);
    assert!(change < -10.0, "{change:.1} dB");
}

#[test]
fn telemetry_shows_the_noise_clean_removes_apart_from_the_resonance_cut() {
    let telemetry = Arc::new(Telemetry::default());
    let mut processor = watched(&[(DEPTH, 0.0), (CLEAN, 12.0)], Some(telemetry.clone()));
    let mut noise = Noise(103);
    run(
        &mut processor,
        &signal(48_000 * 2, |_| {
            let x = 0.05 * noise.next();
            [x, x]
        }),
    );
    let read = |cells: &[std::sync::atomic::AtomicU32]| {
        cells
            .iter()
            .map(|cell| f32::from_bits(cell.load(Ordering::Relaxed)))
            .collect::<Vec<_>>()
    };
    let (cleaning, reduction) = (
        read(&telemetry.cleaning_spectrum),
        read(&telemetry.reduction_spectrum),
    );
    // From 1 to 10 kHz, where the default sensitivity is flat.
    let bands = 145..230;
    assert!(
        cleaning[bands.clone()].iter().all(|&cut| cut > 8.0),
        "{cleaning:?}"
    );
    assert!(
        reduction[bands].iter().all(|&cut| cut < 0.1),
        "{reduction:?}"
    );
}

/// Reson at another sample rate with some parameters set.
fn processor_at(rate: f64, settings: &[(u32, f64)]) -> Reson {
    let mut reson = Reson::new(rate, None);
    for &(id, value) in settings {
        reson.set_parameter(id, value);
    }
    reson.reset();
    reson
}

#[test]
fn every_sample_rate_delays_by_its_latency_and_passes_what_it_does_not_cut() {
    for rate in [44_100.0, 88_200.0, 96_000.0, 192_000.0] {
        let mut processor = processor_at(rate, &[(DEPTH, 0.0)]);
        let lag = processor.latency() as usize;
        let mut noise = Noise(2);
        let input = signal(30_000, |_| [0.5 * noise.next(), 0.3 * noise.next()]);
        let output = run(&mut processor, &input);
        for (out, original) in output[lag..].iter().zip(&input) {
            for channel in 0..2 {
                assert!((out[channel] - original[channel]).abs() < 1.0e-6, "{rate}");
            }
        }
        // An impulse arrives exactly one latency later, with Reson cutting.
        let mut processor = processor_at(rate, &[]);
        let mut impulse = vec![[0.0; 2]; 3 * lag + 4096];
        impulse[1000] = [1.0, -0.5];
        let output = run(&mut processor, &impulse);
        assert!((output[1000 + lag][0] - 1.0).abs() < 1.0e-5, "{rate}");
    }
}

#[test]
fn a_resonance_is_cut_alike_at_every_sample_rate() {
    let cut = |rate: f64| {
        let mut noise = Noise(5);
        let length = (rate * 1.5) as usize;
        let input = signal(length, |i| {
            let x = 0.3 * (TAU * 2400.0 * i as f64 / rate).sin() + 0.02 * noise.next();
            [x, x]
        });
        let mut processor = processor_at(rate, &[(DEPTH, 12.0)]);
        let lag = processor.latency() as usize;
        let output = run(&mut processor, &input);
        let level = |frames: &[Frame], range: std::ops::Range<usize>| {
            let length = range.len() as f64;
            let (re, im) =
                frames[range]
                    .iter()
                    .enumerate()
                    .fold((0.0, 0.0), |(re, im), (i, frame)| {
                        let phase = TAU * 2400.0 * i as f64 / rate;
                        let window = 0.5 - 0.5 * (TAU * i as f64 / length).cos();
                        let x = f64::from(frame[0]) * window;
                        (re + x * phase.cos(), im - x * phase.sin())
                    });
            (re * re + im * im).sqrt()
        };
        let late = length / 2..length - lag;
        db(level(&input, late.clone()) / level(&output, (late.start + lag)..(late.end + lag)))
    };
    let reference = cut(48_000.0);
    assert!(reference > 6.0, "{reference:.2}");
    for rate in [44_100.0, 96_000.0, 192_000.0] {
        let other = cut(rate);
        assert!(
            (other - reference).abs() < 1.5,
            "{rate}: {other:.2} vs {reference:.2}"
        );
    }
}

#[test]
fn focus_and_clean_work_at_every_sample_rate() {
    for rate in [44_100.0, 96_000.0] {
        // A held note over hiss: Focus spares its harmonics, Clean takes the hiss.
        let f0 = 220.0;
        let length = (rate * 3.0) as usize;
        let mut noise = Noise(7);
        let input = signal(length, |i| {
            let t = i as f64 / rate;
            let note: f64 = (1..40)
                .map(|h| {
                    let h = f64::from(h);
                    (TAU * h * f0 * t + h).sin() / h
                })
                .sum();
            let x = 0.1 * note + 0.004 * noise.next();
            [x, x]
        });
        let mut processor = processor_at(rate, &[(DEPTH, 10.0), (FOCUS, 100.0), (CLEAN, 12.0)]);
        let lag = processor.latency() as usize;
        let output = run(&mut processor, &input);
        let late = (length * 2 / 3)..(length - lag);
        let change = |hz: f64| {
            let tone = |frames: &[Frame], range: std::ops::Range<usize>| {
                let length = range.len() as f64;
                let (re, im) =
                    frames[range]
                        .iter()
                        .enumerate()
                        .fold((0.0, 0.0), |(re, im), (i, frame)| {
                            let phase = TAU * hz * i as f64 / rate;
                            let window = 0.5 - 0.5 * (TAU * i as f64 / length).cos();
                            let x = f64::from(frame[0]) * window;
                            (re + x * phase.cos(), im - x * phase.sin())
                        });
                (re * re + im * im).sqrt()
            };
            db(tone(&output, (late.start + lag)..(late.end + lag)) / tone(&input, late.clone()))
        };
        for harmonic in [1.0, 3.0, 10.0, 25.0] {
            let kept = change(harmonic * f0);
            assert!(kept.abs() < 0.5, "{rate}: harmonic {harmonic} {kept:.2}");
        }
        let between = change(4.5 * f0);
        assert!(
            between < -6.0,
            "{rate}: hiss between harmonics {between:.1}"
        );
    }
}

#[test]
fn the_display_reads_the_same_bands_at_every_sample_rate() {
    for rate in [44_100.0, 96_000.0] {
        let telemetry = Arc::new(Telemetry::default());
        let mut processor = Reson::new(rate, Some(telemetry.clone()));
        processor.set_parameter(DEPTH, 10.0);
        let mut noise = Noise(11);
        let input = signal(rate as usize, |i| {
            let x = 0.25 * (TAU * 2000.0 * i as f64 / rate).sin() + 0.01 * noise.next();
            [x, x]
        });
        run(&mut processor, &input);
        let band = |hz: f64| {
            let position = (hz / 20.0).ln() / 1000.0_f64.ln() * (REDUCTION_BANDS - 1) as f64;
            position.round() as usize
        };
        let cut = telemetry::load(&telemetry.reduction_spectrum);
        assert!(cut[band(2000.0)] > 3.0, "{rate}: {}", cut[band(2000.0)]);
        assert!(cut[band(500.0)] < 0.5, "{rate}: {}", cut[band(500.0)]);
        // A full-scale-ish tone reads near its level in dBFS.
        let input_level = telemetry::load(&telemetry.input_spectrum);
        let position = (2000.0_f64 / 20.0).ln() / 1000.0_f64.ln() * (SPECTRUM_BANDS - 1) as f64;
        let level = input_level[position.round() as usize];
        assert!((level + 12.0).abs() < 2.5, "{rate}: {level:.1} dB");
    }
}
