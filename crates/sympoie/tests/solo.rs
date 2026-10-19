use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use sympoie::Frame;
use sympoie::bounce::BouncePlan;
use sympoie::clip::AudioClip;
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginProcessContext,
    PluginProcessor, PluginSidechainInput, SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings};

fn audio(project: &mut Project, value: f32) -> AudioTrackId {
    let track = project.add_audio_track("Audio");
    let asset = project.add_asset(AudioBuffer::new(vec![[value; 2]; 4_000]).unwrap());
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
                name: None,
            },
        )
        .unwrap();
    track
}

fn solo(project: &mut Project, track: AudioTrackId, selected: bool) {
    let mixer = project
        .audio_track(track)
        .unwrap()
        .mixer()
        .with_soloed(selected);
    project.set_audio_track_mixer(track, mixer).unwrap();
}

fn heard(project: &Project) -> Frame {
    let mut renderer = Renderer::new(project).unwrap();
    let mut output = [[0.0; 2]; 1];
    renderer.render_block(0, &mut output);
    output[0]
}

#[test]
fn solos_are_additive_and_mute_keeps_precedence() {
    let mut project = Project::new();
    let first = audio(&mut project, 1.0);
    let second = audio(&mut project, 2.0);
    audio(&mut project, 4.0);
    assert!(project.tracks().iter().all(|track| !track.mixer().soloed()));
    assert_eq!(heard(&project), [7.0; 2]);
    solo(&mut project, first, true);
    assert_eq!(heard(&project), [1.0; 2]);
    solo(&mut project, second, true);
    assert_eq!(heard(&project), [3.0; 2]);
    let mixer = project.audio_track(first).unwrap().mixer().with_muted(true);
    project.set_audio_track_mixer(first, mixer).unwrap();
    assert_eq!(heard(&project), [2.0; 2]);
    solo(&mut project, first, false);
    solo(&mut project, second, false);
    assert_eq!(heard(&project), [6.0; 2]);
}

#[test]
fn nested_group_solo_selects_descendants_and_member_solo_passes_only_ancestor_buses() {
    let mut project = Project::new();
    let outer = project.add_group_track("Outer");
    let inner = project.add_group_track("Inner");
    project.set_track_group(inner, Some(outer)).unwrap();
    let first = audio(&mut project, 1.0);
    let sibling = audio(&mut project, 2.0);
    let cousin = audio(&mut project, 4.0);
    audio(&mut project, 8.0);
    project.set_track_group(first, Some(inner)).unwrap();
    project.set_track_group(sibling, Some(inner)).unwrap();
    project.set_track_group(cousin, Some(outer)).unwrap();
    project
        .set_group_track_mixer(
            outer,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    solo(&mut project, first, true);
    assert_eq!(heard(&project), [0.5; 2]);
    solo(&mut project, first, false);
    let mixer = project
        .group_track(inner)
        .unwrap()
        .mixer()
        .with_soloed(true);
    project.set_group_track_mixer(inner, mixer).unwrap();
    assert_eq!(heard(&project), [1.5; 2]);
    let mixer = project
        .group_track(outer)
        .unwrap()
        .mixer()
        .with_soloed(true);
    project.set_group_track_mixer(outer, mixer).unwrap();
    assert_eq!(heard(&project), [3.5; 2]);
    let mixer = project.group_track(outer).unwrap().mixer().with_muted(true);
    project.set_group_track_mixer(outer, mixer).unwrap();
    assert_eq!(heard(&project), [0.0; 2]);
}

#[test]
fn master_solo_hears_its_whole_mix_and_disconnected_solo_never_reconnects_a_track() {
    let mut project = Project::new();
    let first = audio(&mut project, 1.0);
    audio(&mut project, 2.0);
    solo(&mut project, first, true);
    let mixer = project.master_track().mixer().with_soloed(true);
    project.set_master_track_mixer(mixer).unwrap();
    assert_eq!(heard(&project), [3.0; 2]);
    project
        .set_master_track_mixer(mixer.with_soloed(false))
        .unwrap();
    let mixer = project
        .audio_track(first)
        .unwrap()
        .mixer()
        .with_output(AudioOutput::Disconnected);
    project.set_audio_track_mixer(first, mixer).unwrap();
    assert_eq!(heard(&project), [0.0; 2]);
}

#[test]
fn live_solo_and_restore_ramp_without_rebuilding_or_restarting_ramps() {
    let mut project = Project::new();
    let first = audio(&mut project, 1.0);
    audio(&mut project, 2.0);
    let mut renderer = Renderer::new(&project).unwrap();
    let mut before = [[0.0; 2]; 32];
    renderer.render_block(0, &mut before);
    assert_eq!(before, [[3.0; 2]; 32]);
    let mut edited = project.clone();
    solo(&mut edited, first, true);
    let mut output = [[0.0; 2]; 512];
    for (index, block) in output.chunks_mut(16).enumerate() {
        assert!(renderer.update_parameters(&edited));
        renderer.render_block(32 + (index * 16) as u64, block);
    }
    assert!(output[0][0] < 3.0 && output[0][0] > 2.9);
    assert_eq!(output[240..], [[1.0; 2]; 272]);
    assert!(
        output
            .windows(2)
            .all(|pair| (pair[1][0] - pair[0][0]).abs() < 0.01)
    );
    assert!(renderer.update_parameters(&project));
    renderer.render_block(544, &mut output);
    assert_eq!(output[240..], [[3.0; 2]; 272]);
}

struct DetectorHost;

#[derive(Debug)]
struct Detector;

impl PluginHost for DetectorHost {
    fn load(&self, _: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        Ok(Box::new(Detector))
    }
}

impl PluginProcessor for Detector {
    fn reset(&mut self) {}
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, _: u32, _: f64) {}
    fn process(&mut self, _: &mut [Frame], _: PluginProcessContext) {
        panic!("detector requires its sidechain");
    }
    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        _: PluginProcessContext,
    ) {
        for (frame, detector) in frames.iter_mut().zip(sidechain.unwrap().frames) {
            for (sample, detector) in frame.iter_mut().zip(detector) {
                *sample -= detector;
            }
        }
    }
}

#[test]
fn nonsolo_tracks_and_group_subtrees_keep_feeding_all_sidechain_taps() {
    for group_source in [false, true] {
        for tap in [
            SidechainTap::PreEffects,
            SidechainTap::PostEffects,
            SidechainTap::PostFader,
        ] {
            let mut project = Project::new();
            let target = audio(&mut project, 1.0);
            let source = audio(&mut project, 0.25);
            let source_index = if group_source {
                let group = project.add_group_track("Detector bus");
                project.set_track_group(source, Some(group)).unwrap();
                let second = audio(&mut project, 0.125);
                project.set_track_group(second, Some(group)).unwrap();
                3
            } else {
                2
            };
            let plugin = PluginInstance::new(
                PluginDescriptor {
                    path: "/fake/Detector.vst3".into(),
                    class_id: "detector".into(),
                    name: "Detector".into(),
                    vendor: "Tests".into(),
                    kind: PluginKind::Effect,
                },
                vec![],
                vec![],
            );
            let id = project.add_effect(target.into(), plugin).unwrap();
            project
                .set_plugin_sidechain(
                    id,
                    Some(SidechainRoute {
                        source_track: source_index,
                        bus_index: 1,
                        gain: 1.0,
                        tap,
                    }),
                )
                .unwrap();
            solo(&mut project, target, true);
            let mut renderer = Renderer::with_plugin_host(&project, &DetectorHost).unwrap();
            let mut output = [[0.0; 2]; 512];
            renderer.render_block(0, &mut output);
            assert_eq!(output, [[if group_source { 0.625 } else { 0.75 }; 2]; 512]);
            let mut restored = project.clone();
            solo(&mut restored, target, false);
            assert!(renderer.update_parameters(&restored));
            renderer.render_block(512, &mut output);
            assert_eq!(output[240..], [[1.0; 2]; 272]);
        }
    }
}

#[test]
fn group_bounce_ignores_monitoring_solos_but_preserves_the_original_selection() {
    let mut project = Project::new();
    let group = project.add_group_track("Bus");
    let first = audio(&mut project, 1.0);
    let second = audio(&mut project, 2.0);
    project.set_track_group(first, Some(group)).unwrap();
    project.set_track_group(second, Some(group)).unwrap();
    solo(&mut project, first, true);
    assert_eq!(heard(&project), [1.0; 2]);
    let bounce = BouncePlan::new(&project, 1).unwrap();
    assert_eq!(heard(&bounce.project), [3.0; 2]);
    assert!(project.audio_track(first).unwrap().mixer().soloed());
}

#[derive(Default)]
struct DelayHost {
    loads: AtomicUsize,
    resets: Arc<AtomicUsize>,
}

#[derive(Debug)]
struct Delay {
    history: [Frame; 37],
    cursor: usize,
    resets: Arc<AtomicUsize>,
}

impl PluginHost for DelayHost {
    fn load(&self, _: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        self.loads.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(Delay {
            history: [[0.0; 2]; 37],
            cursor: 0,
            resets: self.resets.clone(),
        }))
    }
}

impl PluginProcessor for Delay {
    fn reset(&mut self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
        self.history.fill([0.0; 2]);
        self.cursor = 0;
    }
    fn event(&mut self, _: NoteEvent) {}
    fn set_parameter(&mut self, _: u32, _: f64) {}
    fn latency_samples(&self) -> u32 {
        37
    }
    fn process(&mut self, frames: &mut [Frame], _: PluginProcessContext) {
        for frame in frames {
            std::mem::swap(frame, &mut self.history[self.cursor]);
            self.cursor = (self.cursor + 1) % self.history.len();
        }
    }
}

#[test]
fn live_solo_preserves_delay_compensation_and_running_plugin_state() {
    let mut project = Project::new();
    let delayed = project.add_audio_track("Delayed");
    let samples: Vec<_> = (0..4_000)
        .map(|frame| [(frame % 32) as f32 / 32.0; 2])
        .collect();
    let asset = project.add_asset(AudioBuffer::new(samples.clone()).unwrap());
    project
        .add_audio_clip(
            delayed,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 4_000,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    audio(&mut project, 2.0);
    project
        .add_effect(
            delayed.into(),
            PluginInstance::new(
                PluginDescriptor {
                    path: "/fake/Delay.vst3".into(),
                    class_id: "delay".into(),
                    name: "Delay".into(),
                    vendor: "Tests".into(),
                    kind: PluginKind::Effect,
                },
                vec![],
                vec![],
            ),
        )
        .unwrap();
    let host = DelayHost::default();
    let mut renderer = Renderer::with_plugin_host(&project, &host).unwrap();
    let mut output = [[0.0; 2]; 512];
    renderer.render_block(0, &mut output);
    assert_eq!(
        output.to_vec(),
        samples[..512]
            .iter()
            .map(|frame| frame.map(|s| s + 2.0))
            .collect::<Vec<_>>()
    );
    assert_eq!(renderer.latency_frames(), 37);
    let resets = host.resets.load(Ordering::Relaxed);
    let mut edited = project.clone();
    solo(&mut edited, delayed, true);
    assert!(renderer.update_parameters(&edited));
    renderer.render_block(512, &mut output);
    assert_eq!(output[240..], samples[752..1024]);
    assert_eq!(renderer.latency_frames(), 37);
    assert_eq!(host.loads.load(Ordering::Relaxed), 1);
    assert_eq!(host.resets.load(Ordering::Relaxed), resets);
}
