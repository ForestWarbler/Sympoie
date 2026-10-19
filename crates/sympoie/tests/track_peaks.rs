use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiNote};
use sympoie::instrument::{Instrument, NoteEvent, SubtractiveSettings};
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginProcessContext, PluginProcessor,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings};
use sympoie::transport::Transport;

fn audio(project: &mut Project, samples: Vec<Frame>) -> AudioTrackId {
    let count = samples.len() as u64;
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    let track = project.add_audio_track("Audio");
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: count,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    track
}

fn effect(operation: &str) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Meter.vst3".into(),
            class_id: operation.into(),
            name: operation.into(),
            vendor: "Tests".into(),
            kind: PluginKind::Effect,
        },
        vec![],
        vec![],
    )
}

struct Host;

impl PluginHost for Host {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        Ok(Box::new(Processor {
            operation: instance.descriptor.class_id.clone(),
            history: [[0.0; 2]; 4],
            cursor: 0,
        }))
    }
}

#[derive(Debug)]
struct Processor {
    operation: String,
    history: [Frame; 4],
    cursor: usize,
}

impl PluginProcessor for Processor {
    fn reset(&mut self) {
        self.history.fill([0.0; 2]);
        self.cursor = 0;
    }

    fn latency_samples(&self) -> u32 {
        if self.operation == "delay" { 4 } else { 0 }
    }

    fn set_parameter(&mut self, _: u32, _: f64) {}

    fn event(&mut self, _: NoteEvent) {}

    fn process(&mut self, frames: &mut [Frame], _: PluginProcessContext) {
        for (index, frame) in frames.iter_mut().enumerate() {
            match self.operation.as_str() {
                "gain" => *frame = frame.map(|sample| sample * 2.0),
                "nonfinite" if index == 0 => *frame = [f32::NAN, f32::INFINITY],
                "delay" => {
                    std::mem::swap(frame, &mut self.history[self.cursor]);
                    self.cursor = (self.cursor + 1) % self.history.len();
                }
                _ => {}
            }
        }
    }
}

#[test]
fn independent_stereo_peaks_follow_effects_pan_faders_and_bus_summing() {
    let mut project = Project::new();
    let first = audio(&mut project, vec![[0.4, -0.8]; 300]);
    let second = audio(&mut project, vec![[-0.2, 0.1]; 300]);
    let bus = project.add_group_track("Bus");
    let dormant = audio(&mut project, vec![[4.0; 2]; 300]);
    project.add_effect(first.into(), effect("gain")).unwrap();
    project
        .set_audio_track_mixer(
            first,
            MixerSettings::new(0.5, 0.5, AudioOutput::Group(bus)).unwrap(),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            second,
            MixerSettings::new(1.0, 0.0, AudioOutput::Group(bus)).unwrap(),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            dormant,
            MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            bus,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.5, 0.0, AudioOutput::Output).unwrap())
        .unwrap();

    let mut transport = Transport::with_plugin_host(&project, &Host).unwrap();
    transport.play();
    let mut output = [[0.0; 2]; 300];
    transport.process_block(&mut output);
    let peaks = transport.track_peaks();
    assert_eq!(peaks.len(), project.tracks().len());
    assert_eq!(peaks[1], [0.2, 0.8]);
    assert_eq!(peaks[2], [0.2, 0.1]);
    assert_eq!(peaks[3], [0.0, 0.35]);
    assert_eq!(peaks[0], [0.0, 0.175]);
    assert_eq!(peaks[4], [0.0; 2]);
    assert!(output.iter().all(|frame| *frame == [0.0, -0.175]));
}

#[test]
fn peaks_span_loop_segments_and_clear_on_new_silent_or_empty_calls() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    audio(&mut project, vec![[0.2; 2], [0.3; 2], [0.8; 2], [0.1; 2]]);
    let mut transport = Transport::new(&project).unwrap();
    transport.set_loop_ticks(0, 4).unwrap();
    transport.seek_frame(2);
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 3]);
    assert_eq!(transport.track_peaks(), [[0.8; 2]; 2]);
    transport.process_block(&mut [[0.0; 2]; 1]);
    assert_eq!(transport.track_peaks(), [[0.3; 2]; 2]);
    transport.pause();
    transport.process_block(&mut [[99.0; 2]; 1]);
    assert_eq!(transport.track_peaks(), [[0.0; 2]; 2]);
    transport.play();
    transport.process_block(&mut [[0.0; 2]; 1]);
    assert_eq!(transport.track_peaks(), [[0.8; 2]; 2]);
    transport.process_block(&mut []);
    assert_eq!(transport.track_peaks(), [[0.0; 2]; 2]);
}

#[test]
fn stopped_audition_peaks_follow_instrument_and_bus_output() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Preview");
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    let bus = project.add_group_track("Bus");
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(0.5, -1.0, AudioOutput::Group(bus)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            bus,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let mut transport = Transport::new(&project).unwrap();
    assert!(transport.audition_notes(
        1,
        &[MidiNote {
            start_tick: 0,
            duration_ticks: 960,
            key: 60,
            velocity: 100,
            channel: 0,
            slide_ticks: 0,
        }]
    ));
    let mut output = [[0.0; 2]; 512];
    transport.process_monitor_block(&mut output);
    let peaks = transport.track_peaks();
    assert!(peaks[1][0] > 0.0);
    assert_eq!(peaks[1][1], 0.0);
    assert_eq!(peaks[2], [peaks[1][0] * 0.5, 0.0]);
    assert_eq!(peaks[0], peaks[2]);
    let output_peak = output
        .iter()
        .map(|frame| frame[0].abs())
        .fold(0.0, f32::max);
    assert_eq!(peaks[0][0], output_peak);
    assert_eq!(transport.position_frame(), 0);
    transport.process_monitor_block(&mut []);
    assert_eq!(transport.track_peaks(), [[0.0; 2]; 3]);
}

#[test]
fn nonfinite_plugin_output_does_not_poison_peaks() {
    let mut project = Project::new();
    let track = audio(&mut project, vec![[0.25, -0.5]; 2]);
    project
        .add_effect(track.into(), effect("nonfinite"))
        .unwrap();
    let mut renderer = Renderer::with_plugin_host(&project, &Host).unwrap();
    renderer.render_block(0, &mut [[0.0; 2]; 2]);
    assert_eq!(renderer.track_peaks(), [[0.25, 0.5]; 2]);
    renderer.render_block(2, &mut []);
    assert_eq!(renderer.track_peaks(), [[0.0; 2]; 2]);
}

#[test]
fn seek_preroll_does_not_light_meters_for_discarded_audio() {
    let mut project = Project::new();
    let mut samples = vec![[0.0; 2]; 16];
    samples[4] = [0.9; 2];
    let track = audio(&mut project, samples);
    project.add_effect(track.into(), effect("delay")).unwrap();
    let mut transport = Transport::with_plugin_host(&project, &Host).unwrap();
    transport.seek_frame(8);
    transport.play();
    let mut output = [[99.0; 2]; 1];
    transport.process_block(&mut output);
    assert_eq!(output, [[0.0; 2]; 1]);
    assert_eq!(transport.track_peaks(), [[0.0; 2]; 2]);
}
