use std::sync::OnceLock;
use std::time::Instant;

#[derive(Debug)]
struct Anchor {
    instant: Instant,
    frame: u64,
}

#[derive(Debug, Default)]
pub struct RecordingClock {
    anchor: OnceLock<Anchor>,
    stopped: OnceLock<Instant>,
}

impl RecordingClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&self, start_frame: u64) {
        self.start_at(start_frame, Instant::now());
    }

    fn start_at(&self, start_frame: u64, instant: Instant) {
        if self.stopped.get().is_some() {
            return;
        }
        let _ = self.anchor.set(Anchor {
            instant,
            frame: start_frame,
        });
    }

    pub fn stop(&self) -> u64 {
        self.stop_at(Instant::now())
    }

    fn stop_at(&self, instant: Instant) -> u64 {
        self.stopped.get_or_init(|| instant);
        self.elapsed_frames()
    }

    pub fn offset_at(&self, instant: Instant) -> Option<u64> {
        if self.stopped.get().is_some_and(|stop| instant >= *stop) {
            return None;
        }
        let anchor = self.anchor.get()?;
        instant.checked_duration_since(anchor.instant).map(frames)
    }

    pub fn elapsed_frames(&self) -> u64 {
        self.anchor.get().map_or(0, |anchor| {
            frames(
                self.stopped
                    .get()
                    .copied()
                    .unwrap_or_else(Instant::now)
                    .saturating_duration_since(anchor.instant),
            )
        })
    }

    pub fn start_frame(&self) -> Option<u64> {
        self.anchor.get().map(|anchor| anchor.frame)
    }

    pub fn is_started(&self) -> bool {
        self.anchor.get().is_some()
    }
}

fn frames(duration: std::time::Duration) -> u64 {
    (duration
        .as_nanos()
        .saturating_mul(u128::from(sympoie::SAMPLE_RATE))
        / 1_000_000_000)
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn one_origin_gates_input_and_stop_freezes_the_take() {
        let clock = RecordingClock::new();
        let now = Instant::now();
        assert_eq!(clock.offset_at(now), None);
        clock.start_at(1234, now);
        assert_eq!(clock.start_frame(), Some(1234));
        assert_eq!(clock.offset_at(now - Duration::from_millis(1)), None);
        assert_eq!(
            clock.offset_at(now + Duration::from_millis(125)),
            Some(6000)
        );
        assert_eq!(clock.stop_at(now + Duration::from_millis(250)), 12000);
        assert_eq!(
            clock.offset_at(now + Duration::from_millis(125)),
            Some(6000)
        );
        assert_eq!(clock.offset_at(now + Duration::from_millis(300)), None);
        clock.start_at(9000, now + Duration::from_secs(1));
        assert_eq!(clock.stop_at(now + Duration::from_secs(2)), 12000);
        assert_eq!(clock.start_frame(), Some(1234));
    }

    #[test]
    fn stopping_a_queued_start_prevents_capture() {
        let clock = RecordingClock::new();
        assert_eq!(clock.stop(), 0);
        clock.start(10);
        assert!(!clock.is_started());
        assert_eq!(clock.offset_at(Instant::now()), None);
    }
}
