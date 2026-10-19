//! Exporting a project as stems: every audio and MIDI track is rendered to
//! audio, while groups, buses, the master and each track's mixer stay live.
//!
//! What a stem bakes is the track's signal where the mixer picks it up: its
//! clips or instrument plus its insert effects (and their automation). The
//! fader, pan, mute and solo, the mixer-strip effects, sends, output routing
//! and all of their automation are carried over untouched, so the stems
//! project processes the stems exactly as the original processed the tracks.
//!
//! One case keeps the inserts live: when some effect uses a track's
//! pre-effects signal as its sidechain, that signal must still exist, so the
//! stem is rendered dry (clips or instrument only) and its inserts move along.

use crate::automation::AutomationTarget;
use crate::bounce::BouncePlan;
use crate::clip::AudioClip;
use crate::plugin::SidechainTap;
use crate::project::{AudioBuffer, Project};
use crate::track::{AudioTrack, Track};

#[derive(Debug, Clone)]
pub struct StemsPlan {
    source: Project,
    pub stems: Vec<Stem>,
}

#[derive(Debug, Clone)]
pub struct Stem {
    pub track_index: usize,
    pub name: String,
    /// The insert effects stay on the track instead of being rendered in.
    pub keep_inserts: bool,
    /// `None` when the track has nothing to play.
    pub render: Option<StemRender>,
}

/// Render `project` from `start_frame` to `end_frame` to get the stem's audio.
#[derive(Debug, Clone)]
pub struct StemRender {
    pub project: Project,
    pub start_tick: u64,
    pub start_frame: u64,
    pub end_frame: u64,
}

impl StemRender {
    pub fn frame_count(&self) -> u64 {
        self.end_frame - self.start_frame
    }
}

impl StemsPlan {
    pub fn new(source: &Project) -> Result<Self, String> {
        source
            .processing_order()
            .map_err(|error| error.to_string())?;
        let mut stems = Vec::new();
        for (index, track) in source.tracks().iter().enumerate() {
            if !matches!(track, Track::Audio(_) | Track::Midi(_)) {
                continue;
            }
            let keep_inserts = !track.effects().is_empty() && pre_effects_tapped(source, index);
            let render = if has_content(track) {
                let mut plan = BouncePlan::new(source, index)
                    .map_err(|error| format!("{}: {error}", track.name()))?;
                if keep_inserts {
                    let target = &mut plan.project.tracks_mut()[index];
                    target.effects_mut().clear();
                    target.prune_automation();
                }
                Some(StemRender {
                    project: plan.project,
                    start_tick: plan.start_tick,
                    start_frame: plan.start_frame,
                    end_frame: plan.end_frame,
                })
            } else {
                None
            };
            stems.push(Stem {
                track_index: index,
                name: track.name().to_owned(),
                keep_inserts,
                render,
            });
        }
        Ok(Self {
            source: source.clone(),
            stems,
        })
    }

    pub fn total_frames(&self) -> u64 {
        self.stems
            .iter()
            .filter_map(|stem| stem.render.as_ref())
            .map(StemRender::frame_count)
            .sum()
    }

    /// Builds the stems project from one rendered buffer per stem, in order;
    /// stems without a render take `None`.
    pub fn apply(&self, audio: Vec<Option<AudioBuffer>>) -> Result<Project, String> {
        if audio.len() != self.stems.len() {
            return Err("Every stem needs its rendered audio.".into());
        }
        let mut project = self.source.clone();
        for (stem, audio) in self.stems.iter().zip(audio) {
            let source = &self.source.tracks()[stem.track_index];
            let mut track = AudioTrack::new(source.name().to_owned());
            track.set_color(source.color());
            if let Track::Audio(audio) = source {
                track.set_recording_input(audio.recording_input().clone());
            }
            track.set_mixer(*source.mixer());
            let mut track = Track::Audio(track);
            track.set_uid(source.uid());
            if stem.keep_inserts {
                track.effects_mut().extend_from_slice(source.effects());
            }
            track
                .mixer_effects_mut()
                .extend_from_slice(source.mixer_effects());
            track.sends_mut().extend_from_slice(source.sends());
            for lane in source.automation() {
                let live = match lane.target() {
                    AutomationTarget::TrackGain | AutomationTarget::TrackPan => true,
                    AutomationTarget::PluginParameter { plugin, .. } => {
                        track.all_effects().any(|effect| effect.id == plugin)
                    }
                    _ => false,
                };
                if live {
                    track.automation_mut().push(lane.clone());
                }
            }
            match (&stem.render, audio) {
                (Some(render), Some(audio)) => {
                    if audio.len() as u64 != render.frame_count() {
                        return Err(format!(
                            "The rendered audio for {} does not match its length.",
                            stem.name
                        ));
                    }
                    let frame_count = render.frame_count();
                    let asset = project.add_asset(audio);
                    let Track::Audio(audio_track) = &mut track else {
                        unreachable!("stems are audio tracks");
                    };
                    audio_track.push_clip(AudioClip {
                        asset,
                        source_start: 0,
                        start_tick: render.start_tick,
                        frame_count,
                        gain: 1.0,
                        pitch_semitones: 0.0,
                        pitch_algorithm: Default::default(),
                        preserve_formants: false,
                        name: Some(stem.name.clone()),
                    });
                }
                (None, None) => {}
                _ => return Err(format!("Unexpected audio for {}.", stem.name)),
            }
            project.tracks_mut()[stem.track_index] = track;
        }
        project.retain_used_assets();
        project.assign_uids();
        project
            .processing_order()
            .map_err(|error| error.to_string())?;
        Ok(project)
    }
}

fn has_content(track: &Track) -> bool {
    match track {
        Track::Audio(audio) => audio.clips().iter().any(|clip| clip.frame_count != 0),
        Track::Midi(midi) => {
            (midi.instrument().is_some() || midi.plugin_instrument().is_some())
                && midi.clips().iter().any(|clip| clip.length_ticks != 0)
        }
        _ => false,
    }
}

fn pre_effects_tapped(project: &Project, index: usize) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(Track::all_effects)
        .filter_map(|effect| effect.sidechain)
        .any(|route| route.source_track == index && route.tap == SidechainTap::PreEffects)
}
