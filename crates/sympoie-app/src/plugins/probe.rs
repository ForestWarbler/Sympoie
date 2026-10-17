use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sympoie::plugin::{PluginDescriptor, PluginKind};

use super::load_plugin;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ProbePlugin {
    pub(super) path: PathBuf,
    pub(super) class_id: String,
    pub(super) name: String,
    pub(super) vendor: String,
    pub(super) instrument: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct ProbeReport {
    pub(super) plugins: Vec<ProbePlugin>,
    pub(super) failures: Vec<String>,
}

pub fn run_probe(path: &Path, output: &Path) -> Result<(), String> {
    let details = vst3_host::get_detailed_plugin_info(path).map_err(|e| e.to_string())?;
    let mut report = ProbeReport::default();
    for class in details
        .classes
        .iter()
        .filter(|class| class.category == "Audio Module Class")
    {
        let info = if class.class_id == details.info.uid {
            details.info.clone()
        } else {
            let descriptor = PluginDescriptor {
                path: path.to_owned(),
                class_id: class.class_id.clone(),
                name: class.name.clone(),
                vendor: details.info.vendor.clone(),
                kind: PluginKind::Effect,
            };
            match load_plugin(&descriptor) {
                Ok(plugin) => plugin.info().clone(),
                Err(error) => {
                    report.failures.push(error);
                    continue;
                }
            }
        };
        if info.audio_outputs == 0 {
            report
                .failures
                .push(format!("{} has no audio output", info.name));
            continue;
        }
        report.plugins.push(ProbePlugin {
            path: path.to_owned(),
            class_id: class.class_id.clone(),
            name: info.name,
            vendor: info.vendor,
            instrument: info
                .category
                .split('|')
                .any(|category| category == "Instrument")
                || (info.has_midi_input && info.audio_inputs == 0),
        });
    }
    if report.plugins.is_empty() && report.failures.is_empty() {
        return Err("The bundle exports no VST3 audio classes".into());
    }
    let bytes = serde_json::to_vec(&report).map_err(|e| e.to_string())?;
    std::fs::write(output, bytes).map_err(|e| e.to_string())
}
