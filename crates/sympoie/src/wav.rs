use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::project::AudioBuffer;
use crate::{Frame, SAMPLE_RATE};

const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;
const BYTES_PER_FRAME: u16 = CHANNELS * BITS_PER_SAMPLE / 8;

pub fn read_wav(path: impl AsRef<Path>) -> io::Result<AudioBuffer> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    decode_wav(&mut BufReader::new(file), file_len)
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn decode_wav<R: Read + Seek>(reader: &mut R, file_len: u64) -> io::Result<AudioBuffer> {
    if file_len < 12 {
        return Err(invalid_data("WAV file is shorter than its RIFF header"));
    }
    let mut header = [0_u8; 12];
    reader.read_exact(&mut header)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err(invalid_data(
            "expected a RIFF/WAVE file (RF64 is not supported)",
        ));
    }
    let riff_len = u32::from_le_bytes(header[4..8].try_into().unwrap());
    if riff_len < 4 {
        return Err(invalid_data(
            "RIFF size is too small for the WAVE identifier",
        ));
    }
    let riff_end = 8 + u64::from(riff_len);
    if riff_end > file_len {
        return Err(invalid_data(
            "RIFF size extends past the end of the WAV file",
        ));
    }

    let mut position = 12_u64;
    let mut sample_format = None;
    let mut data_chunk = None;
    while position < riff_end {
        if riff_end - position < 8 {
            return Err(invalid_data("incomplete WAV chunk header inside RIFF"));
        }
        let mut chunk_header = [0_u8; 8];
        reader.read_exact(&mut chunk_header)?;
        let chunk_len = u64::from(u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()));
        let payload_start = position + 8;
        let payload_end = payload_start + chunk_len;
        let next_chunk = payload_end + (chunk_len % 2);
        if next_chunk > riff_end {
            return Err(invalid_data(
                "WAV chunk or its padding extends past the RIFF boundary",
            ));
        }

        match &chunk_header[..4] {
            b"fmt " => {
                if sample_format.is_some() {
                    return Err(invalid_data("WAV contains more than one fmt chunk"));
                }
                if chunk_len < 16 {
                    return Err(invalid_data("WAV fmt chunk is shorter than 16 bytes"));
                }
                let mut format = [0_u8; 16];
                reader.read_exact(&mut format)?;
                sample_format = Some(validate_format(&format)?);
            }
            b"data" => {
                if data_chunk.is_some() {
                    return Err(invalid_data("WAV contains more than one data chunk"));
                }
                data_chunk = Some((payload_start, chunk_len));
            }
            _ => {}
        }
        reader.seek(SeekFrom::Start(next_chunk))?;
        position = next_chunk;
    }

    let sample_format =
        sample_format.ok_or_else(|| invalid_data("WAV is missing its fmt chunk"))?;
    let bytes_per_frame = sample_format.bytes_per_frame();
    let (data_start, data_len) =
        data_chunk.ok_or_else(|| invalid_data("WAV is missing its data chunk"))?;
    if data_len % u64::from(bytes_per_frame) != 0 {
        return Err(invalid_data(
            "WAV data ends with an incomplete stereo frame",
        ));
    }
    let frame_count = (data_len / u64::from(bytes_per_frame)) as usize;
    let mut samples = Vec::new();
    samples.try_reserve_exact(frame_count).map_err(|error| {
        io::Error::other(format!("cannot allocate WAV sample storage: {error}"))
    })?;
    reader.seek(SeekFrom::Start(data_start))?;
    let mut bytes = [0_u8; 4096];
    let mut remaining = frame_count;
    while remaining > 0 {
        let frames = remaining.min(bytes.len() / usize::from(bytes_per_frame));
        let batch = &mut bytes[..frames * usize::from(bytes_per_frame)];
        reader.read_exact(batch)?;
        match sample_format {
            SampleFormat::Pcm16 => {
                for frame in batch.as_chunks::<4>().0 {
                    samples.push([
                        f32::from(i16::from_le_bytes([frame[0], frame[1]])) / 32768.0,
                        f32::from(i16::from_le_bytes([frame[2], frame[3]])) / 32768.0,
                    ]);
                }
            }
            SampleFormat::Float32 => {
                for frame in batch.as_chunks::<8>().0 {
                    samples.push([
                        f32::from_le_bytes(frame[..4].try_into().unwrap()),
                        f32::from_le_bytes(frame[4..].try_into().unwrap()),
                    ]);
                }
            }
        }
        remaining -= frames;
    }
    AudioBuffer::new(samples).map_err(|error| invalid_data(error.to_string()))
}

#[derive(Clone, Copy)]
enum SampleFormat {
    Pcm16,
    Float32,
}

impl SampleFormat {
    fn bytes_per_frame(self) -> u16 {
        match self {
            Self::Pcm16 => 4,
            Self::Float32 => 8,
        }
    }
}

fn validate_format(format: &[u8; 16]) -> io::Result<SampleFormat> {
    let encoding = u16::from_le_bytes([format[0], format[1]]);
    let channels = u16::from_le_bytes([format[2], format[3]]);
    let sample_rate = u32::from_le_bytes(format[4..8].try_into().unwrap());
    let byte_rate = u32::from_le_bytes(format[8..12].try_into().unwrap());
    let block_align = u16::from_le_bytes([format[12], format[13]]);
    let bit_depth = u16::from_le_bytes([format[14], format[15]]);
    let sample_format = match (encoding, bit_depth) {
        (1, 16) => SampleFormat::Pcm16,
        (3, 32) => SampleFormat::Float32,
        _ => {
            return Err(invalid_data(format!(
                "unsupported WAV encoding {encoding} with {bit_depth} bits; expected 16-bit PCM or 32-bit IEEE float"
            )));
        }
    };
    if channels != CHANNELS || sample_rate != SAMPLE_RATE {
        return Err(invalid_data(format!(
            "unsupported WAV format: {sample_rate} Hz, {channels} channels; expected 48000 Hz, 2 channels"
        )));
    }
    if block_align != sample_format.bytes_per_frame()
        || byte_rate != SAMPLE_RATE * u32::from(sample_format.bytes_per_frame())
    {
        return Err(invalid_data(
            "WAV block alignment or byte rate is inconsistent with its format",
        ));
    }
    Ok(sample_format)
}

pub struct WavWriter<W: Write> {
    writer: W,
    frame_count: u64,
    written_frames: u64,
    clipped_samples: u64,
}

impl<W: Write> WavWriter<W> {
    pub fn new(mut writer: W, frame_count: u64) -> io::Result<Self> {
        let max_frames = u64::from(u32::MAX - 36) / u64::from(BYTES_PER_FRAME);
        if frame_count > max_frames {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output exceeds the RIFF/WAV size limit",
            ));
        }
        let data_len = frame_count as u32 * u32::from(BYTES_PER_FRAME);

        let mut header = [0_u8; 44];
        header[..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
        header[8..16].copy_from_slice(b"WAVEfmt ");
        header[16..20].copy_from_slice(&16_u32.to_le_bytes());
        header[20..22].copy_from_slice(&1_u16.to_le_bytes());
        header[22..24].copy_from_slice(&CHANNELS.to_le_bytes());
        header[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
        header[28..32].copy_from_slice(&(SAMPLE_RATE * u32::from(BYTES_PER_FRAME)).to_le_bytes());
        header[32..34].copy_from_slice(&BYTES_PER_FRAME.to_le_bytes());
        header[34..36].copy_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&data_len.to_le_bytes());
        writer.write_all(&header)?;

        Ok(Self {
            writer,
            frame_count,
            written_frames: 0,
            clipped_samples: 0,
        })
    }

    pub fn clipped_samples(&self) -> u64 {
        self.clipped_samples
    }

    pub fn write_frames(&mut self, frames: &[Frame]) -> io::Result<()> {
        let count = frames.len() as u64;
        if count > self.frame_count - self.written_frames {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "more audio frames supplied than declared in the WAV header",
            ));
        }
        if frames.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot encode a nonfinite audio sample",
            ));
        }

        let mut bytes = [0_u8; 4096];
        for block in frames.chunks(bytes.len() / usize::from(BYTES_PER_FRAME)) {
            let mut clipped = 0_u64;
            for (frame, encoded) in block.iter().zip(bytes.as_chunks_mut::<4>().0) {
                for (sample, destination) in frame.iter().zip(encoded.as_chunks_mut::<2>().0) {
                    if !(-1.0..=1.0).contains(sample) {
                        clipped += 1;
                    }
                    let quantized = (sample.clamp(-1.0, 1.0) * 32768.0).round() as i16;
                    destination.copy_from_slice(&quantized.to_le_bytes());
                }
            }
            self.writer
                .write_all(&bytes[..block.len() * usize::from(BYTES_PER_FRAME)])?;
            self.written_frames += block.len() as u64;
            self.clipped_samples += clipped;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        if self.written_frames != self.frame_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "WAV expected {} frames but received {}",
                    self.frame_count, self.written_frames
                ),
            ));
        }
        self.writer.flush()?;
        Ok(self.writer)
    }
}

pub struct FloatWavWriter<W: Write> {
    writer: W,
    frame_count: u64,
    written_frames: u64,
}

impl<W: Write> FloatWavWriter<W> {
    pub fn new(mut writer: W, frame_count: u64) -> io::Result<Self> {
        if frame_count > u64::from(u32::MAX - 50) / 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output exceeds the RIFF/WAV size limit",
            ));
        }
        let data_len = frame_count as u32 * 8;
        let mut header = [0_u8; 58];
        header[..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&(50 + data_len).to_le_bytes());
        header[8..16].copy_from_slice(b"WAVEfmt ");
        header[16..20].copy_from_slice(&18_u32.to_le_bytes());
        header[20..22].copy_from_slice(&3_u16.to_le_bytes());
        header[22..24].copy_from_slice(&CHANNELS.to_le_bytes());
        header[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
        header[28..32].copy_from_slice(&(SAMPLE_RATE * 8).to_le_bytes());
        header[32..34].copy_from_slice(&8_u16.to_le_bytes());
        header[34..36].copy_from_slice(&32_u16.to_le_bytes());
        header[38..42].copy_from_slice(b"fact");
        header[42..46].copy_from_slice(&4_u32.to_le_bytes());
        header[46..50].copy_from_slice(&(frame_count as u32).to_le_bytes());
        header[50..54].copy_from_slice(b"data");
        header[54..58].copy_from_slice(&data_len.to_le_bytes());
        writer.write_all(&header)?;
        Ok(Self {
            writer,
            frame_count,
            written_frames: 0,
        })
    }

    pub fn write_frames(&mut self, frames: &[Frame]) -> io::Result<()> {
        if frames.len() as u64 > self.frame_count - self.written_frames {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "more audio frames supplied than declared in the WAV header",
            ));
        }
        if frames.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot encode a nonfinite audio sample",
            ));
        }
        let mut bytes = [0_u8; 4096];
        for block in frames.chunks(bytes.len() / 8) {
            for (frame, encoded) in block.iter().zip(bytes.as_chunks_mut::<8>().0) {
                encoded[..4].copy_from_slice(&frame[0].to_le_bytes());
                encoded[4..].copy_from_slice(&frame[1].to_le_bytes());
            }
            self.writer.write_all(&bytes[..block.len() * 8])?;
            self.written_frames += block.len() as u64;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        if self.written_frames != self.frame_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "WAV expected {} frames but received {}",
                    self.frame_count, self.written_frames
                ),
            ));
        }
        self.writer.flush()?;
        Ok(self.writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn format() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(2_u16.to_le_bytes());
        bytes.extend(48000_u32.to_le_bytes());
        bytes.extend(192000_u32.to_le_bytes());
        bytes.extend(4_u16.to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes
    }

    fn riff(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = b"WAVE".to_vec();
        for (name, data) in chunks {
            body.extend_from_slice(*name);
            body.extend((data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 != 0 {
                body.push(0);
            }
        }
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((body.len() as u32).to_le_bytes());
        bytes.extend(body);
        bytes
    }

    fn decode(bytes: &[u8]) -> io::Result<AudioBuffer> {
        decode_wav(&mut Cursor::new(bytes), bytes.len() as u64)
    }

    #[test]
    fn skips_odd_unknown_chunks_and_metadata_after_data() {
        let bytes = riff(&[
            (b"JUNK", &[42, 43, 44]),
            (b"fmt ", &format()),
            (b"data", &[0, 128, 255, 127, 0, 64, 0, 192]),
            (b"LIST", &[1, 2, 3, 4, 5]),
        ]);
        let audio = decode(&bytes).unwrap();
        assert_eq!(audio.samples(), &[[-1.0, 32767.0 / 32768.0], [0.5, -0.5]]);
    }

    #[test]
    fn accepts_data_before_format_and_ignores_bytes_outside_riff() {
        let mut bytes = riff(&[(b"data", &[0, 0, 0, 0]), (b"fmt ", &format())]);
        bytes.extend_from_slice(b"not a RIFF chunk");
        assert_eq!(decode(&bytes).unwrap().samples(), &[[0.0, 0.0]]);
    }

    #[test]
    fn rejects_truncated_files_and_chunks() {
        let mut bytes = riff(&[(b"fmt ", &format()), (b"data", &[0, 0, 0, 0])]);
        bytes.pop();
        assert!(decode(&bytes).unwrap_err().to_string().contains("end"));
        let mut bytes = riff(&[(b"fmt ", &format()), (b"data", &[0, 0, 0, 0])]);
        bytes[40..44].copy_from_slice(&100_u32.to_le_bytes());
        assert!(decode(&bytes).unwrap_err().to_string().contains("boundary"));
    }

    #[test]
    fn rejects_missing_chunk_padding_and_partial_chunk_headers() {
        let mut bytes = riff(&[(b"fmt ", &format()), (b"data", &[]), (b"JUNK", &[1])]);
        bytes.pop();
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(decode(&bytes).unwrap_err().to_string().contains("padding"));
        let mut bytes = riff(&[(b"fmt ", &format()), (b"data", &[])]);
        bytes.push(0);
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(decode(&bytes).unwrap_err().to_string().contains("header"));
    }

    #[test]
    fn rejects_unsupported_and_inconsistent_formats() {
        for (offset, bytes) in [
            (0, 3_u16.to_le_bytes().to_vec()),
            (0, 65534_u16.to_le_bytes().to_vec()),
            (2, 1_u16.to_le_bytes().to_vec()),
            (4, 44100_u32.to_le_bytes().to_vec()),
            (8, 1_u32.to_le_bytes().to_vec()),
            (12, 2_u16.to_le_bytes().to_vec()),
            (14, 24_u16.to_le_bytes().to_vec()),
        ] {
            let mut fmt = format();
            fmt[offset..offset + bytes.len()].copy_from_slice(&bytes);
            assert!(decode(&riff(&[(b"fmt ", &fmt), (b"data", &[])])).is_err());
        }
    }

    #[test]
    fn rejects_missing_duplicate_and_incomplete_critical_chunks() {
        for chunks in [
            vec![(b"fmt ", format())],
            vec![(b"data", vec![])],
            vec![(b"fmt ", format()), (b"fmt ", format()), (b"data", vec![])],
            vec![(b"fmt ", format()), (b"data", vec![]), (b"data", vec![])],
            vec![(b"fmt ", vec![0; 15]), (b"data", vec![])],
            vec![(b"fmt ", format()), (b"data", vec![0, 0])],
        ] {
            let chunks: Vec<_> = chunks
                .iter()
                .map(|(name, bytes)| (*name, bytes.as_slice()))
                .collect();
            assert!(decode(&riff(&chunks)).is_err());
        }
    }

    #[test]
    fn every_pcm16_value_round_trips_exactly() {
        let frames: Vec<_> = (i32::from(i16::MIN)..=i32::from(i16::MAX))
            .map(|sample| [sample as f32 / 32768.0; 2])
            .collect();
        let mut writer = WavWriter::new(Vec::new(), frames.len() as u64).unwrap();
        writer.write_frames(&frames).unwrap();
        assert_eq!(writer.clipped_samples(), 0);
        let bytes = writer.finish().unwrap();
        assert_eq!(decode(&bytes).unwrap().samples(), frames);
    }

    #[test]
    fn quantizes_rounds_and_clips_to_pcm16_limits() {
        let mut writer = WavWriter::new(Vec::new(), 4).unwrap();
        writer
            .write_frames(&[
                [-1.0, 1.0],
                [-2.0, 2.0],
                [-0.5 / 32768.0, 0.5 / 32768.0],
                [f32::MIN, f32::MAX],
            ])
            .unwrap();
        assert_eq!(writer.clipped_samples(), 4);
        let bytes = writer.finish().unwrap();
        let values: Vec<_> = bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
            .collect();
        assert_eq!(values, [-32768, 32767, -32768, 32767, -1, 1, -32768, 32767]);
    }

    #[test]
    fn writes_header_and_requires_exact_frame_count() {
        let mut writer = WavWriter::new(Vec::new(), 1).unwrap();
        assert!(writer.write_frames(&[[0.0; 2]; 2]).is_err());
        assert!(writer.write_frames(&[[0.0, f32::NAN]]).is_err());
        assert!(writer.write_frames(&[[f32::INFINITY, 0.0]]).is_err());
        writer.write_frames(&[[0.0; 2]]).unwrap();
        let bytes = writer.finish().unwrap();
        assert_eq!(bytes.len(), 48);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 40);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 4);
        assert!(WavWriter::new(Vec::new(), 1).unwrap().finish().is_err());
    }

    #[test]
    fn supports_empty_audio_and_rejects_oversized_output_before_writing() {
        let bytes = WavWriter::new(Vec::new(), 0).unwrap().finish().unwrap();
        assert!(decode(&bytes).unwrap().is_empty());
        let mut destination = Vec::new();
        assert!(WavWriter::new(&mut destination, u64::MAX).is_err());
        assert!(destination.is_empty());
        let max_frames = u64::from(u32::MAX - 36) / 4;
        assert!(WavWriter::new(&mut destination, max_frames + 1).is_err());
        assert!(destination.is_empty());
    }

    #[test]
    fn float_wav_preserves_every_finite_sample_bit_without_clipping() {
        let frames = [
            [-0.0, f32::from_bits(1)],
            [f32::MIN, f32::MAX],
            [1.2345679, -2.5],
        ];
        let mut writer = FloatWavWriter::new(Vec::new(), frames.len() as u64).unwrap();
        writer.write_frames(&frames[..1]).unwrap();
        writer.write_frames(&frames[1..]).unwrap();
        let bytes = writer.finish().unwrap();
        assert_eq!(&bytes[38..42], b"fact");
        assert_eq!(u32::from_le_bytes(bytes[46..50].try_into().unwrap()), 3);
        let loaded = decode(&bytes).unwrap();
        assert_eq!(
            loaded
                .samples()
                .iter()
                .flatten()
                .map(|x| x.to_bits())
                .collect::<Vec<_>>(),
            frames
                .iter()
                .flatten()
                .map(|x| x.to_bits())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn float_wav_validates_lengths_and_nonfinite_audio() {
        let mut writer = FloatWavWriter::new(Vec::new(), 1).unwrap();
        assert!(writer.write_frames(&[[0.0; 2]; 2]).is_err());
        assert!(writer.write_frames(&[[f32::NAN, 0.0]]).is_err());
        assert!(writer.write_frames(&[[0.0, f32::INFINITY]]).is_err());
        assert!(writer.finish().is_err());
        let mut bytes = FloatWavWriter::new(Vec::new(), 0)
            .unwrap()
            .finish()
            .unwrap();
        assert!(decode(&bytes).unwrap().is_empty());
        assert!(FloatWavWriter::new(&mut bytes, u64::MAX).is_err());
        let mut writer = FloatWavWriter::new(Vec::new(), 1).unwrap();
        writer.write_frames(&[[0.0; 2]]).unwrap();
        let mut bytes = writer.finish().unwrap();
        bytes[58..62].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&bytes).is_err());
        bytes.truncate(bytes.len() - 4);
        let riff_size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
        bytes[54..58].copy_from_slice(&4_u32.to_le_bytes());
        assert!(
            decode(&bytes)
                .unwrap_err()
                .to_string()
                .contains("incomplete stereo frame")
        );
    }
}
