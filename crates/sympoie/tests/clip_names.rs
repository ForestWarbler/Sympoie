use std::fs;
use std::path::PathBuf;

use sympoie::clip::{AudioClip, MidiClip, normalized_clip_name};
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{read_project, write_project};
use sympoie::project_json::{load_project_folder, save_project_folder};
use sympoie::track::{Track, TrackId};

fn named_project() -> Project {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25, -0.25]; 4800]).unwrap());
    let audio = project.add_audio_track("Audio");
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 4800,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    project
        .set_audio_clip_name(audio, 0, Some("  Lead vocal — take 3  "))
        .unwrap();
    let midi = project.add_midi_track("Keys");
    project.add_midi_clip(midi, MidiClip::new(0, 960)).unwrap();
    project
        .add_midi_clip(midi, MidiClip::new(960, 960))
        .unwrap();
    project
        .set_midi_clip_name(midi, 0, Some("Verse chords"))
        .unwrap();
    project
}

fn names(project: &Project) -> Vec<Option<String>> {
    project
        .tracks()
        .iter()
        .flat_map(|track| match track {
            Track::Audio(audio) => audio.clips().iter().map(|c| c.name.clone()).collect(),
            Track::Midi(midi) => midi.clips().iter().map(|c| c.name.clone()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn names_are_trimmed_and_blank_names_clear_the_label() {
    let mut project = named_project();
    assert_eq!(
        names(&project),
        [
            Some("Lead vocal — take 3".into()),
            Some("Verse chords".into()),
            None
        ]
    );
    let Some(TrackId::Midi(midi)) = project.track_ids().nth(2) else {
        panic!("expected a MIDI id")
    };
    project.set_midi_clip_name(midi, 0, Some("   ")).unwrap();
    project.set_midi_clip_name(midi, 1, None).unwrap();
    assert_eq!(names(&project)[1..], [None, None]);
    assert_eq!(normalized_clip_name("\tx\n"), Some("x".into()));
}

#[test]
fn names_round_trip_through_the_binary_format() {
    let project = named_project();
    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    assert_eq!(&bytes[8..12], &16_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(names(&restored), names(&project));
}

#[test]
fn version_fourteen_files_without_names_still_load() {
    let mut project = Project::new();
    let midi = project.add_midi_track("x");
    project.add_midi_clip(midi, MidiClip::new(0, 960)).unwrap();
    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    // Version 14 had no name between the clip's offset and its note count, and no
    // pitch-bend count after the notes.
    assert_eq!(&bytes[150..158], &0_u64.to_le_bytes());
    bytes.drain(150..158);
    assert_eq!(&bytes[134..142], &0_u64.to_le_bytes());
    bytes.drain(134..142);
    bytes[8..12].copy_from_slice(&14_u32.to_le_bytes());
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(restored.tracks()[1].clip_uids().len(), 1);
    assert_eq!(names(&restored), [None]);
}

#[test]
fn names_round_trip_through_the_project_folder() {
    let folder: PathBuf =
        std::env::temp_dir().join(format!("sympoie-clip-names-{}.symp", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    let project = named_project();
    save_project_folder(&project, &folder).unwrap();
    let track = fs::read_dir(folder.join("tracks"))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .find(|text| text.contains("Verse chords"));
    assert!(
        track.is_some(),
        "the MIDI clip name is written to its track"
    );
    let restored = load_project_folder(&folder).unwrap();
    assert_eq!(names(&restored), names(&project));
    fs::remove_dir_all(&folder).unwrap();
}
