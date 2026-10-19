use std::collections::BTreeSet;
use std::path::PathBuf;

use sympoie::automation::{AutomationLane, AutomationTarget};
use sympoie::clip::MidiNote;
use sympoie::instrument::Instrument;
use sympoie::pitch::PitchAlgorithm;
use sympoie::plugin::{PluginDescriptor, PluginKind, SidechainRoute};
use sympoie::time::{SnapGrid, TimeSignature};
use sympoie::track::{AudioRecordingInput, MidiRecordingInput, MixerSettings};
use sympoie::transport::TransportCommand;

use crate::document::edit::{ClipRef, TrackDrop, TrackKind};
use crate::state::{GridMode, TimeRange};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Transport(TransportCommand),
    TogglePlay,
    ToggleRecord,
    ToggleTrackArm(usize),
    SetAudioRecordingInput {
        track: usize,
        input: AudioRecordingInput,
    },
    SetMidiRecordingInput {
        track: usize,
        input: MidiRecordingInput,
    },
    RefreshRecordingInputs,
    Stop,
    Seek(u64),
    SetLoop {
        start: u64,
        end: u64,
        enable: bool,
    },
    ToggleLoop,
    LoopSelection,
    LoopClip(ClipRef),
    ToggleFollow,
    ToggleAgentMode,
    ToggleAutomation,
    SetAutomation {
        track: usize,
        lane: AutomationLane,
    },
    RemoveAutomation {
        track: usize,
        target: AutomationTarget,
    },

    SetBpm(f64),
    SetTimeSignature(TimeSignature),
    SetSnap(SnapGrid),
    SetGridMode(GridMode),
    StepGrid {
        narrower: bool,
    },

    AddTrack(TrackKind),
    AddBusTrack,
    RenameTrack(usize, String),
    SetTrackColor {
        tracks: BTreeSet<usize>,
        color: Option<u32>,
    },
    DeleteTrack(usize),
    MoveTrack {
        track: usize,
        target: TrackDrop,
    },
    ExitGroup(usize),
    BounceTrack {
        track: usize,
        duplicate: bool,
    },
    GroupSelected,
    Ungroup(usize),
    SetMixer {
        track: usize,
        mixer: MixerSettings,
    },
    SetTrackSend {
        track: usize,
        bus: usize,
        gain: f32,
    },
    RemoveTrackSend {
        track: usize,
        bus: usize,
    },
    SetSolo {
        track: usize,
        soloed: bool,
    },
    SetInstrument {
        track: usize,
        instrument: Option<Instrument>,
    },
    SetInstrumentBypassed {
        track: usize,
        bypassed: bool,
    },
    SetInstrumentParameter {
        track: usize,
        instrument: Instrument,
        parameter: &'static str,
    },
    ToggleFold(usize),

    MoveClip {
        clip: ClipRef,
        to_track: usize,
        start: u64,
    },
    CopyClipTo {
        clip: ClipRef,
        to_track: usize,
        start: u64,
    },
    CopyClip(ClipRef),
    PasteClip,
    TrimClipStart {
        clip: ClipRef,
        start: u64,
    },
    TrimClipEnd {
        clip: ClipRef,
        end: u64,
    },
    SetClipGain {
        clip: ClipRef,
        gain: f32,
    },
    RenameClip {
        clip: ClipRef,
        name: String,
    },
    SetClipPitch {
        clip: ClipRef,
        semitones: f32,
    },
    SetClipPitchOptions {
        clip: ClipRef,
        algorithm: PitchAlgorithm,
        preserve_formants: bool,
    },
    ResetClipPitch(ClipRef),
    ReverseClip(ClipRef),
    ReverseSelection,
    DeleteClip(ClipRef),
    DuplicateClip(ClipRef),
    SplitClip(ClipRef),
    InsertMidiClip {
        track: usize,
        start: u64,
    },
    OpenPianoRoll(ClipRef),
    ClosePianoRoll,
    AuditionMidiNotes {
        track: usize,
        notes: Vec<MidiNote>,
    },
    BeginMidiAudition {
        track: usize,
        notes: Vec<MidiNote>,
    },
    ReleaseMidiAudition,
    EditMidiNotes {
        clip: ClipRef,
        before: Vec<MidiNote>,
        notes: Vec<MidiNote>,
        label: &'static str,
    },
    EditMidiPitchBend {
        clip: ClipRef,
        before: Vec<sympoie::clip::BendPoint>,
        points: Vec<sympoie::clip::BendPoint>,
        label: &'static str,
    },
    ImportAudio {
        paths: Vec<PathBuf>,
        track: Option<usize>,
        start: Option<u64>,
    },
    ChooseAudioToImport {
        track: Option<usize>,
        start: Option<u64>,
    },
    AddLibraryFolder,
    RemoveLibraryFolder(PathBuf),

    SelectClip(Option<ClipRef>),
    SelectTracks(BTreeSet<usize>),
    SetTimeSelection(Option<TimeRange>),
    MoveTimeSelection {
        to: u64,
    },
    DeleteTimeSelection,
    SplitTimeSelection,
    InsertMidiClipInSelection,

    NewProject,
    OpenProject,
    OpenProjectFolder,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    ShowExport,
    ExportAll,
    ExportClipMidi(ClipRef),
    ExportClipAudio(ClipRef),
    Undo,
    Redo,
    Quit,

    BrowsePlugins {
        track: usize,
        kind: PluginKind,
    },
    BrowseMixerEffects {
        track: usize,
    },
    LoadMixerVst3 {
        track: usize,
        plugin: PluginDescriptor,
        before: Option<u64>,
    },
    LoadVst3 {
        track: usize,
        plugin: PluginDescriptor,
    },
    InsertVst3Before {
        track: usize,
        plugin: PluginDescriptor,
        before: u64,
    },
    SetVst3Parameter {
        id: u64,
        parameter: u32,
        value: f64,
    },
    LoadSamplerSample {
        id: u64,
    },
    ResetBuiltinParameters {
        id: u64,
    },
    LoadBuiltinPreset {
        id: u64,
        preset: usize,
    },
    SetVst3Bypassed {
        id: u64,
        bypassed: bool,
    },
    SetVst3Sidechain {
        id: u64,
        route: Option<SidechainRoute>,
    },
    SetVst3SidechainGain {
        id: u64,
        gain: f32,
    },
    OpenVst3Sidechain(u64),
    CloseVst3Sidechain(u64),
    RemoveVst3 {
        track: usize,
        id: u64,
    },
    MoveEffect {
        track: usize,
        from: usize,
        to: usize,
    },
    MoveMixerEffect {
        track: usize,
        from: usize,
        to: usize,
    },
    OpenVst3Editor(u64),
    CloseVst3Editor(u64),
    OpenNativeVst3Editor(u64),
    /// Open the before/after waveform and spectrum window of a built-in device.
    OpenBuiltinScope(u64),
    CloseBuiltinScope(u64),
    ShowDevices,
    CloseDrawer,
    ShowClipView,
    ShowMixer,
    OpenPluginEditor(usize),
    ClosePluginEditor(usize),
    ToggleMixer,
    ToggleBrowser,
    ToggleRack,
    ToggleDetailView,
    Zoom(f64),
    ZoomToFit,
    ResetTrackHeights,
    ShowShortcuts,
    ShowAbout,
    ShowSettings,
}
