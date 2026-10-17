use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use crate::automation::{AutomationLane, AutomationPoint, AutomationTarget, SubtractiveParameter};
use crate::builtin::{self, BuiltinKind, ParameterSpec};
use crate::clip::{AudioClip, MidiClip, MidiNote};
use crate::instrument::{Instrument, SubtractiveSettings, Waveform};
use crate::json::{self, Value};
use crate::pitch::PitchAlgorithm;
use crate::plugin::{
    PluginDescriptor, PluginInstance, PluginKind, PluginParameter, SidechainRoute, SidechainTap,
};
use crate::project::{AssetId, AudioBuffer, Project};
use crate::project_file::{
    ProjectFileError, atomic_write, read_sample, save_sample, sync_directory,
};
use crate::time::{SnapGrid, TimeSignature};
use crate::track::{
    AudioInputChannels, AudioOutput, AudioRecordingInput, BusTrackId, GroupTrackId,
    MidiRecordingInput, MixerSettings, Track, TrackId,
};
use crate::{SAMPLE_RATE, TICKS_PER_QUARTER};

pub const MANIFEST: &str = "project.json";
pub const FORMAT: &str = "sympoie-project";
pub const VERSION: u64 = 1;

pub fn is_project_path(path: &Path) -> bool {
    let named = |name: &str| path.file_name().is_some_and(|file| file == name);
    if path.is_dir() {
        return path.join(MANIFEST).is_file()
            || path.join(crate::project_file::PROJECT_MANIFEST).is_file();
    }
    named(MANIFEST)
        || path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("symp"))
}

pub fn project_location(path: &Path) -> PathBuf {
    let manifest = path.file_name().is_some_and(|name| {
        name == MANIFEST
            || name == crate::project_file::PROJECT_MANIFEST
                && path
                    .parent()
                    .is_some_and(|parent| parent.join("samples").is_dir())
    });
    match path.parent() {
        Some(parent) if manifest && !parent.as_os_str().is_empty() => parent.to_owned(),
        Some(_) if manifest => PathBuf::from("."),
        _ => path.to_owned(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveReport {
    pub written: Vec<String>,
    pub removed: Vec<String>,
}

pub fn save_project_folder(
    project: &Project,
    folder: impl AsRef<Path>,
) -> Result<SaveReport, ProjectFileError> {
    project.processing_order()?;
    let folder = folder.as_ref();
    fs::create_dir_all(folder)?;
    let samples = real_directory(folder, "samples")?;
    let mut report = SaveReport::default();

    let mut sample_paths = Vec::with_capacity(project.assets().len());
    for asset in project.assets() {
        sample_paths.push(saved_sample(asset, &samples)?);
    }
    let mut state_files = StateFiles {
        folder,
        directory: None,
    };
    let mut track_paths = Vec::with_capacity(project.tracks().len());
    let mut files = Vec::with_capacity(project.tracks().len());
    for track in project.tracks() {
        let value = write_track(project, track, &sample_paths, &mut state_files)?;
        let path = track_file(track);
        track_paths.push(path.clone());
        files.push((path, value.to_pretty()));
    }
    sync_directory(&samples)?;
    if let Some(directory) = &state_files.directory {
        sync_directory(directory)?;
    }
    let tracks = real_directory(folder, "tracks")?;
    for (path, text) in &files {
        if write_if_changed(&folder.join(path), text)? {
            report.written.push(path.clone());
        }
    }
    sync_directory(&tracks)?;

    let manifest = Value::object()
        .with("format", FORMAT)
        .with("version", VERSION)
        .with("sample_rate", SAMPLE_RATE)
        .with("ticks_per_quarter", TICKS_PER_QUARTER)
        .with("tempo", project.bpm())
        .with(
            "time_signature",
            vec![
                project.time_signature().numerator(),
                project.time_signature().denominator(),
            ],
        )
        .with("snap", snap_name(project.snap_grid()))
        .with("next_id", project.next_uid())
        .with("next_device_id", project.next_plugin_id())
        .with("tracks", track_paths.clone());
    if write_if_changed(&folder.join(MANIFEST), &manifest.to_pretty())? {
        report.written.push(MANIFEST.to_owned());
    }

    let current: HashSet<_> = track_paths.iter().map(String::as_str).collect();
    for entry in fs::read_dir(&tracks)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = format!("tracks/{name}");
        if is_track_file_name(name) && !current.contains(path.as_str()) {
            match fs::remove_file(tracks.join(name)) {
                Ok(()) => report.removed.push(path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    let legacy = folder.join(crate::project_file::PROJECT_MANIFEST);
    if legacy.is_file() {
        fs::remove_file(&legacy)?;
        report
            .removed
            .push(crate::project_file::PROJECT_MANIFEST.to_owned());
    }
    sync_directory(folder)?;
    Ok(report)
}

fn real_directory(folder: &Path, name: &str) -> Result<PathBuf, ProjectFileError> {
    let path = folder.join(name);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProjectFileError::Invalid(format!(
            "{name} must be a real folder"
        )));
    }
    Ok(path)
}

fn write_if_changed(path: &Path, text: &str) -> Result<bool, ProjectFileError> {
    match fs::read(path) {
        Ok(existing) if existing == text.as_bytes() => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    atomic_write(path, |writer| Ok(writer.write_all(text.as_bytes())?))?;
    Ok(true)
}

pub fn track_file(track: &Track) -> String {
    format!("tracks/{}-{}.json", track.uid(), slug(track))
}

fn slug(track: &Track) -> String {
    let mut slug = String::new();
    for character in track.name().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 32 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        type_name(track).to_owned()
    } else {
        slug.to_owned()
    }
}

fn is_track_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    let Some((id, rest)) = stem.split_once('-') else {
        return false;
    };
    !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn type_name(track: &Track) -> &'static str {
    match track {
        Track::Audio(_) => "audio",
        Track::Midi(_) => "midi",
        Track::Group(_) => "group",
        Track::Bus(_) => "bus",
        Track::Master(_) => "master",
    }
}

fn snap_name(grid: SnapGrid) -> &'static str {
    match grid {
        SnapGrid::Off => "off",
        SnapGrid::Bar => "bar",
        SnapGrid::Quarter => "1/4",
        SnapGrid::Eighth => "1/8",
        SnapGrid::Sixteenth => "1/16",
    }
}

fn tap_name(tap: SidechainTap) -> &'static str {
    match tap {
        SidechainTap::PreEffects => "pre-effects",
        SidechainTap::PostEffects => "post-effects",
        SidechainTap::PostFader => "post-fader",
    }
}

fn algorithm_name(algorithm: PitchAlgorithm) -> &'static str {
    match algorithm {
        PitchAlgorithm::Builtin => "builtin",
        PitchAlgorithm::Signalsmith => "signalsmith",
        PitchAlgorithm::RubberBandR3 => "rubberband",
    }
}

fn waveform_name(waveform: Waveform) -> &'static str {
    match waveform {
        Waveform::Sine => "sine",
        Waveform::Saw => "saw",
        Waveform::Square => "square",
    }
}

fn subtractive_name(parameter: SubtractiveParameter) -> &'static str {
    match parameter {
        SubtractiveParameter::Waveform => "waveform",
        SubtractiveParameter::Cutoff => "cutoff",
        SubtractiveParameter::Resonance => "resonance",
        SubtractiveParameter::Attack => "attack",
        SubtractiveParameter::Decay => "decay",
        SubtractiveParameter::Sustain => "sustain",
        SubtractiveParameter::Release => "release",
        SubtractiveParameter::Gain => "gain",
    }
}

fn builtin_name(kind: BuiltinKind) -> String {
    kind.name().to_ascii_lowercase()
}

struct StateFiles<'a> {
    folder: &'a Path,
    directory: Option<PathBuf>,
}

impl StateFiles<'_> {
    fn save(&mut self, state: &[u8]) -> Result<String, ProjectFileError> {
        if self.directory.is_none() {
            self.directory = Some(real_directory(self.folder, "state")?);
        }
        let directory = self.directory.as_ref().unwrap();
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for &byte in state {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        for suffix in 0_u64.. {
            let name = if suffix == 0 {
                format!("{hash:016x}-{}.bin", state.len())
            } else {
                format!("{hash:016x}-{}-{suffix}.bin", state.len())
            };
            let path = directory.join(&name);
            if known_file(&path) {
                return Ok(format!("state/{name}"));
            }
            match fs::read(&path) {
                Ok(existing) if existing == state => {
                    remember_file(&path);
                    return Ok(format!("state/{name}"));
                }
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    atomic_write(&path, |writer| Ok(writer.write_all(state)?))?;
                    remember_file(&path);
                    return Ok(format!("state/{name}"));
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("state file suffix space exhausted")
    }
}

#[derive(Default)]
struct SaveCache {
    files: HashMap<PathBuf, (u64, Option<std::time::SystemTime>)>,
    samples: Vec<(PathBuf, Weak<AudioBuffer>, String)>,
}

static SAVE_CACHE: Mutex<Option<SaveCache>> = Mutex::new(None);

fn signature(path: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && !metadata.file_type().is_symlink())
        .then(|| (metadata.len(), metadata.modified().ok()))
}

fn with_cache<T>(use_cache: impl FnOnce(&mut SaveCache) -> T) -> T {
    let mut cache = SAVE_CACHE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    use_cache(cache.get_or_insert_with(SaveCache::default))
}

fn known_file(path: &Path) -> bool {
    let current = signature(path);
    current.is_some() && with_cache(|cache| cache.files.get(path) == current.as_ref())
}

fn remember_file(path: &Path) {
    if let Some(current) = signature(path) {
        with_cache(|cache| {
            if cache.files.len() > 65_536 {
                cache.files.clear();
            }
            cache.files.insert(path.to_owned(), current);
        });
    }
}

fn saved_sample(asset: &Arc<AudioBuffer>, samples: &Path) -> Result<String, ProjectFileError> {
    let cached = with_cache(|cache| {
        cache
            .samples
            .iter()
            .find(|(directory, buffer, _)| {
                directory == samples
                    && buffer
                        .upgrade()
                        .is_some_and(|buffer| Arc::ptr_eq(&buffer, asset))
            })
            .map(|(_, _, reference)| reference.clone())
    });
    if let Some(reference) = cached
        && known_file(&samples.join(reference.trim_start_matches("samples/")))
    {
        return Ok(reference);
    }
    let reference = save_sample(asset, samples)?;
    remember_file(&samples.join(reference.trim_start_matches("samples/")));
    with_cache(|cache| {
        cache
            .samples
            .retain(|(_, buffer, _)| buffer.strong_count() > 0);
        cache
            .samples
            .push((samples.to_owned(), Arc::downgrade(asset), reference.clone()));
    });
    Ok(reference)
}

fn write_track(
    project: &Project,
    track: &Track,
    sample_paths: &[String],
    states: &mut StateFiles<'_>,
) -> Result<Value, ProjectFileError> {
    let uid_of = |index: usize| project.tracks()[index].uid();
    let mut value = Value::object()
        .with("id", track.uid())
        .with("type", type_name(track))
        .with("name", track.name());
    if let Some(color) = track.color() {
        value.push("color", format!("#{color:06x}"));
    }
    let mixer = track.mixer();
    let output: Value = match mixer.output() {
        AudioOutput::Master => "master".into(),
        AudioOutput::Output => "output".into(),
        AudioOutput::Disconnected => "none".into(),
        AudioOutput::Group(group) => Value::object().with("group", uid_of(group.0)),
        AudioOutput::Bus(bus) => Value::object().with("bus", uid_of(bus.0)),
    };
    value.push(
        "mixer",
        Value::object()
            .with("gain", mixer.gain())
            .with("pan", mixer.pan())
            .with("mute", mixer.muted())
            .with("solo", mixer.soloed())
            .with("output", output),
    );
    if !track.sends().is_empty() {
        value.push(
            "sends",
            track
                .sends()
                .iter()
                .map(|send| {
                    Value::object()
                        .with("bus", uid_of(send.destination.0))
                        .with("gain", send.gain)
                })
                .collect::<Vec<_>>(),
        );
    }
    match track {
        Track::Audio(audio) => {
            let input = audio.recording_input();
            if *input != AudioRecordingInput::default() {
                let channels = match input.channels {
                    AudioInputChannels::Mono(channel) => vec![channel],
                    AudioInputChannels::Stereo(left, right) => vec![left, right],
                };
                value.push(
                    "input",
                    Value::object()
                        .with("device", input.device.clone())
                        .with("channels", channels),
                );
            }
        }
        Track::Midi(midi) => {
            let input = midi.recording_input();
            if *input != MidiRecordingInput::default() {
                value.push(
                    "input",
                    Value::object()
                        .with("device", input.device.clone())
                        .with("channel", input.channel),
                );
            }
            if let Some(Instrument::Subtractive(settings)) = midi.instrument() {
                let mut instrument = Value::object()
                    .with("type", "subtractive")
                    .with("waveform", waveform_name(settings.waveform))
                    .with("cutoff_hz", settings.cutoff_hz)
                    .with("resonance", settings.resonance)
                    .with("attack_seconds", settings.attack_seconds)
                    .with("decay_seconds", settings.decay_seconds)
                    .with("sustain", settings.sustain)
                    .with("release_seconds", settings.release_seconds)
                    .with("gain", settings.gain);
                if midi.instrument_bypassed() {
                    instrument.push("bypassed", true);
                }
                value.push("instrument", instrument);
            } else if let Some(instrument) = midi.plugin_instrument() {
                value.push("instrument", write_device(project, instrument, states)?);
            }
        }
        Track::Group(_) | Track::Bus(_) | Track::Master(_) => {}
    }
    let devices = track
        .effects()
        .iter()
        .map(|device| write_device(project, device, states))
        .collect::<Result<Vec<_>, _>>()?;
    value.push("devices", devices);
    if !track.mixer_effects().is_empty() {
        let devices = track
            .mixer_effects()
            .iter()
            .map(|device| write_device(project, device, states))
            .collect::<Result<Vec<_>, _>>()?;
        value.push("mixer_devices", devices);
    }
    if !track.automation().is_empty() {
        let lanes: Vec<Value> = track
            .automation()
            .iter()
            .map(|lane| {
                let target: Value = match lane.target() {
                    AutomationTarget::TrackGain => "gain".into(),
                    AutomationTarget::TrackPan => "pan".into(),
                    AutomationTarget::Subtractive(parameter) => {
                        Value::object().with("subtractive", subtractive_name(parameter))
                    }
                    AutomationTarget::PluginParameter { plugin, parameter } => Value::object()
                        .with("device", plugin)
                        .with("parameter", parameter),
                };
                let mut entry = Value::object().with("target", target);
                if !lane.enabled() {
                    entry.push("enabled", false);
                }
                entry.with(
                    "points",
                    lane.points()
                        .iter()
                        .map(|point| Value::Array(vec![point.tick.into(), point.value.into()]))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        value.push("automation", lanes);
    }
    match track {
        Track::Audio(audio) => {
            let clips: Vec<Value> = audio
                .clips()
                .iter()
                .zip(audio.clip_uids())
                .map(|(clip, &uid)| {
                    let mut entry = Value::object()
                        .with("id", uid)
                        .with("sample", sample_paths[clip.asset.index()].clone())
                        .with("start_tick", clip.start_tick)
                        .with("source_start_frame", clip.source_start)
                        .with("frame_count", clip.frame_count)
                        .with("gain", clip.gain);
                    if clip.pitch_semitones != 0.0 {
                        entry.push("pitch", clip.pitch_semitones);
                    }
                    if clip.pitch_algorithm != PitchAlgorithm::Builtin {
                        entry.push("pitch_algorithm", algorithm_name(clip.pitch_algorithm));
                    }
                    if clip.preserve_formants {
                        entry.push("preserve_formants", true);
                    }
                    entry
                })
                .collect();
            value.push("clips", clips);
        }
        Track::Midi(midi) => {
            let clips: Vec<Value> = midi
                .clips()
                .iter()
                .zip(midi.clip_uids())
                .map(|(clip, &uid)| {
                    let mut entry = Value::object()
                        .with("id", uid)
                        .with("start_tick", clip.start_tick)
                        .with("length_ticks", clip.length_ticks);
                    if clip.offset_ticks != 0 {
                        entry.push("offset_ticks", clip.offset_ticks);
                    }
                    let notes: Vec<Value> = clip
                        .notes
                        .iter()
                        .map(|note| {
                            let mut entry = Value::object()
                                .with("start", note.start_tick)
                                .with("length", note.duration_ticks)
                                .with("key", note.key)
                                .with("velocity", note.velocity);
                            if note.channel != 0 {
                                entry.push("channel", note.channel);
                            }
                            entry
                        })
                        .collect();
                    entry.with("notes", notes)
                })
                .collect();
            value.push("clips", clips);
        }
        Track::Group(_) | Track::Bus(_) | Track::Master(_) => {}
    }
    Ok(value)
}

fn write_device(
    project: &Project,
    device: &PluginInstance,
    states: &mut StateFiles<'_>,
) -> Result<Value, ProjectFileError> {
    device
        .validate()
        .map_err(crate::project::ProjectError::from)?;
    let builtin = BuiltinKind::from_descriptor(&device.descriptor);
    let mut value = Value::object().with("id", device.id);
    match builtin {
        Some(kind) => value.push("builtin", builtin_name(kind)),
        None => {
            let descriptor = &device.descriptor;
            value.push(
                "plugin",
                Value::object()
                    .with("name", descriptor.name.as_str())
                    .with("vendor", descriptor.vendor.as_str())
                    .with(
                        "path",
                        descriptor.path.to_str().ok_or_else(|| {
                            ProjectFileError::Invalid("a plug-in path is not UTF-8".into())
                        })?,
                    )
                    .with("class_id", descriptor.class_id.as_str()),
            );
        }
    }
    if device.bypassed {
        value.push("bypassed", true);
    }
    let specs = builtin.map(builtin::parameters).unwrap_or_default();
    let parameters: Vec<Value> = device
        .parameters
        .iter()
        .map(|parameter| {
            let mut entry = Value::object()
                .with("id", parameter.id)
                .with("name", parameter.name.as_str());
            let spec = specs.iter().find(|spec| spec.id == parameter.id);
            match spec.and_then(|spec| exact_physical(*spec, parameter.value)) {
                Some(physical) => {
                    entry.push("value", physical);
                    if !parameter.unit.is_empty() {
                        entry.push("unit", parameter.unit.as_str());
                    }
                }
                None => {
                    entry.push("normalized", parameter.value);
                    if !parameter.unit.is_empty() {
                        entry.push("unit", parameter.unit.as_str());
                    }
                    if spec.is_none() {
                        if parameter.steps != 0 {
                            entry.push("steps", parameter.steps);
                        }
                        if parameter.read_only {
                            entry.push("read_only", true);
                        }
                    }
                }
            }
            entry
        })
        .collect();
    value.push("parameters", parameters);
    if !device.state.is_empty() {
        value.push("state", states.save(&device.state)?);
    }
    if let Some(route) = device.sidechain {
        let source = project
            .tracks()
            .get(route.source_track)
            .map(Track::uid)
            .ok_or_else(|| ProjectFileError::Invalid("a sidechain source is missing".into()))?;
        value.push(
            "sidechain",
            Value::object()
                .with("source", source)
                .with("input", route.bus_index)
                .with("gain", route.gain)
                .with("tap", tap_name(route.tap)),
        );
    }
    Ok(value)
}

fn exact_physical(spec: ParameterSpec, normalized: f64) -> Option<f64> {
    let physical = spec.denormalize(normalized);
    if spec.normalize(physical) == normalized {
        return Some(physical);
    }
    let mut down = physical;
    let mut up = physical;
    for _ in 0..64 {
        down = next_toward(down, f64::NEG_INFINITY);
        up = next_toward(up, f64::INFINITY);
        for candidate in [down, up] {
            if spec.normalize(candidate) == normalized {
                return Some(candidate);
            }
        }
    }
    None
}

fn next_toward(value: f64, direction: f64) -> f64 {
    if value == 0.0 {
        return if direction > 0.0 {
            f64::from_bits(1)
        } else {
            -f64::from_bits(1)
        };
    }
    let bits = value.to_bits();
    let away_from_zero = (direction > value) == (value > 0.0);
    f64::from_bits(if away_from_zero { bits + 1 } else { bits - 1 })
}

pub fn load_project_folder(folder: impl AsRef<Path>) -> Result<Project, ProjectFileError> {
    let folder = folder.as_ref();
    let manifest_text = read_text(folder, MANIFEST)?;
    let manifest = parse(MANIFEST, &manifest_text)?;
    let manifest = Node::root(MANIFEST, &manifest)?;
    manifest.only(&[
        "format",
        "version",
        "sample_rate",
        "ticks_per_quarter",
        "tempo",
        "time_signature",
        "snap",
        "next_id",
        "next_device_id",
        "tracks",
    ])?;
    if manifest.string("format")? != FORMAT {
        return Err(manifest
            .field("format")
            .error(format!("must be \"{FORMAT}\"")));
    }
    let version = manifest.u64("version")?;
    if version == 0 || version > VERSION {
        return Err(manifest.field("version").error(format!(
            "version {version} is not supported; this build reads versions 1–{VERSION}"
        )));
    }
    if let Some(rate) = manifest.optional("sample_rate")
        && rate.as_u64()? != u64::from(SAMPLE_RATE)
    {
        return Err(rate.error(format!("must be {SAMPLE_RATE}")));
    }
    if let Some(ticks) = manifest.optional("ticks_per_quarter")
        && ticks.as_u64()? != TICKS_PER_QUARTER
    {
        return Err(ticks.error(format!("must be {TICKS_PER_QUARTER}")));
    }

    let mut project = Project::new();
    let tempo = manifest.field("tempo");
    project
        .set_bpm(tempo.as_f64()?)
        .map_err(|error| tempo.error(error))?;
    let meter = manifest.field("time_signature");
    let parts = meter.items()?;
    if parts.len() != 2 {
        return Err(meter.error("must be [beats per bar, beat unit], such as [4, 4]"));
    }
    let signature = TimeSignature::new(parts[0].as_u16()?, parts[1].as_u16()?)
        .map_err(|error| meter.error(error))?;
    project.set_time_signature(signature);
    let grid = match manifest.optional("snap") {
        None => SnapGrid::Off,
        Some(snap) => match snap.as_str()? {
            "off" => SnapGrid::Off,
            "bar" => SnapGrid::Bar,
            "1/4" => SnapGrid::Quarter,
            "1/8" => SnapGrid::Eighth,
            "1/16" => SnapGrid::Sixteenth,
            _ => return Err(snap.error("must be \"off\", \"bar\", \"1/4\", \"1/8\" or \"1/16\"")),
        },
    };

    let list = manifest.field("tracks");
    let paths = list.items()?;
    if paths.is_empty() {
        return Err(list.error("must list the master track first"));
    }
    let mut texts = Vec::with_capacity(paths.len());
    for path in &paths {
        let name = path.as_str()?;
        if !valid_relative(name, "tracks/", ".json") {
            return Err(path.error(
                "must be a file in tracks/ named with letters, digits, '-' or '_', ending in .json",
            ));
        }
        if texts.iter().any(|(existing, _)| existing == name) {
            return Err(path.error("lists the same track file twice"));
        }
        texts.push((name.to_owned(), read_text(folder, name)?));
    }
    let mut documents = Vec::with_capacity(texts.len());
    for (name, text) in &texts {
        documents.push(parse(name, text)?);
    }
    let tracks: Vec<Node<'_>> = texts
        .iter()
        .zip(&documents)
        .map(|((name, _), document)| Node::root(name, document))
        .collect::<Result<_, _>>()?;

    let mut uids = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        if let Some(id) = track.optional("id") {
            let uid = id.as_u64()?;
            if uid == 0 {
                return Err(id.error("must be at least 1, or left out to assign one"));
            }
            if uids.insert(uid, index).is_some() {
                return Err(id.error(format!("track ID {uid} is used by another track")));
            }
        }
    }
    let mut next_device = manifest
        .optional("next_device_id")
        .map(|node| node.as_u64())
        .transpose()?
        .unwrap_or(1)
        .max(1);
    let mut device_ids = HashSet::new();
    for track in &tracks {
        for device in track.all_devices()? {
            if let Some(id) = device.optional("id") {
                let id = id.as_u64()?;
                if id == 0 || !device_ids.insert(id) {
                    return Err(device
                        .field("id")
                        .error(format!("device ID {id} is used by another device")));
                }
                next_device = next_device.max(id.saturating_add(1));
            }
        }
    }
    let mut loader = Loader {
        folder,
        project: &mut project,
        uids: &uids,
        samples: HashMap::new(),
        next_device,
        mixers: Vec::new(),
        sends: Vec::new(),
        sidechains: Vec::new(),
        ids: Vec::new(),
    };
    for (index, track) in tracks.iter().enumerate() {
        loader.track(index, track)?;
    }
    let Loader {
        next_device,
        mixers,
        sends,
        sidechains,
        ids,
        ..
    } = loader;
    project.restore_next_plugin_id(next_device.max(project.next_plugin_id()))?;

    for (node, id, mixer) in mixers {
        let result = match id {
            TrackId::Audio(id) => project.set_audio_track_mixer(id, mixer),
            TrackId::Midi(id) => project.set_midi_track_mixer(id, mixer),
            TrackId::Group(id) => project.set_group_track_mixer(id, mixer),
            TrackId::Bus(id) => project.set_bus_track_mixer(id, mixer),
            TrackId::Master(_) => project.set_master_track_mixer(mixer),
        };
        result.map_err(|error| node.error(error))?;
    }
    for (node, id, destination, gain) in sends {
        project
            .set_track_send(id, destination, gain)
            .map_err(|error| node.error(error))?;
    }
    for (node, device, route) in sidechains {
        project
            .set_plugin_sidechain(device, Some(route))
            .map_err(|error| node.error(error))?;
    }
    for (index, uid, clips) in ids {
        let restored = project.restore_uids(index, uid, &clips);
        debug_assert!(restored, "every track and clip was added");
    }
    let next = manifest
        .optional("next_id")
        .map(|next| next.as_u64())
        .transpose()?
        .unwrap_or(1);
    project.reset_next_uid(next);
    project.assign_uids();
    project.set_snap_grid(grid);
    Ok(project)
}

fn read_text(folder: &Path, name: &str) -> Result<String, ProjectFileError> {
    let path = folder.join(name);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| io::Error::new(error.kind(), format!("cannot read {name}: {error}")))?;
    if !metadata.is_file() {
        return Err(ProjectFileError::Invalid(format!("{name} is not a file")));
    }
    let bytes = fs::read(&path)
        .map_err(|error| io::Error::new(error.kind(), format!("cannot read {name}: {error}")))?;
    String::from_utf8(bytes)
        .map_err(|_| ProjectFileError::Invalid(format!("{name} is not valid UTF-8 text")))
}

fn parse(name: &str, text: &str) -> Result<Value, ProjectFileError> {
    json::parse(text).map_err(|error| ProjectFileError::Invalid(format!("{name}: {error}")))
}

fn valid_relative(path: &str, prefix: &str, suffix: &str) -> bool {
    path.strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(suffix))
        .is_some_and(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

type Pending<'a, T> = Vec<(Node<'a>, TrackId, T)>;

struct Loader<'a, 'p> {
    folder: &'a Path,
    project: &'p mut Project,
    uids: &'a HashMap<u64, usize>,
    samples: HashMap<String, AssetId>,
    next_device: u64,
    mixers: Pending<'a, MixerSettings>,
    sends: Vec<(Node<'a>, TrackId, BusTrackId, f32)>,
    sidechains: Vec<(Node<'a>, u64, SidechainRoute)>,
    ids: Vec<(usize, u64, Vec<u64>)>,
}

impl<'a> Loader<'a, '_> {
    fn track_ref(&self, node: &Node<'_>) -> Result<usize, ProjectFileError> {
        let uid = node.as_u64()?;
        self.uids
            .get(&uid)
            .copied()
            .ok_or_else(|| node.error(format!("no track has ID {uid}")))
    }

    fn track(&mut self, index: usize, node: &Node<'a>) -> Result<(), ProjectFileError> {
        let kind = node.field("type");
        let kind_name = kind.as_str()?;
        if (index == 0) != (kind_name == "master") {
            return Err(kind.error("the first track must be the master, and only the first"));
        }
        let mut allowed = vec![
            "id",
            "type",
            "name",
            "color",
            "mixer",
            "sends",
            "devices",
            "mixer_devices",
            "automation",
        ];
        match kind_name {
            "audio" => allowed.extend(["input", "clips"]),
            "midi" => allowed.extend(["input", "instrument", "clips"]),
            "group" | "bus" | "master" => {}
            _ => {
                return Err(
                    kind.error("must be \"audio\", \"midi\", \"group\", \"bus\" or \"master\"")
                );
            }
        }
        node.only(&allowed)?;
        let name = node
            .optional("name")
            .map(|name| name.as_str())
            .transpose()?;
        let id: TrackId = match kind_name {
            "audio" => self.project.add_audio_track(name.unwrap_or("Audio")).into(),
            "midi" => self.project.add_midi_track(name.unwrap_or("MIDI")).into(),
            "group" => self.project.add_group_track(name.unwrap_or("Group")).into(),
            "bus" => self.project.add_bus_track(name.unwrap_or("Bus")).into(),
            _ => {
                if let Some(name) = name
                    && name != self.project.master_track().name()
                {
                    return Err(node.field("name").error("the master's name is \"Master\""));
                }
                self.project.master_track_id().into()
            }
        };
        if let Some(color) = node.optional("color") {
            let text = color.as_str()?;
            let parsed = text
                .strip_prefix('#')
                .filter(|hex| hex.len() == 6)
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .ok_or_else(|| color.error("must be a colour like \"#e0a040\""))?;
            self.project
                .set_track_color(id, Some(parsed))
                .map_err(|error| color.error(error))?;
        }
        self.mixer(node, id)?;
        if let Some(sends) = node.optional("sends") {
            for send in sends.items()? {
                send.only(&["bus", "gain"])?;
                let destination = self.track_ref(&send.field("bus"))?;
                self.sends.push((
                    send.clone(),
                    id,
                    BusTrackId(destination),
                    send.f32_or("gain", 1.0)?,
                ));
            }
        }
        if let Some(input) = node.optional("input") {
            self.input(&input, id)?;
        }
        if let Some(instrument) = node.optional("instrument") {
            self.instrument(&instrument, id)?;
        }
        for (key, mixer_chain) in [("devices", false), ("mixer_devices", true)] {
            if let Some(devices) = node.optional(key) {
                for device in devices.items()? {
                    let instance = self.device(&device, PluginKind::Effect)?;
                    let result = if mixer_chain {
                        self.project.restore_mixer_plugin(id, instance)
                    } else {
                        self.project.restore_plugin(id, instance, false)
                    };
                    result.map_err(|error| device.error(error))?;
                }
            }
        }
        if let Some(lanes) = node.optional("automation") {
            for lane in lanes.items()? {
                self.lane(&lane, id)?;
            }
        }
        let mut clip_uids = Vec::new();
        if let Some(clips) = node.optional("clips") {
            for clip in clips.items()? {
                clip_uids.push(self.clip(&clip, id)?);
            }
        }
        let uid = node
            .optional("id")
            .map(|id| id.as_u64())
            .transpose()?
            .unwrap_or(0);
        self.ids.push((index, uid, clip_uids));
        Ok(())
    }

    fn mixer(&mut self, node: &Node<'a>, id: TrackId) -> Result<(), ProjectFileError> {
        let defaults = *self.project.tracks()[id_index(id)].mixer();
        let Some(mixer) = node.optional("mixer") else {
            return Ok(());
        };
        mixer.only(&["gain", "pan", "mute", "solo", "output"])?;
        let output = match mixer.optional("output") {
            None => defaults.output(),
            Some(output) => match &output.value {
                Value::String(text) => match text.as_str() {
                    "master" => AudioOutput::Master,
                    "output" => AudioOutput::Output,
                    "none" => AudioOutput::Disconnected,
                    _ => {
                        return Err(output.error(
                            "must be \"master\", \"output\", \"none\", {\"group\": ID} or {\"bus\": ID}",
                        ));
                    }
                },
                Value::Object(_) => {
                    output.only(&["group", "bus"])?;
                    if let Some(group) = output.optional("group") {
                        AudioOutput::Group(GroupTrackId(self.track_ref(&group)?))
                    } else if let Some(bus) = output.optional("bus") {
                        AudioOutput::Bus(BusTrackId(self.track_ref(&bus)?))
                    } else {
                        return Err(output.error("must name a \"group\" or \"bus\" track ID"));
                    }
                }
                _ => return Err(output.error("must be a string or an object")),
            },
        };
        let settings = MixerSettings::new(
            mixer.f32_or("gain", defaults.gain())?,
            mixer.f32_or("pan", defaults.pan())?,
            output,
        )
        .map_err(|error| mixer.error(error))?
        .with_muted(mixer.bool_or("mute", false)?)
        .with_soloed(mixer.bool_or("solo", false)?);
        self.mixers.push((mixer, id, settings));
        Ok(())
    }

    fn input(&mut self, node: &Node<'a>, id: TrackId) -> Result<(), ProjectFileError> {
        let device = match node.optional("device") {
            None => None,
            Some(device) if matches!(device.value, Value::Null) => None,
            Some(device) => Some(device.as_str()?.to_owned()),
        };
        let result = match id {
            TrackId::Audio(id) => {
                node.only(&["device", "channels"])?;
                let channels = match node.optional("channels") {
                    None => AudioInputChannels::default(),
                    Some(list) => {
                        let items = list.items()?;
                        match items.as_slice() {
                            [mono] => AudioInputChannels::Mono(mono.as_u16()?),
                            [left, right] => {
                                AudioInputChannels::Stereo(left.as_u16()?, right.as_u16()?)
                            }
                            _ => return Err(list.error("must hold one or two channel numbers")),
                        }
                    }
                };
                self.project
                    .set_audio_recording_input(id, AudioRecordingInput { device, channels })
            }
            TrackId::Midi(id) => {
                node.only(&["device", "channel"])?;
                let channel = match node.optional("channel") {
                    None => None,
                    Some(channel) if matches!(channel.value, Value::Null) => None,
                    Some(channel) => Some(channel.as_u8()?),
                };
                self.project
                    .set_midi_recording_input(id, MidiRecordingInput { device, channel })
            }
            _ => return Err(node.error("only audio and MIDI tracks have inputs")),
        };
        result.map_err(|error| node.error(error))
    }

    fn instrument(&mut self, node: &Node<'a>, id: TrackId) -> Result<(), ProjectFileError> {
        let TrackId::Midi(midi) = id else {
            return Err(node.error("only MIDI tracks have instruments"));
        };
        if node.optional("type").is_some() {
            node.only(&[
                "type",
                "waveform",
                "cutoff_hz",
                "resonance",
                "attack_seconds",
                "decay_seconds",
                "sustain",
                "release_seconds",
                "gain",
                "bypassed",
            ])?;
            let kind = node.field("type");
            if kind.as_str()? != "subtractive" {
                return Err(kind.error("must be \"subtractive\"; use \"builtin\" for devices"));
            }
            let defaults = SubtractiveSettings::default();
            let waveform = match node.optional("waveform") {
                None => defaults.waveform,
                Some(waveform) => match waveform.as_str()? {
                    "sine" => Waveform::Sine,
                    "saw" => Waveform::Saw,
                    "square" => Waveform::Square,
                    _ => return Err(waveform.error("must be \"sine\", \"saw\" or \"square\"")),
                },
            };
            let settings = SubtractiveSettings {
                waveform,
                cutoff_hz: node.f32_or("cutoff_hz", defaults.cutoff_hz)?,
                resonance: node.f32_or("resonance", defaults.resonance)?,
                attack_seconds: node.f32_or("attack_seconds", defaults.attack_seconds)?,
                decay_seconds: node.f32_or("decay_seconds", defaults.decay_seconds)?,
                sustain: node.f32_or("sustain", defaults.sustain)?,
                release_seconds: node.f32_or("release_seconds", defaults.release_seconds)?,
                gain: node.f32_or("gain", defaults.gain)?,
            };
            self.project
                .set_midi_track_instrument(midi, Some(Instrument::Subtractive(settings)))
                .map_err(|error| node.error(error))?;
            if node.bool_or("bypassed", false)? {
                self.project
                    .set_midi_track_instrument_bypassed(midi, true)
                    .map_err(|error| node.error(error))?;
            }
            return Ok(());
        }
        let instance = self.device(node, PluginKind::Instrument)?;
        self.project
            .restore_plugin(id, instance, true)
            .map_err(|error| node.error(error))
    }

    fn device(
        &mut self,
        node: &Node<'a>,
        kind: PluginKind,
    ) -> Result<PluginInstance, ProjectFileError> {
        node.only(&[
            "id",
            "builtin",
            "plugin",
            "bypassed",
            "parameters",
            "state",
            "sidechain",
        ])?;
        let id = match node.optional("id") {
            Some(id) => id.as_u64()?,
            None => {
                let id = self.next_device;
                self.next_device += 1;
                id
            }
        };
        let (descriptor, specs) = match (node.optional("builtin"), node.optional("plugin")) {
            (Some(builtin), None) => {
                let name = builtin.as_str()?;
                let builtin_kind = BuiltinKind::ALL
                    .into_iter()
                    .find(|candidate| builtin_name(*candidate) == name)
                    .ok_or_else(|| {
                        builtin.error("must be \"oscil\", \"sampler\", \"eq\" or \"comp\"")
                    })?;
                let descriptor = builtin_kind.descriptor();
                if descriptor.kind != kind {
                    return Err(builtin.error(match kind {
                        PluginKind::Instrument => "is an effect, not an instrument",
                        PluginKind::Effect => "is an instrument; put it in \"instrument\"",
                    }));
                }
                (descriptor, builtin::parameters(builtin_kind))
            }
            (None, Some(plugin)) => {
                plugin.only(&["name", "vendor", "path", "class_id"])?;
                (
                    PluginDescriptor {
                        path: PathBuf::from(plugin.string("path")?),
                        class_id: plugin.string("class_id")?.to_owned(),
                        name: plugin.string("name")?.to_owned(),
                        vendor: plugin
                            .optional("vendor")
                            .map(|vendor| vendor.as_str())
                            .transpose()?
                            .unwrap_or("")
                            .to_owned(),
                        kind,
                    },
                    Vec::new(),
                )
            }
            _ => return Err(node.error("needs either \"builtin\" or \"plugin\", not both")),
        };
        let mut parameters = Vec::new();
        if let Some(list) = node.optional("parameters") {
            for entry in list.items()? {
                entry.only(&[
                    "id",
                    "name",
                    "value",
                    "normalized",
                    "unit",
                    "steps",
                    "read_only",
                ])?;
                let spec = match entry.optional("id") {
                    Some(id) => {
                        let id = id.as_u32()?;
                        specs.iter().find(|spec| spec.id == id).copied()
                    }
                    None => {
                        let name = entry.string("name")?;
                        Some(
                            specs
                                .iter()
                                .find(|spec| spec.name.eq_ignore_ascii_case(name))
                                .copied()
                                .ok_or_else(|| {
                                    entry.error(format!("this device has no parameter \"{name}\""))
                                })?,
                        )
                    }
                };
                let value = match (entry.optional("value"), entry.optional("normalized")) {
                    (Some(value), None) => {
                        let spec = spec.ok_or_else(|| {
                            value.error(
                                "only built-in parameters take \"value\"; use \"normalized\" (0–1)",
                            )
                        })?;
                        let physical = value.as_f64()?;
                        if !(spec.min..=spec.max).contains(&physical) {
                            return Err(value.error(format!(
                                "must be between {} and {}{}",
                                spec.min,
                                spec.max,
                                if spec.unit.is_empty() {
                                    String::new()
                                } else {
                                    format!(" {}", spec.unit)
                                }
                            )));
                        }
                        spec.normalize(physical)
                    }
                    (None, Some(normalized)) => {
                        let value = normalized.as_f64()?;
                        if !(0.0..=1.0).contains(&value) {
                            return Err(normalized.error("must be between 0 and 1"));
                        }
                        value
                    }
                    _ => {
                        return Err(
                            entry.error("needs either \"value\" or \"normalized\", not both")
                        );
                    }
                };
                parameters.push(match spec {
                    Some(spec) => PluginParameter {
                        id: spec.id,
                        name: entry
                            .optional("name")
                            .map(|name| name.as_str())
                            .transpose()?
                            .unwrap_or(spec.name)
                            .to_owned(),
                        unit: spec.unit.to_owned(),
                        value,
                        steps: spec.steps,
                        read_only: false,
                    },
                    None => PluginParameter {
                        id: entry.u32("id")?,
                        name: entry.string("name")?.to_owned(),
                        unit: entry
                            .optional("unit")
                            .map(|unit| unit.as_str())
                            .transpose()?
                            .unwrap_or("")
                            .to_owned(),
                        value,
                        steps: entry
                            .optional("steps")
                            .map(|steps| steps.as_u32())
                            .transpose()?
                            .unwrap_or(0),
                        read_only: entry.bool_or("read_only", false)?,
                    },
                });
            }
        }
        let state = match node.optional("state") {
            None => Vec::new(),
            Some(state) => {
                let path = state.as_str()?;
                if !valid_relative(path, "state/", ".bin") {
                    return Err(state.error("must be a file in state/ ending in .bin"));
                }
                let full = self.folder.join(path);
                if fs::symlink_metadata(&full).is_ok_and(|meta| meta.file_type().is_symlink()) {
                    return Err(state.error("cannot be a symbolic link"));
                }
                fs::read(&full).map_err(|error| state.error(format!("cannot read: {error}")))?
            }
        };
        let mut instance = PluginInstance::new(descriptor, parameters, state);
        instance.id = id;
        instance.bypassed = node.bool_or("bypassed", false)?;
        instance
            .validate()
            .map_err(|error| node.error(crate::project::ProjectError::from(error)))?;
        if let Some(sidechain) = node.optional("sidechain") {
            sidechain.only(&["source", "input", "gain", "tap"])?;
            let source = sidechain.field("source");
            let tap = match sidechain.optional("tap") {
                None => SidechainTap::PostFader,
                Some(tap) => match tap.as_str()? {
                    "pre-effects" => SidechainTap::PreEffects,
                    "post-effects" => SidechainTap::PostEffects,
                    "post-fader" => SidechainTap::PostFader,
                    _ => {
                        return Err(tap
                            .error("must be \"pre-effects\", \"post-effects\" or \"post-fader\""));
                    }
                },
            };
            let route = SidechainRoute {
                source_track: self.track_ref(&source)?,
                bus_index: sidechain
                    .optional("input")
                    .map(|input| input.as_u32())
                    .transpose()?
                    .unwrap_or(1),
                gain: sidechain.f32_or("gain", 1.0)?,
                tap,
            };
            self.sidechains.push((sidechain, id, route));
        }
        Ok(instance)
    }

    fn lane(&mut self, node: &Node<'a>, id: TrackId) -> Result<(), ProjectFileError> {
        node.only(&["target", "enabled", "points"])?;
        let target_node = node.field("target");
        let target = match &target_node.value {
            Value::String(name) => match name.as_str() {
                "gain" => AutomationTarget::TrackGain,
                "pan" => AutomationTarget::TrackPan,
                _ => {
                    return Err(target_node.error(
                        "must be \"gain\", \"pan\", {\"device\": ID, \"parameter\": ID} or {\"subtractive\": NAME}",
                    ));
                }
            },
            Value::Object(_) => {
                target_node.only(&["device", "parameter", "subtractive"])?;
                if let Some(parameter) = target_node.optional("subtractive") {
                    let name = parameter.as_str()?;
                    AutomationTarget::Subtractive(
                        SubtractiveParameter::ALL
                            .into_iter()
                            .find(|candidate| subtractive_name(*candidate) == name)
                            .ok_or_else(|| parameter.error("is not a subtractive parameter"))?,
                    )
                } else {
                    AutomationTarget::PluginParameter {
                        plugin: target_node.u64("device")?,
                        parameter: target_node.u32("parameter")?,
                    }
                }
            }
            _ => return Err(target_node.error("must be a string or an object")),
        };
        let mut points = Vec::new();
        for point in node.field("points").items()? {
            let pair = point.items()?;
            let [tick, value] = pair.as_slice() else {
                return Err(point.error("must be [tick, value]"));
            };
            points.push(AutomationPoint {
                tick: tick.as_u64()?,
                value: value.as_f64()?,
            });
        }
        let lane = AutomationLane::new(target, points)
            .map_err(|error| node.field("points").error(error))?
            .with_enabled(node.bool_or("enabled", true)?);
        self.project
            .set_track_automation(id, lane)
            .map_err(|error| node.error(error))
    }

    fn clip(&mut self, node: &Node<'a>, id: TrackId) -> Result<u64, ProjectFileError> {
        let uid = node
            .optional("id")
            .map(|id| id.as_u64())
            .transpose()?
            .filter(|uid| !self.uids.contains_key(uid))
            .unwrap_or(0);
        match id {
            TrackId::Audio(track) => {
                node.only(&[
                    "id",
                    "sample",
                    "start_tick",
                    "source_start_frame",
                    "frame_count",
                    "gain",
                    "pitch",
                    "pitch_algorithm",
                    "preserve_formants",
                ])?;
                let sample = node.field("sample");
                let reference = sample.as_str()?;
                let asset = match self.samples.get(reference) {
                    Some(asset) => *asset,
                    None => {
                        if !valid_relative(reference, "samples/", ".wav") {
                            return Err(sample.error(
                                "must be a .wav file in samples/ named with letters, digits, '-' or '_'",
                            ));
                        }
                        let audio = read_sample(self.folder, reference, None)
                            .map_err(|error| sample.error(error))?;
                        let asset = self.project.add_asset(audio);
                        self.samples.insert(reference.to_owned(), asset);
                        asset
                    }
                };
                let length = self.project.audio_asset(asset)?.len() as u64;
                let source_start = node.u64_or("source_start_frame", 0)?;
                let clip = AudioClip {
                    asset,
                    source_start,
                    start_tick: node.u64("start_tick")?,
                    frame_count: node.u64_or("frame_count", length.saturating_sub(source_start))?,
                    gain: node.f32_or("gain", 1.0)?,
                    pitch_semitones: node.f32_or("pitch", 0.0)?,
                    pitch_algorithm: match node.optional("pitch_algorithm") {
                        None => PitchAlgorithm::Builtin,
                        Some(algorithm) => match algorithm.as_str()? {
                            "builtin" => PitchAlgorithm::Builtin,
                            "signalsmith" => PitchAlgorithm::Signalsmith,
                            "rubberband" => PitchAlgorithm::RubberBandR3,
                            _ => {
                                return Err(algorithm.error(
                                    "must be \"builtin\", \"signalsmith\" or \"rubberband\"",
                                ));
                            }
                        },
                    },
                    preserve_formants: node.bool_or("preserve_formants", false)?,
                };
                self.add_without_snapping(node, |project| project.add_audio_clip(track, clip))?;
            }
            TrackId::Midi(track) => {
                node.only(&["id", "start_tick", "length_ticks", "offset_ticks", "notes"])?;
                let mut notes = Vec::new();
                if let Some(list) = node.optional("notes") {
                    for note in list.items()? {
                        note.only(&["start", "length", "key", "velocity", "channel"])?;
                        let key = note.field("key");
                        let key_value = key.as_u8()?;
                        if key_value > 127 {
                            return Err(key.error("must be at most 127"));
                        }
                        let velocity = note.u8_or("velocity", 100)?;
                        if !(1..=127).contains(&velocity) {
                            return Err(note.field("velocity").error("must be 1–127"));
                        }
                        let channel = note.u8_or("channel", 0)?;
                        if channel > 15 {
                            return Err(note.field("channel").error("must be 0–15"));
                        }
                        let length = note.u64("length")?;
                        if length == 0 {
                            return Err(note.field("length").error("must be at least 1 tick"));
                        }
                        notes.push(MidiNote {
                            start_tick: note.u64("start")?,
                            duration_ticks: length,
                            key: key_value,
                            velocity,
                            channel,
                        });
                    }
                }
                let clip = MidiClip {
                    start_tick: node.u64("start_tick")?,
                    length_ticks: node.u64("length_ticks")?,
                    offset_ticks: node.u64_or("offset_ticks", 0)?,
                    notes,
                };
                self.add_without_snapping(node, |project| project.add_midi_clip(track, clip))?;
            }
            _ => return Err(node.error("only audio and MIDI tracks have clips")),
        }
        Ok(uid)
    }

    fn add_without_snapping(
        &mut self,
        node: &Node<'a>,
        add: impl FnOnce(&mut Project) -> Result<(), crate::project::ProjectError>,
    ) -> Result<(), ProjectFileError> {
        let grid = self.project.snap_grid();
        self.project.set_snap_grid(SnapGrid::Off);
        let result = add(self.project);
        self.project.set_snap_grid(grid);
        result.map_err(|error| node.error(error))
    }
}

fn id_index(id: TrackId) -> usize {
    match id {
        TrackId::Audio(id) => id.0,
        TrackId::Midi(id) => id.0,
        TrackId::Group(id) => id.0,
        TrackId::Bus(id) => id.0,
        TrackId::Master(id) => id.0,
    }
}

#[derive(Clone)]
struct Node<'a> {
    file: &'a str,
    path: String,
    value: &'a Value,
}

impl<'a> Node<'a> {
    fn root(file: &'a str, value: &'a Value) -> Result<Self, ProjectFileError> {
        let node = Self {
            file,
            path: String::new(),
            value,
        };
        if !matches!(value, Value::Object(_)) {
            return Err(node.error("must be a JSON object"));
        }
        Ok(node)
    }

    fn error(&self, message: impl std::fmt::Display) -> ProjectFileError {
        if self.path.is_empty() {
            ProjectFileError::Invalid(format!("{}: {message}", self.file))
        } else {
            ProjectFileError::Invalid(format!("{}: {}: {message}", self.file, self.path))
        }
    }

    fn child(&self, key: &str, value: &'a Value) -> Self {
        Self {
            file: self.file,
            path: if self.path.is_empty() {
                key.to_owned()
            } else {
                format!("{}.{key}", self.path)
            },
            value,
        }
    }

    fn only(&self, allowed: &[&str]) -> Result<(), ProjectFileError> {
        let Value::Object(entries) = self.value else {
            return Err(self.error(format!("must be an object, not {}", self.value.kind())));
        };
        for (key, _) in entries {
            if !allowed.contains(&key.as_str()) {
                return Err(self.error(format!(
                    "unknown field \"{key}\"; expected one of: {}",
                    allowed.join(", ")
                )));
            }
        }
        Ok(())
    }

    fn optional(&self, key: &str) -> Option<Self> {
        self.value.get(key).map(|value| self.child(key, value))
    }

    fn field(&self, key: &str) -> Self {
        static MISSING: Value = Value::Null;
        match self.value.get(key) {
            Some(value) => self.child(key, value),
            None => Self {
                file: self.file,
                path: format!(
                    "{}{}{key} (missing)",
                    self.path,
                    if self.path.is_empty() { "" } else { "." }
                ),
                value: &MISSING,
            },
        }
    }

    fn items(&self) -> Result<Vec<Self>, ProjectFileError> {
        match self.value {
            Value::Array(items) => Ok(items
                .iter()
                .enumerate()
                .map(|(index, value)| Self {
                    file: self.file,
                    path: format!("{}[{index}]", self.path),
                    value,
                })
                .collect()),
            other => Err(self.error(format!(
                "must be an array, not {}",
                kind_or_missing(other, self)
            ))),
        }
    }

    fn all_devices(&self) -> Result<Vec<Self>, ProjectFileError> {
        let mut devices = Vec::new();
        for key in ["devices", "mixer_devices"] {
            if let Some(list) = self.optional(key) {
                devices.extend(list.items()?);
            }
        }
        if let Some(instrument) = self.optional("instrument")
            && instrument.value.get("type").is_none()
        {
            devices.push(instrument);
        }
        Ok(devices)
    }

    fn number_text(&self) -> Result<&'a str, ProjectFileError> {
        match self.value {
            Value::Number(text) => Ok(text),
            other => Err(self.error(format!(
                "must be a number, not {}",
                kind_or_missing(other, self)
            ))),
        }
    }

    fn as_u64(&self) -> Result<u64, ProjectFileError> {
        let text = self.number_text()?;
        if let Ok(value) = text.parse::<u64>() {
            return Ok(value);
        }
        match text.parse::<f64>() {
            Ok(value)
                if value >= 0.0 && value.fract() == 0.0 && value < 9.007_199_254_740_992e15 =>
            {
                Ok(value as u64)
            }
            _ => Err(self.error("must be a whole number of at least 0")),
        }
    }

    fn as_u32(&self) -> Result<u32, ProjectFileError> {
        u32::try_from(self.as_u64()?).map_err(|_| self.error("is too large"))
    }

    fn as_u16(&self) -> Result<u16, ProjectFileError> {
        u16::try_from(self.as_u64()?).map_err(|_| self.error("is too large"))
    }

    fn as_u8(&self) -> Result<u8, ProjectFileError> {
        u8::try_from(self.as_u64()?).map_err(|_| self.error("must be at most 255"))
    }

    fn as_f64(&self) -> Result<f64, ProjectFileError> {
        self.number_text()?
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| self.error("must be a finite number"))
    }

    fn as_f32(&self) -> Result<f32, ProjectFileError> {
        self.number_text()?
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| self.error("must be a finite number"))
    }

    fn as_str(&self) -> Result<&'a str, ProjectFileError> {
        match self.value {
            Value::String(text) => Ok(text),
            other => Err(self.error(format!(
                "must be a string, not {}",
                kind_or_missing(other, self)
            ))),
        }
    }

    fn as_bool(&self) -> Result<bool, ProjectFileError> {
        match self.value {
            Value::Bool(value) => Ok(*value),
            other => Err(self.error(format!(
                "must be true or false, not {}",
                kind_or_missing(other, self)
            ))),
        }
    }

    fn string(&self, key: &str) -> Result<&'a str, ProjectFileError> {
        self.field(key).as_str()
    }

    fn u64(&self, key: &str) -> Result<u64, ProjectFileError> {
        self.field(key).as_u64()
    }

    fn u32(&self, key: &str) -> Result<u32, ProjectFileError> {
        self.field(key).as_u32()
    }

    fn u64_or(&self, key: &str, default: u64) -> Result<u64, ProjectFileError> {
        self.optional(key).map_or(Ok(default), |node| node.as_u64())
    }

    fn u8_or(&self, key: &str, default: u8) -> Result<u8, ProjectFileError> {
        self.optional(key).map_or(Ok(default), |node| node.as_u8())
    }

    fn f32_or(&self, key: &str, default: f32) -> Result<f32, ProjectFileError> {
        self.optional(key).map_or(Ok(default), |node| node.as_f32())
    }

    fn bool_or(&self, key: &str, default: bool) -> Result<bool, ProjectFileError> {
        self.optional(key)
            .map_or(Ok(default), |node| node.as_bool())
    }
}

fn kind_or_missing(value: &Value, node: &Node<'_>) -> &'static str {
    if node.path.ends_with("(missing)") {
        "missing"
    } else {
        value.kind()
    }
}
