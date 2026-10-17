use std::collections::BTreeSet;
use std::sync::Arc;

use sympoie::clip::{AudioClip, MidiClip};
use sympoie::instrument::Instrument;
#[cfg(test)]
use sympoie::instrument::SubtractiveSettings;
use sympoie::pitch::PitchAlgorithm;
use sympoie::plugin::{PluginInstance, PluginKind};
use sympoie::project::{AudioBuffer, Project, ProjectError};
use sympoie::time::SnapGrid;
use sympoie::track::{AudioOutput, AudioTrackId, MidiTrackId, MixerSettings, Track, TrackId};

use crate::document::rows;

pub use super::track_order::{TrackDrop, can_drop_track, exit_group, move_track};

pub type EditResult<T> = Result<T, String>;

fn text(error: ProjectError) -> String {
    error.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClipRef {
    pub track: usize,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub enum ClipContent {
    Audio {
        clip: AudioClip,
        asset: Arc<AudioBuffer>,
    },
    Midi(MidiClip),
}

impl ClipContent {
    pub fn is_compatible(&self, track: &Track) -> bool {
        matches!(
            (self, track),
            (Self::Audio { .. }, Track::Audio(_)) | (Self::Midi(_), Track::Midi(_))
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Midi,
    Group,
    Bus,
}

pub fn track_id(project: &Project, index: usize) -> EditResult<TrackId> {
    project
        .track_ids()
        .nth(index)
        .ok_or_else(|| format!("track {index} no longer exists"))
}

pub fn audio_id(project: &Project, index: usize) -> EditResult<AudioTrackId> {
    match track_id(project, index)? {
        TrackId::Audio(id) => Ok(id),
        _ => Err("this operation needs an audio track".to_owned()),
    }
}

pub fn midi_id(project: &Project, index: usize) -> EditResult<MidiTrackId> {
    match track_id(project, index)? {
        TrackId::Midi(id) => Ok(id),
        _ => Err("this operation needs a MIDI track".to_owned()),
    }
}

fn unsnapped<T>(project: &mut Project, change: impl FnOnce(&mut Project) -> T) -> T {
    let grid = project.snap_grid();
    project.set_snap_grid(SnapGrid::Off);
    let result = change(project);
    project.set_snap_grid(grid);
    result
}

pub fn next_track_name(project: &Project, kind: TrackKind) -> String {
    let base = match kind {
        TrackKind::Audio => "Audio",
        TrackKind::Midi => "MIDI",
        TrackKind::Group => "Group",
        TrackKind::Bus => "Bus",
    };
    let names: BTreeSet<&str> = project.tracks().iter().map(Track::name).collect();
    (1..)
        .map(|number| format!("{base} {number}"))
        .find(|name| !names.contains(name.as_str()))
        .unwrap_or_else(|| base.to_owned())
}

pub fn add_track(project: &mut Project, kind: TrackKind, name: Option<String>) -> usize {
    let name = name.unwrap_or_else(|| next_track_name(project, kind));
    match kind {
        TrackKind::Audio => {
            project.add_audio_track(name);
        }
        TrackKind::Midi => {
            project.add_midi_track(name);
        }
        TrackKind::Group => {
            project.add_group_track(name);
        }
        TrackKind::Bus => {
            project.add_bus_track(name);
        }
    }
    project.tracks().len() - 1
}

pub fn set_track_color(project: &mut Project, index: usize, color: Option<u32>) -> EditResult<()> {
    let id = track_id(project, index)?;
    project.set_track_color(id, color).map_err(text)
}

pub fn rename_track(project: &mut Project, index: usize, name: &str) -> EditResult<()> {
    let id = track_id(project, index)?;
    project.rename_track(id, name.trim()).map_err(text)
}

pub fn set_mixer(project: &mut Project, index: usize, mixer: MixerSettings) -> EditResult<()> {
    match track_id(project, index)? {
        TrackId::Audio(id) => project.set_audio_track_mixer(id, mixer),
        TrackId::Midi(id) => project.set_midi_track_mixer(id, mixer),
        TrackId::Group(id) => project.set_group_track_mixer(id, mixer),
        TrackId::Bus(id) => project.set_bus_track_mixer(id, mixer),
        TrackId::Master(_) => project.set_master_track_mixer(mixer),
    }
    .map_err(text)
}

pub fn set_track_send(
    project: &mut Project,
    index: usize,
    bus: usize,
    gain: f32,
) -> EditResult<()> {
    let track = track_id(project, index)?;
    let TrackId::Bus(destination) = track_id(project, bus)? else {
        return Err("Sends need a Bus destination".to_owned());
    };
    project
        .set_track_send(track, destination, gain)
        .map_err(text)
}

pub fn remove_track_send(project: &mut Project, index: usize, bus: usize) -> EditResult<()> {
    let track = track_id(project, index)?;
    let TrackId::Bus(destination) = track_id(project, bus)? else {
        return Err("Sends need a Bus destination".to_owned());
    };
    project
        .remove_track_send(track, destination)
        .map(|_| ())
        .map_err(text)
}

pub fn set_instrument(
    project: &mut Project,
    index: usize,
    instrument: Option<Instrument>,
) -> EditResult<()> {
    let track = midi_id(project, index)?;
    project
        .set_midi_track_instrument(track, instrument)
        .map_err(text)
}

pub fn validate_plugin_destination(
    project: &Project,
    index: usize,
    kind: PluginKind,
    before: Option<u64>,
) -> EditResult<()> {
    let track = project
        .tracks()
        .get(index)
        .ok_or("Track no longer exists")?;
    if matches!(track, Track::Bus(_)) {
        return Err("Add Bus effects in the Mixer.".to_owned());
    }
    if kind == PluginKind::Instrument {
        midi_id(project, index)?;
        if before.is_some() {
            return Err("Instruments replace the MIDI track's instrument".to_owned());
        }
    } else if let Some(id) = before
        && !track.effects().iter().any(|effect| effect.id == id)
    {
        return Err("The destination effect no longer exists on this track".to_owned());
    }
    Ok(())
}

pub fn insert_plugin(
    project: &mut Project,
    index: usize,
    plugin: PluginInstance,
    before: Option<u64>,
) -> EditResult<u64> {
    validate_plugin_destination(project, index, plugin.descriptor.kind, before)?;
    match plugin.descriptor.kind {
        PluginKind::Instrument => {
            let midi = midi_id(project, index)?;
            project
                .set_plugin_instrument(midi, Some(plugin))
                .map_err(text)?;
            Ok(project
                .midi_track(midi)
                .map_err(text)?
                .plugin_instrument()
                .expect("assigned plugin")
                .id)
        }
        PluginKind::Effect => {
            let track = track_id(project, index)?;
            let effects = project.tracks()[index].effects();
            let end = effects.len();
            let position = before.and_then(|id| effects.iter().position(|effect| effect.id == id));
            let id = project.add_effect(track, plugin).map_err(text)?;
            if let Some(position) = position {
                project.move_effect(track, end, position).map_err(text)?;
            }
            Ok(id)
        }
    }
}

pub fn validate_mixer_effect_destination(
    project: &Project,
    index: usize,
    kind: PluginKind,
    before: Option<u64>,
) -> EditResult<()> {
    let track = project
        .tracks()
        .get(index)
        .ok_or("Track no longer exists")?;
    if kind != PluginKind::Effect {
        return Err("Mixer inserts accept audio effects only".to_owned());
    }
    if let Some(id) = before
        && !track.mixer_effects().iter().any(|effect| effect.id == id)
    {
        return Err("The destination Mixer effect no longer exists".to_owned());
    }
    Ok(())
}

pub fn insert_mixer_plugin(
    project: &mut Project,
    index: usize,
    plugin: PluginInstance,
    before: Option<u64>,
) -> EditResult<u64> {
    validate_mixer_effect_destination(project, index, plugin.descriptor.kind, before)?;
    let track = track_id(project, index)?;
    let effects = project.tracks()[index].mixer_effects();
    let end = effects.len();
    let position = before.and_then(|id| effects.iter().position(|effect| effect.id == id));
    let id = project.add_mixer_effect(track, plugin).map_err(text)?;
    if let Some(position) = position {
        project
            .move_mixer_effect(track, end, position)
            .map_err(text)?;
    }
    Ok(id)
}

pub fn delete_track(project: &mut Project, index: usize) -> EditResult<usize> {
    if index == 0 {
        return Err("the master track cannot be deleted".to_owned());
    }
    let id = track_id(project, index)?;
    if let TrackId::Bus(bus) = id {
        let destination = project.tracks()[index].mixer().output();
        let sources: Vec<_> = project
            .tracks()
            .iter()
            .enumerate()
            .filter(|(_, track)| track.mixer().output() == AudioOutput::Bus(bus))
            .map(|(index, track)| (index, track.mixer().with_output(destination)))
            .collect();
        for (source, mixer) in sources {
            set_mixer(project, source, mixer)?;
        }
        project.remove_track(id).map_err(text)?;
        return Ok(1);
    }
    let mut root = index;
    let mut removed = 0;
    loop {
        let children = rows::children(project);
        let leaf = rows::subtree(project, root)
            .into_iter()
            .filter(|node| children[*node].is_empty())
            .max()
            .expect("a subtree always has a leaf");
        let id = track_id(project, leaf)?;
        project.remove_track(id).map_err(text)?;
        removed += 1;
        if leaf == root {
            return Ok(removed);
        }
        if leaf < root {
            root -= 1;
        }
    }
}

pub fn group_tracks(project: &mut Project, indices: &[usize]) -> EditResult<usize> {
    let parents = rows::parents(project);
    let selected: BTreeSet<usize> = indices
        .iter()
        .copied()
        .filter(|index| {
            *index != 0
                && *index < parents.len()
                && !matches!(project.tracks()[*index], Track::Bus(_))
        })
        .collect();
    let tops: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|index| {
            !selected
                .iter()
                .any(|other| rows::is_descendant(&parents, *index, *other))
        })
        .collect();
    let Some(first) = tops.first() else {
        return Err("select one or more tracks to group".to_owned());
    };
    let shared_parent =
        parents[*first].filter(|parent| tops.iter().all(|top| parents[*top] == Some(*parent)));
    let shared_output = project.tracks()[*first].mixer().output();
    let shared_bus = matches!(shared_output, AudioOutput::Bus(_))
        && tops
            .iter()
            .all(|top| project.tracks()[*top].mixer().output() == shared_output);
    let ids: Vec<TrackId> = project.track_ids().collect();
    let group = add_track(project, TrackKind::Group, None);
    let TrackId::Group(group_id) = track_id(project, group)? else {
        unreachable!("a group track was just added");
    };
    if let Some(parent) = shared_parent {
        let TrackId::Group(parent_id) = ids[parent] else {
            unreachable!("parents are groups");
        };
        project
            .set_track_group(group_id, Some(parent_id))
            .map_err(text)?;
    } else if shared_bus {
        let mixer = project.tracks()[group].mixer().with_output(shared_output);
        set_mixer(project, group, mixer)?;
    }
    for top in tops {
        project
            .set_track_group(ids[top], Some(group_id))
            .map_err(text)?;
    }
    Ok(group)
}

pub fn ungroup(project: &mut Project, index: usize) -> EditResult<()> {
    let TrackId::Group(group) = track_id(project, index)? else {
        return Err("only group tracks can be ungrouped".to_owned());
    };
    let destination = project.group_track(group).map_err(text)?.mixer().output();
    let members: Vec<TrackId> = project.group_members(group).map_err(text)?.collect();
    for member in members {
        let index = project
            .track_ids()
            .position(|id| id == member)
            .expect("existing member");
        set_mixer(
            project,
            index,
            project.tracks()[index].mixer().with_output(destination),
        )?;
    }
    project.remove_track(group).map_err(text)?;
    Ok(())
}

pub fn clip_span(project: &Project, clip: ClipRef) -> Option<(u64, u64)> {
    match project.tracks().get(clip.track)? {
        Track::Audio(track) => {
            let audio = track.clips().get(clip.index)?;
            let tempo = project.tempo();
            let start = tempo.ticks_to_frames(audio.start_tick).ok()?;
            let end = tempo.frames_to_ticks(start + audio.frame_count).ok()?;
            Some((audio.start_tick, end.max(audio.start_tick + 1)))
        }
        Track::Midi(track) => {
            let midi = track.clips().get(clip.index)?;
            Some((midi.start_tick, midi.start_tick + midi.length_ticks))
        }
        _ => None,
    }
}

pub fn audio_clip(project: &Project, clip: ClipRef) -> Option<&AudioClip> {
    match project.tracks().get(clip.track)? {
        Track::Audio(track) => track.clips().get(clip.index),
        _ => None,
    }
}

pub fn midi_clip(project: &Project, clip: ClipRef) -> Option<&MidiClip> {
    match project.tracks().get(clip.track)? {
        Track::Midi(track) => track.clips().get(clip.index),
        _ => None,
    }
}

pub fn copy_clip(project: &Project, clip: ClipRef) -> EditResult<ClipContent> {
    match track_id(project, clip.track)? {
        TrackId::Audio(_) => {
            let clip = *audio_clip(project, clip).ok_or("clip no longer exists")?;
            let asset = project.shared_audio_asset(clip.asset).map_err(text)?;
            Ok(ClipContent::Audio { clip, asset })
        }
        TrackId::Midi(_) => midi_clip(project, clip)
            .cloned()
            .map(ClipContent::Midi)
            .ok_or_else(|| "clip no longer exists".to_owned()),
        _ => Err("only audio and MIDI tracks hold clips".to_owned()),
    }
}

pub fn paste_clip(
    project: &mut Project,
    content: &ClipContent,
    to_track: usize,
    start: u64,
) -> EditResult<ClipRef> {
    unsnapped(project, |project| {
        let count = match content {
            ClipContent::Audio { clip, asset } => {
                let id = audio_id(project, to_track)?;
                let mut pasted = *clip;
                pasted.start_tick = start;
                pasted.asset = project.add_shared_asset(Arc::clone(asset));
                project.add_audio_clip(id, pasted).map_err(text)?;
                project.audio_track(id).map_err(text)?.clips().len()
            }
            ClipContent::Midi(clip) => {
                let id = midi_id(project, to_track)?;
                let mut pasted = clip.clone();
                pasted.start_tick = start;
                project.add_midi_clip(id, pasted).map_err(text)?;
                project.midi_track(id).map_err(text)?.clips().len()
            }
        };
        Ok(ClipRef {
            track: to_track,
            index: count - 1,
        })
    })
}

pub fn copy_clip_to(
    project: &mut Project,
    clip: ClipRef,
    to_track: usize,
    start: u64,
) -> EditResult<ClipRef> {
    let content = copy_clip(project, clip)?;
    paste_clip(project, &content, to_track, start)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Grid {
    interval: Option<u64>,
}

impl Grid {
    #[cfg(test)]
    pub const OFF: Self = Self { interval: None };

    pub fn every(interval: u64) -> Self {
        Self {
            interval: (interval > 0).then_some(interval),
        }
    }

    pub fn of_project(project: &Project) -> Self {
        Self {
            interval: project.snap_grid().interval(project.time_signature()),
        }
    }

    pub fn interval(self) -> Option<u64> {
        self.interval
    }

    pub fn snap(self, tick: u64) -> u64 {
        let Some(interval) = self.interval else {
            return tick;
        };
        let remainder = tick % interval;
        if remainder < interval - remainder {
            tick - remainder
        } else {
            tick.saturating_add(interval - remainder)
        }
    }

    pub fn floor(self, tick: u64) -> u64 {
        self.interval
            .map_or(tick, |interval| tick - tick % interval)
    }

    pub fn ceil(self, tick: u64) -> u64 {
        self.interval.map_or(tick, |interval| {
            tick.div_ceil(interval).saturating_mul(interval)
        })
    }
}

pub fn snap(grid: Grid, tick: u64, snap: bool) -> u64 {
    if snap { grid.snap(tick) } else { tick }
}

fn ceil_to_grid(grid: Grid, tick: u64, snap: bool) -> u64 {
    if snap { grid.ceil(tick) } else { tick }
}

fn floor_tick(grid: Grid, tick: u64, snap: bool) -> u64 {
    if snap { grid.floor(tick) } else { tick }
}

fn tick_at_or_before(project: &Project, frame: u64) -> Option<u64> {
    let tempo = project.tempo();
    let mut tick = tempo.frames_to_ticks(frame).ok()?;
    while tick > 0 && tempo.ticks_to_frames(tick).ok()? > frame {
        tick -= 1;
    }
    Some(tick)
}

fn tick_at_or_after(project: &Project, frame: u64) -> Option<u64> {
    let tempo = project.tempo();
    let mut tick = tempo.frames_to_ticks(frame).ok()?;
    while tempo.ticks_to_frames(tick).ok()? < frame {
        tick += 1;
    }
    Some(tick)
}

pub fn trim_end_target(
    project: &Project,
    clip: ClipRef,
    raw_end: u64,
    grid: Grid,
    snap_on: bool,
) -> Option<u64> {
    let (start, _) = clip_span(project, clip)?;
    let minimum_step = grid.interval().filter(|_| snap_on).unwrap_or(1);
    if let Some(audio) = audio_clip(project, clip) {
        let tempo = project.tempo();
        let start_frame = tempo.ticks_to_frames(audio.start_tick).ok()?;
        let asset = project.audio_asset(audio.asset).ok()?;
        let max_frame = start_frame + (asset.len() as u64 - audio.source_start);
        let max_tick = tick_at_or_before(project, max_frame)?;
        let mut end =
            snap(grid, raw_end, snap_on).max(ceil_to_grid(grid, start + minimum_step, snap_on));
        if end > max_tick {
            end = floor_tick(grid, max_tick, snap_on);
            if end <= start {
                end = max_tick;
            }
        }
        (tempo.ticks_to_frames(end).ok()? > start_frame).then_some(end)
    } else {
        midi_clip(project, clip)?;
        let minimum = start + 1;
        let end = snap(grid, raw_end, snap_on).max(ceil_to_grid(
            grid,
            minimum.max(start + minimum_step),
            snap_on,
        ));
        Some(end.max(minimum))
    }
}

pub fn trim_start_target(
    project: &Project,
    clip: ClipRef,
    raw_start: u64,
    grid: Grid,
    snap_on: bool,
) -> Option<u64> {
    let (_, end) = clip_span(project, clip)?;
    if let Some(audio) = audio_clip(project, clip) {
        let tempo = project.tempo();
        let start_frame = tempo.ticks_to_frames(audio.start_tick).ok()?;
        let end_frame = start_frame + audio.frame_count;
        let earliest = tick_at_or_after(project, start_frame.saturating_sub(audio.source_start))?;
        let mut start = snap(grid, raw_start, snap_on);
        if start < earliest {
            start = ceil_to_grid(grid, earliest, snap_on);
        }
        (tempo.ticks_to_frames(start).ok()? < end_frame).then_some(start)
    } else {
        midi_clip(project, clip)?;
        let latest = end - 1;
        let start = snap(grid, raw_start, snap_on);
        let start = if start > latest {
            floor_tick(grid, latest, snap_on)
        } else {
            start
        };
        (start < end).then_some(start)
    }
}

pub fn move_clip(
    project: &mut Project,
    clip: ClipRef,
    to_track: usize,
    start: u64,
) -> EditResult<ClipRef> {
    let same_kind = matches!(
        (
            project.tracks().get(clip.track),
            project.tracks().get(to_track)
        ),
        (Some(Track::Audio(_)), Some(Track::Audio(_)))
            | (Some(Track::Midi(_)), Some(Track::Midi(_)))
    );
    let to_track = if same_kind { to_track } else { clip.track };
    unsnapped(project, |project| {
        if to_track == clip.track {
            match track_id(project, clip.track)? {
                TrackId::Audio(id) => project.move_audio_clip(id, clip.index, start),
                TrackId::Midi(id) => project.move_midi_clip(id, clip.index, start),
                _ => return Err("only audio and MIDI tracks hold clips".to_owned()),
            }
            .map_err(text)?;
            return Ok(clip);
        }
        match track_id(project, clip.track)? {
            TrackId::Audio(from) => {
                let to = audio_id(project, to_track)?;
                let mut moved = project.remove_audio_clip(from, clip.index).map_err(text)?;
                moved.start_tick = start;
                project.add_audio_clip(to, moved).map_err(text)?;
                let count = project.audio_track(to).map_err(text)?.clips().len();
                Ok(ClipRef {
                    track: to_track,
                    index: count - 1,
                })
            }
            TrackId::Midi(from) => {
                let to = midi_id(project, to_track)?;
                let mut moved = project.remove_midi_clip(from, clip.index).map_err(text)?;
                moved.start_tick = start;
                project.add_midi_clip(to, moved).map_err(text)?;
                let count = project.midi_track(to).map_err(text)?.clips().len();
                Ok(ClipRef {
                    track: to_track,
                    index: count - 1,
                })
            }
            _ => Err("only audio and MIDI tracks hold clips".to_owned()),
        }
    })
}

pub fn trim_end(project: &mut Project, clip: ClipRef, end: u64) -> EditResult<()> {
    unsnapped(project, |project| match track_id(project, clip.track)? {
        TrackId::Audio(id) => project
            .trim_audio_clip_end(id, clip.index, end)
            .map_err(text),
        TrackId::Midi(id) => project
            .trim_midi_clip_end(id, clip.index, end)
            .map_err(text),
        _ => Err("only audio and MIDI tracks hold clips".to_owned()),
    })
}

pub fn trim_start(project: &mut Project, clip: ClipRef, start: u64) -> EditResult<ClipRef> {
    unsnapped(project, |project| match track_id(project, clip.track)? {
        TrackId::Audio(id) => {
            let old = *audio_clip(project, clip).ok_or("clip no longer exists")?;
            let tempo = project.tempo();
            let old_start = tempo
                .ticks_to_frames(old.start_tick)
                .map_err(|e| e.to_string())?;
            let new_start = tempo.ticks_to_frames(start).map_err(|e| e.to_string())?;
            let end = old_start + old.frame_count;
            if new_start >= end {
                return Err("the clip would be empty".to_owned());
            }
            let source_start = (old.source_start + new_start)
                .checked_sub(old_start)
                .ok_or("there is no earlier audio to reveal")?;
            project.remove_audio_clip(id, clip.index).map_err(text)?;
            project
                .add_audio_clip(
                    id,
                    AudioClip {
                        source_start,
                        start_tick: start,
                        frame_count: end - new_start,
                        ..old
                    },
                )
                .map_err(text)?;
            let count = project.audio_track(id).map_err(text)?.clips().len();
            Ok(ClipRef {
                track: clip.track,
                index: count - 1,
            })
        }
        TrackId::Midi(id) => {
            let old = midi_clip(project, clip)
                .ok_or("clip no longer exists")?
                .clone();
            let end = old.start_tick + old.length_ticks;
            if start >= end {
                return Err("the clip would be empty".to_owned());
            }
            let mut trimmed = old.clone();
            if start >= old.start_tick {
                trimmed.offset_ticks = old.offset_ticks + (start - old.start_tick);
            } else {
                let shift = old.start_tick - start;
                if shift <= old.offset_ticks {
                    trimmed.offset_ticks = old.offset_ticks - shift;
                } else {
                    let room = shift - old.offset_ticks;
                    for note in &mut trimmed.notes {
                        note.start_tick += room;
                    }
                    trimmed.offset_ticks = 0;
                }
            }
            trimmed.start_tick = start;
            trimmed.length_ticks = end - start;
            project.remove_midi_clip(id, clip.index).map_err(text)?;
            project.add_midi_clip(id, trimmed).map_err(text)?;
            let count = project.midi_track(id).map_err(text)?.clips().len();
            Ok(ClipRef {
                track: clip.track,
                index: count - 1,
            })
        }
        _ => Err("only audio and MIDI tracks hold clips".to_owned()),
    })
}

pub fn split_clip(project: &mut Project, clip: ClipRef, tick: u64) -> EditResult<ClipRef> {
    unsnapped(project, |project| match track_id(project, clip.track)? {
        TrackId::Audio(id) => {
            let old = *audio_clip(project, clip).ok_or("clip no longer exists")?;
            let tempo = project.tempo();
            let start = tempo
                .ticks_to_frames(old.start_tick)
                .map_err(|e| e.to_string())?;
            let split = tempo.ticks_to_frames(tick).map_err(|e| e.to_string())?;
            let end = start + old.frame_count;
            if split <= start || split >= end {
                return Err("the split point is not inside the clip".to_owned());
            }
            project
                .trim_audio_clip_end(id, clip.index, tick)
                .map_err(text)?;
            project
                .add_audio_clip(
                    id,
                    AudioClip {
                        source_start: old.source_start + (split - start),
                        start_tick: tick,
                        frame_count: end - split,
                        ..old
                    },
                )
                .map_err(text)?;
            let count = project.audio_track(id).map_err(text)?.clips().len();
            Ok(ClipRef {
                track: clip.track,
                index: count - 1,
            })
        }
        TrackId::Midi(id) => {
            let old = midi_clip(project, clip)
                .ok_or("clip no longer exists")?
                .clone();
            if tick <= old.start_tick || tick >= old.start_tick + old.length_ticks {
                return Err("the split point is not inside the clip".to_owned());
            }
            let at = tick - old.start_tick;
            let left = MidiClip {
                length_ticks: at,
                ..old.clone()
            };
            let right = MidiClip {
                start_tick: tick,
                length_ticks: old.length_ticks - at,
                offset_ticks: old.offset_ticks + at,
                notes: old.notes,
            };
            project.remove_midi_clip(id, clip.index).map_err(text)?;
            for part in [left, right] {
                project.add_midi_clip(id, part).map_err(text)?;
            }
            let count = project.midi_track(id).map_err(text)?.clips().len();
            Ok(ClipRef {
                track: clip.track,
                index: count - 1,
            })
        }
        _ => Err("only audio and MIDI tracks hold clips".to_owned()),
    })
}

pub fn duplicate_clip(project: &mut Project, clip: ClipRef) -> EditResult<ClipRef> {
    let (_, end) = clip_span(project, clip).ok_or("clip no longer exists")?;
    copy_clip_to(project, clip, clip.track, end)
}

pub fn delete_clip(project: &mut Project, clip: ClipRef) -> EditResult<()> {
    match track_id(project, clip.track)? {
        TrackId::Audio(id) => project.remove_audio_clip(id, clip.index).map(drop),
        TrackId::Midi(id) => project.remove_midi_clip(id, clip.index).map(drop),
        _ => return Err("only audio and MIDI tracks hold clips".to_owned()),
    }
    .map_err(text)
}

pub fn set_clip_gain(project: &mut Project, clip: ClipRef, gain: f32) -> EditResult<()> {
    let id = audio_id(project, clip.track)?;
    project
        .set_audio_clip_gain(id, clip.index, gain)
        .map_err(text)
}

pub fn set_clip_pitch(project: &mut Project, clip: ClipRef, semitones: f32) -> EditResult<()> {
    let id = audio_id(project, clip.track)?;
    project
        .set_audio_clip_pitch(id, clip.index, semitones)
        .map_err(text)
}

pub fn set_clip_pitch_options(
    project: &mut Project,
    clip: ClipRef,
    algorithm: PitchAlgorithm,
    preserve_formants: bool,
) -> EditResult<()> {
    let id = audio_id(project, clip.track)?;
    project
        .set_audio_clip_pitch_options(id, clip.index, algorithm, preserve_formants)
        .map_err(text)
}

pub fn reverse_clip(project: &mut Project, clip: ClipRef) -> EditResult<()> {
    let id = audio_id(project, clip.track)?;
    project.reverse_audio_clip(id, clip.index).map_err(text)
}

pub fn range_has_audio(project: &Project, tracks: &BTreeSet<usize>, start: u64, end: u64) -> bool {
    let tempo = project.tempo();
    let (Ok(from), Ok(to)) = (tempo.ticks_to_frames(start), tempo.ticks_to_frames(end)) else {
        return false;
    };
    from < to
        && tracks.iter().any(|&track| {
            let Some(Track::Audio(audio)) = project.tracks().get(track) else {
                return false;
            };
            audio.clips().iter().any(|clip| {
                tempo
                    .ticks_to_frames(clip.start_tick)
                    .is_ok_and(|first| first < to && first + clip.frame_count > from)
            })
        })
}

pub fn reverse_range(
    project: &mut Project,
    tracks: &BTreeSet<usize>,
    start: u64,
    end: u64,
) -> EditResult<Vec<ClipRef>> {
    if !range_has_audio(project, tracks, start, end) {
        return Err("the selection contains no audio".to_owned());
    }
    let mut reversed = Vec::new();
    for &track in tracks {
        if !matches!(project.tracks().get(track), Some(Track::Audio(_))) {
            continue;
        }
        split_track_at(project, track, start)?;
        split_track_at(project, track, end)?;
        for index in clips_within(project, track, start, end) {
            let clip = ClipRef { track, index };
            reverse_clip(project, clip)?;
            reversed.push(clip);
        }
    }
    Ok(reversed)
}

pub fn place_audio(
    project: &mut Project,
    audio: AudioBuffer,
    name: &str,
    track: Option<usize>,
    start: u64,
) -> EditResult<ClipRef> {
    if audio.is_empty() {
        return Err(format!("'{name}' contains no audio"));
    }
    let frame_count = audio.len() as u64;
    let track =
        match track.filter(|index| matches!(project.tracks().get(*index), Some(Track::Audio(_)))) {
            Some(index) => index,
            None => add_track(project, TrackKind::Audio, Some(name.to_owned())),
        };
    let id = audio_id(project, track)?;
    let asset = project.add_asset(audio);
    unsnapped(project, |project| {
        project
            .add_audio_clip(
                id,
                AudioClip {
                    asset,
                    source_start: 0,
                    start_tick: start,
                    frame_count,
                    gain: 1.0,
                    pitch_semitones: 0.0,
                    pitch_algorithm: PitchAlgorithm::Signalsmith,
                    preserve_formants: true,
                },
            )
            .map_err(text)
    })?;
    let count = project.audio_track(id).map_err(text)?.clips().len();
    Ok(ClipRef {
        track,
        index: count - 1,
    })
}

pub fn insert_midi_clip(
    project: &mut Project,
    track: usize,
    start: u64,
    length: u64,
) -> EditResult<ClipRef> {
    let id = midi_id(project, track)?;
    unsnapped(project, |project| {
        project
            .add_midi_clip(
                id,
                MidiClip {
                    offset_ticks: 0,
                    start_tick: start,
                    length_ticks: length.max(1),
                    notes: Vec::new(),
                },
            )
            .map_err(text)
    })?;
    let count = project.midi_track(id).map_err(text)?.clips().len();
    Ok(ClipRef {
        track,
        index: count - 1,
    })
}

fn clip_crossing(project: &Project, track: usize, tick: u64) -> Option<usize> {
    match project.tracks().get(track)? {
        Track::Audio(audio) => {
            let tempo = project.tempo();
            let at = tempo.ticks_to_frames(tick).ok()?;
            audio.clips().iter().position(|clip| {
                tempo
                    .ticks_to_frames(clip.start_tick)
                    .is_ok_and(|start| start < at && at < start + clip.frame_count)
            })
        }
        Track::Midi(midi) => midi
            .clips()
            .iter()
            .position(|clip| clip.start_tick < tick && tick < clip.start_tick + clip.length_ticks),
        _ => None,
    }
}

fn split_track_at(project: &mut Project, track: usize, tick: u64) -> EditResult<()> {
    while let Some(index) = clip_crossing(project, track, tick) {
        split_clip(project, ClipRef { track, index }, tick)?;
    }
    Ok(())
}

fn clips_within(project: &Project, track: usize, start: u64, end: u64) -> Vec<usize> {
    match project.tracks().get(track) {
        Some(Track::Audio(audio)) => {
            let tempo = project.tempo();
            let (Ok(from), Ok(to)) = (tempo.ticks_to_frames(start), tempo.ticks_to_frames(end))
            else {
                return Vec::new();
            };
            (0..audio.clips().len())
                .filter(|index| {
                    let clip = &audio.clips()[*index];
                    tempo
                        .ticks_to_frames(clip.start_tick)
                        .is_ok_and(|first| first >= from && first + clip.frame_count <= to)
                })
                .collect()
        }
        Some(Track::Midi(midi)) => (0..midi.clips().len())
            .filter(|index| {
                let clip = &midi.clips()[*index];
                clip.start_tick >= start && clip.start_tick + clip.length_ticks <= end
            })
            .collect(),
        _ => Vec::new(),
    }
}

enum Taken {
    Audio(Vec<AudioClip>),
    Midi(Vec<MidiClip>),
}

fn cut(project: &mut Project, track: usize, start: u64, end: u64) -> EditResult<Option<Taken>> {
    if !matches!(
        project.tracks().get(track),
        Some(Track::Audio(_) | Track::Midi(_))
    ) {
        return Ok(None);
    }
    split_track_at(project, track, start)?;
    split_track_at(project, track, end)?;
    let mut inside = clips_within(project, track, start, end);
    inside.sort_unstable_by(|a, b| b.cmp(a));
    Ok(Some(match track_id(project, track)? {
        TrackId::Audio(id) => {
            let mut clips = Vec::with_capacity(inside.len());
            for index in inside {
                clips.push(project.remove_audio_clip(id, index).map_err(text)?);
            }
            Taken::Audio(clips)
        }
        TrackId::Midi(id) => {
            let mut clips = Vec::with_capacity(inside.len());
            for index in inside {
                clips.push(project.remove_midi_clip(id, index).map_err(text)?);
            }
            Taken::Midi(clips)
        }
        _ => unreachable!("only audio and MIDI tracks reach here"),
    }))
}

pub fn split_range(
    project: &mut Project,
    tracks: &BTreeSet<usize>,
    start: u64,
    end: u64,
) -> EditResult<()> {
    for &track in tracks {
        split_track_at(project, track, start)?;
        split_track_at(project, track, end)?;
    }
    Ok(())
}

pub fn clear_range(
    project: &mut Project,
    tracks: &BTreeSet<usize>,
    start: u64,
    end: u64,
) -> EditResult<()> {
    if start >= end {
        return Err("the selection is empty".to_owned());
    }
    for &track in tracks {
        cut(project, track, start, end)?;
    }
    Ok(())
}

pub fn move_range(
    project: &mut Project,
    tracks: &BTreeSet<usize>,
    start: u64,
    end: u64,
    to: u64,
) -> EditResult<()> {
    if start >= end {
        return Err("the selection is empty".to_owned());
    }
    let length = end - start;
    let to_end = to
        .checked_add(length)
        .ok_or("the selection would move past the end of the timeline")?;
    unsnapped(project, |project| {
        for &track in tracks {
            let Some(taken) = cut(project, track, start, end)? else {
                continue;
            };
            cut(project, track, to, to_end)?;
            match (taken, track_id(project, track)?) {
                (Taken::Audio(clips), TrackId::Audio(id)) => {
                    for clip in clips {
                        let start_tick = clip.start_tick.saturating_sub(start) + to;
                        project
                            .add_audio_clip(id, AudioClip { start_tick, ..clip })
                            .map_err(text)?;
                    }
                }
                (Taken::Midi(clips), TrackId::Midi(id)) => {
                    for clip in clips {
                        let start_tick = clip.start_tick.saturating_sub(start) + to;
                        project
                            .add_midi_clip(id, MidiClip { start_tick, ..clip })
                            .map_err(text)?;
                    }
                }
                _ => unreachable!("a track keeps its kind"),
            }
        }
        Ok(())
    })
}

pub fn range_is_empty(project: &Project, track: usize, start: u64, end: u64) -> bool {
    let count = match project.tracks().get(track) {
        Some(Track::Audio(audio)) => audio.clips().len(),
        Some(Track::Midi(midi)) => midi.clips().len(),
        _ => 0,
    };
    (0..count).all(|index| {
        clip_span(project, ClipRef { track, index })
            .is_none_or(|(from, to)| from >= end || to <= start)
    })
}

#[cfg(test)]
#[path = "clip_copy_tests.rs"]
mod clip_copy_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use sympoie::clip::MidiNote;
    use sympoie::plugin::PluginDescriptor;
    use sympoie::render::Renderer;
    use sympoie::time::TimeSignature;

    fn device(name: &str, kind: PluginKind) -> PluginInstance {
        PluginInstance::new(
            PluginDescriptor {
                path: format!("/test/{name}.vst3").into(),
                class_id: name.to_owned(),
                name: name.to_owned(),
                vendor: "Test".to_owned(),
                kind,
            },
            Vec::new(),
            vec![1, 2, 3],
        )
    }

    #[test]
    fn plugin_insertions_preserve_order_identity_and_form_one_undo_step() {
        for track in 0..4 {
            let mut project = Project::new();
            project.add_midi_track("MIDI");
            project.add_audio_track("Audio");
            project.add_group_track("Group");
            let first = insert_plugin(
                &mut project,
                track,
                device("First", PluginKind::Effect),
                None,
            )
            .unwrap();
            let last = insert_plugin(
                &mut project,
                track,
                device("Last", PluginKind::Effect),
                None,
            )
            .unwrap();
            let original = project.tracks()[track].effects().to_vec();
            let mut document = Document::new(project, Some("plugins.symp".into()));
            let inserted = document
                .edit_structural("Load Plugin", |project| {
                    insert_plugin(
                        project,
                        track,
                        device("Middle", PluginKind::Effect),
                        Some(last),
                    )
                })
                .unwrap();
            let effects = document.project().tracks()[track].effects();
            assert_eq!(
                effects.iter().map(|plugin| plugin.id).collect::<Vec<_>>(),
                [first, inserted, last]
            );
            assert_eq!(effects[0], original[0]);
            assert_eq!(effects[2], original[1]);
            document.undo();
            assert_eq!(document.project().tracks()[track].effects(), original);
            assert!(!document.is_dirty());
            document.redo();
            assert_eq!(document.project().tracks()[track].effects()[1].id, inserted);
            let new_first = document
                .edit_structural("Load Plugin", |project| {
                    insert_plugin(
                        project,
                        track,
                        device("Before first", PluginKind::Effect),
                        Some(first),
                    )
                })
                .unwrap();
            assert_eq!(
                document.project().tracks()[track].effects()[0].id,
                new_first
            );
            let new_last = document
                .edit_structural("Load Plugin", |project| {
                    insert_plugin(project, track, device("Appended", PluginKind::Effect), None)
                })
                .unwrap();
            assert_eq!(
                document.project().tracks()[track]
                    .effects()
                    .last()
                    .unwrap()
                    .id,
                new_last
            );
        }
    }

    #[test]
    fn plugin_instruments_replace_the_existing_instrument_and_keep_effects() {
        let mut project = Project::new();
        let midi = project.add_midi_track("MIDI");
        let builtin = Instrument::Subtractive(SubtractiveSettings::default());
        project
            .set_midi_track_instrument(midi, Some(builtin))
            .unwrap();
        insert_midi_clip(&mut project, 1, 0, 3840).unwrap();
        insert_plugin(&mut project, 1, device("Effect", PluginKind::Effect), None).unwrap();
        let original = project.midi_track(midi).unwrap().clone();
        let mut document = Document::new(project, Some("plugins.symp".into()));
        let first = document
            .edit_structural("Load Plugin", |project| {
                insert_plugin(
                    project,
                    1,
                    device("First synth", PluginKind::Instrument),
                    None,
                )
            })
            .unwrap();
        let second = document
            .edit_structural("Load Plugin", |project| {
                insert_plugin(
                    project,
                    1,
                    device("Second synth", PluginKind::Instrument),
                    None,
                )
            })
            .unwrap();
        assert_ne!(first, second);
        let track = document.project().midi_track(midi).unwrap();
        assert!(track.instrument().is_none());
        assert_eq!(track.plugin_instrument().unwrap().id, second);
        assert_eq!(track.effects(), original.effects());
        assert_eq!(track.clips(), original.clips());
        document.undo();
        assert_eq!(
            document
                .project()
                .midi_track(midi)
                .unwrap()
                .plugin_instrument()
                .unwrap()
                .id,
            first
        );
        document.undo();
        let restored = document.project().midi_track(midi).unwrap();
        assert_eq!(restored.instrument(), Some(&builtin));
        assert!(restored.plugin_instrument().is_none());
        assert_eq!(restored.effects(), original.effects());
        assert!(!document.is_dirty());
    }

    #[test]
    fn invalid_plugin_drop_destinations_leave_the_document_and_history_unchanged() {
        let mut project = Project::new();
        project.add_midi_track("MIDI");
        project.add_audio_track("Audio");
        let foreign =
            insert_plugin(&mut project, 2, device("Effect", PluginKind::Effect), None).unwrap();
        let original = project.tracks()[2].effects().to_vec();
        let mut document = Document::new(project, Some("plugins.symp".into()));
        for (track, kind, before) in [
            (1, PluginKind::Effect, Some(foreign)),
            (1, PluginKind::Effect, Some(u64::MAX)),
            (2, PluginKind::Instrument, None),
            (0, PluginKind::Instrument, None),
            (1, PluginKind::Instrument, Some(foreign)),
            (usize::MAX, PluginKind::Effect, None),
        ] {
            assert!(
                document
                    .edit_structural("Load Plugin", |project| {
                        insert_plugin(project, track, device("Rejected", kind), before)
                    })
                    .is_err()
            );
        }
        assert_eq!(document.project().tracks()[2].effects(), original);
        assert!(document.project().tracks()[1].effects().is_empty());
        assert!(document.undo_label().is_none());
        assert!(!document.is_dirty());
    }

    fn ramp() -> (Project, ClipRef) {
        let mut project = Project::new();
        let frames = (0..48_000).map(|i| [i as f32; 2]).collect();
        let clip = place_audio(
            &mut project,
            AudioBuffer::new(frames).unwrap(),
            "Ramp",
            None,
            3840,
        )
        .unwrap();
        (project, clip)
    }

    fn render_at(project: &Project, frame: u64) -> f32 {
        let mut out = [[0.0; 2]; 1];
        Renderer::new(project)
            .unwrap()
            .render_block(frame, &mut out);
        out[0][0]
    }

    #[test]
    fn placing_audio_creates_a_named_track_and_ignores_the_grid() {
        let mut project = Project::new();
        project.set_snap_grid(SnapGrid::Bar);
        let clip = place_audio(
            &mut project,
            AudioBuffer::new(vec![[0.5; 2]; 10]).unwrap(),
            "Kick",
            None,
            100,
        )
        .unwrap();
        assert_eq!(clip, ClipRef { track: 1, index: 0 });
        assert_eq!(project.tracks()[1].name(), "Kick");
        assert_eq!(audio_clip(&project, clip).unwrap().start_tick, 100);
        assert_eq!(project.snap_grid(), SnapGrid::Bar);
        let second = place_audio(
            &mut project,
            AudioBuffer::new(vec![[0.5; 2]; 10]).unwrap(),
            "Snare",
            Some(1),
            0,
        )
        .unwrap();
        assert_eq!(second, ClipRef { track: 1, index: 1 });
        assert!(
            place_audio(
                &mut project,
                AudioBuffer::new(Vec::new()).unwrap(),
                "Empty",
                None,
                0
            )
            .is_err()
        );
    }

    #[test]
    fn splitting_audio_keeps_every_sample_in_place() {
        let (mut project, clip) = ramp();
        let right = split_clip(&mut project, clip, 3840 + 960).unwrap();
        assert_eq!(right.index, 1);
        let left = audio_clip(&project, clip).unwrap();
        let right_clip = audio_clip(&project, right).unwrap();
        assert_eq!(left.frame_count, 24_000);
        assert_eq!(right_clip.source_start, 24_000);
        assert_eq!(right_clip.frame_count, 24_000);
        for frame in [96_000, 119_999, 120_000, 143_999] {
            assert_eq!(render_at(&project, frame), (frame - 96_000) as f32);
        }
        assert!(split_clip(&mut project, clip, 3840).is_err());
    }

    #[test]
    fn trimming_the_start_reveals_or_hides_source_audio() {
        let (mut project, clip) = ramp();
        let trimmed = trim_start(&mut project, clip, 3840 + 480).unwrap();
        let audio = *audio_clip(&project, trimmed).unwrap();
        assert_eq!((audio.source_start, audio.frame_count), (12_000, 36_000));
        assert_eq!(render_at(&project, 108_000), 12_000.0);
        let restored = trim_start(&mut project, trimmed, 3840).unwrap();
        let audio = *audio_clip(&project, restored).unwrap();
        assert_eq!((audio.source_start, audio.frame_count), (0, 48_000));
        assert!(trim_start(&mut project, restored, 3000).is_err());
        assert_eq!(
            trim_start_target(&project, restored, 3000, Grid::OFF, false),
            Some(3840)
        );
    }

    #[test]
    fn trim_targets_respect_the_grid_and_source_length() {
        let (mut project, clip) = ramp();
        project.set_snap_grid(SnapGrid::Quarter);
        let grid = Grid::of_project(&project);
        assert_eq!(
            trim_end_target(&project, clip, 9000, grid, true),
            Some(5760)
        );
        assert_eq!(
            trim_end_target(&project, clip, 4500, grid, true),
            Some(4800)
        );
        assert_eq!(
            trim_end_target(&project, clip, 3000, grid, true),
            Some(4800)
        );
        assert_eq!(
            trim_end_target(&project, clip, 4001, grid, false),
            Some(4001)
        );
        let end = trim_end_target(&project, clip, 4500, grid, true).unwrap();
        trim_end(&mut project, clip, end).unwrap();
        assert_eq!(audio_clip(&project, clip).unwrap().frame_count, 24_000);
        assert_eq!(project.snap_grid(), SnapGrid::Quarter);
    }

    #[test]
    fn clips_move_between_tracks_of_the_same_kind() {
        let (mut project, clip) = ramp();
        let other = add_track(&mut project, TrackKind::Audio, None);
        let keys = add_track(&mut project, TrackKind::Midi, None);
        let moved = move_clip(&mut project, clip, other, 0).unwrap();
        assert_eq!(
            moved,
            ClipRef {
                track: other,
                index: 0
            }
        );
        assert_eq!(render_at(&project, 5), 5.0);
        let stayed = move_clip(&mut project, moved, keys, 960).unwrap();
        assert_eq!(stayed, moved);
        assert_eq!(audio_clip(&project, stayed).unwrap().start_tick, 960);
    }

    #[test]
    fn duplicate_and_delete() {
        let (mut project, clip) = ramp();
        let copy = duplicate_clip(&mut project, clip).unwrap();
        assert_eq!(audio_clip(&project, copy).unwrap().start_tick, 3840 + 1920);
        assert_eq!(render_at(&project, 144_000 + 7), 7.0);
        delete_clip(&mut project, clip).unwrap();
        assert_eq!(
            audio_clip(&project, ClipRef { track: 1, index: 0 })
                .unwrap()
                .start_tick,
            5760
        );
        set_clip_gain(&mut project, ClipRef { track: 1, index: 0 }, -0.5).unwrap();
        assert_eq!(render_at(&project, 144_000 + 8), -4.0);
    }

    fn note(start_tick: u64, duration_ticks: u64) -> MidiNote {
        MidiNote {
            start_tick,
            duration_ticks,
            key: 60,
            velocity: 100,
            channel: 0,
        }
    }

    fn keys() -> (Project, ClipRef) {
        let mut project = Project::new();
        let keys = add_track(&mut project, TrackKind::Midi, None);
        let id = midi_id(&project, keys).unwrap();
        project
            .set_midi_track_instrument(
                id,
                Some(Instrument::Subtractive(SubtractiveSettings::default())),
            )
            .unwrap();
        project
            .add_midi_clip(
                id,
                MidiClip {
                    start_tick: 960,
                    length_ticks: 3840,
                    offset_ticks: 0,
                    notes: vec![note(0, 480), note(960, 1920)],
                },
            )
            .unwrap();
        (
            project,
            ClipRef {
                track: keys,
                index: 0,
            },
        )
    }

    #[test]
    fn midi_split_keeps_every_note_in_both_parts() {
        let (mut project, clip) = keys();
        let right = split_clip(&mut project, clip, 960 + 1920).unwrap();
        let left = midi_clip(&project, clip).unwrap();
        let right_clip = midi_clip(&project, right).unwrap();
        assert_eq!(left.notes, [note(0, 480), note(960, 1920)]);
        assert_eq!(right_clip.notes, left.notes);
        assert_eq!((left.offset_ticks, left.length_ticks), (0, 1920));
        assert_eq!(
            (right_clip.start_tick, right_clip.offset_ticks),
            (2880, 1920)
        );
        assert_eq!(
            left.audible_notes().collect::<Vec<_>>(),
            [note(0, 480), note(960, 960)]
        );
        assert_eq!(right_clip.audible_notes().count(), 0);
        assert_eq!(
            trim_end_target(&project, right, 0, Grid::OFF, false),
            Some(2881)
        );
        assert_eq!(
            trim_start_target(&project, clip, 5000, Grid::OFF, false),
            Some(2879)
        );
    }

    #[test]
    fn trimming_a_midi_clip_hides_and_reveals_notes() {
        let (mut project, clip) = keys();
        let trimmed = trim_start(&mut project, clip, 960 + 480).unwrap();
        let shown = midi_clip(&project, trimmed).unwrap().clone();
        assert_eq!((shown.offset_ticks, shown.length_ticks), (480, 3360));
        assert_eq!(shown.notes, [note(0, 480), note(960, 1920)]);
        assert_eq!(shown.audible_notes().collect::<Vec<_>>(), [note(480, 1920)]);
        let restored = trim_start(&mut project, trimmed, 960).unwrap();
        let clip_now = midi_clip(&project, restored).unwrap();
        assert_eq!(clip_now.offset_ticks, 0);
        assert_eq!(clip_now.audible_notes().count(), 2);
        let extended = trim_start(&mut project, restored, 0).unwrap();
        let clip_now = midi_clip(&project, extended).unwrap();
        assert_eq!(clip_now.offset_ticks, 0);
        assert_eq!(clip_now.notes, [note(960, 480), note(1920, 1920)]);
        trim_end(&mut project, extended, 1920).unwrap();
        let clip_now = midi_clip(&project, extended).unwrap();
        assert_eq!(clip_now.length_ticks, 1920);
        assert_eq!(clip_now.notes.len(), 2);
        assert_eq!(clip_now.audible_notes().count(), 1);
    }

    #[test]
    fn moving_a_selection_splits_clips_at_its_edges_and_keeps_hidden_notes() {
        let (mut project, clip) = keys();
        let tracks = BTreeSet::from([clip.track]);
        move_range(&mut project, &tracks, 1920, 3840, 1920 + 3840).unwrap();
        let Track::Midi(track) = &project.tracks()[clip.track] else {
            panic!("expected MIDI")
        };
        let mut spans: Vec<_> = track
            .clips()
            .iter()
            .map(|clip| (clip.start_tick, clip.length_ticks, clip.offset_ticks))
            .collect();
        spans.sort_unstable();
        assert_eq!(spans, [(960, 960, 0), (3840, 960, 2880), (5760, 1920, 960)]);
        for part in track.clips() {
            assert_eq!(part.notes, [note(0, 480), note(960, 1920)]);
        }
        let moved = track
            .clips()
            .iter()
            .find(|clip| clip.start_tick == 5760)
            .unwrap();
        assert_eq!(moved.audible_notes().collect::<Vec<_>>(), [note(0, 1920)]);
    }

    #[test]
    fn clearing_a_selection_and_finding_empty_ranges() {
        let (mut project, clip) = keys();
        let tracks = BTreeSet::from([clip.track]);
        assert!(!range_is_empty(&project, clip.track, 0, 1000));
        assert!(range_is_empty(&project, clip.track, 0, 960));
        assert!(range_is_empty(&project, clip.track, 4800, 9600));
        clear_range(&mut project, &tracks, 1920, 2880).unwrap();
        assert!(range_is_empty(&project, clip.track, 1920, 2880));
        let Track::Midi(track) = &project.tracks()[clip.track] else {
            panic!("expected MIDI")
        };
        assert_eq!(track.clips().len(), 2);
        let before = project.clone();
        split_range(&mut project, &tracks, 1440, 3360).unwrap();
        let Track::Midi(track) = &project.tracks()[clip.track] else {
            panic!("expected MIDI")
        };
        assert_eq!(track.clips().len(), 4);
        for frame in [24_000, 30_000, 36_000, 50_000] {
            assert_eq!(render_at(&project, frame), render_at(&before, frame));
        }
        assert!(clear_range(&mut project, &tracks, 10, 10).is_err());
    }

    #[test]
    fn moving_audio_in_a_selection_keeps_its_samples() {
        let (mut project, clip) = ramp();
        let tracks = BTreeSet::from([clip.track]);
        move_range(&mut project, &tracks, 4800, 5760, 11_520).unwrap();
        assert_eq!(render_at(&project, 288_000), 24_000.0);
        assert_eq!(render_at(&project, 96_000 + 100), 100.0);
        assert_eq!(render_at(&project, 120_000 + 5), 0.0);
    }

    #[test]
    fn grids_snap_floor_and_ceil() {
        let grid = Grid::every(960);
        assert_eq!(grid.snap(479), 0);
        assert_eq!(grid.snap(480), 960);
        assert_eq!(grid.floor(1919), 960);
        assert_eq!(grid.ceil(961), 1920);
        assert_eq!(Grid::OFF.snap(7), 7);
        assert_eq!(Grid::every(0), Grid::OFF);
    }

    #[test]
    fn track_colours_are_undoable_edits() {
        let mut project = Project::new();
        let track = add_track(&mut project, TrackKind::Audio, None);
        let mut document = Document::new(project, Some("colours.symp".into()));
        document
            .edit("Track Color", |project| {
                set_track_color(project, track, Some(0x5480e4))
            })
            .unwrap();
        assert_eq!(document.project().tracks()[track].color(), Some(0x5480e4));
        assert!(
            document
                .edit("Track Color", |project| set_track_color(
                    project,
                    0,
                    Some(1)
                ))
                .is_err()
        );
        document.undo();
        assert_eq!(document.project().tracks()[track].color(), None);
    }

    #[test]
    fn grouping_and_ungrouping_preserve_the_mix() {
        let (mut project, _) = ramp();
        let second = add_track(&mut project, TrackKind::Audio, None);
        let group = group_tracks(&mut project, &[1, second]).unwrap();
        assert_eq!(project.tracks()[group].name(), "Group 1");
        assert_eq!(rows::children(&project)[group], [1, second]);
        let inner = group_tracks(&mut project, &[1]).unwrap();
        assert_eq!(rows::parents(&project)[inner], Some(group));
        let before = render_at(&project, 100_000);
        ungroup(&mut project, inner).unwrap();
        assert_eq!(rows::parents(&project)[1], Some(group));
        assert_eq!(render_at(&project, 100_000), before);
        let removed = delete_track(&mut project, group).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(project.tracks().len(), 1);
        assert!(delete_track(&mut project, 0).is_err());
    }

    #[test]
    fn new_midi_tracks_start_without_devices_or_clips() {
        let mut project = Project::new();
        let index = add_track(&mut project, TrackKind::Midi, None);
        let Track::Midi(track) = &project.tracks()[index] else {
            panic!("expected MIDI");
        };
        assert!(track.instrument().is_none());
        assert!(track.plugin_instrument().is_none());
        assert!(track.effects().is_empty());
        assert!(track.clips().is_empty());
    }

    #[test]
    fn default_names_skip_existing_ones() {
        let mut project = Project::new();
        project.set_time_signature(TimeSignature::new(3, 4).unwrap());
        add_track(&mut project, TrackKind::Audio, None);
        add_track(&mut project, TrackKind::Audio, None);
        rename_track(&mut project, 1, "  Vox  ").unwrap();
        assert_eq!(project.tracks()[1].name(), "Vox");
        assert_eq!(next_track_name(&project, TrackKind::Audio), "Audio 1");
        assert_eq!(next_track_name(&project, TrackKind::Midi), "MIDI 1");
    }
}
