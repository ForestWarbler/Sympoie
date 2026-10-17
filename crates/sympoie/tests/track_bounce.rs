use sympoie::Frame;
use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::bounce::{BounceMode, BouncePlan, apply_bounce};
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, NoteEvent, SubtractiveSettings};
use sympoie::plugin::{
    PLUGIN_TAIL_FRAMES, PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginParameter,
    PluginProcessContext, PluginProcessor, PluginSidechainInput, SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings, Track, TrackId};

fn project() -> Project {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    project
}

fn audio(project: &mut Project, name: &str, start: u64, samples: Vec<Frame>) -> AudioTrackId {
    let track = project.add_audio_track(name);
    let frame_count = samples.len() as u64;
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: start,
                frame_count,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    track
}

fn render(project: &Project, host: Option<&dyn PluginHost>, end: u64) -> Vec<Frame> {
    let mut renderer = match host {
        Some(host) => Renderer::with_plugin_host(project, host).unwrap(),
        None => Renderer::new(project).unwrap(),
    };
    let mut frames = vec![[0.0; 2]; end as usize];
    for (index, chunk) in frames.chunks_mut(113).enumerate() {
        renderer.render_block((index * 113) as u64, chunk);
    }
    frames
}

fn bounced(plan: &BouncePlan, host: Option<&dyn PluginHost>) -> AudioBuffer {
    let frames = render(&plan.project, host, plan.end_frame);
    AudioBuffer::new(frames[plan.start_frame as usize..].to_vec()).unwrap()
}

#[test]
fn bounce_retains_target_mixer_automation_and_does_not_bake_master_automation() {
    let mut source = project();
    let audio = audio(&mut source, "Automated", 0, vec![[1.0; 2]; 300]);
    let envelope = |target, from, to| {
        AutomationLane::new(
            target,
            vec![
                AutomationPoint {
                    tick: 0,
                    value: from,
                },
                AutomationPoint {
                    tick: 299,
                    value: to,
                },
            ],
        )
        .unwrap()
    };
    source
        .set_track_automation(audio, envelope(AutomationTarget::TrackGain, 0.0, 1.0))
        .unwrap();
    source
        .set_track_automation(audio, envelope(AutomationTarget::TrackPan, 0.0, 1.0))
        .unwrap();
    source
        .set_track_automation(
            source.master_track_id(),
            envelope(AutomationTarget::TrackGain, 0.2, 0.4),
        )
        .unwrap();
    let expected = render(&source, None, 300);
    let plan = BouncePlan::new(&source, 1).unwrap();
    assert!(plan.project.tracks()[0].automation().is_empty());
    assert!(plan.project.tracks()[1].automation().is_empty());
    let samples = bounced(&plan, None);
    assert!(samples.samples().iter().all(|frame| *frame == [1.0; 2]));
    for mode in [BounceMode::InPlace, BounceMode::Duplicate] {
        let mut result = source.clone();
        let index = apply_bounce(&mut result, &plan, samples.clone(), mode).unwrap();
        assert_eq!(
            result.tracks()[index].automation(),
            source.tracks()[1].automation()
        );
        assert_close(&render(&result, None, 300), &expected);
    }
}

fn assert_close(actual: &[Frame], expected: &[Frame]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-6 && (actual[1] - expected[1]).abs() < 1e-6,
            "frame {index}: {actual:?} != {expected:?}"
        );
    }
}

fn device(name: &str) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Bounce.vst3".into(),
            class_id: name.into(),
            name: name.into(),
            vendor: "Tests".into(),
            kind: PluginKind::Effect,
        },
        vec![],
        vec![],
    )
}

struct Host;
impl PluginHost for Host {
    fn load(&self, settings: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        if settings.descriptor.name == "missing" {
            return Err("missing plugin".into());
        }
        Ok(Box::new(Processor {
            name: settings.descriptor.name.clone(),
            amount: settings
                .parameters
                .first()
                .map_or(1.0, |value| value.value as f32),
            previous: [0.0; 2],
            history: vec![[0.0; 2]; 37],
            cursor: 0,
        }))
    }
}

#[derive(Debug)]
struct Processor {
    name: String,
    amount: f32,
    previous: Frame,
    history: Vec<Frame>,
    cursor: usize,
}
impl PluginProcessor for Processor {
    fn reset(&mut self) {
        self.previous = [0.0; 2];
        self.history.fill([0.0; 2]);
        self.cursor = 0;
    }
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, _: u32, value: f64) {
        self.amount = value as f32;
    }
    fn latency_samples(&self) -> u32 {
        if self.name == "tail" { 37 } else { 0 }
    }
    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        self.process_with_sidechain(frames, None, context);
    }
    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        _: PluginProcessContext,
    ) {
        for (index, frame) in frames.iter_mut().enumerate() {
            let mut value = *frame;
            for (channel, sample) in value.iter_mut().enumerate() {
                *sample = match self.name.as_str() {
                    "double" => *sample * 2.0,
                    "gain" => *sample * self.amount,
                    "tail" => *sample + self.previous[channel] * 0.5,
                    "sidechain" => *sample + sidechain.as_ref().unwrap().frames[index][channel],
                    _ => *sample,
                };
            }
            self.previous = value;
            if self.name == "tail" {
                *frame = self.history[self.cursor];
                self.history[self.cursor] = value;
                self.cursor = (self.cursor + 1) % self.history.len();
            } else {
                *frame = value;
            }
        }
    }
}

#[test]
fn audio_bounce_keeps_position_color_gain_pan_and_parent_routing() {
    let mut project = project();
    let track = audio(&mut project, "Clip", 73, vec![[0.25, 0.5]; 251]);
    let group = project.add_group_track("Bus");
    project.set_track_color(track, Some(0x56789a)).unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.5, -0.5, AudioOutput::Group(group)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(0.25, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.5, 0.0, AudioOutput::Output).unwrap())
        .unwrap();
    let expected = render(&project, None, 400);
    let plan = BouncePlan::new(&project, 1).unwrap();
    assert_eq!(
        (plan.start_tick, plan.start_frame, plan.end_frame),
        (73, 73, 324)
    );
    let output = bounced(&plan, None);
    assert_eq!(output.samples()[0], [0.25, 0.5]);
    let index = apply_bounce(&mut project, &plan, output, BounceMode::InPlace).unwrap();
    assert_eq!(index, 1);
    assert_eq!(project.tracks()[index].color(), Some(0x56789a));
    assert_eq!(
        project.tracks()[index].mixer().output(),
        AudioOutput::Group(group)
    );
    assert_close(&render(&project, None, 400), &expected);
}

#[test]
fn midi_bounce_includes_release_and_becomes_an_audio_track() {
    let mut project = project();
    let track = project.add_midi_track("Synth");
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings {
                attack_seconds: 0.0,
                release_seconds: 0.01,
                ..Default::default()
            })),
        )
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                start_tick: 80,
                offset_ticks: 0,
                length_ticks: 130,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 130,
                    key: 60,
                    velocity: 100,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    assert_eq!(plan.end_frame, 80 + 130 + 480);
    let expected = render(&project, None, plan.end_frame + 100);
    let output = bounced(&plan, None);
    assert!(output.samples()[130..].iter().any(|frame| frame[0] != 0.0));
    apply_bounce(&mut project, &plan, output, BounceMode::InPlace).unwrap();
    assert!(matches!(&project.tracks()[1], Track::Audio(audio) if audio.clips().len() == 1));
    assert_close(&render(&project, None, plan.end_frame + 100), &expected);
}

#[test]
fn duplicate_preserves_source_instrument_and_mixer_but_mutes_it() {
    let mut project = project();
    let track = project.add_midi_track("Keep me");
    project
        .set_midi_track_instrument(track, Some(Instrument::Subtractive(Default::default())))
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                start_tick: 0,
                offset_ticks: 0,
                length_ticks: 100,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 100,
                    key: 64,
                    velocity: 100,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    project
        .set_midi_track_mixer(
            track,
            MixerSettings::new(0.5, 0.25, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let original = project.clone();
    let plan = BouncePlan::new(&project, 1).unwrap();
    let expected = render(&project, None, plan.end_frame);
    let output = bounced(&plan, None);
    let index = apply_bounce(&mut project, &plan, output, BounceMode::Duplicate).unwrap();
    assert_eq!(index, 2);
    let Track::Midi(source) = &project.tracks()[1] else {
        panic!()
    };
    assert_eq!(
        source.instrument(),
        original.midi_track(track).unwrap().instrument()
    );
    assert_eq!(source.clips(), original.midi_track(track).unwrap().clips());
    assert_eq!(source.mixer().gain(), 0.5);
    assert_eq!(source.mixer().pan(), 0.25);
    assert!(source.mixer().muted());
    assert!(!project.tracks()[index].mixer().muted());
    assert_close(&render(&project, None, plan.end_frame), &expected);
}

#[test]
fn muted_tracks_can_be_bounced_without_losing_their_saved_gain() {
    let mut project = project();
    let track = audio(&mut project, "Muted", 0, vec![[0.5; 2]; 50]);
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.25, 0.0, AudioOutput::Master)
                .unwrap()
                .with_muted(true),
        )
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    let output = bounced(&plan, None);
    assert_eq!(output.samples()[0], [0.5; 2]);
    apply_bounce(&mut project, &plan, output, BounceMode::InPlace).unwrap();
    assert!(project.tracks()[1].mixer().muted());
    assert_eq!(project.tracks()[1].mixer().gain(), 0.25);
    assert!(
        render(&project, None, 50)
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
}

#[test]
fn duplicate_remaps_later_groups_and_sidechain_sources() {
    let mut project = project();
    audio(&mut project, "Target", 0, vec![[0.25; 2]; 30]);
    let group = project.add_group_track("Later group");
    let member = audio(&mut project, "Member", 0, vec![[0.5; 2]; 30]);
    project.set_track_group(member, Some(group)).unwrap();
    let receiver = audio(&mut project, "Receiver", 0, vec![[0.125; 2]; 30]);
    let effect = project
        .add_effect(receiver.into(), device("sidechain"))
        .unwrap();
    project
        .set_plugin_sidechain(
            effect,
            Some(SidechainRoute {
                source_track: 3,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PostFader,
            }),
        )
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    let output = bounced(&plan, None);
    apply_bounce(&mut project, &plan, output, BounceMode::Duplicate).unwrap();
    let TrackId::Group(remapped) = project.track_ids().nth(3).unwrap() else {
        panic!()
    };
    assert_eq!(
        project.tracks()[4].mixer().output(),
        AudioOutput::Group(remapped)
    );
    assert_eq!(
        project.tracks()[5].effects()[0]
            .sidechain
            .unwrap()
            .source_track,
        4
    );
    project.processing_order().unwrap();
}

#[test]
fn group_bounce_flattens_members_and_repairs_earlier_removed_indices() {
    let mut project = project();
    let child = audio(&mut project, "Child", 41, vec![[0.25; 2]; 50]);
    let group = project.add_group_track("Flatten");
    project.set_track_group(child, Some(group)).unwrap();
    project
        .set_group_track_mixer(
            group,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let later = project.add_group_track("Later");
    let other = audio(&mut project, "Other", 0, vec![[0.5; 2]; 100]);
    project.set_track_group(other, Some(later)).unwrap();
    let expected = render(&project, None, 100);
    let plan = BouncePlan::new(&project, 2).unwrap();
    let output = bounced(&plan, None);
    assert_eq!(
        apply_bounce(&mut project, &plan, output, BounceMode::InPlace).unwrap(),
        1
    );
    assert_eq!(project.tracks().len(), 4);
    assert_eq!(project.tracks()[1].name(), "Flatten");
    let TrackId::Group(later) = project.track_ids().nth(2).unwrap() else {
        panic!()
    };
    assert_eq!(
        project.tracks()[3].mixer().output(),
        AudioOutput::Group(later)
    );
    assert_close(&render(&project, None, 100), &expected);
}

#[test]
fn duplicate_group_keeps_its_members_and_only_mutes_the_original_bus() {
    let mut project = project();
    let group = project.add_group_track("Drums");
    let child = audio(&mut project, "Kick", 0, vec![[0.25; 2]; 50]);
    project.set_track_group(child, Some(group)).unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    let expected = render(&project, None, 50);
    let output = bounced(&plan, None);
    apply_bounce(&mut project, &plan, output, BounceMode::Duplicate).unwrap();
    assert!(matches!(project.tracks()[1], Track::Group(_)));
    assert!(project.tracks()[1].mixer().muted());
    assert!(!project.tracks()[3].mixer().muted());
    assert_eq!(
        project.tracks()[3].mixer().output(),
        AudioOutput::Group(group)
    );
    assert_close(&render(&project, None, 50), &expected);
}

#[test]
fn only_own_effects_are_baked_with_latency_and_tail_preserved() {
    let mut project = project();
    let track = audio(&mut project, "Impulse", 63, vec![[0.25; 2]]);
    let parent = project.add_group_track("Parent");
    project.set_track_group(track, Some(parent)).unwrap();
    project.add_effect(track.into(), device("tail")).unwrap();
    project.add_effect(parent.into(), device("double")).unwrap();
    project
        .add_effect(project.master_track_id().into(), device("double"))
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    assert_eq!(plan.end_frame, 64 + PLUGIN_TAIL_FRAMES);
    let output = bounced(&plan, Some(&Host));
    assert_eq!(
        &output.samples()[..3],
        &[[0.25; 2], [0.125; 2], [0.0625; 2]]
    );
    let expected = render(&project, Some(&Host), 500);
    apply_bounce(&mut project, &plan, output, BounceMode::InPlace).unwrap();
    assert!(project.tracks()[1].effects().is_empty());
    assert_eq!(project.tracks()[2].effects().len(), 1);
    assert_eq!(project.master_track().effects().len(), 1);
    assert_close(&render(&project, Some(&Host), 500), &expected);
}

#[test]
fn sidechain_inputs_are_retained_without_leaking_into_the_main_mix() {
    let mut project = project();
    let target = audio(&mut project, "Target", 80, vec![[0.25; 2]; 50]);
    audio(&mut project, "Source", 0, vec![[0.5; 2]; 500]);
    let effect = project
        .add_effect(target.into(), device("sidechain"))
        .unwrap();
    project
        .set_plugin_sidechain(
            effect,
            Some(SidechainRoute {
                source_track: 2,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PostFader,
            }),
        )
        .unwrap();
    audio(
        &mut project,
        "Unrelated late clip",
        10_000_000,
        vec![[1.0; 2]; 50],
    );
    let plan = BouncePlan::new(&project, 1).unwrap();
    assert_eq!(plan.end_frame, 130 + PLUGIN_TAIL_FRAMES);
    let output = bounced(&plan, Some(&Host));
    assert_eq!(output.samples()[0], [0.75; 2]);
    assert_eq!(output.samples()[50], [0.5; 2]);
}

#[test]
fn irrelevant_missing_plugins_and_late_clips_do_not_block_bounce() {
    let mut project = project();
    audio(&mut project, "Target", 10, vec![[0.25; 2]; 50]);
    let other = audio(&mut project, "Missing", 1_000_000, vec![[0.5; 2]; 500]);
    project.add_effect(other.into(), device("missing")).unwrap();
    project
        .add_effect(project.master_track_id().into(), device("missing"))
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    assert_eq!(plan.end_frame, 60);
    assert_eq!(bounced(&plan, None).len(), 50);
}

#[test]
fn group_replacement_refuses_to_remove_an_external_sidechain_source() {
    let mut project = project();
    let group = project.add_group_track("Source bus");
    let child = audio(&mut project, "Child", 0, vec![[0.25; 2]; 50]);
    project.set_track_group(child, Some(group)).unwrap();
    let receiver = audio(&mut project, "External", 0, vec![[0.5; 2]; 50]);
    let effect = project
        .add_effect(receiver.into(), device("sidechain"))
        .unwrap();
    project
        .set_plugin_sidechain(
            effect,
            Some(SidechainRoute {
                source_track: 2,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PostEffects,
            }),
        )
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    let output = bounced(&plan, None);
    assert!(
        apply_bounce(&mut project, &plan, output, BounceMode::InPlace)
            .unwrap_err()
            .contains("sidechain")
    );
    assert_eq!(project.tracks().len(), 4);
    assert!(matches!(project.tracks()[1], Track::Group(_)));
    let output = bounced(&plan, None);
    assert!(apply_bounce(&mut project, &plan, output, BounceMode::Duplicate).is_ok());
}

#[test]
fn invalid_targets_lengths_and_changed_mixer_fail_without_editing() {
    let mut project = project();
    let track = project.add_audio_track("Empty");
    assert!(BouncePlan::new(&project, 0).is_err());
    assert!(BouncePlan::new(&project, 1).is_err());
    assert!(BouncePlan::new(&project, 999).is_err());
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 50]).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 50,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    let wrong = AudioBuffer::new(vec![[0.5; 2]; 49]).unwrap();
    assert!(apply_bounce(&mut project, &plan, wrong, BounceMode::Duplicate).is_err());
    assert_eq!(project.tracks().len(), 2);
    project
        .set_audio_track_mixer(track, MixerSettings::default().with_muted(true))
        .unwrap();
    let output = bounced(&plan, None);
    assert!(apply_bounce(&mut project, &plan, output, BounceMode::InPlace).is_err());
    assert_eq!(project.tracks().len(), 2);
}

#[test]
fn live_mute_ramps_without_rebuilding_or_losing_the_fader_value() {
    let mut project = project();
    let track = audio(&mut project, "Live mute", 0, vec![[0.5; 2]; 2000]);
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.25, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    let mut renderer = Renderer::new(&project).unwrap();
    let mut output = vec![[0.0; 2]; 500];
    renderer.render_block(0, &mut output);
    assert_eq!(output[499], [0.125; 2]);
    let mut muted = project.clone();
    muted
        .set_audio_track_mixer(
            track,
            project.audio_track(track).unwrap().mixer().with_muted(true),
        )
        .unwrap();
    assert!(renderer.update_parameters(&muted));
    renderer.render_block(500, &mut output);
    assert!(output[0][0] > 0.0 && output[0][0] < 0.125);
    assert_eq!(output[499], [0.0; 2]);
    assert!(renderer.update_parameters(&project));
    renderer.render_block(1000, &mut output);
    assert_eq!(output[499], [0.125; 2]);
}

#[test]
fn duplicate_preserves_all_outgoing_sidechain_taps_when_original_is_muted() {
    let mut project = project();
    let source = audio(&mut project, "Source", 0, vec![[0.25; 2]; 50]);
    project.add_effect(source.into(), device("double")).unwrap();
    project
        .set_audio_track_mixer(
            source,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    for tap in [
        SidechainTap::PreEffects,
        SidechainTap::PostEffects,
        SidechainTap::PostFader,
    ] {
        let receiver = audio(&mut project, "Receiver", 0, vec![[0.0; 2]; 50]);
        let effect = project
            .add_effect(receiver.into(), device("sidechain"))
            .unwrap();
        project
            .set_plugin_sidechain(
                effect,
                Some(SidechainRoute {
                    source_track: 1,
                    bus_index: 1,
                    gain: 1.0,
                    tap,
                }),
            )
            .unwrap();
    }
    let expected = render(&project, Some(&Host), 100);
    let plan = BouncePlan::new(&project, 1).unwrap();
    assert!(
        plan.validate_mode(BounceMode::InPlace)
            .unwrap_err()
            .contains("pre-effects")
    );
    plan.validate_mode(BounceMode::Duplicate).unwrap();
    let output = bounced(&plan, Some(&Host));
    apply_bounce(&mut project, &plan, output, BounceMode::Duplicate).unwrap();
    assert_eq!(
        project.tracks()[3].effects()[0]
            .sidechain
            .unwrap()
            .source_track,
        1
    );
    assert_eq!(
        project.tracks()[4].effects()[0]
            .sidechain
            .unwrap()
            .source_track,
        1
    );
    assert_eq!(
        project.tracks()[5].effects()[0]
            .sidechain
            .unwrap()
            .source_track,
        2
    );
    assert_close(&render(&project, Some(&Host), 100), &expected);
}

#[test]
fn midi_bounce_extent_follows_trimmed_visible_notes_and_ignores_hidden_notes() {
    let mut project = project();
    let track = project.add_midi_track("Trimmed");
    project
        .set_midi_track_instrument(
            track,
            Some(Instrument::Subtractive(SubtractiveSettings {
                release_seconds: 0.01,
                ..Default::default()
            })),
        )
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                start_tick: 80,
                offset_ticks: 500,
                length_ticks: 100,
                notes: vec![
                    MidiNote {
                        start_tick: 0,
                        duration_ticks: 50,
                        key: 60,
                        velocity: 100,
                        channel: 0,
                    },
                    MidiNote {
                        start_tick: 510,
                        duration_ticks: 20,
                        key: 62,
                        velocity: 100,
                        channel: 0,
                    },
                    MidiNote {
                        start_tick: u64::MAX - 1,
                        duration_ticks: 1,
                        key: 64,
                        velocity: 100,
                        channel: 0,
                    },
                ],
            },
        )
        .unwrap();
    let plan = BouncePlan::new(&project, 1).unwrap();
    assert_eq!(plan.end_frame, 80 + 30 + 480);
    let expected = render(&project, None, plan.end_frame + 20);
    let output = bounced(&plan, None);
    apply_bounce(&mut project, &plan, output, BounceMode::InPlace).unwrap();
    assert_close(&render(&project, None, plan.end_frame + 20), &expected);
}

#[test]
fn bounce_preserves_mixer_inserts_automation_and_bus_route_without_baking_them() {
    for enabled in [true, false] {
        let mut source = project();
        let target = audio(&mut source, "Designed", 0, vec![[0.25; 2]; 300]);
        source.add_effect(target.into(), device("double")).unwrap();
        let mut insert = device("gain");
        insert.parameters.push(PluginParameter {
            id: 7,
            name: "Amount".into(),
            unit: String::new(),
            value: 0.5,
            steps: 0,
            read_only: false,
        });
        let plugin = source.add_mixer_effect(target.into(), insert).unwrap();
        let envelope = AutomationLane::new(
            AutomationTarget::PluginParameter {
                plugin,
                parameter: 7,
            },
            vec![
                AutomationPoint {
                    tick: 0,
                    value: 0.25,
                },
                AutomationPoint {
                    tick: 299,
                    value: 0.75,
                },
            ],
        )
        .unwrap()
        .with_enabled(enabled);
        source
            .set_track_automation(target, envelope.clone())
            .unwrap();
        let bus = source.add_bus_track("Mix bus");
        source
            .set_audio_track_mixer(
                target,
                MixerSettings::new(0.5, 0.0, AudioOutput::Bus(bus)).unwrap(),
            )
            .unwrap();
        source
            .add_mixer_effect(bus.into(), device("double"))
            .unwrap();
        let expected = render(&source, Some(&Host), 300);
        let plan = BouncePlan::new(&source, 1).unwrap();
        assert!(plan.project.tracks()[1].mixer_effects().is_empty());
        assert!(plan.project.tracks()[1].automation().is_empty());
        let samples = bounced(&plan, Some(&Host));
        assert!(
            samples.samples()[..300]
                .iter()
                .all(|frame| *frame == [0.5; 2])
        );
        for mode in [BounceMode::InPlace, BounceMode::Duplicate] {
            let mut result = source.clone();
            let index = apply_bounce(&mut result, &plan, samples.clone(), mode).unwrap();
            assert!(result.tracks()[index].effects().is_empty());
            let inserted = &result.tracks()[index].mixer_effects()[0];
            assert_eq!(inserted.descriptor.name, "gain");
            assert_eq!(inserted.parameters[0].value, 0.5);
            let lane = &result.tracks()[index].automation()[0];
            assert_eq!(
                lane.target(),
                AutomationTarget::PluginParameter {
                    plugin: inserted.id,
                    parameter: 7
                }
            );
            assert_eq!(lane.points(), envelope.points());
            assert_eq!(lane.enabled(), enabled);
            if mode == BounceMode::InPlace {
                assert_eq!(inserted.id, plugin);
            } else {
                assert_ne!(inserted.id, plugin);
                assert_eq!(result.tracks()[1].mixer_effects()[0].id, plugin);
                assert!(result.tracks()[1].mixer().muted());
            }
            let TrackId::Bus(bus) = result.track_ids().last().unwrap() else {
                panic!()
            };
            assert_eq!(
                result.tracks()[index].mixer().output(),
                AudioOutput::Bus(bus)
            );
            let mut ids: Vec<_> = result
                .tracks()
                .iter()
                .flat_map(Track::all_effects)
                .map(|effect| effect.id)
                .collect();
            let count = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), count);
            result.processing_order().unwrap();
            assert_close(&render(&result, Some(&Host), 300), &expected);
        }
    }
}

#[test]
fn bounce_does_not_load_target_mixer_inserts_or_their_sidechain_dependencies() {
    let mut source = project();
    let target = audio(&mut source, "Target", 0, vec![[0.25; 2]; 50]);
    let insert = source
        .add_mixer_effect(target.into(), device("missing"))
        .unwrap();
    source
        .set_plugin_sidechain(
            insert,
            Some(SidechainRoute {
                source_track: 0,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PreEffects,
            }),
        )
        .unwrap_err();
    let unrelated = audio(&mut source, "Mixer detector", 1_000_000, vec![[1.0; 2]; 10]);
    source
        .add_mixer_effect(unrelated.into(), device("missing"))
        .unwrap();
    source
        .set_plugin_sidechain(
            insert,
            Some(SidechainRoute {
                source_track: 2,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PreEffects,
            }),
        )
        .unwrap();
    let plan = BouncePlan::new(&source, 1).unwrap();
    assert_eq!(plan.end_frame, 50);
    let samples = bounced(&plan, None);
    apply_bounce(&mut source, &plan, samples, BounceMode::InPlace).unwrap();
    assert_eq!(source.tracks()[1].mixer_effects()[0].id, insert);
    assert_eq!(
        source
            .plugin(insert)
            .unwrap()
            .sidechain
            .unwrap()
            .source_track,
        2
    );
}

#[test]
fn group_bounce_keeps_own_mixer_inserts_but_refuses_to_bake_members_mixer_inserts() {
    let mut source = project();
    let group = source.add_group_track("Group");
    let child = audio(&mut source, "Child", 0, vec![[0.25; 2]; 50]);
    source.set_track_group(child, Some(group)).unwrap();
    let own_insert = source
        .add_mixer_effect(group.into(), device("double"))
        .unwrap();
    let plan = BouncePlan::new(&source, 1).unwrap();
    let expected = render(&source, Some(&Host), 50);
    let samples = bounced(&plan, None);
    let mut result = source.clone();
    let index = apply_bounce(&mut result, &plan, samples, BounceMode::InPlace).unwrap();
    assert_eq!(result.tracks()[index].mixer_effects()[0].id, own_insert);
    assert_close(&render(&result, Some(&Host), 50), &expected);
    source
        .add_mixer_effect(child.into(), device("double"))
        .unwrap();
    assert!(
        BouncePlan::new(&source, 1)
            .unwrap_err()
            .contains("individually")
    );
    source.add_bus_track("Hidden mixing bus");
    assert!(BouncePlan::new(&source, source.tracks().len() - 1).is_err());
}

#[test]
fn in_place_group_bounce_preserves_mixer_sidechain_sources_by_refusing_removed_members() {
    let mut source = project();
    let group = source.add_group_track("Group");
    let child = audio(&mut source, "Child", 0, vec![[0.25; 2]; 50]);
    source.set_track_group(child, Some(group)).unwrap();
    let insert = source
        .add_mixer_effect(group.into(), device("sidechain"))
        .unwrap();
    source
        .set_plugin_sidechain(
            insert,
            Some(SidechainRoute {
                source_track: 2,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PostEffects,
            }),
        )
        .unwrap();
    let plan = BouncePlan::new(&source, 1).unwrap();
    assert!(
        plan.validate_mode(BounceMode::InPlace)
            .unwrap_err()
            .contains("sidechain")
    );
    let expected = render(&source, Some(&Host), 50);
    let samples = bounced(&plan, None);
    apply_bounce(&mut source, &plan, samples, BounceMode::Duplicate).unwrap();
    assert_close(&render(&source, Some(&Host), 50), &expected);
}

#[test]
fn bounce_retains_multiple_post_fader_sends_without_baking_returns_or_duplicating_them() {
    let mut source = project();
    let target = audio(&mut source, "Target", 0, vec![[0.25; 2]; 50]);
    let first = source.add_bus_track("First return");
    let second = source.add_bus_track("Second return");
    let other = audio(&mut source, "Other", 0, vec![[0.125; 2]; 50]);
    source.add_effect(target.into(), device("double")).unwrap();
    source
        .add_mixer_effect(target.into(), device("double"))
        .unwrap();
    source
        .add_mixer_effect(first.into(), device("double"))
        .unwrap();
    source
        .add_mixer_effect(second.into(), device("double"))
        .unwrap();
    source.set_track_send(target, first, 0.5).unwrap();
    source.set_track_send(target, second, 0.25).unwrap();
    source.set_track_send(other, second, 0.5).unwrap();
    let expected = render(&source, Some(&Host), 50);
    assert_eq!(expected, [[2.75; 2]; 50]);
    let plan = BouncePlan::new(&source, 1).unwrap();
    assert!(plan.project.tracks()[1].sends().is_empty());
    let samples = bounced(&plan, Some(&Host));
    assert_eq!(samples.samples()[..50], [[0.5; 2]; 50]);
    for mode in [BounceMode::InPlace, BounceMode::Duplicate] {
        let mut result = source.clone();
        let index = apply_bounce(&mut result, &plan, samples.clone(), mode).unwrap();
        let ids: Vec<_> = result.track_ids().collect();
        let shift = usize::from(mode == BounceMode::Duplicate);
        let (TrackId::Bus(first), TrackId::Bus(second)) = (ids[2 + shift], ids[3 + shift]) else {
            panic!()
        };
        assert_eq!(result.tracks()[index].sends().len(), 2);
        assert_eq!(result.tracks()[index].sends()[0].destination, first);
        assert_eq!(result.tracks()[index].sends()[0].gain, 0.5);
        assert_eq!(result.tracks()[index].sends()[1].destination, second);
        assert_eq!(result.tracks()[4 + shift].sends()[0].destination, second);
        assert_eq!(result.tracks()[4 + shift].sends()[0].gain, 0.5);
        assert_close(&render(&result, Some(&Host), 50), &expected);
    }
}

#[test]
fn group_bounce_preserves_own_sends_and_rejects_member_sends() {
    let mut source = project();
    let child = audio(&mut source, "Child", 0, vec![[0.25; 2]; 50]);
    let group = source.add_group_track("Group");
    let bus = source.add_bus_track("Return");
    source.set_track_group(child, Some(group)).unwrap();
    source.set_track_send(group, bus, 0.5).unwrap();
    source
        .add_mixer_effect(bus.into(), device("double"))
        .unwrap();
    let plan = BouncePlan::new(&source, 2).unwrap();
    let expected = render(&source, Some(&Host), 50);
    let samples = bounced(&plan, None);
    let mut result = source.clone();
    let index = apply_bounce(&mut result, &plan, samples, BounceMode::InPlace).unwrap();
    let TrackId::Bus(bus_after) = result.track_ids().nth(2).unwrap() else {
        panic!()
    };
    assert_eq!(result.tracks()[index].sends()[0].destination, bus_after);
    assert_close(&render(&result, Some(&Host), 50), &expected);
    source.set_track_send(child, bus, 0.25).unwrap();
    assert!(
        BouncePlan::new(&source, 2)
            .unwrap_err()
            .contains("individually")
    );
}

#[test]
fn bounce_sidechain_dependencies_include_bus_send_feeders_with_disconnected_main_outputs() {
    let mut source = project();
    let target = audio(&mut source, "Target", 0, vec![[0.25; 2]; 50]);
    let detector = audio(&mut source, "Detector", 0, vec![[0.5; 2]; 50]);
    let bus = source.add_bus_track("Detector return");
    source
        .set_audio_track_mixer(
            detector,
            MixerSettings::default().with_output(AudioOutput::Disconnected),
        )
        .unwrap();
    source.set_track_send(detector, bus, 0.5).unwrap();
    source
        .add_mixer_effect(bus.into(), device("double"))
        .unwrap();
    let effect = source
        .add_effect(target.into(), device("sidechain"))
        .unwrap();
    source
        .set_plugin_sidechain(
            effect,
            Some(SidechainRoute {
                source_track: 3,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PostFader,
            }),
        )
        .unwrap();
    let plan = BouncePlan::new(&source, 1).unwrap();
    let samples = bounced(&plan, Some(&Host));
    assert_eq!(samples.samples()[..50], [[0.75; 2]; 50]);
}
