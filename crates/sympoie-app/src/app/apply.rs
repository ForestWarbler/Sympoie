use std::collections::BTreeSet;

use eframe::egui::{self, ViewportCommand};
use sympoie::automation::{AutomationTarget, SubtractiveParameter};
use sympoie::plugin::PluginKind;
use sympoie::project::Project;
use sympoie::track::{Track, TrackId};
use sympoie::transport::{PlaybackState, TransportCommand};

use super::{ClipClipboard, DetailView, Pending, SympoieApp};
use crate::actions::Action;
use crate::document::edit;
use crate::plugins::PluginEditor;
use crate::state::{GridMode, Selection, TimeRange};
use crate::ui::format;
use crate::views::{arrangement, builtin_devices, devices};

impl SympoieApp {
    pub(super) fn apply(&mut self, ctx: &egui::Context, action: Action) {
        if self.recording.is_some() {
            if matches!(action, Action::TogglePlay | Action::Stop) {
                self.finish_recording();
                return;
            }
            if matches!(
                action,
                Action::Transport(_)
                    | Action::SetBpm(_)
                    | Action::SetTimeSignature(_)
                    | Action::SetLoop { .. }
                    | Action::ToggleLoop
                    | Action::LoopSelection
                    | Action::LoopClip(_)
                    | Action::Undo
                    | Action::Redo
                    | Action::Save
                    | Action::SaveAs
                    | Action::NewProject
                    | Action::OpenProject
                    | Action::OpenProjectFolder
                    | Action::OpenPath(_)
                    | Action::Quit
                    | Action::ExportAll
                    | Action::ExportClipAudio(_)
                    | Action::ExportClipMidi(_)
                    | Action::BounceTrack { .. }
            ) {
                self.finish_recording();
            }
        }
        let structural_generation = self.structural_generation;
        let playing = self.status.state == PlaybackState::Playing;
        match action {
            Action::ToggleRecord => self.toggle_recording(),
            Action::ToggleTrackArm(track) => self.toggle_track_arm(track),
            Action::RefreshRecordingInputs => self.refresh_recording_inputs(),
            Action::SetAudioRecordingInput { track, input } => {
                self.finish_recording();
                self.edit("Recording Input", |project| {
                    project
                        .set_audio_recording_input(edit::audio_id(project, track)?, input)
                        .map_err(|error| error.to_string())
                });
            }
            Action::SetMidiRecordingInput { track, input } => {
                self.finish_recording();
                self.edit("Recording Input", |project| {
                    project
                        .set_midi_recording_input(edit::midi_id(project, track)?, input)
                        .map_err(|error| error.to_string())
                });
            }
            Action::Transport(TransportCommand::Play) => {
                self.sync_engine();
                self.engine.play_from_tick(self.arrangement.playback_start);
            }
            Action::Transport(command) => {
                if matches!(
                    command,
                    TransportCommand::Stop
                        | TransportCommand::ReturnToStart
                        | TransportCommand::SeekFrame(_)
                        | TransportCommand::SeekTick(_)
                ) {
                    self.piano_roll.cancel_audition();
                }
                if command == TransportCommand::ReturnToStart {
                    self.arrangement.playback_start = 0;
                }
                self.engine.command(command);
            }
            Action::TogglePlay => {
                if !playing && let Some(reason) = self.status.render_error.clone() {
                    self.warn(format!("Playback unavailable: {reason}"));
                } else {
                    self.sync_engine();
                    self.engine
                        .toggle_play_from_tick(self.arrangement.playback_start);
                }
            }
            Action::Stop => {
                self.piano_roll.cancel_audition();
                if self.status.state == PlaybackState::Stopped {
                    self.arrangement.playback_start = 0;
                    self.engine.command(TransportCommand::ReturnToStart);
                } else {
                    self.engine.command(TransportCommand::Stop);
                }
            }
            Action::Seek(tick) => self.arrangement.playback_start = tick,
            Action::SetLoop { start, end, enable } => {
                self.loop_region.start = start;
                self.loop_region.end = end.max(start + 1);
                self.loop_region.enabled |= enable;
                self.send_loop();
            }
            Action::ToggleLoop => {
                self.loop_region.enabled = !self.loop_region.enabled;
                self.send_loop();
            }
            Action::LoopSelection => {
                let span = self
                    .selection
                    .range
                    .as_ref()
                    .map(|range| (range.start, range.end))
                    .or_else(|| {
                        self.selection
                            .clip
                            .and_then(|clip| edit::clip_span(self.document.project(), clip))
                    });
                if let Some((start, end)) = span {
                    self.apply(
                        ctx,
                        Action::SetLoop {
                            start,
                            end,
                            enable: true,
                        },
                    );
                }
            }
            Action::LoopClip(clip) => {
                if let Some((start, end)) = edit::clip_span(self.document.project(), clip) {
                    self.apply(
                        ctx,
                        Action::SetLoop {
                            start,
                            end,
                            enable: true,
                        },
                    );
                }
            }
            Action::ToggleFollow => self.arrangement.follow = !self.arrangement.follow,
            Action::ToggleAgentMode => self.toggle_agent_mode(),
            Action::ToggleAutomation => {
                arrangement::cancel_interactions(ctx);
                self.arrangement.automation = !self.arrangement.automation;
                self.piano_roll.focused = false;
                self.selection.clip = None;
                self.selection.range = None;
            }
            Action::SetAutomation { track, lane } => {
                self.edit("Edit Automation", |project| {
                    let id = edit::track_id(project, track)?;
                    project
                        .set_track_automation(id, lane)
                        .map_err(|e| e.to_string())
                });
            }
            Action::RemoveAutomation { track, target } => {
                self.edit("Clear Automation", |project| {
                    let id = edit::track_id(project, track)?;
                    project
                        .remove_track_automation(id, target)
                        .map_err(|e| e.to_string())
                });
            }

            Action::SetBpm(bpm) => {
                self.edit_coalesced("bpm", "Tempo Change", |project| {
                    project.set_bpm(bpm).map_err(|e| e.to_string())
                });
            }
            Action::SetTimeSignature(meter) => {
                self.edit("Meter Change", |project| {
                    project.set_time_signature(meter);
                    Ok(())
                });
            }
            Action::SetSnap(grid) => {
                self.edit("Grid Change", |project| {
                    project.set_snap_grid(grid);
                    Ok(())
                });
            }
            Action::SetGridMode(mode) => self.arrangement.grid = mode,
            Action::StepGrid { narrower } => match self.arrangement.grid {
                GridMode::Adaptive(density) => {
                    self.arrangement.grid = GridMode::Adaptive(density.step(narrower));
                }
                GridMode::Fixed => {
                    let fixed = &format::GRIDS[1..];
                    let current = self.document.project().snap_grid();
                    if let Some(index) = fixed.iter().position(|grid| *grid == current) {
                        let next = if narrower {
                            (index + 1).min(fixed.len() - 1)
                        } else {
                            index.saturating_sub(1)
                        };
                        if next != index {
                            self.apply(ctx, Action::SetSnap(fixed[next]));
                        }
                    }
                }
            },

            Action::AddBusTrack => {
                self.apply(ctx, Action::AddTrack(edit::TrackKind::Bus));
                self.apply(ctx, Action::ShowMixer);
            }
            Action::AddTrack(kind) => {
                if let Some(index) = self.edit_structural("Add Track", |project| {
                    Ok(edit::add_track(project, kind, None))
                }) {
                    self.selection = Selection {
                        tracks: BTreeSet::from([index]),
                        clip: None,
                        range: None,
                    };
                    self.piano_roll.focused = false;
                }
            }
            Action::SetTrackColor { tracks, color } => {
                self.edit("Track Color", |project| {
                    for track in &tracks {
                        edit::set_track_color(project, *track, color)?;
                    }
                    Ok(())
                });
            }
            Action::RenameTrack(index, name) => {
                self.edit("Rename Track", |project| {
                    edit::rename_track(project, index, &name)
                });
            }
            Action::DeleteTrack(index) => {
                if self
                    .edit_structural("Delete Track", |project| edit::delete_track(project, index))
                    .is_some()
                {
                    self.selection = Selection::default();
                    self.piano_roll.close();
                    self.plugin_editors.clear();
                    self.arrangement.folded.clear();
                }
            }
            Action::BounceTrack { track, duplicate } => self.start_bounce(track, duplicate),
            Action::MoveTrack { track, target } => {
                if edit::can_drop_track(self.document.project(), track, target) {
                    self.edit_track_order("Move Track", |project| {
                        edit::move_track(project, track, target)
                    });
                }
            }
            Action::ExitGroup(track) => {
                if crate::document::rows::parents(self.document.project())
                    .get(track)
                    .is_some_and(Option::is_some)
                {
                    self.edit_track_order("Exit Group", |project| edit::exit_group(project, track));
                }
            }
            Action::GroupSelected => {
                let tracks: Vec<usize> = self.selection.tracks.iter().copied().collect();
                if let Some(group) = self.edit_structural("Group Tracks", |project| {
                    edit::group_tracks(project, &tracks)
                }) {
                    self.selection = Selection {
                        tracks: BTreeSet::from([group]),
                        clip: None,
                        range: None,
                    };
                    self.piano_roll.close();
                }
            }
            Action::Ungroup(index) => {
                if self
                    .edit_structural("Ungroup", |project| edit::ungroup(project, index))
                    .is_some()
                {
                    self.selection = Selection::default();
                    self.piano_roll.close();
                    self.plugin_editors.clear();
                    self.arrangement.folded.clear();
                }
            }
            Action::SetMixer { track, mixer } => {
                if self.arrangement.automation
                    && let Some(current) = self.document.project().tracks().get(track)
                {
                    let target = if mixer.gain() != current.mixer().gain() {
                        Some(AutomationTarget::TrackGain)
                    } else if mixer.pan() != current.mixer().pan() {
                        Some(AutomationTarget::TrackPan)
                    } else {
                        None
                    };
                    if let Some(target) = target {
                        self.arrangement.automation_targets.insert(track, target);
                    }
                }
                let key = format!("mixer-{track}");
                self.edit_coalesced(&key, "Mixer Change", |project| {
                    edit::set_mixer(project, track, mixer)
                });
            }
            Action::SetTrackSend { track, bus, gain } => {
                let project = self.document.project();
                let destination = project.track_ids().nth(bus);
                let exists = project.tracks().get(track).is_some_and(|track| {
                    track
                        .sends()
                        .iter()
                        .any(|send| Some(TrackId::Bus(send.destination)) == destination)
                });
                if exists {
                    self.edit_coalesced(&format!("send-{track}-{bus}"), "Send Level", |project| {
                        edit::set_track_send(project, track, bus, gain)
                    });
                } else {
                    self.edit("Add Send", |project| {
                        edit::set_track_send(project, track, bus, gain)
                    });
                }
            }
            Action::RemoveTrackSend { track, bus } => {
                self.edit("Remove Send", |project| {
                    edit::remove_track_send(project, track, bus)
                });
            }
            Action::SetSolo { track, soloed } => {
                self.edit("Track Solo", |project| {
                    let mixer = project
                        .tracks()
                        .get(track)
                        .ok_or("Track no longer exists")?
                        .mixer()
                        .with_soloed(soloed);
                    edit::set_mixer(project, track, mixer)
                });
            }
            Action::SetInstrument { track, instrument } => {
                self.edit("Change Instrument", |project| {
                    edit::set_instrument(project, track, instrument)
                });
            }
            Action::SetInstrumentBypassed { track, bypassed } => {
                self.edit("Instrument On/Off", |project| {
                    let midi = edit::midi_id(project, track)?;
                    project
                        .set_midi_track_instrument_bypassed(midi, bypassed)
                        .map_err(|error| error.to_string())
                });
            }
            Action::SetInstrumentParameter {
                track,
                instrument,
                parameter,
            } => {
                if self.arrangement.automation {
                    let target = match parameter {
                        "waveform" => Some(SubtractiveParameter::Waveform),
                        "cutoff" => Some(SubtractiveParameter::Cutoff),
                        "resonance" => Some(SubtractiveParameter::Resonance),
                        "attack" => Some(SubtractiveParameter::Attack),
                        "decay" => Some(SubtractiveParameter::Decay),
                        "sustain" => Some(SubtractiveParameter::Sustain),
                        "release" => Some(SubtractiveParameter::Release),
                        "gain" => Some(SubtractiveParameter::Gain),
                        _ => None,
                    };
                    if let Some(target) = target {
                        self.arrangement
                            .automation_targets
                            .insert(track, AutomationTarget::Subtractive(target));
                    }
                }
                let key = format!("instrument-{track}-{parameter}");
                self.edit_coalesced(&key, "Instrument Parameter", |project| {
                    edit::set_instrument(project, track, Some(instrument))
                });
            }
            Action::ToggleFold(index) => {
                if !self.arrangement.folded.remove(&index) {
                    self.arrangement.folded.insert(index);
                }
            }

            Action::MoveClip {
                clip,
                to_track,
                start,
            } => {
                if let Some(moved) = self.edit_structural("Move Clip", |project| {
                    edit::move_clip(project, clip, to_track, start)
                }) {
                    self.selection.select_clip(moved);
                    self.piano_roll.close();
                }
            }
            Action::CopyClipTo {
                clip,
                to_track,
                start,
            } => {
                if let Some(copy) = self.edit_structural("Copy Clip", |project| {
                    edit::copy_clip_to(project, clip, to_track, start)
                }) {
                    self.selection.select_clip(copy);
                    self.piano_roll.close();
                }
            }
            Action::CopyClip(clip) => match edit::copy_clip(self.document.project(), clip) {
                Ok(content) => {
                    let text = format!(
                        "Sympoie clip: track {}, clip {}, revision {}",
                        clip.track,
                        clip.index,
                        self.document.revision(),
                    );
                    ctx.copy_text(text.clone());
                    self.clip_clipboard = Some(ClipClipboard { content, text });
                }
                Err(error) => self.warn(error),
            },
            Action::PasteClip => {
                if let Some(clipboard) = &self.clip_clipboard {
                    let content = clipboard.content.clone();
                    let track = self.selection.tracks.first().copied();
                    if let Some(track) = track {
                        let start = self
                            .selection
                            .range
                            .as_ref()
                            .map_or(self.arrangement.playback_start, |range| range.start);
                        if let Some(copy) = self.edit_structural("Paste Clip", |project| {
                            edit::paste_clip(project, &content, track, start)
                        }) {
                            self.selection.select_clip(copy);
                            self.piano_roll.close();
                            if let Some((_, end)) = edit::clip_span(self.document.project(), copy) {
                                self.arrangement.playback_start = end;
                            }
                        }
                    }
                }
            }
            Action::TrimClipStart { clip, start } => {
                if let Some(trimmed) = self.edit_structural("Trim Clip", |project| {
                    edit::trim_start(project, clip, start)
                }) {
                    self.selection.select_clip(trimmed);
                    self.piano_roll.close();
                }
            }
            Action::TrimClipEnd { clip, end } => {
                if self
                    .edit_structural("Trim Clip", |project| edit::trim_end(project, clip, end))
                    .is_some()
                {
                    self.piano_roll.close();
                }
            }
            Action::RenameClip { clip, name } => {
                if edit::clip_name(self.document.project(), clip).unwrap_or_default() != name.trim()
                {
                    self.edit("Rename Clip", |project| {
                        edit::rename_clip(project, clip, &name)
                    });
                }
            }
            Action::SetClipGain { clip, gain } => {
                let key = format!("clip-gain-{}-{}", clip.track, clip.index);
                self.edit_coalesced(&key, "Clip Gain", |project| {
                    edit::set_clip_gain(project, clip, gain)
                });
            }
            Action::SetClipPitch { clip, semitones } => {
                if edit::audio_clip(self.document.project(), clip)
                    .is_some_and(|audio| audio.pitch_semitones != semitones)
                {
                    let key = format!("clip-pitch-{}-{}", clip.track, clip.index);
                    self.edit_coalesced(&key, "Clip Pitch", |project| {
                        edit::set_clip_pitch(project, clip, semitones)
                    });
                }
            }
            Action::SetClipPitchOptions {
                clip,
                algorithm,
                preserve_formants,
            } => {
                if edit::audio_clip(self.document.project(), clip).is_some_and(|audio| {
                    audio.pitch_algorithm != algorithm
                        || audio.preserve_formants != preserve_formants
                }) {
                    self.edit("Clip Pitch Algorithm", |project| {
                        edit::set_clip_pitch_options(project, clip, algorithm, preserve_formants)
                    });
                }
            }
            Action::ReverseClip(clip) => {
                if self
                    .edit("Reverse Clip", |project| edit::reverse_clip(project, clip))
                    .is_some()
                {
                    self.selection.select_clip(clip);
                }
            }
            Action::ResetClipPitch(clip) => {
                if edit::audio_clip(self.document.project(), clip)
                    .is_some_and(|audio| audio.pitch_semitones != 0.0)
                {
                    self.edit("Reset Clip Pitch", |project| {
                        edit::set_clip_pitch(project, clip, 0.0)
                    });
                }
            }
            Action::ReverseSelection => {
                if let Some(range) = self.selection.range.clone() {
                    if self.selection.has_audio(self.document.project())
                        && let Some(clips) = self.edit_structural("Reverse Selection", |project| {
                            edit::reverse_range(project, &range.tracks, range.start, range.end)
                        })
                    {
                        self.piano_roll.close();
                        if let [clip] = clips.as_slice() {
                            self.selection.select_clip(*clip);
                        }
                    }
                } else if let Some(clip) = self.selection.clip
                    && edit::audio_clip(self.document.project(), clip).is_some()
                {
                    self.apply(ctx, Action::ReverseClip(clip));
                }
            }
            Action::DeleteClip(clip) => {
                if self
                    .edit_structural("Delete Clip", |project| edit::delete_clip(project, clip))
                    .is_some()
                {
                    self.selection.clip = None;
                    self.piano_roll.close();
                }
            }
            Action::DuplicateClip(clip) => {
                if let Some(copy) = self.edit_structural("Duplicate Clip", |project| {
                    edit::duplicate_clip(project, clip)
                }) {
                    self.selection.select_clip(copy);
                    self.piano_roll.close();
                }
            }
            Action::SplitClip(clip) => {
                let tick = self.playhead();
                if let Some(right) = self.edit_structural("Split Clip", |project| {
                    edit::split_clip(project, clip, tick)
                }) {
                    self.selection.select_clip(right);
                    self.piano_roll.close();
                }
            }
            Action::InsertMidiClip { track, start } => {
                let bar = self.document.project().time_signature().ticks_per_bar();
                self.insert_midi_clip(track, start, bar);
            }
            Action::OpenPianoRoll(clip) => {
                if edit::midi_clip(self.document.project(), clip).is_some() {
                    self.selection.select_clip(clip);
                    self.detail_view = DetailView::Clip;
                    self.piano_roll.open(clip, self.document.project());
                }
            }
            Action::ClosePianoRoll => {
                self.piano_roll.close();
                self.detail_view = DetailView::Hidden;
            }
            Action::AuditionMidiNotes { track, notes } => {
                self.sync_engine();
                self.engine.audition_notes(track, notes);
            }
            Action::BeginMidiAudition { track, notes } => {
                self.sync_engine();
                self.engine.begin_audition_notes(track, notes);
            }
            Action::ReleaseMidiAudition => self.engine.release_audition_notes(),
            Action::EditMidiNotes {
                clip,
                before,
                notes,
                label,
            } => {
                if self.piano_roll.clip == Some(clip)
                    && edit::midi_clip(self.document.project(), clip)
                        .is_some_and(|current| current.notes == before && current.notes != notes)
                {
                    let committed = self
                        .edit(label, |project| {
                            let track = edit::midi_id(project, clip.track)?;
                            project
                                .set_midi_clip_notes(track, clip.index, notes)
                                .map_err(|e| e.to_string())
                        })
                        .is_some();
                    if committed {
                        self.piano_roll.notes_committed();
                    } else {
                        self.piano_roll.invalidate();
                    }
                } else {
                    self.piano_roll.invalidate();
                }
            }
            Action::EditMidiPitchBend {
                clip,
                before,
                points,
                label,
            } => {
                if self.piano_roll.clip == Some(clip)
                    && edit::midi_clip(self.document.project(), clip).is_some_and(|current| {
                        current.pitch_bend == before && current.pitch_bend != points
                    })
                {
                    let committed = self
                        .edit(label, |project| {
                            let track = edit::midi_id(project, clip.track)?;
                            project
                                .set_midi_clip_pitch_bend(track, clip.index, points)
                                .map_err(|e| e.to_string())
                        })
                        .is_some();
                    if !committed {
                        self.piano_roll.invalidate();
                    }
                } else {
                    self.piano_roll.invalidate();
                }
            }
            Action::ImportAudio {
                paths,
                track,
                start,
            } => self.import(paths, track, start),
            Action::ChooseAudioToImport { track, start } => {
                self.release_preview_before_dialog();
                if let Some(paths) = rfd::FileDialog::new()
                    .set_title("Import Audio")
                    .add_filter(
                        "WAV audio (48 kHz stereo, PCM16 or float32)",
                        &["wav", "WAV"],
                    )
                    .pick_files()
                {
                    self.import(paths, track, start);
                }
            }

            Action::AddLibraryFolder => {
                self.release_preview_before_dialog();
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Add Folder to Custom Library")
                    .pick_folder()
                    && let Err(error) = self.browser.add_folder(path)
                {
                    self.error(error);
                }
            }
            Action::RemoveLibraryFolder(path) => {
                if let Err(error) = self.browser.remove_folder(&path) {
                    self.error(error);
                }
            }

            Action::SelectClip(clip) => {
                self.piano_roll.focused = false;
                match clip {
                    Some(clip) => {
                        self.selection.select_clip(clip);
                    }
                    None => {
                        self.selection.clip = None;
                        self.selection.range = None;
                    }
                }
            }
            Action::SetTimeSelection(range) => {
                self.piano_roll.focused = false;
                if let Some(range) = &range {
                    self.selection.clip = None;
                    self.selection.tracks = range.tracks.clone();
                }
                self.selection.range = range;
                self.selection.validate(self.document.project());
            }
            Action::MoveTimeSelection { to } => {
                if let Some(range) = self.selection.range.clone()
                    && self
                        .edit_structural("Move Selection", |project| {
                            edit::move_range(project, &range.tracks, range.start, range.end, to)
                        })
                        .is_some()
                {
                    self.selection.clip = None;
                    self.selection.range = Some(TimeRange {
                        start: to,
                        end: to + range.len(),
                        tracks: range.tracks,
                    });
                    self.piano_roll.close();
                }
            }
            Action::DeleteTimeSelection => {
                if let Some(range) = self.selection.range.clone()
                    && self
                        .edit_structural("Delete Selection", |project| {
                            edit::clear_range(project, &range.tracks, range.start, range.end)
                        })
                        .is_some()
                {
                    self.selection.clip = None;
                    self.piano_roll.close();
                }
            }
            Action::SplitTimeSelection => {
                if let Some(range) = self.selection.range.clone()
                    && self
                        .edit_structural("Split", |project| {
                            edit::split_range(project, &range.tracks, range.start, range.end)
                        })
                        .is_some()
                {
                    self.selection.clip = None;
                    self.piano_roll.close();
                }
            }
            Action::InsertMidiClipInSelection => self.insert_midi_clip_in_selection(),
            Action::SelectTracks(tracks) => {
                self.selection.tracks = tracks;
                self.selection.clip = None;
                self.selection.validate(self.document.project());
                self.piano_roll.focused = false;
            }

            Action::NewProject => self.after_saving(ctx, Pending::New),
            Action::OpenProject => self.after_saving(ctx, Pending::Open(None)),
            Action::OpenProjectFolder => self.after_saving(ctx, Pending::OpenFolder),
            Action::OpenPath(path) => self.after_saving(ctx, Pending::Open(Some(path))),
            Action::Save => {
                self.save(false);
            }
            Action::SaveAs => {
                self.save(true);
            }
            Action::ShowExport => self.open_export_window(),
            Action::ExportAll => self.export_all(),
            Action::ExportClipMidi(clip) => self.choose_clip_midi_export(clip),
            Action::ExportClipAudio(clip) => self.choose_clip_audio_export(clip),
            Action::Undo | Action::Redo => {
                arrangement::cancel_interactions(ctx);
                let track_count = self.document.project().tracks().len();
                let undo = action == Action::Undo;
                let track_mapping = if undo {
                    self.document.undo_track_mapping()
                } else {
                    self.document.redo_track_mapping()
                }
                .map(<[usize]>::to_vec);
                let invalidates_clip_refs = if undo {
                    self.document.undo_invalidates_clip_refs()
                } else {
                    self.document.redo_invalidates_clip_refs()
                };
                let label = if undo {
                    self.document.undo()
                } else {
                    self.document.redo()
                };
                if let Some(label) = label {
                    if self.document.project().tracks().len() != track_count {
                        self.arrangement.automation_targets.clear();
                        self.arrangement.track_heights.clear();
                        self.armed_tracks.clear();
                    }
                    if let Some(mapping) = track_mapping {
                        self.structural_generation = self.structural_generation.wrapping_add(1);
                        self.remap_track_references(&mapping);
                    } else if invalidates_clip_refs {
                        self.piano_roll.close();
                        self.plugin_editors.clear();
                        self.plugin_browser.open = false;
                        self.structural_generation = self.structural_generation.wrapping_add(1);
                    } else {
                        self.piano_roll.invalidate();
                    }
                    self.info(format!("{} {label}", if undo { "Undid" } else { "Redid" }));
                } else {
                    self.info(format!("Nothing to {}", if undo { "undo" } else { "redo" }));
                }
            }
            Action::Quit => {
                if self.document.is_dirty() {
                    self.pending = Some(Pending::Quit);
                } else {
                    self.allowed_to_close = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }

            Action::BrowsePlugins { track, kind } => {
                if kind == PluginKind::Effect
                    && (self.detail_view == DetailView::Mixer
                        || matches!(
                            self.document.project().tracks().get(track),
                            Some(Track::Bus(_))
                        ))
                {
                    self.plugin_browser.open_mixer(track);
                } else {
                    self.plugin_browser.open(track, kind);
                }
            }
            Action::BrowseMixerEffects { track } => self.plugin_browser.open_mixer(track),
            Action::LoadMixerVst3 {
                track,
                plugin,
                before,
            } => {
                self.queue_plugin_load_in_chain(track, plugin, before, true);
            }
            Action::LoadVst3 { track, plugin } => {
                if matches!(
                    self.document.project().tracks().get(track),
                    Some(Track::Bus(_))
                ) {
                    self.queue_plugin_load_in_chain(track, plugin, None, true);
                } else {
                    self.queue_plugin_load(track, plugin, None);
                }
            }
            Action::InsertVst3Before {
                track,
                plugin,
                before,
            } => {
                self.queue_plugin_load(track, plugin, Some(before));
            }
            Action::SetVst3Parameter {
                id,
                parameter,
                value,
            } => self.set_plugin_parameter(id, parameter, value),
            Action::LoadSamplerSample { id } => self.load_sampler_sample(id),
            Action::ResetBuiltinParameters { id } => {
                self.edit("Reset Device Controls", |project| {
                    let plugin = project.plugin(id).ok_or("Device no longer exists")?;
                    let kind = sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)
                        .ok_or("Device is not built in")?;
                    for spec in sympoie::builtin::parameters(kind) {
                        project
                            .set_plugin_parameter(id, spec.id, spec.normalize(spec.default))
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                });
            }
            Action::LoadBuiltinPreset { id, preset } => {
                self.edit("Load Device Preset", |project| {
                    let plugin = project.plugin(id).ok_or("Device no longer exists")?;
                    let kind = sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)
                        .ok_or("Device is not built in")?;
                    let values = sympoie::builtin::preset_parameters(kind, preset)
                        .ok_or("Unknown preset")?;
                    for (parameter, value) in values {
                        project
                            .set_plugin_parameter(id, parameter, value)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                });
            }
            Action::SetVst3Bypassed { id, bypassed } => {
                self.edit("Plugin Bypass", |project| {
                    project
                        .set_plugin_bypassed(id, bypassed)
                        .map_err(|e| e.to_string())
                });
            }
            Action::SetVst3Sidechain { id, route } => {
                self.edit("Sidechain Routing", |project| {
                    project
                        .set_plugin_sidechain(id, route)
                        .map_err(|e| e.to_string())
                });
            }
            Action::OpenVst3Sidechain(id) => {
                if self
                    .document
                    .project()
                    .plugin(id)
                    .is_some_and(|plugin| plugin.descriptor.kind == PluginKind::Effect)
                {
                    if let Some(track) = self.plugin_track(id) {
                        self.select_editor_track(track);
                    }
                    self.sidechain_editors.insert(id);
                }
            }
            Action::CloseVst3Sidechain(id) => {
                self.sidechain_editors.remove(&id);
            }
            Action::SetVst3SidechainGain { id, gain } => {
                self.edit_coalesced(
                    &format!("sidechain-gain-{id}"),
                    "Sidechain Gain",
                    |project| {
                        let mut route = project
                            .plugin(id)
                            .and_then(|plugin| plugin.sidechain)
                            .ok_or("Plugin has no sidechain connection")?;
                        route.gain = gain;
                        project
                            .set_plugin_sidechain(id, Some(route))
                            .map_err(|e| e.to_string())
                    },
                );
            }
            Action::RemoveVst3 { track, id } => {
                self.edit_structural("Remove Plugin", |project| {
                    let target = project.tracks().get(track).ok_or("Track no longer exists")?;
                    if let Some(position) = target.effects().iter().position(|plugin| plugin.id == id) {
                        let track = edit::track_id(project, track)?;
                        project.remove_effect(track, position).map_err(|e| e.to_string())?;
                    } else if let Some(position) = target.mixer_effects().iter().position(|plugin| plugin.id == id) {
                        let track = edit::track_id(project, track)?;
                        project.remove_mixer_effect(track, position).map_err(|e| e.to_string())?;
                    } else if matches!(target, Track::Midi(midi) if midi.plugin_instrument().is_some_and(|plugin| plugin.id == id)) {
                        let track = edit::midi_id(project, track)?;
                        project.set_plugin_instrument(track, None).map_err(|e| e.to_string())?;
                    } else { return Err("Plugin no longer exists on this track".to_owned()); }
                    Ok(())
                });
            }
            Action::MoveEffect { track, from, to } => {
                self.edit_structural("Move Effect", |project| {
                    let track = edit::track_id(project, track)?;
                    project
                        .move_effect(track, from, to)
                        .map_err(|e| e.to_string())
                });
            }
            Action::MoveMixerEffect { track, from, to } => {
                self.edit_structural("Move Mixer Effect", |project| {
                    let track = edit::track_id(project, track)?;
                    project
                        .move_mixer_effect(track, from, to)
                        .map_err(|e| e.to_string())
                });
            }
            Action::OpenVst3Editor(id) => {
                let panel = self.document.project().plugin(id).is_some_and(|plugin| {
                    sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)
                        .is_some_and(builtin_devices::has_panel)
                });
                if !panel && self.focus_builtin_controls(ctx, id) {
                    return;
                }
                if let Some(track) = self.plugin_track(id) {
                    self.select_editor_track(track);
                    self.vst3_editors.insert(id);
                    ctx.send_viewport_cmd_to(
                        devices::vst3_editor_viewport_id(id),
                        ViewportCommand::Focus,
                    );
                }
            }
            Action::CloseVst3Editor(id) => {
                self.vst3_editors.remove(&id);
            }
            Action::OpenBuiltinScope(id) => {
                if self.document.project().plugin(id).is_some_and(|plugin| {
                    sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)
                        .is_some_and(builtin_devices::has_scope)
                }) {
                    self.scope_windows.insert(id);
                    ctx.send_viewport_cmd_to(
                        devices::builtin_scope_viewport_id(id),
                        ViewportCommand::Focus,
                    );
                }
            }
            Action::CloseBuiltinScope(id) => {
                self.scope_windows.remove(&id);
            }
            Action::OpenNativeVst3Editor(id) => {
                if self.document.project().plugin(id).is_some_and(|plugin| {
                    sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor).is_some()
                }) {
                    self.apply(ctx, Action::OpenVst3Editor(id));
                    return;
                }
                let Some(track) = self.plugin_track(id) else {
                    return;
                };
                self.select_editor_track(track);
                if let Some(editor) = self.native_editors.get_mut(&id)
                    && editor.is_open()
                {
                    if let Err(error) = editor.focus() {
                        self.warn(format!("Cannot focus plugin editor: {error}"));
                    }
                } else {
                    self.native_editors.remove(&id);
                    match PluginEditor::open(&self.plugins, id, self.plugin_window_owner.as_ref()) {
                        Ok(editor) => {
                            self.native_editors.insert(id, editor);
                        }
                        Err(error) => {
                            self.vst3_editors.insert(id);
                            ctx.send_viewport_cmd_to(
                                devices::vst3_editor_viewport_id(id),
                                ViewportCommand::Focus,
                            );
                            self.warn(format!("Plugin UI unavailable: {error}. Its parameters are available in the separate editor."));
                        }
                    }
                }
            }
            Action::ShowDevices => {
                self.show_rack = true;
                self.close_drawer();
            }
            Action::CloseDrawer => self.close_drawer(),
            Action::ShowClipView => {
                let midi = self
                    .selection
                    .clip
                    .filter(|clip| edit::midi_clip(self.document.project(), *clip).is_some());
                if let Some(clip) = midi {
                    self.apply(ctx, Action::OpenPianoRoll(clip));
                } else {
                    self.piano_roll.close();
                    self.detail_view = DetailView::Clip;
                }
            }
            Action::ShowMixer => {
                self.piano_roll.close();
                self.detail_view = DetailView::Mixer;
            }
            Action::OpenPluginEditor(track) => {
                if matches!(self.document.project().tracks().get(track),
                    Some(Track::Midi(midi)) if midi.instrument().is_some())
                {
                    self.select_editor_track(track);
                    self.plugin_editors.insert(track);
                    ctx.send_viewport_cmd_to(
                        devices::editor_viewport_id(track),
                        ViewportCommand::Focus,
                    );
                }
            }
            Action::ClosePluginEditor(track) => {
                self.plugin_editors.remove(&track);
            }
            Action::ToggleMixer => {
                let visible =
                    self.piano_roll.clip.is_none() && self.detail_view == DetailView::Mixer;
                self.piano_roll.close();
                self.detail_view = if visible {
                    DetailView::Hidden
                } else {
                    DetailView::Mixer
                };
            }
            Action::ToggleBrowser => self.show_browser = !self.show_browser,
            Action::ToggleRack => self.show_rack = !self.show_rack,
            Action::ToggleDetailView => {
                let hidden =
                    self.piano_roll.clip.is_none() && self.detail_view == DetailView::Hidden;
                self.piano_roll.close();
                self.detail_view = if hidden {
                    DetailView::Devices
                } else {
                    DetailView::Hidden
                };
            }
            Action::Zoom(factor) => {
                let playhead = arrangement::playhead_tick(
                    self.document.project(),
                    &self.status,
                    self.loop_region,
                    self.arrangement.playback_start,
                );
                let view = &mut self.arrangement;
                let anchor = ((playhead - view.scroll_tick) * view.pixels_per_tick) as f32;
                let anchor = if (0.0..=view.lanes_width).contains(&anchor) {
                    anchor
                } else {
                    view.lanes_width / 2.0
                };
                view.zoom_around(factor, anchor);
            }
            Action::ZoomToFit => self.zoom_to_fit(),
            Action::ResetTrackHeights => {
                arrangement::cancel_interactions(ctx);
                self.arrangement.set_all_track_heights(68.0);
            }
            Action::ShowShortcuts => self.show_shortcuts = true,
            Action::ShowAbout => self.show_about = true,
            Action::ShowSettings => self.open_settings(),
        }
        if self.structural_generation != structural_generation {
            arrangement::cancel_interactions(ctx);
        }
        self.sync_editor_visibility();
    }

    fn insert_midi_clip_in_selection(&mut self) {
        let project = self.document.project();
        let bar = project.time_signature().ticks_per_bar();
        let is_midi = |track: usize| matches!(project.tracks().get(track), Some(Track::Midi(_)));
        let Some(range) = self.selection.range.clone() else {
            let tracks = &self.selection.tracks;
            let track = tracks
                .first()
                .copied()
                .filter(|track| tracks.len() == 1 && is_midi(*track));
            let start = edit::snap(self.arrangement.grid(project), self.playhead(), true);
            match track {
                Some(track) => self.insert_midi_clip(track, start, bar),
                None => self.warn(
                    "Select an empty range on a MIDI track, or a MIDI track, to insert a clip."
                        .to_owned(),
                ),
            }
            return;
        };
        let track = *range.tracks.first().expect("a time selection has tracks");
        let single_midi = range.tracks.len() == 1 && is_midi(track);
        let empty = edit::range_is_empty(project, track, range.start, range.end);
        if !single_midi {
            self.warn("Select an empty range on a single MIDI track to insert a clip.".to_owned());
        } else if !empty {
            self.warn(
                "The selection overlaps a clip. Select an empty range to insert a clip.".to_owned(),
            );
        } else {
            self.insert_midi_clip(track, range.start, range.len());
        }
    }

    fn insert_midi_clip(&mut self, track: usize, start: u64, length: u64) {
        if let Some(clip) = self.edit_structural("Insert MIDI Clip", |project| {
            edit::insert_midi_clip(project, track, start, length)
        }) {
            self.selection.select_clip(clip);
            self.detail_view = DetailView::Clip;
            self.piano_roll.open(clip, self.document.project());
            self.info("Inserted an empty MIDI clip. Draw notes in the piano roll; choose the track's instrument in its Rack.".to_owned());
        }
    }

    fn edit<T>(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Option<T> {
        self.document.break_coalescing();
        let result = self.document.edit(label, change);
        self.report_edit(label, result)
    }

    pub(super) fn edit_structural<T>(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Option<T> {
        self.finish_recording();
        self.document.break_coalescing();
        let track_count = self.document.project().tracks().len();
        let result = self.document.edit_structural(label, change);
        if result.is_ok() {
            self.piano_roll.cancel_audition();
            if self.document.project().tracks().len() != track_count {
                self.arrangement.automation_targets.clear();
                self.armed_tracks.clear();
            }
            if self.document.project().tracks().len() < track_count {
                self.arrangement.track_heights.clear();
            }
            self.structural_generation = self.structural_generation.wrapping_add(1);
            self.plugin_browser.open = false;
        }
        self.report_edit(label, result)
    }

    pub(super) fn edit_coalesced<T>(
        &mut self,
        key: &str,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Option<T> {
        let result = self.document.edit_coalesced(key, label, change);
        self.report_edit(label, result)
    }

    fn edit_track_order(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut Project) -> Result<Vec<usize>, String>,
    ) {
        self.finish_recording();
        self.document.break_coalescing();
        let result = self.document.edit_track_order(label, change);
        if let Some(mapping) = self.report_edit(label, result) {
            self.structural_generation = self.structural_generation.wrapping_add(1);
            self.remap_track_references(&mapping);
        }
    }

    fn remap_track_references(&mut self, mapping: &[usize]) {
        let remap_set = |indices: &BTreeSet<usize>| {
            indices
                .iter()
                .filter_map(|&index| mapping.get(index).copied())
                .collect()
        };
        let remap_clip = |clip: &mut Option<edit::ClipRef>| {
            *clip = clip.and_then(|clip| {
                mapping
                    .get(clip.track)
                    .map(|&track| edit::ClipRef { track, ..clip })
            });
        };
        self.selection.tracks = remap_set(&self.selection.tracks);
        self.armed_tracks = remap_set(&self.armed_tracks);
        remap_clip(&mut self.selection.clip);
        if let Some(range) = &mut self.selection.range {
            range.tracks = remap_set(&range.tracks);
        }
        remap_clip(&mut self.piano_roll.clip);
        self.piano_roll.invalidate();
        self.plugin_editors = remap_set(&self.plugin_editors);
        self.arrangement.folded = self
            .arrangement
            .folded
            .iter()
            .filter_map(|&index| mapping.get(index).copied())
            .collect();
        self.arrangement.renaming = self
            .arrangement
            .renaming
            .take()
            .and_then(|(index, name)| mapping.get(index).map(|&index| (index, name)));
        self.arrangement.automation_targets = self
            .arrangement
            .automation_targets
            .iter()
            .filter_map(|(&index, &target)| mapping.get(index).map(|&index| (index, target)))
            .collect();
        self.arrangement.track_heights = self
            .arrangement
            .track_heights
            .iter()
            .filter_map(|(&index, &height)| mapping.get(index).map(|&index| (index, height)))
            .collect();
        self.plugin_browser.open = false;
        self.plugin_load = self.plugin_load.take().and_then(|mut load| {
            if load.generation != self.structural_generation.wrapping_sub(1) {
                return None;
            }
            load.track = *mapping.get(load.track)?;
            load.generation = self.structural_generation;
            Some(load)
        });
        self.drop_target = None;
    }

    pub(super) fn set_plugin_parameter(&mut self, id: u64, parameter: u32, value: f64) {
        if self.arrangement.automation
            && let Some(track) = self.plugin_track(id)
        {
            self.arrangement.automation_targets.insert(
                track,
                AutomationTarget::PluginParameter {
                    plugin: id,
                    parameter,
                },
            );
        }
        let key = if parameter < 30
            && self.document.project().plugin(id).is_some_and(|plugin| {
                sympoie::builtin::BuiltinKind::from_descriptor(&plugin.descriptor)
                    == Some(sympoie::builtin::BuiltinKind::Eq)
            }) {
            format!("builtin-eq-{id}-band-{}", parameter / 5)
        } else {
            format!("vst3-{id}-{parameter}")
        };
        self.edit_coalesced(&key, "Plugin Parameter", |project| {
            project
                .set_plugin_parameter(id, parameter, value)
                .map_err(|e| e.to_string())
        });
    }

    fn report_edit<T>(&mut self, label: &str, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error(format!("{label}: {error}"));
                None
            }
        }
    }

    pub(super) fn send_loop(&mut self) {
        if self.loop_region.enabled {
            self.engine.command(TransportCommand::SetLoopTicks {
                start: self.loop_region.start,
                end: self.loop_region.end,
            });
        } else {
            self.engine.command(TransportCommand::ClearLoop);
        }
    }
}
