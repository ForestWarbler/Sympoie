use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sympoie::Frame;
use sympoie::automation::{
    AutomationLane, AutomationPoint, AutomationTarget, SubtractiveParameter,
};
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, NoteEvent, SubtractiveSettings, Waveform};
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginParameter,
    PluginProcessContext, PluginProcessor,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings};
use sympoie::transport::Transport;

fn lane(target: AutomationTarget, points: &[(u64, f64)]) -> AutomationLane {
    AutomationLane::new(
        target,
        points
            .iter()
            .map(|&(tick, value)| AutomationPoint { tick, value })
            .collect(),
    )
    .unwrap()
}

fn audio(project: &mut Project) -> AudioTrackId {
    let track = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[1.0; 2]; 4_000]).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 4_000,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    track
}

fn project() -> Project {
    let mut project = Project::new();
    project.set_bpm(3_000.0).unwrap();
    project
}

#[test]
fn gain_and_pan_follow_each_sample_on_audio_group_and_master_without_changing_base_values() {
    let mut project = project();
    let group = project.add_group_track("Bus");
    let track = audio(&mut project);
    project.set_track_group(track, Some(group)).unwrap();
    project
        .set_track_automation(
            track,
            lane(AutomationTarget::TrackGain, &[(0, 0.0), (256, 0.5)]),
        )
        .unwrap();
    project
        .set_track_automation(
            group,
            lane(AutomationTarget::TrackPan, &[(0, 0.0), (256, 1.0)]),
        )
        .unwrap();
    project
        .set_track_automation(
            project.master_track_id(),
            lane(AutomationTarget::TrackGain, &[(0, 0.25)]),
        )
        .unwrap();
    let before = project.clone();
    let mut renderer = Renderer::new(&project).unwrap();
    let mut output = [[0.0; 2]; 300];
    renderer.render_block(0, &mut output);
    for (index, frame) in output.iter().enumerate() {
        let progress = (index as f32 / 256.0).min(1.0);
        let pan = progress * 2.0 - 1.0;
        let expected = [
            progress * 0.5 * (1.0 - pan.max(0.0)),
            progress * 0.5 * (1.0 + pan.min(0.0)),
        ];
        assert_eq!(*frame, expected, "frame {index}");
    }
    for (track, original) in project.tracks().iter().zip(before.tracks()) {
        assert_eq!(track.mixer(), original.mixer());
        assert_eq!(track.automation(), original.automation());
    }
}

#[test]
fn loop_seek_and_callback_partitioning_use_the_same_sample_accurate_envelope() {
    let mut project = project();
    let track = audio(&mut project);
    project
        .set_track_automation(
            track,
            lane(AutomationTarget::TrackGain, &[(0, 0.0), (256, 0.5)]),
        )
        .unwrap();
    let mut renderer = Renderer::new(&project).unwrap();
    let mut sought = [[0.0; 2]; 17];
    renderer.render_block(91, &mut sought);
    assert_eq!(sought[0], [91.0 / 256.0; 2]);
    renderer.render_block(23, &mut sought);
    assert_eq!(sought[0], [23.0 / 256.0; 2]);
    let render = |chunk: usize| {
        let mut transport = Transport::new(&project).unwrap();
        transport.set_loop_ticks(37, 173).unwrap();
        transport.play();
        let mut output = vec![[0.0; 2]; 700];
        for frames in output.chunks_mut(chunk) {
            transport.process_block(frames);
        }
        output
    };
    let expected = render(700);
    assert_eq!(expected, render(7));
    for (index, frame) in expected.iter().enumerate() {
        let position = if index < 173 {
            index
        } else {
            37 + (index - 173) % 136
        };
        assert_eq!(*frame, [position as f32 / 256.0; 2]);
    }
}

#[test]
fn live_lane_edits_and_disabled_lanes_restore_manual_mixer_without_rebuild_and_mute_wins() {
    let mut project = project();
    let track = audio(&mut project);
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.25, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let automation = lane(AutomationTarget::TrackGain, &[(0, 0.5)]);
    project
        .set_track_automation(track, automation.clone())
        .unwrap();
    let mut renderer = Renderer::new(&project).unwrap();
    let mut output = [[0.0; 2]; 512];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[1.0; 2]; 512]);
    let mut edited = project.clone();
    edited
        .set_track_automation(track, lane(AutomationTarget::TrackGain, &[(0, 0.75)]))
        .unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(512, &mut output);
    assert_eq!(output, [[1.5; 2]; 512]);
    edited
        .set_audio_track_mixer(
            track,
            edited.audio_track(track).unwrap().mixer().with_muted(true),
        )
        .unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(1024, &mut output);
    assert_eq!(output[240..], [[0.0; 2]; 272]);
    edited
        .set_audio_track_mixer(
            track,
            edited.audio_track(track).unwrap().mixer().with_muted(false),
        )
        .unwrap();
    edited
        .set_track_automation(track, automation.with_enabled(false))
        .unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(1536, &mut output);
    assert_eq!(output[240..], [[0.25; 2]; 272]);
}

struct GainHost;
#[derive(Debug)]
struct GainPlugin {
    value: f64,
    instrument: bool,
    latency: usize,
    history: Vec<Frame>,
    cursor: usize,
}
impl PluginHost for GainHost {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        let latency = instance.descriptor.class_id.parse().unwrap();
        Ok(Box::new(GainPlugin {
            value: instance.parameters[0].value,
            instrument: instance.descriptor.kind == PluginKind::Instrument,
            latency,
            history: vec![[0.0; 2]; latency + 1],
            cursor: 0,
        }))
    }
}
impl PluginProcessor for GainPlugin {
    fn reset(&mut self) {
        self.history.fill([0.0; 2]);
        self.cursor = 0;
    }
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, _: u32, value: f64) {
        self.value = value;
    }
    fn latency_samples(&self) -> u32 {
        self.latency as u32
    }
    fn process(&mut self, frames: &mut [Frame], _: PluginProcessContext) {
        for frame in frames {
            let source = if self.instrument { [1.0; 2] } else { *frame };
            let value = source.map(|sample| sample * self.value as f32);
            *frame = if self.latency == 0 {
                value
            } else {
                self.history[(self.cursor + 1) % self.history.len()]
            };
            self.history[self.cursor] = value;
            self.cursor = (self.cursor + 1) % self.history.len();
        }
    }
}
fn device(kind: PluginKind, latency: usize) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Gain.vst3".into(),
            class_id: latency.to_string(),
            name: "Gain".into(),
            vendor: "Test".into(),
            kind,
        },
        vec![PluginParameter {
            id: 7,
            name: "Amount".into(),
            unit: String::new(),
            value: 1.0,
            steps: 0,
            read_only: false,
        }],
        vec![],
    )
}

struct NativeEditorHost(Arc<[AtomicU64; 2]>);

#[derive(Debug)]
struct NativeEditorPlugin(Arc<[AtomicU64; 2]>);

impl PluginHost for NativeEditorHost {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        for (shared, parameter) in self.0.iter().zip(&instance.parameters) {
            shared.store(parameter.value.to_bits(), Ordering::Relaxed);
        }
        Ok(Box::new(NativeEditorPlugin(self.0.clone())))
    }
}

impl PluginProcessor for NativeEditorPlugin {
    fn reset(&mut self) {}
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, id: u32, value: f64) {
        self.0[(id - 7) as usize].store(value.to_bits(), Ordering::Relaxed);
    }
    fn process(&mut self, frames: &mut [Frame], _: PluginProcessContext) {
        frames.fill(std::array::from_fn(|index| {
            f64::from_bits(self.0[index].load(Ordering::Relaxed)) as f32
        }));
    }
}

#[test]
fn constant_plugin_automation_reasserts_after_native_gestures_and_restores_each_manual_parameter() {
    let mut project = project();
    let track = project.add_midi_track("Native editor");
    let mut plugin = device(PluginKind::Instrument, 0);
    plugin.parameters[0].value = 0.5;
    plugin.parameters.push(PluginParameter {
        id: 8,
        name: "Second".into(),
        unit: String::new(),
        value: 0.75,
        steps: 0,
        read_only: false,
    });
    project.set_plugin_instrument(track, Some(plugin)).unwrap();
    let plugin = project
        .midi_track(track)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    let target = AutomationTarget::PluginParameter {
        plugin,
        parameter: 7,
    };
    project
        .set_track_automation(track, lane(target, &[(0, 0.5)]))
        .unwrap();
    project
        .set_track_automation(
            track,
            lane(
                AutomationTarget::PluginParameter {
                    plugin,
                    parameter: 8,
                },
                &[(0, 0.25)],
            ),
        )
        .unwrap();
    let shared = Arc::new(std::array::from_fn(|_| AtomicU64::new(0)));
    let mut renderer =
        Renderer::with_plugin_host(&project, &NativeEditorHost(shared.clone())).unwrap();
    let mut output = [[0.0; 2]; 64];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[0.5, 0.25]; 64]);

    shared[0].store(0.8_f64.to_bits(), Ordering::Relaxed);
    renderer.render_block(64, &mut output);
    assert_eq!(output, [[0.5, 0.25]; 64]);
    let mut edited = project.clone();
    edited.set_plugin_parameter(plugin, 7, 0.8).unwrap();
    shared[0].store(0.8_f64.to_bits(), Ordering::Relaxed);
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(128, &mut output);
    assert_eq!(output, [[0.5, 0.25]; 64]);
    renderer.render_monitor_block(192, &mut output);
    assert_eq!(output, [[0.8, 0.75]; 64]);
    renderer.render_block(192, &mut output);
    assert_eq!(output, [[0.5, 0.25]; 64]);

    edited.set_plugin_parameter(plugin, 7, 0.5).unwrap();
    edited.remove_track_automation(track, target).unwrap();
    shared[0].store(0.9_f64.to_bits(), Ordering::Relaxed);
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(256, &mut output);
    assert_eq!(output, [[0.5, 0.25]; 64]);
}

#[test]
fn device_automation_and_faders_follow_compensated_audio_through_plugin_chain() {
    let mut project = project();
    let track = audio(&mut project);
    project
        .add_effect(track.into(), device(PluginKind::Effect, 113))
        .unwrap();
    project
        .add_effect(track.into(), device(PluginKind::Effect, 29))
        .unwrap();
    let plugin = project.audio_track(track).unwrap().effects()[1].id;
    project
        .set_track_automation(
            track,
            lane(
                AutomationTarget::PluginParameter {
                    plugin,
                    parameter: 7,
                },
                &[(0, 0.0), (512, 1.0)],
            ),
        )
        .unwrap();
    project
        .set_track_automation(
            track,
            lane(AutomationTarget::TrackGain, &[(0, 0.0), (512, 0.5)]),
        )
        .unwrap();
    let render = |chunk: usize| {
        let mut renderer = Renderer::with_plugin_host(&project, &GainHost).unwrap();
        let mut output = vec![[0.0; 2]; 600];
        for (index, frames) in output.chunks_mut(chunk).enumerate() {
            renderer.render_block((index * chunk) as u64, frames);
        }
        output
    };
    let output = render(600);
    assert_eq!(output, render(11));
    for (position, frame) in output.iter().enumerate() {
        let amount = (position / 64 * 64).min(512) as f32 / 512.0;
        let gain = position.min(512) as f32 / 512.0;
        assert_eq!(*frame, [amount * gain; 2], "position {position}");
    }
    let render_loop = |chunk: usize| {
        let mut transport = Transport::with_plugin_host(&project, &GainHost).unwrap();
        transport.set_loop_ticks(37, 173).unwrap();
        transport.play();
        let mut output = vec![[0.0; 2]; 700];
        for frames in output.chunks_mut(chunk) {
            transport.process_block(frames);
        }
        output
    };
    let output = render_loop(700);
    assert_eq!(output, render_loop(11));
    for (index, frame) in output.iter().enumerate() {
        let position = if index < 173 {
            index
        } else {
            37 + (index - 173) % 136
        };
        let origin = if position >= 37 { 37 } else { 0 };
        let parameter_position = origin + (position - origin) / 64 * 64;
        assert_eq!(
            *frame,
            [(parameter_position * position) as f32 / (512.0 * 512.0); 2],
            "loop position {position}"
        );
    }
}

#[test]
fn plugin_loop_quanta_are_independent_of_callback_sizes_and_monitor_restores_base() {
    let mut project = project();
    let track = project.add_midi_track("External instrument");
    project
        .set_plugin_instrument(track, Some(device(PluginKind::Instrument, 0)))
        .unwrap();
    let plugin = project
        .midi_track(track)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    project
        .set_track_automation(
            track,
            lane(
                AutomationTarget::PluginParameter {
                    plugin,
                    parameter: 7,
                },
                &[(0, 0.0), (256, 1.0)],
            ),
        )
        .unwrap();
    let render = |chunk: usize| {
        let mut transport = Transport::with_plugin_host(&project, &GainHost).unwrap();
        transport.set_loop_ticks(37, 173).unwrap();
        transport.play();
        let mut output = vec![[0.0; 2]; 500];
        for frames in output.chunks_mut(chunk) {
            transport.process_block(frames);
        }
        output
    };
    let output = render(500);
    assert_eq!(output, render(3));
    assert!(output.iter().any(|frame| frame[0] != 0.0));
    let mut renderer = Renderer::with_plugin_host(&project, &GainHost).unwrap();
    let mut output = [[0.0; 2]; 128];
    renderer.render_block(0, &mut output);
    assert_eq!(output[0], [0.0; 2]);
    assert_eq!(output[64], [0.25; 2]);
    renderer.render_monitor_block(128, &mut output);
    assert_eq!(output, [[1.0; 2]; 128]);
    assert_eq!(
        project
            .midi_track(track)
            .unwrap()
            .plugin_instrument()
            .unwrap()
            .parameters[0]
            .value,
        1.0
    );
}

#[test]
fn builtin_gain_automation_is_block_independent_and_live_disable_restores_base() {
    let mut project = project();
    let track = project.add_midi_track("Subtractive");
    let instrument = Instrument::Subtractive(SubtractiveSettings {
        waveform: Waveform::Sine,
        cutoff_hz: 20_000.0,
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.01,
        gain: 0.5,
        ..SubtractiveSettings::default()
    });
    project
        .set_midi_track_instrument(track, Some(instrument))
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 4_000,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 4_000,
                    key: 60,
                    velocity: 127,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    let automation = lane(
        AutomationTarget::Subtractive(SubtractiveParameter::Gain),
        &[(0, 0.0), (512, 0.5)],
    );
    project
        .set_track_automation(track, automation.clone())
        .unwrap();
    let render = |chunk: usize| {
        let mut renderer = Renderer::new(&project).unwrap();
        let mut output = vec![[0.0; 2]; 1_200];
        for (index, frames) in output.chunks_mut(chunk).enumerate() {
            renderer.render_block((index * chunk) as u64, frames);
        }
        output
    };
    assert_eq!(render(1_200), render(13));
    let mut silent = project.clone();
    silent
        .set_track_automation(track, lane(automation.target(), &[(0, 0.0)]))
        .unwrap();
    let mut renderer = Renderer::new(&silent).unwrap();
    let mut output = [[0.0; 2]; 1200];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]; 1200]);
    let mut restored = silent.clone();
    restored
        .remove_track_automation(track, automation.target())
        .unwrap();
    assert!(renderer.update_parameters(&restored));
    renderer.render_block(1200, &mut output);
    assert!(output[960..].iter().any(|frame| frame[0] != 0.0));
    assert_eq!(
        silent.midi_track(track).unwrap().instrument(),
        Some(&instrument)
    );
}
