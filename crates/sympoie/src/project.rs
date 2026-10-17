use std::collections::{BTreeSet, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

use crate::Frame;
use crate::automation::{
    AutomationLane, AutomationTarget, MAX_AUTOMATION_LANES, SubtractiveParameter,
};
use crate::clip::{AudioClip, MidiClip, MidiNote};
use crate::instrument::{Instrument, InstrumentError};
use crate::pitch::{PitchAlgorithm, PitchError};
use crate::plugin::{
    MAX_PLUGIN_STATE_BYTES, PLUGIN_TAIL_FRAMES, PluginError, PluginInstance, PluginKind,
    PluginParameter, SidechainRoute, SidechainTap,
};
use crate::time::{SnapGrid, Tempo, TimeError, TimeSignature};
use crate::track::{
    AudioOutput, AudioRecordingInput, AudioTrack, AudioTrackId, BusTrack, BusTrackId, GroupTrack,
    GroupTrackId, MasterTrack, MasterTrackId, MidiRecordingInput, MidiTrack, MidiTrackId,
    MixerSettings, Track, TrackId, TrackSend,
};

#[derive(Debug)]
pub struct AudioBuffer {
    samples: Vec<Frame>,
    pitch_cache: Mutex<VecDeque<CachedPitch>>,
}

#[derive(Debug)]
struct CachedPitch {
    key: PitchKey,
    samples: Weak<[Frame]>,
    retained: Option<Arc<[Frame]>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PitchKey {
    semitones: u32,
    algorithm: PitchAlgorithm,
    preserve_formants: bool,
}

impl PitchKey {
    fn for_clip(clip: &AudioClip) -> Self {
        Self {
            semitones: clip.pitch_semitones.to_bits(),
            algorithm: clip.pitch_algorithm,
            preserve_formants: clip.preserve_formants && clip.pitch_algorithm.supports_formants(),
        }
    }
}

#[cfg(test)]
impl From<f32> for PitchKey {
    fn from(semitones: f32) -> Self {
        Self {
            semitones: semitones.to_bits(),
            algorithm: PitchAlgorithm::Builtin,
            preserve_formants: false,
        }
    }
}

impl Clone for AudioBuffer {
    fn clone(&self) -> Self {
        Self {
            samples: self.samples.clone(),
            pitch_cache: Mutex::default(),
        }
    }
}

impl AudioBuffer {
    pub fn new(samples: Vec<Frame>) -> Result<Self, ProjectError> {
        for (frame, sample) in samples.iter().enumerate() {
            for (channel, value) in sample.iter().enumerate() {
                if !value.is_finite() {
                    return Err(ProjectError::NonFiniteSample { frame, channel });
                }
            }
        }
        Ok(Self {
            samples,
            pitch_cache: Mutex::default(),
        })
    }

    pub fn samples(&self) -> &[Frame] {
        &self.samples
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub(crate) fn processed_samples(&self, clip: &AudioClip) -> Result<Arc<[Frame]>, PitchError> {
        self.prepare_pitched_samples(PitchKey::for_clip(clip), || false)?
            .ok_or_else(|| PitchError("pitch preparation was unexpectedly cancelled".into()))
    }

    #[cfg(test)]
    pub(crate) fn pitched_samples(&self, semitones: f32) -> Arc<[Frame]> {
        self.prepare_pitched_samples(semitones.into(), || false)
            .expect("built-in pitch preparation")
            .expect("uncancelled pitch preparation")
    }

    fn cached_pitched_samples(&self, key: PitchKey) -> Option<Arc<[Frame]>> {
        let mut cache = self
            .pitch_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let index = cache.iter().position(|entry| entry.key == key)?;
        let mut entry = cache.remove(index).expect("located cached pitch");
        let samples = entry.samples.upgrade()?;
        entry.retained = Some(Arc::clone(&samples));
        cache.push_back(entry);
        Self::limit_pitch_retention(&mut cache);
        Some(samples)
    }

    fn pitch_is_prepared(&self, key: PitchKey) -> bool {
        self.pitch_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|entry| entry.key == key && entry.samples.strong_count() > 0)
    }

    fn prepare_pitched_samples(
        &self,
        key: PitchKey,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Arc<[Frame]>>, PitchError> {
        if cancelled() {
            return Ok(None);
        }
        if let Some(samples) = self.cached_pitched_samples(key) {
            return Ok(Some(samples));
        }
        let Some(shifted) = crate::pitch::process_cancellable(
            &self.samples,
            f32::from_bits(key.semitones),
            key.algorithm,
            key.preserve_formants,
            &cancelled,
        )?
        else {
            return Ok(None);
        };
        if cancelled() {
            return Ok(None);
        }
        let samples: Arc<[Frame]> = shifted.into_owned().into();
        if cancelled() {
            return Ok(None);
        }
        let mut cache = self
            .pitch_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = cache.iter().position(|entry| entry.key == key) {
            let mut entry = cache.remove(index).expect("located cached pitch");
            if let Some(existing) = entry.samples.upgrade() {
                entry.retained = Some(Arc::clone(&existing));
                cache.push_back(entry);
                Self::limit_pitch_retention(&mut cache);
                return Ok(Some(existing));
            }
        }
        cache.push_back(CachedPitch {
            key,
            samples: Arc::downgrade(&samples),
            retained: Some(Arc::clone(&samples)),
        });
        Self::limit_pitch_retention(&mut cache);
        Ok(Some(samples))
    }

    fn limit_pitch_retention(cache: &mut VecDeque<CachedPitch>) {
        const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
        let mut retained_count = 0usize;
        let mut retained_bytes = 0usize;
        for entry in cache.iter_mut().rev() {
            if let Some(samples) = &entry.retained {
                let bytes = std::mem::size_of_val(samples.as_ref());
                if retained_count >= 2 || bytes > MAX_RETAINED_BYTES - retained_bytes {
                    entry.retained = None;
                } else {
                    retained_count += 1;
                    retained_bytes += bytes;
                }
            }
        }
        cache.retain(|entry| entry.samples.strong_count() != 0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetId(usize);

impl AssetId {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    assets: Vec<Arc<AudioBuffer>>,
    tracks: Vec<Track>,
    tempo: Tempo,
    time_signature: TimeSignature,
    snap_grid: SnapGrid,
    next_plugin_id: u64,
    next_uid: u64,
}

impl Default for Project {
    fn default() -> Self {
        let mut project = Self {
            assets: Vec::new(),
            tracks: vec![Track::Master(MasterTrack::new())],
            tempo: Tempo::default(),
            time_signature: TimeSignature::default(),
            snap_grid: SnapGrid::Off,
            next_plugin_id: 1,
            next_uid: 1,
        };
        project.assign_uids();
        project
    }
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prepare_audio_pitch(
        &self,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Vec<Arc<[Frame]>>>, PitchError> {
        let mut seen = BTreeSet::new();
        let mut prepared = Vec::new();
        for clip in self.audio_clips() {
            if cancelled() {
                return Ok(None);
            }
            let key = PitchKey::for_clip(clip);
            if clip.pitch_semitones != 0.0 && seen.insert((clip.asset.index(), key)) {
                let Some(samples) = self
                    .asset(clip.asset)
                    .prepare_pitched_samples(key, &cancelled)?
                else {
                    return Ok(None);
                };
                prepared.push(samples);
            }
        }
        Ok((!cancelled()).then_some(prepared))
    }

    pub fn cached_audio_pitch(&self) -> Option<Vec<Arc<[Frame]>>> {
        let mut seen = BTreeSet::new();
        let mut prepared = Vec::new();
        for clip in self.audio_clips() {
            if clip.pitch_semitones != 0.0
                && seen.insert((clip.asset.index(), PitchKey::for_clip(clip)))
            {
                prepared.push(
                    self.asset(clip.asset)
                        .cached_pitched_samples(PitchKey::for_clip(clip))?,
                );
            }
        }
        Some(prepared)
    }

    pub fn audio_pitch_is_prepared(&self) -> bool {
        self.audio_clips().all(|clip| {
            clip.pitch_semitones == 0.0
                || self
                    .asset(clip.asset)
                    .pitch_is_prepared(PitchKey::for_clip(clip))
        })
    }

    pub fn save(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), crate::project_file::ProjectFileError> {
        crate::project_file::save_project(self, path)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, crate::project_file::ProjectFileError> {
        crate::project_file::load_project(path)
    }

    pub fn tempo(&self) -> Tempo {
        self.tempo
    }

    pub fn bpm(&self) -> f64 {
        self.tempo.bpm()
    }

    pub fn set_bpm(&mut self, bpm: f64) -> Result<(), ProjectError> {
        let tempo = Tempo::new(bpm)?;
        for clip in self.audio_clips() {
            Self::audio_clip_end_frame(tempo, clip)?;
        }
        for lane in self.tracks.iter().flat_map(Track::automation) {
            if let Some(point) = lane.points().last() {
                tempo.ticks_to_frames(point.tick)?;
            }
        }
        self.tempo = tempo;
        Ok(())
    }

    pub fn time_signature(&self) -> TimeSignature {
        self.time_signature
    }

    pub fn set_time_signature(&mut self, time_signature: TimeSignature) {
        self.time_signature = time_signature;
    }

    pub fn snap_grid(&self) -> SnapGrid {
        self.snap_grid
    }

    pub fn set_snap_grid(&mut self, grid: SnapGrid) {
        self.snap_grid = grid;
    }

    pub fn snap_tick(&self, tick: u64) -> Result<u64, TimeError> {
        self.snap_grid.snap(tick, self.time_signature)
    }

    pub fn master_track_id(&self) -> MasterTrackId {
        MasterTrackId(0)
    }

    pub fn master_track(&self) -> &MasterTrack {
        match &self.tracks[0] {
            Track::Master(track) => track,
            _ => unreachable!("a project always has its master at index zero"),
        }
    }

    pub fn add_asset(&mut self, asset: AudioBuffer) -> AssetId {
        let id = AssetId(self.assets.len());
        self.assets.push(Arc::new(asset));
        id
    }

    pub fn add_shared_asset(&mut self, asset: Arc<AudioBuffer>) -> AssetId {
        if let Some(index) = self.assets.iter().position(|old| Arc::ptr_eq(old, &asset)) {
            return AssetId(index);
        }
        let id = AssetId(self.assets.len());
        self.assets.push(asset);
        id
    }

    pub fn shared_audio_asset(&self, id: AssetId) -> Result<Arc<AudioBuffer>, ProjectError> {
        self.assets
            .get(id.0)
            .cloned()
            .ok_or(ProjectError::UnknownAsset { asset: id })
    }

    pub fn audio_asset(&self, id: AssetId) -> Result<&AudioBuffer, ProjectError> {
        self.assets
            .get(id.0)
            .map(Arc::as_ref)
            .ok_or(ProjectError::UnknownAsset { asset: id })
    }

    pub fn add_audio_track(&mut self, name: impl Into<String>) -> AudioTrackId {
        let id = AudioTrackId(self.tracks.len());
        self.tracks.push(Track::Audio(AudioTrack::new(name.into())));
        let uid = self.fresh_uid();
        self.tracks.last_mut().unwrap().set_uid(uid);
        id
    }

    pub fn add_midi_track(&mut self, name: impl Into<String>) -> MidiTrackId {
        let id = MidiTrackId(self.tracks.len());
        self.tracks.push(Track::Midi(MidiTrack::new(name.into())));
        let uid = self.fresh_uid();
        self.tracks.last_mut().unwrap().set_uid(uid);
        id
    }

    pub fn add_group_track(&mut self, name: impl Into<String>) -> GroupTrackId {
        let id = GroupTrackId(self.tracks.len());
        self.tracks.push(Track::Group(GroupTrack::new(name.into())));
        let uid = self.fresh_uid();
        self.tracks.last_mut().unwrap().set_uid(uid);
        id
    }

    pub fn add_bus_track(&mut self, name: impl Into<String>) -> BusTrackId {
        let id = BusTrackId(self.tracks.len());
        self.tracks.push(Track::Bus(BusTrack::new(name.into())));
        let uid = self.fresh_uid();
        self.tracks.last_mut().unwrap().set_uid(uid);
        id
    }

    pub fn bus_track(&self, id: BusTrackId) -> Result<&BusTrack, ProjectError> {
        match self.tracks.get(id.0) {
            Some(Track::Bus(track)) => Ok(track),
            _ => Err(ProjectError::UnknownBusTrack { track: id }),
        }
    }

    pub fn audio_track(&self, id: AudioTrackId) -> Result<&AudioTrack, ProjectError> {
        match self.tracks.get(id.0) {
            Some(Track::Audio(track)) => Ok(track),
            _ => Err(ProjectError::UnknownAudioTrack { track: id }),
        }
    }

    pub fn midi_track(&self, id: MidiTrackId) -> Result<&MidiTrack, ProjectError> {
        match self.tracks.get(id.0) {
            Some(Track::Midi(track)) => Ok(track),
            _ => Err(ProjectError::UnknownMidiTrack { track: id }),
        }
    }

    pub fn set_audio_recording_input(
        &mut self,
        id: AudioTrackId,
        input: AudioRecordingInput,
    ) -> Result<(), ProjectError> {
        self.audio_track(id)?;
        input.validate()?;
        let Some(Track::Audio(track)) = self.tracks.get_mut(id.0) else {
            unreachable!("audio track was checked before mutation");
        };
        track.set_recording_input(input);
        Ok(())
    }

    pub fn set_midi_recording_input(
        &mut self,
        id: MidiTrackId,
        input: MidiRecordingInput,
    ) -> Result<(), ProjectError> {
        self.midi_track(id)?;
        input.validate()?;
        let Some(Track::Midi(track)) = self.tracks.get_mut(id.0) else {
            unreachable!("MIDI track was checked before mutation");
        };
        track.set_recording_input(input);
        Ok(())
    }

    pub fn group_track(&self, id: GroupTrackId) -> Result<&GroupTrack, ProjectError> {
        match self.tracks.get(id.0) {
            Some(Track::Group(track)) => Ok(track),
            _ => Err(ProjectError::UnknownGroupTrack { track: id }),
        }
    }

    pub fn add_audio_clip(
        &mut self,
        id: AudioTrackId,
        mut clip: AudioClip,
    ) -> Result<(), ProjectError> {
        self.audio_track(id)?;
        clip.start_tick = self.snap_tick(clip.start_tick)?;
        self.validate_audio_clip(&clip)?;
        let track = match self.tracks.get_mut(id.0) {
            Some(Track::Audio(track)) => track,
            _ => unreachable!("audio track was checked before mutation"),
        };
        track.push_clip(clip);
        let uid = self.fresh_uid();
        if let Some(last) = self.tracks[id.0].clip_uids_mut().last_mut() {
            *last = uid;
        }
        Ok(())
    }

    fn validate_audio_clip(&self, clip: &AudioClip) -> Result<(), ProjectError> {
        let asset = self
            .assets
            .get(clip.asset.0)
            .ok_or(ProjectError::UnknownAsset { asset: clip.asset })?;
        if clip.frame_count == 0 {
            return Err(ProjectError::EmptyClip);
        }
        if !clip.gain.is_finite() {
            return Err(ProjectError::NonFiniteGain);
        }
        if !crate::pitch::valid_semitones(clip.pitch_semitones) {
            return Err(ProjectError::InvalidClipPitch);
        }
        let source_end = clip
            .source_start
            .checked_add(clip.frame_count)
            .ok_or(ProjectError::SourceRangeOverflow)?;
        if source_end > asset.len() as u64 {
            return Err(ProjectError::SourceRangeOutOfBounds {
                start: clip.source_start,
                frame_count: clip.frame_count,
                asset_frames: asset.len() as u64,
            });
        }
        Self::audio_clip_end_frame(self.tempo, clip)?;
        Ok(())
    }

    fn audio_clip_end_frame(tempo: Tempo, clip: &AudioClip) -> Result<u64, ProjectError> {
        tempo
            .ticks_to_frames(clip.start_tick)?
            .checked_add(clip.frame_count)
            .ok_or(ProjectError::TimelineRangeOverflow)
    }

    pub fn add_midi_clip(
        &mut self,
        id: MidiTrackId,
        mut clip: MidiClip,
    ) -> Result<(), ProjectError> {
        self.midi_track(id)?;
        clip.start_tick = self.snap_tick(clip.start_tick)?;
        Self::validate_midi_clip(&clip)?;
        let track = match self.tracks.get_mut(id.0) {
            Some(Track::Midi(track)) => track,
            _ => unreachable!("MIDI track was checked before mutation"),
        };
        track.push_clip(clip);
        let uid = self.fresh_uid();
        if let Some(last) = self.tracks[id.0].clip_uids_mut().last_mut() {
            *last = uid;
        }
        Ok(())
    }

    fn validate_midi_clip(clip: &MidiClip) -> Result<(), ProjectError> {
        if clip.length_ticks == 0 {
            return Err(ProjectError::EmptyMidiClip);
        }
        clip.start_tick
            .checked_add(clip.length_ticks)
            .ok_or(ProjectError::MidiTimelineRangeOverflow)?;
        clip.offset_ticks
            .checked_add(clip.length_ticks)
            .ok_or(ProjectError::MidiTimelineRangeOverflow)?;

        for (index, note) in clip.notes.iter().enumerate() {
            if note.key > 127 {
                return Err(ProjectError::InvalidMidiKey {
                    note: index,
                    key: note.key,
                });
            }
            if !(1..=127).contains(&note.velocity) {
                return Err(ProjectError::InvalidMidiVelocity {
                    note: index,
                    velocity: note.velocity,
                });
            }
            if note.channel > 15 {
                return Err(ProjectError::InvalidMidiChannel {
                    note: index,
                    channel: note.channel,
                });
            }
            if note.duration_ticks == 0 {
                return Err(ProjectError::EmptyMidiNote { note: index });
            }
            note.start_tick
                .checked_add(note.duration_ticks)
                .ok_or(ProjectError::MidiNoteRangeOverflow { note: index })?;
        }
        Ok(())
    }

    pub fn move_audio_clip(
        &mut self,
        track: AudioTrackId,
        index: usize,
        start_tick: u64,
    ) -> Result<(), ProjectError> {
        let mut clip = *self.audio_clip(track, index)?;
        clip.start_tick = self.snap_tick(start_tick)?;
        self.replace_audio_clip(track, index, clip)
    }

    pub fn set_audio_clip_pitch(
        &mut self,
        track: AudioTrackId,
        index: usize,
        semitones: f32,
    ) -> Result<(), ProjectError> {
        let mut clip = *self.audio_clip(track, index)?;
        clip.pitch_semitones = semitones;
        self.replace_audio_clip(track, index, clip)
    }

    pub fn set_audio_clip_pitch_options(
        &mut self,
        track: AudioTrackId,
        index: usize,
        algorithm: PitchAlgorithm,
        preserve_formants: bool,
    ) -> Result<(), ProjectError> {
        let mut clip = *self.audio_clip(track, index)?;
        clip.pitch_algorithm = algorithm;
        clip.preserve_formants = preserve_formants;
        self.replace_audio_clip(track, index, clip)
    }

    pub fn move_midi_clip(
        &mut self,
        track: MidiTrackId,
        index: usize,
        start_tick: u64,
    ) -> Result<(), ProjectError> {
        let mut clip = self.midi_clip(track, index)?.clone();
        clip.start_tick = self.snap_tick(start_tick)?;
        self.replace_midi_clip(track, index, clip)
    }

    pub fn set_midi_clip_notes(
        &mut self,
        track: MidiTrackId,
        index: usize,
        notes: Vec<MidiNote>,
    ) -> Result<(), ProjectError> {
        let mut clip = self.midi_clip(track, index)?.clone();
        clip.notes = notes;
        self.replace_midi_clip(track, index, clip)
    }

    pub fn trim_audio_clip_end(
        &mut self,
        track: AudioTrackId,
        index: usize,
        end_tick: u64,
    ) -> Result<(), ProjectError> {
        let mut clip = *self.audio_clip(track, index)?;
        let end_frame = self.tempo.ticks_to_frames(self.snap_tick(end_tick)?)?;
        clip.frame_count = end_frame
            .checked_sub(self.audio_clip_start_frame(&clip))
            .ok_or(ProjectError::EmptyClip)?;
        self.replace_audio_clip(track, index, clip)
    }

    pub fn trim_midi_clip_end(
        &mut self,
        track: MidiTrackId,
        index: usize,
        end_tick: u64,
    ) -> Result<(), ProjectError> {
        let mut clip = self.midi_clip(track, index)?.clone();
        clip.length_ticks = self
            .snap_tick(end_tick)?
            .checked_sub(clip.start_tick)
            .ok_or(ProjectError::EmptyMidiClip)?;
        self.replace_midi_clip(track, index, clip)
    }

    pub fn set_audio_clip_gain(
        &mut self,
        track: AudioTrackId,
        index: usize,
        gain: f32,
    ) -> Result<(), ProjectError> {
        let mut clip = *self.audio_clip(track, index)?;
        clip.gain = gain;
        self.replace_audio_clip(track, index, clip)
    }

    pub fn reverse_audio_clip(
        &mut self,
        track: AudioTrackId,
        index: usize,
    ) -> Result<(), ProjectError> {
        let mut clip = *self.audio_clip(track, index)?;
        self.validate_audio_clip(&clip)?;
        let mut audio = self.audio_asset(clip.asset)?.clone();
        let start = clip.source_start as usize;
        let end = start + clip.frame_count as usize;
        audio.samples[start..end].reverse();
        clip.asset = self.add_asset(audio);
        self.replace_audio_clip(track, index, clip)
    }

    pub fn remove_audio_clip(
        &mut self,
        track: AudioTrackId,
        index: usize,
    ) -> Result<AudioClip, ProjectError> {
        self.audio_clip(track, index)?;
        match &mut self.tracks[track.0] {
            Track::Audio(track) => Ok(track.remove_clip(index)),
            _ => unreachable!("audio track was checked before mutation"),
        }
    }

    pub fn remove_midi_clip(
        &mut self,
        track: MidiTrackId,
        index: usize,
    ) -> Result<MidiClip, ProjectError> {
        self.midi_clip(track, index)?;
        match &mut self.tracks[track.0] {
            Track::Midi(track) => Ok(track.remove_clip(index)),
            _ => unreachable!("MIDI track was checked before mutation"),
        }
    }

    fn audio_clip(&self, track: AudioTrackId, index: usize) -> Result<&AudioClip, ProjectError> {
        self.audio_track(track)?
            .clips()
            .get(index)
            .ok_or(ProjectError::UnknownAudioClip { track, index })
    }

    fn midi_clip(&self, track: MidiTrackId, index: usize) -> Result<&MidiClip, ProjectError> {
        self.midi_track(track)?
            .clips()
            .get(index)
            .ok_or(ProjectError::UnknownMidiClip { track, index })
    }

    fn replace_audio_clip(
        &mut self,
        track: AudioTrackId,
        index: usize,
        clip: AudioClip,
    ) -> Result<(), ProjectError> {
        self.validate_audio_clip(&clip)?;
        match &mut self.tracks[track.0] {
            Track::Audio(track) => track.replace_clip(index, clip),
            _ => unreachable!("audio track was checked before mutation"),
        }
        Ok(())
    }

    fn replace_midi_clip(
        &mut self,
        track: MidiTrackId,
        index: usize,
        clip: MidiClip,
    ) -> Result<(), ProjectError> {
        Self::validate_midi_clip(&clip)?;
        match &mut self.tracks[track.0] {
            Track::Midi(track) => track.replace_clip(index, clip),
            _ => unreachable!("MIDI track was checked before mutation"),
        }
        Ok(())
    }

    pub fn set_audio_track_mixer(
        &mut self,
        id: AudioTrackId,
        mixer: MixerSettings,
    ) -> Result<(), ProjectError> {
        self.set_track_mixer(id.into(), mixer)
    }

    pub fn set_midi_track_mixer(
        &mut self,
        id: MidiTrackId,
        mixer: MixerSettings,
    ) -> Result<(), ProjectError> {
        self.set_track_mixer(id.into(), mixer)
    }

    pub fn set_midi_track_instrument(
        &mut self,
        id: MidiTrackId,
        instrument: Option<Instrument>,
    ) -> Result<(), ProjectError> {
        self.midi_track(id)?;
        if let Some(instrument) = instrument {
            instrument.validate()?;
        }
        match &mut self.tracks[id.0] {
            Track::Midi(track) => track.set_instrument(instrument),
            _ => unreachable!("MIDI track was checked before mutation"),
        }
        self.tracks[id.0].prune_automation();
        Ok(())
    }

    pub fn set_midi_track_instrument_bypassed(
        &mut self,
        id: MidiTrackId,
        bypassed: bool,
    ) -> Result<(), ProjectError> {
        self.midi_track(id)?;
        match &mut self.tracks[id.0] {
            Track::Midi(track) => track.set_instrument_bypassed(bypassed),
            _ => unreachable!("MIDI track was checked before mutation"),
        }
        Ok(())
    }

    pub fn set_plugin_instrument(
        &mut self,
        id: MidiTrackId,
        mut instrument: Option<PluginInstance>,
    ) -> Result<(), ProjectError> {
        self.midi_track(id)?;
        if let Some(instance) = &mut instrument {
            Self::validate_plugin_kind(instance, PluginKind::Instrument)?;
            crate::builtin::complete_parameters(instance)?;
            instance.id = self.allocate_plugin_id()?;
        }
        match &mut self.tracks[id.0] {
            Track::Midi(track) => track.set_plugin_instrument(instrument),
            _ => unreachable!("MIDI track was checked before mutation"),
        }
        self.tracks[id.0].prune_automation();
        Ok(())
    }

    pub fn add_effect(
        &mut self,
        track: TrackId,
        mut effect: PluginInstance,
    ) -> Result<u64, ProjectError> {
        self.track(track)?;
        Self::validate_plugin_kind(&effect, PluginKind::Effect)?;
        crate::builtin::complete_parameters(&mut effect)?;
        self.tracks[track.index()].effects_mut().push(effect);
        let inserted = self
            .processing_order()
            .and_then(|_| self.allocate_plugin_id());
        let id = match inserted {
            Ok(id) => id,
            Err(error) => {
                self.tracks[track.index()].effects_mut().pop();
                return Err(error);
            }
        };
        self.tracks[track.index()]
            .effects_mut()
            .last_mut()
            .unwrap()
            .id = id;
        Ok(id)
    }

    pub fn remove_effect(
        &mut self,
        track: TrackId,
        index: usize,
    ) -> Result<PluginInstance, ProjectError> {
        self.validate_effect_index(track, index)?;
        let removed = self.tracks[track.index()].effects_mut().remove(index);
        self.tracks[track.index()].prune_automation();
        Ok(removed)
    }

    pub fn move_effect(
        &mut self,
        track: TrackId,
        from: usize,
        to: usize,
    ) -> Result<(), ProjectError> {
        self.validate_effect_index(track, from)?;
        self.validate_effect_index(track, to)?;
        let effects = self.tracks[track.index()].effects_mut();
        let effect = effects.remove(from);
        effects.insert(to, effect);
        Ok(())
    }

    fn validate_effect_index(&self, track: TrackId, index: usize) -> Result<(), ProjectError> {
        if index >= self.track(track)?.effects().len() {
            return Err(ProjectError::UnknownEffect { track, index });
        }
        Ok(())
    }

    pub fn add_mixer_effect(
        &mut self,
        track: TrackId,
        mut effect: PluginInstance,
    ) -> Result<u64, ProjectError> {
        self.track(track)?;
        Self::validate_plugin_kind(&effect, PluginKind::Effect)?;
        crate::builtin::complete_parameters(&mut effect)?;
        self.tracks[track.index()].mixer_effects_mut().push(effect);
        let inserted = self
            .processing_order()
            .and_then(|_| self.allocate_plugin_id());
        let id = match inserted {
            Ok(id) => id,
            Err(error) => {
                self.tracks[track.index()].mixer_effects_mut().pop();
                return Err(error);
            }
        };
        self.tracks[track.index()]
            .mixer_effects_mut()
            .last_mut()
            .unwrap()
            .id = id;
        Ok(id)
    }

    pub fn remove_mixer_effect(
        &mut self,
        track: TrackId,
        index: usize,
    ) -> Result<PluginInstance, ProjectError> {
        self.validate_mixer_effect_index(track, index)?;
        let removed = self.tracks[track.index()].mixer_effects_mut().remove(index);
        self.tracks[track.index()].prune_automation();
        Ok(removed)
    }

    pub fn move_mixer_effect(
        &mut self,
        track: TrackId,
        from: usize,
        to: usize,
    ) -> Result<(), ProjectError> {
        self.validate_mixer_effect_index(track, from)?;
        self.validate_mixer_effect_index(track, to)?;
        let effects = self.tracks[track.index()].mixer_effects_mut();
        let effect = effects.remove(from);
        effects.insert(to, effect);
        Ok(())
    }

    fn validate_mixer_effect_index(
        &self,
        track: TrackId,
        index: usize,
    ) -> Result<(), ProjectError> {
        if index >= self.track(track)?.mixer_effects().len() {
            return Err(ProjectError::UnknownEffect { track, index });
        }
        Ok(())
    }

    pub fn set_track_automation(
        &mut self,
        track: impl Into<TrackId>,
        lane: AutomationLane,
    ) -> Result<(), ProjectError> {
        let id = track.into();
        let track = self.track(id)?;
        if lane.target().value(track).is_none() {
            return Err(ProjectError::InvalidAutomationTarget);
        }
        if let Some(point) = lane.points().last() {
            self.tempo.ticks_to_frames(point.tick)?;
        }
        let existing = track
            .automation()
            .iter()
            .position(|existing| existing.target() == lane.target());
        if existing.is_none() && track.automation().len() >= MAX_AUTOMATION_LANES {
            return Err(ProjectError::TooManyAutomationLanes);
        }
        let lanes = self.tracks[id.index()].automation_mut();
        if let Some(index) = existing {
            lanes[index] = lane;
        } else {
            lanes.push(lane);
        }
        Ok(())
    }

    pub fn remove_track_automation(
        &mut self,
        track: impl Into<TrackId>,
        target: AutomationTarget,
    ) -> Result<(), ProjectError> {
        let id = track.into();
        self.track(id)?;
        self.tracks[id.index()]
            .automation_mut()
            .retain(|lane| lane.target() != target);
        Ok(())
    }

    pub fn plugin(&self, instance_id: u64) -> Option<&PluginInstance> {
        for track in &self.tracks {
            if let Track::Midi(midi) = track
                && let Some(instrument) = midi.plugin_instrument()
                && instrument.id == instance_id
            {
                return Some(instrument);
            }
            if let Some(effect) = track.all_effects().find(|effect| effect.id == instance_id) {
                return Some(effect);
            }
        }
        None
    }

    fn plugin_mut(&mut self, instance_id: u64) -> Option<&mut PluginInstance> {
        let location = self.tracks.iter().enumerate().find_map(|(index, track)| {
            if let Track::Midi(midi) = track
                && midi
                    .plugin_instrument()
                    .is_some_and(|plugin| plugin.id == instance_id)
            {
                return Some((index, None));
            }
            track
                .all_effects()
                .position(|effect| effect.id == instance_id)
                .map(|effect| (index, Some(effect)))
        })?;
        let track = &mut self.tracks[location.0];
        match location.1 {
            Some(index) => track.all_effects_mut().nth(index),
            None => match track {
                Track::Midi(midi) => midi.plugin_instrument_mut(),
                _ => unreachable!("instrument location resolved from a MIDI track"),
            },
        }
    }

    pub fn set_plugin_parameter(
        &mut self,
        instance_id: u64,
        parameter_id: u32,
        value: f64,
    ) -> Result<(), ProjectError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(PluginError::InvalidParameterValue.into());
        }
        let instance = self
            .plugin_mut(instance_id)
            .ok_or(PluginError::UnknownInstance(instance_id))?;
        let parameter = instance
            .parameters
            .iter_mut()
            .find(|parameter| parameter.id == parameter_id)
            .ok_or(PluginError::UnknownParameter(parameter_id))?;
        if parameter.read_only {
            return Err(PluginError::ReadOnlyParameter(parameter_id).into());
        }
        parameter.value = value;
        Ok(())
    }

    pub fn set_plugin_bypassed(
        &mut self,
        instance_id: u64,
        bypassed: bool,
    ) -> Result<(), ProjectError> {
        self.plugin_mut(instance_id)
            .ok_or(PluginError::UnknownInstance(instance_id))?
            .bypassed = bypassed;
        Ok(())
    }

    pub fn set_plugin_sidechain(
        &mut self,
        instance_id: u64,
        route: Option<SidechainRoute>,
    ) -> Result<(), ProjectError> {
        let instance = self
            .plugin(instance_id)
            .ok_or(PluginError::UnknownInstance(instance_id))?;
        if instance.descriptor.kind != PluginKind::Effect {
            return Err(PluginError::SidechainRequiresEffect.into());
        }
        if let Some(route) = route {
            route.validate()?;
        }
        let previous = instance.sidechain;
        self.plugin_mut(instance_id).unwrap().sidechain = route;
        if previous.map(|route| route.source_track) != route.map(|route| route.source_track)
            && let Err(error) = self.processing_order()
        {
            self.plugin_mut(instance_id).unwrap().sidechain = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn set_plugin_state(
        &mut self,
        instance_id: u64,
        state: Vec<u8>,
    ) -> Result<(), ProjectError> {
        if state.len() > MAX_PLUGIN_STATE_BYTES {
            return Err(PluginError::StateTooLarge.into());
        }
        self.plugin_mut(instance_id)
            .ok_or(PluginError::UnknownInstance(instance_id))?
            .state = state;
        Ok(())
    }

    pub fn set_plugin_snapshot(
        &mut self,
        instance_id: u64,
        parameters: Vec<PluginParameter>,
        state: Vec<u8>,
    ) -> Result<(), ProjectError> {
        let instance = self
            .plugin_mut(instance_id)
            .ok_or(PluginError::UnknownInstance(instance_id))?;
        let mut updated = PluginInstance {
            id: instance.id,
            descriptor: instance.descriptor.clone(),
            parameters,
            state,
            bypassed: instance.bypassed,
            sidechain: instance.sidechain,
        };
        crate::builtin::complete_parameters(&mut updated)?;
        *instance = updated;
        for track in &mut self.tracks {
            track.prune_automation();
        }
        Ok(())
    }

    fn validate_plugin_kind(
        instance: &PluginInstance,
        kind: PluginKind,
    ) -> Result<(), ProjectError> {
        instance.validate()?;
        if instance.descriptor.kind != kind {
            return Err(PluginError::WrongKind.into());
        }
        Ok(())
    }

    fn allocate_plugin_id(&mut self) -> Result<u64, ProjectError> {
        let id = self.next_plugin_id;
        self.next_plugin_id = id.checked_add(1).ok_or(PluginError::InvalidInstanceId)?;
        Ok(id)
    }

    pub(crate) fn next_plugin_id(&self) -> u64 {
        self.next_plugin_id
    }

    pub(crate) fn restore_next_plugin_id(&mut self, id: u64) -> Result<(), ProjectError> {
        if id < self.next_plugin_id || id == 0 {
            return Err(PluginError::InvalidInstanceId.into());
        }
        self.next_plugin_id = id;
        Ok(())
    }

    pub(crate) fn restore_plugin(
        &mut self,
        track: TrackId,
        mut instance: PluginInstance,
        instrument: bool,
    ) -> Result<(), ProjectError> {
        self.track(track)?;
        if instrument && !matches!(track, TrackId::Midi(_)) {
            return Err(PluginError::WrongKind.into());
        }
        Self::validate_plugin_kind(
            &instance,
            if instrument {
                PluginKind::Instrument
            } else {
                PluginKind::Effect
            },
        )?;
        crate::builtin::complete_parameters(&mut instance)?;
        if instance.id == 0 || self.plugin(instance.id).is_some() {
            return Err(PluginError::InvalidInstanceId.into());
        }
        let next = instance
            .id
            .checked_add(1)
            .ok_or(PluginError::InvalidInstanceId)?;
        self.next_plugin_id = self.next_plugin_id.max(next);
        if instrument {
            match &mut self.tracks[track.index()] {
                Track::Midi(midi) => midi.set_plugin_instrument(Some(instance)),
                _ => unreachable!("instrument target checked above"),
            }
        } else {
            self.tracks[track.index()].effects_mut().push(instance);
        }
        Ok(())
    }

    pub(crate) fn restore_mixer_plugin(
        &mut self,
        track: TrackId,
        instance: PluginInstance,
    ) -> Result<(), ProjectError> {
        self.restore_plugin(track, instance, false)?;
        let restored = self.tracks[track.index()].effects_mut().pop().unwrap();
        self.tracks[track.index()]
            .mixer_effects_mut()
            .push(restored);
        Ok(())
    }

    pub fn set_bus_track_mixer(
        &mut self,
        id: BusTrackId,
        mixer: MixerSettings,
    ) -> Result<(), ProjectError> {
        self.set_track_mixer(id.into(), mixer)
    }

    pub fn set_group_track_mixer(
        &mut self,
        id: GroupTrackId,
        mixer: MixerSettings,
    ) -> Result<(), ProjectError> {
        self.set_track_mixer(id.into(), mixer)
    }

    pub fn set_master_track_mixer(&mut self, mixer: MixerSettings) -> Result<(), ProjectError> {
        self.set_track_mixer(self.master_track_id().into(), mixer)
    }

    pub fn set_track_send(
        &mut self,
        track: impl Into<TrackId>,
        destination: BusTrackId,
        gain: f32,
    ) -> Result<(), ProjectError> {
        let id = track.into();
        self.track(id)?;
        self.bus_track(destination)?;
        if matches!(id, TrackId::Master(_)) {
            return Err(ProjectError::MasterSend);
        }
        if !gain.is_finite() || gain < 0.0 {
            return Err(ProjectError::InvalidSendGain);
        }
        let sends = self.tracks[id.index()].sends_mut();
        if let Some(send) = sends
            .iter_mut()
            .find(|send| send.destination == destination)
        {
            send.gain = gain;
            return Ok(());
        }
        sends.push(TrackSend { destination, gain });
        if let Err(error) = self.processing_order() {
            self.tracks[id.index()].sends_mut().pop();
            return Err(error);
        }
        Ok(())
    }

    pub fn remove_track_send(
        &mut self,
        track: impl Into<TrackId>,
        destination: BusTrackId,
    ) -> Result<(), ProjectError> {
        let id = track.into();
        self.track(id)?;
        self.bus_track(destination)?;
        self.tracks[id.index()]
            .sends_mut()
            .retain(|send| send.destination != destination);
        Ok(())
    }

    pub fn set_track_group(
        &mut self,
        track: impl Into<TrackId>,
        group: Option<GroupTrackId>,
    ) -> Result<(), ProjectError> {
        let id = track.into();
        let output = group.map_or(AudioOutput::Master, AudioOutput::Group);
        let mixer = self.track(id)?.mixer().with_output(output);
        self.set_track_mixer(id, mixer)
    }

    pub fn group_members(
        &self,
        group: GroupTrackId,
    ) -> Result<impl Iterator<Item = TrackId> + '_, ProjectError> {
        self.group_track(group)?;
        Ok(self
            .tracks
            .iter()
            .enumerate()
            .filter(move |(_, track)| track.mixer().output() == AudioOutput::Group(group))
            .map(|(index, track)| track_id(index, track)))
    }

    pub fn track_ids(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.tracks
            .iter()
            .enumerate()
            .map(|(index, track)| track_id(index, track))
    }

    pub fn reorder_tracks(&mut self, order: &[usize]) -> Result<Vec<usize>, ProjectError> {
        if order.len() != self.tracks.len() || order.first() != Some(&0) {
            return Err(ProjectError::InvalidTrackOrder);
        }
        let mut old_to_new = vec![usize::MAX; self.tracks.len()];
        for (new, &old) in order.iter().enumerate() {
            if old >= old_to_new.len() || old_to_new[old] != usize::MAX {
                return Err(ProjectError::InvalidTrackOrder);
            }
            old_to_new[old] = new;
        }

        let mut previous: Vec<_> = std::mem::take(&mut self.tracks)
            .into_iter()
            .map(Some)
            .collect();
        self.tracks = order
            .iter()
            .map(|&old| previous[old].take().expect("validated track permutation"))
            .collect();
        for track in &mut self.tracks {
            let output = match track.mixer().output() {
                AudioOutput::Group(group) => AudioOutput::Group(GroupTrackId(old_to_new[group.0])),
                AudioOutput::Bus(bus) => AudioOutput::Bus(BusTrackId(old_to_new[bus.0])),
                output => output,
            };
            track.set_mixer(track.mixer().with_output(output));
            for send in track.sends_mut() {
                send.destination = BusTrackId(old_to_new[send.destination.0]);
            }
            for effect in track.all_effects_mut() {
                if let Some(route) = &mut effect.sidechain {
                    route.source_track = old_to_new[route.source_track];
                }
            }
        }
        Ok(old_to_new)
    }

    pub fn rename_track(
        &mut self,
        track: impl Into<TrackId>,
        name: impl Into<String>,
    ) -> Result<(), ProjectError> {
        let id = track.into();
        self.track(id)?;
        match &mut self.tracks[id.index()] {
            Track::Audio(track) => track.set_name(name.into()),
            Track::Midi(track) => track.set_name(name.into()),
            Track::Group(track) => track.set_name(name.into()),
            Track::Bus(track) => track.set_name(name.into()),
            Track::Master(_) => return Err(ProjectError::MasterRename),
        }
        Ok(())
    }

    pub fn set_track_color(
        &mut self,
        track: impl Into<TrackId>,
        color: Option<u32>,
    ) -> Result<(), ProjectError> {
        let id = track.into();
        self.track(id)?;
        if let Some(color) = color
            && color > 0xff_ffff
        {
            return Err(ProjectError::InvalidTrackColor { color });
        }
        match &mut self.tracks[id.index()] {
            Track::Audio(track) => track.set_color(color),
            Track::Midi(track) => track.set_color(color),
            Track::Group(track) => track.set_color(color),
            Track::Bus(track) => track.set_color(color),
            Track::Master(_) => return Err(ProjectError::MasterColor),
        }
        Ok(())
    }

    pub fn remove_track(&mut self, track: impl Into<TrackId>) -> Result<Track, ProjectError> {
        let id = track.into();
        self.track(id)?;
        match id {
            TrackId::Master(_) => return Err(ProjectError::MasterRemoval),
            TrackId::Group(group) => {
                if self.group_members(group)?.next().is_some() {
                    return Err(ProjectError::GroupNotEmpty { track: group });
                }
            }
            TrackId::Bus(bus) => {
                if self
                    .tracks
                    .iter()
                    .any(|track| track.mixer().output() == AudioOutput::Bus(bus))
                {
                    return Err(ProjectError::BusNotEmpty { track: bus });
                }
            }
            TrackId::Audio(_) | TrackId::Midi(_) => {}
        }
        let index = id.index();
        let removed = self.tracks.remove(index);
        for track in &mut self.tracks {
            let output = match track.mixer().output() {
                AudioOutput::Group(group) if group.0 > index => {
                    AudioOutput::Group(GroupTrackId(group.0 - 1))
                }
                AudioOutput::Bus(bus) if bus.0 > index => AudioOutput::Bus(BusTrackId(bus.0 - 1)),
                output => output,
            };
            track.set_mixer(track.mixer().with_output(output));
            track.sends_mut().retain(|send| send.destination.0 != index);
            for send in track.sends_mut() {
                if send.destination.0 > index {
                    send.destination.0 -= 1;
                }
            }
            for effect in track.all_effects_mut() {
                if let Some(route) = &mut effect.sidechain {
                    if route.source_track == index {
                        effect.sidechain = None;
                    } else if route.source_track > index {
                        route.source_track -= 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    fn track(&self, id: TrackId) -> Result<&Track, ProjectError> {
        match (id, self.tracks.get(id.index())) {
            (TrackId::Master(_), _) => Ok(&self.tracks[0]),
            (TrackId::Audio(_), Some(track @ Track::Audio(_)))
            | (TrackId::Midi(_), Some(track @ Track::Midi(_)))
            | (TrackId::Group(_), Some(track @ Track::Group(_)))
            | (TrackId::Bus(_), Some(track @ Track::Bus(_))) => Ok(track),
            (TrackId::Audio(track), _) => Err(ProjectError::UnknownAudioTrack { track }),
            (TrackId::Midi(track), _) => Err(ProjectError::UnknownMidiTrack { track }),
            (TrackId::Group(track), _) => Err(ProjectError::UnknownGroupTrack { track }),
            (TrackId::Bus(track), _) => Err(ProjectError::UnknownBusTrack { track }),
        }
    }

    fn set_track_mixer(&mut self, id: TrackId, mixer: MixerSettings) -> Result<(), ProjectError> {
        self.track(id)?;
        match (id, mixer.output()) {
            (
                TrackId::Master(_),
                AudioOutput::Master | AudioOutput::Group(_) | AudioOutput::Bus(_),
            ) => {
                return Err(ProjectError::InvalidMasterOutput {
                    output: mixer.output(),
                });
            }
            (
                TrackId::Audio(_) | TrackId::Midi(_) | TrackId::Group(_) | TrackId::Bus(_),
                AudioOutput::Output,
            ) => {
                return Err(ProjectError::MasterBypass { track: id });
            }
            (TrackId::Bus(_), AudioOutput::Group(_)) => {
                return Err(ProjectError::InvalidBusOutput {
                    output: mixer.output(),
                });
            }
            _ => {}
        }
        if let AudioOutput::Bus(destination) = mixer.output() {
            self.bus_track(destination)?;
        }
        if let AudioOutput::Group(destination) = mixer.output() {
            self.group_track(destination)?;
            if let TrackId::Group(track) = id {
                let mut output = mixer.output();
                while let AudioOutput::Group(ancestor) = output {
                    if ancestor == track {
                        return Err(ProjectError::RoutingCycle { track, destination });
                    }
                    output = self.tracks[ancestor.0].mixer().output();
                }
            }
        }
        let previous = *self.tracks[id.index()].mixer();
        self.tracks[id.index()].set_mixer(mixer);
        if previous.output() != mixer.output()
            && let Err(error) = self.processing_order()
        {
            self.tracks[id.index()].set_mixer(previous);
            return Err(error);
        }
        Ok(())
    }

    pub fn processing_order(&self) -> Result<Vec<usize>, ProjectError> {
        let mut outputs = vec![Vec::new(); self.tracks.len()];
        for (index, track) in self.tracks.iter().enumerate() {
            match track.mixer().output() {
                AudioOutput::Master => outputs[index].push(0),
                AudioOutput::Group(group) => {
                    self.group_track(group)?;
                    outputs[index].push(group.0);
                }
                AudioOutput::Bus(bus) => {
                    self.bus_track(bus)?;
                    outputs[index].push(bus.0);
                }
                AudioOutput::Output | AudioOutput::Disconnected => {}
            }
            if matches!(track, Track::Master(_)) && !track.sends().is_empty() {
                return Err(ProjectError::MasterSend);
            }
            for send in track.sends() {
                self.bus_track(send.destination)?;
                if !send.gain.is_finite() || send.gain < 0.0 {
                    return Err(ProjectError::InvalidSendGain);
                }
                outputs[index].push(send.destination.0);
            }
            for effect in track.all_effects() {
                if let Some(route) = effect.sidechain {
                    route.validate()?;
                    let source = outputs.get_mut(route.source_track).ok_or(
                        ProjectError::UnknownSidechainSource {
                            track: route.source_track,
                        },
                    )?;
                    source.push(index);
                }
            }
        }
        let mut incoming = vec![0_usize; self.tracks.len()];
        for destinations in &mut outputs {
            destinations.sort_unstable();
            destinations.dedup();
            for &destination in destinations.iter() {
                incoming[destination] += 1;
            }
        }
        let mut ready: BTreeSet<_> = incoming
            .iter()
            .enumerate()
            .filter_map(|(index, count)| (*count == 0).then_some(index))
            .collect();
        let mut ordered = Vec::with_capacity(self.tracks.len());
        while let Some(index) = ready.pop_first() {
            ordered.push(index);
            for &destination in &outputs[index] {
                incoming[destination] -= 1;
                if incoming[destination] == 0 {
                    ready.insert(destination);
                }
            }
        }
        if ordered.len() != self.tracks.len() {
            return Err(ProjectError::SidechainRoutingCycle);
        }
        Ok(ordered)
    }

    pub(crate) fn assign_uids(&mut self) {
        let mut next = self.next_uid.max(1);
        for track in &self.tracks {
            for &uid in std::iter::once(&track.uid()).chain(track.clip_uids()) {
                next = next.max(uid.saturating_add(1));
            }
        }
        let mut seen = HashSet::new();
        let mut settle = |uid: &mut u64| {
            if *uid == 0 || !seen.insert(*uid) {
                *uid = next;
                seen.insert(next);
                next += 1;
            }
        };
        for track in &mut self.tracks {
            let mut uid = track.uid();
            settle(&mut uid);
            track.set_uid(uid);
            for uid in track.clip_uids_mut() {
                settle(uid);
            }
        }
        self.next_uid = next;
    }

    fn fresh_uid(&mut self) -> u64 {
        let uid = self.next_uid;
        self.next_uid += 1;
        uid
    }

    pub fn next_uid(&self) -> u64 {
        self.next_uid
    }

    pub(crate) fn reset_next_uid(&mut self, next: u64) {
        self.next_uid = next.max(1);
    }

    pub(crate) fn restore_uids(&mut self, track: usize, uid: u64, clips: &[u64]) -> bool {
        let Some(entry) = self.tracks.get_mut(track) else {
            return false;
        };
        if clips.len() != entry.clip_uids().len() {
            return false;
        }
        entry.set_uid(uid);
        entry.clip_uids_mut().copy_from_slice(clips);
        true
    }

    pub fn track_index_by_uid(&self, uid: u64) -> Option<usize> {
        self.tracks.iter().position(|track| track.uid() == uid)
    }

    pub fn clip_by_uid(&self, uid: u64) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(track, entry)| {
            entry
                .clip_uids()
                .iter()
                .position(|&clip| clip == uid)
                .map(|clip| (track, clip))
        })
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub(crate) fn tracks_mut(&mut self) -> &mut Vec<Track> {
        &mut self.tracks
    }

    pub fn audio_duration_frames(&self) -> u64 {
        self.audio_clips()
            .map(|clip| self.audio_clip_start_frame(clip) + clip.frame_count)
            .max()
            .unwrap_or(0)
    }

    pub fn duration_frames(&self) -> Result<u64, TimeError> {
        let mut end = self.audio_duration_frames();
        for track in &self.tracks {
            let Track::Midi(track) = track else {
                continue;
            };
            let release_frames = match (track.instrument(), track.plugin_instrument()) {
                (Some(instrument), _) => {
                    let automated = track
                        .automation()
                        .iter()
                        .filter(|lane| {
                            lane.enabled()
                                && lane.target()
                                    == AutomationTarget::Subtractive(SubtractiveParameter::Release)
                        })
                        .flat_map(AutomationLane::points)
                        .map(|point| {
                            (f64::from((point.value * 10.0) as f32) * f64::from(crate::SAMPLE_RATE))
                                .ceil() as u64
                        })
                        .max()
                        .unwrap_or(0);
                    instrument.release_frames().max(automated)
                }
                (_, Some(plugin)) => crate::builtin::release_frames(plugin, track.automation())
                    .unwrap_or(PLUGIN_TAIL_FRAMES),
                (None, None) => continue,
            };
            for clip in track.clips() {
                end = end.max(
                    self.tempo
                        .ticks_to_frames(clip.start_tick + clip.length_ticks)?,
                );
                for note in clip.audible_notes() {
                    let note_start = self
                        .tempo
                        .ticks_to_frames(clip.start_tick + note.start_tick)?;
                    let note_end = self
                        .tempo
                        .ticks_to_frames(clip.start_tick + note.start_tick + note.duration_ticks)?;
                    if note_start == note_end {
                        continue;
                    }
                    let release_end = note_end
                        .checked_add(release_frames)
                        .ok_or(TimeError::ConversionOverflow)?;
                    end = end.max(release_end);
                    if let Some(plugin) = track.plugin_instrument()
                        && crate::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)
                            == Some(crate::builtin::BuiltinKind::Sampler)
                    {
                        let tail =
                            crate::builtin::instruments::sampler_note_tail_frames_with_automation(
                                plugin,
                                note.key,
                                track.automation(),
                            )
                            .unwrap_or(0);
                        end = end.max(
                            note_start
                                .checked_add(tail)
                                .ok_or(TimeError::ConversionOverflow)?,
                        );
                    }
                }
            }
        }
        if end > 0 {
            let mut longest_chain = 0_u64;
            if self.tracks.iter().all(|track| {
                track.sends().is_empty()
                    && track.all_effects().all(|effect| effect.sidechain.is_none())
            }) {
                for index in 0..self.tracks.len() {
                    let mut current = index;
                    let mut count = 0_u64;
                    loop {
                        let track = &self.tracks[current];
                        count = count
                            .checked_add(track.all_effects().count() as u64)
                            .ok_or(TimeError::ConversionOverflow)?;
                        current = match track.mixer().output() {
                            AudioOutput::Master => 0,
                            AudioOutput::Group(group) => group.0,
                            AudioOutput::Bus(bus) => bus.0,
                            AudioOutput::Output | AudioOutput::Disconnected => break,
                        };
                    }
                    longest_chain = longest_chain.max(count);
                }
            } else {
                let mut pre_effects = vec![0_u64; self.tracks.len()];
                let mut post_effects = vec![0_u64; self.tracks.len()];
                let order = self
                    .processing_order()
                    .expect("project mutations preserve valid acyclic routing");
                for index in order {
                    let track = &self.tracks[index];
                    let mut input_tail = pre_effects[index];
                    for effect in track.all_effects() {
                        if let Some(route) = effect.sidechain {
                            let source_tail = match route.tap {
                                SidechainTap::PreEffects => pre_effects[route.source_track],
                                SidechainTap::PostEffects | SidechainTap::PostFader => {
                                    post_effects[route.source_track]
                                }
                            };
                            input_tail = input_tail.max(source_tail);
                        }
                    }
                    let count = input_tail
                        .checked_add(track.all_effects().count() as u64)
                        .ok_or(TimeError::ConversionOverflow)?;
                    post_effects[index] = count;
                    longest_chain = longest_chain.max(count);
                    let destination = match track.mixer().output() {
                        AudioOutput::Master => Some(0),
                        AudioOutput::Group(group) => Some(group.0),
                        AudioOutput::Bus(bus) => Some(bus.0),
                        AudioOutput::Output | AudioOutput::Disconnected => None,
                    };
                    if let Some(destination) = destination {
                        pre_effects[destination] = pre_effects[destination].max(count);
                    }
                    for send in track.sends() {
                        let destination = send.destination.0;
                        pre_effects[destination] = pre_effects[destination].max(count);
                    }
                }
            }
            end = end
                .checked_add(
                    longest_chain
                        .checked_mul(PLUGIN_TAIL_FRAMES)
                        .ok_or(TimeError::ConversionOverflow)?,
                )
                .ok_or(TimeError::ConversionOverflow)?;
        }
        Ok(end)
    }

    fn audio_clips(&self) -> impl Iterator<Item = &AudioClip> {
        self.tracks
            .iter()
            .filter_map(|track| match track {
                Track::Audio(track) => Some(track.clips()),
                _ => None,
            })
            .flatten()
    }

    pub(crate) fn audio_clip_start_frame(&self, clip: &AudioClip) -> u64 {
        self.tempo
            .ticks_to_frames(clip.start_tick)
            .expect("audio positions were validated on insertion, edit, and tempo change")
    }

    pub(crate) fn asset(&self, id: AssetId) -> &AudioBuffer {
        &self.assets[id.0]
    }

    pub(crate) fn assets(&self) -> &[Arc<AudioBuffer>] {
        &self.assets
    }
}

fn track_id(index: usize, track: &Track) -> TrackId {
    match track {
        Track::Audio(_) => TrackId::Audio(AudioTrackId(index)),
        Track::Midi(_) => TrackId::Midi(MidiTrackId(index)),
        Track::Group(_) => TrackId::Group(GroupTrackId(index)),
        Track::Bus(_) => TrackId::Bus(BusTrackId(index)),
        Track::Master(_) => TrackId::Master(MasterTrackId(index)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    Time(TimeError),
    Instrument(InstrumentError),
    Plugin(PluginError),
    UnknownEffect {
        track: TrackId,
        index: usize,
    },
    UnknownAudioClip {
        track: AudioTrackId,
        index: usize,
    },
    UnknownMidiClip {
        track: MidiTrackId,
        index: usize,
    },
    NonFiniteSample {
        frame: usize,
        channel: usize,
    },
    UnknownAsset {
        asset: AssetId,
    },
    UnknownAudioTrack {
        track: AudioTrackId,
    },
    UnknownMidiTrack {
        track: MidiTrackId,
    },
    UnknownGroupTrack {
        track: GroupTrackId,
    },
    UnknownBusTrack {
        track: BusTrackId,
    },
    InvalidBusOutput {
        output: AudioOutput,
    },
    BusNotEmpty {
        track: BusTrackId,
    },
    InvalidMasterOutput {
        output: AudioOutput,
    },
    MasterBypass {
        track: TrackId,
    },
    RoutingCycle {
        track: GroupTrackId,
        destination: GroupTrackId,
    },
    UnknownSidechainSource {
        track: usize,
    },
    SidechainRoutingCycle,
    MasterRename,
    MasterRemoval,
    InvalidTrackOrder,
    GroupNotEmpty {
        track: GroupTrackId,
    },
    MasterSend,
    InvalidSendGain,
    InvalidTrackGain,
    InvalidTrackPan,
    InvalidRecordingDevice,
    InvalidAudioInputChannels,
    InvalidMidiInputChannel,
    InvalidAutomationValue,
    InvalidAutomationOrder,
    InvalidAutomationTarget,
    TooManyAutomationPoints,
    TooManyAutomationLanes,
    EmptyClip,
    NonFiniteGain,
    InvalidClipPitch,
    SourceRangeOverflow,
    SourceRangeOutOfBounds {
        start: u64,
        frame_count: u64,
        asset_frames: u64,
    },
    TimelineRangeOverflow,
    EmptyMidiClip,
    MidiTimelineRangeOverflow,
    InvalidMidiKey {
        note: usize,
        key: u8,
    },
    InvalidMidiVelocity {
        note: usize,
        velocity: u8,
    },
    InvalidMidiChannel {
        note: usize,
        channel: u8,
    },
    EmptyMidiNote {
        note: usize,
    },
    MidiNoteRangeOverflow {
        note: usize,
    },
    MasterColor,
    InvalidTrackColor {
        color: u32,
    },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Time(error) => error.fmt(f),
            Self::Instrument(error) => error.fmt(f),
            Self::Plugin(error) => error.fmt(f),
            Self::UnknownEffect { track, index } => {
                write!(f, "unknown effect {index} on track {}", track.index())
            }
            Self::UnknownAudioClip { track, index } => {
                write!(f, "unknown audio clip {index} on track {}", track.0)
            }
            Self::UnknownMidiClip { track, index } => {
                write!(f, "unknown MIDI clip {index} on track {}", track.0)
            }
            Self::NonFiniteSample { frame, channel } => {
                write!(
                    f,
                    "non-finite audio sample at frame {frame}, channel {channel}"
                )
            }
            Self::UnknownAsset { asset } => write!(f, "unknown audio asset {}", asset.0),
            Self::UnknownAudioTrack { track } => write!(f, "unknown audio track {}", track.0),
            Self::UnknownMidiTrack { track } => write!(f, "unknown MIDI track {}", track.0),
            Self::UnknownGroupTrack { track } => write!(f, "unknown group track {}", track.0),
            Self::UnknownBusTrack { track } => write!(f, "unknown bus track {}", track.0),
            Self::InvalidBusOutput { output } => write!(
                f,
                "bus track cannot route to {output:?}; expected Master, Bus, or Disconnected"
            ),
            Self::BusNotEmpty { track } => write!(
                f,
                "bus track {} still has inputs; route them elsewhere before removing it",
                track.0
            ),
            Self::InvalidMasterOutput { output } => write!(
                f,
                "master track cannot route to {output:?}; expected Output or Disconnected"
            ),
            Self::MasterBypass { track } => write!(
                f,
                "track {} cannot bypass the master; only the master can route to Output",
                track.index()
            ),
            Self::RoutingCycle { track, destination } => write!(
                f,
                "routing group track {} to group {} would create a cycle",
                track.0, destination.0
            ),
            Self::UnknownSidechainSource { track } => {
                write!(f, "unknown sidechain source track {track}")
            }
            Self::SidechainRoutingCycle => write!(
                f,
                "audio and sidechain routing would create a feedback cycle"
            ),
            Self::MasterRename => write!(f, "the master track's name cannot be changed"),
            Self::MasterRemoval => write!(f, "the master track cannot be removed"),
            Self::InvalidTrackOrder => write!(
                f,
                "track order must contain every track index once with the master first"
            ),
            Self::GroupNotEmpty { track } => write!(
                f,
                "group track {} still has members; route them elsewhere before removing it",
                track.0
            ),
            Self::MasterSend => write!(f, "the master track cannot send to a bus"),
            Self::InvalidSendGain => write!(f, "send gain must be finite and nonnegative"),
            Self::InvalidTrackGain => write!(f, "track gain must be finite and nonnegative"),
            Self::InvalidTrackPan => write!(f, "track pan must be finite and in -1..=1"),
            Self::InvalidRecordingDevice => write!(
                f,
                "recording device key must be nonempty, at most 4096 bytes, and contain no NUL"
            ),
            Self::InvalidAudioInputChannels => write!(
                f,
                "audio input channels must be below 65535 and stereo channels must differ"
            ),
            Self::InvalidMidiInputChannel => {
                write!(f, "MIDI input channel must be in 0..=15")
            }
            Self::InvalidAutomationValue => {
                write!(f, "automation values must be finite and in 0..=1")
            }
            Self::InvalidAutomationOrder => {
                write!(f, "automation points must have strictly increasing ticks")
            }
            Self::InvalidAutomationTarget => write!(
                f,
                "automation target must be an editable parameter on this track"
            ),
            Self::TooManyAutomationPoints => write!(f, "automation lane exceeds the point limit"),
            Self::TooManyAutomationLanes => write!(f, "track exceeds the automation lane limit"),
            Self::EmptyClip => write!(f, "clip must contain at least one frame"),
            Self::NonFiniteGain => write!(f, "clip gain must be finite"),
            Self::InvalidClipPitch => {
                write!(f, "clip pitch must be finite and in -24..=24 semitones")
            }
            Self::SourceRangeOverflow => write!(f, "clip source range overflows u64"),
            Self::SourceRangeOutOfBounds {
                start,
                frame_count,
                asset_frames,
            } => write!(
                f,
                "clip source range ({start} + {frame_count} frames) exceeds asset length {asset_frames}"
            ),
            Self::TimelineRangeOverflow => write!(f, "clip timeline range overflows u64"),
            Self::EmptyMidiClip => write!(f, "MIDI clip must contain at least one tick"),
            Self::MidiTimelineRangeOverflow => write!(f, "MIDI clip timeline range overflows u64"),
            Self::InvalidMidiKey { note, key } => {
                write!(f, "MIDI note {note} has key {key}; expected 0..=127")
            }
            Self::InvalidMidiVelocity { note, velocity } => {
                write!(
                    f,
                    "MIDI note {note} has velocity {velocity}; expected 1..=127"
                )
            }
            Self::InvalidMidiChannel { note, channel } => {
                write!(f, "MIDI note {note} has channel {channel}; expected 0..=15")
            }
            Self::EmptyMidiNote { note } => {
                write!(f, "MIDI note {note} must have a positive duration")
            }
            Self::MidiNoteRangeOverflow { note } => {
                write!(f, "MIDI note {note} range overflows u64")
            }
            Self::MasterColor => write!(f, "the master track's colour is fixed"),
            Self::InvalidTrackColor { color } => {
                write!(f, "track colour {color:#x} is not a 24-bit RGB value")
            }
        }
    }
}

impl From<TimeError> for ProjectError {
    fn from(error: TimeError) -> Self {
        Self::Time(error)
    }
}

impl From<InstrumentError> for ProjectError {
    fn from(error: InstrumentError) -> Self {
        Self::Instrument(error)
    }
}

impl From<PluginError> for ProjectError {
    fn from(error: PluginError) -> Self {
        Self::Plugin(error)
    }
}

impl Error for ProjectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Time(error) => Some(error),
            Self::Instrument(error) => Some(error),
            Self::Plugin(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod pitch_preparation_tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn ongoing_preparation_does_not_lock_out_cached_playback() {
        let source = AudioBuffer::new(vec![[0.25, -0.125]; 4096]).unwrap();
        let first = source.pitched_samples(1.0);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (lookup_tx, lookup_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let source_ref = &source;
            scope.spawn(move || {
                let calls = Cell::new(0);
                let result = source_ref.prepare_pitched_samples(2.0.into(), || {
                    calls.set(calls.get() + 1);
                    if calls.get() == 3 {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        return true;
                    }
                    false
                });
                assert!(result.unwrap().is_none());
            });
            entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let source_ref = &source;
            let first_ref = &first;
            scope.spawn(move || {
                assert!(source_ref.pitch_is_prepared(1.0.into()));
                lookup_tx
                    .send(Arc::ptr_eq(first_ref, &source_ref.pitched_samples(1.0)))
                    .unwrap();
            });
            let lookup = lookup_rx.recv_timeout(Duration::from_secs(10));
            release_tx.send(()).unwrap();
            assert!(lookup.unwrap());
        });
        assert!(!source.pitch_is_prepared(2.0.into()));
    }

    #[test]
    fn strong_retention_is_bounded_without_evicting_live_weak_entries() {
        let source = AudioBuffer::new(vec![[0.0; 2]; 16]).unwrap();
        let first = source.pitched_samples(1.0);
        let first_weak = Arc::downgrade(&first);
        source.pitched_samples(2.0);
        source.pitched_samples(3.0);
        assert!(source.pitch_is_prepared(1.0.into()));
        {
            let cache = source.pitch_cache.lock().unwrap();
            assert_eq!(cache.len(), 3);
            assert_eq!(
                cache
                    .iter()
                    .filter(|entry| entry.retained.is_some())
                    .count(),
                2
            );
        }
        drop(first);
        assert!(first_weak.upgrade().is_none());
        assert!(!source.pitch_is_prepared(1.0.into()));
    }
}
