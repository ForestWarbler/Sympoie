use crate::automation::AutomationLane;
use crate::clip::{AudioClip, MidiClip};
use crate::instrument::Instrument;
use crate::plugin::PluginInstance;
use crate::project::ProjectError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioTrackId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MidiTrackId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupTrackId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BusTrackId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MasterTrackId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackId {
    Audio(AudioTrackId),
    Midi(MidiTrackId),
    Group(GroupTrackId),
    Bus(BusTrackId),
    Master(MasterTrackId),
}

impl TrackId {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Audio(id) => id.0,
            Self::Midi(id) => id.0,
            Self::Group(id) => id.0,
            Self::Bus(id) => id.0,
            Self::Master(id) => id.0,
        }
    }
}

impl From<AudioTrackId> for TrackId {
    fn from(id: AudioTrackId) -> Self {
        Self::Audio(id)
    }
}

impl From<MidiTrackId> for TrackId {
    fn from(id: MidiTrackId) -> Self {
        Self::Midi(id)
    }
}

impl From<GroupTrackId> for TrackId {
    fn from(id: GroupTrackId) -> Self {
        Self::Group(id)
    }
}

impl From<BusTrackId> for TrackId {
    fn from(id: BusTrackId) -> Self {
        Self::Bus(id)
    }
}

impl From<MasterTrackId> for TrackId {
    fn from(id: MasterTrackId) -> Self {
        Self::Master(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOutput {
    Master,
    Group(GroupTrackId),
    Bus(BusTrackId),
    Output,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSend {
    pub destination: BusTrackId,
    pub gain: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixerSettings {
    gain: f32,
    pan: f32,
    output: AudioOutput,
    muted: bool,
    soloed: bool,
}

impl MixerSettings {
    pub fn new(gain: f32, pan: f32, output: AudioOutput) -> Result<Self, ProjectError> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(ProjectError::InvalidTrackGain);
        }
        if !(-1.0..=1.0).contains(&pan) {
            return Err(ProjectError::InvalidTrackPan);
        }
        Ok(Self {
            gain,
            pan,
            output,
            muted: false,
            soloed: false,
        })
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn pan(&self) -> f32 {
        self.pan
    }

    pub fn output(&self) -> AudioOutput {
        self.output
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn with_muted(self, muted: bool) -> Self {
        Self { muted, ..self }
    }

    pub fn soloed(&self) -> bool {
        self.soloed
    }

    pub fn with_soloed(self, soloed: bool) -> Self {
        Self { soloed, ..self }
    }

    pub(crate) fn effective_gain(&self) -> f32 {
        if self.muted { 0.0 } else { self.gain }
    }

    pub fn with_output(self, output: AudioOutput) -> Self {
        Self { output, ..self }
    }
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            output: AudioOutput::Master,
            muted: false,
            soloed: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioInputChannels {
    Mono(u16),
    Stereo(u16, u16),
}

impl Default for AudioInputChannels {
    fn default() -> Self {
        Self::Mono(0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioRecordingInput {
    pub device: Option<String>,
    pub channels: AudioInputChannels,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiRecordingInput {
    pub device: Option<String>,
    pub channel: Option<u8>,
}

pub(crate) const MAX_RECORDING_DEVICE_BYTES: usize = 4096;

fn validate_recording_device(device: Option<&str>) -> Result<(), ProjectError> {
    if device.is_some_and(|key| {
        key.is_empty() || key.len() > MAX_RECORDING_DEVICE_BYTES || key.contains('\0')
    }) {
        return Err(ProjectError::InvalidRecordingDevice);
    }
    Ok(())
}

impl AudioRecordingInput {
    pub(crate) fn validate(&self) -> Result<(), ProjectError> {
        validate_recording_device(self.device.as_deref())?;
        let valid = match self.channels {
            AudioInputChannels::Mono(channel) => channel < u16::MAX,
            AudioInputChannels::Stereo(left, right) => {
                left < u16::MAX && right < u16::MAX && left != right
            }
        };
        if !valid {
            return Err(ProjectError::InvalidAudioInputChannels);
        }
        Ok(())
    }
}

impl MidiRecordingInput {
    pub(crate) fn validate(&self) -> Result<(), ProjectError> {
        validate_recording_device(self.device.as_deref())?;
        if self.channel.is_some_and(|channel| channel > 15) {
            return Err(ProjectError::InvalidMidiInputChannel);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AudioTrack {
    uid: u64,
    name: String,
    color: Option<u32>,
    mixer: MixerSettings,
    sends: Vec<TrackSend>,
    recording_input: AudioRecordingInput,
    effects: Vec<PluginInstance>,
    mixer_effects: Vec<PluginInstance>,
    automation: Vec<AutomationLane>,
    clips: Vec<AudioClip>,
    clip_uids: Vec<u64>,
}

impl AudioTrack {
    pub(crate) fn new(name: String) -> Self {
        Self {
            uid: 0,
            name,
            color: None,
            mixer: MixerSettings::default(),
            sends: Vec::new(),
            recording_input: AudioRecordingInput::default(),
            effects: Vec::new(),
            mixer_effects: Vec::new(),
            automation: Vec::new(),
            clips: Vec::new(),
            clip_uids: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn color(&self) -> Option<u32> {
        self.color
    }

    pub(crate) fn set_color(&mut self, color: Option<u32>) {
        self.color = color;
    }

    pub fn mixer(&self) -> &MixerSettings {
        &self.mixer
    }

    pub fn sends(&self) -> &[TrackSend] {
        &self.sends
    }

    pub fn recording_input(&self) -> &AudioRecordingInput {
        &self.recording_input
    }

    pub(crate) fn set_recording_input(&mut self, input: AudioRecordingInput) {
        self.recording_input = input;
    }

    pub fn effects(&self) -> &[PluginInstance] {
        &self.effects
    }

    pub fn mixer_effects(&self) -> &[PluginInstance] {
        &self.mixer_effects
    }

    pub fn automation(&self) -> &[AutomationLane] {
        &self.automation
    }

    pub fn clips(&self) -> &[AudioClip] {
        &self.clips
    }

    pub fn clip_uids(&self) -> &[u64] {
        &self.clip_uids
    }

    pub(crate) fn push_clip(&mut self, clip: AudioClip) {
        self.push_clip_with_uid(clip, 0);
    }

    pub(crate) fn push_clip_with_uid(&mut self, clip: AudioClip, uid: u64) {
        self.clips.push(clip);
        self.clip_uids.push(uid);
    }

    pub(crate) fn replace_clip(&mut self, index: usize, clip: AudioClip) {
        self.clips[index] = clip;
    }

    pub(crate) fn remove_clip(&mut self, index: usize) -> AudioClip {
        self.clip_uids.remove(index);
        self.clips.remove(index)
    }

    pub(crate) fn set_name(&mut self, name: String) {
        self.name = name;
    }

    pub(crate) fn set_mixer(&mut self, mixer: MixerSettings) {
        self.mixer = mixer;
    }
}

#[derive(Debug, Clone)]
pub struct MidiTrack {
    uid: u64,
    name: String,
    color: Option<u32>,
    mixer: MixerSettings,
    sends: Vec<TrackSend>,
    recording_input: MidiRecordingInput,
    effects: Vec<PluginInstance>,
    mixer_effects: Vec<PluginInstance>,
    automation: Vec<AutomationLane>,
    instrument: Option<Instrument>,
    instrument_bypassed: bool,
    plugin_instrument: Option<Box<PluginInstance>>,
    clips: Vec<MidiClip>,
    clip_uids: Vec<u64>,
}

impl MidiTrack {
    pub(crate) fn new(name: String) -> Self {
        Self {
            uid: 0,
            name,
            color: None,
            mixer: MixerSettings::default(),
            sends: Vec::new(),
            recording_input: MidiRecordingInput::default(),
            effects: Vec::new(),
            mixer_effects: Vec::new(),
            automation: Vec::new(),
            instrument: None,
            instrument_bypassed: false,
            plugin_instrument: None,
            clips: Vec::new(),
            clip_uids: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn color(&self) -> Option<u32> {
        self.color
    }

    pub(crate) fn set_color(&mut self, color: Option<u32>) {
        self.color = color;
    }

    pub fn mixer(&self) -> &MixerSettings {
        &self.mixer
    }

    pub fn sends(&self) -> &[TrackSend] {
        &self.sends
    }

    pub fn recording_input(&self) -> &MidiRecordingInput {
        &self.recording_input
    }

    pub(crate) fn set_recording_input(&mut self, input: MidiRecordingInput) {
        self.recording_input = input;
    }

    pub fn effects(&self) -> &[PluginInstance] {
        &self.effects
    }

    pub fn mixer_effects(&self) -> &[PluginInstance] {
        &self.mixer_effects
    }

    pub fn automation(&self) -> &[AutomationLane] {
        &self.automation
    }

    pub fn clips(&self) -> &[MidiClip] {
        &self.clips
    }

    pub fn instrument(&self) -> Option<&Instrument> {
        self.instrument.as_ref()
    }

    pub fn instrument_bypassed(&self) -> bool {
        self.instrument_bypassed
    }

    pub(crate) fn set_instrument_bypassed(&mut self, bypassed: bool) {
        self.instrument_bypassed = bypassed && self.instrument.is_some();
    }

    pub(crate) fn set_instrument(&mut self, instrument: Option<Instrument>) {
        if instrument.is_some() {
            self.plugin_instrument = None;
        } else {
            self.instrument_bypassed = false;
        }
        self.instrument = instrument;
    }

    pub fn plugin_instrument(&self) -> Option<&PluginInstance> {
        self.plugin_instrument.as_deref()
    }

    pub(crate) fn plugin_instrument_mut(&mut self) -> Option<&mut PluginInstance> {
        self.plugin_instrument.as_deref_mut()
    }

    pub(crate) fn set_plugin_instrument(&mut self, instrument: Option<PluginInstance>) {
        if instrument.is_some() {
            self.instrument = None;
            self.instrument_bypassed = false;
        }
        self.plugin_instrument = instrument.map(Box::new);
    }

    pub fn clip_uids(&self) -> &[u64] {
        &self.clip_uids
    }

    pub(crate) fn push_clip(&mut self, clip: MidiClip) {
        self.push_clip_with_uid(clip, 0);
    }

    pub(crate) fn push_clip_with_uid(&mut self, clip: MidiClip, uid: u64) {
        self.clips.push(clip);
        self.clip_uids.push(uid);
    }

    pub(crate) fn replace_clip(&mut self, index: usize, clip: MidiClip) {
        self.clips[index] = clip;
    }

    pub(crate) fn remove_clip(&mut self, index: usize) -> MidiClip {
        self.clip_uids.remove(index);
        self.clips.remove(index)
    }

    pub(crate) fn set_name(&mut self, name: String) {
        self.name = name;
    }

    pub(crate) fn set_mixer(&mut self, mixer: MixerSettings) {
        self.mixer = mixer;
    }
}

#[derive(Debug, Clone)]
pub struct GroupTrack {
    uid: u64,
    name: String,
    color: Option<u32>,
    mixer: MixerSettings,
    sends: Vec<TrackSend>,
    effects: Vec<PluginInstance>,
    mixer_effects: Vec<PluginInstance>,
    automation: Vec<AutomationLane>,
}

impl GroupTrack {
    pub(crate) fn new(name: String) -> Self {
        Self {
            uid: 0,
            name,
            color: None,
            mixer: MixerSettings::default(),
            sends: Vec::new(),
            effects: Vec::new(),
            mixer_effects: Vec::new(),
            automation: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn color(&self) -> Option<u32> {
        self.color
    }

    pub(crate) fn set_color(&mut self, color: Option<u32>) {
        self.color = color;
    }

    pub fn mixer(&self) -> &MixerSettings {
        &self.mixer
    }

    pub fn sends(&self) -> &[TrackSend] {
        &self.sends
    }

    pub fn effects(&self) -> &[PluginInstance] {
        &self.effects
    }

    pub fn mixer_effects(&self) -> &[PluginInstance] {
        &self.mixer_effects
    }

    pub fn automation(&self) -> &[AutomationLane] {
        &self.automation
    }

    pub(crate) fn set_mixer(&mut self, mixer: MixerSettings) {
        self.mixer = mixer;
    }

    pub(crate) fn set_name(&mut self, name: String) {
        self.name = name;
    }
}

#[derive(Debug, Clone)]
pub struct BusTrack {
    uid: u64,
    name: String,
    color: Option<u32>,
    mixer: MixerSettings,
    sends: Vec<TrackSend>,
    effects: Vec<PluginInstance>,
    mixer_effects: Vec<PluginInstance>,
    automation: Vec<AutomationLane>,
}

impl BusTrack {
    pub(crate) fn new(name: String) -> Self {
        Self {
            uid: 0,
            name,
            color: None,
            mixer: MixerSettings::default(),
            sends: Vec::new(),
            effects: Vec::new(),
            mixer_effects: Vec::new(),
            automation: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn color(&self) -> Option<u32> {
        self.color
    }

    pub(crate) fn set_color(&mut self, color: Option<u32>) {
        self.color = color;
    }

    pub fn mixer(&self) -> &MixerSettings {
        &self.mixer
    }

    pub fn sends(&self) -> &[TrackSend] {
        &self.sends
    }

    pub fn effects(&self) -> &[PluginInstance] {
        &self.effects
    }

    pub fn mixer_effects(&self) -> &[PluginInstance] {
        &self.mixer_effects
    }

    pub fn automation(&self) -> &[AutomationLane] {
        &self.automation
    }

    pub(crate) fn set_mixer(&mut self, mixer: MixerSettings) {
        self.mixer = mixer;
    }

    pub(crate) fn set_name(&mut self, name: String) {
        self.name = name;
    }
}

#[derive(Debug, Clone)]
pub struct MasterTrack {
    uid: u64,
    mixer: MixerSettings,
    sends: Vec<TrackSend>,
    effects: Vec<PluginInstance>,
    mixer_effects: Vec<PluginInstance>,
    automation: Vec<AutomationLane>,
}

impl MasterTrack {
    pub(crate) fn new() -> Self {
        Self {
            uid: 0,
            mixer: MixerSettings::default().with_output(AudioOutput::Output),
            sends: Vec::new(),
            effects: Vec::new(),
            mixer_effects: Vec::new(),
            automation: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        "Master"
    }

    pub fn mixer(&self) -> &MixerSettings {
        &self.mixer
    }

    pub fn sends(&self) -> &[TrackSend] {
        &self.sends
    }

    pub fn effects(&self) -> &[PluginInstance] {
        &self.effects
    }

    pub fn mixer_effects(&self) -> &[PluginInstance] {
        &self.mixer_effects
    }

    pub fn automation(&self) -> &[AutomationLane] {
        &self.automation
    }

    pub(crate) fn set_mixer(&mut self, mixer: MixerSettings) {
        self.mixer = mixer;
    }
}

#[derive(Debug, Clone)]
pub enum Track {
    Audio(AudioTrack),
    Midi(MidiTrack),
    Group(GroupTrack),
    Bus(BusTrack),
    Master(MasterTrack),
}

impl Track {
    pub fn uid(&self) -> u64 {
        match self {
            Self::Audio(track) => track.uid,
            Self::Midi(track) => track.uid,
            Self::Group(track) => track.uid,
            Self::Bus(track) => track.uid,
            Self::Master(track) => track.uid,
        }
    }

    pub(crate) fn set_uid(&mut self, uid: u64) {
        match self {
            Self::Audio(track) => track.uid = uid,
            Self::Midi(track) => track.uid = uid,
            Self::Group(track) => track.uid = uid,
            Self::Bus(track) => track.uid = uid,
            Self::Master(track) => track.uid = uid,
        }
    }

    pub fn clip_uids(&self) -> &[u64] {
        match self {
            Self::Audio(track) => &track.clip_uids,
            Self::Midi(track) => &track.clip_uids,
            Self::Group(_) | Self::Bus(_) | Self::Master(_) => &[],
        }
    }

    pub(crate) fn clip_uids_mut(&mut self) -> &mut [u64] {
        match self {
            Self::Audio(track) => &mut track.clip_uids,
            Self::Midi(track) => &mut track.clip_uids,
            Self::Group(_) | Self::Bus(_) | Self::Master(_) => &mut [],
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Audio(track) => track.name(),
            Self::Midi(track) => track.name(),
            Self::Group(track) => track.name(),
            Self::Bus(track) => track.name(),
            Self::Master(track) => track.name(),
        }
    }

    pub fn color(&self) -> Option<u32> {
        match self {
            Self::Audio(track) => track.color(),
            Self::Midi(track) => track.color(),
            Self::Group(track) => track.color(),
            Self::Bus(track) => track.color(),
            Self::Master(_) => None,
        }
    }

    pub fn mixer(&self) -> &MixerSettings {
        match self {
            Self::Audio(track) => track.mixer(),
            Self::Midi(track) => track.mixer(),
            Self::Group(track) => track.mixer(),
            Self::Bus(track) => track.mixer(),
            Self::Master(track) => track.mixer(),
        }
    }

    pub fn sends(&self) -> &[TrackSend] {
        match self {
            Self::Audio(track) => &track.sends,
            Self::Midi(track) => &track.sends,
            Self::Group(track) => &track.sends,
            Self::Bus(track) => &track.sends,
            Self::Master(track) => &track.sends,
        }
    }

    pub(crate) fn sends_mut(&mut self) -> &mut Vec<TrackSend> {
        match self {
            Self::Audio(track) => &mut track.sends,
            Self::Midi(track) => &mut track.sends,
            Self::Group(track) => &mut track.sends,
            Self::Bus(track) => &mut track.sends,
            Self::Master(track) => &mut track.sends,
        }
    }

    pub fn effects(&self) -> &[PluginInstance] {
        match self {
            Self::Audio(track) => &track.effects,
            Self::Midi(track) => &track.effects,
            Self::Group(track) => &track.effects,
            Self::Bus(track) => &track.effects,
            Self::Master(track) => &track.effects,
        }
    }

    pub fn mixer_effects(&self) -> &[PluginInstance] {
        match self {
            Self::Audio(track) => &track.mixer_effects,
            Self::Midi(track) => &track.mixer_effects,
            Self::Group(track) => &track.mixer_effects,
            Self::Bus(track) => &track.mixer_effects,
            Self::Master(track) => &track.mixer_effects,
        }
    }

    pub fn all_effects(&self) -> impl Iterator<Item = &PluginInstance> {
        self.effects().iter().chain(self.mixer_effects())
    }

    pub(crate) fn mixer_effects_mut(&mut self) -> &mut Vec<PluginInstance> {
        match self {
            Self::Audio(track) => &mut track.mixer_effects,
            Self::Midi(track) => &mut track.mixer_effects,
            Self::Group(track) => &mut track.mixer_effects,
            Self::Bus(track) => &mut track.mixer_effects,
            Self::Master(track) => &mut track.mixer_effects,
        }
    }

    pub(crate) fn all_effects_mut(&mut self) -> impl Iterator<Item = &mut PluginInstance> {
        let (devices, mixer) = match self {
            Self::Audio(track) => (&mut track.effects, &mut track.mixer_effects),
            Self::Midi(track) => (&mut track.effects, &mut track.mixer_effects),
            Self::Group(track) => (&mut track.effects, &mut track.mixer_effects),
            Self::Bus(track) => (&mut track.effects, &mut track.mixer_effects),
            Self::Master(track) => (&mut track.effects, &mut track.mixer_effects),
        };
        devices.iter_mut().chain(mixer)
    }

    pub fn automation(&self) -> &[AutomationLane] {
        match self {
            Self::Audio(track) => &track.automation,
            Self::Midi(track) => &track.automation,
            Self::Group(track) => &track.automation,
            Self::Bus(track) => &track.automation,
            Self::Master(track) => &track.automation,
        }
    }

    pub(crate) fn automation_mut(&mut self) -> &mut Vec<AutomationLane> {
        match self {
            Self::Audio(track) => &mut track.automation,
            Self::Midi(track) => &mut track.automation,
            Self::Group(track) => &mut track.automation,
            Self::Bus(track) => &mut track.automation,
            Self::Master(track) => &mut track.automation,
        }
    }

    pub(crate) fn prune_automation(&mut self) {
        let old = std::mem::take(self.automation_mut());
        let retained = old
            .into_iter()
            .filter(|lane| lane.target().value(self).is_some())
            .collect();
        *self.automation_mut() = retained;
    }

    pub(crate) fn effects_mut(&mut self) -> &mut Vec<PluginInstance> {
        match self {
            Self::Audio(track) => &mut track.effects,
            Self::Midi(track) => &mut track.effects,
            Self::Group(track) => &mut track.effects,
            Self::Bus(track) => &mut track.effects,
            Self::Master(track) => &mut track.effects,
        }
    }

    pub(crate) fn set_mixer(&mut self, mixer: MixerSettings) {
        match self {
            Self::Audio(track) => track.set_mixer(mixer),
            Self::Midi(track) => track.set_mixer(mixer),
            Self::Group(track) => track.set_mixer(mixer),
            Self::Bus(track) => track.set_mixer(mixer),
            Self::Master(track) => track.set_mixer(mixer),
        }
    }
}
