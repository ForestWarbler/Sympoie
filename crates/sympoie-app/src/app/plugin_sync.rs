use std::collections::BTreeSet;

use eframe::egui;
use sympoie::plugin::PluginDescriptor;
use sympoie::project::Project;

use super::{PluginLoad, SympoieApp, track_plugin_ids};
use crate::actions::Action;
use crate::document::edit;
use crate::state::Selection;
use crate::views::devices;

impl SympoieApp {
    pub(super) fn focus_builtin_controls(&mut self, ctx: &egui::Context, id: u64) -> bool {
        if !self.document.project().plugin(id).is_some_and(|plugin| {
            sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor).is_some()
        }) {
            return false;
        }
        let Some(track) = self.plugin_track(id) else {
            return false;
        };
        let mixer = self.document.project().tracks()[track]
            .mixer_effects()
            .iter()
            .any(|plugin| plugin.id == id);
        self.select_editor_track(track);
        self.vst3_editors.remove(&id);
        if mixer {
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("mixer-builtin-inspector"), id));
            self.apply(ctx, Action::ShowMixer);
        } else {
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("builtin-chain-focus"), id));
            self.apply(ctx, Action::ShowDevices);
        }
        true
    }

    pub(super) fn queue_plugin_load(
        &mut self,
        track: usize,
        plugin: PluginDescriptor,
        before: Option<u64>,
    ) {
        self.queue_plugin_load_in_chain(track, plugin, before, false);
    }

    pub(super) fn queue_plugin_load_in_chain(
        &mut self,
        track: usize,
        plugin: PluginDescriptor,
        before: Option<u64>,
        mixer: bool,
    ) {
        if self.plugin_load.is_some() {
            return;
        }
        let validate = if mixer {
            edit::validate_mixer_effect_destination
        } else {
            edit::validate_plugin_destination
        };
        if let Err(error) = validate(self.document.project(), track, plugin.kind, before) {
            self.warn(error);
            return;
        }
        self.plugin_load = Some(PluginLoad {
            track,
            generation: self.structural_generation,
            plugin,
            before,
            mixer,
            ready: false,
        });
    }

    pub(super) fn forget_closed_editor_viewports(&mut self, ctx: &egui::Context) {
        ctx.input(|input| {
            let closed = |id| {
                input
                    .raw
                    .viewports
                    .get(&id)
                    .is_some_and(|viewport| viewport.close_requested())
            };
            self.plugin_editors
                .retain(|&track| !closed(devices::editor_viewport_id(track)));
            self.vst3_editors
                .retain(|&id| !closed(devices::vst3_editor_viewport_id(id)));
        });
    }

    pub(super) fn editor_track(&self) -> Option<usize> {
        if self.selection.tracks.len() != 1 {
            return None;
        }
        self.selection
            .tracks
            .first()
            .copied()
            .filter(|&track| track < self.document.project().tracks().len())
    }

    pub(super) fn selected_plugin_ids(&self) -> BTreeSet<u64> {
        let Some(index) = self.editor_track() else {
            return BTreeSet::new();
        };
        track_plugin_ids(&self.document.project().tracks()[index]).collect()
    }

    pub(super) fn plugin_track(&self, id: u64) -> Option<usize> {
        self.document
            .project()
            .tracks()
            .iter()
            .position(|track| track_plugin_ids(track).any(|candidate| candidate == id))
    }

    pub(super) fn select_editor_track(&mut self, track: usize) {
        if self.editor_track() != Some(track) {
            self.selection = Selection {
                tracks: BTreeSet::from([track]),
                clip: None,
                range: None,
            };
            self.piano_roll.focused = false;
        }
        self.sync_editor_visibility();
    }

    pub(super) fn sync_editor_visibility(&mut self) {
        let visible = self.selected_plugin_ids();
        let mut failures = Vec::new();
        self.native_editors.retain(|id, editor| {
            if !editor.is_open() {
                return false;
            }
            if let Err(error) = editor.set_visible(visible.contains(id)) {
                failures.push(error);
                return false;
            }
            true
        });
        if !failures.is_empty() {
            self.warn(format!(
                "Cannot update plugin windows: {}",
                failures.join("; ")
            ));
        }
    }

    pub(super) fn poll_plugins(&mut self, ctx: &egui::Context) {
        self.plugins.service();
        if self.plugins.take_state_dirty() {
            self.document.mark_external_state_dirty();
        }
        let mut failures = Vec::new();
        self.native_editors.retain(|_, editor| {
            if let Err(error) = editor.service() {
                failures.push(error);
                return false;
            }
            editor.is_open()
        });
        for (id, parameter, value) in self.plugins.poll_edits() {
            if self
                .document
                .project()
                .plugin(id)
                .and_then(|p| p.parameters.iter().find(|p| p.id == parameter))
                .is_some_and(|p| !p.read_only && p.value != value)
            {
                self.set_plugin_parameter(id, parameter, value);
            }
        }
        failures.extend(self.plugins.take_errors());
        if !failures.is_empty() {
            self.error(failures.join("; "));
        }
        if let Some(load) = &mut self.plugin_load
            && !load.ready
        {
            load.ready = true;
            return;
        }
        if let Some(load) = self.plugin_load.take() {
            if load.generation != self.structural_generation {
                self.warn("The arrangement changed during loading. Choose the destination track and load the plugin again.".to_owned());
                return;
            }
            match self.plugins.inspect(&load.plugin) {
                Ok(plugin) => {
                    let track = load.track;
                    let descriptor = plugin.descriptor.clone();
                    let inserted = self.edit_structural("Load Plugin", |project| {
                        if load.mixer {
                            edit::insert_mixer_plugin(project, track, plugin, load.before)
                        } else {
                            edit::insert_plugin(project, track, plugin, load.before)
                        }
                    });
                    if let Some(id) = inserted {
                        self.selection = Selection {
                            tracks: BTreeSet::from([track]),
                            clip: None,
                            range: None,
                        };
                        self.piano_roll.focused = false;
                        self.plugin_browser.open = false;
                        self.info(format!("Loaded {}", descriptor.name));
                        self.sync_engine();
                        self.apply(ctx, Action::OpenNativeVst3Editor(id));
                    } else {
                        self.plugins.discard_staged(&descriptor);
                    }
                }
                Err(error) => self.error(format!("Could not load plugin: {error}")),
            }
        }
    }

    pub(super) fn flush_plugin_edits(&mut self) -> Result<(), String> {
        let mut failures = Vec::new();
        self.native_editors.retain(|_, editor| {
            if let Err(error) = editor.service() {
                failures.push(error);
                return false;
            }
            editor.is_open()
        });
        if let Err(error) = self.plugins.service_blocking() {
            failures.push(error);
        }
        if self.plugins.take_state_dirty() {
            self.document.mark_external_state_dirty();
        }
        match self.plugins.poll_edits_blocking() {
            Ok(edits) => {
                for (id, parameter, value) in edits {
                    if self
                        .document
                        .project()
                        .plugin(id)
                        .and_then(|plugin| plugin.parameters.iter().find(|p| p.id == parameter))
                        .is_some_and(|p| !p.read_only && p.value != value)
                    {
                        self.set_plugin_parameter(id, parameter, value);
                    }
                }
            }
            Err(error) => failures.push(error),
        }
        failures.extend(self.plugins.take_errors());
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    pub(super) fn project_with_plugin_state(&self) -> Result<Project, String> {
        let mut project = self.document.project().clone();
        let ids: Vec<_> = project.tracks().iter().flat_map(track_plugin_ids).collect();
        for id in ids {
            if self.plugins.has_instance(id) {
                let (_parameters, state) = self.plugins.snapshot(id)?;
                project
                    .set_plugin_state(id, state)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(project)
    }
}
