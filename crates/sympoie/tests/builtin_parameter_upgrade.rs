use sympoie::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use sympoie::builtin::{self, BuiltinKind};
use sympoie::clip::{MidiClip, MidiNote};
use sympoie::instrument::NoteEvent;
use sympoie::plugin::{PluginInstance, PluginKind, PluginParameter, PluginProcessContext};
use sympoie::project::Project;
use sympoie::project_file::{read_project, write_project};
use sympoie::render::Renderer;

const MATRIX_START: u32 = 200;
const MATRIX_END: u32 = 218;

fn is_matrix(id: u32) -> bool {
    (MATRIX_START..MATRIX_END).contains(&id)
}

fn legacy_instance() -> PluginInstance {
    let mut instance = BuiltinKind::Oscil.instance();
    instance.parameters.retain(|p| !is_matrix(p.id));
    for (id, value) in [(23, 0.64), (41, 0.2), (42, 0.125)] {
        instance
            .parameters
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap()
            .value = value;
    }
    instance.parameters.push(PluginParameter {
        id: 900,
        name: "Future parameter".into(),
        unit: "future".into(),
        value: 0.123,
        steps: 0,
        read_only: false,
    });
    instance.parameters.reverse();
    instance.state = vec![7, 8, 9];
    instance
}

fn project_with_instrument(instance: PluginInstance) -> (Project, u64) {
    let mut project = Project::new();
    project.set_bpm(3000.0).unwrap();
    let track = project.add_midi_track("Matrix compatibility");
    project
        .set_plugin_instrument(track, Some(instance))
        .unwrap();
    let id = project
        .midi_track(track)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    project
        .add_midi_clip(
            track,
            MidiClip {
                start_tick: 0,
                offset_ticks: 0,
                length_ticks: 4096,
                notes: vec![MidiNote {
                    start_tick: 0,
                    duration_ticks: 3072,
                    key: 60,
                    velocity: 100,
                    channel: 0,
                }],
            },
        )
        .unwrap();
    (project, id)
}

fn lane(id: u64, parameter: u32, values: [f64; 2]) -> AutomationLane {
    AutomationLane::new(
        AutomationTarget::PluginParameter {
            plugin: id,
            parameter,
        },
        vec![
            AutomationPoint {
                tick: 0,
                value: values[0],
            },
            AutomationPoint {
                tick: 2048,
                value: values[1],
            },
        ],
    )
    .unwrap()
}

fn bytes(project: &Project) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(project, &mut bytes).unwrap();
    bytes
}

fn plugin_bytes(instance: &PluginInstance) -> Vec<u8> {
    fn string(out: &mut Vec<u8>, value: &str) {
        out.extend((value.len() as u64).to_le_bytes());
        out.extend(value.as_bytes());
    }
    let mut out = instance.id.to_le_bytes().to_vec();
    string(&mut out, instance.descriptor.path.to_str().unwrap());
    string(&mut out, &instance.descriptor.class_id);
    string(&mut out, &instance.descriptor.name);
    string(&mut out, &instance.descriptor.vendor);
    out.push(match instance.descriptor.kind {
        PluginKind::Instrument => 0,
        PluginKind::Effect => 1,
    });
    out.push(u8::from(instance.bypassed));
    out.extend((instance.parameters.len() as u64).to_le_bytes());
    for p in &instance.parameters {
        out.extend(p.id.to_le_bytes());
        string(&mut out, &p.name);
        string(&mut out, &p.unit);
        out.extend(p.value.to_le_bytes());
        out.extend(p.steps.to_le_bytes());
        out.push(u8::from(p.read_only));
    }
    out.extend((instance.state.len() as u64).to_le_bytes());
    out.extend(&instance.state);
    out
}

fn old_file(project: &Project, id: u64) -> Vec<u8> {
    let current = project.plugin(id).unwrap();
    let mut old = current.clone();
    old.parameters.retain(|p| !is_matrix(p.id));
    let current_chunk = plugin_bytes(current);
    let mut encoded = bytes(project);
    let matches: Vec<usize> = encoded
        .windows(current_chunk.len())
        .enumerate()
        .filter_map(|(i, chunk)| (chunk == current_chunk).then_some(i))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "plugin wire encoding must match exactly once"
    );
    let at = matches[0];
    encoded.splice(at..at + current_chunk.len(), plugin_bytes(&old));
    encoded
}

#[test]
fn old_files_gain_editable_matrix_routes_without_changing_legacy_automation() {
    let original = legacy_instance();
    let (mut project, id) = project_with_instrument(original.clone());
    let track = project.track_ids().nth(1).unwrap();
    for (parameter, values) in [(23, [0.64, 0.4]), (41, [0.2, 0.3]), (42, [0.125, 0.25])] {
        project
            .set_track_automation(track, lane(id, parameter, values))
            .unwrap();
    }
    let legacy_bytes = old_file(&project, id);
    let mut restored = read_project(legacy_bytes.as_slice()).unwrap();
    let plugin = restored.plugin(id).unwrap();
    assert_eq!(
        &plugin.parameters[..original.parameters.len()],
        original.parameters.as_slice()
    );
    assert_eq!(plugin.id, id);
    assert_eq!(plugin.state, original.state);
    assert_eq!(plugin.descriptor, original.descriptor);
    assert_eq!(
        restored.tracks()[1].automation(),
        project.tracks()[1].automation()
    );
    for row in 0..6 {
        assert_eq!(builtin::value(plugin, MATRIX_START + row * 3), 0.0);
        assert_eq!(builtin::value(plugin, MATRIX_START + row * 3 + 2), 0.0);
    }
    for (parameter, value) in [(200, 1.0), (201, 14.0 / 16.0), (202, 0.8)] {
        restored.set_plugin_parameter(id, parameter, value).unwrap();
    }
    restored
        .set_track_automation(track, lane(id, 202, [0.5, 0.8]))
        .unwrap();
    let once = bytes(&restored);
    assert_ne!(legacy_bytes, once);
    let twice = read_project(once.as_slice()).unwrap();
    assert_eq!(bytes(&twice), once);
    assert_eq!(twice.plugin(id), restored.plugin(id));
    assert_eq!(
        twice.tracks()[1].automation(),
        restored.tracks()[1].automation()
    );
}

#[test]
fn default_off_matrix_preserves_the_sound_of_legacy_modulation() {
    let old = legacy_instance();
    let mut upgraded = old.clone();
    assert!(builtin::complete_parameters(&mut upgraded).unwrap());
    assert!(!builtin::complete_parameters(&mut upgraded).unwrap());
    let mut a = builtin::create_processor(&old).unwrap();
    let mut b = builtin::create_processor(&upgraded).unwrap();
    let on = NoteEvent::On {
        id: 1,
        key: 64,
        velocity: 110,
        channel: 0,
    };
    a.event(on);
    b.event(on);
    let context = PluginProcessContext {
        frame: 0,
        bpm: 120.0,
        playing: true,
    };
    let mut original = vec![[0.0; 2]; 4096];
    let mut migrated = original.clone();
    a.process(&mut original, context);
    b.process(&mut migrated, context);
    assert_eq!(original, migrated);
    assert!(original.iter().flatten().any(|sample| sample.abs() > 0.01));
    a.event(NoteEvent::Off { id: 1 });
    b.event(NoteEvent::Off { id: 1 });
    a.process(&mut original, context);
    b.process(&mut migrated, context);
    assert_eq!(original, migrated);
}

#[test]
fn new_matrix_automation_survives_loading_and_render_partitioning() {
    let (mut project, id) = project_with_instrument(BuiltinKind::Oscil.instance());
    let track = project.track_ids().nth(1).unwrap();
    project.set_plugin_parameter(id, 200, 1.0).unwrap();
    project.set_plugin_parameter(id, 201, 14.0 / 16.0).unwrap();
    project
        .set_track_automation(track, lane(id, 202, [0.5, 0.9]))
        .unwrap();
    let restored = read_project(bytes(&project).as_slice()).unwrap();
    let mut whole = vec![[0.0; 2]; 8192];
    Renderer::new(&restored)
        .unwrap()
        .render_block(0, &mut whole);
    let mut split = whole.clone();
    let mut renderer = Renderer::new(&restored).unwrap();
    for (chunk, frames) in split.chunks_mut(127).enumerate() {
        renderer.render_block((chunk * 127) as u64, frames);
    }
    assert_eq!(whole, split);
    assert!(whole.iter().flatten().all(|sample| sample.is_finite()));
    let mut unmodulated = restored.clone();
    unmodulated.set_plugin_parameter(id, 200, 0.0).unwrap();
    let mut dry = vec![[0.0; 2]; 8192];
    Renderer::new(&unmodulated)
        .unwrap()
        .render_block(0, &mut dry);
    let maximum_change = whole
        .iter()
        .zip(dry)
        .map(|(a, b)| (a[0] - b[0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        maximum_change > 0.001,
        "matrix must change rendered audio; largest difference: {maximum_change}"
    );
}

#[test]
fn hydration_precedes_automation_loading_and_snapshot_pruning() {
    let (mut project, id) = project_with_instrument(BuiltinKind::Oscil.instance());
    let track = project.track_ids().nth(1).unwrap();
    project
        .set_track_automation(track, lane(id, 202, [0.5, 0.9]))
        .unwrap();
    let restored = read_project(old_file(&project, id).as_slice()).unwrap();
    assert_eq!(
        restored.tracks()[1].automation(),
        project.tracks()[1].automation()
    );
    let mut old_snapshot = project.plugin(id).unwrap().parameters.clone();
    old_snapshot.retain(|p| !is_matrix(p.id));
    project
        .set_plugin_snapshot(id, old_snapshot, vec![])
        .unwrap();
    assert_eq!(
        project.tracks()[1].automation(),
        restored.tracks()[1].automation()
    );
    project.set_plugin_parameter(id, 202, 0.75).unwrap();
}

#[test]
fn external_devices_are_not_changed_by_native_schema_completion() {
    let mut external = legacy_instance();
    external.descriptor.path = "/missing-oscillator.vst3".into();
    external.descriptor.class_id = "external-oscillator".into();
    let original = external.clone();
    assert!(!builtin::complete_parameters(&mut external).unwrap());
    assert_eq!(external, original);
    let (project, id) = project_with_instrument(external);
    assert_eq!(project.plugin(id).unwrap().parameters, original.parameters);
    let loaded = read_project(bytes(&project).as_slice()).unwrap();
    assert_eq!(loaded.plugin(id), project.plugin(id));
    assert!(
        loaded
            .plugin(id)
            .unwrap()
            .parameters
            .iter()
            .all(|p| !is_matrix(p.id))
    );
}
