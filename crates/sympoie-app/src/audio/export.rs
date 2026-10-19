use std::fs::{self, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

use sympoie::audio_file::{AudioFileWriter, Encoding};
use sympoie::plugin::RenderQuality;
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::stems::StemsPlan;

use crate::plugins::PluginRuntime;

const BLOCK_FRAMES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub path: PathBuf,
    pub frames: u64,
    pub clipped_samples: u64,
}

/// Offline plug-in runtimes for one task: one for an audio file, one per rendered stem.
type TaskPlugins = Vec<Option<Arc<PluginRuntime>>>;

/// One file (or, for stems, one project folder) an export writes.
#[derive(Debug)]
pub enum ExportTask {
    Audio {
        project: Project,
        path: PathBuf,
        start_frame: u64,
        frames: u64,
        encoding: Encoding,
    },
    Stems {
        plan: StemsPlan,
        path: PathBuf,
    },
}

impl ExportTask {
    pub fn path(&self) -> &Path {
        match self {
            Self::Audio { path, .. } | Self::Stems { path, .. } => path,
        }
    }

    /// The engine frames rendering this task takes, for progress.
    fn work(&self) -> u64 {
        match self {
            Self::Audio { frames, .. } => *frames,
            Self::Stems { plan, .. } => plan
                .stems
                .iter()
                .filter_map(|stem| stem.render.as_ref())
                .map(|render| render.end_frame)
                .sum(),
        }
    }
}

/// Writes a list of exports one after another on a background thread.
pub struct ExportJob {
    names: Vec<String>,
    total: u64,
    /// Work finished in completed tasks.
    finished: Arc<AtomicU64>,
    /// Work finished in the current task.
    current: Arc<AtomicU64>,
    index: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<Vec<ExportSummary>, String>>>,
}

impl ExportJob {
    /// Prepares every task's plug-ins here, on the main thread, since VST3 plug-ins crash when
    /// created on the export thread; `host` services and releases them once the worker is done.
    pub fn start(tasks: Vec<ExportTask>, host: &PluginRuntime) -> Result<Self, String> {
        let tasks = tasks
            .into_iter()
            .map(|task| {
                let plugins = match &task {
                    ExportTask::Audio { project, .. } => vec![Some(host.prepare_offline(project)?)],
                    ExportTask::Stems { plan, .. } => plan
                        .stems
                        .iter()
                        .map(|stem| {
                            stem.render
                                .as_ref()
                                .map(|render| host.prepare_offline(&render.project))
                                .transpose()
                        })
                        .collect::<Result<_, String>>()?,
                };
                Ok((task, plugins))
            })
            .collect::<Result<Vec<(ExportTask, TaskPlugins)>, String>>()?;
        let names = tasks
            .iter()
            .map(|(task, _)| {
                task.path()
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
            })
            .collect();
        let total = tasks.iter().map(|(task, _)| task.work()).sum();
        let finished = Arc::new(AtomicU64::new(0));
        let current = Arc::new(AtomicU64::new(0));
        let index = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = {
            let (finished, current, index, cancel) = (
                Arc::clone(&finished),
                Arc::clone(&current),
                Arc::clone(&index),
                Arc::clone(&cancel),
            );
            thread::Builder::new()
                .name("sympoie-export".into())
                .spawn(move || {
                    let mut summaries = Vec::new();
                    for (position, (task, plugins)) in tasks.into_iter().enumerate() {
                        index.store(position, Ordering::Relaxed);
                        current.store(0, Ordering::Relaxed);
                        let work = task.work();
                        // Each task lets go of its plug-ins when it ends, so the main thread can release
                        // them without waiting for the rest of the export.
                        let summary = run(task, plugins, &current, &cancel).map_err(|error| {
                            if summaries.is_empty() {
                                error
                            } else {
                                format!(
                                    "{error} (after writing {} file{})",
                                    summaries.len(),
                                    if summaries.len() == 1 { "" } else { "s" }
                                )
                            }
                        })?;
                        summaries.push(summary);
                        finished.fetch_add(work, Ordering::Relaxed);
                    }
                    current.store(0, Ordering::Relaxed);
                    Ok(summaries)
                })
                .expect("the operating system refused to start the export thread")
        };
        Ok(Self {
            names,
            total,
            finished,
            current,
            index,
            cancel,
            thread: Some(thread),
        })
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            let done = self.finished.load(Ordering::Relaxed) + self.current.load(Ordering::Relaxed);
            (done as f64 / self.total as f64).min(1.0) as f32
        }
    }

    /// What is being written, as "name" or "name (2 of 5)".
    pub fn description(&self) -> String {
        let index = self
            .index
            .load(Ordering::Relaxed)
            .min(self.names.len().saturating_sub(1));
        let name = self.names.get(index).map_or("", String::as_str);
        if self.names.len() > 1 {
            format!("{name} ({} of {})", index + 1, self.names.len())
        } else {
            name.to_owned()
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&mut self) -> Option<Result<Vec<ExportSummary>, String>> {
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            let thread = self.thread.take()?;
            return Some(
                thread
                    .join()
                    .unwrap_or_else(|_| Err("the export thread crashed".to_owned())),
            );
        }
        None
    }
}

fn run(
    task: ExportTask,
    plugins: TaskPlugins,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<ExportSummary, String> {
    match task {
        ExportTask::Audio {
            project,
            path,
            start_frame,
            frames,
            encoding,
        } => export(
            &project,
            plugins
                .into_iter()
                .flatten()
                .next()
                .ok_or("export has no plug-in host")?,
            &path,
            start_frame,
            frames,
            encoding,
            progress,
            cancel,
        ),
        ExportTask::Stems { plan, path } => export_stems(&plan, plugins, &path, progress, cancel),
    }
}

/// Reports plug-in errors. Servicing the plug-ins is the main thread's job.
fn plugin_service(plugins: &PluginRuntime) -> Result<(), String> {
    let errors = plugins.take_errors();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[allow(clippy::too_many_arguments)]
fn export(
    project: &Project,
    plugins: Arc<PluginRuntime>,
    path: &Path,
    start_frame: u64,
    frames: u64,
    encoding: Encoding,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<ExportSummary, String> {
    let _pitch_pins = project
        .prepare_audio_pitch(|| cancel.load(Ordering::Relaxed))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "export cancelled".to_owned())?;
    let mut renderer =
        Renderer::with_plugin_host(project, &*plugins).map_err(|error| error.to_string())?;
    renderer.set_quality(RenderQuality::Offline);
    export_renderer(
        &mut renderer,
        path,
        start_frame,
        frames,
        encoding,
        progress,
        cancel,
        || plugin_service(&plugins),
    )
}

fn export_stems(
    plan: &StemsPlan,
    plugins: TaskPlugins,
    path: &Path,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<ExportSummary, String> {
    let mut audio = Vec::with_capacity(plan.stems.len());
    let mut done = 0;
    for (stem, plugins) in plan.stems.iter().zip(plugins) {
        let (Some(render), Some(plugins)) = (&stem.render, plugins) else {
            audio.push(None);
            continue;
        };
        let _pitch_pins = render
            .project
            .prepare_audio_pitch(|| cancel.load(Ordering::Relaxed))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "export cancelled".to_owned())?;
        let mut renderer = Renderer::with_plugin_host(&render.project, &*plugins)
            .map_err(|error| format!("{}: {error}", stem.name))?;
        renderer.set_quality(RenderQuality::Offline);
        let rendered = crate::audio::bounce::render_audio(
            &mut renderer,
            render.start_frame,
            render.end_frame,
            |frames| progress.store(done + frames, Ordering::Relaxed),
            cancel,
            || plugin_service(&plugins),
        )
        .map_err(|error| format!("{}: {error}", stem.name))?;
        done += render.end_frame;
        progress.store(done, Ordering::Relaxed);
        audio.push(Some(rendered));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("export cancelled".into());
    }
    let frames = audio
        .iter()
        .flatten()
        .map(|buffer: &AudioBuffer| buffer.len() as u64)
        .max()
        .unwrap_or(0);
    let project = plan.apply(audio)?;
    if path.exists() {
        return Err(format!("'{}' already exists", path.display()));
    }
    sympoie::project_json::save_project_folder(&project, path).map_err(|error| {
        let _ = fs::remove_dir_all(path);
        format!("cannot save '{}': {error}", path.display())
    })?;
    Ok(ExportSummary {
        path: path.to_owned(),
        frames,
        clipped_samples: 0,
    })
}

#[allow(clippy::too_many_arguments)]
fn export_renderer(
    renderer: &mut Renderer<'_>,
    path: &Path,
    start_frame: u64,
    frames: u64,
    encoding: Encoding,
    progress: &AtomicU64,
    cancel: &AtomicBool,
    mut service: impl FnMut() -> Result<(), String>,
) -> Result<ExportSummary, String> {
    let file_name = path
        .file_name()
        .ok_or("the destination needs a file name")?
        .to_string_lossy()
        .into_owned();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create '{}': {error}", parent.display()))?;
    }
    let temporary = path.with_file_name(format!(".{file_name}.{}.partial", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("cannot create '{}': {error}", temporary.display()))?;
    let mut cleanup = Cleanup {
        path: &temporary,
        armed: true,
    };
    let mut writer = AudioFileWriter::new(BufWriter::new(file), encoding, frames)
        .map_err(|error| error.to_string())?;
    let mut block = vec![[0.0; 2]; BLOCK_FRAMES];
    let mut done = 0;
    while done < frames {
        if cancel.load(Ordering::Relaxed) {
            return Err("export cancelled".to_owned());
        }
        service()?;
        let count = (frames - done).min(BLOCK_FRAMES as u64) as usize;
        renderer.render_block(start_frame + done, &mut block[..count]);
        if let Some(error) = renderer.latency_error() {
            return Err(format!("export failed at frame {done}: {error}"));
        }
        service()?;
        writer
            .write_frames(&block[..count])
            .map_err(|error| format!("export failed at frame {done}: {error}"))?;
        done += count as u64;
        progress.store(done, Ordering::Relaxed);
    }
    let clipped_samples = writer.clipped_samples();
    let file = writer.finish().map_err(|error| error.to_string())?;
    file.get_ref()
        .sync_all()
        .map_err(|error| error.to_string())?;
    drop(file);
    fs::rename(&temporary, path)
        .map_err(|error| format!("cannot replace '{}': {error}", path.display()))?;
    cleanup.armed = false;
    Ok(ExportSummary {
        path: path.to_owned(),
        frames,
        clipped_samples,
    })
}

struct Cleanup<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use sympoie::clip::AudioClip;
    use sympoie::project::AudioBuffer;
    use sympoie::wav::read_wav;

    fn wav16() -> Encoding {
        Encoding {
            depth: sympoie::audio_file::BitDepth::Int16,
            ..Encoding::default()
        }
    }

    struct DelayHost;

    #[derive(Debug)]
    struct DelayProcessor(std::collections::VecDeque<sympoie::Frame>);

    impl sympoie::plugin::PluginHost for DelayHost {
        fn load(
            &self,
            _: &sympoie::plugin::PluginInstance,
        ) -> Result<Box<dyn sympoie::plugin::PluginProcessor>, String> {
            Ok(Box::new(DelayProcessor(std::collections::VecDeque::from(
                vec![[0.0; 2]; 73],
            ))))
        }
    }

    impl sympoie::plugin::PluginProcessor for DelayProcessor {
        fn reset(&mut self) {
            self.0.make_contiguous().fill([0.0; 2]);
        }
        fn event(&mut self, _: sympoie::instrument::NoteEvent) {}
        fn set_parameter(&mut self, _: u32, _: f64) {}
        fn latency_samples(&self) -> u32 {
            73
        }
        fn process(
            &mut self,
            frames: &mut [sympoie::Frame],
            _: sympoie::plugin::PluginProcessContext,
        ) {
            for frame in frames {
                self.0.push_back(*frame);
                *frame = self.0.pop_front().unwrap();
            }
        }
    }

    #[test]
    fn latency_compensated_export_preserves_range_edges_and_propagates_host_errors() {
        use sympoie::plugin::{PluginDescriptor, PluginInstance, PluginKind};
        let mut project = Project::new();
        let source: Vec<_> = (0..5000)
            .map(|i| [((i * 17) % 101) as f32 / 256.0; 2])
            .collect();
        let asset = project.add_asset(AudioBuffer::new(source.clone()).unwrap());
        let track = project.add_audio_track("Delayed");
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 5000,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        project
            .add_effect(
                track.into(),
                PluginInstance::new(
                    PluginDescriptor {
                        path: "/fake/Delay.vst3".into(),
                        class_id: "delay".into(),
                        name: "Delay".into(),
                        vendor: "Test".into(),
                        kind: PluginKind::Effect,
                    },
                    vec![],
                    vec![],
                ),
            )
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("sympoie-export-delay-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("mix.wav");
        let progress = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        for start in [0, 123] {
            let mut renderer = Renderer::with_plugin_host(&project, &DelayHost).unwrap();
            let summary = export_renderer(
                &mut renderer,
                &path,
                start,
                5037,
                wav16(),
                &progress,
                &cancel,
                || Ok(()),
            )
            .unwrap();
            assert_eq!(summary.frames, 5037);
            let audio = read_wav(&path).unwrap();
            for (index, frame) in audio.samples().iter().enumerate() {
                let expected = source
                    .get(start as usize + index)
                    .copied()
                    .unwrap_or([0.0; 2]);
                assert!(
                    (frame[0] - expected[0]).abs() < 1.0 / 32768.0,
                    "start {start}, frame {index}"
                );
            }
        }
        let previous = fs::read(&path).unwrap();
        let mut renderer = Renderer::with_plugin_host(&project, &DelayHost).unwrap();
        let mut calls = 0;
        let failed = export_renderer(
            &mut renderer,
            &path,
            0,
            5000,
            wav16(),
            &progress,
            &cancel,
            || {
                calls += 1;
                if calls == 2 {
                    Err("plugin processing failed".into())
                } else {
                    Ok(())
                }
            },
        );
        assert!(failed.unwrap_err().contains("plugin processing failed"));
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_a_range_and_replaces_the_destination() {
        let mut project = Project::new();
        let frames: Vec<_> = (0..10_000).map(|i| [i as f32 / 20_000.0, 0.0]).collect();
        let asset = project.add_asset(AudioBuffer::new(frames).unwrap());
        let track = project.add_audio_track("Ramp");
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: 0,
                    frame_count: 10_000,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                    name: None,
                },
            )
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("sympoie-export-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("mix.wav");
        fs::write(&path, b"old").unwrap();

        let mut job = ExportJob::start(
            vec![ExportTask::Audio {
                project,
                path: path.clone(),
                start_frame: 100,
                frames: 5000,
                encoding: wav16(),
            }],
            &PluginRuntime::new(),
        )
        .unwrap();
        let summary = loop {
            if let Some(result) = job.poll() {
                break result.unwrap().remove(0);
            }
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(summary.frames, 5000);
        assert_eq!(job.fraction(), 1.0);
        let audio = read_wav(&path).unwrap();
        assert_eq!(audio.len(), 5000);
        assert!((audio.samples()[0][0] - 100.0 / 20_000.0).abs() < 1e-4);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(&directory).unwrap();
    }
}
