use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{ProjectFileError, read_project, write_project};
use sympoie::render::Renderer;
use sympoie::time::{SnapGrid, TimeSignature};
use sympoie::track::{AudioOutput, MixerSettings, Track};

fn encoded(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn fixture(midi_notes: bool) -> Project {
    let mut project = Project::new();
    project.set_bpm(137.125).unwrap();
    project.set_time_signature(TimeSignature::new(7, 8).unwrap());
    project.add_asset(AudioBuffer::new(Vec::new()).unwrap());
    let shared =
        project.add_asset(AudioBuffer::new(vec![[0.12345679, -0.0], [-0.73123455, 0.75]]).unwrap());
    project.add_asset(
        AudioBuffer::new(vec![[f32::from_bits(1), f32::MAX], [-0.0, -f32::MAX]]).unwrap(),
    );
    let audio = project.add_audio_track("鼓 🥁\0take");
    let midi = project.add_midi_track("Piano");
    let second = project.add_audio_track("");
    let parent = project.add_group_track("Band");
    let child = project.add_group_track("Rhythm");
    let clip = AudioClip {
        asset: shared,
        source_start: 0,
        start_tick: 3,
        frame_count: 2,
        gain: -0.625,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
    };
    project.add_audio_clip(audio, clip).unwrap();
    project
        .add_audio_clip(
            audio,
            AudioClip {
                start_tick: 7,
                gain: -0.0,
                ..clip
            },
        )
        .unwrap();
    project
        .add_audio_clip(
            second,
            AudioClip {
                source_start: 1,
                frame_count: 1,
                start_tick: 10,
                ..clip
            },
        )
        .unwrap();
    if midi_notes {
        project
            .add_midi_clip(
                midi,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 17,
                    length_ticks: 2880,
                    notes: vec![
                        MidiNote {
                            start_tick: 1000,
                            duration_ticks: 12,
                            key: 127,
                            velocity: 1,
                            channel: 15,
                        },
                        MidiNote {
                            start_tick: 0,
                            duration_ticks: 960,
                            key: 60,
                            velocity: 127,
                            channel: 0,
                        },
                    ],
                },
            )
            .unwrap();
    }
    project
        .set_audio_track_mixer(
            audio,
            MixerSettings::new(0.8, -0.2, AudioOutput::Group(child)).unwrap(),
        )
        .unwrap();
    project
        .set_midi_track_mixer(
            midi,
            MixerSettings::new(1.125, 0.25, AudioOutput::Group(child)).unwrap(),
        )
        .unwrap();
    project
        .set_audio_track_mixer(
            second,
            MixerSettings::new(0.5, 0.5, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            child,
            MixerSettings::new(0.9, 0.3, AudioOutput::Group(parent)).unwrap(),
        )
        .unwrap();
    project
        .set_group_track_mixer(
            parent,
            MixerSettings::new(0.7, -0.3, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .set_master_track_mixer(MixerSettings::new(0.875, -0.125, AudioOutput::Output).unwrap())
        .unwrap();
    project.set_snap_grid(SnapGrid::Bar);
    project
}

#[test]
fn complete_round_trip_preserves_bits_assets_identity_tracks_routes_notes_and_off_grid_positions() {
    let original = fixture(true);
    let bytes = encoded(&original);
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(encoded(&restored), bytes);
    assert_eq!(restored.bpm().to_bits(), original.bpm().to_bits());
    assert_eq!(restored.time_signature(), original.time_signature());
    assert_eq!(restored.snap_grid(), SnapGrid::Bar);
    assert_eq!(
        restored.audio_duration_frames(),
        original.audio_duration_frames()
    );
    assert_eq!(
        restored
            .tracks()
            .iter()
            .map(Track::name)
            .collect::<Vec<_>>(),
        ["Master", "鼓 🥁\0take", "Piano", "", "Band", "Rhythm"]
    );
    let Track::Audio(first) = &restored.tracks()[1] else {
        panic!()
    };
    let Track::Audio(second) = &restored.tracks()[3] else {
        panic!()
    };
    assert_eq!(first.clips()[0].asset, first.clips()[1].asset);
    assert_eq!(first.clips()[0].asset, second.clips()[0].asset);
    assert_eq!(first.clips()[0].start_tick, 3);
    assert_eq!(first.clips()[1].gain.to_bits(), (-0.0_f32).to_bits());
    let Track::Midi(midi) = &restored.tracks()[2] else {
        panic!()
    };
    assert_eq!(midi.clips()[0].start_tick, 17);
    assert_eq!(midi.clips()[0].notes[0].start_tick, 1000);
    assert_eq!(midi.clips()[0].notes[1].start_tick, 0);
}

#[test]
fn restored_audio_renders_bit_identically_with_forward_group_routes() {
    let original = fixture(false);
    let restored = read_project(encoded(&original).as_slice()).unwrap();
    let count = original.audio_duration_frames() as usize + 10;
    let mut before = vec![[0.0; 2]; count];
    let mut after = before.clone();
    Renderer::new(&original)
        .unwrap()
        .render_block(0, &mut before);
    Renderer::new(&restored)
        .unwrap()
        .render_block(0, &mut after);
    assert!(before.iter().flatten().any(|sample| *sample != 0.0));
    assert_eq!(
        before
            .iter()
            .flatten()
            .map(|x| x.to_bits())
            .collect::<Vec<_>>(),
        after
            .iter()
            .flatten()
            .map(|x| x.to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn empty_project_and_every_snap_grid_round_trip() {
    for grid in [
        SnapGrid::Off,
        SnapGrid::Bar,
        SnapGrid::Quarter,
        SnapGrid::Eighth,
        SnapGrid::Sixteenth,
    ] {
        let mut project = Project::new();
        project.set_snap_grid(grid);
        project
            .set_master_track_mixer(
                MixerSettings::new(0.0, 1.0, AudioOutput::Disconnected).unwrap(),
            )
            .unwrap();
        let restored = read_project(encoded(&project).as_slice()).unwrap();
        assert_eq!(encoded(&restored), encoded(&project));
        assert_eq!(restored.tracks().len(), 1);
    }
}

#[test]
fn rejects_every_truncation_and_trailing_data() {
    let bytes = encoded(&fixture(true));
    for end in 0..bytes.len() {
        assert!(
            read_project(&bytes[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
    let mut extra = bytes;
    extra.push(0);
    assert!(matches!(
        read_project(extra.as_slice()),
        Err(ProjectFileError::InvalidFormat(_))
    ));
}

#[test]
fn rejects_bad_header_timing_strings_and_mixer_values() {
    let empty = encoded(&Project::new());
    for (offset, replacement) in [
        (0, vec![0]),
        (12, 44_100_u32.to_le_bytes().to_vec()),
        (16, 480_u64.to_le_bytes().to_vec()),
        (24, f64::NAN.to_le_bytes().to_vec()),
        (24, 0.0_f64.to_le_bytes().to_vec()),
        (32, 0_u16.to_le_bytes().to_vec()),
        (34, 3_u16.to_le_bytes().to_vec()),
        (36, vec![255]),
        (37, vec![255]),
        (63, vec![255]),
        (63, vec![b'x']),
        (69, f32::INFINITY.to_le_bytes().to_vec()),
        (73, 1.5_f32.to_le_bytes().to_vec()),
        (77, vec![255]),
        (78, vec![2]),
        (78, vec![1]),
        (79, vec![2]),
    ] {
        let mut bytes = empty.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        assert!(
            read_project(bytes.as_slice()).is_err(),
            "accepted invalid field at {offset}"
        );
    }
    let mut version = empty;
    version[8..12].copy_from_slice(&14_u32.to_le_bytes());
    assert!(matches!(
        read_project(version.as_slice()),
        Err(ProjectFileError::UnsupportedVersion(14))
    ));
}

#[test]
fn rejects_huge_counts_before_allocating_the_claimed_contents() {
    for offset in [38, 46, 55] {
        let mut bytes = encoded(&Project::new());
        bytes[offset..offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            read_project(bytes.as_slice()),
            Err(ProjectFileError::InvalidFormat(_))
        ));
    }
    let mut project = Project::new();
    project.add_asset(AudioBuffer::new(vec![[0.0; 2]]).unwrap());
    let mut bytes = encoded(&project);
    bytes[46..54].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(read_project(bytes.as_slice()).is_err());
}

fn route_record(kind: u8, destination: u8, group: u64) -> Vec<u8> {
    let mut bytes = vec![kind];
    let name: &[u8] = if kind == 3 { b"Master" } else { b"x" };
    bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(&1.0_f32.to_le_bytes());
    bytes.extend_from_slice(&0.0_f32.to_le_bytes());
    bytes.push(destination);
    if destination == 1 {
        bytes.extend_from_slice(&group.to_le_bytes());
    }
    if kind == 0 || kind == 1 {
        bytes.extend_from_slice(&0_u64.to_le_bytes());
    }
    bytes
}

fn routing_file(records: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"SYMPROJ\0".to_vec();
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&960_u64.to_le_bytes());
    bytes.extend_from_slice(&120.0_f64.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&(records.len() as u64).to_le_bytes());
    for record in records {
        bytes.extend_from_slice(record);
    }
    bytes
}

#[test]
fn rejects_missing_duplicate_misplaced_and_unknown_track_kinds() {
    for records in [
        vec![],
        vec![route_record(0, 0, 0)],
        vec![route_record(3, 2, 0), route_record(3, 2, 0)],
        vec![route_record(3, 2, 0), route_record(255, 0, 0)],
    ] {
        assert!(matches!(
            read_project(routing_file(&records).as_slice()),
            Err(ProjectFileError::InvalidFormat(_))
        ));
    }
}

#[test]
fn rejects_invalid_group_references_cycles_and_master_bypasses() {
    let master = route_record(3, 2, 0);
    for records in [
        vec![master.clone(), route_record(0, 1, 999)],
        vec![master.clone(), route_record(0, 1, 1)],
        vec![master.clone(), route_record(2, 1, 0)],
        vec![master.clone(), route_record(2, 1, 1)],
        vec![master.clone(), route_record(2, 1, 2), route_record(2, 1, 1)],
        vec![master.clone(), route_record(0, 2, 0)],
        vec![master.clone(), route_record(1, 2, 0)],
        vec![master.clone(), route_record(2, 2, 0)],
        vec![route_record(3, 0, 0)],
        vec![route_record(3, 1, 1), route_record(2, 0, 0)],
    ] {
        assert!(matches!(
            read_project(routing_file(&records).as_slice()),
            Err(ProjectFileError::Project(_))
        ));
    }
    let cycle = routing_file(&[master, route_record(2, 1, 2), route_record(2, 1, 1)]);
    assert!(matches!(
        read_project(cycle.as_slice()),
        Err(ProjectFileError::Project(ProjectError::RoutingCycle { .. }))
    ));
}

#[test]
fn rejects_nonfinite_audio_invalid_clip_asset_source_and_timeline_ranges() {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.0; 2]]).unwrap());
    let audio = project.add_audio_track("x");
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 1,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
            },
        )
        .unwrap();
    let original = encoded(&project);
    for (offset, replacement) in [
        (54, f32::NAN.to_le_bytes().to_vec()),
        (125, 1_u64.to_le_bytes().to_vec()),
        (133, 1_u64.to_le_bytes().to_vec()),
        (141, u64::MAX.to_le_bytes().to_vec()),
        (149, 0_u64.to_le_bytes().to_vec()),
        (149, u64::MAX.to_le_bytes().to_vec()),
        (157, f32::INFINITY.to_le_bytes().to_vec()),
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        assert!(
            read_project(bytes.as_slice()).is_err(),
            "accepted malformed audio at {offset}"
        );
    }
}

#[test]
fn rejects_invalid_midi_ranges_and_note_fields() {
    let mut project = Project::new();
    let midi = project.add_midi_track("x");
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 960,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 960,
                    key: 60,
                    velocity: 100,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    let original = encoded(&project);
    for (offset, replacement) in [
        (102, u64::MAX.to_le_bytes().to_vec()),
        (110, u64::MAX.to_le_bytes().to_vec()),
        (118, 0_u64.to_le_bytes().to_vec()),
        (126, u64::MAX.to_le_bytes().to_vec()),
        (134, u64::MAX.to_le_bytes().to_vec()),
        (142, u64::MAX.to_le_bytes().to_vec()),
        (150, 0_u64.to_le_bytes().to_vec()),
        (158, vec![128]),
        (159, vec![0]),
        (159, vec![128]),
        (160, vec![16]),
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        assert!(
            read_project(bytes.as_slice()).is_err(),
            "accepted malformed MIDI at {offset}"
        );
    }
}

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "sympoie-storage-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
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

#[test]
fn path_save_creates_and_replaces_complete_projects_without_leaving_temporary_files() {
    let directory = TestDirectory::new();
    let path = directory.0.join("工程.symp");
    Project::new().save(&path).unwrap();
    assert_eq!(Project::load(&path).unwrap().tracks().len(), 1);
    let project = fixture(true);
    project.save(&path).unwrap();
    assert_eq!(encoded(&Project::load(&path).unwrap()), encoded(&project));
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
}

#[test]
fn failed_replacement_cleans_up_temporary_file_and_preserves_destination() {
    let directory = TestDirectory::new();
    let destination = directory.0.join("existing.symp");
    fs::create_dir(&destination).unwrap();
    let original_path = destination.join("original.symp");
    Project::new().save(&original_path).unwrap();
    let original_bytes = fs::read(&original_path).unwrap();
    assert!(fixture(true).save(&destination).is_err());
    assert_eq!(fs::read(&original_path).unwrap(), original_bytes);
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
}

#[test]
fn stream_io_errors_propagate() {
    struct FailsAfter(usize);
    impl Write for FailsAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::other("test disk failure"));
            }
            let count = self.0.min(bytes.len());
            self.0 -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert!(matches!(
        write_project(&fixture(true), FailsAfter(100)),
        Err(ProjectFileError::Io(_))
    ));
    struct BrokenReader;
    impl io::Read for BrokenReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("test read failure"))
        }
    }
    assert!(matches!(
        read_project(BrokenReader),
        Err(ProjectFileError::Io(_))
    ));
}

#[test]
fn track_colours_and_hidden_midi_notes_round_trip() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Drums");
    let group = project.add_group_track("Bus");
    let keys = project.add_midi_track("Keys");
    project.set_track_color(audio, Some(0xff94a6)).unwrap();
    project.set_track_color(group, Some(0x5480e4)).unwrap();
    assert_eq!(
        project.set_track_color(project.master_track_id(), Some(1)),
        Err(ProjectError::MasterColor)
    );
    assert_eq!(
        project.set_track_color(audio, Some(0x100_0000)),
        Err(ProjectError::InvalidTrackColor { color: 0x100_0000 })
    );
    let note = MidiNote {
        start_tick: 0,
        duration_ticks: 480,
        key: 60,
        velocity: 90,
        channel: 0,
    };
    let clip = MidiClip {
        start_tick: 3840,
        length_ticks: 960,
        offset_ticks: 960,
        notes: vec![
            note,
            MidiNote {
                start_tick: 1200,
                ..note
            },
        ],
    };
    project.add_midi_clip(keys, clip.clone()).unwrap();

    let loaded = read_project(encoded(&project).as_slice()).unwrap();
    let colors: Vec<_> = loaded.tracks().iter().map(Track::color).collect();
    assert_eq!(colors, [None, Some(0xff94a6), Some(0x5480e4), None]);
    assert_eq!(
        loaded.midi_track(keys).unwrap().clips(),
        std::slice::from_ref(&clip)
    );
}
