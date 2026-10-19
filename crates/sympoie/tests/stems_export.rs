use sympoie::Frame;
use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::builtin::BuiltinKind;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::plugin::{SidechainRoute, SidechainTap};
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_json::{load_project_folder, save_project_folder};
use sympoie::render::Renderer;
use sympoie::stems::StemsPlan;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings, Track};

fn signal(frames: usize, seed: u32) -> Vec<Frame> {
    let mut state = seed;
    (0..frames)
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 9) as f32 / (1 << 23) as f32 - 0.5;
            let tone = (i as f32 * 0.031).sin();
            [0.6 * tone + 0.2 * noise, 0.5 * tone - 0.25 * noise]
        })
        .collect()
}

fn audio(project: &mut Project, name: &str, start_tick: u64, samples: Vec<Frame>) -> AudioTrackId {
    let track = project.add_audio_track(name);
    let frame_count = samples.len() as u64;
    let asset = project.add_asset(AudioBuffer::new(samples).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 100,
                start_tick,
                frame_count: frame_count - 300,
                gain: 0.8,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    track
}

/// An EQ that audibly colours the signal, so baking it in or not matters.
fn shaped_eq() -> sympoie::plugin::PluginInstance {
    let mut eq = BuiltinKind::Eq.instance();
    for parameter in &mut eq.parameters {
        match parameter.id {
            3 => parameter.value = 0.85,
            8 => parameter.value = 0.2,
            30 => parameter.value = 0.4,
            _ => {}
        }
    }
    eq
}

fn ramp(target: AutomationTarget, from: f64, to: f64) -> AutomationLane {
    AutomationLane::new(
        target,
        vec![
            AutomationPoint {
                tick: 0,
                value: from,
            },
            AutomationPoint {
                tick: 3840,
                value: to,
            },
        ],
    )
    .unwrap()
}

/// Audio and MIDI tracks with inserts, mixer-strip effects, automation, sends,
/// a bus, a group with its own chain, a muted track, a pre-effects sidechain,
/// and a master chain.
fn arrangement() -> Project {
    let mut project = Project::new();
    project.set_bpm(150.0).unwrap();
    let group = project.add_group_track("Drums");
    let bus = project.add_bus_track("Reverb bus");
    project.add_effect(bus.into(), shaped_eq()).unwrap();

    let kick = audio(&mut project, "Kick", 0, signal(30_000, 1));
    project.set_track_group(kick, Some(group)).unwrap();
    let insert = project
        .add_effect(kick.into(), BuiltinKind::Comp.instance())
        .unwrap();
    project.set_plugin_parameter(insert, 0, 0.2).unwrap();
    project.add_effect(kick.into(), shaped_eq()).unwrap();
    let strip = project.add_mixer_effect(kick.into(), shaped_eq()).unwrap();
    project
        .set_track_automation(
            kick,
            ramp(
                AutomationTarget::PluginParameter {
                    plugin: insert,
                    parameter: 0,
                },
                0.1,
                0.9,
            ),
        )
        .unwrap();
    project
        .set_track_automation(
            kick,
            ramp(
                AutomationTarget::PluginParameter {
                    plugin: strip,
                    parameter: 1,
                },
                0.3,
                0.7,
            ),
        )
        .unwrap();
    project
        .set_track_automation(kick, ramp(AutomationTarget::TrackGain, 0.4, 0.7))
        .unwrap();
    project.set_track_send(kick, bus, 0.5).unwrap();

    let snare = audio(&mut project, "Snare", 960, signal(20_000, 2));
    project.set_track_group(snare, Some(group)).unwrap();
    project
        .set_audio_track_mixer(
            snare,
            MixerSettings::new(0.6, -0.4, AudioOutput::Group(group)).unwrap(),
        )
        .unwrap();
    project.add_effect(snare.into(), shaped_eq()).unwrap();
    project
        .add_effect(group.into(), BuiltinKind::Comp.instance())
        .unwrap();

    let keys = project.add_midi_track("Keys");
    project
        .set_plugin_instrument(keys, Some(BuiltinKind::Oscil.instance()))
        .unwrap();
    project.add_effect(keys.into(), shaped_eq()).unwrap();
    project
        .add_midi_clip(
            keys,
            MidiClip {
                notes: [0, 480, 1200]
                    .into_iter()
                    .enumerate()
                    .map(|(index, start)| MidiNote {
                        start_tick: start,
                        duration_ticks: 400,
                        key: 57 + index as u8 * 4,
                        velocity: 100,
                        channel: 0,
                        slide_ticks: 0,
                    })
                    .collect(),
                ..MidiClip::new(480, 1920)
            },
        )
        .unwrap();
    project
        .set_track_automation(keys, ramp(AutomationTarget::TrackPan, 0.2, 0.8))
        .unwrap();
    project.set_track_send(keys, bus, 0.3).unwrap();

    // Its pre-effects signal keys the master compressor, so its insert stays live.
    let key_source = audio(&mut project, "Key source", 240, signal(12_000, 3));
    project.add_effect(key_source.into(), shaped_eq()).unwrap();
    let master = project
        .add_effect(
            project.master_track_id().into(),
            BuiltinKind::Comp.instance(),
        )
        .unwrap();
    project.set_plugin_parameter(master, 10, 1.0).unwrap();
    project.set_plugin_parameter(master, 0, 0.3).unwrap();
    project
        .set_plugin_sidechain(
            master,
            Some(SidechainRoute {
                source_track: 6,
                bus_index: 1,
                gain: 1.0,
                tap: SidechainTap::PreEffects,
            }),
        )
        .unwrap();

    let muted = audio(&mut project, "Muted", 0, signal(8_000, 4));
    project
        .set_audio_track_mixer(muted, MixerSettings::default().with_muted(true))
        .unwrap();
    project.add_midi_track("Empty MIDI");
    project
}

fn render(project: &Project, frames: u64) -> Vec<Frame> {
    let _pins = project.prepare_audio_pitch(|| false).unwrap().unwrap();
    let mut renderer = Renderer::new(project).unwrap();
    let mut output = vec![[0.0; 2]; frames as usize];
    for (index, block) in output.chunks_mut(512).enumerate() {
        renderer.render_block(index as u64 * 512, block);
    }
    output
}

fn stems(project: &Project) -> Project {
    let plan = StemsPlan::new(project).unwrap();
    let audio = plan
        .stems
        .iter()
        .map(|stem| {
            stem.render.as_ref().map(|plan| {
                let frames = render(&plan.project, plan.end_frame);
                AudioBuffer::new(frames[plan.start_frame as usize..].to_vec()).unwrap()
            })
        })
        .collect();
    plan.apply(audio).unwrap()
}

#[test]
fn the_stems_project_sounds_like_the_original() {
    let original = arrangement();
    let exported = stems(&original);
    let frames = original
        .duration_frames()
        .unwrap()
        .max(exported.duration_frames().unwrap());
    let expected = render(&original, frames);
    assert!(
        expected.iter().flatten().any(|sample| sample.abs() > 0.05),
        "the arrangement is audible"
    );
    let actual = render(&exported, frames);
    let worst = expected
        .iter()
        .zip(&actual)
        .flat_map(|(a, b)| [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()])
        .fold(0.0_f32, f32::max);
    assert!(worst < 1e-5, "largest difference {worst}");

    // The same holds after saving the stems project and loading it again.
    let folder =
        std::env::temp_dir().join(format!("sympoie-stems-test-{}.symp", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    save_project_folder(&exported, &folder).unwrap();
    let reloaded = load_project_folder(&folder).unwrap();
    std::fs::remove_dir_all(&folder).unwrap();
    let worst = render(&reloaded, frames)
        .iter()
        .zip(&expected)
        .flat_map(|(a, b)| [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()])
        .fold(0.0_f32, f32::max);
    assert!(worst < 1e-5, "largest difference after reload {worst}");
}

#[test]
fn stems_bake_sources_and_inserts_but_keep_the_mixer_live() {
    let original = arrangement();
    let exported = stems(&original);
    assert_eq!(exported.tracks().len(), original.tracks().len());
    for (before, after) in original.tracks().iter().zip(exported.tracks()) {
        assert_eq!(before.name(), after.name());
        assert_eq!(before.uid(), after.uid());
        assert_eq!(before.mixer(), after.mixer());
        assert_eq!(before.sends(), after.sends());
        assert_eq!(before.mixer_effects(), after.mixer_effects());
        match before {
            Track::Group(_) | Track::Bus(_) | Track::Master(_) => {
                assert_eq!(before.effects(), after.effects());
                assert_eq!(before.automation(), after.automation());
                assert_eq!(
                    std::mem::discriminant(before),
                    std::mem::discriminant(after)
                );
            }
            Track::Audio(_) | Track::Midi(_) => {
                let Track::Audio(stem) = after else {
                    panic!("{} became an audio track", before.name());
                };
                assert!(stem.clips().len() <= 1);
                if let Some(clip) = stem.clips().first() {
                    assert_eq!(clip.name.as_deref(), Some(before.name()));
                    assert_eq!(clip.gain, 1.0);
                }
                if before.name() == "Key source" {
                    assert_eq!(
                        after.effects(),
                        before.effects(),
                        "tapped inserts stay live"
                    );
                } else {
                    assert!(
                        after.effects().is_empty(),
                        "{} inserts are baked",
                        before.name()
                    );
                }
                for lane in after.automation() {
                    assert!(before.automation().contains(lane));
                }
            }
        }
    }
    // Only automation of what stays live moves along.
    assert_eq!(exported.tracks()[3].automation().len(), 2);
    assert_eq!(exported.tracks()[5].automation().len(), 1);
    // Empty tracks keep their place.
    let Track::Audio(empty) = &exported.tracks()[8] else {
        panic!("expected the empty track")
    };
    assert!(empty.clips().is_empty());
}
