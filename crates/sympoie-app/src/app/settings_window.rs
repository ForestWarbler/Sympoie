use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui::{self, Align, Color32, Id, Layout, RichText, Ui, vec2};

use super::SympoieApp;
use crate::audio::Output;
use crate::audio::output::{self, AudioCatalog, STANDARD_RATES};
use crate::settings::{self, AudioSettings, GeneralSettings, Settings};
use crate::ui::{rack, theme};

const BUFFER_SIZES: [u32; 8] = [32, 64, 128, 256, 512, 1024, 2048, 4096];
const LABEL_WIDTH: f32 = 128.0;
const FIELD_WIDTH: f32 = 290.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Audio,
    General,
}

pub struct SettingsWindow {
    tab: Tab,
    draft: AudioSettings,
    catalog: Option<AudioCatalog>,
    discovery: Option<Receiver<AudioCatalog>>,
}

impl SettingsWindow {
    pub fn new(settings: &Settings) -> Self {
        Self {
            tab: Tab::Audio,
            draft: settings.audio.clone(),
            catalog: None,
            discovery: None,
        }
    }

    fn discover(&mut self, enabled: bool) {
        if !enabled {
            self.catalog = Some(AudioCatalog::default());
            return;
        }
        if self.discovery.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("sympoie-audio-devices".into())
            .spawn(move || {
                let _ = sender.send(output::catalog());
            });
        if spawned.is_ok() {
            self.discovery = Some(receiver);
        } else {
            self.catalog = Some(AudioCatalog::default());
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.discovery else {
            return;
        };
        match receiver.try_recv() {
            Ok(catalog) => {
                self.catalog = Some(catalog);
                self.discovery = None;
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50))
            }
            Err(TryRecvError::Disconnected) => {
                self.catalog = Some(AudioCatalog::default());
                self.discovery = None;
            }
        }
    }
}

impl SympoieApp {
    pub(super) fn open_settings(&mut self) {
        self.show_settings = true;
        self.settings_window.draft = self.settings.audio.clone();
        if self.settings_window.catalog.is_none() {
            self.settings_window.discover(self.use_output_device);
        }
    }

    pub(super) fn save_settings(&mut self) {
        if let Some(path) = &self.settings_path
            && let Err(error) = settings::save(path, &self.settings)
        {
            self.warn(error);
        }
    }

    pub(super) fn apply_audio_settings(&mut self, audio: AudioSettings) {
        self.settings.audio = audio;
        self.output = Output::open(self.engine.shared(), false);
        self.output = Output::open_with(
            self.engine.shared(),
            self.use_output_device,
            &self.settings.audio,
        );
        self.save_settings();
        let info = self.output.info().clone();
        if info.is_device {
            self.info(format!(
                "Audio output: {} at {:.1} kHz",
                info.description,
                info.sample_rate as f32 / 1000.0
            ));
        } else {
            self.warn(format!("No audio output: {}", info.detail));
        }
    }

    fn set_general(&mut self, ctx: &egui::Context, general: GeneralSettings) {
        if general.interface_scale != self.settings.general.interface_scale {
            ctx.set_zoom_factor(general.interface_scale);
        }
        self.settings.general = general;
        self.settings = std::mem::take(&mut self.settings).sanitized();
        self.save_settings();
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        self.settings_window.poll(ctx);
        let mut open = true;
        egui::Window::new("Settings")
            .id(Id::new("settings-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.set_width(LABEL_WIDTH + FIELD_WIDTH + 34.0);
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                let tabs = [Tab::Audio, Tab::General];
                let current = tabs
                    .iter()
                    .position(|tab| *tab == self.settings_window.tab)
                    .unwrap_or(0);
                let chosen = rack::selector_bank(
                    ui,
                    &["Audio", "General"],
                    current,
                    |_| theme::ACCENT,
                    |index| Id::new(("settings-tab", index)),
                );
                self.settings_window.tab = tabs[chosen];
                ui.add_space(4.0);
                match self.settings_window.tab {
                    Tab::Audio => self.audio_tab(ui),
                    Tab::General => self.general_tab(ui),
                }
            });
        if !open {
            self.show_settings = false;
        }
    }

    fn audio_tab(&mut self, ui: &mut Ui) {
        section(ui, "Output");
        if !self.use_output_device {
            ui.label(
                RichText::new(
                    "Audio output is disabled for this session (SYMPOIE_NO_AUDIO). \
                     Playback runs on a silent clock.",
                )
                .color(theme::WARN),
            );
        }
        let discovering = self.settings_window.discovery.is_some();
        let catalog = self.settings_window.catalog.clone().unwrap_or_default();
        let mut draft = self.settings_window.draft.clone();

        let driver = catalog.driver(draft.driver.as_deref());
        row(ui, "Driver", |ui| {
            let automatic = catalog
                .driver(None)
                .map_or("Automatic".to_owned(), |driver| {
                    format!("Automatic ({})", driver.name)
                });
            let text = draft.driver.clone().unwrap_or_else(|| automatic.clone());
            egui::ComboBox::from_id_salt("settings-driver")
                .width(FIELD_WIDTH)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(draft.driver.is_none(), automatic)
                        .clicked()
                    {
                        draft.driver = None;
                        draft.output_device = None;
                    }
                    for option in &catalog.drivers {
                        let response = ui.add_enabled(
                            option.error.is_none(),
                            egui::Button::selectable(
                                draft.driver.as_deref() == Some(option.name.as_str()),
                                option.name.as_str(),
                            ),
                        );
                        let response = match &option.error {
                            Some(error) => response.on_disabled_hover_text(error),
                            None => response,
                        };
                        if response.clicked() {
                            draft.driver = Some(option.name.clone());
                            draft.output_device = None;
                        }
                    }
                });
            if discovering {
                ui.spinner();
            }
        });

        let device = driver.and_then(|driver| driver.device(draft.output_device.as_deref()));
        row(ui, "Output device", |ui| {
            let default_name = driver
                .and_then(|driver| driver.default_device.clone())
                .map_or("System default".to_owned(), |name| {
                    format!("System default ({name})")
                });
            let text = draft
                .output_device
                .clone()
                .unwrap_or_else(|| default_name.clone());
            egui::ComboBox::from_id_salt("settings-output-device")
                .width(FIELD_WIDTH)
                .selected_text(text)
                .truncate()
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(draft.output_device.is_none(), default_name)
                        .clicked()
                    {
                        draft.output_device = None;
                    }
                    for option in driver
                        .map(|driver| driver.devices.as_slice())
                        .unwrap_or(&[])
                    {
                        if ui
                            .selectable_label(
                                draft.output_device.as_deref() == Some(option.name.as_str()),
                                option.name.as_str(),
                            )
                            .clicked()
                        {
                            draft.output_device = Some(option.name.clone());
                        }
                    }
                });
        });

        row(ui, "Sample rate", |ui| {
            let current = device
                .and_then(|device| device.current_rate)
                .map_or("Device setting".to_owned(), |rate| {
                    format!("Device setting ({})", rate_label(rate))
                });
            let text = draft.sample_rate.map_or(current.clone(), rate_label);
            let rates: Vec<u32> = match device {
                Some(device) if !device.sample_rates.is_empty() => device.sample_rates.clone(),
                _ => STANDARD_RATES.to_vec(),
            };
            egui::ComboBox::from_id_salt("settings-sample-rate")
                .width(FIELD_WIDTH)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(draft.sample_rate.is_none(), current)
                        .clicked()
                    {
                        draft.sample_rate = None;
                    }
                    for rate in rates {
                        if ui
                            .selectable_label(draft.sample_rate == Some(rate), rate_label(rate))
                            .clicked()
                        {
                            draft.sample_rate = Some(rate);
                        }
                    }
                });
        });

        let rate_for_latency = draft
            .sample_rate
            .or_else(|| device.and_then(|device| device.current_rate))
            .unwrap_or(sympoie::SAMPLE_RATE);
        row(ui, "Buffer size", |ui| {
            let range = device.and_then(|device| device.buffer_range);
            let text = draft
                .buffer_frames
                .map_or("Device default".to_owned(), |frames| {
                    buffer_label(frames, rate_for_latency)
                });
            egui::ComboBox::from_id_salt("settings-buffer")
                .width(FIELD_WIDTH)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(draft.buffer_frames.is_none(), "Device default")
                        .clicked()
                    {
                        draft.buffer_frames = None;
                    }
                    for frames in BUFFER_SIZES {
                        let supported =
                            range.is_none_or(|(min, max)| (min..=max).contains(&frames));
                        if ui
                            .add_enabled(
                                supported,
                                egui::Button::selectable(
                                    draft.buffer_frames == Some(frames),
                                    buffer_label(frames, rate_for_latency),
                                ),
                            )
                            .clicked()
                        {
                            draft.buffer_frames = Some(frames);
                        }
                    }
                });
        });

        row(ui, "Output channels", |ui| {
            let channels = device.map_or(2, |device| device.max_channels.max(2));
            let pair = |first: u16| format!("{} / {}", first + 1, first + 2);
            egui::ComboBox::from_id_salt("settings-channels")
                .width(FIELD_WIDTH)
                .selected_text(pair(draft.first_output_channel))
                .show_ui(ui, |ui| {
                    let mut first = 0;
                    while first + 1 < channels.max(draft.first_output_channel + 2) {
                        if ui
                            .selectable_label(draft.first_output_channel == first, pair(first))
                            .clicked()
                        {
                            draft.first_output_channel = first;
                        }
                        first += 2;
                    }
                });
        });

        let changed = draft != self.settings.audio;
        self.settings_window.draft = draft.clone();

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!discovering, egui::Button::new("Refresh Devices"))
                .on_hover_text("List drivers and devices again, e.g. after connecting an interface")
                .clicked()
            {
                self.settings_window.discover(self.use_output_device);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if changed { "Apply" } else { "Restart Audio" };
                let apply = ui.add(
                    egui::Button::new(RichText::new(label).color(if changed {
                        Color32::BLACK
                    } else {
                        theme::TEXT
                    }))
                    .fill(if changed {
                        theme::ACCENT
                    } else {
                        theme::BG_WIDGET
                    }),
                );
                ui.interact(
                    apply.rect,
                    Id::new("settings-apply-audio"),
                    egui::Sense::hover(),
                );
                if apply
                    .on_hover_text("Reopen the audio device with these settings")
                    .clicked()
                {
                    self.apply_audio_settings(draft.clone());
                }
                if ui
                    .add_enabled(changed, egui::Button::new("Revert"))
                    .clicked()
                {
                    self.settings_window.draft = self.settings.audio.clone();
                }
            });
        });

        let info = self.output.info().clone();
        status_panel(ui, |ui| {
            ui.label(
                RichText::new(if info.is_device {
                    format!("● {}", info.description)
                } else {
                    format!("○ {}", info.description)
                })
                .color(if info.is_device {
                    theme::OK
                } else {
                    theme::WARN
                }),
            );
            let mut facts = vec![if info.is_device {
                format!("{} device", rate_label(info.sample_rate))
            } else {
                format!("silent clock at {}", rate_label(info.sample_rate))
            }];
            if info.is_device {
                facts.push(match info.buffer_frames {
                    Some(frames) => buffer_label(frames, info.sample_rate),
                    None => "device buffer".to_owned(),
                });
                facts.push(format!(
                    "outputs {} / {}",
                    info.first_channel + 1,
                    info.first_channel + 2
                ));
            }
            ui.label(RichText::new(facts.join(" · ")).color(theme::TEXT_DIM));
            for line in info.detail.lines() {
                ui.label(RichText::new(line).small().color(theme::TEXT_DIM));
            }
            if let Some(error) = self.output.device_error() {
                ui.label(RichText::new(error).small().color(theme::ERROR));
            }
            ui.label(
                RichText::new(format!(
                    "The engine renders at {} in stereo; other device rates are resampled.",
                    rate_label(sympoie::SAMPLE_RATE)
                ))
                .small()
                .color(theme::TEXT_FAINT),
            );
        });

        section(ui, "Recording inputs");
        let inputs = self.input_catalog.clone();
        status_panel(ui, |ui| {
            if inputs.audio.is_empty() && inputs.midi.is_empty() {
                ui.label(
                    RichText::new(if self.input_discovery.is_some() {
                        "Looking for inputs…"
                    } else {
                        "No audio or MIDI inputs found"
                    })
                    .color(theme::TEXT_DIM),
                );
            }
            for device in &inputs.audio {
                input_row(
                    ui,
                    "Audio",
                    &device.name,
                    &format!(
                        "{} ch{}",
                        device.channels,
                        if device.is_default { " · default" } else { "" }
                    ),
                );
            }
            for device in &inputs.midi {
                input_row(ui, "MIDI", &device.name, "");
            }
            if let Some(error) = &inputs.error {
                ui.label(RichText::new(error).small().color(theme::WARN));
            }
            ui.label(
                RichText::new("Choose each track's input from the selector in its header.")
                    .small()
                    .color(theme::TEXT_FAINT),
            );
        });
        if ui
            .add_enabled(
                self.input_discovery.is_none(),
                egui::Button::new("Rescan Inputs"),
            )
            .clicked()
        {
            self.refresh_recording_inputs();
        }
    }

    fn general_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let mut general = self.settings.general.clone();

        section(ui, "Interface");
        row(ui, "Interface scale", |ui| {
            let mut percent = (general.interface_scale * 100.0).round();
            let response = ui.add(
                egui::Slider::new(
                    &mut percent,
                    settings::MIN_SCALE * 100.0..=settings::MAX_SCALE * 100.0,
                )
                .step_by(5.0)
                .suffix(" %"),
            );
            if response.drag_stopped() || (response.changed() && !response.dragged()) {
                general.interface_scale = percent / 100.0;
            }
        });
        row(ui, "Follow playback", |ui| {
            ui.checkbox(
                &mut general.follow_playback,
                "Keep the play position in view at startup",
            );
        });

        section(ui, "New projects");
        row(ui, "Tempo", |ui| {
            ui.add(
                egui::DragValue::new(&mut general.default_tempo)
                    .range(20.0..=999.0)
                    .speed(0.5)
                    .fixed_decimals(2)
                    .suffix(" BPM"),
            );
        });
        row(ui, "Time signature", |ui| {
            ui.add(egui::DragValue::new(&mut general.default_beats_per_bar).range(1..=32));
            ui.label(RichText::new("/ 4").color(theme::TEXT_DIM));
        });

        section(ui, "Plug-ins");
        row(ui, "Startup scan", |ui| {
            ui.checkbox(
                &mut general.scan_plugins_at_startup,
                "Look for VST3 plug-ins at startup",
            );
        });
        row(ui, "", |ui| {
            if ui.button("Rescan Plug-ins Now").clicked() {
                self.start_plugin_scan();
            }
        });

        if general != self.settings.general {
            self.set_general(&ctx, general);
        }

        section(ui, "Settings file");
        status_panel(ui, |ui| {
            ui.label(
                RichText::new(match &self.settings_path {
                    Some(path) => path.display().to_string(),
                    None => "Not saved: this session's settings are kept in memory only".to_owned(),
                })
                .small()
                .color(theme::TEXT_DIM),
            );
        });
        if ui.button("Restore Defaults").clicked() {
            self.set_general(&ctx, GeneralSettings::default());
        }
    }
}

fn rate_label(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", rate as f32 / 1000.0)
    }
}

fn buffer_label(frames: u32, rate: u32) -> String {
    format!(
        "{frames} samples · {:.1} ms",
        frames as f32 * 1000.0 / rate.max(1) as f32
    )
}

fn input_row(ui: &mut Ui, kind: &str, name: &str, detail: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(40.0, 16.0), egui::Sense::hover());
        rack::print(
            ui.painter(),
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            kind,
            theme::TEXT_FAINT,
        );
        ui.label(name);
        if !detail.is_empty() {
            ui.label(RichText::new(detail).color(theme::TEXT_DIM));
        }
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    rack::section_label(ui, title);
}

fn row(ui: &mut Ui, label: &str, contents: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            vec2(LABEL_WIDTH, 20.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_width(LABEL_WIDTH);
                ui.label(RichText::new(label).color(theme::TEXT_DIM));
            },
        );
        contents(ui);
    });
}

fn status_panel(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::BG_DEEP)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(3)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(LABEL_WIDTH + FIELD_WIDTH);
            ui.spacing_mut().item_spacing.y = 2.0;
            contents(ui);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Action;
    use crate::app::Startup;
    use eframe::egui::{Pos2, RawInput, Rect};

    fn frame(ctx: &egui::Context, app: &mut SympoieApp) {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1880.0, 900.0))),
                ..RawInput::default()
            },
            |ui| app.frame(ui),
        );
        output.textures_delta.clear();
    }

    fn app_with(
        settings: Settings,
        path: Option<std::path::PathBuf>,
    ) -> (egui::Context, SympoieApp) {
        let ctx = egui::Context::default();
        let app = SympoieApp::new(
            &ctx,
            Startup {
                settings,
                settings_path: path,
                ..Startup::default()
            },
        );
        (ctx, app)
    }

    #[test]
    fn startup_settings_shape_new_projects_scale_and_follow() {
        let mut settings = Settings::default();
        settings.general.default_tempo = 96.0;
        settings.general.default_beats_per_bar = 3;
        settings.general.interface_scale = 1.25;
        settings.general.follow_playback = false;
        let (ctx, mut app) = app_with(settings, None);
        frame(&ctx, &mut app);
        let project = app.document.project();
        assert_eq!(project.bpm(), 96.0);
        assert_eq!(project.time_signature().numerator(), 3);
        assert_eq!(ctx.zoom_factor(), 1.25);
        assert!(!app.arrangement.follow);
    }

    #[test]
    fn settings_window_opens_from_its_action_and_shows_both_tabs() {
        let (ctx, mut app) = app_with(Settings::default(), None);
        frame(&ctx, &mut app);
        app.apply_for_tests(&ctx, Action::ShowSettings);
        for _ in 0..3 {
            frame(&ctx, &mut app);
        }
        assert!(app.show_settings);
        assert_eq!(app.settings_window.catalog, Some(AudioCatalog::default()));
        assert!(ctx.read_response(Id::new("settings-apply-audio")).is_some());
        app.settings_window.tab = Tab::General;
        frame(&ctx, &mut app);
        frame(&ctx, &mut app);
        assert!(ctx.read_response(Id::new("settings-apply-audio")).is_none());
    }

    #[test]
    fn applying_audio_and_general_changes_saves_them() {
        let path = std::env::temp_dir()
            .join(format!("sympoie-settings-window-{}", std::process::id()))
            .join("settings.json");
        let (ctx, mut app) = app_with(Settings::default(), Some(path.clone()));
        let audio = AudioSettings {
            driver: Some("Imaginary".into()),
            output_device: Some("Interface".into()),
            sample_rate: Some(96_000),
            buffer_frames: Some(256),
            first_output_channel: 2,
        };
        app.apply_audio_settings(audio.clone());
        assert!(!app.output.info().is_device);
        assert_eq!(app.settings.audio, audio);
        let general = GeneralSettings {
            interface_scale: 0.9,
            default_tempo: 140.0,
            ..GeneralSettings::default()
        };
        app.set_general(&ctx, general.clone());
        frame(&ctx, &mut app);
        assert_eq!(ctx.zoom_factor(), 0.9);
        let (saved, error) = settings::load(&path);
        assert!(error.is_none());
        assert_eq!(saved.audio, audio);
        assert_eq!(saved.general, general);
        app.apply_for_tests(&ctx, Action::NewProject);
        frame(&ctx, &mut app);
        assert_eq!(app.document.project().bpm(), 140.0);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
