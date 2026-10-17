use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use sympoie::bounce::BouncePlan;
use sympoie::project::AudioBuffer;
use sympoie::render::Renderer;
use sympoie::{Frame, SAMPLE_RATE};

use crate::plugins::PluginRuntime;

const BLOCK_FRAMES: usize = 4096;
const MAX_AUDIO_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_TIMELINE_FRAMES: u64 = 24 * 60 * 60 * SAMPLE_RATE as u64;

#[derive(Debug)]
pub struct BounceResult {
    pub plan: BouncePlan,
    pub audio: AudioBuffer,
}

#[derive(Debug)]
pub struct BounceJob {
    pub total_frames: u64,
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<BounceResult, String>>>,
}

impl BounceJob {
    pub fn start(plan: BouncePlan) -> Result<Self, String> {
        checked_frame_count(plan.start_frame, plan.end_frame)?;
        let total_frames = plan.end_frame;
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = {
            let (progress, cancel) = (Arc::clone(&progress), Arc::clone(&cancel));
            thread::Builder::new()
                .name("sympoie-bounce".into())
                .spawn(move || bounce(plan, &progress, &cancel))
                .map_err(|error| format!("cannot start bounce: {error}"))?
        };
        Ok(Self {
            total_frames,
            progress,
            cancel,
            thread: Some(thread),
        })
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

    pub fn poll(&mut self) -> Option<Result<BounceResult, String>> {
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            let thread = self.thread.take()?;
            let result = thread
                .join()
                .unwrap_or_else(|_| Err("the bounce thread crashed".to_owned()));
            return Some(cancelled(&self.cancel).and(result));
        }
        None
    }
}

impl Drop for BounceJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn checked_frame_count(start_frame: u64, end_frame: u64) -> Result<usize, String> {
    let frames = end_frame
        .checked_sub(start_frame)
        .filter(|frames| *frames != 0)
        .ok_or("there is no audio range to bounce")?;
    if end_frame > MAX_TIMELINE_FRAMES {
        return Err("bounce supports arrangements ending within 24 hours".into());
    }
    if frames > MAX_AUDIO_BYTES / std::mem::size_of::<Frame>() as u64 {
        return Err("the bounced audio would exceed the 1 GiB memory limit".into());
    }
    usize::try_from(frames).map_err(|_| "the bounced audio is too large for this system".into())
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("bounce cancelled".into())
    } else {
        Ok(())
    }
}

fn bounce(
    plan: BouncePlan,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<BounceResult, String> {
    cancelled(cancel)?;
    let audio = {
        let _pitch_pins = plan
            .project
            .prepare_audio_pitch(|| cancel.load(Ordering::Relaxed))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "bounce cancelled".to_owned())?;
        let plugins = PluginRuntime::new();
        let mut renderer = Renderer::with_plugin_host(&plan.project, &*plugins)
            .map_err(|error| error.to_string())?;
        render_audio(
            &mut renderer,
            plan.start_frame,
            plan.end_frame,
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
        )?
    };
    cancelled(cancel)?;
    Ok(BounceResult { plan, audio })
}

fn render_audio(
    renderer: &mut Renderer<'_>,
    start_frame: u64,
    end_frame: u64,
    progress: &AtomicU64,
    cancel: &AtomicBool,
    mut service: impl FnMut() -> Result<(), String>,
) -> Result<AudioBuffer, String> {
    cancelled(cancel)?;
    let frame_count = checked_frame_count(start_frame, end_frame)?;
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(frame_count)
        .map_err(|error| format!("cannot allocate the bounced audio: {error}"))?;
    let mut block = [[0.0; 2]; BLOCK_FRAMES];
    let mut done = 0;
    while done < end_frame {
        cancelled(cancel)?;
        service()?;
        let count = (end_frame - done).min(BLOCK_FRAMES as u64) as usize;
        renderer.render_block(done, &mut block[..count]);
        if let Some(error) = renderer.latency_error() {
            return Err(format!("bounce failed at frame {done}: {error}"));
        }
        service()?;
        cancelled(cancel)?;
        for (offset, frame) in block[..count].iter().enumerate() {
            if let Some(channel) = frame.iter().position(|sample| !sample.is_finite()) {
                return Err(format!(
                    "bounce produced non-finite audio at frame {}, channel {}",
                    done + offset as u64,
                    channel + 1
                ));
            }
        }
        let skip = start_frame.saturating_sub(done).min(count as u64) as usize;
        samples.extend_from_slice(&block[skip..count]);
        done += count as u64;
        progress.store(done, Ordering::Relaxed);
    }
    cancelled(cancel)?;
    debug_assert_eq!(samples.len(), frame_count);
    AudioBuffer::new(samples).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use sympoie::clip::{AudioClip, MidiClip, MidiNote};
    use sympoie::instrument::{Instrument, NoteEvent, SubtractiveSettings};
    use sympoie::plugin::{
        PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginProcessContext,
        PluginProcessor,
    };
    use sympoie::project::Project;

    fn audio_project(start_tick: u64, frames: usize) -> (Project, Vec<Frame>) {
        let samples: Vec<_> = (0..frames)
            .map(|i| {
                [
                    1.25 + (i % 101) as f32 / 131_072.0,
                    -((i % 97) as f32) * 0.000_001,
                ]
            })
            .collect();
        let mut project = Project::new();
        let asset = project.add_asset(AudioBuffer::new(samples.clone()).unwrap());
        let track = project.add_audio_track("Float source");
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    start_tick,
                    source_start: 0,
                    frame_count: frames as u64,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                },
            )
            .unwrap();
        (project, samples)
    }

    fn finish(job: &mut BounceJob) -> BounceResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = job.poll() {
                return result.unwrap();
            }
            assert!(Instant::now() < deadline, "bounce worker did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn job_preserves_float_headroom_and_alignment_and_delivers_only_once() {
        let (project, source) = audio_project(127, 10_123);
        let plan = BouncePlan::new(&project, 1).unwrap();
        let mut job = BounceJob::start(plan).unwrap();
        let result = finish(&mut job);
        assert_eq!(result.plan.start_tick, 127);
        assert_eq!(result.audio.samples(), source);
        assert_eq!(job.fraction(), 1.0);
        assert!(job.poll().is_none());
        assert_eq!(project.tracks().len(), 2);
    }

    #[test]
    fn cancel_after_render_before_acceptance_discards_the_result() {
        let (project, _) = audio_project(0, 32);
        let mut job = BounceJob::start(BouncePlan::new(&project, 1).unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !job.thread.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "bounce worker did not finish");
            thread::sleep(Duration::from_millis(1));
        }
        job.cancel();
        assert_eq!(job.poll().unwrap().unwrap_err(), "bounce cancelled");
        assert!(job.poll().is_none());
    }

    #[test]
    fn builtin_instrument_bounce_contains_note_and_release_tail() {
        let mut project = Project::new();
        let track = project.add_midi_track("Synth");
        project
            .set_midi_track_instrument(
                track,
                Some(Instrument::Subtractive(SubtractiveSettings::default())),
            )
            .unwrap();
        let mut clip = MidiClip::new(91, 960);
        clip.notes.push(MidiNote {
            start_tick: 0,
            duration_ticks: 960,
            key: 69,
            velocity: 100,
            channel: 0,
        });
        project.add_midi_clip(track, clip).unwrap();
        let note_end = project.tempo().ticks_to_frames(91 + 960).unwrap();
        let mut job = BounceJob::start(BouncePlan::new(&project, 1).unwrap()).unwrap();
        let result = finish(&mut job);
        let gate = (note_end - result.plan.start_frame) as usize;
        assert!(result.audio.len() > gate);
        assert!(
            result.audio.samples()[..gate]
                .iter()
                .any(|f| f[0].abs() > 0.01)
        );
        assert!(
            result.audio.samples()[gate..]
                .iter()
                .any(|f| f[0].abs() > 0.001)
        );
        assert!(result.audio.samples().last().unwrap()[0].abs() < 0.001);
    }

    struct DelayHost;

    #[derive(Debug)]
    struct DelayProcessor(std::collections::VecDeque<Frame>);

    impl PluginHost for DelayHost {
        fn load(&self, _: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
            Ok(Box::new(DelayProcessor(std::collections::VecDeque::from(
                vec![[0.0; 2]; 73],
            ))))
        }
    }

    impl PluginProcessor for DelayProcessor {
        fn reset(&mut self) {
            self.0.make_contiguous().fill([0.0; 2]);
        }
        fn event(&mut self, _: NoteEvent) {}
        fn set_parameter(&mut self, _: u32, _: f64) {}
        fn latency_samples(&self) -> u32 {
            73
        }
        fn process(&mut self, frames: &mut [Frame], _: PluginProcessContext) {
            for frame in frames {
                self.0.push_back(*frame);
                *frame = self.0.pop_front().unwrap();
            }
        }
    }

    #[test]
    fn latency_compensated_render_has_no_silent_prefix_and_keeps_tail() {
        let (mut project, source) = audio_project(127, 10_123);
        let track = project.track_ids().nth(1).unwrap();
        project
            .add_effect(
                track,
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
        let plan = BouncePlan::new(&project, 1).unwrap();
        let mut renderer = Renderer::with_plugin_host(&plan.project, &DelayHost).unwrap();
        let audio = render_audio(
            &mut renderer,
            plan.start_frame,
            plan.end_frame,
            &AtomicU64::new(0),
            &AtomicBool::new(false),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(&audio.samples()[..source.len()], source);
        assert!(audio.len() > source.len());
        assert!(
            audio.samples()[source.len()..]
                .iter()
                .all(|f| *f == [0.0; 2])
        );
    }

    #[test]
    fn cancellation_and_processing_errors_never_return_partial_audio() {
        let (project, _) = audio_project(0, BLOCK_FRAMES * 3);
        let progress = AtomicU64::new(0);
        let cancel = AtomicBool::new(true);
        let mut renderer = Renderer::new(&project).unwrap();
        let result = render_audio(
            &mut renderer,
            0,
            (BLOCK_FRAMES * 3) as u64,
            &progress,
            &cancel,
            || panic!("pre-cancelled bounce must not service plugins"),
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert_eq!(progress.load(Ordering::Relaxed), 0);

        cancel.store(false, Ordering::Relaxed);
        let mut service_calls = 0;
        let result = render_audio(
            &mut renderer,
            0,
            (BLOCK_FRAMES * 3) as u64,
            &progress,
            &cancel,
            || {
                service_calls += 1;
                if service_calls == 3 {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert_eq!(progress.load(Ordering::Relaxed), BLOCK_FRAMES as u64);

        cancel.store(false, Ordering::Relaxed);
        let result = render_audio(
            &mut renderer,
            0,
            (BLOCK_FRAMES * 3) as u64,
            &progress,
            &cancel,
            || Err("plugin failed while processing".into()),
        );
        assert_eq!(result.unwrap_err(), "plugin failed while processing");
    }

    #[test]
    fn limits_reject_empty_oversized_and_extremely_sparse_ranges() {
        assert!(checked_frame_count(10, 10).is_err());
        assert!(checked_frame_count(10, 9).is_err());
        assert!(
            checked_frame_count(0, MAX_AUDIO_BYTES / 8 + 1)
                .unwrap_err()
                .contains("1 GiB")
        );
        assert!(
            checked_frame_count(MAX_TIMELINE_FRAMES, MAX_TIMELINE_FRAMES + 1)
                .unwrap_err()
                .contains("24 hours")
        );
        assert_eq!(checked_frame_count(100, 101).unwrap(), 1);
    }

    #[test]
    fn render_rejects_non_finite_output_instead_of_saving_corrupt_audio() {
        let mut project = Project::new();
        let asset = project.add_asset(AudioBuffer::new(vec![[f32::MAX; 2]; 32]).unwrap());
        let track = project.add_audio_track("Overflow");
        project
            .add_audio_clip(
                track,
                AudioClip {
                    asset,
                    start_tick: 0,
                    source_start: 0,
                    frame_count: 32,
                    gain: 2.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: Default::default(),
                    preserve_formants: false,
                },
            )
            .unwrap();
        let mut renderer = Renderer::new(&project).unwrap();
        let progress = AtomicU64::new(0);
        let result = render_audio(
            &mut renderer,
            0,
            32,
            &progress,
            &AtomicBool::new(false),
            || Ok(()),
        );
        assert!(
            result
                .unwrap_err()
                .contains("non-finite audio at frame 0, channel 1")
        );
        assert_eq!(progress.load(Ordering::Relaxed), 0);
    }
}
