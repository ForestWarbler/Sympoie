use super::*;
use crate::builtin::BuiltinKind;

fn context() -> PluginProcessContext {
    PluginProcessContext {
        frame: 0,
        bpm: 120.0,
        playing: true,
        quality: crate::plugin::RenderQuality::Realtime,
    }
}
fn on(id: u64, key: u8) -> NoteEvent {
    NoteEvent::On {
        id,
        key,
        velocity: 127,
        channel: 0,
        slide_frames: 0,
    }
}
fn set(instance: &mut PluginInstance, id: u32, physical: f64) {
    let spec = slicer_parameters()
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
fn set_slices(instance: &mut PluginInstance, starts: &[f64]) {
    set(instance, SLICE_COUNT, starts.len() as f64);
    for (slice, &start) in starts.iter().enumerate() {
        set(instance, SLICE_BASE + slice as u32, start);
    }
}
/// Sampler-format state holding `audio` at 48 kHz.
fn state(audio: &[Frame]) -> Vec<u8> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "sympoie-slicer-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut wav = b"RIFF".to_vec();
    let data_len = audio.len() as u32 * 8;
    wav.extend((36 + data_len).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16_u32.to_le_bytes());
    wav.extend(3_u16.to_le_bytes());
    wav.extend(2_u16.to_le_bytes());
    wav.extend(48000_u32.to_le_bytes());
    wav.extend((48000_u32 * 8).to_le_bytes());
    wav.extend(8_u16.to_le_bytes());
    wav.extend(32_u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(data_len.to_le_bytes());
    for frame in audio {
        wav.extend(frame[0].to_le_bytes());
        wav.extend(frame[1].to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    let state = sample_state_from_wav(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    state
}
/// Decaying noise bursts starting at each of `hits` (frames), over `length` frames.
fn drum_loop(length: usize, hits: &[usize]) -> Vec<f32> {
    let mut noise = 0x1234_5678_u32;
    let mut audio = vec![0.0_f32; length];
    for &hit in hits {
        for (offset, sample) in audio[hit..].iter_mut().take(9600).enumerate() {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let white = noise as f32 / u32::MAX as f32 * 2.0 - 1.0;
            *sample += white * 0.8 * (-(offset as f32) / 1200.0).exp();
        }
    }
    audio
}
fn settings(detection: SliceDetection) -> SliceSettings {
    SliceSettings {
        detection,
        sensitivity: 0.5,
        min_length_seconds: 0.05,
        divisions: 8,
    }
}
fn render(processor: &mut dyn PluginProcessor, frames: usize) -> Vec<Frame> {
    let mut audio = vec![[0.0; 2]; frames];
    processor.process(&mut audio, context());
    audio
}

#[test]
fn transient_detection_finds_each_hit_just_before_its_attack() {
    let hits = [0, 12000, 24000, 30000, 36000];
    let audio = drum_loop(48000, &hits);
    let starts = detect_slices(&audio, 48000, settings(SliceDetection::Transient));
    assert_eq!(starts.len(), hits.len(), "{starts:?}");
    for (start, hit) in starts.iter().zip(hits) {
        assert!(
            *start <= hit + 4 && hit <= start + 96,
            "slice at {start} for hit at {hit}"
        );
    }
}

#[test]
fn minimum_length_merges_hits_that_are_too_close() {
    let audio = drum_loop(48000, &[0, 2400, 24000]);
    let mut close = settings(SliceDetection::Transient);
    close.min_length_seconds = 0.02;
    assert_eq!(detect_slices(&audio, 48000, close).len(), 3);
    close.min_length_seconds = 0.1;
    assert_eq!(detect_slices(&audio, 48000, close).len(), 2);
}

#[test]
fn volume_detection_starts_slices_where_sound_leaves_silence() {
    let mut audio = vec![0.0_f32; 48000];
    for (from, to) in [(4800, 12000), (24000, 36000)] {
        for (index, sample) in audio[from..to].iter_mut().enumerate() {
            *sample = 0.5 * (index as f32 * 0.05).sin();
        }
    }
    let starts = detect_slices(&audio, 48000, settings(SliceDetection::Volume));
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert!((4790..=4800).contains(&starts[0]), "{starts:?}");
    assert!((23990..=24000).contains(&starts[1]), "{starts:?}");
}

#[test]
fn grid_divides_evenly_and_silence_keeps_one_slice() {
    let audio = vec![0.0_f32; 8000];
    assert_eq!(
        detect_slices(&audio, 48000, settings(SliceDetection::Grid)),
        [0, 1000, 2000, 3000, 4000, 5000, 6000, 7000]
    );
    assert_eq!(
        detect_slices(&audio, 48000, settings(SliceDetection::Transient)),
        [0]
    );
    assert_eq!(
        detect_slices(&[], 48000, settings(SliceDetection::Volume)),
        [0]
    );
}

#[test]
fn detection_never_returns_more_slices_than_keys_it_can_map() {
    let hits: Vec<usize> = (0..100).map(|hit| hit * 4800).collect();
    let audio = drum_loop(480000 + 9600, &hits);
    let mut dense = settings(SliceDetection::Transient);
    dense.min_length_seconds = 0.01;
    let starts = detect_slices(&audio, 48000, dense);
    assert_eq!(starts.len(), MAX_SLICES);
    assert!(starts.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn analysis_reads_sample_state_and_returns_normalized_starts() {
    let audio = drum_loop(48000, &[0, 24000]);
    let frames: Vec<Frame> = audio.iter().map(|&sample| [sample, sample]).collect();
    let starts = analyze_slices(&state(&frames), settings(SliceDetection::Transient)).unwrap();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0], 0.0);
    assert!((starts[1] - 0.5).abs() < 0.002, "{starts:?}");
    assert!(analyze_slices(&[], settings(SliceDetection::Grid)).is_err());
}

#[test]
fn regions_end_at_the_next_later_slice_whatever_the_order() {
    assert_eq!(
        slice_regions(&[0.0, 0.5, 0.25]),
        [(0.0, 0.25), (0.5, 1.0), (0.25, 0.5)]
    );
    let mut instance = BuiltinKind::Slicer.instance();
    assert_eq!(slice_starts(&instance), [0.0]);
    set_slices(&mut instance, &[0.0, 0.5]);
    assert_eq!(slice_starts(&instance), [0.0, 0.5]);
}

/// Four slices of constant, distinct levels, so the output shows which slice played.
fn stepped() -> PluginInstance {
    let audio: Vec<Frame> = (0..4800)
        .map(|frame| {
            let level = 0.1 * (frame / 1200 + 1) as f32;
            [level, level]
        })
        .collect();
    let mut instance = BuiltinKind::Slicer.instance();
    instance.state = state(&audio);
    set_slices(&mut instance, &[0.0, 0.25, 0.5, 0.75]);
    set(&mut instance, SLICE_FADE, 0.0);
    set(&mut instance, GAIN, 0.0);
    set(&mut instance, VELOCITY, 0.0);
    instance
}

#[test]
fn each_key_from_the_root_plays_its_own_slice_to_its_end() {
    let instance = stepped();
    for (key, level) in [(36, 0.1), (37, 0.2), (39, 0.4)] {
        let mut slicer = create_slicer(&instance).unwrap();
        slicer.event(on(1, key));
        slicer.event(NoteEvent::Off { id: 1 });
        let audio = render(slicer.as_mut(), 1400);
        assert!(
            (audio[600][0] - level).abs() < 0.01,
            "key {key}: {}",
            audio[600][0]
        );
        assert!(audio[1300][0].abs() < 1e-6, "one-shot ends with its slice");
    }
    for key in [35, 40] {
        let mut slicer = create_slicer(&instance).unwrap();
        slicer.event(on(1, key));
        assert!(render(slicer.as_mut(), 64).iter().all(|f| f[0] == 0.0));
    }
}

#[test]
fn gate_mode_releases_and_loop_mode_repeats_the_slice() {
    let mut instance = stepped();
    set(&mut instance, SLICE_MODE, 1.0);
    set(&mut instance, RELEASE, 0.001);
    let mut slicer = create_slicer(&instance).unwrap();
    slicer.event(on(1, 36));
    render(slicer.as_mut(), 200);
    slicer.event(NoteEvent::Off { id: 1 });
    let audio = render(slicer.as_mut(), 200);
    assert!(audio[199][0].abs() < 1e-6);

    set(&mut instance, SLICE_MODE, 2.0);
    let mut slicer = create_slicer(&instance).unwrap();
    slicer.event(on(1, 37));
    let audio = render(slicer.as_mut(), 3000);
    assert!((audio[2900][0] - 0.2).abs() < 0.01, "{}", audio[2900][0]);
}

#[test]
fn choke_cuts_the_previous_slice() {
    let mut instance = stepped();
    let mut slicer = create_slicer(&instance).unwrap();
    slicer.event(on(1, 36));
    slicer.event(on(2, 37));
    let open = render(slicer.as_mut(), 600);
    assert!((open[500][0] - 0.3).abs() < 0.01);

    set(&mut instance, SLICE_CHOKE, 1.0);
    let mut slicer = create_slicer(&instance).unwrap();
    slicer.event(on(1, 36));
    slicer.event(on(2, 37));
    let choked = render(slicer.as_mut(), 600);
    assert!((choked[500][0] - 0.2).abs() < 0.01, "{}", choked[500][0]);
}

#[test]
fn one_shot_tail_covers_the_slice_and_other_modes_have_none() {
    let mut instance = stepped();
    assert_eq!(
        slicer_note_tail_frames_with_automation(&instance, 37, &[]).unwrap(),
        1200
    );
    assert_eq!(
        slicer_note_tail_frames_with_automation(&instance, 50, &[]).unwrap(),
        0
    );
    set(&mut instance, SLICE_TRANSPOSE, 12.0);
    assert_eq!(
        slicer_note_tail_frames_with_automation(&instance, 37, &[]).unwrap(),
        600
    );
    set(&mut instance, SLICE_MODE, 1.0);
    assert_eq!(
        slicer_note_tail_frames_with_automation(&instance, 37, &[]).unwrap(),
        0
    );
}
