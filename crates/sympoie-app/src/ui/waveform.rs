use std::collections::HashMap;

use sympoie::Frame;
use sympoie::project::{AssetId, AudioBuffer, Project};

const BASE_BUCKET: usize = 32;
const LEVEL_FACTOR: usize = 8;

pub type Extent = [f32; 4];

const EMPTY: Extent = [
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::INFINITY,
    f32::NEG_INFINITY,
];

fn merge(a: Extent, b: Extent) -> Extent {
    [
        a[0].min(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].max(b[3]),
    ]
}

fn extent_of(frames: &[Frame]) -> Extent {
    frames.iter().fold(EMPTY, |extent, frame| {
        merge(extent, [frame[0], frame[0], frame[1], frame[1]])
    })
}

struct Level {
    bucket: usize,
    extents: Vec<Extent>,
}

pub struct Overview {
    levels: Vec<Level>,
}

impl Overview {
    pub fn build(frames: &[Frame]) -> Self {
        let mut levels = vec![Level {
            bucket: BASE_BUCKET,
            extents: frames.chunks(BASE_BUCKET).map(extent_of).collect(),
        }];
        while levels
            .last()
            .is_some_and(|level| level.extents.len() > LEVEL_FACTOR)
        {
            let below = levels.last().expect("checked above");
            let extents = below
                .extents
                .chunks(LEVEL_FACTOR)
                .map(|chunk| chunk.iter().copied().fold(EMPTY, merge))
                .collect();
            let bucket = below.bucket * LEVEL_FACTOR;
            levels.push(Level { bucket, extents });
        }
        Self { levels }
    }

    pub fn extent(&self, frames: &[Frame], start: u64, end: u64) -> Option<Extent> {
        let end = end.min(frames.len() as u64);
        if start >= end {
            return None;
        }
        let span = (end - start) as usize;
        if span <= BASE_BUCKET * 2 {
            return Some(extent_of(&frames[start as usize..end as usize]));
        }
        let level = self
            .levels
            .iter()
            .rev()
            .find(|level| level.bucket * 2 <= span)
            .unwrap_or(&self.levels[0]);
        let first = start as usize / level.bucket;
        let last = ((end as usize - 1) / level.bucket).min(level.extents.len() - 1);
        Some(
            level.extents[first..=last]
                .iter()
                .copied()
                .fold(EMPTY, merge),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    asset: AssetId,
    address: usize,
    len: usize,
}

#[derive(Default)]
pub struct WaveformCache {
    overviews: HashMap<Key, Overview>,
}

impl WaveformCache {
    pub fn get<'a>(
        &'a mut self,
        project: &'a Project,
        asset: AssetId,
    ) -> Option<(&'a Overview, &'a AudioBuffer)> {
        let buffer = project.audio_asset(asset).ok()?;
        let key = Key {
            asset,
            address: buffer.samples().as_ptr() as usize,
            len: buffer.len(),
        };
        let overview = self
            .overviews
            .entry(key)
            .or_insert_with(|| Overview::build(buffer.samples()));
        Some((overview, buffer))
    }

    pub fn retain(&mut self, project: &Project) {
        self.overviews.retain(|key, _| {
            project.audio_asset(key.asset).is_ok_and(|buffer| {
                buffer.samples().as_ptr() as usize == key.address && buffer.len() == key.len
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extents_match_a_direct_scan_at_every_scale() {
        let frames: Vec<Frame> = (0..100_000)
            .map(|i| {
                let x = (i as f32 * 0.001).sin();
                [x, -x * 0.5]
            })
            .collect();
        let overview = Overview::build(&frames);
        for (start, end) in [
            (0, 10),
            (5, 70),
            (100, 5000),
            (0, 100_000),
            (12_345, 67_890),
        ] {
            let exact = extent_of(&frames[start..end]);
            let summary = overview.extent(&frames, start as u64, end as u64).unwrap();
            assert!(summary[0] <= exact[0] && summary[1] >= exact[1]);
            assert!((summary[1] - exact[1]).abs() < 0.05, "{start}..{end}");
        }
        assert!(overview.extent(&frames, 10, 10).is_none());
        assert!(overview.extent(&frames, 200_000, 300_000).is_none());
    }
}
