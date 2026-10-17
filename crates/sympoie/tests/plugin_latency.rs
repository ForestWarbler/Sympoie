use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginProcessContext,
    PluginProcessor, PluginSidechainInput, SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings};
use sympoie::transport::{PlaybackState, Transport};

#[derive(Debug, Default)]
struct Host {
    loads: Arc<AtomicUsize>,
    resets: Arc<AtomicUsize>,
    dynamic: Arc<AtomicU32>,
    contexts: Arc<Mutex<Vec<PluginProcessContext>>>,
}

impl PluginHost for Host {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        self.loads.fetch_add(1, Ordering::Relaxed);
        let mut parts = instance.descriptor.class_id.split(':');
        let operation = parts.next().unwrap().to_owned();
        if operation == "missing" {
            return Err("plugin binary is missing".into());
        }
        let latency: u32 = parts.next().unwrap().parse().unwrap();
        Ok(Box::new(Processor {
            operation,
            latency,
            dynamic: self.dynamic.clone(),
            resets: self.resets.clone(),
            contexts: self.contexts.clone(),
            history: vec![[0.0; 2]; 4096.max((latency as usize).min(MAX_HISTORY) + 1)],
            cursor: 0,
            notes: HashSet::new(),
        }))
    }
}

#[derive(Debug)]
struct Processor {
    operation: String,
    latency: u32,
    dynamic: Arc<AtomicU32>,
    resets: Arc<AtomicUsize>,
    contexts: Arc<Mutex<Vec<PluginProcessContext>>>,
    history: Vec<Frame>,
    cursor: usize,
    notes: HashSet<u64>,
}

impl PluginProcessor for Processor {
    fn reset(&mut self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
        self.history.fill([0.0; 2]);
        self.notes.clear();
    }
    fn event(&mut self, event: NoteEvent) {
        match event {
            NoteEvent::On { id, .. } => {
                self.notes.insert(id);
            }
            NoteEvent::Off { id } => {
                self.notes.remove(&id);
            }
        }
    }
    fn set_parameter(&mut self, _: u32, _: f64) {}
    fn latency_samples(&self) -> u32 {
        if self.operation == "dynamic" {
            self.dynamic.load(Ordering::Relaxed)
        } else {
            self.latency
        }
    }
    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        self.process_with_sidechain(frames, None, context);
    }
    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        context: PluginProcessContext,
    ) {
        self.contexts.lock().unwrap().push(context);
        let latency = self.latency_samples() as usize;
        assert!(latency < self.history.len());
        for (index, frame) in frames.iter_mut().enumerate() {
            let mut value = *frame;
            if self.operation == "instrument" {
                value = [self.notes.len() as f32; 2];
            }
            if self.operation == "native" {
                value = [0.25; 2];
            }
            if self.operation == "gate" && (context.frame + index as u64) % 20 >= 10 {
                value = [0.0; 2];
            }
            if self.operation == "subtract" {
                let sidechain = sidechain.expect("sidechain must be supplied");
                assert_eq!(sidechain.bus_index, 2);
                for (sample, source) in value.iter_mut().zip(sidechain.frames[index]) {
                    *sample -= source;
                }
            }
            *frame = if latency == 0 {
                value
            } else {
                self.history[(self.cursor + self.history.len() - latency) % self.history.len()]
            };
            if self.operation == "lookahead" && value[0] > 1.0 {
                *frame = [0.0; 2];
            }
            self.history[self.cursor] = value;
            self.cursor = (self.cursor + 1) % self.history.len();
        }
    }
}

const MAX_HISTORY: usize = sympoie::SAMPLE_RATE as usize * 11;

fn device(operation: &str, latency: u32) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Latency.vst3".into(),
            class_id: format!("{operation}:{latency}"),
            name: operation.into(),
            vendor: "Tests".into(),
            kind: if matches!(operation, "instrument" | "native") {
                PluginKind::Instrument
            } else {
                PluginKind::Effect
            },
        },
        vec![],
        vec![],
    )
}

#[test]
fn audition_after_rebuild_reaches_the_audible_block_with_long_plugin_latency() {
    let mut project = project();
    let midi = project.add_midi_track("Preview");
    let latency = sympoie::SAMPLE_RATE / 4;
    project
        .set_plugin_instrument(midi, Some(device("instrument", latency)))
        .unwrap();
    let mut renderer = Renderer::with_plugin_host(&project, &Host::default()).unwrap();
    let gate = 12_345;
    assert!(renderer.audition_notes(
        1,
        &[MidiNote {
            start_tick: 0,
            duration_ticks: gate as u64,
            key: 60,
            velocity: 100,
            channel: 0,
        }]
    ));
    let mut output = vec![[0.0; 2]; gate + 1];
    renderer.render_block(u64::from(latency) * 2, &mut output);
    assert!(output[..gate].iter().all(|frame| *frame == [1.0; 2]));
    assert_eq!(output[gate], [0.0; 2]);
}

#[test]
fn audition_keeps_its_audible_gate_when_short_loops_wrap_plugin_delay_storage() {
    for latency in [7, sympoie::SAMPLE_RATE / 4] {
        let mut project = project();
        let midi = project.add_midi_track("Preview");
        project
            .set_plugin_instrument(midi, Some(device("instrument", latency)))
            .unwrap();
        let mut transport = Transport::with_plugin_host(&project, &Host::default()).unwrap();
        transport.set_loop_ticks(0, 41).unwrap();
        transport.play();
        let gate = 12_345;
        assert!(transport.audition_notes(
            1,
            &[MidiNote {
                start_tick: 0,
                duration_ticks: gate as u64,
                key: 60,
                velocity: 100,
                channel: 0,
            }]
        ));
        let mut output = vec![[0.0; 2]; gate + latency as usize + 100];
        transport.process_block(&mut output);
        assert!(
            output[..gate].iter().all(|frame| *frame == [1.0; 2]),
            "latency {latency}"
        );
        assert!(
            output[gate..].iter().all(|frame| *frame == [0.0; 2]),
            "latency {latency}"
        );
        assert!(!transport.has_auditions());
    }
}

#[test]
fn held_preview_survives_short_loops_and_drains_delayed_audio_after_release() {
    for latency in [0, 7, sympoie::SAMPLE_RATE / 4] {
        let mut project = project();
        let midi = project.add_midi_track("Held preview");
        project
            .set_plugin_instrument(midi, Some(device("instrument", latency)))
            .unwrap();
        let mut transport = Transport::with_plugin_host(&project, &Host::default()).unwrap();
        transport.set_loop_ticks(0, 41).unwrap();
        transport.play();
        let note = MidiNote {
            start_tick: 0,
            duration_ticks: 1,
            key: 60,
            velocity: 100,
            channel: 0,
        };
        assert!(transport.begin_audition_notes(1, &[note]));
        let mut held = vec![[0.0; 2]; sympoie::SAMPLE_RATE as usize * 2];
        transport.process_block(&mut held);
        assert!(
            held.iter().all(|frame| *frame == [1.0; 2]),
            "latency {latency}"
        );
        assert!(transport.has_auditions());
        let mut edited = project.clone();
        edited
            .add_midi_clip(
                midi,
                MidiClip {
                    start_tick: 1000,
                    offset_ticks: 0,
                    length_ticks: 960,
                    notes: vec![note],
                },
            )
            .unwrap();
        assert!(transport.update_midi_clips(&edited));
        let mut after_edit = [[0.0; 2]; 127];
        transport.process_block(&mut after_edit);
        assert_eq!(after_edit, [[1.0; 2]; 127]);
        transport.release_audition_notes();
        let mut release = vec![[0.0; 2]; latency as usize + 100];
        transport.process_block(&mut release);
        assert!(
            release[..latency as usize]
                .iter()
                .all(|frame| *frame == [1.0; 2]),
            "latency {latency}"
        );
        assert!(
            release[latency as usize..]
                .iter()
                .all(|frame| *frame == [0.0; 2]),
            "latency {latency}"
        );
        assert!(!transport.has_auditions());
        assert!(transport.begin_audition_notes(1, &[note]));
        transport.process_block(&mut [[0.0; 2]; 5]);
        transport.stop();
        assert!(!transport.has_auditions());
        let mut stopped = vec![[1.0; 2]; latency as usize + 64];
        transport.process_monitor_block(&mut stopped);
        assert!(stopped.iter().all(|frame| *frame == [0.0; 2]));
    }
}

fn project() -> Project {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    project
}

fn add_audio(project: &mut Project, samples: Vec<Frame>) -> AudioTrackId {
    let track = project.add_audio_track("Audio");
    let length = samples.len();
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: length as u64,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    track
}

fn signal(count: usize) -> Vec<Frame> {
    (0..count)
        .map(|i| [((i * 17) % 101) as f32 / 128.0; 2])
        .collect()
}

#[test]
fn unequal_serial_parallel_group_and_master_latencies_are_sample_aligned() {
    let mut project = project();
    let source = signal(2048);
    let first = add_audio(&mut project, source.clone());
    let second = add_audio(&mut project, source.clone());
    let third = add_audio(&mut project, source.clone());
    let group = project.add_group_track("Bus");
    project.set_track_group(first, Some(group)).unwrap();
    project.set_track_group(second, Some(group)).unwrap();
    project
        .add_effect(first.into(), device("delay", 5))
        .unwrap();
    project
        .add_effect(first.into(), device("delay", 7))
        .unwrap();
    project
        .add_effect(second.into(), device("delay", 3))
        .unwrap();
    project
        .add_effect(group.into(), device("delay", 2))
        .unwrap();
    project
        .add_effect(third.into(), device("delay", 13))
        .unwrap();
    project
        .add_effect(project.master_track_id().into(), device("delay", 11))
        .unwrap();
    let host = Host::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    assert_eq!(renderer.latency_frames(), 25);
    let mut actual = vec![[0.0; 2]; source.len()];
    for (index, block) in actual.chunks_mut(53).enumerate() {
        renderer.render_block((index * 53) as u64, block);
    }
    let expected: Vec<_> = source.iter().map(|frame| [frame[0] * 3.0; 2]).collect();
    assert_eq!(actual, expected);
    let mut seek = [[0.0; 2]; 73];
    renderer.render_block(901, &mut seek);
    assert_eq!(seek, expected[901..974]);
}

#[test]
fn sidechain_taps_align_main_and_auxiliary_inputs_without_leaking_disconnected_sources() {
    for (tap, gain, expected_gain, latency) in [
        (SidechainTap::PreEffects, 1.0, 0.0, 5),
        (SidechainTap::PostEffects, 1.0, 0.0, 11),
        (SidechainTap::PostEffects, 2.0, -1.0, 11),
        (SidechainTap::PostFader, 1.0, 1.0, 11),
    ] {
        let mut project = project();
        let source = signal(1024);
        let target = add_audio(&mut project, source.clone());
        let sender = add_audio(&mut project, source.clone());
        project
            .set_audio_track_mixer(
                sender,
                MixerSettings::new(0.0, 0.0, AudioOutput::Disconnected).unwrap(),
            )
            .unwrap();
        project
            .add_effect(sender.into(), device("delay", 9))
            .unwrap();
        project
            .add_effect(target.into(), device("delay", 3))
            .unwrap();
        let id = project
            .add_effect(target.into(), device("subtract", 2))
            .unwrap();
        project
            .set_plugin_sidechain(
                id,
                Some(SidechainRoute {
                    source_track: 2,
                    bus_index: 2,
                    gain,
                    tap,
                }),
            )
            .unwrap();
        let host = Host::default();
        let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
        assert_eq!(renderer.latency_frames(), latency);
        let mut output = vec![[0.0; 2]; source.len()];
        renderer.render_block(0, &mut output);
        assert_eq!(
            output,
            source
                .iter()
                .map(|frame| [frame[0] * expected_gain; 2])
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn bypass_keeps_the_delayed_signal_and_instances() {
    let mut project = project();
    let source = signal(4000);
    let track = add_audio(&mut project, source.clone());
    let mut effect = device("delay", 17);
    effect.bypassed = true;
    let id = project.add_effect(track.into(), effect).unwrap();
    let host = Host::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 500];
    renderer.render_block(0, &mut output);
    assert_eq!(output, source[..500]);
    let resets = host.resets.load(Ordering::Relaxed);
    let mut edited = project.clone();
    edited.set_plugin_bypassed(id, false).unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(500, &mut output);
    assert_eq!(output, source[500..1000]);
    assert_eq!(host.resets.load(Ordering::Relaxed), resets);
    assert_eq!(host.loads.load(Ordering::Relaxed), 1);
}

#[test]
fn instrument_latency_and_loop_seek_transport_positions_are_compensated() {
    let mut project = project();
    let midi = project.add_midi_track("Instrument");
    project
        .set_plugin_instrument(midi, Some(device("instrument", 7)))
        .unwrap();
    project.add_effect(midi.into(), device("delay", 5)).unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 40,
                notes: vec![MidiNote {
                    start_tick: 10,
                    duration_ticks: 10,
                    key: 60,
                    velocity: 127,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    assert_eq!(transport.latency_frames(), 12);
    transport.set_loop_ticks(0, 40).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 103];
    transport.process_block(&mut output);
    assert_eq!(transport.position_frame(), 23);
    for (index, frame) in output.iter().enumerate() {
        assert_eq!(
            *frame,
            [if (10..20).contains(&(index % 40)) {
                1.0
            } else {
                0.0
            }; 2]
        );
    }
    transport.seek_frame(15);
    transport.process_block(&mut output[..10]);
    assert_eq!(output[..5], [[1.0; 2]; 5]);
    assert_eq!(output[5..10], [[0.0; 2]; 5]);
    assert_eq!(transport.position_frame(), 25);
}

#[test]
fn monitoring_keeps_intrinsic_latency_and_fixed_stopped_context() {
    let mut project = project();
    let midi = project.add_midi_track("Keyboard");
    project
        .set_plugin_instrument(midi, Some(device("native", 7)))
        .unwrap();
    project.add_effect(midi.into(), device("delay", 5)).unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 30];
    transport.process_monitor_block(&mut output);
    assert_eq!(output[..12], [[0.0; 2]; 12]);
    assert_eq!(output[12..], [[0.25; 2]; 18]);
    assert_eq!(transport.position_frame(), 0);
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert!(
        host.contexts
            .lock()
            .unwrap()
            .iter()
            .all(|context| !context.playing && context.frame == 0)
    );
}

#[test]
fn latency_changes_replan_without_reloading_or_resetting_and_shrink_on_seek() {
    let mut project = project();
    let source = signal(5000);
    let delayed = add_audio(&mut project, source.clone());
    add_audio(&mut project, source.clone());
    project
        .add_effect(delayed.into(), device("dynamic", 0))
        .unwrap();
    let host = Host::default();
    host.dynamic.store(5, Ordering::Relaxed);
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 700];
    renderer.render_block(0, &mut output);
    let resets = host.resets.load(Ordering::Relaxed);
    for (start, latency, effective) in [(700, 23, 23), (1400, 3, 23)] {
        host.dynamic.store(latency, Ordering::Relaxed);
        renderer.render_block(start, &mut output);
        assert_eq!(renderer.latency_frames(), effective);
        assert_eq!(
            output[300..],
            source[start as usize + 300..start as usize + 700]
                .iter()
                .map(|frame| [frame[0] * 2.0; 2])
                .collect::<Vec<_>>()
        );
        assert_eq!(host.resets.load(Ordering::Relaxed), resets);
        assert_eq!(host.loads.load(Ordering::Relaxed), 1);
    }
    renderer.render_block(50, &mut output);
    assert_eq!(renderer.latency_frames(), 3);
    assert_eq!(
        output,
        source[50..750]
            .iter()
            .map(|frame| [frame[0] * 2.0; 2])
            .collect::<Vec<_>>()
            .as_slice()
    );
}

#[test]
fn excessive_latency_is_rejected_without_unbounded_buffers() {
    let mut project = project();
    let track = add_audio(&mut project, signal(10));
    project
        .add_effect(track.into(), device("delay", u32::MAX))
        .unwrap();
    assert!(Renderer::with_plugin_host(&project, &Host::default()).is_err());
}

#[test]
fn tempo_synchronized_effects_receive_the_aligned_input_timeline() {
    let mut project = project();
    let track = add_audio(&mut project, vec![[1.0; 2]; 2000]);
    project
        .add_effect(track.into(), device("delay", 103))
        .unwrap();
    project
        .add_effect(track.into(), device("gate", 17))
        .unwrap();
    let mut renderer = Renderer::with_plugin_host(&project, &Host::default()).unwrap();
    let mut output = [[0.0; 2]; 517];
    renderer.render_block(0, &mut output);
    for (index, frame) in output.iter().enumerate() {
        assert_eq!(
            *frame,
            [if index % 20 < 10 { 1.0 } else { 0.0 }; 2],
            "frame {index}"
        );
    }
    renderer.render_block(733, &mut output);
    for (index, frame) in output.iter().enumerate() {
        assert_eq!(*frame, [if (733 + index) % 20 < 10 { 1.0 } else { 0.0 }; 2]);
    }
}

#[test]
fn live_sidechain_gain_ramps_without_replacing_or_resetting_processors() {
    let mut project = project();
    let target = add_audio(&mut project, vec![[1.0; 2]; 3000]);
    let sender = add_audio(&mut project, vec![[1.0; 2]; 3000]);
    project
        .set_audio_track_mixer(
            sender,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    project
        .add_effect(sender.into(), device("delay", 31))
        .unwrap();
    let id = project
        .add_effect(target.into(), device("subtract", 19))
        .unwrap();
    let route = SidechainRoute {
        source_track: 2,
        bus_index: 2,
        gain: 0.0,
        tap: SidechainTap::PostEffects,
    };
    project.set_plugin_sidechain(id, Some(route)).unwrap();
    let host = Host::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 500];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[1.0; 2]; 500]);
    let resets = host.resets.load(Ordering::Relaxed);
    let mut edited = project.clone();
    edited
        .set_plugin_sidechain(id, Some(SidechainRoute { gain: 1.0, ..route }))
        .unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(500, &mut output);
    assert!(
        output
            .windows(2)
            .all(|pair| (pair[1][0] - pair[0][0]).abs() < 0.005)
    );
    assert_eq!(output[499], [0.0; 2]);
    assert_eq!(host.resets.load(Ordering::Relaxed), resets);
    assert_eq!(host.loads.load(Ordering::Relaxed), 2);
    edited
        .set_plugin_sidechain(
            id,
            Some(SidechainRoute {
                tap: SidechainTap::PreEffects,
                ..route
            }),
        )
        .unwrap();
    assert!(!renderer.update_parameters(&edited));
}

#[test]
fn lookahead_at_loop_end_uses_wrapped_audio_instead_of_material_outside_the_loop() {
    let mut project = project();
    let mut source = vec![[1.0; 2]; 40];
    source.extend(vec![[100.0; 2]; 40]);
    let track = add_audio(&mut project, source);
    project
        .add_effect(track.into(), device("lookahead", 13))
        .unwrap();
    let mut transport = Transport::with_plugin_host(&project, &Host::default()).unwrap();
    transport.set_loop_ticks(0, 40).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 123];
    transport.process_block(&mut output);
    assert_eq!(output, [[1.0; 2]; 123]);
    assert_eq!(transport.position_frame(), 3);
}

#[test]
fn delayed_instrument_notes_at_loop_end_survive_lookahead_schedule_wraps() {
    let mut project = project();
    let midi = project.add_midi_track("Loop-end note");
    project
        .set_plugin_instrument(midi, Some(device("instrument", 11)))
        .unwrap();
    project.add_effect(midi.into(), device("delay", 7)).unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 40,
                notes: vec![MidiNote {
                    start_tick: 35,
                    duration_ticks: 5,
                    key: 60,
                    velocity: 127,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    let mut transport = Transport::with_plugin_host(&project, &Host::default()).unwrap();
    transport.set_loop_ticks(0, 40).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 160];
    transport.process_block(&mut output);
    for (index, frame) in output.iter().enumerate() {
        assert_eq!(
            *frame,
            [if index % 40 >= 35 { 1.0 } else { 0.0 }; 2],
            "frame {index}"
        );
    }
}

#[test]
fn direct_renderer_seek_resets_instruments_even_when_preroll_meets_the_old_input_cursor() {
    let mut project = project();
    let midi = project.add_midi_track("Held instrument");
    project
        .set_plugin_instrument(midi, Some(device("instrument", 7)))
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 2000,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 2000,
                    key: 60,
                    velocity: 127,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    let host = Host::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 100];
    renderer.render_block(0, &mut output);
    let resets = host.resets.load(Ordering::Relaxed);
    renderer.render_block(114, &mut output);
    assert_eq!(host.resets.load(Ordering::Relaxed), resets + 1);
    assert_eq!(output, [[1.0; 2]; 100]);
}

#[test]
fn disconnected_unused_plugins_are_not_loaded_but_sidechain_dependencies_are() {
    let mut project = project();
    let source = signal(500);
    let target = add_audio(&mut project, source.clone());
    let dormant = add_audio(&mut project, vec![[100.0; 2]; 500]);
    let group = project.add_group_track("Disconnected bus");
    project.set_track_group(dormant, Some(group)).unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    project
        .add_effect(dormant.into(), device("missing", 0))
        .unwrap();
    let host = Host::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 500];
    renderer.render_block(0, &mut output);
    assert_eq!(output, source.as_slice());
    assert_eq!(host.loads.load(Ordering::Relaxed), 0);
    assert!(!renderer.has_live_plugins());
    drop(renderer);
    let effect = project
        .add_effect(target.into(), device("subtract", 0))
        .unwrap();
    project
        .set_plugin_sidechain(
            effect,
            Some(SidechainRoute {
                source_track: 3,
                bus_index: 2,
                gain: 1.0,
                tap: SidechainTap::PostFader,
            }),
        )
        .unwrap();
    let error = Renderer::with_plugin_host(&project, &host)
        .unwrap_err()
        .to_string();
    assert!(error.contains("plugin binary is missing"));
}

#[test]
fn muting_preserves_native_monitor_processors_in_the_audible_graph() {
    let mut project = project();
    let midi = project.add_midi_track("Muted native keyboard");
    project
        .set_plugin_instrument(midi, Some(device("native", 0)))
        .unwrap();
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(0.0, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let host = Host::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 500];
    assert!(renderer.has_live_plugins());
    renderer.render_monitor_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]; 500]);
    let mut edited = project.clone();
    edited
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(1.0, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_monitor_block(0, &mut output);
    assert_eq!(output[499], [0.25; 2]);
    assert_eq!(host.loads.load(Ordering::Relaxed), 1);
    assert_eq!(host.resets.load(Ordering::Relaxed), 0);
}

#[test]
fn hidden_bus_and_mixer_insert_latencies_align_siblings_and_sidechains() {
    let mut project = project();
    let source = signal(1024);
    let first = add_audio(&mut project, source.clone());
    let second = add_audio(&mut project, source.clone());
    let sender = add_audio(&mut project, source.clone());
    let bus = project.add_bus_track("Mix bus");
    project
        .set_audio_track_mixer(
            first,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            second,
            MixerSettings::default().with_output(AudioOutput::Bus(bus)),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            sender,
            MixerSettings::default().with_output(AudioOutput::Disconnected),
        )
        .unwrap();
    project
        .add_effect(first.into(), device("delay", 3))
        .unwrap();
    project
        .add_mixer_effect(first.into(), device("delay", 7))
        .unwrap();
    project
        .add_mixer_effect(second.into(), device("delay", 19))
        .unwrap();
    project
        .add_mixer_effect(sender.into(), device("delay", 31))
        .unwrap();
    let effect = project
        .add_mixer_effect(bus.into(), device("subtract", 5))
        .unwrap();
    project
        .set_plugin_sidechain(
            effect,
            Some(SidechainRoute {
                source_track: 3,
                bus_index: 2,
                gain: 1.0,
                tap: SidechainTap::PostEffects,
            }),
        )
        .unwrap();
    let mut renderer = Renderer::with_plugin_host(&project, &Host::default()).unwrap();
    assert_eq!(renderer.latency_frames(), 36);
    let mut output = vec![[0.0; 2]; source.len()];
    for (index, chunk) in output.chunks_mut(53).enumerate() {
        renderer.render_block((index * 53) as u64, chunk);
    }
    assert_eq!(output, source);
    let mut seek = [[0.0; 2]; 73];
    renderer.render_block(501, &mut seek);
    assert_eq!(seek, source[501..574]);
}

#[test]
fn dry_and_multiple_send_returns_are_compensated_independently() {
    let mut project = project();
    let source = signal(1024);
    let track = add_audio(&mut project, source.clone());
    let first = project.add_bus_track("First return");
    let second = project.add_bus_track("Second return");
    project
        .add_effect(track.into(), device("delay", 3))
        .unwrap();
    project
        .add_mixer_effect(first.into(), device("delay", 7))
        .unwrap();
    project
        .add_mixer_effect(second.into(), device("delay", 19))
        .unwrap();
    project
        .add_mixer_effect(project.master_track_id().into(), device("delay", 5))
        .unwrap();
    project.set_track_send(track, first, 0.5).unwrap();
    project.set_track_send(track, second, 0.25).unwrap();
    project.set_track_send(first, second, 0.5).unwrap();
    let mut renderer = Renderer::with_plugin_host(&project, &Host::default()).unwrap();
    assert_eq!(renderer.latency_frames(), 34);
    let mut output = vec![[0.0; 2]; source.len()];
    for (index, chunk) in output.chunks_mut(53).enumerate() {
        renderer.render_block((index * 53) as u64, chunk);
    }
    let expected: Vec<_> = source.iter().map(|frame| [frame[0] * 2.0; 2]).collect();
    assert_eq!(output, expected);
    let mut seek = [[0.0; 2]; 73];
    renderer.render_block(501, &mut seek);
    assert_eq!(seek, expected[501..574]);
}
