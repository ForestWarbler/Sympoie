use crate::automation::{AutomationLane, AutomationTarget};
use crate::clip::AudioClip;
use crate::plugin::SidechainTap;
use crate::project::{AudioBuffer, Project};
use crate::track::{AudioOutput, AudioTrack, BusTrackId, GroupTrackId, MixerSettings, Track};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BounceMode {
    InPlace,
    Duplicate,
}

#[derive(Debug, Clone)]
pub struct BouncePlan {
    pub project: Project,
    pub track_index: usize,
    pub start_tick: u64,
    pub start_frame: u64,
    pub end_frame: u64,
    source_track_count: usize,
    source_track: Track,
    members: Vec<bool>,
    in_place_error: Option<String>,
}

impl BouncePlan {
    pub fn new(source: &Project, track_index: usize) -> Result<Self, String> {
        let track = source
            .tracks()
            .get(track_index)
            .ok_or_else(|| "The track no longer exists.".to_owned())?;
        if matches!(track, Track::Master(_) | Track::Bus(_)) {
            return Err("Bounce is available for audio, MIDI, and group tracks.".into());
        }

        let children = children(source);
        let members = dependencies(source, &children, track_index, false);
        if members.iter().enumerate().any(|(index, member)| {
            *member
                && index != track_index
                && (!source.tracks()[index].mixer_effects().is_empty()
                    || !source.tracks()[index].sends().is_empty()
                    || matches!(source.tracks()[index], Track::Bus(_)))
        }) {
            return Err("This group's members have mixer effects, sends, or buses. Bounce its members individually to keep their mixing editable.".into());
        }
        let mut project = source.clone();
        project.tracks_mut()[track_index]
            .mixer_effects_mut()
            .clear();
        project.tracks_mut()[track_index].sends_mut().clear();
        project.tracks_mut()[track_index]
            .automation_mut()
            .retain(|lane| !is_mixer_effect_target(track, lane.target()));
        let needed = dependencies(&project, &children, track_index, true);
        if needed[0] {
            return Err("This track uses the master bus as a sidechain source and cannot be isolated for bounce.".into());
        }
        let in_place_error = in_place_conflict(source, &members, track_index).map(str::to_owned);
        let start_tick = source
            .tracks()
            .iter()
            .enumerate()
            .filter(|(index, _)| members[*index])
            .filter_map(|(_, track)| match track {
                Track::Audio(audio) => audio
                    .clips()
                    .iter()
                    .filter(|clip| clip.frame_count != 0)
                    .map(|clip| clip.start_tick)
                    .min(),
                Track::Midi(midi)
                    if midi.instrument().is_some() || midi.plugin_instrument().is_some() =>
                {
                    midi.clips()
                        .iter()
                        .filter(|clip| clip.length_ticks != 0)
                        .map(|clip| clip.start_tick)
                        .min()
                }
                _ => None,
            })
            .min()
            .ok_or_else(|| "The track has no audio or instrument clips to bounce.".to_owned())?;
        let start_frame = source
            .tempo()
            .ticks_to_frames(start_tick)
            .map_err(|error| error.to_string())?;

        for (index, track) in project.tracks_mut().iter_mut().enumerate() {
            track.set_mixer(track.mixer().with_soloed(false));
            if index == 0 {
                track.effects_mut().clear();
                track.mixer_effects_mut().clear();
                track.sends_mut().clear();
                track.automation_mut().clear();
                track.set_mixer(MixerSettings::default().with_output(AudioOutput::Output));
            } else if !needed[index] {
                clear_content(track);
                track.effects_mut().clear();
                track.mixer_effects_mut().clear();
                track.sends_mut().clear();
                track.automation_mut().clear();
                track.set_mixer(track.mixer().with_output(AudioOutput::Disconnected));
            } else if index == track_index {
                track.set_mixer(MixerSettings::default());
                track.automation_mut().retain(|lane| {
                    !matches!(
                        lane.target(),
                        AutomationTarget::TrackGain | AutomationTarget::TrackPan
                    )
                });
            } else {
                track.sends_mut().retain(|send| needed[send.destination.0]);
                let destination = match track.mixer().output() {
                    AudioOutput::Group(group) if needed[group.0] => AudioOutput::Group(group),
                    AudioOutput::Bus(bus) if needed[bus.0] => AudioOutput::Bus(bus),
                    _ => AudioOutput::Disconnected,
                };
                track.set_mixer(track.mixer().with_output(destination));
            }
        }
        project
            .processing_order()
            .map_err(|error| error.to_string())?;

        let mut extent = project.clone();
        for (index, track) in extent.tracks_mut().iter_mut().enumerate() {
            if !members[index] {
                clear_clips(track);
            }
        }
        let end_frame = extent
            .duration_frames()
            .map_err(|error| error.to_string())?;
        if end_frame <= start_frame {
            return Err("The track has no representable audio frames to bounce.".into());
        }
        Ok(Self {
            project,
            track_index,
            start_tick,
            start_frame,
            end_frame,
            source_track_count: source.tracks().len(),
            source_track: track.clone(),
            members,
            in_place_error,
        })
    }

    pub fn frame_count(&self) -> u64 {
        self.end_frame - self.start_frame
    }

    pub fn validate_mode(&self, mode: BounceMode) -> Result<(), String> {
        if mode == BounceMode::InPlace
            && let Some(error) = &self.in_place_error
        {
            return Err(error.clone());
        }
        Ok(())
    }
}

pub fn apply_bounce(
    project: &mut Project,
    plan: &BouncePlan,
    audio: AudioBuffer,
    mode: BounceMode,
) -> Result<usize, String> {
    plan.validate_mode(mode)?;
    let current = project
        .tracks()
        .get(plan.track_index)
        .ok_or_else(|| "The source track changed while bouncing.".to_owned())?;
    if project.tracks().len() != plan.source_track_count
        || std::mem::discriminant(current) != std::mem::discriminant(&plan.source_track)
        || current.name() != plan.source_track.name()
        || current.mixer() != plan.source_track.mixer()
        || current.color() != plan.source_track.color()
        || current.automation() != plan.source_track.automation()
        || current.mixer_effects() != plan.source_track.mixer_effects()
        || current.sends() != plan.source_track.sends()
        || project.tempo() != plan.project.tempo()
    {
        return Err("The source project changed while bouncing.".into());
    }
    if audio.len() as u64 != plan.frame_count() {
        return Err("The bounced audio length does not match the render plan.".into());
    }
    let removed: Vec<_> = plan
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| mode == BounceMode::InPlace && *member && index != plan.track_index)
        .collect();
    if mode == BounceMode::InPlace
        && let Some(error) = in_place_conflict(project, &plan.members, plan.track_index)
    {
        return Err(error.into());
    }

    let source = &plan.source_track;
    let mut bounced = AudioTrack::new(match mode {
        BounceMode::InPlace => source.name().to_owned(),
        BounceMode::Duplicate => format!("{} (Bounce)", source.name()),
    });
    bounced.set_color(source.color());
    if let Track::Audio(track) = current {
        bounced.set_recording_input(track.recording_input().clone());
    }
    bounced.set_mixer(match mode {
        BounceMode::InPlace => *source.mixer(),
        BounceMode::Duplicate => source.mixer().with_muted(false),
    });

    let mut bounced = Track::Audio(bounced);
    if mode == BounceMode::InPlace {
        bounced.set_uid(source.uid());
    }
    bounced
        .mixer_effects_mut()
        .extend_from_slice(source.mixer_effects());
    bounced.sends_mut().extend_from_slice(source.sends());
    let mut next_plugin_id = project.next_plugin_id();
    let mut copied_ids = Vec::new();
    if mode == BounceMode::Duplicate {
        for effect in bounced.mixer_effects_mut() {
            let old_id = effect.id;
            effect.id = next_plugin_id;
            next_plugin_id = next_plugin_id
                .checked_add(1)
                .ok_or_else(|| "There are no remaining plugin instance IDs.".to_owned())?;
            copied_ids.push((old_id, effect.id));
        }
    }
    for lane in source.automation().iter().filter(|lane| {
        matches!(
            lane.target(),
            AutomationTarget::TrackGain | AutomationTarget::TrackPan
        ) || is_mixer_effect_target(source, lane.target())
    }) {
        let target = match lane.target() {
            AutomationTarget::PluginParameter { plugin, parameter } => {
                AutomationTarget::PluginParameter {
                    plugin: copied_ids
                        .iter()
                        .find_map(|(old, new)| (*old == plugin).then_some(*new))
                        .unwrap_or(plugin),
                    parameter,
                }
            }
            target => target,
        };
        bounced.automation_mut().push(
            AutomationLane::new(target, lane.points().to_vec())
                .map_err(|error| error.to_string())?
                .with_enabled(lane.enabled())
                .with_anchor(lane.anchor()),
        );
    }

    project
        .restore_next_plugin_id(next_plugin_id)
        .map_err(|error| error.to_string())?;
    let asset = project.add_asset(audio);
    let Track::Audio(audio) = &mut bounced else {
        unreachable!("bounce replacements are audio tracks");
    };
    audio.push_clip(AudioClip {
        asset,
        source_start: 0,
        start_tick: plan.start_tick,
        frame_count: plan.frame_count(),
        gain: 1.0,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: None,
    });
    let mut mapping = vec![None; project.tracks().len()];
    let mut tracks =
        Vec::with_capacity(project.tracks().len() + usize::from(mode == BounceMode::Duplicate));
    let mut bounced = Some(bounced);
    let mut result = 0;
    for (index, mut track) in std::mem::take(project.tracks_mut()).into_iter().enumerate() {
        if removed[index] {
            continue;
        }
        mapping[index] = Some(tracks.len());
        if index == plan.track_index {
            if mode == BounceMode::Duplicate {
                track.set_mixer(track.mixer().with_muted(true));
                tracks.push(track);
            }
            result = tracks.len();
            tracks.push(bounced.take().expect("one target track"));
        } else {
            tracks.push(track);
        }
    }
    for track in &mut tracks {
        if let AudioOutput::Group(group) = track.mixer().output() {
            let remapped = mapping[group.0].expect("surviving group destination");
            track.set_mixer(
                track
                    .mixer()
                    .with_output(AudioOutput::Group(GroupTrackId(remapped))),
            );
        } else if let AudioOutput::Bus(bus) = track.mixer().output() {
            let remapped = mapping[bus.0].expect("surviving bus destination");
            track.set_mixer(
                track
                    .mixer()
                    .with_output(AudioOutput::Bus(BusTrackId(remapped))),
            );
        }
        for send in track.sends_mut() {
            send.destination = BusTrackId(mapping[send.destination.0].expect("surviving send bus"));
        }
        for effect in track.all_effects_mut() {
            if let Some(route) = &mut effect.sidechain {
                route.source_track = if mode == BounceMode::Duplicate
                    && route.source_track == plan.track_index
                    && route.tap == SidechainTap::PostFader
                {
                    result
                } else {
                    mapping[route.source_track].expect("surviving sidechain source")
                };
            }
        }
    }
    *project.tracks_mut() = tracks;
    project.assign_uids();
    Ok(result)
}

fn in_place_conflict(project: &Project, members: &[bool], target: usize) -> Option<&'static str> {
    if project.tracks()[target]
        .mixer_effects()
        .iter()
        .filter_map(|effect| effect.sidechain)
        .any(|route| route.source_track != target && members[route.source_track])
    {
        return Some(
            "The group's mixer uses a member as a sidechain source. Use Duplicate and Bounce to preserve it.",
        );
    }
    for (index, track) in project.tracks().iter().enumerate() {
        if members[index] {
            continue;
        }
        for route in track.all_effects().filter_map(|effect| effect.sidechain) {
            if route.source_track != target && members[route.source_track] {
                return Some(
                    "A group member is used by another track's sidechain. Use Duplicate and Bounce to preserve it.",
                );
            }
            if route.source_track == target
                && route.tap == SidechainTap::PreEffects
                && !project.tracks()[target].effects().is_empty()
            {
                return Some(
                    "Another track uses this track's pre-effects sidechain signal. Use Duplicate and Bounce to preserve it.",
                );
            }
        }
    }
    None
}

fn children(project: &Project) -> Vec<Vec<usize>> {
    let mut children = vec![Vec::new(); project.tracks().len()];
    for (index, track) in project.tracks().iter().enumerate() {
        let parent = match track.mixer().output() {
            AudioOutput::Master => Some(0),
            AudioOutput::Group(group) => Some(group.0),
            AudioOutput::Bus(bus) => Some(bus.0),
            AudioOutput::Output | AudioOutput::Disconnected => None,
        };
        if let Some(parent) = parent {
            children[parent].push(index);
        }
    }
    children
}

fn dependencies(
    project: &Project,
    children: &[Vec<usize>],
    target: usize,
    sidechains: bool,
) -> Vec<bool> {
    let mut needed = vec![false; project.tracks().len()];
    let mut pending = vec![target];
    while let Some(index) = pending.pop() {
        if std::mem::replace(&mut needed[index], true) {
            continue;
        }
        pending.extend(children[index].iter().copied());
        if sidechains {
            pending.extend(
                project
                    .tracks()
                    .iter()
                    .enumerate()
                    .filter_map(|(source, track)| {
                        track
                            .sends()
                            .iter()
                            .any(|send| send.destination.0 == index)
                            .then_some(source)
                    }),
            );
            pending.extend(
                project.tracks()[index]
                    .all_effects()
                    .filter_map(|effect| effect.sidechain.map(|route| route.source_track)),
            );
        }
    }
    needed
}

/// One MIDI track's instrument isolated for an offline render of its own: like a bounce,
/// but also without the track's insert effects and at unity gain, so the result can stand
/// in for the instrument's realtime output inside the unchanged track.
#[derive(Debug, Clone)]
pub struct InstrumentRenderPlan {
    pub project: Project,
    pub track_index: usize,
    /// The render covers frames 0..end_frame, so it lines up with the timeline.
    pub end_frame: u64,
}

impl InstrumentRenderPlan {
    /// `None` when the track has no notes to render.
    pub fn new(source: &Project, track_index: usize) -> Result<Option<Self>, String> {
        let Some(Track::Midi(track)) = source.tracks().get(track_index) else {
            return Err("The track is not a MIDI track.".into());
        };
        if track
            .clips()
            .iter()
            .all(|clip| clip.audible_notes().next().is_none())
        {
            return Ok(None);
        }
        let mut plan = BouncePlan::new(source, track_index)?;
        let target = &mut plan.project.tracks_mut()[track_index];
        let effects: Vec<u64> = target.effects().iter().map(|effect| effect.id).collect();
        target.effects_mut().clear();
        target.automation_mut().retain(|lane| {
            !matches!(lane.target(), AutomationTarget::PluginParameter { plugin, .. }
                if effects.contains(&plugin))
        });
        plan.project
            .processing_order()
            .map_err(|error| error.to_string())?;
        Ok(Some(Self {
            project: plan.project,
            track_index,
            end_frame: plan.end_frame,
        }))
    }
}

/// A fingerprint of everything an instrument render of `track_index` depends on: the
/// track's clips (notes, slides, bends), its plug-in instrument's parameters, state and
/// bypass, the automation of that instrument, and the tempo. Equal keys mean a finished
/// render is still valid.
pub fn instrument_render_key(project: &Project, track_index: usize) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let Track::Midi(track) = project.tracks().get(track_index)? else {
        return None;
    };
    let plugin = track.plugin_instrument()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project.tempo().bpm().to_bits().hash(&mut hasher);
    format!("{:?}", track.clips()).hash(&mut hasher);
    plugin.id.hash(&mut hasher);
    plugin.bypassed.hash(&mut hasher);
    plugin.state.hash(&mut hasher);
    for parameter in &plugin.parameters {
        parameter.id.hash(&mut hasher);
        parameter.value.to_bits().hash(&mut hasher);
    }
    for lane in track.automation() {
        if matches!(lane.target(), AutomationTarget::PluginParameter { plugin: id, .. } if id == plugin.id)
        {
            format!("{lane:?}").hash(&mut hasher);
        }
    }
    Some(hasher.finish())
}

fn is_mixer_effect_target(track: &Track, target: AutomationTarget) -> bool {
    matches!(target, AutomationTarget::PluginParameter { plugin, .. }
        if track.mixer_effects().iter().any(|effect| effect.id == plugin))
}

fn clear_clips(track: &mut Track) {
    match track {
        Track::Audio(audio) => {
            while !audio.clips().is_empty() {
                audio.remove_clip(audio.clips().len() - 1);
            }
        }
        Track::Midi(midi) => {
            while !midi.clips().is_empty() {
                midi.remove_clip(midi.clips().len() - 1);
            }
        }
        _ => {}
    }
}

fn clear_content(track: &mut Track) {
    clear_clips(track);
    if let Track::Midi(midi) = track {
        midi.set_instrument(None);
        midi.set_plugin_instrument(None);
    }
}
