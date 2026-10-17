use super::*;
use crate::builtin::BuiltinKind;
use std::io::Cursor;

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
    let spec = sampler_parameters()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap();
    instance
        .parameters
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .value = spec.normalize(physical);
}
fn state(audio: &[Frame], rate: u32) -> Vec<u8> {
    let mut state = MAGIC.to_vec();
    state.extend(rate.to_le_bytes());
    state.extend((audio.len() as u32).to_le_bytes());
    state.extend(8_u32.to_le_bytes());
    state.extend(0_u64.to_le_bytes());
    state.extend(b"test.wav");
    for frame in audio {
        for sample in frame {
            state.extend(sample.to_le_bytes());
        }
    }
    seal_state(&mut state);
    state
}
fn instance(length: usize) -> PluginInstance {
    let mut instance = BuiltinKind::Sampler.instance();
    instance.state = state(&vec![[0.5, -0.25]; length], 48000);
    set(&mut instance, ATTACK, 0.0001);
    set(&mut instance, RELEASE, 0.001);
    instance
}
fn wav(encoding: u16, bits: u16, channels: u16, rate: u32, audio: &[u8]) -> Vec<u8> {
    let align = bits / 8 * channels;
    let mut body = b"WAVEfmt ".to_vec();
    body.extend(16_u32.to_le_bytes());
    body.extend(encoding.to_le_bytes());
    body.extend(channels.to_le_bytes());
    body.extend(rate.to_le_bytes());
    body.extend((rate * u32::from(align)).to_le_bytes());
    body.extend(align.to_le_bytes());
    body.extend(bits.to_le_bytes());
    body.extend(b"data");
    body.extend((audio.len() as u32).to_le_bytes());
    body.extend(audio);
    if !audio.len().is_multiple_of(2) {
        body.push(0);
    }
    let mut result = b"RIFF".to_vec();
    result.extend((body.len() as u32).to_le_bytes());
    result.extend(body);
    result
}

#[test]
fn import_pcm16_24_32_and_float_at_common_rates_preserves_audio() {
    for rate in [44100, 48000, 96000] {
        for (encoding, bits, bytes) in [
            (1, 8, vec![0, 192]),
            (1, 16, vec![0, 128, 0, 64]),
            (1, 24, vec![0, 0, 128, 0, 0, 64]),
            (1, 32, vec![0, 0, 0, 128, 0, 0, 0, 64]),
            (
                3,
                32,
                [(-1_f32).to_le_bytes(), 0.5_f32.to_le_bytes()].concat(),
            ),
            (
                3,
                64,
                [(-1_f64).to_le_bytes(), 0.5_f64.to_le_bytes()].concat(),
            ),
        ] {
            let bytes = wav(encoding, bits, 1, rate, &bytes);
            let decoded =
                decode_wav(&mut Cursor::new(&bytes), bytes.len() as u64, "mono.wav").unwrap();
            let parsed = header(&decoded).unwrap();
            assert_eq!(parsed.rate, rate);
            assert_eq!(parsed.frames, 2);
            assert_eq!(decode_frame(&parsed.audio[..8]).unwrap(), [-1.0; 2]);
            assert_eq!(decode_frame(&parsed.audio[8..]).unwrap(), [0.5; 2]);
        }
    }
}

#[test]
fn state_is_self_contained_and_preview_reports_both_channels() {
    let instance = instance(1024);
    let preview = sample_preview(&instance.state).unwrap();
    assert_eq!(preview.name, "test.wav");
    assert_eq!(preview.frames, 1024);
    assert_eq!(preview.sample_rate, 48000);
    assert!(preview.peaks.iter().all(|p| *p == [-0.25, 0.5]));
    let mut a = Sampler::new(&instance).unwrap();
    let mut b = Sampler::new(&instance.clone()).unwrap();
    a.event(on(1, 60));
    b.event(on(1, 60));
    let mut x = [[0.0; 2]; 1024];
    let mut y = x;
    a.process(&mut x, context());
    for chunk in y.chunks_mut(17) {
        b.process(chunk, context());
    }
    assert_eq!(x, y);
    assert!(x.iter().any(|sample| sample[0] > 0.1 && sample[1] < -0.05));
}

#[test]
fn state_and_import_reject_truncation_corruption_nonfinite_and_unsupported_audio() {
    let original = instance(10).state;
    for length in [0, 1, 7, 19, 20, original.len() - 1] {
        if length > 0 {
            assert!(sample_preview(&original[..length]).is_err());
        }
    }
    let mut bad = original.clone();
    bad[8..12].copy_from_slice(&0_u32.to_le_bytes());
    assert!(sample_preview(&bad).is_err());
    let mut bad = original;
    bad[36..40].copy_from_slice(&f32::NAN.to_le_bytes());
    seal_state(&mut bad);
    assert!(sample_preview(&bad).is_err());
    for bytes in [
        wav(1, 16, 3, 48000, &[0; 6]),
        wav(3, 32, 1, 48000, &f32::INFINITY.to_le_bytes()),
        wav(1, 16, 1, 48000, &[1]),
    ] {
        assert!(decode_wav(&mut Cursor::new(&bytes), bytes.len() as u64, "bad.wav").is_err());
    }
    let mut bytes = wav(1, 16, 1, 48000, &[0, 0]);
    bytes.pop();
    assert!(decode_wav(&mut Cursor::new(&bytes), bytes.len() as u64, "bad.wav").is_err());
}

#[test]
fn root_transpose_tune_and_source_sample_rate_determine_playback_speed() {
    for (rate, key, root, transpose, tune, expected) in [
        (48000_u32, 60, 60.0, 0.0, 0.0, 100.0),
        (44100, 60, 60.0, 0.0, 0.0, 91.875),
        (48000, 72, 60.0, 0.0, 0.0, 200.0),
        (48000, 60, 72.0, 12.0, 0.0, 100.0),
        (
            48000,
            60,
            60.0,
            0.0,
            100.0,
            100.0 * 2.0_f64.powf(1.0 / 12.0),
        ),
    ] {
        let mut instance = instance(10000);
        instance.state[8..12].copy_from_slice(&rate.to_le_bytes());
        seal_state(&mut instance.state);
        set(&mut instance, SAMPLE_ROOT, root);
        set(&mut instance, SAMPLE_TRANSPOSE, transpose);
        set(&mut instance, SAMPLE_TUNE, tune);
        let mut sampler = Sampler::new(&instance).unwrap();
        sampler.event(on(1, key));
        sampler.process(&mut [[0.0; 2]; 100], context());
        assert!((sampler.voices[0].position - expected).abs() < 1e-8);
    }
}

#[test]
fn gate_releases_but_one_shot_continues_until_end() {
    for mode in [0.0, 1.0, 2.0] {
        let mut instance = instance(1000);
        set(&mut instance, SAMPLE_MODE, mode);
        let mut sampler = Sampler::new(&instance).unwrap();
        sampler.event(on(1, 60));
        sampler.process(&mut [[0.0; 2]; 100], context());
        sampler.event(NoteEvent::Off { id: 1 });
        let mut out = [[0.0; 2]; 100];
        sampler.process(&mut out, context());
        if mode == 0.0 {
            assert!(out[99][0] > 0.1);
        } else {
            assert_eq!(out[99], [0.0; 2]);
        }
        sampler.process(&mut [[0.0; 2]; 1000], context());
        assert!(
            sampler
                .voices
                .iter()
                .all(|v| v.envelope.stage == Stage::Silent)
        );
    }
}

#[test]
fn trim_reverse_and_tiny_loops_are_bounded_at_high_playback_rates() {
    for reverse in [0.0, 1.0] {
        for crossfade in [0.0, 5.0] {
            let mut instance = instance(1000);
            set(&mut instance, SAMPLE_START, 0.2);
            set(&mut instance, SAMPLE_END, 0.8);
            set(&mut instance, SAMPLE_LOOP_START, 0.3);
            set(&mut instance, SAMPLE_LOOP_END, 0.301);
            set(&mut instance, SAMPLE_MODE, 2.0);
            set(&mut instance, SAMPLE_REVERSE, reverse);
            set(&mut instance, SAMPLE_CROSSFADE, crossfade);
            let mut sampler = Sampler::new(&instance).unwrap();
            sampler.event(on(1, 127));
            assert_eq!(
                sampler.voices[0].position,
                if reverse > 0.0 { 799.0 } else { 200.0 }
            );
            let mut output = [[0.0; 2]; 4096];
            sampler.process(&mut output, context());
            assert!(output.iter().flatten().all(|x| x.is_finite()));
            assert_ne!(sampler.voices[0].envelope.stage, Stage::Silent);
        }
    }
}

#[test]
fn empty_and_single_frame_samples_and_invalid_trim_order_are_safe() {
    let mut empty = create_sampler(&BuiltinKind::Sampler.instance()).unwrap();
    empty.event(on(1, 60));
    let mut audio = [[1.0; 2]; 128];
    empty.process(&mut audio, context());
    assert_eq!(audio, [[0.0; 2]; 128]);
    let mut instance = instance(1);
    set(&mut instance, SAMPLE_START, 1.0);
    set(&mut instance, SAMPLE_END, 0.0);
    let mut sampler = create_sampler(&instance).unwrap();
    sampler.event(on(1, 60));
    sampler.process(&mut audio, context());
    assert!(audio.iter().flatten().all(|x| x.is_finite()));
    assert_eq!(audio[127], [0.0; 2]);
}

#[test]
fn one_shot_tail_reflects_key_tuning_rate_and_trim() {
    let mut instance = instance(48000);
    set(&mut instance, SAMPLE_MODE, 0.0);
    assert_eq!(sampler_note_tail_frames(&instance, 60).unwrap(), 48000);
    assert_eq!(sampler_note_tail_frames(&instance, 48).unwrap(), 96000);
    set(&mut instance, SAMPLE_START, 0.25);
    set(&mut instance, SAMPLE_END, 0.75);
    assert_eq!(sampler_note_tail_frames(&instance, 60).unwrap(), 24000);
    set(&mut instance, SAMPLE_MODE, 1.0);
    assert_eq!(sampler_note_tail_frames(&instance, 60).unwrap(), 0);
}

#[test]
fn sampler_polyphony_and_same_key_ids_release_independently() {
    let mut instance = instance(128);
    set(&mut instance, SAMPLE_MODE, 2.0);
    let mut sampler = Sampler::new(&instance).unwrap();
    for id in 0..40 {
        sampler.event(on(id, 60));
    }
    assert_eq!(
        sampler
            .voices
            .iter()
            .filter(|v| v.envelope.stage != Stage::Silent)
            .count(),
        VOICES
    );
    sampler.process(&mut [[0.0; 2]; 64], context());
    let id = sampler.voices[1].id;
    sampler.event(NoteEvent::Off { id });
    sampler.process(&mut [[0.0; 2]; 128], context());
    assert_eq!(
        sampler
            .voices
            .iter()
            .filter(|v| v.envelope.stage != Stage::Silent)
            .count(),
        VOICES - 1
    );
}

#[test]
fn high_transpositions_reject_aliases_and_octave_selection_is_continuous() {
    let resampler = Resampler::new();
    let render = |hz: f64, step: f64| {
        let audio: Vec<Frame> = (0..48000)
            .map(|n| [(TAU * hz * f64::from(n) / 48000.0).sin() as f32; 2])
            .collect();
        let octaves = prepare_octaves(&audio).unwrap();
        let mut energy = 0.0;
        for n in 100..1500 {
            let sample = resampler.interpolate_prepared(
                (&audio, &octaves),
                f64::from(n) * step,
                0..audio.len(),
                step,
                false,
            );
            energy += sample[0] * sample[0];
        }
        energy.sqrt()
    };
    for step in [2.0, 4.0, 16.0] {
        let pass = render(500.0, step);
        let stop = render(24000.0 / step * 2.1, step);
        assert!(
            stop < pass * 0.005,
            "step {step}: alias/pass = {}",
            stop / pass
        );
    }
    let audio: Vec<Frame> = (0..48000)
        .map(|n| [(TAU * 9000.0 * f64::from(n) / 48000.0).sin() as f32; 2])
        .collect();
    let octaves = prepare_octaves(&audio).unwrap();
    for rate in [2.0, 4.0, 8.0] {
        let a = resampler.interpolate_prepared(
            (&audio, &octaves),
            1234.7,
            0..audio.len(),
            rate - 1e-7,
            false,
        );
        let b = resampler.interpolate_prepared(
            (&audio, &octaves),
            1234.7,
            0..audio.len(),
            rate + 1e-7,
            false,
        );
        assert!((a[0] - b[0]).abs() < 1e-6);
    }
}

#[test]
fn waveform_fingerprint_changes_for_same_sized_audio_and_rejects_corruption() {
    let first = state(&[[0.1; 2]; 10], 48000);
    let second = state(&[[0.2; 2]; 10], 48000);
    assert_eq!(first.len(), second.len());
    assert_ne!(
        sample_state_fingerprint(&first),
        sample_state_fingerprint(&second)
    );
    let mut corrupted = first;
    corrupted[HEADER_BYTES + 8] ^= 1;
    assert!(sample_preview(&corrupted).unwrap_err().contains("checksum"));
}

#[test]
fn tail_extrema_include_pitch_trim_and_mode_automation() {
    use crate::automation::{AutomationLane, AutomationPoint, AutomationTarget};
    let mut instance = instance(48000);
    instance.id = 5;
    set(&mut instance, SAMPLE_START, 0.5);
    let lane = |id, physical| {
        let spec = sampler_parameters()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap();
        AutomationLane::new(
            AutomationTarget::PluginParameter {
                plugin: 5,
                parameter: id,
            },
            vec![AutomationPoint {
                tick: 0,
                value: spec.normalize(physical),
            }],
        )
        .unwrap()
    };
    let lanes = [
        lane(SAMPLE_MODE, 0.0),
        lane(SAMPLE_TRANSPOSE, -12.0),
        lane(SAMPLE_ROOT, 72.0),
        lane(SAMPLE_START, 0.0),
    ];
    assert_eq!(
        sampler_note_tail_frames_with_automation(&instance, 60, &lanes).unwrap(),
        192000
    );
    let lanes = lanes.map(|lane| lane.with_enabled(false));
    assert_eq!(
        sampler_note_tail_frames_with_automation(&instance, 60, &lanes).unwrap(),
        0
    );
}

#[test]
fn opening_filter_remains_continuous_and_retains_its_history() {
    let mut instance = instance(10000);
    set(&mut instance, SAMPLE_MODE, 2.0);
    let mut sampler = Sampler::new(&instance).unwrap();
    sampler.event(on(1, 60));
    let mut output = [[0.0; 2]; 512];
    sampler.process(&mut output, context());
    let before = output[511][0];
    let spec = sampler_parameters()
        .into_iter()
        .find(|p| p.id == SAMPLE_CUTOFF)
        .unwrap();
    sampler.set_automation_parameter(SAMPLE_CUTOFF, spec.normalize(19998.0));
    sampler.process(&mut output, context());
    assert!((before - output[0][0]).abs() < 0.0001);
}
