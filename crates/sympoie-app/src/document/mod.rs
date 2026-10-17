pub mod edit;
pub mod rows;
mod track_order;

#[cfg(test)]
mod track_order_tests;

#[cfg(test)]
mod audio_processing_tests;

#[cfg(test)]
mod mixer_tests;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sympoie::project::Project;

const UNDO_LIMIT: usize = 200;
const COALESCE_WINDOW: Duration = Duration::from_millis(1200);

struct Snapshot {
    project: Project,
    version: u64,
    label: String,
    invalidates_clip_refs: bool,
    track_mapping: Option<Vec<usize>>,
}

pub struct Document {
    project: Project,
    path: Option<PathBuf>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    version: u64,
    saved_version: Option<u64>,
    external_state_dirty: bool,
    external_state_revision: u64,
    next_version: u64,
    revision: u64,
    coalescing: Option<(String, Instant)>,
}

impl Document {
    pub fn new(project: Project, path: Option<PathBuf>) -> Self {
        Self {
            project,
            saved_version: path.as_ref().map(|_| 0),
            external_state_dirty: false,
            external_state_revision: 0,
            path,
            undo: Vec::new(),
            redo: Vec::new(),
            version: 0,
            next_version: 1,
            revision: 0,
            coalescing: None,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn display_name(&self) -> String {
        self.path.as_deref().and_then(Path::file_name).map_or_else(
            || "Untitled".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    pub fn is_dirty(&self) -> bool {
        self.external_state_dirty
            || match self.saved_version {
                Some(saved) => saved != self.version,
                None => self.version != 0 || self.project.tracks().len() > 1,
            }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn mark_external_state_dirty(&mut self) {
        self.external_state_dirty = true;
        self.external_state_revision = self.external_state_revision.wrapping_add(1);
    }

    pub fn external_state_revision(&self) -> u64 {
        self.external_state_revision
    }

    pub fn capture_plugin_states(&mut self, states: Vec<(u64, Vec<u8>)>) -> Result<(), String> {
        let mut project = self.project.clone();
        for (id, state) in states {
            project
                .set_plugin_state(id, state)
                .map_err(|error| error.to_string())?;
        }
        self.project = project;
        Ok(())
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn mark_version_saved(&mut self, version: u64, external_state_revision: u64) {
        self.saved_version = Some(version);
        if self.external_state_revision == external_state_revision {
            self.external_state_dirty = false;
        }
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.saved_version = Some(self.version);
        self.external_state_dirty = false;
        self.coalescing = None;
    }

    pub fn edit<T>(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Result<T, String> {
        self.edit_inner(label, None, false, change)
    }

    pub fn edit_structural<T>(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Result<T, String> {
        self.edit_inner(label, None, true, change)
    }

    pub fn edit_track_order(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<Vec<usize>, String>,
    ) -> Result<Vec<usize>, String> {
        let mapping = self.edit_structural(label, change)?;
        self.undo
            .last_mut()
            .expect("recorded track move")
            .track_mapping = Some(invert_mapping(&mapping));
        Ok(mapping)
    }

    pub fn edit_coalesced<T>(
        &mut self,
        key: &str,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Result<T, String> {
        self.edit_inner(label, Some(key), false, change)
    }

    pub fn break_coalescing(&mut self) {
        self.coalescing = None;
    }

    fn edit_inner<T>(
        &mut self,
        label: &str,
        key: Option<&str>,
        invalidates_clip_refs: bool,
        change: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut next = self.project.clone();
        let result = change(&mut next)?;
        let now = Instant::now();
        let continues = match (&self.coalescing, key) {
            (Some((last, at)), Some(key)) => {
                last == key && now.duration_since(*at) < COALESCE_WINDOW
            }
            _ => false,
        };
        if !continues {
            let previous = std::mem::replace(&mut self.project, next);
            self.undo.push(Snapshot {
                project: previous,
                version: self.version,
                label: label.to_owned(),
                invalidates_clip_refs,
                track_mapping: None,
            });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        } else {
            self.project = next;
        }
        self.coalescing = key.map(|key| (key.to_owned(), now));
        self.redo.clear();
        self.version = self.next_version;
        self.next_version += 1;
        self.revision += 1;
        Ok(result)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|step| step.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|step| step.label.as_str())
    }

    pub fn undo_invalidates_clip_refs(&self) -> bool {
        self.undo
            .last()
            .is_some_and(|step| step.invalidates_clip_refs)
    }

    pub fn redo_invalidates_clip_refs(&self) -> bool {
        self.redo
            .last()
            .is_some_and(|step| step.invalidates_clip_refs)
    }

    pub fn undo_track_mapping(&self) -> Option<&[usize]> {
        self.undo.last()?.track_mapping.as_deref()
    }

    pub fn redo_track_mapping(&self) -> Option<&[usize]> {
        self.redo.last()?.track_mapping.as_deref()
    }

    pub fn undo(&mut self) -> Option<String> {
        let step = self.undo.pop()?;
        let label = step.label.clone();
        let current = std::mem::replace(&mut self.project, step.project);
        self.redo.push(Snapshot {
            project: current,
            version: self.version,
            label: step.label,
            invalidates_clip_refs: step.invalidates_clip_refs,
            track_mapping: step.track_mapping.as_deref().map(invert_mapping),
        });
        self.version = step.version;
        self.revision += 1;
        self.coalescing = None;
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let step = self.redo.pop()?;
        let label = step.label.clone();
        let current = std::mem::replace(&mut self.project, step.project);
        self.undo.push(Snapshot {
            project: current,
            version: self.version,
            label: step.label,
            invalidates_clip_refs: step.invalidates_clip_refs,
            track_mapping: step.track_mapping.as_deref().map(invert_mapping),
        });
        self.version = step.version;
        self.revision += 1;
        self.coalescing = None;
        Some(label)
    }
}

fn invert_mapping(mapping: &[usize]) -> Vec<usize> {
    let mut inverse = vec![0; mapping.len()];
    for (old, &new) in mapping.iter().enumerate() {
        inverse[new] = old;
    }
    inverse
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bpm(document: &Document) -> f64 {
        document.project().bpm()
    }

    #[test]
    fn edits_undo_and_redo_in_order() {
        let mut document = Document::new(Project::new(), None);
        assert!(!document.is_dirty());
        document
            .edit("Tempo", |project| {
                project.set_bpm(100.0).map_err(|e| e.to_string())
            })
            .unwrap();
        document
            .edit("Tempo", |project| {
                project.set_bpm(90.0).map_err(|e| e.to_string())
            })
            .unwrap();
        assert!(document.is_dirty());
        assert_eq!(document.undo_label(), Some("Tempo"));
        document.undo();
        assert_eq!(bpm(&document), 100.0);
        document.undo();
        assert_eq!(bpm(&document), 120.0);
        assert!(document.undo().is_none());
        assert!(!document.is_dirty());
        document.redo();
        assert_eq!(bpm(&document), 100.0);
        document
            .edit("Tempo", |project| {
                project.set_bpm(80.0).map_err(|e| e.to_string())
            })
            .unwrap();
        assert!(document.redo().is_none());
    }

    #[test]
    fn failed_edits_change_nothing() {
        let mut document = Document::new(Project::new(), None);
        let revision = document.revision();
        let result = document.edit("Add and fail", |project| {
            project.add_audio_track("Half-made");
            Err::<(), _>("refused".to_owned())
        });
        assert_eq!(result, Err("refused".to_owned()));
        assert_eq!(document.project().tracks().len(), 1);
        assert_eq!(document.revision(), revision);
        assert!(document.undo_label().is_none());
    }

    #[test]
    fn coalesced_edits_share_one_undo_step() {
        let mut document = Document::new(Project::new(), None);
        for value in [100.0, 101.0, 102.0] {
            document
                .edit_coalesced("bpm", "Tempo", |project| {
                    project.set_bpm(value).map_err(|e| e.to_string())
                })
                .unwrap();
        }
        document.break_coalescing();
        document
            .edit_coalesced("bpm", "Tempo", |project| {
                project.set_bpm(140.0).map_err(|e| e.to_string())
            })
            .unwrap();
        document.undo();
        assert_eq!(bpm(&document), 102.0);
        document.undo();
        assert_eq!(bpm(&document), 120.0);
    }

    #[test]
    fn structural_history_metadata_survives_undo_redo_and_breaks_coalescing() {
        let mut document = Document::new(Project::new(), None);
        assert!(!document.undo_invalidates_clip_refs());
        assert!(!document.redo_invalidates_clip_refs());
        for value in [100.0, 101.0] {
            document
                .edit_coalesced("bpm", "Edit", |project| {
                    project.set_bpm(value).map_err(|e| e.to_string())
                })
                .unwrap();
        }
        document
            .edit_structural("Edit", |project| {
                project.add_midi_track("Keys");
                Ok(())
            })
            .unwrap();
        assert!(document.undo_invalidates_clip_refs());
        for value in [90.0, 91.0] {
            document
                .edit_coalesced("bpm", "Edit", |project| {
                    project.set_bpm(value).map_err(|e| e.to_string())
                })
                .unwrap();
        }

        assert!(!document.undo_invalidates_clip_refs());
        document.undo();
        assert_eq!(bpm(&document), 101.0);
        assert!(!document.redo_invalidates_clip_refs());
        assert!(document.undo_invalidates_clip_refs());
        document.undo();
        assert_eq!(document.project().tracks().len(), 1);
        assert!(document.redo_invalidates_clip_refs());
        assert!(!document.undo_invalidates_clip_refs());

        document.redo();
        assert_eq!(document.project().tracks().len(), 2);
        assert!(document.undo_invalidates_clip_refs());
        assert!(!document.redo_invalidates_clip_refs());
        document.redo();
        assert_eq!(bpm(&document), 91.0);
        assert!(!document.undo_invalidates_clip_refs());
        document.undo();
        document.undo();
        document.undo();
        assert_eq!(bpm(&document), 120.0);
        assert!(document.undo().is_none());
    }

    #[test]
    fn saving_tracks_dirty_state_through_undo() {
        let mut document = Document::new(Project::new(), None);
        document
            .edit("Track", |project| {
                project.add_audio_track("A");
                Ok(())
            })
            .unwrap();
        document.mark_saved(PathBuf::from("song.symp"));
        assert!(!document.is_dirty());
        assert_eq!(document.display_name(), "song.symp");
        document.undo();
        assert!(document.is_dirty());
        document.redo();
        assert!(!document.is_dirty());
    }

    #[test]
    fn external_state_changes_do_not_request_a_render_update_or_modify_history() {
        let mut document = Document::new(Project::new(), Some(PathBuf::from("song.symp")));
        document
            .edit("Tempo", |project| {
                project.set_bpm(100.0).map_err(|e| e.to_string())
            })
            .unwrap();
        document.undo();
        assert!(!document.is_dirty());
        let revision = document.revision();
        document.mark_external_state_dirty();
        document.mark_external_state_dirty();
        assert!(document.is_dirty());
        assert_eq!(document.revision(), revision);
        assert_eq!(document.project().bpm(), 120.0);
        assert!(document.undo_label().is_none());
        assert_eq!(document.redo_label(), Some("Tempo"));
        document.redo();
        assert_eq!(document.project().bpm(), 100.0);
    }

    #[test]
    fn captured_plugin_state_survives_the_next_edit_and_its_undo() {
        use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind};
        use sympoie::track::TrackId;

        let mut project = Project::new();
        let track = project.add_audio_track("Source");
        let id = project
            .add_effect(
                TrackId::Audio(track),
                PluginInstance::new(
                    PluginDescriptor {
                        path: "/fake/Test.vst3".into(),
                        class_id: "test".into(),
                        name: "Test".into(),
                        vendor: "Tests".into(),
                        kind: PluginKind::Effect,
                    },
                    vec![],
                    vec![1],
                ),
            )
            .unwrap();
        let mut document = Document::new(project, Some("song.symp".into()));
        document.mark_external_state_dirty();
        let revision = document.revision();
        document
            .capture_plugin_states(vec![(id, vec![2, 3])])
            .unwrap();
        assert_eq!(document.revision(), revision);
        assert!(document.undo_label().is_none());
        assert!(document.is_dirty());
        assert!(
            document
                .capture_plugin_states(vec![(id, vec![4]), (id + 1, vec![])])
                .is_err()
        );
        assert_eq!(document.project().plugin(id).unwrap().state, [2, 3]);
        document
            .edit_structural("Remove effect", |project| {
                project
                    .remove_effect(TrackId::Audio(track), 0)
                    .map_err(|error| error.to_string())
            })
            .unwrap();
        assert!(document.project().plugin(id).is_none());
        document.undo();
        assert_eq!(document.project().plugin(id).unwrap().state, [2, 3]);
        document.redo();
        assert!(document.project().plugin(id).is_none());
    }

    #[test]
    fn external_dirty_state_survives_model_undo_redo_and_is_cleared_only_on_save() {
        let path = PathBuf::from("song.symp");
        let mut document = Document::new(Project::new(), Some(path.clone()));
        document
            .edit("Tempo", |project| {
                project.set_bpm(100.0).map_err(|e| e.to_string())
            })
            .unwrap();
        document.mark_saved(path.clone());
        document.mark_external_state_dirty();
        document.undo();
        assert!(document.is_dirty());
        document.redo();
        assert!(
            document.is_dirty(),
            "returning to the saved model cannot clear plugin state changes"
        );
        let revision = document.revision();
        document.mark_saved(path.clone());
        assert!(!document.is_dirty());
        assert_eq!(document.revision(), revision);
        document.undo();
        assert!(
            document.is_dirty(),
            "normal model history is still tracked after the save"
        );
        document.redo();
        assert!(!document.is_dirty());
        document.mark_external_state_dirty();
        assert!(
            document.is_dirty(),
            "a later native edit needs another save"
        );
        document.mark_saved(path);
        assert!(!document.is_dirty());
    }
}
