use crate::project::AssetId;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioClip {
    pub asset: AssetId,
    pub source_start: u64,
    pub start_tick: u64,
    pub frame_count: u64,
    pub gain: f32,
    pub pitch_semitones: f32,
    pub pitch_algorithm: crate::pitch::PitchAlgorithm,
    pub preserve_formants: bool,
    /// Shown on the clip instead of its source; `None` keeps the default label.
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiClip {
    pub start_tick: u64,
    pub length_ticks: u64,
    pub offset_ticks: u64,
    pub notes: Vec<MidiNote>,
    /// Pitch-bend breakpoints in clip ticks, sorted by tick; values ramp linearly between points.
    pub pitch_bend: Vec<BendPoint>,
    /// Shown on the clip instead of its note count; `None` keeps the default label.
    pub name: Option<String>,
}

impl MidiClip {
    pub fn new(start_tick: u64, length_ticks: u64) -> Self {
        Self {
            start_tick,
            length_ticks,
            offset_ticks: 0,
            notes: Vec::new(),
            pitch_bend: Vec::new(),
            name: None,
        }
    }

    pub fn offset_end(&self) -> u64 {
        self.offset_ticks.saturating_add(self.length_ticks)
    }

    pub fn audible_notes(&self) -> impl Iterator<Item = MidiNote> + '_ {
        let (from, to) = (self.offset_ticks, self.offset_end());
        self.notes
            .iter()
            .filter(move |note| note.start_tick >= from && note.start_tick < to)
            .map(move |note| MidiNote {
                start_tick: note.start_tick - from,
                duration_ticks: note.duration_ticks.min(to - note.start_tick),
                ..*note
            })
    }

    /// Bend points inside the audible window, relative to the clip start. When the window
    /// begins between two points, a point carrying the interpolated value is added at 0.
    pub fn audible_bend(&self) -> Vec<BendPoint> {
        let (from, to) = (self.offset_ticks, self.offset_end());
        let mut points = Vec::new();
        if !self.pitch_bend.is_empty() && !self.pitch_bend.iter().any(|p| p.tick == from) {
            points.push(BendPoint {
                tick: 0,
                value: bend_value_at(&self.pitch_bend, from),
            });
        }
        points.extend(
            self.pitch_bend
                .iter()
                .filter(|p| p.tick >= from && p.tick < to)
                .map(|p| BendPoint {
                    tick: p.tick - from,
                    value: p.value,
                }),
        );
        points
    }

    pub fn content_end(&self) -> u64 {
        self.notes
            .iter()
            .map(|note| note.start_tick.saturating_add(note.duration_ticks))
            .max()
            .unwrap_or(0)
            .max(self.offset_end())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNote {
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub key: u8,
    pub velocity: u8,
    pub channel: u8,
    /// Portamento into this note from the previous pitch, in ticks; 0 plays the note normally.
    /// Instruments that support it treat a sliding note as legato.
    pub slide_ticks: u32,
}

impl MidiNote {
    pub const fn new(start_tick: u64, duration_ticks: u64, key: u8, velocity: u8) -> Self {
        Self {
            start_tick,
            duration_ticks,
            key,
            velocity,
            channel: 0,
            slide_ticks: 0,
        }
    }
}

impl Default for MidiNote {
    fn default() -> Self {
        Self::new(0, 0, 60, 100)
    }
}

pub const BEND_MIN: i16 = -8192;
pub const BEND_MAX: i16 = 8191;

/// One pitch-bend breakpoint: a 14-bit MIDI bend value centred on 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BendPoint {
    pub tick: u64,
    pub value: i16,
}

impl BendPoint {
    /// The bend as -1..=1, where ±1 is the instrument's full bend range.
    pub fn normalized(self) -> f32 {
        bend_normalized(self.value)
    }
}

pub fn bend_normalized(value: i16) -> f32 {
    if value < 0 {
        value as f32 / -(BEND_MIN as f32)
    } else {
        value as f32 / BEND_MAX as f32
    }
}

/// Linearly interpolated bend at `tick`; holds the first/last value outside the points.
pub fn bend_value_at(points: &[BendPoint], tick: u64) -> i16 {
    let index = points.partition_point(|p| p.tick <= tick);
    match (index.checked_sub(1).map(|i| points[i]), points.get(index)) {
        (None, None) => 0,
        (None, Some(next)) => next.value,
        (Some(prev), None) => prev.value,
        (Some(prev), Some(next)) => {
            let span = (next.tick - prev.tick) as f64;
            let t = (tick - prev.tick) as f64 / span;
            (prev.value as f64 + (next.value as f64 - prev.value as f64) * t).round() as i16
        }
    }
}

/// Ramer–Douglas–Peucker thinning of a densely recorded or drawn bend curve. Keeps the
/// endpoints and every point that deviates from the simplified line by more than `tolerance`.
pub fn thin_bend(points: &[BendPoint], tolerance: f64) -> Vec<BendPoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        let (a, b) = (points[first], points[last]);
        let span = (b.tick - a.tick).max(1) as f64;
        let mut worst = (0.0, first);
        for (index, point) in points.iter().enumerate().take(last).skip(first + 1) {
            let t = (point.tick - a.tick) as f64 / span;
            let line = a.value as f64 + (b.value as f64 - a.value as f64) * t;
            let error = (point.value as f64 - line).abs();
            if error > worst.0 {
                worst = (error, index);
            }
        }
        if worst.0 > tolerance {
            keep[worst.1] = true;
            stack.push((first, worst.1));
            stack.push((worst.1, last));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(point, kept)| kept.then_some(*point))
        .collect()
}

/// Trims a user-entered clip name; blank names mean "use the default label".
pub fn normalized_clip_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}
