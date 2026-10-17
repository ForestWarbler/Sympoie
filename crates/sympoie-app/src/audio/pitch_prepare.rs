use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use sympoie::Frame;
use sympoie::project::Project;

type PreparedAudio = Vec<Arc<[Frame]>>;
type PreparationResult = Result<PreparedAudio, String>;

struct Request {
    generation: u64,
    epoch: u64,
    project: Project,
}

#[derive(Default)]
struct Queue {
    pending: Option<Request>,
    stopping: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    epoch: AtomicU64,
}

pub(super) struct PitchWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl PitchWorker {
    pub(super) fn new(notify: impl Fn(u64, PreparationResult) + Send + 'static) -> Self {
        Self::with_processor(notify, |project, cancelled| {
            project
                .prepare_audio_pitch(cancelled)
                .map_err(|error| error.to_string())
        })
    }

    fn with_processor(
        notify: impl Fn(u64, PreparationResult) + Send + 'static,
        mut prepare: impl FnMut(&Project, &dyn Fn() -> bool) -> Result<Option<PreparedAudio>, String>
        + Send
        + 'static,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("sympoie-pitch".into())
            .spawn(move || {
                loop {
                    let request = {
                        let mut queue = worker.queue.lock().unwrap_or_else(PoisonError::into_inner);
                        while queue.pending.is_none() && !queue.stopping {
                            queue = worker
                                .wake
                                .wait(queue)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                        if queue.stopping {
                            return;
                        }
                        queue.pending.take().expect("a pending pitch request")
                    };
                    let cancelled = || worker.epoch.load(Ordering::Acquire) != request.epoch;
                    let result = prepare(&request.project, &cancelled);
                    if !cancelled() && !matches!(result, Ok(None)) {
                        notify(
                            request.generation,
                            result.map(|audio| audio.expect("cancelled results are excluded")),
                        );
                    }
                }
            })
            .expect("the operating system refused to start the pitch worker");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    pub(super) fn request(&self, generation: u64, project: Project) {
        let retired = {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if queue.stopping {
                return;
            }
            let epoch = self
                .shared
                .epoch
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            queue.pending.replace(Request {
                generation,
                epoch,
                project,
            })
        };
        self.shared.wake.notify_one();
        drop(retired);
    }

    pub(super) fn cancel(&self) {
        let retired = {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.shared.epoch.fetch_add(1, Ordering::AcqRel);
            queue.pending.take()
        };
        drop(retired);
    }

    pub(super) fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let retired = {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            queue.stopping = true;
            self.shared.epoch.fetch_add(1, Ordering::AcqRel);
            queue.pending.take()
        };
        self.shared.wake.notify_one();
        drop(retired);
        let _ = thread.join();
    }
}

impl Drop for PitchWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const DEADLINE: Duration = Duration::from_secs(2);

    fn project(bpm: f64) -> Project {
        let mut project = Project::new();
        project.set_bpm(bpm).unwrap();
        project
    }

    #[test]
    fn rapid_requests_cancel_active_work_and_keep_only_the_latest_pending_project() {
        let (started, starts) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (finished, results) = mpsc::channel();
        let worker = PitchWorker::with_processor(
            move |generation, _| finished.send(generation).unwrap(),
            move |project, cancelled| {
                let bpm = project.bpm() as u32;
                started.send(bpm).unwrap();
                if bpm == 100 {
                    resume.recv_timeout(DEADLINE).unwrap();
                    assert!(cancelled());
                }
                Ok(Some(Vec::new()))
            },
        );
        worker.request(1, project(100.0));
        assert_eq!(starts.recv_timeout(DEADLINE).unwrap(), 100);
        worker.request(2, project(110.0));
        worker.request(3, project(120.0));
        release.send(()).unwrap();
        assert_eq!(starts.recv_timeout(DEADLINE).unwrap(), 120);
        assert_eq!(results.recv_timeout(DEADLINE).unwrap(), 3);
        assert!(starts.try_recv().is_err());
        assert!(results.try_recv().is_err());
    }

    #[test]
    fn cancel_discards_pending_work_and_a_later_request_can_reuse_a_generation() {
        let (started, starts) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (finished, results) = mpsc::channel();
        let worker = PitchWorker::with_processor(
            move |generation, _| finished.send(generation).unwrap(),
            move |project, cancelled| {
                let bpm = project.bpm() as u32;
                started.send(bpm).unwrap();
                if bpm == 100 {
                    resume.recv_timeout(DEADLINE).unwrap();
                    assert!(cancelled());
                    return Ok(None);
                }
                Ok(Some(Vec::new()))
            },
        );
        worker.request(7, project(100.0));
        assert_eq!(starts.recv_timeout(DEADLINE).unwrap(), 100);
        worker.request(8, project(110.0));
        worker.cancel();
        worker.request(7, project(120.0));
        release.send(()).unwrap();
        assert_eq!(starts.recv_timeout(DEADLINE).unwrap(), 120);
        assert_eq!(results.recv_timeout(DEADLINE).unwrap(), 7);
        assert!(starts.try_recv().is_err());
        assert!(results.try_recv().is_err());
    }

    #[test]
    fn shutdown_cancels_in_flight_work_before_joining_and_is_idempotent() {
        let (started, starts) = mpsc::channel();
        let (observed, cancellation) = mpsc::channel();
        let (finished, results) = mpsc::channel();
        let mut worker = PitchWorker::with_processor(
            move |generation, _| finished.send(generation).unwrap(),
            move |_, cancelled| {
                started.send(()).unwrap();
                let deadline = Instant::now() + DEADLINE;
                while !cancelled() {
                    assert!(
                        Instant::now() < deadline,
                        "shutdown did not cancel preparation"
                    );
                    thread::yield_now();
                }
                observed.send(()).unwrap();
                Ok(None)
            },
        );
        worker.request(1, Project::new());
        starts.recv_timeout(DEADLINE).unwrap();
        worker.shutdown();
        cancellation.recv_timeout(DEADLINE).unwrap();
        assert!(results.try_recv().is_err());
        worker.shutdown();
    }

    #[test]
    fn dropping_an_idle_worker_wakes_and_joins_it() {
        let worker = PitchWorker::new(|_, _| panic!("no request was submitted"));
        drop(worker);
    }
}
