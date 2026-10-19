//! Output configurations for the Export window: what to render, over which
//! range, into which file format, and where. They are kept in the user
//! settings together with named presets and the set used by the last export.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sympoie::SAMPLE_RATE;
use sympoie::audio_file::{BitDepth, Encoding, FileFormat, MP3_BITRATES, SAMPLE_RATES};
use sympoie::project::Project;
use sympoie::stems::StemsPlan;
use sympoie::track::Track;

use crate::audio::export::ExportTask;
use crate::document::edit;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportSource {
    /// The master output.
    #[default]
    Mix,
    /// One file per audio or MIDI track, each soloed through the mix.
    Tracks,
    /// A new project with every audio and MIDI track rendered to audio.
    StemsProject,
}

impl ExportSource {
    pub const ALL: [Self; 3] = [Self::Mix, Self::Tracks, Self::StemsProject];

    pub fn label(self) -> &'static str {
        match self {
            Self::Mix => "Master mix",
            Self::Tracks => "Each track",
            Self::StemsProject => "Stems project",
        }
    }

    pub fn default_file_name(self) -> &'static str {
        match self {
            Self::Mix => "{project}",
            Self::Tracks => "{project} - {track}",
            Self::StemsProject => "{project} Stems",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportRange {
    #[default]
    Project,
    Loop,
    Selection,
}

impl ExportRange {
    pub const ALL: [Self; 3] = [Self::Project, Self::Loop, Self::Selection];

    pub fn label(self) -> &'static str {
        match self {
            Self::Project => "Whole project",
            Self::Loop => "Loop region",
            Self::Selection => "Time selection",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    #[default]
    Wav,
    Aiff,
    Flac,
    Mp3,
}

impl ExportFormat {
    pub const ALL: [Self; 4] = [Self::Wav, Self::Aiff, Self::Flac, Self::Mp3];

    pub fn file_format(self) -> FileFormat {
        match self {
            Self::Wav => FileFormat::Wav,
            Self::Aiff => FileFormat::Aiff,
            Self::Flac => FileFormat::Flac,
            Self::Mp3 => FileFormat::Mp3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportDepth {
    Int16,
    #[default]
    Int24,
    Float32,
}

impl ExportDepth {
    pub const ALL: [Self; 3] = [Self::Int16, Self::Int24, Self::Float32];

    pub fn bit_depth(self) -> BitDepth {
        match self {
            Self::Int16 => BitDepth::Int16,
            Self::Int24 => BitDepth::Int24,
            Self::Float32 => BitDepth::Float32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportConfig {
    pub name: String,
    /// Included when exporting all configurations.
    pub enabled: bool,
    pub source: ExportSource,
    pub range: ExportRange,
    pub format: ExportFormat,
    pub depth: ExportDepth,
    pub sample_rate: u32,
    pub dither: bool,
    /// Constant bitrate of MP3 files, in kbit/s.
    pub mp3_bitrate: u32,
    /// The file name without extension; `{project}`, `{track}` and `{name}`
    /// are replaced by the project, track and configuration names.
    pub file_name: String,
    /// `None` exports next to the project.
    pub folder: Option<PathBuf>,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            name: "Master".into(),
            enabled: true,
            source: ExportSource::Mix,
            range: ExportRange::Project,
            format: ExportFormat::Wav,
            depth: ExportDepth::Int24,
            sample_rate: SAMPLE_RATE,
            dither: false,
            mp3_bitrate: 320,
            file_name: ExportSource::Mix.default_file_name().into(),
            folder: None,
        }
    }
}

impl ExportConfig {
    pub fn encoding(&self) -> Encoding {
        Encoding {
            format: self.format.file_format(),
            depth: self.depth.bit_depth(),
            sample_rate: self.sample_rate,
            dither: self.dither
                && self.depth != ExportDepth::Float32
                && self.format != ExportFormat::Mp3,
            bitrate_kbps: self.mp3_bitrate,
        }
    }

    /// The bit depth, or an MP3's bitrate.
    pub fn quality_label(&self) -> String {
        let format = self.format.file_format();
        if self.source == ExportSource::StemsProject {
            BitDepth::Float32.label().to_owned()
        } else if format.has_bit_depth() {
            self.depth.bit_depth().label().to_owned()
        } else {
            format!("{} kbps", self.mp3_bitrate)
        }
    }

    /// The sample rate of the files written.
    pub fn rate(&self) -> u32 {
        if self.source == ExportSource::StemsProject {
            SAMPLE_RATE
        } else {
            self.sample_rate
        }
    }

    /// The file format at a glance, as "WAV · 24-bit · 48 kHz".
    pub fn format_line(&self) -> String {
        let format = if self.source == ExportSource::StemsProject {
            "Project"
        } else {
            self.format.file_format().label()
        };
        format!(
            "{format} · {} · {}",
            self.quality_label(),
            sample_rate_label(self.rate())
        )
    }

    /// What is rendered, as "Mix · Whole project".
    pub fn render_line(&self) -> String {
        if self.source == ExportSource::StemsProject {
            return format!("{} · Whole project", self.source.label());
        }
        format!("{} · {}", self.source.label(), self.range.label())
    }

    pub fn summary(&self) -> String {
        format!("{} · {}", self.render_line(), self.format_line())
    }

    pub fn sanitized(mut self) -> Self {
        let format = self.format.file_format();
        if !SAMPLE_RATES.contains(&self.sample_rate) || !format.supports_rate(self.sample_rate) {
            self.sample_rate = SAMPLE_RATE;
        }
        if !format.supports(self.depth.bit_depth()) {
            self.depth = ExportDepth::Int24;
        }
        if !MP3_BITRATES.contains(&self.mp3_bitrate) {
            self.mp3_bitrate = 320;
        }
        if self.file_name.trim().is_empty() {
            self.file_name = self.source.default_file_name().into();
        }
        if self.name.trim().is_empty() {
            self.name = self.source.label().into();
        }
        self
    }
}

pub fn sample_rate_label(rate: u32) -> String {
    let khz = f64::from(rate) / 1000.0;
    if rate.is_multiple_of(1000) {
        format!("{khz:.0} kHz")
    } else {
        format!("{khz:.1} kHz")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub config: ExportConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// The configurations used by the most recent export.
    pub last: Vec<ExportConfig>,
    pub presets: Vec<ExportPreset>,
}

impl ExportSettings {
    pub fn sanitized(mut self) -> Self {
        self.last = self.last.into_iter().map(ExportConfig::sanitized).collect();
        for preset in &mut self.presets {
            preset.config = std::mem::take(&mut preset.config).sanitized();
        }
        self.presets.retain(|preset| !preset.name.trim().is_empty());
        self
    }

    /// Saves `config` under its name, replacing a preset of the same name.
    pub fn save_preset(&mut self, config: &ExportConfig) {
        let name = config.name.trim().to_owned();
        let preset = ExportPreset {
            name: name.clone(),
            config: config.clone(),
        };
        match self.presets.iter_mut().find(|preset| preset.name == name) {
            Some(existing) => *existing = preset,
            None => self.presets.push(preset),
        }
    }
}

/// What an export needs to know about the open document.
pub struct ExportContext<'a> {
    pub project: &'a Project,
    pub project_name: &'a str,
    /// Where configurations without their own folder export to.
    pub default_folder: Option<&'a Path>,
    pub loop_range: Option<(u64, u64)>,
    pub selection: Option<(u64, u64)>,
}

/// Turns the configurations into the files to write, in order.
pub fn plan_exports(
    context: &ExportContext<'_>,
    configs: &[ExportConfig],
) -> Result<Vec<ExportTask>, String> {
    let mut tasks = Vec::new();
    let mut used = HashSet::new();
    for config in configs.iter().filter(|config| config.enabled) {
        let fail = |error: String| format!("{}: {error}", config.name);
        let folder = config
            .folder
            .as_deref()
            .or(context.default_folder)
            .ok_or_else(|| fail("choose a folder to export to".into()))?;
        let name = |track: Option<&str>| {
            let mut template = config.file_name.clone();
            if track.is_some() && !template.contains("{track}") {
                template.push_str(" - {track}");
            }
            file_name(&template, context.project_name, track, &config.name)
        };
        if config.source == ExportSource::StemsProject {
            let plan = StemsPlan::new(context.project).map_err(fail)?;
            let path = unique_path(folder, &name(None), "symp", &mut used, true);
            tasks.push(ExportTask::Stems { plan, path });
            continue;
        }
        config.encoding().validate().map_err(fail)?;
        let (start, end) = range(context, config.range).map_err(fail)?;
        let tempo = context.project.tempo();
        let start_frame = tempo
            .ticks_to_frames(start)
            .map_err(|error| fail(error.to_string()))?;
        let frames = match config.range {
            ExportRange::Project => context
                .project
                .duration_frames()
                .map_err(|error| fail(error.to_string()))?,
            _ => tempo
                .ticks_to_frames(end)
                .map_err(|error| fail(error.to_string()))?
                .saturating_sub(start_frame),
        };
        if frames == 0 {
            return Err(fail("there is no audio to export".into()));
        }
        let extension = config.format.file_format().extension();
        let mut audio = |project: Project, track: Option<&str>| {
            let path = unique_path(folder, &name(track), extension, &mut used, false);
            tasks.push(ExportTask::Audio {
                project,
                path,
                start_frame,
                frames,
                encoding: config.encoding(),
            });
        };
        match config.source {
            ExportSource::Mix => audio(context.project.clone(), None),
            ExportSource::Tracks => {
                let mut any = false;
                for (index, track) in context.project.tracks().iter().enumerate() {
                    if !plays(track) {
                        continue;
                    }
                    any = true;
                    audio(
                        soloed(context.project, index).map_err(fail)?,
                        Some(track.name()),
                    );
                }
                if !any {
                    return Err(fail("no unmuted track has anything to play".into()));
                }
            }
            ExportSource::StemsProject => unreachable!("handled above"),
        }
    }
    if tasks.is_empty() {
        return Err("Turn on at least one configuration to export.".into());
    }
    Ok(tasks)
}

fn range(context: &ExportContext<'_>, range: ExportRange) -> Result<(u64, u64), String> {
    match range {
        ExportRange::Project => Ok((0, 0)),
        ExportRange::Loop => context
            .loop_range
            .filter(|(start, end)| end > start)
            .ok_or_else(|| "turn on the loop to export the loop region".into()),
        ExportRange::Selection => context
            .selection
            .filter(|(start, end)| end > start)
            .ok_or_else(|| "select a time range to export it".into()),
    }
}

fn plays(track: &Track) -> bool {
    !track.mixer().muted()
        && match track {
            Track::Audio(audio) => !audio.clips().is_empty(),
            Track::Midi(midi) => {
                (midi.instrument().is_some() || midi.plugin_instrument().is_some())
                    && !midi.clips().is_empty()
            }
            _ => false,
        }
}

/// The project with only `index` soloed.
fn soloed(project: &Project, index: usize) -> Result<Project, String> {
    let mut soloed = project.clone();
    for track in 1..soloed.tracks().len() {
        let mixer = soloed.tracks()[track].mixer().with_soloed(track == index);
        if mixer != *soloed.tracks()[track].mixer() {
            edit::set_mixer(&mut soloed, track, mixer)?;
        }
    }
    Ok(soloed)
}

pub fn file_name(template: &str, project: &str, track: Option<&str>, config: &str) -> String {
    let name = template
        .replace("{project}", project)
        .replace("{track}", track.unwrap_or(""))
        .replace("{name}", config);
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        "Export".into()
    } else {
        cleaned.to_owned()
    }
}

/// A path for `stem.extension` in `folder` that no earlier export in this
/// batch uses. Existing files are replaced, unless `keep_existing` asks for a
/// fresh name, as for project folders.
fn unique_path(
    folder: &Path,
    stem: &str,
    extension: &str,
    used: &mut HashSet<PathBuf>,
    keep_existing: bool,
) -> PathBuf {
    let mut counter = 1;
    loop {
        let name = if counter == 1 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem} {counter}.{extension}")
        };
        let path = folder.join(name);
        if !used.contains(&path) && !(keep_existing && path.exists()) {
            used.insert(path.clone());
            return path;
        }
        counter += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sympoie::clip::AudioClip;
    use sympoie::project::AudioBuffer;
    use sympoie::track::MixerSettings;

    fn project() -> Project {
        let mut project = Project::new();
        let asset = project.add_asset(AudioBuffer::new(vec![[0.5; 2]; 9600]).unwrap());
        for name in ["Bass", "Drums", "Muted"] {
            let track = project.add_audio_track(name);
            project
                .add_audio_clip(
                    track,
                    AudioClip {
                        asset,
                        source_start: 0,
                        start_tick: 0,
                        frame_count: 9600,
                        gain: 1.0,
                        pitch_semitones: 0.0,
                        pitch_algorithm: Default::default(),
                        preserve_formants: false,
                        name: None,
                    },
                )
                .unwrap();
            if name == "Muted" {
                project
                    .set_audio_track_mixer(track, MixerSettings::default().with_muted(true))
                    .unwrap();
            }
        }
        project.add_group_track("Group");
        project
    }

    fn context<'a>(project: &'a Project, folder: &'a Path) -> ExportContext<'a> {
        ExportContext {
            project,
            project_name: "Song: Final",
            default_folder: Some(folder),
            loop_range: None,
            selection: Some((960, 1920)),
        }
    }

    #[test]
    fn templates_fill_in_names_and_make_safe_file_names() {
        assert_eq!(
            file_name("{project} - {track} ({name})", "A/B", Some("Kick"), "Mix"),
            "A-B - Kick (Mix)"
        );
        assert_eq!(file_name(" ../{track} ", "P", Some(""), "X"), "-");
        assert_eq!(file_name("{track}", "P", None, "X"), "Export");
    }

    #[test]
    fn configurations_become_one_task_per_output_file() {
        let project = project();
        let folder = std::env::temp_dir();
        let configs = vec![
            ExportConfig::default(),
            ExportConfig {
                name: "Selection".into(),
                range: ExportRange::Selection,
                format: ExportFormat::Aiff,
                depth: ExportDepth::Int16,
                sample_rate: 44_100,
                dither: true,
                ..ExportConfig::default()
            },
            ExportConfig {
                name: "Tracks".into(),
                source: ExportSource::Tracks,
                file_name: "{project}".into(),
                ..ExportConfig::default()
            },
            ExportConfig {
                enabled: false,
                ..ExportConfig::default()
            },
        ];
        let tasks = plan_exports(&context(&project, &folder), &configs).unwrap();
        let summary: Vec<_> = tasks
            .iter()
            .map(|task| match task {
                ExportTask::Audio {
                    path,
                    start_frame,
                    frames,
                    encoding,
                    ..
                } => (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    *start_frame,
                    *frames,
                    encoding.sample_rate,
                ),
                ExportTask::Stems { .. } => panic!("no stems were asked for"),
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Song- Final.wav".to_owned(), 0, 9600, 48_000),
                ("Song- Final.aiff".to_owned(), 24_000, 24_000, 44_100),
                ("Song- Final - Bass.wav".to_owned(), 0, 9600, 48_000),
                ("Song- Final - Drums.wav".to_owned(), 0, 9600, 48_000),
            ]
        );
        let ExportTask::Audio {
            project: soloed, ..
        } = &tasks[3]
        else {
            unreachable!()
        };
        let soloed: Vec<_> = soloed
            .tracks()
            .iter()
            .map(|track| track.mixer().soloed())
            .collect();
        assert_eq!(soloed, [false, false, true, false, false]);
    }

    #[test]
    fn missing_ranges_folders_and_empty_sets_are_explained() {
        let project = project();
        let folder = std::env::temp_dir();
        let loop_config = ExportConfig {
            name: "Loop".into(),
            range: ExportRange::Loop,
            ..ExportConfig::default()
        };
        let error = plan_exports(&context(&project, &folder), &[loop_config]).unwrap_err();
        assert!(error.starts_with("Loop: turn on the loop"), "{error}");
        let mut context = context(&project, &folder);
        context.default_folder = None;
        let error = plan_exports(&context, &[ExportConfig::default()]).unwrap_err();
        assert!(error.contains("choose a folder"), "{error}");
        let disabled = ExportConfig {
            enabled: false,
            ..ExportConfig::default()
        };
        assert!(plan_exports(&context, &[disabled]).is_err());
    }

    #[test]
    fn duplicate_names_get_numbered_and_stems_projects_never_replace_a_folder() {
        let project = project();
        let folder =
            std::env::temp_dir().join(format!("sympoie-export-plan-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("Song- Final Stems.symp")).unwrap();
        let stems = ExportConfig {
            source: ExportSource::StemsProject,
            file_name: ExportSource::StemsProject.default_file_name().into(),
            ..ExportConfig::default()
        };
        let configs = [ExportConfig::default(), ExportConfig::default(), stems];
        let tasks = plan_exports(&context(&project, &folder), &configs).unwrap();
        let names: Vec<_> = tasks
            .iter()
            .map(|task| {
                task.path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "Song- Final.wav",
                "Song- Final 2.wav",
                "Song- Final Stems 2.symp"
            ]
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn settings_repair_impossible_combinations_and_replace_presets_by_name() {
        let broken = ExportConfig {
            name: " ".into(),
            format: ExportFormat::Aiff,
            depth: ExportDepth::Float32,
            sample_rate: 12_345,
            file_name: String::new(),
            ..ExportConfig::default()
        }
        .sanitized();
        assert_eq!(broken.depth, ExportDepth::Int24);
        assert_eq!(broken.sample_rate, SAMPLE_RATE);
        assert_eq!(broken.file_name, "{project}");
        assert_eq!(broken.name, "Master mix");
        let mp3 = ExportConfig {
            format: ExportFormat::Mp3,
            sample_rate: 96_000,
            mp3_bitrate: 100,
            dither: true,
            ..ExportConfig::default()
        }
        .sanitized();
        assert_eq!((mp3.sample_rate, mp3.mp3_bitrate), (SAMPLE_RATE, 320));
        assert!(!mp3.encoding().dither, "MP3 is encoded from floats");
        assert_eq!(mp3.format_line(), "MP3 · 320 kbps · 48 kHz");
        let mut settings = ExportSettings::default();
        settings.save_preset(&ExportConfig::default());
        settings.save_preset(&ExportConfig {
            sample_rate: 96_000,
            ..ExportConfig::default()
        });
        assert_eq!(settings.presets.len(), 1);
        assert_eq!(settings.presets[0].config.sample_rate, 96_000);
        let text = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<ExportSettings>(&text).unwrap(),
            settings
        );
    }
}
