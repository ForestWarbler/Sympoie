use eframe::egui::{self, Color32, RichText, ViewportCommand};
use sympoie::SAMPLE_RATE;

use super::{Pending, SympoieApp, new_project};
use crate::document::Document;
use crate::ui::{brand, theme};

impl SympoieApp {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(pending) = self.pending.clone() {
            let modal = egui::Modal::new(egui::Id::new("unsaved-changes")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new("Save changes?").size(17.0).color(theme::TEXT));
                ui.add_space(4.0);
                let why = match &pending {
                    Pending::New => "before starting a new project",
                    Pending::Open(_) | Pending::OpenFolder => "before opening another project",
                    Pending::Quit => "before quitting",
                };
                ui.label(
                    RichText::new(format!(
                        "“{}” has unsaved changes. Save them {why}?",
                        self.document.display_name()
                    ))
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(12.0);
                let (discard, save) = egui::Sides::new().show(
                    ui,
                    |ui| ui.button("Don't Save").clicked(),
                    |ui| {
                        let save = ui
                            .add(
                                egui::Button::new(RichText::new("Save").color(Color32::BLACK))
                                    .fill(theme::ACCENT),
                            )
                            .clicked();
                        if ui.button("Cancel").clicked() {
                            ui.close();
                        }
                        save
                    },
                );
                if save {
                    Some(true)
                } else if discard {
                    Some(false)
                } else {
                    None
                }
            });
            if modal.should_close() {
                self.pending = None;
            } else if let Some(save) = modal.inner {
                self.pending = None;
                if !save || self.save(false) {
                    self.finish_pending(ctx, pending);
                }
            }
        }

        if self.show_shortcuts {
            let mut open = true;
            egui::Window::new("Keyboard Shortcuts")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    let (command, shift, alt) = if cfg!(target_os = "macos") {
                        ("⌘", "⇧", "⌥")
                    } else {
                        ("Ctrl+", "Shift+", "Alt+")
                    };
                    egui::Grid::new("shortcuts")
                        .num_columns(2)
                        .spacing([24.0, 6.0])
                        .striped(true)
                        .show(ui, |ui| {
                            for (keys, what) in [
                                ("Space".to_owned(), "Play from marker / pause"),
                                ("Q".to_owned(), "Start / stop recording on armed tracks"),
                                ("Home or Enter".to_owned(), "Return to start"),
                                ("L".to_owned(), "Loop on / off"),
                                ("F".to_owned(), "Follow playhead"),
                                ("A".to_owned(), "Show / hide arrangement automation"),
                                ("R".to_owned(), "Reverse selected audio clip or time selection"),
                                ("+  /  −  /  Z".to_owned(), "Zoom in / out / to fit"),
                                (format!("{command}Wheel"), "Zoom around the pointer"),
                                ("Shift+Wheel".to_owned(), "Scroll the timeline"),
                                ("Overview drag".to_owned(), "Pan; drag either edge to zoom"),
                                ("Overview double-click".to_owned(), "Fit the project"),
                                (
                                    "Delete".to_owned(),
                                    "Delete selected notes, automation point, clip, or time selection",
                                ),
                                (
                                    format!("{command}D"),
                                    "Duplicate selected notes or arrangement clip",
                                ),
                                (
                                    if cfg!(target_os = "macos") {
                                        "⌘C / ⌘V or Ctrl+C / Ctrl+V".to_owned()
                                    } else {
                                        "Ctrl+C / Ctrl+V".to_owned()
                                    },
                                    "Copy / paste notes or an arrangement clip",
                                ),
                                (
                                    format!("{command}Drag"),
                                    "Copy notes or a clip, keeping the originals",
                                ),
                                (
                                    format!("{command}E"),
                                    "Split clip, or clips at the time selection",
                                ),
                                (
                                    format!("{command}M"),
                                    "Insert a MIDI clip filling the time selection",
                                ),
                                (
                                    format!("{command}1 / {command}2"),
                                    "Narrow / widen the grid",
                                ),
                                ("Shift+Click".to_owned(), "Extend the time selection"),
                                (
                                    if cfg!(target_os = "macos") {
                                        format!("Ctrl+L / {command}L")
                                    } else {
                                        "Ctrl+L".to_owned()
                                    },
                                    "Enable loop over the selected clip or time range",
                                ),
                                (
                                    format!("{command}T / {shift}{command}T"),
                                    "Add audio / MIDI track",
                                ),
                                (format!("{command}G"), "Group selected tracks"),
                                (format!("{command}Z / {shift}{command}Z"), "Undo / redo"),
                                (format!("{command}I"), "Import audio"),
                                (format!("{shift}{command}R"), "Open the Export window"),
                                ("Alt while dragging".to_owned(), "Ignore the grid"),
                                ("Double-click name".to_owned(), "Rename a track"),
                                (
                                    "Right-click clip title".to_owned(),
                                    "Rename a clip or export it as a file",
                                ),
                                (
                                    "Click lane / clip body".to_owned(),
                                    "Set the next playback start",
                                ),
                                (
                                    "Drag lane / clip body".to_owned(),
                                    "Select a time range; Shift-click to extend",
                                ),
                                (
                                    "Drag clip title / title edge".to_owned(),
                                    "Move / trim a clip",
                                ),
                                (
                                    "Double-click MIDI lane".to_owned(),
                                    "Insert an empty MIDI clip",
                                ),
                                (
                                    format!("{alt}{command}B / R / L / M"),
                                    "Show or hide the Library / Rack / bottom drawer / mixer",
                                ),
                                (format!("{command},"), "Open Settings"),
                                (
                                    "Double-click clip title".to_owned(),
                                    "Open it in the clip editor (MIDI: the note editor)",
                                ),
                                ("Double-click piano grid".to_owned(), "Insert a note"),
                                (
                                    "Shift+click / drag empty grid".to_owned(),
                                    "Select multiple notes / marquee",
                                ),
                                (
                                    format!("{command}A / C / X / V"),
                                    "Select all / copy / cut / paste notes",
                                ),
                                (
                                    "Arrow keys".to_owned(),
                                    "Nudge selected notes in the piano roll",
                                ),
                                (
                                    "Shift+Up / Down".to_owned(),
                                    "Transpose selected notes by one octave",
                                ),
                            ] {
                                ui.label(
                                    RichText::new(keys)
                                        .font(theme::mono(12.0))
                                        .color(theme::ACCENT),
                                );
                                ui.label(what);
                                ui.end_row();
                            }
                        });
                });
            self.show_shortcuts = open;
        }

        if self.show_about {
            let mut open = true;
            egui::Window::new("About Sympoie")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    let logo = brand::logo_texture(ctx);
                    ui.horizontal(|ui| {
                        ui.image((logo.id(), egui::vec2(64.0, 64.0)));
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(format!("Sympoie {}", env!("CARGO_PKG_VERSION")))
                                    .size(18.0)
                                    .color(theme::ACCENT),
                            );
                            ui.label("An open-source audio workstation built from scratch in Rust.");
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!(
                        "Engine: {SAMPLE_RATE} Hz stereo, 960 ticks per quarter note.\nOutput: {} ({})",
                        self.output.info().description,
                        self.output.info().detail
                    )).color(theme::TEXT_DIM));
                    ui.add_space(6.0);
                    ui.label(RichText::new(
                        "Copyright © 2026 ForestWarbler and contributors.\n\
                         Free software under the GNU General Public License, version 3 or \
                         later; you may redistribute and change it under its terms.\n\
                         There is NO WARRANTY, to the extent permitted by law.\n\
                         See LICENSE, and THIRD_PARTY_LICENSES.md for the components it includes.\n\
                         MP3 files are encoded with LAME (lame.sourceforge.io), FLAC files with flacenc.\n\
                         Music and projects you make with Sympoie are yours, under any terms you choose.",
                    ).size(10.5).color(theme::TEXT_DIM));
                });
            self.show_about = open;
        }
    }

    pub(super) fn after_saving(&mut self, ctx: &egui::Context, pending: Pending) {
        if self.document.is_dirty() {
            self.pending = Some(pending);
        } else {
            self.finish_pending(ctx, pending);
        }
    }

    fn finish_pending(&mut self, ctx: &egui::Context, pending: Pending) {
        self.release_preview_before_dialog();
        match pending {
            Pending::New => self.replace_document(Document::new(new_project(&self.settings), None)),
            Pending::Open(Some(path)) => self.open(&path),
            Pending::OpenFolder => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Open Project Folder")
                    .pick_folder()
                {
                    self.open(&path);
                }
            }
            Pending::Open(None) => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Open Project")
                    .add_filter("Sympoie project", &["json", "symp"])
                    .pick_file()
                {
                    self.open(&path);
                }
            }
            Pending::Quit => {
                self.allowed_to_close = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }
}
