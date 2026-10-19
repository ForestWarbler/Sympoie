//! Cached offline renders for instruments set to their Render engine (Ferro).
//!
//! The manager watches the document. Whenever a rendering instrument's notes, settings,
//! automation or the tempo change, its old render stops playing at once (the realtime
//! engine takes over) and a new render starts once edits have settled for a moment. Renders
//! run one at a time on a worker thread with the accurate offline engine; finished ones
//! are handed to the audio engine, which plays them in place of the instrument's notes.
//! Progress and state show on the device through its telemetry.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sympoie::Frame;
use sympoie::bounce::{InstrumentRenderPlan, instrument_render_key};
use sympoie::builtin::{BuiltinKind, RenderState, ferro};
use sympoie::plugin::RenderQuality;
use sympoie::project::Project;
use sympoie::render::Renderer;
use sympoie::track::Track;

use super::bounce::render_frames;
use crate::plugins::PluginRuntime;

/// Edits within this long of each other share one render.
pub const SETTLE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
enum State {
    /// Waiting for edits to settle (or for the worker), with the project to render.
    Waiting {
        due: Instant,
        project: Box<Project>,
    },
    Rendering,
    Ready(Arc<[Frame]>),
    /// The last render failed; the user was told when it happened.
    Failed,
}

#[derive(Debug, Clone)]
struct Entry {
    track: usize,
    key: u64,
    state: State,
}

struct Job {
    plugin: u64,
    key: u64,
    total: u64,
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<Vec<Frame>, String>>>,
}

impl Job {
    fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.progress.load(Ordering::Relaxed) as f32 / self.total as f32
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a finished poll asks of the host.
#[derive(Debug, Default, PartialEq)]
pub struct Poll {
    /// The set of playable renders changed; hand [`AnalogRenders::renders`] to the engine.
    pub changed: bool,
    /// A render failed, with a message for the user.
    pub failure: Option<String>,
}

#[derive(Default)]
pub struct AnalogRenders {
    entries: BTreeMap<u64, Entry>,
    job: Option<Job>,
}

/// Plug-in instruments set to render offline, as (track index, plug-in id).
fn wanted(project: &Project) -> Vec<(usize, u64)> {
    project
        .tracks()
        .iter()
        .enumerate()
        .filter_map(|(index, track)| match track {
            Track::Midi(midi) => midi.plugin_instrument().and_then(|plugin| {
                (BuiltinKind::from_descriptor(&plugin.descriptor) == Some(BuiltinKind::Ferro)
                    && ferro::renders_offline(plugin))
                .then_some((index, plugin.id))
            }),
            _ => None,
        })
        .collect()
}

impl AnalogRenders {
    /// Brings the wanted renders in line with `project`. Returns whether the playable set
    /// changed (a render went stale or an instrument stopped rendering).
    pub fn sync(&mut self, project: &Project, plugins: &PluginRuntime) -> bool {
        let now = Instant::now();
        let wanted = wanted(project);
        let mut changed = false;
        self.entries.retain(|plugin, entry| {
            let keep = wanted.iter().any(|(_, id)| id == plugin);
            if !keep {
                changed |= matches!(entry.state, State::Ready(_));
                if let Some(telemetry) = plugins.builtin_telemetry(*plugin) {
                    telemetry.set_render_state(RenderState::Live, 0.0);
                }
            }
            keep
        });
        for (track, plugin) in wanted {
            let Some(key) = instrument_render_key(project, track) else {
                continue;
            };
            if self
                .entries
                .get(&plugin)
                .is_some_and(|entry| entry.key == key && entry.track == track)
            {
                continue;
            }
            let previous = self.entries.insert(
                plugin,
                Entry {
                    track,
                    key,
                    state: State::Waiting {
                        due: now + SETTLE,
                        project: Box::new(project.clone()),
                    },
                },
            );
            changed |= previous.is_some_and(|entry| matches!(entry.state, State::Ready(_)));
            if let Some(telemetry) = plugins.builtin_telemetry(plugin) {
                telemetry.set_render_state(RenderState::Rendering, 0.0);
            }
        }
        if self.job.as_ref().is_some_and(|job| {
            self.entries
                .get(&job.plugin)
                .is_none_or(|entry| entry.key != job.key)
        }) {
            self.job = None;
        }
        changed
    }

    /// Finishes and starts renders. Call every frame while [`Self::busy`].
    pub fn poll(&mut self, plugins: &PluginRuntime) -> Poll {
        let mut poll = Poll::default();
        if let Some(job) = &mut self.job {
            if let Some(telemetry) = plugins.builtin_telemetry(job.plugin) {
                telemetry.set_render_state(RenderState::Rendering, job.fraction());
            }
            if job.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                let result = job
                    .thread
                    .take()
                    .expect("checked above")
                    .join()
                    .unwrap_or_else(|_| Err("the render thread crashed".to_owned()));
                let (plugin, key) = (job.plugin, job.key);
                self.job = None;
                if let Some(entry) = self.entries.get_mut(&plugin)
                    && entry.key == key
                {
                    let telemetry = plugins.builtin_telemetry(plugin);
                    match result {
                        Ok(frames) => {
                            entry.state = State::Ready(Arc::from(frames));
                            poll.changed = true;
                            if let Some(telemetry) = telemetry {
                                telemetry.set_render_state(RenderState::Ready, 1.0);
                            }
                        }
                        Err(error) => {
                            poll.failure = Some(error);
                            entry.state = State::Failed;
                            if let Some(telemetry) = telemetry {
                                telemetry.set_render_state(RenderState::Failed, 0.0);
                            }
                        }
                    }
                }
            }
        }
        if self.job.is_none() {
            let now = Instant::now();
            let due = self
                .entries
                .iter()
                .find_map(|(plugin, entry)| match &entry.state {
                    State::Waiting { due, .. } if *due <= now => Some(*plugin),
                    _ => None,
                });
            if let Some(plugin) = due {
                let entry = self.entries.get_mut(&plugin).expect("found above");
                let State::Waiting { project, .. } =
                    std::mem::replace(&mut entry.state, State::Rendering)
                else {
                    unreachable!("found waiting above");
                };
                match start(&project, entry.track, plugin, entry.key, plugins) {
                    Ok(Some(job)) => self.job = Some(job),
                    Ok(None) => {
                        // No notes: the render is silence.
                        entry.state = State::Ready(Arc::from(Vec::new()));
                        poll.changed = true;
                        if let Some(telemetry) = plugins.builtin_telemetry(plugin) {
                            telemetry.set_render_state(RenderState::Ready, 1.0);
                        }
                    }
                    Err(error) => {
                        poll.failure = Some(error);
                        entry.state = State::Failed;
                        if let Some(telemetry) = plugins.builtin_telemetry(plugin) {
                            telemetry.set_render_state(RenderState::Failed, 0.0);
                        }
                    }
                }
            }
        }
        poll
    }

    /// Whether renders are waiting or running, so the host should keep polling.
    pub fn busy(&self) -> bool {
        self.job.is_some()
            || self
                .entries
                .values()
                .any(|entry| matches!(entry.state, State::Waiting { .. }))
    }

    /// The finished renders to play, by plug-in id.
    pub fn renders(&self) -> Vec<(u64, Arc<[Frame]>)> {
        self.entries
            .iter()
            .filter_map(|(plugin, entry)| match &entry.state {
                State::Ready(frames) => Some((*plugin, Arc::clone(frames))),
                _ => None,
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.job = None;
        self.entries.clear();
    }
}

fn start(
    project: &Project,
    track: usize,
    plugin: u64,
    key: u64,
    plugins: &PluginRuntime,
) -> Result<Option<Job>, String> {
    let Some(plan) = InstrumentRenderPlan::new(project, track)? else {
        return Ok(None);
    };
    let runtime = plugins.prepare_offline(&plan.project)?;
    let progress = Arc::new(AtomicU64::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let total = plan.end_frame;
    let thread = {
        let (progress, cancel) = (Arc::clone(&progress), Arc::clone(&cancel));
        thread::Builder::new()
            .name("sympoie-analog-render".into())
            .spawn(move || {
                let _pitch_pins = plan
                    .project
                    .prepare_audio_pitch(|| cancel.load(Ordering::Relaxed))
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "render cancelled".to_owned())?;
                let mut renderer = Renderer::with_plugin_host(&plan.project, &*runtime)
                    .map_err(|error| error.to_string())?;
                renderer.set_quality(RenderQuality::Offline);
                render_frames(
                    &mut renderer,
                    0,
                    plan.end_frame,
                    |done| progress.store(done, Ordering::Relaxed),
                    &cancel,
                    || {
                        let errors = runtime.take_errors();
                        if errors.is_empty() {
                            Ok(())
                        } else {
                            Err(errors.join("; "))
                        }
                    },
                )
            })
            .map_err(|error| format!("cannot start the render: {error}"))?
    };
    Ok(Some(Job {
        plugin,
        key,
        total,
        progress,
        cancel,
        thread: Some(thread),
    }))
}

#[cfg(test)]
#[path = "analog_render_tests.rs"]
mod tests;
