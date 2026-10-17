use sympoie::Frame;
use sympoie::clip::AudioClip;
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::project_file::{read_project, write_project};
use sympoie::render::render_block;

fn samples() -> Vec<Frame> {
    vec![
        [0.1, -0.9],
        [0.2, -0.8],
        [0.3, -0.7],
        [0.4, -0.6],
        [0.5, -0.5],
        [0.6, -0.4],
        [0.7, -0.3],
        [0.8, -0.2],
    ]
}

#[test]
fn reversing_a_trimmed_clip_preserves_hidden_audio_shared_clips_and_snapshots() {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let original = samples();
    let asset = project.add_asset(AudioBuffer::new(original.clone()).unwrap());
    let track = project.add_audio_track("Audio");
    let clip = AudioClip {
        asset,
        source_start: 2,
        start_tick: 5,
        frame_count: 3,
        gain: 0.5,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
    };
    project.add_audio_clip(track, clip).unwrap();
    let shared = AudioClip {
        start_tick: 20,
        ..clip
    };
    project.add_audio_clip(track, shared).unwrap();
    let before = project.clone();

    project.reverse_audio_clip(track, 0).unwrap();

    let reversed = project.audio_track(track).unwrap().clips()[0];
    assert_ne!(reversed.asset, asset);
    assert_eq!(AudioClip { asset, ..reversed }, clip);
    let expected = vec![
        original[0],
        original[1],
        original[4],
        original[3],
        original[2],
        original[5],
        original[6],
        original[7],
    ];
    assert_eq!(
        project.audio_asset(reversed.asset).unwrap().samples(),
        expected
    );
    assert_eq!(project.audio_asset(asset).unwrap().samples(), original);
    assert_eq!(project.audio_track(track).unwrap().clips()[1], shared);
    assert_eq!(before.audio_track(track).unwrap().clips()[0], clip);
    assert_eq!(before.audio_asset(asset).unwrap().samples(), original);

    let mut rendered = [[0.0; 2]; 5];
    render_block(&project, 4, &mut rendered).unwrap();
    assert_eq!(rendered[0], [0.0; 2]);
    for (actual, frame) in rendered[1..4].iter().zip(original[2..5].iter().rev()) {
        assert_eq!(*actual, [frame[0] * 0.5, frame[1] * 0.5]);
    }
    assert_eq!(rendered[4], [0.0; 2]);

    project.trim_audio_clip_end(track, 0, 11).unwrap();
    let mut extended = [[0.0; 2]; 6];
    render_block(&project, 5, &mut extended).unwrap();
    for (actual, frame) in extended.iter().zip(&expected[2..]) {
        assert_eq!(*actual, [frame[0] * 0.5, frame[1] * 0.5]);
    }
}

#[test]
fn reversing_twice_restores_source_and_reversed_audio_survives_save_load() {
    let mut project = Project::new();
    let original = samples();
    let asset = project.add_asset(AudioBuffer::new(original.clone()).unwrap());
    let track = project.add_audio_track("Audio");
    let clip = AudioClip {
        asset,
        source_start: 1,
        start_tick: 0,
        frame_count: 6,
        gain: 1.0,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
    };
    project.add_audio_clip(track, clip).unwrap();
    project.reverse_audio_clip(track, 0).unwrap();

    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    let mut loaded = read_project(bytes.as_slice()).unwrap();
    let reversed = loaded.audio_track(track).unwrap().clips()[0];
    assert_eq!(
        loaded.audio_asset(reversed.asset).unwrap().samples(),
        project.audio_asset(reversed.asset).unwrap().samples()
    );

    loaded.reverse_audio_clip(track, 0).unwrap();
    let restored = loaded.audio_track(track).unwrap().clips()[0];
    assert_eq!(
        loaded.audio_asset(restored.asset).unwrap().samples(),
        original
    );
    assert_eq!(AudioClip { asset, ..restored }, clip);
}

#[test]
fn reversing_a_missing_clip_leaves_the_project_unchanged() {
    let mut project = Project::new();
    let track = project.add_audio_track("Audio");
    let mut before = Vec::new();
    write_project(&project, &mut before).unwrap();

    assert_eq!(
        project.reverse_audio_clip(track, 0),
        Err(ProjectError::UnknownAudioClip { track, index: 0 })
    );

    let mut after = Vec::new();
    write_project(&project, &mut after).unwrap();
    assert_eq!(after, before);
}
