use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::builtin::BuiltinKind;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings};
use sympoie::pitch::PitchAlgorithm;
use sympoie::plugin::{
    PluginDescriptor, PluginInstance, PluginKind, PluginParameter, SidechainRoute, SidechainTap,
};
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{load_project, save_project_directory, write_project};
use sympoie::project_json::{MANIFEST, load_project_folder, save_project_folder};
use sympoie::track::{
    AudioInputChannels, AudioOutput, AudioRecordingInput, MidiRecordingInput, MixerSettings, Track,
    TrackId,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "sympoie-json-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("{error}"),
            }
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn uids(project: &Project) -> Vec<(u64, Vec<u64>)> {
    project
        .tracks()
        .iter()
        .map(|track| (track.uid(), track.clip_uids().to_vec()))
        .collect()
}

fn external(kind: PluginKind, name: &str) -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: PathBuf::from(format!("/Library/Audio/Plug-Ins/VST3/{name}.vst3")),
            class_id: format!("{name}-class"),
            name: name.into(),
            vendor: "Test \"Vendor\"".into(),
            kind,
        },
        vec![
            PluginParameter {
                id: 7,
                name: "Mix".into(),
                unit: "%".into(),
                value: 0.1,
                steps: 0,
                read_only: false,
            },
            PluginParameter {
                id: 9,
                name: "Mode".into(),
                unit: String::new(),
                value: 1.0 / 3.0,
                steps: 3,
                read_only: true,
            },
        ],
        vec![0, 1, 255, 7, 0],
    )
}

fn fixture() -> Project {
    let mut project = Project::new();
    project.set_bpm(137.125).unwrap();
    project.set_time_signature(sympoie::time::TimeSignature::new(7, 8).unwrap());
    let drums = project.add_audio_track("Drums / Room");
    let keys = project.add_midi_track("Keys");
    let synth = project.add_midi_track("Synth");
    let band = project.add_group_track("Band");
    let reverb = project.add_bus_track("Reverb");
    project.set_track_group(drums, Some(band)).unwrap();
    project
        .set_midi_track_mixer(
            keys,
            MixerSettings::new(0.75, -0.25, AudioOutput::Group(band))
                .unwrap()
                .with_muted(true)
                .with_soloed(true),
        )
        .unwrap();
    project.set_track_send(keys, reverb, 0.3).unwrap();
    project.set_track_color(drums, Some(0x4488ee)).unwrap();
    project
        .set_audio_recording_input(
            drums,
            AudioRecordingInput {
                device: Some("Interface \u{1}".into()),
                channels: AudioInputChannels::Stereo(2, 3),
            },
        )
        .unwrap();
    project
        .set_midi_recording_input(
            keys,
            MidiRecordingInput {
                device: Some("Keyboard".into()),
                channel: Some(9),
            },
        )
        .unwrap();
    let frames = vec![
        [0.12345679, -0.0],
        [f32::from_bits(1), f32::MAX],
        [-3.0, f32::MIN],
    ];
    let asset = project.add_asset(AudioBuffer::new(frames).unwrap());
    for (start, gain, pitch) in [(49, -0.125, 0.0), (5000, 0.1, -3.5)] {
        project
            .add_audio_clip(
                drums,
                AudioClip {
                    asset,
                    source_start: 1,
                    start_tick: start,
                    frame_count: 2,
                    gain,
                    pitch_semitones: pitch,
                    pitch_algorithm: if pitch == 0.0 {
                        PitchAlgorithm::Builtin
                    } else {
                        PitchAlgorithm::Signalsmith
                    },
                    preserve_formants: pitch != 0.0,
                    name: None,
                },
            )
            .unwrap();
    }
    project
        .set_midi_track_instrument(
            keys,
            Some(Instrument::Subtractive(SubtractiveSettings {
                cutoff_hz: 1234.5,
                ..SubtractiveSettings::default()
            })),
        )
        .unwrap();
    project
        .set_midi_track_instrument_bypassed(keys, true)
        .unwrap();
    project
        .add_midi_clip(
            keys,
            MidiClip {
                start_tick: 49,
                length_ticks: 960,
                offset_ticks: 120,
                notes: vec![
                    MidiNote {
                        start_tick: 240,
                        duration_ticks: 480,
                        key: 61,
                        velocity: 100,
                        channel: 3,
                        slide_ticks: 0,
                    },
                    MidiNote {
                        start_tick: 0,
                        duration_ticks: 1,
                        key: 127,
                        velocity: 1,
                        channel: 0,
                        slide_ticks: 0,
                    },
                ],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    project
        .add_midi_clip(keys, MidiClip::new(3840, 1920))
        .unwrap();

    project
        .set_plugin_instrument(synth, Some(BuiltinKind::Oscil.instance()))
        .unwrap();
    let oscil = project
        .midi_track(synth)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    for (parameter, value) in [(0, 0.123_456_789_012_345_67), (1, 1.0 / 3.0), (2, 0.5)] {
        project
            .set_plugin_parameter(oscil, parameter, value)
            .unwrap();
    }
    let eq = project
        .add_effect(band.into(), BuiltinKind::Eq.instance())
        .unwrap();
    project.set_plugin_bypassed(eq, true).unwrap();
    let comp = project
        .add_effect(band.into(), BuiltinKind::Comp.instance())
        .unwrap();
    project
        .set_plugin_sidechain(
            comp,
            Some(SidechainRoute {
                source_track: 2,
                bus_index: 1,
                gain: 0.5,
                tap: SidechainTap::PreEffects,
            }),
        )
        .unwrap();
    let outside = project
        .add_mixer_effect(reverb.into(), external(PluginKind::Effect, "Space"))
        .unwrap();
    project
        .add_effect(
            project.master_track_id().into(),
            external(PluginKind::Effect, "Limiter"),
        )
        .unwrap();
    project
        .set_track_automation(
            synth,
            AutomationLane::new(
                AutomationTarget::PluginParameter {
                    plugin: oscil,
                    parameter: 0,
                },
                vec![
                    AutomationPoint {
                        tick: 0,
                        value: 0.25,
                    },
                    AutomationPoint {
                        tick: 1920,
                        value: 0.1,
                    },
                ],
            )
            .unwrap(),
        )
        .unwrap();
    project
        .set_track_automation(
            reverb,
            AutomationLane::new(
                AutomationTarget::PluginParameter {
                    plugin: outside,
                    parameter: 7,
                },
                vec![AutomationPoint {
                    tick: 960,
                    value: 1.0,
                }],
            )
            .unwrap()
            .with_enabled(false),
        )
        .unwrap();
    project
        .set_track_automation(
            keys,
            AutomationLane::new(
                AutomationTarget::Subtractive(sympoie::automation::SubtractiveParameter::Cutoff),
                vec![AutomationPoint {
                    tick: 10,
                    value: 0.75,
                }],
            )
            .unwrap(),
        )
        .unwrap();
    project
        .set_track_automation(
            drums,
            AutomationLane::new(
                AutomationTarget::TrackGain,
                vec![AutomationPoint {
                    tick: 0,
                    value: 0.5,
                }],
            )
            .unwrap(),
        )
        .unwrap();
    project.set_snap_grid(sympoie::time::SnapGrid::Sixteenth);
    project
}

#[test]
fn every_saved_feature_round_trips_with_its_ids() {
    let folder = TestDirectory::new();
    let project = fixture();
    save_project_folder(&project, &folder.0).unwrap();
    let loaded = load_project_folder(&folder.0).unwrap();
    assert_eq!(encoded(&loaded), encoded(&project));
    assert_eq!(uids(&loaded), uids(&project));
    assert_eq!(loaded.next_uid(), project.next_uid());
    assert_eq!(
        encoded(&load_project(&folder.0).unwrap()),
        encoded(&project)
    );
    assert_eq!(
        encoded(&load_project(folder.0.join(MANIFEST)).unwrap()),
        encoded(&project)
    );
    let again = TestDirectory::new();
    save_project_folder(&loaded, &again.0).unwrap();
    for name in [MANIFEST.to_owned()].into_iter().chain(
        project
            .tracks()
            .iter()
            .map(|track| track_file(&folder.0, track.uid())),
    ) {
        assert_eq!(
            fs::read_to_string(folder.0.join(&name)).unwrap(),
            fs::read_to_string(again.0.join(&name)).unwrap(),
            "{name}"
        );
    }
}

fn track_file(folder: &Path, uid: u64) -> String {
    let prefix = format!("{uid}-");
    let name = fs::read_dir(folder.join("tracks"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .find(|name| name.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no file for track {uid}"));
    format!("tracks/{name}")
}

#[test]
fn files_are_readable_and_use_ids_for_references() {
    let folder = TestDirectory::new();
    let project = fixture();
    save_project_folder(&project, &folder.0).unwrap();
    let manifest = fs::read_to_string(folder.0.join(MANIFEST)).unwrap();
    assert!(manifest.contains("\"format\": \"sympoie-project\""));
    assert!(manifest.contains("\"tempo\": 137.125"));
    assert!(manifest.contains("\"time_signature\": [7, 8]"));
    assert!(manifest.contains("\"snap\": \"1/16\""));
    let band = project.tracks()[4].uid();
    let reverb = project.tracks()[5].uid();
    let keys = fs::read_to_string(
        folder
            .0
            .join(track_file(&folder.0, project.tracks()[2].uid())),
    )
    .unwrap();
    assert!(
        keys.contains(&format!("\"output\": {{\"group\": {band}}}")),
        "{keys}"
    );
    assert!(keys.contains(&format!(
        "\"sends\": [{{\"bus\": {reverb}, \"gain\": 0.3}}]"
    )));
    assert!(keys.contains(
        "{\"start\": 240, \"length\": 480, \"key\": 61, \"velocity\": 100, \"channel\": 3}"
    ));
    let synth = fs::read_to_string(
        folder
            .0
            .join(track_file(&folder.0, project.tracks()[3].uid())),
    )
    .unwrap();
    assert!(synth.contains("\"builtin\": \"oscil\""));
    assert!(synth.contains("\"name\": \"Filter type\""), "{synth}");
    assert!(synth.contains("\"unit\": \"Hz\""));
    let master = fs::read_to_string(
        folder
            .0
            .join(track_file(&folder.0, project.tracks()[0].uid())),
    )
    .unwrap();
    assert!(master.contains("\"state\": \"state/"));
    assert!(fs::read_dir(folder.0.join("state")).unwrap().count() == 1);
}

#[test]
fn saving_again_rewrites_only_what_changed() {
    let folder = TestDirectory::new();
    let mut project = fixture();
    let first = save_project_folder(&project, &folder.0).unwrap();
    assert_eq!(first.written.len(), project.tracks().len() + 1);
    assert!(
        save_project_folder(&project, &folder.0)
            .unwrap()
            .written
            .is_empty()
    );

    let TrackId::Midi(keys) = project.track_ids().nth(2).unwrap() else {
        panic!()
    };
    project.move_midi_clip(keys, 0, 960).unwrap();
    let report = save_project_folder(&project, &folder.0).unwrap();
    assert_eq!(
        report.written,
        vec![track_file(&folder.0, project.tracks()[2].uid())]
    );

    let old = track_file(&folder.0, project.tracks()[2].uid());
    project.rename_track(keys, "Piano").unwrap();
    let report = save_project_folder(&project, &folder.0).unwrap();
    let new = track_file(&folder.0, project.tracks()[2].uid());
    assert!(new.ends_with("-piano.json"));
    assert_eq!(report.removed, vec![old]);
    assert!(report.written.contains(&new) && report.written.contains(&MANIFEST.to_owned()));
}

#[test]
fn ids_survive_reordering_removal_and_edits_in_place() {
    let mut project = fixture();
    let before = uids(&project);
    let all: Vec<u64> = before
        .iter()
        .flat_map(|(track, clips)| std::iter::once(*track).chain(clips.iter().copied()))
        .collect();
    let mut unique = all.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), all.len(), "IDs are unique: {all:?}");

    project.reorder_tracks(&[0, 3, 1, 2, 4, 5]).unwrap();
    assert_eq!(project.tracks()[1].uid(), before[3].0);
    assert_eq!(project.tracks()[2].uid(), before[1].0);
    assert_eq!(project.track_index_by_uid(before[1].0), Some(2));
    assert_eq!(project.clip_by_uid(before[2].1[1]), Some((3, 1)));

    let TrackId::Midi(keys) = project.track_ids().nth(3).unwrap() else {
        panic!()
    };
    project.move_midi_clip(keys, 0, 0).unwrap();
    assert_eq!(project.tracks()[3].clip_uids(), before[2].1.as_slice());
    let removed = project
        .remove_midi_clip(keys, 1)
        .map(|_| before[2].1[1])
        .unwrap();
    project.add_midi_clip(keys, MidiClip::new(0, 960)).unwrap();
    let added = *project.tracks()[3].clip_uids().last().unwrap();
    assert!(!all.contains(&added) && added != removed);
    let track = project.add_audio_track("New");
    let new_uid = project.tracks()[project.tracks().len() - 1].uid();
    assert!(new_uid > added);
    project.remove_track(track).unwrap();
    project.add_group_track("Later");
    assert!(project.tracks().last().unwrap().uid() > new_uid);
}

#[test]
fn in_place_bounce_keeps_the_track_id() {
    use sympoie::bounce::{BounceMode, BouncePlan, apply_bounce};
    let mut project = Project::new();
    let track = project.add_midi_track("Lead");
    project
        .set_plugin_instrument(track, Some(BuiltinKind::Oscil.instance()))
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 240,
                    key: 60,
                    velocity: 100,
                    channel: 0,
                    slide_ticks: 0,
                }],
                ..MidiClip::new(0, 960)
            },
        )
        .unwrap();
    let uid = project.tracks()[1].uid();
    let plan = BouncePlan::new(&project, 1).unwrap();
    let audio = AudioBuffer::new(vec![[0.0; 2]; plan.frame_count() as usize]).unwrap();
    let mut duplicate = project.clone();
    let index = apply_bounce(&mut project, &plan, audio.clone(), BounceMode::InPlace).unwrap();
    assert_eq!(project.tracks()[index].uid(), uid);
    assert!(matches!(project.tracks()[index], Track::Audio(_)));
    let index = apply_bounce(&mut duplicate, &plan, audio, BounceMode::Duplicate).unwrap();
    assert_ne!(duplicate.tracks()[index].uid(), uid);
    assert_eq!(duplicate.tracks()[1].uid(), uid);
}

fn write(folder: &Path, name: &str, text: &str) {
    let path = folder.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn hand_written_projects_load_and_get_ids() {
    let folder = TestDirectory::new();
    write(
        &folder.0,
        MANIFEST,
        r#"{
  "format": "sympoie-project",
  "version": 1,
  "tempo": 96,
  "time_signature": [3, 4],
  "tracks": ["tracks/master.json", "tracks/bass.json", "tracks/loop.json", "tracks/verb.json"]
}"#,
    );
    write(&folder.0, "tracks/master.json", r#"{"type": "master"}"#);
    write(
        &folder.0,
        "tracks/bass.json",
        r#"{
  "type": "midi",
  "name": "Bass",
  "instrument": {"builtin": "oscil", "parameters": [{"name": "cutoff", "value": 800}]},
  "devices": [
    {"builtin": "comp", "parameters": [{"id": 0, "value": -24}],
     "sidechain": {"source": 30}}
  ],
  "mixer": {"gain": 0.5, "output": {"bus": 40}},
  "clips": [
    {"start_tick": 0, "length_ticks": 3840, "notes": [
      {"start": 0, "length": 480, "key": 36},
      {"start": 960, "length": 480, "key": 43, "velocity": 90}
    ]},
    {"id": 99, "start_tick": 3840, "length_ticks": 3840},
    {"id": 99, "start_tick": 7680, "length_ticks": 3840}
  ]
}"#,
    );
    write(
        &folder.0,
        "tracks/loop.json",
        r#"{"id": 30, "type": "audio", "name": "Loop",
            "clips": [{"sample": "samples/loop_1.wav", "start_tick": 0}]}"#,
    );
    write(
        &folder.0,
        "tracks/verb.json",
        r#"{"id": 40, "type": "bus", "name": "Verb"}"#,
    );
    let mut wav = sympoie::wav::FloatWavWriter::new(Vec::new(), 4).unwrap();
    wav.write_frames(&[[0.5, -0.5]; 4]).unwrap();
    fs::create_dir_all(folder.0.join("samples")).unwrap();
    fs::write(folder.0.join("samples/loop_1.wav"), wav.finish().unwrap()).unwrap();

    let project = load_project(&folder.0).unwrap();
    assert_eq!(project.bpm(), 96.0);
    assert_eq!(project.tracks().len(), 4);
    let Track::Midi(bass) = &project.tracks()[1] else {
        panic!()
    };
    assert_eq!(bass.clips()[0].notes[0].velocity, 100);
    assert_eq!(bass.clips()[0].notes[1].key, 43);
    let oscil = bass.plugin_instrument().unwrap();
    let cutoff = sympoie::builtin::parameters(BuiltinKind::Oscil)
        .into_iter()
        .find(|spec| spec.name == "Cutoff")
        .unwrap()
        .id;
    assert!((sympoie::builtin::value(oscil, cutoff) - 800.0).abs() < 1e-6);
    let comp = &bass.effects()[0];
    assert!((sympoie::builtin::value(comp, 0) + 24.0).abs() < 1e-9);
    assert_eq!(comp.sidechain.unwrap().source_track, 2);
    let TrackId::Bus(verb) = project.track_ids().nth(3).unwrap() else {
        panic!()
    };
    assert_eq!(bass.mixer().output(), AudioOutput::Bus(verb));
    let Track::Audio(audio) = &project.tracks()[2] else {
        panic!()
    };
    assert_eq!(audio.clips()[0].frame_count, 4);
    assert_eq!(project.tracks()[2].uid(), 30);
    let clips = project.tracks()[1].clip_uids();
    assert_eq!(clips[1], 99);
    assert!(clips.iter().all(|&uid| uid != 0));
    assert_ne!(clips[2], 99);
    assert!(project.tracks().iter().all(|track| track.uid() != 0));

    save_project_folder(&project, &folder.0).unwrap();
    let reloaded = load_project(&folder.0).unwrap();
    assert_eq!(encoded(&reloaded), encoded(&project));
    assert_eq!(uids(&reloaded), uids(&project));
    assert!(folder.0.join("tracks/30-loop.json").is_file());
    assert!(folder.0.join("tracks/bass.json").is_file());
}

fn error_for(track: &str) -> String {
    let folder = TestDirectory::new();
    write(
        &folder.0,
        MANIFEST,
        r#"{"format": "sympoie-project", "version": 1, "tempo": 120,
            "time_signature": [4, 4], "tracks": ["tracks/master.json", "tracks/t.json"]}"#,
    );
    write(&folder.0, "tracks/master.json", r#"{"type": "master"}"#);
    write(&folder.0, "tracks/t.json", track);
    load_project(&folder.0).unwrap_err().to_string()
}

#[test]
fn errors_name_the_file_and_field() {
    let error = error_for(
        r#"{"type": "midi", "clips": [{"start_tick": 0, "length_ticks": 960,
            "notes": [{"start": 0, "length": 1, "key": 128}]}]}"#,
    );
    assert!(
        error.contains("tracks/t.json: clips[0].notes[0].key: must be at most 127"),
        "{error}"
    );
    let error = error_for(
        r#"{"type": "midi", "clips": [{"start_tick": 0, "length_ticks": 960,
            "notes": [{"start": 0, "length": 1, "key": 60, "veloctiy": 9}]}]}"#,
    );
    assert!(error.contains("unknown field \"veloctiy\""), "{error}");
    let error = error_for(r#"{"type": "audio", "mixer": {"output": {"group": 77}}}"#);
    assert!(
        error.contains("mixer.output.group: no track has ID 77"),
        "{error}"
    );
    let error = error_for(r#"{"type": "midi", "clips": [{"start_tick": 0}]}"#);
    assert!(error.contains("clips[0].length_ticks (missing)"), "{error}");
    let error = error_for(r#"{"type": "midi", "devices": [{"builtin": "oscil"}]}"#);
    assert!(
        error.contains("devices[0].builtin: is an instrument"),
        "{error}"
    );
    let error = error_for(
        r#"{"type": "midi", "instrument": {"builtin": "oscil",
            "parameters": [{"name": "Cutoff", "value": 5}]}}"#,
    );
    assert!(error.contains("must be between"), "{error}");
    let error = error_for("{\"type\": \"midi\",\n \"name\": \"x\",}");
    assert!(error.contains("tracks/t.json: line 2, column"), "{error}");
    let error =
        error_for(r#"{"type": "audio", "clips": [{"sample": "../x.wav", "start_tick": 0}]}"#);
    assert!(error.contains("clips[0].sample"), "{error}");
}

#[test]
fn duplicate_track_ids_are_rejected() {
    let folder = TestDirectory::new();
    write(
        &folder.0,
        MANIFEST,
        r#"{"format": "sympoie-project", "version": 1, "tempo": 120,
            "time_signature": [4, 4], "tracks": ["tracks/a.json", "tracks/b.json"]}"#,
    );
    write(&folder.0, "tracks/a.json", r#"{"id": 5, "type": "master"}"#);
    write(&folder.0, "tracks/b.json", r#"{"id": 5, "type": "audio"}"#);
    let error = load_project(&folder.0).unwrap_err().to_string();
    assert!(
        error.contains("tracks/b.json: id: track ID 5 is used by another track"),
        "{error}"
    );
}

#[test]
fn json_saves_replace_the_old_binary_manifest() {
    let folder = TestDirectory::new();
    let project = fixture();
    save_project_directory(&project, &folder.0).unwrap();
    assert!(folder.0.join("project.symp").is_file());
    let legacy = load_project(&folder.0).unwrap();
    save_project_folder(&legacy, &folder.0).unwrap();
    assert!(!folder.0.join("project.symp").exists());
    assert!(folder.0.join(MANIFEST).is_file());
    assert_eq!(
        encoded(&load_project(&folder.0).unwrap()),
        encoded(&project)
    );
    let fresh = TestDirectory::new();
    save_project_directory(&project, &fresh.0).unwrap();
    assert_eq!(uids(&load_project(&fresh.0).unwrap()), uids(&legacy));
}

#[test]
fn newer_versions_are_refused_with_a_clear_message() {
    let folder = TestDirectory::new();
    write(
        &folder.0,
        MANIFEST,
        r#"{"format": "sympoie-project", "version": 2, "tempo": 120,
            "time_signature": [4, 4], "tracks": []}"#,
    );
    let error = load_project(&folder.0).unwrap_err().to_string();
    assert!(error.contains("version 2 is not supported"), "{error}");
}
