use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_queue::ArrayQueue;
use sympoie::track::{AudioInputChannels, AudioRecordingInput, MidiRecordingInput};
use sympoie::{Frame, SAMPLE_RATE};

use super::record_clock::RecordingClock;

const PACKET_FRAMES: usize = 512;
const AUDIO_PACKETS: usize = 1024;
const MIDI_PACKETS: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct InputCatalog {
    pub audio: Vec<AudioInputDevice>,
    pub midi: Vec<MidiInputDevice>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AudioInputDevice {
    pub id: String,
    pub name: String,
    pub channels: u16,
    pub is_default: bool,
}

#[derive(Clone, Debug)]
pub struct MidiInputDevice {
    pub id: String,
    pub name: String,
}

pub fn discover_inputs() -> InputCatalog {
    #[cfg(feature = "audio-device")]
    {
        device::discover()
    }
    #[cfg(not(feature = "audio-device"))]
    {
        InputCatalog {
            error: Some("Recording requires an audio-device enabled build".into()),
            ..InputCatalog::default()
        }
    }
}

#[derive(Debug)]
pub enum CaptureEvent {
    Audio {
        track: usize,
        offset: u64,
        frames: Vec<Frame>,
    },
    Midi {
        track: usize,
        offset: u64,
        source: usize,
        message: [u8; 3],
    },
    Error(String),
}

struct AudioPacket {
    route: usize,
    anchor_offset: u64,
    source_start: u64,
    len: usize,
    frames: [Frame; PACKET_FRAMES],
}

struct MidiPacket {
    track: usize,
    offset: u64,
    source: usize,
    message: [u8; 3],
}

struct AudioChunk {
    offset: u64,
    frames: Vec<Frame>,
}

struct AudioRoute {
    tracks: Vec<usize>,
    resampler: Resampler,
}

impl AudioRoute {
    fn append(&self, chunk: AudioChunk, events: &mut Vec<CaptureEvent>) {
        let Some((last, others)) = self.tracks.split_last() else {
            return;
        };
        for track in others {
            events.push(CaptureEvent::Audio {
                track: *track,
                offset: chunk.offset,
                frames: chunk.frames.clone(),
            });
        }
        events.push(CaptureEvent::Audio {
            track: *last,
            offset: chunk.offset,
            frames: chunk.frames,
        });
    }
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
struct AudioRoutePlan {
    channels: AudioInputChannels,
    tracks: Vec<usize>,
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
struct AudioDevicePlan {
    device: usize,
    routes: Vec<AudioRoutePlan>,
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
fn plan_audio(
    requests: &[(usize, AudioRecordingInput)],
    devices: &[AudioInputDevice],
) -> Result<Vec<AudioDevicePlan>, String> {
    let mut plans: Vec<AudioDevicePlan> = Vec::new();
    for (track, input) in requests {
        let device = devices
            .iter()
            .position(|device| {
                input
                    .device
                    .as_ref()
                    .map_or(device.is_default, |id| *id == device.id)
            })
            .ok_or_else(|| match &input.device {
                Some(id) => format!("Audio input {id} is unavailable; select a connected input"),
                None => "No default audio input device is available".into(),
            })?;
        let highest = match input.channels {
            AudioInputChannels::Mono(channel) => channel,
            AudioInputChannels::Stereo(left, right) => left.max(right),
        };
        if highest >= devices[device].channels {
            return Err(format!(
                "{} has no input channel {}; choose an available channel",
                devices[device].name,
                u32::from(highest) + 1
            ));
        }
        let plan = match plans.iter_mut().find(|plan| plan.device == device) {
            Some(plan) => plan,
            None => {
                plans.push(AudioDevicePlan {
                    device,
                    routes: Vec::new(),
                });
                plans.last_mut().expect("just inserted audio device")
            }
        };
        if let Some(route) = plan
            .routes
            .iter_mut()
            .find(|route| route.channels == input.channels)
        {
            route.tracks.push(*track);
        } else {
            plan.routes.push(AudioRoutePlan {
                channels: input.channels,
                tracks: vec![*track],
            });
        }
    }
    Ok(plans)
}

#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
struct MidiRoute {
    track: usize,
    channel: Option<u8>,
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
fn plan_midi(
    requests: &[(usize, MidiRecordingInput)],
    ports: &[MidiInputDevice],
) -> Result<Vec<Vec<MidiRoute>>, String> {
    let mut routes = vec![Vec::new(); ports.len()];
    for (track, input) in requests {
        if input.channel.is_some_and(|channel| channel >= 16) {
            return Err("MIDI input channels must be between 1 and 16".into());
        }
        let mut matched = false;
        for (index, port) in ports.iter().enumerate() {
            if input.device.as_ref().is_none_or(|id| *id == port.id) {
                routes[index].push(MidiRoute {
                    track: *track,
                    channel: input.channel,
                });
                matched = true;
            }
        }
        if !matched {
            return Err(match &input.device {
                Some(id) => format!("MIDI input {id} is unavailable; select a connected input"),
                None => "No MIDI input devices are connected".into(),
            });
        }
    }
    Ok(routes)
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
fn route_midi(queues: &Queues, routes: &[MidiRoute], offset: u64, source: usize, message: &[u8]) {
    if message.len() < 2 || !(0x80..0xf0).contains(&message[0]) {
        return;
    }
    let channel = message[0] & 0x0f;
    for route in routes {
        if route.channel.is_some_and(|wanted| wanted != channel) {
            continue;
        }
        let packet = MidiPacket {
            track: route.track,
            offset,
            source,
            message: [message[0], message[1], message.get(2).copied().unwrap_or(0)],
        };
        if queues.midi.push(packet).is_err() {
            queues.overflow.store(true, Ordering::Release);
        }
    }
}

struct Queues {
    audio: ArrayQueue<AudioPacket>,
    midi: ArrayQueue<MidiPacket>,
    overflow: AtomicBool,
    device_failed: AtomicBool,
}

impl Queues {
    fn new() -> Self {
        Self {
            audio: ArrayQueue::new(AUDIO_PACKETS),
            midi: ArrayQueue::new(MIDI_PACKETS),
            overflow: AtomicBool::new(false),
            device_failed: AtomicBool::new(false),
        }
    }
}

pub struct InputCapture {
    queues: Arc<Queues>,
    routes: Vec<AudioRoute>,
    description: String,
    finished: bool,
    tail_drained: bool,
    #[cfg(feature = "audio-device")]
    audio: Vec<cpal::Stream>,
    #[cfg(feature = "audio-device")]
    midi: Vec<midir::MidiInputConnection<()>>,
    #[cfg(test)]
    injected: Vec<CaptureEvent>,
}

impl InputCapture {
    pub fn open(
        audio: &[(usize, AudioRecordingInput)],
        midi: &[(usize, MidiRecordingInput)],
        use_devices: bool,
        clock: Arc<RecordingClock>,
    ) -> Result<Self, String> {
        if audio.is_empty() && midi.is_empty() {
            return Err("Arm an audio or MIDI track before recording".into());
        }
        let mut capture = Self {
            queues: Arc::new(Queues::new()),
            routes: Vec::new(),
            description: "Test recording input".into(),
            finished: false,
            tail_drained: false,
            #[cfg(feature = "audio-device")]
            audio: Vec::new(),
            #[cfg(feature = "audio-device")]
            midi: Vec::new(),
            #[cfg(test)]
            injected: Vec::new(),
        };
        if !use_devices {
            #[cfg(test)]
            return Ok(capture);
            #[cfg(not(test))]
            return Err("Recording input is disabled in silent-clock mode".into());
        }
        #[cfg(feature = "audio-device")]
        {
            let mut descriptions = Vec::new();
            if !audio.is_empty() {
                let (streams, routes, names) =
                    device::open_audio(audio, Arc::clone(&capture.queues), Arc::clone(&clock))?;
                capture.audio = streams;
                capture.routes = routes;
                descriptions.extend(names);
            }
            if !midi.is_empty() {
                let (connections, names) = device::open_midi(midi, capture.queues.clone(), clock)?;
                capture.midi = connections;
                descriptions.extend(names);
            }
            capture.description = descriptions.join(" · ");
            Ok(capture)
        }
        #[cfg(not(feature = "audio-device"))]
        {
            let _ = (&mut capture, clock);
            Err("Recording requires an audio-device enabled build".into())
        }
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn drain(&mut self) -> Vec<CaptureEvent> {
        let mut events = Vec::new();
        while let Some(packet) = self.queues.audio.pop() {
            if let Some(route) = self.routes.get_mut(packet.route)
                && let Some(chunk) = route.resampler.process(&packet)
            {
                route.append(chunk, &mut events);
            }
        }
        while let Some(packet) = self.queues.midi.pop() {
            events.push(CaptureEvent::Midi {
                track: packet.track,
                offset: packet.offset,
                source: packet.source,
                message: packet.message,
            });
        }
        if self.finished && !self.tail_drained {
            for route in &mut self.routes {
                if let Some(chunk) = route.resampler.finish() {
                    route.append(chunk, &mut events);
                }
            }
            self.tail_drained = true;
        }
        if self.queues.overflow.swap(false, Ordering::AcqRel) {
            events.push(CaptureEvent::Error(
                "Recording input overflowed; part of this take may be missing".into(),
            ));
        }
        if self.queues.device_failed.swap(false, Ordering::AcqRel) {
            events.push(CaptureEvent::Error(
                "The audio input disconnected or stopped unexpectedly".into(),
            ));
        }
        #[cfg(test)]
        events.append(&mut self.injected);
        events
    }

    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        #[cfg(feature = "audio-device")]
        {
            self.audio.clear();
            self.midi.clear();
        }
        self.finished = true;
    }

    #[cfg(test)]
    pub fn inject(&mut self, event: CaptureEvent) {
        self.injected.push(event);
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
struct AudioWriter {
    queues: Arc<Queues>,
    clock: Arc<RecordingClock>,
    rate: u32,
    route: usize,
    anchor_offset: Option<u64>,
    source_cursor: u64,
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
impl AudioWriter {
    fn new(queues: Arc<Queues>, clock: Arc<RecordingClock>, rate: u32, route: usize) -> Self {
        Self {
            queues,
            clock,
            rate,
            route,
            anchor_offset: None,
            source_cursor: 0,
        }
    }

    fn push(&mut self, count: usize, capture_time: Instant, mut frame: impl FnMut(usize) -> Frame) {
        let mut packet = AudioPacket {
            route: self.route,
            anchor_offset: self.anchor_offset.unwrap_or(0),
            source_start: self.source_cursor,
            len: 0,
            frames: [[0.0; 2]; PACKET_FRAMES],
        };
        for index in 0..count {
            let when = capture_time + Duration::from_secs_f64(index as f64 / f64::from(self.rate));
            let Some(offset) = self.clock.offset_at(when) else {
                continue;
            };
            if self.anchor_offset.is_none() {
                self.anchor_offset = Some(offset);
                packet.anchor_offset = offset;
            }
            packet.frames[packet.len] = frame(index).map(finite_sample);
            packet.len += 1;
            self.source_cursor += 1;
            if packet.len == PACKET_FRAMES {
                self.send(packet);
                packet = AudioPacket {
                    route: self.route,
                    anchor_offset: self.anchor_offset.unwrap_or(0),
                    source_start: self.source_cursor,
                    len: 0,
                    frames: [[0.0; 2]; PACKET_FRAMES],
                };
            }
        }
        if packet.len > 0 {
            self.send(packet);
        }
    }

    fn send(&self, packet: AudioPacket) {
        if self.queues.audio.push(packet).is_err() {
            self.queues.overflow.store(true, Ordering::Release);
        }
    }
}

fn finite_sample(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

struct Resampler {
    rate: u32,
    anchor_offset: u64,
    previous: Option<(u64, Frame)>,
    next_output: u64,
}

impl Resampler {
    #[cfg(any(feature = "audio-device", test))]
    fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1),
            anchor_offset: 0,
            previous: None,
            next_output: 0,
        }
    }

    fn first_output(&self, source: u64) -> u64 {
        (u128::from(source) * u128::from(SAMPLE_RATE)).div_ceil(u128::from(self.rate)) as u64
    }

    fn process(&mut self, packet: &AudioPacket) -> Option<AudioChunk> {
        if packet.len == 0 {
            return None;
        }
        self.anchor_offset = packet.anchor_offset;
        if self.rate == SAMPLE_RATE {
            return Some(AudioChunk {
                offset: packet.anchor_offset.saturating_add(packet.source_start),
                frames: packet.frames[..packet.len].to_vec(),
            });
        }
        if self
            .previous
            .is_none_or(|(index, _)| index + 1 != packet.source_start)
        {
            self.previous = None;
            self.next_output = self.first_output(packet.source_start);
        }
        let first = self.next_output;
        let mut frames = Vec::with_capacity(self.first_output(packet.len as u64) as usize + 1);
        for (index, frame) in packet.frames[..packet.len].iter().copied().enumerate() {
            let index = packet.source_start + index as u64;
            while u128::from(self.next_output) * u128::from(self.rate)
                <= u128::from(index) * u128::from(SAMPLE_RATE)
            {
                let output = match self.previous {
                    Some((previous_index, previous)) => {
                        let position =
                            self.next_output as f64 * f64::from(self.rate) / f64::from(SAMPLE_RATE);
                        let fraction = (position - previous_index as f64).clamp(0.0, 1.0) as f32;
                        [
                            previous[0] + (frame[0] - previous[0]) * fraction,
                            previous[1] + (frame[1] - previous[1]) * fraction,
                        ]
                    }
                    None => frame,
                };
                frames.push(output);
                self.next_output += 1;
            }
            self.previous = Some((index, frame));
        }
        (!frames.is_empty()).then_some(AudioChunk {
            offset: self.anchor_offset.saturating_add(first),
            frames,
        })
    }

    fn finish(&mut self) -> Option<AudioChunk> {
        let (index, frame) = self.previous.take()?;
        let end = self.first_output(index + 1);
        let count = end.saturating_sub(self.next_output) as usize;
        let offset = self.anchor_offset.saturating_add(self.next_output);
        self.next_output = end;
        (count > 0).then_some(AudioChunk {
            offset,
            frames: vec![frame; count],
        })
    }
}

#[cfg(feature = "audio-device")]
mod device {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SampleFormat, SizedSample};
    use midir::{Ignore, MidiInput};

    use super::*;

    fn audio_devices() -> (Vec<(cpal::Device, AudioInputDevice)>, Vec<String>) {
        let mut devices = Vec::new();
        let mut errors = Vec::new();
        let mut default_id = None;
        for host_id in cpal::available_hosts() {
            let host = match cpal::host_from_id(host_id) {
                Ok(host) => host,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
            if default_id.is_none() {
                default_id = host
                    .default_input_device()
                    .and_then(|device| device.id().ok())
                    .map(|id| id.to_string());
            }
            let inputs = match host.input_devices() {
                Ok(inputs) => inputs,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
            for device in inputs {
                let id = match device.id() {
                    Ok(id) => id.to_string(),
                    Err(error) => {
                        errors.push(error.to_string());
                        continue;
                    }
                };
                let name = device
                    .description()
                    .map(|description| description.name().to_owned())
                    .unwrap_or_else(|_| id.clone());
                let channels = device
                    .supported_input_configs()
                    .ok()
                    .and_then(|configs| configs.map(|config| config.channels()).max())
                    .or_else(|| {
                        device
                            .default_input_config()
                            .ok()
                            .map(|config| config.channels())
                    })
                    .unwrap_or(0);
                let description = AudioInputDevice {
                    is_default: default_id.as_ref() == Some(&id),
                    id,
                    name,
                    channels,
                };
                devices.push((device, description));
            }
        }
        (devices, errors)
    }

    fn midi_devices(input: &MidiInput, ports: &[midir::MidiInputPort]) -> Vec<MidiInputDevice> {
        ports
            .iter()
            .enumerate()
            .map(|(index, port)| MidiInputDevice {
                id: port.id(),
                name: input
                    .port_name(port)
                    .unwrap_or_else(|_| format!("MIDI input {}", index + 1)),
            })
            .collect()
    }

    pub(super) fn discover() -> InputCatalog {
        let (devices, mut errors) = audio_devices();
        let audio = devices
            .into_iter()
            .map(|(_, description)| description)
            .collect();
        let midi = match MidiInput::new("Sympoie input discovery") {
            Ok(input) => midi_devices(&input, &input.ports()),
            Err(error) => {
                errors.push(format!("Cannot initialize MIDI input: {error}"));
                Vec::new()
            }
        };
        InputCatalog {
            audio,
            midi,
            error: (!errors.is_empty()).then(|| errors.join(" · ")),
        }
    }

    type AudioStreams = (Vec<cpal::Stream>, Vec<AudioRoute>, Vec<String>);

    pub(super) fn open_audio(
        requests: &[(usize, AudioRecordingInput)],
        queues: Arc<Queues>,
        clock: Arc<RecordingClock>,
    ) -> Result<AudioStreams, String> {
        let (devices, errors) = audio_devices();
        if devices.is_empty() && !errors.is_empty() {
            return Err(errors.join(" · "));
        }
        let catalog: Vec<_> = devices
            .iter()
            .map(|(_, description)| description.clone())
            .collect();
        let plans = plan_audio(requests, &catalog)?;
        let mut streams = Vec::with_capacity(plans.len());
        let mut routes = Vec::new();
        let mut names = Vec::with_capacity(plans.len());
        for plan in plans {
            let (device, description) = &devices[plan.device];
            let name = &description.name;
            let required_channels = plan
                .routes
                .iter()
                .map(|route| match route.channels {
                    AudioInputChannels::Mono(channel) => channel + 1,
                    AudioInputChannels::Stereo(left, right) => left.max(right) + 1,
                })
                .max()
                .unwrap_or(1);
            let supported = select_config(device, required_channels)
                .map_err(|error| format!("Cannot open {name}: {error}"))?;
            let config = supported.config();
            let rate = config.sample_rate;
            let mut writers = Vec::with_capacity(plan.routes.len());
            for route in plan.routes {
                let writer = AudioWriter::new(queues.clone(), clock.clone(), rate, routes.len());
                writers.push((route.channels, writer));
                routes.push(AudioRoute {
                    tracks: route.tracks,
                    resampler: Resampler::new(rate),
                });
            }
            let stream = match supported.sample_format() {
                SampleFormat::F32 => build::<f32>(device, config, queues.clone(), writers),
                SampleFormat::F64 => build::<f64>(device, config, queues.clone(), writers),
                SampleFormat::I8 => build::<i8>(device, config, queues.clone(), writers),
                SampleFormat::I16 => build::<i16>(device, config, queues.clone(), writers),
                SampleFormat::I24 => build::<cpal::I24>(device, config, queues.clone(), writers),
                SampleFormat::I32 => build::<i32>(device, config, queues.clone(), writers),
                SampleFormat::I64 => build::<i64>(device, config, queues.clone(), writers),
                SampleFormat::U8 => build::<u8>(device, config, queues.clone(), writers),
                SampleFormat::U16 => build::<u16>(device, config, queues.clone(), writers),
                SampleFormat::U24 => build::<cpal::U24>(device, config, queues.clone(), writers),
                SampleFormat::U32 => build::<u32>(device, config, queues.clone(), writers),
                SampleFormat::U64 => build::<u64>(device, config, queues.clone(), writers),
                format => return Err(format!("{name} uses unsupported input format {format}")),
            }
            .map_err(|error| format!("Cannot record from {name}: {error}"))?;
            stream
                .play()
                .map_err(|error| format!("Cannot start {name}: {error}"))?;
            streams.push(stream);
            names.push(name.clone());
        }
        Ok((streams, routes, names))
    }

    fn select_config(
        device: &cpal::Device,
        channels: u16,
    ) -> Result<cpal::SupportedStreamConfig, String> {
        let default = device
            .default_input_config()
            .map_err(|error| error.to_string())?;
        if default.channels() >= channels {
            return Ok(default);
        }
        let config = device
            .supported_input_configs()
            .map_err(|error| error.to_string())?
            .filter(|config| config.channels() >= channels)
            .min_by_key(|config| config.channels())
            .ok_or_else(|| format!("No input configuration provides {channels} channels"))?;
        let rate = SAMPLE_RATE.clamp(config.min_sample_rate(), config.max_sample_rate());
        Ok(config.with_sample_rate(rate))
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        queues: Arc<Queues>,
        mut writers: Vec<(AudioInputChannels, AudioWriter)>,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = usize::from(config.channels.max(1));
        device.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                let timestamp = info.timestamp();
                let delay = timestamp.callback.duration_since(timestamp.capture);
                let now = Instant::now();
                let capture_time = now.checked_sub(delay).unwrap_or(now);
                for (selection, writer) in &mut writers {
                    writer.push(data.len() / channels, capture_time, |index| {
                        selected_frame(&data[index * channels..][..channels], *selection)
                    });
                }
            },
            move |_error| {
                queues.device_failed.store(true, Ordering::Release);
            },
            None,
        )
    }

    pub(super) fn selected_frame<T: SizedSample>(
        samples: &[T],
        selection: AudioInputChannels,
    ) -> Frame
    where
        f32: FromSample<T>,
    {
        let channel = |index: u16| f32::from_sample(samples[usize::from(index)]);
        match selection {
            AudioInputChannels::Mono(index) => {
                let sample = channel(index);
                [sample, sample]
            }
            AudioInputChannels::Stereo(left, right) => [channel(left), channel(right)],
        }
    }

    type MidiConnections = Vec<midir::MidiInputConnection<()>>;

    pub(super) fn open_midi(
        requests: &[(usize, MidiRecordingInput)],
        queues: Arc<Queues>,
        clock: Arc<RecordingClock>,
    ) -> Result<(MidiConnections, Vec<String>), String> {
        let enumerator = MidiInput::new("Sympoie input discovery")
            .map_err(|error| format!("Cannot initialize MIDI input: {error}"))?;
        let ports = enumerator.ports();
        let devices = midi_devices(&enumerator, &ports);
        let routes = plan_midi(requests, &devices)?;
        let mut connections = Vec::new();
        let mut names = Vec::new();
        for (source, ((port, device), routes)) in ports.iter().zip(&devices).zip(routes).enumerate()
        {
            if routes.is_empty() {
                continue;
            }
            let name = &device.name;
            let mut input = MidiInput::new("Sympoie recording")
                .map_err(|error| format!("Cannot initialize MIDI input: {error}"))?;
            input.ignore(Ignore::SysexAndTime);
            let queues = queues.clone();
            let clock = clock.clone();
            let connection = input
                .connect(
                    port,
                    "Sympoie recording input",
                    move |_, message, ()| {
                        if let Some(offset) = clock.offset_at(Instant::now()) {
                            route_midi(&queues, &routes, offset, source, message);
                        }
                    },
                    (),
                )
                .map_err(|error| format!("Cannot record MIDI from {name}: {error}"))?;
            connections.push(connection);
            names.push(name.clone());
        }
        Ok((connections, names))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_devices() -> Vec<AudioInputDevice> {
        vec![
            AudioInputDevice {
                id: "device-a".into(),
                name: "Interface A".into(),
                channels: 4,
                is_default: true,
            },
            AudioInputDevice {
                id: "device-b".into(),
                name: "Microphone B".into(),
                channels: 1,
                is_default: false,
            },
        ]
    }

    #[test]
    fn audio_plans_share_default_and_explicit_devices_and_identical_routes() {
        let requests = [
            (1, AudioRecordingInput::default()),
            (
                2,
                AudioRecordingInput {
                    device: Some("device-a".into()),
                    channels: AudioInputChannels::Mono(0),
                },
            ),
            (
                3,
                AudioRecordingInput {
                    device: Some("device-a".into()),
                    channels: AudioInputChannels::Stereo(2, 3),
                },
            ),
            (
                4,
                AudioRecordingInput {
                    device: Some("device-b".into()),
                    channels: AudioInputChannels::Mono(0),
                },
            ),
        ];
        let plans = plan_audio(&requests, &audio_devices()).unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].device, 0);
        assert_eq!(plans[0].routes.len(), 2);
        assert_eq!(plans[0].routes[0].tracks, [1, 2]);
        assert_eq!(plans[0].routes[1].tracks, [3]);
        assert_eq!(plans[1].device, 1);
        assert_eq!(plans[1].routes[0].tracks, [4]);
    }

    #[test]
    fn unavailable_audio_devices_or_channels_do_not_fall_back() {
        let missing = AudioRecordingInput {
            device: Some("missing".into()),
            channels: AudioInputChannels::Mono(0),
        };
        assert!(plan_audio(&[(1, missing)], &audio_devices()).is_err());
        let wrong_channel = AudioRecordingInput {
            device: Some("device-b".into()),
            channels: AudioInputChannels::Mono(1),
        };
        assert!(plan_audio(&[(1, wrong_channel)], &audio_devices()).is_err());
        let mut no_default = audio_devices();
        for device in &mut no_default {
            device.is_default = false;
        }
        assert!(plan_audio(&[(1, AudioRecordingInput::default())], &no_default).is_err());
    }

    fn midi_devices() -> Vec<MidiInputDevice> {
        vec![
            MidiInputDevice {
                id: "keys-a".into(),
                name: "Keyboard A".into(),
            },
            MidiInputDevice {
                id: "keys-b".into(),
                name: "Keyboard B".into(),
            },
        ]
    }

    #[test]
    fn midi_routing_selects_ports_and_channels_without_duplicate_connections() {
        let requests = [
            (1, MidiRecordingInput::default()),
            (
                2,
                MidiRecordingInput {
                    device: Some("keys-a".into()),
                    channel: Some(1),
                },
            ),
            (
                3,
                MidiRecordingInput {
                    device: Some("keys-b".into()),
                    channel: None,
                },
            ),
        ];
        let plans = plan_midi(&requests, &midi_devices()).unwrap();
        assert_eq!(plans.len(), 2);
        let queues = Queues::new();
        route_midi(&queues, &plans[0], 10, 0, &[0x91, 60, 100]);
        route_midi(&queues, &plans[0], 20, 0, &[0x90, 61, 101]);
        route_midi(&queues, &plans[1], 30, 1, &[0x81, 60, 0]);
        let mut actual = Vec::new();
        while let Some(packet) = queues.midi.pop() {
            actual.push((packet.track, packet.offset, packet.source, packet.message));
        }
        assert_eq!(
            actual,
            [
                (1, 10, 0, [0x91, 60, 100]),
                (2, 10, 0, [0x91, 60, 100]),
                (1, 20, 0, [0x90, 61, 101]),
                (1, 30, 1, [0x81, 60, 0]),
                (3, 30, 1, [0x81, 60, 0]),
            ]
        );
    }

    #[test]
    fn missing_midi_ports_or_invalid_channels_fail_explicitly() {
        let missing = MidiRecordingInput {
            device: Some("missing".into()),
            channel: None,
        };
        assert!(plan_midi(&[(1, missing)], &midi_devices()).is_err());
        let invalid = MidiRecordingInput {
            device: None,
            channel: Some(16),
        };
        assert!(plan_midi(&[(1, invalid)], &midi_devices()).is_err());
        assert!(plan_midi(&[(1, MidiRecordingInput::default())], &[]).is_err());
    }

    #[test]
    fn devices_resample_independently_and_identical_routes_fan_out_to_tracks() {
        let clock = Arc::new(RecordingClock::new());
        let mut capture =
            InputCapture::open(&[(1, AudioRecordingInput::default())], &[], false, clock).unwrap();
        capture.routes = vec![
            AudioRoute {
                tracks: vec![1, 2],
                resampler: Resampler::new(24_000),
            },
            AudioRoute {
                tracks: vec![3],
                resampler: Resampler::new(SAMPLE_RATE),
            },
        ];
        let first = packet(0, 3, 10);
        let mut second = packet(0, 3, 20);
        second.route = 1;
        second.frames[..3].fill([0.25, 0.25]);
        assert!(capture.queues.audio.push(first).is_ok());
        assert!(capture.queues.audio.push(second).is_ok());
        capture.finish();
        let mut tracks = [Vec::new(), Vec::new(), Vec::new()];
        for event in capture.drain() {
            let CaptureEvent::Audio {
                track,
                offset,
                frames,
            } = event
            else {
                panic!("expected audio");
            };
            let expected_start = if track == 3 { 20 } else { 10 };
            assert_eq!(offset, expected_start + tracks[track - 1].len() as u64);
            tracks[track - 1].extend(frames);
        }
        assert_eq!(tracks[0].len(), 6);
        assert_eq!(tracks[0], tracks[1]);
        assert_eq!(tracks[2], [[0.25, 0.25]; 3]);
    }

    fn packet(start: u64, len: usize, anchor: u64) -> AudioPacket {
        let mut packet = AudioPacket {
            route: 0,
            anchor_offset: anchor,
            source_start: start,
            len,
            frames: [[0.0; 2]; PACKET_FRAMES],
        };
        for (index, frame) in packet.frames[..len].iter_mut().enumerate() {
            let value = (start + index as u64) as f32;
            *frame = [value, -value];
        }
        packet
    }

    fn audio(chunk: AudioChunk) -> (u64, Vec<Frame>) {
        (chunk.offset, chunk.frames)
    }

    #[test]
    fn native_rate_preserves_frames_and_gaps() {
        let mut resampler = Resampler::new(SAMPLE_RATE);
        let (offset, frames) = audio(resampler.process(&packet(99, 10, 42)).unwrap());
        assert_eq!(offset, 141);
        assert_eq!(frames[0], [99.0, -99.0]);
        assert_eq!(frames.len(), 10);
        assert!(resampler.finish().is_none());
    }

    #[test]
    fn fractional_rate_is_independent_of_packet_boundaries() {
        for rate in [44_100, 96_000] {
            let mut whole = Resampler::new(rate);
            let mut expected = audio(whole.process(&packet(0, 441, 0)).unwrap()).1;
            if let Some(tail) = whole.finish() {
                expected.extend(audio(tail).1);
            }
            let mut split = Resampler::new(rate);
            let mut actual = Vec::new();
            for (start, len) in [(0, 101), (101, 130), (231, 210)] {
                let (offset, frames) = audio(split.process(&packet(start, len, 0)).unwrap());
                assert_eq!(offset, actual.len() as u64);
                actual.extend(frames);
            }
            if let Some(tail) = split.finish() {
                actual.extend(audio(tail).1);
            }
            assert_eq!(actual, expected);
            assert_eq!(
                actual.len(),
                (441_u64 * u64::from(SAMPLE_RATE)).div_ceil(u64::from(rate)) as usize
            );
        }
    }

    #[test]
    fn resampling_does_not_fill_a_dropped_packet_with_old_audio() {
        let mut resampler = Resampler::new(24_000);
        resampler.process(&packet(0, 10, 12)).unwrap();
        let (offset, frames) = audio(resampler.process(&packet(20, 10, 12)).unwrap());
        assert_eq!(offset, 52);
        assert_eq!(frames[0], [20.0, -20.0]);
    }

    #[test]
    fn callbacks_gate_preroll_and_keep_only_finite_samples() {
        let clock = Arc::new(RecordingClock::new());
        let queues = Arc::new(Queues::new());
        let mut writer = AudioWriter::new(queues.clone(), clock.clone(), SAMPLE_RATE, 0);
        writer.push(3, Instant::now(), |_| [1.0, 2.0]);
        assert!(queues.audio.pop().is_none());
        clock.start(500);
        writer.push(3, Instant::now(), |_| [f32::NAN, -0.5]);
        let packet = queues.audio.pop().unwrap();
        assert_eq!(packet.len, 3);
        assert_eq!(packet.frames[0], [0.0, -0.5]);
        clock.stop();
        writer.push(3, Instant::now(), |_| [1.0, 2.0]);
        assert!(queues.audio.pop().is_none());
    }

    #[test]
    fn no_device_capture_can_finish_and_drain_injected_events() {
        let clock = Arc::new(RecordingClock::new());
        let mut capture = InputCapture::open(
            &[(1, AudioRecordingInput::default())],
            &[(4, MidiRecordingInput::default())],
            false,
            clock,
        )
        .unwrap();
        capture.inject(CaptureEvent::Midi {
            track: 4,
            offset: 100,
            source: 2,
            message: [0x91, 64, 100],
        });
        capture.finish();
        assert!(matches!(
            capture.drain().as_slice(),
            [CaptureEvent::Midi {
                track: 4,
                offset: 100,
                source: 2,
                message: [0x91, 64, 100]
            }]
        ));
        assert!(capture.drain().is_empty());
    }

    #[cfg(feature = "audio-device")]
    #[test]
    fn input_conversion_duplicates_selected_mono_even_on_a_multichannel_device() {
        assert_eq!(
            device::selected_frame(&[16_384_i16], AudioInputChannels::Mono(0)),
            [0.5, 0.5]
        );
        assert_eq!(
            device::selected_frame(&[16_384_i16, 0], AudioInputChannels::Mono(0)),
            [0.5, 0.5]
        );
        assert_eq!(
            device::selected_frame(&[0_i16, 16_384], AudioInputChannels::Mono(1)),
            [0.5, 0.5]
        );
        assert_eq!(
            device::selected_frame(&[32_768_u16], AudioInputChannels::Mono(0)),
            [0.0, 0.0]
        );
    }

    #[cfg(feature = "audio-device")]
    #[test]
    fn stereo_selection_keeps_real_separate_channels_including_silence() {
        assert_eq!(
            device::selected_frame(&[-32_768_i16, 0, 16_384], AudioInputChannels::Stereo(0, 2)),
            [-1.0, 0.5]
        );
        assert_eq!(
            device::selected_frame(&[16_384_i16, 0], AudioInputChannels::Stereo(0, 1)),
            [0.5, 0.0]
        );
    }

    #[test]
    fn finish_drains_pending_audio_and_the_resampler_tail_once() {
        let clock = Arc::new(RecordingClock::new());
        let mut capture =
            InputCapture::open(&[(1, AudioRecordingInput::default())], &[], false, clock).unwrap();
        capture.routes.push(AudioRoute {
            tracks: vec![1],
            resampler: Resampler::new(24_000),
        });
        assert!(capture.queues.audio.push(packet(0, 3, 10)).is_ok());
        capture.finish();
        let events = capture.drain();
        assert_eq!(events.len(), 2);
        let mut output = Vec::new();
        for event in events {
            let CaptureEvent::Audio {
                track: 1,
                offset,
                frames,
            } = event
            else {
                panic!("expected track 1 audio");
            };
            assert_eq!(offset, 10 + output.len() as u64);
            output.extend(frames);
        }
        assert_eq!(output.len(), 6);
        assert_eq!(output[5], [2.0, -2.0]);
        assert!(capture.drain().is_empty());
    }
}
