use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginParameter,
    PluginProcessContext, PluginProcessor,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::{RenderError, Renderer};
use sympoie::track::{AudioOutput, MixerSettings, Track};
use sympoie::transport::{PlaybackState, Transport};

#[derive(Debug, Default)]
struct Counts {
    loads: usize,
    resets: usize,
    parameters: usize,
    events: usize,
    frames: usize,
}

#[derive(Debug, Default)]
struct FakeHost(Arc<Mutex<Counts>>);

impl PluginHost for FakeHost {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        self.0.lock().unwrap().loads += 1;
        Ok(Box::new(FakePlugin {
            counts: self.0.clone(),
            operation: instance.descriptor.class_id.clone(),
            value: instance.parameters[0].value as f32,
            elapsed: 0,
            voices: HashSet::new(),
            previous_end: None,
        }))
    }
}

#[derive(Debug)]
struct FakePlugin {
    counts: Arc<Mutex<Counts>>,
    operation: String,
    value: f32,
    elapsed: u64,
    voices: HashSet<u64>,
    previous_end: Option<u64>,
}

impl PluginProcessor for FakePlugin {
    fn reset(&mut self) {
        self.counts.lock().unwrap().resets += 1;
        self.elapsed = 0;
        self.voices.clear();
        self.previous_end = None;
    }

    fn event(&mut self, event: NoteEvent) {
        self.counts.lock().unwrap().events += 1;
        match event {
            NoteEvent::On { id, .. } => {
                self.voices.insert(id);
            }
            NoteEvent::Off { id } => {
                self.voices.remove(&id);
            }
            NoteEvent::PitchBend { .. } => {}
        }
    }

    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        assert!(context.playing);
        assert_eq!(context.bpm, 3000.0);
        if let Some(end) = self.previous_end {
            assert_eq!(
                context.frame, end,
                "processor timeline skipped or restarted"
            );
        }
        self.previous_end = Some(context.frame + frames.len() as u64);
        self.counts.lock().unwrap().frames += frames.len();
        for frame in frames {
            for channel in frame {
                *channel = match self.operation.as_str() {
                    "gain" => *channel * self.value,
                    "bias" => *channel + self.value,
                    "instrument" => self.voices.len() as f32 * self.value,
                    "counter" => self.elapsed as f32 * 0.0001 * self.value,
                    other => panic!("unknown fake processor {other}"),
                };
            }
            self.elapsed += 1;
        }
    }

    fn set_parameter(&mut self, id: u32, value: f64) {
        assert_eq!(id, 7);
        self.counts.lock().unwrap().parameters += 1;
        self.value = value as f32;
    }
}

fn device(operation: &str, value: f64) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Test.vst3".into(),
            class_id: operation.to_owned(),
            name: operation.to_owned(),
            vendor: "Test".to_owned(),
            kind: if operation == "instrument" {
                PluginKind::Instrument
            } else {
                PluginKind::Effect
            },
        },
        vec![PluginParameter {
            id: 7,
            name: "Amount".to_owned(),
            unit: String::new(),
            value,
            steps: 0,
            read_only: false,
        }],
        Vec::new(),
    )
}

fn audio_project() -> (Project, sympoie::track::AudioTrackId) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[1.0; 2]; 10_000]).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 10_000,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    (project, track)
}

#[test]
fn plugin_projects_require_an_explicit_host_even_when_disconnected() {
    let (mut project, track) = audio_project();
    project
        .add_effect(track.into(), device("gain", 0.5))
        .unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        Renderer::new(&project),
        Err(RenderError::Plugin(_))
    ));
    assert!(matches!(
        Transport::new(&project),
        Err(RenderError::Plugin(_))
    ));
}

#[test]
fn effect_generators_do_not_render_past_the_representable_timeline() {
    let (mut project, track) = audio_project();
    project
        .add_effect(track.into(), device("bias", 0.125))
        .unwrap();
    let host = FakeHost::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[99.0; 2]; 16];
    renderer.render_block(u64::MAX - 3, &mut output);
    assert_eq!(output[..3], [[0.125; 2]; 3]);
    assert_eq!(output[3..], [[0.0; 2]; 13]);
    assert_eq!(host.0.lock().unwrap().frames, 3);
}

#[test]
fn effect_chain_order_and_bus_processing_precede_each_mixer() {
    let (mut project, track) = audio_project();
    project
        .add_effect(track.into(), device("bias", 0.25))
        .unwrap();
    project
        .add_effect(track.into(), device("gain", 0.5))
        .unwrap();
    let group = project.add_group_track("Bus");
    project.set_track_group(track, Some(group)).unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.5, 0.0, AudioOutput::Group(group)).unwrap(),
        )
        .unwrap();
    project
        .add_effect(group.into(), device("bias", 0.125))
        .unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .add_effect(project.master_track_id().into(), device("gain", 0.5))
        .unwrap();
    let host = FakeHost::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 19];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[0.109375; 2]; 19]);
    assert_eq!(host.0.lock().unwrap().loads, 4);
}

#[test]
fn external_instrument_has_sample_accurate_overlapping_notes_and_effects() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let midi = project.add_midi_track("External");
    project
        .set_plugin_instrument(midi, Some(device("instrument", 0.5)))
        .unwrap();
    project
        .add_effect(midi.into(), device("gain", 0.5))
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 7,
                length_ticks: 30,
                notes: vec![
                    MidiNote {
                        start_tick: 0,
                        duration_ticks: 10,
                        key: 60,
                        velocity: 100,
                        channel: 0,
                        slide_ticks: 0,
                    },
                    MidiNote {
                        start_tick: 5,
                        duration_ticks: 10,
                        key: 60,
                        velocity: 100,
                        channel: 0,
                        slide_ticks: 0,
                    },
                ],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = FakeHost::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 40];
    for (index, block) in output.chunks_mut(3).enumerate() {
        renderer.render_block((index * 3) as u64, block);
    }
    assert_eq!(output[..7], [[0.0; 2]; 7]);
    assert_eq!(output[7..12], [[0.25; 2]; 5]);
    assert_eq!(output[12..17], [[0.5; 2]; 5]);
    assert_eq!(output[17..22], [[0.25; 2]; 5]);
    assert_eq!(output[22..], [[0.0; 2]; 18]);
    assert_eq!(host.0.lock().unwrap().events, 4);
}

#[test]
fn effect_parameter_and_bypass_updates_preserve_live_processor_state() {
    let (mut project, track) = audio_project();
    let id = project
        .add_effect(track.into(), device("counter", 0.5))
        .unwrap();
    let host = FakeHost::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 600];
    transport.process_block(&mut output);
    let initial_resets = host.0.lock().unwrap().resets;

    let mut edited = project.clone();
    edited.set_plugin_parameter(id, 7, 1.0).unwrap();
    assert!(transport.update_parameters(&edited));
    assert!(transport.update_parameters(&edited));
    transport.process_block(&mut output);
    assert_eq!(output[0], [600.0 * 0.0001; 2]);
    assert_eq!(host.0.lock().unwrap().parameters, 1);

    edited.set_plugin_bypassed(id, true).unwrap();
    assert!(transport.update_parameters(&edited));
    transport.process_block(&mut output);
    assert!(
        output[0][0] < 0.13,
        "bypass must crossfade, not jump to dry"
    );
    assert_eq!(output[599], [1.0; 2]);

    edited.set_plugin_bypassed(id, false).unwrap();
    assert!(transport.update_parameters(&edited));
    transport.process_block(&mut output);
    assert!(output[0][0] > 0.99);
    assert_eq!(output[599], [2399.0 * 0.0001; 2]);
    let counts = host.0.lock().unwrap();
    assert_eq!(counts.loads, 1);
    assert_eq!(counts.resets, initial_resets);
    assert_eq!(counts.frames, 2400);
    assert_eq!(transport.position_frame(), 2400);
    assert_eq!(transport.state(), PlaybackState::Playing);
}

#[test]
fn external_instrument_parameters_do_not_retrigger_held_notes() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let midi = project.add_midi_track("External");
    project
        .set_plugin_instrument(midi, Some(device("instrument", 0.5)))
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 5000,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 5000,
                    key: 60,
                    velocity: 100,
                    channel: 0,
                    slide_ticks: 0,
                }],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let id = match &project.tracks()[1] {
        Track::Midi(track) => track.plugin_instrument().unwrap().id,
        _ => unreachable!(),
    };
    let host = FakeHost::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 200];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.5; 2]; 200]);
    let mut edited = project.clone();
    edited.set_plugin_parameter(id, 7, 0.25).unwrap();
    assert!(transport.update_parameters(&edited));
    transport.process_block(&mut output);
    assert_eq!(output, [[0.25; 2]; 200]);
    assert_eq!(host.0.lock().unwrap().loads, 1);
    assert_eq!(host.0.lock().unwrap().events, 1);
}

#[test]
fn plugin_replacement_state_and_order_changes_require_repreparation() {
    let (mut project, track) = audio_project();
    let first = project
        .add_effect(track.into(), device("gain", 0.5))
        .unwrap();
    project
        .add_effect(track.into(), device("bias", 0.125))
        .unwrap();
    let host = FakeHost::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    let mut edited = project.clone();
    edited.set_plugin_parameter(first, 7, 0.0).unwrap();
    edited.set_plugin_state(first, vec![1]).unwrap();
    assert!(!transport.update_parameters(&edited));
    let mut edited = project.clone();
    edited.move_effect(track.into(), 0, 1).unwrap();
    assert!(!transport.update_parameters(&edited));
    let mut edited = project.clone();
    edited.remove_effect(track.into(), 1).unwrap();
    edited
        .add_effect(track.into(), device("bias", 0.125))
        .unwrap();
    assert!(!transport.update_parameters(&edited));
    transport.play();
    let mut output = [[0.0; 2]; 16];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.625; 2]; 16]);
    assert_eq!(host.0.lock().unwrap().parameters, 0);
}

#[test]
fn separate_mixer_inserts_follow_sound_design_and_feed_hidden_bus_before_faders() {
    let (mut project, track) = audio_project();
    project
        .add_effect(track.into(), device("bias", 0.25))
        .unwrap();
    project
        .add_mixer_effect(track.into(), device("gain", 0.5))
        .unwrap();
    let bus = project.add_bus_track("Mix bus");
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.5, 0.0, AudioOutput::Bus(bus)).unwrap(),
        )
        .unwrap();
    project
        .add_mixer_effect(bus.into(), device("bias", 0.125))
        .unwrap();
    project
        .set_bus_track_mixer(
            bus,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .add_mixer_effect(project.master_track_id().into(), device("gain", 0.5))
        .unwrap();
    let host = FakeHost::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 19];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[0.109375; 2]; 19]);
    assert_eq!(
        renderer.track_peaks(),
        &[[0.109375; 2], [0.3125; 2], [0.21875; 2]]
    );
    assert_eq!(host.0.lock().unwrap().loads, 4);
    assert!(Renderer::new(&project).is_err());
}

#[test]
fn mixer_insert_updates_reuse_processors_and_automation_targets() {
    let (mut project, track) = audio_project();
    let id = project
        .add_mixer_effect(track.into(), device("counter", 0.5))
        .unwrap();
    let host = FakeHost::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 600];
    renderer.render_block(0, &mut output);
    let resets = host.0.lock().unwrap().resets;
    let mut edited = project.clone();
    edited.set_plugin_parameter(id, 7, 1.0).unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(600, &mut output);
    assert_eq!(output[0], [600.0 * 0.0001; 2]);
    edited.set_plugin_bypassed(id, true).unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(1200, &mut output);
    assert_eq!(output[599], [1.0; 2]);
    let counts = host.0.lock().unwrap();
    assert_eq!(counts.loads, 1);
    assert_eq!(counts.resets, resets);
    assert_eq!(counts.parameters, 1);
}

#[test]
fn bus_solo_includes_its_inputs_and_track_solo_excludes_bus_siblings() {
    let (mut project, first) = audio_project();
    let second = project.add_audio_track("Second");
    let clip = project.audio_track(first).unwrap().clips()[0].clone();
    project.add_audio_clip(second, clip).unwrap();
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
        .set_bus_track_mixer(bus, MixerSettings::default().with_soloed(true))
        .unwrap();
    let mut output = [[0.0; 2]; 16];
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[2.0; 2]; 16]);
    project
        .set_bus_track_mixer(bus, MixerSettings::default())
        .unwrap();
    project
        .set_audio_track_mixer(
            first,
            MixerSettings::default()
                .with_output(AudioOutput::Bus(bus))
                .with_soloed(true),
        )
        .unwrap();
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[1.0; 2]; 16]);
    project
        .set_bus_track_mixer(bus, MixerSettings::default().with_muted(true))
        .unwrap();
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]; 16]);
}

#[test]
fn post_fader_sends_add_multiple_returns_and_keep_the_main_output_independent() {
    let (mut project, track) = audio_project();
    let first = project.add_bus_track("Short return");
    let second = project.add_bus_track("Long return");
    project
        .add_effect(track.into(), device("bias", 0.25))
        .unwrap();
    project
        .add_mixer_effect(track.into(), device("gain", 0.5))
        .unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.5, -0.5, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .add_mixer_effect(first.into(), device("gain", 0.5))
        .unwrap();
    project
        .add_mixer_effect(second.into(), device("gain", 0.25))
        .unwrap();
    project.set_track_send(track, first, 0.5).unwrap();
    project.set_track_send(track, second, 0.25).unwrap();
    let mut output = [[0.0; 2]; 16];
    Renderer::with_plugin_host(&project, &FakeHost::default())
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.41015625, 0.41015625 / 2.0]; 16]);
    assert_eq!(
        project.audio_track(track).unwrap().mixer().output(),
        AudioOutput::Master
    );
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.5, -0.5, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    Renderer::with_plugin_host(&project, &FakeHost::default())
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.09765625, 0.048828125]; 16]);
    project
        .set_audio_track_mixer(
            track,
            project.audio_track(track).unwrap().mixer().with_muted(true),
        )
        .unwrap();
    Renderer::with_plugin_host(&project, &FakeHost::default())
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]; 16]);
}

#[test]
fn send_gain_edits_ramp_independently_without_resetting_processors() {
    let (mut project, track) = audio_project();
    let first = project.add_bus_track("First");
    let second = project.add_bus_track("Second");
    project
        .add_mixer_effect(track.into(), device("counter", 0.5))
        .unwrap();
    project.set_track_send(track, first, 0.5).unwrap();
    project.set_track_send(track, second, 0.25).unwrap();
    let host = FakeHost::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 600];
    renderer.render_block(0, &mut output);
    let resets = host.0.lock().unwrap().resets;
    let mut edited = project.clone();
    edited.set_track_send(track, first, 0.0).unwrap();
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(600, &mut output);
    let expected_last = 1199.0_f32 * 0.0001 * 0.5 * 1.25;
    assert!((output[599][0] - expected_last).abs() < 1e-7);
    assert!(
        output[0][0] > 600.0 * 0.0001 * 0.5 * 1.25,
        "send level should ramp"
    );
    assert_eq!(host.0.lock().unwrap().loads, 1);
    assert_eq!(host.0.lock().unwrap().resets, resets);
    edited.remove_track_send(track, first).unwrap();
    assert!(
        !renderer.update_parameters(&edited),
        "removing a destination changes topology"
    );
}

#[test]
fn source_solo_keeps_its_returns_while_bus_solo_isolates_the_wet_return() {
    let (mut project, first) = audio_project();
    let second = project.add_audio_track("Second");
    let clip = project.audio_track(first).unwrap().clips()[0].clone();
    project.add_audio_clip(second, clip).unwrap();
    let bus = project.add_bus_track("Shared return");
    project.set_track_send(first, bus, 0.5).unwrap();
    project.set_track_send(second, bus, 0.25).unwrap();
    let mut output = [[0.0; 2]; 16];
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[2.75; 2]; 16]);
    project
        .set_audio_track_mixer(first, MixerSettings::default().with_soloed(true))
        .unwrap();
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[1.5; 2]; 16]);
    project
        .set_audio_track_mixer(first, MixerSettings::default())
        .unwrap();
    project
        .set_bus_track_mixer(bus, MixerSettings::default().with_soloed(true))
        .unwrap();
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(output, [[0.75; 2]; 16]);
    project
        .set_bus_track_mixer(bus, MixerSettings::default())
        .unwrap();
    let group = project.add_group_track("Selected section");
    project.set_track_group(first, Some(group)).unwrap();
    project.set_track_send(group, bus, 0.25).unwrap();
    project
        .set_group_track_mixer(group, MixerSettings::default().with_soloed(true))
        .unwrap();
    Renderer::new(&project)
        .unwrap()
        .render_block(0, &mut output);
    assert_eq!(
        output, [[1.75; 2]; 16],
        "group solo keeps member and group sends"
    );
}
