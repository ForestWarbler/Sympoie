use crate::project::AssetId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioClip {
    pub asset: AssetId,
    pub source_start: u64,
    pub start_tick: u64,
    pub frame_count: u64,
    pub gain: f32,
    pub pitch_semitones: f32,
    pub pitch_algorithm: crate::pitch::PitchAlgorithm,
    pub preserve_formants: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiClip {
    pub start_tick: u64,
    pub length_ticks: u64,
    pub offset_ticks: u64,
    pub notes: Vec<MidiNote>,
}

impl MidiClip {
    pub fn new(start_tick: u64, length_ticks: u64) -> Self {
        Self {
            start_tick,
            length_ticks,
            offset_ticks: 0,
            notes: Vec::new(),
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
}
