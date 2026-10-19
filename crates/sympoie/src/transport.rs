use std::error::Error;
use std::fmt;

use crate::Frame;
use crate::clip::MidiNote;
use crate::plugin::PluginHost;
use crate::project::Project;
use crate::render::{RenderError, Renderer};
use crate::time::{Tempo, TimeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopRange {
    start_tick: u64,
    end_tick: u64,
    start_frame: u64,
    end_frame: u64,
}

impl LoopRange {
    pub fn start_tick(self) -> u64 {
        self.start_tick
    }

    pub fn end_tick(self) -> u64 {
        self.end_tick
    }

    pub fn start_frame(self) -> u64 {
        self.start_frame
    }

    pub fn end_frame(self) -> u64 {
        self.end_frame
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCommand {
    Play,
    Pause,
    Stop,
    ReturnToStart,
    SeekFrame(u64),
    SeekTick(u64),
    SetLoopTicks { start: u64, end: u64 },
    ClearLoop,
}

#[derive(Debug)]
pub struct Transport<'a> {
    renderer: Renderer<'a>,
    tempo: Tempo,
    duration_frames: u64,
    state: PlaybackState,
    position_frame: u64,
    loop_range: Option<LoopRange>,
    play_past_end: bool,
    recording: bool,
    track_peaks: Vec<Frame>,
}

impl<'a> Transport<'a> {
    pub fn new(project: &'a Project) -> Result<Self, RenderError> {
        let renderer = Renderer::new(project)?;
        Ok(Self::from_renderer(project, renderer))
    }

    pub fn with_plugin_host(
        project: &'a Project,
        host: &dyn PluginHost,
    ) -> Result<Self, RenderError> {
        let renderer = Renderer::with_plugin_host(project, host)?;
        Ok(Self::from_renderer(project, renderer))
    }

    fn from_renderer(project: &'a Project, renderer: Renderer<'a>) -> Self {
        let duration_frames = renderer.duration_frames();
        Self {
            renderer,
            tempo: project.tempo(),
            duration_frames,
            state: PlaybackState::Stopped,
            position_frame: 0,
            loop_range: None,
            play_past_end: false,
            recording: false,
            track_peaks: vec![[0.0; 2]; project.tracks().len()],
        }
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }

    pub fn position_frame(&self) -> u64 {
        self.position_frame
    }

    pub fn duration_frames(&self) -> u64 {
        self.duration_frames
    }

    pub fn track_peaks(&self) -> &[Frame] {
        &self.track_peaks
    }

    pub fn latency_frames(&self) -> u64 {
        self.renderer.latency_frames()
    }

    pub fn latency_error(&self) -> Option<&str> {
        self.renderer.latency_error()
    }

    pub fn has_live_plugins(&self) -> bool {
        self.renderer.has_live_plugins()
    }

    pub fn audition_notes(&mut self, track: usize, notes: &[MidiNote]) -> bool {
        self.renderer.audition_notes(track, notes)
    }

    pub fn begin_audition_notes(&mut self, track: usize, notes: &[MidiNote]) -> bool {
        self.renderer.begin_audition_notes(track, notes)
    }

    pub fn release_audition_notes(&mut self) {
        self.renderer.release_audition_notes();
    }

    pub fn live_midi(&mut self, track: usize, message: [u8; 3]) -> bool {
        self.renderer.live_midi(track, message)
    }

    pub fn live_midi_from(&mut self, track: usize, source: usize, message: [u8; 3]) -> bool {
        self.renderer.live_midi_from(track, source, message)
    }

    pub fn release_live_midi(&mut self) {
        self.renderer.release_live_midi();
    }

    pub fn set_play_past_end(&mut self, enabled: bool) {
        self.play_past_end = enabled;
    }

    pub fn play_past_end(&self) -> bool {
        self.play_past_end
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    pub fn has_auditions(&self) -> bool {
        self.renderer.has_auditions()
    }

    /// See [`Renderer::set_instrument_renders`].
    pub fn set_instrument_renders(&mut self, renders: &[(u64, std::sync::Arc<[Frame]>)]) {
        self.renderer.set_instrument_renders(renders);
    }

    pub fn process_monitor_block(&mut self, output: &mut [Frame]) {
        self.renderer
            .render_monitor_block(self.position_frame, output);
        self.track_peaks
            .copy_from_slice(self.renderer.track_peaks());
    }

    pub fn update_instruments(&mut self, edited: &Project) -> bool {
        if !self.renderer.update_instruments(edited) {
            return false;
        }
        self.duration_frames = self.renderer.duration_frames();
        true
    }

    pub fn update_parameters(&mut self, edited: &Project) -> bool {
        if !self.renderer.update_parameters(edited) {
            return false;
        }
        self.duration_frames = self.renderer.duration_frames();
        true
    }

    pub fn update_midi_clips(&mut self, edited: &Project) -> bool {
        if !self.renderer.update_midi_clips(edited) {
            return false;
        }
        self.duration_frames = self.renderer.duration_frames();
        true
    }

    pub fn position_tick(&self) -> Result<u64, TimeError> {
        self.tempo.frames_to_ticks(self.position_frame)
    }

    pub fn play(&mut self) {
        self.state = PlaybackState::Playing;
    }

    pub fn pause(&mut self) {
        self.state = PlaybackState::Paused;
    }

    pub fn pause_at_frame(&mut self, position: u64) {
        self.renderer.begin_monitoring();
        self.position_frame = position;
        self.state = PlaybackState::Paused;
    }

    pub fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
        self.recording = false;
        self.renderer.reset();
    }

    pub fn return_to_start(&mut self) {
        self.seek_frame(0);
    }

    pub fn seek_frame(&mut self, position: u64) {
        self.position_frame = position;
        self.renderer.reset();
    }

    pub fn seek_tick(&mut self, position: u64) -> Result<(), TimeError> {
        let frame = self.tempo.ticks_to_frames(position)?;
        self.seek_frame(frame);
        Ok(())
    }

    pub fn loop_range(&self) -> Option<LoopRange> {
        self.loop_range
    }

    pub fn set_loop_ticks(&mut self, start: u64, end: u64) -> Result<(), TransportError> {
        if start >= end {
            return Err(TransportError::InvalidLoopRange);
        }
        let start_frame = self.tempo.ticks_to_frames(start)?;
        let end_frame = self.tempo.ticks_to_frames(end)?;
        if start_frame == end_frame {
            return Err(TransportError::LoopShorterThanFrame);
        }
        self.loop_range = Some(LoopRange {
            start_tick: start,
            end_tick: end,
            start_frame,
            end_frame,
        });
        Ok(())
    }

    pub fn clear_loop(&mut self) {
        self.loop_range = None;
    }

    pub fn apply(&mut self, command: TransportCommand) -> Result<(), TransportError> {
        match command {
            TransportCommand::Play => self.play(),
            TransportCommand::Pause => self.pause(),
            TransportCommand::Stop => self.stop(),
            TransportCommand::ReturnToStart => self.return_to_start(),
            TransportCommand::SeekFrame(position) => self.seek_frame(position),
            TransportCommand::SeekTick(position) => self.seek_tick(position)?,
            TransportCommand::SetLoopTicks { start, end } => self.set_loop_ticks(start, end)?,
            TransportCommand::ClearLoop => self.clear_loop(),
        }
        Ok(())
    }

    pub fn process_block(&mut self, output: &mut [Frame]) {
        self.track_peaks.fill([0.0; 2]);
        if output.is_empty() {
            return;
        }
        if self.state != PlaybackState::Playing {
            output.fill([0.0; 2]);
            return;
        }

        let mut written = 0;
        while written < output.len() {
            let end = if let Some(range) = self.loop_range {
                if self.position_frame >= range.end_frame {
                    self.position_frame = range.start_frame;
                    self.renderer
                        .reset_for_loop(range.start_frame, range.end_frame);
                }
                range.end_frame
            } else {
                let end = if self.play_past_end || self.recording {
                    u64::MAX
                } else {
                    self.duration_frames
                };
                if self.position_frame >= end {
                    self.state = PlaybackState::Stopped;
                    output[written..].fill([0.0; 2]);
                    break;
                }
                end
            };

            let count = (end - self.position_frame).min((output.len() - written) as u64) as usize;
            let block = &mut output[written..written + count];
            if let Some(range) = self.loop_range {
                self.renderer.render_loop_block(
                    self.position_frame,
                    block,
                    range.start_frame,
                    range.end_frame,
                );
            } else {
                self.renderer.render_block(self.position_frame, block);
            }
            for (peaks, rendered) in self.track_peaks.iter_mut().zip(self.renderer.track_peaks()) {
                for (peak, sample) in peaks.iter_mut().zip(rendered) {
                    *peak = peak.max(*sample);
                }
            }
            self.position_frame += count as u64;
            written += count;

            if self.position_frame == end {
                if let Some(range) = self.loop_range {
                    self.position_frame = range.start_frame;
                    self.renderer
                        .reset_for_loop(range.start_frame, range.end_frame);
                } else {
                    self.state = PlaybackState::Stopped;
                    output[written..].fill([0.0; 2]);
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    Time(TimeError),
    InvalidLoopRange,
    LoopShorterThanFrame,
}

impl From<TimeError> for TransportError {
    fn from(value: TimeError) -> Self {
        Self::Time(value)
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Time(error) => error.fmt(f),
            Self::InvalidLoopRange => f.write_str("loop end must be later than its start"),
            Self::LoopShorterThanFrame => {
                f.write_str("loop endpoints must span at least one sample frame")
            }
        }
    }
}

impl Error for TransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Time(error) => Some(error),
            _ => None,
        }
    }
}
