use std::path::Path;

use eframe::egui::{self, RichText};

use serde_json::{Value, json};
use sympoie::track::Track;
use sympoie::transport::PlaybackState;

use super::SympoieApp;
use crate::actions::Action;
use crate::agent::{AgentLink, Event, SaveError};
use crate::mcp::server::{McpServer, Request};
use crate::ui::theme;

const AUTOSAVE_DELAY: f64 = 1.0;
const RETRY_DELAY: f64 = 5.0;
const FLASH: f64 = 1.6;

#[derive(Default)]
pub(super) struct AgentState {
    link: Option<AgentLink>,
    next_token: u64,
    in_flight: Option<(u64, u64, u64)>,
    seen: (u64, u64),
    changed_at: f64,
    retry_at: f64,
    generation: u64,
    conflict: Option<(u64, Box<sympoie::project::Project>)>,
    broken: Option<String>,
    flash_until: f64,
    mcp: Option<McpServer>,
    mcp_failed: bool,
    last_change: Option<Result<(), String>>,
}

impl SympoieApp {
    pub(super) fn agent_mode(&self) -> bool {
        self.agent.link.is_some()
    }

    pub(super) fn agent_led(&self, now: f64) -> bool {
        if now < self.agent.flash_until {
            return (now * 8.0).fract() < 0.5;
        }
        self.agent_mode()
    }

    pub(super) fn agent_problem(&self) -> Option<&str> {
        self.agent.broken.as_deref()
    }

    pub(super) fn toggle_agent_mode(&mut self) {
        if self.agent_mode() {
            if let Some(folder) = self.document.path().map(Path::to_owned) {
                self.settings.agent.remember(&folder, false);
                self.save_settings();
            }
            self.stop_agent_mode();
            self.info("Agent mode off".to_owned());
            return;
        }
        if (self.document.path().is_none_or(|path| !path.is_dir()) || self.document.is_dirty())
            && !self.save(false)
        {
            return;
        }
        let Some(folder) = self.document.path().filter(|path| path.is_dir()) else {
            return;
        };
        let folder = folder.to_owned();
        self.settings.agent.remember(&folder, true);
        self.save_settings();
        self.start_agent_mode(&folder);
        self.info(format!(
            "Agent mode on: {} saves as you work and loads the agent's changes",
            super::file_name(&folder)
        ));
    }

    pub(super) fn start_agent_mode(&mut self, folder: &Path) {
        let now = self.last_time.unwrap_or(0.0);
        let exe = std::env::current_exe().unwrap_or_else(|_| "sympoie".into());
        for note in crate::agent_docs::write(folder, &exe) {
            self.warn(format!("Agent mode: {note}"));
        }
        self.agent = AgentState {
            link: Some(AgentLink::start(folder.to_owned())),
            seen: (
                self.document.version(),
                self.document.external_state_revision(),
            ),
            changed_at: now,
            ..AgentState::default()
        };
    }

    pub(super) fn stop_agent_mode(&mut self) {
        self.agent = AgentState::default();
    }

    pub(super) fn save_through_agent(
        &mut self,
        path: &Path,
        project: sympoie::project::Project,
    ) -> Option<Result<(), String>> {
        let link = self.agent.link.as_ref()?;
        if link.folder() != path {
            self.stop_agent_mode();
            return None;
        }
        let result = link
            .save_now(project, self.agent.generation)
            .map_err(|error| error.to_string());
        if result.is_ok() {
            self.agent.broken = None;
        }
        Some(result)
    }

    pub(super) fn poll_agent(&mut self, ctx: &egui::Context) {
        if self.agent.link.is_none() {
            return;
        }
        let now = ctx.input(|input| input.time);
        if self.agent.mcp.is_none() && !self.agent.mcp_failed {
            let folder = self
                .agent
                .link
                .as_ref()
                .map(|link| link.folder().to_owned());
            if let Some(folder) = folder {
                let wake = ctx.clone();
                match McpServer::start(&folder, move || wake.request_repaint()) {
                    Ok(server) => self.agent.mcp = Some(server),
                    Err(error) => {
                        self.agent.mcp_failed = true;
                        self.warn(format!("Agent tools are unavailable: {error}"));
                    }
                }
            }
        }
        while let Some(request) = self.agent.mcp.as_ref().and_then(McpServer::try_request) {
            self.answer_tool(ctx, request);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(
            if now < self.agent.flash_until {
                40
            } else {
                200
            },
        ));
        while let Some(event) = self.agent.link.as_ref().and_then(AgentLink::try_event) {
            match event {
                Event::Saved { token, result } => self.finish_autosave(token, result, now),
                Event::Changed { generation, result } => match *result {
                    Ok(project) => {
                        self.agent.last_change = Some(Ok(()));
                        if self.document.is_dirty() || self.agent.in_flight.is_some() {
                            self.agent.conflict = Some((generation, Box::new(project)));
                        } else {
                            self.agent.generation = generation;
                            self.apply_agent_project(project, now);
                        }
                    }
                    Err(error) => {
                        self.agent.last_change = Some(Err(error.clone()));
                        self.agent.generation = generation;
                        self.warn(format!(
                            "The project files have an error, so autosave is paused until it is fixed: {error}"
                        ));
                        self.agent.broken = Some(error);
                    }
                },
            }
        }
        let current = (
            self.document.version(),
            self.document.external_state_revision(),
        );
        if current != self.agent.seen {
            self.agent.seen = current;
            self.agent.changed_at = now;
        }
        let busy = ctx.input(|input| input.pointer.any_down())
            || self.recording.is_some()
            || self.agent.in_flight.is_some()
            || self.agent.conflict.is_some()
            || self.agent.broken.is_some();
        if busy
            || !self.document.is_dirty()
            || now - self.agent.changed_at < AUTOSAVE_DELAY
            || now < self.agent.retry_at
        {
            return;
        }
        let project = match self.project_with_plugin_state() {
            Ok(project) => project,
            Err(error) => {
                self.warn(format!("Autosave could not capture plug-in state: {error}"));
                self.agent.retry_at = now + RETRY_DELAY;
                return;
            }
        };
        self.agent.next_token += 1;
        let token = self.agent.next_token;
        self.agent.in_flight = Some((token, current.0, current.1));
        if let Some(link) = &self.agent.link {
            link.save(project, token, self.agent.generation);
        }
    }

    fn finish_autosave(&mut self, token: u64, result: Result<(), SaveError>, now: f64) {
        let Some((expected, version, external)) = self.agent.in_flight else {
            return;
        };
        if token != expected {
            return;
        }
        self.agent.in_flight = None;
        match result {
            Ok(()) => self.document.mark_version_saved(version, external),
            Err(SaveError::Stale) => self.agent.retry_at = now + AUTOSAVE_DELAY,
            Err(SaveError::Failed(error)) => {
                self.error(format!("Autosave failed: {error}"));
                self.agent.retry_at = now + RETRY_DELAY;
            }
        }
    }

    fn apply_agent_project(&mut self, project: sympoie::project::Project, now: f64) {
        self.agent.broken = None;
        let applied = self
            .edit_structural("Agent Edit", move |current| {
                *current = project;
                Ok(())
            })
            .is_some();
        if applied {
            let version = self.document.version();
            let external = self.document.external_state_revision();
            self.document.mark_version_saved(version, external);
            self.agent.seen = (version, external);
            self.agent.flash_until = now + FLASH;
            self.info("Loaded the agent's changes · ⌘Z undoes them".to_owned());
        }
    }

    fn answer_tool(&mut self, ctx: &egui::Context, request: Request) {
        let tool = request.tool.clone();
        match tool.as_str() {
            "get_context" => {
                let context = self.agent_context();
                request.answer(Ok(context));
            }
            "transport" => {
                let result = self.agent_transport(ctx, &request.arguments);
                request.answer(result);
            }
            "analyze_audio" => {
                let prepared = self.prepare_analysis(&request.arguments);
                let (_, _, responder) = request.into_parts();
                match prepared {
                    Err(error) => responder.answer(Err(error)),
                    Ok((project, start, end, wav)) => {
                        // Plug-ins are created here on the main thread; JUCE plug-ins crash
                        // when instantiated on the analysis thread.
                        let plugins = match self.plugins.prepare_offline(&project) {
                            Ok(plugins) => plugins,
                            Err(error) => return responder.answer(Err(error)),
                        };
                        let spawned = std::thread::Builder::new()
                            .name("sympoie-analysis".into())
                            .spawn(move || {
                                let result = crate::audio::analysis::analyze(
                                    &project,
                                    &plugins,
                                    start,
                                    end,
                                    wav.as_deref(),
                                );
                                // The main thread releases the plug-ins once this lets go.
                                drop(plugins);
                                responder.answer(result.map(|mut value| {
                                    if let Some(path) = wav {
                                        value["saved_wav"] = json!(path.display().to_string());
                                    }
                                    value
                                }));
                            });
                        if let Err(error) = spawned {
                            self.warn(format!("Could not start the analysis: {error}"));
                        }
                    }
                }
            }
            other => request.answer(Err(format!("unknown tool {other}"))),
        }
    }

    fn agent_context(&self) -> Value {
        let project = self.document.project();
        let meter = project.time_signature();
        let ticks_per_bar = meter.ticks_per_bar();
        let ticks_per_beat = ticks_per_bar / u64::from(meter.numerator().max(1));
        let describe = |index: usize| -> Value {
            let Some(track) = project.tracks().get(index) else {
                return Value::Null;
            };
            json!({
                "id": track.uid(),
                "type": match track {
                    Track::Audio(_) => "audio",
                    Track::Midi(_) => "midi",
                    Track::Group(_) => "group",
                    Track::Bus(_) => "bus",
                    Track::Master(_) => "master",
                },
                "name": track.name(),
                "file": sympoie::project_json::track_file(track),
            })
        };
        let position = |tick: u64| {
            json!({
                "tick": tick,
                "bar": tick / ticks_per_bar + 1,
                "beat": (tick % ticks_per_bar) / ticks_per_beat.max(1) + 1,
            })
        };
        let clip = self.selection.clip.and_then(|clip| {
            let track = project.tracks().get(clip.track)?;
            let (start, end) = crate::document::edit::clip_span(project, clip)?;
            Some(json!({
                "id": track.clip_uids().get(clip.index)?,
                "track_id": track.uid(),
                "start_tick": start,
                "end_tick": end,
            }))
        });
        let range = self.selection.range.as_ref().map(|range| {
            json!({
                "start_tick": range.start,
                "end_tick": range.end,
                "track_ids": range
                    .tracks
                    .iter()
                    .filter_map(|&index| project.tracks().get(index).map(Track::uid))
                    .collect::<Vec<_>>(),
            })
        });
        json!({
            "project_folder": self.document.path().map(|path| path.display().to_string()),
            "unsaved_changes": self.document.is_dirty(),
            "files_status": match &self.agent.broken {
                Some(error) => json!({"loads": false, "error": error}),
                None => json!({"loads": true}),
            },
            "last_outside_change": match &self.agent.last_change {
                None => Value::Null,
                Some(Ok(())) if self.agent.conflict.is_some() =>
                    json!("waiting: the person is choosing between their version and yours"),
                Some(Ok(())) => json!("loaded"),
                Some(Err(error)) => json!(format!("not loaded: {error}")),
            },
            "tempo": project.bpm(),
            "time_signature": [meter.numerator(), meter.denominator()],
            "ticks_per_quarter": sympoie::TICKS_PER_QUARTER,
            "ticks_per_bar": ticks_per_bar,
            "playback": match self.status.state {
                PlaybackState::Playing => "playing",
                PlaybackState::Paused => "paused",
                PlaybackState::Stopped => "stopped",
            },
            "playhead": position(self.playhead()),
            "start_marker": position(self.arrangement.playback_start),
            "loop": {
                "enabled": self.loop_region.enabled,
                "start_tick": self.loop_region.start,
                "end_tick": self.loop_region.end,
            },
            "selection": {
                "tracks": self.selection.tracks.iter().map(|&index| describe(index)).collect::<Vec<_>>(),
                "clip": clip,
                "time_range": range,
            },
            "tracks": (0..project.tracks().len()).map(|index| {
                let mut entry = describe(index);
                entry["clips"] = json!(project.tracks()[index].clip_uids().len());
                entry
            }).collect::<Vec<_>>(),
        })
    }

    fn agent_transport(&mut self, ctx: &egui::Context, arguments: &Value) -> Result<Value, String> {
        let tick = |key: &str| {
            arguments
                .get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("{key} is required and must be a whole number of ticks"))
        };
        let playing = self.status.state == PlaybackState::Playing;
        match arguments
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "play" if !playing => self.apply(ctx, Action::TogglePlay),
            "pause" if playing => self.apply(ctx, Action::TogglePlay),
            "play" | "pause" => {}
            "stop" => {
                if self.status.state != PlaybackState::Stopped {
                    self.apply(ctx, Action::Stop);
                }
            }
            "seek" => self.apply(ctx, Action::Seek(tick("tick")?)),
            "set_loop" => {
                let (start, end) = (tick("start_tick")?, tick("end_tick")?);
                if end <= start {
                    return Err("end_tick must be after start_tick".to_owned());
                }
                self.apply(
                    ctx,
                    Action::SetLoop {
                        start,
                        end,
                        enable: true,
                    },
                );
            }
            "clear_loop" => {
                if self.loop_region.enabled {
                    self.apply(ctx, Action::ToggleLoop);
                }
            }
            other => return Err(format!("unknown action \"{other}\"")),
        }
        Ok(json!({"done": true}))
    }

    #[allow(clippy::type_complexity)]
    fn prepare_analysis(
        &self,
        arguments: &Value,
    ) -> Result<
        (
            sympoie::project::Project,
            u64,
            u64,
            Option<std::path::PathBuf>,
        ),
        String,
    > {
        let mut project = self.project_with_plugin_state()?;
        let end_of_project = {
            let frames = project
                .duration_frames()
                .map_err(|error| error.to_string())?;
            let seconds = frames as f64 / f64::from(sympoie::SAMPLE_RATE);
            (seconds * project.bpm() / 60.0 * sympoie::TICKS_PER_QUARTER as f64).ceil() as u64
        };
        let (default_start, default_end) = if self.loop_region.enabled {
            (self.loop_region.start, self.loop_region.end)
        } else {
            (
                0,
                end_of_project.max(project.time_signature().ticks_per_bar()),
            )
        };
        let start = arguments
            .get("start_tick")
            .and_then(Value::as_u64)
            .unwrap_or(default_start);
        let end = arguments
            .get("end_tick")
            .and_then(Value::as_u64)
            .unwrap_or(default_end.max(start + 1));
        if let Some(ids) = arguments.get("tracks").and_then(Value::as_array) {
            let mut solo = Vec::new();
            for id in ids {
                let uid = id.as_u64().ok_or("tracks must be track IDs")?;
                let index = project
                    .track_index_by_uid(uid)
                    .ok_or_else(|| format!("no track has ID {uid}"))?;
                solo.push(index);
            }
            let ids: Vec<_> = project.track_ids().collect();
            for index in solo {
                let id = ids[index];
                let mixer = project.tracks()[index].mixer().with_soloed(true);
                let result = match id {
                    sympoie::track::TrackId::Audio(id) => project.set_audio_track_mixer(id, mixer),
                    sympoie::track::TrackId::Midi(id) => project.set_midi_track_mixer(id, mixer),
                    sympoie::track::TrackId::Group(id) => project.set_group_track_mixer(id, mixer),
                    sympoie::track::TrackId::Bus(id) => project.set_bus_track_mixer(id, mixer),
                    sympoie::track::TrackId::Master(_) => Ok(()),
                };
                result.map_err(|error| error.to_string())?;
            }
        }
        let wav = match arguments.get("save_wav").and_then(Value::as_str) {
            None => None,
            Some(path) => {
                let name = path
                    .strip_prefix("renders/")
                    .and_then(|name| name.strip_suffix(".wav"))
                    .filter(|name| {
                        !name.is_empty()
                            && name.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
                            })
                    })
                    .ok_or("save_wav must look like renders/NAME.wav, with letters, digits, '-' or '_'")?;
                let folder = self
                    .document
                    .path()
                    .filter(|path| path.is_dir())
                    .ok_or("the project has no folder to save into")?
                    .join("renders");
                std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
                Some(folder.join(format!("{name}.wav")))
            }
        };
        Ok((project, start, end, wav))
    }

    pub(super) fn agent_conflict_window(&mut self, ctx: &egui::Context) {
        if self.agent.conflict.is_none() {
            return;
        }
        let mut choice = None;
        egui::Window::new("The project changed on disk")
            .id(egui::Id::new("agent-conflict"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size(egui::vec2(380.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    "The project's files were changed from outside Sympoie while you had \
                     edits that were not saved yet.",
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Using the agent's version keeps yours one undo step back. Keeping \
                         yours saves it over the agent's changes.",
                    )
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Use the Agent's Version").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Keep Mine").clicked() {
                        choice = Some(false);
                    }
                });
            });
        let Some(take_agent) = choice else {
            return;
        };
        let now = ctx.input(|input| input.time);
        let Some((generation, project)) = self.agent.conflict.take() else {
            return;
        };
        self.agent.generation = generation;
        if take_agent {
            self.apply_agent_project(*project, now);
        } else {
            self.agent.changed_at = f64::NEG_INFINITY;
            self.agent.retry_at = 0.0;
            self.info("Kept your version; saving it over the agent's changes".to_owned());
        }
    }
}

#[cfg(test)]
impl SympoieApp {
    pub fn agent_mode_for_tests(&self) -> bool {
        self.agent_mode()
    }

    pub fn agent_problem_for_tests(&self) -> Option<&str> {
        self.agent_problem()
    }

    pub fn agent_conflict_for_tests(&self) -> bool {
        self.agent.conflict.is_some()
    }
}
