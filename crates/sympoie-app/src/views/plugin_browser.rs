use std::path::PathBuf;

use eframe::egui::{self, RichText};
use sympoie::builtin::BuiltinKind;
use sympoie::plugin::{PluginDescriptor, PluginKind};
use sympoie::project::Project;

use crate::actions::Action;
use crate::plugins::scan::{ScanMode, ScanTask};
use crate::ui::theme;

#[derive(Default)]
pub struct PluginBrowser {
    pub open: bool,
    target: Option<(usize, PluginKind)>,
    mixer: bool,
    scan: Option<ScanTask>,
    catalog: Vec<PluginDescriptor>,
    failures: Vec<String>,
    paths: Vec<PathBuf>,
    query: String,
    scanned: bool,
}

impl PluginBrowser {
    pub fn start_scan(&mut self) {
        if self.scan.is_none() {
            self.rescan(ScanMode::Cached);
        }
    }

    pub fn open(&mut self, track: usize, kind: PluginKind) {
        self.mixer = false;
        self.open = true;
        self.target = Some((track, kind));
        if !self.scanned && self.scan.is_none() {
            self.start_scan();
        }
    }

    pub fn open_mixer(&mut self, track: usize) {
        self.open(track, PluginKind::Effect);
        self.mixer = true;
    }

    fn rescan(&mut self, mode: ScanMode) {
        self.scan = Some(ScanTask::start(self.paths.clone(), mode));
        self.failures.clear();
    }

    pub fn busy(&self) -> bool {
        self.scan.is_some()
    }

    pub fn catalog(&self) -> &[PluginDescriptor] {
        &self.catalog
    }

    pub fn scan_status(&self) -> Option<String> {
        self.scan.as_ref().map(|scan| {
            let progress = scan.progress();
            if progress.total == 0 {
                "Checking VST3 plugins…".to_owned()
            } else {
                format!(
                    "Scanning VST3… {} / {} · {} cached",
                    progress.completed, progress.total, progress.cached
                )
            }
        })
    }

    fn poll_scan(&mut self) {
        if let Some(scan) = &self.scan
            && let Some(result) = scan.poll()
        {
            match result {
                Ok(update) => {
                    self.catalog = update.result.plugins;
                    self.failures = update.result.failures;
                    self.paths = update.extra_paths;
                    if update.finished {
                        self.scan = None;
                        self.scanned = true;
                    }
                }
                Err(error) => {
                    self.failures.push(error);
                    self.scan = None;
                    self.scanned = true;
                }
            }
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        loading: bool,
        actions: &mut Vec<Action>,
    ) {
        self.poll_scan();
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Plug-Ins")
            .id(egui::Id::new("vst3-browser"))
            .open(&mut open)
            .default_size([620.0, 460.0])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(self.scan.is_none(), egui::Button::new("Rescan Plugins"))
                        .on_hover_text("Check for new or changed plugins using the saved scan results.")
                        .clicked()
                    {
                        self.rescan(ScanMode::Cached);
                    }
                    if ui
                        .add_enabled(self.scan.is_none(), egui::Button::new("Full Rescan"))
                        .on_hover_text("Probe every plugin again, including previous failures. This takes longer.")
                        .clicked()
                    {
                        self.rescan(ScanMode::Force);
                    }
                    if ui
                        .add_enabled(self.scan.is_none(), egui::Button::new("Add Folder…"))
                        .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_title("VST3 plugin folder")
                            .pick_folder()
                    {
                        if !self.paths.contains(&path) {
                            self.paths.push(path);
                        }
                        self.rescan(ScanMode::Cached);
                    }
                    if let Some(status) = self.scan_status() {
                        ui.spinner();
                        ui.label(status);
                    }
                });
                ui.label(
                    RichText::new(
                        "Scans at launch. Unchanged plugins use saved results; new and changed plugins appear as they are scanned.",
                    )
                    .small()
                    .color(theme::TEXT_DIM),
                );
                if !self.paths.is_empty() {
                    ui.collapsing("Additional folders", |ui| {
                        for path in &self.paths {
                            ui.label(path.display().to_string());
                        }
                    });
                }
                ui.separator();
                let Some((track, kind)) = self
                    .target
                    .filter(|(index, _)| *index < project.tracks().len())
                else {
                    ui.label("Select a track and open its plugin browser to load a plugin.");
                    return;
                };
                let label = match kind {
                    PluginKind::Instrument => "instrument",
                    PluginKind::Effect if self.mixer => "Mixer effect",
                    PluginKind::Effect => "effect",
                };
                ui.label(format!(
                    "Load {label} on {}",
                    project.tracks()[track].name()
                ));
                ui.add(
                    egui::TextEdit::singleline(&mut self.query).hint_text("Search name or vendor"),
                );
                if loading {
                    ui.label("Loading plugin…");
                }
                let query = self.query.to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("vst3-results")
                    .max_height(270.0)
                    .show(ui, |ui| {
                        let mut shown = 0;
                        let builtins = BuiltinKind::ALL.map(BuiltinKind::descriptor);
                        for plugin in builtins.iter().chain(self.catalog.iter()).filter(|plugin| plugin.kind == kind) {
                            if !query.is_empty()
                                && !format!("{} {}", plugin.name, plugin.vendor)
                                    .to_lowercase()
                                    .contains(&query)
                            {
                                continue;
                            }
                            shown += 1;
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(!loading, egui::Button::new("Load"))
                                    .clicked()
                                {
                                    actions.push(if self.mixer {
                                        Action::LoadMixerVst3 {
                                            track, plugin: plugin.clone(), before: None,
                                        }
                                    } else {
                                        Action::LoadVst3 { track, plugin: plugin.clone() }
                                    });
                                }
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&plugin.name).strong());
                                    ui.label(
                                        RichText::new(&plugin.vendor)
                                            .small()
                                            .color(theme::TEXT_DIM),
                                    )
                                    .on_hover_text(plugin.path.display().to_string());
                                });
                            });
                            ui.separator();
                        }
                        if shown == 0 && self.scan.is_none() {
                            ui.label(format!("No matching {label}s found."));
                        }
                    });
                if !self.failures.is_empty() {
                    ui.collapsing(
                        format!("{} plugins could not be scanned", self.failures.len()),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(100.0)
                                .show(ui, |ui| {
                                    for error in &self.failures {
                                        ui.label(RichText::new(error).small().color(theme::WARN));
                                    }
                                });
                        },
                    );
                }
            });
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::plugins::scan::{ScanResult, ScanUpdate};

    fn plugin(name: &str) -> PluginDescriptor {
        PluginDescriptor {
            path: PathBuf::from(format!("/test/{name}.vst3")),
            class_id: name.to_owned(),
            name: name.to_owned(),
            vendor: "Test".to_owned(),
            kind: PluginKind::Effect,
        }
    }

    #[test]
    fn hidden_browser_accepts_partial_results_and_keeps_scanning_when_opened() {
        let (sender, receiver) = mpsc::channel();
        let mut browser = PluginBrowser {
            scan: Some(ScanTask::from_updates(receiver)),
            ..PluginBrowser::default()
        };
        sender
            .send(Ok(ScanUpdate {
                result: ScanResult {
                    plugins: vec![plugin("Cached")],
                    failures: Vec::new(),
                },
                extra_paths: vec![PathBuf::from("/test/custom")],
                finished: false,
            }))
            .unwrap();
        let mut actions = Vec::new();
        browser.show(
            &egui::Context::default(),
            &Project::new(),
            false,
            &mut actions,
        );
        assert!(!browser.open);
        assert!(browser.busy());
        assert_eq!(browser.catalog[0].name, "Cached");
        assert_eq!(browser.paths, [PathBuf::from("/test/custom")]);
        assert!(actions.is_empty());

        browser.open(0, PluginKind::Effect);
        sender
            .send(Ok(ScanUpdate {
                result: ScanResult {
                    plugins: vec![plugin("New")],
                    failures: vec!["Failed plugin".to_owned()],
                },
                extra_paths: browser.paths.clone(),
                finished: true,
            }))
            .unwrap();
        browser.poll_scan();
        assert!(!browser.busy());
        assert!(browser.scanned);
        assert_eq!(browser.catalog.len(), 1);
        assert_eq!(browser.catalog[0].name, "New");
        assert_eq!(browser.failures, ["Failed plugin"]);
    }

    #[test]
    fn scanner_failure_preserves_available_plugins_and_ends_progress() {
        let (sender, receiver) = mpsc::channel();
        let mut browser = PluginBrowser {
            scan: Some(ScanTask::from_updates(receiver)),
            catalog: vec![plugin("Available")],
            ..PluginBrowser::default()
        };
        sender.send(Err("Scanner unavailable".to_owned())).unwrap();
        browser.poll_scan();
        assert!(!browser.busy());
        assert!(browser.scanned);
        assert_eq!(browser.catalog[0].name, "Available");
        assert_eq!(browser.failures, ["Scanner unavailable"]);
        assert!(browser.scan_status().is_none());
    }
}
