use super::*;
use crate::builtin::BuiltinKind;

fn context(quality: RenderQuality) -> PluginProcessContext {
    PluginProcessContext {
        frame: 0,
        bpm: 120.0,
        playing: true,
        quality,
    }
}

fn instance(values: &[(u32, f64)]) -> PluginInstance {
    let mut instance = BuiltinKind::Ferro.instance();
    for &(id, physical) in values {
        let spec = spec(id).expect("known parameter");
        instance
            .parameters
            .iter_mut()
            .find(|parameter| parameter.id == id)
            .expect("parameter present")
            .value = spec.normalize(physical);
    }
    instance
}

fn on(id: u64, key: u8, slide_frames: u32) -> NoteEvent {
    NoteEvent::On {
        id,
        key,
        velocity: 100,
        channel: 0,
        slide_frames,
    }
}

/// Renders `frames` with `events` at their frames, processing in blocks of `block` (or
/// a varied pattern when `block` is 0).
fn render(
    ferro: &mut Ferro,
    events: &[(usize, NoteEvent)],
    frames: usize,
    block: usize,
    quality: RenderQuality,
) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; frames];
    let pattern = [1, 7, 63, 256, 3, 128, 17];
    let mut written = 0;
    let mut next = 0;
    let mut step = 0;
    while written < frames {
        let mut length = if block == 0 {
            pattern[step % pattern.len()]
        } else {
            block
        };
        step += 1;
        while let Some(&(frame, event)) = events.get(next) {
            if frame <= written {
                ferro.event(event);
                next += 1;
            } else {
                length = length.min(frame - written);
                break;
            }
        }
        let length = length.min(frames - written);
        ferro.process(&mut output[written..written + length], context(quality));
        written += length;
    }
    output
}

fn peak(frames: &[Frame]) -> f32 {
    frames
        .iter()
        .flat_map(|frame| frame.iter())
        .fold(0.0, |peak, sample| peak.max(sample.abs()))
}

fn sequence() -> Vec<(usize, NoteEvent)> {
    vec![
        (10, on(1, 48, 0)),
        (900, on(2, 55, 0)),
        (2000, NoteEvent::Off { id: 1 }),
        (
            2100,
            NoteEvent::PitchBend {
                channel: 0,
                value: 0.5,
                ramp_frames: 700,
            },
        ),
        (3000, NoteEvent::Off { id: 2 }),
        (3000, on(3, 60, 600)),
        (5000, NoteEvent::Off { id: 3 }),
    ]
}

#[test]
fn output_does_not_depend_on_block_size_in_either_engine() {
    for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
        for mode in [0.0, 1.0, 2.0] {
            let preset = instance(&[(VOICE_MODE, mode), (CHORUS, 3.0), (GLIDE_MODE, 1.0)]);
            let mut whole = Ferro::new(&preset, None);
            let mut split = Ferro::new(&preset, None);
            let a = render(&mut whole, &sequence(), 7000, 256, quality);
            let b = render(&mut split, &sequence(), 7000, 0, quality);
            assert!(peak(&a) > 0.01, "{quality:?} mode {mode} is silent");
            assert_eq!(a, b, "{quality:?} mode {mode} depends on the block split");
        }
    }
}

#[test]
fn reset_makes_renders_repeat_exactly() {
    for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
        let mut ferro = Ferro::new(&instance(&[(DRIFT, 100.0), (NOISE, 3.0)]), None);
        let first = render(&mut ferro, &sequence(), 6000, 256, quality);
        ferro.reset();
        let second = render(&mut ferro, &sequence(), 6000, 256, quality);
        assert_eq!(first, second, "{quality:?}");
    }
}

#[test]
fn extreme_settings_stay_finite_and_bounded() {
    let mut loud = Vec::new();
    let mut quiet = Vec::new();
    for spec in &PARAMETERS {
        loud.push((spec.id, spec.max));
        quiet.push((spec.id, spec.min));
    }
    for values in [loud, quiet] {
        for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
            let mut ferro = Ferro::new(&instance(&values), None);
            let events: Vec<_> = (0..8)
                .map(|n| (n * 50, on(n as u64, 20 + n as u8 * 12, 0)))
                .collect();
            let output = render(&mut ferro, &events, 12_000, 256, quality);
            assert!(
                output
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite() && sample.abs() < 8.0),
                "{quality:?} blew up"
            );
        }
    }
}

/// Power spectrum by a plain radix-2 FFT of a Hann-windowed mono signal.
fn spectrum(signal: &[f64]) -> Vec<f64> {
    let n = signal.len();
    assert!(n.is_power_of_two());
    let mut re: Vec<f64> = signal
        .iter()
        .enumerate()
        .map(|(i, x)| x * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()))
        .collect();
    let mut im = vec![0.0; n];
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
        }
    }
    let mut length = 2;
    while length <= n {
        let angle = -std::f64::consts::TAU / length as f64;
        for start in (0..n).step_by(length) {
            for k in 0..length / 2 {
                let (w_re, w_im) = ((angle * k as f64).cos(), (angle * k as f64).sin());
                let (a, b) = (start + k, start + k + length / 2);
                let t_re = re[b] * w_re - im[b] * w_im;
                let t_im = re[b] * w_im + im[b] * w_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
            }
        }
        length <<= 1;
    }
    (0..n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
}

fn mono(frames: &[Frame]) -> Vec<f64> {
    frames.iter().map(|frame| f64::from(frame[0])).collect()
}

#[test]
fn filter_self_oscillates_near_its_cutoff() {
    let cutoff = 880.0;
    for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
        let mut ferro = Ferro::new(
            &instance(&[
                (osc(0, OSC_LEVEL), 0.0),
                (osc(1, OSC_LEVEL), 0.0),
                (NOISE, 0.2),
                (EMPHASIS, 10.0),
                (CUTOFF, cutoff),
                (CONTOUR, 0.0),
                (KEY_TRACK, 0.0),
                (FILTER_VELOCITY, 0.0),
                (DRIFT, 0.0),
                (HPF, 1.0),
                (AMP_SUSTAIN, 1.0),
            ]),
            None,
        );
        let output = render(
            &mut ferro,
            &[(0, on(1, 60, 0))],
            24_000 + 16_384,
            256,
            quality,
        );
        let power = spectrum(&mono(&output[24_000..]));
        let bin = power
            .iter()
            .enumerate()
            .skip(4)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(bin, _)| bin)
            .unwrap();
        let hz = bin as f64 * RATE / 16_384.0;
        assert!(
            (hz / cutoff - 1.0).abs() < 0.12,
            "{quality:?} rings at {hz:.0} Hz, not near {cutoff} Hz"
        );
    }
}

fn alias_ratio(quality: RenderQuality) -> f64 {
    // Key 118 at 4' is a 14.9 kHz saw: its fundamental is the only line in the audible
    // band, so everything else below 20 kHz is aliasing folded down from the harmonics.
    let mut ferro = Ferro::new(
        &instance(&[
            (osc(0, OSC_RANGE), 4.0),
            (osc(1, OSC_ON), 0.0),
            (CUTOFF, 20_000.0),
            (EMPHASIS, 0.0),
            (CONTOUR, 0.0),
            (DRIVE, 0.0),
            (DRIFT, 0.0),
            (KEY_TRACK, 0.0),
            (FILTER_VELOCITY, 0.0),
            (AMP_SUSTAIN, 1.0),
        ]),
        None,
    );
    let output = render(
        &mut ferro,
        &[(0, on(1, 118, 0))],
        4800 + 16_384,
        256,
        quality,
    );
    let power = spectrum(&mono(&output[4800..]));
    let f0 = 2.0 * 440.0 * ((118.0 - 69.0) / 12.0_f64).exp2();
    let resolution = RATE / 16_384.0;
    let mut tone = 0.0;
    let mut alias = 0.0;
    for (bin, value) in power.iter().enumerate().skip(8) {
        let hz = bin as f64 * resolution;
        if hz > 20_000.0 {
            break;
        }
        if (hz - f0).abs() < 6.0 * resolution {
            tone += value;
        } else {
            alias += value;
        }
    }
    alias / tone
}

#[test]
fn offline_engine_aliases_less_than_realtime() {
    let realtime = alias_ratio(RenderQuality::Realtime);
    let offline = alias_ratio(RenderQuality::Offline);
    assert!(
        offline < realtime * 0.5,
        "offline alias ratio {offline:e} is not clearly below realtime {realtime:e}"
    );
}

#[test]
fn slides_glide_smoothly_and_land_on_the_target() {
    for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
        for mode in [0.0, 1.0, 2.0] {
            let mut ferro = Ferro::new(&instance(&[(VOICE_MODE, mode)]), None);
            render(&mut ferro, &[(0, on(1, 48, 0))], 2000, 256, quality);
            ferro.event(NoteEvent::Off { id: 1 });
            ferro.event(on(2, 60, 4800));
            let sounding = ferro.voices.iter().filter(|voice| voice.sounding()).count();
            assert_eq!(sounding, 1, "a slide reuses the sounding voice");
            let voice = ferro
                .voices
                .iter()
                .position(|voice| voice.id == Some(2))
                .unwrap();
            let mut previous = ferro.voices[voice].glide.pitch;
            let mut largest = 0.0_f64;
            let mut frame = [[0.0; 2]];
            for _ in 0..4800 {
                ferro.process(&mut frame, context(quality));
                let pitch = ferro.voices[voice].glide.pitch;
                assert!(pitch >= previous, "slide reversed");
                largest = largest.max(pitch - previous);
                previous = pitch;
            }
            assert_eq!(previous, 60.0);
            // An RC slide of 12 semitones over 0.1 s moves at most ~0.012 st per sample.
            assert!(largest < 0.015, "slide stepped by {largest} semitones");
            // The contour continued instead of restarting from silence.
            assert_ne!(ferro.voices[voice].amp.stage, Stage::Attack);
        }
    }
}

#[test]
fn mono_mode_returns_to_the_held_note_legato() {
    let mut ferro = Ferro::new(&instance(&[(VOICE_MODE, 2.0)]), None);
    let events = [
        (0, on(1, 50, 0)),
        (500, on(2, 62, 0)),
        (1500, NoteEvent::Off { id: 2 }),
    ];
    render(&mut ferro, &events, 3000, 256, RenderQuality::Realtime);
    let voice = &ferro.voices[0];
    assert_eq!(voice.id, Some(1));
    assert_eq!(voice.glide.pitch, 50.0);
    assert!(voice.gate);
    assert_ne!(voice.amp.stage, Stage::Release);
}

#[test]
fn poly_mode_spreads_a_chord_over_voices() {
    let mut ferro = Ferro::new(&instance(&[]), None);
    let events: Vec<_> = [48, 52, 55, 59]
        .iter()
        .enumerate()
        .map(|(n, &key)| (n, on(n as u64, key, 0)))
        .collect();
    render(&mut ferro, &events, 500, 256, RenderQuality::Realtime);
    assert_eq!(ferro.voices.iter().filter(|voice| voice.gate).count(), 4);
}

#[test]
fn scheduled_bend_ramps_are_followed_exactly() {
    let mut ferro = Ferro::new(&instance(&[(BEND_RANGE, 12.0)]), None);
    render(
        &mut ferro,
        &[(0, on(1, 60, 0))],
        100,
        256,
        RenderQuality::Realtime,
    );
    ferro.event(NoteEvent::PitchBend {
        channel: 0,
        value: 1.0,
        ramp_frames: 1000,
    });
    let mut frame = [[0.0; 2]];
    for _ in 0..500 {
        ferro.process(&mut frame, context(RenderQuality::Realtime));
    }
    assert!((ferro.bend.smoothed - 0.5).abs() < 1.0e-9);
    for _ in 0..500 {
        ferro.process(&mut frame, context(RenderQuality::Realtime));
    }
    assert_eq!(ferro.bend.smoothed, 1.0);
}

#[test]
fn release_tail_follows_the_release_parameter() {
    let preset = instance(&[(AMP_RELEASE, 0.2), (CHORUS, 0.0)]);
    let tail = crate::builtin::release_frames(&preset, &[]).unwrap() as usize;
    assert_eq!(tail, (0.2 * RATE).ceil() as usize);
    let mut ferro = Ferro::new(&preset, None);
    let output = render(
        &mut ferro,
        &[(0, on(1, 60, 0)), (4800, NoteEvent::Off { id: 1 })],
        4800 + tail + 4800,
        256,
        RenderQuality::Realtime,
    );
    assert!(peak(&output[4800..4800 + tail / 4]) > 0.01);
    assert!(peak(&output[4800 + tail + 480..]) < 1.0e-3);
}

#[test]
fn chorus_widens_the_mono_voice() {
    let mut dry = Ferro::new(&instance(&[(CHORUS, 0.0)]), None);
    let mut wide = Ferro::new(&instance(&[(CHORUS, 1.0)]), None);
    let events = [(0, on(1, 57, 0))];
    let dry = render(&mut dry, &events, 9600, 256, RenderQuality::Realtime);
    let wide = render(&mut wide, &events, 9600, 256, RenderQuality::Realtime);
    assert!(dry.iter().all(|frame| frame[0] == frame[1]));
    let difference = wide[4800..]
        .iter()
        .map(|frame| (frame[0] - frame[1]).abs())
        .fold(0.0, f32::max);
    assert!(difference > 0.01);
}

#[test]
fn presets_cover_known_parameters_and_keep_the_engine_switch() {
    for (index, preset) in PRESETS.iter().enumerate() {
        for (id, _) in preset.values {
            assert!(spec(*id).is_some(), "{} sets unknown {id}", preset.name);
        }
        let values = preset_parameters(index).unwrap();
        assert!(values.iter().all(|(id, _)| *id != ENGINE));
        let mut target = instance(&[(ENGINE, 1.0)]);
        for (id, value) in values {
            target
                .parameters
                .iter_mut()
                .find(|p| p.id == id)
                .unwrap()
                .value = value;
        }
        assert_eq!(matching_preset(&target), Some(index));
        assert!(renders_offline(&target));
    }
}

#[test]
#[ignore = "timing report; run with --release --ignored --nocapture"]
fn report_render_speed() {
    for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
        let mut ferro = Ferro::new(&instance(&[(VOICES, 8.0), (CHORUS, 1.0)]), None);
        let events: Vec<_> = (0..8)
            .map(|n| (n, on(n as u64, 40 + n as u8 * 5, 0)))
            .collect();
        let start = std::time::Instant::now();
        render(&mut ferro, &events, 48_000 * 10, 256, quality);
        let seconds = start.elapsed().as_secs_f64();
        println!(
            "{quality:?}: 10 s of 8 voices in {seconds:.2} s ({:.1}x realtime)",
            10.0 / seconds
        );
    }
}

#[test]
#[ignore = "level report; run with --release --ignored --nocapture"]
fn report_preset_levels() {
    for (index, preset) in PRESETS.iter().enumerate() {
        let mut instance = BuiltinKind::Ferro.instance();
        for (id, value) in preset_parameters(index).unwrap() {
            instance
                .parameters
                .iter_mut()
                .find(|p| p.id == id)
                .unwrap()
                .value = value;
        }
        for quality in [RenderQuality::Realtime, RenderQuality::Offline] {
            let mut ferro = Ferro::new(&instance, None);
            let events: Vec<_> = [48, 55, 60, 64]
                .iter()
                .enumerate()
                .map(|(n, &key)| (n * 10, on(n as u64, key, 0)))
                .collect();
            let output = render(&mut ferro, &events, 48_000 * 2, 256, quality);
            let rms = (output.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>()
                / output.len() as f64)
                .sqrt();
            println!(
                "{:<14} {quality:?}: peak {:>6.1} dB, rms {:>6.1} dB",
                preset.name,
                20.0 * f64::from(peak(&output)).log10(),
                20.0 * rms.log10()
            );
        }
    }
}

/// The ratio of oscillator 1's pitch to oscillator 2's, and the voice's VCO levels,
/// sampled once a second while a note is held.
fn pitch_ratios(drift: f64) -> Vec<(f64, [f64; 3])> {
    let mut ferro = Ferro::new(&instance(&[(DRIFT, drift)]), None);
    render(
        &mut ferro,
        &[(0, on(1, 48, 0))],
        4800,
        256,
        RenderQuality::Realtime,
    );
    (0..4)
        .map(|_| {
            render(&mut ferro, &[], 48_000, 256, RenderQuality::Realtime);
            let voice = ferro.voices.iter().find(|voice| voice.gate).unwrap();
            (voice.steps[0] / voice.steps[1], voice.amplitude)
        })
        .collect()
}

#[test]
fn oscillators_drift_independently() {
    let drifting = pitch_ratios(100.0);
    let cents = |ratio: f64| 1200.0 * ratio.log2();
    let spread = drifting
        .iter()
        .map(|(ratio, _)| cents(*ratio))
        .fold((f64::MAX, f64::MIN), |(lo, hi), c| (lo.min(c), hi.max(c)));
    assert!(
        spread.1 - spread.0 > 0.5,
        "oscillators 1 and 2 kept a fixed interval: {spread:?}"
    );
    assert!(
        drifting
            .iter()
            .all(|(_, levels)| levels.iter().all(|l| (l - 1.0).abs() < 0.1))
    );
    assert!(
        drifting
            .iter()
            .any(|(_, levels)| levels.iter().any(|l| (l - 1.0).abs() > 1e-3))
    );

    let stable = pitch_ratios(0.0);
    assert!(stable.windows(2).all(|pair| pair[0].0 == pair[1].0));
    assert!(stable.iter().all(|(_, levels)| *levels == [1.0; 3]));
}

#[test]
fn chorus_hiss_dies_with_the_note() {
    let mut ferro = Ferro::new(&instance(&[(AMP_RELEASE, 0.2), (CHORUS, 1.0)]), None);
    let output = render(
        &mut ferro,
        &[(0, on(1, 60, 0)), (4800, NoteEvent::Off { id: 1 })],
        4800 + 48_000,
        256,
        RenderQuality::Realtime,
    );
    assert!(peak(&output[4800 + 24_000..]) < 1.0e-4);
}

#[test]
fn coupling_sags_the_tops_of_low_squares() {
    // A 55 Hz square through the coupling capacitor, at the realtime oversampled rate.
    let rate = RATE * 2.0;
    let step = 55.0 / rate;
    let coefficient = one_pole_coefficient(COUPLING_HZ, rate);
    let mut vco = Vco::default();
    let mut coupling = OnePole::default();
    let period = (1.0 / step) as usize;
    let mut cycle = Vec::new();
    for n in 0..period * 40 {
        let raw = vco.tick(3, step, 0.0);
        let out = raw - coupling.lowpass(raw, coefficient);
        if n >= period * 39 {
            cycle.push(out);
        }
    }
    // Within the high half, the level falls from just after the edge to just before it.
    let high: Vec<f64> = cycle.iter().copied().filter(|v| *v > 0.0).collect();
    let (first, last) = (high[4], high[high.len() - 5]);
    assert!(first - last > 0.1 * first, "top went {first} → {last}");
}
