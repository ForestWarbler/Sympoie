use sympoie::plugin::{PluginDescriptor, PluginKind};
use sympoie::project::Project;
use sympoie::track::Track;

use crate::actions::Action;

#[derive(Clone, Debug)]
pub enum BrowserDevice {
    Plugin(PluginDescriptor),
}

impl BrowserDevice {
    pub fn name(&self) -> &str {
        match self {
            Self::Plugin(plugin) => &plugin.name,
        }
    }

    pub fn is_instrument(&self) -> bool {
        match self {
            Self::Plugin(plugin) => plugin.kind == PluginKind::Instrument,
        }
    }

    pub fn can_load_on(&self, project: &Project, track: usize) -> bool {
        project
            .tracks()
            .get(track)
            .is_some_and(|track| !self.is_instrument() || matches!(track, Track::Midi(_)))
    }

    pub fn load_action(&self, track: usize, before: Option<u64>) -> Action {
        match self {
            Self::Plugin(plugin) if plugin.kind == PluginKind::Effect && before.is_some() => {
                Action::InsertVst3Before {
                    track,
                    plugin: plugin.clone(),
                    before: before.expect("checked above"),
                }
            }
            Self::Plugin(plugin) => Action::LoadVst3 {
                track,
                plugin: plugin.clone(),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum DeviceDrag {
    Browser(BrowserDevice),
    Effect { track: usize, id: u64 },
}
