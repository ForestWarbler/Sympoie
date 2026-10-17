use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub audio: AudioSettings,
    pub general: GeneralSettings,
    pub agent: AgentSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub projects: Vec<PathBuf>,
}

impl AgentSettings {
    pub fn remembers(&self, folder: &Path) -> bool {
        self.projects
            .iter()
            .any(|project| same_folder(project, folder))
    }

    pub fn remember(&mut self, folder: &Path, on: bool) {
        self.projects
            .retain(|project| !same_folder(project, folder));
        if on {
            self.projects.push(folder.to_owned());
        }
    }
}

fn same_folder(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (left.canonicalize(), right.canonicalize()),
            (Ok(left), Ok(right)) if left == right
        )
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub driver: Option<String>,
    pub output_device: Option<String>,
    pub sample_rate: Option<u32>,
    pub buffer_frames: Option<u32>,
    pub first_output_channel: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub interface_scale: f32,
    pub default_tempo: f64,
    pub default_beats_per_bar: u8,
    pub follow_playback: bool,
    pub scan_plugins_at_startup: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            interface_scale: 1.0,
            default_tempo: 120.0,
            default_beats_per_bar: 4,
            follow_playback: true,
            scan_plugins_at_startup: true,
        }
    }
}

pub const MIN_SCALE: f32 = 0.75;
pub const MAX_SCALE: f32 = 1.5;

impl Settings {
    pub fn sanitized(mut self) -> Self {
        let general = &mut self.general;
        if !general.interface_scale.is_finite() {
            general.interface_scale = 1.0;
        }
        general.interface_scale = general.interface_scale.clamp(MIN_SCALE, MAX_SCALE);
        if !general.default_tempo.is_finite() {
            general.default_tempo = 120.0;
        }
        general.default_tempo = general.default_tempo.clamp(20.0, 999.0);
        general.default_beats_per_bar = general.default_beats_per_bar.clamp(1, 32);
        if self.audio.buffer_frames == Some(0) {
            self.audio.buffer_frames = None;
        }
        if self.audio.sample_rate == Some(0) {
            self.audio.sample_rate = None;
        }
        self
    }
}

pub fn user_settings_path() -> Result<PathBuf, String> {
    let nonempty_env = |key| std::env::var_os(key).filter(|value| !value.is_empty());
    #[cfg(target_os = "macos")]
    let root = nonempty_env("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/Sympoie"));
    #[cfg(target_os = "windows")]
    let root = nonempty_env("APPDATA").map(|home| PathBuf::from(home).join("Sympoie"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let root = nonempty_env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| nonempty_env("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|root| root.join("sympoie"));
    root.map(|root| root.join("settings.json"))
        .ok_or_else(|| "Could not locate the user settings folder".to_owned())
}

pub fn load(path: &Path) -> (Settings, Option<String>) {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => (settings.sanitized(), None),
            Err(error) => (
                Settings::default(),
                Some(format!(
                    "Settings in {} could not be read ({error}); using defaults",
                    path.display()
                )),
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(error) => (
            Settings::default(),
            Some(format!("Could not read {}: {error}", path.display())),
        ),
    }
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(settings)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    };
    write().map_err(|error| format!("Could not save settings to {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("sympoie-settings-{}-{name}", std::process::id()))
            .join("settings.json")
    }

    #[test]
    fn settings_round_trip_through_their_file() {
        let path = temp_path("round-trip");
        let mut settings = Settings::default();
        settings.audio.driver = Some("CoreAudio".into());
        settings.audio.output_device = Some("Studio Interface".into());
        settings.audio.sample_rate = Some(96_000);
        settings.audio.buffer_frames = Some(128);
        settings.audio.first_output_channel = 2;
        settings.general.interface_scale = 1.25;
        settings.general.default_tempo = 90.0;
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings, None));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_partial_and_broken_files_fall_back_to_defaults() {
        let path = temp_path("fallbacks");
        assert_eq!(load(&path), (Settings::default(), None));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"general":{"default_tempo":5000,"interface_scale":9}}"#,
        )
        .unwrap();
        let (settings, error) = load(&path);
        assert!(error.is_none());
        assert_eq!(settings.general.default_tempo, 999.0);
        assert_eq!(settings.general.interface_scale, MAX_SCALE);
        assert!(settings.general.follow_playback);
        assert_eq!(settings.audio, AudioSettings::default());
        fs::write(&path, "not json").unwrap();
        let (settings, error) = load(&path);
        assert_eq!(settings, Settings::default());
        assert!(error.unwrap().contains("could not be read"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
