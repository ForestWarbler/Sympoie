use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginParameter,
    PluginProcessContext, PluginProcessor,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::track::{AudioOutput, MidiTrackId, MixerSettings};
use sympoie::transport::{PlaybackState, Transport};

#[derive(Debug, Default)]
struct Device {
    native_key: bool,
    scheduled: HashSet<u64>,
    events: Vec<NoteEvent>,
    contexts: Vec<PluginProcessContext>,
    frames: usize,
    resets: usize,
    gain: f32,
}

#[derive(Debug, Default)]
struct Host(Mutex<HashMap<u64, Arc<Mutex<Device>>>>);

impl Host {
    fn device(&self, id: u64) -> Arc<Mutex<Device>> {
        Arc::clone(&self.0.lock().unwrap()[&id])
    }
}

impl PluginHost for Host {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        let device = Arc::new(Mutex::new(Device {
            gain: instance.parameters[0].value as f32,
            ..Device::default()
        }));
        assert!(
            self.0
                .lock()
                .unwrap()
                .insert(instance.id, Arc::clone(&device))
                .is_none(),
            "monitoring must reuse the loaded instance"
        );
        Ok(Box::new(Processor {
            device,
            instrument: instance.descriptor.kind == PluginKind::Instrument,
        }))
    }
}

#[derive(Debug)]
struct Processor {
    device: Arc<Mutex<Device>>,
    instrument: bool,
}

impl PluginProcessor for Processor {
    fn reset(&mut self) {
        let mut device = self.device.lock().unwrap();
        device.resets += 1;
        device.native_key = false;
        device.scheduled.clear();
    }

    fn event(&mut self, event: NoteEvent) {
        let mut device = self.device.lock().unwrap();
        device.events.push(event);
        match event {
            NoteEvent::PitchBend { .. } => {}
            NoteEvent::On { id, .. } => {
                device.scheduled.insert(id);
            }
            NoteEvent::Off { id } => {
                device.scheduled.remove(&id);
            }
        }
    }

    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        if frames.is_empty() {
            return;
        }
        let mut device = self.device.lock().unwrap();
        device.contexts.push(context);
        device.frames += frames.len();
        let voices = device.scheduled.len() as f32 + if device.native_key { 0.25 } else { 0.0 };
        for frame in frames {
            for sample in frame {
                *sample = if self.instrument {
                    voices * device.gain
                } else {
                    *sample * device.gain
                };
            }
        }
    }

    fn set_parameter(&mut self, id: u32, value: f64) {
        assert_eq!(id, 1);
        self.device.lock().unwrap().gain = value as f32;
    }
}

fn plugin(kind: PluginKind, gain: f64) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Monitor.vst3".into(),
            class_id: format!("{kind:?}"),
            name: "Monitor".into(),
            vendor: "Test".into(),
            kind,
        },
        vec![PluginParameter {
            id: 1,
            name: "Gain".into(),
            unit: String::new(),
            value: gain,
            steps: 0,
            read_only: false,
        }],
        Vec::new(),
    )
}

fn fixture() -> (Project, MidiTrackId, u64) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let midi = project.add_midi_track("Native keyboard");
    project
        .set_plugin_instrument(midi, Some(plugin(PluginKind::Instrument, 1.0)))
        .unwrap();
    let id = project
        .midi_track(midi)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    (project, midi, id)
}

fn note(start: u64, duration: u64) -> MidiNote {
    MidiNote {
        start_tick: start,
        duration_ticks: duration,
        key: 60,
        velocity: 100,
        channel: 0,
        slide_ticks: 0,
    }
}

#[test]
fn live_input_has_no_fixed_gate_and_note_off_preserves_arrangement_voices() {
    let (mut project, midi, instrument_id) = fixture();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 60_000,
                notes: vec![note(0, 60_000)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    assert!(transport.live_midi(1, [0x90, 60, 100]));
    let mut block = vec![[0.0; 2]; 12_345];
    transport.process_block(&mut block);
    assert!(block.iter().all(|frame| *frame == [2.0; 2]));
    transport.process_block(&mut block);
    assert!(block.iter().all(|frame| *frame == [2.0; 2]));
    assert!(transport.live_midi(1, [0x80, 60, 0]));
    transport.process_block(&mut block);
    assert!(block.iter().all(|frame| *frame == [1.0; 2]));
    assert_eq!(
        host.device(instrument_id).lock().unwrap().scheduled,
        HashSet::from([0])
    );
    assert!(!transport.has_auditions());
}

#[test]
fn live_input_sustain_retrigger_channels_and_stop_do_not_leave_stuck_notes() {
    let (project, _, instrument_id) = fixture();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    let mut block = [[0.0; 2]; 127];
    assert!(transport.live_midi(1, [0x91, 60, 100]));
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[1.0; 2]; 127]);
    transport.live_midi(1, [0xb1, 64, 127]);
    transport.live_midi(1, [0x91, 60, 0]);
    transport.live_midi(1, [0x91, 62, 100]);
    transport.process_monitor_block(&mut block);
    assert_eq!(
        block, [[2.0; 2]; 127],
        "a new key must not release a sustained key"
    );
    transport.live_midi(1, [0x81, 62, 0]);
    transport.live_midi(1, [0xb0, 64, 0]);
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[2.0; 2]; 127], "sustain is per MIDI channel");
    transport.live_midi(1, [0xb1, 64, 0]);
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[0.0; 2]; 127]);
    transport.live_midi(1, [0x90, 70, 127]);
    transport.process_monitor_block(&mut block);
    transport.live_midi(1, [0x90, 70, 127]);
    transport.process_monitor_block(&mut block);
    assert_eq!(
        block, [[1.0; 2]; 127],
        "one key retriggers without stacking leaked gates"
    );
    transport.stop();
    assert!(!transport.has_auditions());
    assert!(
        host.device(instrument_id)
            .lock()
            .unwrap()
            .scheduled
            .is_empty()
    );
}

#[test]
fn independent_input_ports_do_not_release_each_others_keys_or_sustain() {
    let (project, _, _) = fixture();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    let mut block = [[0.0; 2]; 127];
    transport.live_midi_from(1, 20, [0x90, 60, 100]);
    transport.live_midi_from(1, 20, [0xb0, 64, 127]);
    transport.process_monitor_block(&mut block);
    transport.live_midi_from(1, 20, [0x80, 60, 0]);
    transport.live_midi_from(1, 30, [0x90, 60, 100]);
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[2.0; 2]; 127]);
    transport.live_midi_from(1, 30, [0xb0, 64, 0]);
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[2.0; 2]; 127]);
    transport.live_midi_from(1, 20, [0xb0, 64, 0]);
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[1.0; 2]; 127]);
    transport.live_midi_from(1, 30, [0x80, 60, 0]);
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[0.0; 2]; 127]);
    assert!(!transport.has_auditions());
}

#[test]
fn live_input_source_and_voice_capacity_stays_bounded_and_all_notes_release() {
    let (project, _, instrument_id) = fixture();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    for source in 0..1000 {
        transport.live_midi_from(1, source, [0xb0, 64, 127]);
        transport.live_midi_from(1, source, [0x90, 60, 100]);
    }
    let mut block = [[0.0; 2]; 127];
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[128.0; 2]; 127]);
    for source in 0..1000 {
        transport.live_midi_from(1, source, [0x80, 60, 0]);
        transport.live_midi_from(1, source, [0xb0, 64, 0]);
    }
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[0.0; 2]; 127]);
    assert!(!transport.has_auditions());
    assert!(
        host.device(instrument_id)
            .lock()
            .unwrap()
            .scheduled
            .is_empty()
    );
    assert!(transport.live_midi_from(1, 1001, [0xb0, 64, 127]));
    assert!(transport.live_midi_from(1, 1001, [0x90, 60, 100]));
    transport.process_monitor_block(&mut block);
    assert_eq!(block, [[1.0; 2]; 127]);
    drop(transport);
    assert!(
        host.device(instrument_id)
            .lock()
            .unwrap()
            .scheduled
            .is_empty()
    );
}

#[test]
fn empty_stopped_track_monitors_native_keys_through_effects_and_bus_mixers() {
    let (mut project, midi, instrument_id) = fixture();
    let effect_id = project
        .add_effect(midi.into(), plugin(PluginKind::Effect, 0.5))
        .unwrap();
    let group = project.add_group_track("Bus");
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(0.5, 1.0, AudioOutput::Group(group)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.5, 0.0, AudioOutput::Output).unwrap())
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), 0);
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    assert!(transport.has_live_plugins());
    let instrument = host.device(instrument_id);
    instrument.lock().unwrap().native_key = true;
    for size in [13, 600, 31] {
        let mut output = vec![[99.0; 2]; size];
        transport.process_monitor_block(&mut output);
        assert_eq!(output, vec![[0.0, 0.015625]; size]);
        assert_eq!(transport.position_frame(), 0);
        assert_eq!(transport.state(), PlaybackState::Stopped);
    }
    for id in [instrument_id, effect_id] {
        let device = host.device(id);
        let device = device.lock().unwrap();
        assert_eq!(device.resets, 0);
        assert_eq!(device.frames, 644);
        assert!(device.events.is_empty());
        assert!(
            device
                .contexts
                .iter()
                .all(|context| !context.playing && context.frame == 0 && context.bpm == 3000.0)
        );
    }
    let empty = Project::new();
    assert!(!Transport::new(&empty).unwrap().has_live_plugins());
}

#[test]
fn paused_monitor_excludes_arrangement_audio_and_notes_without_moving_the_cursor() {
    let (mut project, midi, instrument_id) = fixture();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 1000,
                notes: vec![note(0, 1000)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let audio = project.add_audio_track("Arrangement audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[10.0; 2]; 1000]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 1000,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.seek_frame(200);
    transport.pause();
    let instrument = host.device(instrument_id);
    instrument.lock().unwrap().native_key = true;
    let mut output = [[99.0; 2]; 700];
    transport.process_block(&mut output);
    assert_eq!(
        output, [[0.0; 2]; 700],
        "offline playback remains silent while paused"
    );
    transport.process_monitor_block(&mut output);
    assert_eq!(output, [[0.25; 2]; 700]);
    assert_eq!(transport.position_frame(), 200);
    assert_eq!(transport.state(), PlaybackState::Paused);
    let device = instrument.lock().unwrap();
    assert!(device.events.is_empty());
    assert!(
        device
            .contexts
            .iter()
            .all(|context| !context.playing && context.frame == 200)
    );
    assert_eq!(device.resets, 1, "only the explicit seek resets the plugin");
}

#[test]
fn monitoring_releases_scheduled_notes_once_and_resume_preserves_native_keys() {
    let (mut project, midi, instrument_id) = fixture();
    let effect_id = project
        .add_effect(midi.into(), plugin(PluginKind::Effect, 1.0))
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 150,
                notes: vec![note(0, 100), note(10, 60)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 50]);
    let instrument = host.device(instrument_id);
    let effect = host.device(effect_id);
    let resets = instrument.lock().unwrap().resets;
    let effect_resets = effect.lock().unwrap().resets;
    instrument.lock().unwrap().native_key = true;
    transport.pause();
    transport.process_monitor_block(&mut []);
    assert_eq!(instrument.lock().unwrap().events.len(), 2);
    let mut output = [[0.0; 2]; 128];
    transport.process_monitor_block(&mut output);
    assert_eq!(output, [[0.25; 2]; 128]);
    assert_eq!(instrument.lock().unwrap().events.len(), 4);
    transport.process_monitor_block(&mut output);
    assert_eq!(output, [[0.25; 2]; 128]);
    assert_eq!(
        instrument.lock().unwrap().events.len(),
        4,
        "monitor blocks must not repeat note-offs"
    );

    let mut edited = project.clone();
    edited.set_plugin_parameter(instrument_id, 1, 0.5).unwrap();
    assert!(transport.update_parameters(&edited));
    transport.process_monitor_block(&mut output);
    assert_eq!(output, [[0.125; 2]; 128]);
    assert_eq!(transport.position_frame(), 50);
    transport.play();
    let mut resumed = [[0.0; 2]; 70];
    transport.process_block(&mut resumed);
    assert_eq!(resumed[..20], [[1.125; 2]; 20]);
    assert_eq!(resumed[20..50], [[0.625; 2]; 30]);
    assert_eq!(resumed[50..], [[0.125; 2]; 20]);
    assert_eq!(transport.position_frame(), 120);
    let device = instrument.lock().unwrap();
    assert!(device.native_key);
    assert!(device.scheduled.is_empty());
    assert_eq!(device.resets, resets);
    assert_eq!(
        device
            .events
            .iter()
            .filter(|event| matches!(event, NoteEvent::On { .. }))
            .count(),
        4
    );
    assert_eq!(
        device
            .events
            .iter()
            .filter(|event| matches!(event, NoteEvent::Off { .. }))
            .count(),
        4
    );
    assert_eq!(effect.lock().unwrap().resets, effect_resets);
    assert_eq!(effect.lock().unwrap().frames, 50 + 3 * 128 + 70);
}

#[test]
fn pausing_at_audible_frame_preserves_native_keys_while_rewinding_the_schedule() {
    let (mut project, midi, instrument_id) = fixture();
    let effect_id = project
        .add_effect(midi.into(), plugin(PluginKind::Effect, 1.0))
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 100,
                notes: vec![note(0, 45), note(45, 20)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 50]);
    let instrument = host.device(instrument_id);
    let effect = host.device(effect_id);
    let resets = instrument.lock().unwrap().resets;
    let effect_resets = effect.lock().unwrap().resets;
    instrument.lock().unwrap().native_key = true;

    transport.pause_at_frame(40);
    assert_eq!(transport.position_frame(), 40);
    assert_eq!(transport.state(), PlaybackState::Paused);
    let mut output = [[0.0; 2]; 64];
    transport.process_monitor_block(&mut output);
    assert_eq!(output, [[0.25; 2]; 64]);
    let released_events = instrument.lock().unwrap().events.len();
    assert_eq!(released_events, 4);
    assert!(matches!(
        instrument.lock().unwrap().events.last(),
        Some(NoteEvent::Off { id: 1 })
    ));
    transport.process_monitor_block(&mut output);
    assert_eq!(instrument.lock().unwrap().events.len(), released_events);

    transport.play();
    let mut resumed = [[0.0; 2]; 30];
    transport.process_block(&mut resumed);
    assert_eq!(resumed[..25], [[1.25; 2]; 25]);
    assert_eq!(resumed[25..], [[0.25; 2]; 5]);
    assert_eq!(transport.position_frame(), 70);
    let device = instrument.lock().unwrap();
    assert!(device.native_key);
    assert!(device.scheduled.is_empty());
    assert_eq!(device.resets, resets);
    assert_eq!(device.events.len(), 8);
    assert!(matches!(device.events[4], NoteEvent::On { id: 0, .. }));
    assert!(
        device
            .contexts
            .iter()
            .filter(|context| !context.playing)
            .all(|context| context.frame == 40)
    );
    assert_eq!(effect.lock().unwrap().resets, effect_resets);
    assert_eq!(effect.lock().unwrap().frames, 50 + 2 * 64 + 30);
}

#[test]
fn immediate_pause_and_resume_preserves_native_keys_without_an_idle_audio_block() {
    let (mut project, midi, instrument_id) = fixture();
    let effect_id = project
        .add_effect(midi.into(), plugin(PluginKind::Effect, 1.0))
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 100,
                notes: vec![note(0, 100)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 50]);
    let instrument = host.device(instrument_id);
    let effect = host.device(effect_id);
    let resets = instrument.lock().unwrap().resets;
    let effect_resets = effect.lock().unwrap().resets;
    instrument.lock().unwrap().native_key = true;

    transport.pause_at_frame(40);
    assert_eq!(instrument.lock().unwrap().events.len(), 2);
    assert!(matches!(
        instrument.lock().unwrap().events.last(),
        Some(NoteEvent::Off { id: 0 })
    ));
    transport.play();
    let mut output = [[0.0; 2]; 10];
    transport.process_block(&mut output);
    assert_eq!(output, [[1.25; 2]; 10]);
    assert_eq!(transport.position_frame(), 50);
    let device = instrument.lock().unwrap();
    assert!(device.native_key);
    assert_eq!(device.resets, resets);
    assert_eq!(device.events.len(), 3);
    assert!(matches!(
        device.events.last(),
        Some(NoteEvent::On { id: 0, .. })
    ));
    assert!(device.contexts.iter().all(|context| context.playing));
    assert_eq!(effect.lock().unwrap().resets, effect_resets);
    assert_eq!(effect.lock().unwrap().frames, 60);
}

#[test]
fn midi_clip_edits_preserve_the_entire_builtin_preview_release() {
    use sympoie::instrument::{Instrument, SubtractiveSettings};
    let mut project = Project::new();
    let midi = project.add_midi_track("Preview release");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings {
                release_seconds: 0.18,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                start_tick: 0,
                offset_ticks: 0,
                length_ticks: 960,
                notes: vec![note(0, 960)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let mut reference = Transport::new(&project).unwrap();
    let mut edited = Transport::new(&project).unwrap();
    for transport in [&mut reference, &mut edited] {
        assert!(transport.begin_audition_notes(1, &[note(0, 1)]));
        transport.process_monitor_block(&mut [[0.0; 2]; 5000]);
        transport.release_audition_notes();
    }
    let mut changed = project.clone();
    changed
        .set_midi_clip_notes(
            midi,
            0,
            vec![MidiNote {
                key: 72,
                ..note(0, 720)
            }],
        )
        .unwrap();
    assert!(!edited.update_parameters(&changed));
    assert!(edited.update_midi_clips(&changed));
    let mut expected = vec![[0.0; 2]; 10_000];
    let mut actual = vec![[0.0; 2]; 10_000];
    reference.process_monitor_block(&mut expected);
    edited.process_monitor_block(&mut actual);
    assert_eq!(
        actual, expected,
        "editing must preserve the running envelope"
    );
    assert!(actual[4800..6000].iter().any(|frame| frame[0] != 0.0));
    assert!(actual[9000..].iter().all(|frame| *frame == [0.0; 2]));
    assert!(!edited.has_auditions());
}

#[test]
fn midi_schedule_updates_chase_arrangement_without_resetting_preview_or_live_input() {
    let (mut project, midi, instrument_id) = fixture();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                start_tick: 0,
                offset_ticks: 0,
                length_ticks: 10_000,
                notes: vec![note(0, 10_000)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    assert!(transport.begin_audition_notes(1, &[note(0, 1)]));
    assert!(transport.live_midi(1, [0x90, 60, 100]));
    transport.process_block(&mut [[0.0; 2]; 64]);
    let instrument = host.device(instrument_id);
    let resets = instrument.lock().unwrap().resets;
    let mut changed = project.clone();
    changed
        .set_midi_clip_notes(
            midi,
            0,
            vec![MidiNote {
                key: 72,
                ..note(0, 1000)
            }],
        )
        .unwrap();
    assert!(transport.update_midi_clips(&changed));
    assert_eq!(transport.position_frame(), 64);
    assert_eq!(transport.state(), PlaybackState::Playing);
    assert_eq!(
        transport.duration_frames(),
        changed.duration_frames().unwrap()
    );
    let mut output = [[0.0; 2]; 64];
    transport.process_block(&mut output);
    assert_eq!(output, [[3.0; 2]; 64]);
    {
        let device = instrument.lock().unwrap();
        assert_eq!(device.resets, resets);
        assert!(matches!(
            device.events[3..],
            [
                NoteEvent::Off { id: 0 },
                NoteEvent::On { id: 0, key: 72, .. }
            ]
        ));
    }
    assert!(
        !transport.update_parameters(&project),
        "stale clip contents must not pass as a parameter edit"
    );
    assert!(transport.update_parameters(&changed));
    let mut next = changed.clone();
    next.set_midi_clip_notes(
        midi,
        0,
        vec![MidiNote {
            key: 74,
            ..note(0, 10_000)
        }],
    )
    .unwrap();
    assert!(transport.update_midi_clips(&next));
    assert!(!transport.update_parameters(&changed));
    assert!(transport.update_parameters(&next));
    transport.process_block(&mut output);
    assert_eq!(output, [[3.0; 2]; 64]);
    transport.release_audition_notes();
    transport.live_midi(1, [0x80, 60, 0]);
    transport.process_block(&mut output);
    assert_eq!(output, [[1.0; 2]; 64]);
    let mut incompatible = next.clone();
    incompatible.set_bpm(2000.0).unwrap();
    assert!(!transport.update_midi_clips(&incompatible));
    incompatible = next.clone();
    incompatible.add_midi_track("Additional track");
    assert!(!transport.update_midi_clips(&incompatible));
    incompatible = next.clone();
    incompatible
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    assert!(!transport.update_midi_clips(&incompatible));
    assert!(
        transport.update_parameters(&next),
        "rejected edits leave current schedule metadata intact"
    );
}

#[test]
fn held_auditions_ignore_note_lengths_and_release_only_on_request() {
    for bpm in [60.0, 3000.0, f64::MAX] {
        let (mut project, _, instrument_id) = fixture();
        project.set_bpm(bpm).unwrap();
        let host = Host::default();
        let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
        let notes = [note(u64::MAX, 0), note(0, u64::MAX), note(0, 1)];
        assert!(transport.begin_audition_notes(1, &notes));
        let mut output = vec![[0.0; 2]; sympoie::SAMPLE_RATE as usize * 2];
        transport.process_monitor_block(&mut output);
        assert!(output.iter().all(|frame| *frame == [1.0; 2]));
        assert!(transport.has_auditions());
        let instrument = host.device(instrument_id);
        assert_eq!(instrument.lock().unwrap().events.len(), 1);
        transport.release_audition_notes();
        transport.process_monitor_block(&mut output);
        assert!(output.iter().all(|frame| *frame == [0.0; 2]));
        assert!(!transport.has_auditions());
        let device = instrument.lock().unwrap();
        assert!(
            matches!(device.events.as_slice(), [NoteEvent::On { id, .. }, NoteEvent::Off { id: off }] if id == off)
        );
    }
}

#[test]
fn releasing_preview_before_processing_never_sends_a_note_on() {
    let (project, _, instrument_id) = fixture();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    assert!(transport.begin_audition_notes(1, &[note(0, 1)]));
    transport.release_audition_notes();
    assert!(!transport.has_auditions());
    let mut output = [[1.0; 2]; 64];
    transport.process_monitor_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 64]);
    assert!(host.device(instrument_id).lock().unwrap().events.is_empty());
    assert!(transport.audition_notes(1, &[note(0, 1_000_000)]));
    transport.release_audition_notes();
    assert!(!transport.has_auditions());
}

#[test]
fn held_auditions_keep_independent_gates_from_live_input_and_arrangement() {
    let (mut project, midi, instrument_id) = fixture();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 100_000,
                notes: vec![note(0, 100_000)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 1]);
    assert!(transport.live_midi_from(1, 7, [0x90, 60, 100]));
    assert!(transport.begin_audition_notes(1, &[note(0, 1)]));
    let mut output = [[0.0; 2]; 64];
    transport.process_block(&mut output);
    assert_eq!(output, [[3.0; 2]; 64]);
    transport.release_audition_notes();
    transport.process_block(&mut output);
    assert_eq!(output, [[2.0; 2]; 64]);
    assert!(transport.begin_audition_notes(1, &[note(0, 1)]));
    transport.process_block(&mut output);
    assert_eq!(output, [[3.0; 2]; 64]);
    transport.release_live_midi();
    transport.process_block(&mut output);
    assert_eq!(output, [[2.0; 2]; 64]);
    transport.release_audition_notes();
    transport.process_block(&mut output);
    assert_eq!(output, [[1.0; 2]; 64]);
    let instrument = host.device(instrument_id);
    assert_eq!(instrument.lock().unwrap().scheduled, HashSet::from([0]));
    assert!(transport.begin_audition_notes(1, &[note(0, 1)]));
    transport.process_block(&mut output);
    drop(transport);
    assert_eq!(instrument.lock().unwrap().scheduled, HashSet::from([0]));
}

#[test]
fn held_builtin_auditions_finish_the_natural_release_after_mouse_up() {
    use sympoie::instrument::{Instrument, SubtractiveSettings};
    let mut project = Project::new();
    let midi = project.add_midi_track("Held preview");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings {
                release_seconds: 0.025,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    let mut transport = Transport::new(&project).unwrap();
    assert!(transport.begin_audition_notes(1, &[note(0, 1)]));
    let mut held = vec![[0.0; 2]; sympoie::SAMPLE_RATE as usize];
    transport.process_monitor_block(&mut held);
    assert!(held[held.len() - 100..].iter().any(|frame| frame[0] != 0.0));
    assert!(transport.has_auditions());
    transport.release_audition_notes();
    assert!(transport.has_auditions());
    let mut release = [[0.0; 2]; 1300];
    transport.process_monitor_block(&mut release);
    assert!(release[..100].iter().any(|frame| frame[0] != 0.0));
    assert!(release[1200..].iter().all(|frame| *frame == [0.0; 2]));
    assert!(!transport.has_auditions());
    assert_eq!(transport.position_frame(), 0);
    assert_eq!(transport.state(), PlaybackState::Stopped);
}

#[test]
fn auditions_follow_each_note_duration_at_the_project_tempo() {
    for bpm in [60.0, 120.0, 240.0] {
        let (mut project, _, instrument_id) = fixture();
        project.set_bpm(bpm).unwrap();
        let host = Host::default();
        let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
        let preview = note(u64::MAX, sympoie::TICKS_PER_QUARTER);
        let gate = (sympoie::SAMPLE_RATE as f64 * 60.0 / bpm) as usize;
        assert!(transport.audition_notes(1, &[preview]));
        let old_gate = sympoie::SAMPLE_RATE as usize * 180 / 1000;
        let mut first = vec![[0.0; 2]; old_gate + 1];
        transport.process_monitor_block(&mut first);
        assert!(first.iter().all(|frame| *frame == [1.0; 2]), "BPM {bpm}");
        assert!(transport.has_auditions());
        let mut remaining = vec![[0.0; 2]; gate - first.len() + 17];
        transport.process_monitor_block(&mut remaining);
        assert!(
            remaining[..gate - first.len()]
                .iter()
                .all(|frame| *frame == [1.0; 2])
        );
        assert!(
            remaining[gate - first.len()..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert!(!transport.has_auditions());
        let instrument = host.device(instrument_id);
        let device = instrument.lock().unwrap();
        assert!(
            matches!(device.events.as_slice(), [NoteEvent::On { id, .. }, NoteEvent::Off { id: off }] if id == off)
        );
    }
}

#[test]
fn audition_chord_notes_release_independently_and_duplicate_pitches_use_longest_duration() {
    let (mut project, _, instrument_id) = fixture();
    project.set_bpm(120.0).unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    let quarter = sympoie::TICKS_PER_QUARTER;
    let notes = [
        note(0, quarter / 4),
        MidiNote {
            channel: 1,
            ..note(0, quarter / 2)
        },
        MidiNote {
            key: 67,
            ..note(0, quarter * 3 / 4)
        },
        note(quarter, quarter),
        note(quarter * 2, quarter / 4),
    ];
    assert!(transport.audition_notes(1, &notes));
    let mut output = vec![[0.0; 2]; 24_007];
    transport.process_monitor_block(&mut output);
    assert!(output[..12_000].iter().all(|frame| *frame == [3.0; 2]));
    assert!(
        output[12_000..18_000]
            .iter()
            .all(|frame| *frame == [2.0; 2])
    );
    assert!(
        output[18_000..24_000]
            .iter()
            .all(|frame| *frame == [1.0; 2])
    );
    assert!(output[24_000..].iter().all(|frame| *frame == [0.0; 2]));
    assert!(!transport.has_auditions());
    let instrument = host.device(instrument_id);
    let device = instrument.lock().unwrap();
    assert_eq!(device.events.len(), 6);
    let ids: Vec<_> = device.events[..3]
        .iter()
        .map(|event| {
            let NoteEvent::On { id, .. } = event else {
                panic!("missing chord note-on")
            };
            *id
        })
        .collect();
    assert_eq!(
        device.events[3..],
        [
            NoteEvent::Off { id: ids[1] },
            NoteEvent::Off { id: ids[2] },
            NoteEvent::Off { id: ids[0] }
        ]
    );
}

#[test]
fn auditions_ignore_unrepresentable_gates_and_bound_extreme_tail_arithmetic() {
    let (project, _, _) = fixture();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    assert!(!transport.audition_notes(1, &[note(0, 0), note(0, u64::MAX)]));
    assert!(!transport.has_auditions());
    assert!(transport.audition_notes(1, &[note(0, 0), note(0, 3), note(0, u64::MAX)]));
    let mut output = [[0.0; 2]; 4];
    transport.process_monitor_block(&mut output);
    assert_eq!(output, [[1.0; 2], [1.0; 2], [1.0; 2], [0.0; 2]]);
    assert!(!transport.has_auditions());

    use sympoie::instrument::{Instrument, SubtractiveSettings};
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let midi = project.add_midi_track("Long preview");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    let mut transport = Transport::new(&project).unwrap();
    assert!(transport.audition_notes(1, &[note(0, u64::MAX - 2048)]));
    transport.process_monitor_block(&mut [[0.0; 2]; 64]);
    assert!(transport.has_auditions());
    transport.stop();
    assert!(!transport.has_auditions());

    project.set_bpm(f64::MAX).unwrap();
    let mut transport = Transport::new(&project).unwrap();
    assert!(!transport.audition_notes(1, &[note(0, 1)]));
    assert!(!transport.has_auditions());
}

#[test]
fn auditions_use_plugin_effects_and_mixers_and_release_at_the_note_duration() {
    let (mut project, midi, instrument_id) = fixture();
    project
        .add_effect(midi.into(), plugin(PluginKind::Effect, 0.5))
        .unwrap();
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(0.5, 1.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.seek_frame(123);
    transport.pause();
    let instrument = host.device(instrument_id);
    instrument.lock().unwrap().native_key = true;
    let preview = MidiNote {
        key: 74,
        velocity: 72,
        channel: 3,
        ..note(999, 12_345)
    };
    assert!(transport.audition_notes(1, &[preview]));
    let gate = project
        .tempo()
        .ticks_to_frames(preview.duration_ticks)
        .unwrap() as usize;
    let mut output = vec![[0.0; 2]; gate + 17];
    transport.process_monitor_block(&mut output);
    assert!(output[..gate].iter().all(|frame| *frame == [0.0, 0.3125]));
    assert!(output[gate..].iter().all(|frame| *frame == [0.0, 0.0625]));
    assert_eq!(transport.position_frame(), 123);
    assert_eq!(transport.state(), PlaybackState::Paused);
    assert!(!transport.has_auditions());
    let device = instrument.lock().unwrap();
    assert_eq!(device.events.len(), 2);
    assert!(matches!(
        device.events[0],
        NoteEvent::On {
            key: 74,
            velocity: 72,
            channel: 3,
            ..
        }
    ));
    assert!(device.scheduled.is_empty());
    assert!(device.native_key);
    assert_eq!(
        device.resets, 1,
        "audition must not reset native plugin voices"
    );
}

#[test]
fn replacing_and_dropping_auditions_releases_only_preview_voices() {
    let (mut project, midi, instrument_id) = fixture();
    let gate = 12_345;
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: gate as u64 * 4,
                notes: vec![note(0, gate as u64 * 4)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 31]);
    let instrument = host.device(instrument_id);
    assert!(transport.audition_notes(1, &[note(0, gate as u64)]));
    let mut output = [[0.0; 2]; 43];
    transport.process_block(&mut output);
    assert_eq!(output, [[2.0; 2]; 43]);
    let next = MidiNote {
        key: 72,
        ..note(0, gate as u64)
    };
    assert!(transport.audition_notes(1, &[next, next]));
    transport.process_block(&mut output);
    assert_eq!(
        output, [[2.0; 2]; 43],
        "repeated previews must replace their prior gate"
    );
    let mut tail = vec![[0.0; 2]; gate];
    transport.process_block(&mut tail);
    assert!(tail[..gate - 43].iter().all(|frame| *frame == [2.0; 2]));
    assert!(tail[gate - 43..].iter().all(|frame| *frame == [1.0; 2]));
    assert_eq!(instrument.lock().unwrap().scheduled, HashSet::from([0]));
    assert!(transport.audition_notes(1, &[next]));
    transport.process_block(&mut output);
    drop(transport);
    assert_eq!(
        instrument.lock().unwrap().scheduled,
        HashSet::from([0]),
        "dropping a renderer must release transient plugin voices before their IDs are lost"
    );
}

#[test]
fn builtin_auditions_work_without_clips_then_finish_the_release_tail() {
    use sympoie::instrument::{Instrument, SubtractiveSettings};
    let mut project = Project::new();
    let midi = project.add_midi_track("Preview");
    let settings = SubtractiveSettings {
        release_seconds: 0.025,
        ..SubtractiveSettings::default()
    };
    project
        .set_midi_track_instrument(midi, Some(Instrument::Subtractive(settings)))
        .unwrap();
    let mut transport = Transport::new(&project).unwrap();
    assert_eq!(transport.duration_frames(), 0);
    assert!(!transport.has_live_plugins());
    let preview = note(100, sympoie::TICKS_PER_QUARTER / 2);
    let gate = project
        .tempo()
        .ticks_to_frames(preview.duration_ticks)
        .unwrap() as usize;
    assert!(transport.audition_notes(1, &[preview]));
    assert!(transport.has_auditions());
    let mut output = vec![[0.0; 2]; gate + 1300];
    transport.process_monitor_block(&mut output);
    assert!(output.iter().any(|frame| frame[0] != 0.0));
    assert!(output[gate..gate + 100].iter().any(|frame| frame[0] != 0.0));
    assert!(
        output[output.len() - 100..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert!(!transport.has_auditions());
    assert_eq!(transport.state(), PlaybackState::Stopped);
    assert_eq!(transport.position_frame(), 0);
    assert_eq!(project.midi_track(midi).unwrap().clips().len(), 0);
}

#[test]
fn audition_gate_survives_timeline_loop_wraps_and_explicit_stop_clears_it() {
    let (project, _, instrument_id) = fixture();
    let host = Host::default();
    let mut transport = Transport::with_plugin_host(&project, &host).unwrap();
    transport.set_loop_ticks(0, 41).unwrap();
    transport.play();
    let gate = 12_345;
    assert!(transport.audition_notes(1, &[note(0, gate as u64)]));
    let mut output = vec![[0.0; 2]; gate + 15];
    transport.process_block(&mut output);
    assert!(output[..gate].iter().all(|frame| *frame == [1.0; 2]));
    assert!(output[gate..].iter().all(|frame| *frame == [0.0; 2]));
    assert!(!transport.has_auditions());
    assert!(transport.audition_notes(1, &[note(0, gate as u64)]));
    transport.process_block(&mut [[0.0; 2]; 5]);
    transport.stop();
    assert!(!transport.has_auditions());
    assert!(
        host.device(instrument_id)
            .lock()
            .unwrap()
            .scheduled
            .is_empty()
    );
}
