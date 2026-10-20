use std::f64::consts::PI;
use std::path::PathBuf;

use sympoie::audio_file::{AudioFileWriter, BitDepth, Encoding, FileFormat, SAMPLE_RATES};
use sympoie::audio_import::{decode_file, has_supported_extension, read_audio};
use sympoie::{Frame, SAMPLE_RATE};

/// Different tones on each side, so swapped or merged channels show up.
fn signal(frames: usize) -> Vec<Frame> {
    (0..frames)
        .map(|frame| {
            let time = frame as f64 / f64::from(SAMPLE_RATE);
            [
                (0.4 * (2.0 * PI * 440.0 * time).sin()) as f32,
                (0.3 * (2.0 * PI * 660.0 * time).sin()) as f32,
            ]
        })
        .collect()
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("sympoie-import-{}-{name}", std::process::id()))
}

fn export(encoding: Encoding, frames: &[Frame], path: &PathBuf) {
    let mut writer = AudioFileWriter::new(Vec::new(), encoding, frames.len() as u64).unwrap();
    for block in frames.chunks(1000) {
        writer.write_frames(block).unwrap();
    }
    std::fs::write(path, writer.finish().unwrap()).unwrap();
}

/// Largest per-sample error and error-to-signal power ratio, away from the
/// ends where the resampler fades in and out.
fn compare(expected: &[Frame], actual: &[Frame]) -> (f32, f64) {
    let range = 600..expected.len() - 600;
    let mut max_error = 0.0_f32;
    let (mut error_power, mut signal_power) = (0.0_f64, 0.0_f64);
    for (want, got) in expected[range.clone()].iter().zip(&actual[range]) {
        for channel in 0..2 {
            let error = got[channel] - want[channel];
            max_error = max_error.max(error.abs());
            error_power += f64::from(error).powi(2);
            signal_power += f64::from(want[channel]).powi(2);
        }
    }
    (max_error, error_power / signal_power)
}

#[test]
fn every_export_format_imports_back_at_the_same_length_and_content() {
    let frames = signal(9600);
    let mut checked = 0;
    for format in FileFormat::ALL {
        for depth in BitDepth::ALL {
            for sample_rate in SAMPLE_RATES {
                let encoding = Encoding {
                    format,
                    depth,
                    sample_rate,
                    ..Encoding::default()
                };
                if encoding.validate().is_err()
                    || (format == FileFormat::Mp3 && depth != BitDepth::Int24)
                {
                    // MP3 ignores the bit depth, so one is enough.
                    continue;
                }
                let label = format!("{} {} {sample_rate} Hz", format.label(), depth.label());
                let path = temp_path(&format!("round-trip.{}", format.extension()));
                export(encoding, &frames, &path);
                let decoded = decode_file(&path).unwrap_or_else(|error| panic!("{label}: {error}"));
                assert_eq!(decoded.sample_rate, sample_rate, "{label}");
                let audio = read_audio(&path).unwrap();
                std::fs::remove_file(&path).unwrap();
                assert_eq!(audio.len(), frames.len(), "{label}");
                let (max_error, noise) = compare(&frames, audio.samples());
                let limit = match (format, depth, sample_rate == SAMPLE_RATE) {
                    (FileFormat::Mp3, _, _) => {
                        assert!(noise < 1e-3, "{label}: noise ratio {noise}");
                        continue;
                    }
                    (_, BitDepth::Float32, true) => 0.0,
                    (_, BitDepth::Int16, true) => 0.5 / 32768.0,
                    (_, BitDepth::Int24, true) => 0.5 / 8_388_608.0,
                    // Two passes through the resampler.
                    (_, _, false) => 2e-3,
                };
                assert!(
                    max_error <= limit,
                    "{label}: error {max_error} over {limit}"
                );
                checked += 1;
            }
        }
    }
    // WAV in three depths and AIFF and FLAC in two, at four rates each.
    assert_eq!(checked, 28);
}

#[test]
fn files_are_recognized_by_content_not_name() {
    let frames = signal(4800);
    for format in [
        FileFormat::Wav,
        FileFormat::Aiff,
        FileFormat::Flac,
        FileFormat::Mp3,
    ] {
        let path = temp_path(&format!("misnamed-{}.bin", format.extension()));
        let encoding = Encoding {
            format,
            ..Encoding::default()
        };
        export(encoding, &frames, &path);
        let audio = read_audio(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(audio.unwrap().len(), frames.len(), "{}", format.label());
    }
    let path = temp_path("not-audio.flac");
    std::fs::write(&path, b"plain text, not audio").unwrap();
    let error = read_audio(&path).unwrap_err().to_string();
    std::fs::remove_file(&path).unwrap();
    assert!(error.contains("WAV, AIFF, FLAC and MP3"), "{error}");
}

#[test]
fn damaged_lossless_files_are_rejected() {
    let frames = signal(4800);
    for format in [FileFormat::Aiff, FileFormat::Flac] {
        let path = temp_path(&format!("damaged.{}", format.extension()));
        let encoding = Encoding {
            format,
            ..Encoding::default()
        };
        export(encoding, &frames, &path);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() / 2);
        if format == FileFormat::Flac {
            // Corrupt audio inside a frame rather than cutting it short.
            let at = bytes.len() / 2;
            bytes[at] ^= 0x5a;
        }
        std::fs::write(&path, &bytes).unwrap();
        let error = read_audio(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(
            error.contains("damaged or cut off"),
            "{}: {error}",
            format.label()
        );
    }
}

#[test]
fn import_extensions_cover_every_export_format() {
    for format in FileFormat::ALL {
        assert!(has_supported_extension(&PathBuf::from(format!(
            "a.{}",
            format.extension()
        ))));
    }
    for name in ["kick.WAV", "loop.aif", "pad.AIFC", "a.wave"] {
        assert!(has_supported_extension(&PathBuf::from(name)), "{name}");
    }
    for name in ["kick.wav.txt", "wav", "song.ogg"] {
        assert!(!has_supported_extension(&PathBuf::from(name)), "{name}");
    }
}

#[test]
fn sampler_loads_every_export_format_at_its_own_rate() {
    let frames = signal(4800);
    for (format, sample_rate) in [
        (FileFormat::Wav, 44_100),
        (FileFormat::Aiff, 88_200),
        (FileFormat::Flac, 44_100),
        (FileFormat::Mp3, 44_100),
    ] {
        let path = temp_path(&format!("sampler.{}", format.extension()));
        let encoding = Encoding {
            format,
            sample_rate,
            ..Encoding::default()
        };
        export(encoding, &frames, &path);
        let state = sympoie::builtin::instruments::sample_state_from_wav(&path);
        std::fs::remove_file(&path).unwrap();
        let preview = sympoie::builtin::instruments::sample_preview(&state.unwrap()).unwrap();
        assert_eq!(preview.sample_rate, sample_rate, "{}", format.label());
        assert_eq!(
            preview.frames,
            encoding.output_frames(4800) as usize,
            "{}",
            format.label()
        );
        assert!(
            preview.peaks.iter().any(|peak| peak[0].abs() > 0.3),
            "{}",
            format.label()
        );
    }
}
