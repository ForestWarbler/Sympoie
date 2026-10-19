use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings};
use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind, SidechainRoute, SidechainTap};
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{
    PROJECT_MANIFEST, load_project, read_project, save_project_directory, write_project,
};
use sympoie::track::{AudioOutput, MixerSettings};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "sympoie-directory-test-{}-{}",
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

fn fixture() -> Project {
    let mut project = Project::new();
    project.set_bpm(137.125).unwrap();
    let audio = project.add_audio_track("Bounced keys");
    let midi = project.add_midi_track("Original keys");
    let group = project.add_group_track("Band");
    project.set_track_group(audio, Some(group)).unwrap();
    project.set_track_group(midi, Some(group)).unwrap();
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(0.75, -0.25, AudioOutput::Group(group))
                .unwrap()
                .with_muted(true),
        )
        .unwrap();
    project.set_track_color(audio, Some(0x4488ee)).unwrap();
    let frames = vec![
        [0.12345679, -0.0],
        [f32::from_bits(1), f32::MAX],
        [-3.0, f32::MIN],
    ];
    let asset = project.add_asset(AudioBuffer::new(frames.clone()).unwrap());
    project.add_asset(AudioBuffer::new(frames).unwrap());
    project.add_asset(AudioBuffer::new(Vec::new()).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 1,
                start_tick: 49,
                frame_count: 2,
                gain: -0.125,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    project
        .set_midi_track_instrument(
            midi,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                start_tick: 49,
                length_ticks: 960,
                offset_ticks: 120,
                notes: vec![MidiNote {
                    start_tick: 240,
                    duration_ticks: 480,
                    key: 61,
                    velocity: 100,
                    channel: 3,
                    slide_ticks: 0,
                }],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    let effect = project
        .add_effect(
            group.into(),
            PluginInstance::new(
                PluginDescriptor {
                    path: PathBuf::from("/missing/Compressor.vst3"),
                    class_id: "compressor".into(),
                    name: "Compressor".into(),
                    vendor: "Test".into(),
                    kind: PluginKind::Effect,
                },
                Vec::new(),
                vec![0, 1, 255, 7],
            ),
        )
        .unwrap();
    project
        .set_plugin_sidechain(
            effect,
            Some(SidechainRoute {
                source_track: 1,
                bus_index: 1,
                gain: 0.5,
                tap: SidechainTap::PostFader,
            }),
        )
        .unwrap();
    project
}

fn sample_files(directory: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(directory.join("samples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

#[test]
fn folder_and_manifest_round_trip_every_bit_and_preserve_muted_original_and_plugins() {
    let temp = TestDirectory::new();
    let original = fixture();
    save_project_directory(&original, &temp.0).unwrap();
    assert_eq!(encoded(&load_project(&temp.0).unwrap()), encoded(&original));
    assert_eq!(
        encoded(&load_project(temp.0.join(PROJECT_MANIFEST)).unwrap()),
        encoded(&original)
    );
    let manifest = fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap();
    assert_eq!(&manifest[8..12], &16_u32.to_le_bytes());
    assert_eq!(manifest[37], 1);
    assert_eq!(sample_files(&temp.0).len(), 2);
    for sample in sample_files(&temp.0) {
        let wav = fs::read(sample).unwrap();
        assert_eq!(&wav[20..22], &3_u16.to_le_bytes());
        assert_eq!(&wav[34..36], &32_u16.to_le_bytes());
    }
    assert!(
        read_project(manifest.as_slice())
            .unwrap_err()
            .to_string()
            .contains("project directory")
    );
}

#[test]
fn relocated_directory_and_save_as_need_no_original_media() {
    let temp = TestDirectory::new();
    let first = temp.0.join("First");
    let moved = temp.0.join("移动后的工程");
    let copy = temp.0.join("Copy");
    let project = fixture();
    save_project_directory(&project, &first).unwrap();
    fs::rename(&first, &moved).unwrap();
    let loaded = load_project(&moved).unwrap();
    save_project_directory(&loaded, &copy).unwrap();
    fs::remove_dir_all(&moved).unwrap();
    assert_eq!(encoded(&load_project(&copy).unwrap()), encoded(&project));
}

#[test]
fn resaving_reuses_samples_and_leaves_old_manifest_assets_intact() {
    let temp = TestDirectory::new();
    let mut project = fixture();
    save_project_directory(&project, &temp.0).unwrap();
    let original = encoded(&project);
    let old_manifest = fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap();
    let sample_paths = sample_files(&temp.0);
    let old_samples = sample_paths
        .iter()
        .map(|path| {
            (
                fs::read(path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    project.set_bpm(91.0).unwrap();
    save_project_directory(&project, &temp.0).unwrap();
    assert_eq!(sample_files(&temp.0), sample_paths);
    for (path, (bytes, modified)) in sample_paths.iter().zip(old_samples) {
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    }
    project.add_asset(AudioBuffer::new(vec![[0.31415927, -0.2]; 32]).unwrap());
    save_project_directory(&project, &temp.0).unwrap();
    assert_eq!(sample_files(&temp.0).len(), 3);
    fs::write(temp.0.join("old.symp"), old_manifest).unwrap();
    assert_eq!(
        encoded(&load_project(temp.0.join("old.symp")).unwrap()),
        original
    );
    assert!(
        sample_files(&temp.0)
            .iter()
            .all(|path| path.extension().unwrap() == "wav")
    );
}

#[test]
fn missing_corrupt_truncated_and_wrong_length_samples_are_rejected() {
    let temp = TestDirectory::new();
    let mut project = Project::new();
    project.add_asset(AudioBuffer::new(vec![[0.5, -0.5]; 2]).unwrap());
    save_project_directory(&project, &temp.0).unwrap();
    let sample = sample_files(&temp.0).pop().unwrap();
    let original = fs::read(&sample).unwrap();
    fs::remove_file(&sample).unwrap();
    assert!(load_project(&temp.0).is_err());
    for bad in [
        b"not wave audio".to_vec(),
        original[..original.len() - 1].to_vec(),
    ] {
        fs::write(&sample, bad).unwrap();
        assert!(load_project(&temp.0).is_err());
    }
    fs::write(&sample, &original).unwrap();
    let mut manifest = fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap();
    let name_len = u64::from_le_bytes(manifest[46..54].try_into().unwrap()) as usize;
    let count_offset = 54 + name_len;
    manifest[count_offset..count_offset + 8].copy_from_slice(&99_u64.to_le_bytes());
    fs::write(temp.0.join(PROJECT_MANIFEST), manifest).unwrap();
    assert!(
        load_project(&temp.0)
            .unwrap_err()
            .to_string()
            .contains("frame count")
    );
}

#[test]
fn sample_paths_reject_traversal_absolute_paths_and_nested_directories() {
    let temp = TestDirectory::new();
    let mut project = Project::new();
    project.add_asset(AudioBuffer::new(vec![[0.5, -0.5]]).unwrap());
    save_project_directory(&project, &temp.0).unwrap();
    let original = fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap();
    let name_len = u64::from_le_bytes(original[46..54].try_into().unwrap()) as usize;
    for reference in [
        "../outside.wav",
        "/tmp/outside.wav",
        "samples/../outside.wav",
        "samples/sub/nested.wav",
        "samples/evil\\path.wav",
        "samples/C:drive.wav",
    ] {
        let mut manifest = original[..46].to_vec();
        manifest.extend_from_slice(&(reference.len() as u64).to_le_bytes());
        manifest.extend_from_slice(reference.as_bytes());
        manifest.extend_from_slice(&original[54 + name_len..]);
        fs::write(temp.0.join(PROJECT_MANIFEST), manifest).unwrap();
        assert!(
            load_project(&temp.0)
                .unwrap_err()
                .to_string()
                .contains("invalid sample path"),
            "{reference}"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_sample_and_samples_directory_are_rejected() {
    use std::os::unix::fs::symlink;
    let temp = TestDirectory::new();
    let mut project = Project::new();
    project.add_asset(AudioBuffer::new(vec![[0.5, -0.5]]).unwrap());
    let bundle = temp.0.join("Bundle");
    save_project_directory(&project, &bundle).unwrap();
    let sample = sample_files(&bundle).pop().unwrap();
    let outside = temp.0.join("outside.wav");
    fs::rename(&sample, &outside).unwrap();
    symlink(&outside, &sample).unwrap();
    assert!(
        load_project(&bundle)
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
    fs::remove_file(&sample).unwrap();
    fs::rename(&outside, &sample).unwrap();
    fs::rename(bundle.join("samples"), temp.0.join("outside")).unwrap();
    symlink(temp.0.join("outside"), bundle.join("samples")).unwrap();
    assert!(load_project(&bundle).is_err());
    let manifest = fs::read(bundle.join(PROJECT_MANIFEST)).unwrap();
    assert!(save_project_directory(&project, &bundle).is_err());
    assert_eq!(fs::read(bundle.join(PROJECT_MANIFEST)).unwrap(), manifest);
}

#[test]
fn failed_sample_save_preserves_previous_project_and_manifest() {
    let temp = TestDirectory::new();
    save_project_directory(&Project::new(), &temp.0).unwrap();
    let original = fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap();
    fs::remove_dir(temp.0.join("samples")).unwrap();
    fs::write(temp.0.join("samples"), b"unrelated file").unwrap();
    assert!(save_project_directory(&fixture(), &temp.0).is_err());
    assert_eq!(fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap(), original);
    assert_eq!(load_project(&temp.0).unwrap().tracks().len(), 1);
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 2);
}

#[test]
fn occupied_sample_name_is_never_overwritten_and_reuses_the_alternative_on_resave() {
    let temp = TestDirectory::new();
    let mut project = Project::new();
    project.add_asset(AudioBuffer::new(vec![[0.5, -0.5]]).unwrap());
    save_project_directory(&project, &temp.0).unwrap();
    let occupied = sample_files(&temp.0).pop().unwrap();
    fs::write(&occupied, b"unrelated user data").unwrap();
    save_project_directory(&project, &temp.0).unwrap();
    assert_eq!(fs::read(&occupied).unwrap(), b"unrelated user data");
    assert_eq!(sample_files(&temp.0).len(), 2);
    assert_eq!(encoded(&load_project(&temp.0).unwrap()), encoded(&project));
    save_project_directory(&project, &temp.0).unwrap();
    assert_eq!(sample_files(&temp.0).len(), 2);
}

#[test]
fn independently_constructed_v5_file_still_loads_with_unmuted_tracks() {
    let temp = TestDirectory::new();
    let mut bytes = b"SYMPROJ\0".to_vec();
    bytes.extend_from_slice(&5_u32.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&120.0_f64.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.push(3);
    bytes.extend_from_slice(&6_u64.to_le_bytes());
    bytes.extend_from_slice(b"Master");
    bytes.extend_from_slice(&1.0_f32.to_le_bytes());
    bytes.extend_from_slice(&0.0_f32.to_le_bytes());
    bytes.extend_from_slice(&[2, 0]);
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    let path = temp.0.join("old.symp");
    fs::write(&path, &bytes).unwrap();
    let project = load_project(&path).unwrap();
    assert!(!project.master_track().mixer().muted());
    assert_eq!(
        encoded(&project),
        encoded(&read_project(bytes.as_slice()).unwrap())
    );
    save_project_directory(&project, temp.0.join("Migrated")).unwrap();
    assert_eq!(
        encoded(&load_project(temp.0.join("Migrated")).unwrap()),
        encoded(&project)
    );
}
