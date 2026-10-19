//! Writing rendered audio to WAV, AIFF, FLAC and MP3 files at a chosen sample
//! rate and bit depth (or, for MP3, bitrate).
//!
//! The engine runs at [`SAMPLE_RATE`]; other rates go through a windowed-sinc
//! resampler. Integer formats can be TPDF-dithered. FLAC is encoded by the
//! pure-Rust `flacenc` crate and MP3 by LAME, through `mp3lame-encoder`.

use std::f64::consts::PI;
use std::io::{self, Write};

use crate::{Frame, SAMPLE_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileFormat {
    Wav,
    Aiff,
    Flac,
    Mp3,
}

impl FileFormat {
    pub const ALL: [Self; 4] = [Self::Wav, Self::Aiff, Self::Flac, Self::Mp3];

    pub fn label(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Aiff => "AIFF",
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
        }
    }

    /// Whether the format stores samples at `depth`. MP3 has no bit depth: it
    /// is set by its bitrate instead, and accepts any.
    pub fn supports(self, depth: BitDepth) -> bool {
        match self {
            Self::Wav | Self::Mp3 => true,
            Self::Aiff | Self::Flac => depth != BitDepth::Float32,
        }
    }

    pub fn has_bit_depth(self) -> bool {
        self != Self::Mp3
    }

    pub fn supports_rate(self, rate: u32) -> bool {
        self != Self::Mp3 || MP3_SAMPLE_RATES.contains(&rate)
    }
}

/// MPEG-1 Layer III rates; higher rates have no MP3 encoding.
pub const MP3_SAMPLE_RATES: [u32; 2] = [44_100, 48_000];
pub const MP3_BITRATES: [u32; 6] = [128, 160, 192, 224, 256, 320];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BitDepth {
    Int16,
    Int24,
    Float32,
}

impl BitDepth {
    pub const ALL: [Self; 3] = [Self::Int16, Self::Int24, Self::Float32];

    pub fn label(self) -> &'static str {
        match self {
            Self::Int16 => "16-bit",
            Self::Int24 => "24-bit",
            Self::Float32 => "32-bit float",
        }
    }

    pub fn bytes(self) -> u16 {
        match self {
            Self::Int16 => 2,
            Self::Int24 => 3,
            Self::Float32 => 4,
        }
    }

    pub fn is_integer(self) -> bool {
        self != Self::Float32
    }
}

pub const SAMPLE_RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Encoding {
    pub format: FileFormat,
    pub depth: BitDepth,
    pub sample_rate: u32,
    pub dither: bool,
    /// Constant MP3 bitrate in kbit/s; other formats ignore it.
    pub bitrate_kbps: u32,
}

impl Default for Encoding {
    fn default() -> Self {
        Self {
            format: FileFormat::Wav,
            depth: BitDepth::Int24,
            sample_rate: SAMPLE_RATE,
            dither: false,
            bitrate_kbps: 320,
        }
    }
}

impl Encoding {
    pub fn validate(&self) -> Result<(), String> {
        if !self.format.supports(self.depth) {
            return Err(format!(
                "{} files cannot hold {} samples",
                self.format.label(),
                self.depth.label()
            ));
        }
        if !self.format.supports_rate(self.sample_rate) {
            return Err(format!(
                "MP3 files are 44.1 or 48 kHz, not {} Hz",
                self.sample_rate
            ));
        }
        if self.format == FileFormat::Mp3 && mp3_bitrate(self.bitrate_kbps).is_none() {
            return Err(format!(
                "{} kbit/s is not an MP3 bitrate",
                self.bitrate_kbps
            ));
        }
        if !(8_000..=384_000).contains(&self.sample_rate) {
            return Err(format!(
                "{} Hz is not a supported sample rate",
                self.sample_rate
            ));
        }
        Ok(())
    }

    fn stores_floats(&self) -> bool {
        matches!(
            (self.format, self.depth),
            (FileFormat::Wav, BitDepth::Float32) | (FileFormat::Mp3, _)
        )
    }

    /// How many output frames `frames` engine frames become.
    pub fn output_frames(&self, frames: u64) -> u64 {
        if self.sample_rate == SAMPLE_RATE {
            frames
        } else {
            ((u128::from(frames) * u128::from(self.sample_rate) + u128::from(SAMPLE_RATE) / 2)
                / u128::from(SAMPLE_RATE)) as u64
        }
    }
}

/// Streams engine-rate frames into an audio file. Feed it with
/// [`write_frames`](Self::write_frames), then call [`finish`](Self::finish).
pub struct AudioFileWriter<W: Write> {
    writer: W,
    encoding: Encoding,
    expected_input: u64,
    received_input: u64,
    written: u64,
    clipped_samples: u64,
    resampler: Option<Resampler>,
    resampled: Vec<Frame>,
    dither: Option<Dither>,
    bytes: Vec<u8>,
    sink: Sink,
}

enum Sink {
    /// WAV and AIFF: the header is written first, samples follow as they come.
    Pcm,
    /// FLAC: the whole stream is encoded at the end, when its length and
    /// checksum are known.
    Flac(Vec<i32>),
    Mp3(Box<mp3lame_encoder::Encoder>, Vec<f32>),
}

fn mp3_bitrate(kbps: u32) -> Option<mp3lame_encoder::Bitrate> {
    use mp3lame_encoder::Bitrate::*;
    Some(match kbps {
        128 => Kbps128,
        160 => Kbps160,
        192 => Kbps192,
        224 => Kbps224,
        256 => Kbps256,
        320 => Kbps320,
        _ => return None,
    })
}

fn mp3_encoder(encoding: Encoding) -> io::Result<mp3lame_encoder::Encoder> {
    let failed = |error: mp3lame_encoder::BuildError| {
        io::Error::other(format!("cannot set up the MP3 encoder: {error}"))
    };
    let mut builder = mp3lame_encoder::Builder::new()
        .ok_or_else(|| io::Error::other("cannot start the MP3 encoder"))?;
    builder.set_num_channels(2).map_err(failed)?;
    builder
        .set_sample_rate(encoding.sample_rate)
        .map_err(failed)?;
    builder
        .set_brate(mp3_bitrate(encoding.bitrate_kbps).expect("validated bitrate"))
        .map_err(failed)?;
    builder
        .set_quality(mp3lame_encoder::Quality::Best)
        .map_err(failed)?;
    builder.build().map_err(failed)
}

impl<W: Write> AudioFileWriter<W> {
    /// `frames` is the number of engine-rate frames that will be written.
    pub fn new(mut writer: W, encoding: Encoding, frames: u64) -> io::Result<Self> {
        encoding
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let output = encoding.output_frames(frames);
        let sink = match encoding.format {
            FileFormat::Wav | FileFormat::Aiff => {
                let data = output
                    .checked_mul(u64::from(encoding.depth.bytes()) * 2)
                    .filter(|bytes| *bytes <= u64::from(u32::MAX) - 128)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "the output exceeds the 4 GB {} size limit",
                                encoding.format.label()
                            ),
                        )
                    })?;
                write_header(&mut writer, encoding, output, data as u32)?;
                Sink::Pcm
            }
            FileFormat::Flac => {
                if output > u64::from(u32::MAX) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "the output is too long for one FLAC file",
                    ));
                }
                let mut samples = Vec::new();
                samples
                    .try_reserve_exact(output as usize * 2)
                    .map_err(|_| io::Error::other("not enough memory to encode the FLAC file"))?;
                Sink::Flac(samples)
            }
            FileFormat::Mp3 => Sink::Mp3(Box::new(mp3_encoder(encoding)?), Vec::new()),
        };
        Ok(Self {
            writer,
            encoding,
            expected_input: frames,
            received_input: 0,
            written: 0,
            clipped_samples: 0,
            resampler: (encoding.sample_rate != SAMPLE_RATE)
                .then(|| Resampler::new(SAMPLE_RATE, encoding.sample_rate)),
            resampled: Vec::new(),
            dither: (encoding.dither && !encoding.stores_floats()).then(Dither::new),
            bytes: Vec::new(),
            sink,
        })
    }

    pub fn clipped_samples(&self) -> u64 {
        self.clipped_samples
    }

    pub fn write_frames(&mut self, frames: &[Frame]) -> io::Result<()> {
        if frames.len() as u64 > self.expected_input - self.received_input {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "more audio frames supplied than declared",
            ));
        }
        if frames.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot encode a nonfinite audio sample",
            ));
        }
        self.received_input += frames.len() as u64;
        match &mut self.resampler {
            Some(resampler) => {
                let mut resampled = std::mem::take(&mut self.resampled);
                resampled.clear();
                resampler.process(frames, &mut resampled);
                self.encode(&resampled)?;
                self.resampled = resampled;
            }
            None => self.encode(frames)?,
        }
        Ok(())
    }

    fn encode(&mut self, frames: &[Frame]) -> io::Result<()> {
        let target = self.encoding.output_frames(self.expected_input);
        let frames = &frames[..frames.len().min((target - self.written) as usize)];
        self.bytes.clear();
        let depth = self.encoding.depth;
        let big_endian = self.encoding.format == FileFormat::Aiff;
        if let Sink::Mp3(encoder, samples) = &mut self.sink {
            samples.clear();
            for &sample in frames.iter().flatten() {
                if !(-1.0..=1.0).contains(&sample) {
                    self.clipped_samples += 1;
                }
                samples.push(sample.clamp(-1.0, 1.0));
            }
            self.bytes
                .reserve(mp3lame_encoder::max_required_buffer_size(frames.len()));
            encoder
                .encode_to_vec(mp3lame_encoder::InterleavedPcm(samples), &mut self.bytes)
                .map_err(|error| io::Error::other(format!("MP3 encoding failed: {error}")))?;
            self.writer.write_all(&self.bytes)?;
            self.written += frames.len() as u64;
            return Ok(());
        }
        for frame in frames {
            for &sample in frame {
                if depth == BitDepth::Float32 {
                    self.bytes.extend_from_slice(&sample.to_le_bytes());
                    continue;
                }
                let scale = if depth == BitDepth::Int16 {
                    32_768.0
                } else {
                    8_388_608.0
                };
                let mut value = f64::from(sample) * scale;
                if let Some(dither) = &mut self.dither {
                    value += dither.next();
                }
                let (low, high) = (-scale, scale - 1.0);
                if !(-1.0..=1.0).contains(&sample) {
                    self.clipped_samples += 1;
                }
                let quantized = value.round().clamp(low, high) as i32;
                if let Sink::Flac(samples) = &mut self.sink {
                    samples.push(quantized);
                    continue;
                }
                let bytes = quantized.to_le_bytes();
                let bytes = &bytes[..usize::from(depth.bytes())];
                if big_endian {
                    self.bytes.extend(bytes.iter().rev());
                } else {
                    self.bytes.extend_from_slice(bytes);
                }
            }
        }
        self.writer.write_all(&self.bytes)?;
        self.written += frames.len() as u64;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        if self.received_input != self.expected_input {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "expected {} frames but received {}",
                    self.expected_input, self.received_input
                ),
            ));
        }
        let target = self.encoding.output_frames(self.expected_input);
        if let Some(resampler) = &mut self.resampler {
            let mut rest = Vec::new();
            resampler.flush(&mut rest);
            self.encode(&rest)?;
        }
        if self.written < target {
            let silence = vec![[0.0; 2]; (target - self.written) as usize];
            self.dither = None;
            self.encode(&silence)?;
        }
        match std::mem::replace(&mut self.sink, Sink::Pcm) {
            Sink::Pcm => {}
            Sink::Flac(samples) => {
                let bytes = encode_flac(samples, self.encoding)?;
                self.writer.write_all(&bytes)?;
            }
            Sink::Mp3(mut encoder, _) => {
                let mut bytes = Vec::with_capacity(7200);
                encoder
                    .flush_to_vec::<mp3lame_encoder::FlushGap>(&mut bytes)
                    .map_err(|error| io::Error::other(format!("MP3 encoding failed: {error}")))?;
                self.writer.write_all(&bytes)?;
            }
        }
        self.writer.flush()?;
        Ok(self.writer)
    }
}

/// Interleaved samples handed to the FLAC encoder without another copy.
struct FlacSource {
    samples: Vec<i32>,
    bits: usize,
    rate: usize,
    read: usize,
}

impl flacenc::source::Source for FlacSource {
    fn channels(&self) -> usize {
        2
    }

    fn bits_per_sample(&self) -> usize {
        self.bits
    }

    fn sample_rate(&self) -> usize {
        self.rate
    }

    fn read_samples<F: flacenc::source::Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> Result<usize, flacenc::error::SourceError> {
        let end = (self.read + block_size * 2).min(self.samples.len());
        dest.fill_interleaved(&self.samples[self.read..end])?;
        let frames = (end - self.read) / 2;
        self.read = end;
        Ok(frames)
    }

    fn len_hint(&self) -> Option<usize> {
        Some(self.samples.len() / 2)
    }
}

fn encode_flac(samples: Vec<i32>, encoding: Encoding) -> io::Result<Vec<u8>> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, error)| io::Error::other(format!("FLAC settings: {error}")))?;
    let source = FlacSource {
        samples,
        bits: usize::from(encoding.depth.bytes()) * 8,
        rate: encoding.sample_rate as usize,
        read: 0,
    };
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|error| io::Error::other(format!("FLAC encoding failed: {error}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|error| io::Error::other(format!("FLAC encoding failed: {error:?}")))?;
    Ok(sink.into_inner())
}

fn write_header(
    writer: &mut impl Write,
    encoding: Encoding,
    frames: u64,
    data: u32,
) -> io::Result<()> {
    let channels = 2_u16;
    let bytes = encoding.depth.bytes();
    let bits = bytes * 8;
    let rate = encoding.sample_rate;
    let mut header = Vec::new();
    match encoding.format {
        FileFormat::Wav => {
            let float = encoding.depth == BitDepth::Float32;
            let format_len: u32 = if float { 18 } else { 16 };
            let fact_len: u32 = if float { 12 } else { 0 };
            // Stereo frames have an even size, so the data never needs a pad byte.
            header.extend_from_slice(b"RIFF");
            header.extend_from_slice(&(4 + 8 + format_len + fact_len + 8 + data).to_le_bytes());
            header.extend_from_slice(b"WAVEfmt ");
            header.extend_from_slice(&format_len.to_le_bytes());
            header.extend_from_slice(&(if float { 3_u16 } else { 1 }).to_le_bytes());
            header.extend_from_slice(&channels.to_le_bytes());
            header.extend_from_slice(&rate.to_le_bytes());
            header.extend_from_slice(&(rate * u32::from(bytes * channels)).to_le_bytes());
            header.extend_from_slice(&(bytes * channels).to_le_bytes());
            header.extend_from_slice(&bits.to_le_bytes());
            if float {
                header.extend_from_slice(&0_u16.to_le_bytes());
                header.extend_from_slice(b"fact");
                header.extend_from_slice(&4_u32.to_le_bytes());
                header.extend_from_slice(&(frames as u32).to_le_bytes());
            }
            header.extend_from_slice(b"data");
            header.extend_from_slice(&data.to_le_bytes());
        }
        FileFormat::Aiff => {
            header.extend_from_slice(b"FORM");
            header.extend_from_slice(&(4 + 8 + 18 + 8 + 8 + data).to_be_bytes());
            header.extend_from_slice(b"AIFFCOMM");
            header.extend_from_slice(&18_u32.to_be_bytes());
            header.extend_from_slice(&channels.to_be_bytes());
            header.extend_from_slice(&(frames as u32).to_be_bytes());
            header.extend_from_slice(&bits.to_be_bytes());
            header.extend_from_slice(&extended(rate));
            header.extend_from_slice(b"SSND");
            header.extend_from_slice(&(8 + data).to_be_bytes());
            header.extend_from_slice(&[0; 8]);
        }
        FileFormat::Flac | FileFormat::Mp3 => unreachable!("only PCM files start with a header"),
    }
    writer.write_all(&header)
}

/// An 80-bit IEEE 754 extended float, as AIFF stores its sample rate.
fn extended(value: u32) -> [u8; 10] {
    let mut bytes = [0; 10];
    if value == 0 {
        return bytes;
    }
    let shift = value.leading_zeros();
    let exponent = 16_383 + 31 - shift as u16;
    let mantissa = u64::from(value) << (32 + shift);
    bytes[..2].copy_from_slice(&exponent.to_be_bytes());
    bytes[2..].copy_from_slice(&mantissa.to_be_bytes());
    bytes
}

/// Triangular dither of one least significant bit, from a fixed seed so that
/// exports are reproducible.
struct Dither(u64);

impl Dither {
    fn new() -> Self {
        Self(0x9e37_79b9_7f4a_7c15)
    }

    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1_u64 << 53) as f64
    }

    fn next(&mut self) -> f64 {
        self.uniform() - self.uniform()
    }
}

const PHASES: usize = 512;
const ZERO_CROSSINGS: f64 = 64.0;

/// A streaming windowed-sinc (Blackman-Harris) sample-rate converter.
pub struct Resampler {
    /// Input frames advanced per output frame.
    step: f64,
    half: usize,
    table: Vec<f32>,
    history: Vec<Frame>,
    /// Absolute input index of `history[0]`; negative indices are silence.
    base: i64,
    received: u64,
    produced: u64,
    expected: Option<u64>,
    from: u32,
    to: u32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let step = f64::from(from) / f64::from(to);
        // Below the lower Nyquist frequency, with a little room for the transition.
        let cutoff =
            (f64::from(to) / f64::from(from)).min(1.0) * if to < from { 0.95 } else { 1.0 };
        let half = (ZERO_CROSSINGS / cutoff).ceil() as usize;
        let width = 2 * half;
        let mut table = vec![0.0_f32; (PHASES + 1) * width];
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            for tap in 0..width {
                // Distance from the output instant to input tap `tap`.
                let x = fraction + (half as f64 - 1.0) - tap as f64;
                table[phase * width + tap] = kernel(x, cutoff, half as f64) as f32;
            }
        }
        Self {
            step,
            half,
            table,
            history: vec![[0.0; 2]; half],
            base: -(half as i64),
            received: 0,
            produced: 0,
            expected: None,
            from,
            to,
        }
    }

    pub fn process(&mut self, input: &[Frame], output: &mut Vec<Frame>) {
        self.history.extend_from_slice(input);
        self.received += input.len() as u64;
        self.drain(output);
    }

    fn drain(&mut self, output: &mut Vec<Frame>) {
        let width = 2 * self.half;
        loop {
            if self
                .expected
                .is_some_and(|expected| self.produced >= expected)
            {
                break;
            }
            let t = self.produced as f64 * self.step;
            let floor = t.floor();
            let first = floor as i64 - self.half as i64 + 1;
            let last = first + width as i64;
            if last > self.base + self.history.len() as i64 {
                break;
            }
            let fraction = t - floor;
            let position = fraction * PHASES as f64;
            let phase = (position as usize).min(PHASES - 1);
            let blend = (position - phase as f64) as f32;
            let start = (first - self.base) as usize;
            let taps = &self.history[start..start + width];
            let (a, b) = (
                &self.table[phase * width..(phase + 1) * width],
                &self.table[(phase + 1) * width..(phase + 2) * width],
            );
            let mut sum = [0.0_f32; 2];
            for ((frame, low), high) in taps.iter().zip(a).zip(b) {
                let weight = low + (high - low) * blend;
                sum[0] += frame[0] * weight;
                sum[1] += frame[1] * weight;
            }
            output.push(sum);
            self.produced += 1;
        }
        // Keep only what later outputs can still reach.
        let next = (self.produced as f64 * self.step).floor() as i64 - self.half as i64 + 1;
        let drop = (next - self.base).clamp(0, self.history.len() as i64) as usize;
        if drop > 4096 {
            self.history.drain(..drop);
            self.base += drop as i64;
        }
    }

    /// Emits the remaining output for the input received so far.
    pub fn flush(&mut self, output: &mut Vec<Frame>) {
        let expected = ((u128::from(self.received) * u128::from(self.to)
            + u128::from(self.from) / 2)
            / u128::from(self.from)) as u64;
        self.expected = Some(expected);
        self.history
            .extend(std::iter::repeat_n([0.0; 2], self.half * 2 + 1));
        self.drain(output);
    }
}

fn kernel(x: f64, cutoff: f64, half: f64) -> f64 {
    if x.abs() >= half {
        return 0.0;
    }
    let sinc = if x == 0.0 {
        1.0
    } else {
        (PI * cutoff * x).sin() / (PI * cutoff * x)
    };
    // Blackman-Harris over [-half, half].
    let n = (x + half) / (2.0 * half);
    let window = 0.35875 - 0.48829 * (2.0 * PI * n).cos() + 0.14128 * (4.0 * PI * n).cos()
        - 0.01168 * (6.0 * PI * n).cos();
    cutoff * sinc * window
}
