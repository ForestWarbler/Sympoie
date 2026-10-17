use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use sympoie::Frame;

pub const CHUNK_FRAMES: usize = 256;

pub struct Ring {
    samples: Box<[AtomicU32]>,
    chunk_starts: Box<[AtomicU64]>,
    chunk_advances: Box<[AtomicBool]>,
    chunk_loop_starts: Box<[AtomicU64]>,
    chunk_loop_ends: Box<[AtomicU64]>,
    capacity: u64,
    write: AtomicU64,
    read: AtomicU64,
    flush_to: AtomicU64,
}

impl Ring {
    pub fn new(chunks: usize) -> Self {
        assert!(chunks >= 2, "a ring needs room for at least two chunks");
        let frames = chunks * CHUNK_FRAMES;
        Self {
            samples: (0..frames * 2).map(|_| AtomicU32::new(0)).collect(),
            chunk_starts: (0..chunks).map(|_| AtomicU64::new(0)).collect(),
            chunk_advances: (0..chunks).map(|_| AtomicBool::new(true)).collect(),
            chunk_loop_starts: (0..chunks).map(|_| AtomicU64::new(0)).collect(),
            chunk_loop_ends: (0..chunks).map(|_| AtomicU64::new(0)).collect(),
            capacity: frames as u64,
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            flush_to: AtomicU64::new(0),
        }
    }

    pub fn buffered(&self) -> u64 {
        let write = self.write.load(Ordering::Relaxed);
        let read = self
            .read
            .load(Ordering::Acquire)
            .max(self.flush_to.load(Ordering::Relaxed));
        write.saturating_sub(read)
    }

    pub fn free(&self) -> u64 {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        self.capacity - (write - read)
    }

    pub fn push_chunk(&self, frames: &[Frame; CHUNK_FRAMES], start_position: u64) -> bool {
        self.push(frames, start_position, true, None)
    }

    pub fn push_loop_chunk(
        &self,
        frames: &[Frame; CHUNK_FRAMES],
        start_position: u64,
        loop_start: u64,
        loop_end: u64,
    ) -> bool {
        assert!(loop_start < loop_end);
        let start_position = if start_position >= loop_end {
            loop_start
        } else {
            start_position
        };
        self.push(frames, start_position, true, Some((loop_start, loop_end)))
    }

    pub fn push_stationary_chunk(&self, frames: &[Frame; CHUNK_FRAMES], position: u64) -> bool {
        self.push(frames, position, false, None)
    }

    fn push(
        &self,
        frames: &[Frame; CHUNK_FRAMES],
        start_position: u64,
        advances: bool,
        loop_range: Option<(u64, u64)>,
    ) -> bool {
        if self.free() < CHUNK_FRAMES as u64 {
            return false;
        }
        let write = self.write.load(Ordering::Relaxed);
        let first = (write % self.capacity) as usize;
        for (offset, frame) in frames.iter().enumerate() {
            let slot = (first + offset) * 2;
            self.samples[slot].store(frame[0].to_bits(), Ordering::Relaxed);
            self.samples[slot + 1].store(frame[1].to_bits(), Ordering::Relaxed);
        }
        let index = self.chunk_index(write);
        self.chunk_starts[index].store(start_position, Ordering::Relaxed);
        self.chunk_advances[index].store(advances, Ordering::Relaxed);
        let (loop_start, loop_end) = loop_range.unwrap_or((0, 0));
        self.chunk_loop_starts[index].store(loop_start, Ordering::Relaxed);
        self.chunk_loop_ends[index].store(loop_end, Ordering::Relaxed);
        self.write
            .store(write + CHUNK_FRAMES as u64, Ordering::Release);
        true
    }

    pub fn write_cursor(&self) -> u64 {
        self.write.load(Ordering::Acquire)
    }

    pub fn has_consumed_through(&self, cursor: u64) -> bool {
        self.read
            .load(Ordering::Acquire)
            .max(self.flush_to.load(Ordering::Acquire))
            >= cursor
    }

    pub fn flush(&self) {
        self.flush_to
            .store(self.write.load(Ordering::Relaxed), Ordering::Release);
    }

    #[cfg(test)]
    pub fn is_drained(&self) -> bool {
        self.buffered() == 0
    }

    #[cfg(test)]
    pub fn pop(&self, output: &mut [Frame]) -> (usize, Option<u64>) {
        let (count, position, _) = self.pop_marking_skips(output);
        (count, position)
    }

    pub fn pop_marking_skips(&self, output: &mut [Frame]) -> (usize, Option<u64>, bool) {
        let previous = self.read.load(Ordering::Relaxed);
        let read = previous.max(self.flush_to.load(Ordering::Acquire));
        let skipped = read > previous;
        let write = self.write.load(Ordering::Acquire);
        let count = (write.saturating_sub(read)).min(output.len() as u64) as usize;
        for (offset, frame) in output[..count].iter_mut().enumerate() {
            let slot = ((read + offset as u64) % self.capacity) as usize * 2;
            *frame = [
                f32::from_bits(self.samples[slot].load(Ordering::Relaxed)),
                f32::from_bits(self.samples[slot + 1].load(Ordering::Relaxed)),
            ];
        }
        let position = (count > 0).then(|| self.position_after(read + count as u64 - 1));
        self.read.store(read + count as u64, Ordering::Release);
        (count, position, skipped)
    }

    fn chunk_index(&self, frame_count: u64) -> usize {
        ((frame_count / CHUNK_FRAMES as u64) % self.chunk_starts.len() as u64) as usize
    }

    fn position_after(&self, frame_count: u64) -> u64 {
        let index = self.chunk_index(frame_count);
        let start = self.chunk_starts[index].load(Ordering::Relaxed);
        if self.chunk_advances[index].load(Ordering::Relaxed) {
            let offset = frame_count % CHUNK_FRAMES as u64 + 1;
            let loop_end = self.chunk_loop_ends[index].load(Ordering::Relaxed);
            if loop_end != 0 && offset >= loop_end - start {
                let loop_start = self.chunk_loop_starts[index].load(Ordering::Relaxed);
                loop_start + (offset - (loop_end - start)) % (loop_end - loop_start)
            } else {
                start.saturating_add(offset)
            }
        } else {
            start
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(value: f32) -> [Frame; CHUNK_FRAMES] {
        let mut frames = [[0.0; 2]; CHUNK_FRAMES];
        for (index, frame) in frames.iter_mut().enumerate() {
            *frame = [value + index as f32, -value];
        }
        frames
    }

    #[test]
    fn frames_come_out_in_order_with_their_timeline_positions() {
        let ring = Ring::new(4);
        assert!(ring.push_chunk(&chunk(0.0), 1000));
        assert!(ring.push_chunk(&chunk(1000.0), 5000));
        assert_eq!(ring.buffered(), 2 * CHUNK_FRAMES as u64);

        let mut out = [[0.0; 2]; 300];
        let (count, position) = ring.pop(&mut out);
        assert_eq!(count, 300);
        assert_eq!(out[0], [0.0, -0.0]);
        assert_eq!(out[255], [255.0, -0.0]);
        assert_eq!(out[256], [1000.0, -1000.0]);
        assert_eq!(position, Some(5000 + 44));

        let (count, _) = ring.pop(&mut out);
        assert_eq!(count, 2 * CHUNK_FRAMES - 300);
        assert!(ring.is_drained());
        assert_eq!(ring.pop(&mut out), (0, None));
    }

    #[test]
    fn loop_tags_wrap_partial_reads_and_survive_multiple_laps_inside_one_chunk() {
        let ring = Ring::new(2);
        assert!(ring.push_loop_chunk(&chunk(0.0), 108, 100, 110));
        let mut out = [[0.0; 2]; CHUNK_FRAMES];
        assert_eq!(ring.pop(&mut out[..2]), (2, Some(100)));
        assert_eq!(ring.pop(&mut out[..37]), (37, Some(107)));
        assert_eq!(ring.pop(&mut out), (217, Some(104)));
        assert!(ring.push_stationary_chunk(&chunk(1.0), 500));
        assert_eq!(ring.pop(&mut out), (CHUNK_FRAMES, Some(500)));
        assert!(ring.push_chunk(&chunk(2.0), 1000));
        assert_eq!(ring.pop(&mut out), (CHUNK_FRAMES, Some(1256)));
    }

    #[test]
    fn a_full_ring_refuses_chunks_until_the_consumer_catches_up() {
        let ring = Ring::new(2);
        assert!(ring.push_chunk(&chunk(0.0), 0));
        assert!(ring.push_chunk(&chunk(0.0), 256));
        assert_eq!(ring.free(), 0);
        assert!(!ring.push_chunk(&chunk(9.0), 512));
        let mut out = [[0.0; 2]; CHUNK_FRAMES];
        ring.pop(&mut out);
        assert!(ring.push_chunk(&chunk(9.0), 512));
        ring.pop(&mut out);
        let (count, position) = ring.pop(&mut out);
        assert_eq!(count, CHUNK_FRAMES);
        assert_eq!(out[0][0], 9.0);
        assert_eq!(position, Some(512 + CHUNK_FRAMES as u64));
    }

    #[test]
    fn flushing_skips_stale_audio_but_keeps_new_chunks() {
        let ring = Ring::new(8);
        ring.push_chunk(&chunk(1.0), 0);
        ring.push_chunk(&chunk(2.0), 256);
        ring.flush();
        assert!(ring.is_drained());
        ring.push_chunk(&chunk(7.0), 48_000);
        let mut out = [[0.0; 2]; 10];
        let (count, position) = ring.pop(&mut out);
        assert_eq!(count, 10);
        assert_eq!(out[0][0], 7.0);
        assert_eq!(position, Some(48_010));
    }

    #[test]
    fn samples_round_trip_bit_exactly() {
        let ring = Ring::new(2);
        let mut frames = [[0.0; 2]; CHUNK_FRAMES];
        frames[0] = [f32::MIN_POSITIVE, -0.0];
        frames[1] = [1.0e30, -1.0e-30];
        ring.push_chunk(&frames, 0);
        let mut out = [[1.0; 2]; 2];
        ring.pop(&mut out);
        assert_eq!(out[0][0].to_bits(), f32::MIN_POSITIVE.to_bits());
        assert_eq!(out[0][1].to_bits(), (-0.0_f32).to_bits());
        assert_eq!(out[1], [1.0e30, -1.0e-30]);
    }

    #[test]
    fn stationary_monitor_chunks_keep_the_cursor_fixed_across_partial_reads() {
        let ring = Ring::new(4);
        assert!(ring.push_stationary_chunk(&chunk(1.0), 48_000));
        assert!(ring.push_stationary_chunk(&chunk(2.0), 48_000));
        let mut output = [[0.0; 2]; 137];
        for expected_count in [137, 137, 137, 101] {
            assert_eq!(ring.pop(&mut output), (expected_count, Some(48_000)));
        }
        assert_eq!(ring.pop(&mut output), (0, None));
        assert!(ring.is_drained());
    }

    #[test]
    fn reads_follow_each_chunks_timeline_mode_across_play_pause_and_resume() {
        let ring = Ring::new(4);
        assert!(ring.push_chunk(&chunk(1.0), 1000));
        assert!(ring.push_stationary_chunk(&chunk(2.0), 1256));
        assert!(ring.push_chunk(&chunk(3.0), 1256));
        let mut output = [[0.0; 2]; 300];
        assert_eq!(ring.pop(&mut output), (300, Some(1256)));
        assert_eq!(output[256][0], 2.0);
        assert_eq!(ring.pop(&mut output), (300, Some(1344)));
        assert_eq!(output[212][0], 3.0);
        assert_eq!(ring.pop(&mut output), (168, Some(1512)));
    }

    #[test]
    fn stationary_tags_are_overwritten_on_wrap_and_new_tags_survive_flush() {
        let ring = Ring::new(2);
        let mut output = [[0.0; 2]; CHUNK_FRAMES];
        assert!(ring.push_stationary_chunk(&chunk(1.0), 800));
        assert!(ring.push_chunk(&chunk(2.0), 800));
        assert_eq!(ring.pop(&mut output), (CHUNK_FRAMES, Some(800)));
        assert!(ring.push_chunk(&chunk(3.0), 1056));
        assert_eq!(ring.pop(&mut output), (CHUNK_FRAMES, Some(1056)));
        assert!(ring.push_stationary_chunk(&chunk(4.0), 1312));
        assert_eq!(ring.pop(&mut output), (CHUNK_FRAMES, Some(1312)));
        ring.flush();
        assert!(ring.push_stationary_chunk(&chunk(5.0), u64::MAX));
        assert_eq!(ring.pop(&mut output), (CHUNK_FRAMES, Some(u64::MAX)));
        assert_eq!(output[0][0], 5.0);
    }

    #[test]
    fn end_watermark_completes_even_when_monitor_audio_remains_queued() {
        let ring = Ring::new(4);
        assert_eq!(ring.write_cursor(), 0);
        assert!(ring.has_consumed_through(0));
        assert!(ring.push_chunk(&chunk(1.0), 0));
        let end = ring.write_cursor();
        assert_eq!(end, CHUNK_FRAMES as u64);
        assert!(ring.push_stationary_chunk(&chunk(2.0), CHUNK_FRAMES as u64));
        assert!(ring.push_stationary_chunk(&chunk(3.0), CHUNK_FRAMES as u64));
        assert!(!ring.has_consumed_through(end));
        let mut first = [[0.0; 2]; CHUNK_FRAMES - 1];
        ring.pop(&mut first);
        assert!(!ring.has_consumed_through(end));
        ring.pop(&mut [[0.0; 2]; 1]);
        assert!(ring.has_consumed_through(end));
        assert!(!ring.is_drained());
        assert!(ring.push_stationary_chunk(&chunk(4.0), CHUNK_FRAMES as u64));
        assert!(
            ring.has_consumed_through(end),
            "new monitor writes cannot postpone the boundary"
        );
        let later = ring.write_cursor();
        assert!(!ring.has_consumed_through(later));
        ring.flush();
        assert!(ring.has_consumed_through(later));
    }
}
