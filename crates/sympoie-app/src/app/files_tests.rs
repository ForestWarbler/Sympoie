use super::*;
use crate::actions::Action;
use crate::app::Startup;
use eframe::egui;
use sympoie::builtin::{BuiltinKind, instruments::sample_preview};
use sympoie::track::Track;

/// A 24-bit mono 44.1 kHz WAV, the kind the old importer rejected.
fn write_kick(dir: &Path, name: &str, frames: u32) -> PathBuf {
    let data_len = frames * 3;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_len + data_len % 2).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(44_100_u32.to_le_bytes());
    bytes.extend((44_100_u32 * 3).to_le_bytes());
    bytes.extend(3_u16.to_le_bytes());
    bytes.extend(24_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_len.to_le_bytes());
    for frame in 0..frames {
        let sample = ((f64::from(frame) * 0.05).sin() * 4_000_000.0) as i32;
        bytes.extend(&sample.to_le_bytes()[..3]);
    }
    if data_len % 2 == 1 {
        bytes.push(0);
    }
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn sampler(app: &SympoieApp, track: usize) -> &sympoie::plugin::PluginInstance {
    match &app.document.project().tracks()[track] {
        Track::Midi(midi) => midi.plugin_instrument().unwrap(),
        _ => panic!("track {track} is not a MIDI track"),
    }
}

#[test]
fn double_clicked_sample_adds_a_sampler_track_or_reloads_the_selected_sampler() {
    let dir = std::env::temp_dir().join(format!("sympoie-sampler-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let kick = write_kick(&dir, "Kick 24.wav", 4410);
    let snare = write_kick(&dir, "Snare.wav", 2205);
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    let tracks = app.document.project().tracks().len();

    app.apply(
        &ctx,
        Action::LoadSampleIntoSampler {
            path: kick.clone(),
            track: None,
        },
    );
    let added = tracks;
    assert_eq!(app.document.project().tracks().len(), tracks + 1);
    assert_eq!(app.document.project().tracks()[added].name(), "Kick 24");
    assert_eq!(app.selection.tracks, [added].into());
    let plugin = sampler(&app, added);
    assert_eq!(
        BuiltinKind::from_descriptor(&plugin.descriptor),
        Some(BuiltinKind::Sampler)
    );
    let id = plugin.id;
    let preview = sample_preview(&plugin.state).unwrap();
    assert_eq!(preview.frames, 4410, "kept at its own rate");

    // With that Sampler track selected, the next sample replaces its sample in place.
    app.apply(
        &ctx,
        Action::LoadSampleIntoSampler {
            path: snare,
            track: Some(added),
        },
    );
    assert_eq!(app.document.project().tracks().len(), tracks + 1);
    let plugin = sampler(&app, added);
    assert_eq!(plugin.id, id);
    assert_eq!(sample_preview(&plugin.state).unwrap().frames, 2205);

    // One undo step each.
    app.apply(&ctx, Action::Undo);
    assert_eq!(
        sample_preview(&sampler(&app, added).state).unwrap().frames,
        4410
    );
    app.apply(&ctx, Action::Undo);
    assert_eq!(app.document.project().tracks().len(), tracks);
    let _ = std::fs::remove_dir_all(dir);
}

/// A 32-bit float mono 48 kHz WAV of decaying clicks at each of `hits` (frames).
fn write_loop(dir: &Path, name: &str, frames: usize, hits: &[usize]) -> PathBuf {
    let mut audio = vec![0.0_f32; frames];
    for &hit in hits {
        for (offset, sample) in audio[hit..].iter_mut().take(6000).enumerate() {
            *sample += 0.8 * (offset as f32 * 0.9).sin() * (-(offset as f32) / 900.0).exp();
        }
    }
    let data_len = frames as u32 * 4;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_len).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(3_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(48_000_u32.to_le_bytes());
    bytes.extend((48_000_u32 * 4).to_le_bytes());
    bytes.extend(4_u16.to_le_bytes());
    bytes.extend(32_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_len.to_le_bytes());
    for sample in audio {
        bytes.extend(sample.to_le_bytes());
    }
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn slicer_slices_a_loaded_sample_and_each_slice_edit_is_one_undo_step() {
    use sympoie::builtin::instruments::{SLICE_DETECT, slice_starts, slicer_parameters};
    let dir = std::env::temp_dir().join(format!("sympoie-slicer-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let drums = write_loop(&dir, "Drums.wav", 48_000, &[0, 12_000, 24_000, 36_000]);
    let ctx = egui::Context::default();
    let mut app = SympoieApp::new(&ctx, Startup::default());
    let track = app
        .edit_structural("Add Slicer Track", |project| {
            let index = edit::add_track(project, edit::TrackKind::Midi, None);
            edit::insert_plugin(project, index, BuiltinKind::Slicer.instance(), None)?;
            Ok(index)
        })
        .unwrap();
    let id = sampler(&app, track).id;
    assert_eq!(edit::sampler_on(app.document.project(), track), Some(id));

    // A sample dropped on a Slicer track lands in that Slicer, already sliced.
    app.apply(
        &ctx,
        Action::LoadSampleIntoSampler {
            path: drums,
            track: Some(track),
        },
    );
    let starts = slice_starts(sampler(&app, track));
    assert_eq!(starts.len(), 4, "{starts:?}");
    for (start, hit) in starts.iter().zip([0.0, 0.25, 0.5, 0.75]) {
        assert!((start - hit).abs() < 0.002, "{starts:?}");
    }

    app.apply(
        &ctx,
        Action::SetSlices {
            id,
            starts: vec![0.0, 0.5],
            label: "Remove Slice",
        },
    );
    assert_eq!(slice_starts(sampler(&app, track)), [0.0, 0.5]);

    let detect = slicer_parameters()
        .into_iter()
        .find(|spec| spec.id == SLICE_DETECT)
        .unwrap();
    app.apply(
        &ctx,
        Action::SetVst3Parameter {
            id,
            parameter: SLICE_DETECT,
            value: detect.normalize(2.0),
        },
    );
    app.apply(&ctx, Action::AnalyzeSlices { id });
    let grid = slice_starts(sampler(&app, track));
    assert_eq!(grid.len(), 8);
    assert_eq!(grid[1], 0.125);

    app.apply(&ctx, Action::Undo);
    app.apply(&ctx, Action::Undo);
    assert_eq!(slice_starts(sampler(&app, track)), [0.0, 0.5]);
    app.apply(&ctx, Action::Undo);
    assert_eq!(slice_starts(sampler(&app, track)).len(), 4);
    app.apply(&ctx, Action::Undo);
    assert!(sampler(&app, track).state.is_empty());
    assert_eq!(slice_starts(sampler(&app, track)), [0.0]);
    let _ = std::fs::remove_dir_all(dir);
}
