use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginParameter,
    PluginProcessContext, PluginProcessor, PluginSidechainInput, SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, MixerSettings, Track, TrackId};

fn device(kind: PluginKind) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/TrackOrder.vst3".into(),
            class_id: "track-order".into(),
            name: "Track order test".into(),
            vendor: "Tests".into(),
            kind,
        },
        vec![PluginParameter {
            id: 7,
            name: "Amount".into(),
            unit: String::new(),
            value: 0.25,
            steps: 0,
            read_only: false,
        }],
        vec![1, 2, 3, 4],
    )
}

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn fixture() -> Project {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let audio = project.add_audio_track("Audio");
    let source = project.add_audio_track("Sidechain source");
    let midi = project.add_midi_track("MIDI");
    let inner = project.add_group_track("Inner");
    let outer = project.add_group_track("Outer");
    project.add_audio_track("Unrelated");
    project.set_track_group(audio, Some(inner)).unwrap();
    project.set_track_group(midi, Some(inner)).unwrap();
    project.set_track_group(inner, Some(outer)).unwrap();
    project.set_track_color(audio, Some(0x336699)).unwrap();
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::new(0.5, -0.25, AudioOutput::Group(inner)).unwrap(),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            source,
            MixerSettings::new(0.25, 0.5, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            outer,
            MixerSettings::new(0.75, 0.25, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    for (track, signal) in [(audio, 0.125), (source, 0.25)] {
        let asset = project.add_asset(
            AudioBuffer::new(
                (0..512)
                    .map(|index| [signal * (index % 13) as f32, signal * 0.5])
                    .collect(),
            )
            .unwrap(),
        );
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 7,
                    start_tick: 11,
                    frame_count: 400,
                    gain: 0.75,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                },
            )
            .unwrap();
    }
    let mut midi_clip = MidiClip::new(19, 320);
    midi_clip.notes.push(MidiNote {
        start_tick: 31,
        duration_ticks: 101,
        key: 60,
        velocity: 87,
        channel: 2,
    });
    project.add_midi_clip(midi, midi_clip).unwrap();
    project
        .set_plugin_instrument(midi, Some(device(PluginKind::Instrument)))
        .unwrap();
    let targets: [TrackId; 5] = [
        audio.into(),
        midi.into(),
        inner.into(),
        outer.into(),
        project.master_track_id().into(),
    ];
    for (index, target) in targets.into_iter().enumerate() {
        let id = project
            .add_effect(target, device(PluginKind::Effect))
            .unwrap();
        project
            .set_plugin_sidechain(
                id,
                Some(SidechainRoute {
                    source_track: if index == 4 { 4 } else { 2 },
                    bus_index: 2,
                    gain: 0.5,
                    tap: [
                        SidechainTap::PreEffects,
                        SidechainTap::PostEffects,
                        SidechainTap::PostFader,
                    ][index % 3],
                }),
            )
            .unwrap();
        if index == 1 {
            project.set_plugin_bypassed(id, true).unwrap();
        }
    }
    project
}

#[test]
fn invalid_permutations_leave_content_routes_and_plugin_ids_unchanged() {
    let mut project = fixture();
    let before = encoded(&project);
    for order in [
        vec![],
        vec![0, 1, 2, 3, 4, 5],
        vec![0, 1, 2, 3, 4, 5, 6, 7],
        vec![0, 1, 2, 3, 4, 5, 5],
        vec![0, 1, 2, 3, 4, 5, 7],
        vec![0, 1, 2, 3, 4, 5, usize::MAX],
        vec![1, 0, 2, 3, 4, 5, 6],
        vec![0, 1, 2, 0, 4, 5, 6],
    ] {
        assert_eq!(
            project.reorder_tracks(&order),
            Err(ProjectError::InvalidTrackOrder),
            "unexpected result for {order:?}"
        );
        assert_eq!(encoded(&project), before);
    }
}

#[test]
fn identity_order_including_a_master_only_project_is_supported() {
    let mut empty = Project::new();
    assert_eq!(empty.reorder_tracks(&[0]).unwrap(), [0]);
    let mut project = fixture();
    let before = encoded(&project);
    assert_eq!(
        project.reorder_tracks(&[0, 1, 2, 3, 4, 5, 6]).unwrap(),
        [0, 1, 2, 3, 4, 5, 6]
    );
    assert_eq!(encoded(&project), before);
}

#[test]
fn reordering_preserves_nested_groups_all_sidechain_taps_and_track_contents() {
    let before = fixture();
    let mut project = before.clone();
    let order = [0, 5, 4, 3, 6, 1, 2];
    let mapping = project.reorder_tracks(&order).unwrap();
    assert_eq!(mapping, [0, 5, 6, 3, 2, 1, 4]);
    let ids: Vec<_> = project.track_ids().collect();
    for (new, &old) in order.iter().enumerate() {
        let old_track = &before.tracks()[old];
        let new_track = &project.tracks()[new];
        assert_eq!(new_track.name(), old_track.name());
        assert_eq!(new_track.color(), old_track.color());
        let output = match old_track.mixer().output() {
            AudioOutput::Group(old_group) => {
                let old_index = before
                    .track_ids()
                    .position(|id| id == old_group.into())
                    .unwrap();
                let TrackId::Group(new_group) = ids[mapping[old_index]] else {
                    panic!("group type changed");
                };
                AudioOutput::Group(new_group)
            }
            other => other,
        };
        assert_eq!(*new_track.mixer(), old_track.mixer().with_output(output));
        let expected_effects: Vec<_> = old_track
            .effects()
            .iter()
            .cloned()
            .map(|mut effect| {
                if let Some(route) = &mut effect.sidechain {
                    route.source_track = mapping[route.source_track];
                }
                effect
            })
            .collect();
        assert_eq!(new_track.effects(), expected_effects);
        match (old_track, new_track) {
            (Track::Audio(old), Track::Audio(new)) => assert_eq!(old.clips(), new.clips()),
            (Track::Midi(old), Track::Midi(new)) => {
                assert_eq!(old.clips(), new.clips());
                assert_eq!(old.instrument(), new.instrument());
                assert_eq!(old.plugin_instrument(), new.plugin_instrument());
            }
            (Track::Group(_), Track::Group(_)) | (Track::Master(_), Track::Master(_)) => {}
            _ => panic!("track type changed"),
        }
    }
    let TrackId::Group(inner) = ids[2] else {
        panic!("missing inner group")
    };
    let TrackId::Group(outer) = ids[1] else {
        panic!("missing outer group")
    };
    assert_eq!(
        project.group_members(inner).unwrap().collect::<Vec<_>>(),
        [ids[3], ids[5]]
    );
    assert_eq!(
        project.group_members(outer).unwrap().collect::<Vec<_>>(),
        [ids[2]]
    );
    assert!(project.processing_order().is_ok());
    project.reorder_tracks(&mapping).unwrap();
    assert_eq!(
        encoded(&project),
        encoded(&before),
        "inverse order restores all saved content"
    );
}

#[test]
fn reordered_projects_round_trip_and_keep_allocating_unique_plugin_ids() {
    let mut project = fixture();
    project.reorder_tracks(&[0, 5, 4, 3, 6, 1, 2]).unwrap();
    let bytes = encoded(&project);
    let mut loaded = read_project(&bytes[..]).unwrap();
    assert_eq!(encoded(&loaded), bytes);
    let next = project
        .add_effect(project.master_track_id().into(), device(PluginKind::Effect))
        .unwrap();
    assert_eq!(
        loaded
            .add_effect(loaded.master_track_id().into(), device(PluginKind::Effect))
            .unwrap(),
        next
    );
    assert_eq!(encoded(&loaded), encoded(&project));
}

struct Host;

impl PluginHost for Host {
    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        Ok(Box::new(Processor(instance.descriptor.kind)))
    }
}

#[derive(Debug)]
struct Processor(PluginKind);

impl PluginProcessor for Processor {
    fn reset(&mut self) {}
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, _: u32, _: f64) {}
    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        self.process_with_sidechain(frames, None, context);
    }
    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        _: PluginProcessContext,
    ) {
        if self.0 == PluginKind::Instrument {
            frames.fill([0.03125; 2]);
        } else if let Some(sidechain) = sidechain {
            assert_eq!(sidechain.bus_index, 2);
            for (frame, sidechain) in frames.iter_mut().zip(sidechain.frames) {
                for (sample, input) in frame.iter_mut().zip(sidechain) {
                    *sample = *sample * 0.5 + input;
                }
            }
        }
    }
}

fn render(project: &Project) -> Vec<Frame> {
    let mut renderer = Renderer::with_plugin_host(project, &Host).unwrap();
    let mut frames = vec![[0.0; 2]; 640];
    for (index, chunk) in frames.chunks_mut(113).enumerate() {
        renderer.render_block((index * 113) as u64, chunk);
    }
    frames
}

#[test]
fn reordered_nested_groups_and_sidechains_render_the_same_audio() {
    let mut project = fixture();
    let before = render(&project);
    assert!(before.iter().any(|frame| frame[0] != 0.0));
    project.reorder_tracks(&[0, 5, 4, 3, 6, 1, 2]).unwrap();
    let after = render(&project);
    for (index, (before, after)) in before.iter().zip(&after).enumerate() {
        for channel in 0..2 {
            assert!(
                (before[channel] - after[channel]).abs() < 1e-6,
                "render changed at frame {index}: {before:?} != {after:?}"
            );
        }
    }
    let loaded = read_project(&encoded(&project)[..]).unwrap();
    assert_eq!(render(&loaded), after);
}
