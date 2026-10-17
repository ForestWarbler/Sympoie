use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sympoie::clip::AudioClip;
use sympoie::pitch::PitchAlgorithm;
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{
    PROJECT_MANIFEST, load_project, read_project, save_project, save_project_directory,
    write_project,
};
use sympoie::track::Track;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sympoie-pitch-test-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn samples() -> Vec<[f32; 2]> {
    (0..8).map(|index| [index as f32 / 8.0, -0.125]).collect()
}

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn fixture() -> Project {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(samples()).unwrap());
    let audio = project.add_audio_track("Audio");
    let settings = [
        PitchAlgorithm::Builtin,
        PitchAlgorithm::Signalsmith,
        PitchAlgorithm::RubberBandR3,
    ]
    .into_iter()
    .flat_map(|algorithm| {
        [false, true].into_iter().flat_map(move |formants| {
            [-24.0, -5.375, 0.0, 12.25, 24.0]
                .into_iter()
                .map(move |pitch| (algorithm, formants, pitch))
        })
    });
    for (index, (algorithm, formants, pitch)) in settings.enumerate() {
        project
            .add_audio_clip(
                audio,
                AudioClip {
                    asset,
                    source_start: 2,
                    start_tick: 77 + index as u64 * 960,
                    frame_count: 3,
                    gain: 0.5,
                    pitch_semitones: pitch,
                    pitch_algorithm: algorithm,
                    preserve_formants: formants,
                },
            )
            .unwrap();
    }
    project
}

fn assert_preserved(original: &Project, loaded: &Project) {
    assert_eq!(encoded(original), encoded(loaded));
    let (Track::Audio(before), Track::Audio(after)) = (&original.tracks()[1], &loaded.tracks()[1])
    else {
        panic!("expected audio tracks");
    };
    assert_eq!(before.clips(), after.clips());
    assert_eq!(
        loaded
            .audio_asset(after.clips()[0].asset)
            .unwrap()
            .samples(),
        samples()
    );
    assert_eq!(
        original.audio_duration_frames(),
        loaded.audio_duration_frames()
    );
}

#[test]
fn all_pitch_algorithms_and_formant_options_round_trip_with_trimmed_sources() {
    let project = fixture();
    let bytes = encoded(&project);
    assert_eq!(&bytes[8..12], &13_u32.to_le_bytes());
    assert_preserved(&project, &read_project(bytes.as_slice()).unwrap());

    let temp = TestDirectory::new();
    let standalone = temp.0.join("Pitched.symp");
    save_project(&project, &standalone).unwrap();
    assert_preserved(&project, &load_project(standalone).unwrap());
    let directory = temp.0.join("Pitched");
    save_project_directory(&project, &directory).unwrap();
    assert_preserved(&project, &load_project(&directory).unwrap());
    assert_preserved(
        &project,
        &load_project(directory.join(PROJECT_MANIFEST)).unwrap(),
    );
    assert_eq!(fs::read_dir(directory.join("samples")).unwrap().count(), 1);
}

fn historical_project(version: u32, reference: Option<&str>, pitch: f32) -> Vec<u8> {
    project_bytes(version, reference, pitch, 0, 0)
}

fn project_bytes(
    version: u32,
    reference: Option<&str>,
    pitch: f32,
    algorithm: u8,
    formants: u8,
) -> Vec<u8> {
    let mut bytes = b"SYMPROJ\0".to_vec();
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&120.0_f64.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.push(0);
    if version >= 6 {
        bytes.push(u8::from(reference.is_some()));
    }
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    if let Some(reference) = reference {
        bytes.extend_from_slice(&(reference.len() as u64).to_le_bytes());
        bytes.extend_from_slice(reference.as_bytes());
        bytes.extend_from_slice(&8_u64.to_le_bytes());
    } else {
        bytes.extend_from_slice(&8_u64.to_le_bytes());
        for sample in samples().into_iter().flatten() {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&2_u64.to_le_bytes());
    for (tag, name, route) in [(3, "Master", 2), (0, "Audio", 0)] {
        bytes.push(tag);
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        bytes.push(route);
        if version >= 5 {
            bytes.push(0);
        }
        if version >= 6 {
            bytes.push(0);
        }
        if tag == 0 {
            bytes.extend_from_slice(&1_u64.to_le_bytes());
            bytes.extend_from_slice(&0_u64.to_le_bytes());
            bytes.extend_from_slice(&2_u64.to_le_bytes());
            bytes.extend_from_slice(&77_u64.to_le_bytes());
            bytes.extend_from_slice(&3_u64.to_le_bytes());
            bytes.extend_from_slice(&0.5_f32.to_le_bytes());
            if version >= 9 {
                bytes.extend_from_slice(&pitch.to_le_bytes());
            }
            if version >= 10 {
                bytes.extend_from_slice(&[algorithm, formants]);
            }
        }
    }
    if version >= 3 {
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        for _ in 0..2 {
            bytes.push(0);
            bytes.extend_from_slice(&0_u64.to_le_bytes());
        }
    }
    if version >= 4 {
        bytes.extend_from_slice(&0_u64.to_le_bytes());
    }
    if version >= 7 {
        bytes.extend_from_slice(&[0, 0]);
    }
    if version >= 8 {
        for _ in 0..2 {
            bytes.extend_from_slice(&0_u64.to_le_bytes());
        }
    }
    bytes
}

fn assert_original_pitch(project: &Project) {
    let Track::Audio(audio) = &project.tracks()[1] else {
        panic!("expected audio track");
    };
    let clip = audio.clips()[0];
    assert_eq!(clip.pitch_semitones, 0.0);
    assert_eq!(clip.pitch_algorithm, PitchAlgorithm::Builtin);
    assert!(!clip.preserve_formants);
    assert_eq!(clip.source_start, 2);
    assert_eq!(clip.frame_count, 3);
    assert_eq!(clip.start_tick, 77);
    assert_eq!(clip.gain, 0.5);
    assert_eq!(
        project.audio_asset(clip.asset).unwrap().samples(),
        samples()
    );
}

#[test]
fn versions_one_through_eight_load_audio_clips_at_original_pitch_and_upgrade_losslessly() {
    for version in 1..=8 {
        let loaded = read_project(historical_project(version, None, 0.0).as_slice()).unwrap();
        assert_original_pitch(&loaded);
        let upgraded = read_project(encoded(&loaded).as_slice()).unwrap();
        assert_original_pitch(&upgraded);
        assert_preserved(&loaded, &upgraded);
    }
}

#[test]
fn version_eight_directory_loads_at_original_pitch_and_saves_as_version_thirteen() {
    let temp = TestDirectory::new();
    save_project_directory(&fixture(), &temp.0).unwrap();
    let sample = fs::read_dir(temp.0.join("samples"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    let reference = format!("samples/{}", sample.to_str().unwrap());
    fs::write(
        temp.0.join(PROJECT_MANIFEST),
        historical_project(8, Some(&reference), 0.0),
    )
    .unwrap();
    let loaded = load_project(&temp.0).unwrap();
    assert_original_pitch(&loaded);
    save_project_directory(&loaded, &temp.0).unwrap();
    assert_original_pitch(&load_project(&temp.0).unwrap());
    assert_eq!(
        &fs::read(temp.0.join(PROJECT_MANIFEST)).unwrap()[8..12],
        &13_u32.to_le_bytes()
    );
}

#[test]
fn loading_rejects_nonfinite_and_out_of_range_pitch() {
    for pitch in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -24.001, 24.001] {
        assert!(
            read_project(historical_project(9, None, pitch).as_slice()).is_err(),
            "accepted invalid pitch {pitch}",
        );
    }
}

#[test]
fn version_nine_pitch_keeps_builtin_algorithm_when_upgraded() {
    for pitch in [-5.375, 0.0, 12.25] {
        let loaded = read_project(historical_project(9, None, pitch).as_slice()).unwrap();
        let Track::Audio(audio) = &loaded.tracks()[1] else {
            panic!("expected audio track");
        };
        assert_eq!(audio.clips()[0].pitch_semitones, pitch);
        assert_eq!(audio.clips()[0].pitch_algorithm, PitchAlgorithm::Builtin);
        assert!(!audio.clips()[0].preserve_formants);
        let upgraded = read_project(encoded(&loaded).as_slice()).unwrap();
        assert_preserved(&loaded, &upgraded);
    }
}

#[test]
fn loading_rejects_unknown_algorithm_and_nonboolean_formant_flag() {
    for algorithm in [3, 127, 255] {
        let error =
            read_project(project_bytes(10, None, 7.25, algorithm, 1).as_slice()).unwrap_err();
        assert!(error.to_string().contains("unknown pitch algorithm"));
    }
    for formants in [2, 127, 255] {
        assert!(
            read_project(project_bytes(10, None, 7.25, 1, formants).as_slice()).is_err(),
            "accepted invalid formant flag {formants}",
        );
    }
}
