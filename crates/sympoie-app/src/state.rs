use std::collections::{BTreeSet, HashMap, HashSet};

use sympoie::TICKS_PER_QUARTER;
use sympoie::automation::AutomationTarget;
use sympoie::project::Project;
use sympoie::time::TimeSignature;
use sympoie::transport::PlaybackState;

use crate::audio::EngineStatus;
use crate::document::edit::{self, ClipRef, Grid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub start: u64,
    pub end: u64,
    pub tracks: BTreeSet<usize>,
}

impl TimeRange {
    pub fn contains(&self, track: usize, tick: f64) -> bool {
        self.tracks.contains(&track) && tick >= self.start as f64 && tick < self.end as f64
    }

    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub tracks: BTreeSet<usize>,
    pub clip: Option<ClipRef>,
    pub range: Option<TimeRange>,
}

impl Selection {
    pub fn has_audio(&self, project: &Project) -> bool {
        if let Some(range) = &self.range {
            edit::range_has_audio(project, &range.tracks, range.start, range.end)
        } else {
            self.clip
                .is_some_and(|clip| edit::audio_clip(project, clip).is_some())
        }
    }

    pub fn validate(&mut self, project: &Project) {
        let count = project.tracks().len();
        self.tracks.retain(|index| *index < count);
        if let Some(clip) = self.clip
            && edit::clip_span(project, clip).is_none()
        {
            self.clip = None;
        }
        if let Some(range) = &mut self.range {
            range.tracks.retain(|index| *index < count);
            if range.tracks.is_empty() || range.start >= range.end {
                self.range = None;
            }
        }
    }

    pub fn select_clip(&mut self, clip: ClipRef) {
        self.clip = Some(clip);
        self.tracks = BTreeSet::from([clip.track]);
        self.range = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopRegion {
    pub start: u64,
    pub end: u64,
    pub enabled: bool,
}

impl Default for LoopRegion {
    fn default() -> Self {
        Self {
            start: 0,
            end: TICKS_PER_QUARTER * 16,
            enabled: false,
        }
    }
}

pub const MIN_ZOOM: f64 = 0.0005;
pub const MAX_ZOOM: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridDensity {
    Narrowest,
    Narrow,
    Medium,
    Wide,
    Widest,
}

impl GridDensity {
    pub const ALL: [Self; 5] = [
        Self::Narrowest,
        Self::Narrow,
        Self::Medium,
        Self::Wide,
        Self::Widest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Narrowest => "Narrowest",
            Self::Narrow => "Narrow",
            Self::Medium => "Medium",
            Self::Wide => "Wide",
            Self::Widest => "Widest",
        }
    }

    pub fn min_pixels(self) -> f64 {
        match self {
            Self::Narrowest => 6.0,
            Self::Narrow => 12.0,
            Self::Medium => 24.0,
            Self::Wide => 48.0,
            Self::Widest => 96.0,
        }
    }

    pub fn step(self, narrower: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|density| *density == self)
            .unwrap_or(2);
        let next = if narrower {
            index.saturating_sub(1)
        } else {
            (index + 1).min(Self::ALL.len() - 1)
        };
        Self::ALL[next]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridMode {
    Adaptive(GridDensity),
    Fixed,
}

impl Default for GridMode {
    fn default() -> Self {
        Self::Adaptive(GridDensity::Medium)
    }
}

pub fn adaptive_interval(meter: TimeSignature, pixels_per_tick: f64, min_pixels: f64) -> u64 {
    let bar = meter.ticks_per_bar();
    let notes = [8, 4, 2, 1]
        .map(|divisor| TICKS_PER_QUARTER / divisor)
        .into_iter()
        .chain([TICKS_PER_QUARTER * 2])
        .filter(|step| *step < bar && bar.is_multiple_of(*step));
    let bars = (0..14).map(|power| bar << power);
    let mut last = bar;
    for step in notes.chain(bars) {
        last = step;
        if step as f64 * pixels_per_tick >= min_pixels {
            return step;
        }
    }
    last
}

pub fn interval_label(meter: TimeSignature, interval: u64) -> String {
    let bar = meter.ticks_per_bar();
    if interval >= bar && interval.is_multiple_of(bar) {
        let bars = interval / bar;
        return format!("{bars} Bar{}", if bars == 1 { "" } else { "s" });
    }
    let whole = TICKS_PER_QUARTER * 4;
    if whole.is_multiple_of(interval) {
        format!("1/{}", whole / interval)
    } else {
        format!("{interval} ticks")
    }
}

pub struct ArrangementState {
    pub pixels_per_tick: f64,
    pub playback_start: u64,
    pub scroll_tick: f64,
    pub overview_end_tick: f64,
    pub row_height: f32,
    pub track_heights: HashMap<usize, f32>,
    pub follow: bool,
    pub folded: HashSet<usize>,
    pub lanes_width: f32,
    pub renaming: Option<(usize, String)>,
    pub grid: GridMode,
    pub automation: bool,
    pub automation_targets: HashMap<usize, AutomationTarget>,
}

impl Default for ArrangementState {
    fn default() -> Self {
        Self {
            pixels_per_tick: 0.02,
            playback_start: 0,
            scroll_tick: 0.0,
            overview_end_tick: 0.0,
            row_height: 68.0,
            track_heights: HashMap::new(),
            follow: true,
            folded: HashSet::new(),
            lanes_width: 900.0,
            renaming: None,
            grid: GridMode::default(),
            automation: false,
            automation_targets: HashMap::new(),
        }
    }
}

impl ArrangementState {
    pub fn track_height(&self, index: usize) -> f32 {
        self.track_heights
            .get(&index)
            .copied()
            .unwrap_or(self.row_height)
            .clamp(if self.automation { 92.0 } else { 46.0 }, 320.0)
    }

    pub fn set_track_height(&mut self, index: usize, height: f32) {
        if height.is_finite() {
            self.track_heights.insert(index, height.clamp(46.0, 320.0));
        }
    }

    pub fn set_all_track_heights(&mut self, height: f32) {
        self.row_height = height;
        self.track_heights.clear();
    }

    pub fn grid(&self, project: &Project) -> Grid {
        match self.grid {
            GridMode::Adaptive(density) => Grid::every(adaptive_interval(
                project.time_signature(),
                self.pixels_per_tick,
                density.min_pixels(),
            )),
            GridMode::Fixed => Grid::of_project(project),
        }
    }

    pub fn visible_ticks(&self) -> f64 {
        f64::from(self.lanes_width) / self.pixels_per_tick
    }

    pub fn zoom_around(&mut self, factor: f64, anchor_x: f32) {
        let anchor_tick = self.scroll_tick + f64::from(anchor_x) / self.pixels_per_tick;
        self.pixels_per_tick = (self.pixels_per_tick * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.scroll_tick = (anchor_tick - f64::from(anchor_x) / self.pixels_per_tick).max(0.0);
    }

    pub fn fit(&mut self, start: u64, end: u64) {
        let span = (end.saturating_sub(start)).max(TICKS_PER_QUARTER * 4) as f64;
        let width = f64::from(self.lanes_width.max(200.0));
        self.pixels_per_tick = (width / (span * 1.08)).clamp(MIN_ZOOM, MAX_ZOOM);
        self.scroll_tick = (start as f64 - span * 0.02).max(0.0);
        self.overview_end_tick = 0.0;
    }

    pub fn follow_playhead(&mut self, tick: f64, status: &EngineStatus) {
        if !self.follow || status.state != PlaybackState::Playing {
            return;
        }
        let visible = self.visible_ticks();
        let right_margin = visible * 0.04;
        if tick > self.scroll_tick + visible - right_margin || tick < self.scroll_tick {
            self.scroll_tick = (tick - visible * 0.05).max(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_grids_follow_zoom_and_density() {
        let four = TimeSignature::default();
        let at = |density: GridDensity| adaptive_interval(four, 0.02, density.min_pixels());
        assert_eq!(at(GridDensity::Narrowest), 480);
        assert_eq!(at(GridDensity::Medium), 1920);
        assert_eq!(at(GridDensity::Widest), 7680);
        assert_eq!(adaptive_interval(four, 1.0, 24.0), 120);
        assert_eq!(adaptive_interval(four, 0.0005, 24.0), 3840 * 16);
        let three = TimeSignature::new(3, 4).unwrap();
        assert_eq!(adaptive_interval(three, 0.02, 24.0), 2880);
        assert_eq!(GridDensity::Medium.step(true), GridDensity::Narrow);
        assert_eq!(GridDensity::Widest.step(false), GridDensity::Widest);
        assert_eq!(interval_label(four, 240), "1/16");
        assert_eq!(interval_label(four, 3840), "1 Bar");
        assert_eq!(interval_label(four, 7680), "2 Bars");
        assert_eq!(interval_label(three, 2880), "1 Bar");
    }

    #[test]
    fn zooming_keeps_the_anchor_in_place() {
        let mut view = ArrangementState {
            scroll_tick: 1000.0,
            ..ArrangementState::default()
        };
        let anchor = 300.0;
        let before = view.scroll_tick + f64::from(anchor) / view.pixels_per_tick;
        view.zoom_around(2.0, anchor);
        let after = view.scroll_tick + f64::from(anchor) / view.pixels_per_tick;
        assert!((before - after).abs() < 1e-6);
        view.zoom_around(1e9, anchor);
        assert_eq!(view.pixels_per_tick, MAX_ZOOM);
    }

    #[test]
    fn following_pages_forward_only_while_playing() {
        let mut view = ArrangementState::default();
        let visible = view.visible_ticks();
        let mut status = EngineStatus {
            state: PlaybackState::Stopped,
            position_frame: 0,
            render_error: None,
        };
        view.follow_playhead(visible * 2.0, &status);
        assert_eq!(view.scroll_tick, 0.0);
        status.state = PlaybackState::Playing;
        view.follow_playhead(visible * 2.0, &status);
        assert!(view.scroll_tick > visible);
    }
}
