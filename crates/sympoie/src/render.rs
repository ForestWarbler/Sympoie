use std::error::Error;
use std::fmt;
use std::mem;
use std::sync::Arc;

use crate::automation::{AutomationLane, AutomationTarget};
use crate::clip::{MidiClip, MidiNote};
use crate::instrument::{Instrument, InstrumentError, InstrumentProcessor, NoteEvent};
use crate::plugin::{
    PluginHost, PluginInstance, PluginProcessContext, PluginProcessor, PluginSidechainInput,
    RenderQuality, SidechainRoute, SidechainTap,
};
use crate::project::Project;
use crate::time::{Tempo, TimeError};
use crate::track::{AudioOutput, MidiTrack, Track};
use crate::{Frame, SAMPLE_RATE, TICKS_PER_QUARTER};

const SCRATCH_FRAMES: usize = 256;
const AUTOMATION_FRAMES: u64 = 64;
const PARAMETER_RAMP_FRAMES: usize = SAMPLE_RATE as usize / 200;
const MAX_AUDITION_NOTES: usize = 128;
const MAX_LATENCY_FRAMES: usize = SAMPLE_RATE as usize * 10;
const MAX_DELAY_STORAGE_FRAMES: usize = SAMPLE_RATE as usize * 120;

#[derive(Debug, Default)]
struct DelayLine {
    samples: Vec<Frame>,
    cursor: usize,
    delay: usize,
}

impl DelayLine {
    fn configure(&mut self, delay: usize) {
        let capacity = (delay + 1).max(SCRATCH_FRAMES);
        if capacity > self.samples.len() {
            let mut samples = vec![[0.0; 2]; capacity];
            let retained = self.samples.len();
            for index in 0..retained {
                samples[capacity - retained + index] =
                    self.samples[(self.cursor + index) % retained];
            }
            self.samples = samples;
            self.cursor = 0;
        }
        self.delay = delay;
    }

    fn clear(&mut self) {
        self.samples.fill([0.0; 2]);
        self.cursor = 0;
    }

    fn process(&mut self, frames: &mut [Frame]) {
        for frame in frames {
            let input = *frame;
            if self.delay != 0 {
                *frame = self.samples
                    [(self.cursor + self.samples.len() - self.delay) % self.samples.len()];
            }
            self.samples[self.cursor] = input;
            self.cursor = (self.cursor + 1) % self.samples.len();
        }
    }
}

#[derive(Debug)]
struct SmoothedValue {
    current: f64,
    target: f32,
    remaining: usize,
}

impl SmoothedValue {
    fn new(value: f32) -> Self {
        Self {
            current: f64::from(value),
            target: value,
            remaining: 0,
        }
    }

    fn set_target(&mut self, target: f32) {
        if self.target != target {
            self.target = target;
            self.remaining = PARAMETER_RAMP_FRAMES;
        }
    }

    fn advance(&mut self, frames: usize) -> f32 {
        if self.remaining != 0 {
            let count = frames.min(self.remaining);
            self.current +=
                (f64::from(self.target) - self.current) * count as f64 / self.remaining as f64;
            self.remaining -= count;
        }
        if self.remaining == 0 {
            self.current = f64::from(self.target);
        }
        self.current as f32
    }
}

pub fn render_block(
    project: &Project,
    start_frame: u64,
    output: &mut [Frame],
) -> Result<(), RenderError> {
    match Renderer::new(project) {
        Ok(mut renderer) => {
            renderer.render_block(start_frame, output);
            Ok(())
        }
        Err(error) => {
            output.fill([0.0; 2]);
            Err(error)
        }
    }
}

#[derive(Debug)]
struct RenderTrack<'a> {
    index: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    send_inputs: Vec<usize>,
    sends: Vec<RenderSend>,
    output: bool,
    route_delay: DelayLine,
    solo_gain: SmoothedValue,
    gain: SmoothedValue,
    pan: SmoothedValue,
    automation_mute: SmoothedValue,
    automation: Vec<AutomationLane>,
    output_latency: usize,
    clips: Vec<RenderClip<'a>>,
    instrument: Option<RenderInstrument>,
    effects: Vec<RenderPlugin>,
}

#[derive(Debug)]
struct RenderSend {
    destination: usize,
    buffer: usize,
    gain: SmoothedValue,
    solo_gain: SmoothedValue,
    delay: DelayLine,
}

#[derive(Debug)]
struct RenderClip<'a> {
    start_frame: u64,
    samples: RenderAudio<'a>,
    gain: SmoothedValue,
}

#[derive(Debug)]
enum RenderAudio<'a> {
    Source(&'a [Frame]),
    Pitched {
        source: Arc<[Frame]>,
        range: std::ops::Range<usize>,
    },
}

impl std::ops::Deref for RenderAudio<'_> {
    type Target = [Frame];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Source(samples) => samples,
            Self::Pitched { source, range } => &source[range.clone()],
        }
    }
}

#[derive(Debug)]
struct ScheduledNote {
    start: u64,
    end: u64,
    on: NoteEvent,
}

#[derive(Debug)]
struct TimedEvent {
    frame: u64,
    event: NoteEvent,
}

#[derive(Debug)]
struct RenderPlugin {
    id: u64,
    processor: Box<dyn PluginProcessor>,
    parameters: Vec<(u32, f64)>,
    parameter_values: Vec<f64>,
    parameter_automation: Vec<bool>,
    parameters_dirty: bool,
    parameters_automated: bool,
    wet: SmoothedValue,
    sidechain: Option<SidechainRoute>,
    sidechain_gain: SmoothedValue,
    latency: usize,
    input_latency: usize,
    main_delay: DelayLine,
    sidechain_delay: DelayLine,
    dry_delay: DelayLine,
}

struct PluginDelayPlan {
    latency: usize,
    input_latency: usize,
    main: usize,
    sidechain: usize,
}

struct TrackDelayPlan {
    instrument: Option<PluginDelayPlan>,
    effects: Vec<PluginDelayPlan>,
    route: usize,
    sends: Vec<usize>,
}

fn checked_latency(value: usize) -> Result<usize, String> {
    if value > MAX_LATENCY_FRAMES {
        Err("plugin compensation path exceeds the ten-second latency limit".into())
    } else {
        Ok(value)
    }
}

fn loop_position(position: u64, range: Option<(u64, u64)>) -> u64 {
    match range {
        Some((start, end)) if position >= end => start + (position - start) % (end - start),
        _ => position,
    }
}

fn device_automation_active(lanes: &[AutomationLane]) -> bool {
    lanes.iter().any(|lane| {
        lane.enabled()
            && !lane.points().is_empty()
            && !matches!(
                lane.target(),
                AutomationTarget::TrackGain | AutomationTarget::TrackPan
            )
    })
}

fn automation_quantum_start(frame: u64, loop_range: Option<(u64, u64)>) -> u64 {
    let origin = loop_range.map_or(0, |(start, _)| if frame >= start { start } else { 0 });
    origin + (frame - origin) / AUTOMATION_FRAMES * AUTOMATION_FRAMES
}

fn automation_quantum_remaining(frame: u64, loop_range: Option<(u64, u64)>) -> usize {
    let remaining = AUTOMATION_FRAMES - (frame - automation_quantum_start(frame, loop_range));
    loop_range
        .filter(|(start, _)| frame < *start)
        .map_or(remaining, |(start, _)| remaining.min(start - frame)) as usize
}

impl RenderPlugin {
    fn new(settings: &PluginInstance, host: Option<&dyn PluginHost>) -> Result<Self, RenderError> {
        let processor =
            if crate::builtin::BuiltinKind::from_descriptor(&settings.descriptor).is_some() {
                match host {
                    Some(host) => host.load_builtin(settings),
                    None => crate::builtin::create_processor(settings),
                }
                .map_err(RenderError::Plugin)?
            } else if let Some(host) = host {
                host.load(settings).map_err(RenderError::Plugin)?
            } else {
                return Err(RenderError::Plugin(format!(
                    "{} requires a plugin host",
                    settings.descriptor.name
                )));
            };
        Ok(Self {
            id: settings.id,
            processor,
            parameters: settings
                .parameters
                .iter()
                .map(|p| (p.id, p.value))
                .collect(),
            parameter_values: settings.parameters.iter().map(|p| p.value).collect(),
            parameter_automation: vec![false; settings.parameters.len()],
            parameters_dirty: false,
            parameters_automated: false,
            wet: SmoothedValue::new(if settings.bypassed { 0.0 } else { 1.0 }),
            sidechain: settings.sidechain,
            sidechain_gain: SmoothedValue::new(settings.sidechain.map_or(1.0, |route| route.gain)),
            latency: 0,
            input_latency: 0,
            main_delay: DelayLine::default(),
            sidechain_delay: DelayLine::default(),
            dry_delay: DelayLine::default(),
        })
    }

    fn update(&mut self, settings: &PluginInstance) {
        for ((id, value), parameter) in self.parameters.iter_mut().zip(&settings.parameters) {
            debug_assert_eq!(*id, parameter.id);
            self.parameters_dirty |= *value != parameter.value;
            *value = parameter.value;
        }
        self.wet
            .set_target(if settings.bypassed { 0.0 } else { 1.0 });
        if let Some(route) = settings.sidechain {
            self.sidechain_gain.set_target(route.gain);
        }
    }

    fn automate(&mut self, lanes: &[AutomationLane], tick: Option<f64>) {
        let automated = tick.is_some() && lanes.iter().any(|lane| {
            matches!(lane.target(), AutomationTarget::PluginParameter { plugin, .. } if plugin == self.id)
                && lane.enabled()
                && !lane.points().is_empty()
        });
        if !automated && !self.parameters_automated && !self.parameters_dirty {
            return;
        }
        for (((id, base), current), was_automated) in self
            .parameters
            .iter()
            .zip(&mut self.parameter_values)
            .zip(&mut self.parameter_automation)
        {
            let automated_value = tick.and_then(|tick| {
                lanes
                    .iter()
                    .find(|lane| {
                        lane.target()
                            == AutomationTarget::PluginParameter {
                                plugin: self.id,
                                parameter: *id,
                            }
                    })
                    .and_then(|lane| lane.trimmed_value_at(tick, *base))
            });
            let value = automated_value.unwrap_or(*base);
            if automated_value.is_some() {
                self.processor.set_automation_parameter(*id, value);
            } else if *current != value || *was_automated {
                self.processor.set_parameter(*id, value);
            }
            *current = value;
            *was_automated = automated_value.is_some();
        }
        self.parameters_dirty = false;
        self.parameters_automated = automated;
    }

    fn process(
        &mut self,
        output: &mut [Frame],
        sidechain: Option<&mut [Frame]>,
        mut context: PluginProcessContext,
        loop_range: Option<(u64, u64)>,
    ) {
        if output.is_empty() {
            return;
        }
        debug_assert!(output.len() <= SCRATCH_FRAMES);
        let mut dry = [[0.0; 2]; SCRATCH_FRAMES];
        self.main_delay.process(output);
        dry[..output.len()].copy_from_slice(output);
        self.dry_delay.process(&mut dry[..output.len()]);
        let sidechain = sidechain.map(|frames| {
            self.sidechain_delay.process(frames);
            for frame in frames.iter_mut() {
                let gain = self.sidechain_gain.advance(1);
                for sample in frame {
                    *sample = if gain == 0.0 { 0.0 } else { *sample * gain };
                }
            }
            PluginSidechainInput {
                bus_index: self.sidechain.expect("configured sidechain").bus_index,
                frames,
            }
        });
        if context.playing {
            context.frame = loop_position(
                context.frame.saturating_sub(self.input_latency as u64),
                loop_range,
            );
        }
        self.processor
            .process_with_sidechain(output, sidechain, context);
        for (frame, dry) in output.iter_mut().zip(dry) {
            let wet = self.wet.advance(1);
            if wet == 0.0 {
                *frame = dry;
            } else if wet != 1.0 {
                for channel in 0..2 {
                    let dry = f64::from(dry[channel]);
                    frame[channel] =
                        (dry + (f64::from(frame[channel]) - dry) * f64::from(wet)) as f32;
                }
            }
        }
    }

    fn clear_delays(&mut self) {
        self.main_delay.clear();
        self.sidechain_delay.clear();
        self.dry_delay.clear();
    }
}

#[derive(Debug)]
enum RenderProcessor {
    Builtin(Box<dyn InstrumentProcessor>),
    Plugin(Box<RenderPlugin>),
}

impl RenderProcessor {
    fn reset(&mut self) {
        match self {
            Self::Builtin(processor) => processor.reset(),
            Self::Plugin(plugin) => plugin.processor.reset(),
        }
    }

    fn event(&mut self, event: NoteEvent) {
        match self {
            Self::Builtin(processor) => processor.event(event),
            Self::Plugin(plugin) => plugin.processor.event(event),
        }
    }

    fn process(
        &mut self,
        output: &mut [Frame],
        frame: u64,
        bpm: f64,
        playing: bool,
        quality: RenderQuality,
    ) {
        match self {
            Self::Builtin(processor) => processor.process(output),
            Self::Plugin(plugin) => plugin.process(
                output,
                None,
                PluginProcessContext {
                    frame,
                    bpm,
                    playing,
                    quality,
                },
                None,
            ),
        }
    }
}

#[derive(Debug)]
struct RenderInstrument {
    processor: RenderProcessor,
    builtin_settings: Option<Instrument>,
    applied_settings: Option<Instrument>,
    builtin_gain: SmoothedValue,
    notes: Vec<ScheduledNote>,
    events: Vec<TimedEvent>,
    bends: Vec<BendCurve>,
    quality: RenderQuality,
    /// A finished offline render of this instrument's notes. While set, scheduled notes
    /// play from it and the processor only serves live input and auditions.
    cached: Option<Arc<[Frame]>>,
    next_event: usize,
    next_frame: Option<u64>,
    monitoring: bool,
    auditions: Vec<AuditionNote>,
    live_notes: Vec<LiveNote>,
    sustain: Vec<(usize, [bool; 16])>,
    next_audition_id: u64,
    audition_tail: u64,
    release_frames: u64,
}

#[derive(Debug)]
struct AuditionNote {
    on: NoteEvent,
    remaining: Option<u64>,
    started: bool,
}

#[derive(Debug)]
struct LiveNote {
    source: usize,
    on: NoteEvent,
    started: bool,
    key_down: bool,
}

impl RenderInstrument {
    fn new(
        project: &Project,
        track: &MidiTrack,
        host: Option<&dyn PluginHost>,
    ) -> Result<Option<Self>, RenderError> {
        let processor = if let Some(instrument) = track.instrument() {
            RenderProcessor::Builtin(instrument.create_processor()?)
        } else if let Some(plugin) = track.plugin_instrument() {
            RenderProcessor::Plugin(Box::new(RenderPlugin::new(plugin, host)?))
        } else {
            return Ok(None);
        };
        let InstrumentSchedule {
            notes,
            events,
            bends,
        } = InstrumentSchedule::new(project.tempo(), track.clips())?;
        Ok(Some(Self {
            processor,
            builtin_settings: track.instrument().copied(),
            applied_settings: track.instrument().copied(),
            builtin_gain: SmoothedValue::new(if track.instrument_bypassed() {
                0.0
            } else {
                1.0
            }),
            notes,
            events,
            bends,
            quality: RenderQuality::Realtime,
            cached: None,
            next_event: 0,
            next_frame: None,
            monitoring: false,
            auditions: Vec::with_capacity(MAX_AUDITION_NOTES),
            live_notes: Vec::with_capacity(MAX_AUDITION_NOTES),
            sustain: Vec::with_capacity(MAX_AUDITION_NOTES),
            next_audition_id: u64::MAX,
            audition_tail: 0,
            release_frames: track
                .instrument()
                .map(Instrument::release_frames)
                .or_else(|| {
                    track.plugin_instrument().and_then(|plugin| {
                        crate::builtin::release_frames(plugin, track.automation())
                    })
                })
                .unwrap_or(0),
        }))
    }

    fn automate(&mut self, lanes: &[AutomationLane], tick: Option<f64>) {
        match &mut self.processor {
            RenderProcessor::Builtin(processor) => {
                let Some(mut settings) = self.builtin_settings else {
                    return;
                };
                let mut automated = false;
                if let Some(tick) = tick {
                    let Instrument::Subtractive(parameters) = &mut settings;
                    for lane in lanes {
                        if let AutomationTarget::Subtractive(parameter) = lane.target()
                            && let Some(value) =
                                lane.trimmed_value_at(tick, parameter.normalized_value(parameters))
                        {
                            parameter.set_normalized(parameters, value);
                            automated = true;
                        }
                    }
                }
                if self.applied_settings != Some(settings) {
                    let accepted = if automated {
                        processor.set_automation_parameters(&settings)
                    } else {
                        processor.set_parameters(&settings)
                    };
                    debug_assert!(accepted);
                    self.applied_settings = Some(settings);
                }
            }
            RenderProcessor::Plugin(plugin) => plugin.automate(lanes, tick),
        }
    }

    fn reset(&mut self, preserve_auditions: bool) {
        if preserve_auditions {
            for note in &mut self.auditions {
                note.started = false;
            }
            for note in &mut self.live_notes {
                note.started = false;
            }
        } else {
            self.release_auditions();
            self.release_live();
            self.audition_tail = 0;
        }
        self.processor.reset();
        self.next_frame = None;
        self.monitoring = false;
    }

    fn release_auditions(&mut self) -> bool {
        let mut released = false;
        let had_notes = !self.auditions.is_empty();
        for note in self.auditions.drain(..) {
            if note.started
                && let NoteEvent::On { id, .. } = note.on
            {
                self.processor.event(NoteEvent::Off { id });
                released = true;
            }
        }
        if had_notes {
            self.audition_tail = if released {
                self.release_frames
            } else {
                self.audition_tail.min(self.release_frames)
            };
        }
        released
    }

    fn audition(&mut self, notes: &[MidiNote], tempo: Tempo, held: bool) -> bool {
        for note in notes.iter().filter(|note| {
            note.key <= 127 && (1..=127).contains(&note.velocity) && note.channel <= 15
        }) {
            let remaining = if held {
                None
            } else {
                let Ok(duration) = tempo.ticks_to_frames(note.duration_ticks) else {
                    continue;
                };
                if duration == 0 {
                    continue;
                }
                Some(duration)
            };
            if let Some(old) = self.auditions.iter_mut().find(|old| {
                matches!(old.on, NoteEvent::On { key, channel, .. }
                    if key == note.key && channel == note.channel)
            }) {
                old.remaining = match (old.remaining, remaining) {
                    (Some(old), Some(new)) => Some(old.max(new)),
                    _ => None,
                };
                continue;
            }
            if self.auditions.len() == MAX_AUDITION_NOTES {
                continue;
            }
            self.auditions.push(AuditionNote {
                on: NoteEvent::On {
                    id: self.next_audition_id,
                    key: note.key,
                    velocity: note.velocity,
                    channel: note.channel,
                    slide_frames: 0,
                },
                remaining,
                started: false,
            });
            self.next_audition_id = self.next_audition_id.wrapping_sub(1);
        }
        if self.auditions.is_empty() {
            return false;
        }
        self.audition_tail = self.audition_tail.max(
            self.auditions
                .iter()
                .filter_map(|note| note.remaining)
                .max()
                .unwrap_or(0)
                .saturating_add(self.release_frames),
        );
        true
    }

    fn release_live(&mut self) {
        for note in self.live_notes.drain(..) {
            if note.started
                && let NoteEvent::On { id, .. } = note.on
            {
                self.processor.event(NoteEvent::Off { id });
            }
        }
        self.sustain.clear();
        self.audition_tail = self.audition_tail.max(self.release_frames);
    }

    fn live_midi(&mut self, source: usize, message: [u8; 3]) -> bool {
        let [status, key, velocity] = message;
        if key > 127 || velocity > 127 {
            return false;
        }
        let channel = status & 15;
        let kind = status & 0xf0;
        if kind == 0xe0 {
            let raw = i32::from(key) | i32::from(velocity) << 7;
            self.processor.event(NoteEvent::PitchBend {
                channel,
                value: crate::clip::bend_normalized((raw - 8192) as i16),
                ramp_frames: 0,
            });
            return true;
        }
        let note_on = kind == 0x90 && velocity != 0;
        let note_off = kind == 0x80 || kind == 0x90 && velocity == 0;
        let sustain = kind == 0xb0 && key == 64;
        let panic = kind == 0xb0 && (key == 120 || key == 123);
        if !(note_on || note_off || sustain || panic) {
            return false;
        }
        if sustain || panic {
            let down = sustain && velocity >= 64;
            if let Some((_, channels)) = self.sustain.iter_mut().find(|(id, _)| *id == source) {
                channels[usize::from(channel)] = down;
            } else if down {
                if self.sustain.len() == MAX_AUDITION_NOTES {
                    return false;
                }
                let mut channels = [false; 16];
                channels[usize::from(channel)] = true;
                self.sustain.push((source, channels));
            }
        }
        let sustained = self
            .sustain
            .iter()
            .any(|(id, channels)| *id == source && channels[usize::from(channel)]);
        self.live_notes.retain_mut(|note| {
            let NoteEvent::On {
                id,
                key: old_key,
                channel: old_channel,
                ..
            } = note.on
            else {
                return true;
            };
            if note.source != source || old_channel != channel {
                return true;
            }
            if (note_on || note_off) && old_key == key || panic {
                note.key_down = false;
            }
            let release = !note.key_down && (!sustained || note_on && old_key == key);
            if release && note.started {
                self.processor.event(NoteEvent::Off { id });
            }
            !release
        });
        if note_on && self.live_notes.len() < MAX_AUDITION_NOTES {
            self.live_notes.push(LiveNote {
                source,
                on: NoteEvent::On {
                    id: self.next_audition_id,
                    key,
                    velocity,
                    channel,
                    slide_frames: 0,
                },
                started: false,
                key_down: true,
            });
            self.next_audition_id = self.next_audition_id.wrapping_sub(1);
        }
        self.sustain
            .retain(|(_, channels)| channels.iter().any(|down| *down));
        self.audition_tail = self.audition_tail.max(self.release_frames);
        true
    }

    fn process(
        &mut self,
        output: &mut [Frame],
        frame: u64,
        bpm: f64,
        playing: bool,
        start_auditions: bool,
    ) {
        if output.is_empty() {
            return;
        }
        for note in &mut self.auditions {
            if !note.started && start_auditions {
                self.processor.event(note.on);
                note.started = true;
            }
        }
        for note in &mut self.live_notes {
            if !note.started && start_auditions {
                self.processor.event(note.on);
                note.started = true;
            }
        }
        let mut written = 0;
        while written < output.len() {
            let count = self
                .auditions
                .iter()
                .filter(|note| note.started)
                .filter_map(|note| note.remaining)
                .map(|remaining| remaining.min((output.len() - written) as u64) as usize)
                .min()
                .unwrap_or(output.len() - written)
                .min(output.len() - written);
            self.processor.process(
                &mut output[written..written + count],
                if playing {
                    frame + written as u64
                } else {
                    frame
                },
                bpm,
                playing,
                self.quality,
            );
            if matches!(self.processor, RenderProcessor::Builtin(_)) {
                for sample in &mut output[written..written + count] {
                    let gain = self.builtin_gain.advance(1);
                    for channel in sample {
                        *channel = if gain == 0.0 { 0.0 } else { *channel * gain };
                    }
                }
            }
            let advancing_auditions =
                self.auditions.is_empty() || self.auditions.iter().any(|note| note.started);
            for note in self.auditions.iter_mut().filter(|note| note.started) {
                let Some(remaining) = &mut note.remaining else {
                    continue;
                };
                *remaining -= count as u64;
                if *remaining == 0
                    && let NoteEvent::On { id, .. } = note.on
                {
                    self.processor.event(NoteEvent::Off { id });
                }
            }
            self.auditions.retain(|note| note.remaining != Some(0));
            if advancing_auditions {
                self.audition_tail = self.audition_tail.saturating_sub(count as u64);
            }
            written += count;
        }
    }

    fn begin_monitoring(&mut self) {
        if self.monitoring {
            return;
        }
        if let Some(position) = self.next_frame {
            for note in &self.notes {
                if note.start < position
                    && position <= note.end
                    && let NoteEvent::On { id, .. } = note.on
                {
                    self.processor.event(NoteEvent::Off { id });
                }
            }
        }
        self.next_frame = None;
        self.monitoring = true;
    }

    fn render(&mut self, start: u64, output: &mut [Frame], bpm: f64, start_auditions: bool) {
        if let Some(cache) = self.cached.clone() {
            self.render_cached(&cache, start, output, bpm, start_auditions);
            return;
        }
        if self.next_frame != Some(start) {
            if !self.monitoring {
                self.processor.reset();
                self.auditions.retain(|note| !note.started);
                self.live_notes.retain(|note| !note.started);
                if self.auditions.is_empty() {
                    self.audition_tail = 0;
                }
            }
            for bend in &self.bends {
                let (value, ramp) = bend.at(start);
                let channel = bend.channel;
                self.processor.event(NoteEvent::PitchBend {
                    channel,
                    value,
                    ramp_frames: 0,
                });
                if let Some((target, remaining)) = ramp {
                    self.processor.event(NoteEvent::PitchBend {
                        channel,
                        value: target,
                        ramp_frames: remaining,
                    });
                }
            }
            for note in &self.notes {
                if note.start < start && start < note.end {
                    // A chased note starts at its pitch; its slide is already over or cut.
                    let on = match note.on {
                        NoteEvent::On {
                            id,
                            key,
                            velocity,
                            channel,
                            ..
                        } => NoteEvent::On {
                            id,
                            key,
                            velocity,
                            channel,
                            slide_frames: 0,
                        },
                        other => other,
                    };
                    self.processor.event(on);
                }
            }
            self.next_event = self.events.partition_point(|event| event.frame < start);
        }
        self.monitoring = false;
        let end = start.saturating_add(output.len() as u64);
        let count = (end - start) as usize;
        let mut written = 0;
        while let Some(event) = self.events.get(self.next_event) {
            if event.frame >= end {
                break;
            }
            let offset = (event.frame - start) as usize;
            let event = event.event;
            self.process(
                &mut output[written..offset],
                start + written as u64,
                bpm,
                true,
                start_auditions,
            );
            self.processor.event(event);
            written = offset;
            self.next_event += 1;
        }
        self.process(
            &mut output[written..count],
            start + written as u64,
            bpm,
            true,
            start_auditions,
        );
        output[count..].fill([0.0; 2]);
        self.next_frame = Some(end);
    }
}

impl RenderInstrument {
    fn render_cached(
        &mut self,
        cache: &[Frame],
        start: u64,
        output: &mut [Frame],
        bpm: f64,
        start_auditions: bool,
    ) {
        if self.next_frame != Some(start) && !self.monitoring {
            self.processor.reset();
            self.auditions.retain(|note| !note.started);
            self.live_notes.retain(|note| !note.started);
            if self.auditions.is_empty() {
                self.audition_tail = 0;
            }
        }
        self.monitoring = false;
        let end = start.saturating_add(output.len() as u64);
        self.process(output, start, bpm, true, start_auditions);
        let from = usize::try_from(start)
            .unwrap_or(usize::MAX)
            .min(cache.len());
        let to = usize::try_from(end).unwrap_or(usize::MAX).min(cache.len());
        for (frame, rendered) in output.iter_mut().zip(&cache[from..to]) {
            frame[0] += rendered[0];
            frame[1] += rendered[1];
        }
        self.next_event = self.events.partition_point(|event| event.frame < end);
        self.next_frame = Some(end);
    }

    fn plugin_id(&self) -> Option<u64> {
        match &self.processor {
            RenderProcessor::Plugin(plugin) => Some(plugin.id),
            RenderProcessor::Builtin(_) => None,
        }
    }

    /// Switches between cached and live playback. A change forces the next block to
    /// resynchronise, so live playback picks up the notes already sounding.
    fn set_cached(&mut self, cached: Option<Arc<[Frame]>>) {
        let same = match (&self.cached, &cached) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.cached = cached;
            self.next_frame = None;
        }
    }
}

impl Drop for RenderInstrument {
    fn drop(&mut self) {
        self.release_auditions();
        self.release_live();
    }
}

struct InstrumentSchedule {
    notes: Vec<ScheduledNote>,
    events: Vec<TimedEvent>,
    bends: Vec<BendCurve>,
}

/// A channel's pitch bend as a piecewise-linear curve in frames. Two points on the
/// same frame are a jump. The curve holds its first and last values outside its points.
#[derive(Debug, Clone, Default, PartialEq)]
struct BendCurve {
    channel: u8,
    points: Vec<(u64, f32)>,
}

impl BendCurve {
    /// The bend at `frame` and, while ramping, the target and the frames left to reach it.
    fn at(&self, frame: u64) -> (f32, Option<(f32, u32)>) {
        let index = self.points.partition_point(|point| point.0 <= frame);
        let Some(&(from_frame, from)) = index.checked_sub(1).map(|i| &self.points[i]) else {
            return (0.0, None);
        };
        match self.points.get(index) {
            Some(&(to_frame, to)) if to != from => {
                let span = (to_frame - from_frame) as f64;
                let t = (frame - from_frame) as f64 / span;
                let value = from + (to - from) * t as f32;
                let remaining = u32::try_from(to_frame - frame).unwrap_or(u32::MAX);
                (value, Some((to, remaining)))
            }
            _ => (from, None),
        }
    }

    fn events(&self, events: &mut Vec<TimedEvent>) {
        let channel = self.channel;
        let mut current = 0.0;
        for (index, &(frame, value)) in self.points.iter().enumerate() {
            if value != current {
                events.push(TimedEvent {
                    frame,
                    event: NoteEvent::PitchBend {
                        channel,
                        value,
                        ramp_frames: 0,
                    },
                });
                current = value;
            }
            if let Some(&(next_frame, next)) = self.points.get(index + 1)
                && next_frame > frame
                && next != current
            {
                events.push(TimedEvent {
                    frame,
                    event: NoteEvent::PitchBend {
                        channel,
                        value: next,
                        ramp_frames: u32::try_from(next_frame - frame).unwrap_or(u32::MAX),
                    },
                });
                current = next;
            }
        }
    }
}

impl InstrumentSchedule {
    fn new(tempo: Tempo, clips: &[MidiClip]) -> Result<Self, TimeError> {
        let mut notes = Vec::new();
        let mut events = Vec::new();
        for clip in clips {
            for note in clip.audible_notes() {
                let start_tick = clip.start_tick + note.start_tick;
                let start = tempo.ticks_to_frames(start_tick)?;
                let end = tempo.ticks_to_frames(start_tick + note.duration_ticks)?;
                if start == end {
                    continue;
                }
                let id = notes.len() as u64;
                let slide_frames = if note.slide_ticks == 0 {
                    0
                } else {
                    let slide_end =
                        tempo.ticks_to_frames(start_tick + u64::from(note.slide_ticks))?;
                    u32::try_from(slide_end - start).unwrap_or(u32::MAX).max(1)
                };
                let on = NoteEvent::On {
                    id,
                    key: note.key,
                    velocity: note.velocity,
                    channel: note.channel,
                    slide_frames,
                };
                notes.push(ScheduledNote { start, end, on });
                events.push(TimedEvent {
                    frame: start,
                    event: on,
                });
                events.push(TimedEvent {
                    frame: end,
                    event: NoteEvent::Off { id },
                });
            }
        }
        let mut bends: Vec<BendCurve> = Vec::new();
        for clip in clips {
            let points = clip.audible_bend();
            if points.is_empty() {
                continue;
            }
            let clip_start = tempo.ticks_to_frames(clip.start_tick)?;
            let clip_end = tempo.ticks_to_frames(clip.start_tick + clip.length_ticks)?;
            let mut channels: Vec<u8> = clip.audible_notes().map(|note| note.channel).collect();
            channels.sort_unstable();
            channels.dedup();
            if channels.is_empty() {
                channels.push(0);
            }
            let mut curve = vec![(clip_start, points[0].normalized())];
            for point in &points {
                let frame = tempo.ticks_to_frames(clip.start_tick + point.tick)?;
                curve.push((frame.min(clip_end), point.normalized()));
            }
            let last = curve.last().map_or(0.0, |point| point.1);
            curve.push((clip_end, last));
            curve.push((clip_end, 0.0));
            for channel in channels {
                let index = match bends.iter().position(|bend| bend.channel == channel) {
                    Some(index) => index,
                    None => {
                        bends.push(BendCurve {
                            channel,
                            points: Vec::new(),
                        });
                        bends.len() - 1
                    }
                };
                bends[index].points.extend_from_slice(&curve);
            }
        }
        for bend in &mut bends {
            bend.points.sort_by_key(|point| point.0);
            bend.events(&mut events);
        }
        notes.sort_by_key(|note| note.start);
        events.sort_by_key(|event| match event.event {
            NoteEvent::Off { id } => (event.frame, 0, id),
            NoteEvent::PitchBend { .. } => (event.frame, 1, 0),
            NoteEvent::On { id, .. } => (event.frame, 2, id),
        });
        Ok(Self {
            notes,
            events,
            bends,
        })
    }
}

#[derive(Debug)]
pub struct Renderer<'a> {
    project: &'a Project,
    midi_clips: Vec<Vec<MidiClip>>,
    order: Vec<RenderTrack<'a>>,
    scratch: Vec<[Frame; SCRATCH_FRAMES]>,
    pre_effects: Vec<[Frame; SCRATCH_FRAMES]>,
    post_effects: Vec<[Frame; SCRATCH_FRAMES]>,
    routed: Vec<[Frame; SCRATCH_FRAMES]>,
    send_routed: Vec<[Frame; SCRATCH_FRAMES]>,
    track_peaks: Vec<Frame>,
    roots: Vec<usize>,
    duration_frames: u64,
    next_frame: Option<u64>,
    input_frame: Option<u64>,
    monitoring: bool,
    latency: usize,
    final_delay: DelayLine,
    replan: bool,
    latency_error: Option<String>,
    last_output: Frame,
    transition_from: Frame,
    transition_remaining: usize,
    loop_range: Option<(u64, u64)>,
    audition_output_remaining: u64,
    device_automation: bool,
    quality: RenderQuality,
}

impl<'a> Renderer<'a> {
    pub fn new(project: &'a Project) -> Result<Self, RenderError> {
        Self::prepare(project, None)
    }

    /// Plays these finished offline renders, keyed by plugin instrument id, in place of
    /// the instruments' scheduled notes. Instruments not listed play live.
    pub fn set_instrument_renders(&mut self, renders: &[(u64, Arc<[Frame]>)]) {
        for instrument in self
            .order
            .iter_mut()
            .filter_map(|step| step.instrument.as_mut())
        {
            let cached = instrument.plugin_id().and_then(|id| {
                renders
                    .iter()
                    .find(|(plugin, _)| *plugin == id)
                    .map(|(_, frames)| Arc::clone(frames))
            });
            instrument.set_cached(cached);
        }
    }

    /// Offline renders let devices use their slower, more accurate processing.
    pub fn set_quality(&mut self, quality: RenderQuality) {
        self.quality = quality;
        for instrument in self
            .order
            .iter_mut()
            .filter_map(|step| step.instrument.as_mut())
        {
            instrument.quality = quality;
        }
    }

    pub fn quality(&self) -> RenderQuality {
        self.quality
    }

    pub fn with_plugin_host(
        project: &'a Project,
        host: &dyn PluginHost,
    ) -> Result<Self, RenderError> {
        Self::prepare(project, Some(host))
    }

    fn prepare(project: &'a Project, host: Option<&dyn PluginHost>) -> Result<Self, RenderError> {
        let tracks = project.tracks();
        let solo_routes = solo_routes(tracks);
        let duration_frames = project.duration_frames()?;
        if host.is_none()
            && tracks.iter().any(|track| {
                track.all_effects().any(|plugin| crate::builtin::BuiltinKind::from_descriptor(&plugin.descriptor).is_none())
                    || matches!(track, Track::Midi(midi) if midi.plugin_instrument().is_some_and(|plugin| crate::builtin::BuiltinKind::from_descriptor(&plugin.descriptor).is_none()))
            })
        {
            return Err(RenderError::Plugin(
                "external devices require a plugin host".to_owned(),
            ));
        }

        let master = project.master_track_id().0;
        let mut children = vec![Vec::new(); tracks.len()];
        let mut send_inputs = vec![Vec::new(); tracks.len()];
        let mut send_sources = vec![Vec::new(); tracks.len()];
        let mut send_buffers = vec![Vec::new(); tracks.len()];
        let mut send_count = 0;
        let mut roots = Vec::new();
        for (index, track) in tracks.iter().enumerate() {
            match track.mixer().output() {
                AudioOutput::Master => children[master].push(index),
                AudioOutput::Group(group) => children[group.0].push(index),
                AudioOutput::Bus(bus) => children[bus.0].push(index),
                AudioOutput::Output => roots.push(index),
                AudioOutput::Disconnected => {}
            }
            for send in track.sends() {
                send_inputs[send.destination.0].push(send_count);
                send_sources[send.destination.0].push(index);
                send_buffers[index].push(send_count);
                send_count += 1;
            }
        }
        let indices = project
            .processing_order()
            .map_err(|error| RenderError::Plugin(error.to_string()))?;
        let mut needed = vec![false; tracks.len()];
        let mut pending = roots.clone();
        while let Some(index) = pending.pop() {
            if mem::replace(&mut needed[index], true) {
                continue;
            }
            pending.extend(children[index].iter().copied());
            pending.extend(send_sources[index].iter().copied());
            pending.extend(
                tracks[index]
                    .all_effects()
                    .filter_map(|effect| effect.sidechain.map(|route| route.source_track)),
            );
        }
        let mut order = Vec::with_capacity(indices.len());
        for index in indices.into_iter().filter(|&index| needed[index]) {
            let mixer = tracks[index].mixer();
            order.push(RenderTrack {
                index,
                parent: (match mixer.output() {
                    AudioOutput::Master => Some(master),
                    AudioOutput::Group(group) => Some(group.0),
                    AudioOutput::Bus(bus) => Some(bus.0),
                    AudioOutput::Output | AudioOutput::Disconnected => None,
                })
                .filter(|&parent| needed[parent]),
                children: mem::take(&mut children[index]),
                send_inputs: mem::take(&mut send_inputs[index]),
                sends: tracks[index]
                    .sends()
                    .iter()
                    .enumerate()
                    .filter(|(_, send)| needed[send.destination.0])
                    .map(|(position, send)| RenderSend {
                        destination: send.destination.0,
                        buffer: send_buffers[index][position],
                        gain: SmoothedValue::new(send.gain),
                        solo_gain: SmoothedValue::new(if solo_routes.sends[index][position] {
                            1.0
                        } else {
                            0.0
                        }),
                        delay: DelayLine::default(),
                    })
                    .collect(),
                output: mixer.output() == AudioOutput::Output,
                route_delay: DelayLine::default(),
                solo_gain: SmoothedValue::new(if solo_routes.main[index] { 1.0 } else { 0.0 }),
                gain: SmoothedValue::new(mixer.effective_gain()),
                pan: SmoothedValue::new(mixer.pan()),
                automation_mute: SmoothedValue::new(if mixer.muted() { 0.0 } else { 1.0 }),
                automation: tracks[index].automation().to_vec(),
                output_latency: 0,
                clips: match &tracks[index] {
                    Track::Audio(track) => track
                        .clips()
                        .iter()
                        .map(|clip| {
                            let source_start = clip.source_start as usize;
                            let source_end = source_start + clip.frame_count as usize;
                            let source = project.asset(clip.asset);
                            Ok(RenderClip {
                                start_frame: project.audio_clip_start_frame(clip),
                                samples: if clip.pitch_semitones == 0.0 {
                                    RenderAudio::Source(&source.samples()[source_start..source_end])
                                } else {
                                    RenderAudio::Pitched {
                                        source: source
                                            .processed_samples(clip)
                                            .map_err(RenderError::Pitch)?,
                                        range: source_start..source_end,
                                    }
                                },
                                gain: SmoothedValue::new(clip.gain),
                            })
                        })
                        .collect::<Result<Vec<_>, RenderError>>()?,
                    _ => Vec::new(),
                },
                instrument: match &tracks[index] {
                    Track::Midi(track) => RenderInstrument::new(project, track, host)?,
                    _ => None,
                },
                effects: tracks[index]
                    .all_effects()
                    .map(|settings| RenderPlugin::new(settings, host))
                    .collect::<Result<_, _>>()?,
            });
        }
        let mut renderer = Self {
            project,
            midi_clips: tracks
                .iter()
                .map(|track| match track {
                    Track::Midi(track) => track.clips().to_vec(),
                    _ => Vec::new(),
                })
                .collect(),
            order,
            scratch: vec![[[0.0; 2]; SCRATCH_FRAMES]; tracks.len()],
            pre_effects: vec![[[0.0; 2]; SCRATCH_FRAMES]; tracks.len()],
            post_effects: vec![[[0.0; 2]; SCRATCH_FRAMES]; tracks.len()],
            routed: vec![[[0.0; 2]; SCRATCH_FRAMES]; tracks.len()],
            send_routed: vec![[[0.0; 2]; SCRATCH_FRAMES]; send_count],
            track_peaks: vec![[0.0; 2]; tracks.len()],
            roots,
            duration_frames,
            next_frame: None,
            input_frame: None,
            monitoring: false,
            latency: 0,
            final_delay: DelayLine::default(),
            replan: true,
            latency_error: None,
            last_output: [0.0; 2],
            transition_from: [0.0; 2],
            transition_remaining: 0,
            loop_range: None,
            audition_output_remaining: 0,
            device_automation: tracks
                .iter()
                .any(|track| device_automation_active(track.automation())),
            quality: RenderQuality::Realtime,
        };
        renderer.refresh_latency().map_err(RenderError::Plugin)?;
        Ok(renderer)
    }

    pub fn update_instruments(&mut self, edited: &Project) -> bool {
        if !same_arrangement(self.project, edited, false, Some(&self.midi_clips)) {
            return false;
        }
        self.update_parameters(edited)
    }

    pub fn update_parameters(&mut self, edited: &Project) -> bool {
        if !same_arrangement(self.project, edited, true, Some(&self.midi_clips)) {
            return false;
        }
        self.update_runtime_parameters(edited)
    }

    pub fn update_midi_clips(&mut self, edited: &Project) -> bool {
        if !same_arrangement(self.project, edited, true, None) {
            return false;
        }
        let mut schedules = Vec::new();
        for step in &self.order {
            if let Track::Midi(track) = &edited.tracks()[step.index]
                && self.midi_clips[step.index] != track.clips()
                && step.instrument.is_some()
            {
                let Ok(schedule) = InstrumentSchedule::new(edited.tempo(), track.clips()) else {
                    return false;
                };
                schedules.push((step.index, schedule));
            }
        }
        if !self.update_runtime_parameters(edited) {
            return false;
        }
        for (index, schedule) in schedules {
            let instrument = self
                .order
                .iter_mut()
                .find(|step| step.index == index)
                .and_then(|step| step.instrument.as_mut())
                .expect("validated instrument remains present");
            instrument.begin_monitoring();
            instrument.notes = schedule.notes;
            instrument.events = schedule.events;
            instrument.bends = schedule.bends;
            instrument.next_event = 0;
        }
        for (clips, track) in self.midi_clips.iter_mut().zip(edited.tracks()) {
            if let Track::Midi(track) = track
                && clips != track.clips()
            {
                track.clips().clone_into(clips);
            }
        }
        true
    }

    fn update_runtime_parameters(&mut self, edited: &Project) -> bool {
        let Ok(duration_frames) = edited.duration_frames() else {
            return false;
        };
        let tracks = edited.tracks();
        let solo_routes = solo_routes(tracks);
        if self
            .order
            .iter()
            .filter_map(|step| midi_instrument(&tracks[step.index]))
            .any(|i| i.validate().is_err())
        {
            return false;
        }
        for step in &mut self.order {
            let track = &tracks[step.index];
            step.gain.set_target(track.mixer().effective_gain());
            step.automation_mute
                .set_target(if track.mixer().muted() { 0.0 } else { 1.0 });
            if step.automation != track.automation() {
                step.automation = track.automation().to_vec();
            }
            step.solo_gain.set_target(if solo_routes.main[step.index] {
                1.0
            } else {
                0.0
            });
            for send in &mut step.sends {
                let position = track
                    .sends()
                    .iter()
                    .position(|settings| settings.destination.0 == send.destination)
                    .expect("send topology checked before parameter update");
                send.gain.set_target(track.sends()[position].gain);
                send.solo_gain
                    .set_target(if solo_routes.sends[step.index][position] {
                        1.0
                    } else {
                        0.0
                    });
            }
            step.pan.set_target(track.mixer().pan());
            if let Track::Audio(audio) = track {
                for (clip, settings) in step.clips.iter_mut().zip(audio.clips()) {
                    clip.gain.set_target(settings.gain);
                }
            }
            if let (Some(instrument), Some(settings)) =
                (&mut step.instrument, midi_instrument(track))
            {
                instrument.builtin_settings = Some(*settings);
                instrument.release_frames = settings.release_frames();
                if instrument.audition_tail != 0 {
                    instrument.audition_tail = instrument.audition_tail.max(
                        instrument
                            .auditions
                            .iter()
                            .filter_map(|note| note.remaining)
                            .max()
                            .unwrap_or(0)
                            .saturating_add(instrument.release_frames),
                    );
                }
            }
            if let (Some(instrument), Track::Midi(midi)) = (&mut step.instrument, track) {
                instrument
                    .builtin_gain
                    .set_target(if midi.instrument_bypassed() { 0.0 } else { 1.0 });
                if let (RenderProcessor::Plugin(plugin), Some(settings)) =
                    (&mut instrument.processor, midi.plugin_instrument())
                {
                    plugin.update(settings);
                    if let Some(release) =
                        crate::builtin::release_frames(settings, midi.automation())
                    {
                        instrument.release_frames = release;
                        if instrument.audition_tail != 0 {
                            instrument.audition_tail = instrument.audition_tail.max(release);
                        }
                    }
                }
            }
            for (effect, settings) in step.effects.iter_mut().zip(track.all_effects()) {
                effect.update(settings);
            }
        }
        self.duration_frames = duration_frames;
        self.device_automation = self
            .order
            .iter()
            .any(|step| device_automation_active(&step.automation));
        true
    }

    pub fn duration_frames(&self) -> u64 {
        self.duration_frames
    }

    pub fn track_peaks(&self) -> &[Frame] {
        &self.track_peaks
    }

    pub fn latency_frames(&self) -> u64 {
        self.latency as u64
    }

    pub fn latency_error(&self) -> Option<&str> {
        self.latency_error.as_deref()
    }

    fn refresh_latency(&mut self) -> Result<(), String> {
        let changed = self.order.iter().any(|track| {
            track
                .effects
                .iter()
                .any(|plugin| plugin.latency != plugin.processor.latency_samples() as usize)
                || track.instrument.as_ref().is_some_and(|instrument| {
                    matches!(&instrument.processor, RenderProcessor::Plugin(plugin)
                        if plugin.latency != plugin.processor.latency_samples() as usize)
                })
        });
        if !changed && !self.final_delay.samples.is_empty() {
            if self.replan {
                self.latency -= self.final_delay.delay;
                self.final_delay.configure(0);
                self.replan = false;
            }
            return Ok(());
        }
        let mut taps = vec![[0usize; 3]; self.scratch.len()];
        let mut incoming = vec![0usize; self.scratch.len()];
        let mut plans = Vec::with_capacity(self.order.len());
        let mut root_latency = 0;
        for track in &self.order {
            let instrument = match track.instrument.as_ref().map(|i| &i.processor) {
                Some(RenderProcessor::Plugin(plugin)) => Some(PluginDelayPlan {
                    latency: checked_latency(plugin.processor.latency_samples() as usize)?,
                    input_latency: 0,
                    main: 0,
                    sidechain: 0,
                }),
                _ => None,
            };
            let mut latency =
                incoming[track.index].max(instrument.as_ref().map_or(0, |p| p.latency));
            taps[track.index][0] = latency;
            let mut effects = Vec::with_capacity(track.effects.len());
            for plugin in &track.effects {
                let sidechain_latency = plugin.sidechain.map_or(latency, |route| {
                    taps[route.source_track][match route.tap {
                        SidechainTap::PreEffects => 0,
                        SidechainTap::PostEffects => 1,
                        SidechainTap::PostFader => 2,
                    }]
                });
                let input_latency = latency.max(sidechain_latency);
                let intrinsic = checked_latency(plugin.processor.latency_samples() as usize)?;
                effects.push(PluginDelayPlan {
                    latency: intrinsic,
                    input_latency,
                    main: input_latency - latency,
                    sidechain: input_latency - sidechain_latency,
                });
                latency = checked_latency(input_latency + intrinsic)?;
            }
            taps[track.index][1] = latency;
            taps[track.index][2] = latency;
            if let Some(parent) = track.parent {
                incoming[parent] = incoming[parent].max(latency);
            } else if track.output {
                root_latency = root_latency.max(latency);
            }
            for send in &track.sends {
                incoming[send.destination] = incoming[send.destination].max(latency);
            }
            plans.push(TrackDelayPlan {
                instrument,
                effects,
                route: 0,
                sends: Vec::with_capacity(track.sends.len()),
            });
        }
        let effective = if self.replan {
            root_latency
        } else {
            self.latency.max(root_latency)
        };
        let mut storage = self
            .final_delay
            .samples
            .len()
            .max((effective - root_latency + 1).max(SCRATCH_FRAMES));
        for (track, plan) in self.order.iter().zip(&mut plans) {
            plan.route = if let Some(parent) = track.parent {
                incoming[parent] - taps[track.index][2]
            } else if track.output {
                root_latency - taps[track.index][2]
            } else {
                0
            };
            storage += track
                .route_delay
                .samples
                .len()
                .max((plan.route + 1).max(SCRATCH_FRAMES));
            for send in &track.sends {
                let delay = incoming[send.destination] - taps[track.index][2];
                plan.sends.push(delay);
                storage += send
                    .delay
                    .samples
                    .len()
                    .max((delay + 1).max(SCRATCH_FRAMES));
            }
            let instrument = track.instrument.as_ref().and_then(|i| match &i.processor {
                RenderProcessor::Plugin(plugin) => Some(plugin.as_ref()),
                _ => None,
            });
            for (plugin, plan) in instrument
                .into_iter()
                .zip(plan.instrument.iter())
                .chain(track.effects.iter().zip(&plan.effects))
            {
                for (delay, length) in [
                    (&plugin.main_delay, plan.main),
                    (&plugin.sidechain_delay, plan.sidechain),
                    (&plugin.dry_delay, plan.latency),
                ] {
                    storage += delay.samples.len().max((length + 1).max(SCRATCH_FRAMES));
                }
            }
        }
        if storage > MAX_DELAY_STORAGE_FRAMES {
            return Err("plugin compensation buffers exceed the 44 MiB storage limit".into());
        }
        for (track, plan) in self.order.iter_mut().zip(plans) {
            track.route_delay.configure(plan.route);
            for (send, delay) in track.sends.iter_mut().zip(plan.sends) {
                send.delay.configure(delay);
            }
            track.output_latency = taps[track.index][2];
            let instrument = track
                .instrument
                .as_mut()
                .and_then(|i| match &mut i.processor {
                    RenderProcessor::Plugin(plugin) => Some(plugin.as_mut()),
                    _ => None,
                });
            for (plugin, plan) in instrument
                .into_iter()
                .zip(plan.instrument)
                .chain(track.effects.iter_mut().zip(plan.effects))
            {
                plugin.latency = plan.latency;
                plugin.input_latency = plan.input_latency;
                plugin.main_delay.configure(plan.main);
                plugin.sidechain_delay.configure(plan.sidechain);
                plugin.dry_delay.configure(plan.latency);
            }
        }
        self.final_delay.configure(effective - root_latency);
        if !self.replan && (self.input_frame.is_some() || self.monitoring) {
            self.transition_from = self.last_output;
            self.transition_remaining = PARAMETER_RAMP_FRAMES;
        }
        self.latency = effective;
        self.replan = false;
        Ok(())
    }

    pub fn has_live_plugins(&self) -> bool {
        self.order.iter().any(|track| {
            !track.effects.is_empty()
                || track.instrument.as_ref().is_some_and(|instrument| {
                    matches!(instrument.processor, RenderProcessor::Plugin(_))
                })
        })
    }

    pub fn audition_notes(&mut self, track: usize, notes: &[MidiNote]) -> bool {
        self.start_audition_notes(track, notes, false)
    }

    pub fn begin_audition_notes(&mut self, track: usize, notes: &[MidiNote]) -> bool {
        self.start_audition_notes(track, notes, true)
    }

    fn start_audition_notes(&mut self, track: usize, notes: &[MidiNote], held: bool) -> bool {
        self.release_audition_notes();
        let Some(instrument) = self
            .order
            .iter_mut()
            .find(|step| step.index == track)
            .and_then(|step| step.instrument.as_mut())
        else {
            return false;
        };
        if !instrument.audition(notes, self.project.tempo(), held) {
            return false;
        }
        self.audition_output_remaining = self
            .audition_output_remaining
            .max(instrument.audition_tail.saturating_add(self.latency as u64));
        true
    }

    pub fn release_audition_notes(&mut self) {
        let mut released = false;
        let mut tail = 0;
        for step in &mut self.order {
            if let Some(instrument) = &mut step.instrument {
                released |= instrument.release_auditions();
                tail = tail.max(instrument.audition_tail);
            }
        }
        let output_tail = tail.saturating_add(self.latency as u64);
        self.audition_output_remaining = if released {
            output_tail
        } else {
            self.audition_output_remaining.min(output_tail)
        };
    }

    pub fn live_midi(&mut self, track: usize, message: [u8; 3]) -> bool {
        self.live_midi_from(track, 0, message)
    }

    pub fn live_midi_from(&mut self, track: usize, source: usize, message: [u8; 3]) -> bool {
        let Some(instrument) = self
            .order
            .iter_mut()
            .find(|step| step.index == track)
            .and_then(|step| step.instrument.as_mut())
        else {
            return false;
        };
        if !instrument.live_midi(source, message) {
            return false;
        }
        self.audition_output_remaining = self.audition_output_remaining.max(
            instrument
                .release_frames
                .saturating_add(self.latency as u64),
        );
        true
    }

    pub fn release_live_midi(&mut self) {
        for step in &mut self.order {
            if let Some(instrument) = &mut step.instrument {
                instrument.release_live();
                self.audition_output_remaining = self.audition_output_remaining.max(
                    instrument
                        .release_frames
                        .saturating_add(self.latency as u64),
                );
            }
        }
    }

    pub fn has_auditions(&self) -> bool {
        self.audition_output_remaining != 0
            || self.order.iter().any(|step| {
                step.instrument.as_ref().is_some_and(|instrument| {
                    instrument.audition_tail != 0
                        || !instrument.auditions.is_empty()
                        || !instrument.live_notes.is_empty()
                })
            })
    }

    pub fn reset(&mut self) {
        self.reset_devices(false);
    }

    pub(crate) fn reset_for_loop(&mut self, start: u64, end: u64) {
        if self.has_auditions()
            && self.loop_range == Some((start, end))
            && self.next_frame == Some(end)
        {
            self.next_frame = Some(start);
            self.input_frame = self
                .input_frame
                .map(|frame| frame.saturating_sub(end - start));
            return;
        }
        self.reset_devices(true);
    }

    fn reset_devices(&mut self, preserve_auditions: bool) {
        if !preserve_auditions {
            self.audition_output_remaining = 0;
        }
        self.next_frame = None;
        self.input_frame = None;
        self.monitoring = false;
        self.replan = true;
        self.transition_remaining = 0;
        self.clear_delays();
        for track in &mut self.order {
            if let Some(instrument) = &mut track.instrument {
                instrument.reset(preserve_auditions);
            }
            for effect in &mut track.effects {
                effect.processor.reset();
            }
        }
    }

    fn clear_delays(&mut self) {
        self.final_delay.clear();
        for track in &mut self.order {
            track.route_delay.clear();
            for send in &mut track.sends {
                send.delay.clear();
            }
            if let Some(instrument) = &mut track.instrument
                && let RenderProcessor::Plugin(plugin) = &mut instrument.processor
            {
                plugin.clear_delays();
            }
            for effect in &mut track.effects {
                effect.clear_delays();
            }
        }
    }

    pub fn render_block(&mut self, start_frame: u64, output: &mut [Frame]) {
        self.track_peaks.fill([0.0; 2]);
        self.render_timeline(start_frame, output, None);
    }

    pub(crate) fn render_loop_block(
        &mut self,
        start_frame: u64,
        output: &mut [Frame],
        start: u64,
        end: u64,
    ) {
        self.track_peaks.fill([0.0; 2]);
        self.render_timeline(start_frame, output, Some((start, end)));
    }

    fn render_timeline(
        &mut self,
        start_frame: u64,
        output: &mut [Frame],
        loop_range: Option<(u64, u64)>,
    ) {
        if output.is_empty() {
            return;
        }
        let discontinuity = self.next_frame != Some(start_frame) || self.loop_range != loop_range;
        self.loop_range = loop_range;
        let resuming_monitor = self.monitoring;
        if discontinuity && !resuming_monitor {
            self.clear_delays();
            self.replan = true;
            self.input_frame = None;
            self.transition_remaining = 0;
            for track in &mut self.order {
                if let Some(instrument) = &mut track.instrument {
                    instrument.next_frame = None;
                    instrument.monitoring = false;
                }
                for effect in &mut track.effects {
                    effect.processor.reset();
                }
            }
        }
        if let Err(error) = self.refresh_latency() {
            self.latency_error = Some(error);
            output.fill([0.0; 2]);
            return;
        }
        self.latency_error = None;
        let target_input = start_frame.saturating_add(self.latency as u64);
        let mut input = match (discontinuity, self.input_frame) {
            (false, Some(input)) => input,
            _ if resuming_monitor => start_frame,
            _ => start_frame.saturating_sub(self.latency as u64),
        };
        let mut discard = [[0.0; 2]; SCRATCH_FRAMES];
        while input < target_input {
            let mut count = (target_input - input).min(SCRATCH_FRAMES as u64) as usize;
            if input < start_frame {
                count = count.min((start_frame - input) as usize);
            }
            self.render_prepared(
                input,
                &mut discard[..count],
                true,
                input >= start_frame,
                false,
            );
            input += count as u64;
        }
        self.monitoring = false;
        self.next_frame = start_frame.checked_add(output.len() as u64);
        self.input_frame = target_input.checked_add(output.len() as u64);
        self.render_prepared(target_input, output, true, true, true);
        self.finish_output(output);
    }

    fn finish_output(&mut self, output: &mut [Frame]) {
        self.audition_output_remaining = self
            .audition_output_remaining
            .saturating_sub(output.len() as u64);
        for frame in output {
            if self.transition_remaining != 0 {
                let amount = 1.0 - self.transition_remaining as f64 / PARAMETER_RAMP_FRAMES as f64;
                for (sample, previous) in frame.iter_mut().zip(self.transition_from) {
                    *sample = (f64::from(previous)
                        + (f64::from(*sample) - f64::from(previous)) * amount)
                        as f32;
                }
                self.transition_remaining -= 1;
            }
            self.last_output = *frame;
        }
    }

    pub fn render_monitor_block(&mut self, position: u64, output: &mut [Frame]) {
        self.track_peaks.fill([0.0; 2]);
        if output.is_empty() {
            return;
        }
        self.begin_monitoring();
        if let Err(error) = self.refresh_latency() {
            self.latency_error = Some(error);
            output.fill([0.0; 2]);
            return;
        }
        self.latency_error = None;
        self.render_prepared(position, output, false, true, true);
        self.finish_output(output);
    }

    pub(crate) fn begin_monitoring(&mut self) {
        if !self.monitoring {
            for track in &mut self.order {
                if let Some(instrument) = &mut track.instrument {
                    instrument.begin_monitoring();
                }
            }
            self.next_frame = None;
            self.input_frame = None;
            self.monitoring = true;
        }
    }

    fn render_prepared(
        &mut self,
        start_frame: u64,
        output: &mut [Frame],
        playing: bool,
        start_auditions: bool,
        capture_peaks: bool,
    ) {
        output.fill([0.0; 2]);
        let ticks_per_frame =
            self.project.tempo().bpm() * TICKS_PER_QUARTER as f64 / (60.0 * f64::from(SAMPLE_RATE));
        let mut written = 0;
        while written < output.len() {
            let block_start = if playing {
                let Some(start) = start_frame.checked_add(written as u64) else {
                    break;
                };
                start
            } else {
                start_frame
            };
            let source_start = if playing {
                loop_position(block_start, self.loop_range)
            } else {
                block_start
            };
            let mut count = SCRATCH_FRAMES.min(output.len() - written);
            if playing {
                count = count.min((u64::MAX - block_start).min(usize::MAX as u64) as usize);
                if let Some((_, end)) = self.loop_range {
                    count = count.min((end - source_start).min(usize::MAX as u64) as usize);
                }
                if self.device_automation {
                    count = count.min(automation_quantum_remaining(source_start, self.loop_range));
                }
                for effect in self.order.iter().flat_map(|track| &track.effects) {
                    if effect.input_latency as u64 > block_start {
                        count = count.min((effect.input_latency as u64 - block_start) as usize);
                    } else if let Some((_, end)) = self.loop_range {
                        let input_position = loop_position(
                            block_start - effect.input_latency as u64,
                            self.loop_range,
                        );
                        count = count.min((end - input_position).min(usize::MAX as u64) as usize);
                    }
                    if self.device_automation {
                        let input_position = loop_position(
                            block_start.saturating_sub(effect.input_latency as u64),
                            self.loop_range,
                        );
                        count = count.min(automation_quantum_remaining(
                            input_position,
                            self.loop_range,
                        ));
                    }
                }
            }
            if count == 0 {
                break;
            }
            let block = &mut output[written..written + count];
            for step in &self.order {
                self.scratch[step.index][..block.len()].fill([0.0; 2]);
            }
            for step in &mut self.order {
                for &child in &step.children {
                    for (out, source) in self.scratch[step.index][..block.len()]
                        .iter_mut()
                        .zip(&self.routed[child])
                    {
                        for (out, sample) in out.iter_mut().zip(source) {
                            *out += sample;
                        }
                    }
                }
                for &input in &step.send_inputs {
                    for (out, source) in self.scratch[step.index][..block.len()]
                        .iter_mut()
                        .zip(&self.send_routed[input])
                    {
                        for (out, sample) in out.iter_mut().zip(source) {
                            *out += sample;
                        }
                    }
                }
                if playing {
                    mix_clips(
                        &mut step.clips,
                        source_start,
                        &mut self.scratch[step.index][..block.len()],
                    );
                } else {
                    for clip in &mut step.clips {
                        clip.gain.advance(block.len());
                    }
                }
                if let Some(instrument) = &mut step.instrument {
                    instrument.automate(
                        &step.automation,
                        playing.then(|| {
                            automation_quantum_start(source_start, self.loop_range) as f64
                                * ticks_per_frame
                        }),
                    );
                    let output = &mut self.scratch[step.index][..block.len()];
                    if playing {
                        if self.loop_range.is_some()
                            && instrument.next_frame.is_some()
                            && instrument.next_frame != Some(source_start)
                        {
                            instrument.begin_monitoring();
                        }
                        instrument.render(
                            source_start,
                            output,
                            self.project.tempo().bpm(),
                            start_auditions,
                        );
                    } else {
                        instrument.process(
                            output,
                            block_start,
                            self.project.tempo().bpm(),
                            false,
                            start_auditions,
                        );
                    }
                }
                self.pre_effects[step.index][..block.len()]
                    .copy_from_slice(&self.scratch[step.index][..block.len()]);
                for effect in &mut step.effects {
                    effect.automate(
                        &step.automation,
                        playing.then(|| {
                            let position = loop_position(
                                block_start.saturating_sub(effect.input_latency as u64),
                                self.loop_range,
                            );
                            automation_quantum_start(position, self.loop_range) as f64
                                * ticks_per_frame
                        }),
                    );
                    let mut sidechain = [[0.0; 2]; SCRATCH_FRAMES];
                    let sidechain_input = effect.sidechain.map(|route| {
                        let source = match route.tap {
                            SidechainTap::PreEffects => &self.pre_effects[route.source_track],
                            SidechainTap::PostEffects => &self.post_effects[route.source_track],
                            SidechainTap::PostFader => &self.scratch[route.source_track],
                        };
                        sidechain[..block.len()].copy_from_slice(&source[..block.len()]);
                        &mut sidechain[..block.len()]
                    });
                    effect.process(
                        &mut self.scratch[step.index][..block.len()],
                        sidechain_input,
                        PluginProcessContext {
                            frame: block_start,
                            bpm: self.project.tempo().bpm(),
                            playing,
                            quality: self.quality,
                        },
                        if playing { self.loop_range } else { None },
                    );
                }
                self.post_effects[step.index][..block.len()]
                    .copy_from_slice(&self.scratch[step.index][..block.len()]);
                let gain_lane = playing
                    .then(|| {
                        step.automation
                            .iter()
                            .find(|lane| lane.target() == AutomationTarget::TrackGain)
                    })
                    .flatten();
                let pan_lane = playing
                    .then(|| {
                        step.automation
                            .iter()
                            .find(|lane| lane.target() == AutomationTarget::TrackPan)
                    })
                    .flatten();
                for (offset, input) in self.scratch[step.index][..block.len()]
                    .iter_mut()
                    .enumerate()
                {
                    let manual_gain = step.gain.advance(1);
                    let manual_pan = step.pan.advance(1);
                    let mute = step.automation_mute.advance(1);
                    let position = loop_position(
                        block_start
                            .saturating_add(offset as u64)
                            .saturating_sub(step.output_latency as u64),
                        self.loop_range,
                    );
                    let tick = position as f64 * ticks_per_frame;
                    let gain = gain_lane
                        .and_then(|lane| lane.trimmed_value_at(tick, f64::from(manual_gain) / 2.0))
                        .map_or(manual_gain, |value| value as f32 * 2.0 * mute);
                    let pan = pan_lane
                        .and_then(|lane| {
                            lane.trimmed_value_at(tick, (f64::from(manual_pan) + 1.0) / 2.0)
                        })
                        .map_or(manual_pan, |value| value as f32 * 2.0 - 1.0);
                    let balance = [1.0 - pan.max(0.0), 1.0 + pan.min(0.0)];
                    for (sample, balance) in input.iter_mut().zip(balance) {
                        *sample = if gain == 0.0 || balance == 0.0 {
                            0.0
                        } else {
                            *sample * gain * balance
                        };
                    }
                }
                if capture_peaks {
                    let peaks = &mut self.track_peaks[step.index];
                    for frame in &self.scratch[step.index][..block.len()] {
                        for (peak, sample) in peaks.iter_mut().zip(frame) {
                            if sample.is_finite() {
                                *peak = peak.max(sample.abs());
                            }
                        }
                    }
                }
                for send in &mut step.sends {
                    let output = &mut self.send_routed[send.buffer][..block.len()];
                    output.copy_from_slice(&self.scratch[step.index][..block.len()]);
                    for frame in output.iter_mut() {
                        let gain = send.gain.advance(1);
                        for sample in frame {
                            *sample = if gain == 0.0 { 0.0 } else { *sample * gain };
                        }
                    }
                    send.delay.process(output);
                    for frame in output {
                        let gain = send.solo_gain.advance(1);
                        for sample in frame {
                            *sample = if gain == 0.0 { 0.0 } else { *sample * gain };
                        }
                    }
                }
                self.routed[step.index] = self.scratch[step.index];
                step.route_delay
                    .process(&mut self.routed[step.index][..block.len()]);
                for frame in &mut self.routed[step.index][..block.len()] {
                    let gain = step.solo_gain.advance(1);
                    for sample in frame {
                        *sample = if gain == 0.0 { 0.0 } else { *sample * gain };
                    }
                }
            }
            for &root in &self.roots {
                for (destination, source) in block.iter_mut().zip(&self.routed[root]) {
                    for (out, sample) in destination.iter_mut().zip(source) {
                        *out += sample;
                    }
                }
            }
            self.final_delay.process(block);
            written += count;
        }
    }
}

struct SoloRoutes {
    main: Vec<bool>,
    sends: Vec<Vec<bool>>,
}

fn solo_routes(tracks: &[Track]) -> SoloRoutes {
    let mut result = SoloRoutes {
        main: vec![false; tracks.len()],
        sends: tracks
            .iter()
            .map(|track| vec![false; track.sends().len()])
            .collect(),
    };
    let solos: Vec<_> = tracks
        .iter()
        .enumerate()
        .filter_map(|(index, track)| track.mixer().soloed().then_some(index))
        .collect();
    if solos.is_empty() {
        result.main.fill(true);
        for sends in &mut result.sends {
            sends.fill(true);
        }
        return result;
    }
    let destinations: Vec<_> = tracks
        .iter()
        .map(|track| match track.mixer().output() {
            AudioOutput::Master => Some(0),
            AudioOutput::Group(group) => Some(group.0),
            AudioOutput::Bus(bus) => Some(bus.0),
            AudioOutput::Output | AudioOutput::Disconnected => None,
        })
        .collect();
    let mut incoming = vec![Vec::new(); tracks.len()];
    for (source, track) in tracks.iter().enumerate() {
        if let Some(destination) = destinations[source] {
            incoming[destination].push((source, None));
        }
        for (position, send) in track.sends().iter().enumerate() {
            incoming[send.destination.0].push((source, Some(position)));
        }
    }
    let mut audible = vec![false; tracks.len()];
    let mut origins = Vec::new();
    for solo in solos {
        let bus = matches!(tracks[solo], Track::Bus(_));
        let mut visited = vec![false; tracks.len()];
        let mut pending = vec![solo];
        while let Some(index) = pending.pop() {
            if mem::replace(&mut visited[index], true) {
                continue;
            }
            audible[index] = true;
            if !bus || index == solo {
                origins.push(index);
            }
            for &(source, send) in &incoming[index] {
                match send {
                    Some(position) => result.sends[source][position] = true,
                    None => result.main[source] = true,
                }
                pending.push(source);
            }
        }
    }
    let mut visited = vec![false; tracks.len()];
    while let Some(source) = origins.pop() {
        if mem::replace(&mut visited[source], true) {
            continue;
        }
        audible[source] = true;
        if let Some(destination) = destinations[source] {
            result.main[source] = true;
            origins.push(destination);
        } else if tracks[source].mixer().output() == AudioOutput::Output {
            result.main[source] = true;
        }
        for (position, send) in tracks[source].sends().iter().enumerate() {
            result.sends[source][position] = true;
            origins.push(send.destination.0);
        }
    }
    for (source, track) in tracks.iter().enumerate() {
        if destinations[source].is_some_and(|destination| !audible[destination]) {
            result.main[source] = true;
        }
        for (position, send) in track.sends().iter().enumerate() {
            if !audible[send.destination.0] {
                result.sends[source][position] = true;
            }
        }
    }
    result
}

fn midi_instrument(track: &Track) -> Option<&Instrument> {
    match track {
        Track::Midi(track) => track.instrument(),
        _ => None,
    }
}

fn same_arrangement(
    prepared: &Project,
    edited: &Project,
    allow_mix_edits: bool,
    midi_clips: Option<&[Vec<MidiClip>]>,
) -> bool {
    let same_assets = prepared.assets().len() == edited.assets().len()
        && prepared
            .assets()
            .iter()
            .zip(edited.assets())
            .all(|(old, new)| Arc::ptr_eq(old, new));
    let same_tracks = prepared.tracks().len() == edited.tracks().len()
        && prepared
            .tracks()
            .iter()
            .zip(edited.tracks())
            .enumerate()
            .all(|(index, (old, new))| {
                (if allow_mix_edits {
                    old.mixer().output() == new.mixer().output()
                } else {
                    old.mixer() == new.mixer()
                }) && same_plugins(old.effects(), new.effects(), allow_mix_edits)
                    && same_plugins(old.mixer_effects(), new.mixer_effects(), allow_mix_edits)
                    && old.sends().len() == new.sends().len()
                    && old.sends().iter().zip(new.sends()).all(|(old, new)| {
                        old.destination == new.destination
                            && (allow_mix_edits || old.gain == new.gain)
                    })
                    && match (old, new) {
                        (Track::Audio(old), Track::Audio(new)) => {
                            old.clips().len() == new.clips().len()
                                && old.clips().iter().zip(new.clips()).all(|(old, new)| {
                                    old.asset == new.asset
                                        && old.source_start == new.source_start
                                        && old.start_tick == new.start_tick
                                        && old.frame_count == new.frame_count
                                        && old.pitch_semitones == new.pitch_semitones
                                        && old.pitch_algorithm == new.pitch_algorithm
                                        && old.preserve_formants == new.preserve_formants
                                        && (allow_mix_edits || old.gain == new.gain)
                                })
                        }
                        (Track::Midi(old), Track::Midi(new)) => {
                            midi_clips.is_none_or(|clips| clips[index] == new.clips())
                                && match (old.plugin_instrument(), new.plugin_instrument()) {
                                    (None, None) => true,
                                    (Some(old), Some(new)) => {
                                        same_plugin(old, new, allow_mix_edits)
                                    }
                                    _ => false,
                                }
                                && match (old.instrument(), new.instrument()) {
                                    (None, None) => true,
                                    (Some(old), Some(new)) => {
                                        mem::discriminant(old) == mem::discriminant(new)
                                    }
                                    _ => false,
                                }
                        }
                        (Track::Group(_), Track::Group(_))
                        | (Track::Bus(_), Track::Bus(_))
                        | (Track::Master(_), Track::Master(_)) => true,
                        _ => false,
                    }
            });
    prepared.tempo() == edited.tempo() && same_assets && same_tracks
}

fn same_plugins(
    prepared: &[PluginInstance],
    edited: &[PluginInstance],
    allow_parameters: bool,
) -> bool {
    prepared.len() == edited.len()
        && prepared
            .iter()
            .zip(edited)
            .all(|(old, new)| same_plugin(old, new, allow_parameters))
}

fn same_plugin(prepared: &PluginInstance, edited: &PluginInstance, allow_parameters: bool) -> bool {
    if !allow_parameters {
        return prepared == edited;
    }
    prepared.id == edited.id
        && prepared.descriptor.path == edited.descriptor.path
        && prepared.descriptor.class_id == edited.descriptor.class_id
        && prepared.descriptor.kind == edited.descriptor.kind
        && prepared.state == edited.state
        && match (prepared.sidechain, edited.sidechain) {
            (None, None) => true,
            (Some(old), Some(new)) => {
                old.source_track == new.source_track
                    && old.bus_index == new.bus_index
                    && old.tap == new.tap
                    && new.gain.is_finite()
                    && (0.0..=4.0).contains(&new.gain)
            }
            _ => false,
        }
        && prepared.parameters.len() == edited.parameters.len()
        && prepared
            .parameters
            .iter()
            .zip(&edited.parameters)
            .all(|(old, new)| {
                old.id == new.id
                    && old.steps == new.steps
                    && old.read_only == new.read_only
                    && new.value.is_finite()
                    && (0.0..=1.0).contains(&new.value)
            })
}

fn mix_clips(clips: &mut [RenderClip<'_>], start_frame: u64, output: &mut [Frame]) {
    let block_end = start_frame.saturating_add(output.len() as u64);
    for clip in clips {
        let clip_end = clip.start_frame + clip.samples.len() as u64;
        let overlap_start = start_frame.max(clip.start_frame);
        let overlap_end = block_end.min(clip_end);
        if overlap_start >= overlap_end {
            clip.gain.advance(output.len());
            continue;
        }

        let output_start = (overlap_start - start_frame) as usize;
        let source_start = (overlap_start - clip.start_frame) as usize;
        let count = (overlap_end - overlap_start) as usize;
        clip.gain.advance(output_start);
        let source = &clip.samples[source_start..source_start + count];
        let destination = &mut output[output_start..output_start + count];
        for (out, input) in destination.iter_mut().zip(source) {
            let gain = clip.gain.advance(1);
            out[0] += input[0] * gain;
            out[1] += input[1] * gain;
        }
        clip.gain.advance(output.len() - output_start - count);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    Time(TimeError),
    Instrument(InstrumentError),
    Plugin(String),
    Pitch(crate::pitch::PitchError),
}

impl From<TimeError> for RenderError {
    fn from(error: TimeError) -> Self {
        Self::Time(error)
    }
}

impl From<InstrumentError> for RenderError {
    fn from(error: InstrumentError) -> Self {
        Self::Instrument(error)
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Time(error) => write!(f, "cannot prepare musical timeline: {error}"),
            Self::Instrument(error) => write!(f, "cannot prepare instrument: {error}"),
            Self::Plugin(error) => write!(f, "cannot prepare plugin: {error}"),
            Self::Pitch(error) => write!(f, "cannot prepare clip pitch: {error}"),
        }
    }
}

impl Error for RenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Time(error) => Some(error),
            Self::Instrument(error) => Some(error),
            Self::Plugin(_) => None,
            Self::Pitch(error) => Some(error),
        }
    }
}
