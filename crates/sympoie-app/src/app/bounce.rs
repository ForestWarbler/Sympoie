use sympoie::bounce::{BounceMode, BouncePlan, apply_bounce};

use super::{SympoieApp, track_plugin_ids};
use crate::audio::bounce::BounceJob;
use crate::document::edit::ClipRef;

pub(super) struct TrackBounce {
    pub job: BounceJob,
    pub name: String,
    revision: u64,
    external_revision: u64,
    mode: BounceMode,
    plugin_states: Vec<(u64, Vec<u8>)>,
}

impl SympoieApp {
    pub(super) fn start_bounce(&mut self, track: usize, duplicate: bool) {
        if self.bounce.is_some() || self.export.is_some() {
            self.warn("Wait for the current bounce or export, or cancel it first.".into());
            return;
        }
        if let Err(error) = self.flush_plugin_edits() {
            self.error(format!("Couldn't capture plugin edits for bounce: {error}"));
            return;
        }
        let mode = if duplicate {
            BounceMode::Duplicate
        } else {
            BounceMode::InPlace
        };
        let prepared = self.project_with_plugin_state().and_then(|project| {
            let plan = BouncePlan::new(&project, track)?;
            plan.validate_mode(mode)?;
            let states = project
                .tracks()
                .iter()
                .flat_map(track_plugin_ids)
                .map(|id| (id, project.plugin(id).unwrap().state.clone()))
                .collect();
            let plugins = self.plugins.prepare_offline(&plan.project)?;
            Ok((BounceJob::start(plan, plugins)?, states))
        });
        match prepared {
            Ok((job, plugin_states)) => {
                self.bounce = Some(TrackBounce {
                    job,
                    name: self.document.project().tracks()[track].name().to_owned(),
                    revision: self.document.revision(),
                    external_revision: self.document.external_state_revision(),
                    mode,
                    plugin_states,
                });
            }
            Err(error) => self.error(format!("Couldn't bounce track: {error}")),
        }
    }

    pub(super) fn poll_bounce(&mut self) {
        let Some(result) = self.bounce.as_mut().and_then(|pending| pending.job.poll()) else {
            return;
        };
        let pending = self.bounce.take().expect("a completed bounce");
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.warn(format!("Bounce stopped: {error}"));
                return;
            }
        };
        if let Err(error) = self.flush_plugin_edits() {
            self.error(format!(
                "Bounce discarded because plugin edits could not be synchronized: {error}"
            ));
            return;
        }
        if pending.revision != self.document.revision()
            || pending.external_revision != self.document.external_state_revision()
        {
            self.warn("Bounce discarded because the project changed. Bounce the track again to include your edits.".into());
            return;
        }
        let label = match pending.mode {
            BounceMode::InPlace => "Bounce Track in Place",
            BounceMode::Duplicate => "Duplicate and Bounce Track",
        };
        if let Err(error) = self.document.capture_plugin_states(pending.plugin_states) {
            self.error(format!("Couldn't preserve plugin state: {error}"));
            return;
        }
        if let Some(track) = self.edit_structural(label, |project| {
            apply_bounce(project, &result.plan, result.audio, pending.mode)
        }) {
            self.plugin_editors.clear();
            self.arrangement.folded.clear();
            self.arrangement.track_heights.clear();
            self.arrangement.renaming = None;
            self.plugin_browser.open = false;
            self.piano_roll.close();
            self.selection.select_clip(ClipRef { track, index: 0 });
            self.info(match pending.mode {
                BounceMode::InPlace => format!("Bounced {} in place", pending.name),
                BounceMode::Duplicate => {
                    format!("Bounced a copy of {}; original track muted", pending.name)
                }
            });
        }
    }
}
