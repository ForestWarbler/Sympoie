use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::{RenderError, Renderer, render_block};
use sympoie::time::{SnapGrid, TimeError};
use sympoie::track::{AudioOutput, MidiTrackId, MixerSettings};
use sympoie::transport::{PlaybackState, Transport};

fn note(start_tick: u64, duration_ticks: u64, key: u8) -> MidiNote {
    MidiNote {
        start_tick,
        duration_ticks,
        key,
        velocity: 100,
        channel: 7,
    }
}

fn fixture(notes: Vec<MidiNote>, release: f32) -> (Project, MidiTrackId) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_midi_track("Subtractive");
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings {
                waveform: Waveform::Square,
                attack_seconds: 0.0,
                decay_seconds: 0.0,
                sustain: 1.0,
                release_seconds: release,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    let length_ticks = notes
        .iter()
        .map(|note| note.start_tick + note.duration_ticks)
        .max()
        .unwrap_or(1000);
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 73,
                length_ticks,
                notes,
            },
        )
        .unwrap();
    (project, track)
}

fn full_render(project: &Project, block_size: usize) -> Vec<Frame> {
    let mut renderer = Renderer::new(project).unwrap();
    let mut frames = vec![[99.0; 2]; renderer.duration_frames() as usize + 37];
    for (index, block) in frames.chunks_mut(block_size).enumerate() {
        renderer.render_block((index * block_size) as u64, block);
    }
    frames
}

#[test]
fn midi_timing_is_clip_relative_and_rendering_is_independent_of_block_size() {
    let (project, _) = fixture(
        vec![note(257, 601, 60), note(0, 500, 64), note(499, 170, 67)],
        0.006,
    );
    let expected = full_render(&project, 8192);
    assert!(expected[..73].iter().all(|frame| *frame == [0.0; 2]));
    assert!(expected[74..573].iter().any(|frame| frame[0] != 0.0));
    let end = project.duration_frames().unwrap() as usize;
    assert!(expected[end..].iter().all(|frame| *frame == [0.0; 2]));
    for size in [1, 7, 73, 255, 256, 257, 511, 1024] {
        assert_eq!(full_render(&project, size), expected, "block size {size}");
    }
}

#[test]
fn same_key_overlapping_notes_do_not_release_each_other() {
    let (both, _) = fixture(vec![note(0, 500, 60), note(0, 900, 60)], 0.0);
    let (long, _) = fixture(vec![note(0, 900, 60)], 0.0);
    let both = full_render(&both, 47);
    let long = full_render(&long, 53);
    assert_eq!(&both[573..], &long[573..]);
    assert!(both[573..973].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn midi_mixes_with_audio_through_nested_groups_and_master() {
    let (reference, _) = fixture(vec![note(0, 2000, 69)], 0.002);
    let reference = full_render(&reference, 333);
    let (mut project, midi) = fixture(vec![note(0, 2000, 69)], 0.002);
    let inner = project.add_group_track("Inner");
    let outer = project.add_group_track("Outer");
    project.set_track_group(midi, Some(inner)).unwrap();
    project.set_track_group(inner, Some(outer)).unwrap();
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(0.5, -0.5, AudioOutput::Group(inner)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            inner,
            MixerSettings::new(0.5, 0.0, AudioOutput::Group(outer)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            outer,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.5, 0.0, AudioOutput::Output).unwrap())
        .unwrap();
    let audio = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.1, 0.2]; 1000]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                start_tick: 0,
                source_start: 0,
                frame_count: 1000,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    let actual = full_render(&project, 53);
    for (index, (actual, reference)) in actual.iter().zip(reference).enumerate() {
        let audio = if index < 1000 { [0.05, 0.1] } else { [0.0; 2] };
        assert!((actual[0] - (reference[0] * 0.0625 + audio[0])).abs() < 1e-7);
        assert!((actual[1] - (reference[1] * 0.03125 + audio[1])).abs() < 1e-7);
    }
}

#[test]
fn transport_plays_release_tail_before_stopping() {
    let (project, track) = fixture(vec![note(0, 1000, 60)], 0.01);
    let release = project
        .midi_track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .release_frames();
    assert_eq!(project.audio_duration_frames(), 0);
    assert_eq!(project.duration_frames().unwrap(), 1073 + release);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut frames = vec![[99.0; 2]; transport.duration_frames() as usize + 99];
    transport.process_block(&mut frames);
    assert!(frames[1073..1200].iter().any(|frame| frame[0] != 0.0));
    assert!(
        frames[transport.duration_frames() as usize..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert_eq!(transport.position_frame(), transport.duration_frames());
}

#[test]
fn seek_retriggers_held_notes_and_drops_old_release_tails() {
    let (project, _) = fixture(vec![note(0, 1000, 60), note(500, 1000, 67)], 0.005);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 1000]);
    transport.seek_frame(800);
    let mut expected = [[0.0; 2]; 200];
    Renderer::new(&project)
        .unwrap()
        .render_block(800, &mut expected);
    let mut actual = [[0.0; 2]; 200];
    transport.process_block(&mut actual);
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| frame[0] != 0.0));

    transport.seek_frame(1573);
    transport.process_block(&mut actual);
    assert_eq!(actual, [[0.0; 2]; 200]);
    transport.stop();
    actual.fill([99.0; 2]);
    transport.process_block(&mut actual);
    assert_eq!(actual, [[0.0; 2]; 200]);
}

#[test]
fn pause_keeps_the_frame_and_stop_releases_runtime_state() {
    let (project, _) = fixture(vec![note(0, 4000, 60)], 0.1);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut scratch = [[0.0; 2]; 777];
    transport.process_block(&mut scratch);
    transport.pause();
    transport.process_block(&mut scratch);
    assert_eq!(scratch, [[0.0; 2]; 777]);
    assert_eq!(transport.position_frame(), 777);
    transport.play();
    transport.process_block(&mut scratch);
    assert_eq!(scratch.as_slice(), &full_render(&project, 500)[777..1554]);
    transport.stop();
    transport.play();
    transport.process_block(&mut scratch);
    let mut expected = [[0.0; 2]; 777];
    Renderer::new(&project)
        .unwrap()
        .render_block(1554, &mut expected);
    assert_eq!(scratch, expected);
}

#[test]
fn loop_boundaries_reset_voices_and_repeat_identically_across_blocks() {
    let (project, _) = fixture(vec![note(0, 1600, 60), note(499, 500, 64)], 0.3);
    let render = |block_size| {
        let mut transport = Transport::new(&project).unwrap();
        transport.set_loop_ticks(400, 1000).unwrap();
        transport.seek_tick(400).unwrap();
        transport.play();
        let mut frames = vec![[0.0; 2]; 2400];
        for block in frames.chunks_mut(block_size) {
            transport.process_block(block);
        }
        frames
    };
    let reference = render(4096);
    assert!(reference.iter().any(|frame| frame[0] != 0.0));
    for iteration in reference.chunks(600) {
        assert_eq!(iteration, &reference[..600]);
    }
    for size in [1, 37, 256, 601] {
        assert_eq!(render(size), reference);
    }
}

#[test]
fn invalid_frame_conversion_is_reported_and_convenience_output_is_cleared() {
    let (mut project, track) = fixture(vec![note(0, 1000, 60)], 0.1);
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: u64::MAX - 1000,
                length_ticks: 100,
                notes: vec![],
            },
        )
        .unwrap();
    project.set_bpm(1.0).unwrap();
    let mut output = [[99.0; 2]; 8];
    assert_eq!(
        render_block(&project, 0, &mut output),
        Err(RenderError::Time(TimeError::ConversionOverflow))
    );
    assert_eq!(output, [[0.0; 2]; 8]);
}

#[test]
fn a_note_shorter_than_one_sample_does_not_leak_a_stuck_voice() {
    let (mut project, _) = fixture(vec![note(0, 1, 60)], 0.3);
    project.set_bpm(3_000_000.0).unwrap();
    assert_eq!(project.duration_frames().unwrap(), 0);
    let mut renderer = Renderer::new(&project).unwrap();
    let mut output = [[99.0; 2]; 512];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]; 512]);
}

fn set_synth(project: &mut Project, track: MidiTrackId, settings: SubtractiveSettings) {
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(settings)))
        .unwrap();
}

fn largest_step(frames: &[Frame]) -> f32 {
    frames
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn instrument_edits_while_playing_keep_notes_sounding_without_a_click() {
    let (mut project, track) = fixture(vec![note(0, 30_000, 12)], 0.05);
    let playing = SubtractiveSettings {
        waveform: Waveform::Sine,
        cutoff_hz: 20_000.0,
        attack_seconds: 0.01,
        gain: 0.8,
        ..SubtractiveSettings::default()
    };
    set_synth(&mut project, track, playing);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut heard = vec![[0.0; 2]; 6_000];
    transport.process_block(&mut heard);
    let steady = largest_step(&heard[2_000..]);

    let mut edited = project.clone();
    edited.rename_track(track, "Renamed").unwrap();
    edited.set_snap_grid(SnapGrid::Bar);
    for step in 1..=20 {
        let settings = SubtractiveSettings {
            cutoff_hz: 20_000.0 - 950.0 * step as f32,
            resonance: 0.7 + 0.2 * step as f32,
            sustain: 0.65 - 0.02 * step as f32,
            attack_seconds: 0.01 * step as f32,
            gain: 0.8 - 0.03 * step as f32,
            ..playing
        };
        set_synth(&mut edited, track, settings);
        assert!(transport.update_instruments(&edited));
        let start = heard.len() - 1;
        heard.resize(heard.len() + 300, [0.0; 2]);
        transport.process_block(&mut heard[start + 1..]);
        assert!(
            largest_step(&heard[start..]) < 2.0 * steady + 1e-3,
            "step {step}: {} vs {steady}",
            largest_step(&heard[start..])
        );
    }
    assert_eq!(transport.position_frame(), heard.len() as u64);
    assert_eq!(transport.state(), PlaybackState::Playing);

    let mut rest = vec![[0.0; 2]; 12_000];
    transport.process_block(&mut rest);
    let mut fresh = Transport::new(&edited).unwrap();
    fresh.play();
    let mut expected = vec![[0.0; 2]; 24_000];
    fresh.process_block(&mut expected);
    for (actual, expected) in rest[6_000..].iter().zip(&expected[18_000..]) {
        assert!((actual[0] - expected[0]).abs() < 1e-6);
    }
}

#[test]
fn instrument_updates_refuse_other_edits_and_extend_the_release_tail() {
    let (project, track) = fixture(vec![note(0, 1000, 60)], 0.01);
    let mut transport = Transport::new(&project).unwrap();
    transport.play();
    let mut heard = vec![[0.0; 2]; 500];
    transport.process_block(&mut heard);

    let settings = |release_seconds| SubtractiveSettings {
        waveform: Waveform::Square,
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds,
        ..SubtractiveSettings::default()
    };
    let mut rejected = Vec::new();
    let mut edit = |change: &dyn Fn(&mut Project)| {
        let mut edited = project.clone();
        set_synth(&mut edited, track, settings(0.5));
        change(&mut edited);
        rejected.push(edited);
    };
    edit(&|project| {
        project
            .set_midi_clip_notes(track, 0, vec![note(0, 900, 60)])
            .unwrap();
    });
    edit(&|project| project.set_bpm(2_999.0).unwrap());
    edit(&|project| {
        project
            .set_midi_track_mixer(
                track,
                MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
            )
            .unwrap();
    });
    edit(&|project| project.set_midi_track_instrument(track, None).unwrap());
    edit(&|project| {
        let asset = project.add_asset(AudioBuffer::new(vec![[0.0; 2]]).unwrap());
        let audio = project.add_audio_track("Audio");
        project
            .add_audio_clip(
                audio,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 1,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                },
            )
            .unwrap();
    });
    for edited in &rejected {
        assert!(!transport.update_instruments(edited));
    }
    let mut actual = vec![[0.0; 2]; 500];
    transport.process_block(&mut actual);
    assert_eq!(actual, full_render(&project, 4096)[500..1_000]);

    let mut longer = project.clone();
    set_synth(&mut longer, track, settings(0.5));
    assert!(transport.update_instruments(&longer));
    assert_eq!(
        transport.duration_frames(),
        longer.duration_frames().unwrap()
    );
    assert!(transport.duration_frames() > project.duration_frames().unwrap());
    let mut tail = vec![[0.0; 2]; 20_000];
    transport.process_block(&mut tail);
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert!(tail[19_000..].iter().any(|frame| frame[0] != 0.0));
}
