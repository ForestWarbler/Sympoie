//! Writes a demo project with Ferro playing an acid line that uses per-note slides, a
//! pitch-bend swoop and a pad, so both engines and every expression path can be heard.
//!
//! `cargo run -p sympoie --example make_ferro_demo -- /tmp/ferro-demo.symp`
use std::path::PathBuf;

use sympoie::TICKS_PER_QUARTER;
use sympoie::builtin::{self, BuiltinKind};
use sympoie::clip::{BendPoint, MidiClip, MidiNote};
use sympoie::project::Project;
use sympoie::project_json::save_project_folder;

fn ferro(preset: usize) -> sympoie::plugin::PluginInstance {
    let mut instance = BuiltinKind::Ferro.instance();
    for (id, value) in builtin::preset_parameters(BuiltinKind::Ferro, preset).unwrap() {
        if let Some(parameter) = instance.parameters.iter_mut().find(|p| p.id == id) {
            parameter.value = value;
        }
    }
    instance
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/ferro-demo.symp"));
    let sixteenth = TICKS_PER_QUARTER / 4;
    let bar = 16 * sixteenth;
    let mut project = Project::new();
    project.set_bpm(124.0)?;

    let acid = project.add_midi_track("Ferro acid");
    let preset = builtin::preset_names(BuiltinKind::Ferro)
        .iter()
        .position(|name| *name == "Acid slide")
        .unwrap();
    project.set_plugin_instrument(acid, Some(ferro(preset)))?;
    // (step, key, slide into it, accent)
    let line = [
        (0, 36, false, true),
        (2, 36, false, false),
        (3, 48, true, false),
        (4, 36, false, true),
        (6, 39, false, false),
        (7, 41, true, false),
        (8, 36, false, true),
        (10, 43, false, false),
        (11, 46, true, true),
        (12, 48, true, false),
        (14, 36, false, false),
        (15, 34, true, false),
    ];
    let mut notes = Vec::new();
    for repeat in 0..4 {
        for (index, &(step, key, slide, accent)) in line.iter().enumerate() {
            let next = line.get(index + 1).map_or(16, |&(next, ..)| next);
            let sliding_out = line.get(index + 1).is_some_and(|&(_, _, slide, _)| slide);
            let start = repeat * bar + step * sixteenth;
            // A note that slides into the next one overlaps it, like a 303's tie.
            let length = (next - step) * sixteenth + if sliding_out { 20 } else { 0 };
            notes.push(MidiNote {
                velocity: if accent { 120 } else { 84 },
                slide_ticks: if slide { sixteenth as u32 } else { 0 },
                ..MidiNote::new(start, length - if sliding_out { 0 } else { 30 }, key, 100)
            });
        }
    }
    project.add_midi_clip(
        acid,
        MidiClip {
            name: Some("Slides".to_owned()),
            notes,
            ..MidiClip::new(0, 4 * bar)
        },
    )?;

    let lead = project.add_midi_track("Ferro lead");
    let lead_preset = builtin::preset_names(BuiltinKind::Ferro)
        .iter()
        .position(|name| *name == "Lead one")
        .unwrap();
    project.set_plugin_instrument(lead, Some(ferro(lead_preset)))?;
    project.add_midi_clip(
        lead,
        MidiClip {
            name: Some("Bend swoop".to_owned()),
            notes: vec![
                MidiNote::new(0, 2 * bar, 67, 100),
                MidiNote::new(2 * bar, 2 * bar, 72, 100),
            ],
            // Dive two semitones under, rise back, and wobble at the end.
            pitch_bend: vec![
                BendPoint {
                    tick: 0,
                    value: -8192,
                },
                BendPoint {
                    tick: bar / 2,
                    value: 0,
                },
                BendPoint {
                    tick: 3 * bar,
                    value: 0,
                },
                BendPoint {
                    tick: 3 * bar + bar / 4,
                    value: 4096,
                },
                BendPoint {
                    tick: 3 * bar + bar / 2,
                    value: -4096,
                },
                BendPoint {
                    tick: 4 * bar - 1,
                    value: 0,
                },
            ],
            ..MidiClip::new(0, 4 * bar)
        },
    )?;

    let pad = project.add_midi_track("Ferro pad");
    let pad_preset = builtin::preset_names(BuiltinKind::Ferro)
        .iter()
        .position(|name| *name == "Dream pad")
        .unwrap();
    project.set_plugin_instrument(pad, Some(ferro(pad_preset)))?;
    let chords = [
        [48, 55, 60, 63],
        [44, 51, 56, 60],
        [46, 53, 58, 62],
        [43, 50, 55, 59],
    ];
    let mut pad_notes = Vec::new();
    for (index, chord) in chords.iter().enumerate() {
        for &key in chord {
            pad_notes.push(MidiNote::new(index as u64 * bar, bar, key, 80));
        }
    }
    project.add_midi_clip(
        pad,
        MidiClip {
            name: Some("Chords".to_owned()),
            notes: pad_notes,
            ..MidiClip::new(0, 4 * bar)
        },
    )?;

    save_project_folder(&project, &directory)?;
    println!("wrote {}", directory.display());
    Ok(())
}
