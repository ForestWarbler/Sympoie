use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use sympoie::builtin::{self, BuiltinKind};
use sympoie::clip::{MidiClip, MidiNote};
use sympoie::plugin::PluginInstance;
use sympoie::project::Project;
use sympoie::project_file::write_project;
use sympoie::render::Renderer;
use sympoie::time::SnapGrid;
use sympoie::track::{AudioOutput, MixerSettings};
use sympoie::wav::WavWriter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let directory = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("demo-output/synth"));
    if arguments.next().is_some() {
        return Err("usage: cargo run --example make_synth_demo -- [OUTPUT_DIRECTORY]".into());
    }
    fs::create_dir_all(&directory)?;

    let mut project = Project::new();
    project.set_bpm(110.0)?;
    project.set_snap_grid(SnapGrid::Sixteenth);
    let pad = project.add_midi_track("Chord pad");
    let bass = project.add_midi_track("Bass");
    let group = project.add_group_track("Built-in synths");
    let patch = |values: &[(u32, f64)]| -> PluginInstance {
        let mut instance = BuiltinKind::Oscil.instance();
        let specs = builtin::parameters(BuiltinKind::Oscil);
        for &(id, physical) in values {
            let spec = specs.iter().find(|spec| spec.id == id).unwrap();
            instance
                .parameters
                .iter_mut()
                .find(|p| p.id == id)
                .unwrap()
                .value = spec.normalize(physical);
        }
        instance
    };
    project.set_plugin_instrument(
        pad,
        Some(patch(&[
            (0, -22.0),
            (100, 3.0),
            (20, 1800.0),
            (21, 0.8),
            (10, 0.04),
            (11, 0.2),
            (12, 0.55),
            (13, 0.4),
            (113, -10.0),
            (112, 7.0),
            (114, 0.5),
            (123, -10.0),
            (122, -7.0),
            (124, -0.5),
        ])),
    )?;
    project.set_plugin_instrument(
        bass,
        Some(patch(&[
            (0, -14.0),
            (100, 2.0),
            (20, 900.0),
            (21, 1.2),
            (10, 0.003),
            (11, 0.17),
            (12, 0.35),
            (13, 0.09),
        ])),
    )?;
    project.set_midi_track_mixer(
        pad,
        MixerSettings::new(1.0, -0.15, AudioOutput::Group(group))?,
    )?;
    project.set_midi_track_mixer(
        bass,
        MixerSettings::new(1.0, 0.1, AudioOutput::Group(group))?,
    )?;
    let mut chords = Vec::new();
    let mut pulse = Vec::new();
    for (bar, chord) in [[57, 60, 64], [53, 57, 60], [60, 64, 67], [55, 59, 62]]
        .iter()
        .enumerate()
    {
        for (voice, key) in chord.iter().enumerate() {
            chords.push(MidiNote {
                start_tick: bar as u64 * 3840,
                duration_ticks: 3840,
                key: *key,
                velocity: 76 + voice as u8 * 7,
                channel: voice as u8,
                slide_ticks: 0,
            });
        }
        for beat in 0..8 {
            pulse.push(MidiNote {
                start_tick: bar as u64 * 3840 + beat * 480,
                duration_ticks: 320,
                key: chord[0] - 12,
                velocity: if beat % 2 == 0 { 98 } else { 73 },
                channel: 0,
                slide_ticks: 0,
            });
        }
    }
    project.add_midi_clip(
        pad,
        MidiClip {
            offset_ticks: 0,
            start_tick: 0,
            length_ticks: 15_360,
            notes: chords,
            name: None,
            pitch_bend: Vec::new(),
        },
    )?;
    project.add_midi_clip(
        bass,
        MidiClip {
            offset_ticks: 0,
            start_tick: 0,
            length_ticks: 15_360,
            notes: pulse,
            name: None,
            pitch_bend: Vec::new(),
        },
    )?;

    let project_path = directory.join("oscil.symp");
    let wav_path = directory.join("oscil.wav");
    let mut file = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&project_path)?,
    );
    write_project(&project, &mut file)?;
    file.flush()?;
    let mut renderer = Renderer::new(&project)?;
    let total = renderer.duration_frames();
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&wav_path)?;
    let mut wav = WavWriter::new(BufWriter::new(file), total)?;
    let mut block = [[0.0; 2]; 512];
    let mut frame = 0;
    while frame < total {
        let count = (total - frame).min(block.len() as u64) as usize;
        renderer.render_block(frame, &mut block[..count]);
        wav.write_frames(&block[..count])?;
        frame += count as u64;
    }
    let clipped = wav.clipped_samples();
    wav.finish()?;
    println!("Created {}", project_path.display());
    println!(
        "Created {} ({total} frames, {clipped} clipped samples)",
        wav_path.display()
    );
    Ok(())
}
