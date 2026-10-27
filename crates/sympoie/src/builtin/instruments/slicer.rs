//! Slicer: one sample cut into regions, each played by its own MIDI key.
//!
//! The sample is held in the same state as the Sampler's. Slice starts are parameters, so
//! moving one is a live, undoable control change; a slice ends where the next later slice
//! starts, or at the end of the sample. Slice `n` (in parameter order) plays on key
//! `root + n`.

use std::sync::LazyLock;

use super::sampler::{Resampler, decode_audio, header, prepare_octaves};
use super::*;

pub const SLICE_ROOT: u32 = 20;
pub const SLICE_TRANSPOSE: u32 = 21;
pub const SLICE_TUNE: u32 = 22;
pub const SLICE_MODE: u32 = 23;
pub const SLICE_REVERSE: u32 = 24;
pub const SLICE_CHOKE: u32 = 25;
pub const SLICE_FADE: u32 = 26;
pub const SLICE_CUTOFF: u32 = 40;
pub const SLICE_RESONANCE: u32 = 41;
pub const SLICE_COUNT: u32 = 50;
pub const SLICE_DETECT: u32 = 51;
pub const SLICE_SENSITIVITY: u32 = 52;
pub const SLICE_MIN_LENGTH: u32 = 53;
pub const SLICE_DIVISIONS: u32 = 54;
/// Slice `n` starts at parameter `SLICE_BASE + n`, normalized over the whole sample.
pub const SLICE_BASE: u32 = 100;
pub const MAX_SLICES: usize = 64;
pub const SLICE_DETECTORS: [&str; 3] = ["Transient", "Volume", "Grid"];

/// How fast a choked slice fades out.
const CHOKE_SECONDS: f64 = 0.004;

static SLICE_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| {
    (1..=MAX_SLICES)
        .map(|slice| format!("Slice {slice} start"))
        .collect()
});

pub fn slicer_parameters() -> Vec<ParameterSpec> {
    let mut result = common_parameters(-6.0);
    for (id, default) in [(SUSTAIN, 1.0), (ATTACK, 0.0001), (RELEASE, 0.05)] {
        result.iter_mut().find(|p| p.id == id).unwrap().default = default;
    }
    result.extend([
        spec(
            SLICE_ROOT,
            "Root key",
            "MIDI",
            [0.0, 127.0, 36.0],
            Scale::Linear,
            127,
        ),
        spec(
            SLICE_TRANSPOSE,
            "Transpose",
            "st",
            [-48.0, 48.0, 0.0],
            Scale::Linear,
            96,
        ),
        spec(
            SLICE_TUNE,
            "Tune",
            "cent",
            [-100.0, 100.0, 0.0],
            Scale::Linear,
            0,
        ),
        spec(
            SLICE_MODE,
            "Playback",
            "",
            [0.0, 2.0, 0.0],
            Scale::Linear,
            2,
        ),
        spec(
            SLICE_REVERSE,
            "Reverse",
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            1,
        ),
        spec(SLICE_CHOKE, "Choke", "", [0.0, 1.0, 0.0], Scale::Linear, 1),
        spec(
            SLICE_FADE,
            "Slice fade",
            "ms",
            [0.0, 50.0, 1.0],
            Scale::Linear,
            0,
        ),
        spec(
            SLICE_CUTOFF,
            "Cutoff",
            "Hz",
            [20.0, 20000.0, 20000.0],
            Scale::Log,
            0,
        ),
        spec(
            SLICE_RESONANCE,
            "Resonance",
            "Q",
            [0.5, 12.0, 0.707],
            Scale::Log,
            0,
        ),
        spec(
            SLICE_COUNT,
            "Slices",
            "",
            [1.0, MAX_SLICES as f64, 1.0],
            Scale::Linear,
            MAX_SLICES as u32 - 1,
        ),
        spec(
            SLICE_DETECT,
            "Detect",
            "",
            [0.0, 2.0, 0.0],
            Scale::Linear,
            2,
        ),
        spec(
            SLICE_SENSITIVITY,
            "Sensitivity",
            "%",
            [0.0, 100.0, 50.0],
            Scale::Linear,
            0,
        ),
        spec(
            SLICE_MIN_LENGTH,
            "Min slice",
            "ms",
            [10.0, 2000.0, 80.0],
            Scale::Log,
            0,
        ),
        spec(
            SLICE_DIVISIONS,
            "Divisions",
            "",
            [2.0, MAX_SLICES as f64, 8.0],
            Scale::Linear,
            MAX_SLICES as u32 - 2,
        ),
    ]);
    result.extend((0..MAX_SLICES).map(|slice| {
        spec(
            SLICE_BASE + slice as u32,
            SLICE_NAMES[slice].as_str(),
            "",
            [0.0, 1.0, 0.0],
            Scale::Linear,
            0,
        )
    }));
    result
}

/// Each slice's normalized `(start, end)` in key order: a slice ends where the next later
/// slice starts, or at the end of the sample.
pub fn slice_regions(starts: &[f64]) -> Vec<(f64, f64)> {
    starts
        .iter()
        .map(|&start| (start, region_end(start, starts.iter().copied())))
        .collect()
}

fn region_end(start: f64, starts: impl Iterator<Item = f64>) -> f64 {
    starts.filter(|&other| other > start).fold(1.0, f64::min)
}

/// The normalized start of every active slice of a Slicer, in key order.
pub fn slice_starts(instance: &PluginInstance) -> Vec<f64> {
    let specs = slicer_parameters();
    let value = |id: u32| {
        let spec = specs.iter().find(|spec| spec.id == id).unwrap();
        instance
            .parameters
            .iter()
            .find(|p| p.id == id)
            .map_or(spec.default, |p| spec.denormalize(p.value))
    };
    let count = (value(SLICE_COUNT).round() as usize).clamp(1, MAX_SLICES);
    (0..count)
        .map(|slice| value(SLICE_BASE + slice as u32))
        .collect()
}

/// How [`detect_slices`] finds slice starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SliceDetection {
    /// Sudden rises in level: drum hits, plucks, note attacks.
    Transient,
    /// Sound rising out of quiet: phrases or one-shots separated by silence.
    Volume,
    /// Equal divisions of the whole sample.
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceSettings {
    pub detection: SliceDetection,
    /// 0 finds only the clearest slices, 1 finds the faintest.
    pub sensitivity: f64,
    /// Detected slices start at least this far apart.
    pub min_length_seconds: f64,
    /// Number of slices of a [`SliceDetection::Grid`].
    pub divisions: usize,
}

impl SliceSettings {
    /// The detection settings stored on a Slicer.
    pub fn of(instance: &PluginInstance) -> Self {
        let specs = slicer_parameters();
        let value = |id: u32| {
            let spec = specs.iter().find(|spec| spec.id == id).unwrap();
            instance
                .parameters
                .iter()
                .find(|p| p.id == id)
                .map_or(spec.default, |p| spec.denormalize(p.value))
        };
        Self {
            detection: match value(SLICE_DETECT).round() as u32 {
                0 => SliceDetection::Transient,
                1 => SliceDetection::Volume,
                _ => SliceDetection::Grid,
            },
            sensitivity: value(SLICE_SENSITIVITY) / 100.0,
            min_length_seconds: value(SLICE_MIN_LENGTH) / 1000.0,
            divisions: value(SLICE_DIVISIONS).round() as usize,
        }
    }
}

/// Analyzes the sample in Slicer (or Sampler) state, returning normalized slice starts in
/// ascending order. There is always at least one slice.
pub fn analyze_slices(state: &[u8], settings: SliceSettings) -> Result<Vec<f64>, String> {
    if state.is_empty() {
        return Err("Load a sample before slicing".into());
    }
    let (audio, rate) = decode_audio(state)?;
    let mono: Vec<f32> = audio
        .iter()
        .map(|frame| 0.5 * (frame[0] + frame[1]))
        .collect();
    let length = mono.len() as f64;
    Ok(detect_slices(&mono, rate, settings)
        .into_iter()
        .map(|frame| frame as f64 / length)
        .collect())
}

/// Finds slice starts in `audio` as frame positions in ascending order, at most
/// [`MAX_SLICES`] of them and never none.
pub fn detect_slices(audio: &[f32], rate: u32, settings: SliceSettings) -> Vec<usize> {
    if audio.is_empty() {
        return vec![0];
    }
    let sensitivity = settings.sensitivity.clamp(0.0, 1.0);
    if settings.detection == SliceDetection::Grid {
        let divisions = settings.divisions.clamp(1, MAX_SLICES).min(audio.len());
        return (0..divisions)
            .map(|slice| slice * audio.len() / divisions)
            .collect();
    }
    let hop = (rate as usize / 200).max(32);
    let window = 2 * hop;
    let frames = audio.len().div_ceil(hop);
    let level = |weight: &dyn Fn(usize) -> f32| -> Vec<f64> {
        (0..frames)
            .map(|frame| {
                let range = frame * hop..(frame * hop + window).min(audio.len());
                let count = range.len().max(1) as f64;
                let energy: f64 = range
                    .map(|index| f64::from(weight(index)).powi(2))
                    .sum::<f64>()
                    / count;
                10.0 * (energy + 1e-12).log10()
            })
            .collect()
    };
    let broadband = level(&|index| audio[index]);
    // Pre-emphasis favours the bright edge of an attack over sustained low end.
    let bright = level(&|index| audio[index] - 0.85 * audio[index.saturating_sub(1)]);
    let peak = broadband.iter().copied().fold(f64::MIN, f64::max);
    let gate = peak - (40.0 + 30.0 * sensitivity);
    let min_frames = ((settings.min_length_seconds.max(0.0) * f64::from(rate)) as usize).max(1);

    // (frame index of the analysis window, strength)
    let mut onsets: Vec<(usize, f64)> = Vec::new();
    let accept =
        |onsets: &mut Vec<(usize, f64)>, frame: usize, strength: f64, replace: bool| match onsets
            .last_mut()
        {
            Some(last) if (frame - last.0) * hop < min_frames => {
                if replace && strength > last.1 {
                    *last = (frame, strength);
                }
            }
            _ => onsets.push((frame, strength)),
        };
    match settings.detection {
        SliceDetection::Transient => {
            let threshold = 15.0 - 12.0 * sensitivity;
            let rise = |frame: usize| {
                if frame == 0 {
                    return f64::MIN;
                }
                let before = bright[frame - 1].min(bright[frame.saturating_sub(2)]);
                bright[frame] - before
            };
            for (frame, &level) in broadband.iter().enumerate().skip(1) {
                let strength = rise(frame);
                if strength >= threshold
                    && level >= gate
                    && strength >= rise(frame - 1)
                    && (frame + 1 >= frames || strength >= rise(frame + 1))
                {
                    accept(&mut onsets, frame, strength, true);
                }
            }
        }
        SliceDetection::Volume => {
            let threshold = peak - (12.0 + 36.0 * sensitivity);
            let mut quiet = true;
            for (frame, &level) in broadband.iter().enumerate() {
                if quiet && level >= threshold {
                    accept(&mut onsets, frame, level, false);
                    quiet = false;
                } else if !quiet && level < threshold - 6.0 {
                    quiet = true;
                }
            }
        }
        SliceDetection::Grid => unreachable!(),
    }
    if onsets.len() > MAX_SLICES {
        onsets.sort_by(|a, b| b.1.total_cmp(&a.1));
        onsets.truncate(MAX_SLICES);
        onsets.sort_by_key(|onset| onset.0);
    }
    let mut starts: Vec<usize> = onsets
        .iter()
        .map(|&(frame, _)| {
            attack_start(
                audio,
                rate,
                frame * hop,
                (frame * hop + 3 * hop).min(audio.len()),
            )
        })
        .collect();
    starts.dedup_by(|later, earlier| *later <= *earlier || *later - *earlier < hop);
    // Sound before the first transient would otherwise be unreachable.
    let audible_before = |start: usize| {
        broadband
            .iter()
            .enumerate()
            .any(|(frame, &level)| frame * hop + window <= start && level >= gate)
    };
    match starts.first() {
        None => starts.push(0),
        Some(&first)
            if first > 0
                && settings.detection == SliceDetection::Transient
                && audible_before(first) =>
        {
            if first < min_frames {
                starts[0] = 0;
            } else {
                starts.insert(0, 0);
                starts.truncate(MAX_SLICES);
            }
        }
        _ => {}
    }
    starts
}

/// Where the attack whose loudest sample lies in `from..to` begins: walks back from that
/// peak while the recent level stays above a tenth of it, then settles on the quietest
/// sample just before, so the slice starts cleanly.
fn attack_start(audio: &[f32], rate: u32, from: usize, to: usize) -> usize {
    if from >= to {
        return from.min(audio.len().saturating_sub(1));
    }
    let (peak_index, peak) = audio[from..to]
        .iter()
        .enumerate()
        .map(|(offset, sample)| (from + offset, sample.abs()))
        .fold(
            (from, 0.0_f32),
            |best, item| if item.1 > best.1 { item } else { best },
        );
    let span = (rate as usize / 1000).max(1);
    let lowest = from.saturating_sub(to - from);
    let recent = |index: usize| {
        audio[index.saturating_sub(span)..=index]
            .iter()
            .fold(0.0_f32, |level, sample| level.max(sample.abs()))
    };
    let mut start = peak_index;
    while start > lowest && recent(start - 1) > 0.1 * peak {
        start -= 1;
    }
    let search = start.saturating_sub(span)..=start;
    search
        .rev()
        .min_by(|&a, &b| audio[a].abs().total_cmp(&audio[b].abs()))
        .unwrap_or(start)
}

/// Frames a note on `key` keeps sounding after its note-off: a one-shot slice plays to
/// its end, however short the note.
pub fn slicer_note_tail_frames_with_automation(
    instance: &PluginInstance,
    key: u8,
    lanes: &[crate::automation::AutomationLane],
) -> Result<u64, String> {
    use crate::automation::AutomationTarget;
    if instance.state.is_empty() {
        return Ok(0);
    }
    let sample = header(&instance.state)?;
    let specs = slicer_parameters();
    let get = |id: u32, largest: bool| {
        let spec = specs.iter().find(|spec| spec.id == id).unwrap();
        let base = instance.parameters.iter().find(|p| p.id == id);
        let saved = base.map_or(spec.default, |p| spec.denormalize(p.value));
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
            .flat_map(|lane| {
                lane.trimmed_values(base.map_or(lane.anchor().unwrap_or(0.0), |p| p.value))
            })
            .map(|value| spec.denormalize(value))
            .fold(saved, |a, b| if largest { a.max(b) } else { a.min(b) })
    };
    if get(SLICE_MODE, false).round() != 0.0 {
        return Ok(0);
    }
    let starts = slice_starts(instance);
    let slice = i32::from(key) - get(SLICE_ROOT, false).round() as i32;
    let Some((start, end)) = usize::try_from(slice)
        .ok()
        .and_then(|slice| slice_regions(&starts).get(slice).copied())
    else {
        return Ok(0);
    };
    let pitch = get(SLICE_TRANSPOSE, false) + get(SLICE_TUNE, false) / 100.0;
    let step = (f64::from(sample.rate) / f64::from(SAMPLE_RATE)) * (pitch / 12.0).exp2();
    let length = ((end - start) * sample.frames as f64).max(1.0);
    Ok((length / step).ceil() as u64)
}

#[derive(Debug, Clone, Copy)]
struct SliceVoice {
    id: u64,
    velocity: u8,
    position: f64,
    start: usize,
    end: usize,
    envelope: Envelope,
    filter: Filter,
}

impl SliceVoice {
    const SILENT: Self = Self {
        id: 0,
        velocity: 0,
        position: 0.0,
        start: 0,
        end: 0,
        envelope: Envelope::SILENT,
        filter: Filter::EMPTY,
    };
}

#[derive(Debug)]
struct Slicer {
    params: Parameters,
    audio: Vec<Frame>,
    octaves: Vec<Vec<Frame>>,
    rate: u32,
    voices: [SliceVoice; VOICES],
    control_counter: u32,
    resampler: Resampler,
}

pub fn create_slicer(instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
    instance.validate().map_err(|e| e.to_string())?;
    let (audio, rate) = decode_audio(&instance.state)?;
    let octaves = prepare_octaves(&audio)?;
    Ok(Box::new(Slicer {
        params: Parameters::new(instance, slicer_parameters()),
        audio,
        octaves,
        rate,
        voices: [SliceVoice::SILENT; VOICES],
        control_counter: 0,
        resampler: Resampler::new(),
    }))
}

impl Slicer {
    /// The frames slice `slice` plays, of `count` active slices.
    fn region(&self, slice: usize, count: usize) -> (usize, usize) {
        let length = self.audio.len();
        let start = self.params.get(SLICE_BASE + slice as u32);
        let end = region_end(
            start,
            (0..count).map(|other| self.params.get(SLICE_BASE + other as u32)),
        );
        let start = ((start * length as f64) as usize).min(length - 1);
        let end = ((end * length as f64) as usize).clamp(start + 1, length);
        (start, end)
    }

    fn free_voice(&self, id: u64) -> usize {
        self.voices
            .iter()
            .position(|v| v.envelope.stage != Stage::Silent && v.id == id)
            .or_else(|| {
                self.voices
                    .iter()
                    .position(|v| v.envelope.stage == Stage::Silent)
            })
            .unwrap_or_else(|| {
                let weight = |voice: &SliceVoice| {
                    voice.envelope.level
                        + if voice.envelope.stage == Stage::Release {
                            0.0
                        } else {
                            2.0
                        }
                };
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| weight(a).total_cmp(&weight(b)))
                    .unwrap()
                    .0
            })
    }
}

impl PluginProcessor for Slicer {
    fn reset(&mut self) {
        self.params.finish();
        self.voices.fill(SliceVoice::SILENT);
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
            NoteEvent::PitchBend { .. } => {}
            NoteEvent::On {
                id, key, velocity, ..
            } => {
                if self.audio.is_empty() {
                    return;
                }
                let count = (self.params.get(SLICE_COUNT).round() as usize).clamp(1, MAX_SLICES);
                let slice = i32::from(key) - self.params.get(SLICE_ROOT).round() as i32;
                let Some(slice) = usize::try_from(slice).ok().filter(|&slice| slice < count) else {
                    return;
                };
                if self.params.get(SLICE_CHOKE) > 0.5 {
                    for voice in &mut self.voices {
                        voice.envelope.choke(CHOKE_SECONDS);
                    }
                }
                let (start, end) = self.region(slice, count);
                let index = self.free_voice(id);
                let reverse = self.params.get(SLICE_REVERSE) > 0.5;
                self.voices[index] = SliceVoice {
                    id,
                    velocity,
                    position: if reverse {
                        (end - 1) as f64
                    } else {
                        start as f64
                    },
                    start,
                    end,
                    envelope: Envelope::trigger(),
                    filter: Filter::EMPTY,
                };
                self.control_counter = 0;
            }
            NoteEvent::Off { id } => {
                if self.params.get(SLICE_MODE).round() as u32 != 0 {
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
            let looping = p.get(SLICE_MODE).round() as u32 == 2;
            let reverse = p.get(SLICE_REVERSE) > 0.5;
            let envelope = p.envelope(ATTACK);
            let step = f64::from(self.rate) / f64::from(SAMPLE_RATE)
                * ((p.get(SLICE_TRANSPOSE) + p.get(SLICE_TUNE) / 100.0) / 12.0).exp2();
            let fade = p.get(SLICE_FADE) * f64::from(self.rate) / 1000.0;
            let output_pan = balance(p.get(PAN));
            let gain = amplitude(p.get(GAIN));
            let mut mixed = [0.0; 2];
            for voice in &mut self.voices {
                if voice.envelope.stage == Stage::Silent {
                    continue;
                }
                let (start, end) = (voice.start as f64, voice.end as f64);
                if looping && (voice.position < start || voice.position >= end) {
                    voice.position = start + (voice.position - start).rem_euclid(end - start);
                }
                if voice.position < start || voice.position >= end {
                    *voice = SliceVoice::SILENT;
                    continue;
                }
                let mut audio = self.resampler.interpolate_prepared(
                    (&self.audio, &self.octaves),
                    voice.position,
                    voice.start..voice.end,
                    step,
                    looping,
                );
                if self.control_counter == 0 {
                    voice
                        .filter
                        .coefficients(p.get(SLICE_CUTOFF), p.get(SLICE_RESONANCE));
                }
                let filtered = voice.filter.process(audio, 0);
                let wet = ((20000.0 - p.get(SLICE_CUTOFF)) / 2000.0).clamp(0.0, 1.0);
                for channel in 0..2 {
                    audio[channel] += (filtered[channel] - audio[channel]) * wet;
                }
                let fade = fade.min((end - start) * 0.25);
                let edge_gain = if fade >= 1.0 {
                    ((voice.position - start + 1.0) / fade).min(1.0)
                        * ((end - voice.position) / fade).min(1.0)
                } else {
                    1.0
                };
                let level = voice.envelope.next(envelope)
                    * velocity_gain(voice.velocity, p.get(VELOCITY))
                    * edge_gain;
                for channel in 0..2 {
                    mixed[channel] += audio[channel] * level;
                }
                voice.position += if reverse { -step } else { step };
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
