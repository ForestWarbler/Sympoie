use sympoie::TICKS_PER_QUARTER;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::time::{SnapGrid, TimeError, TimeSignature};
use sympoie::track::{AudioOutput, MixerSettings, Track, TrackId};

fn note() -> MidiNote {
    MidiNote {
        start_tick: 0,
        duration_ticks: TICKS_PER_QUARTER,
        key: 60,
        velocity: 100,
        channel: 0,
        slide_ticks: 0,
    }
}

fn midi_clip() -> MidiClip {
    MidiClip {
        offset_ticks: 0,
        start_tick: TICKS_PER_QUARTER * 4,
        length_ticks: TICKS_PER_QUARTER * 4,
        notes: vec![note()],
        name: None,
        pitch_bend: Vec::new(),
    }
}

#[test]
fn construction_always_provides_one_master_before_user_tracks() {
    for mut project in [Project::new(), Project::default()] {
        assert_eq!(project.tracks().len(), 1);
        assert!(matches!(project.tracks()[0], Track::Master(_)));
        assert_eq!(project.master_track().name(), "Master");
        let default_master = MixerSettings::new(1.0, 0.0, AudioOutput::Output).unwrap();
        assert_eq!(*project.master_track().mixer(), default_master);
        assert_eq!(project.audio_duration_frames(), 0);

        let master_id = project.master_track_id();
        project.add_audio_track("Audio");
        project.add_midi_track("MIDI");
        project.add_group_track("Group");
        assert_eq!(project.master_track_id(), master_id);
        assert_eq!(TrackId::from(master_id), TrackId::Master(master_id));
        assert_eq!(
            project.tracks().iter().map(Track::name).collect::<Vec<_>>(),
            ["Master", "Audio", "MIDI", "Group"]
        );
        assert_eq!(
            project
                .tracks()
                .iter()
                .filter(|track| matches!(track, Track::Master(_)))
                .count(),
            1
        );
        assert_eq!(*project.master_track().mixer(), default_master);
        for track in &project.tracks()[1..] {
            assert_eq!(*track.mixer(), MixerSettings::default());
        }
    }
}

#[test]
fn master_routes_only_to_final_output_or_disconnected_and_cannot_be_grouped() {
    let mut project = Project::new();
    let group = project.add_group_track("Band");
    let audio = project.add_audio_track("Voice");
    project.set_track_group(audio, Some(group)).unwrap();
    let master = project.master_track_id();

    for output in [AudioOutput::Disconnected, AudioOutput::Output] {
        let mixer = MixerSettings::new(0.75, -0.5, output).unwrap();
        project.set_master_track_mixer(mixer).unwrap();
        assert_eq!(*project.master_track().mixer(), mixer);
        assert_eq!(*project.tracks()[0].mixer(), mixer);
    }
    let original: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| *track.mixer())
        .collect();
    for output in [AudioOutput::Master, AudioOutput::Group(group)] {
        let invalid = MixerSettings::new(0.1, 1.0, output).unwrap();
        assert_eq!(
            project.set_master_track_mixer(invalid),
            Err(ProjectError::InvalidMasterOutput { output })
        );
    }
    for destination in [None, Some(group)] {
        let output = destination.map_or(AudioOutput::Master, AudioOutput::Group);
        assert_eq!(
            project.set_track_group(master, destination),
            Err(ProjectError::InvalidMasterOutput { output })
        );
    }
    assert_eq!(
        project
            .tracks()
            .iter()
            .map(|track| *track.mixer())
            .collect::<Vec<_>>(),
        original
    );
    assert_eq!(
        project.group_members(group).unwrap().collect::<Vec<_>>(),
        [audio.into()]
    );
}

#[test]
fn audio_midi_and_group_tracks_cannot_bypass_master() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Voice");
    let midi = project.add_midi_track("Piano");
    let group = project.add_group_track("Band");
    project.set_track_group(audio, Some(group)).unwrap();
    project.set_track_group(midi, Some(group)).unwrap();
    let original: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| *track.mixer())
        .collect();
    let bypass = MixerSettings::new(0.2, 0.5, AudioOutput::Output).unwrap();
    assert_eq!(
        project.set_audio_track_mixer(audio, bypass),
        Err(ProjectError::MasterBypass {
            track: audio.into()
        })
    );
    assert_eq!(
        project.set_midi_track_mixer(midi, bypass),
        Err(ProjectError::MasterBypass { track: midi.into() })
    );
    assert_eq!(
        project.set_group_track_mixer(group, bypass),
        Err(ProjectError::MasterBypass {
            track: group.into()
        })
    );
    assert_eq!(
        project
            .tracks()
            .iter()
            .map(|track| *track.mixer())
            .collect::<Vec<_>>(),
        original
    );
    assert_eq!(
        project.group_members(group).unwrap().collect::<Vec<_>>(),
        [audio.into(), midi.into()]
    );
}

#[test]
fn tracks_preserve_mixed_insertion_order_and_have_independent_mixers() {
    let mut project = Project::new();
    let drums = project.add_audio_track("Drums");
    let piano = project.add_midi_track(String::from("Piano"));
    let voice = project.add_audio_track("Voice");
    assert_eq!(project.tracks().len(), 4);
    assert!(matches!(project.tracks()[0], Track::Master(_)));
    assert!(matches!(project.tracks()[1], Track::Audio(_)));
    assert!(matches!(project.tracks()[2], Track::Midi(_)));
    assert!(matches!(project.tracks()[3], Track::Audio(_)));
    assert_eq!(
        project.tracks().iter().map(Track::name).collect::<Vec<_>>(),
        ["Master", "Drums", "Piano", "Voice"]
    );
    for track in &project.tracks()[1..] {
        assert_eq!(*track.mixer(), MixerSettings::default());
    }
    assert_eq!(project.audio_track(drums).unwrap().name(), "Drums");
    assert_eq!(project.midi_track(piano).unwrap().name(), "Piano");
    assert!(project.audio_track(drums).unwrap().clips().is_empty());
    assert!(project.midi_track(piano).unwrap().clips().is_empty());

    let drums_mixer = MixerSettings::new(0.5, -0.25, AudioOutput::Master).unwrap();
    let piano_mixer = MixerSettings::new(1.5, 1.0, AudioOutput::Disconnected).unwrap();
    project.set_audio_track_mixer(drums, drums_mixer).unwrap();
    project.set_midi_track_mixer(piano, piano_mixer).unwrap();
    assert_eq!(*project.audio_track(drums).unwrap().mixer(), drums_mixer);
    assert_eq!(*project.midi_track(piano).unwrap().mixer(), piano_mixer);
    assert_eq!(
        *project.audio_track(voice).unwrap().mixer(),
        MixerSettings::default()
    );
    assert_eq!(project.tracks()[2].mixer().gain(), 1.5);
    assert_eq!(project.tracks()[2].mixer().pan(), 1.0);
    assert_eq!(
        project.tracks()[2].mixer().output(),
        AudioOutput::Disconnected
    );
}

#[test]
fn shared_assets_support_independent_clips_and_audio_duration_ignores_midi() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25, 0.5]; 100]).unwrap());
    let first = project.add_audio_track("First");
    let midi = project.add_midi_track("Notes");
    let second = project.add_audio_track("Second");
    let clip = AudioClip {
        asset,
        source_start: 0,
        start_tick: 0,
        frame_count: 100,
        gain: 1.0,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    let later_clip = AudioClip {
        source_start: 20,
        start_tick: 150,
        frame_count: 50,
        gain: -0.5,
        ..clip.clone()
    };
    project.add_audio_clip(first, clip.clone()).unwrap();
    project.add_audio_clip(second, later_clip.clone()).unwrap();
    project
        .add_audio_clip(
            first,
            AudioClip {
                frame_count: 10,
                ..clip.clone()
            },
        )
        .unwrap();
    assert_eq!(project.audio_track(first).unwrap().clips()[0], clip);
    assert_eq!(project.audio_track(second).unwrap().clips(), &[later_clip]);
    assert_eq!(project.audio_duration_frames(), 200);
    project
        .set_audio_track_mixer(
            second,
            MixerSettings::new(0.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    project
        .add_midi_clip(
            midi,
            MidiClip {
                offset_ticks: 0,
                start_tick: u64::MAX - 1,
                length_ticks: 1,
                notes: Vec::new(),
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(project.audio_duration_frames(), 200);
}

#[test]
fn mixer_settings_reject_nonfinite_and_out_of_range_values() {
    for gain in [-0.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            MixerSettings::new(gain, 0.0, AudioOutput::Master),
            Err(ProjectError::InvalidTrackGain)
        );
    }
    for pan in [-1.01, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            MixerSettings::new(1.0, pan, AudioOutput::Master),
            Err(ProjectError::InvalidTrackPan)
        );
    }
    for gain in [0.0, 1.0, f32::MAX] {
        for pan in [-1.0, 0.0, 1.0] {
            let settings = MixerSettings::new(gain, pan, AudioOutput::Disconnected).unwrap();
            assert_eq!(settings.gain(), gain);
            assert_eq!(settings.pan(), pan);
        }
    }
}

#[test]
fn invalid_track_ids_do_not_mutate_tracks() {
    let mut source = Project::new();
    let audio_id = source.add_audio_track("Source audio");
    let midi_id = source.add_midi_track("Source MIDI");
    let mut target = Project::new();
    let target_midi = target.add_midi_track("Target MIDI");
    let asset = target.add_asset(AudioBuffer::new(vec![[0.25; 2]]).unwrap());
    let new_mixer = MixerSettings::new(0.2, 1.0, AudioOutput::Disconnected).unwrap();
    assert_eq!(
        target.audio_track(audio_id).unwrap_err(),
        ProjectError::UnknownAudioTrack { track: audio_id }
    );
    assert_eq!(
        target.set_audio_track_mixer(audio_id, new_mixer),
        Err(ProjectError::UnknownAudioTrack { track: audio_id })
    );
    assert_eq!(
        target.add_audio_clip(
            audio_id,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 1,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            }
        ),
        Err(ProjectError::UnknownAudioTrack { track: audio_id })
    );
    assert_eq!(
        target.midi_track(midi_id).unwrap_err(),
        ProjectError::UnknownMidiTrack { track: midi_id }
    );
    assert_eq!(
        target.set_midi_track_mixer(midi_id, new_mixer),
        Err(ProjectError::UnknownMidiTrack { track: midi_id })
    );
    assert_eq!(
        target.add_midi_clip(midi_id, midi_clip()),
        Err(ProjectError::UnknownMidiTrack { track: midi_id })
    );
    assert_eq!(target.tracks().len(), 2);
    assert!(target.midi_track(target_midi).unwrap().clips().is_empty());
    assert_eq!(*target.tracks()[1].mixer(), MixerSettings::default());
    assert_eq!(target.audio_duration_frames(), 0);
}

#[test]
fn midi_accepts_boundary_values_empty_patterns_and_unsorted_overlapping_notes() {
    let mut project = Project::new();
    let track = project.add_midi_track("Piano");
    let clip = MidiClip {
        offset_ticks: 0,
        start_tick: 0,
        length_ticks: TICKS_PER_QUARTER * 2,
        notes: vec![
            MidiNote {
                start_tick: TICKS_PER_QUARTER,
                key: 127,
                velocity: 1,
                channel: 15,
                ..note()
            },
            MidiNote {
                duration_ticks: TICKS_PER_QUARTER * 2,
                key: 0,
                velocity: 127,
                ..note()
            },
        ],
        name: None,
        pitch_bend: Vec::new(),
    };
    project.add_midi_clip(track, clip.clone()).unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: u64::MAX - 1,
                length_ticks: 1,
                notes: Vec::new(),
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    project
        .add_midi_clip(
            track,
            MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: u64::MAX,
                notes: vec![MidiNote {
                    duration_ticks: u64::MAX,
                    ..note()
                }],
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(project.midi_track(track).unwrap().clips()[0], clip);
    assert_eq!(project.midi_track(track).unwrap().clips().len(), 3);
    assert_eq!(project.audio_duration_frames(), 0);
}

#[test]
fn invalid_midi_clips_leave_existing_content_unchanged() {
    let mut project = Project::new();
    let track = project.add_midi_track("Piano");
    let valid = midi_clip();
    project.add_midi_clip(track, valid.clone()).unwrap();
    let cases = [
        (
            MidiClip {
                length_ticks: 0,
                ..valid.clone()
            },
            ProjectError::EmptyMidiClip,
        ),
        (
            MidiClip {
                start_tick: u64::MAX,
                ..valid.clone()
            },
            ProjectError::MidiTimelineRangeOverflow,
        ),
        (
            MidiClip {
                notes: vec![MidiNote { key: 128, ..note() }],
                ..valid.clone()
            },
            ProjectError::InvalidMidiKey { note: 0, key: 128 },
        ),
        (
            MidiClip {
                notes: vec![MidiNote {
                    velocity: 0,
                    ..note()
                }],
                ..valid.clone()
            },
            ProjectError::InvalidMidiVelocity {
                note: 0,
                velocity: 0,
            },
        ),
        (
            MidiClip {
                notes: vec![MidiNote {
                    velocity: 128,
                    ..note()
                }],
                ..valid.clone()
            },
            ProjectError::InvalidMidiVelocity {
                note: 0,
                velocity: 128,
            },
        ),
        (
            MidiClip {
                notes: vec![
                    note(),
                    MidiNote {
                        channel: 16,
                        ..note()
                    },
                ],
                ..valid.clone()
            },
            ProjectError::InvalidMidiChannel {
                note: 1,
                channel: 16,
            },
        ),
        (
            MidiClip {
                notes: vec![MidiNote {
                    duration_ticks: 0,
                    ..note()
                }],
                ..valid.clone()
            },
            ProjectError::EmptyMidiNote { note: 0 },
        ),
        (
            MidiClip {
                notes: vec![MidiNote {
                    start_tick: u64::MAX,
                    ..note()
                }],
                ..valid.clone()
            },
            ProjectError::MidiNoteRangeOverflow { note: 0 },
        ),
    ];
    for (invalid, expected) in cases {
        assert_eq!(project.add_midi_clip(track, invalid), Err(expected));
        assert_eq!(
            project.midi_track(track).unwrap().clips(),
            std::slice::from_ref(&valid)
        );
        assert_eq!(project.audio_duration_frames(), 0);
    }
}

#[test]
fn groups_collect_all_track_kinds_in_insertion_order_without_flattening() {
    let mut project = Project::new();
    let drums = project.add_audio_track("Drums");
    let band = project.add_group_track("Band");
    let piano = project.add_midi_track("Piano");
    let keys = project.add_group_track("Keys");
    let synth = project.add_midi_track("Synth");
    assert_eq!(project.group_track(band).unwrap().name(), "Band");
    assert_eq!(
        *project.group_track(band).unwrap().mixer(),
        MixerSettings::default()
    );
    assert_eq!(project.group_members(band).unwrap().count(), 0);
    assert_eq!(project.audio_duration_frames(), 0);
    assert!(matches!(project.tracks()[2], Track::Group(_)));
    assert_eq!(
        project.tracks().iter().map(Track::name).collect::<Vec<_>>(),
        ["Master", "Drums", "Band", "Piano", "Keys", "Synth"]
    );

    project.set_track_group(keys, Some(band)).unwrap();
    project.set_track_group(piano, Some(band)).unwrap();
    project.set_track_group(drums, Some(band)).unwrap();
    project.set_track_group(synth, Some(keys)).unwrap();
    assert_eq!(
        project.group_members(band).unwrap().collect::<Vec<_>>(),
        [
            TrackId::Audio(drums),
            TrackId::Midi(piano),
            TrackId::Group(keys)
        ]
    );
    assert_eq!(
        project.group_members(keys).unwrap().collect::<Vec<_>>(),
        [TrackId::Midi(synth)]
    );
    assert_eq!(project.tracks().len(), 6);
    assert_eq!(
        project.audio_track(drums).unwrap().mixer().output(),
        AudioOutput::Group(band)
    );
    assert_eq!(
        project.midi_track(piano).unwrap().mixer().output(),
        AudioOutput::Group(band)
    );
    assert_eq!(
        project.group_track(keys).unwrap().mixer().output(),
        AudioOutput::Group(band)
    );
}

#[test]
fn reparenting_and_ungrouping_preserve_gain_pan_and_use_a_single_route() {
    let mut project = Project::new();
    let track = project.add_audio_track("Voice");
    let first = project.add_group_track("First");
    let second = project.add_group_track("Second");
    let mixer = MixerSettings::new(0.4, -0.75, AudioOutput::Disconnected).unwrap();
    project.set_audio_track_mixer(track, mixer).unwrap();
    project.set_track_group(track, Some(first)).unwrap();
    project.set_track_group(track, Some(second)).unwrap();
    assert_eq!(project.group_members(first).unwrap().count(), 0);
    assert_eq!(
        project.group_members(second).unwrap().collect::<Vec<_>>(),
        [track.into()]
    );
    assert_eq!(
        *project.audio_track(track).unwrap().mixer(),
        mixer.with_output(AudioOutput::Group(second))
    );
    project
        .set_track_group(TrackId::Audio(track), None)
        .unwrap();
    assert_eq!(project.group_members(second).unwrap().count(), 0);
    assert_eq!(
        *project.audio_track(track).unwrap().mixer(),
        mixer.with_output(AudioOutput::Master)
    );

    project
        .set_audio_track_mixer(track, mixer.with_output(AudioOutput::Group(first)))
        .unwrap();
    assert_eq!(
        project.group_members(first).unwrap().collect::<Vec<_>>(),
        [track.into()]
    );
    project.set_audio_track_mixer(track, mixer).unwrap();
    assert_eq!(project.group_members(first).unwrap().count(), 0);
}

#[test]
fn group_mixer_is_independent_and_reparenting_preserves_its_settings() {
    let mut project = Project::new();
    let child = project.add_group_track("Child");
    let parent = project.add_group_track("Parent");
    let midi = project.add_midi_track("Instrument");
    let group_mixer = MixerSettings::new(0.6, 0.25, AudioOutput::Disconnected).unwrap();
    let midi_mixer = MixerSettings::new(0.8, -0.5, AudioOutput::Group(child)).unwrap();
    project.set_group_track_mixer(child, group_mixer).unwrap();
    project.set_midi_track_mixer(midi, midi_mixer).unwrap();
    project.set_track_group(child, Some(parent)).unwrap();
    assert_eq!(
        *project.group_track(child).unwrap().mixer(),
        group_mixer.with_output(AudioOutput::Group(parent))
    );
    assert_eq!(
        *project.group_track(parent).unwrap().mixer(),
        MixerSettings::default()
    );
    assert_eq!(*project.midi_track(midi).unwrap().mixer(), midi_mixer);
    project.set_track_group(child, None).unwrap();
    assert_eq!(
        *project.group_track(child).unwrap().mixer(),
        group_mixer.with_output(AudioOutput::Master)
    );
    assert_eq!(
        project.group_members(child).unwrap().collect::<Vec<_>>(),
        [midi.into()]
    );
}

#[test]
fn invalid_group_sources_and_destinations_leave_mixers_unchanged() {
    let mut foreign = Project::new();
    let wrong_kind = foreign.add_group_track("Foreign at index one");
    foreign.add_group_track("Foreign at index two");
    foreign.add_group_track("Foreign at index three");
    let absent = foreign.add_group_track("Foreign at index four");
    let mut project = Project::new();
    let audio = project.add_audio_track("Audio");
    let midi = project.add_midi_track("MIDI");
    let group = project.add_group_track("Group");
    let original = MixerSettings::new(0.3, 0.5, AudioOutput::Master).unwrap();
    project.set_audio_track_mixer(audio, original).unwrap();
    project.set_midi_track_mixer(midi, original).unwrap();
    project.set_group_track_mixer(group, original).unwrap();
    for invalid in [wrong_kind, absent] {
        let error = ProjectError::UnknownGroupTrack { track: invalid };
        assert_eq!(project.group_track(invalid).unwrap_err(), error);
        assert!(matches!(project.group_members(invalid), Err(ref actual) if *actual == error));
        let mixer = MixerSettings::new(0.9, -1.0, AudioOutput::Group(invalid)).unwrap();
        assert_eq!(
            project.set_audio_track_mixer(audio, mixer),
            Err(error.clone())
        );
        assert_eq!(
            project.set_midi_track_mixer(midi, mixer),
            Err(error.clone())
        );
        assert_eq!(
            project.set_group_track_mixer(group, mixer),
            Err(error.clone())
        );
        assert_eq!(
            project.set_group_track_mixer(invalid, original),
            Err(error.clone())
        );
        assert_eq!(
            project.set_track_group(audio, Some(invalid)),
            Err(error.clone())
        );
        assert_eq!(project.set_track_group(invalid, Some(group)), Err(error));
        for track in &project.tracks()[1..] {
            assert_eq!(*track.mixer(), original);
        }
        assert_eq!(project.group_members(group).unwrap().count(), 0);
    }
}

#[test]
fn self_routes_and_indirect_group_cycles_are_rejected_before_any_change() {
    let mut project = Project::new();
    let child = project.add_group_track("Child");
    let middle = project.add_group_track("Middle");
    let root = project.add_group_track("Root");
    project.set_track_group(child, Some(middle)).unwrap();
    project.set_track_group(middle, Some(root)).unwrap();
    let original: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| *track.mixer())
        .collect();
    for (track, destination) in [
        (child, child),
        (middle, child),
        (root, child),
        (root, middle),
    ] {
        let error = ProjectError::RoutingCycle { track, destination };
        assert_eq!(
            project.set_track_group(track, Some(destination)),
            Err(error.clone())
        );
        let mixer = MixerSettings::new(0.2, -0.5, AudioOutput::Group(destination)).unwrap();
        assert_eq!(project.set_group_track_mixer(track, mixer), Err(error));
        assert_eq!(
            project
                .tracks()
                .iter()
                .map(|track| *track.mixer())
                .collect::<Vec<_>>(),
            original
        );
    }
    project.set_track_group(middle, None).unwrap();
    project.set_track_group(root, Some(child)).unwrap();
    assert_eq!(
        project.group_members(child).unwrap().collect::<Vec<_>>(),
        [root.into()]
    );
}

#[test]
fn tempo_edits_preserve_ticks_and_audio_source_length_and_recompute_duration() {
    let mut project = Project::new();
    assert_eq!(project.bpm(), 120.0);
    assert_eq!(project.time_signature(), TimeSignature::default());
    assert_eq!(project.snap_grid(), SnapGrid::Off);
    let track = project.add_audio_track("Voice");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 100]).unwrap());
    let clip = AudioClip {
        asset,
        source_start: 10,
        start_tick: 960,
        frame_count: 50,
        gain: 0.7,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    project.add_audio_clip(track, clip.clone()).unwrap();
    assert_eq!(project.audio_duration_frames(), 24_050);
    project.set_bpm(60.0).unwrap();
    assert_eq!(project.audio_track(track).unwrap().clips(), &[clip]);
    assert_eq!(project.audio_duration_frames(), 48_050);
    project.set_time_signature(TimeSignature::new(3, 4).unwrap());
    assert_eq!(project.audio_duration_frames(), 48_050);
    project.set_bpm(240.0).unwrap();
    assert_eq!(project.audio_duration_frames(), 12_050);
}

#[test]
fn invalid_tempo_or_unrepresentable_audio_time_leaves_project_unchanged() {
    let mut project = Project::new();
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]]).unwrap());
    let track = project.add_audio_track("Voice");
    let clip = AudioClip {
        asset,
        source_start: 0,
        start_tick: 960,
        frame_count: 1,
        gain: 1.0,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    project.add_audio_clip(track, clip.clone()).unwrap();
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(0.0, 0.0, AudioOutput::Disconnected).unwrap(),
        )
        .unwrap();
    for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            project.set_bpm(invalid),
            Err(ProjectError::Time(TimeError::InvalidTempo))
        );
    }
    assert_eq!(
        project.set_bpm(1e-300),
        Err(ProjectError::Time(TimeError::ConversionOverflow))
    );
    assert_eq!(project.bpm(), 120.0);
    assert_eq!(
        project.audio_track(track).unwrap().clips(),
        std::slice::from_ref(&clip)
    );
    assert_eq!(project.audio_duration_frames(), 24_001);
    assert_eq!(
        project.add_audio_clip(
            track,
            AudioClip {
                start_tick: u64::MAX,
                ..clip.clone()
            }
        ),
        Err(ProjectError::Time(TimeError::ConversionOverflow))
    );
    assert_eq!(project.audio_track(track).unwrap().clips(), &[clip]);
}

#[test]
fn insertion_snaps_both_clip_kinds_without_quantizing_lengths_or_notes() {
    let mut project = Project::new();
    project.set_snap_grid(SnapGrid::Sixteenth);
    let audio = project.add_audio_track("Voice");
    let midi = project.add_midi_track("Piano");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 80]).unwrap());
    let clip = AudioClip {
        asset,
        source_start: 7,
        start_tick: 120,
        frame_count: 51,
        gain: 0.5,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    let pattern = MidiClip {
        offset_ticks: 0,
        start_tick: 120,
        length_ticks: 1199,
        notes: vec![MidiNote {
            start_tick: 13,
            duration_ticks: 111,
            ..note()
        }],
        name: None,
        pitch_bend: Vec::new(),
    };
    project.add_audio_clip(audio, clip.clone()).unwrap();
    project.add_midi_clip(midi, pattern.clone()).unwrap();
    assert_eq!(
        project.audio_track(audio).unwrap().clips(),
        &[AudioClip {
            start_tick: 240,
            ..clip.clone()
        }]
    );
    assert_eq!(
        project.midi_track(midi).unwrap().clips(),
        &[MidiClip {
            start_tick: 240,
            ..pattern.clone()
        }]
    );
    assert_eq!(project.audio_duration_frames(), 6051);

    project.set_snap_grid(SnapGrid::Off);
    project.add_audio_clip(audio, clip.clone()).unwrap();
    project.add_midi_clip(midi, pattern.clone()).unwrap();
    assert_eq!(project.audio_track(audio).unwrap().clips()[1], clip);
    assert_eq!(project.midi_track(midi).unwrap().clips()[1], pattern);
    assert_eq!(
        project.audio_track(audio).unwrap().clips()[0].start_tick,
        240
    );
}

#[test]
fn moving_clips_preserves_content_and_updates_shortened_project_duration() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Voice");
    let midi = project.add_midi_track("Piano");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 10]).unwrap());
    let clip = AudioClip {
        asset,
        source_start: 2,
        start_tick: 3840,
        frame_count: 5,
        gain: -0.5,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    let pattern = midi_clip();
    project.add_audio_clip(audio, clip.clone()).unwrap();
    project.add_midi_clip(midi, pattern.clone()).unwrap();
    assert_eq!(project.audio_duration_frames(), 96_005);
    project.set_snap_grid(SnapGrid::Quarter);
    project.move_audio_clip(audio, 0, 480).unwrap();
    project.move_midi_clip(midi, 0, 480).unwrap();
    assert_eq!(
        project.audio_track(audio).unwrap().clips(),
        &[AudioClip {
            start_tick: 960,
            ..clip
        }]
    );
    assert_eq!(
        project.midi_track(midi).unwrap().clips(),
        &[MidiClip {
            start_tick: 960,
            ..pattern
        }]
    );
    assert_eq!(project.audio_duration_frames(), 24_005);
}

#[test]
fn bar_snapping_uses_current_meter_only_for_subsequent_edits() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Piano");
    project.set_snap_grid(SnapGrid::Bar);
    project
        .add_midi_clip(
            midi,
            MidiClip {
                start_tick: 1600,
                ..midi_clip()
            },
        )
        .unwrap();
    assert_eq!(project.midi_track(midi).unwrap().clips()[0].start_tick, 0);
    project.set_time_signature(TimeSignature::new(3, 4).unwrap());
    assert_eq!(project.midi_track(midi).unwrap().clips()[0].start_tick, 0);
    project.move_midi_clip(midi, 0, 1600).unwrap();
    assert_eq!(
        project.midi_track(midi).unwrap().clips()[0].start_tick,
        2880
    );
    assert_eq!(project.time_signature().position_at(2880).bar, 2);
    project.set_time_signature(TimeSignature::new(6, 8).unwrap());
    assert_eq!(project.snap_tick(1600), Ok(2880));
    assert_eq!(project.time_signature().ticks_per_beat(), 480);
}

#[test]
fn audio_end_trim_uses_snapped_absolute_boundary_and_source_limits() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Voice");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 72_010]).unwrap());
    let clip = AudioClip {
        asset,
        source_start: 10,
        start_tick: 960,
        frame_count: 72_000,
        gain: 0.5,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    project.add_audio_clip(audio, clip.clone()).unwrap();
    project.set_snap_grid(SnapGrid::Quarter);
    project.trim_audio_clip_end(audio, 0, 2100).unwrap();
    let trimmed = AudioClip {
        frame_count: 24_000,
        ..clip.clone()
    };
    assert_eq!(
        project.audio_track(audio).unwrap().clips(),
        std::slice::from_ref(&trimmed)
    );
    assert_eq!(project.audio_duration_frames(), 48_000);
    for end in [0, 960] {
        assert_eq!(
            project.trim_audio_clip_end(audio, 0, end),
            Err(ProjectError::EmptyClip)
        );
        assert_eq!(
            project.audio_track(audio).unwrap().clips(),
            std::slice::from_ref(&trimmed)
        );
    }
    assert!(matches!(
        project.trim_audio_clip_end(audio, 0, 4800),
        Err(ProjectError::SourceRangeOutOfBounds { .. })
    ));
    assert_eq!(project.audio_track(audio).unwrap().clips(), &[trimmed]);
    project.trim_audio_clip_end(audio, 0, 3840).unwrap();
    assert_eq!(project.audio_track(audio).unwrap().clips(), &[clip]);
}

#[test]
fn audio_end_trim_converts_endpoints_instead_of_rounding_a_duration() {
    let mut project = Project::new();
    project.set_bpm(137.0).unwrap();
    let audio = project.add_audio_track("Voice");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]; 100]).unwrap());
    project
        .add_audio_clip(
            audio,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 4,
                frame_count: 100,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    project.trim_audio_clip_end(audio, 0, 5).unwrap();
    assert_eq!(
        project.audio_track(audio).unwrap().clips()[0].frame_count,
        21
    );
    assert_eq!(project.audio_duration_frames(), 109);
    assert_eq!(project.tempo().ticks_to_frames(1), Ok(22));
}

#[test]
fn midi_end_trim_snaps_without_discarding_notes() {
    let mut project = Project::new();
    let midi = project.add_midi_track("Piano");
    let clip = MidiClip {
        start_tick: 960,
        ..midi_clip()
    };
    project.add_midi_clip(midi, clip.clone()).unwrap();
    project.set_snap_grid(SnapGrid::Quarter);
    project.trim_midi_clip_end(midi, 0, 2100).unwrap();
    let trimmed = MidiClip {
        length_ticks: 960,
        ..clip
    };
    assert_eq!(
        project.midi_track(midi).unwrap().clips(),
        std::slice::from_ref(&trimmed)
    );
    for end in [0, 960] {
        assert_eq!(
            project.trim_midi_clip_end(midi, 0, end),
            Err(ProjectError::EmptyMidiClip)
        );
    }
    project.set_snap_grid(SnapGrid::Eighth);
    project.trim_midi_clip_end(midi, 0, 1440).unwrap();
    let shortened = &project.midi_track(midi).unwrap().clips()[0];
    assert_eq!(shortened.length_ticks, 480);
    assert_eq!(shortened.notes, trimmed.notes);
}

#[test]
fn invalid_edits_and_snap_overflow_leave_existing_clips_unchanged() {
    let mut project = Project::new();
    let audio = project.add_audio_track("Voice");
    let midi = project.add_midi_track("Piano");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.25; 2]]).unwrap());
    let clip = AudioClip {
        asset,
        source_start: 0,
        start_tick: 0,
        frame_count: 1,
        gain: 1.0,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    };
    let pattern = midi_clip();
    project.add_audio_clip(audio, clip.clone()).unwrap();
    project.add_midi_clip(midi, pattern.clone()).unwrap();
    let missing_audio = Err(ProjectError::UnknownAudioClip {
        track: audio,
        index: 1,
    });
    assert_eq!(project.move_audio_clip(audio, 1, 960), missing_audio);
    assert_eq!(project.trim_audio_clip_end(audio, 1, 960), missing_audio);
    let missing_midi = Err(ProjectError::UnknownMidiClip {
        track: midi,
        index: 1,
    });
    assert_eq!(project.move_midi_clip(midi, 1, 960), missing_midi);
    assert_eq!(project.trim_midi_clip_end(midi, 1, 960), missing_midi);
    assert_eq!(
        project.move_audio_clip(audio, 0, u64::MAX),
        Err(ProjectError::Time(TimeError::ConversionOverflow))
    );
    assert_eq!(
        project.move_midi_clip(midi, 0, u64::MAX),
        Err(ProjectError::MidiTimelineRangeOverflow)
    );
    project.set_time_signature(TimeSignature::new(14, 4).unwrap());
    project.set_snap_grid(SnapGrid::Bar);
    let overflow = Err(ProjectError::Time(TimeError::ConversionOverflow));
    assert_eq!(project.move_audio_clip(audio, 0, u64::MAX), overflow);
    assert_eq!(project.move_midi_clip(midi, 0, u64::MAX), overflow);
    assert_eq!(
        project.add_audio_clip(
            audio,
            AudioClip {
                start_tick: u64::MAX,
                ..clip.clone()
            }
        ),
        overflow
    );
    assert_eq!(
        project.add_midi_clip(
            midi,
            MidiClip {
                start_tick: u64::MAX,
                ..pattern.clone()
            }
        ),
        overflow
    );
    assert_eq!(project.audio_track(audio).unwrap().clips(), &[clip]);
    assert_eq!(project.midi_track(midi).unwrap().clips(), &[pattern]);
    assert_eq!(project.audio_duration_frames(), 1);
}
