use super::*;
use sympoie::builtin::{self, BuiltinKind};

fn load(harness: &mut Harness, track: usize, kind: BuiltinKind) -> u64 {
    harness.dispatch(Action::LoadVst3 {
        track,
        plugin: kind.descriptor(),
    });
    harness.steps(3);
    let target = &harness.project().tracks()[track];
    match kind {
        BuiltinKind::Oscil | BuiltinKind::Sampler => {
            let Track::Midi(midi) = target else {
                panic!("MIDI destination required")
            };
            midi.plugin_instrument().expect("built-in loaded").id
        }
        _ => target.effects().last().expect("built-in effect loaded").id,
    }
}

#[test]
fn builtin_loading_editing_bypass_history_and_replacement_use_normal_app_flow() {
    let mut harness = Harness::with_empty_project();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    let id = load(&mut harness, 1, BuiltinKind::Oscil);
    assert!(!harness.app.vst3_editors_for_tests().contains(&id));
    assert!(harness.app.showing_devices_for_tests());
    let original = harness.project().plugin(id).unwrap().clone();
    harness.dispatch(Action::SetVst3Parameter {
        id,
        parameter: 20,
        value: 0.2,
    });
    assert_eq!(
        harness
            .project()
            .plugin(id)
            .unwrap()
            .parameters
            .iter()
            .find(|p| p.id == 20)
            .unwrap()
            .value,
        0.2
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().plugin(id), Some(&original));
    harness.dispatch(Action::Redo);
    let edited = harness.project().plugin(id).unwrap().clone();
    harness.dispatch(Action::ResetBuiltinParameters { id });
    assert_eq!(
        harness.project().plugin(id).unwrap().parameters,
        original.parameters
    );
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().plugin(id), Some(&edited));
    harness.dispatch(Action::SetVst3Bypassed { id, bypassed: true });
    assert!(harness.project().plugin(id).unwrap().bypassed);
    harness.dispatch(Action::Undo);
    assert!(!harness.project().plugin(id).unwrap().bypassed);
    load(&mut harness, 1, BuiltinKind::Eq);
    load(&mut harness, 1, BuiltinKind::Comp);
    let sampler = load(&mut harness, 1, BuiltinKind::Sampler);
    assert!(harness.project().plugin(id).is_none());
    assert_eq!(harness.project().tracks()[1].effects().len(), 2);
    assert_eq!(
        BuiltinKind::from_descriptor(&harness.project().plugin(sampler).unwrap().descriptor),
        Some(BuiltinKind::Sampler)
    );
    harness.dispatch(Action::Undo);
    assert!(harness.project().plugin(id).is_some());
    assert!(harness.project().plugin(sampler).is_none());
}

#[test]
fn builtin_effects_load_on_main_and_mixer_and_comp_exposes_sidechain() {
    let runtime = crate::plugins::PluginRuntime::new();
    let mut project = Project::new();
    let id = project
        .add_effect(
            project.master_track_id().into(),
            BuiltinKind::Comp.instance(),
        )
        .unwrap();
    runtime.prepare_project(&project).unwrap();
    assert!(runtime.builtin_telemetry(id).is_some());
    assert_eq!(runtime.sidechain_inputs(id)[0].index, 1);
    assert_eq!(runtime.latency_samples(id), Some(0));
    let lease = runtime.pin_project(&project);
    let mut render = sympoie::render::Renderer::with_plugin_host(&project, &lease).unwrap();
    render.render_block(0, &mut [[0.0; 2]; 256]);

    let mut harness = Harness::with_empty_project();
    load(&mut harness, 0, BuiltinKind::Eq);
    harness.dispatch(Action::LoadMixerVst3 {
        track: 0,
        plugin: BuiltinKind::Comp.descriptor(),
        before: None,
    });
    harness.steps(3);
    assert_eq!(harness.project().tracks()[0].effects().len(), 1);
    assert_eq!(harness.project().tracks()[0].mixer_effects().len(), 1);
    assert_eq!(harness.app.detail_view_for_tests(), "mixer");
    assert!(harness.app.vst3_editors_for_tests().is_empty());
}

#[test]
fn sample_is_embedded_and_survives_save_without_source_file() {
    let mut harness = Harness::with_empty_project();
    let sample_path = harness._files.0.join("sample.wav");
    write_wav(&sample_path, &vec![[0.25, -0.25]; 4800]);
    let mut sampler = BuiltinKind::Sampler.instance();
    sampler.state = builtin::instruments::sample_state_from_wav(&sample_path).unwrap();
    let mut project = Project::new();
    let track = project.add_midi_track("Sampler");
    project.set_plugin_instrument(track, Some(sampler)).unwrap();
    let path = harness._files.0.join("embedded.symp");
    sympoie::project_file::save_project_directory(&project, &path).unwrap();
    std::fs::remove_file(sample_path).unwrap();
    harness.dispatch(Action::OpenPath(path));
    let Track::Midi(midi) = &harness.project().tracks()[1] else {
        panic!()
    };
    let sample = midi.plugin_instrument().unwrap();
    assert_eq!(
        builtin::instruments::sample_preview(&sample.state)
            .unwrap()
            .frames,
        4800
    );
    sympoie::render::Renderer::new(harness.project()).unwrap();
}

#[test]
fn builtin_meter_registry_does_not_follow_reused_external_device_ids() {
    use std::sync::Arc;
    let runtime = crate::plugins::PluginRuntime::new();
    let mut original = Project::new();
    let id = original
        .add_effect(
            original.master_track_id().into(),
            BuiltinKind::Comp.instance(),
        )
        .unwrap();
    runtime.prepare_project(&original).unwrap();
    let old = runtime.builtin_telemetry(id).unwrap();
    runtime.prepare_project(&original).unwrap();
    assert!(Arc::ptr_eq(&old, &runtime.builtin_telemetry(id).unwrap()));
    let mut replacement = Project::new();
    let external = replacement
        .add_effect(
            replacement.master_track_id().into(),
            fake_vst3(PluginKind::Effect),
        )
        .unwrap();
    assert_eq!(external, id);
    assert!(runtime.prepare_project(&replacement).is_err());
    assert!(runtime.builtin_telemetry(id).is_none());
    assert!(runtime.sidechain_inputs(id).is_empty());
    assert_eq!(runtime.latency_samples(id), None);
    runtime.prepare_project(&original).unwrap();
    assert!(!Arc::ptr_eq(&old, &runtime.builtin_telemetry(id).unwrap()));
    assert!(
        !runtime.has_instance(id),
        "built-in state remains document-owned"
    );
}

#[test]
fn eq_node_axes_share_one_undo_step() {
    let mut harness = Harness::with_empty_project();
    let id = load(&mut harness, 0, BuiltinKind::Eq);
    let original = harness.project().plugin(id).unwrap().clone();
    for (parameter, value) in [(12, 0.4), (13, 0.7), (12, 0.5), (13, 0.8)] {
        harness.app.apply_for_tests(
            &harness.ctx,
            Action::SetVst3Parameter {
                id,
                parameter,
                value,
            },
        );
    }
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().plugin(id), Some(&original));
}

#[test]
fn modulation_matrix_controls_support_edit_history_and_reset() {
    let mut harness = Harness::with_empty_project();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    let id = load(&mut harness, 1, BuiltinKind::Oscil);
    let original = harness.project().plugin(id).unwrap().clone();
    for (parameter, value) in [(200, 1.0), (201, 14.0 / 16.0), (202, 0.85)] {
        harness.dispatch(Action::SetVst3Parameter {
            id,
            parameter,
            value,
        });
        assert_eq!(
            harness
                .project()
                .plugin(id)
                .unwrap()
                .parameters
                .iter()
                .find(|p| p.id == parameter)
                .unwrap()
                .value,
            value
        );
    }
    let edited = harness.project().plugin(id).unwrap().clone();
    harness.dispatch(Action::Undo);
    assert_eq!(
        builtin::value(harness.project().plugin(id).unwrap(), 202),
        0.0
    );
    harness.dispatch(Action::Redo);
    assert_eq!(harness.project().plugin(id), Some(&edited));
    harness.dispatch(Action::ResetBuiltinParameters { id });
    assert_eq!(harness.project().plugin(id), Some(&original));
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().plugin(id), Some(&edited));
    assert!(harness.app.vst3_editors_for_tests().is_empty());
}
