use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use sympoie::Frame;
use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::pitch::PitchAlgorithm;
use sympoie::project::{AudioBuffer, Project};
use sympoie::time::{SnapGrid, Tempo};
use sympoie::track::Track;
use sympoie::transport::TransportCommand;

use super::SympoieApp;
use crate::audio::record_clock::RecordingClock;
use crate::audio::record_input::{CaptureEvent, InputCapture, discover_inputs};
use crate::document::edit::{self, ClipRef};
use crate::state::LoopRegion;

pub(super) struct RecordingSession {
    pub clock: Arc<RecordingClock>,
    pub input: InputCapture,
    tempo: Tempo,
    audio: BTreeMap<usize, Vec<Frame>>,
    pub input_peaks: BTreeMap<usize, Frame>,
    midi: BTreeMap<usize, MidiPerformance>,
    previous_loop: LoopRegion,
    requested_at: Instant,
}

impl RecordingSession {
    pub fn start_tick(&self) -> Option<u64> {
        self.tempo.frames_to_ticks(self.clock.start_frame()?).ok()
    }

    fn accept_audio(&mut self, track: usize, offset: u64, frames: &[Frame]) -> Result<(), String> {
        let Some(audio) = self.audio.get_mut(&track) else {
            return Ok(());
        };
        let start = usize::try_from(offset).map_err(|_| "Recording is too long")?;
        let end = start
            .checked_add(frames.len())
            .ok_or("Recording is too long")?;
        if end > audio.len() {
            audio
                .try_reserve(end - audio.len())
                .map_err(|_| "Not enough memory to continue recording")?;
            audio.resize(end, [0.0; 2]);
        }
        audio[start..end].copy_from_slice(frames);
        for frame in frames {
            for (peak, sample) in self
                .input_peaks
                .entry(track)
                .or_default()
                .iter_mut()
                .zip(frame)
            {
                *peak = peak.max(sample.abs());
            }
        }
        Ok(())
    }
}

impl SympoieApp {
    pub(super) fn refresh_recording_inputs(&mut self) {
        if !self.use_input_devices || self.input_discovery.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("recording-input-discovery".into())
            .spawn(move || {
                let _ = sender.send(discover_inputs());
            }) {
            Ok(_) => self.input_discovery = Some(receiver),
            Err(error) => self.error(format!("Couldn't discover recording inputs: {error}")),
        }
    }

    pub(super) fn poll_recording_inputs(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.input_discovery else {
            return;
        };
        match receiver.try_recv() {
            Ok(catalog) => {
                self.input_catalog = catalog;
                self.input_discovery = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100))
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.input_discovery = None;
                self.input_catalog.error = Some("Recording input discovery failed".into());
            }
        }
    }

    pub(super) fn toggle_track_arm(&mut self, track: usize) {
        if !matches!(
            self.document.project().tracks().get(track),
            Some(Track::Audio(_) | Track::Midi(_))
        ) {
            return;
        }
        self.finish_recording();
        if !self.armed_tracks.remove(&track) {
            self.armed_tracks.insert(track);
        }
    }

    pub(super) fn toggle_recording(&mut self) {
        if self.recording.is_some() {
            self.finish_recording();
            return;
        }
        if self.pending.is_some() {
            self.warn("Close the project confirmation dialog before recording.".into());
            return;
        }
        self.armed_tracks.retain(|&track| {
            matches!(
                self.document.project().tracks().get(track),
                Some(Track::Audio(_) | Track::Midi(_))
            )
        });
        if self.armed_tracks.is_empty() {
            self.warn(
                "Enable recording on an Audio or MIDI track using its red circle first.".into(),
            );
            return;
        }
        if self.bounce.is_some() || self.export.is_some() || self.plugin_load.is_some() {
            self.warn(
                "Wait for the current bounce, export, or plugin load before recording.".into(),
            );
            return;
        }
        self.sync_engine();
        if self.engine.is_preparing_pitch() || !self.document.project().audio_pitch_is_prepared() {
            self.warn("Wait for clip pitch preparation before recording.".into());
            return;
        }
        if let Some(error) = self.engine.status().render_error {
            self.error(format!("Recording unavailable: {error}"));
            return;
        }
        let audio: Vec<_> = self
            .armed_tracks
            .iter()
            .filter_map(|&index| match &self.document.project().tracks()[index] {
                Track::Audio(track) => Some((index, track.recording_input().clone())),
                _ => None,
            })
            .collect();
        let midi: Vec<_> = self
            .armed_tracks
            .iter()
            .filter_map(|&index| match &self.document.project().tracks()[index] {
                Track::Midi(track) => Some((index, track.recording_input().clone())),
                _ => None,
            })
            .collect();
        let clock = Arc::new(RecordingClock::new());
        let input =
            match InputCapture::open(&audio, &midi, self.use_input_devices, Arc::clone(&clock)) {
                Ok(input) => input,
                Err(error) => {
                    self.error(format!("Couldn't start recording: {error}"));
                    return;
                }
            };
        let description = input.description().to_owned();
        let previous_loop = self.loop_region;
        if previous_loop.enabled {
            self.loop_region.enabled = false;
            self.send_loop();
        }
        self.recording = Some(RecordingSession {
            clock: Arc::clone(&clock),
            input,
            tempo: self.document.project().tempo(),
            audio: audio
                .into_iter()
                .map(|(track, _)| (track, Vec::new()))
                .collect(),
            input_peaks: BTreeMap::new(),
            midi: midi
                .into_iter()
                .map(|(track, _)| (track, MidiPerformance::default()))
                .collect(),
            previous_loop,
            requested_at: Instant::now(),
        });
        self.engine
            .start_recording(self.arrangement.playback_start, clock);
        let suffix = if previous_loop.enabled {
            " · Loop temporarily off"
        } else {
            ""
        };
        self.info(format!(
            "Recording: {description} · Q or Stop to finish{suffix}"
        ));
    }

    pub(super) fn poll_recording(&mut self, ctx: &egui::Context) {
        if self.recording.is_none() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        let mut error = self.drain_recording(true);
        if let Some(take) = &self.recording {
            if let Some(reason) = self.status.render_error.as_ref() {
                error = Some(format!("Playback engine: {reason}"));
            } else if !take.clock.is_started()
                && take.requested_at.elapsed() > Duration::from_secs(5)
            {
                error = Some("The playback engine did not start recording.".into());
            }
        }
        if let Some(error) = error {
            self.finish_recording();
            self.error(format!(
                "Recording stopped: {error}. Captured material was kept."
            ));
        }
    }

    fn drain_recording(&mut self, monitor: bool) -> Option<String> {
        let take = self.recording.as_mut()?;
        let mut error = None;
        for event in take.input.drain() {
            match event {
                CaptureEvent::Audio {
                    track,
                    offset,
                    frames,
                } => {
                    if let Err(reason) = take.accept_audio(track, offset, &frames) {
                        error = Some(reason);
                    }
                }
                CaptureEvent::Midi {
                    track,
                    offset,
                    source,
                    message,
                } => {
                    if let Some(midi) = take.midi.get_mut(&track) {
                        midi.event(source, offset, message);
                        if monitor {
                            self.engine.live_midi(track, source, message);
                        }
                    }
                }
                CaptureEvent::Error(reason) => error = Some(reason),
            }
        }
        error
    }

    pub(super) fn finish_recording(&mut self) {
        let Some(take) = self.recording.as_mut() else {
            return;
        };
        let length = take.clock.stop();
        take.input.finish();
        let input_error = self.drain_recording(false);
        let Some(mut take) = self.recording.take() else {
            return;
        };
        self.engine.stop_recording();
        self.engine.command(TransportCommand::Stop);
        self.loop_region = take.previous_loop;
        if self.loop_region.enabled {
            self.send_loop();
        }
        let Some(start_tick) = take.start_tick() else {
            return;
        };
        if length == 0 {
            return;
        }
        for audio in take.audio.values_mut() {
            audio.truncate(usize::try_from(length).unwrap_or(usize::MAX));
            if !audio.is_empty() {
                let end = usize::try_from(length).unwrap_or(audio.len());
                if audio.try_reserve(end.saturating_sub(audio.len())).is_ok() {
                    audio.resize(end, [0.0; 2]);
                }
            }
        }
        let midi: BTreeMap<_, _> = take
            .midi
            .into_iter()
            .map(|(track, performance)| (track, performance.finish(length, take.tempo)))
            .collect();
        let length_ticks = take.tempo.frames_to_ticks(length).unwrap_or(1).max(1);
        let captured = self.edit_structural("Record", move |project| {
            commit_take(project, start_tick, length_ticks, take.audio, midi)
        });
        if let Some(clips) = captured {
            if let Some(&clip) = clips.first() {
                self.selection.select_clip(clip);
            }
            self.info(format!(
                "Recorded {} clip{}",
                clips.len(),
                if clips.len() == 1 { "" } else { "s" }
            ));
        }
        if let Some(error) = input_error {
            self.warn(format!(
                "Recording input: {error}. Captured material was kept."
            ));
        }
    }
}

fn commit_take(
    project: &mut Project,
    start: u64,
    length: u64,
    audio: BTreeMap<usize, Vec<Frame>>,
    midi: BTreeMap<usize, Vec<MidiNote>>,
) -> Result<Vec<ClipRef>, String> {
    let grid = project.snap_grid();
    project.set_snap_grid(SnapGrid::Off);
    let result = (|| {
        let mut clips = Vec::new();
        for (track, samples) in audio {
            if samples.is_empty() {
                continue;
            }
            let id = edit::audio_id(project, track)?;
            let index = project
                .audio_track(id)
                .map_err(|e| e.to_string())?
                .clips()
                .len();
            let frame_count = samples.len() as u64;
            let asset = project.add_asset(AudioBuffer::new(samples).map_err(|e| e.to_string())?);
            project
                .add_audio_clip(
                    id,
                    AudioClip {
                        asset,
                        source_start: 0,
                        start_tick: start,
                        frame_count,
                        gain: 1.0,
                        pitch_semitones: 0.0,
                        pitch_algorithm: PitchAlgorithm::Signalsmith,
                        preserve_formants: true,
                    },
                )
                .map_err(|e| e.to_string())?;
            clips.push(ClipRef { track, index });
        }
        for (track, notes) in midi {
            let id = edit::midi_id(project, track)?;
            let index = project
                .midi_track(id)
                .map_err(|e| e.to_string())?
                .clips()
                .len();
            let length_ticks = notes
                .iter()
                .map(|n| n.start_tick + n.duration_ticks)
                .max()
                .unwrap_or(0)
                .max(length);
            project
                .add_midi_clip(
                    id,
                    MidiClip {
                        start_tick: start,
                        length_ticks,
                        offset_ticks: 0,
                        notes,
                    },
                )
                .map_err(|e| e.to_string())?;
            clips.push(ClipRef { track, index });
        }
        clips.sort_by_key(|clip| clip.track);
        Ok(clips)
    })();
    project.set_snap_grid(grid);
    result
}

#[derive(Default)]
struct MidiPerformance {
    held: BTreeMap<(usize, u8, u8), HeldNote>,
    sustain: BTreeSet<(usize, u8)>,
    notes: Vec<(u64, u64, u8, u8, u8)>,
}

struct HeldNote {
    start: u64,
    velocity: u8,
    released: bool,
}

impl MidiPerformance {
    fn end(&mut self, key: (usize, u8, u8), at: u64) {
        if let Some(note) = self.held.remove(&key) {
            self.notes.push((
                note.start,
                at.max(note.start + 1),
                key.2,
                note.velocity,
                key.1,
            ));
        }
    }

    fn event(&mut self, source: usize, offset: u64, message: [u8; 3]) {
        let [status, key, value] = message;
        if key > 127 || value > 127 {
            return;
        }
        let channel = status & 0xf;
        let identity = (source, channel, key);
        match status & 0xf0 {
            0x90 if value > 0 => {
                self.end(identity, offset);
                self.held.insert(
                    identity,
                    HeldNote {
                        start: offset,
                        velocity: value,
                        released: false,
                    },
                );
            }
            0x80 | 0x90 => {
                if self.sustain.contains(&(source, channel)) {
                    if let Some(note) = self.held.get_mut(&identity) {
                        note.released = true;
                    }
                } else {
                    self.end(identity, offset);
                }
            }
            0xb0 if key == 64 || key == 120 || key == 121 || key == 123 => {
                if key == 64 && value >= 64 {
                    self.sustain.insert((source, channel));
                    return;
                }
                self.sustain.remove(&(source, channel));
                let keys: Vec<_> = self
                    .held
                    .iter()
                    .filter(|(id, note)| {
                        id.0 == source
                            && id.1 == channel
                            && (note.released || key == 120 || key == 123)
                    })
                    .map(|(&id, _)| id)
                    .collect();
                for id in keys {
                    self.end(id, offset);
                }
            }
            _ => {}
        }
    }

    fn finish(mut self, end: u64, tempo: Tempo) -> Vec<MidiNote> {
        let keys: Vec<_> = self.held.keys().copied().collect();
        for key in keys {
            self.end(key, end);
        }
        let mut notes: Vec<_> = self
            .notes
            .into_iter()
            .filter(|(start, ..)| *start < end)
            .map(|(start, stop, key, velocity, channel)| {
                let start_tick = tempo.frames_to_ticks(start).unwrap_or(0);
                let stop_tick = tempo.frames_to_ticks(stop.min(end)).unwrap_or(start_tick);
                MidiNote {
                    start_tick,
                    duration_ticks: stop_tick.saturating_sub(start_tick).max(1),
                    key,
                    velocity,
                    channel,
                }
            })
            .collect();
        notes.sort_by_key(|n| (n.start_tick, n.channel, n.key));
        notes
    }
}

#[cfg(test)]
#[path = "recording_tests.rs"]
mod tests;
