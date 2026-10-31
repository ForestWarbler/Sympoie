//! Writes a demo project with Reson taming a harsh, ringing voice: a sung vowel whose
//! harmonics sweep through two narrow resonances (a whistle near 3.2 kHz and a sizzle
//! near 5.6 kHz) over breath noise. Focus is all the way up, so Reson follows the melody
//! and leaves its harmonics alone, except where a resonance lifts one above its
//! neighbours, and Clean takes 12 dB off the breathy noise between them. Play it and
//! toggle Delta or Bypass to hear what it removes, turn Clean down to bring the breath
//! back, or turn Focus down to hear Reson work on everything that sticks out.
//!
//! `cargo run -p sympoie --example make_reson_demo -- /tmp/reson-demo.symp`
use std::f64::consts::TAU;
use std::path::PathBuf;

use sympoie::builtin::{BuiltinKind, reson};
use sympoie::clip::AudioClip;
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_json::save_project_folder;
use sympoie::{Frame, SAMPLE_RATE};

/// Level of a harmonic at `hz`: a falling vocal tilt with three formants, plus the two
/// narrow resonances Reson should find.
fn spectrum(hz: f64) -> f64 {
    let bump = |centre: f64, width: f64, gain: f64| {
        let octaves = (hz / centre).log2() / width;
        gain * (-0.5 * octaves * octaves).exp()
    };
    let db = -6.0 * (hz / 200.0).log2().max(0.0)
        + bump(700.0, 0.35, 10.0)
        + bump(1200.0, 0.3, 8.0)
        + bump(2600.0, 0.3, 6.0)
        + bump(3200.0, 0.04, 18.0)
        + bump(5600.0, 0.05, 14.0);
    10.0_f64.powf(db / 20.0)
}

fn voice(seconds: usize) -> Vec<Frame> {
    let rate = f64::from(SAMPLE_RATE);
    let notes = [196.0, 220.0, 246.94, 261.63, 293.66, 261.63, 220.0, 174.61];
    let mut phase = 0.0_f64;
    let mut noise = 0x2545_f491_u32;
    (0..seconds * SAMPLE_RATE as usize)
        .map(|frame| {
            let time = frame as f64 / rate;
            let note = notes[(time as usize) % notes.len()];
            // A gentle glide between notes and a 5.5 Hz vibrato.
            let within = time.fract();
            let previous = notes[(time as usize + notes.len() - 1) % notes.len()];
            let glide = previous + (note - previous) * (within / 0.08).min(1.0);
            let pitch = glide * (1.0 + 0.006 * (TAU * 5.5 * time).sin());
            phase = (phase + pitch / rate).fract();
            let mut sample = 0.0;
            let mut harmonic = 1;
            while harmonic as f64 * pitch < 12_000.0 {
                let hz = harmonic as f64 * pitch;
                sample += spectrum(hz) * (TAU * phase * harmonic as f64).sin() / harmonic as f64;
                harmonic += 1;
            }
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let breath = (f64::from(noise) / f64::from(u32::MAX) - 0.5) * 0.02;
            let swell = (within / 0.05).min(1.0) * (1.0 - 0.25 * within);
            let left = (0.22 * sample * swell + breath) as f32;
            let right = (0.22 * sample * swell - breath) as f32;
            [left, right]
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/reson-demo.symp"));
    let mut project = Project::new();
    project.set_bpm(120.0)?;
    let audio = voice(32);
    let frames = audio.len() as u64;
    let asset = project.add_asset(AudioBuffer::new(audio)?);
    let track = project.add_audio_track("Harsh voice");
    // The phrase plays four times from one sample, for two minutes of listening.
    let ticks = project.tempo().frames_to_ticks(frames)?;
    for repeat in 0..4 {
        project.add_audio_clip(
            track,
            AudioClip {
                asset,
                source_start: 0,
                start_tick: repeat * ticks,
                frame_count: frames,
                gain: 1.0,
                pitch_semitones: 0.0,
                pitch_algorithm: Default::default(),
                preserve_formants: false,
                name: Some("Vowel with whistle".to_owned()),
            },
        )?;
    }
    let mut instance = BuiltinKind::Reson.instance();
    for (id, value) in [
        (reson::DEPTH, 9.0),
        (reson::THRESHOLD, 4.5),
        (reson::FOCUS, 100.0),
        (reson::CLEAN, 12.0),
    ] {
        let spec = reson::parameters()[id as usize];
        instance.parameters[id as usize].value = spec.normalize(value);
    }
    project.add_effect(track.into(), instance)?;
    save_project_folder(&project, &directory)?;
    println!("Created {}", directory.display());
    Ok(())
}
