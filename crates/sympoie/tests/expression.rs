//! Per-note slides and clip pitch bend: storage, scheduling, seeking and export.

use std::sync::{Arc, Mutex};

use sympoie::Frame;
use sympoie::clip::{BendPoint, MidiClip, MidiNote, bend_value_at, thin_bend};
use sympoie::clip_export::write_midi_clip;
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginProcessContext, PluginProcessor,
};
use sympoie::project::Project;
use sympoie::project_file::{read_project, write_project};
use sympoie::project_json::{load_project_folder, save_project_folder};
use sympoie::render::Renderer;
use sympoie::time::TimeSignature;

type Log = Arc<Mutex<Vec<(u64, NoteEvent)>>>;

/// Records every event with the frame it arrived at.
#[derive(Debug)]
struct Recorder {
    log: Log,
    frame: u64,
}

impl PluginProcessor for Recorder {
    fn reset(&mut self) {}

    fn event(&mut self, event: NoteEvent) {
        self.log.lock().unwrap().push((self.frame, event));
    }

    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        self.frame = context.frame + frames.len() as u64;
        frames.fill([0.0; 2]);
    }

    fn set_parameter(&mut self, _: u32, _: f64) {}
}

struct Host(Log);

impl PluginHost for Host {
    fn load(&self, _: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        Ok(Box::new(Recorder {
            log: Arc::clone(&self.0),
            frame: 0,
        }))
    }
}

fn instrument() -> PluginInstance {
    PluginInstance::new(
        PluginDescriptor {
            path: "/fake/Recorder.vst3".into(),
            class_id: "recorder".to_owned(),
            name: "Recorder".to_owned(),
            vendor: "Test".to_owned(),
            kind: PluginKind::Instrument,
        },
        Vec::new(),
        Vec::new(),
    )
}

fn clip(project: &Project) -> MidiClip {
    match &project.tracks()[1] {
        sympoie::track::Track::Midi(track) => track.clips()[0].clone(),
        _ => panic!("expected the MIDI track"),
    }
}

/// At 120 bpm a tick is 25 frames.
fn project() -> Project {
    project_with(vec![
        BendPoint {
            tick: 960,
            value: 0,
        },
        BendPoint {
            tick: 1920,
            value: 8191,
        },
        BendPoint {
            tick: 2880,
            value: -8192,
        },
    ])
}

fn project_with(bend: Vec<BendPoint>) -> Project {
    let mut project = Project::new();
    let track = project.add_midi_track("Expressive");
    project
        .set_plugin_instrument(track, Some(instrument()))
        .unwrap();
    let mut clip = MidiClip::new(960, 3840);
    clip.notes = vec![
        MidiNote::new(0, 960, 48, 100),
        MidiNote {
            slide_ticks: 240,
            ..MidiNote::new(960, 960, 55, 90)
        },
    ];
    clip.pitch_bend = bend;
    project.add_midi_clip(track, clip).unwrap();
    project
}

/// Renders `from..to` in one pass after seeking to `from`, and returns the events seen.
fn events(project: &Project, from: u64, to: u64) -> Vec<(u64, NoteEvent)> {
    let log = Log::default();
    let host = Host(Arc::clone(&log));
    let mut renderer = Renderer::with_plugin_host(project, &host).unwrap();
    let mut block = [[0.0; 2]; 256];
    let mut frame = from;
    while frame < to {
        let count = (to - frame).min(256) as usize;
        renderer.render_block(frame, &mut block[..count]);
        frame += count as u64;
    }
    log.lock().unwrap().clone()
}

#[test]
fn slides_and_bends_are_scheduled_sample_accurately() {
    let project = project();
    let all = events(&project, 0, 25 * 6000);
    let slide = all.iter().find_map(|(frame, event)| match event {
        NoteEvent::On {
            key: 55,
            slide_frames,
            ..
        } => Some((*frame, *slide_frames)),
        _ => None,
    });
    // The second note starts at tick 960 + 960 and slides for 240 ticks.
    assert_eq!(slide, Some((25 * 1920, 25 * 240)));
    let bends: Vec<_> = all
        .iter()
        .filter_map(|(frame, event)| match event {
            NoteEvent::PitchBend {
                value, ramp_frames, ..
            } => Some((*frame, *value, *ramp_frames)),
            _ => None,
        })
        .collect();
    assert_eq!(
        bends,
        vec![
            // Starting playback sends the bend in effect (none yet)…
            (0, 0.0, 0),
            // …then up to full bend between the first two points…
            (25 * 1920, 1.0, 25 * 960),
            // …down through zero to full negative…
            (25 * 2880, -1.0, 25 * 960),
            // …held to the clip's end, where the bend returns to centre.
            (25 * 4800, 0.0, 0),
        ]
    );
}

#[test]
fn events_on_one_frame_end_notes_then_bend_then_start_notes() {
    // Flat until the second note, then a ramp that starts on its first frame.
    let project = project_with(vec![
        BendPoint { tick: 0, value: 0 },
        BendPoint {
            tick: 960,
            value: 0,
        },
        BendPoint {
            tick: 1000,
            value: 4000,
        },
    ]);
    let all = events(&project, 0, 25 * 2000);
    let at: Vec<_> = all
        .iter()
        .filter(|(frame, _)| *frame == 25 * 1920)
        .map(|(_, event)| match event {
            NoteEvent::Off { .. } => "off",
            NoteEvent::PitchBend { .. } => "bend",
            NoteEvent::On { .. } => "on",
        })
        .collect();
    assert_eq!(at, ["off", "bend", "on"]);
}

#[test]
fn seeking_into_a_ramp_chases_its_value_and_the_rest_of_the_ramp() {
    let project = project();
    // Halfway up the first ramp.
    let from = 25 * (960 + 960 + 480);
    let all = events(&project, from, from + 256);
    let chased: Vec<_> = all
        .iter()
        .take_while(|(frame, _)| *frame == 0)
        .filter_map(|(_, event)| match event {
            NoteEvent::PitchBend {
                value, ramp_frames, ..
            } => Some((*value, *ramp_frames)),
            _ => None,
        })
        .collect();
    assert_eq!(chased.len(), 2);
    assert!((chased[0].0 - 0.5).abs() < 1.0e-3, "{chased:?}");
    assert_eq!(chased[0].1, 0);
    assert_eq!(chased[1], (1.0, 25 * 480));
    // The chased note starts at its pitch: its slide is already over.
    assert!(all.iter().any(|(_, event)| matches!(
        event,
        NoteEvent::On {
            key: 55,
            slide_frames: 0,
            ..
        }
    )));
}

#[test]
fn slides_and_bends_survive_both_project_formats() {
    let project = project();
    let mut bytes = Vec::new();
    write_project(&project, &mut bytes).unwrap();
    let restored = read_project(bytes.as_slice()).unwrap();
    assert_eq!(clip(&restored), clip(&project));

    let folder =
        std::env::temp_dir().join(format!("sympoie-expression-{}.symp", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    save_project_folder(&project, &folder).unwrap();
    let text = std::fs::read_dir(folder.join("tracks"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(text.contains("\"slide\": 240"), "{text}");
    assert!(text.contains("\"bend\""), "{text}");
    let loaded = load_project_folder(&folder).unwrap();
    std::fs::remove_dir_all(&folder).unwrap();
    assert_eq!(clip(&loaded), clip(&project));
}

#[test]
fn unordered_bend_points_are_rejected() {
    let mut project = Project::new();
    let track = project.add_midi_track("x");
    let mut clip = MidiClip::new(0, 960);
    clip.pitch_bend = vec![
        BendPoint { tick: 10, value: 0 },
        BendPoint { tick: 10, value: 1 },
    ];
    assert!(project.add_midi_clip(track, clip).is_err());
}

#[test]
fn trimmed_clips_start_with_the_bend_they_cut_into() {
    let mut clip = MidiClip::new(0, 1000);
    clip.offset_ticks = 500;
    clip.pitch_bend = vec![
        BendPoint { tick: 0, value: 0 },
        BendPoint {
            tick: 1000,
            value: 8000,
        },
    ];
    assert_eq!(
        clip.audible_bend(),
        vec![
            BendPoint {
                tick: 0,
                value: 4000
            },
            BendPoint {
                tick: 500,
                value: 8000
            }
        ]
    );
    assert_eq!(bend_value_at(&clip.pitch_bend, 250), 2000);
    assert_eq!(bend_value_at(&clip.pitch_bend, 5000), 8000);
}

#[test]
fn thinning_keeps_corners_and_drops_points_on_straight_lines() {
    let ramp: Vec<_> = (0..=100)
        .map(|tick| BendPoint {
            tick,
            value: (tick * 40) as i16,
        })
        .chain((101..=200).map(|tick| BendPoint { tick, value: 4000 }))
        .collect();
    assert_eq!(
        thin_bend(&ramp, 16.0),
        vec![
            BendPoint { tick: 0, value: 0 },
            BendPoint {
                tick: 100,
                value: 4000
            },
            BendPoint {
                tick: 200,
                value: 4000
            }
        ]
    );
}

#[test]
fn exported_midi_files_carry_the_bend() {
    let project = project();
    let clip = clip(&project);
    let mut file = Vec::new();
    write_midi_clip(
        &clip,
        "x",
        project.tempo(),
        TimeSignature::new(4, 4).unwrap(),
        &mut file,
    )
    .unwrap();
    let bends = file.windows(3).filter(|bytes| bytes[0] == 0xe0).count();
    assert!(
        bends > 50,
        "a ramp becomes many bend messages, found {bends}"
    );
    // The file ends the bend back at centre (0x2000 = 0, 64).
    let last = file
        .windows(3)
        .rev()
        .find(|bytes| bytes[0] == 0xe0)
        .unwrap();
    assert_eq!(last, [0xe0, 0, 64]);
}
