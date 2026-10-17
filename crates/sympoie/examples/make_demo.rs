use std::env;
use std::f32::consts::TAU;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use sympoie::wav::WavWriter;
use sympoie::{Frame, SAMPLE_RATE};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let directory = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("demo-output"));
    if arguments.next().is_some() {
        return Err("usage: cargo run --example make_demo -- [OUTPUT_DIRECTORY]".into());
    }
    fs::create_dir_all(&directory)?;

    let frames = SAMPLE_RATE as usize * 4;
    let mut drums = Vec::with_capacity(frames);
    let mut bass = Vec::with_capacity(frames);
    let notes = [
        110.0, 110.0, 130.8128, 146.8324, 110.0, 110.0, 164.8138, 146.8324,
    ];
    let mut noise_state = 1_u32;
    for frame in 0..frames {
        let time = frame as f32 / SAMPLE_RATE as f32;
        let beat_time = (frame % (SAMPLE_RATE as usize / 2)) as f32 / SAMPLE_RATE as f32;
        let hat_time = (frame % (SAMPLE_RATE as usize / 4)) as f32 / SAMPLE_RATE as f32;
        let fade = ((4.0 - time) / 0.03).clamp(0.0, 1.0);

        noise_state ^= noise_state << 13;
        noise_state ^= noise_state >> 17;
        noise_state ^= noise_state << 5;
        let noise = (noise_state as f64 / f64::from(u32::MAX) * 2.0 - 1.0) as f32;
        let kick = (TAU * 55.0 * beat_time).sin() * (-18.0 * beat_time).exp() * 0.28;
        let hat = noise * (-100.0 * hat_time).exp() * (hat_time / 0.001).min(1.0) * 0.045;
        drums.push([(kick + hat) * fade, (kick + hat * 0.7) * fade]);

        let note = notes[(frame / (SAMPLE_RATE as usize / 2)) % notes.len()];
        let envelope =
            (beat_time / 0.008).min(1.0) * ((0.44 - beat_time) / 0.06).clamp(0.0, 1.0) * fade;
        let sample = ((TAU * note * beat_time).sin() + 0.15 * (TAU * note * 2.0 * beat_time).sin())
            * 0.18
            * envelope;
        bass.push([sample, sample]);
    }

    let drums_path = directory.join("drums.wav");
    let bass_path = directory.join("bass.wav");
    save(&drums_path, &drums)?;
    save(&bass_path, &bass)?;
    println!("Created {}", drums_path.display());
    println!("Created {}", bass_path.display());
    Ok(())
}

fn save(path: &Path, samples: &[Frame]) -> io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = WavWriter::new(BufWriter::new(file), samples.len() as u64)?;
    writer.write_frames(samples)?;
    writer.finish()?;
    Ok(())
}
