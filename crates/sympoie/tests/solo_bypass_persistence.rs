use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{
    PROJECT_MANIFEST, ProjectFileError, load_project, read_project, save_project,
    save_project_directory, write_project,
};
use sympoie::track::{AudioOutput, MixerSettings, Track, TrackId};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "sympoie-solo-bypass-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
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

fn synth() -> Instrument {
    Instrument::Subtractive(SubtractiveSettings {
        waveform: Waveform::Square,
        cutoff_hz: 1234.5679,
        resonance: 2.125,
        attack_seconds: 0.003125,
        decay_seconds: 0.45625,
        sustain: 0.2345679,
        release_seconds: 1.125,
        gain: 0.3456789,
    })
}

fn fixture() -> Project {
    let mut project = Project::new();
    let outer = project.add_group_track("Outer");
    let inner = project.add_group_track("Inner");
    let audio = project.add_audio_track("Muted solo");
    let midi = project.add_midi_track("Bypassed solo synth");
    let empty = project.add_midi_track("Empty");
    let enabled = project.add_midi_track("Enabled synth");
    project
        .set_master_track_mixer(
            MixerSettings::new(0.75, 0.125, AudioOutput::Output)
                .unwrap()
                .with_soloed(true),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            outer,
            MixerSettings::new(0.875, 0.25, AudioOutput::Master)
                .unwrap()
                .with_soloed(true),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            inner,
            MixerSettings::new(0.625, -0.375, AudioOutput::Group(outer))
                .unwrap()
                .with_muted(true),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::new(0.5, -0.25, AudioOutput::Group(inner))
                .unwrap()
                .with_muted(true)
                .with_soloed(true),
        )
        .unwrap();
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(0.8, -0.125, AudioOutput::Group(inner))
                .unwrap()
                .with_soloed(true),
        )
        .unwrap();
    project.set_track_group(empty, Some(outer)).unwrap();
    project
        .set_midi_track_instrument(midi, Some(synth()))
        .unwrap();
    project
        .set_midi_track_instrument_bypassed(midi, true)
        .unwrap();
    project
        .set_midi_track_instrument(
            enabled,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                start_tick: 17,
                length_ticks: 960,
                offset_ticks: 120,
                notes: vec![MidiNote {
                    start_tick: 240,
                    duration_ticks: 480,
                    key: 64,
                    velocity: 100,
                    channel: 3,
                }],
            },
        )
        .unwrap();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.12345679, -0.0]]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 1,
                frame_count: 1,
                gain: 0.875,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    project
}

fn assert_preserved(original: &Project, restored: &Project) {
    assert_eq!(encoded(original), encoded(restored));
    for (before, after) in original.tracks().iter().zip(restored.tracks()) {
        assert_eq!(before.mixer(), after.mixer());
        if let (Track::Midi(before), Track::Midi(after)) = (before, after) {
            assert_eq!(before.instrument(), after.instrument());
            assert_eq!(before.instrument_bypassed(), after.instrument_bypassed());
            assert_eq!(before.clips(), after.clips());
        }
    }
}

#[test]
fn standalone_and_directory_round_trip_solo_mute_nested_groups_and_builtin_bypass() {
    let original = fixture();
    let bytes = encoded(&original);
    assert_eq!(&bytes[8..12], &13_u32.to_le_bytes());
    let flags_end = bytes.len() - (4 + 3 * 2) - fixture().tracks().len() * 16;
    assert_eq!(
        &bytes[flags_end - 10..flags_end],
        &[1, 1, 0, 1, 1, 0, 0, 1, 0, 0]
    );
    assert_preserved(&original, &read_project(bytes.as_slice()).unwrap());

    let temp = TestDirectory::new();
    let standalone = temp.0.join("Solo.symp");
    save_project(&original, &standalone).unwrap();
    assert_preserved(&original, &load_project(standalone).unwrap());
    let directory = temp.0.join("Solo");
    save_project_directory(&original, &directory).unwrap();
    assert_preserved(&original, &load_project(&directory).unwrap());
    assert_preserved(
        &original,
        &load_project(directory.join(PROJECT_MANIFEST)).unwrap(),
    );
}

fn old_project(version: u32) -> Vec<u8> {
    let mut bytes = b"SYMPROJ\0".to_vec();
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&120.0_f64.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.push(0);
    if version >= 6 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&4_u64.to_le_bytes());
    for (tag, name, route) in [
        (3, "Master", 2),
        (1, "Synth", 1),
        (2, "Group", 0),
        (0, "Audio", 0),
    ] {
        bytes.push(tag);
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&0.875_f32.to_le_bytes());
        bytes.extend_from_slice(&0.125_f32.to_le_bytes());
        bytes.push(route);
        if route == 1 {
            bytes.extend_from_slice(&2_u64.to_le_bytes());
        }
        if version >= 5 {
            bytes.push(0);
        }
        if version >= 6 {
            bytes.push(u8::from(tag == 1));
        }
        if tag == 1 && version >= 2 {
            bytes.extend_from_slice(&[1, 2]);
            let Instrument::Subtractive(settings) = synth();
            for value in [
                settings.cutoff_hz,
                settings.resonance,
                settings.attack_seconds,
                settings.decay_seconds,
                settings.sustain,
                settings.release_seconds,
                settings.gain,
            ] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        if tag <= 1 {
            bytes.extend_from_slice(&0_u64.to_le_bytes());
        }
    }
    if version >= 3 {
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        for _ in 0..4 {
            bytes.push(0);
            bytes.extend_from_slice(&0_u64.to_le_bytes());
        }
    }
    if version >= 4 {
        bytes.extend_from_slice(&0_u64.to_le_bytes());
    }
    bytes
}

fn assert_old_defaults(project: &Project, version: u32) {
    assert!(project.tracks().iter().all(|track| !track.mixer().soloed()));
    let Track::Midi(midi) = &project.tracks()[1] else {
        panic!("expected old MIDI track");
    };
    assert!(!midi.instrument_bypassed());
    assert_eq!(midi.instrument(), (version >= 2).then(synth).as_ref());
    assert_eq!(midi.mixer().muted(), version >= 6);
    assert_eq!(midi.mixer().gain(), 0.875);
    assert_eq!(midi.mixer().pan(), 0.125);
    let TrackId::Group(group) = project.track_ids().nth(2).unwrap() else {
        panic!("expected old group track");
    };
    assert_eq!(midi.mixer().output(), AudioOutput::Group(group));
}

#[test]
fn versions_one_through_six_load_unsoloed_with_builtin_instruments_enabled() {
    for version in 1..=6 {
        let restored = read_project(old_project(version).as_slice()).unwrap();
        assert_old_defaults(&restored, version);
        let upgraded = read_project(encoded(&restored).as_slice()).unwrap();
        assert_old_defaults(&upgraded, version);
    }
}

#[test]
fn version_six_directory_loads_and_migrates_without_changing_existing_mute_or_synth() {
    let temp = TestDirectory::new();
    let mut bytes = old_project(6);
    bytes[37] = 1;
    fs::write(temp.0.join(PROJECT_MANIFEST), bytes).unwrap();
    let loaded = load_project(&temp.0).unwrap();
    assert_old_defaults(&loaded, 6);
    save_project_directory(&loaded, &temp.0).unwrap();
    let upgraded = load_project(&temp.0).unwrap();
    assert_old_defaults(&upgraded, 6);
    assert_eq!(
        &fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap()[8..12],
        &13_u32.to_le_bytes()
    );
}

#[test]
fn rejects_invalid_solo_and_bypass_flags_and_truncated_version_seven_extensions() {
    let project = fixture();
    let bytes = encoded(&project);
    let extension_end = bytes.len() - (4 + 3 * 2) - project.tracks().len() * 16;
    let extension_start = extension_end - 10;
    for offset in extension_start..extension_end {
        for flag in [2, 255] {
            let mut invalid = bytes.clone();
            invalid[offset] = flag;
            assert!(matches!(
                read_project(invalid.as_slice()),
                Err(ProjectFileError::InvalidFormat("invalid boolean flag"))
            ));
        }
        assert!(read_project(&bytes[..offset]).is_err());
    }
}

#[test]
fn rejects_bypassing_a_missing_builtin_instrument() {
    let mut bytes = encoded(&fixture());
    let empty_midi_bypass = bytes.len() - (4 + 3 * 2) - fixture().tracks().len() * 16 - 2;
    bytes[empty_midi_bypass] = 1;
    assert!(matches!(
        read_project(bytes.as_slice()),
        Err(ProjectFileError::InvalidFormat(
            "missing built-in instrument cannot be bypassed"
        ))
    ));
}
