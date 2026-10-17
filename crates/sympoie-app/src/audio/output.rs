use std::sync::Arc;

use sympoie::{Frame, SAMPLE_RATE};

use super::engine::{Shared, SilentClock};
use crate::settings::AudioSettings;

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
const SCRATCH_FRAMES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    pub description: String,
    pub detail: String,
    pub sample_rate: u32,
    pub is_device: bool,
    pub buffer_frames: Option<u32>,
    pub channels: u16,
    pub first_channel: u16,
}

pub const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioCatalog {
    pub drivers: Vec<DriverInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverInfo {
    pub name: String,
    pub default_device: Option<String>,
    pub devices: Vec<DeviceInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub sample_rates: Vec<u32>,
    pub current_rate: Option<u32>,
    pub max_channels: u16,
    pub buffer_range: Option<(u32, u32)>,
}

impl AudioCatalog {
    pub fn driver(&self, name: Option<&str>) -> Option<&DriverInfo> {
        match name {
            Some(name) => self.drivers.iter().find(|driver| driver.name == name),
            None => self.drivers.iter().find(|driver| driver.error.is_none()),
        }
    }
}

impl DriverInfo {
    pub fn device(&self, name: Option<&str>) -> Option<&DeviceInfo> {
        let name = name.or(self.default_device.as_deref())?;
        self.devices.iter().find(|device| device.name == name)
    }
}

pub fn catalog() -> AudioCatalog {
    #[cfg(feature = "audio-device")]
    {
        device::catalog()
    }
    #[cfg(not(feature = "audio-device"))]
    {
        AudioCatalog::default()
    }
}

pub enum Output {
    #[cfg(feature = "audio-device")]
    Device {
        _stream: cpal::Stream,
        info: OutputInfo,
        errors: Arc<std::sync::Mutex<Option<String>>>,
    },
    Silent {
        _clock: SilentClock,
        info: OutputInfo,
    },
}

impl Output {
    pub fn open(shared: Arc<Shared>, use_device: bool) -> Self {
        Self::open_with(shared, use_device, &AudioSettings::default())
    }

    pub fn open_with(shared: Arc<Shared>, use_device: bool, settings: &AudioSettings) -> Self {
        #[cfg(feature = "audio-device")]
        if use_device {
            match device::open(Arc::clone(&shared), settings) {
                Ok(output) => return output,
                Err(reason) => return Self::silent(shared, reason),
            }
        }
        #[cfg(not(feature = "audio-device"))]
        let _ = settings;
        let reason = if use_device {
            "This build has no audio output (the audio-device feature is off)"
        } else {
            "Audio output was disabled with SYMPOIE_NO_AUDIO"
        };
        Self::silent(shared, reason.to_owned())
    }

    fn silent(shared: Arc<Shared>, reason: String) -> Self {
        Self::Silent {
            _clock: SilentClock::start(shared),
            info: OutputInfo {
                description: "No audio output".to_owned(),
                detail: reason,
                sample_rate: SAMPLE_RATE,
                is_device: false,
                buffer_frames: None,
                channels: 2,
                first_channel: 0,
            },
        }
    }

    pub fn info(&self) -> &OutputInfo {
        match self {
            #[cfg(feature = "audio-device")]
            Self::Device { info, .. } => info,
            Self::Silent { info, .. } => info,
        }
    }

    pub fn device_error(&self) -> Option<String> {
        match self {
            #[cfg(feature = "audio-device")]
            Self::Device { errors, .. } => errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            Self::Silent { .. } => None,
        }
    }
}

const RAMP_IN_FRAMES: usize = SAMPLE_RATE as usize / 200;
const HOLD_FRAMES: usize = SAMPLE_RATE as usize / 100;

#[derive(Debug, Clone)]
struct Declicker {
    last: Frame,
    hold: Frame,
    hold_left: usize,
    ramp: usize,
    silent: bool,
}

impl Default for Declicker {
    fn default() -> Self {
        Self {
            last: [0.0; 2],
            hold: [0.0; 2],
            hold_left: 0,
            ramp: RAMP_IN_FRAMES,
            silent: true,
        }
    }
}

impl Declicker {
    fn cut(&mut self) {
        self.hold = self.last;
        self.hold_left = HOLD_FRAMES;
        self.ramp = 0;
    }

    fn process(&mut self, frames: &mut [Frame], real: usize, skipped: bool) {
        for (index, frame) in frames.iter_mut().enumerate() {
            let is_real = index < real;
            if is_real {
                if self.silent || (index == 0 && skipped) {
                    self.cut();
                    self.silent = false;
                }
            } else {
                if !self.silent {
                    self.cut();
                    self.silent = true;
                }
                *frame = [0.0; 2];
            }
            let fade_in = if self.ramp < RAMP_IN_FRAMES {
                (self.ramp + 1) as f32 / RAMP_IN_FRAMES as f32
            } else {
                1.0
            };
            self.ramp = self.ramp.saturating_add(1);
            let fade_out = self.hold_left as f32 / HOLD_FRAMES as f32;
            self.hold_left = self.hold_left.saturating_sub(1);
            for (sample, held) in frame.iter_mut().zip(self.hold) {
                *sample = *sample * fade_in + held * fade_out;
            }
            self.last = *frame;
        }
    }
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
pub struct Feeder {
    shared: Arc<Shared>,
    declick: Declicker,
    step: f64,
    phase: f64,
    previous: Frame,
    next: Frame,
    scratch: Vec<Frame>,
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
impl Feeder {
    pub fn new(shared: Arc<Shared>, device_rate: u32) -> Self {
        Self {
            shared,
            declick: Declicker::default(),
            step: f64::from(SAMPLE_RATE) / f64::from(device_rate.max(1)),
            phase: 0.0,
            previous: [0.0; 2],
            next: [0.0; 2],
            scratch: vec![[0.0; 2]; SCRATCH_FRAMES],
        }
    }

    pub fn render(&mut self, frames: usize, mut write: impl FnMut(usize, Frame)) {
        if self.step == 1.0 {
            let mut done = 0;
            while done < frames {
                let count = (frames - done).min(SCRATCH_FRAMES);
                let (real, skipped) = self.shared.consume_marking_cuts(&mut self.scratch[..count]);
                self.declick
                    .process(&mut self.scratch[..count], real, skipped);
                for (offset, frame) in self.scratch[..count].iter().enumerate() {
                    write(done + offset, safe(*frame));
                }
                done += count;
            }
            return;
        }
        let mut done = 0;
        while done < frames {
            let mut count = (frames - done).min(SCRATCH_FRAMES / 2);
            let mut needed = (self.phase + count as f64 * self.step).floor() as usize;
            while needed > SCRATCH_FRAMES {
                count /= 2;
                needed = (self.phase + count as f64 * self.step).floor() as usize;
            }
            if needed > 0 {
                let (real, skipped) = self
                    .shared
                    .consume_marking_cuts(&mut self.scratch[..needed]);
                self.declick
                    .process(&mut self.scratch[..needed], real, skipped);
            }
            let mut source = self.scratch[..needed].iter();
            for offset in 0..count {
                let t = self.phase as f32;
                let frame = [
                    self.previous[0] + (self.next[0] - self.previous[0]) * t,
                    self.previous[1] + (self.next[1] - self.previous[1]) * t,
                ];
                write(done + offset, safe(frame));
                self.phase += self.step;
                while self.phase >= 1.0 {
                    self.phase -= 1.0;
                    self.previous = self.next;
                    self.next = source.next().copied().unwrap_or([0.0; 2]);
                }
            }
            done += count;
        }
    }
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
fn safe(frame: Frame) -> Frame {
    frame.map(|sample| {
        if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    })
}

#[cfg(feature = "audio-device")]
mod device {
    use std::sync::{Arc, Mutex, PoisonError};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};

    use super::{
        AudioCatalog, AudioSettings, DeviceInfo, DriverInfo, Feeder, Output, OutputInfo,
        SAMPLE_RATE, STANDARD_RATES, Shared,
    };

    fn device_name(device: &cpal::Device) -> Option<String> {
        device
            .description()
            .ok()
            .map(|description| description.name().to_owned())
    }

    pub(super) fn catalog() -> AudioCatalog {
        let mut drivers = Vec::new();
        for id in cpal::available_hosts() {
            let name = id.name().to_owned();
            let host = match cpal::host_from_id(id) {
                Ok(host) => host,
                Err(error) => {
                    drivers.push(DriverInfo {
                        name,
                        default_device: None,
                        devices: Vec::new(),
                        error: Some(error.to_string()),
                    });
                    continue;
                }
            };
            let default_device = host.default_output_device().as_ref().and_then(device_name);
            let (devices, error) = match host.output_devices() {
                Ok(devices) => (
                    devices
                        .filter_map(|device| {
                            let name = device_name(&device)?;
                            Some(describe(&device, name))
                        })
                        .collect(),
                    None,
                ),
                Err(error) => (Vec::new(), Some(error.to_string())),
            };
            drivers.push(DriverInfo {
                name,
                default_device,
                devices,
                error,
            });
        }
        AudioCatalog { drivers }
    }

    fn describe(device: &cpal::Device, name: String) -> DeviceInfo {
        let ranges: Vec<_> = device
            .supported_output_configs()
            .map(|ranges| ranges.collect())
            .unwrap_or_default();
        let sample_rates = STANDARD_RATES
            .into_iter()
            .filter(|rate| {
                ranges.iter().any(|range| {
                    range.min_sample_rate() <= *rate && *rate <= range.max_sample_rate()
                })
            })
            .collect();
        let max_channels = ranges
            .iter()
            .map(|range| range.channels())
            .max()
            .unwrap_or(2);
        let buffer_range = ranges.iter().find_map(|range| match range.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
            cpal::SupportedBufferSize::Unknown => None,
        });
        DeviceInfo {
            name,
            sample_rates,
            current_rate: device
                .default_output_config()
                .ok()
                .map(|config| config.sample_rate()),
            max_channels,
            buffer_range,
        }
    }

    fn choose_device(
        settings: &AudioSettings,
        notes: &mut Vec<String>,
    ) -> Result<cpal::Device, String> {
        let mut ids = cpal::available_hosts();
        if let Some(wanted) = &settings.driver {
            match ids.iter().position(|id| id.name() == wanted) {
                Some(index) => {
                    let id = ids.remove(index);
                    ids.insert(0, id);
                }
                None => notes.push(format!(
                    "{wanted} is not available; using the system driver"
                )),
            }
        }
        let mut reason = "no audio host is available".to_owned();
        let mut hosts = Vec::new();
        for id in ids {
            match cpal::host_from_id(id) {
                Ok(host) => hosts.push(host),
                Err(error) => reason = error.to_string(),
            }
        }
        if let Some(wanted) = &settings.output_device {
            let searched = if settings.driver.is_some() {
                &hosts[..hosts.len().min(1)]
            } else {
                &hosts[..]
            };
            for host in searched {
                if let Some(device) = host.output_devices().ok().and_then(|mut devices| {
                    devices.find(|device| device_name(device).as_deref() == Some(wanted.as_str()))
                }) {
                    return Ok(device);
                }
            }
            notes.push(format!(
                "{wanted} is not connected; using the default output"
            ));
        }
        for host in &hosts {
            match host.default_output_device() {
                Some(device) => return Ok(device),
                None => reason = format!("{} has no default output device", host.id().name()),
            }
        }
        Err(reason)
    }

    pub(super) fn open(shared: Arc<Shared>, settings: &AudioSettings) -> Result<Output, String> {
        let mut notes = Vec::new();
        let device = choose_device(settings, &mut notes)?;
        let name = device_name(&device).unwrap_or_else(|| "Default output".to_owned());
        let mut failures = Vec::new();
        for supported in candidate_configs(&device, settings, &mut notes) {
            let format = supported.sample_format();
            let mut config = supported.config();
            let mut buffer_frames = None;
            if let Some(frames) = settings.buffer_frames {
                let fits = match supported.buffer_size() {
                    cpal::SupportedBufferSize::Range { min, max } => {
                        (*min..=*max).contains(&frames)
                    }
                    cpal::SupportedBufferSize::Unknown => true,
                };
                if fits {
                    config.buffer_size = cpal::BufferSize::Fixed(frames);
                    buffer_frames = Some(frames);
                } else if !notes.iter().any(|note| note.contains("buffer")) {
                    notes.push(format!(
                        "a {frames}-frame buffer is not supported; using the device's own"
                    ));
                }
            }
            let mut first = settings.first_output_channel;
            if u32::from(first) + 2 > u32::from(config.channels) {
                if first > 0 {
                    notes.push(format!(
                        "outputs {}/{} do not exist; using 1/2",
                        first + 1,
                        first + 2
                    ));
                }
                first = 0;
            }
            let errors = Arc::new(Mutex::new(None));
            let shared = Arc::clone(&shared);
            let stream = match format {
                SampleFormat::F32 => build::<f32>(&device, config, first, shared, &errors),
                SampleFormat::F64 => build::<f64>(&device, config, first, shared, &errors),
                SampleFormat::I16 => build::<i16>(&device, config, first, shared, &errors),
                SampleFormat::I32 => build::<i32>(&device, config, first, shared, &errors),
                SampleFormat::U16 => build::<u16>(&device, config, first, shared, &errors),
                SampleFormat::U8 => build::<u8>(&device, config, first, shared, &errors),
                SampleFormat::I8 => build::<i8>(&device, config, first, shared, &errors),
                other => {
                    failures.push(format!("unsupported sample format {other}"));
                    continue;
                }
            };
            let stream = match stream.and_then(|stream| stream.play().map(|()| stream)) {
                Ok(stream) => stream,
                Err(error) => {
                    failures.push(error.to_string());
                    continue;
                }
            };
            let mut detail = if config.sample_rate == SAMPLE_RATE {
                format!("{} channels, {format}", config.channels)
            } else {
                format!(
                    "{} channels, {format}; resampled from {} Hz",
                    config.channels, SAMPLE_RATE
                )
            };
            for note in notes {
                detail.push('\n');
                detail.push_str(&note);
            }
            return Ok(Output::Device {
                _stream: stream,
                info: OutputInfo {
                    description: name,
                    detail,
                    sample_rate: config.sample_rate,
                    is_device: true,
                    buffer_frames,
                    channels: config.channels,
                    first_channel: first,
                },
                errors,
            });
        }
        Err(failures
            .pop()
            .unwrap_or_else(|| format!("{name} offers no usable output format")))
    }

    fn candidate_configs(
        device: &cpal::Device,
        settings: &AudioSettings,
        notes: &mut Vec<String>,
    ) -> Vec<cpal::SupportedStreamConfig> {
        let rank = |format: SampleFormat| match format {
            SampleFormat::F32 => 0,
            SampleFormat::I32 | SampleFormat::F64 => 1,
            SampleFormat::I16 => 2,
            _ => 3,
        };
        let needed = settings.first_output_channel.saturating_add(2);
        let ranges: Vec<_> = device
            .supported_output_configs()
            .map(|ranges| ranges.collect())
            .unwrap_or_default();
        let at_rate = |rate: u32| -> Vec<cpal::SupportedStreamConfig> {
            let mut configs: Vec<_> = ranges
                .iter()
                .cloned()
                .filter_map(|range| range.try_with_sample_rate(rate))
                .collect();
            configs.sort_by_key(|config| {
                (
                    u16::from(config.channels() < needed),
                    u16::from(config.channels() < 2),
                    config.channels(),
                    rank(config.sample_format()),
                )
            });
            configs
        };
        let mut candidates = Vec::new();
        if let Some(rate) = settings.sample_rate {
            candidates.extend(at_rate(rate));
            if candidates.is_empty() {
                notes.push(format!(
                    "{:.1} kHz is not supported; using the device's rate",
                    rate as f32 / 1000.0
                ));
            }
        }
        if let Ok(current) = device.default_output_config()
            && (settings.first_output_channel == 0 || current.channels() >= needed)
        {
            candidates.push(current);
        }
        candidates.extend(at_rate(SAMPLE_RATE));
        if let Ok(current) = device.default_output_config() {
            candidates.push(current);
        }
        candidates
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        first: u16,
        shared: Arc<Shared>,
        errors: &Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = usize::from(config.channels.max(1));
        let first = usize::from(first);
        let mut feeder = Feeder::new(shared, config.sample_rate);
        let error_slot = Arc::clone(errors);
        device.build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
                let frames = data.len() / channels;
                feeder.render(frames, |index, frame| {
                    let samples = &mut data[index * channels..(index + 1) * channels];
                    if channels == 1 {
                        samples[0] = T::from_sample(0.5 * (frame[0] + frame[1]));
                        return;
                    }
                    for sample in samples.iter_mut() {
                        *sample = T::EQUILIBRIUM;
                    }
                    samples[first] = T::from_sample(frame[0]);
                    samples[first + 1] = T::from_sample(frame[1]);
                });
            },
            move |error| {
                *error_slot.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(error.to_string());
            },
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::engine::AudioEngine;
    use std::time::Duration;
    use sympoie::clip::AudioClip;
    use sympoie::project::{AudioBuffer, Project};
    use sympoie::transport::{PlaybackState, TransportCommand};

    fn constant_project(level: f32) -> Project {
        let mut project = Project::new();
        let asset = project.add_asset(AudioBuffer::new(vec![[level, -level]; 48_000]).unwrap());
        let track = project.add_audio_track("Tone");
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 48_000,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                },
            )
            .unwrap();
        project
    }

    fn playing(project: Project) -> AudioEngine {
        let engine = AudioEngine::start(project);
        engine.command(TransportCommand::Play);
        for _ in 0..500 {
            if engine.status().state == PlaybackState::Playing && engine_has_audio(&engine) {
                return engine;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("engine did not start");
    }

    fn engine_has_audio(engine: &AudioEngine) -> bool {
        engine.shared().ring.buffered() > 2048
    }

    #[test]
    fn matching_rates_copy_frames_and_clamp_overs() {
        let engine = playing(constant_project(1.5));
        let mut feeder = Feeder::new(engine.shared(), 48_000);
        let mut ramp = vec![[0.0; 2]; RAMP_IN_FRAMES];
        feeder.render(RAMP_IN_FRAMES, |index, frame| ramp[index] = frame);
        let mut out = vec![[0.0; 2]; 64];
        feeder.render(64, |index, frame| out[index] = frame);
        assert!(out.iter().all(|frame| *frame == [1.0, -1.0]));
    }

    #[test]
    fn resampling_produces_the_requested_frame_count_and_consumes_proportionally() {
        let engine = playing(constant_project(0.5));
        let shared = engine.shared();
        let before = shared.ring.buffered();
        let mut feeder = Feeder::new(Arc::clone(&shared), 44_100);
        let mut written = 0;
        let mut last = [0.0; 2];
        feeder.render(441, |index, frame| {
            written = index + 1;
            last = frame;
        });
        assert_eq!(written, 441);
        let consumed = before - shared.ring.buffered();
        assert!((479..=481).contains(&consumed), "consumed {consumed}");
        assert_eq!(last, [0.5, -0.5]);
    }

    #[test]
    fn declicker_fades_in_after_silence_and_out_at_every_cut() {
        let mut declick = Declicker::default();
        let mut frames = vec![[0.8, -0.8]; RAMP_IN_FRAMES + 64];
        declick.process(&mut frames, RAMP_IN_FRAMES + 64, false);
        assert!(frames[0][0] < 0.01, "a new stream ramps in");
        assert!(
            frames[RAMP_IN_FRAMES..]
                .iter()
                .all(|frame| *frame == [0.8, -0.8])
        );
        let mut stop = vec![[0.0; 2]; HOLD_FRAMES + 10];
        declick.process(&mut stop, 0, false);
        assert_steps_below(&[[0.8, -0.8]], &stop, 0.01);
        assert_eq!(stop.last(), Some(&[0.0, 0.0]));
        let mut start = vec![[0.6, 0.6]; RAMP_IN_FRAMES + 10];
        declick.process(&mut start, RAMP_IN_FRAMES + 10, false);
        assert!(start[0][0] < 0.01);
        assert_steps_below(&stop, &start, 0.01);
        assert_eq!(start.last(), Some(&[0.6, 0.6]));
        let mut seek = vec![[-0.6, -0.6]; HOLD_FRAMES + 10];
        declick.process(&mut seek, HOLD_FRAMES + 10, true);
        assert_steps_below(&start, &seek, 0.01);
        assert_eq!(seek.last(), Some(&[-0.6, -0.6]));
    }

    #[test]
    fn pausing_resuming_and_seeking_never_step_the_device_signal() {
        let engine = playing(constant_project(0.5));
        let mut feeder = Feeder::new(engine.shared(), 48_000);
        let mut heard = vec![[0.0; 2]; 1];
        let listen = |feeder: &mut Feeder, frames: usize, heard: &mut Vec<Frame>| {
            let start = heard.len();
            heard.resize(start + frames, [0.0; 2]);
            feeder.render(frames, |index, frame| heard[start + index] = frame);
        };
        let wait = |state: PlaybackState| {
            for _ in 0..500 {
                if engine.status().state == state {
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("engine did not reach {state:?}");
        };
        listen(&mut feeder, 512, &mut heard);
        engine.command(TransportCommand::Pause);
        wait(PlaybackState::Paused);
        listen(&mut feeder, 2048, &mut heard);
        assert_eq!(
            heard.last(),
            Some(&[0.0, 0.0]),
            "pausing must reach silence"
        );
        engine.command(TransportCommand::Play);
        wait(PlaybackState::Playing);
        while !engine_has_audio(&engine) {
            std::thread::sleep(Duration::from_millis(2));
        }
        listen(&mut feeder, 1024, &mut heard);
        assert_eq!(
            heard.last(),
            Some(&[0.5, -0.5]),
            "playback must resume at full level"
        );
        engine.command(TransportCommand::SeekFrame(24_000));
        std::thread::sleep(Duration::from_millis(20));
        listen(&mut feeder, 1024, &mut heard);
        assert_steps_below(&heard[..1], &heard[1..], 0.01);
    }

    fn assert_steps_below(before: &[Frame], frames: &[Frame], limit: f32) {
        let mut previous = *before.last().unwrap();
        for (index, frame) in frames.iter().enumerate() {
            for channel in 0..2 {
                let step = (frame[channel] - previous[channel]).abs();
                assert!(step < limit, "step of {step} at frame {index}");
            }
            previous = *frame;
        }
    }

    #[test]
    fn non_finite_samples_become_silence() {
        assert_eq!(safe([f32::NAN, f32::INFINITY]), [0.0, 0.0]);
        assert_eq!(safe([-3.0, 0.25]), [-1.0, 0.25]);
    }
}
