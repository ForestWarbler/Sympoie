use sympoie::TICKS_PER_QUARTER;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::instrument::{Instrument, SubtractiveSettings};
use sympoie::project::{AssetId, AudioBuffer, Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, GroupTrackId, MixerSettings, Track, TrackId};

fn clip(asset: AssetId, start_tick: u64, gain: f32) -> AudioClip {
    AudioClip {
        asset,
        source_start: 0,
        start_tick,
        frame_count: 2,
        gain,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    }
}

fn render(project: &Project, frames: usize) -> Vec<[f32; 2]> {
    let mut output = vec![[0.0; 2]; frames];
    Renderer::new(project).unwrap().render_block(0, &mut output);
    output
}

fn names(project: &Project) -> Vec<&str> {
    project.tracks().iter().map(Track::name).collect()
}

fn round_trip(project: &Project) -> Project {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    read_project(bytes.as_slice()).unwrap()
}

#[test]
fn track_ids_address_every_track_of_a_loaded_project_in_order() {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.5, 0.25]; 2]).unwrap());
    let voice = project.add_audio_track("Voice");
    let keys = project.add_midi_track("Keys");
    let band = project.add_group_track("Band");
    project.set_track_group(voice, Some(band)).unwrap();
    project.add_audio_clip(voice, clip(asset, 0, 1.0)).unwrap();

    let ids: Vec<_> = project.track_ids().collect();
    assert_eq!(
        ids,
        [
            project.master_track_id().into(),
            voice.into(),
            keys.into(),
            band.into()
        ]
    );
    assert_eq!(ids.len(), project.tracks().len());

    let mut loaded = round_trip(&project);
    let loaded_ids: Vec<_> = loaded.track_ids().collect();
    assert_eq!(loaded_ids, ids);
    let TrackId::Audio(loaded_voice) = loaded_ids[1] else {
        panic!("expected an audio track");
    };
    let TrackId::Group(loaded_band) = loaded_ids[3] else {
        panic!("expected a group track");
    };
    assert_eq!(
        loaded
            .group_members(loaded_band)
            .unwrap()
            .collect::<Vec<_>>(),
        [loaded_ids[1]]
    );
    loaded
        .move_audio_clip(loaded_voice, 0, TICKS_PER_QUARTER)
        .unwrap();
    assert_eq!(
        loaded.audio_track(loaded_voice).unwrap().clips()[0].start_tick,
        TICKS_PER_QUARTER
    );
}

#[test]
fn audio_assets_are_readable_by_id_and_unknown_ids_are_rejected() {
    let mut other = Project::new();
    other.add_asset(AudioBuffer::new(vec![[0.0; 2]]).unwrap());
    let foreign = other.add_asset(AudioBuffer::new(vec![[0.0; 2]]).unwrap());

    let mut project = Project::new();
    let samples = vec![[0.5, -0.5], [0.25, 0.125]];
    let asset = project.add_asset(AudioBuffer::new(samples.clone()).unwrap());
    assert_eq!(project.audio_asset(asset).unwrap().samples(), samples);
    assert_eq!(
        project.audio_asset(foreign).unwrap_err(),
        ProjectError::UnknownAsset { asset: foreign }
    );
    let loaded = round_trip(&project);
    assert_eq!(loaded.audio_asset(asset).unwrap().samples(), samples);
}

#[test]
fn removing_clips_returns_them_and_shifts_later_indices() {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[1.0, 1.0]; 2]).unwrap());
    let track = project.add_audio_track("Audio");
    for (start, gain) in [(0, 0.5), (TICKS_PER_QUARTER, 0.25), (0, 0.125)] {
        project
            .add_audio_clip(track, clip(asset, start, gain))
            .unwrap();
    }
    assert_eq!(render(&project, 1), [[0.625; 2]]);

    assert_eq!(
        project.remove_audio_clip(track, 0).unwrap(),
        clip(asset, 0, 0.5)
    );
    let gains: Vec<_> = project
        .audio_track(track)
        .unwrap()
        .clips()
        .iter()
        .map(|clip| clip.gain)
        .collect();
    assert_eq!(gains, [0.25, 0.125]);
    assert_eq!(render(&project, 1), [[0.125; 2]]);

    let before = project.audio_track(track).unwrap().clips().to_vec();
    assert_eq!(
        project.remove_audio_clip(track, 2),
        Err(ProjectError::UnknownAudioClip { track, index: 2 })
    );
    assert_eq!(project.audio_track(track).unwrap().clips(), before);
    project.remove_audio_clip(track, 0).unwrap();
    project.remove_audio_clip(track, 0).unwrap();
    assert!(project.audio_track(track).unwrap().clips().is_empty());
    assert_eq!(project.audio_duration_frames(), 0);
    project.add_audio_clip(track, clip(asset, 0, 1.0)).unwrap();
}

#[test]
fn removing_midi_clips_removes_their_instrument_audio_from_a_new_renderer() {
    let mut project = Project::new();
    let keys = project.add_midi_track("Keys");
    project
        .set_midi_track_instrument(
            keys,
            Some(Instrument::Subtractive(SubtractiveSettings::default())),
        )
        .unwrap();
    let clip = MidiClip {
        offset_ticks: 0,
        start_tick: 0,
        length_ticks: TICKS_PER_QUARTER,
        notes: vec![MidiNote {
            start_tick: 0,
            duration_ticks: TICKS_PER_QUARTER,
            key: 60,
            velocity: 90,
            channel: 0,
            slide_ticks: 0,
        }],
        name: None,
        pitch_bend: Vec::new(),
    };
    project.add_midi_clip(keys, clip.clone()).unwrap();
    assert!(
        render(&project, 512)
            .iter()
            .flatten()
            .any(|sample| *sample != 0.0)
    );
    assert_eq!(
        project.remove_midi_clip(keys, 1),
        Err(ProjectError::UnknownMidiClip {
            track: keys,
            index: 1
        })
    );
    assert_eq!(project.remove_midi_clip(keys, 0).unwrap(), clip);
    assert_eq!(render(&project, 512), vec![[0.0; 2]; 512]);
    assert!(project.midi_track(keys).unwrap().instrument().is_some());
    assert_eq!(project.duration_frames().unwrap(), 0);
}

#[test]
fn clip_gain_edits_are_validated_and_rendered() {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.5, 0.25]; 2]).unwrap());
    let track = project.add_audio_track("Audio");
    project.add_audio_clip(track, clip(asset, 0, 1.0)).unwrap();

    project.set_audio_clip_gain(track, 0, -2.0).unwrap();
    assert_eq!(render(&project, 1), [[-1.0, -0.5]]);
    for gain in [f32::NAN, f32::INFINITY] {
        assert_eq!(
            project.set_audio_clip_gain(track, 0, gain),
            Err(ProjectError::NonFiniteGain)
        );
    }
    assert_eq!(
        project.set_audio_clip_gain(track, 1, 1.0),
        Err(ProjectError::UnknownAudioClip { track, index: 1 })
    );
    assert_eq!(project.audio_track(track).unwrap().clips()[0].gain, -2.0);
}

#[test]
fn renaming_changes_saved_names_but_not_the_master() {
    let mut project = Project::new();
    let voice = project.add_audio_track("/tmp/take 3.wav");
    let keys = project.add_midi_track("Keys");
    let band = project.add_group_track("Band");
    project.rename_track(voice, "Lead Vocal").unwrap();
    project.rename_track(keys, "Piano / 钢琴").unwrap();
    project.rename_track(band, "").unwrap();
    assert_eq!(
        project.rename_track(project.master_track_id(), "Main"),
        Err(ProjectError::MasterRename)
    );
    assert_eq!(
        names(&project),
        ["Master", "Lead Vocal", "Piano / 钢琴", ""]
    );
    assert_eq!(names(&round_trip(&project)), names(&project));
}

#[test]
fn removing_tracks_renumbers_later_group_routes() {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[1.0, 1.0]; 2]).unwrap());
    let scratch = project.add_audio_track("Scratch");
    let inner = project.add_group_track("Inner");
    let voice = project.add_audio_track("Voice");
    let outer = project.add_group_track("Outer");
    project.set_track_group(voice, Some(inner)).unwrap();
    project.set_track_group(inner, Some(outer)).unwrap();
    project
        .set_group_track_mixer(
            outer,
            MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
    project
        .add_audio_clip(scratch, clip(asset, 0, 0.25))
        .unwrap();
    project.add_audio_clip(voice, clip(asset, 0, 1.0)).unwrap();
    assert_eq!(render(&project, 1), [[0.75; 2]]);

    let removed = project.remove_track(scratch).unwrap();
    assert!(matches!(&removed, Track::Audio(track) if track.clips().len() == 1));
    assert_eq!(names(&project), ["Master", "Inner", "Voice", "Outer"]);
    let ids: Vec<_> = project.track_ids().collect();
    let (TrackId::Group(inner), TrackId::Audio(voice), TrackId::Group(outer)) =
        (ids[1], ids[2], ids[3])
    else {
        panic!("unexpected track kinds after removal");
    };
    assert_eq!(
        project.audio_track(voice).unwrap().mixer().output(),
        AudioOutput::Group(inner)
    );
    assert_eq!(
        project.group_track(inner).unwrap().mixer().output(),
        AudioOutput::Group(outer)
    );
    assert_eq!(render(&project, 1), [[0.5; 2]]);
    assert_eq!(render(&round_trip(&project), 1), [[0.5; 2]]);
}

#[test]
fn removal_rejects_the_master_unknown_tracks_and_groups_with_members() {
    let mut project = Project::new();
    let band = project.add_group_track("Band");
    let voice = project.add_audio_track("Voice");
    project.set_track_group(voice, Some(band)).unwrap();
    let before = names(&project).join("|");

    assert_eq!(
        project.remove_track(project.master_track_id()).unwrap_err(),
        ProjectError::MasterRemoval
    );
    assert_eq!(
        project.remove_track(band).unwrap_err(),
        ProjectError::GroupNotEmpty { track: band }
    );
    assert_eq!(names(&project).join("|"), before);

    project.set_track_group(voice, None).unwrap();
    assert!(matches!(project.remove_track(band), Ok(Track::Group(_))));
    assert_eq!(names(&project), ["Master", "Voice"]);
    let stale: GroupTrackId = band;
    assert_eq!(
        project.remove_track(stale).unwrap_err(),
        ProjectError::UnknownGroupTrack { track: stale }
    );
}

#[test]
fn clones_share_audio_but_edit_independently() {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.5, 0.5]; 2]).unwrap());
    let track = project.add_audio_track("Audio");
    project.add_audio_clip(track, clip(asset, 0, 1.0)).unwrap();

    let snapshot = project.clone();
    project.set_audio_clip_gain(track, 0, 0.5).unwrap();
    project.rename_track(track, "Edited").unwrap();
    project.set_bpm(90.0).unwrap();

    assert_eq!(render(&snapshot, 1), [[0.5; 2]]);
    assert_eq!(render(&project, 1), [[0.25; 2]]);
    assert_eq!(names(&snapshot), ["Master", "Audio"]);
    assert_eq!(snapshot.bpm(), 120.0);
    assert!(std::ptr::eq(
        snapshot.audio_asset(asset).unwrap().samples(),
        project.audio_asset(asset).unwrap().samples()
    ));
}
