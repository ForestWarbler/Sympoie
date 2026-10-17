use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::*;
use crate::plugin::MAX_PLUGIN_STATE_BYTES;

const MAGIC: &[u8; 8] = b"SYMSMP01";
const HEADER_BYTES: usize = 28;
const MAX_NAME_BYTES: usize = 1024;

#[derive(Debug, Clone)]
pub struct SamplePreview {
    pub name: String,
    pub frames: usize,
    pub sample_rate: u32,
    pub peaks: Vec<[f32; 2]>,
}

struct SampleHeader<'a> {
    name: &'a str,
    frames: usize,
    rate: u32,
    audio: &'a [u8],
    fingerprint: u64,
}

fn header(state: &[u8]) -> Result<SampleHeader<'_>, String> {
    if state.len() > MAX_PLUGIN_STATE_BYTES || state.len() < HEADER_BYTES || &state[..8] != MAGIC {
        return Err("Invalid or unsupported Sampler state".into());
    }
    let rate = u32::from_le_bytes(state[8..12].try_into().unwrap());
    let frames = u32::from_le_bytes(state[12..16].try_into().unwrap()) as usize;
    let name_len = u32::from_le_bytes(state[16..20].try_into().unwrap()) as usize;
    if !(8000..=384000).contains(&rate) || frames == 0 || name_len > MAX_NAME_BYTES {
        return Err("Sampler state has invalid sample metadata".into());
    }
    let data_start = HEADER_BYTES + name_len;
    let expected = data_start
        .checked_add(
            frames
                .checked_mul(8)
                .ok_or("Sampler sample size overflow")?,
        )
        .ok_or("Sampler sample size overflow")?;
    if expected != state.len() {
        return Err("Sampler state contains incomplete or excess audio".into());
    }
    let name = std::str::from_utf8(&state[HEADER_BYTES..data_start])
        .map_err(|_| "Sampler sample name is not UTF-8")?;
    if name.contains('\0') {
        return Err("Sampler sample name contains a null character".into());
    }
    let fingerprint = u64::from_le_bytes(state[20..28].try_into().unwrap());
    Ok(SampleHeader {
        name,
        frames,
        rate,
        audio: &state[data_start..],
        fingerprint,
    })
}

fn checksum(state: &[u8]) -> u64 {
    state[..20]
        .iter()
        .chain(state[HEADER_BYTES..].iter())
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn seal_state(state: &mut [u8]) {
    let fingerprint = checksum(state);
    state[20..28].copy_from_slice(&fingerprint.to_le_bytes());
}

pub fn sample_state_fingerprint(state: &[u8]) -> Option<u64> {
    if state.is_empty() {
        Some(0)
    } else {
        header(state).ok().map(|h| h.fingerprint)
    }
}

fn decode_frame(bytes: &[u8]) -> Result<Frame, String> {
    let frame = [
        f32::from_le_bytes(bytes[..4].try_into().unwrap()),
        f32::from_le_bytes(bytes[4..8].try_into().unwrap()),
    ];
    if frame.iter().any(|x| !x.is_finite()) {
        return Err("Sample contains nonfinite audio".into());
    }
    Ok(frame)
}

pub fn sample_preview(state: &[u8]) -> Result<SamplePreview, String> {
    if state.is_empty() {
        return Ok(SamplePreview {
            name: "No sample loaded".into(),
            frames: 0,
            sample_rate: SAMPLE_RATE,
            peaks: Vec::new(),
        });
    }
    let sample = header(state)?;
    if checksum(state) != sample.fingerprint {
        return Err("Sampler state checksum mismatch".into());
    }
    let columns = sample.frames.min(512);
    let mut peaks = vec![[f32::MAX, f32::MIN]; columns];
    for (index, bytes) in sample.audio.as_chunks::<8>().0.iter().enumerate() {
        let frame = decode_frame(bytes)?;
        let peak = &mut peaks[index * columns / sample.frames];
        peak[0] = peak[0].min(frame[0]).min(frame[1]);
        peak[1] = peak[1].max(frame[0]).max(frame[1]);
    }
    Ok(SamplePreview {
        name: sample.name.into(),
        frames: sample.frames,
        sample_rate: sample.rate,
        peaks,
    })
}

pub fn sampler_note_tail_frames(instance: &PluginInstance, key: u8) -> Result<u64, String> {
    sampler_note_tail_frames_with_automation(instance, key, &[])
}

pub fn sampler_note_tail_frames_with_automation(
    instance: &PluginInstance,
    key: u8,
    lanes: &[crate::automation::AutomationLane],
) -> Result<u64, String> {
    use crate::automation::AutomationTarget;
    if instance.state.is_empty() {
        return Ok(0);
    }
    let sample = header(&instance.state)?;
    let get = |id, min: f64, max: f64, default, steps: f64, largest: bool| {
        let physical = |normalized: f64| {
            let normalized = normalized.clamp(0.0, 1.0);
            let normalized = if steps > 0.0 {
                (normalized * steps).round() / steps
            } else {
                normalized
            };
            min + normalized * (max - min)
        };
        let saved = instance
            .parameters
            .iter()
            .find(|p| p.id == id)
            .map_or(default, |p| physical(p.value));
        lanes
            .iter()
            .filter(|lane| {
                lane.enabled()
                    && lane.target()
                        == AutomationTarget::PluginParameter {
                            plugin: instance.id,
                            parameter: id,
                        }
            })
            .flat_map(|lane| lane.points())
            .map(|point| physical(point.value))
            .fold(saved, |a, b| if largest { a.max(b) } else { a.min(b) })
    };
    if get(SAMPLE_MODE, 0.0, 2.0, 1.0, 2.0, false).round() != 0.0 {
        return Ok(0);
    }
    let pitch = f64::from(key) - get(SAMPLE_ROOT, 0.0, 127.0, 60.0, 127.0, true)
        + get(SAMPLE_TRANSPOSE, -48.0, 48.0, 0.0, 96.0, false)
        + get(SAMPLE_TUNE, -100.0, 100.0, 0.0, 0.0, false) / 100.0;
    let step = (f64::from(sample.rate) / f64::from(SAMPLE_RATE)) * (pitch / 12.0).exp2();
    let start = ((get(SAMPLE_START, 0.0, 1.0, 0.0, 0.0, false) * sample.frames as f64) as usize)
        .min(sample.frames - 1);
    let end = ((get(SAMPLE_END, 0.0, 1.0, 1.0, 0.0, true) * sample.frames as f64) as usize)
        .clamp(start + 1, sample.frames);
    let length = (end - start) as f64;
    Ok((length / step).ceil() as u64)
}

pub fn sample_state_from_wav(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("Cannot open sample: {e}"))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Sample.wav");
    let name: String = name.chars().filter(|c| *c != '\0').take(255).collect();
    decode_wav(&mut BufReader::new(file), length, &name)
}

fn decode_wav<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    name: &str,
) -> Result<Vec<u8>, String> {
    let io = |error: std::io::Error| error.to_string();
    let mut riff = [0_u8; 12];
    reader.read_exact(&mut riff).map_err(io)?;
    if &riff[..4] != b"RIFF" || &riff[8..] != b"WAVE" {
        return Err("Expected a RIFF/WAVE sample".into());
    }
    let end = 8 + u64::from(u32::from_le_bytes(riff[4..8].try_into().unwrap()));
    if end < 12 || end > file_len {
        return Err("Truncated WAV container".into());
    }
    let mut position = 12;
    let mut format = None;
    let mut data = None;
    while position < end {
        if end - position < 8 {
            return Err("Truncated WAV chunk header".into());
        }
        let mut chunk = [0_u8; 8];
        reader.read_exact(&mut chunk).map_err(io)?;
        let length = u64::from(u32::from_le_bytes(chunk[4..].try_into().unwrap()));
        let start = position + 8;
        let next = start + length + length % 2;
        if next > end {
            return Err("WAV chunk or padding exceeds RIFF boundary".into());
        }
        match &chunk[..4] {
            b"fmt " => {
                if format.is_some() || length < 16 {
                    return Err("Invalid or duplicate WAV format".into());
                }
                let mut bytes = [0_u8; 40];
                reader
                    .read_exact(&mut bytes[..length.min(40) as usize])
                    .map_err(io)?;
                let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
                let mut encoding = u16_at(0);
                let channels = u16_at(2);
                let rate = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                let bytes_per_second = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
                let align = u16_at(12);
                let bits = u16_at(14);
                if encoding == 0xfffe {
                    if length < 40
                        || u16_at(16) < 22
                        || u16_at(18) > bits
                        || 18 + u64::from(u16_at(16)) > length
                        || bytes[26..40] != [0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]
                    {
                        return Err("Unsupported extensible WAV format".into());
                    }
                    encoding = u16_at(24);
                }
                if !matches!((encoding, bits), (1, 8 | 16 | 24 | 32) | (3, 32 | 64)) {
                    return Err(
                        "Sampler supports PCM 8/16/24/32-bit and float 32/64-bit WAV".into(),
                    );
                }
                if !(1..=2).contains(&channels) || !(8000..=384000).contains(&rate) {
                    return Err("Sampler requires mono or stereo WAV at 8–384 kHz".into());
                }
                if align != channels * (bits / 8) || bytes_per_second != rate * u32::from(align) {
                    return Err("WAV block alignment or byte rate is inconsistent".into());
                }
                format = Some((encoding, channels, rate, bits, align));
            }
            b"data" => {
                if data.is_some() {
                    return Err("WAV contains multiple audio chunks".into());
                }
                data = Some((start, length));
            }
            _ => {}
        }
        reader.seek(SeekFrom::Start(next)).map_err(io)?;
        position = next;
    }
    let (encoding, channels, rate, bits, align) = format.ok_or("WAV is missing its format")?;
    let (data_start, data_length) = data.ok_or("WAV is missing its audio")?;
    if data_length == 0 || data_length % u64::from(align) != 0 {
        return Err("WAV contains empty or incomplete audio frames".into());
    }
    let frames = data_length / u64::from(align);
    let state_len = HEADER_BYTES as u64 + name.len() as u64 + frames * 8;
    if state_len > MAX_PLUGIN_STATE_BYTES as u64 || name.len() > MAX_NAME_BYTES {
        return Err("Decoded sample exceeds the 64 MiB embedded sample limit".into());
    }
    let mut state = Vec::new();
    state
        .try_reserve_exact(state_len as usize)
        .map_err(|e| format!("Cannot allocate sample: {e}"))?;
    state.extend_from_slice(MAGIC);
    state.extend_from_slice(&rate.to_le_bytes());
    state.extend_from_slice(&(frames as u32).to_le_bytes());
    state.extend_from_slice(&(name.len() as u32).to_le_bytes());
    state.extend_from_slice(&0_u64.to_le_bytes());
    state.extend_from_slice(name.as_bytes());
    reader.seek(SeekFrom::Start(data_start)).map_err(io)?;
    let mut buffer = [0_u8; 8192];
    let mut remaining = frames;
    while remaining > 0 {
        let batch_frames = remaining.min((buffer.len() / usize::from(align)) as u64) as usize;
        let batch = &mut buffer[..batch_frames * usize::from(align)];
        reader.read_exact(batch).map_err(io)?;
        for frame in batch.chunks_exact(usize::from(align)) {
            let mut stereo = [0.0_f32; 2];
            for (channel, sample) in frame.chunks_exact(usize::from(bits / 8)).enumerate() {
                let value = match (encoding, bits) {
                    (1, 8) => (f64::from(sample[0]) - 128.0) / 128.0,
                    (1, 16) => f64::from(i16::from_le_bytes(sample.try_into().unwrap())) / 32768.0,
                    (1, 24) => {
                        f64::from(i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8)
                            / 8388608.0
                    }
                    (1, 32) => {
                        f64::from(i32::from_le_bytes(sample.try_into().unwrap())) / 2147483648.0
                    }
                    (3, 32) => f64::from(f32::from_le_bytes(sample.try_into().unwrap())),
                    (3, 64) => f64::from_le_bytes(sample.try_into().unwrap()),
                    _ => unreachable!(),
                };
                if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
                    return Err("Sample contains nonfinite or out-of-range audio".into());
                }
                stereo[channel] = value as f32;
            }
            if channels == 1 {
                stereo[1] = stereo[0];
            }
            state.extend_from_slice(&stereo[0].to_le_bytes());
            state.extend_from_slice(&stereo[1].to_le_bytes());
        }
        remaining -= batch_frames as u64;
    }
    seal_state(&mut state);
    Ok(state)
}

#[derive(Debug, Clone, Copy)]
struct SampleVoice {
    id: u64,
    key: u8,
    velocity: u8,
    position: f64,
    reverse: bool,
    entered_loop: bool,
    envelope: Envelope,
    filter: Filter,
}

impl SampleVoice {
    const SILENT: Self = Self {
        id: 0,
        key: 0,
        velocity: 0,
        position: 0.0,
        reverse: false,
        entered_loop: false,
        envelope: Envelope::SILENT,
        filter: Filter::EMPTY,
    };
}

#[derive(Debug)]
struct Sampler {
    params: Parameters,
    audio: Vec<Frame>,
    octaves: Vec<Vec<Frame>>,
    rate: u32,
    voices: [SampleVoice; VOICES],
    control_counter: u32,
    resampler: Resampler,
}

pub fn create_sampler(instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
    instance.validate().map_err(|e| e.to_string())?;
    Ok(Box::new(Sampler::new(instance)?))
}

impl Sampler {
    fn new(instance: &PluginInstance) -> Result<Self, String> {
        let (audio, rate) = if instance.state.is_empty() {
            (Vec::new(), SAMPLE_RATE)
        } else {
            let sample = header(&instance.state)?;
            if checksum(&instance.state) != sample.fingerprint {
                return Err("Sampler state checksum mismatch".into());
            }
            let mut audio = Vec::new();
            audio
                .try_reserve_exact(sample.frames)
                .map_err(|e| format!("Cannot allocate sample: {e}"))?;
            for frame in sample.audio.as_chunks::<8>().0 {
                audio.push(decode_frame(frame)?);
            }
            (audio, sample.rate)
        };
        let octaves = prepare_octaves(&audio)?;
        Ok(Self {
            params: Parameters::new(instance, sampler_parameters()),
            audio,
            octaves,
            rate,
            voices: [SampleVoice::SILENT; VOICES],
            control_counter: 0,
            resampler: Resampler::new(),
        })
    }
}

fn prepare_octaves(audio: &[Frame]) -> Result<Vec<Vec<Frame>>, String> {
    let mut kernel = [0.0; 33];
    for (tap, value) in kernel.iter_mut().enumerate() {
        let distance = tap as f64 - 16.0;
        let x = PI * distance * 0.45;
        let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
        *value = sinc * (0.5 + 0.5 * (PI * distance / 16.0).cos());
    }
    let total: f64 = kernel.iter().sum();
    for value in &mut kernel {
        *value /= total;
    }
    let mut levels: Vec<Vec<Frame>> = Vec::new();
    loop {
        let source = levels.last().map_or(audio, Vec::as_slice);
        if source.len() <= 1 {
            break;
        }
        let frames = source.len().div_ceil(2);
        let mut level = Vec::new();
        level
            .try_reserve_exact(frames)
            .map_err(|e| format!("Cannot prepare sample pitch levels: {e}"))?;
        for frame in 0..frames {
            let mut sum = [0.0; 2];
            for (tap, weight) in kernel.iter().enumerate() {
                let index =
                    (frame as i64 * 2 + tap as i64 - 16).clamp(0, source.len() as i64 - 1) as usize;
                for channel in 0..2 {
                    sum[channel] += f64::from(source[index][channel]) * weight;
                }
            }
            level.push([
                sum[0].clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32,
                sum[1].clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32,
            ]);
        }
        levels.push(level);
    }
    Ok(levels)
}

fn bounds(params: &Parameters, len: usize) -> (usize, usize, usize, usize) {
    let start = ((params.get(SAMPLE_START) * len as f64) as usize).min(len.saturating_sub(1));
    let end = ((params.get(SAMPLE_END) * len as f64) as usize).clamp(start + 1, len);
    let loop_start = ((params.get(SAMPLE_LOOP_START) * len as f64) as usize).clamp(start, end - 1);
    let loop_end = ((params.get(SAMPLE_LOOP_END) * len as f64) as usize).clamp(loop_start + 1, end);
    (start, end, loop_start, loop_end)
}

#[derive(Debug)]
struct Resampler {
    kernels: Vec<[f64; 16]>,
}

impl Resampler {
    fn new() -> Self {
        let mut kernels = Vec::with_capacity(9 * 129);
        for band in 0..9 {
            let cutoff = 0.95 * 2.0_f64.powi(-band);
            for phase in 0..=128 {
                let mut kernel = [0.0; 16];
                for (tap, value) in kernel.iter_mut().enumerate() {
                    let distance = f64::from(phase) / 128.0 - (tap as f64 - 7.0);
                    let x = PI * distance * cutoff;
                    let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
                    let window = 0.5 + 0.5 * (PI * distance / 8.0).cos();
                    *value = sinc * window * cutoff;
                }
                let sum: f64 = kernel.iter().sum();
                for value in &mut kernel {
                    *value /= sum;
                }
                kernels.push(kernel);
            }
        }
        Self { kernels }
    }

    fn interpolate_prepared(
        &self,
        sources: (&[Frame], &[Vec<Frame>]),
        position: f64,
        region: std::ops::Range<usize>,
        step: f64,
        wrap: bool,
    ) -> [f64; 2] {
        let level = (step.max(1.0).log2().floor() as usize).min(sources.1.len());
        let scale = 1_usize << level;
        let read_level = |level: usize| {
            let scale = 1_usize << level;
            let audio = if level == 0 {
                sources.0
            } else {
                &sources.1[level - 1]
            };
            let start = (region.start / scale).min(audio.len() - 1);
            let end = region.end.div_ceil(scale).clamp(start + 1, audio.len());
            self.interpolate(
                audio,
                position / scale as f64,
                start,
                end,
                step / scale as f64,
                wrap,
            )
        };
        let mut output = read_level(level);
        let blend = (step / scale as f64 - 1.0).clamp(0.0, 1.0);
        if blend > 0.0 && level < sources.1.len() {
            let next = read_level(level + 1);
            for channel in 0..2 {
                output[channel] += (next[channel] - output[channel]) * blend;
            }
        }
        output
    }

    fn interpolate(
        &self,
        audio: &[Frame],
        position: f64,
        start: usize,
        end: usize,
        step: f64,
        wrap: bool,
    ) -> [f64; 2] {
        let center = position.floor() as i64;
        let phase = position.fract() * 128.0;
        let phase_index = (phase as usize).min(127);
        let phase_mix = phase - phase_index as f64;
        let band = step.max(1.0).log2().clamp(0.0, 8.0);
        let band_index = (band as usize).min(7);
        let upper_step = 2.0_f64.powi(band_index as i32);
        let band_mix = (2.0 - 2.0 * upper_step / step.max(1.0)).clamp(0.0, 1.0);
        let offset = band_index * 129 + phase_index;
        let mut sum = [0.0; 2];
        for tap in 0..16 {
            let low = self.kernels[offset][tap] * (1.0 - phase_mix)
                + self.kernels[offset + 1][tap] * phase_mix;
            let high = self.kernels[offset + 129][tap] * (1.0 - phase_mix)
                + self.kernels[offset + 130][tap] * phase_mix;
            let weight = low * (1.0 - band_mix) + high * band_mix;
            let index = center + tap as i64 - 7;
            let index = if wrap {
                start as i64 + (index - start as i64).rem_euclid((end - start) as i64)
            } else {
                index.clamp(start as i64, end as i64 - 1)
            } as usize;
            sum[0] += f64::from(audio[index][0]) * weight;
            sum[1] += f64::from(audio[index][1]) * weight;
        }
        sum
    }
}

impl PluginProcessor for Sampler {
    fn reset(&mut self) {
        self.params.finish();
        self.voices.fill(SampleVoice::SILENT);
        self.control_counter = 0;
    }
    fn set_parameter(&mut self, id: u32, value: f64) {
        self.params.set(id, value, false);
    }
    fn set_automation_parameter(&mut self, id: u32, value: f64) {
        self.params.set(id, value, true);
    }
    fn event(&mut self, event: NoteEvent) {
        match event {
            NoteEvent::On {
                id, key, velocity, ..
            } => {
                if self.audio.is_empty() {
                    return;
                }
                let index = self
                    .voices
                    .iter()
                    .position(|v| v.envelope.stage != Stage::Silent && v.id == id)
                    .or_else(|| {
                        self.voices
                            .iter()
                            .position(|v| v.envelope.stage == Stage::Silent)
                    })
                    .unwrap_or_else(|| {
                        self.voices
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| {
                                let a_level = a.envelope.level
                                    + if a.envelope.stage == Stage::Release {
                                        0.0
                                    } else {
                                        2.0
                                    };
                                let b_level = b.envelope.level
                                    + if b.envelope.stage == Stage::Release {
                                        0.0
                                    } else {
                                        2.0
                                    };
                                a_level.total_cmp(&b_level)
                            })
                            .unwrap()
                            .0
                    });
                let (start, end, _, _) = bounds(&self.params, self.audio.len());
                let reverse = self.params.get(SAMPLE_REVERSE) > 0.5;
                self.voices[index] = SampleVoice {
                    id,
                    key,
                    velocity,
                    reverse,
                    position: if reverse {
                        (end - 1) as f64
                    } else {
                        start as f64
                    },
                    envelope: Envelope::trigger(),
                    ..SampleVoice::SILENT
                };
                self.control_counter = 0;
            }
            NoteEvent::Off { id } => {
                if self.params.get(SAMPLE_MODE) as u32 != 0 {
                    for voice in &mut self.voices {
                        if voice.id == id {
                            voice.envelope.off(self.params.envelope(ATTACK).release);
                        }
                    }
                }
            }
        }
    }
    fn process(&mut self, frames: &mut [Frame], _context: PluginProcessContext) {
        if self.audio.is_empty() {
            frames.fill([0.0; 2]);
            return;
        }
        for frame in frames {
            self.params.advance();
            let p = &self.params;
            let (start, end, loop_start, loop_end) = bounds(p, self.audio.len());
            let looping = p.get(SAMPLE_MODE) as u32 == 2;
            let crossfade = (p.get(SAMPLE_CROSSFADE) * f64::from(self.rate) / 1000.0)
                .min((loop_end - loop_start) as f64 * 0.5);
            let envelope = p.envelope(ATTACK);
            let ratio = f64::from(self.rate) / f64::from(SAMPLE_RATE);
            let transpose =
                p.get(SAMPLE_TRANSPOSE) - p.get(SAMPLE_ROOT) + p.get(SAMPLE_TUNE) / 100.0;
            let output_pan = balance(p.get(PAN));
            let gain = amplitude(p.get(GAIN));
            let mut mixed = [0.0; 2];
            for voice in &mut self.voices {
                if voice.envelope.stage == Stage::Silent {
                    continue;
                }
                voice.reverse = p.get(SAMPLE_REVERSE) > 0.5;
                let step = ratio * ((f64::from(voice.key) + transpose) / 12.0).exp2();
                if looping {
                    if voice.reverse && voice.position < loop_start as f64 {
                        let span = (loop_end - loop_start) as f64 - crossfade;
                        voice.position = loop_start as f64
                            + (voice.position - loop_start as f64).rem_euclid(span);
                        voice.entered_loop = true;
                    } else if !voice.reverse && voice.position >= loop_end as f64 {
                        let span = (loop_end - loop_start) as f64 - crossfade;
                        voice.position = loop_start as f64
                            + crossfade
                            + (voice.position - loop_end as f64).rem_euclid(span);
                        voice.entered_loop = true;
                    }
                }
                if voice.position < start as f64 || voice.position >= end as f64 {
                    *voice = SampleVoice::SILENT;
                    continue;
                }
                let can_wrap = looping && voice.entered_loop && crossfade < 1.0;
                let (read_start, read_end) = if can_wrap {
                    (loop_start, loop_end)
                } else {
                    (start, end)
                };
                let mut audio = self.resampler.interpolate_prepared(
                    (&self.audio, &self.octaves),
                    voice.position,
                    read_start..read_end,
                    step,
                    can_wrap,
                );
                if looping && crossfade >= 1.0 {
                    let fade = if voice.reverse {
                        ((loop_start as f64 + crossfade - voice.position) / crossfade)
                            .clamp(0.0, 1.0)
                    } else {
                        ((voice.position - (loop_end as f64 - crossfade)) / crossfade)
                            .clamp(0.0, 1.0)
                    };
                    if fade > 0.0 {
                        let other_position = if voice.reverse {
                            loop_end as f64 - crossfade + voice.position - loop_start as f64
                        } else {
                            loop_start as f64 + voice.position - (loop_end as f64 - crossfade)
                        };
                        let other = self.resampler.interpolate_prepared(
                            (&self.audio, &self.octaves),
                            other_position,
                            loop_start..loop_end,
                            step,
                            false,
                        );
                        for channel in 0..2 {
                            audio[channel] = audio[channel] * (1.0 - fade) + other[channel] * fade;
                        }
                    }
                }
                if self.control_counter == 0 {
                    voice
                        .filter
                        .coefficients(p.get(SAMPLE_CUTOFF), p.get(SAMPLE_RESONANCE));
                }
                let filtered = voice.filter.process(audio, 0);
                let wet = ((20000.0 - p.get(SAMPLE_CUTOFF)) / 2000.0).clamp(0.0, 1.0);
                for channel in 0..2 {
                    audio[channel] += (filtered[channel] - audio[channel]) * wet;
                }
                let edge_gain = if looping {
                    1.0
                } else {
                    let remaining = if voice.reverse {
                        voice.position - start as f64 + 1.0
                    } else {
                        end as f64 - voice.position
                    };
                    let fade_frames = ((end - start) as f64 / step * 0.25).clamp(1.0, 64.0);
                    (remaining / (step * fade_frames)).min(1.0)
                };
                let level = voice.envelope.next(envelope)
                    * velocity_gain(voice.velocity, p.get(VELOCITY))
                    * edge_gain;
                for channel in 0..2 {
                    mixed[channel] += audio[channel] * level;
                }
                voice.position += if voice.reverse { -step } else { step };
            }
            self.control_counter = (self.control_counter + 1) % 16;
            *frame = [
                (mixed[0] * gain * output_pan[0]).clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
                    as f32,
                (mixed[1] * gain * output_pan[1]).clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
                    as f32,
            ];
        }
    }
}

#[cfg(test)]
mod tests;
