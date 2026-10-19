use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sympoie::Frame;
use sympoie::clip::MidiNote;
use sympoie::project::Project;
use sympoie::transport::{PlaybackState, Transport, TransportCommand};

use super::pitch_prepare::PitchWorker;
use super::record_clock::RecordingClock;
use super::ring::{CHUNK_FRAMES, Ring};
use crate::plugins::{PluginLease, PluginRuntime};

#[cfg(test)]
#[path = "pitch_engine_tests.rs"]
mod pitch_engine_tests;

#[cfg(test)]
#[path = "pitch_wait_tests.rs"]
mod pitch_wait_tests;

const TARGET_BUFFERED_FRAMES: u64 = 3072;
const RING_CHUNKS: usize = 64;

pub struct Shared {
    pub(super) ring: Ring,
    audible_frame: AtomicU64,
    transport_frame: AtomicU64,
    state: AtomicU8,
    peaks: [AtomicU32; 2],
    render_error: Mutex<Option<String>>,
    pitch_error: Mutex<Option<String>>,
    preparing_pitch: AtomicBool,
    shutdown: AtomicBool,
    /// Finished offline instrument renders to play instead of their instruments' notes,
    /// with a generation the render thread watches for changes.
    instrument_renders: Mutex<Arc<InstrumentRenders>>,
    renders_generation: AtomicU64,
}

pub type InstrumentRenders = Vec<(u64, Arc<[Frame]>)>;

impl Shared {
    fn new() -> Self {
        Self {
            ring: Ring::new(RING_CHUNKS),
            audible_frame: AtomicU64::new(0),
            transport_frame: AtomicU64::new(0),
            state: AtomicU8::new(encode_state(PlaybackState::Stopped)),
            peaks: [AtomicU32::new(0), AtomicU32::new(0)],
            render_error: Mutex::new(None),
            pitch_error: Mutex::new(None),
            preparing_pitch: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            instrument_renders: Mutex::new(Arc::new(Vec::new())),
            renders_generation: AtomicU64::new(0),
        }
    }

    pub fn consume(&self, output: &mut [Frame]) -> usize {
        self.consume_marking_cuts(output).0
    }

    pub fn consume_marking_cuts(&self, output: &mut [Frame]) -> (usize, bool) {
        let (count, position, skipped) = self.ring.pop_marking_skips(output);
        output[count..].fill([0.0; 2]);
        if let Some(position) = position {
            self.audible_frame.store(position, Ordering::Relaxed);
        }
        let mut peak = [0.0_f32; 2];
        for frame in &output[..count] {
            for channel in 0..2 {
                let level = frame[channel].abs();
                if level > peak[channel] {
                    peak[channel] = level;
                }
            }
        }
        for (slot, level) in self.peaks.iter().zip(peak) {
            if level > 0.0 && level.is_finite() {
                slot.fetch_max(level.to_bits(), Ordering::Relaxed);
            }
        }
        (count, skipped)
    }

    fn publish(&self, transport: &Transport<'_>, ending: bool) {
        let state = if ending {
            PlaybackState::Playing
        } else {
            transport.state()
        };
        self.state.store(encode_state(state), Ordering::Relaxed);
        self.transport_frame
            .store(transport.position_frame(), Ordering::Relaxed);
    }

    fn set_render_error(&self, error: Option<String>) {
        *self
            .render_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = error;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineStatus {
    pub state: PlaybackState,
    pub position_frame: u64,
    pub render_error: Option<String>,
}

struct TrackPeakBank(Vec<[AtomicU32; 2]>);

impl TrackPeakBank {
    fn new(project: &Project) -> Self {
        Self(
            project
                .tracks()
                .iter()
                .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
                .collect(),
        )
    }

    fn publish(&self, levels: &[[f32; 2]]) {
        debug_assert_eq!(self.0.len(), levels.len());
        for (track, levels) in self.0.iter().zip(levels) {
            for (slot, &level) in track.iter().zip(levels) {
                if level > 0.0 && level.is_finite() {
                    slot.fetch_max(level.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }

    fn take(&self) -> Vec<[f32; 2]> {
        self.0
            .iter()
            .map(|track| {
                track
                    .each_ref()
                    .map(|peak| f32::from_bits(peak.swap(0, Ordering::Relaxed)))
            })
            .collect()
    }
}

struct ProjectSnapshot {
    project: Box<Project>,
    plugins: PluginLease,
    track_peaks: Arc<TrackPeakBank>,
    pitch_pins: Vec<Arc<[Frame]>>,
}

impl ProjectSnapshot {
    fn new(project: Project, plugins: &PluginRuntime) -> Self {
        let lease = plugins.pin_project(&project);
        let track_peaks = Arc::new(TrackPeakBank::new(&project));
        Self {
            project: Box::new(project),
            plugins: lease,
            track_peaks,
            pitch_pins: Vec::new(),
        }
    }
}

enum Message {
    PitchReady {
        generation: u64,
        pins: Vec<Arc<[Frame]>>,
    },
    Update {
        snapshot: ProjectSnapshot,
        force_rebuild: bool,
    },
    Replace(ProjectSnapshot),
    Transport(TransportCommand),
    PlayFromTick {
        tick: u64,
        toggle: bool,
    },
    Audition {
        track: usize,
        notes: Vec<MidiNote>,
    },
    BeginAudition {
        track: usize,
        notes: Vec<MidiNote>,
    },
    ReleaseAudition,
    StartRecording {
        tick: u64,
        clock: Arc<RecordingClock>,
    },
    StopRecording,
    LiveMidi {
        track: usize,
        source: usize,
        message: [u8; 3],
    },
    PitchFailed {
        generation: u64,
        error: String,
    },
    #[cfg(test)]
    Synchronize(Sender<()>),
    Shutdown,
}

pub struct AudioEngine {
    sender: Sender<Message>,
    shared: Arc<Shared>,
    plugins: Arc<PluginRuntime>,
    track_peaks: Mutex<Arc<TrackPeakBank>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioEngine {
    #[cfg(test)]
    pub fn start(project: Project) -> Self {
        Self::with_plugins(project, PluginRuntime::new())
    }

    pub fn with_plugins(project: Project, plugins: Arc<PluginRuntime>) -> Self {
        let shared = Arc::new(Shared::new());
        let (sender, receiver) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let snapshot = ProjectSnapshot::new(project, &plugins);
        let track_peaks = Mutex::new(Arc::clone(&snapshot.track_peaks));
        let preparation_sender = sender.clone();
        let thread = thread::Builder::new()
            .name("sympoie-render".into())
            .spawn(move || render_thread(snapshot, &receiver, &thread_shared, preparation_sender))
            .expect("the operating system refused to start the render thread");
        Self {
            sender,
            shared,
            plugins,
            track_peaks,
            thread: Some(thread),
        }
    }

    pub fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    pub fn is_preparing_pitch(&self) -> bool {
        self.shared.preparing_pitch.load(Ordering::Relaxed)
    }

    /// Plays these finished offline renders, keyed by plug-in instrument id, in place of
    /// those instruments' notes; every other instrument plays live.
    pub fn set_instrument_renders(&self, renders: InstrumentRenders) {
        *self
            .shared
            .instrument_renders
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(renders);
        self.shared
            .renders_generation
            .fetch_add(1, Ordering::AcqRel);
    }

    pub fn update_project(&self, project: Project) {
        self.submit_update(project, false);
    }

    pub fn update_project_structural(&self, project: Project) {
        self.submit_update(project, true);
    }

    fn submit_update(&self, project: Project, force_rebuild: bool) {
        let snapshot = ProjectSnapshot::new(project, &self.plugins);
        *self
            .track_peaks
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::clone(&snapshot.track_peaks);
        let _ = self.sender.send(Message::Update {
            snapshot,
            force_rebuild,
        });
    }

    pub fn replace_project(&self, project: Project) {
        let snapshot = ProjectSnapshot::new(project, &self.plugins);
        *self
            .track_peaks
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::clone(&snapshot.track_peaks);
        let _ = self.sender.send(Message::Replace(snapshot));
    }

    pub fn command(&self, command: TransportCommand) {
        let _ = self.sender.send(Message::Transport(command));
    }

    pub fn play_from_tick(&self, tick: u64) {
        let _ = self.sender.send(Message::PlayFromTick {
            tick,
            toggle: false,
        });
    }

    pub fn toggle_play_from_tick(&self, tick: u64) {
        let _ = self
            .sender
            .send(Message::PlayFromTick { tick, toggle: true });
    }

    pub fn audition_notes(&self, track: usize, notes: Vec<MidiNote>) {
        let _ = self.sender.send(Message::Audition { track, notes });
    }

    pub fn begin_audition_notes(&self, track: usize, notes: Vec<MidiNote>) {
        let _ = self.sender.send(Message::BeginAudition { track, notes });
    }

    pub fn release_audition_notes(&self) {
        let _ = self.sender.send(Message::ReleaseAudition);
    }

    pub fn start_recording(&self, start_tick: u64, clock: Arc<RecordingClock>) {
        if let Err(error) = self.sender.send(Message::StartRecording {
            tick: start_tick,
            clock,
        }) && let Message::StartRecording { clock, .. } = error.0
        {
            clock.stop();
        }
    }

    pub fn stop_recording(&self) {
        let _ = self.sender.send(Message::StopRecording);
    }

    pub fn live_midi(&self, track: usize, source: usize, message: [u8; 3]) {
        let _ = self.sender.send(Message::LiveMidi {
            track,
            source,
            message,
        });
    }

    pub fn status(&self) -> EngineStatus {
        let state = decode_state(self.shared.state.load(Ordering::Relaxed));
        let position_frame = if state == PlaybackState::Playing {
            self.shared.audible_frame.load(Ordering::Relaxed)
        } else {
            self.shared.transport_frame.load(Ordering::Relaxed)
        };
        let render_error = self
            .shared
            .render_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .or_else(|| {
                self.shared
                    .pitch_error
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            });
        EngineStatus {
            state,
            position_frame,
            render_error,
        }
    }

    pub fn take_peaks(&self) -> [f32; 2] {
        self.shared
            .peaks
            .each_ref()
            .map(|peak| f32::from_bits(peak.swap(0, Ordering::Relaxed)))
    }

    pub fn take_track_peaks(&self) -> Vec<[f32; 2]> {
        self.track_peaks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    pub fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.shared.shutdown.store(true, Ordering::Release);
            let _ = self.sender.send(Message::Shutdown);
            let _ = thread.join();
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn encode_state(state: PlaybackState) -> u8 {
    match state {
        PlaybackState::Stopped => 0,
        PlaybackState::Playing => 1,
        PlaybackState::Paused => 2,
    }
}

fn decode_state(value: u8) -> PlaybackState {
    match value {
        1 => PlaybackState::Playing,
        2 => PlaybackState::Paused,
        _ => PlaybackState::Stopped,
    }
}

#[derive(Debug)]
struct Cursor {
    state: PlaybackState,
    frame: u64,
    loop_ticks: Option<(u64, u64)>,
    ending: Option<u64>,
    recording: Option<Arc<RecordingClock>>,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            frame: 0,
            loop_ticks: None,
            ending: None,
            recording: None,
        }
    }
}

impl Cursor {
    fn stop_recording(&mut self) {
        if let Some(clock) = self.recording.take() {
            clock.stop();
        }
    }
}

impl Drop for Cursor {
    fn drop(&mut self) {
        self.stop_recording();
    }
}

enum Next {
    Update(ProjectSnapshot),
    Replace(ProjectSnapshot),
    WaitingForPitch { reset: bool },
    Shutdown,
}

struct PendingPitch {
    snapshot: ProjectSnapshot,
    force_rebuild: bool,
    replace: bool,
    generation: u64,
}

struct PitchPreparation {
    worker: PitchWorker,
    pending: Option<PendingPitch>,
    generation: u64,
}

impl PitchPreparation {
    fn new(sender: Sender<Message>) -> Self {
        Self {
            worker: PitchWorker::new(move |generation, result| {
                let message = match result {
                    Ok(pins) => Message::PitchReady { generation, pins },
                    Err(error) => Message::PitchFailed { generation, error },
                };
                let _ = sender.send(message);
            }),
            pending: None,
            generation: 0,
        }
    }

    fn submit(
        &mut self,
        mut snapshot: ProjectSnapshot,
        force_rebuild: bool,
        replace: bool,
        shared: &Shared,
    ) -> Option<PendingPitch> {
        *shared
            .pitch_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        let previous = self.pending.take();
        let force_rebuild = force_rebuild
            || (!replace
                && previous
                    .as_ref()
                    .is_some_and(|pending| pending.force_rebuild));
        let replace = replace || previous.as_ref().is_some_and(|pending| pending.replace);
        self.generation = self.generation.wrapping_add(1);
        self.worker.cancel();
        if let Some(pins) = snapshot.project.cached_audio_pitch() {
            snapshot.pitch_pins = pins;
            shared.preparing_pitch.store(false, Ordering::Relaxed);
            return Some(PendingPitch {
                snapshot,
                force_rebuild,
                replace,
                generation: self.generation,
            });
        }
        self.worker
            .request(self.generation, (*snapshot.project).clone());
        self.pending = Some(PendingPitch {
            snapshot,
            force_rebuild,
            replace,
            generation: self.generation,
        });
        shared.preparing_pitch.store(true, Ordering::Relaxed);
        None
    }

    fn fail(&mut self, generation: u64, error: String, shared: &Shared) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            *shared
                .pitch_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(error);
            shared.preparing_pitch.store(false, Ordering::Relaxed);
        }
    }

    fn finish(
        &mut self,
        generation: u64,
        pins: Vec<Arc<[Frame]>>,
        shared: &Shared,
    ) -> Option<PendingPitch> {
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| pending.generation != generation)
        {
            return None;
        }
        let mut pending = self.pending.take().expect("checked pending generation");
        pending.snapshot.pitch_pins = pins;
        shared.preparing_pitch.store(false, Ordering::Relaxed);
        Some(pending)
    }
}

impl PendingPitch {
    fn into_message(self) -> Message {
        if self.replace {
            Message::Replace(self.snapshot)
        } else {
            Message::Update {
                snapshot: self.snapshot,
                force_rebuild: self.force_rebuild,
            }
        }
    }
}

const TRANSITION_FRAMES: usize = sympoie::SAMPLE_RATE as usize / 50;

struct Transition {
    tail: Vec<Frame>,
    offset: usize,
}

impl Transition {
    fn mix(&mut self, output: &mut [Frame]) {
        for frame in output {
            let Some(old) = self.tail.get(self.offset) else {
                break;
            };
            let wet = self.offset as f32 / (self.tail.len() - 1).max(1) as f32;
            for channel in 0..2 {
                frame[channel] = (f64::from(old[channel])
                    + (f64::from(frame[channel]) - f64::from(old[channel])) * f64::from(wet))
                    as f32;
            }
            self.offset += 1;
        }
    }
}

fn render_thread(
    snapshot: ProjectSnapshot,
    receiver: &Receiver<Message>,
    shared: &Shared,
    sender: Sender<Message>,
) {
    let mut cursor = Cursor::default();
    let mut transition = None;
    let mut pitch = PitchPreparation::new(sender);
    let mut snapshot = pitch
        .submit(snapshot, true, true, shared)
        .map(|ready| ready.snapshot);
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        let next = if let Some(snapshot) = &snapshot {
            match Transport::with_plugin_host(&snapshot.project, &snapshot.plugins) {
                Ok(transport) => {
                    shared.set_render_error(None);
                    run(
                        transport,
                        &mut cursor,
                        receiver,
                        shared,
                        &mut transition,
                        Arc::clone(&snapshot.track_peaks),
                        &mut pitch,
                    )
                }
                Err(error) => {
                    shared.set_render_error(Some(error.to_string()));
                    idle(&snapshot.project, &mut cursor, receiver, shared, &mut pitch)
                }
            }
        } else {
            wait_for_pitch(&mut cursor, receiver, shared, &mut pitch)
        };
        match next {
            Next::Update(updated) => {
                if let Some(snapshot) = &snapshot
                    && updated.project.bpm() != snapshot.project.bpm()
                    && let Ok(tick) = snapshot.project.tempo().frames_to_ticks(cursor.frame)
                    && let Ok(frame) = updated.project.tempo().ticks_to_frames(tick)
                {
                    cursor.frame = frame;
                }
                snapshot = Some(updated);
            }
            Next::Replace(replacement) => {
                shared.ring.flush();
                cursor = Cursor::default();
                transition = None;
                shared.audible_frame.store(0, Ordering::Relaxed);
                snapshot = Some(replacement);
            }
            Next::WaitingForPitch { reset } => {
                shared.ring.flush();
                if reset {
                    cursor = Cursor::default();
                }
                cursor.ending = None;
                transition = None;
                shared.audible_frame.store(cursor.frame, Ordering::Relaxed);
                shared.set_render_error(None);
                snapshot = None;
            }
            Next::Shutdown => return,
        }
    }
}

fn run(
    mut transport: Transport<'_>,
    cursor: &mut Cursor,
    receiver: &Receiver<Message>,
    shared: &Shared,
    transition: &mut Option<Transition>,
    mut track_peaks: Arc<TrackPeakBank>,
    pitch: &mut PitchPreparation,
) -> Next {
    transport.set_play_past_end(true);
    transport.seek_frame(cursor.frame);
    transport.set_recording(cursor.recording.is_some());
    if let Some((start, end)) = cursor.loop_ticks
        && transport.set_loop_ticks(start, end).is_err()
    {
        cursor.loop_ticks = None;
    }
    match cursor.state {
        PlaybackState::Playing => transport.play(),
        PlaybackState::Paused => transport.pause(),
        PlaybackState::Stopped => transport.stop(),
    }
    let mut ending = cursor.ending;
    shared.publish(&transport, ending.is_some());

    let save = |transport: &Transport<'_>, ending: Option<u64>, cursor: &mut Cursor| {
        cursor.state = transport.state();
        cursor.frame = transport.position_frame();
        cursor.loop_ticks = transport
            .loop_range()
            .map(|range| (range.start_tick(), range.end_tick()));
        cursor.ending = ending;
    };

    let mut block = [[0.0; 2]; CHUNK_FRAMES];
    let mut pending: Option<Message> = None;
    let mut applied_renders = None;
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return Next::Shutdown;
        }
        loop {
            if shared.shutdown.load(Ordering::Acquire) {
                return Next::Shutdown;
            }
            let message = match pending.take() {
                Some(message) => message,
                None => match receiver.try_recv() {
                    Ok(message) => message,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Next::Shutdown,
                },
            };
            let message = match message {
                Message::Update {
                    snapshot,
                    force_rebuild,
                } => {
                    let Some(ready) = pitch.submit(snapshot, force_rebuild, false, shared) else {
                        continue;
                    };
                    ready.into_message()
                }
                Message::Replace(snapshot) => {
                    let Some(ready) = pitch.submit(snapshot, true, true, shared) else {
                        return Next::WaitingForPitch { reset: true };
                    };
                    ready.into_message()
                }
                Message::PitchFailed { generation, error } => {
                    pitch.fail(generation, error, shared);
                    continue;
                }
                Message::PitchReady { generation, pins } => {
                    let Some(ready) = pitch.finish(generation, pins, shared) else {
                        continue;
                    };
                    ready.into_message()
                }
                message => message,
            };
            match message {
                Message::PitchReady { .. } | Message::PitchFailed { .. } => {
                    unreachable!("pitch results were resolved above")
                }
                Message::Audition { track, notes } => {
                    transport.audition_notes(track, &notes);
                }
                Message::BeginAudition { track, notes } => {
                    transport.begin_audition_notes(track, &notes);
                }
                Message::ReleaseAudition => transport.release_audition_notes(),
                Message::LiveMidi {
                    track,
                    source,
                    message,
                } => {
                    transport.live_midi_from(track, source, message);
                }
                Message::StartRecording { tick, clock } => {
                    if cursor.recording.is_some() {
                        clock.stop();
                        continue;
                    }
                    let audibly_playing =
                        transport.state() == PlaybackState::Playing || ending.is_some();
                    let frame = if audibly_playing {
                        shared.audible_frame.load(Ordering::Relaxed)
                    } else {
                        if let Err(error) = transport.seek_tick(tick) {
                            shared.set_render_error(Some(error.to_string()));
                            clock.stop();
                            continue;
                        }
                        transport.position_frame()
                    };
                    if !audibly_playing || transport.state() != PlaybackState::Playing {
                        shared.ring.flush();
                        if audibly_playing {
                            transport.seek_frame(frame);
                        }
                        shared.audible_frame.store(frame, Ordering::Relaxed);
                    }
                    clock.start(frame);
                    if clock.offset_at(std::time::Instant::now()).is_none() {
                        continue;
                    }
                    transport.set_recording(true);
                    transport.play();
                    cursor.recording = Some(clock);
                    ending = None;
                    *transition = None;
                    shared.publish(&transport, false);
                }
                Message::StopRecording => {
                    cursor.stop_recording();
                    transport.set_recording(false);
                    transport.release_live_midi();
                }
                Message::Transport(command) => {
                    if !matches!(
                        command,
                        TransportCommand::Play
                            | TransportCommand::SetLoopTicks { .. }
                            | TransportCommand::ClearLoop
                    ) {
                        cursor.stop_recording();
                        transport.set_recording(false);
                    }
                    let audibly_playing =
                        transport.state() == PlaybackState::Playing || ending.is_some();
                    ending = None;
                    *transition = None;
                    apply(&mut transport, command, shared, audibly_playing);
                    shared.publish(&transport, ending.is_some());
                }
                Message::PlayFromTick { tick, toggle } => {
                    let audibly_playing =
                        transport.state() == PlaybackState::Playing || ending.is_some();
                    if audibly_playing && !toggle {
                        continue;
                    }
                    if audibly_playing {
                        cursor.stop_recording();
                        transport.set_recording(false);
                    }
                    if !audibly_playing && transport.seek_tick(tick).is_err() {
                        continue;
                    }
                    ending = None;
                    *transition = None;
                    let command = if audibly_playing {
                        TransportCommand::Pause
                    } else {
                        TransportCommand::Play
                    };
                    apply(&mut transport, command, shared, audibly_playing);
                    shared.publish(&transport, false);
                }
                Message::Update {
                    snapshot: updated,
                    force_rebuild,
                } => {
                    if !force_rebuild
                        && (transport.update_parameters(&updated.project)
                            || transport.update_midi_clips(&updated.project))
                    {
                        track_peaks = updated.track_peaks;
                        continue;
                    }
                    save(&transport, ending, cursor);
                    if transport.state() == PlaybackState::Playing
                        || transport.has_live_plugins()
                        || transport.has_auditions()
                    {
                        let mut tail = vec![[0.0; 2]; TRANSITION_FRAMES];
                        if transport.state() == PlaybackState::Playing {
                            transport.process_block(&mut tail);
                        } else {
                            transport.process_monitor_block(&mut tail);
                        }
                        if let Some(previous) = transition {
                            previous.mix(&mut tail);
                        }
                        *transition = Some(Transition { tail, offset: 0 });
                    }
                    return Next::Update(updated);
                }
                Message::Replace(replacement) => return Next::Replace(replacement),
                #[cfg(test)]
                Message::Synchronize(acknowledged) => {
                    let _ = acknowledged.send(());
                }
                Message::Shutdown => return Next::Shutdown,
            }
        }

        let renders = shared.renders_generation.load(Ordering::Acquire);
        if applied_renders != Some(renders) {
            let list = Arc::clone(
                &shared
                    .instrument_renders
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            );
            transport.set_instrument_renders(&list);
            applied_renders = Some(renders);
        }
        let monitoring = transport.has_live_plugins() || transport.has_auditions();
        if transport.state() == PlaybackState::Playing || transition.is_some() || monitoring {
            while shared.ring.buffered() < TARGET_BUFFERED_FRAMES
                && shared.ring.free() >= CHUNK_FRAMES as u64
                && !shared.shutdown.load(Ordering::Acquire)
            {
                let start = transport.position_frame();
                let playing = transport.state() == PlaybackState::Playing;
                let fading = transition.is_some();
                if playing {
                    transport.process_block(&mut block);
                } else if monitoring {
                    transport.process_monitor_block(&mut block);
                } else {
                    block.fill([0.0; 2]);
                }
                if playing || monitoring {
                    track_peaks.publish(transport.track_peaks());
                }
                shared.set_render_error(transport.latency_error().map(str::to_owned));
                if let Some(fade) = transition {
                    fade.mix(&mut block);
                    if fade.offset == fade.tail.len() {
                        *transition = None;
                    }
                }
                if playing {
                    if let Some(range) = transport.loop_range() {
                        shared.ring.push_loop_chunk(
                            &block,
                            start,
                            range.start_frame(),
                            range.end_frame(),
                        );
                    } else {
                        shared.ring.push_chunk(&block, start);
                    }
                } else {
                    shared.ring.push_stationary_chunk(&block, start);
                }
                if transport.state() != PlaybackState::Playing
                    && (playing || (fading && ending.is_some()))
                {
                    ending = Some(shared.ring.write_cursor());
                    if transition.is_none() && !monitoring {
                        break;
                    }
                }
            }
        }
        if ending.is_some_and(|boundary| shared.ring.has_consumed_through(boundary)) {
            ending = None;
        }
        shared.publish(&transport, ending.is_some());

        let busy = transport.state() == PlaybackState::Playing
            || ending.is_some()
            || transition.is_some()
            || monitoring;
        let wait = if busy {
            Duration::from_millis(2)
        } else {
            Duration::from_millis(250)
        };
        match receiver.recv_timeout(wait) {
            Ok(message) => pending = Some(message),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Next::Shutdown,
        }
    }
}

fn apply(
    transport: &mut Transport<'_>,
    command: TransportCommand,
    shared: &Shared,
    audibly_playing: bool,
) {
    let playing = transport.state() == PlaybackState::Playing;
    let resync = |transport: &mut Transport<'_>| {
        if audibly_playing {
            let audible = shared.audible_frame.load(Ordering::Relaxed);
            shared.ring.flush();
            transport.seek_frame(audible);
        }
    };
    match command {
        TransportCommand::Play => {
            if !playing {
                shared.ring.flush();
                shared
                    .audible_frame
                    .store(transport.position_frame(), Ordering::Relaxed);
                transport.play();
            }
        }
        TransportCommand::Pause => {
            if audibly_playing {
                transport.pause_at_frame(shared.audible_frame.load(Ordering::Relaxed));
            } else {
                transport.pause();
            }
            shared.ring.flush();
        }
        TransportCommand::Stop => {
            resync(transport);
            shared.ring.flush();
            transport.stop();
        }
        TransportCommand::SetLoopTicks { .. } | TransportCommand::ClearLoop => {
            resync(transport);
            let _ = transport.apply(command);
        }
        TransportCommand::ReturnToStart
        | TransportCommand::SeekFrame(_)
        | TransportCommand::SeekTick(_) => {
            shared.ring.flush();
            let _ = transport.apply(command);
            shared
                .audible_frame
                .store(transport.position_frame(), Ordering::Relaxed);
        }
    }
}

fn idle(
    project: &Project,
    cursor: &mut Cursor,
    receiver: &Receiver<Message>,
    shared: &Shared,
    pitch: &mut PitchPreparation,
) -> Next {
    cursor.stop_recording();
    cursor.state = PlaybackState::Stopped;
    cursor.ending = None;
    shared.ring.flush();
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return Next::Shutdown;
        }
        shared
            .state
            .store(encode_state(PlaybackState::Stopped), Ordering::Relaxed);
        shared
            .transport_frame
            .store(cursor.frame, Ordering::Relaxed);
        match receiver.recv() {
            Ok(Message::Transport(command)) => match command {
                TransportCommand::ReturnToStart => cursor.frame = 0,
                TransportCommand::SeekFrame(frame) => cursor.frame = frame,
                TransportCommand::SeekTick(tick) => {
                    if let Ok(frame) = project.tempo().ticks_to_frames(tick) {
                        cursor.frame = frame;
                    }
                }
                TransportCommand::SetLoopTicks { start, end } => {
                    cursor.loop_ticks = Some((start, end));
                }
                TransportCommand::ClearLoop => cursor.loop_ticks = None,
                TransportCommand::Play | TransportCommand::Pause | TransportCommand::Stop => {}
            },
            Ok(Message::Update {
                snapshot,
                force_rebuild,
            }) => {
                return match pitch.submit(snapshot, force_rebuild, false, shared) {
                    Some(ready) => Next::Update(ready.snapshot),
                    None => Next::WaitingForPitch { reset: false },
                };
            }
            Ok(
                Message::Audition { .. } | Message::BeginAudition { .. } | Message::ReleaseAudition,
            ) => {}
            Ok(Message::LiveMidi { .. }) | Ok(Message::StopRecording) => {}
            Ok(Message::StartRecording { clock, .. }) => {
                clock.stop();
            }
            Ok(Message::PlayFromTick { .. }) => {}
            Ok(Message::Replace(replacement)) => {
                return match pitch.submit(replacement, true, true, shared) {
                    Some(ready) => Next::Replace(ready.snapshot),
                    None => Next::WaitingForPitch { reset: true },
                };
            }
            Ok(Message::PitchFailed { generation, error }) => {
                pitch.fail(generation, error, shared);
            }
            Ok(Message::PitchReady { generation, pins }) => {
                if let Some(ready) = pitch.finish(generation, pins, shared) {
                    return Next::Update(ready.snapshot);
                }
            }
            #[cfg(test)]
            Ok(Message::Synchronize(acknowledged)) => {
                let _ = acknowledged.send(());
            }
            Ok(Message::Shutdown) | Err(_) => return Next::Shutdown,
        }
    }
}

fn wait_for_pitch(
    cursor: &mut Cursor,
    receiver: &Receiver<Message>,
    shared: &Shared,
    pitch: &mut PitchPreparation,
) -> Next {
    cursor.stop_recording();
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return Next::Shutdown;
        }
        shared
            .state
            .store(encode_state(cursor.state), Ordering::Relaxed);
        shared
            .transport_frame
            .store(cursor.frame, Ordering::Relaxed);
        shared.audible_frame.store(cursor.frame, Ordering::Relaxed);
        match receiver.recv() {
            Ok(Message::PitchFailed { generation, error }) => {
                pitch.fail(generation, error, shared);
            }
            Ok(Message::PitchReady { generation, pins }) => {
                if let Some(ready) = pitch.finish(generation, pins, shared) {
                    return Next::Update(ready.snapshot);
                }
            }
            Ok(Message::Update {
                snapshot,
                force_rebuild,
            }) => {
                if let Some(pending) = &pitch.pending
                    && pending.snapshot.project.bpm() != snapshot.project.bpm()
                {
                    if let Ok(tick) = pending
                        .snapshot
                        .project
                        .tempo()
                        .frames_to_ticks(cursor.frame)
                        && let Ok(frame) = snapshot.project.tempo().ticks_to_frames(tick)
                    {
                        cursor.frame = frame;
                    }
                    if let Some((start, end)) = cursor.loop_ticks
                        && !waiting_loop_is_valid(&snapshot.project, start, end)
                    {
                        cursor.loop_ticks = None;
                    }
                }
                if let Some(ready) = pitch.submit(snapshot, force_rebuild, false, shared) {
                    return Next::Update(ready.snapshot);
                }
            }
            Ok(Message::Replace(snapshot)) => {
                *cursor = Cursor::default();
                if let Some(ready) = pitch.submit(snapshot, true, true, shared) {
                    return Next::Replace(ready.snapshot);
                }
            }
            Ok(Message::Transport(command)) => match command {
                TransportCommand::Play => {
                    play_waiting_cursor(cursor);
                }
                TransportCommand::Pause => cursor.state = PlaybackState::Paused,
                TransportCommand::Stop => cursor.state = PlaybackState::Stopped,
                TransportCommand::ReturnToStart => cursor.frame = 0,
                TransportCommand::SeekFrame(frame) => cursor.frame = frame,
                TransportCommand::SeekTick(tick) => {
                    if let Some(pending) = &pitch.pending
                        && let Ok(frame) = pending.snapshot.project.tempo().ticks_to_frames(tick)
                    {
                        cursor.frame = frame;
                    }
                }
                TransportCommand::SetLoopTicks { start, end } => {
                    if let Some(pending) = &pitch.pending
                        && waiting_loop_is_valid(&pending.snapshot.project, start, end)
                    {
                        cursor.loop_ticks = Some((start, end));
                    }
                }
                TransportCommand::ClearLoop => cursor.loop_ticks = None,
            },
            Ok(Message::PlayFromTick { tick, toggle }) => {
                if cursor.state == PlaybackState::Playing {
                    if toggle {
                        cursor.state = PlaybackState::Paused;
                    }
                } else if let Some(pending) = &pitch.pending
                    && let Ok(frame) = pending.snapshot.project.tempo().ticks_to_frames(tick)
                {
                    cursor.frame = frame;
                    play_waiting_cursor(cursor);
                }
            }
            Ok(
                Message::Audition { .. } | Message::BeginAudition { .. } | Message::ReleaseAudition,
            ) => {}
            Ok(Message::LiveMidi { .. }) | Ok(Message::StopRecording) => {}
            Ok(Message::StartRecording { clock, .. }) => {
                clock.stop();
                shared.set_render_error(Some(
                    "Recording is unavailable while clip pitch is being prepared".into(),
                ));
            }
            #[cfg(test)]
            Ok(Message::Synchronize(acknowledged)) => {
                let _ = acknowledged.send(());
            }
            Ok(Message::Shutdown) | Err(_) => return Next::Shutdown,
        }
    }
}

fn play_waiting_cursor(cursor: &mut Cursor) {
    cursor.state = PlaybackState::Playing;
}

fn waiting_loop_is_valid(project: &Project, start: u64, end: u64) -> bool {
    if start >= end {
        return false;
    }
    let (Ok(start_frame), Ok(end_frame)) = (
        project.tempo().ticks_to_frames(start),
        project.tempo().ticks_to_frames(end),
    ) else {
        return false;
    };
    start_frame < end_frame
}

pub struct SilentClock {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SilentClock {
    pub fn start(shared: Arc<Shared>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("sympoie-silent-clock".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let rate = f64::from(sympoie::SAMPLE_RATE);
                let mut scratch = vec![[0.0; 2]; 4096];
                let mut consumed = 0_u64;
                while !thread_stop.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(5));
                    let due = (started.elapsed().as_secs_f64() * rate) as u64;
                    let mut remaining = due.saturating_sub(consumed);
                    consumed = due;
                    while remaining > 0 {
                        let count = remaining.min(scratch.len() as u64) as usize;
                        shared.consume(&mut scratch[..count]);
                        remaining -= count as u64;
                    }
                }
            })
            .expect("the operating system refused to start the playback clock");
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for SilentClock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sympoie::clip::{AudioClip, MidiClip, MidiNote};
    use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
    use sympoie::project::AudioBuffer;
    use sympoie::transport::PlaybackState;

    struct MonitorHost(Arc<AtomicBool>);

    #[derive(Debug)]
    struct NativeGate(Arc<AtomicBool>);

    impl sympoie::plugin::PluginHost for MonitorHost {
        fn load(
            &self,
            _: &sympoie::plugin::PluginInstance,
        ) -> Result<Box<dyn sympoie::plugin::PluginProcessor>, String> {
            Ok(Box::new(NativeGate(Arc::clone(&self.0))))
        }
    }

    impl sympoie::plugin::PluginProcessor for NativeGate {
        fn reset(&mut self) {}
        fn event(&mut self, _: sympoie::instrument::NoteEvent) {}
        fn set_parameter(&mut self, _: u32, _: f64) {}
        fn process(&mut self, frames: &mut [Frame], _: sympoie::plugin::PluginProcessContext) {
            frames.fill(
                [if self.0.load(Ordering::Relaxed) {
                    0.25
                } else {
                    0.0
                }; 2],
            );
        }
    }

    fn monitoring_project(audio_frames: usize) -> Project {
        use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind};

        let mut project = Project::new();
        let midi = project.add_midi_track("Native keyboard");
        project
            .set_plugin_instrument(
                midi,
                Some(PluginInstance::new(
                    PluginDescriptor {
                        path: "/unused-test-plugin.vst3".into(),
                        class_id: "test-monitor".into(),
                        name: "Native keyboard".into(),
                        vendor: "Test".into(),
                        kind: PluginKind::Instrument,
                    },
                    Vec::new(),
                    Vec::new(),
                )),
            )
            .unwrap();
        if audio_frames > 0 {
            let audio = project.add_audio_track("Arrangement ramp");
            let frames = (0..audio_frames)
                .map(|index| [index as f32 / 48_000.0; 2])
                .collect();
            let asset = project.add_asset(AudioBuffer::new(frames).unwrap());
            project
                .add_audio_clip(
                    audio,
                    AudioClip {
                        asset,
                        source_start: 0,
                        start_tick: 0,
                        frame_count: audio_frames as u64,
                        gain: 1.0,
                        pitch_semitones: 0.0,
                        pitch_algorithm: Default::default(),
                        preserve_formants: false,
                        name: None,
                    },
                )
                .unwrap();
        }
        project
    }

    fn monitor_engine(project: Project, native_gate: Arc<AtomicBool>) -> AudioEngine {
        let shared = Arc::new(Shared::new());
        let track_peaks = Arc::new(TrackPeakBank::new(&project));
        let (sender, receiver) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread_peaks = Arc::clone(&track_peaks);
        let pitch_sender = sender.clone();
        let thread = thread::spawn(move || {
            let transport =
                Transport::with_plugin_host(&project, &MonitorHost(native_gate)).unwrap();
            let result = run(
                transport,
                &mut Cursor::default(),
                &receiver,
                &thread_shared,
                &mut None,
                thread_peaks,
                &mut PitchPreparation::new(pitch_sender),
            );
            assert!(
                matches!(result, Next::Shutdown),
                "compatible edits must retain the running monitor"
            );
        });
        AudioEngine {
            sender,
            shared,
            plugins: PluginRuntime::new(),
            track_peaks: Mutex::new(track_peaks),
            thread: Some(thread),
        }
    }

    fn ramp_project() -> Project {
        let mut project = Project::new();
        let frames = (0..48_000).map(|i| [i as f32 / 48_000.0; 2]).collect();
        let asset = project.add_asset(AudioBuffer::new(frames).unwrap());
        let track = project.add_audio_track("Ramp");
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
                    name: None,
                },
            )
            .unwrap();
        project
    }

    fn synth_project(instrument: Option<Instrument>) -> Project {
        let mut project = Project::new();
        let keys = project.add_midi_track("Keys");
        project.set_midi_track_instrument(keys, instrument).unwrap();
        project
            .add_midi_clip(
                keys,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 0,
                    length_ticks: 3840,
                    notes: vec![MidiNote {
                        start_tick: 0,
                        duration_ticks: 960,
                        key: 60,
                        velocity: 100,
                        channel: 0,
                        slide_ticks: 0,
                    }],
                    name: None,
                    pitch_bend: Vec::new(),
                },
            )
            .unwrap();
        project
    }

    fn default_instrument() -> Option<Instrument> {
        Some(Instrument::Subtractive(SubtractiveSettings::default()))
    }

    fn two_level_project() -> Project {
        let mut project = Project::new();
        for (name, level) in [("Quiet", [0.125, 0.25]), ("Loud", [0.5, 0.0625])] {
            let track = project.add_audio_track(name);
            let asset = project.add_asset(AudioBuffer::new(vec![level; 48_000]).unwrap());
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
                        name: None,
                    },
                )
                .unwrap();
        }
        project
    }

    fn wait_until(engine: &AudioEngine, condition: impl Fn(&EngineStatus) -> bool) -> EngineStatus {
        for _ in 0..500 {
            let status = engine.status();
            if condition(&status) {
                return status;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "engine never reached the expected state: {:?}",
            engine.status()
        );
    }

    fn drain(shared: &Shared, frames: usize) -> Vec<Frame> {
        let mut out = vec![[0.0; 2]; frames];
        let mut filled = 0;
        for _ in 0..500 {
            filled += shared.consume(&mut out[filled..]);
            if filled == frames {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        out.truncate(filled);
        out
    }

    fn synchronize_messages(engine: &AudioEngine) {
        let (acknowledged, receiver) = mpsc::channel();
        engine
            .sender
            .send(Message::Synchronize(acknowledged))
            .unwrap();
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("render worker did not apply the queued messages");
    }

    struct DelayHost;

    #[derive(Debug)]
    struct DelayProcessor(std::collections::VecDeque<Frame>);

    impl sympoie::plugin::PluginHost for DelayHost {
        fn load(
            &self,
            _: &sympoie::plugin::PluginInstance,
        ) -> Result<Box<dyn sympoie::plugin::PluginProcessor>, String> {
            Ok(Box::new(DelayProcessor(std::collections::VecDeque::from(
                vec![[0.0; 2]; 73],
            ))))
        }
    }

    impl sympoie::plugin::PluginProcessor for DelayProcessor {
        fn reset(&mut self) {
            self.0.make_contiguous().fill([0.0; 2]);
        }
        fn event(&mut self, _: sympoie::instrument::NoteEvent) {}
        fn set_parameter(&mut self, _: u32, _: f64) {}
        fn latency_samples(&self) -> u32 {
            73
        }
        fn process(&mut self, frames: &mut [Frame], _: sympoie::plugin::PluginProcessContext) {
            for frame in frames {
                self.0.push_back(*frame);
                *frame = self.0.pop_front().unwrap();
            }
        }
    }

    #[test]
    fn plugin_compensation_preserves_ring_timestamps_and_audible_seeks() {
        use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind};
        let mut project = ramp_project();
        let track = project.track_ids().nth(1).unwrap();
        project
            .add_effect(
                track,
                PluginInstance::new(
                    PluginDescriptor {
                        path: "/fake/Delay.vst3".into(),
                        class_id: "delay".into(),
                        name: "Delay".into(),
                        vendor: "Test".into(),
                        kind: PluginKind::Effect,
                    },
                    vec![],
                    vec![],
                ),
            )
            .unwrap();
        let shared = Arc::new(Shared::new());
        let track_peaks = Arc::new(TrackPeakBank::new(&project));
        let (sender, receiver) = mpsc::channel();
        let thread_shared = shared.clone();
        let thread_peaks = Arc::clone(&track_peaks);
        let pitch_sender = sender.clone();
        let thread = thread::spawn(move || {
            let transport = Transport::with_plugin_host(&project, &DelayHost).unwrap();
            assert!(matches!(
                run(
                    transport,
                    &mut Cursor::default(),
                    &receiver,
                    &thread_shared,
                    &mut None,
                    thread_peaks,
                    &mut PitchPreparation::new(pitch_sender),
                ),
                Next::Shutdown
            ));
        });
        let engine = AudioEngine {
            sender,
            shared,
            plugins: PluginRuntime::new(),
            track_peaks: Mutex::new(track_peaks),
            thread: Some(thread),
        };
        engine.command(TransportCommand::SeekFrame(1000));
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let heard = drain(&engine.shared(), 500);
        assert_eq!(heard.len(), 500);
        for (offset, frame) in heard.iter().enumerate() {
            assert_eq!(*frame, [(1000 + offset) as f32 / 48_000.0; 2]);
        }
        assert_eq!(engine.status().position_frame, 1500);
        engine.command(TransportCommand::Pause);
        wait_until(&engine, |status| status.state == PlaybackState::Paused);
        engine.command(TransportCommand::SeekFrame(12_345));
        wait_until(&engine, |status| status.position_frame == 12_345);
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let heard = drain(&engine.shared(), 777);
        for (offset, frame) in heard.iter().enumerate() {
            assert_eq!(*frame, [(12_345 + offset) as f32 / 48_000.0; 2]);
        }
        assert_eq!(engine.status().position_frame, 13_122);
        engine.command(TransportCommand::Pause);
        wait_until(&engine, |status| status.state == PlaybackState::Paused);
        engine.command(TransportCommand::SetLoopTicks {
            start: 96,
            end: 192,
        });
        engine.command(TransportCommand::SeekFrame(2400));
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let heard = drain(&engine.shared(), 2410);
        assert_eq!(heard.len(), 2410);
        for (offset, frame) in heard.iter().enumerate() {
            assert_eq!(*frame, [(2400 + offset % 2400) as f32 / 48_000.0; 2]);
        }
        assert_eq!(engine.status().position_frame, 2410);
        engine.command(TransportCommand::Pause);
        let status = wait_until(&engine, |status| status.state == PlaybackState::Paused);
        assert_eq!(status.position_frame, 2410);
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        assert_eq!(drain(&engine.shared(), 1), [[2410.0 / 48_000.0; 2]]);
    }

    #[test]
    fn plays_the_project_and_reports_the_audible_position() {
        let engine = AudioEngine::start(ramp_project());
        let shared = engine.shared();
        engine.command(TransportCommand::SeekFrame(1000));
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let heard = drain(&shared, 500);
        assert_eq!(heard.len(), 500);
        assert_eq!(heard[0], [1000.0 / 48_000.0; 2]);
        assert_eq!(heard[499], [1499.0 / 48_000.0; 2]);
        assert_eq!(engine.status().position_frame, 1500);
        assert!(engine.take_peaks()[0] > 0.03);
    }

    #[test]
    fn empty_project_plays_at_a_far_marker_and_keeps_its_position_across_rebuilds() {
        let mut project = Project::new();
        let marker = 64_000 * sympoie::TICKS_PER_QUARTER;
        let start_frame = project.tempo().ticks_to_frames(marker).unwrap();
        let engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.play_from_tick(marker);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().position_frame, start_frame);
        assert_eq!(drain(&shared, 4096), [[0.0; 2]; 4096]);
        assert_eq!(engine.status().position_frame, start_frame + 4096);

        project.add_audio_track("Empty track");
        engine.update_project_structural(project);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 4096), [[0.0; 2]; 4096]);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().position_frame, start_frame + 8192);

        engine.command(TransportCommand::Stop);
        engine.command(TransportCommand::SeekFrame(start_frame + 100_000));
        engine.command(TransportCommand::Play);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 4096), [[0.0; 2]; 4096]);
        assert_eq!(engine.status().position_frame, start_frame + 104_096);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().render_error, None);
    }

    #[test]
    fn empty_project_wraps_a_loop_in_empty_space_then_continues_when_cleared() {
        let project = Project::new();
        let marker = 1000 * sympoie::TICKS_PER_QUARTER;
        let start_frame = project.tempo().ticks_to_frames(marker).unwrap();
        let engine = AudioEngine::start(project);
        let shared = engine.shared();
        engine.command(TransportCommand::SetLoopTicks {
            start: marker,
            end: marker + 4,
        });
        engine.play_from_tick(marker);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 1024), [[0.0; 2]; 1024]);
        assert_eq!(engine.status().position_frame, start_frame + 24);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        engine.command(TransportCommand::ClearLoop);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 4096), [[0.0; 2]; 4096]);
        assert_eq!(engine.status().position_frame, start_frame + 4120);
        assert_eq!(engine.status().state, PlaybackState::Playing);
    }

    #[test]
    fn track_peak_bank_accumulates_finite_channels_and_clears_after_reading() {
        let bank = TrackPeakBank::new(&two_level_project());
        bank.publish(&[[0.2, 0.1], [0.125, 0.25], [0.5, 0.0625]]);
        bank.publish(&[[0.1, 0.3], [f32::NAN, f32::INFINITY], [-1.0, 0.5]]);
        assert_eq!(bank.take(), [[0.2, 0.3], [0.125, 0.25], [0.5, 0.5]]);
        assert_eq!(bank.take(), [[0.0; 2]; 3]);
    }

    #[test]
    fn track_peaks_report_each_track_independently() {
        let engine = AudioEngine::start(two_level_project());
        engine.command(TransportCommand::Play);
        wait_until(&engine, |_| {
            engine.shared.ring.buffered() >= TARGET_BUFFERED_FRAMES
        });
        assert_eq!(
            engine.take_track_peaks(),
            [[0.625, 0.3125], [0.125, 0.25], [0.5, 0.0625]]
        );
        assert_eq!(engine.take_track_peaks(), [[0.0; 2]; 3]);
    }

    #[test]
    fn track_peaks_switch_banks_before_reindexed_or_replacement_projects_render() {
        let mut project = two_level_project();
        let engine = AudioEngine::start(project.clone());
        let retired = Arc::clone(&engine.track_peaks.lock().unwrap());
        retired.publish(&[[0.625, 0.3125], [0.125, 0.25], [0.5, 0.0625]]);

        let first_audio = project.track_ids().nth(1).unwrap();
        project.remove_track(first_audio).unwrap();
        engine.update_project(project);
        retired.publish(&[[1.0; 2]; 3]);
        assert_eq!(engine.take_track_peaks(), [[0.0; 2]; 2]);
        synchronize_messages(&engine);
        engine.command(TransportCommand::Play);
        wait_until(&engine, |_| {
            engine.shared.ring.buffered() >= TARGET_BUFFERED_FRAMES
        });
        assert_eq!(engine.take_track_peaks(), [[0.5, 0.0625]; 2]);

        engine.replace_project(Project::new());
        assert_eq!(engine.take_track_peaks(), [[0.0; 2]]);
        synchronize_messages(&engine);
        assert_eq!(engine.take_track_peaks(), [[0.0; 2]]);
    }

    #[test]
    fn play_from_tick_preserves_active_audio_even_for_back_to_back_requests() {
        let engine = AudioEngine::start(ramp_project());
        let shared = engine.shared();
        engine.play_from_tick(96);
        engine.play_from_tick(960);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        let heard = drain(&shared, 500);
        assert_eq!(heard.len(), 500);
        for (offset, frame) in heard.iter().enumerate() {
            assert_eq!(*frame, [(2400 + offset) as f32 / 48_000.0; 2]);
        }

        engine.play_from_tick(192);
        synchronize_messages(&engine);
        assert_eq!(engine.status().position_frame, 2900);
        assert_eq!(drain(&shared, 1), [[2900.0 / 48_000.0; 2]]);
    }

    #[test]
    fn toggle_play_from_tick_resolves_queued_toggles_and_restarts_at_the_marker() {
        let engine = AudioEngine::start(ramp_project());
        let shared = engine.shared();
        engine.toggle_play_from_tick(96);
        engine.toggle_play_from_tick(480);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Paused);
        assert_eq!(engine.status().position_frame, 2400);

        engine.toggle_play_from_tick(480);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(drain(&shared, 1), [[12_000.0 / 48_000.0; 2]]);
        engine.toggle_play_from_tick(960);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Paused);
        assert_eq!(engine.status().position_frame, 12_001);

        engine.toggle_play_from_tick(960);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 1), [[24_000.0 / 48_000.0; 2]]);
    }

    #[test]
    fn play_from_tick_and_toggle_preserve_queued_audio_across_the_last_clip_end() {
        let native_gate = Arc::new(AtomicBool::new(true));
        let engine = monitor_engine(monitoring_project(512), native_gate);
        let shared = engine.shared();
        engine.play_from_tick(0);
        wait_until(&engine, |status| {
            status.state == PlaybackState::Playing
                && shared.transport_frame.load(Ordering::Relaxed) > 512
                && shared.ring.buffered() >= TARGET_BUFFERED_FRAMES
        });
        assert_eq!(drain(&shared, 100).len(), 100);
        engine.play_from_tick(960);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().position_frame, 100);
        assert_eq!(drain(&shared, 1), [[0.25 + 100.0 / 48_000.0; 2]]);

        engine.toggle_play_from_tick(960);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Paused);
        assert_eq!(engine.status().position_frame, 101);
        assert_eq!(drain(&shared, 700), [[0.25; 2]; 700]);

        engine.toggle_play_from_tick(0);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 1), [[0.25; 2]]);
    }

    #[test]
    fn pausing_rewinds_to_what_was_heard() {
        let engine = AudioEngine::start(ramp_project());
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        drain(&shared, 700);
        engine.command(TransportCommand::Pause);
        let status = wait_until(&engine, |status| status.state == PlaybackState::Paused);
        assert_eq!(status.position_frame, 700);
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let resumed = drain(&shared, 1);
        assert_eq!(resumed, [[700.0 / 48_000.0; 2]]);
    }

    #[test]
    fn edits_keep_playing_from_the_same_place() {
        let engine = AudioEngine::start(ramp_project());
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        drain(&shared, 100);
        let mut edited = ramp_project();
        edited.set_snap_grid(sympoie::time::SnapGrid::Bar);
        engine.update_project(edited);
        let heard = drain(&shared, 8000);
        for (index, frame) in heard.iter().enumerate() {
            assert_eq!(frame[0], (100 + index) as f32 / 48_000.0);
        }
        assert_eq!(engine.status().state, PlaybackState::Playing);
    }

    #[test]
    fn forced_structural_updates_keep_the_playback_cursor_and_loop() {
        let project = ramp_project();
        let engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.command(TransportCommand::SetLoopTicks { start: 0, end: 40 });
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        assert_eq!(drain(&shared, 100).len(), 100);
        engine.update_project_structural(project.clone());
        engine.update_project(project);
        synchronize_messages(&engine);
        let heard = drain(&shared, 8000);
        assert_eq!(heard.len(), 8000);
        for (index, frame) in heard.iter().enumerate() {
            assert_eq!(frame[0], ((100 + index) % 1000) as f32 / 48_000.0);
        }
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().render_error, None);
    }

    #[test]
    fn reaching_the_end_continues_through_silence_and_pause_resumes_there() {
        let engine = AudioEngine::start(ramp_project());
        let shared = engine.shared();
        engine.command(TransportCommand::SeekFrame(47_000));
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        let heard = drain(&shared, 1024);
        assert_eq!(heard.len(), 1024);
        assert_eq!(heard[999], [47_999.0 / 48_000.0; 2]);
        assert_eq!(heard[1000], [0.0; 2]);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().position_frame, 48_024);
        assert_eq!(drain(&shared, 4096), [[0.0; 2]; 4096]);
        assert_eq!(engine.status().position_frame, 52_120);
        engine.command(TransportCommand::Pause);
        synchronize_messages(&engine);
        assert_eq!(engine.status().position_frame, 52_120);
        assert_eq!(engine.status().state, PlaybackState::Paused);
        engine.command(TransportCommand::Play);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 1), [[0.0; 2]]);
        assert_eq!(engine.status().position_frame, 52_121);
        assert_eq!(engine.status().state, PlaybackState::Playing);
    }

    #[test]
    fn loops_repeat_the_selected_range() {
        let engine = AudioEngine::start(ramp_project());
        let shared = engine.shared();
        engine.command(TransportCommand::SetLoopTicks {
            start: 96,
            end: 192,
        });
        engine.command(TransportCommand::SeekTick(96));
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let heard = drain(&shared, 2400 * 2 + 10);
        assert_eq!(heard[0][0], 2400.0 / 48_000.0);
        assert_eq!(heard[2399][0], 4799.0 / 48_000.0);
        assert_eq!(heard[2400][0], 2400.0 / 48_000.0);
    }

    #[test]
    fn synth_playback_pause_and_rebuild_chase_notes_at_the_audible_position() {
        let project = synth_project(default_instrument());
        let mut expected = vec![[0.0; 2]; 700];
        sympoie::render::render_block(&project, 0, &mut expected).unwrap();
        assert!(expected.iter().any(|frame| frame[0].abs() > 0.001));
        let engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let heard = drain(&shared, 700);
        assert_eq!(heard, expected);
        engine.command(TransportCommand::Pause);
        let status = wait_until(&engine, |status| status.state == PlaybackState::Paused);
        assert_eq!(status.position_frame, 700);
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let mut resumed = vec![[0.0; 2]; 500];
        sympoie::render::render_block(&project, 700, &mut resumed).unwrap();
        assert_eq!(drain(&shared, 500), resumed);
        engine.update_project(project);
        let after_update = drain(&shared, 8000);
        assert_eq!(after_update.len(), 8000);
        assert!(after_update.iter().any(|frame| frame[0].abs() > 0.001));
        assert!(
            after_update
                .iter()
                .flatten()
                .all(|sample| sample.is_finite())
        );
        assert_eq!(engine.status().position_frame, 9200);
        assert_eq!(engine.status().render_error, None);
    }

    #[test]
    fn turning_an_instrument_control_while_playing_does_not_restart_notes() {
        let settings = SubtractiveSettings {
            waveform: Waveform::Sine,
            cutoff_hz: 20_000.0,
            attack_seconds: 0.001,
            decay_seconds: 0.0,
            sustain: 1.0,
            gain: 0.5,
            ..SubtractiveSettings::default()
        };
        let mut project = Project::new();
        let keys = project.add_midi_track("Keys");
        project
            .set_midi_track_instrument(keys, Some(Instrument::Subtractive(settings)))
            .unwrap();
        project
            .add_midi_clip(
                keys,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 0,
                    length_ticks: 7680,
                    notes: vec![MidiNote {
                        start_tick: 0,
                        duration_ticks: 7680,
                        key: 24,
                        velocity: 127,
                        channel: 0,
                        slide_ticks: 0,
                    }],
                    name: None,
                    pitch_bend: Vec::new(),
                },
            )
            .unwrap();
        let engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let mut heard = drain(&shared, 4000);

        for step in 1..=5 {
            let gain = 0.5 - 0.05 * step as f32;
            let cutoff_hz = 20_000.0 - 3_000.0 * step as f32;
            project
                .set_midi_track_instrument(
                    keys,
                    Some(Instrument::Subtractive(SubtractiveSettings {
                        gain,
                        cutoff_hz,
                        ..settings
                    })),
                )
                .unwrap();
            engine.update_project(project.clone());
            synchronize_messages(&engine);
            heard.extend(drain(&shared, 1500));
        }
        heard.extend(drain(&shared, 8000));
        assert_eq!(heard.len(), 4000 + 5 * 1500 + 8000);
        assert_eq!(engine.status().render_error, None);

        let largest_step = |frames: &[Frame]| {
            frames
                .windows(2)
                .map(|pair| (pair[1][0] - pair[0][0]).abs())
                .fold(0.0, f32::max)
        };
        let steady = largest_step(&heard[500..4000]);
        assert!(
            largest_step(&heard[500..]) < 2.0 * steady + 1e-3,
            "{} vs {steady}",
            largest_step(&heard[500..])
        );
        let peak = |frames: &[Frame]| {
            frames
                .iter()
                .map(|frame| frame[0].abs())
                .fold(0.0, f32::max)
        };
        let ratio = peak(&heard[heard.len() - 3000..]) / peak(&heard[500..4000]);
        assert!((ratio - 0.5).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn mixer_master_and_clip_edits_keep_the_running_instrument_phase() {
        use sympoie::track::{AudioOutput, MixerSettings};

        let mut project = Project::new();
        let keys = project.add_midi_track("Held sine");
        project
            .set_midi_track_instrument(
                keys,
                Some(Instrument::Subtractive(SubtractiveSettings {
                    waveform: Waveform::Sine,
                    cutoff_hz: 20_000.0,
                    attack_seconds: 0.001,
                    decay_seconds: 0.0,
                    sustain: 1.0,
                    gain: 0.5,
                    ..SubtractiveSettings::default()
                })),
            )
            .unwrap();
        project
            .add_midi_clip(
                keys,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 0,
                    length_ticks: 7680,
                    notes: vec![MidiNote {
                        start_tick: 0,
                        duration_ticks: 7680,
                        key: 12,
                        velocity: 127,
                        channel: 0,
                        slide_ticks: 0,
                    }],
                    name: None,
                    pitch_bend: Vec::new(),
                },
            )
            .unwrap();
        let audio = project.add_audio_track("Constant");
        let asset = project.add_asset(AudioBuffer::new(vec![[0.1; 2]; 48_000]).unwrap());
        project
            .add_audio_clip(
                audio,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 48_000,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        let engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let mut heard = drain(&shared, 6000);

        project
            .set_midi_track_mixer(
                keys,
                MixerSettings::new(0.6, 0.4, AudioOutput::Master).unwrap(),
            )
            .unwrap();
        project.set_audio_clip_gain(audio, 0, -0.5).unwrap();
        project
            .set_audio_track_mixer(
                audio,
                MixerSettings::new(0.8, -0.3, AudioOutput::Master).unwrap(),
            )
            .unwrap();
        project
            .set_master_track_mixer(MixerSettings::new(0.7, 0.0, AudioOutput::Output).unwrap())
            .unwrap();
        engine.update_project(project.clone());
        synchronize_messages(&engine);
        heard.extend(drain(&shared, 9000));
        assert_eq!(heard.len(), 15_000);
        assert_eq!(engine.status().position_frame, 15_000);
        assert_eq!(engine.status().render_error, None);
        assert_eq!(engine.status().state, PlaybackState::Playing);

        let mut expected = vec![[0.0; 2]; heard.len()];
        sympoie::render::render_block(&project, 0, &mut expected).unwrap();
        assert_eq!(heard[12_000..], expected[12_000..]);
        for pair in heard[5000..].windows(2) {
            for (previous, next) in pair[0].iter().zip(pair[1]) {
                assert!((next - previous).abs() < 0.005);
            }
        }
    }

    #[test]
    fn routing_rebuild_crossfades_without_flushing_queued_audio() {
        use sympoie::track::{AudioOutput, MixerSettings};

        let mut project = Project::new();
        let audio = project.add_audio_track("Constant");
        let asset = project.add_asset(AudioBuffer::new(vec![[1.0; 2]; 48_000]).unwrap());
        project
            .add_audio_clip(
                audio,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 48_000,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        let engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        assert_eq!(drain(&shared, 6000), [[1.0; 2]; 6000]);
        project
            .set_audio_track_mixer(
                audio,
                MixerSettings::new(1.0, 0.0, AudioOutput::Disconnected).unwrap(),
            )
            .unwrap();
        engine.update_project(project);
        let heard = drain(&shared, 9000);
        assert_eq!(heard.len(), 9000);
        assert_eq!(heard[0], [1.0; 2]);
        assert_eq!(heard[8999], [0.0; 2]);
        let intermediate = heard
            .iter()
            .filter(|frame| frame[0] > 0.0 && frame[0] < 1.0)
            .count();
        assert_eq!(intermediate, TRANSITION_FRAMES - 2);
        for pair in heard.windows(2) {
            assert!(pair[1][0] <= pair[0][0]);
            assert!((pair[1][0] - pair[0][0]).abs() < 0.002);
        }
        assert_eq!(engine.status().position_frame, 15_000);
        assert_eq!(engine.status().render_error, None);
        assert_eq!(engine.status().state, PlaybackState::Playing);
    }

    #[test]
    fn structural_transition_preserves_equal_signals_and_block_boundaries() {
        let old: Vec<Frame> = (0..TRANSITION_FRAMES)
            .map(|index| [index as f32 / 48_000.0; 2])
            .collect();
        let mut unchanged = old.clone();
        let mut transition = Transition {
            tail: old.clone(),
            offset: 0,
        };
        for block in unchanged.chunks_mut(17) {
            transition.mix(block);
        }
        assert_eq!(unchanged, old);

        let mut complete = vec![[0.75; 2]; TRANSITION_FRAMES + 40];
        Transition {
            tail: old.clone(),
            offset: 0,
        }
        .mix(&mut complete);
        let mut fragmented = vec![[0.75; 2]; TRANSITION_FRAMES + 40];
        let mut transition = Transition {
            tail: old,
            offset: 0,
        };
        for block in fragmented.chunks_mut(23) {
            transition.mix(block);
        }
        assert_eq!(fragmented, complete);
        assert_eq!(complete[0], [0.0; 2]);
        assert_eq!(complete[TRANSITION_FRAMES - 1..], [[0.75; 2]; 41]);
    }

    #[test]
    fn deleting_last_clip_finishes_the_entire_fade_and_keeps_playing() {
        let mut project = Project::new();
        let audio = project.add_audio_track("Constant");
        let asset = project.add_asset(AudioBuffer::new(vec![[1.0; 2]; 48_000]).unwrap());
        project
            .add_audio_clip(
                audio,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 48_000,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        let mut engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        assert_eq!(drain(&shared, 6000), [[1.0; 2]; 6000]);
        project.remove_audio_clip(audio, 0).unwrap();
        engine.update_project(project);
        synchronize_messages(&engine);
        let heard = drain(&shared, 9000);
        assert_eq!(heard.len(), 9000);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().position_frame, 15_000);
        assert_eq!(engine.status().render_error, None);
        assert_eq!(heard.last(), Some(&[0.0; 2]));
        assert_eq!(
            heard
                .iter()
                .filter(|frame| frame[0] > 0.0 && frame[0] < 1.0)
                .count(),
            TRANSITION_FRAMES - 2,
            "the empty arrangement must not truncate the old audio's fade"
        );
        for pair in heard.windows(2) {
            assert!(pair[1][0] <= pair[0][0]);
            assert!((pair[1][0] - pair[0][0]).abs() < 0.002);
        }
        engine.shutdown();
        engine.shutdown();
        assert!(engine.thread.is_none());
    }

    #[test]
    fn seeking_past_note_release_and_removing_an_instrument_leave_no_stuck_notes() {
        let project = synth_project(default_instrument());
        let engine = AudioEngine::start(project);
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        assert!(drain(&shared, 1000).iter().any(|frame| frame[0] != 0.0));
        engine.command(TransportCommand::SeekFrame(48_000));
        wait_until(&engine, |status| status.position_frame == 48_000);
        assert!(drain(&shared, 1000).iter().all(|frame| *frame == [0.0; 2]));

        engine.command(TransportCommand::SeekFrame(1000));
        wait_until(&engine, |status| status.position_frame == 1000);
        assert!(drain(&shared, 1000).iter().any(|frame| frame[0] != 0.0));
        engine.update_project(synth_project(None));
        drain(&shared, 8000);
        let mut after_removal = [[1.0; 2]; 512];
        shared.consume(&mut after_removal);
        assert_eq!(after_removal, [[0.0; 2]; 512]);
        assert_eq!(engine.status().render_error, None);
    }

    #[test]
    fn midi_without_an_instrument_is_silent_and_does_not_block_other_tracks() {
        let mut project = ramp_project();
        let keys = project.add_midi_track("Unassigned");
        project
            .add_midi_clip(
                keys,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: 0,
                    length_ticks: 3840,
                    notes: vec![MidiNote {
                        start_tick: 0,
                        duration_ticks: 960,
                        key: 64,
                        velocity: 127,
                        channel: 0,
                        slide_ticks: 0,
                    }],
                    name: None,
                    pitch_bend: Vec::new(),
                },
            )
            .unwrap();
        let engine = AudioEngine::start(project);
        let shared = engine.shared();
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| status.state == PlaybackState::Playing);
        let heard = drain(&shared, 1000);
        assert_eq!(heard.len(), 1000);
        assert_eq!(heard[999], [999.0 / 48_000.0; 2]);
        assert_eq!(engine.status().render_error, None);
    }

    #[test]
    fn unrenderable_musical_time_reports_an_error_and_recovers_on_replacement() {
        let mut project = synth_project(default_instrument());
        let sympoie::track::TrackId::Midi(track) = project.track_ids().nth(1).unwrap() else {
            panic!("expected MIDI track")
        };
        project.move_midi_clip(track, 0, u64::MAX - 3840).unwrap();
        let engine = AudioEngine::start(project);
        wait_until(&engine, |status| status.render_error.is_some());
        engine.command(TransportCommand::SeekTick(960));
        wait_until(&engine, |status| status.position_frame == 24_000);
        engine.command(TransportCommand::Play);
        thread::sleep(Duration::from_millis(20));
        assert_eq!(engine.status().state, PlaybackState::Stopped);
        engine.replace_project(ramp_project());
        wait_until(&engine, |status| {
            status.render_error.is_none() && status.position_frame == 0
        });
    }

    #[test]
    fn native_monitor_audio_continues_stopped_and_paused_without_playing_arrangement() {
        use sympoie::track::{AudioOutput, MixerSettings};

        let native_gate = Arc::new(AtomicBool::new(true));
        let mut project = monitoring_project(48_000);
        let engine = monitor_engine(project.clone(), Arc::clone(&native_gate));
        let shared = engine.shared();
        engine.command(TransportCommand::SeekFrame(1234));
        wait_until(&engine, |status| status.position_frame == 1234);
        assert_eq!(drain(&shared, 4000), [[0.25; 2]; 4000]);
        assert_eq!(engine.status().state, PlaybackState::Stopped);
        assert_eq!(engine.status().position_frame, 1234);

        engine.command(TransportCommand::Pause);
        wait_until(&engine, |status| status.state == PlaybackState::Paused);
        assert_eq!(drain(&shared, 4000), [[0.25; 2]; 4000]);
        assert_eq!(engine.status().position_frame, 1234);
        native_gate.store(false, Ordering::Relaxed);
        let released = drain(&shared, 6000);
        assert_eq!(released.len(), 6000);
        assert_eq!(released[5500..], [[0.0; 2]; 500]);
        native_gate.store(true, Ordering::Relaxed);
        project
            .set_master_track_mixer(MixerSettings::new(0.5, 0.0, AudioOutput::Output).unwrap())
            .unwrap();
        engine.update_project(project);
        synchronize_messages(&engine);
        let changed = drain(&shared, 6000);
        assert_eq!(changed.len(), 6000);
        assert_eq!(changed[5500..], [[0.125; 2]; 500]);
        assert_eq!(engine.status().state, PlaybackState::Paused);
        assert_eq!(engine.status().position_frame, 1234);
    }

    #[test]
    fn empty_plugin_arrangement_keeps_processing_native_audio_and_advancing() {
        let native_gate = Arc::new(AtomicBool::new(true));
        let project = monitoring_project(0);
        assert_eq!(project.duration_frames().unwrap(), 0);
        let engine = monitor_engine(project, native_gate);
        let shared = engine.shared();
        assert_eq!(drain(&shared, 1), [[0.25; 2]]);
        engine.command(TransportCommand::Play);
        wait_until(&engine, |status| {
            status.state == PlaybackState::Playing
                && shared.ring.buffered() >= TARGET_BUFFERED_FRAMES
        });
        assert_eq!(drain(&shared, CHUNK_FRAMES), [[0.25; 2]; CHUNK_FRAMES]);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().position_frame, CHUNK_FRAMES as u64);
        assert!(!shared.ring.is_drained());
        assert_eq!(drain(&shared, 4000), [[0.25; 2]; 4000]);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(engine.status().position_frame, CHUNK_FRAMES as u64 + 4000);
        engine.command(TransportCommand::Stop);
        synchronize_messages(&engine);
        assert_eq!(drain(&shared, 4000), [[0.25; 2]; 4000]);
        assert_eq!(engine.status().state, PlaybackState::Stopped);
        assert_eq!(engine.status().position_frame, CHUNK_FRAMES as u64 + 4000);
    }

    #[test]
    fn pause_and_stop_resume_at_the_audible_frame_before_and_after_the_last_clip() {
        for (command, expected_state) in [
            (TransportCommand::Pause, PlaybackState::Paused),
            (TransportCommand::Stop, PlaybackState::Stopped),
        ] {
            for audible in [100, 700] {
                let native_gate = Arc::new(AtomicBool::new(true));
                let engine = monitor_engine(monitoring_project(512), native_gate);
                let shared = engine.shared();
                engine.command(TransportCommand::Play);
                wait_until(&engine, |status| {
                    status.state == PlaybackState::Playing
                        && shared.transport_frame.load(Ordering::Relaxed) > 512
                        && shared.ring.buffered() >= TARGET_BUFFERED_FRAMES
                });
                let before = drain(&shared, audible);
                assert_eq!(before.len(), audible);
                assert_eq!(before[99], [0.25 + 99.0 / 48_000.0; 2]);
                assert_eq!(engine.status().position_frame, audible as u64);
                engine.command(command);
                let status = wait_until(&engine, |status| status.state == expected_state);
                assert_eq!(status.position_frame, audible as u64);
                assert_eq!(drain(&shared, 700), [[0.25; 2]; 700]);
                assert_eq!(engine.status().position_frame, audible as u64);
                engine.command(TransportCommand::Play);
                wait_until(&engine, |status| status.state == PlaybackState::Playing);
                let expected = if audible < 512 {
                    0.25 + audible as f32 / 48_000.0
                } else {
                    0.25
                };
                assert_eq!(drain(&shared, 1), [[expected; 2]]);
                assert_eq!(engine.status().position_frame, audible as u64 + 1);
            }
        }
    }

    #[test]
    fn piano_auditions_are_audible_stopped_and_paused_without_moving_the_cursor() {
        for paused in [false, true] {
            let engine = AudioEngine::start(synth_project(default_instrument()));
            let shared = engine.shared();
            engine.command(TransportCommand::SeekFrame(321));
            if paused {
                engine.command(TransportCommand::Pause);
            }
            engine.audition_notes(
                1,
                vec![MidiNote {
                    start_tick: 999,
                    duration_ticks: 960,
                    key: 72,
                    velocity: 100,
                    channel: 0,
                    slide_ticks: 0,
                }],
            );
            synchronize_messages(&engine);
            let output = drain(&shared, 22_000);
            assert_eq!(output.len(), 22_000);
            assert!(output.iter().any(|frame| frame[0].abs() > 0.01));
            assert!(output[21_000..].iter().any(|frame| frame[0].abs() > 0.01));
            assert_eq!(engine.status().position_frame, 321);
            assert_eq!(
                engine.status().state,
                if paused {
                    PlaybackState::Paused
                } else {
                    PlaybackState::Stopped
                }
            );
            engine.command(TransportCommand::Stop);
            synchronize_messages(&engine);
            let mut after_stop = [[99.0; 2]; CHUNK_FRAMES];
            assert_eq!(shared.consume(&mut after_stop), 0);
            assert_eq!(after_stop, [[0.0; 2]; CHUNK_FRAMES]);
        }
    }

    #[test]
    fn held_piano_auditions_wait_for_release_while_stopped_and_paused() {
        for paused in [false, true] {
            let engine = AudioEngine::start(synth_project(default_instrument()));
            let shared = engine.shared();
            engine.command(TransportCommand::SeekFrame(321));
            if paused {
                engine.command(TransportCommand::Pause);
            }
            engine.begin_audition_notes(
                1,
                vec![MidiNote {
                    start_tick: 999,
                    duration_ticks: 1,
                    key: 72,
                    velocity: 100,
                    channel: 0,
                    slide_ticks: 0,
                }],
            );
            synchronize_messages(&engine);
            let held = drain(&shared, 96_000);
            assert_eq!(held.len(), 96_000);
            assert!(held[95_000..].iter().any(|frame| frame[0].abs() > 0.01));

            engine.release_audition_notes();
            synchronize_messages(&engine);
            let released = drain(&shared, 48_000);
            assert!(
                released.len() < 24_000,
                "preview never finished its release"
            );
            assert!(released.iter().any(|frame| frame[0].abs() > 0.001));
            assert!(
                released[released.len() - 64..]
                    .iter()
                    .all(|frame| *frame == [0.0; 2])
            );
            assert_eq!(engine.status().position_frame, 321);
            assert_eq!(
                engine.status().state,
                if paused {
                    PlaybackState::Paused
                } else {
                    PlaybackState::Stopped
                }
            );
        }
    }

    #[test]
    fn held_preview_release_after_rebuild_does_not_replay_the_note() {
        let mut project = synth_project(default_instrument());
        let engine = AudioEngine::start(project.clone());
        let shared = engine.shared();
        engine.begin_audition_notes(
            1,
            vec![MidiNote {
                start_tick: 0,
                duration_ticks: 1,
                key: 72,
                velocity: 100,
                channel: 0,
                slide_ticks: 0,
            }],
        );
        synchronize_messages(&engine);
        assert!(
            drain(&shared, 1024)
                .iter()
                .any(|frame| frame[0].abs() > 0.01)
        );
        project.add_audio_track("New track");
        engine.update_project_structural(project);
        engine.release_audition_notes();
        synchronize_messages(&engine);
        let after = drain(&shared, 24_000);
        assert!(after.len() < 16_000);
        assert!(
            after[after.len() - 64..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert_eq!(engine.status().render_error, None);
    }

    #[test]
    fn reordering_identical_midi_tracks_releases_the_old_index_audition() {
        use sympoie::track::{AudioOutput, MixerSettings};

        let mut project = Project::new();
        for (name, pan) in [("Left", -1.0), ("Right", 1.0)] {
            let track = project.add_midi_track(name);
            project
                .set_midi_track_instrument(track, default_instrument())
                .unwrap();
            project
                .set_midi_track_mixer(
                    track,
                    MixerSettings::new(1.0, pan, AudioOutput::Master).unwrap(),
                )
                .unwrap();
        }
        let reordered = {
            let mut reordered = project.clone();
            reordered.reorder_tracks(&[0, 2, 1]).unwrap();
            reordered
        };
        let mut transport = Transport::new(&project).unwrap();
        assert!(transport.update_parameters(&reordered));

        let engine = AudioEngine::start(project);
        let shared = engine.shared();
        engine.command(TransportCommand::SeekFrame(321));
        engine.command(TransportCommand::Pause);
        let note = MidiNote {
            start_tick: 0,
            duration_ticks: 96_000,
            key: 72,
            velocity: 100,
            channel: 0,
            slide_ticks: 0,
        };
        engine.audition_notes(1, vec![note]);
        synchronize_messages(&engine);
        let initial = drain(&shared, 1024);
        assert_eq!(initial.len(), 1024);
        assert!(initial.iter().any(|frame| frame[0].abs() > 0.01));
        assert!(initial.iter().all(|frame| frame[1] == 0.0));
        wait_until(&engine, |_| {
            shared.ring.buffered() >= TARGET_BUFFERED_FRAMES
        });

        engine.update_project_structural(reordered.clone());
        engine.update_project(reordered);
        synchronize_messages(&engine);
        let settling_frames = TARGET_BUFFERED_FRAMES as usize + CHUNK_FRAMES + TRANSITION_FRAMES;
        let read_frames = settling_frames + CHUNK_FRAMES;
        let after = drain(&shared, read_frames);
        assert_eq!(after.len(), read_frames);
        assert!(
            after[settling_frames..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert_eq!(engine.status().state, PlaybackState::Paused);
        assert_eq!(engine.status().position_frame, 321);
        assert_eq!(engine.status().render_error, None);

        engine.audition_notes(1, vec![note]);
        synchronize_messages(&engine);
        let current = drain(&shared, read_frames);
        assert_eq!(current.len(), read_frames);
        assert!(current.iter().all(|frame| frame[0] == 0.0));
        assert!(current.iter().any(|frame| frame[1].abs() > 0.01));
    }

    #[test]
    fn piano_audition_uses_the_instrument_from_a_preceding_project_rebuild() {
        let engine = AudioEngine::start(synth_project(None));
        engine.update_project(synth_project(default_instrument()));
        engine.audition_notes(
            1,
            vec![MidiNote {
                start_tick: 0,
                duration_ticks: 960,
                key: 84,
                velocity: 100,
                channel: 2,
                slide_ticks: 0,
            }],
        );
        synchronize_messages(&engine);
        let output = drain(&engine.shared(), 1024);
        assert!(output.iter().any(|frame| frame[0].abs() > 0.01));
        assert_eq!(engine.status().state, PlaybackState::Stopped);
        assert_eq!(engine.status().position_frame, 0);
        assert!(engine.status().render_error.is_none());
    }

    #[test]
    fn recording_empty_project_begins_at_marker_and_survives_rebuild() {
        let mut project = Project::new();
        let marker = 7 * sympoie::TICKS_PER_QUARTER;
        let start_frame = project.tempo().ticks_to_frames(marker).unwrap();
        let engine = AudioEngine::start(project.clone());
        let clock = Arc::new(RecordingClock::new());
        engine.start_recording(marker, Arc::clone(&clock));
        synchronize_messages(&engine);
        assert_eq!(clock.start_frame(), Some(start_frame));
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(drain(&engine.shared(), 4096).len(), 4096);
        assert_eq!(engine.status().position_frame, start_frame + 4096);
        project.add_audio_track("New track");
        engine.update_project_structural(project);
        synchronize_messages(&engine);
        assert_eq!(drain(&engine.shared(), 4096).len(), 4096);
        assert_eq!(engine.status().position_frame, start_frame + 8192);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert!(clock.offset_at(std::time::Instant::now()).is_some());
        engine.stop_recording();
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Playing);
        assert_eq!(drain(&engine.shared(), 4096), [[0.0; 2]; 4096]);
        assert_eq!(engine.status().position_frame, start_frame + 12_288);
        assert!(clock.offset_at(std::time::Instant::now()).is_none());
        engine.command(TransportCommand::Stop);
        synchronize_messages(&engine);
        assert_eq!(engine.status().state, PlaybackState::Stopped);
        assert!(clock.offset_at(std::time::Instant::now()).is_none());
    }

    #[test]
    fn recording_running_playback_anchors_to_audible_frame_without_restart() {
        let engine = AudioEngine::start(ramp_project());
        engine.command(TransportCommand::Play);
        synchronize_messages(&engine);
        let shared = engine.shared();
        assert_eq!(drain(&shared, 512).len(), 512);
        let clock = Arc::new(RecordingClock::new());
        engine.start_recording(8 * sympoie::TICKS_PER_QUARTER, Arc::clone(&clock));
        synchronize_messages(&engine);
        assert_eq!(clock.start_frame(), Some(512));
        let output = drain(&shared, 128);
        assert!((output[0][0] - 512.0 / 48_000.0).abs() < 1e-7);
        assert_eq!(engine.status().position_frame, 640);
    }

    #[test]
    fn canceled_recording_start_never_starts_playback() {
        let engine = AudioEngine::start(Project::new());
        let clock = Arc::new(RecordingClock::new());
        clock.stop();
        engine.start_recording(0, Arc::clone(&clock));
        synchronize_messages(&engine);
        assert!(!clock.is_started());
        assert_eq!(engine.status().state, PlaybackState::Stopped);
    }
}
