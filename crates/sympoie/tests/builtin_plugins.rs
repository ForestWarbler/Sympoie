use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::builtin::{self, BuiltinKind};
use sympoie::clip::{MidiClip, MidiNote};
use sympoie::plugin::{PluginHost, PluginInstance, PluginProcessor};
use sympoie::project::Project;
use sympoie::project_file::{read_project, write_project};
use sympoie::render::Renderer;

fn synth_project() -> (Project, u64) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_midi_track("Oscil");
    project
        .set_plugin_instrument(track, Some(BuiltinKind::Oscil.instance()))
        .unwrap();
    let id = project
        .midi_track(track)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    project
        .add_midi_clip(
            track,
            MidiClip {
                start_tick: 0,
                offset_ticks: 0,
                length_ticks: 4096,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 4096,
                    key: 57,
                    velocity: 100,
                    channel: 0,
                    slide_ticks: 0,
                }],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    project
        .add_effect(track.into(), BuiltinKind::Eq.instance())
        .unwrap();
    project
        .add_effect(track.into(), BuiltinKind::Comp.instance())
        .unwrap();
    (project, id)
}

#[test]
fn builtins_render_and_roundtrip_without_external_host() {
    let (project, _) = synth_project();
    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    let restored = read_project(bytes.as_slice()).unwrap();
    let mut rewritten = Vec::new();
    write_project(&restored, &mut rewritten).unwrap();
    assert_eq!(bytes, rewritten);
    let mut renderer = Renderer::new(&restored).unwrap();
    let mut audio = vec![[0.0; 2]; 8192];
    renderer.render_block(0, &mut audio);
    assert!(audio.iter().flatten().all(|sample| sample.is_finite()));
    assert!(audio.iter().flatten().any(|sample| sample.abs() > 0.01));
}

#[test]
fn builtin_parameter_specs_have_stable_unique_ids_and_roundtrip_units() {
    for kind in BuiltinKind::ALL {
        let instance = kind.instance();
        instance.validate().unwrap();
        assert_eq!(
            BuiltinKind::from_descriptor(&instance.descriptor),
            Some(kind)
        );
        let mut ids = std::collections::HashSet::new();
        for spec in builtin::parameters(kind) {
            assert!(ids.insert(spec.id));
            assert!(spec.min < spec.max && (spec.min..=spec.max).contains(&spec.default));
            assert!(
                (spec.denormalize(spec.normalize(spec.default)) - spec.default).abs() < 1e-6,
                "{} {}",
                kind.name(),
                spec.name
            );
        }
    }
}

struct ExternalOnlyHost;
impl PluginHost for ExternalOnlyHost {
    fn load(&self, _: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        panic!("Built-in devices must not require an external format host");
    }
}

#[test]
fn builtins_remain_portable_when_external_host_is_installed() {
    let (project, _) = synth_project();
    let mut renderer = Renderer::with_plugin_host(&project, &ExternalOnlyHost).unwrap();
    let mut audio = [[0.0; 2]; 4096];
    renderer.render_block(0, &mut audio);
    assert!(audio.iter().flatten().any(|sample| sample.abs() > 0.01));
}

#[test]
fn builtin_automation_is_independent_of_render_block_size() {
    let (mut project, id) = synth_project();
    let track = project.track_ids().nth(1).unwrap();
    project
        .set_track_automation(
            track,
            AutomationLane::new(
                AutomationTarget::PluginParameter {
                    plugin: id,
                    parameter: 20,
                },
                vec![
                    AutomationPoint {
                        tick: 0,
                        value: 0.25,
                    },
                    AutomationPoint {
                        tick: 3072,
                        value: 0.85,
                    },
                ],
            )
            .unwrap(),
        )
        .unwrap();
    let mut whole = vec![[0.0; 2]; 8192];
    Renderer::new(&project).unwrap().render_block(0, &mut whole);
    let mut split = vec![[0.0; 2]; 8192];
    let mut renderer = Renderer::new(&project).unwrap();
    for (chunk, frames) in split.chunks_mut(127).enumerate() {
        renderer.render_block((chunk * 127) as u64, frames);
    }
    assert_eq!(whole, split);
}

#[test]
fn long_release_and_automated_release_extend_export_duration() {
    let (mut project, id) = synth_project();
    let release = builtin::parameters(BuiltinKind::Oscil)
        .into_iter()
        .find(|p| p.id == 13)
        .unwrap();
    project
        .set_plugin_parameter(id, 13, release.normalize(4.0))
        .unwrap();
    let manual = project.duration_frames().unwrap();
    let track = project.track_ids().nth(1).unwrap();
    project
        .set_track_automation(
            track,
            AutomationLane::new(
                AutomationTarget::PluginParameter {
                    plugin: id,
                    parameter: 13,
                },
                vec![AutomationPoint {
                    tick: 0,
                    value: release.normalize(8.0),
                }],
            )
            .unwrap(),
        )
        .unwrap();
    assert!(project.duration_frames().unwrap() >= manual + 4 * 48000 - 1);
}

#[test]
fn one_shot_sample_export_includes_playback_after_short_midi_gate() {
    let path =
        std::env::temp_dir().join(format!("sympoie-sampler-tail-{}.wav", std::process::id()));
    let mut wav =
        sympoie::wav::WavWriter::new(std::fs::File::create(&path).unwrap(), 48000).unwrap();
    wav.write_frames(&vec![[0.25; 2]; 48000]).unwrap();
    wav.finish().unwrap();
    let mut sampler = BuiltinKind::Sampler.instance();
    sampler.state = builtin::instruments::sample_state_from_wav(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    sampler
        .parameters
        .iter_mut()
        .find(|p| p.id == 23)
        .unwrap()
        .value = 0.0;
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_midi_track("One shot");
    project.set_plugin_instrument(track, Some(sampler)).unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                start_tick: 0,
                offset_ticks: 0,
                length_ticks: 256,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 128,
                    key: 48,
                    velocity: 127,
                    channel: 0,
                    slide_ticks: 0,
                }],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(project.duration_frames().unwrap(), 96000);
    let mut renderer = Renderer::new(&project).unwrap();
    let mut audio = vec![[0.0; 2]; 100000];
    renderer.render_block(0, &mut audio);
    assert!(audio[80000][0] > 0.05);
    assert!(
        audio[96000..]
            .iter()
            .flatten()
            .all(|sample| sample.abs() < 1e-6)
    );
}

#[test]
fn reson_latency_is_compensated_against_an_unprocessed_track() {
    use sympoie::builtin::reson;
    use sympoie::clip::AudioClip;
    use sympoie::project::AudioBuffer;

    // Two tracks play the same audio; one passes through a Reson with nothing to cut,
    // which only delays it. Delay compensation lines them up again.
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let source: Vec<_> = (0..4096)
        .map(|i| {
            [
                ((i * 17) % 101) as f32 / 256.0,
                ((i * 29) % 97) as f32 / -256.0,
            ]
        })
        .collect();
    let asset = project.add_asset(AudioBuffer::new(source.clone()).unwrap());
    let mut tracks = Vec::new();
    for name in ["Dry", "Through Reson"] {
        let track = project.add_audio_track(name);
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: source.len() as u64,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        tracks.push(track);
    }
    let mut instance = BuiltinKind::Reson.instance();
    instance.parameters[reson::DEPTH as usize].value = 0.0;
    project.add_effect(tracks[1].into(), instance).unwrap();
    let mut renderer = Renderer::new(&project).unwrap();
    assert_eq!(renderer.latency_frames(), u64::from(reson::LATENCY));
    let mut audio = vec![[0.0; 2]; source.len()];
    for (index, block) in audio.chunks_mut(53).enumerate() {
        renderer.render_block((index * 53) as u64, block);
    }
    for (rendered, original) in audio.iter().zip(&source) {
        for channel in 0..2 {
            assert!((rendered[channel] - 2.0 * original[channel]).abs() < 1.0e-5);
        }
    }
}

#[test]
fn calor_latency_is_compensated_against_an_unprocessed_track() {
    use sympoie::builtin::calor;
    use sympoie::clip::AudioClip;
    use sympoie::project::AudioBuffer;

    // Two tracks play the same audio; one passes through a Calor with Mix at 0, which only
    // delays it. Delay compensation lines them up again.
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let source: Vec<_> = (0..4096)
        .map(|i| {
            [
                ((i * 13) % 89) as f32 / 256.0,
                ((i * 31) % 83) as f32 / -256.0,
            ]
        })
        .collect();
    let asset = project.add_asset(AudioBuffer::new(source.clone()).unwrap());
    let mut tracks = Vec::new();
    for name in ["Dry", "Through Calor"] {
        let track = project.add_audio_track(name);
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: source.len() as u64,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        tracks.push(track);
    }
    let mut instance = BuiltinKind::Calor.instance();
    instance.parameters[calor::MIX as usize].value = 0.0;
    project.add_effect(tracks[1].into(), instance).unwrap();
    let mut renderer = Renderer::new(&project).unwrap();
    assert_eq!(renderer.latency_frames(), u64::from(calor::LATENCY));
    let mut audio = vec![[0.0; 2]; source.len()];
    for (index, block) in audio.chunks_mut(53).enumerate() {
        renderer.render_block((index * 53) as u64, block);
    }
    for (rendered, original) in audio.iter().zip(&source) {
        for channel in 0..2 {
            assert!((rendered[channel] - 2.0 * original[channel]).abs() < 1.0e-5);
        }
    }
}
