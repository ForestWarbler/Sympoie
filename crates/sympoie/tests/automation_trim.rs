use sympoie::Frame;
use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::clip::AudioClip;
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_file::{read_project, write_project};
use sympoie::project_json::{load_project_folder, save_project_folder};
use sympoie::render::Renderer;
use sympoie::track::{AudioOutput, AudioTrackId, MixerSettings};

fn flat(target: AutomationTarget, value: f64) -> AutomationLane {
    AutomationLane::new(
        target,
        vec![
            AutomationPoint { tick: 0, value },
            AutomationPoint {
                tick: 10_000,
                value,
            },
        ],
    )
    .unwrap()
}

/// A track playing a constant 0.5 with a flat volume lane at 0.25 (−6 dB),
/// written while the fader sat at unity.
fn automated() -> (Project, AudioTrackId) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_audio_track("Pump");
    let asset = project.add_asset(AudioBuffer::new(vec![[0.5; 2]; 2000]).unwrap());
    project
        .add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: 0,
                frame_count: 2000,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: None,
            },
        )
        .unwrap();
    project
        .set_track_automation(track, flat(AutomationTarget::TrackGain, 0.25))
        .unwrap();
    (project, track)
}

fn set_fader(project: &mut Project, track: AudioTrackId, gain: f32) {
    project
        .set_audio_track_mixer(
            track,
            MixerSettings::new(gain, 0.0, AudioOutput::Master).unwrap(),
        )
        .unwrap();
}

fn level(project: &Project) -> f32 {
    let mut renderer = Renderer::new(project).unwrap();
    let mut frames: Vec<Frame> = vec![[0.0; 2]; 1000];
    for (index, chunk) in frames.chunks_mut(113).enumerate() {
        renderer.render_block((index * 113) as u64, chunk);
    }
    frames[999][0]
}

#[test]
fn volume_trim_scales_the_lane_silences_at_minus_infinity_and_caps_at_plus_six() {
    let (mut project, track) = automated();
    assert_eq!(level(&project), 0.25);
    set_fader(&mut project, track, 0.5);
    assert_eq!(level(&project), 0.125);
    set_fader(&mut project, track, 0.0);
    assert_eq!(level(&project), 0.0);
    set_fader(&mut project, track, 1.0);
    assert_eq!(level(&project), 0.25, "the shape survives the fader at −∞");
    set_fader(&mut project, track, 4.0);
    assert_eq!(level(&project), 1.0, "capped at +6 dB");
    assert_eq!(project.tracks()[1].automation()[0].points()[0].value, 0.25);
}

#[test]
fn other_targets_shift_and_cap_to_their_range() {
    let target = AutomationTarget::PluginParameter {
        plugin: 1,
        parameter: 2,
    };
    assert!((target.trim(0.6, 0.5, 0.4) - 0.7).abs() < 1e-12);
    assert_eq!(target.trim(0.9, 0.8, 0.4), 1.0);
    assert_eq!(target.trim(0.2, 0.0, 0.4), 0.0);
    assert!((target.base_for(0.7, 0.6, 0.4) - 0.5).abs() < 1e-12);
    let gain = AutomationTarget::TrackGain;
    assert_eq!(gain.trim(0.4, 0.25, 0.5), 0.2);
    assert_eq!(gain.base_for(0.2, 0.4, 0.5), 0.25);
}

#[test]
fn redrawing_a_lane_keeps_its_trim() {
    let (mut project, track) = automated();
    set_fader(&mut project, track, 0.5);
    project
        .set_track_automation(track, flat(AutomationTarget::TrackGain, 0.25))
        .unwrap();
    assert_eq!(project.tracks()[1].automation()[0].anchor(), Some(0.5));
    assert_eq!(level(&project), 0.125);
}

#[test]
fn trims_survive_both_file_formats_and_untrimmed_json_lanes_have_no_anchor() {
    let (mut project, track) = automated();
    let folder = std::env::temp_dir().join(format!("sympoie-trim-{}.symp", std::process::id()));
    save_project_folder(&project, &folder).unwrap();
    let text = std::fs::read_dir(folder.join("tracks"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(!text.contains("anchor"));

    set_fader(&mut project, track, 0.5);
    save_project_folder(&project, &folder).unwrap();
    let loaded = load_project_folder(&folder).unwrap();
    std::fs::remove_dir_all(&folder).unwrap();
    assert_eq!(level(&loaded), 0.125);

    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    let loaded = read_project(&mut bytes.as_slice()).unwrap();
    assert_eq!(level(&loaded), 0.125);
}
