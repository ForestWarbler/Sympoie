use std::fs::{self, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use sympoie::project::Project;
use sympoie::render::Renderer;
use sympoie::wav::WavWriter;

use crate::plugins::PluginRuntime;

const BLOCK_FRAMES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub path: PathBuf,
    pub frames: u64,
    pub clipped_samples: u64,
}

pub struct ExportJob {
    pub path: PathBuf,
    pub total_frames: u64,
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<ExportSummary, String>>>,
}

impl ExportJob {
    pub fn start(project: Project, path: PathBuf, start_frame: u64, frames: u64) -> Self {
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = {
            let (progress, cancel, path) =
                (Arc::clone(&progress), Arc::clone(&cancel), path.clone());
            thread::Builder::new()
                .name("sympoie-export".into())
                .spawn(move || export(&project, &path, start_frame, frames, &progress, &cancel))
                .expect("the operating system refused to start the export thread")
        };
        Self {
            path,
            total_frames: frames,
            progress,
            cancel,
            thread: Some(thread),
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.total_frames == 0 {
            1.0
        } else {
            self.progress.load(Ordering::Relaxed) as f32 / self.total_frames as f32
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&mut self) -> Option<Result<ExportSummary, String>> {
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

fn export(
    project: &Project,
    path: &Path,
    start_frame: u64,
    frames: u64,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<ExportSummary, String> {
    let _pitch_pins = project
        .prepare_audio_pitch(|| cancel.load(Ordering::Relaxed))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "export cancelled".to_owned())?;
    let plugins = PluginRuntime::new();
    let mut renderer =
        Renderer::with_plugin_host(project, &*plugins).map_err(|error| error.to_string())?;
    export_renderer(
        &mut renderer,
        path,
        start_frame,
        frames,
        progress,
        cancel,
        || {
            plugins.service();
            let errors = plugins.take_errors();
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; "))
            }
        },
    )
}

fn export_renderer(
    renderer: &mut Renderer<'_>,
    path: &Path,
    start_frame: u64,
    frames: u64,
    progress: &AtomicU64,
    cancel: &AtomicBool,
    mut service: impl FnMut() -> Result<(), String>,
) -> Result<ExportSummary, String> {
    let file_name = path
        .file_name()
        .ok_or("the destination needs a file name")?
        .to_string_lossy()
        .into_owned();
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
    let mut writer =
        WavWriter::new(BufWriter::new(file), frames).map_err(|error| error.to_string())?;
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
        let failed = export_renderer(&mut renderer, &path, 0, 5000, &progress, &cancel, || {
            calls += 1;
            if calls == 2 {
                Err("plugin processing failed".into())
            } else {
                Ok(())
            }
        });
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
                },
            )
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("sympoie-export-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("mix.wav");
        fs::write(&path, b"old").unwrap();

        let mut job = ExportJob::start(project, path.clone(), 100, 5000);
        let summary = loop {
            if let Some(result) = job.poll() {
                break result.unwrap();
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
