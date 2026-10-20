mod agent_mode;
mod apply;
mod bounce;
mod dialogs;
pub(crate) mod export_window;
mod files;
mod input;
mod menus;
mod plugin_sync;
mod recording;
mod settings_window;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Frame, Margin, Panel, Ui, ViewportCommand};
use sympoie::SAMPLE_RATE;
use sympoie::plugin::PluginDescriptor;
use sympoie::project::Project;
use sympoie::track::Track;
use sympoie::transport::PlaybackState;

use self::bounce::TrackBounce;
use crate::audio::export::ExportJob;
use crate::audio::record_input::InputCatalog;
use crate::audio::{AudioEngine, EngineStatus, Output};
use crate::document::Document;
use crate::document::edit::ClipContent;
use crate::plugins::{PluginEditor, PluginRuntime};
use crate::settings::Settings;
use crate::state::{ArrangementState, LoopRegion, Selection};
use crate::ui::waveform::WaveformCache;
use crate::ui::widgets::MeterState;
use crate::ui::{format, theme};
use crate::views::arrangement::DropTarget;
use crate::views::browser::{self, BrowserState};
use crate::views::piano_roll::{self, PianoRollState};
use crate::views::plugin_browser::PluginBrowser;
use crate::views::{arrangement, clip_view, devices, mixer, transport_bar};

#[derive(Debug, Default, Clone)]
pub struct Startup {
    pub project: Option<PathBuf>,
    pub audio: Vec<PathBuf>,
    pub use_audio_device: bool,
    pub settings: Settings,
    pub settings_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Info,
    Warning,
    Error,
}

struct Notice {
    text: String,
    kind: NoticeKind,
    at: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    New,
    Open(Option<PathBuf>),
    OpenFolder,
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailView {
    Devices,
    Clip,
    Mixer,
    Hidden,
}

struct PluginLoad {
    track: usize,
    generation: u64,
    plugin: PluginDescriptor,
    before: Option<u64>,
    mixer: bool,
    ready: bool,
}

struct ClipClipboard {
    content: ClipContent,
    text: String,
}

pub struct SympoieApp {
    document: Document,
    plugins: Arc<PluginRuntime>,
    plugin_browser: PluginBrowser,
    browser: BrowserState,
    plugin_load: Option<PluginLoad>,
    vst3_editors: BTreeSet<u64>,
    analog_renders: crate::audio::analog_render::AnalogRenders,
    scope_windows: BTreeSet<u64>,
    sidechain_editors: BTreeSet<u64>,
    native_editors: BTreeMap<u64, PluginEditor>,
    plugin_window_owner: Option<vst3_host::PluginWindowOwner>,
    structural_generation: u64,
    engine: AudioEngine,
    output: Output,
    use_input_devices: bool,
    use_output_device: bool,
    settings: Settings,
    settings_path: Option<PathBuf>,
    show_settings: bool,
    settings_window: settings_window::SettingsWindow,
    armed_tracks: BTreeSet<usize>,
    recording: Option<recording::RecordingSession>,
    input_catalog: InputCatalog,
    input_discovery: Option<std::sync::mpsc::Receiver<InputCatalog>>,
    synced_revision: u64,
    synced_structural_generation: u64,
    status: EngineStatus,
    meter: MeterState,
    track_meters: Vec<MeterState>,
    track_meter_generation: u64,
    last_time: Option<f64>,
    arrangement: ArrangementState,
    piano_roll: PianoRollState,
    selection: Selection,
    clip_clipboard: Option<ClipClipboard>,
    loop_region: LoopRegion,
    detail_view: DetailView,
    plugin_editors: BTreeSet<usize>,
    show_browser: bool,
    show_rack: bool,
    notice: Option<Notice>,
    pending: Option<Pending>,
    allowed_to_close: bool,
    show_shortcuts: bool,
    show_about: bool,
    export: Option<ExportJob>,
    export_window: export_window::ExportWindow,
    bounce: Option<TrackBounce>,
    waveforms: WaveformCache,
    window_title: String,
    drop_target: Option<DropTarget>,
    fit_pending: bool,
    frames_drawn: u64,
    agent: agent_mode::AgentState,
}

impl SympoieApp {
    pub fn load_library_folders(&mut self) {
        if let Err(error) = self.browser.load_folders() {
            self.warn(error);
        }
    }

    pub fn set_plugin_window_owner(&mut self, owner: vst3_host::PluginWindowOwner) {
        self.plugin_window_owner = Some(owner);
    }

    pub fn new(ctx: &egui::Context, startup: Startup) -> Self {
        theme::install(ctx);
        let plugins = PluginRuntime::new();
        plugins
            .prepare_project(&Project::new())
            .expect("an empty project has no plugins to prepare");
        let engine = AudioEngine::with_plugins(Project::new(), Arc::clone(&plugins));
        let settings = startup.settings.clone().sanitized();
        ctx.set_zoom_factor(settings.general.interface_scale);
        let output = Output::open_with(engine.shared(), startup.use_audio_device, &settings.audio);
        let status = engine.status();
        let mut app = Self {
            document: Document::new(new_project(&settings), None),
            plugins,
            plugin_browser: PluginBrowser::default(),
            browser: BrowserState::default(),
            plugin_load: None,
            vst3_editors: BTreeSet::new(),
            analog_renders: Default::default(),
            scope_windows: BTreeSet::new(),
            sidechain_editors: BTreeSet::new(),
            native_editors: BTreeMap::new(),
            plugin_window_owner: None,
            structural_generation: 0,
            engine,
            output,
            use_input_devices: startup.use_audio_device,
            use_output_device: startup.use_audio_device,
            settings_window: settings_window::SettingsWindow::new(&settings),
            settings_path: startup.settings_path,
            show_settings: false,
            armed_tracks: BTreeSet::new(),
            recording: None,
            input_catalog: InputCatalog::default(),
            input_discovery: None,
            synced_revision: 0,
            synced_structural_generation: 0,
            status,
            meter: MeterState::default(),
            track_meters: Vec::new(),
            track_meter_generation: 0,
            last_time: None,
            arrangement: ArrangementState {
                follow: settings.general.follow_playback,
                ..ArrangementState::default()
            },
            piano_roll: PianoRollState::default(),
            selection: Selection::default(),
            clip_clipboard: None,
            loop_region: LoopRegion::default(),
            detail_view: DetailView::Devices,
            plugin_editors: BTreeSet::new(),
            show_browser: true,
            show_rack: true,
            notice: None,
            pending: None,
            allowed_to_close: false,
            show_shortcuts: false,
            show_about: false,
            export: None,
            export_window: export_window::ExportWindow::default(),
            bounce: None,
            waveforms: WaveformCache::default(),
            window_title: String::new(),
            drop_target: None,
            fit_pending: false,
            frames_drawn: 0,
            agent: agent_mode::AgentState::default(),
            settings,
        };
        if let Some(path) = startup.project {
            app.open(&path);
        }
        if !startup.audio.is_empty() {
            app.import(startup.audio, None, Some(0));
            app.document.break_coalescing();
        }
        app.refresh_recording_inputs();
        let waker = ctx.clone();
        vst3_host::set_editor_key_waker(move || waker.request_repaint());
        app
    }

    pub fn start_plugin_scan(&mut self) {
        self.plugin_browser.start_scan();
    }

    pub fn scans_plugins_at_startup(&self) -> bool {
        self.settings.general.scan_plugins_at_startup
    }

    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.forget_closed_editor_viewports(&ctx);
        self.poll(&ctx);
        self.sync_editor_visibility();
        let mut actions = Vec::new();
        self.piano_roll.flush_audition_release(&ctx, &mut actions);
        arrangement::resize_shortcuts(&ctx, &mut self.arrangement);
        piano_roll::shortcuts(
            &ctx,
            &mut self.piano_roll,
            self.document.project(),
            &mut actions,
        );
        self.shortcuts(&ctx, &mut actions);
        self.collect_drops(&ctx, &mut actions);
        if self.fit_pending && self.frames_drawn > 0 {
            self.fit_pending = false;
            self.zoom_to_fit();
        }

        Panel::top("menu-bar")
            .frame(
                Frame::NONE
                    .fill(theme::BG_DEEP)
                    .inner_margin(Margin::symmetric(6, 1)),
            )
            .show(ui, |ui| self.menu_bar(ui, &mut actions));
        let playhead = arrangement::playhead_tick(
            self.document.project(),
            &self.status,
            self.loop_region,
            self.arrangement.playback_start,
        );
        Panel::top("transport")
            .exact_size(32.0)
            .frame(
                Frame::NONE
                    .fill(theme::BG_PANEL)
                    .inner_margin(Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| {
                let inputs = transport_bar::Inputs {
                    project: self.document.project(),
                    status: &self.status,
                    loop_region: self.loop_region,
                    follow: self.arrangement.follow,
                    playhead_tick: playhead,
                    output: self.output.info(),
                    recording: self.recording.is_some(),
                };
                transport_bar::show(ui, &inputs, &mut self.meter, &mut actions);
            });
        Panel::bottom("status-bar")
            .exact_size(22.0)
            .frame(
                Frame::NONE
                    .fill(theme::BG_PANEL)
                    .inner_margin(Margin::symmetric(8, 2)),
            )
            .show(ui, |ui| self.status_bar(ui, &mut actions));
        if self.show_rack {
            Panel::right("rack")
                .resizable(false)
                .exact_size(devices::RACK_WIDTH)
                .frame(Frame::NONE.fill(theme::BG_DEEP))
                .show(ui, |ui| {
                    devices::show(
                        ui,
                        self.document.project(),
                        &self.selection,
                        &self.plugins,
                        &mut actions,
                    );
                });
        }
        if self.piano_roll.clip.is_some() {
            Panel::bottom("piano-roll")
                .resizable(true)
                .default_size(320.0)
                .size_range(260.0..=600.0)
                .frame(Frame::NONE.fill(theme::BG_PANEL))
                .show(ui, |ui| {
                    piano_roll::show(
                        ui,
                        &mut self.piano_roll,
                        self.document.project(),
                        playhead.max(0.0).round() as u64,
                        &mut actions,
                    );
                });
        } else if self.detail_view == DetailView::Clip {
            Panel::bottom("clip-view")
                .resizable(true)
                .default_size(250.0)
                .size_range(180.0..=560.0)
                .frame(Frame::NONE.fill(theme::BG_PANEL))
                .show(ui, |ui| {
                    clip_view::show(
                        ui,
                        self.document.project(),
                        &self.selection,
                        &mut self.waveforms,
                        &mut actions,
                    );
                });
        } else if self.detail_view == DetailView::Mixer {
            Panel::bottom("mixer")
                .resizable(true)
                .default_size(400.0)
                .size_range(330.0..=700.0)
                .frame(Frame::NONE.fill(theme::BG_DEEP))
                .show(ui, |ui| {
                    mixer::show_with_runtime(
                        ui,
                        self.document.project(),
                        &self.selection,
                        &self.arrangement.folded,
                        &mut self.meter,
                        &mut self.track_meters,
                        &self.plugins,
                        &mut actions,
                    );
                });
        }
        if self.show_browser {
            Panel::right("browser")
                .resizable(true)
                .default_size(244.0)
                .size_range(220.0..=380.0)
                .frame(Frame::NONE.fill(theme::BG_PANEL))
                .show(ui, |ui| {
                    let inputs = browser::Inputs {
                        project: self.document.project(),
                        selection: &self.selection,
                        catalog: self.plugin_browser.catalog(),
                        scan_status: self.plugin_browser.scan_status(),
                        loading: self.plugin_load.is_some(),
                        mixer: self.detail_view == DetailView::Mixer
                            && self.piano_roll.clip.is_none(),
                    };
                    browser::show(ui, &mut self.browser, &inputs, &mut actions);
                });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let inputs = arrangement::Inputs {
                project: self.document.project(),
                status: &self.status,
                selection: &self.selection,
                loop_region: self.loop_region,
                armed_tracks: &self.armed_tracks,
                recording: self.recording.is_some(),
                recording_start_tick: self.recording.as_ref().and_then(|take| take.start_tick()),
                input_catalog: &self.input_catalog,
            };
            self.drop_target = arrangement::show(
                ui,
                &mut self.arrangement,
                &inputs,
                &mut self.waveforms,
                &mut self.track_meters,
                &mut self.meter,
                &mut actions,
            );
        });

        let mut editor_actions = Vec::new();
        if let Some(track) = self.editor_track()
            && self.plugin_editors.contains(&track)
        {
            devices::show_editor(&ctx, self.document.project(), track, &mut editor_actions);
        }
        let visible_plugins = self.selected_plugin_ids();
        for &id in self.vst3_editors.intersection(&visible_plugins) {
            devices::show_vst3_editor_with_runtime(
                &ctx,
                self.document.project(),
                &self.plugins,
                id,
                &mut editor_actions,
            );
        }
        // Scopes are monitors, so they stay up while another track is selected.
        for &id in &self.scope_windows {
            devices::show_builtin_scope(
                &ctx,
                self.document.project(),
                &self.plugins,
                id,
                &mut editor_actions,
            );
        }
        self.keep_tool_windows_above_main(&visible_plugins);
        for &id in self.sidechain_editors.intersection(&visible_plugins) {
            devices::show_sidechain_editor(
                &ctx,
                self.document.project(),
                &self.plugins,
                id,
                &mut editor_actions,
            );
        }
        self.plugin_browser.show(
            &ctx,
            self.document.project(),
            self.plugin_load.is_some(),
            &mut actions,
        );
        self.dialogs(&ctx);
        self.settings_window(&ctx);
        self.export_window(&ctx);
        self.agent_conflict_window(&ctx);
        self.handle_close_request(&ctx);
        let had_actions = !editor_actions.is_empty() || !actions.is_empty();
        for action in editor_actions.into_iter().chain(actions) {
            self.apply(&ctx, action);
        }
        let mut preview_cleanup = Vec::new();
        self.piano_roll
            .flush_audition_release(&ctx, &mut preview_cleanup);
        for action in preview_cleanup {
            self.apply(&ctx, action);
        }
        self.sync_engine();
        self.sync_editor_visibility();
        self.update_title(&ctx);
        self.frames_drawn += 1;

        if had_actions {
            ctx.request_repaint();
        }

        let busy = self.status.state == PlaybackState::Playing
            || self.meter.is_active()
            || self.track_meters.iter().any(MeterState::is_active)
            || self.export.is_some()
            || self.bounce.is_some()
            || self.notice.is_some()
            || self.plugin_load.is_some()
            || self
                .selected_plugin_ids()
                .iter()
                .any(|id| self.native_editors.contains_key(id));
        if busy {
            ctx.request_repaint();
        } else if self.plugin_browser.busy()
            || self.document.project().tracks().iter().any(|track| {
                track_plugin_ids(track).next().is_some()
                    || matches!(track, Track::Midi(midi) if midi.instrument().is_some())
            })
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }

    fn close_drawer(&mut self) {
        self.piano_roll.close();
        self.detail_view = DetailView::Devices;
    }

    fn release_preview_before_dialog(&mut self) {
        self.piano_roll.cancel_audition();
        self.engine.release_audition_notes();
    }

    /// While the window is minimized, hidden or covered, eframe runs no ui pass, so
    /// `frame` never runs; it only calls `App::logic`. Agent tool calls must still be
    /// answered then.
    pub fn poll_while_hidden(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().visible().unwrap_or(true)) {
            return;
        }
        self.status = self.engine.status();
        self.poll_agent(ctx);
        self.sync_engine();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_recording_inputs(ctx);
        let structural_generation = self.structural_generation;
        self.poll_plugins(ctx);
        self.poll_bounce();
        self.poll_analog_renders(ctx);
        if self.structural_generation != structural_generation {
            arrangement::cancel_interactions(ctx);
        }
        self.status = self.engine.status();
        self.poll_recording(ctx);
        self.poll_agent(ctx);
        let now = ctx.input(|i| i.time);
        let dt = self
            .last_time
            .map_or(0.0, |last| (now - last) as f32)
            .clamp(0.0, 0.25);
        self.last_time = Some(now);
        self.meter.update(self.engine.take_peaks(), dt);
        let track_count = self.document.project().tracks().len();
        if self.track_meter_generation != self.structural_generation
            || self.track_meters.len() != track_count
        {
            self.track_meters = vec![MeterState::default(); track_count];
            self.track_meter_generation = self.structural_generation;
        }
        let peaks = self.engine.take_track_peaks();
        let input_peaks = self
            .recording
            .as_mut()
            .map(|take| std::mem::take(&mut take.input_peaks))
            .unwrap_or_default();
        for (index, meter) in self.track_meters.iter_mut().enumerate().skip(1) {
            let mut peak = if peaks.len() == track_count {
                peaks[index]
            } else {
                [0.0; 2]
            };
            if let Some(input) = input_peaks.get(&index) {
                peak = [peak[0].max(input[0]), peak[1].max(input[1])];
            }
            meter.update(peak, dt);
        }
        if let Some(notice) = &self.notice
            && now - notice.at > 8.0
        {
            self.notice = None;
        }
        if let Some(job) = &mut self.export
            && let Some(result) = job.poll()
        {
            self.export = None;
            match result {
                Ok(summaries) => {
                    let clipped: u64 = summaries
                        .iter()
                        .map(|summary| summary.clipped_samples)
                        .sum();
                    let message = match summaries.as_slice() {
                        [summary] => {
                            let seconds = summary.frames as f64 / f64::from(SAMPLE_RATE);
                            format!(
                                "Exported {} to {}",
                                format::duration(seconds),
                                file_name(&summary.path)
                            )
                        }
                        summaries => format!("Exported {} files", summaries.len()),
                    };
                    if clipped > 0 {
                        self.warn(format!(
                            "{message}; {clipped} samples clipped at full scale"
                        ));
                    } else {
                        self.info(message);
                    }
                }
                Err(error) => self.error(format!("Export failed: {error}")),
            }
        }
        if let Some(error) = self.output.device_error() {
            let message = format!("Audio device: {error}");
            if self
                .notice
                .as_ref()
                .is_none_or(|notice| notice.text != message)
            {
                self.warn(message);
            }
        }
    }

    fn notify(&mut self, kind: NoticeKind, text: String) {
        self.notice = Some(Notice {
            text,
            kind,
            at: self.last_time.unwrap_or(0.0),
        });
    }

    fn info(&mut self, text: String) {
        self.notify(NoticeKind::Info, text);
    }

    fn warn(&mut self, text: String) {
        self.notify(NoticeKind::Warning, text);
    }

    fn error(&mut self, text: String) {
        self.notify(NoticeKind::Error, text);
    }

    fn sync_engine(&mut self) {
        if self.document.revision() != self.synced_revision {
            self.synced_revision = self.document.revision();
            self.selection.validate(self.document.project());
            self.plugin_editors.retain(|&index| {
                matches!(self.document.project().tracks().get(index),
                    Some(Track::Midi(track)) if track.instrument().is_some())
            });
            self.vst3_editors
                .retain(|id| self.document.project().plugin(*id).is_some());
            self.scope_windows
                .retain(|id| self.document.project().plugin(*id).is_some());
            self.sidechain_editors
                .retain(|id| self.document.project().plugin(*id).is_some());
            self.native_editors
                .retain(|id, _| self.document.project().plugin(*id).is_some());
            if let Err(error) = self.plugins.prepare_project(self.document.project()) {
                self.error(format!("Could not prepare plugins: {error}"));
            }
            // Retire stale renders before the engine sees the edit, so it never plays a
            // render of the old notes against the new ones.
            if self
                .analog_renders
                .sync(self.document.project(), &self.plugins)
            {
                self.engine
                    .set_instrument_renders(self.analog_renders.renders());
            }
            if self.synced_structural_generation != self.structural_generation {
                self.engine
                    .update_project_structural(self.document.project().clone());
                self.synced_structural_generation = self.structural_generation;
            } else {
                self.engine.update_project(self.document.project().clone());
            }
            self.waveforms.retain(self.document.project());
        }
    }

    fn poll_analog_renders(&mut self, ctx: &egui::Context) {
        let poll = self.analog_renders.poll(&self.plugins);
        if poll.changed {
            self.engine
                .set_instrument_renders(self.analog_renders.renders());
        }
        if let Some(error) = poll.failure {
            self.warn(format!(
                "Ferro render failed; playing live instead: {error}"
            ));
        }
        if self.analog_renders.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn replace_document(&mut self, document: Document) {
        self.finish_recording();
        self.stop_agent_mode();
        self.armed_tracks.clear();
        self.bounce = None;
        self.analog_renders.clear();
        self.engine.set_instrument_renders(Vec::new());
        self.structural_generation = self.structural_generation.wrapping_add(1);
        self.vst3_editors.clear();
        self.scope_windows.clear();
        self.sidechain_editors.clear();
        self.native_editors.clear();
        self.plugin_load = None;
        self.plugin_browser.open = false;
        self.plugins.clear();
        self.document = document;
        if let Err(error) = self.plugins.prepare_project(self.document.project()) {
            self.error(format!("Could not prepare plugins: {error}"));
        }
        self.engine.replace_project(self.document.project().clone());
        self.synced_structural_generation = self.structural_generation;
        self.track_meters = vec![MeterState::default(); self.document.project().tracks().len()];
        self.track_meter_generation = self.structural_generation;
        self.synced_revision = self.document.revision();
        self.selection = Selection::default();
        self.piano_roll.close();
        self.detail_view = DetailView::Devices;
        self.plugin_editors.clear();
        self.loop_region = LoopRegion::default();
        self.arrangement.folded.clear();
        self.arrangement.automation_targets.clear();
        self.arrangement.track_heights.clear();
        self.arrangement.renaming = None;
        self.arrangement.scroll_tick = 0.0;
        self.arrangement.playback_start = 0;
        self.waveforms = WaveformCache::default();
        self.fit_pending = true;
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let dirty = if self.document.is_dirty() { " •" } else { "" };
        let title = format!("{}{dirty} — Sympoie", self.document.display_name());
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn playhead(&self) -> u64 {
        arrangement::playhead_tick(
            self.document.project(),
            &self.status,
            self.loop_region,
            self.arrangement.playback_start,
        )
        .max(0.0)
        .round() as u64
    }

    fn zoom_to_fit(&mut self) {
        let project = self.document.project();
        let end =
            (project.duration_frames().unwrap_or(0) as f64 * project.bpm() / 3000.0).ceil() as u64;
        let midi_end = project
            .tracks()
            .iter()
            .filter_map(|track| match track {
                Track::Midi(midi) => midi
                    .clips()
                    .iter()
                    .map(|clip| clip.start_tick + clip.length_ticks)
                    .max(),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let bar = project.time_signature().ticks_per_bar();
        self.arrangement.fit(0, end.max(midi_end).max(bar * 8));
    }
}

impl Drop for SympoieApp {
    fn drop(&mut self) {
        if let Some(mut take) = self.recording.take() {
            take.clock.stop();
            take.input.finish();
        }
        self.native_editors.clear();
        self.engine.shutdown();
        self.plugins.clear();
        self.plugins.service();
    }
}

fn new_project(settings: &Settings) -> Project {
    let mut project = Project::new();
    project.set_snap_grid(sympoie::time::SnapGrid::Quarter);
    let general = &settings.general;
    let _ = project.set_bpm(general.default_tempo);
    if let Ok(signature) =
        sympoie::time::TimeSignature::new(u16::from(general.default_beats_per_bar), 4)
    {
        project.set_time_signature(signature);
    }
    project
}

fn track_plugin_ids(track: &Track) -> impl Iterator<Item = u64> + '_ {
    let instrument = match track {
        Track::Midi(midi) => midi.plugin_instrument(),
        _ => None,
    };
    instrument
        .into_iter()
        .chain(track.all_effects())
        .map(|plugin| plugin.id)
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Every extension Sympoie imports, in both cases for case-sensitive dialogs.
fn audio_dialog_extensions() -> Vec<String> {
    sympoie::audio_import::EXTENSIONS
        .iter()
        .flat_map(|extension| [extension.to_string(), extension.to_uppercase()])
        .collect()
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
impl SympoieApp {
    pub fn recording_for_tests(&self) -> bool {
        self.recording.is_some()
    }

    pub fn armed_tracks_for_tests(&self) -> &BTreeSet<usize> {
        &self.armed_tracks
    }

    pub fn apply_for_tests(&mut self, ctx: &egui::Context, action: crate::actions::Action) {
        self.apply(ctx, action);
    }

    pub fn document_for_tests(&self) -> &Document {
        &self.document
    }

    pub fn selection_for_tests(&self) -> &Selection {
        &self.selection
    }

    pub fn plugin_editors_for_tests(&self) -> &BTreeSet<usize> {
        &self.plugin_editors
    }

    pub fn vst3_editors_for_tests(&self) -> &BTreeSet<u64> {
        &self.vst3_editors
    }

    pub fn scope_windows_for_tests(&self) -> &BTreeSet<u64> {
        &self.scope_windows
    }

    pub fn sidechain_editors_for_tests(&self) -> &BTreeSet<u64> {
        &self.sidechain_editors
    }

    pub fn showing_devices_for_tests(&self) -> bool {
        self.show_rack && self.piano_roll.clip.is_none() && self.detail_view == DetailView::Devices
    }

    pub fn detail_view_for_tests(&self) -> &'static str {
        if self.piano_roll.clip.is_some() {
            "piano"
        } else {
            match self.detail_view {
                DetailView::Devices => "devices",
                DetailView::Clip => "clip",
                DetailView::Mixer => "mixer",
                DetailView::Hidden => "hidden",
            }
        }
    }

    pub fn status_for_tests(&self) -> &EngineStatus {
        &self.status
    }

    pub fn track_meter_for_tests(&self, index: usize) -> MeterState {
        if index == 0 {
            self.meter
        } else {
            self.track_meters[index]
        }
    }

    pub fn capture_audio_for_tests(&mut self) -> Arc<crate::audio::engine::Shared> {
        self.engine =
            AudioEngine::with_plugins(self.document.project().clone(), Arc::clone(&self.plugins));
        self.synced_revision = self.document.revision();
        self.synced_structural_generation = self.structural_generation;
        self.engine.shared()
    }

    pub fn loop_for_tests(&self) -> LoopRegion {
        self.loop_region
    }

    pub fn arrangement_for_tests(&self) -> &ArrangementState {
        &self.arrangement
    }

    pub fn notice_for_tests(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }

    pub fn bouncing_for_tests(&self) -> bool {
        self.bounce.is_some()
    }

    pub fn cancel_bounce_for_tests(&self) {
        if let Some(pending) = &self.bounce {
            pending.job.cancel();
        }
    }

    pub fn exporting_for_tests(&self) -> bool {
        self.export.is_some()
    }

    pub fn export_window_for_tests(&mut self) -> &mut export_window::ExportWindow {
        &mut self.export_window
    }

    pub fn settings_for_tests(&self) -> &Settings {
        &self.settings
    }

    pub fn export_clip_midi_for_tests(
        &mut self,
        clip: crate::document::edit::ClipRef,
        path: &Path,
    ) {
        self.export_clip_midi(clip, path);
    }

    pub fn export_clip_audio_for_tests(
        &mut self,
        clip: crate::document::edit::ClipRef,
        path: PathBuf,
    ) {
        self.export_clip_audio(clip, path);
    }
}

impl eframe::App for SympoieApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_while_hidden(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // TEMP-RAIN-SHOT: temporary screenshot hook for visual review; remove before commit.
        if let Some(dir) = std::env::var_os("SYMPOIE_SHOT_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let trigger = dir.join("take");
            let pinned = egui::Id::new("rain-shot-pinned");
            if !ui
                .ctx()
                .data_mut(|data| data.get_temp::<bool>(pinned).unwrap_or(false))
            {
                ui.ctx().data_mut(|data| data.insert_temp(pinned, true));
                ui.ctx().send_viewport_cmd(ViewportCommand::WindowLevel(
                    egui::WindowLevel::AlwaysOnTop,
                ));
            }
            if trigger.exists() {
                let _ = std::fs::remove_file(&trigger);
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
            }
            ui.ctx().input(|input| {
                for event in &input.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        let stamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let [w, h] = image.size;
                        let path = dir.join(format!("shot-{stamp}-{w}x{h}.rgba"));
                        let _ = std::fs::write(path, image.as_raw());
                    }
                }
            });
            let command = dir.join("cmd");
            if let Ok(text) = std::fs::read_to_string(&command) {
                let _ = std::fs::remove_file(&command);
                let ctx = ui.ctx().clone();
                let (rain_track, plugin_id) = self
                    .document
                    .project()
                    .tracks()
                    .iter()
                    .enumerate()
                    .find_map(|(index, track)| match track {
                        Track::Midi(midi) => midi.plugin_instrument().map(|p| (index, p.id)),
                        _ => None,
                    })
                    .unwrap_or((0, 0));
                let editor = devices::vst3_editor_viewport_id(plugin_id);
                for line in text.lines() {
                    let words: Vec<&str> = line.split_whitespace().collect();
                    match words.as_slice() {
                        ["select"] => self.apply(
                            &ctx,
                            crate::actions::Action::SelectTracks(std::collections::BTreeSet::from(
                                [rain_track],
                            )),
                        ),
                        ["play"] => self.apply(&ctx, crate::actions::Action::TogglePlay),
                        ["select-none"] => self.apply(
                            &ctx,
                            crate::actions::Action::SelectTracks(std::collections::BTreeSet::new()),
                        ),
                        ["editor-off"] => {
                            self.show_rack = true;
                            ctx.data_mut(|data| {
                                data.insert_temp(egui::Id::new("rain-shot-editor-inline"), false)
                            });
                        }
                        ["editor"] => {
                            self.vst3_editors.insert(plugin_id);
                        }
                        ["editor-inline"] => {
                            self.show_rack = false;
                            ctx.data_mut(|data| {
                                data.insert_temp(egui::Id::new("rain-shot-editor-inline"), true)
                            });
                        }
                        ["shot-editor"] => ctx.send_viewport_cmd_to(
                            editor,
                            ViewportCommand::Screenshot(egui::UserData::default()),
                        ),
                        ["param", parameter, value] => {
                            let parameter: u32 = parameter.parse().unwrap_or(0);
                            let physical: f64 = value.parse().unwrap_or(0.0);
                            let specs =
                                sympoie::builtin::parameters(sympoie::builtin::BuiltinKind::Rain);
                            let spec = specs.iter().find(|spec| spec.id == parameter);
                            let id = Some(plugin_id);
                            if let (Some(spec), Some(id)) = (spec, id) {
                                self.apply(
                                    &ctx,
                                    crate::actions::Action::SetVst3Parameter {
                                        id,
                                        parameter,
                                        value: spec.normalize(physical),
                                    },
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            let plugin_id = self
                .document
                .project()
                .tracks()
                .iter()
                .find_map(|track| match track {
                    Track::Midi(midi) => midi.plugin_instrument().map(|p| p.id),
                    _ => None,
                })
                .unwrap_or(0);
            let editor = devices::vst3_editor_viewport_id(plugin_id);
            ui.ctx().input_for(editor, |input| {
                for event in &input.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        let stamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let [w, h] = image.size;
                        let path = dir.join(format!("editor-{stamp}-{w}x{h}.rgba"));
                        let _ = std::fs::write(path, image.as_raw());
                    }
                }
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            let inline = ui.ctx().data_mut(|data| {
                data.get_temp::<bool>(egui::Id::new("rain-shot-editor-inline"))
                    .unwrap_or(false)
            });
            if inline {
                let ctx = ui.ctx().clone();
                let project = self.document.project();
                let _ = std::fs::write(
                    dir.join("state.txt"),
                    format!(
                        "plugin_id={plugin_id} found={} tracks={} selection={:?}\n",
                        project.plugin(plugin_id).is_some(),
                        project.tracks().len(),
                        self.selection.tracks
                    ),
                );
                if let Some(plugin) = project.plugin(plugin_id) {
                    let telemetry = self.plugins.builtin_telemetry(plugin_id);
                    let mut actions = Vec::new();
                    let kind = sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)
                        .unwrap_or(sympoie::builtin::BuiltinKind::Rain);
                    let [width, height] = crate::views::builtin_devices::editor_size(kind);
                    egui::Window::new("Editor (inline for review)")
                        .fixed_pos(egui::pos2(20.0, 160.0))
                        .fixed_size(egui::vec2(width, height))
                        .show(&ctx, |ui| {
                            crate::views::builtin_devices::show(
                                ui,
                                plugin,
                                kind,
                                telemetry.as_deref(),
                                &mut actions,
                            );
                        });
                }
            }
        }
        self.frame(ui);
    }
}
