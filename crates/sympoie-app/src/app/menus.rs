use eframe::egui::{
    self, Align, Key, KeyboardShortcut, Layout, Modifiers, RichText, Sense, Stroke, Ui, pos2, vec2,
};
use sympoie::SAMPLE_RATE;
use sympoie::plugin::PluginKind;
use sympoie::track::Track;
use sympoie::transport::{PlaybackState, TransportCommand};

use super::{DetailView, NoticeKind, SympoieApp, file_name};
use crate::actions::Action;
use crate::document::edit::TrackKind;
use crate::state::{self, GridMode};
use crate::ui::{format, theme, widgets};
use crate::views::arrangement::grid_menu;

impl SympoieApp {
    pub(super) fn menu_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let shortcut = |ui: &Ui, modifiers: Modifiers, key: Key| {
            ui.ctx()
                .format_shortcut(&KeyboardShortcut::new(modifiers, key))
        };
        let command = Modifiers::COMMAND;
        let command_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let command_alt = Modifiers::COMMAND | Modifiers::ALT;
        let item = |ui: &mut Ui, label: &str, keys: String, enabled: bool| {
            ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(keys))
                .clicked()
        };
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if item(ui, "New Project", shortcut(ui, command, Key::N), true) {
                    actions.push(Action::NewProject);
                }
                if item(ui, "Open…", shortcut(ui, command, Key::O), true) {
                    actions.push(Action::OpenProject);
                }
                ui.separator();
                if item(ui, "Open Project Folder…", String::new(), true) {
                    actions.push(Action::OpenProjectFolder);
                }
                if item(ui, "Save", shortcut(ui, command, Key::S), true) {
                    actions.push(Action::Save);
                }
                if item(ui, "Save As…", shortcut(ui, command_shift, Key::S), true) {
                    actions.push(Action::SaveAs);
                }
                ui.separator();
                if item(ui, "Import Audio…", shortcut(ui, command, Key::I), true) {
                    actions.push(Action::ChooseAudioToImport {
                        track: None,
                        start: None,
                    });
                }
                if item(ui, "Export…", shortcut(ui, command_shift, Key::R), true) {
                    actions.push(Action::ShowExport);
                }
                ui.separator();
                if item(ui, "Quit", shortcut(ui, command, Key::Q), true) {
                    actions.push(Action::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                let undo = self
                    .document
                    .undo_label()
                    .map_or_else(|| "Undo".to_owned(), |label| format!("Undo {label}"));
                if item(
                    ui,
                    &undo,
                    shortcut(ui, command, Key::Z),
                    self.document.undo_label().is_some(),
                ) {
                    actions.push(Action::Undo);
                }
                let redo = self
                    .document
                    .redo_label()
                    .map_or_else(|| "Redo".to_owned(), |label| format!("Redo {label}"));
                if item(
                    ui,
                    &redo,
                    shortcut(ui, command_shift, Key::Z),
                    self.document.redo_label().is_some(),
                ) {
                    actions.push(Action::Redo);
                }
                ui.separator();
                let clip = self.selection.clip;
                let range = self.selection.range.is_some();
                if item(
                    ui,
                    "Copy Clip",
                    shortcut(ui, command, Key::C),
                    clip.is_some() && !self.piano_roll.focused,
                ) && let Some(clip) = clip
                {
                    actions.push(Action::CopyClip(clip));
                }
                let can_paste = !self.piano_roll.focused
                    && self.clip_clipboard.as_ref().is_some_and(|clipboard| {
                        self.selection
                            .tracks
                            .first()
                            .and_then(|track| self.document.project().tracks().get(*track))
                            .is_some_and(|track| clipboard.content.is_compatible(track))
                    });
                if item(ui, "Paste Clip", shortcut(ui, command, Key::V), can_paste) {
                    actions.push(Action::PasteClip);
                }
                if item(
                    ui,
                    "Split",
                    shortcut(ui, command, Key::E),
                    range || clip.is_some(),
                ) {
                    if range {
                        actions.push(Action::SplitTimeSelection);
                    } else if let Some(clip) = clip {
                        actions.push(Action::SplitClip(clip));
                    }
                }
                if item(
                    ui,
                    "Duplicate",
                    shortcut(ui, command, Key::D),
                    clip.is_some(),
                ) && let Some(clip) = clip
                {
                    actions.push(Action::DuplicateClip(clip));
                }
                if item(
                    ui,
                    "Reverse",
                    "R".to_owned(),
                    !self.piano_roll.focused
                        && !self.arrangement.automation
                        && self.selection.has_audio(self.document.project()),
                ) {
                    actions.push(Action::ReverseSelection);
                }
                if item(
                    ui,
                    "Loop Selection",
                    shortcut(ui, Modifiers::CTRL, Key::L),
                    range || clip.is_some(),
                ) {
                    actions.push(Action::LoopSelection);
                }
                if item(ui, "Delete", "Del".to_owned(), range || clip.is_some()) {
                    if range {
                        actions.push(Action::DeleteTimeSelection);
                    } else if let Some(clip) = clip {
                        actions.push(Action::DeleteClip(clip));
                    }
                }
                ui.separator();
                let selected = !self.selection.tracks.is_empty();
                if item(ui, "Group Tracks", shortcut(ui, command, Key::G), selected) {
                    actions.push(Action::GroupSelected);
                }
                let group = self.selection.tracks.first().copied().filter(|index| {
                    matches!(
                        self.document.project().tracks().get(*index),
                        Some(Track::Group(_))
                    )
                });
                if item(ui, "Ungroup Tracks", String::new(), group.is_some())
                    && let Some(group) = group
                {
                    actions.push(Action::Ungroup(group));
                }
                if item(ui, "Delete Track", String::new(), selected)
                    && let Some(index) = self.selection.tracks.first()
                {
                    actions.push(Action::DeleteTrack(*index));
                }
            });
            ui.menu_button("Create", |ui| {
                if item(
                    ui,
                    "Insert Audio Track",
                    shortcut(ui, command, Key::T),
                    true,
                ) {
                    actions.push(Action::AddTrack(TrackKind::Audio));
                }
                if item(
                    ui,
                    "Insert MIDI Track",
                    shortcut(ui, command_shift, Key::T),
                    true,
                ) {
                    actions.push(Action::AddTrack(TrackKind::Midi));
                }
                if item(
                    ui,
                    "Insert Group Track",
                    shortcut(ui, command_shift, Key::G),
                    true,
                ) {
                    actions.push(Action::AddTrack(TrackKind::Group));
                }
                if item(ui, "Insert Bus Track", String::new(), true) {
                    actions.push(Action::AddBusTrack);
                }
                ui.separator();
                let track = (self.selection.tracks.len() == 1)
                    .then(|| self.selection.tracks.first().copied())
                    .flatten();
                let midi = track.filter(|index| {
                    matches!(
                        self.document.project().tracks().get(*index),
                        Some(Track::Midi(_))
                    )
                });
                let can_insert = self.selection.range.is_some() || midi.is_some();
                if item(
                    ui,
                    "Insert MIDI Clip",
                    shortcut(ui, command, Key::M),
                    can_insert,
                ) {
                    actions.push(Action::InsertMidiClipInSelection);
                }
                ui.separator();
                for kind in sympoie::builtin::BuiltinKind::ALL {
                    let instrument =
                        kind.descriptor().kind == sympoie::plugin::PluginKind::Instrument;
                    if item(
                        ui,
                        &format!("Add {}", kind.name()),
                        String::new(),
                        !instrument || midi.is_some(),
                    ) {
                        let track = if instrument {
                            midi.unwrap_or(0)
                        } else {
                            track.unwrap_or(0)
                        };
                        if !instrument && self.detail_view == DetailView::Mixer {
                            actions.push(Action::LoadMixerVst3 {
                                track,
                                plugin: kind.descriptor(),
                                before: None,
                            });
                        } else {
                            actions.push(Action::LoadVst3 {
                                track,
                                plugin: kind.descriptor(),
                            });
                        }
                    }
                }
                if item(ui, "Add VST3 Instrument…", String::new(), midi.is_some())
                    && let Some(track) = midi
                {
                    actions.push(Action::BrowsePlugins {
                        track,
                        kind: PluginKind::Instrument,
                    });
                }
                if item(ui, "Add VST3 Effect…", String::new(), true) {
                    actions.push(Action::BrowsePlugins {
                        track: track.unwrap_or(0),
                        kind: PluginKind::Effect,
                    });
                }
            });
            ui.menu_button("View", |ui| {
                if item(
                    ui,
                    if self.arrangement.automation {
                        "Hide Automation"
                    } else {
                        "Show Automation"
                    },
                    "A".to_owned(),
                    true,
                ) {
                    actions.push(Action::ToggleAutomation);
                }
                ui.separator();
                let browser = if self.show_browser {
                    "Hide Library"
                } else {
                    "Show Library"
                };
                if item(ui, browser, shortcut(ui, command_alt, Key::B), true) {
                    actions.push(Action::ToggleBrowser);
                }
                let rack = if self.show_rack {
                    "Hide Rack"
                } else {
                    "Show Rack"
                };
                if item(ui, rack, shortcut(ui, command_alt, Key::R), true) {
                    actions.push(Action::ToggleRack);
                }
                ui.separator();
                let piano = self.piano_roll.clip.is_some();
                if item(ui, "Clip Editor", String::new(), true) {
                    actions.push(Action::ShowClipView);
                }
                if item(ui, "Close Drawer", String::new(), true) {
                    actions.push(Action::CloseDrawer);
                }
                let mixer = if !piano && self.detail_view == DetailView::Mixer {
                    "Hide Mixer"
                } else {
                    "Show Mixer"
                };
                if item(ui, mixer, shortcut(ui, command_alt, Key::M), true) {
                    actions.push(Action::ToggleMixer);
                }
                let detail = if !piano && self.detail_view == DetailView::Hidden {
                    "Show Drawer"
                } else {
                    "Hide Drawer"
                };
                if item(ui, detail, shortcut(ui, command_alt, Key::L), true) {
                    actions.push(Action::ToggleDetailView);
                }
                ui.separator();
                if item(ui, "Zoom In", "+".to_owned(), true) {
                    actions.push(Action::Zoom(1.4));
                }
                if item(ui, "Zoom Out", "-".to_owned(), true) {
                    actions.push(Action::Zoom(1.0 / 1.4));
                }
                if item(ui, "Zoom to Fit", "Z".to_owned(), true) {
                    actions.push(Action::ZoomToFit);
                }
                ui.separator();
                if item(ui, "Narrow Grid", shortcut(ui, command, Key::Num1), true) {
                    actions.push(Action::StepGrid { narrower: true });
                }
                if item(ui, "Widen Grid", shortcut(ui, command, Key::Num2), true) {
                    actions.push(Action::StepGrid { narrower: false });
                }
                ui.menu_button("Grid", |ui| {
                    grid_menu(ui, self.arrangement.grid, self.document.project(), actions);
                });
                ui.separator();
                ui.label(RichText::new("Track height").color(theme::TEXT_DIM));
                for (label, height) in [("Compact", 46.0), ("Normal", 68.0), ("Large", 104.0)] {
                    if ui
                        .radio(
                            self.arrangement.row_height == height
                                && self.arrangement.track_heights.is_empty(),
                            label,
                        )
                        .clicked()
                    {
                        self.arrangement.set_all_track_heights(height);
                    }
                }
                if item(ui, "Reset All Track Heights", String::new(), true) {
                    actions.push(Action::ResetTrackHeights);
                }
            });
            ui.menu_button("Options", |ui| {
                let playing = self.status.state == PlaybackState::Playing;
                if item(
                    ui,
                    if playing { "Pause" } else { "Play" },
                    "Space".to_owned(),
                    true,
                ) {
                    actions.push(Action::TogglePlay);
                }
                if item(ui, "Stop", String::new(), true) {
                    actions.push(Action::Stop);
                }
                let record_label = if self.recording.is_some() {
                    "Stop Recording"
                } else {
                    "Record"
                };
                if item(ui, record_label, "Q".to_owned(), true) {
                    actions.push(Action::ToggleRecord);
                }
                if item(ui, "Return to Start", "Home".to_owned(), true) {
                    actions.push(Action::Transport(TransportCommand::ReturnToStart));
                }
                ui.separator();
                let loop_label = if self.loop_region.enabled {
                    "Loop Off"
                } else {
                    "Loop On"
                };
                if item(ui, loop_label, "L".to_owned(), true) {
                    actions.push(Action::ToggleLoop);
                }
                let follow = if self.arrangement.follow {
                    "Follow Off"
                } else {
                    "Follow On"
                };
                if item(ui, follow, "F".to_owned(), true) {
                    actions.push(Action::ToggleFollow);
                }
                let agent = if self.agent_mode() {
                    "Agent Mode Off"
                } else {
                    "Agent Mode On"
                };
                if item(ui, agent, String::new(), true) {
                    actions.push(Action::ToggleAgentMode);
                }
                ui.separator();
                if item(ui, "Manage VST3 Plug-Ins…", String::new(), true) {
                    actions.push(Action::BrowsePlugins {
                        track: self.selection.tracks.first().copied().unwrap_or(0),
                        kind: PluginKind::Effect,
                    });
                }
                ui.separator();
                if item(ui, "Settings…", shortcut(ui, command, Key::Comma), true) {
                    actions.push(Action::ShowSettings);
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard Shortcuts").clicked() {
                    actions.push(Action::ShowShortcuts);
                }
                if ui.button("About Sympoie").clicked() {
                    actions.push(Action::ShowAbout);
                }
            });
        });
    }

    pub(super) fn status_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let rail = ui.max_rect().expand2(egui::vec2(8.0, 2.0));
        crate::ui::analog::gradient_rect(ui.painter(), rail, theme::BG_RAISED, theme::BG_PANEL);
        ui.painter().hline(
            rail.x_range(),
            rail.top() + 0.5,
            egui::Stroke::new(1.0, theme::BORDER),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            self.detail_selector(ui, actions);
            ui.add_space(10.0);
            let project = self.document.project();
            let seconds = project.duration_frames().unwrap_or(0) as f64 / f64::from(SAMPLE_RATE);
            let file = self
                .document
                .path()
                .map_or_else(|| "Not saved".to_owned(), file_name);
            let summary = ui.label(
                RichText::new(format!(
                    "{file} · {} · {} Hz",
                    format::duration(seconds),
                    SAMPLE_RATE
                ))
                .size(10.5)
                .color(theme::TEXT_FAINT),
            );
            if let Some(path) = self.document.path() {
                summary.on_hover_text(path.display().to_string());
            }
            ui.add_space(10.0);
            self.grid_indicator(ui, actions);
            ui.add_space(8.0);
            self.agent_switch(ui, actions);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if let Some(pending) = &self.bounce {
                    ui.add(egui::ProgressBar::new(pending.job.fraction())
                        .desired_width(160.0).show_percentage());
                    ui.label(RichText::new(format!("Bouncing {}", pending.name)).color(theme::TEXT_DIM));
                    if ui.small_button("Cancel").clicked() {
                        pending.job.cancel();
                    }
                } else if let Some(job) = &self.export {
                    ui.add(
                        egui::ProgressBar::new(job.fraction())
                            .desired_width(160.0)
                            .show_percentage(),
                    );
                    ui.label(
                        RichText::new(format!("Exporting {}", job.description()))
                            .color(theme::TEXT_DIM),
                    );
                    if ui.small_button("Cancel").clicked() {
                        job.cancel();
                    }
                } else if let Some(take) = &self.recording {
                    let seconds = take.clock.elapsed_frames() as f64 / f64::from(SAMPLE_RATE);
                    ui.label(RichText::new(format!("● Recording {} · {} · Q to finish", format::duration(seconds), take.input.description())).color(theme::ERROR));
                } else if self.engine.is_preparing_pitch() {
                    ui.spinner();
                    ui.label(RichText::new("Preparing clip pitch…").color(theme::TEXT_DIM));
                } else if let Some(notice) = &self.notice {
                    let color = match notice.kind {
                        NoticeKind::Info => theme::TEXT,
                        NoticeKind::Warning => theme::WARN,
                        NoticeKind::Error => theme::ERROR,
                    };
                    ui.add(egui::Label::new(RichText::new(&notice.text).color(color)).truncate())
                        .on_hover_text(&notice.text);
                } else if let Some(reason) = &self.status.render_error {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("Playback unavailable: {reason}"))
                                .color(theme::WARN),
                        )
                        .truncate(),
                    );
                } else if let Some(status) = self.plugin_browser.scan_status() {
                    ui.spinner();
                    ui.add(
                        egui::Label::new(RichText::new(status).color(theme::TEXT_DIM)).truncate(),
                    );
                } else {
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                if self.arrangement.automation {
                                    "A hides automation · click to add points · drag to edit · Delete removes a point · Escape cancels · Alt ignores the grid"
                                } else {
                                    "Space plays · drag lanes or clip bodies to select time · ⌘M inserts a MIDI clip · right-click a lane for the grid · Alt ignores the grid"
                                },
                            )
                            .size(10.5)
                            .color(theme::TEXT_FAINT),
                        )
                        .truncate(),
                    );
                }
            });
        });
    }

    fn agent_switch(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let now = ui.input(|input| input.time);
        let accent = if self.agent_problem().is_some() {
            theme::WARN
        } else {
            theme::OK
        };
        let hover = if let Some(problem) = self.agent_problem() {
            format!(
                "Agent mode is on, but the project files have an error. Autosave resumes once it is fixed.\n\n{problem}"
            )
        } else if self.agent_mode() {
            "Agent mode is on: Sympoie saves the project folder as you work and loads changes a coding agent makes to its files. Click to turn it off.".to_owned()
        } else {
            "Turn on agent mode to work on this project with a coding agent: Sympoie keeps the project folder saved and loads the agent's changes as they happen.".to_owned()
        };
        if crate::ui::rack::led_switch(ui, "AGENT", self.agent_led(now), accent)
            .on_hover_text(hover)
            .clicked()
        {
            actions.push(Action::ToggleAgentMode);
        }
    }

    fn grid_indicator(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let project = self.document.project();
        let meter = project.time_signature();
        let label = match self.arrangement.grid(project).interval() {
            Some(interval) => state::interval_label(meter, interval),
            None => "Off".to_owned(),
        };
        let mode = match self.arrangement.grid {
            GridMode::Adaptive(density) => format!("Adaptive grid, {}", density.label()),
            GridMode::Fixed => "Fixed grid".to_owned(),
        };
        let button = ui.add(
            egui::Button::new(RichText::new(&label).size(10.5).color(theme::TEXT_DIM))
                .frame(false)
                .min_size(vec2(44.0, 16.0)),
        );
        button
            .on_hover_text(format!(
                "{mode}: {label}\nRight-click an empty part of a lane to change it. ⌘1 narrows, ⌘2 widens."
            ))
            .context_menu(|ui| grid_menu(ui, self.arrangement.grid, project, actions));
    }

    fn detail_selector(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let piano = self.piano_roll.clip.is_some();
        let view = self.detail_view;
        let size = vec2(44.0, 16.0);

        let hidden = !piano && view == DetailView::Hidden;
        let (rect, response) = ui.allocate_exact_size(vec2(18.0, 16.0), Sense::click());
        let color = if response.hovered() {
            egui::Color32::WHITE
        } else {
            theme::TEXT_DIM
        };
        let c = rect.center();
        let points = if hidden {
            vec![
                pos2(c.x - 4.0, c.y + 2.5),
                pos2(c.x + 4.0, c.y + 2.5),
                pos2(c.x, c.y - 3.0),
            ]
        } else {
            vec![
                pos2(c.x - 4.0, c.y - 2.5),
                pos2(c.x + 4.0, c.y - 2.5),
                pos2(c.x, c.y + 3.0),
            ]
        };
        ui.painter()
            .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        if response
            .on_hover_text(if hidden {
                "Open the bottom drawer"
            } else {
                "Close the bottom drawer"
            })
            .clicked()
        {
            actions.push(Action::ToggleDetailView);
        }

        let mixer = !piano && view == DetailView::Mixer;
        if widgets::toggle(ui, "Mixer", mixer, theme::ACCENT, size)
            .on_hover_text("Show the mixer")
            .clicked()
        {
            actions.push(Action::ShowMixer);
        }
        let device = !piano && view == DetailView::Devices;
        if widgets::toggle(ui, "Arrange", device, theme::ACCENT, size)
            .on_hover_text("Close the bottom drawer to expand the arrangement")
            .clicked()
        {
            actions.push(Action::CloseDrawer);
        }
        let clip = piano || view == DetailView::Clip;
        if widgets::toggle(ui, "Clip", clip, theme::ACCENT, size)
            .on_hover_text(
                "Edit the selected clip in the drawer. Double-click a clip to open it here.",
            )
            .clicked()
        {
            actions.push(Action::ShowClipView);
        }
        ui.add_space(8.0);
        if widgets::toggle(ui, "Library", self.show_browser, theme::ACCENT, size)
            .on_hover_text("Show or hide the device and sample library beside the rack")
            .clicked()
        {
            actions.push(Action::ToggleBrowser);
        }
        let rack = widgets::toggle(ui, "Rack", self.show_rack, theme::ACCENT, size)
            .on_hover_text("Show or hide the selected track's Rack");
        ui.interact(
            rack.rect,
            egui::Id::new("status-rack-toggle"),
            Sense::hover(),
        );
        if rack.clicked() {
            actions.push(Action::ToggleRack);
        }
    }
}
