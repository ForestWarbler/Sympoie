// TEMP: generates a demo project with a Rain track holding a chord (delete after review).
use std::path::PathBuf;

use sympoie::builtin::BuiltinKind;
use sympoie::clip::{MidiClip, MidiNote};
use sympoie::project::Project;
use sympoie::project_json::save_project_folder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/rain-demo"));
    let mut project = Project::new();
    project.set_bpm(100.0)?;
    let track = project.add_midi_track("Rain");
    project.set_plugin_instrument(track, Some(BuiltinKind::Rain.instance()))?;
    let bars = 64u64;
    let bar = 4 * 480u64;
    let mut notes = Vec::new();
    for (i, key) in [48u8, 55, 60, 64, 67, 71, 74].iter().enumerate() {
        notes.push(MidiNote {
            start_tick: 0,
            duration_ticks: bars * bar,
            key: *key,
            velocity: 80 + i as u8 * 4,
            channel: 0,
            slide_ticks: 0,
        });
    }
    project.add_midi_clip(
        track,
        MidiClip {
            start_tick: 0,
            length_ticks: bars * bar,
            offset_ticks: 0,
            notes,
            name: Some("Chord".to_owned()),
            pitch_bend: Vec::new(),
        },
    )?;
    save_project_folder(&project, &directory)?;
    println!("wrote {}", directory.display());
    Ok(())
}
