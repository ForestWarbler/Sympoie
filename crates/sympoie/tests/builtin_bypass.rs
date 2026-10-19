use sympoie::Frame;
use sympoie::clip::{MidiClip, MidiNote};
use sympoie::instrument::{Instrument, NoteEvent, SubtractiveSettings, Waveform};
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginProcessContext, PluginProcessor,
};
use sympoie::project::Project;
use sympoie::render::Renderer;
use sympoie::track::MidiTrackId;

fn note(duration_ticks: u64) -> MidiNote {
    MidiNote {
        start_tick: 0,
        duration_ticks,
        key: 12,
        velocity: 127,
        channel: 0,
        slide_ticks: 0,
    }
}

fn fixture() -> (Project, MidiTrackId) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_midi_track("Subtractive");
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings {
                waveform: Waveform::Sine,
                cutoff_hz: 20_000.0,
                attack_seconds: 0.0,
                decay_seconds: 0.0,
                sustain: 1.0,
                release_seconds: 0.01,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 4_000,
                notes: vec![note(4_000)],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    (project, track)
}

fn device(kind: PluginKind) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Bias.vst3".into(),
            class_id: "bias".into(),
            name: "Bias".into(),
            vendor: "Tests".into(),
            kind,
        },
        vec![],
        vec![],
    )
}

#[test]
fn bypass_preserves_parameters_and_resets_when_the_builtin_is_removed_or_replaced() {
    let (mut project, track) = fixture();
    assert!(!project.midi_track(track).unwrap().instrument_bypassed());
    let original = *project.midi_track(track).unwrap().instrument().unwrap();
    project
        .set_midi_track_instrument_bypassed(track, true)
        .unwrap();
    assert_eq!(
        project.midi_track(track).unwrap().instrument(),
        Some(&original)
    );
    let changed = Instrument::Subtractive(SubtractiveSettings::default());
    project
        .set_midi_track_instrument(track, Some(changed))
        .unwrap();
    assert!(project.midi_track(track).unwrap().instrument_bypassed());
    project.set_midi_track_instrument(track, None).unwrap();
    assert!(!project.midi_track(track).unwrap().instrument_bypassed());
    project
        .set_midi_track_instrument_bypassed(track, true)
        .unwrap();
    assert!(!project.midi_track(track).unwrap().instrument_bypassed());
    project
        .set_midi_track_instrument(track, Some(original))
        .unwrap();
    project
        .set_midi_track_instrument_bypassed(track, true)
        .unwrap();
    project
        .set_plugin_instrument(track, Some(device(PluginKind::Instrument)))
        .unwrap();
    assert!(!project.midi_track(track).unwrap().instrument_bypassed());
    assert!(
        project
            .midi_track(track)
            .unwrap()
            .plugin_instrument()
            .is_some()
    );
}

#[test]
fn live_bypass_preserves_voice_phase_and_delivers_note_offs_while_disabled() {
    let (project, track) = fixture();
    let mut bypassed = project.clone();
    bypassed
        .set_midi_track_instrument_bypassed(track, true)
        .unwrap();
    let mut renderer = Renderer::new(&bypassed).unwrap();
    let mut reference = Renderer::new(&project).unwrap();
    let mut output = [[0.0; 2]; 1024];
    let mut expected = output;
    renderer.render_block(0, &mut output);
    reference.render_block(0, &mut expected);
    assert_eq!(output, [[0.0; 2]; 1024]);
    assert!(expected.iter().any(|frame| frame[0] != 0.0));
    assert!(renderer.update_parameters(&project));
    renderer.render_block(1024, &mut output);
    reference.render_block(1024, &mut expected);
    assert_eq!(output[240..], expected[240..]);
    assert!(renderer.update_parameters(&bypassed));
    renderer.render_block(2048, &mut output);
    assert_eq!(output[240..], [[0.0; 2]; 784]);
    for start in [3072, 4096, 5120] {
        renderer.render_block(start, &mut output);
    }
    assert!(renderer.update_parameters(&project));
    renderer.render_block(6144, &mut output);
    assert_eq!(
        output, [[0.0; 2]; 1024],
        "disabled notes must still release"
    );
}

#[test]
fn bypass_during_audition_ramps_and_keeps_preview_note_lifetimes() {
    let (project, track) = fixture();
    let mut bypassed = project.clone();
    bypassed
        .set_midi_track_instrument_bypassed(track, true)
        .unwrap();
    let mut renderer = Renderer::new(&bypassed).unwrap();
    let mut reference = Renderer::new(&project).unwrap();
    assert!(renderer.audition_notes(1, &[note(2_000)]));
    assert!(reference.audition_notes(1, &[note(2_000)]));
    let mut output = [[0.0; 2]; 512];
    let mut expected = output;
    renderer.render_monitor_block(0, &mut output);
    reference.render_monitor_block(0, &mut expected);
    assert_eq!(output, [[0.0; 2]; 512]);
    assert!(renderer.update_parameters(&project));
    renderer.render_monitor_block(0, &mut output);
    reference.render_monitor_block(0, &mut expected);
    assert_eq!(output[240..], expected[240..]);
    assert!(output[240..].iter().any(|frame| frame[0] != 0.0));
    assert!(renderer.update_parameters(&bypassed));
    for _ in 0..5 {
        renderer.render_monitor_block(0, &mut output);
    }
    assert!(renderer.update_parameters(&project));
    renderer.render_monitor_block(0, &mut output);
    assert_eq!(output, [[0.0; 2]; 512]);
    assert!(!renderer.has_auditions());
}

struct BiasHost;
#[derive(Debug)]
struct Bias;
impl PluginHost for BiasHost {
    fn load(&self, _: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        Ok(Box::new(Bias))
    }
}
impl PluginProcessor for Bias {
    fn reset(&mut self) {}
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, _: u32, _: f64) {}
    fn process(&mut self, frames: &mut [Frame], _: PluginProcessContext) {
        for frame in frames {
            for channel in frame {
                *channel += 0.25;
            }
        }
    }
}

#[test]
fn bypass_silences_only_the_builtin_and_keeps_effects_in_playback_and_monitoring() {
    let (mut project, track) = fixture();
    project
        .set_midi_track_instrument_bypassed(track, true)
        .unwrap();
    project
        .add_effect(track.into(), device(PluginKind::Effect))
        .unwrap();
    let mut renderer = Renderer::with_plugin_host(&project, &BiasHost).unwrap();
    let mut output = [[0.0; 2]; 512];
    renderer.render_block(0, &mut output);
    assert_eq!(output, [[0.25; 2]; 512]);
    renderer.render_monitor_block(512, &mut output);
    assert_eq!(output, [[0.25; 2]; 512]);
}
