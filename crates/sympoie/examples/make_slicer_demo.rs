//! Writes a demo project with a Slicer holding a synthesized one-bar drum loop, sliced by
//! transient, and a clip that plays the slices back in a new order.
//!
//! `cargo run -p sympoie --example make_slicer_demo -- /tmp/slicer-demo.symp`
use std::path::PathBuf;

use sympoie::TICKS_PER_QUARTER;
use sympoie::builtin::BuiltinKind;
use sympoie::builtin::instruments::{
    SLICE_BASE, SLICE_COUNT, SliceSettings, analyze_slices, sample_state_from_wav,
    slicer_parameters,
};
use sympoie::clip::{MidiClip, MidiNote};
use sympoie::project::Project;
use sympoie::project_json::save_project_folder;

const RATE: usize = 48_000;
const BPM: f64 = 120.0;

/// One bar of kick, snare and hats at 120 BPM, as a mono float WAV.
fn drum_loop() -> Vec<u8> {
    let sixteenth = RATE * 60 / BPM as usize / 4;
    let mut audio = vec![0.0_f32; 16 * sixteenth];
    let mut noise = 0x9e37_79b9_u32;
    let mut white = move || {
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        noise as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let pattern = "K.h.S.hKh.K.S.hh";
    for (step, hit) in pattern.chars().enumerate() {
        let start = step * sixteenth;
        for (offset, sample) in audio[start..].iter_mut().take(sixteenth * 2).enumerate() {
            let t = offset as f32 / RATE as f32;
            *sample += match hit {
                'K' => {
                    let phase =
                        2.0 * std::f32::consts::PI * (50.0 * t + 6.0 * (1.0 - (-t * 30.0).exp()));
                    0.9 * phase.sin() * (-t * 9.0).exp()
                }
                'S' => {
                    0.5 * white() * (-t * 18.0).exp()
                        + 0.3 * (2.0 * std::f32::consts::PI * 190.0 * t).sin() * (-t * 25.0).exp()
                }
                'h' => 0.25 * white() * (-t * 60.0).exp(),
                _ => 0.0,
            };
        }
    }
    let data_len = audio.len() as u32 * 4;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + data_len).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(3_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend((RATE as u32).to_le_bytes());
    bytes.extend((RATE as u32 * 4).to_le_bytes());
    bytes.extend(4_u16.to_le_bytes());
    bytes.extend(32_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_len.to_le_bytes());
    for sample in audio {
        bytes.extend(sample.to_le_bytes());
    }
    bytes
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/slicer-demo.symp"));
    let wav = std::env::temp_dir().join(format!("Drum loop {}.wav", std::process::id()));
    std::fs::write(&wav, drum_loop())?;
    let mut slicer = BuiltinKind::Slicer.instance();
    slicer.state = sample_state_from_wav(&wav)?;
    std::fs::remove_file(&wav)?;
    let starts = analyze_slices(&slicer.state, SliceSettings::of(&slicer))?;
    let specs = slicer_parameters();
    let mut set = |id: u32, physical: f64| {
        let spec = specs.iter().find(|spec| spec.id == id).unwrap();
        slicer
            .parameters
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap()
            .value = spec.normalize(physical);
    };
    set(SLICE_COUNT, starts.len() as f64);
    for (slice, &start) in starts.iter().enumerate() {
        set(SLICE_BASE + slice as u32, start);
    }
    println!("Sliced the loop into {} slices", starts.len());

    let mut project = Project::new();
    project.set_bpm(BPM)?;
    let track = project.add_midi_track("Slicer drums");
    project.set_plugin_instrument(track, Some(slicer))?;
    let sixteenth = TICKS_PER_QUARTER / 4;
    let slices = starts.len() as u8;
    // Replay the slices in a shuffled order, two bars.
    let order = [0, 1, 2, 3, 4, 2, 5, 6, 0, 7, 3, 1, 4, 5, 2, 6];
    let notes = (0..32)
        .map(|step| {
            let slice = order[step % order.len()] % slices;
            MidiNote::new(step as u64 * 2 * sixteenth, sixteenth, 36 + slice, 110)
        })
        .collect();
    project.add_midi_clip(
        track,
        MidiClip {
            name: Some("Resequenced".to_owned()),
            notes,
            ..MidiClip::new(0, 64 * sixteenth)
        },
    )?;
    save_project_folder(&project, &directory)?;
    println!("Wrote {}", directory.display());
    Ok(())
}
