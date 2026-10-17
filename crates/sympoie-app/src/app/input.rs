use std::path::PathBuf;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, ViewportCommand};
use sympoie::transport::TransportCommand;

use super::{Pending, SympoieApp};
use crate::actions::Action;
use crate::document::edit::TrackKind;

impl SympoieApp {
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let command = Modifiers::COMMAND;
        let command_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let command_alt = Modifiers::COMMAND | Modifiers::ALT;
        let typing = ctx.egui_wants_keyboard_input();
        let clip = self.selection.clip;
        let mut bindings: Vec<(KeyboardShortcut, Action)> = vec![
            (KeyboardShortcut::new(command, Key::N), Action::NewProject),
            (KeyboardShortcut::new(command, Key::Q), Action::Quit),
            (KeyboardShortcut::new(command, Key::O), Action::OpenProject),
            (KeyboardShortcut::new(command_shift, Key::S), Action::SaveAs),
            (KeyboardShortcut::new(command, Key::S), Action::Save),
            (
                KeyboardShortcut::new(command, Key::I),
                Action::ChooseAudioToImport {
                    track: None,
                    start: None,
                },
            ),
            (
                KeyboardShortcut::new(command_shift, Key::R),
                Action::ExportAudio { loop_only: false },
            ),
            (
                KeyboardShortcut::new(command_shift, Key::T),
                Action::AddTrack(TrackKind::Midi),
            ),
            (
                KeyboardShortcut::new(command, Key::T),
                Action::AddTrack(TrackKind::Audio),
            ),
            (
                KeyboardShortcut::new(command_shift, Key::G),
                Action::AddTrack(TrackKind::Group),
            ),
            (
                KeyboardShortcut::new(command, Key::G),
                Action::GroupSelected,
            ),
            (
                KeyboardShortcut::new(command_alt, Key::M),
                Action::ToggleMixer,
            ),
            (
                KeyboardShortcut::new(command_alt, Key::B),
                Action::ToggleBrowser,
            ),
            (
                KeyboardShortcut::new(command_alt, Key::R),
                Action::ToggleRack,
            ),
            (
                KeyboardShortcut::new(command_alt, Key::L),
                Action::ToggleDetailView,
            ),
            (
                KeyboardShortcut::new(command, Key::Comma),
                Action::ShowSettings,
            ),
        ];
        if !typing {
            bindings.extend([
                (KeyboardShortcut::new(command_shift, Key::Z), Action::Redo),
                (KeyboardShortcut::new(command, Key::Z), Action::Undo),
                (KeyboardShortcut::new(command, Key::Y), Action::Redo),
                (
                    KeyboardShortcut::new(command, Key::Num1),
                    Action::StepGrid { narrower: true },
                ),
                (
                    KeyboardShortcut::new(command, Key::Num2),
                    Action::StepGrid { narrower: false },
                ),
                (
                    KeyboardShortcut::new(command, Key::M),
                    Action::InsertMidiClipInSelection,
                ),
                (
                    KeyboardShortcut::new(Modifiers::CTRL, Key::M),
                    Action::InsertMidiClipInSelection,
                ),
                (
                    KeyboardShortcut::new(command, Key::L),
                    Action::LoopSelection,
                ),
                (
                    KeyboardShortcut::new(Modifiers::CTRL, Key::L),
                    Action::LoopSelection,
                ),
            ]);
            if self.selection.range.is_some() && !self.piano_roll.focused {
                bindings.push((
                    KeyboardShortcut::new(command, Key::E),
                    Action::SplitTimeSelection,
                ));
            }
            if let Some(clip) = clip.filter(|_| !self.piano_roll.focused) {
                bindings.extend([
                    (
                        KeyboardShortcut::new(command, Key::D),
                        Action::DuplicateClip(clip),
                    ),
                    (
                        KeyboardShortcut::new(command, Key::E),
                        Action::SplitClip(clip),
                    ),
                ]);
            }
        }
        for (shortcut, action) in bindings {
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                actions.push(action);
            }
        }
        if typing {
            return;
        }
        let record = ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(event, egui::Event::Key {
                    key: Key::Q,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if modifiers.is_none())
            })
        });
        ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Q));
        if record {
            actions.push(Action::ToggleRecord);
        }
        if !self.piano_roll.focused {
            let (mut native_copy, mut native_paste) = (false, false);
            ctx.input_mut(|input| {
                input.events.retain(|event| match event {
                    egui::Event::Copy => {
                        native_copy = true;
                        false
                    }
                    egui::Event::Paste(text) => {
                        native_paste |= self
                            .clip_clipboard
                            .as_ref()
                            .is_some_and(|clipboard| clipboard.text == *text);
                        false
                    }
                    _ => true,
                });
            });
            let copy = ctx.input_mut(|input| {
                input.consume_shortcut(&KeyboardShortcut::new(command, Key::C))
                    | input.consume_shortcut(&KeyboardShortcut::new(Modifiers::CTRL, Key::C))
            });
            let paste = ctx.input_mut(|input| {
                input.consume_shortcut(&KeyboardShortcut::new(command, Key::V))
                    | input.consume_shortcut(&KeyboardShortcut::new(Modifiers::CTRL, Key::V))
            });
            if (copy || native_copy)
                && let Some(clip) = clip
            {
                actions.push(Action::CopyClip(clip));
            }
            if paste || native_paste {
                actions.push(Action::PasteClip);
            }
        }
        let none = Modifiers::NONE;
        let mut keys: Vec<(Key, Action)> = vec![
            (Key::Space, Action::TogglePlay),
            (
                Key::Home,
                Action::Transport(TransportCommand::ReturnToStart),
            ),
            (
                Key::Enter,
                Action::Transport(TransportCommand::ReturnToStart),
            ),
            (Key::L, Action::ToggleLoop),
            (Key::F, Action::ToggleFollow),
        ];
        if !self.piano_roll.focused {
            keys.extend([
                (Key::A, Action::ToggleAutomation),
                (Key::Plus, Action::Zoom(1.4)),
                (Key::Equals, Action::Zoom(1.4)),
                (Key::Minus, Action::Zoom(1.0 / 1.4)),
                (Key::Z, Action::ZoomToFit),
            ]);
            if !self.arrangement.automation && self.selection.has_audio(self.document.project()) {
                keys.push((Key::R, Action::ReverseSelection));
            }
            if !self.arrangement.automation {
                keys.push((Key::Escape, Action::SelectClip(None)));
            }
            if !self.arrangement.automation && self.selection.range.is_some() {
                keys.push((Key::Delete, Action::DeleteTimeSelection));
                keys.push((Key::Backspace, Action::DeleteTimeSelection));
            } else if let Some(clip) = clip.filter(|_| !self.arrangement.automation) {
                keys.push((Key::Delete, Action::DeleteClip(clip)));
                keys.push((Key::Backspace, Action::DeleteClip(clip)));
            }
        }
        for (key, action) in keys {
            if ctx.input_mut(|i| i.consume_key(none, key)) {
                actions.push(action);
            }
        }
    }

    pub(super) fn collect_drops(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let (projects, audio): (Vec<PathBuf>, Vec<PathBuf>) = dropped
            .into_iter()
            .partition(|path| sympoie::project_json::is_project_path(path));
        if let Some(project) = projects.into_iter().next() {
            actions.push(Action::OpenPath(project));
            return;
        }
        let target = self.drop_target;
        actions.push(Action::ImportAudio {
            paths: audio,
            track: target.and_then(|target| target.track),
            start: target.map(|target| target.start),
        });
    }

    pub(super) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allowed_to_close {
            self.finish_recording();
        }
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allowed_to_close
            && self.document.is_dirty()
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.pending = Some(Pending::Quit);
        }
    }
}
