use std::path::{Path, PathBuf};

use sympoie::audio_import::{FORMATS_DESCRIPTION, read_audio};
use sympoie::project::{AudioBuffer, Project};

use super::{SympoieApp, audio_dialog_extensions, file_name, has_extension};
use crate::document::Document;
use crate::document::edit;

/// Sampler start, end, loop start and loop end (normalized) that play a whole new sample.
const FULL_SAMPLE: [(u32, f64); 4] = [(30, 0.0), (31, 1.0), (32, 0.0), (33, 1.0)];

impl SympoieApp {
    pub(super) fn load_sampler_sample(&mut self, id: u64) {
        use sympoie::builtin::BuiltinKind;
        if !self.document.project().plugin(id).is_some_and(|plugin| {
            BuiltinKind::from_descriptor(&plugin.descriptor) == Some(BuiltinKind::Sampler)
        }) {
            return;
        }
        self.release_preview_before_dialog();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Load Sampler Audio")
            .add_filter("Audio", &audio_dialog_extensions())
            .pick_file()
        else {
            return;
        };
        self.set_sampler_sample(id, &path);
    }

    /// Loads `path` into the Sampler on `track`, or adds a MIDI track named after the
    /// file with a Sampler holding it when `track` has no Sampler.
    pub(super) fn load_sample_into_sampler(&mut self, path: &Path, track: Option<usize>) {
        use sympoie::builtin::{BuiltinKind, instruments::sample_state_from_wav};
        if let Some(id) = track.and_then(|track| edit::sampler_on(self.document.project(), track)) {
            self.set_sampler_sample(id, path);
            return;
        }
        let state = match sample_state_from_wav(path) {
            Ok(state) => state,
            Err(error) => {
                self.error(format!("Could not load sample: {error}"));
                return;
            }
        };
        let name = path.file_stem().map_or_else(
            || "Sampler".to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let mut instance = BuiltinKind::Sampler.instance();
        instance.state = state;
        for parameter in &mut instance.parameters {
            if let Some(&(_, value)) = FULL_SAMPLE.iter().find(|(id, _)| *id == parameter.id) {
                parameter.value = value;
            }
        }
        let added = self.edit_structural("Add Sampler Track", |project| {
            let index = edit::add_track(project, edit::TrackKind::Midi, Some(name));
            edit::insert_plugin(project, index, instance, None)?;
            Ok(index)
        });
        if let Some(index) = added {
            self.selection = crate::state::Selection {
                tracks: std::collections::BTreeSet::from([index]),
                clip: None,
                range: None,
            };
            self.piano_roll.focused = false;
            self.sync_engine();
            self.info(format!("Added a Sampler track with {}", file_name(path)));
        }
    }

    /// Replaces the sample in the Sampler `id` with `path`, playing all of it.
    fn set_sampler_sample(&mut self, id: u64, path: &Path) {
        use sympoie::builtin::instruments::sample_state_from_wav;
        match sample_state_from_wav(path) {
            Ok(state) => {
                if self
                    .edit_structural("Load Sampler Audio", |project| {
                        project
                            .set_plugin_state(id, state)
                            .map_err(|e| e.to_string())?;
                        for (parameter, value) in FULL_SAMPLE {
                            project
                                .set_plugin_parameter(id, parameter, value)
                                .map_err(|e| e.to_string())?;
                        }
                        Ok(())
                    })
                    .is_some()
                {
                    self.info(format!("Loaded {} into Sampler", file_name(path)));
                }
            }
            Err(error) => self.error(format!("Could not load sample: {error}")),
        }
    }

    pub(super) fn open(&mut self, path: &Path) {
        match Project::load(path) {
            Ok(project) => {
                let document_path = sympoie::project_json::project_location(path);
                self.replace_document(Document::new(project, Some(document_path.clone())));
                self.info(format!("Opened {}", file_name(path)));
                if document_path.is_dir() && self.settings.agent.remembers(&document_path) {
                    self.start_agent_mode(&document_path);
                    self.info(format!(
                        "Opened {} in agent mode",
                        file_name(&document_path)
                    ));
                }
            }
            Err(error) => self.error(format!("Couldn't open {}: {error}", path.display())),
        }
    }

    pub(super) fn save(&mut self, choose: bool) -> bool {
        self.release_preview_before_dialog();
        self.finish_recording();
        let path = match self.document.path() {
            Some(path) if !choose && path.is_dir() => path.to_owned(),
            _ => {
                let stem = self
                    .document
                    .display_name()
                    .trim_end_matches(".symp")
                    .to_owned();
                let name = if self.document.path().is_some_and(Path::is_file) {
                    format!("{stem} Project.symp")
                } else {
                    format!("{stem}.symp")
                };
                let Some(mut path) = rfd::FileDialog::new()
                    .set_title("Save Project Folder")
                    .add_filter("Sympoie project", &["symp"])
                    .set_file_name(name)
                    .save_file()
                else {
                    return false;
                };
                if !has_extension(&path, "symp") {
                    path.set_extension("symp");
                }
                path
            }
        };
        let project = match self.project_with_plugin_state() {
            Ok(project) => project,
            Err(error) => {
                self.error(format!("Couldn't capture plugin state for saving: {error}"));
                return false;
            }
        };
        let result = match self.save_through_agent(&path, project.clone()) {
            Some(result) => result,
            None => sympoie::project_json::save_project_folder(&project, &path)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        };
        match result {
            Ok(()) => {
                self.document.mark_saved(path.clone());
                self.info(format!("Saved {}", file_name(&path)));
                true
            }
            Err(error) => {
                self.error(format!("Couldn't save {}: {error}", path.display()));
                false
            }
        }
    }

    pub(super) fn import(&mut self, paths: Vec<PathBuf>, track: Option<usize>, start: Option<u64>) {
        let mut decoded: Vec<(String, AudioBuffer)> = Vec::new();
        let mut failures = Vec::new();
        for path in &paths {
            let name = path.file_stem().map_or_else(
                || "Audio".to_owned(),
                |stem| stem.to_string_lossy().into_owned(),
            );
            match read_audio(path) {
                Ok(audio) => decoded.push((name, audio)),
                Err(error) => failures.push(format!("{}: {error}", file_name(path))),
            }
        }
        if !failures.is_empty() {
            self.error(format!(
                "Couldn't import {}. Sympoie reads {FORMATS_DESCRIPTION} files.",
                failures.join("; ")
            ));
        }
        if decoded.is_empty() {
            return;
        }
        let had_audio = self.document.project().duration_frames().unwrap_or(0) > 0;
        let start = start.unwrap_or_else(|| {
            edit::snap(
                self.arrangement.grid(self.document.project()),
                self.playhead(),
                true,
            )
        });
        let count = decoded.len();
        let label = if count == 1 {
            "Import Audio"
        } else {
            "Import Audio Files"
        };
        let placed = self.edit_structural(label, |project| {
            let mut placed = Vec::new();
            for (position, (name, audio)) in decoded.into_iter().enumerate() {
                let target = if position == 0 { track } else { None };
                placed.push(edit::place_audio(project, audio, &name, target, start)?);
            }
            Ok(placed)
        });
        if let Some(placed) = placed {
            if let Some(last) = placed.last() {
                self.selection.select_clip(*last);
            }
            if !had_audio {
                self.fit_pending = true;
            }
            if failures.is_empty() {
                self.info(format!(
                    "Imported {count} file{}",
                    if count == 1 { "" } else { "s" }
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
