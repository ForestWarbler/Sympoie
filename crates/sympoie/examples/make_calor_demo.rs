//! Writes a demo project with Calor warming up a clean little band: a round sine bass, soft
//! electric-piano chords and a light kit, all nearly free of harmonics on their own. Calor
//! splits them into three bands and colours each its own way: an iron transformer on the
//! lows, held steady by a little compression, so the bass grows weight without smearing;
//! a tube on the mids, leaning, with the even harmonics turned up so the keys glow, and
//! hotter as the music gets louder; and tape on the highs, mixed in under the dry hats.
//! An LFO breathes the mids' even harmonics slowly. Play it and pick a band to see its
//! curve, solo a band to hear what Calor does to it alone, or toggle Bypass and Auto gain
//! to compare at the same loudness.
//!
//! `cargo run -p sympoie --example make_calor_demo -- /tmp/calor-demo.symp`
use std::f64::consts::TAU;
use std::path::PathBuf;

use sympoie::builtin::{BuiltinKind, calor};
use sympoie::clip::AudioClip;
use sympoie::project::{AudioBuffer, Project};
use sympoie::project_json::save_project_folder;
use sympoie::{Frame, SAMPLE_RATE};

const BPM: f64 = 100.0;

/// Midi key to Hz.
fn hz(key: f64) -> f64 {
    440.0 * 2.0_f64.powf((key - 69.0) / 12.0)
}

/// Sixteen bars of bass, chords and drums, as stereo frames.
fn band() -> Vec<Frame> {
    let rate = f64::from(SAMPLE_RATE);
    let beat = 60.0 / BPM;
    let bars = 16;
    let length = (f64::from(bars) * 4.0 * beat * rate) as usize;
    // Em7, Cmaj7, G, D: a bass root per bar, chord tones above.
    let progression: [(f64, [f64; 4]); 4] = [
        (40.0, [64.0, 67.0, 71.0, 74.0]),
        (36.0, [64.0, 67.0, 71.0, 72.0]),
        (43.0, [62.0, 67.0, 71.0, 74.0]),
        (38.0, [62.0, 66.0, 69.0, 74.0]),
    ];
    // The bass riff in eighths, in semitones above the root.
    let riff = [0.0, 0.0, 12.0, 0.0, 7.0, 0.0, 10.0, 12.0];
    let mut noise = 0x9e37_79b9_u32;
    let mut audio = vec![[0.0_f32; 2]; length];
    for (index, frame) in audio.iter_mut().enumerate() {
        let time = index as f64 / rate;
        let beats = time / beat;
        let bar = (beats / 4.0) as usize;
        let (root, chord) = progression[bar % progression.len()];
        // Bass: an eighth-note pluck, a sine with a little octave.
        let eighth = beats * 2.0;
        let step = eighth as usize % riff.len();
        let since = (eighth.fract()) * beat / 2.0;
        let pitch = hz(root + riff[step]);
        let pluck = (-since / 0.18).exp() * (since / 0.004).min(1.0);
        let bass =
            0.32 * pluck * ((TAU * pitch * time).sin() + 0.08 * (TAU * 2.0 * pitch * time).sin());
        // Keys: the chord on beats one and three, slow decay.
        let half = beats / 2.0;
        let held = half.fract() * 2.0 * beat;
        let envelope = (-held / 0.9).exp() * (held / 0.01).min(1.0);
        let keys: f64 = chord
            .iter()
            .map(|&key| {
                let f = hz(key);
                (TAU * f * time).sin() + 0.15 * (TAU * 2.0 * f * time + 0.3).sin()
            })
            .sum::<f64>()
            * 0.045
            * envelope;
        // Kick on every beat, a hat on every eighth.
        let since_beat = beats.fract() * beat;
        let sweep = 45.0 + 75.0 * (-since_beat / 0.03).exp();
        let kick = 0.5 * (-since_beat / 0.16).exp() * (TAU * sweep * since_beat).sin();
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        let white = f64::from(noise) / f64::from(u32::MAX) - 0.5;
        let since_eighth = eighth.fract() * beat / 2.0;
        let hat = 0.06 * white * (-since_eighth / 0.025).exp();
        let left = bass + keys * 1.1 + kick + hat * 0.8;
        let right = bass + keys * 0.9 + kick + hat * 1.2;
        *frame = [left as f32, right as f32];
    }
    audio
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/calor-demo.symp"));
    let mut project = Project::new();
    project.set_bpm(BPM)?;
    let audio = band();
    let frames = audio.len() as u64;
    let asset = project.add_asset(AudioBuffer::new(audio)?);
    let track = project.add_audio_track("Clean band");
    // The sixteen bars play three times from one sample, for about two minutes.
    let ticks = project.tempo().frames_to_ticks(frames)?;
    for repeat in 0..3 {
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
                name: Some("Bass, keys and kit".to_owned()),
            },
        )?;
    }
    let mut instance = BuiltinKind::Calor.instance();
    let (lows, mids, highs) = (0, 1, 2);
    let band = calor::band_parameter;
    use calor::Slot;
    // Models: 1 Tube, 2 Tape, 3 Transformer. Modulation: sources 1 LFO and 2 Envelope;
    // target 0 Drive and 4 Even; band 0 is every band, then 1 the first.
    for (id, value) in [
        (calor::BANDS, 3.0),
        (calor::CROSSOVER_BASE, 180.0),
        (calor::CROSSOVER_BASE + 1, 2500.0),
        (band(lows, Slot::Model), 3.0),
        (band(lows, Slot::Drive), 12.0),
        (band(lows, Slot::Hardness), 45.0),
        (band(lows, Slot::Dynamics), 30.0),
        (band(mids, Slot::Model), 1.0),
        (band(mids, Slot::Drive), 15.0),
        (band(mids, Slot::Hardness), 35.0),
        (band(mids, Slot::Bias), 55.0),
        (band(mids, Slot::Even), 160.0),
        (band(mids, Slot::Emphasis), 4.0),
        (band(mids, Slot::EmphasisFrequency), 1500.0),
        (band(highs, Slot::Model), 2.0),
        (band(highs, Slot::Drive), 10.0),
        (band(highs, Slot::Mix), 70.0),
        (calor::TONE, -1.5),
        (calor::LFO_RATE, 0.25),
        (calor::mod_parameter(0, calor::MOD_SOURCE), 2.0),
        (calor::mod_parameter(0, calor::MOD_TARGET), 0.0),
        (calor::mod_parameter(0, calor::MOD_BAND), 2.0),
        (calor::mod_parameter(0, calor::MOD_AMOUNT), 20.0),
        (calor::mod_parameter(1, calor::MOD_SOURCE), 1.0),
        (calor::mod_parameter(1, calor::MOD_TARGET), 4.0),
        (calor::mod_parameter(1, calor::MOD_BAND), 2.0),
        (calor::mod_parameter(1, calor::MOD_AMOUNT), 10.0),
    ] {
        let spec = calor::parameters()[id as usize];
        instance.parameters[id as usize].value = spec.normalize(value);
    }
    project.add_effect(track.into(), instance)?;
    save_project_folder(&project, &directory)?;
    println!("Created {}", directory.display());
    Ok(())
}
