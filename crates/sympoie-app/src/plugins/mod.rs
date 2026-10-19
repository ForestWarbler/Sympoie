mod editor;
mod probe;
pub mod scan;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use sympoie::automation::AutomationTarget;
use sympoie::builtin::{self, BuiltinKind, Telemetry};
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{
    PluginDescriptor, PluginHost, PluginInstance, PluginKind, PluginParameter,
    PluginProcessContext, PluginProcessor, PluginSidechainInput,
};
use sympoie::{Frame, SAMPLE_RATE};
use vst3_host::audio::{AudioBusBuffer, AudioBusLayout, BusDirection, MediaType};
use vst3_host::{
    BusAudioBuffers, MidiChannel, MidiEvent, NoteId, Plugin, TransportPosition, Vst3Host,
};

pub use editor::PluginEditor;
pub use probe::run_probe;

const BLOCK_SIZE: usize = 512;
const RAMP_FRAMES: usize = SAMPLE_RATE as usize / 50;

type SharedPlugin = Arc<Mutex<Plugin>>;
type SharedRamps = Arc<Mutex<Vec<ParameterRamp>>>;
type Registry = Arc<Mutex<HashMap<u64, LiveInstance>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginBusInfo {
    pub index: u32,
    pub name: String,
    pub channels: usize,
}

struct BusState {
    buffers: BusAudioBuffers,
    aux_inputs: Vec<PluginBusInfo>,
    sidechain_bus: Option<u32>,
    main_input: Option<usize>,
    main_output: usize,
}

#[derive(Clone)]
struct LiveInstance {
    plugin: SharedPlugin,
    ramps: SharedRamps,
    descriptor: PluginDescriptor,
    state: Arc<[u8]>,
    buses: Arc<Mutex<BusState>>,
    latency: Arc<AtomicU32>,
}

struct StagedInstance {
    descriptor: PluginDescriptor,
    state: Vec<u8>,
    plugin: Plugin,
}

pub struct PluginLease {
    instances: HashMap<u64, LiveInstance>,
    builtins: HashMap<u64, (BuiltinKind, Arc<Telemetry>)>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl PluginHost for PluginLease {
    fn load_builtin(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        self.load(instance)
    }

    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        if BuiltinKind::from_descriptor(&instance.descriptor).is_some() {
            return builtin::create_processor_with_telemetry(
                instance,
                self.builtins
                    .get(&instance.id)
                    .map(|(_, data)| Arc::clone(data)),
            );
        }
        let entry = self.instances.get(&instance.id).ok_or_else(|| {
            format!(
                "{} was not prepared for this project",
                instance.descriptor.name
            )
        })?;
        processor(instance, entry, Arc::clone(&self.errors))
    }
}

pub struct PluginRuntime {
    live: Registry,
    builtins: Mutex<HashMap<u64, (BuiltinKind, Arc<Telemetry>)>>,
    staged: Mutex<Vec<StagedInstance>>,
    errors: Arc<Mutex<Vec<String>>>,
    pending_edits: Mutex<Vec<(u64, u32, f64)>>,
    automated_parameters: Mutex<HashSet<(u64, u32)>>,
    retired: Mutex<Vec<LiveInstance>>,
    offline: Mutex<Vec<Arc<PluginRuntime>>>,
    prepared_only: AtomicBool,
    state_dirty: AtomicBool,
}

impl fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginRuntime").finish_non_exhaustive()
    }
}

impl PluginRuntime {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            live: Arc::new(Mutex::new(HashMap::new())),
            builtins: Mutex::new(HashMap::new()),
            staged: Mutex::new(Vec::new()),
            errors: Arc::new(Mutex::new(Vec::new())),
            pending_edits: Mutex::new(Vec::new()),
            automated_parameters: Mutex::new(HashSet::new()),
            retired: Mutex::new(Vec::new()),
            offline: Mutex::new(Vec::new()),
            prepared_only: AtomicBool::new(false),
            state_dirty: AtomicBool::new(false),
        })
    }

    pub fn inspect(&self, descriptor: &PluginDescriptor) -> Result<PluginInstance, String> {
        if let Some(kind) = BuiltinKind::from_descriptor(descriptor) {
            return Ok(kind.instance());
        }
        let mut plugin = load_plugin(descriptor)?;
        main_channels(&plugin, &descriptor.name)?;
        plugin.start_processing().map_err(|e| e.to_string())?;
        plugin.stop_processing().map_err(|e| e.to_string())?;
        let parameters = parameters(&plugin)?;
        let state = plugin.save_state().map_err(|e| e.to_string())?;
        let instance = PluginInstance::new(descriptor.clone(), parameters, state);
        self.staged.lock().map_err(poisoned)?.push(StagedInstance {
            descriptor: descriptor.clone(),
            state: instance.state.clone(),
            plugin,
        });
        Ok(instance)
    }

    pub fn prepare_project(&self, project: &sympoie::project::Project) -> Result<(), String> {
        *self.automated_parameters.lock().map_err(poisoned)? = automated_parameters(project);
        self.prepared_only.store(true, Ordering::Relaxed);
        let mut ids = HashSet::new();
        let mut failures = Vec::new();
        for track in project.tracks() {
            let instrument = match track {
                sympoie::track::Track::Midi(track) => track.plugin_instrument(),
                _ => None,
            };
            for instance in instrument.into_iter().chain(track.all_effects()) {
                ids.insert(instance.id);
                if let Err(error) = self.prepare_instance(instance) {
                    failures.push(error);
                    let obsolete = self.live.lock().map_err(poisoned)?.remove(&instance.id);
                    if let Some(obsolete) = obsolete {
                        self.retired.lock().map_err(poisoned)?.push(obsolete);
                    }
                }
            }
        }
        let removed: Vec<_> = self
            .live
            .lock()
            .map_err(poisoned)?
            .extract_if(|id, _| !ids.contains(id))
            .map(|(_, instance)| instance)
            .collect();
        self.retired.lock().map_err(poisoned)?.extend(removed);
        self.builtins
            .lock()
            .map_err(poisoned)?
            .retain(|id, _| ids.contains(id));
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    /// Prepares a separate runtime for rendering `project` on a worker thread.
    ///
    /// VST3 plug-ins must be created, serviced and destroyed on the main thread, so call this
    /// there. The worker may only process audio and read errors; this runtime keeps the other
    /// one, services it and drops it in `service` once the worker has let go of it.
    pub fn prepare_offline(
        &self,
        project: &sympoie::project::Project,
    ) -> Result<Arc<PluginRuntime>, String> {
        let runtime = PluginRuntime::new();
        runtime.prepare_project(project)?;
        self.offline
            .lock()
            .map_err(poisoned)?
            .push(Arc::clone(&runtime));
        Ok(runtime)
    }

    fn plugins_in_use(&self) -> bool {
        self.live.lock().map_or(true, |live| {
            live.values()
                .any(|entry| Arc::strong_count(&entry.plugin) > 1)
        }) || self
            .retired
            .lock()
            .map_or(true, |retired| !retired.is_empty())
    }

    pub fn clear(&self) {
        self.state_dirty.store(false, Ordering::Relaxed);
        if let Ok(mut builtins) = self.builtins.lock() {
            builtins.clear();
        }
        if let Ok(mut live) = self.live.lock()
            && let Ok(mut retired) = self.retired.lock()
        {
            retired.extend(live.drain().map(|(_, instance)| instance));
        }
        if let Ok(mut staged) = self.staged.lock() {
            staged.clear();
        }
        if let Ok(mut edits) = self.pending_edits.lock() {
            edits.clear();
        }
        if let Ok(mut automated) = self.automated_parameters.lock() {
            automated.clear();
        }
    }

    pub fn pin_project(&self, project: &sympoie::project::Project) -> PluginLease {
        let mut instances = HashMap::new();
        if let Ok(live) = self.live.lock() {
            for track in project.tracks() {
                let instrument = match track {
                    sympoie::track::Track::Midi(track) => track.plugin_instrument(),
                    _ => None,
                };
                for instance in instrument.into_iter().chain(track.all_effects()) {
                    if let Some(entry) = live.get(&instance.id)
                        && entry.descriptor == instance.descriptor
                        && entry.state.as_ref() == instance.state.as_slice()
                    {
                        instances.insert(instance.id, entry.clone());
                    }
                }
            }
        }
        PluginLease {
            instances,
            builtins: self
                .builtins
                .lock()
                .map(|entries| entries.clone())
                .unwrap_or_default(),
            errors: Arc::clone(&self.errors),
        }
    }

    fn prepare_instance(&self, instance: &PluginInstance) -> Result<(), String> {
        if let Some(kind) = BuiltinKind::from_descriptor(&instance.descriptor) {
            let obsolete = self.live.lock().map_err(poisoned)?.remove(&instance.id);
            if let Some(obsolete) = obsolete {
                self.retired.lock().map_err(poisoned)?.push(obsolete);
            }
            let mut builtins = self.builtins.lock().map_err(poisoned)?;
            let entry = builtins
                .entry(instance.id)
                .or_insert_with(|| (kind, Arc::new(Telemetry::default())));
            if entry.0 != kind {
                *entry = (kind, Arc::new(Telemetry::default()));
            }
            return Ok(());
        }
        self.builtins.lock().map_err(poisoned)?.remove(&instance.id);
        let existing = self
            .live
            .lock()
            .map_err(poisoned)?
            .get(&instance.id)
            .filter(|entry| {
                entry.descriptor == instance.descriptor
                    && entry.state.as_ref() == instance.state.as_slice()
            })
            .cloned();
        let requested_bus = instance.sidechain.as_ref().map(|route| route.bus_index);
        if let Some(entry) = existing {
            let changed = entry.buses.lock().map_err(poisoned)?.sidechain_bus != requested_bus;
            if changed {
                let mut plugin = entry.plugin.lock().map_err(poisoned)?;
                let mut buses = entry.buses.lock().map_err(poisoned)?;
                let replacement =
                    configure_buses(&mut plugin, &instance.descriptor.name, requested_bus)?;
                *buses = replacement;
                entry
                    .latency
                    .store(plugin.latency_samples(), Ordering::Release);
            }
            return Ok(());
        }
        let staged = {
            let mut staged = self.staged.lock().map_err(poisoned)?;
            staged
                .iter()
                .position(|entry| {
                    entry.descriptor == instance.descriptor && entry.state == instance.state
                })
                .map(|index| staged.swap_remove(index))
        };
        let mut plugin = match staged {
            Some(staged) => staged.plugin,
            None => load_plugin(&instance.descriptor)?,
        };
        if !instance.state.is_empty() {
            plugin
                .load_state(&instance.state)
                .map_err(|e| e.to_string())?;
        }
        for parameter in &instance.parameters {
            if !parameter.read_only {
                plugin
                    .set_parameter(parameter.id, parameter.value)
                    .map_err(|e| e.to_string())?;
            }
        }
        let buses = configure_buses(&mut plugin, &instance.descriptor.name, requested_bus)?;
        plugin.start_processing().map_err(|e| e.to_string())?;
        let latency = plugin.latency_samples();
        let replaced = self.live.lock().map_err(poisoned)?.insert(
            instance.id,
            LiveInstance {
                plugin: Arc::new(Mutex::new(plugin)),
                ramps: Arc::new(Mutex::new(
                    instance.parameters.iter().map(ParameterRamp::new).collect(),
                )),
                descriptor: instance.descriptor.clone(),
                state: Arc::from(instance.state.clone()),
                buses: Arc::new(Mutex::new(buses)),
                latency: Arc::new(AtomicU32::new(latency)),
            },
        );
        if let Some(replaced) = replaced {
            self.retired.lock().map_err(poisoned)?.push(replaced);
        }
        Ok(())
    }

    pub fn sidechain_inputs(&self, id: u64) -> Vec<PluginBusInfo> {
        if self.builtins.lock().is_ok_and(|entries| {
            entries
                .get(&id)
                .is_some_and(|(kind, _)| matches!(kind, BuiltinKind::Comp | BuiltinKind::VariMu))
        }) {
            return vec![PluginBusInfo {
                index: 1,
                name: "External Sidechain".to_owned(),
                channels: 2,
            }];
        }
        self.live
            .lock()
            .ok()
            .and_then(|live| live.get(&id).map(|entry| Arc::clone(&entry.buses)))
            .and_then(|buses| buses.lock().ok().map(|buses| buses.aux_inputs.clone()))
            .unwrap_or_default()
    }

    pub fn latency_samples(&self, id: u64) -> Option<u32> {
        if self
            .builtins
            .lock()
            .is_ok_and(|entries| entries.contains_key(&id))
        {
            return Some(0);
        }
        self.live
            .lock()
            .ok()?
            .get(&id)
            .map(|entry| entry.latency.load(Ordering::Acquire))
    }

    pub fn builtin_telemetry(&self, id: u64) -> Option<Arc<Telemetry>> {
        self.builtins
            .lock()
            .ok()?
            .get(&id)
            .map(|(_, data)| Arc::clone(data))
    }

    pub fn service(&self) {
        if let Err(error) = self.service_with_mode(false)
            && let Ok(mut errors) = self.errors.lock()
        {
            errors.push(error);
        }
    }

    pub fn service_blocking(&self) -> Result<(), String> {
        self.service_with_mode(true)
    }

    fn service_with_mode(&self, wait: bool) -> Result<(), String> {
        let automated = self.automated_parameters.lock().map_err(poisoned)?.clone();
        let offline = {
            let mut offline = self.offline.lock().map_err(poisoned)?;
            offline.retain(|runtime| Arc::strong_count(runtime) > 1 || runtime.plugins_in_use());
            offline.clone()
        };
        for runtime in offline {
            runtime.service();
        }
        self.retired
            .lock()
            .map_err(poisoned)?
            .retain(|entry| Arc::strong_count(&entry.plugin) > 1);
        let handles: Vec<_> = self
            .live
            .lock()
            .map(|live| {
                live.iter()
                    .map(|(&id, entry)| (id, entry.clone()))
                    .collect()
            })
            .map_err(poisoned)?;
        for (id, entry) in handles {
            let Some(mut plugin) = control_lock(&entry.plugin, wait)? else {
                continue;
            };
            if plugin.take_host_notifications().iter().any(|notification| {
                matches!(
                    notification,
                    vst3_host::HostNotification::DirtyChanged(true)
                )
            }) {
                self.state_dirty.store(true, Ordering::Relaxed);
            }
            match plugin.service_host_requests() {
                Ok(flags) => {
                    if flags.io_changed() {
                        let mut buses = entry.buses.lock().map_err(poisoned)?;
                        let requested = buses.sidechain_bus;
                        match configure_buses(&mut plugin, &entry.descriptor.name, requested) {
                            Ok(replacement) => *buses = replacement,
                            Err(error) => {
                                if let Ok(mut replacement) =
                                    configure_buses(&mut plugin, &entry.descriptor.name, None)
                                {
                                    replacement.sidechain_bus = requested;
                                    *buses = replacement;
                                }
                                self.errors
                                    .lock()
                                    .map_err(poisoned)?
                                    .push(format!("{}: {error}", entry.descriptor.name));
                            }
                        }
                    }
                    if flags.latency_changed() || flags.io_changed() {
                        entry
                            .latency
                            .store(plugin.latency_samples(), Ordering::Release);
                    }
                    if flags.param_values_changed() || flags.param_titles_changed() {
                        let parameters = parameters(&plugin)?;
                        let mut ramps = entry.ramps.lock().map_err(poisoned)?;
                        let mut edits = self.pending_edits.lock().map_err(poisoned)?;
                        for parameter in parameters {
                            if automated.contains(&(id, parameter.id)) {
                                continue;
                            }
                            if let Some(ramp) =
                                ramps.iter_mut().find(|ramp| ramp.id == parameter.id)
                            {
                                if flags.param_values_changed() {
                                    ramp.snap(parameter.value);
                                }
                            } else {
                                ramps.push(ParameterRamp::new(&parameter));
                            }
                            if flags.param_values_changed() && !parameter.read_only {
                                edits.push((id, parameter.id, parameter.value));
                            }
                        }
                    }
                }
                Err(error) => {
                    self.errors
                        .lock()
                        .map_err(poisoned)?
                        .push(error.to_string());
                }
            }
            if wait
                && plugin.take_host_notifications().iter().any(|notification| {
                    matches!(
                        notification,
                        vst3_host::HostNotification::DirtyChanged(true)
                    )
                })
            {
                self.state_dirty.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    pub fn discard_staged(&self, descriptor: &PluginDescriptor) {
        if let Ok(mut staged) = self.staged.lock() {
            staged.retain(|entry| &entry.descriptor != descriptor);
        }
    }

    pub fn has_instance(&self, id: u64) -> bool {
        self.live.lock().is_ok_and(|live| live.contains_key(&id))
    }

    pub fn take_state_dirty(&self) -> bool {
        self.state_dirty.swap(false, Ordering::Relaxed)
    }

    pub fn snapshot(&self, id: u64) -> Result<(Vec<PluginParameter>, Vec<u8>), String> {
        let (plugin, ramps) = self.handles(id)?;
        let plugin = plugin.lock().map_err(poisoned)?;
        let mut parameters = parameters(&plugin)?;
        {
            let ramps = ramps.lock().map_err(poisoned)?;
            for parameter in &mut parameters {
                if let Some(ramp) = ramps.iter().find(|ramp| ramp.id == parameter.id) {
                    parameter.value = ramp.target;
                }
            }
        }
        let state = plugin.save_state().map_err(|e| e.to_string())?;
        Ok((parameters, state))
    }

    fn handles(&self, id: u64) -> Result<(SharedPlugin, SharedRamps), String> {
        let live = self.live.lock().map_err(poisoned)?;
        let entry = live
            .get(&id)
            .ok_or_else(|| "The plugin is not loaded in the audio engine".to_owned())?;
        Ok((Arc::clone(&entry.plugin), Arc::clone(&entry.ramps)))
    }

    pub fn poll_edits(&self) -> Vec<(u64, u32, f64)> {
        match self.poll_edits_with_mode(false) {
            Ok(edits) => edits,
            Err(error) => {
                if let Ok(mut errors) = self.errors.lock() {
                    errors.push(error);
                }
                Vec::new()
            }
        }
    }

    pub fn poll_edits_blocking(&self) -> Result<Vec<(u64, u32, f64)>, String> {
        self.poll_edits_with_mode(true)
    }

    fn poll_edits_with_mode(&self, wait: bool) -> Result<Vec<(u64, u32, f64)>, String> {
        let handles: Vec<_> = self
            .live
            .lock()
            .map(|live| {
                live.iter()
                    .map(|(&id, entry)| (id, Arc::clone(&entry.plugin), Arc::clone(&entry.ramps)))
                    .collect()
            })
            .map_err(poisoned)?;
        let mut edits = self
            .pending_edits
            .lock()
            .map(|mut edits| std::mem::take(&mut *edits))
            .map_err(poisoned)?;
        for (id, plugin, ramps) in handles {
            let Some(mut plugin) = control_lock(&plugin, wait)? else {
                continue;
            };
            for edit in plugin.take_parameter_edits() {
                if let Some(value) = edit.value.filter(|value| (0.0..=1.0).contains(value)) {
                    let mut ramps = ramps.lock().map_err(poisoned)?;
                    if let Some(ramp) = ramps.iter_mut().find(|ramp| ramp.id == edit.id) {
                        ramp.snap(value);
                    }
                    edits.push((id, edit.id, value));
                }
            }
        }
        Ok(edits)
    }

    pub fn take_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|mut errors| std::mem::take(&mut *errors))
            .unwrap_or_default()
    }
}

fn automated_parameters(project: &sympoie::project::Project) -> HashSet<(u64, u32)> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.automation())
        .filter_map(|lane| {
            if lane.enabled()
                && !lane.points().is_empty()
                && let AutomationTarget::PluginParameter { plugin, parameter } = lane.target()
            {
                Some((plugin, parameter))
            } else {
                None
            }
        })
        .collect()
}

fn control_lock<T>(mutex: &Mutex<T>, wait: bool) -> Result<Option<MutexGuard<'_, T>>, String> {
    if wait {
        mutex.lock().map(Some).map_err(poisoned)
    } else {
        match mutex.try_lock() {
            Ok(guard) => Ok(Some(guard)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Poisoned(error)) => Err(poisoned(error)),
        }
    }
}

impl PluginHost for PluginRuntime {
    fn load_builtin(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        self.load(instance)
    }

    fn load(&self, instance: &PluginInstance) -> Result<Box<dyn PluginProcessor>, String> {
        if BuiltinKind::from_descriptor(&instance.descriptor).is_some() {
            return builtin::create_processor_with_telemetry(
                instance,
                self.builtin_telemetry(instance.id),
            );
        }
        if !self.prepared_only.load(Ordering::Relaxed) {
            self.prepare_instance(instance)?;
        }
        let live = self.live.lock().map_err(poisoned)?;
        let entry = live
            .get(&instance.id)
            .ok_or_else(|| "Plugin preparation failed".to_owned())?;
        processor(instance, entry, Arc::clone(&self.errors))
    }
}

fn processor(
    instance: &PluginInstance,
    entry: &LiveInstance,
    errors: Arc<Mutex<Vec<String>>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    if entry.descriptor != instance.descriptor || entry.state.as_ref() != instance.state.as_slice()
    {
        return Err(format!(
            "{} has not been prepared for this project revision",
            instance.descriptor.name
        ));
    }
    if let Ok(mut ramps) = entry.ramps.lock() {
        for parameter in &instance.parameters {
            if let Some(ramp) = ramps.iter_mut().find(|ramp| ramp.id == parameter.id) {
                ramp.set(parameter.value);
            }
        }
    }
    Ok(Box::new(VstProcessor {
        plugin: Arc::clone(&entry.plugin),
        ramps: Arc::clone(&entry.ramps),
        name: instance.descriptor.name.clone(),
        instrument: instance.descriptor.kind == PluginKind::Instrument,
        buses: Arc::clone(&entry.buses),
        latency: Arc::clone(&entry.latency),
        notes: Vec::with_capacity(4096),
        bends: [BendRamp::default(); 16],
        errors,
        failed: false,
    }))
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> String {
    "The plugin failed while holding its host lock".into()
}

#[cfg(test)]
pub(crate) static LOAD_THREADS: Mutex<Vec<(std::path::PathBuf, std::thread::ThreadId)>> =
    Mutex::new(Vec::new());

fn load_plugin(descriptor: &PluginDescriptor) -> Result<Plugin, String> {
    #[cfg(test)]
    LOAD_THREADS
        .lock()
        .unwrap()
        .push((descriptor.path.clone(), std::thread::current().id()));
    let mut host = Vst3Host::builder()
        .sample_rate(f64::from(SAMPLE_RATE))
        .block_size(BLOCK_SIZE)
        .input_channels(2)
        .output_channels(2)
        .build()
        .map_err(|e| e.to_string())?;
    host.load_plugin_class(&descriptor.path, &descriptor.class_id)
        .map_err(|e| format!("{}: {e}", descriptor.name))
}

fn main_channels(plugin: &Plugin, name: &str) -> Result<(usize, usize), String> {
    let layout = plugin.audio_bus_layout().map_err(|e| e.to_string())?;
    let info = plugin.bus_info().map_err(|e| e.to_string())?;
    let (main_input, main_output) = main_bus_indices(&info, &layout)?;
    let inputs = main_input.map_or(0, |index| layout.inputs[index].channel_count);
    let outputs = layout.outputs[main_output].channel_count;
    if !(1..=2).contains(&outputs) || inputs > 2 {
        return Err(format!(
            "{name} requires an unsupported main bus layout ({inputs} in, {outputs} out); this host supports mono and stereo"
        ));
    }
    Ok((inputs, outputs))
}

fn main_bus_indices(
    info: &vst3_host::discovery::BusLayout,
    layout: &AudioBusLayout,
) -> Result<(Option<usize>, usize), String> {
    if info.audio_inputs.len() != layout.inputs.len()
        || info.audio_outputs.len() != layout.outputs.len()
    {
        return Err("The plugin changed its bus layout during configuration".into());
    }
    let main_input = info.audio_inputs.iter().position(|bus| bus.bus_type == 0);
    let main_output = info
        .audio_outputs
        .iter()
        .position(|bus| bus.bus_type == 0)
        .ok_or_else(|| "The plugin has no main audio output".to_owned())?;
    Ok((main_input, main_output))
}

fn configure_buses(
    plugin: &mut Plugin,
    name: &str,
    sidechain_bus: Option<u32>,
) -> Result<BusState, String> {
    main_channels(plugin, name)?;
    let info = plugin.bus_info().map_err(|e| e.to_string())?;
    let layout = plugin.audio_bus_layout().map_err(|e| e.to_string())?;
    let (main_input, main_output) = main_bus_indices(&info, &layout)?;
    let aux_inputs: Vec<_> = info
        .audio_inputs
        .iter()
        .enumerate()
        .filter(|(index, bus)| {
            *index > 0
                && bus.bus_type == 1
                && (1..=2).contains(&layout.inputs[*index].channel_count)
        })
        .map(|(index, bus)| PluginBusInfo {
            index: index as u32,
            name: bus.name.clone(),
            channels: layout.inputs[index].channel_count,
        })
        .collect();
    if let Some(index) = sidechain_bus
        && !aux_inputs.iter().any(|bus| bus.index == index)
    {
        return Err(format!(
            "{name} does not expose mono/stereo sidechain input bus {index}"
        ));
    }
    let channels = layout
        .inputs
        .iter()
        .chain(&layout.outputs)
        .try_fold(0_usize, |sum, bus| sum.checked_add(bus.channel_count));
    if channels.is_none_or(|channels| channels > 256) {
        return Err(format!("{name} exposes too many audio channels"));
    }
    let mut activation = Vec::new();
    for bus in &aux_inputs {
        let active = Some(bus.index) == sidechain_bus;
        if layout.inputs[bus.index as usize].active != active {
            activation.push((BusDirection::Input, bus.index as i32, active));
        }
    }
    if let Some(index) = main_input
        && !layout.inputs[index].active
    {
        activation.push((BusDirection::Input, index as i32, true));
    }
    if !layout.outputs[main_output].active {
        activation.push((BusDirection::Output, main_output as i32, true));
    }
    if !activation.is_empty() {
        let was_processing = plugin.is_processing();
        if was_processing {
            plugin
                .stop_processing()
                .map_err(|error| error.to_string())?;
        }
        let change = activation
            .into_iter()
            .try_for_each(|(direction, index, active)| {
                plugin
                    .set_bus_active(MediaType::Audio, direction, index, active)
                    .map_err(|error| error.to_string())
            });
        let resume = if was_processing {
            plugin.start_processing().map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        change?;
        resume?;
    }
    let buffers = plugin
        .create_bus_audio_buffers(BLOCK_SIZE)
        .map_err(|error| error.to_string())?;
    Ok(BusState {
        buffers,
        aux_inputs,
        sidechain_bus,
        main_input,
        main_output,
    })
}

fn copy_stereo_to_bus(bus: &mut AudioBusBuffer, frames: &[Frame]) {
    let mono = bus.channels.len() == 1;
    for (channel, output) in bus.channels.iter_mut().enumerate() {
        for (sample, frame) in output.iter_mut().zip(frames) {
            *sample = if mono {
                (frame[0] + frame[1]) * 0.5
            } else {
                frame[channel]
            };
        }
    }
}

fn fill_bus_inputs(
    buses: &mut BusState,
    frames: &[Frame],
    sidechain: Option<PluginSidechainInput<'_>>,
    instrument: bool,
) -> Result<(), String> {
    if frames.len() > BLOCK_SIZE
        || sidechain.is_some_and(|input| input.frames.len() != frames.len())
    {
        return Err("invalid plugin bus block length".into());
    }
    for bus in buses
        .buffers
        .inputs
        .iter_mut()
        .chain(&mut buses.buffers.outputs)
    {
        for channel in &mut bus.channels {
            channel.resize(frames.len(), 0.0);
            channel.fill(0.0);
        }
    }
    buses.buffers.block_size = frames.len();
    if !instrument && let Some(index) = buses.main_input {
        copy_stereo_to_bus(&mut buses.buffers.inputs[index], frames);
    }
    if let Some(input) = sidechain
        && Some(input.bus_index) == buses.sidechain_bus
        && let Some(bus) = buses.buffers.inputs.get_mut(input.bus_index as usize)
        && bus.active
    {
        copy_stereo_to_bus(bus, input.frames);
    }
    Ok(())
}

fn parameters(plugin: &Plugin) -> Result<Vec<PluginParameter>, String> {
    plugin
        .get_parameters()
        .map(|parameters| {
            parameters
                .into_iter()
                .map(|parameter| PluginParameter {
                    id: parameter.id,
                    name: parameter.name,
                    unit: parameter.unit,
                    value: parameter.value,
                    steps: parameter.step_count.max(0) as u32,
                    read_only: parameter.is_read_only,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[derive(Debug)]
struct ParameterRamp {
    id: u32,
    current: f64,
    target: f64,
    remaining: usize,
    discrete: bool,
    read_only: bool,
    pending: bool,
}

impl ParameterRamp {
    fn new(parameter: &PluginParameter) -> Self {
        Self {
            id: parameter.id,
            current: parameter.value,
            target: parameter.value,
            remaining: 0,
            discrete: parameter.steps != 0,
            read_only: parameter.read_only,
            pending: false,
        }
    }

    fn set(&mut self, target: f64) {
        if self.read_only || !(0.0..=1.0).contains(&target) || self.target == target {
            return;
        }
        self.target = target;
        self.remaining = if self.discrete { 0 } else { RAMP_FRAMES };
        self.pending = true;
    }

    fn snap(&mut self, value: f64) {
        self.current = value;
        self.target = value;
        self.remaining = 0;
        self.pending = false;
    }

    fn automate(&mut self, value: f64) {
        if self.read_only || !(0.0..=1.0).contains(&value) {
            return;
        }
        if self.current != value || self.target != value || self.pending {
            self.snap(value);
            self.pending = true;
        }
    }

    fn advance(&mut self, frames: usize) -> f64 {
        if self.remaining == 0 || frames >= self.remaining {
            self.current = self.target;
            self.remaining = 0;
        } else {
            self.current += (self.target - self.current) * frames as f64 / self.remaining as f64;
            self.remaining -= frames;
        }
        self.current
    }
}

struct VstProcessor {
    plugin: SharedPlugin,
    ramps: SharedRamps,
    name: String,
    instrument: bool,
    buses: Arc<Mutex<BusState>>,
    latency: Arc<AtomicU32>,
    notes: Vec<(u64, NoteId)>,
    /// Per-channel bend ramps. VST3 receives plain MIDI pitch bend, so ramps are
    /// stepped once per processed chunk.
    bends: [BendRamp; 16],
    errors: Arc<Mutex<Vec<String>>>,
    failed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct BendRamp {
    value: f32,
    target: f32,
    remaining: u32,
    sent: Option<u16>,
}

impl BendRamp {
    fn advance(&mut self, frames: u32) {
        if self.remaining <= frames {
            self.value = self.target;
            self.remaining = 0;
        } else {
            self.value += (self.target - self.value) * frames as f32 / self.remaining as f32;
            self.remaining -= frames;
        }
    }

    /// The 14-bit value to send, or `None` when the plug-in already has it.
    fn pending(&mut self) -> Option<u16> {
        let scaled = if self.value < 0.0 { 8192.0 } else { 8191.0 };
        let value = (8192.0 + self.value.clamp(-1.0, 1.0) * scaled).round() as u16;
        (self.sent != Some(value)).then(|| {
            self.sent = Some(value);
            value
        })
    }
}

impl fmt::Debug for VstProcessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VstProcessor")
            .field("name", &self.name)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl VstProcessor {
    fn send_bends(&mut self, frames: u32) {
        for index in 0..self.bends.len() {
            let Some(value) = self.bends[index].pending() else {
                self.bends[index].advance(frames);
                continue;
            };
            self.bends[index].advance(frames);
            let Some(channel) = MidiChannel::from_index(index as u8) else {
                continue;
            };
            let result = self.plugin.lock().map_err(poisoned).and_then(|mut plugin| {
                plugin
                    .send_midi_event(MidiEvent::PitchBend { channel, value })
                    .map_err(|e| e.to_string())
            });
            if let Err(error) = result {
                self.failed(error);
            }
        }
    }

    fn failed(&mut self, error: impl fmt::Display) {
        if !self.failed {
            if let Ok(mut errors) = self.errors.lock() {
                errors.push(format!("{}: {error}", self.name));
            }
            self.failed = true;
        }
    }

    fn process_chunk(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        context: PluginProcessContext,
    ) -> Result<(), String> {
        let mut plugin = self.plugin.lock().map_err(poisoned)?;
        plugin.set_tempo(context.bpm).map_err(|e| e.to_string())?;
        plugin
            .set_playing(context.playing)
            .map_err(|e| e.to_string())?;
        plugin
            .set_transport_position(TransportPosition {
                samples: context.frame.min(i64::MAX as u64) as i64,
                quarter_notes: context.frame as f64 * context.bpm / (60.0 * f64::from(SAMPLE_RATE)),
            })
            .map_err(|e| e.to_string())?;
        let mut buses = self.buses.lock().map_err(poisoned)?;
        fill_bus_inputs(&mut buses, frames, sidechain, self.instrument)?;
        {
            let mut ramps = self.ramps.lock().map_err(poisoned)?;
            for ramp in ramps.iter_mut().filter(|ramp| ramp.pending) {
                if ramp.discrete {
                    plugin
                        .set_parameter_at(ramp.id, ramp.target, 0)
                        .map_err(|e| e.to_string())?;
                    ramp.snap(ramp.target);
                } else {
                    plugin
                        .set_parameter_at(ramp.id, ramp.current, 0)
                        .map_err(|e| e.to_string())?;
                    let mut offset = 0;
                    while offset < frames.len() && ramp.remaining != 0 {
                        let count = 16.min(frames.len() - offset).min(ramp.remaining);
                        let value = ramp.advance(count);
                        offset += count;
                        plugin
                            .set_parameter_at(ramp.id, value, (offset - 1) as i32)
                            .map_err(|e| e.to_string())?;
                    }
                    ramp.pending = ramp.remaining != 0;
                }
            }
        }
        plugin
            .process_bus_audio(&mut buses.buffers)
            .map_err(|e| e.to_string())?;
        let output = &buses.buffers.outputs[buses.main_output];
        let left = &output.channels[0];
        let right = output.channels.get(1).unwrap_or(left);
        for ((frame, left), right) in frames.iter_mut().zip(left).zip(right) {
            *frame = [*left, *right];
            if frame.iter().any(|sample| !sample.is_finite()) {
                return Err("plugin produced a non-finite audio sample".into());
            }
        }
        Ok(())
    }
}

impl PluginProcessor for VstProcessor {
    fn reset(&mut self) {
        self.notes.clear();
        for bend in &mut self.bends {
            *bend = BendRamp {
                sent: bend.sent,
                ..BendRamp::default()
            };
        }
        let result = self
            .plugin
            .lock()
            .map_err(poisoned)
            .and_then(|mut plugin| plugin.midi_panic().map_err(|e| e.to_string()));
        if let Err(error) = result {
            self.failed(error);
        }
    }

    fn event(&mut self, event: NoteEvent) {
        if self.failed {
            return;
        }
        if let NoteEvent::PitchBend {
            channel,
            value,
            ramp_frames,
        } = event
        {
            if let Some(bend) = self.bends.get_mut(usize::from(channel)) {
                bend.target = value;
                bend.remaining = ramp_frames;
                if ramp_frames == 0 {
                    bend.value = value;
                }
            }
            return;
        }
        let result = self.plugin.lock().map_err(poisoned).and_then(|mut plugin| {
            match event {
                NoteEvent::On {
                    id,
                    key,
                    velocity,
                    channel,
                    ..
                } => {
                    if let Some(index) = self.notes.iter().position(|(old_id, _)| *old_id == id) {
                        let (_, note) = self.notes.swap_remove(index);
                        plugin.note_off(note).map_err(|e| e.to_string())?;
                    }
                    if self.notes.len() == self.notes.capacity() {
                        return Err("too many simultaneous plugin notes".into());
                    }
                    let channel = MidiChannel::from_index(channel)
                        .ok_or_else(|| "invalid MIDI channel".to_owned())?;
                    let note = plugin
                        .note_on(channel, key, velocity)
                        .map_err(|e| e.to_string())?;
                    self.notes.push((id, note));
                }
                NoteEvent::Off { id } => {
                    if let Some(index) = self.notes.iter().position(|(old_id, _)| *old_id == id) {
                        let (_, note) = self.notes.swap_remove(index);
                        plugin.note_off(note).map_err(|e| e.to_string())?;
                    }
                }
                NoteEvent::PitchBend { .. } => {}
            }
            Ok(())
        });
        if let Err(error) = result {
            self.failed(error);
        }
    }

    fn process(&mut self, frames: &mut [Frame], context: PluginProcessContext) {
        self.process_with_sidechain(frames, None, context);
    }

    fn latency_samples(&self) -> u32 {
        self.latency.load(Ordering::Acquire)
    }

    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        context: PluginProcessContext,
    ) {
        if sidechain.is_some_and(|input| input.frames.len() != frames.len()) {
            self.failed("sidechain input must match the main audio block length");
            frames.fill([0.0; 2]);
            return;
        }
        for (index, chunk) in frames.chunks_mut(BLOCK_SIZE).enumerate() {
            if !self.failed {
                self.send_bends(chunk.len() as u32);
                let mut context = context;
                let offset = index * BLOCK_SIZE;
                if context.playing {
                    context.frame = context.frame.saturating_add(offset as u64);
                }
                let sidechain = sidechain.map(|input| PluginSidechainInput {
                    bus_index: input.bus_index,
                    frames: &input.frames[offset..offset + chunk.len()],
                });
                if let Err(error) = self.process_chunk(chunk, sidechain, context) {
                    self.failed(error);
                    chunk.fill([0.0; 2]);
                }
            } else {
                chunk.fill([0.0; 2]);
            }
        }
    }

    fn set_parameter(&mut self, id: u32, value: f64) {
        if let Ok(mut ramps) = self.ramps.lock()
            && let Some(ramp) = ramps.iter_mut().find(|ramp| ramp.id == id)
        {
            ramp.set(value);
        }
    }

    fn set_automation_parameter(&mut self, id: u32, value: f64) {
        if let Ok(mut ramps) = self.ramps.lock()
            && let Some(ramp) = ramps.iter_mut().find(|ramp| ramp.id == id)
        {
            ramp.automate(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_targets_bypass_manual_ramp_but_still_reach_processor() {
        let mut ramp = ParameterRamp::new(&PluginParameter {
            id: 1,
            name: "Gain".into(),
            unit: String::new(),
            value: 0.5,
            steps: 0,
            read_only: false,
        });
        ramp.set(0.9);
        assert!(ramp.remaining > 0);
        ramp.automate(0.2);
        assert_eq!(ramp.current, 0.2);
        assert_eq!(ramp.target, 0.2);
        assert_eq!(ramp.remaining, 0);
        assert!(ramp.pending);
        ramp.snap(0.2);
        ramp.automate(0.2);
        assert!(!ramp.pending);
        ramp.set(0.7);
        assert!(ramp.remaining > 0);
    }

    #[test]
    fn bounce_control_flush_waits_for_a_busy_lock_and_reports_poison() {
        let value = Arc::new(Mutex::new(1));
        let mut held = value.lock().unwrap();
        assert!(control_lock(&value, false).unwrap().is_none());
        let cloned = Arc::clone(&value);
        let (started, ready) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started.send(()).unwrap();
            *control_lock(&cloned, true).unwrap().unwrap()
        });
        ready.recv().unwrap();
        *held = 2;
        drop(held);
        assert_eq!(waiter.join().unwrap(), 2);

        let broken = Mutex::new(0);
        let _ = std::panic::catch_unwind(|| {
            let _held = broken.lock().unwrap();
            panic!("simulated failed processor");
        });
        assert!(control_lock(&broken, false).is_err());
        assert!(control_lock(&broken, true).is_err());
    }

    #[test]
    fn bounce_control_flush_drains_edits_and_refuses_a_poisoned_registry() {
        let runtime = PluginRuntime::new();
        runtime.pending_edits.lock().unwrap().push((7, 3, 0.5));
        runtime.service_blocking().unwrap();
        assert_eq!(runtime.poll_edits_blocking().unwrap(), [(7, 3, 0.5)]);
        assert!(runtime.poll_edits_blocking().unwrap().is_empty());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = runtime.live.lock().unwrap();
            panic!("simulated failed registry");
        }));
        assert!(runtime.service_blocking().is_err());
        assert!(runtime.poll_edits_blocking().is_err());
    }
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    fn test_buses() -> BusState {
        use vst3_host::audio::AudioBusConfig;
        let layout = AudioBusLayout {
            inputs: vec![
                AudioBusConfig {
                    channel_count: 2,
                    active: true,
                },
                AudioBusConfig {
                    channel_count: 2,
                    active: false,
                },
                AudioBusConfig {
                    channel_count: 1,
                    active: true,
                },
            ],
            outputs: vec![AudioBusConfig {
                channel_count: 2,
                active: true,
            }],
        };
        BusState {
            buffers: BusAudioBuffers::new(&layout, BLOCK_SIZE, f64::from(SAMPLE_RATE)),
            aux_inputs: vec![PluginBusInfo {
                index: 2,
                name: "Detector".into(),
                channels: 1,
            }],
            sidechain_bus: Some(2),
            main_input: Some(0),
            main_output: 0,
        }
    }

    #[test]
    fn bus_conversion_preserves_indices_downmixes_detector_and_clears_unused_inputs() {
        let mut buses = test_buses();
        let main = [[0.2, 0.4]; 17];
        let sidechain = [[0.8, 0.4]; 17];
        fill_bus_inputs(
            &mut buses,
            &main,
            Some(PluginSidechainInput {
                bus_index: 2,
                frames: &sidechain,
            }),
            false,
        )
        .unwrap();
        assert_eq!(buses.buffers.inputs.len(), 3);
        assert_eq!(buses.buffers.inputs[0].channels[0], vec![0.2; 17]);
        assert_eq!(buses.buffers.inputs[0].channels[1], vec![0.4; 17]);
        assert!(
            buses.buffers.inputs[1]
                .channels
                .iter()
                .flatten()
                .all(|sample| *sample == 0.0)
        );
        assert!(
            buses.buffers.inputs[2].channels[0]
                .iter()
                .all(|sample| (*sample - 0.6).abs() < 1e-6)
        );
        let capacity = buses.buffers.inputs[2].channels[0].capacity();
        fill_bus_inputs(&mut buses, &[[0.1; 2]; BLOCK_SIZE], None, false).unwrap();
        assert!(
            buses.buffers.inputs[2].channels[0]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert_eq!(buses.buffers.inputs[2].channels[0].capacity(), capacity);
        fill_bus_inputs(
            &mut buses,
            &main,
            Some(PluginSidechainInput {
                bus_index: 1,
                frames: &sidechain,
            }),
            false,
        )
        .unwrap();
        assert!(
            buses.buffers.inputs[1..]
                .iter()
                .flat_map(|bus| &bus.channels)
                .flatten()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    fn instruments_do_not_receive_the_track_mix_on_their_main_input() {
        let mut buses = test_buses();
        fill_bus_inputs(&mut buses, &[[0.7; 2]; 4], None, true).unwrap();
        assert!(
            buses
                .buffers
                .inputs
                .iter()
                .flat_map(|bus| &bus.channels)
                .flatten()
                .all(|sample| *sample == 0.0)
        );
        assert!(
            fill_bus_inputs(
                &mut buses,
                &[[0.0; 2]; 4],
                Some(PluginSidechainInput {
                    bus_index: 2,
                    frames: &[[0.0; 2]; 3]
                }),
                false
            )
            .is_err()
        );
    }

    fn parameter(steps: u32) -> PluginParameter {
        PluginParameter {
            id: 42,
            name: "Gain".into(),
            unit: String::new(),
            value: 0.0,
            steps,
            read_only: false,
        }
    }

    #[test]
    fn continuous_parameters_ramp_and_retarget_without_jumping() {
        let mut ramp = ParameterRamp::new(&parameter(0));
        ramp.set(1.0);
        assert_eq!(ramp.current, 0.0);
        assert!((ramp.advance(RAMP_FRAMES / 2) - 0.5).abs() < 1e-12);
        ramp.set(0.0);
        assert!((ramp.current - 0.5).abs() < 1e-12);
        assert!((ramp.advance(RAMP_FRAMES / 2) - 0.25).abs() < 1e-12);
        assert_eq!(ramp.advance(RAMP_FRAMES / 2), 0.0);
        assert_eq!(ramp.remaining, 0);
    }

    #[test]
    fn discrete_and_native_edits_do_not_sweep_through_intermediate_choices() {
        let mut ramp = ParameterRamp::new(&parameter(3));
        ramp.set(1.0);
        assert_eq!(ramp.remaining, 0);
        assert_eq!(ramp.advance(1), 1.0);
        ramp.snap(0.25);
        ramp.set(0.25);
        assert!(!ramp.pending);
        assert_eq!(ramp.current, 0.25);
    }

    #[test]
    fn invalid_and_read_only_values_never_reach_a_plugin() {
        let mut ramp = ParameterRamp::new(&parameter(0));
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            ramp.set(value);
            assert!(!ramp.pending);
        }
        ramp.read_only = true;
        ramp.set(1.0);
        assert_eq!(ramp.current, 0.0);
        assert!(!ramp.pending);
    }

    #[test]
    fn gui_preparation_never_loads_an_unprepared_binary_on_the_audio_thread() {
        let runtime = PluginRuntime::new();
        runtime
            .prepare_project(&sympoie::project::Project::new())
            .unwrap();
        let instance = PluginInstance::new(
            PluginDescriptor {
                path: PathBuf::from("does-not-exist.vst3"),
                class_id: "00000000000000000000000000000000".into(),
                name: "Missing".into(),
                vendor: String::new(),
                kind: PluginKind::Effect,
            },
            Vec::new(),
            Vec::new(),
        );
        let error = runtime.load(&instance).unwrap_err();
        assert_eq!(error, "Plugin preparation failed");
        assert!(!runtime.has_instance(0));
    }

    #[test]
    fn offline_runtimes_are_released_by_the_main_runtime_after_the_worker_lets_go() {
        let main = PluginRuntime::new();
        let mut project = sympoie::project::Project::new();
        let track = project.add_midi_track("Synth");
        project
            .set_plugin_instrument(track, Some(BuiltinKind::Oscil.instance()))
            .unwrap();
        let offline = main.prepare_offline(&project).unwrap();
        let watch = Arc::downgrade(&offline);
        let (release, released) = std::sync::mpsc::channel::<()>();
        let worker = thread::spawn(move || {
            let renderer = sympoie::render::Renderer::with_plugin_host(&project, &*offline);
            assert!(renderer.is_ok());
            released.recv().unwrap();
        });
        main.service();
        assert!(watch.upgrade().is_some());
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            watch.upgrade().is_some(),
            "only the main runtime may drop it"
        );
        main.service();
        assert!(watch.upgrade().is_none());

        let mut missing = sympoie::project::Project::new();
        let track = missing.add_midi_track("External");
        missing
            .set_plugin_instrument(
                track,
                Some(PluginInstance::new(
                    PluginDescriptor {
                        path: PathBuf::from("does-not-exist.vst3"),
                        class_id: "00000000000000000000000000000000".into(),
                        name: "Missing".into(),
                        vendor: String::new(),
                        kind: PluginKind::Instrument,
                    },
                    Vec::new(),
                    Vec::new(),
                )),
            )
            .unwrap();
        assert!(main.prepare_offline(&missing).is_err());
        assert!(main.offline.lock().unwrap().is_empty());
    }

    #[test]
    #[ignore = "requires an explicitly selected installed VST3 plugin"]
    fn installed_vst3_renders_and_restores() {
        let path =
            PathBuf::from(std::env::var_os("SYMPOIE_TEST_VST3").expect("set SYMPOIE_TEST_VST3"));
        let info = vst3_host::get_detailed_plugin_info(&path).unwrap().info;
        let descriptor = PluginDescriptor {
            path,
            class_id: info.uid,
            name: info.name,
            vendor: info.vendor,
            kind: if info.has_midi_input && info.audio_inputs == 0
                || info.category.contains("Instrument")
            {
                PluginKind::Instrument
            } else {
                PluginKind::Effect
            },
        };
        let runtime = PluginRuntime::new();
        let instance = runtime.inspect(&descriptor).unwrap();
        let mut project = sympoie::project::Project::new();
        let id = match descriptor.kind {
            PluginKind::Instrument => {
                let track = project.add_midi_track("Test instrument");
                project
                    .set_plugin_instrument(track, Some(instance))
                    .unwrap();
                project
                    .midi_track(track)
                    .unwrap()
                    .plugin_instrument()
                    .unwrap()
                    .id
            }
            PluginKind::Effect => {
                let track = project.add_audio_track("Test effect");
                project.add_effect(track.into(), instance).unwrap()
            }
        };
        runtime.prepare_project(&project).unwrap();
        let mut instance = project.plugin(id).unwrap().clone();
        let (original_handle, _) = runtime.handles(id).unwrap();
        let mut processor = runtime.load(&instance).unwrap();
        if descriptor.kind == PluginKind::Instrument {
            processor.event(NoteEvent::On {
                id: 1,
                key: 60,
                velocity: 100,
                channel: 0,
                slide_frames: 0,
            });
        }
        let mut peak = 0.0_f32;
        for block in 0..100 {
            let mut frames = [[0.0; 2]; 127];
            if descriptor.kind == PluginKind::Effect {
                for (index, frame) in frames.iter_mut().enumerate() {
                    *frame = [((block * 127 + index) as f32 * 0.04).sin() * 0.1; 2];
                }
            }
            processor.process(
                &mut frames,
                PluginProcessContext {
                    frame: (block * 127) as u64,
                    bpm: 120.0,
                    playing: true,
                    quality: sympoie::plugin::RenderQuality::Realtime,
                },
            );
            assert!(frames.iter().flatten().all(|sample| sample.is_finite()));
            peak = frames
                .iter()
                .flatten()
                .map(|sample| sample.abs())
                .fold(peak, f32::max);
        }
        assert!(peak > 1e-6, "plugin produced no audio");
        assert!(runtime.take_errors().is_empty());
        if let Some(parameter) = instance.parameters.iter().find(|parameter| {
            !parameter.read_only
                && parameter.steps == 0
                && parameter.name.to_lowercase().contains("gain")
        }) {
            let id = parameter.id;
            let target = (parameter.value * 0.9).max(0.001);
            project
                .set_plugin_parameter(instance.id, id, target)
                .unwrap();
            runtime.prepare_project(&project).unwrap();
            processor = std::thread::spawn(move || {
                processor.set_parameter(id, target);
                let mut frames = [[0.1; 2]; RAMP_FRAMES + 17];
                processor.process(
                    &mut frames,
                    PluginProcessContext {
                        frame: 12_700,
                        bpm: 120.0,
                        playing: true,
                        quality: sympoie::plugin::RenderQuality::Realtime,
                    },
                );
                assert!(frames.iter().flatten().all(|sample| sample.is_finite()));
                processor
            })
            .join()
            .unwrap();
            runtime.service();
            let current = original_handle.lock().unwrap().get_parameter(id).unwrap();
            assert!(
                (current - target).abs() < 1e-6,
                "the controller must catch up with its audio processor"
            );
        }
        runtime.service();
        let (parameters, state) = runtime.snapshot(id).unwrap();
        println!(
            "{}: peak {peak:.6}, {} parameters, {} state bytes",
            descriptor.name,
            parameters.len(),
            state.len()
        );
        instance.parameters = parameters;
        instance.state = state;
        project.add_audio_track("Unrelated structural edit");
        runtime.prepare_project(&project).unwrap();
        let (after_save, _) = runtime.handles(id).unwrap();
        assert!(
            Arc::ptr_eq(&original_handle, &after_save),
            "saving and editing must retain the processor"
        );
        drop(processor);
        let old_lease = runtime.pin_project(&project);
        let old_weak = Arc::downgrade(&original_handle);
        runtime.clear();
        runtime.prepare_project(&project).unwrap();
        let (new_handle, _) = runtime.handles(id).unwrap();
        assert!(!Arc::ptr_eq(&original_handle, &new_handle));
        assert!(Arc::ptr_eq(
            &old_lease.instances[&id].plugin,
            &original_handle
        ));
        let old_processor = old_lease.load(project.plugin(id).unwrap()).unwrap();
        drop(old_processor);
        drop(original_handle);
        drop(after_save);
        runtime.service();
        assert!(
            old_weak.upgrade().is_some(),
            "queued lease keeps the retired plugin alive"
        );
        drop(old_lease);
        runtime.service();
        assert!(
            old_weak.upgrade().is_none(),
            "the control thread releases retired instances after their last lease"
        );
        let restored_runtime = PluginRuntime::new();
        let mut restored = restored_runtime.load(&instance).unwrap();
        restored.reset();
        assert!(restored_runtime.take_errors().is_empty());
    }

    #[test]
    #[ignore = "requires an explicitly selected installed VST3 instrument"]
    fn installed_vst3_keyboard_works_while_transport_stopped() {
        use crate::audio::AudioEngine;
        use sympoie::transport::PlaybackState;

        let path =
            PathBuf::from(std::env::var_os("SYMPOIE_TEST_VST3").expect("set SYMPOIE_TEST_VST3"));
        let info = vst3_host::get_detailed_plugin_info(&path).unwrap().info;
        assert!(
            info.has_midi_input && info.audio_inputs == 0 || info.category.contains("Instrument"),
            "select an instrument with SYMPOIE_TEST_VST3"
        );
        let descriptor = PluginDescriptor {
            path,
            class_id: info.uid,
            name: info.name,
            vendor: info.vendor,
            kind: PluginKind::Instrument,
        };
        let runtime = PluginRuntime::new();
        let instance = runtime.inspect(&descriptor).unwrap();
        let mut project = sympoie::project::Project::new();
        let track = project.add_midi_track("Empty live instrument track");
        project
            .set_plugin_instrument(track, Some(instance))
            .unwrap();
        let id = project
            .midi_track(track)
            .unwrap()
            .plugin_instrument()
            .unwrap()
            .id;
        runtime.prepare_project(&project).unwrap();
        let (plugin, _) = runtime.handles(id).unwrap();
        let mut engine = AudioEngine::with_plugins(project, Arc::clone(&runtime));
        let shared = engine.shared();
        let mut frames = [[0.0; 2]; 256];

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime.service();
            let status = engine.status();
            assert!(status.render_error.is_none(), "{status:?}");
            if shared.consume(&mut frames) > 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "stopped engine never processed audio"
            );
            thread::sleep(Duration::from_millis(2));
        }

        let note = plugin
            .lock()
            .unwrap()
            .note_on(MidiChannel::Ch1, 60, 100)
            .unwrap();
        let mut consumed = 0;
        let mut peak = 0.0_f32;
        let mut late_peak = 0.0_f32;
        while consumed < 24_000 {
            runtime.service();
            let count = shared.consume(&mut frames);
            assert!(frames.iter().flatten().all(|sample| sample.is_finite()));
            let block_peak = frames[..count]
                .iter()
                .flatten()
                .map(|sample| sample.abs())
                .fold(0.0_f32, f32::max);
            peak = peak.max(block_peak);
            if consumed >= 12_000 {
                late_peak = late_peak.max(block_peak);
            }
            consumed += count;
            let status = engine.status();
            assert_eq!(status.state, PlaybackState::Stopped);
            assert_eq!(status.position_frame, 0);
            assert!(status.render_error.is_none(), "{status:?}");
            assert!(
                Instant::now() < deadline,
                "stopped processing stalled after {consumed} frames"
            );
            thread::sleep(Duration::from_millis(2));
        }
        plugin.lock().unwrap().note_off(note).unwrap();
        assert!(peak > 1e-5, "native keyboard note produced no audio");
        assert!(
            late_peak > 1e-5,
            "held keyboard note was reset during stopped processing"
        );
        assert!(runtime.take_errors().is_empty());
        println!(
            "{}: stopped keyboard rendered {consumed} frames, peak {peak:.6}, late peak {late_peak:.6}, cursor 0",
            descriptor.name
        );
        engine.shutdown();
        drop(engine);
        drop(plugin);
        runtime.clear();
        runtime.service();
    }
}
