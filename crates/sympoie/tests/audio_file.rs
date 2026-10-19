use std::f64::consts::PI;

use sympoie::audio_file::{AudioFileWriter, BitDepth, Encoding, FileFormat, Resampler};
use sympoie::{Frame, SAMPLE_RATE};

fn encode(encoding: Encoding, frames: &[Frame]) -> (Vec<u8>, u64) {
    let mut writer = AudioFileWriter::new(Vec::new(), encoding, frames.len() as u64).unwrap();
    for block in frames.chunks(777) {
        writer.write_frames(block).unwrap();
    }
    let clipped = writer.clipped_samples();
    (writer.finish().unwrap(), clipped)
}

fn encoding(format: FileFormat, depth: BitDepth, sample_rate: u32) -> Encoding {
    Encoding {
        format,
        depth,
        sample_rate,
        ..Encoding::default()
    }
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}
fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn wav_files_match_their_headers_and_samples() {
    let frames = vec![[0.5, -0.25], [1.5, -1.0], [-0.000_001, 0.999_999]];
    let path = std::env::temp_dir().join(format!("sympoie-audio-file-{}.wav", std::process::id()));

    let (bytes, clipped) = encode(encoding(FileFormat::Wav, BitDepth::Int16, 48_000), &frames);
    assert_eq!(clipped, 1);
    std::fs::write(&path, &bytes).unwrap();
    let decoded = sympoie::wav::read_wav(&path).unwrap();
    assert_eq!(decoded.samples()[0], [0.5, -0.25]);
    assert_eq!(decoded.samples()[1], [32767.0 / 32768.0, -1.0]);

    let (bytes, _) = encode(
        encoding(FileFormat::Wav, BitDepth::Float32, 48_000),
        &frames,
    );
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(sympoie::wav::read_wav(&path).unwrap().samples(), frames);
    std::fs::remove_file(&path).unwrap();

    let (bytes, clipped) = encode(encoding(FileFormat::Wav, BitDepth::Int24, 44_100), &[]);
    assert_eq!((bytes.len(), clipped), (44, 0));
    let (bytes, _) = encode(encoding(FileFormat::Wav, BitDepth::Int24, 48_000), &frames);
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(le32(&bytes, 4) as usize, bytes.len() - 8);
    assert_eq!(&bytes[8..16], b"WAVEfmt ");
    assert_eq!(le16(&bytes, 20), 1);
    assert_eq!(le16(&bytes, 22), 2);
    assert_eq!(le32(&bytes, 24), 48_000);
    assert_eq!(le32(&bytes, 28), 48_000 * 6);
    assert_eq!((le16(&bytes, 32), le16(&bytes, 34)), (6, 24));
    assert_eq!(&bytes[36..40], b"data");
    assert_eq!(le32(&bytes, 40), 18);
    let sample = |index: usize| {
        let at = 44 + index * 3;
        i32::from_le_bytes([0, bytes[at], bytes[at + 1], bytes[at + 2]]) >> 8
    };
    assert_eq!(sample(0), 1 << 22);
    assert_eq!(sample(1), -(1 << 21));
    assert_eq!(sample(2), (1 << 23) - 1);
    assert_eq!(sample(4), -8);
}

#[test]
fn aiff_files_are_big_endian_with_an_extended_sample_rate() {
    let frames = vec![[0.5, -0.5]; 5];
    let (bytes, _) = encode(encoding(FileFormat::Aiff, BitDepth::Int16, 44_100), &frames);
    assert_eq!(&bytes[..4], b"FORM");
    assert_eq!(be32(&bytes, 4) as usize, bytes.len() - 8);
    assert_eq!(&bytes[8..16], b"AIFFCOMM");
    assert_eq!(be32(&bytes, 16), 18);
    assert_eq!(u16::from_be_bytes([bytes[20], bytes[21]]), 2);
    // Five frames at 48 kHz are five frames at 44.1 kHz once rounded.
    assert_eq!(be32(&bytes, 22), 5);
    assert_eq!(u16::from_be_bytes([bytes[26], bytes[27]]), 16);
    assert_eq!(
        bytes[28..38],
        [0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0],
        "44100 as an 80-bit float"
    );
    assert_eq!(&bytes[38..42], b"SSND");
    assert_eq!(be32(&bytes, 42), 8 + 20);
    assert_eq!(bytes.len(), 54 + 20);

    let (bytes, _) = encode(encoding(FileFormat::Aiff, BitDepth::Int24, 48_000), &frames);
    assert_eq!(bytes[28..32], [0x40, 0x0e, 0xbb, 0x80]);
    assert_eq!(&bytes[54..60], &[0x40, 0, 0, 0xc0, 0, 0]);
    let mut writer = AudioFileWriter::new(
        Vec::new(),
        encoding(FileFormat::Aiff, BitDepth::Float32, 48_000),
        1,
    );
    assert!(writer.is_err());
    writer = AudioFileWriter::new(
        Vec::new(),
        encoding(FileFormat::Wav, BitDepth::Int16, 48_000),
        1,
    );
    assert!(
        writer.unwrap().finish().is_err(),
        "missing frames are an error"
    );
}

fn sine(frequency: f64, rate: u32, frames: usize) -> Vec<Frame> {
    (0..frames)
        .map(|i| {
            let phase = 2.0 * PI * frequency * i as f64 / f64::from(rate);
            [(0.5 * phase.sin()) as f32, (0.25 * phase.cos()) as f32]
        })
        .collect()
}

#[test]
fn resampling_keeps_pitch_level_and_length() {
    for (rate, frequency) in [(44_100, 1000.0), (96_000, 7000.0), (88_200, 440.0)] {
        let input = sine(frequency, SAMPLE_RATE, 48_000);
        let mut resampler = Resampler::new(SAMPLE_RATE, rate);
        let mut output = Vec::new();
        for block in input.chunks(1000) {
            resampler.process(block, &mut output);
        }
        resampler.flush(&mut output);
        assert_eq!(output.len(), rate as usize);
        let expected = sine(frequency, rate, rate as usize);
        // Away from the edges, where the filter sees the missing silence.
        let edge = 200;
        let worst = output[edge..output.len() - edge]
            .iter()
            .zip(&expected[edge..])
            .flat_map(|(a, b)| [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()])
            .fold(0.0_f32, f32::max);
        assert!(worst < 2e-3, "{rate} Hz: worst error {worst}");
    }
}

#[test]
fn downsampling_removes_content_above_the_new_nyquist() {
    let input = sine(23_000.0, SAMPLE_RATE, 48_000);
    let encoding = encoding(FileFormat::Wav, BitDepth::Float32, 44_100);
    let (bytes, _) = encode(encoding, &input);
    let samples: Vec<f32> = bytes[58..]
        .chunks(4)
        .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
        .collect();
    assert_eq!(samples.len(), 44_100 * 2);
    let peak = samples[2000..samples.len() - 2000]
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(peak < 0.01, "aliased peak {peak}");
}

#[test]
fn dither_adds_at_most_one_step_of_noise_and_is_reproducible() {
    let silence = vec![[0.0; 2]; 4000];
    let dithered = Encoding {
        dither: true,
        ..encoding(FileFormat::Wav, BitDepth::Int16, 48_000)
    };
    let (first, _) = encode(dithered, &silence);
    let (second, _) = encode(dithered, &silence);
    assert_eq!(first, second);
    let values: Vec<i16> = first[44..]
        .chunks(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect();
    assert!(values.iter().all(|value| (-1..=1).contains(value)));
    assert!(values.iter().any(|value| *value != 0));
    let (plain, _) = encode(encoding(FileFormat::Wav, BitDepth::Int16, 48_000), &silence);
    assert!(plain[44..].iter().all(|byte| *byte == 0));
}

#[test]
fn flac_files_carry_the_quantized_samples() {
    use md5::{Digest, Md5};
    let input = sine(997.0, SAMPLE_RATE, 30_000);
    for (depth, scale) in [(BitDepth::Int16, 32_768.0), (BitDepth::Int24, 8_388_608.0)] {
        let (bytes, _) = encode(encoding(FileFormat::Flac, depth, 44_100), &input);
        assert_eq!(&bytes[..4], b"fLaC");
        // The STREAMINFO block comes first.
        assert_eq!(bytes[4] & 0x7f, 0);
        let info = u64::from_be_bytes(bytes[18..26].try_into().unwrap());
        let frames = (30_000 * 44_100 + 24_000) / 48_000;
        assert_eq!(info >> 44, 44_100, "sample rate");
        assert_eq!((info >> 41) & 7, 1, "two channels");
        assert_eq!((info >> 36) & 31, u64::from(depth.bytes()) * 8 - 1);
        assert_eq!(info & 0xf_ffff_ffff, frames, "total samples");
        // FLAC stores the MD5 of the samples as little-endian integers.
        let resampled = {
            let mut resampler = Resampler::new(SAMPLE_RATE, 44_100);
            let mut output = Vec::new();
            resampler.process(&input, &mut output);
            resampler.flush(&mut output);
            output
        };
        let mut hash = Md5::new();
        for sample in resampled.iter().flatten() {
            let value = (f64::from(*sample) * scale)
                .round()
                .clamp(-scale, scale - 1.0) as i32;
            hash.update(&value.to_le_bytes()[..usize::from(depth.bytes())]);
        }
        assert_eq!(bytes[26..42], hash.finalize()[..], "{depth:?} checksum");
        assert!(bytes.len() < resampled.len() * 2 * usize::from(depth.bytes()));
    }
    let float = AudioFileWriter::new(
        Vec::new(),
        encoding(FileFormat::Flac, BitDepth::Float32, 48_000),
        1,
    );
    assert!(float.is_err(), "FLAC has no float samples");
}

/// The MPEG audio frames in an MP3 file: (bitrate in kbit/s, sample rate).
fn mp3_frames(bytes: &[u8]) -> Vec<(u32, u32)> {
    let mut frames = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let header = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(header >> 21, 0x7ff, "frame sync at byte {at}");
        assert_eq!((header >> 19) & 3, 3, "MPEG-1");
        assert_eq!((header >> 17) & 3, 1, "Layer III");
        let bitrate = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ][((header >> 12) & 15) as usize];
        let rate = [44_100, 48_000, 32_000][((header >> 10) & 3) as usize];
        let padding = (header >> 9) & 1;
        frames.push((bitrate, rate));
        at += (144_000 * bitrate / rate + padding) as usize;
    }
    assert_eq!(at, bytes.len(), "the file ends on a frame boundary");
    frames
}

#[test]
fn mp3_files_hold_whole_frames_at_the_chosen_bitrate_and_rate() {
    let input = sine(440.0, SAMPLE_RATE, 48_000);
    for (rate, kbps) in [(48_000, 320), (44_100, 192)] {
        let encoding = Encoding {
            bitrate_kbps: kbps,
            ..encoding(FileFormat::Mp3, BitDepth::Int24, rate)
        };
        let (bytes, clipped) = encode(encoding, &input);
        assert_eq!(clipped, 0);
        let frames = mp3_frames(&bytes);
        // One second of audio, plus the encoder's delay and padding (and,
        // first, LAME's information frame).
        let samples = frames.len() * 1152;
        assert!(
            (rate as usize..rate as usize + 4 * 1152).contains(&samples),
            "{samples} samples at {rate} Hz"
        );
        assert!(frames[1..].iter().all(|frame| *frame == (kbps, rate)));
    }
    let (_, clipped) = encode(
        encoding(FileFormat::Mp3, BitDepth::Int24, 48_000),
        &[[1.5, -2.0], [0.5, 0.0]],
    );
    assert_eq!(clipped, 2);
    for (rate, kbps) in [(96_000, 320), (48_000, 100)] {
        let encoding = Encoding {
            bitrate_kbps: kbps,
            ..encoding(FileFormat::Mp3, BitDepth::Int24, rate)
        };
        assert!(AudioFileWriter::new(Vec::new(), encoding, 1).is_err());
    }
}
