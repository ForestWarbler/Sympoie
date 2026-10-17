use super::*;
use sympoie::track::{AudioOutput, TrackId};

fn mixer_fixture() -> Harness {
    let mut harness = Harness::with_vst3_devices();
    let mut project = harness.project().clone();
    project.add_bus_track("Mix Bus");
    let ids: Vec<_> = project.track_ids().collect();
    for track in ids {
        project
            .add_mixer_effect(track, fake_vst3(PluginKind::Effect))
            .unwrap();
    }
    let midi = project.track_ids().nth(1).unwrap();
    project
        .add_mixer_effect(midi, fake_vst3(PluginKind::Effect))
        .unwrap();
    let path = harness._files.0.join("mixer.symp");
    sympoie::project_file::save_project_directory(&project, &path).unwrap();
    harness.dispatch(Action::OpenPath(path));
    harness
}

#[test]
fn buses_stay_out_of_arrangement_and_route_without_removing_source_tracks() {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::ShowMixer);
    harness.click(harness.rect(Id::new("mixer-add-bus")).center());
    assert_eq!(harness.app.detail_view_for_tests(), "mixer");
    let bus_index = harness.project().tracks().len() - 1;
    let TrackId::Bus(bus) = harness.project().track_ids().nth(bus_index).unwrap() else {
        panic!("expected a bus");
    };
    assert!(
        harness
            .ctx
            .read_response(Id::new(("header", bus_index)))
            .is_none()
    );
    assert!(
        harness
            .ctx
            .read_response(Id::new(("mixer-strip", bus_index)))
            .is_some()
    );
    for track in [1, 2] {
        let mixer = harness.project().tracks()[track]
            .mixer()
            .with_output(AudioOutput::Bus(bus));
        harness.dispatch(Action::SetMixer { track, mixer });
        assert!(
            harness
                .ctx
                .read_response(Id::new(("header", track)))
                .is_some()
        );
        assert!(
            harness
                .ctx
                .read_response(Id::new(("clip", track, 0usize)))
                .is_some()
        );
    }
    harness.dispatch(Action::RenameTrack(bus_index, "Drum Mix".into()));
    assert_eq!(harness.project().tracks()[bus_index].name(), "Drum Mix");
    harness.dispatch(Action::DeleteTrack(bus_index));
    assert_eq!(harness.project().tracks().len(), 3);
    for track in [1, 2] {
        assert_eq!(
            harness.project().tracks()[track].mixer().output(),
            AudioOutput::Master
        );
        assert!(
            matches!(&harness.project().tracks()[track], Track::Audio(audio) if audio.clips().len() == 1)
        );
    }
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().tracks()[bus_index].name(), "Drum Mix");
    assert_eq!(
        harness.project().tracks()[1].mixer().output(),
        AudioOutput::Bus(bus)
    );
}

#[test]
fn mixer_inserts_are_independent_of_device_cards_and_keep_history_and_saved_state() {
    let mut harness = mixer_fixture();
    let device_chain = harness.project().tracks()[1].effects().to_vec();
    let mixer_chain = harness.project().tracks()[1].mixer_effects().to_vec();
    harness.dispatch(Action::SelectTracks(BTreeSet::from([1])));
    harness.dispatch(Action::ShowDevices);
    for plugin in &device_chain {
        assert!(
            harness
                .ctx
                .read_response(Id::new(("device-vst3", plugin.id as usize)))
                .is_some()
        );
    }
    for plugin in &mixer_chain {
        assert!(
            harness
                .ctx
                .read_response(Id::new(("device-vst3", plugin.id as usize)))
                .is_none()
        );
    }
    harness.dispatch(Action::ShowMixer);
    for plugin in &mixer_chain {
        assert!(
            harness
                .ctx
                .read_response(Id::new(("mixer-effect", plugin.id)))
                .is_some()
        );
    }
    for plugin in &device_chain {
        assert!(
            harness
                .ctx
                .read_response(Id::new(("mixer-effect", plugin.id)))
                .is_none()
        );
    }
    harness.dispatch(Action::MoveMixerEffect {
        track: 1,
        from: 0,
        to: 1,
    });
    assert_eq!(
        harness.project().tracks()[1].mixer_effects()[1].id,
        mixer_chain[0].id
    );
    assert_eq!(harness.project().tracks()[1].effects(), device_chain);
    harness.dispatch(Action::Undo);
    assert_eq!(harness.project().tracks()[1].mixer_effects(), mixer_chain);
    let id = mixer_chain[0].id;
    harness.click(harness.rect(Id::new(("mixer-effect-bypass", id))).center());
    harness.dispatch(Action::SetVst3Parameter {
        id,
        parameter: 37,
        value: 0.8,
    });
    assert!(harness.project().plugin(id).unwrap().bypassed);
    assert_eq!(harness.plugin_parameter(id), 0.8);
    harness.dispatch(Action::RemoveVst3 { track: 1, id });
    assert!(harness.project().plugin(id).is_none());
    assert_eq!(harness.project().tracks()[1].effects(), device_chain);
    harness.dispatch(Action::Undo);
    assert!(harness.project().plugin(id).unwrap().bypassed);
    assert_eq!(harness.plugin_parameter(id), 0.8);
    let path = harness._files.0.join("mixed-saved.symp");
    sympoie::project_file::save_project_directory(harness.project(), &path).unwrap();
    let reopened = Project::load(&path).unwrap();
    assert_eq!(reopened.tracks()[1].effects(), device_chain);
    assert_eq!(
        reopened.tracks()[1].mixer_effects(),
        harness.project().tracks()[1].mixer_effects()
    );
    assert!(matches!(reopened.tracks().last(), Some(Track::Bus(_))));
}

#[test]
fn bus_device_view_points_to_mixer_and_its_inserts_open_parameter_editors() {
    let mut harness = mixer_fixture();
    let bus = harness.project().tracks().len() - 1;
    let id = harness.project().tracks()[bus].mixer_effects()[0].id;
    harness.dispatch(Action::SelectTracks(BTreeSet::from([bus])));
    harness.dispatch(Action::ShowDevices);
    assert!(harness.labelled("Open Mixer").is_positive());
    assert!(
        harness
            .ctx
            .read_response(Id::new(("device-chain", bus)))
            .is_none()
    );
    harness.dispatch(Action::OpenVst3Editor(id));
    assert!(harness.app.vst3_editors_for_tests().contains(&id));
    let open_mixer = harness.labelled("Open Mixer").center();
    harness.click(open_mixer);
    assert_eq!(harness.app.detail_view_for_tests(), "mixer");
    assert!(
        harness
            .ctx
            .read_response(Id::new(("mixer-effect", id)))
            .is_some()
    );
}
