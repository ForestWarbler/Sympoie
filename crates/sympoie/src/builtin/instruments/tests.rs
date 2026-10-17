use super::*;
use crate::builtin::BuiltinKind;

fn context() -> PluginProcessContext {
    PluginProcessContext {
        frame: 0,
        bpm: 120.0,
        playing: true,
    }
}
fn on(id: u64, key: u8) -> NoteEvent {
    NoteEvent::On {
        id,
        key,
        velocity: 127,
        channel: 0,
    }
}
fn set(instance: &mut PluginInstance, id: u32, physical: f64) {
    let spec = oscil_parameters().into_iter().find(|p| p.id == id).unwrap();
    instance
        .parameters
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .value = spec.normalize(physical);
}

#[test]
fn envelope_has_sample_accurate_attack_decay_and_release() {
    let settings = EnvelopeSettings {
        attack: 4.0 / 48000.0,
        decay: 4.0 / 48000.0,
        sustain: 0.5,
        release: 4.0 / 48000.0,
    };
    let mut envelope = Envelope::trigger();
    assert_eq!(
        (0..4).map(|_| envelope.next(settings)).collect::<Vec<_>>(),
        [0.25, 0.5, 0.75, 1.0]
    );
    assert_eq!(
        (0..4).map(|_| envelope.next(settings)).collect::<Vec<_>>(),
        [0.875, 0.75, 0.625, 0.5]
    );
    envelope.off(settings.release);
    assert_eq!(
        (0..4).map(|_| envelope.next(settings)).collect::<Vec<_>>(),
        [0.375, 0.25, 0.125, 0.0]
    );
    assert_eq!(envelope.stage, Stage::Silent);
}

#[test]
fn all_three_oscillators_sound_and_transpose_independently() {
    for osc in 0..3 {
        let mut instance = BuiltinKind::Oscil.instance();
        for index in 0..3 {
            set(
                &mut instance,
                OSC_BASE + index * OSC_STRIDE + OSC_LEVEL,
                -60.0,
            );
        }
        let base = OSC_BASE + osc * OSC_STRIDE;
        set(&mut instance, base + OSC_LEVEL, 0.0);
        set(&mut instance, base + OSC_SHAPE, 0.0);
        set(&mut instance, base + OSC_COARSE, 12.0);
        set(&mut instance, base + OSC_FINE, 0.0);
        set(&mut instance, ATTACK, 0.0001);
        set(&mut instance, SUSTAIN, 1.0);
        let mut synth = Oscil::new(&instance);
        synth.event(on(1, 69));
        let mut audio = vec![[0.0; 2]; 48000];
        synth.process(&mut audio, context());
        let crossings = audio
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        assert!(
            (crossings as i32 - 880).abs() <= 1,
            "oscillator {osc}: {crossings} Hz"
        );
        assert!(audio.iter().any(|f| f[0].abs() > 0.1));
    }
}

#[test]
fn oscillator_shapes_are_bounded_and_pulse_width_changes_duty_cycle() {
    for shape in 0..4 {
        for step in [0.0001, 0.01, 0.2, 0.49] {
            for i in 0..1000 {
                let sample = oscillator(shape, i as f64 / 1000.0, step, 0.13);
                assert!(
                    sample.is_finite() && sample.abs() <= if shape == 3 { 2.01 } else { 1.01 },
                    "shape {shape}: {sample}"
                );
            }
        }
    }
    let average = |width| {
        (0..10000)
            .map(|i| oscillator(3, i as f64 / 10000.0, 0.001, width))
            .sum::<f64>()
            / 10000.0
    };
    assert!(average(0.25).abs() < 0.001);
    assert!(average(0.75).abs() < 0.001);
    assert!((oscillator(3, 0.1, 0.001, 0.25) - 1.5).abs() < 0.001);
    assert!((oscillator(3, 0.9, 0.001, 0.75) + 1.5).abs() < 0.001);
}

#[test]
fn overlapping_notes_have_independent_releases_and_voice_limit() {
    let mut instance = BuiltinKind::Oscil.instance();
    set(&mut instance, RELEASE, 0.001);
    let mut synth = Oscil::new(&instance);
    synth.event(on(1, 60));
    synth.event(on(2, 60));
    synth.process(&mut [[0.0; 2]; 256], context());
    synth.event(NoteEvent::Off { id: 1 });
    synth.process(&mut [[0.0; 2]; 256], context());
    assert_eq!(
        synth
            .voices
            .iter()
            .filter(|v| v.amp.stage != Stage::Silent)
            .count(),
        1
    );
    assert_eq!(
        synth
            .voices
            .iter()
            .find(|v| v.amp.stage != Stage::Silent)
            .unwrap()
            .id,
        2
    );
    for id in 3..100 {
        synth.event(on(id, (id % 128) as u8));
    }
    assert_eq!(
        synth
            .voices
            .iter()
            .filter(|v| v.amp.stage != Stage::Silent)
            .count(),
        VOICES
    );
    let mut audio = [[0.0; 2]; 1024];
    synth.process(&mut audio, context());
    assert!(audio.iter().flatten().all(|sample| sample.is_finite()));
    synth.reset();
    synth.process(&mut audio, context());
    assert_eq!(audio, [[0.0; 2]; 1024]);
}

#[test]
fn rendering_is_independent_of_block_partition_and_parameters_preserve_voices() {
    let instance = BuiltinKind::Oscil.instance();
    let mut whole = Oscil::new(&instance);
    let mut split = Oscil::new(&instance);
    whole.event(on(1, 60));
    split.event(on(1, 60));
    whole.set_parameter(FILTER_CUTOFF, 0.1);
    split.set_parameter(FILTER_CUTOFF, 0.1);
    let mut a = [[0.0; 2]; 1024];
    let mut b = a;
    whole.process(&mut a, context());
    for chunk in b.chunks_mut(37) {
        split.process(chunk, context());
    }
    assert_eq!(a, b);
    assert_eq!(split.voices[0].id, 1);
    assert_ne!(split.voices[0].amp.stage, Stage::Silent);
}

#[test]
fn lowpass_rejects_high_frequencies_and_resonance_stays_stable() {
    let rms = |cutoff, q| {
        let mut filter = Filter::EMPTY;
        filter.coefficients(cutoff, q);
        let mut energy = 0.0;
        for n in 0..48000 {
            let input = (TAU * 10000.0 * f64::from(n) / 48000.0).sin();
            let output = filter.process([input; 2], 0)[0];
            assert!(output.is_finite());
            if n > 24000 {
                energy += output * output;
            }
        }
        energy.sqrt()
    };
    assert!(rms(500.0, 0.707) < rms(20000.0, 0.707) * 0.01);
    assert!(rms(20000.0, 12.0).is_finite());
}

#[test]
fn drive_is_continuous_at_the_clean_position() {
    let instance = BuiltinKind::Oscil.instance();
    let mut clean = Oscil::new(&instance);
    let mut driven = Oscil::new(&instance);
    clean.event(on(1, 60));
    driven.event(on(1, 60));
    clean.process(&mut [[0.0; 2]; 300], context());
    driven.process(&mut [[0.0; 2]; 300], context());
    let spec = oscil_parameters()
        .into_iter()
        .find(|p| p.id == FILTER_DRIVE)
        .unwrap();
    driven.set_automation_parameter(FILTER_DRIVE, spec.normalize(0.002));
    let mut a = [[0.0; 2]; 64];
    let mut b = a;
    clean.process(&mut a, context());
    driven.process(&mut b, context());
    assert!(a.iter().zip(&b).all(|(a, b)| (a[0] - b[0]).abs() < 0.0001));
}

#[test]
fn waveform_filter_and_lfo_switches_preserve_first_sample_and_finish_crossfade() {
    for (id, physical) in [
        (100, 0.0),
        (110, 0.0),
        (120, 0.0),
        (FILTER_TYPE, 1.0),
        (LFO_SHAPE, 1.0),
    ] {
        let mut instance = BuiltinKind::Oscil.instance();
        set(&mut instance, 113, -9.0);
        set(&mut instance, 123, -9.0);
        set(&mut instance, FILTER_CUTOFF, 1200.0);
        set(&mut instance, LFO_PITCH, 600.0);
        let mut clean = Oscil::new(&instance);
        let mut switched = Oscil::new(&instance);
        clean.event(on(1, 60));
        switched.event(on(1, 60));
        clean.process(&mut [[0.0; 2]; 1000], context());
        switched.process(&mut [[0.0; 2]; 1000], context());
        let spec = oscil_parameters().into_iter().find(|p| p.id == id).unwrap();
        let value = spec.normalize(physical);
        switched.set_automation_parameter(id, value);
        let mut a = [[0.0; 2]; 1];
        let mut b = a;
        clean.process(&mut a, context());
        switched.process(&mut b, context());
        assert_eq!(
            a, b,
            "mode switch {id} introduced an instantaneous output step"
        );
        for _ in 1..SMOOTH_SAMPLES {
            switched.set_automation_parameter(id, value);
            switched.process(&mut b, context());
        }
        match id {
            100 | 110 | 120 => assert_eq!(
                switched.oscillator_shapes[((id - 100) / 10) as usize].weights,
                [1.0, 0.0, 0.0, 0.0]
            ),
            FILTER_TYPE => assert_eq!(switched.filter_mode.weights, [0.0, 1.0, 0.0]),
            LFO_SHAPE => assert_eq!(switched.lfo_shape.weights, [0.0, 1.0, 0.0]),
            _ => unreachable!(),
        }
        assert_eq!(switched.voices[0].id, 1);
        assert_ne!(switched.voices[0].amp.stage, Stage::Silent);
    }
}

#[test]
fn mode_crossfade_retargets_its_current_mix_without_a_jump() {
    let mut blend = ModeBlend::<4>::new(2);
    blend.set(0);
    for _ in 0..60 {
        blend.advance();
    }
    let current = blend.weights;
    blend.set(3);
    assert_eq!(blend.weights, current);
    for _ in 0..SMOOTH_SAMPLES {
        blend.advance();
        assert!((blend.weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(
            blend
                .weights
                .iter()
                .all(|weight| (0.0..=1.0).contains(weight))
        );
    }
    assert_eq!(blend.weights, [0.0, 0.0, 0.0, 1.0]);
}

fn set_route(instance: &mut PluginInstance, route: usize, source: u32, target: u32, amount: f64) {
    let base = MATRIX_BASE + route as u32 * MATRIX_STRIDE;
    set(instance, base + MATRIX_SOURCE, f64::from(source));
    set(instance, base + MATRIX_TARGET, f64::from(target));
    set(instance, base + MATRIX_AMOUNT, amount);
}

fn matrix_patch() -> PluginInstance {
    let mut instance = BuiltinKind::Oscil.instance();
    set(&mut instance, GAIN, -18.0);
    set(&mut instance, ATTACK, 0.001);
    set(&mut instance, FILTER_ATTACK, 0.001);
    set(&mut instance, FILTER_SUSTAIN, 0.7);
    set(&mut instance, LFO_RATE, 8.0);
    set(&mut instance, FILTER_CUTOFF, 1200.0);
    for osc in 0..3 {
        let base = OSC_BASE + osc * OSC_STRIDE;
        set(&mut instance, base + OSC_SHAPE, 3.0);
        set(&mut instance, base + OSC_LEVEL, -12.0);
    }
    instance
}

fn render_note(instance: &PluginInstance, frames: usize) -> Vec<Frame> {
    let mut synth = Oscil::new(instance);
    synth.event(on(1, 60));
    let mut audio = vec![[0.0; 2]; frames];
    synth.process(&mut audio, context());
    audio
}

#[test]
fn disabled_or_zero_matrix_preserves_legacy_audio_and_direct_modulation_routes() {
    let mut legacy = matrix_patch();
    set(&mut legacy, FILTER_ENVELOPE, 2.0);
    set(&mut legacy, LFO_PITCH, 34.0);
    set(&mut legacy, LFO_FILTER, 1.5);
    legacy.parameters.retain(|p| p.id < MATRIX_BASE);
    let expected = render_note(&legacy, 4096);
    let mut current = matrix_patch();
    for parameter in &legacy.parameters {
        current
            .parameters
            .iter_mut()
            .find(|p| p.id == parameter.id)
            .unwrap()
            .value = parameter.value;
    }
    for (source, amount) in [(0, 1.0), (0, -1.0), (1, 0.0), (2, 0.0), (3, 0.0)] {
        for route in 0..MATRIX_ROUTES {
            set_route(&mut current, route, source, route as u32, amount);
        }
        assert_eq!(render_note(&current, 4096), expected);
    }
}

#[test]
fn every_matrix_source_and_destination_changes_audio_with_both_depth_polarities() {
    let plain = matrix_patch();
    let expected = render_note(&plain, 2048);
    for source in 0..=3 {
        for target in 0..17 {
            for amount in [-1.0, 1.0] {
                let mut instance = plain.clone();
                set_route(&mut instance, 0, source, target, amount);
                let audio = render_note(&instance, 2048);
                assert!(audio.iter().flatten().all(|x| x.is_finite()));
                if source == 0 {
                    assert_eq!(audio, expected);
                } else {
                    let difference: f64 = audio
                        .iter()
                        .zip(&expected)
                        .flat_map(|(a, b)| a.iter().zip(b))
                        .map(|(a, b)| f64::from((a - b).abs()))
                        .sum();
                    assert!(
                        difference > 0.01,
                        "source {source}, target {target}, amount {amount}: route has no audible effect"
                    );
                }
            }
        }
    }
}

#[test]
fn six_routes_at_extreme_depth_remain_finite_for_all_targets() {
    for target in 0..17 {
        for amount in [-1.0, 1.0] {
            let mut instance = matrix_patch();
            for route in 0..MATRIX_ROUTES {
                set_route(&mut instance, route, (route % 3 + 1) as u32, target, amount);
            }
            let mut synth = Oscil::new(&instance);
            for note in 0..32 {
                synth.event(on(note, 36 + note as u8 * 2));
            }
            let mut output = [[0.0; 2]; 1024];
            synth.process(&mut output, context());
            assert!(
                output
                    .iter()
                    .flatten()
                    .all(|x| x.is_finite() && x.abs() < 1000.0),
                "target {target} amount {amount}"
            );
        }
    }
}

#[test]
fn matrix_envelopes_are_per_voice_and_depth_is_bipolar() {
    let mut instance = matrix_patch();
    set_route(&mut instance, 0, 1, 0, 1.0);
    set(&mut instance, SUSTAIN, 1.0);
    let mut synth = Oscil::new(&instance);
    synth.event(on(1, 60));
    synth.process(&mut [[0.0; 2]; 1000], context());
    let previous_phase = synth.voices[0].phases[0];
    synth.event(on(2, 60));
    synth.process(&mut [[0.0; 2]; 1], context());
    let first_step = (synth.voices[0].phases[0] - previous_phase).rem_euclid(1.0);
    let second_step = synth.voices[1].phases[0];
    assert!(first_step > second_step * 3.5);

    for amount in [-1.0, 1.0] {
        set_route(&mut instance, 0, 3, 14, amount);
        let synth = Oscil::new(&instance);
        let contribution = matrix_displacements(
            &synth.params,
            &synth.matrix_sources,
            &synth.matrix_targets,
            0.25,
            0.75,
            -0.5,
        );
        assert_eq!(contribution[14], -4.0 * amount);
    }
}

#[test]
fn matrix_source_and_target_switches_fade_without_restarting_on_repeated_automation() {
    for (offset, physical) in [(MATRIX_SOURCE, 2.0), (MATRIX_TARGET, 7.0)] {
        let mut instance = matrix_patch();
        set_route(&mut instance, 0, 3, 14, 0.4);
        let mut a = Oscil::new(&instance);
        let mut b = Oscil::new(&instance);
        a.event(on(1, 60));
        b.event(on(1, 60));
        a.process(&mut [[0.0; 2]; 1024], context());
        b.process(&mut [[0.0; 2]; 1024], context());
        let id = MATRIX_BASE + offset;
        let spec = oscil_parameters().into_iter().find(|p| p.id == id).unwrap();
        let value = spec.normalize(physical);
        b.set_automation_parameter(id, value);
        let mut x = [[0.0; 2]; 1];
        let mut y = x;
        a.process(&mut x, context());
        b.process(&mut y, context());
        assert_eq!(x, y, "route switch {id} jumped on its first frame");
        for _ in 1..SMOOTH_SAMPLES {
            b.set_automation_parameter(id, value);
            b.process(&mut y, context());
        }
        assert_eq!(b.matrix_sources[0].remaining, 0);
        assert_eq!(b.matrix_targets[0].remaining, 0);
    }
}

#[test]
fn matrix_automation_is_independent_of_processing_block_boundaries() {
    let mut instance = matrix_patch();
    set_route(&mut instance, 0, 1, 0, 0.5);
    set_route(&mut instance, 1, 2, 15, -0.5);
    let mut a = Oscil::new(&instance);
    let mut b = Oscil::new(&instance);
    a.event(on(1, 60));
    b.event(on(1, 60));
    for (id, normalized) in [
        (MATRIX_BASE, 1.0),
        (MATRIX_BASE + 1, 0.5),
        (MATRIX_BASE + 2, 0.1),
    ] {
        a.set_automation_parameter(id, normalized);
        b.set_automation_parameter(id, normalized);
        let mut x = [[0.0; 2]; 512];
        let mut y = x;
        a.process(&mut x, context());
        for chunk in y.chunks_mut(7) {
            b.process(chunk, context());
        }
        assert_eq!(x, y);
    }
}
