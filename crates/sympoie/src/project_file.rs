use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::automation::{
    AutomationLane, AutomationPoint, AutomationTarget, MAX_AUTOMATION_LANES, MAX_AUTOMATION_POINTS,
    SubtractiveParameter,
};
use crate::clip::{AudioClip, MidiClip, MidiNote};
use crate::instrument::{Instrument, SubtractiveSettings, Waveform};
use crate::pitch::PitchAlgorithm;
use crate::plugin::{
    MAX_PLUGIN_PARAMETERS, MAX_PLUGIN_STATE_BYTES, MAX_PLUGIN_STRING_BYTES, PluginDescriptor,
    PluginInstance, PluginKind, PluginParameter, SidechainRoute, SidechainTap,
};
use crate::project::{AudioBuffer, Project, ProjectError};
use crate::time::{SnapGrid, TimeError, TimeSignature};
use crate::track::{
    AudioInputChannels, AudioOutput, AudioRecordingInput, BusTrackId, GroupTrackId,
    MAX_RECORDING_DEVICE_BYTES, MidiRecordingInput, MixerSettings, Track, TrackId,
};
use crate::{SAMPLE_RATE, TICKS_PER_QUARTER};

const MAGIC: &[u8; 8] = b"SYMPROJ\0";
const VERSION: u32 = 13;
pub const PROJECT_MANIFEST: &str = "project.symp";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub fn save_project(project: &Project, path: impl AsRef<Path>) -> Result<(), ProjectFileError> {
    atomic_write(path.as_ref(), |writer| write_project(project, writer))
}

pub(crate) fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<&File>) -> Result<(), ProjectFileError>,
) -> Result<(), ProjectFileError> {
    let (temporary, file) = temporary_file(path)?;
    {
        let mut writer = BufWriter::new(&file);
        write(&mut writer)?;
        writer.flush()?;
    }
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary.0, path)?;
    Ok(())
}

fn temporary_file(path: &Path) -> io::Result<(TemporaryFile, File)> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "project path needs a file name",
        )
    })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    loop {
        let mut temporary_name = name.to_os_string();
        temporary_name.push(format!(
            ".tmp.{}.{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((TemporaryFile(temporary), file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn save_project_directory(
    project: &Project,
    path: impl AsRef<Path>,
) -> Result<(), ProjectFileError> {
    project.processing_order()?;
    let path = path.as_ref();
    fs::create_dir_all(path)?;
    let samples = path.join("samples");
    match fs::create_dir(&samples) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&samples)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProjectFileError::InvalidFormat(
            "samples must be a real directory",
        ));
    }
    let mut references = Vec::with_capacity(project.assets().len());
    for asset in project.assets() {
        references.push(save_sample(asset, &samples)?);
    }
    sync_directory(&samples)?;
    atomic_write(&path.join(PROJECT_MANIFEST), |writer| {
        write_project_assets(project, writer, Some(&references))
    })?;
    sync_directory(path)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) fn save_sample(
    asset: &AudioBuffer,
    directory: &Path,
) -> Result<String, ProjectFileError> {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in asset.samples().iter().flatten() {
        for byte in sample.to_bits().to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    let stem = format!("sample-{hash:016x}-{}", asset.len());
    let mut temporary = None;
    for suffix in 0_u64.. {
        let name = if suffix == 0 {
            format!("{stem}.wav")
        } else {
            format!("{stem}-{suffix}.wav")
        };
        let destination = directory.join(&name);
        if sample_is_pending(&destination)? {
            continue;
        }
        match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    continue;
                }
                if sample_matches(asset, &destination)? {
                    return Ok(format!("samples/{name}"));
                }
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if temporary.is_none() {
            let (temp, file) = temporary_file(&destination)?;
            {
                let mut writer =
                    crate::wav::FloatWavWriter::new(BufWriter::new(&file), asset.len() as u64)?;
                writer.write_frames(asset.samples())?;
                writer.finish()?;
            }
            file.sync_all()?;
            temporary = Some(temp);
        }
        match publish_sample(
            &temporary.as_ref().unwrap().0,
            &destination,
            |source, target| fs::hard_link(source, target),
        ) {
            Ok(()) => return Ok(format!("samples/{name}")),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if sample_matches(asset, &destination)? {
                    return Ok(format!("samples/{name}"));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("sample suffix space exhausted")
}

fn pending_sample_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap().to_os_string();
    name.push(".pending");
    destination.with_file_name(name)
}

fn sample_is_pending(destination: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(pending_sample_path(destination)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn sample_matches(asset: &AudioBuffer, destination: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    if sample_is_pending(destination)? {
        return Ok(false);
    }
    Ok(crate::wav::read_wav(destination).is_ok_and(|existing| same_sample_bits(asset, &existing)))
}

fn publish_sample(
    source: &Path,
    destination: &Path,
    hard_link: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    match hard_link(source, destination) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(error),
        Err(_) => {}
    }
    let marker = pending_sample_path(destination);
    drop(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?,
    );
    let mut publication = PendingSample {
        marker,
        created: None,
    };
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    publication.created = Some(destination.to_owned());
    let mut input = File::open(source)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    publication.created = None;
    Ok(())
}

struct PendingSample {
    marker: PathBuf,
    created: Option<PathBuf>,
}

impl Drop for PendingSample {
    fn drop(&mut self) {
        if let Some(created) = &self.created
            && let Err(error) = fs::remove_file(created)
            && error.kind() != io::ErrorKind::NotFound
        {
            return;
        }
        let _ = fs::remove_file(&self.marker);
    }
}

fn same_sample_bits(left: &AudioBuffer, right: &AudioBuffer) -> bool {
    left.len() == right.len()
        && left
            .samples()
            .iter()
            .flatten()
            .zip(right.samples().iter().flatten())
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

pub(crate) fn read_sample(
    base: &Path,
    reference: &str,
    frames: Option<u64>,
) -> Result<AudioBuffer, ProjectFileError> {
    let name = reference
        .strip_prefix("samples/")
        .and_then(|name| name.strip_suffix(".wav"))
        .filter(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or(ProjectFileError::InvalidFormat("invalid sample path"))?;
    let samples = base.join("samples");
    let path = samples.join(format!("{name}.wav"));
    for (entry, directory) in [(&samples, true), (&path, false)] {
        let metadata = fs::symlink_metadata(entry).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot load sample {}: {error}", path.display()),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ProjectFileError::InvalidFormat(
                "sample paths cannot contain symlinks",
            ));
        }
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(ProjectFileError::InvalidFormat(
                "sample path has the wrong file type",
            ));
        }
    }
    let audio = crate::wav::read_wav(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot load sample {}: {error}", path.display()),
        )
    })?;
    if frames.is_some_and(|frames| audio.len() as u64 != frames) {
        return Err(ProjectFileError::InvalidFormat(
            "sample frame count differs from manifest",
        ));
    }
    Ok(audio)
}

pub fn load_project(path: impl AsRef<Path>) -> Result<Project, ProjectFileError> {
    let path = path.as_ref();
    if path.is_dir() && path.join(crate::project_json::MANIFEST).is_file() {
        return crate::project_json::load_project_folder(path);
    }
    if path
        .file_name()
        .is_some_and(|name| name == crate::project_json::MANIFEST)
    {
        return crate::project_json::load_project_folder(path.parent().unwrap_or(Path::new(".")));
    }
    let manifest = if path.is_dir() {
        path.join(PROJECT_MANIFEST)
    } else {
        path.to_owned()
    };
    read_project_with_base(File::open(&manifest)?, manifest.parent())
}

pub fn write_project(project: &Project, writer: impl Write) -> Result<(), ProjectFileError> {
    write_project_assets(project, writer, None)
}

fn write_project_assets(
    project: &Project,
    writer: impl Write,
    references: Option<&[String]>,
) -> Result<(), ProjectFileError> {
    project.processing_order()?;
    let mut out = Encoder(writer);
    out.0.write_all(MAGIC)?;
    out.u32(VERSION)?;
    out.u32(SAMPLE_RATE)?;
    out.u64(TICKS_PER_QUARTER)?;
    out.f64(project.bpm())?;
    out.u16(project.time_signature().numerator())?;
    out.u16(project.time_signature().denominator())?;
    out.u8(match project.snap_grid() {
        SnapGrid::Off => 0,
        SnapGrid::Bar => 1,
        SnapGrid::Quarter => 2,
        SnapGrid::Eighth => 3,
        SnapGrid::Sixteenth => 4,
    })?;
    out.u8(u8::from(references.is_some()))?;
    out.u64(project.assets().len() as u64)?;
    for (index, asset) in project.assets().iter().enumerate() {
        if let Some(references) = references {
            out.string(&references[index])?;
            out.u64(asset.len() as u64)?;
            continue;
        }
        out.u64(asset.len() as u64)?;
        for frame in asset.samples() {
            out.f32(frame[0])?;
            out.f32(frame[1])?;
        }
    }
    out.u64(project.tracks().len() as u64)?;
    for track in project.tracks() {
        out.u8(match track {
            Track::Audio(_) => 0,
            Track::Midi(_) => 1,
            Track::Group(_) => 2,
            Track::Master(_) => 3,
            Track::Bus(_) => 4,
        })?;
        out.u64(track.name().len() as u64)?;
        out.0.write_all(track.name().as_bytes())?;
        out.f32(track.mixer().gain())?;
        out.f32(track.mixer().pan())?;
        match track.mixer().output() {
            AudioOutput::Master => out.u8(0)?,
            AudioOutput::Group(group) => {
                out.u8(1)?;
                out.u64(group.0 as u64)?;
            }
            AudioOutput::Bus(bus) => {
                out.u8(4)?;
                out.u64(bus.0 as u64)?;
            }
            AudioOutput::Output => out.u8(2)?,
            AudioOutput::Disconnected => out.u8(3)?,
        }
        match track.color() {
            Some(color) => {
                out.u8(1)?;
                out.u32(color)?;
            }
            None => out.u8(0)?,
        }
        out.u8(u8::from(track.mixer().muted()))?;
        match track {
            Track::Audio(track) => {
                out.u64(track.clips().len() as u64)?;
                for clip in track.clips() {
                    out.u64(clip.asset.index() as u64)?;
                    out.u64(clip.source_start)?;
                    out.u64(clip.start_tick)?;
                    out.u64(clip.frame_count)?;
                    out.f32(clip.gain)?;
                    out.f32(clip.pitch_semitones)?;
                    out.u8(match clip.pitch_algorithm {
                        PitchAlgorithm::Builtin => 0,
                        PitchAlgorithm::Signalsmith => 1,
                        PitchAlgorithm::RubberBandR3 => 2,
                    })?;
                    out.u8(u8::from(clip.preserve_formants))?;
                }
            }
            Track::Midi(track) => {
                match track.instrument() {
                    None => out.u8(0)?,
                    Some(Instrument::Subtractive(settings)) => {
                        out.u8(1)?;
                        out.u8(match settings.waveform {
                            Waveform::Sine => 0,
                            Waveform::Saw => 1,
                            Waveform::Square => 2,
                        })?;
                        for value in [
                            settings.cutoff_hz,
                            settings.resonance,
                            settings.attack_seconds,
                            settings.decay_seconds,
                            settings.sustain,
                            settings.release_seconds,
                            settings.gain,
                        ] {
                            out.f32(value)?;
                        }
                    }
                }
                out.u64(track.clips().len() as u64)?;
                for clip in track.clips() {
                    out.u64(clip.start_tick)?;
                    out.u64(clip.length_ticks)?;
                    out.u64(clip.offset_ticks)?;
                    out.u64(clip.notes.len() as u64)?;
                    for note in &clip.notes {
                        out.u64(note.start_tick)?;
                        out.u64(note.duration_ticks)?;
                        out.u8(note.key)?;
                        out.u8(note.velocity)?;
                        out.u8(note.channel)?;
                    }
                }
            }
            Track::Group(_) | Track::Bus(_) | Track::Master(_) => {}
        }
    }
    out.u64(project.next_plugin_id())?;
    for track in project.tracks() {
        let instrument = match track {
            Track::Midi(track) => track.plugin_instrument(),
            _ => None,
        };
        out.u8(u8::from(instrument.is_some()))?;
        if let Some(instrument) = instrument {
            out.plugin(instrument)?;
        }
        out.u64(track.effects().len() as u64)?;
        for effect in track.effects() {
            out.plugin(effect)?;
        }
        out.u64(track.mixer_effects().len() as u64)?;
        for effect in track.mixer_effects() {
            out.plugin(effect)?;
        }
    }
    let routes: Vec<_> = project
        .tracks()
        .iter()
        .flat_map(Track::all_effects)
        .filter_map(|effect| effect.sidechain.map(|route| (effect.id, route)))
        .collect();
    out.u64(routes.len() as u64)?;
    for (id, route) in routes {
        out.u64(id)?;
        out.u64(route.source_track as u64)?;
        out.u32(route.bus_index)?;
        out.f32(route.gain)?;
        out.u8(match route.tap {
            SidechainTap::PreEffects => 0,
            SidechainTap::PostEffects => 1,
            SidechainTap::PostFader => 2,
        })?;
    }
    for track in project.tracks() {
        out.u8(u8::from(track.mixer().soloed()))?;
    }
    for track in project.tracks() {
        if let Track::Midi(track) = track {
            out.u8(u8::from(track.instrument_bypassed()))?;
        }
    }
    for track in project.tracks() {
        out.u64(track.automation().len() as u64)?;
        for lane in track.automation() {
            match lane.target() {
                AutomationTarget::TrackGain => out.u8(0)?,
                AutomationTarget::TrackPan => out.u8(1)?,
                AutomationTarget::Subtractive(parameter) => {
                    out.u8(2)?;
                    out.u8(SubtractiveParameter::ALL
                        .iter()
                        .position(|p| *p == parameter)
                        .unwrap() as u8)?;
                }
                AutomationTarget::PluginParameter { plugin, parameter } => {
                    out.u8(3)?;
                    out.u64(plugin)?;
                    out.u32(parameter)?;
                }
            }
            out.u8(u8::from(lane.enabled()))?;
            out.u64(lane.points().len() as u64)?;
            for point in lane.points() {
                out.u64(point.tick)?;
                out.f64(point.value)?;
            }
        }
    }
    for track in project.tracks() {
        match track {
            Track::Audio(track) => {
                let input = track.recording_input();
                input.validate()?;
                out.recording_device(input.device.as_deref())?;
                match input.channels {
                    AudioInputChannels::Mono(channel) => {
                        out.u8(0)?;
                        out.u16(channel)?;
                    }
                    AudioInputChannels::Stereo(left, right) => {
                        out.u8(1)?;
                        out.u16(left)?;
                        out.u16(right)?;
                    }
                }
            }
            Track::Midi(track) => {
                let input = track.recording_input();
                input.validate()?;
                out.recording_device(input.device.as_deref())?;
                out.u8(u8::from(input.channel.is_some()))?;
                if let Some(channel) = input.channel {
                    out.u8(channel)?;
                }
            }
            Track::Group(_) | Track::Bus(_) | Track::Master(_) => {}
        }
    }
    for track in project.tracks() {
        out.u64(track.sends().len() as u64)?;
        for send in track.sends() {
            out.u64(send.destination.0 as u64)?;
            out.f32(send.gain)?;
        }
    }
    Ok(())
}

pub fn read_project(reader: impl Read) -> Result<Project, ProjectFileError> {
    read_project_with_base(reader, None)
}

fn read_project_with_base(
    mut reader: impl Read,
    base: Option<&Path>,
) -> Result<Project, ProjectFileError> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    let mut input = Decoder(bytes.as_slice());
    if input.take(MAGIC.len())? != MAGIC {
        return Err(ProjectFileError::InvalidFormat("invalid project signature"));
    }
    let version = input.u32()?;
    if !(1..=VERSION).contains(&version) {
        return Err(ProjectFileError::UnsupportedVersion(version));
    }
    if input.u32()? != SAMPLE_RATE || input.u64()? != TICKS_PER_QUARTER {
        return Err(ProjectFileError::InvalidFormat(
            "unsupported project timebase",
        ));
    }
    let mut project = Project::new();
    project.set_bpm(input.f64()?)?;
    project.set_time_signature(TimeSignature::new(input.u16()?, input.u16()?)?);
    let grid = match input.u8()? {
        0 => SnapGrid::Off,
        1 => SnapGrid::Bar,
        2 => SnapGrid::Quarter,
        3 => SnapGrid::Eighth,
        4 => SnapGrid::Sixteenth,
        _ => return Err(ProjectFileError::InvalidFormat("unknown snap grid")),
    };
    let references = if version >= 6 {
        match input.u8()? {
            0 => false,
            1 if base.is_some() => true,
            1 => {
                return Err(ProjectFileError::InvalidFormat(
                    "external samples require a project directory",
                ));
            }
            _ => {
                return Err(ProjectFileError::InvalidFormat(
                    "unknown asset storage mode",
                ));
            }
        }
    } else {
        false
    };
    let asset_count = input.count(8)?;
    let mut assets = Vec::with_capacity(asset_count);
    for _ in 0..asset_count {
        if references {
            let reference = input.string()?;
            let frames = input.u64()?;
            assets.push(project.add_asset(read_sample(base.unwrap(), &reference, Some(frames))?));
            continue;
        }
        let frame_count = input.count(8)?;
        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            frames.push([input.f32()?, input.f32()?]);
        }
        assets.push(project.add_asset(AudioBuffer::new(frames)?));
    }
    let track_count = input.count(18)?;
    if track_count == 0 {
        return Err(ProjectFileError::InvalidFormat(
            "project has no master track",
        ));
    }
    let mut mixers = Vec::with_capacity(track_count);
    for index in 0..track_count {
        let kind = input.u8()?;
        if (index == 0) != (kind == 3) {
            return Err(ProjectFileError::InvalidFormat(
                "master must occur exactly once at track zero",
            ));
        }
        let name = input.string()?;
        let gain = input.f32()?;
        let pan = input.f32()?;
        let output = match input.u8()? {
            0 => AudioOutput::Master,
            1 => AudioOutput::Group(GroupTrackId(input.index()?)),
            2 => AudioOutput::Output,
            3 => AudioOutput::Disconnected,
            4 if version >= 12 => AudioOutput::Bus(BusTrackId(input.index()?)),
            _ => return Err(ProjectFileError::InvalidFormat("unknown audio route")),
        };
        let mut mixer = MixerSettings::new(gain, pan, output)?;
        let color = if version >= 5 {
            match input.u8()? {
                0 => None,
                1 => Some(input.u32()?),
                _ => return Err(ProjectFileError::InvalidFormat("unknown colour flag")),
            }
        } else {
            None
        };
        if version >= 6 {
            mixer = mixer.with_muted(input.boolean()?);
        }
        let id: TrackId = match kind {
            0 => {
                let id = project.add_audio_track(name);
                let clip_count = input.count(if version >= 10 {
                    42
                } else if version >= 9 {
                    40
                } else {
                    36
                })?;
                for _ in 0..clip_count {
                    let asset = *assets
                        .get(input.index()?)
                        .ok_or(ProjectFileError::InvalidFormat("unknown audio asset"))?;
                    project.add_audio_clip(
                        id,
                        AudioClip {
                            asset,
                            source_start: input.u64()?,
                            start_tick: input.u64()?,
                            frame_count: input.u64()?,
                            gain: input.f32()?,
                            pitch_semitones: if version >= 9 { input.f32()? } else { 0.0 },
                            pitch_algorithm: if version >= 10 {
                                match input.u8()? {
                                    0 => PitchAlgorithm::Builtin,
                                    1 => PitchAlgorithm::Signalsmith,
                                    2 => PitchAlgorithm::RubberBandR3,
                                    _ => {
                                        return Err(ProjectFileError::InvalidFormat(
                                            "unknown pitch algorithm",
                                        ));
                                    }
                                }
                            } else {
                                PitchAlgorithm::Builtin
                            },
                            preserve_formants: if version >= 10 {
                                input.boolean()?
                            } else {
                                false
                            },
                        },
                    )?;
                }
                id.into()
            }
            1 => {
                let id = project.add_midi_track(name);
                if version >= 2 {
                    let instrument = match input.u8()? {
                        0 => None,
                        1 => {
                            let waveform = match input.u8()? {
                                0 => Waveform::Sine,
                                1 => Waveform::Saw,
                                2 => Waveform::Square,
                                _ => {
                                    return Err(ProjectFileError::InvalidFormat(
                                        "unknown waveform",
                                    ));
                                }
                            };
                            Some(Instrument::Subtractive(SubtractiveSettings {
                                waveform,
                                cutoff_hz: input.f32()?,
                                resonance: input.f32()?,
                                attack_seconds: input.f32()?,
                                decay_seconds: input.f32()?,
                                sustain: input.f32()?,
                                release_seconds: input.f32()?,
                                gain: input.f32()?,
                            }))
                        }
                        _ => return Err(ProjectFileError::InvalidFormat("unknown instrument")),
                    };
                    project.set_midi_track_instrument(id, instrument)?;
                }
                let clip_count = input.count(24)?;
                for _ in 0..clip_count {
                    let start_tick = input.u64()?;
                    let length_ticks = input.u64()?;
                    let offset_ticks = if version >= 5 { input.u64()? } else { 0 };
                    let note_count = input.count(19)?;
                    let mut notes = Vec::with_capacity(note_count);
                    for _ in 0..note_count {
                        notes.push(MidiNote {
                            start_tick: input.u64()?,
                            duration_ticks: input.u64()?,
                            key: input.u8()?,
                            velocity: input.u8()?,
                            channel: input.u8()?,
                        });
                    }
                    project.add_midi_clip(
                        id,
                        MidiClip {
                            start_tick,
                            length_ticks,
                            offset_ticks,
                            notes,
                        },
                    )?;
                }
                id.into()
            }
            2 => project.add_group_track(name).into(),
            4 if version >= 12 => project.add_bus_track(name).into(),
            3 => {
                if name != project.master_track().name() {
                    return Err(ProjectFileError::InvalidFormat("invalid master track name"));
                }
                project.master_track_id().into()
            }
            _ => return Err(ProjectFileError::InvalidFormat("unknown track kind")),
        };
        if color.is_some() {
            project.set_track_color(id, color)?;
        }
        mixers.push((id, mixer));
    }
    if version >= 3 {
        let next_plugin_id = input.u64()?;
        let track_ids = project.track_ids().collect::<Vec<_>>();
        for id in track_ids {
            if input.boolean()? {
                if let TrackId::Midi(midi) = id
                    && project.midi_track(midi)?.instrument().is_some()
                {
                    return Err(ProjectFileError::InvalidFormat(
                        "MIDI track has both a built-in and an external instrument",
                    ));
                }
                project.restore_plugin(id, input.plugin()?, true)?;
            }
            let effects = input.count(58)?;
            for _ in 0..effects {
                project.restore_plugin(id, input.plugin()?, false)?;
            }
            if version >= 12 {
                let inserts = input.count(58)?;
                for _ in 0..inserts {
                    project.restore_mixer_plugin(id, input.plugin()?)?;
                }
            }
        }
        project.restore_next_plugin_id(next_plugin_id)?;
    }
    let mut sidechains = Vec::new();
    if version >= 4 {
        let count = input.count(25)?;
        let mut assigned = HashSet::with_capacity(count);
        for _ in 0..count {
            let id = input.u64()?;
            if !assigned.insert(id) {
                return Err(ProjectFileError::InvalidFormat(
                    "duplicate sidechain assignment",
                ));
            }
            let source_track = input.index()?;
            let bus_index = input.u32()?;
            let gain = input.f32()?;
            let tap = match input.u8()? {
                0 => SidechainTap::PreEffects,
                1 => SidechainTap::PostEffects,
                2 => SidechainTap::PostFader,
                _ => return Err(ProjectFileError::InvalidFormat("unknown sidechain tap")),
            };
            sidechains.push((
                id,
                SidechainRoute {
                    source_track,
                    bus_index,
                    gain,
                    tap,
                },
            ));
        }
    }
    if version >= 7 {
        for (_, mixer) in &mut mixers {
            *mixer = mixer.with_soloed(input.boolean()?);
        }
        for (id, _) in &mixers {
            if let TrackId::Midi(id) = *id {
                let bypassed = input.boolean()?;
                if bypassed {
                    if project.midi_track(id)?.instrument().is_none() {
                        return Err(ProjectFileError::InvalidFormat(
                            "missing built-in instrument cannot be bypassed",
                        ));
                    }
                    project.set_midi_track_instrument_bypassed(id, true)?;
                }
            }
        }
    }
    if version >= 8 {
        for (id, _) in &mixers {
            let count = input.count(10)?;
            if count > MAX_AUTOMATION_LANES {
                return Err(ProjectError::TooManyAutomationLanes.into());
            }
            let mut targets = HashSet::with_capacity(count);
            for _ in 0..count {
                let target = match input.u8()? {
                    0 => AutomationTarget::TrackGain,
                    1 => AutomationTarget::TrackPan,
                    2 => AutomationTarget::Subtractive(
                        *SubtractiveParameter::ALL.get(input.u8()? as usize).ok_or(
                            ProjectFileError::InvalidFormat(
                                "unknown subtractive automation parameter",
                            ),
                        )?,
                    ),
                    3 => AutomationTarget::PluginParameter {
                        plugin: input.u64()?,
                        parameter: input.u32()?,
                    },
                    _ => return Err(ProjectFileError::InvalidFormat("unknown automation target")),
                };
                if !targets.insert(target) {
                    return Err(ProjectFileError::InvalidFormat(
                        "duplicate automation target",
                    ));
                }
                let enabled = input.boolean()?;
                let count = input.count(16)?;
                if count > MAX_AUTOMATION_POINTS {
                    return Err(ProjectError::TooManyAutomationPoints.into());
                }
                let mut points = Vec::with_capacity(count);
                for _ in 0..count {
                    points.push(AutomationPoint {
                        tick: input.u64()?,
                        value: input.f64()?,
                    });
                }
                project.set_track_automation(
                    *id,
                    AutomationLane::new(target, points)?.with_enabled(enabled),
                )?;
            }
        }
    }
    if version >= 11 {
        for (id, _) in &mixers {
            match *id {
                TrackId::Audio(id) => {
                    let device = input.recording_device()?;
                    let channels = match input.u8()? {
                        0 => AudioInputChannels::Mono(input.u16()?),
                        1 => AudioInputChannels::Stereo(input.u16()?, input.u16()?),
                        _ => {
                            return Err(ProjectFileError::InvalidFormat(
                                "unknown audio recording channel mode",
                            ));
                        }
                    };
                    project
                        .set_audio_recording_input(id, AudioRecordingInput { device, channels })?;
                }
                TrackId::Midi(id) => {
                    let device = input.recording_device()?;
                    let channel = if input.boolean()? {
                        Some(input.u8()?)
                    } else {
                        None
                    };
                    project.set_midi_recording_input(id, MidiRecordingInput { device, channel })?;
                }
                TrackId::Group(_) | TrackId::Bus(_) | TrackId::Master(_) => {}
            }
        }
    }
    let mut sends = Vec::new();
    if version >= 13 {
        for (id, _) in &mixers {
            let count = input.count(12)?;
            let mut destinations = HashSet::with_capacity(count);
            for _ in 0..count {
                let destination = BusTrackId(input.index()?);
                if !destinations.insert(destination) {
                    return Err(ProjectFileError::InvalidFormat(
                        "duplicate bus send destination",
                    ));
                }
                sends.push((*id, destination, input.f32()?));
            }
        }
    }
    if !input.0.is_empty() {
        return Err(ProjectFileError::InvalidFormat(
            "trailing bytes after project",
        ));
    }
    for (id, mixer) in mixers {
        match id {
            TrackId::Audio(id) => project.set_audio_track_mixer(id, mixer)?,
            TrackId::Midi(id) => project.set_midi_track_mixer(id, mixer)?,
            TrackId::Group(id) => project.set_group_track_mixer(id, mixer)?,
            TrackId::Bus(id) => project.set_bus_track_mixer(id, mixer)?,
            TrackId::Master(_) => project.set_master_track_mixer(mixer)?,
        }
    }
    for (id, destination, gain) in sends {
        project.set_track_send(id, destination, gain)?;
    }
    for (id, route) in sidechains {
        project.set_plugin_sidechain(id, Some(route))?;
    }
    project.set_snap_grid(grid);
    Ok(project)
}

struct Encoder<W>(W);

impl<W: Write> Encoder<W> {
    fn u8(&mut self, value: u8) -> io::Result<()> {
        self.0.write_all(&[value])
    }
    fn u16(&mut self, value: u16) -> io::Result<()> {
        self.0.write_all(&value.to_le_bytes())
    }
    fn u32(&mut self, value: u32) -> io::Result<()> {
        self.0.write_all(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> io::Result<()> {
        self.0.write_all(&value.to_le_bytes())
    }
    fn f32(&mut self, value: f32) -> io::Result<()> {
        self.u32(value.to_bits())
    }
    fn f64(&mut self, value: f64) -> io::Result<()> {
        self.u64(value.to_bits())
    }

    fn string(&mut self, value: &str) -> io::Result<()> {
        self.u64(value.len() as u64)?;
        self.0.write_all(value.as_bytes())
    }

    fn recording_device(&mut self, device: Option<&str>) -> io::Result<()> {
        self.u8(u8::from(device.is_some()))?;
        if let Some(device) = device {
            self.string(device)?;
        }
        Ok(())
    }

    fn plugin(&mut self, instance: &PluginInstance) -> Result<(), ProjectFileError> {
        instance.validate().map_err(ProjectError::from)?;
        self.u64(instance.id)?;
        self.string(
            instance
                .descriptor
                .path
                .to_str()
                .ok_or(ProjectFileError::InvalidFormat("plugin path is not UTF-8"))?,
        )?;
        self.string(&instance.descriptor.class_id)?;
        self.string(&instance.descriptor.name)?;
        self.string(&instance.descriptor.vendor)?;
        self.u8(match instance.descriptor.kind {
            PluginKind::Instrument => 0,
            PluginKind::Effect => 1,
        })?;
        self.u8(u8::from(instance.bypassed))?;
        self.u64(instance.parameters.len() as u64)?;
        for parameter in &instance.parameters {
            self.u32(parameter.id)?;
            self.string(&parameter.name)?;
            self.string(&parameter.unit)?;
            self.f64(parameter.value)?;
            self.u32(parameter.steps)?;
            self.u8(u8::from(parameter.read_only))?;
        }
        self.u64(instance.state.len() as u64)?;
        self.0.write_all(&instance.state)?;
        Ok(())
    }
}

struct Decoder<'a>(&'a [u8]);

impl<'a> Decoder<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], ProjectFileError> {
        let bytes = self
            .0
            .get(..count)
            .ok_or(ProjectFileError::InvalidFormat("truncated project"))?;
        self.0 = &self.0[count..];
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProjectFileError> {
        let mut bytes = [0; N];
        bytes.copy_from_slice(self.take(N)?);
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, ProjectFileError> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, ProjectFileError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, ProjectFileError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, ProjectFileError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn f32(&mut self) -> Result<f32, ProjectFileError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn f64(&mut self) -> Result<f64, ProjectFileError> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn index(&mut self) -> Result<usize, ProjectFileError> {
        usize::try_from(self.u64()?)
            .map_err(|_| ProjectFileError::InvalidFormat("index does not fit this platform"))
    }

    fn count(&mut self, item_bytes: usize) -> Result<usize, ProjectFileError> {
        let count = self.index()?;
        if count > self.0.len() / item_bytes {
            return Err(ProjectFileError::InvalidFormat(
                "count exceeds remaining project data",
            ));
        }
        Ok(count)
    }

    fn string(&mut self) -> Result<String, ProjectFileError> {
        let count = self.count(1)?;
        let bytes = self.take(count)?;
        let value = std::str::from_utf8(bytes)
            .map_err(|_| ProjectFileError::InvalidFormat("track name is not UTF-8"))?;
        Ok(value.to_owned())
    }

    fn boolean(&mut self) -> Result<bool, ProjectFileError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ProjectFileError::InvalidFormat("invalid boolean flag")),
        }
    }

    fn recording_device(&mut self) -> Result<Option<String>, ProjectFileError> {
        if !self.boolean()? {
            return Ok(None);
        }
        let count = self.count(1)?;
        if count > MAX_RECORDING_DEVICE_BYTES {
            return Err(ProjectError::InvalidRecordingDevice.into());
        }
        let key = std::str::from_utf8(self.take(count)?)
            .map_err(|_| ProjectFileError::InvalidFormat("recording device key is not UTF-8"))?;
        Ok(Some(key.to_owned()))
    }

    fn plugin_string(&mut self) -> Result<String, ProjectFileError> {
        let count = self.count(1)?;
        if count > MAX_PLUGIN_STRING_BYTES {
            return Err(ProjectFileError::InvalidFormat(
                "plugin string exceeds size limit",
            ));
        }
        let text = std::str::from_utf8(self.take(count)?)
            .map_err(|_| ProjectFileError::InvalidFormat("plugin string is not UTF-8"))?;
        Ok(text.to_owned())
    }

    fn plugin(&mut self) -> Result<PluginInstance, ProjectFileError> {
        let id = self.u64()?;
        let path = PathBuf::from(self.plugin_string()?);
        let class_id = self.plugin_string()?;
        let name = self.plugin_string()?;
        let vendor = self.plugin_string()?;
        let kind = match self.u8()? {
            0 => PluginKind::Instrument,
            1 => PluginKind::Effect,
            _ => return Err(ProjectFileError::InvalidFormat("unknown plugin kind")),
        };
        let bypassed = self.boolean()?;
        let parameter_count = self.count(33)?;
        if parameter_count > MAX_PLUGIN_PARAMETERS {
            return Err(ProjectFileError::InvalidFormat(
                "plugin has too many parameters",
            ));
        }
        let mut parameters = Vec::with_capacity(parameter_count);
        for _ in 0..parameter_count {
            parameters.push(PluginParameter {
                id: self.u32()?,
                name: self.plugin_string()?,
                unit: self.plugin_string()?,
                value: self.f64()?,
                steps: self.u32()?,
                read_only: self.boolean()?,
            });
        }
        let state_bytes = self.count(1)?;
        if state_bytes > MAX_PLUGIN_STATE_BYTES {
            return Err(ProjectFileError::InvalidFormat(
                "plugin state exceeds size limit",
            ));
        }
        let instance = PluginInstance {
            id,
            descriptor: PluginDescriptor {
                path,
                class_id,
                name,
                vendor,
                kind,
            },
            parameters,
            state: self.take(state_bytes)?.to_vec(),
            bypassed,
            sidechain: None,
        };
        instance.validate().map_err(ProjectError::from)?;
        Ok(instance)
    }
}

#[derive(Debug)]
pub enum ProjectFileError {
    Io(io::Error),
    UnsupportedVersion(u32),
    InvalidFormat(&'static str),
    Invalid(String),
    Project(ProjectError),
}

impl fmt::Display for ProjectFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "project I/O error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported project file version {version}")
            }
            Self::InvalidFormat(message) => write!(f, "invalid project file: {message}"),
            Self::Invalid(message) => write!(f, "invalid project: {message}"),
            Self::Project(error) => write!(f, "invalid project content: {error}"),
        }
    }
}

impl Error for ProjectFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Project(error) => Some(error),
            Self::UnsupportedVersion(_) | Self::InvalidFormat(_) | Self::Invalid(_) => None,
        }
    }
}

impl From<io::Error> for ProjectFileError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ProjectError> for ProjectFileError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

impl From<TimeError> for ProjectFileError {
    fn from(error: TimeError) -> Self {
        Self::Project(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory() -> PathBuf {
        loop {
            let path = std::env::temp_dir().join(format!(
                "sympoie-failed-manifest-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return path,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("{error}"),
            }
        }
    }

    #[test]
    fn failed_manifest_write_keeps_previous_file_and_cleans_temporary() {
        let directory = test_directory();
        let destination = directory.join(PROJECT_MANIFEST);
        save_project(&Project::new(), &destination).unwrap();
        let previous = fs::read(&destination).unwrap();
        let result = atomic_write(&destination, |writer| {
            writer.write_all(b"partial replacement manifest")?;
            writer.flush()?;
            Err(io::Error::other("simulated disk write failure").into())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), previous);
        assert_eq!(load_project(&destination).unwrap().tracks().len(), 1);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    fn no_hard_links(_source: &Path, _destination: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem has no hard links",
        ))
    }

    #[test]
    fn sample_publication_falls_back_to_copy_without_replacing_existing_assets() {
        let directory = test_directory();
        let source = directory.join("prepared.tmp");
        let destination = directory.join("sample.wav");
        let asset = AudioBuffer::new(vec![[1.2345679, -0.0], [f32::from_bits(1), -2.0]]).unwrap();
        let mut writer = crate::wav::FloatWavWriter::new(Vec::new(), asset.len() as u64).unwrap();
        writer.write_frames(asset.samples()).unwrap();
        let wav = writer.finish().unwrap();
        fs::write(&source, &wav).unwrap();
        publish_sample(&source, &destination, no_hard_links).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), wav);
        assert!(sample_matches(&asset, &destination).unwrap());
        assert!(!sample_is_pending(&destination).unwrap());
        fs::write(&source, b"a different asset").unwrap();
        let error = publish_sample(&source, &destination, no_hard_links).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&destination).unwrap(), wav);
        assert!(!sample_is_pending(&destination).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_fallback_copy_removes_only_the_file_it_created() {
        let directory = test_directory();
        let original = directory.join("original.wav");
        fs::write(&original, b"previous saved asset").unwrap();
        let destination = directory.join("sample.wav");
        assert!(
            publish_sample(
                &directory.join("missing-source.tmp"),
                &destination,
                no_hard_links
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert!(!sample_is_pending(&destination).unwrap());
        assert_eq!(fs::read(&original).unwrap(), b"previous saved asset");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_sample_is_not_reused_even_after_all_its_bytes_have_been_copied() {
        let directory = test_directory();
        let destination = directory.join("sample.wav");
        let asset = AudioBuffer::new(vec![[0.25, 0.5]]).unwrap();
        let mut writer = crate::wav::FloatWavWriter::new(Vec::new(), 1).unwrap();
        writer.write_frames(asset.samples()).unwrap();
        fs::write(&destination, writer.finish().unwrap()).unwrap();
        fs::write(pending_sample_path(&destination), []).unwrap();
        assert!(!sample_matches(&asset, &destination).unwrap());
        assert_eq!(
            publish_sample(&directory.join("source.tmp"), &destination, no_hard_links)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(sample_is_pending(&destination).unwrap());
        fs::remove_file(pending_sample_path(&destination)).unwrap();
        assert!(sample_matches(&asset, &destination).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }
}
