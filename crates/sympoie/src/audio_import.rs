//! Reading audio files in every format Sympoie can export: WAV, AIFF, FLAC
//! and MP3.
//!
//! WAV goes through Sympoie's own reader in [`crate::wav`]; the others are
//! decoded by `symphonia`, built with only those codecs. Every format comes
//! out as stereo frames at the file's own rate, and [`read_audio`] converts
//! them to the engine's [`SAMPLE_RATE`].

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::ops::RangeInclusive;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::audio_file::Resampler;
use crate::project::AudioBuffer;
use crate::{Frame, SAMPLE_RATE};

/// File extensions that [`read_audio`] accepts, lowercase.
pub const EXTENSIONS: [&str; 7] = ["wav", "wave", "aif", "aiff", "aifc", "flac", "mp3"];

/// How the supported formats are described to people.
pub const FORMATS_DESCRIPTION: &str = "WAV, AIFF, FLAC and MP3";

/// Rates the resampler converts from; real files sit well inside this.
pub(crate) const SAMPLE_RATES: RangeInclusive<u32> = 1_000..=768_000;

/// Stereo audio at the rate it was stored at. Mono is copied to both
/// channels; files with more channels keep the first two, which WAV, AIFF,
/// FLAC and MP3 all define as front left and front right.
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub frames: Vec<Frame>,
}

/// Whether `path` has one of the [`EXTENSIONS`].
pub fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

/// Reads any supported audio file and converts it to the engine's rate.
pub fn read_audio(path: impl AsRef<Path>) -> io::Result<AudioBuffer> {
    to_engine_rate(decode_file(path)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Wav,
    Aiff,
    Flac,
    Mp3,
}

impl Container {
    fn hint(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
        }
    }
}

/// Recognizes a file by its first bytes, falling back to its extension for
/// MP3 streams whose first frame is preceded by junk.
fn sniff(header: &[u8], path: &Path) -> Option<Container> {
    match header {
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'A',
            b'V',
            b'E',
            ..,
        ] => Some(Container::Wav),
        [
            b'F',
            b'O',
            b'R',
            b'M',
            _,
            _,
            _,
            _,
            b'A',
            b'I',
            b'F',
            b'F' | b'C',
            ..,
        ] => Some(Container::Aiff),
        [b'f', b'L', b'a', b'C', ..] => Some(Container::Flac),
        [b'I', b'D', b'3', ..] => Some(Container::Mp3),
        [0xff, second, ..] if second & 0xe0 == 0xe0 => Some(Container::Mp3),
        _ => path
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| extension.eq_ignore_ascii_case("mp3"))
            .map(|_| Container::Mp3),
    }
}

/// Decodes any supported audio file at its own sample rate.
pub fn decode_file(path: impl AsRef<Path>) -> io::Result<DecodedAudio> {
    let path = path.as_ref();
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut header = [0_u8; 12];
    let read = file.read(&mut header)?;
    file.seek(SeekFrom::Start(0))?;
    let container = sniff(&header[..read], path).ok_or_else(|| {
        invalid_data(format!(
            "not a recognized audio file; Sympoie imports {FORMATS_DESCRIPTION}"
        ))
    })?;
    let decoded = match container {
        Container::Wav => crate::wav::decode_wav(&mut BufReader::new(file), file_len)?,
        _ => decode_with_symphonia(file, container)?,
    };
    if !SAMPLE_RATES.contains(&decoded.sample_rate) {
        return Err(invalid_data(format!(
            "unsupported sample rate {} Hz; expected 1 kHz to 768 kHz",
            decoded.sample_rate
        )));
    }
    Ok(decoded)
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn symphonia_error(container: Container, error: SymphoniaError) -> io::Error {
    match error {
        SymphoniaError::IoError(error) => error,
        error => invalid_data(format!(
            "cannot decode {} audio: {error}",
            container.hint().to_uppercase()
        )),
    }
}

fn decode_with_symphonia(file: File, container: Container) -> io::Result<DecodedAudio> {
    let failed = |error| symphonia_error(container, error);
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(container.hint());
    let options = FormatOptions {
        // Drops the encoder delay and padding that LAME records, so an MP3
        // keeps the length it was exported with.
        enable_gapless: true,
        ..Default::default()
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &options, &MetadataOptions::default())
        .map_err(failed)?;
    let mut reader = probed.format;
    let track = reader
        .default_track()
        .filter(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| invalid_data("the file has no audio track"))?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate;
    let mut declared_frames = track.codec_params.n_frames.filter(|frames| *frames > 0);
    if container == Container::Aiff {
        // symphonia counts the 8-byte offset and block size that open the
        // SSND chunk as audio, which overstates the length by up to 8 frames.
        let frame_bytes = track
            .codec_params
            .bits_per_coded_sample
            .zip(track.codec_params.channels)
            .map(|(bits, channels)| u64::from(bits.div_ceil(8)) * channels.count() as u64);
        declared_frames = declared_frames
            .zip(frame_bytes.filter(|bytes| *bytes > 0))
            .map(|(frames, bytes)| frames.saturating_sub(8 / bytes));
    }
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions { verify: true })
        .map_err(failed)?;

    let mut frames = Vec::new();
    let mut interleaved: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match reader.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(error) => return Err(failed(error)),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A damaged MP3 frame is skipped, as players do; lossless
            // formats carry checksums and are expected to be intact.
            Err(SymphoniaError::DecodeError(_)) if container == Container::Mp3 => continue,
            Err(error) => return Err(failed(error)),
        };
        let spec = *decoded.spec();
        if *sample_rate.get_or_insert(spec.rate) != spec.rate {
            return Err(invalid_data("the sample rate changes partway through"));
        }
        let channels = spec.channels.count();
        if channels == 0 {
            return Err(invalid_data("the audio has no channels"));
        }
        let buffer = match &mut interleaved {
            Some(buffer) if buffer.capacity() >= decoded.capacity() * channels => buffer,
            slot => slot.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
        };
        buffer.copy_interleaved_ref(decoded);
        frames
            .try_reserve(buffer.samples().len() / channels)
            .map_err(|error| io::Error::other(format!("cannot allocate audio: {error}")))?;
        frames.extend(buffer.samples().chunks_exact(channels).map(|frame| {
            if channels == 1 {
                [frame[0]; 2]
            } else {
                [frame[0], frame[1]]
            }
        }));
    }
    // The FLAC reader skips damaged frames to resynchronize, and a cut-off
    // file just ends early, so compare with the length in the header.
    if container != Container::Mp3
        && let Some(declared) = declared_frames
        && declared != frames.len() as u64
    {
        return Err(invalid_data(format!(
            "the file is damaged or cut off: it declares {declared} frames but holds {}",
            frames.len()
        )));
    }
    Ok(DecodedAudio {
        sample_rate: sample_rate.ok_or_else(|| invalid_data("the file has no sample rate"))?,
        frames,
    })
}

/// Converts decoded audio to the engine's rate. Nonfinite samples are
/// rejected first, so the resampler never spreads them to their neighbours.
pub fn to_engine_rate(decoded: DecodedAudio) -> io::Result<AudioBuffer> {
    let DecodedAudio {
        sample_rate,
        frames,
    } = decoded;
    if frames.iter().flatten().any(|sample| !sample.is_finite()) {
        return Err(invalid_data("the audio contains a nonfinite sample"));
    }
    let frames = if sample_rate == SAMPLE_RATE {
        frames
    } else {
        let mut resampler = Resampler::new(sample_rate, SAMPLE_RATE);
        // What `Resampler::flush` will have produced in total.
        let output = (u128::from(frames.len() as u64) * u128::from(SAMPLE_RATE)
            + u128::from(sample_rate) / 2)
            / u128::from(sample_rate);
        let mut resampled = Vec::new();
        usize::try_from(output)
            .map_err(|error| error.to_string())
            .and_then(|output| {
                resampled
                    .try_reserve_exact(output)
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| io::Error::other(format!("cannot allocate audio: {error}")))?;
        for block in frames.chunks(16384) {
            resampler.process(block, &mut resampled);
        }
        resampler.flush(&mut resampled);
        resampled
    };
    AudioBuffer::new(frames).map_err(|error| invalid_data(error.to_string()))
}
