use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Id, RichText, Ui, vec2};
use sympoie::audio_file::{BitDepth, Encoding, FileFormat, MP3_BITRATES, SAMPLE_RATES};
use sympoie::clip_export::{audio_clip_project, write_midi_clip};

use super::{SympoieApp, file_name, has_extension};
use crate::actions::Action;
use crate::audio::export::{ExportJob, ExportTask};
use crate::document::edit::{self, ClipRef};
use crate::export_config::{
    ExportConfig, ExportContext, ExportDepth, ExportFormat, ExportRange, ExportSource,
    plan_exports, sample_rate_label,
};
use crate::ui::{theme, widgets};

const LABEL_WIDTH: f32 = 96.0;
const FIELD_WIDTH: f32 = 300.0;
const CARD_SIZE: f32 = 132.0;
const CARD_GAP: f32 = 10.0;
const SWITCH_SIZE: f32 = 20.0;
const WINDOW_WIDTH: f32 = CARD_SIZE * 4.0 + CARD_GAP * 3.0;

/// The Export window: output configurations shown as cards, all rendered at
/// once. Clicking a card opens its settings.
#[derive(Default)]
pub struct ExportWindow {
    pub open: bool,
    pub configs: Vec<ExportConfig>,
    /// The configuration whose settings are open.
    pub editing: Option<usize>,
    loaded: bool,
}

impl SympoieApp {
    pub(super) fn open_export_window(&mut self) {
        self.release_preview_before_dialog();
        let window = &mut self.export_window;
        if !window.loaded {
            window.configs = self.settings.export.last.clone();
            if window.configs.is_empty() {
                window.configs.push(ExportConfig::default());
            }
            window.editing = None;
            window.loaded = true;
        }
        window.open = true;
    }

    fn project_stem(&self) -> String {
        self.document
            .display_name()
            .trim_end_matches(".symp")
            .to_owned()
    }

    /// Where configurations without a folder of their own export to: beside
    /// the project once it has been saved.
    fn default_export_folder(&self) -> Option<PathBuf> {
        let path = self.document.path()?;
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_owned)
    }

    pub(super) fn export_all(&mut self) {
        self.release_preview_before_dialog();
        if self.export.is_some() || self.bounce.is_some() {
            self.warn("Wait for the current bounce or export, or cancel it first.".into());
            return;
        }
        let configs = self.export_window.configs.clone();
        let mut folder = self.default_export_folder();
        if folder.is_none()
            && configs
                .iter()
                .any(|config| config.enabled && config.folder.is_none())
        {
            folder = rfd::FileDialog::new()
                .set_title("Choose Export Folder")
                .pick_folder();
            if folder.is_none() {
                return;
            }
        }
        if let Err(error) = self.flush_plugin_edits() {
            self.error(format!("Couldn't capture plugin edits for export: {error}"));
            return;
        }
        let project = match self.project_with_plugin_state() {
            Ok(project) => project,
            Err(error) => {
                self.error(format!("Couldn't capture plugin state for export: {error}"));
                return;
            }
        };
        let name = self.project_stem();
        let context = ExportContext {
            project: &project,
            project_name: &name,
            default_folder: folder.as_deref(),
            loop_range: self
                .loop_region
                .enabled
                .then_some((self.loop_region.start, self.loop_region.end)),
            selection: self
                .selection
                .range
                .as_ref()
                .map(|range| (range.start, range.end)),
        };
        let tasks = match plan_exports(&context, &configs) {
            Ok(tasks) => tasks,
            Err(error) => {
                self.error(format!("Couldn't export: {error}"));
                return;
            }
        };
        self.settings.export.last = configs;
        self.save_settings();
        match ExportJob::start(tasks, &self.plugins) {
            Ok(job) => self.export = Some(job),
            Err(error) => self.error(format!("Couldn't prepare plugins for export: {error}")),
        }
    }

    pub(super) fn choose_clip_midi_export(&mut self, clip: ClipRef) {
        self.release_preview_before_dialog();
        let name = self.clip_file_stem(clip);
        let Some(mut path) = rfd::FileDialog::new()
            .set_title("Export MIDI File")
            .add_filter("Standard MIDI file", &["mid", "midi"])
            .set_file_name(format!("{name}.mid"))
            .save_file()
        else {
            return;
        };
        if !has_extension(&path, "mid") && !has_extension(&path, "midi") {
            path.set_extension("mid");
        }
        self.export_clip_midi(clip, &path);
    }

    fn clip_file_stem(&self, clip: ClipRef) -> String {
        let project = self.document.project();
        let name = edit::clip_name(project, clip)
            .map(str::to_owned)
            .or_else(|| {
                project
                    .tracks()
                    .get(clip.track)
                    .map(|track| track.name().to_owned())
            })
            .unwrap_or_default();
        crate::export_config::file_name(&name, "", None, "")
    }

    pub(super) fn export_clip_midi(&mut self, clip: ClipRef, path: &Path) {
        let project = self.document.project();
        let Some(midi) = edit::midi_clip(project, clip) else {
            self.warn("The MIDI clip no longer exists.".into());
            return;
        };
        let name = midi
            .name
            .clone()
            .unwrap_or_else(|| project.tracks()[clip.track].name().to_owned());
        let mut bytes = Vec::new();
        let written = write_midi_clip(
            midi,
            &name,
            project.tempo(),
            project.time_signature(),
            &mut bytes,
        )
        .and_then(|()| std::fs::write(path, &bytes));
        match written {
            Ok(()) => self.info(format!("Exported {}", file_name(path))),
            Err(error) => self.error(format!("Couldn't export {}: {error}", path.display())),
        }
    }

    pub(super) fn choose_clip_audio_export(&mut self, clip: ClipRef) {
        self.release_preview_before_dialog();
        let name = self.clip_file_stem(clip);
        let Some(mut path) = rfd::FileDialog::new()
            .set_title("Export Audio File")
            .add_filter("WAV audio (32-bit float)", &["wav"])
            .add_filter("AIFF audio (24-bit)", &["aiff", "aif"])
            .add_filter("FLAC audio (24-bit)", &["flac"])
            .add_filter("MP3 audio (320 kbps)", &["mp3"])
            .set_file_name(format!("{name}.wav"))
            .save_file()
        else {
            return;
        };
        if !["wav", "aiff", "aif", "flac", "mp3"]
            .iter()
            .any(|extension| has_extension(&path, extension))
        {
            path.set_extension("wav");
        }
        self.export_clip_audio(clip, path);
    }

    /// Writes the clip as it plays: WAV files keep the engine's 32-bit float
    /// samples, AIFF and FLAC files are 24-bit, MP3 files 320 kbps.
    pub(super) fn export_clip_audio(&mut self, clip: ClipRef, path: PathBuf) {
        if self.export.is_some() || self.bounce.is_some() {
            self.warn("Wait for the current bounce or export, or cancel it first.".into());
            return;
        }
        let (project, frames) =
            match audio_clip_project(self.document.project(), clip.track, clip.index) {
                Ok(isolated) => isolated,
                Err(error) => {
                    self.error(format!("Couldn't export the clip: {error}"));
                    return;
                }
            };
        let (format, depth) = if has_extension(&path, "wav") {
            (FileFormat::Wav, BitDepth::Float32)
        } else if has_extension(&path, "flac") {
            (FileFormat::Flac, BitDepth::Int24)
        } else if has_extension(&path, "mp3") {
            (FileFormat::Mp3, BitDepth::Int24)
        } else {
            (FileFormat::Aiff, BitDepth::Int24)
        };
        let encoding = Encoding {
            format,
            depth,
            ..Encoding::default()
        };
        match ExportJob::start(
            vec![ExportTask::Audio {
                project,
                path,
                start_frame: 0,
                frames,
                encoding,
            }],
            &self.plugins,
        ) {
            Ok(job) => self.export = Some(job),
            Err(error) => self.error(format!("Couldn't prepare plugins for export: {error}")),
        }
    }

    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export_window.open {
            return;
        }
        let mut open = true;
        let mut command = None;
        let busy = self.export.is_some() || self.bounce.is_some();
        let default_folder = self.default_export_folder();
        let presets = self.settings.export.presets.clone();
        egui::Window::new("Export")
            .id(Id::new("export-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(WINDOW_WIDTH)
            .show(ctx, |ui| {
                ui.set_width(WINDOW_WIDTH);
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                let window = &mut self.export_window;
                if window
                    .editing
                    .is_some_and(|index| index >= window.configs.len())
                {
                    window.editing = None;
                }
                match window.editing {
                    Some(index) => {
                        command = config_settings(
                            ui,
                            &mut window.configs[index],
                            default_folder.as_deref(),
                        )
                        .map(|action| (index, action));
                    }
                    None => config_cards(ui, window),
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(job) = &self.export {
                        ui.add(
                            egui::ProgressBar::new(job.fraction())
                                .desired_width(200.0)
                                .show_percentage(),
                        );
                        if ui.button("Cancel").clicked() {
                            job.cancel();
                        }
                    } else {
                        let on = window
                            .configs
                            .iter()
                            .filter(|config| config.enabled)
                            .count();
                        ui.label(
                            RichText::new(format!("{on} of {} will render", window.configs.len()))
                                .color(theme::TEXT_DIM),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let render =
                            egui::Button::new(RichText::new("Render").color(Color32::BLACK))
                                .fill(theme::ACCENT);
                        let any = window.configs.iter().any(|config| config.enabled);
                        if ui
                            .add_enabled(any && !busy, render)
                            .on_hover_text("Render every configuration that is on")
                            .on_disabled_hover_text(if busy {
                                "An export or bounce is running"
                            } else {
                                "Turn on a configuration to render it"
                            })
                            .clicked()
                        {
                            command = Some((usize::MAX, CardAction::Render));
                        }
                        ui.menu_button("+ Config", |ui| {
                            if ui
                                .button("Default")
                                .on_hover_text(ExportConfig::default().summary())
                                .clicked()
                            {
                                window.configs.push(ExportConfig::default());
                                ui.close();
                            }
                            if !presets.is_empty() {
                                ui.separator();
                            }
                            for (index, preset) in presets.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if ui
                                        .button(&preset.name)
                                        .on_hover_text(preset.config.summary())
                                        .clicked()
                                    {
                                        window.configs.push(preset.config.clone());
                                        ui.close();
                                    }
                                    if ui
                                        .small_button("×")
                                        .on_hover_text(format!("Delete the preset {}", preset.name))
                                        .clicked()
                                    {
                                        command = Some((index, CardAction::DeletePreset));
                                    }
                                });
                            }
                        });
                    });
                });
            });
        self.export_window.open = open;
        let Some((index, action)) = command else {
            return;
        };
        let window = &mut self.export_window;
        match action {
            CardAction::Back => window.editing = None,
            CardAction::Duplicate => {
                let mut copy = window.configs[index].clone();
                copy.name = format!("{} copy", copy.name);
                window.configs.insert(index + 1, copy);
                window.editing = Some(index + 1);
            }
            CardAction::Remove => {
                window.configs.remove(index);
                window.editing = None;
            }
            CardAction::SavePreset => {
                let config = window.configs[index].clone();
                self.settings.export.save_preset(&config);
                self.save_settings();
                self.info(format!("Saved the export preset “{}”", config.name.trim()));
            }
            CardAction::DeletePreset => {
                if index < self.settings.export.presets.len() {
                    self.settings.export.presets.remove(index);
                    self.save_settings();
                }
            }
            CardAction::Render => self.apply(ctx, Action::ExportAll),
        }
    }
}

#[derive(Clone, Copy)]
enum CardAction {
    Back,
    Duplicate,
    Remove,
    SavePreset,
    DeletePreset,
    Render,
}

/// The configurations as cards; clicking one opens its settings.
fn config_cards(ui: &mut Ui, window: &mut ExportWindow) {
    egui::ScrollArea::vertical()
        .max_height(CARD_SIZE * 2.0 + 24.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if window.configs.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(
                            "Add a configuration with + Config to choose what to render.",
                        )
                        .color(theme::TEXT_DIM),
                    );
                });
                ui.add_space(24.0);
                return;
            }
            let mut removed = None;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(CARD_GAP, CARD_GAP);
                for (index, config) in window.configs.iter_mut().enumerate() {
                    match config_card(ui, index, config) {
                        Some(CardClick::Open) => window.editing = Some(index),
                        Some(CardClick::Remove) => removed = Some(index),
                        None => {}
                    }
                }
            });
            if let Some(index) = removed {
                window.configs.remove(index);
            }
        });
}

/// Returns whether the card was clicked.
enum CardClick {
    Open,
    Remove,
}

fn config_card(ui: &mut Ui, index: usize, config: &mut ExportConfig) -> Option<CardClick> {
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_SIZE, CARD_SIZE), egui::Sense::click());
    let name = config.name.clone();
    let response = ui
        .interact(rect, Id::new(("export-card", index)), egui::Sense::click())
        .union(response)
        .on_hover_text(format!("{}\nClick to edit", config.summary()));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let fill = if response.hovered() {
            theme::tint(theme::LANE, theme::ACCENT, 0.12)
        } else {
            theme::LANE
        };
        painter.rect_filled(rect, 6.0, fill);
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(
                1.0,
                if config.enabled {
                    theme::ACCENT.gamma_multiply(0.7)
                } else {
                    Color32::from_white_alpha(30)
                },
            ),
            egui::StrokeKind::Inside,
        );
        let inner = rect.shrink(10.0);
        let dim = if config.enabled { 1.0 } else { 0.5 };
        let text = |left: f32, top: f32, text: &str, size: f32, color: Color32| {
            // The top row shares the card with the switch and the remove button.
            let right = if top == 0.0 { SWITCH_SIZE } else { 0.0 };
            widgets::fitted_text(
                &painter,
                egui::Rect::from_min_max(
                    egui::pos2(inner.left() + left, inner.top() + top),
                    egui::pos2(inner.right() - right, inner.top() + top + size + 6.0),
                ),
                text,
                theme::prop(size),
                color.gamma_multiply(dim),
                egui::Align2::LEFT_CENTER,
            );
        };
        // Name beside the switch, then the format, then what it renders.
        text(SWITCH_SIZE + 2.0, 0.0, &config.name, 12.5, theme::TEXT);
        let format = if config.source == ExportSource::StemsProject {
            "SYMP".to_owned()
        } else {
            config.format.file_format().label().to_owned()
        };
        text(0.0, 28.0, &format, 22.0, theme::ACCENT);
        let quality = format!(
            "{} · {}",
            config.quality_label(),
            sample_rate_label(config.rate())
        );
        text(0.0, 58.0, &quality, 11.5, theme::TEXT);
        text(0.0, 82.0, config.source.label(), 11.0, theme::TEXT_DIM);
        let range = if config.source == ExportSource::StemsProject {
            ExportRange::Project.label()
        } else {
            config.range.label()
        };
        text(0.0, 97.0, range, 11.0, theme::TEXT_DIM);
    }
    // The switch sits on the card in a child ui, so it neither opens the card
    // nor moves the next card along.
    let switch =
        egui::Rect::from_min_size(rect.min + vec2(6.0, 7.0), vec2(SWITCH_SIZE, SWITCH_SIZE));
    widgets::inside(
        ui,
        switch,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(egui::Checkbox::without_text(&mut config.enabled))
                .on_hover_text("Render this configuration");
        },
    );
    let close = egui::Rect::from_min_size(
        egui::pos2(rect.right() - SWITCH_SIZE - 6.0, rect.top() + 7.0),
        vec2(SWITCH_SIZE, SWITCH_SIZE),
    );
    let remove = widgets::inside(
        ui,
        close,
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.add_sized(
                [SWITCH_SIZE, SWITCH_SIZE],
                egui::Button::new(RichText::new("×").size(15.0)).frame(false),
            )
            .on_hover_text(format!("Remove {}", config.name))
            .clicked()
        },
    );
    if remove {
        Some(CardClick::Remove)
    } else if response.clicked() {
        Some(CardClick::Open)
    } else {
        None
    }
}

/// The settings of one configuration, with what can be done to it.
fn config_settings(
    ui: &mut Ui,
    config: &mut ExportConfig,
    default_folder: Option<&Path>,
) -> Option<CardAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button("‹ Configurations").clicked() {
            action = Some(CardAction::Back);
        }
        ui.label(RichText::new(&config.name).size(15.0).color(theme::TEXT));
    });
    ui.separator();
    configuration_editor(ui, config, default_folder);
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Save as Preset").clicked() {
            action = Some(CardAction::SavePreset);
        }
        if ui.button("Duplicate").clicked() {
            action = Some(CardAction::Duplicate);
        }
        if ui.button("Remove").clicked() {
            action = Some(CardAction::Remove);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Done").clicked() {
                action = Some(CardAction::Back);
            }
        });
    });
    action
}

fn row(ui: &mut Ui, label: &str, contents: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [LABEL_WIDTH, 20.0],
            egui::Label::new(RichText::new(label).color(theme::TEXT_DIM)),
        );
        contents(ui);
    });
}

fn combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    choices: impl IntoIterator<Item = (T, String)>,
    enabled: bool,
) {
    let choices: Vec<_> = choices.into_iter().collect();
    let selected = choices
        .iter()
        .find(|(choice, _)| choice == value)
        .map_or_else(String::new, |(_, label)| label.clone());
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(id)
            .width(FIELD_WIDTH - 8.0)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (choice, label) in choices {
                    ui.selectable_value(value, choice, label);
                }
            });
    });
}

fn configuration_editor(ui: &mut Ui, config: &mut ExportConfig, default_folder: Option<&Path>) {
    let audio = config.source != ExportSource::StemsProject;
    row(ui, "Name", |ui| {
        ui.add(egui::TextEdit::singleline(&mut config.name).desired_width(FIELD_WIDTH - 8.0));
    });
    let previous_source = config.source;
    row(ui, "Render", |ui| {
        combo(
            ui,
            "export-source",
            &mut config.source,
            ExportSource::ALL.map(|source| (source, source.label().to_owned())),
            true,
        );
    });
    if config.source != previous_source && config.file_name == previous_source.default_file_name() {
        config.file_name = config.source.default_file_name().to_owned();
    }
    row(ui, "Range", |ui| {
        combo(
            ui,
            "export-range",
            &mut config.range,
            ExportRange::ALL.map(|range| (range, range.label().to_owned())),
            audio,
        );
    });
    row(ui, "Format", |ui| {
        combo(
            ui,
            "export-format",
            &mut config.format,
            ExportFormat::ALL.map(|format| (format, format.file_format().label().to_owned())),
            audio,
        );
    });
    let format = config.format.file_format();
    if format.has_bit_depth() {
        row(ui, "Bit depth", |ui| {
            combo(
                ui,
                "export-depth",
                &mut config.depth,
                ExportDepth::ALL
                    .into_iter()
                    .filter(|depth| format.supports(depth.bit_depth()))
                    .map(|depth| (depth, depth.bit_depth().label().to_owned())),
                audio,
            );
        });
    } else {
        row(ui, "Bitrate", |ui| {
            combo(
                ui,
                "export-bitrate",
                &mut config.mp3_bitrate,
                MP3_BITRATES.map(|kbps| (kbps, format!("{kbps} kbps (constant)"))),
                audio,
            );
        });
    }
    if !format.supports(config.depth.bit_depth()) {
        config.depth = ExportDepth::Int24;
    }
    if !format.supports_rate(config.sample_rate) {
        config.sample_rate = sympoie::SAMPLE_RATE;
    }
    row(ui, "Sample rate", |ui| {
        combo(
            ui,
            "export-rate",
            &mut config.sample_rate,
            SAMPLE_RATES
                .into_iter()
                .filter(|rate| format.supports_rate(*rate))
                .map(|rate| (rate, sample_rate_label(rate))),
            audio,
        );
    });
    row(ui, "Dither", |ui| {
        ui.add_enabled(
            audio && format.has_bit_depth() && config.depth != ExportDepth::Float32,
            egui::Checkbox::new(&mut config.dither, "Triangular dither"),
        );
    });
    row(ui, "File name", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut config.file_name)
                .desired_width(FIELD_WIDTH - 8.0)
                .hint_text(config.source.default_file_name()),
        )
        .on_hover_text("{project}, {track} and {name} are replaced by the project, track and configuration names");
    });
    row(ui, "Folder", |ui| {
        let shown = match (&config.folder, default_folder) {
            (Some(folder), _) => folder.display().to_string(),
            (None, Some(folder)) => format!("Beside the project ({})", folder.display()),
            (None, None) => "Asked for on export".to_owned(),
        };
        ui.add(egui::Label::new(RichText::new(shown).color(theme::TEXT)).truncate())
            .on_hover_text("Where the files are written");
    });
    row(ui, "", |ui| {
        if ui.button("Choose…").clicked()
            && let Some(folder) = rfd::FileDialog::new()
                .set_title("Choose Export Folder")
                .pick_folder()
        {
            config.folder = Some(folder);
        }
        if ui
            .add_enabled(
                config.folder.is_some(),
                egui::Button::new("Beside the Project"),
            )
            .clicked()
        {
            config.folder = None;
        }
    });
}
