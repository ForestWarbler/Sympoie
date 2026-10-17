use super::*;

fn close_rack(harness: &mut Harness) {
    let close = harness.rect(Id::new("rack-close"));
    harness.click(close.center());
    harness.steps(2);
    harness.assert_visible(Id::new("rack-close"), false);
}

#[test]
fn rack_header_close_restores_arrangement_width_without_editing_devices() {
    let mut harness = Harness::with_vst3_devices();
    harness.select_track(1);
    let selection = harness.app.selection_for_tests().clone();
    let revision = harness.app.document_for_tests().revision();
    let effects = harness.project().tracks()[1].effects().to_vec();
    let Track::Midi(midi) = &harness.project().tracks()[1] else {
        panic!("expected the instrument track");
    };
    let instrument = midi.plugin_instrument().cloned();
    let width_with_rack = harness.rect(Id::new(("lane", 1))).width();
    harness.assert_visible(Id::new(("device-chain", 1)), true);

    close_rack(&mut harness);
    harness.assert_visible(Id::new(("device-chain", 1)), false);
    harness.assert_visible(Id::new("browser-search"), true);
    let width_without_rack = harness.rect(Id::new(("lane", 1))).width();
    assert!(
        width_without_rack - width_with_rack >= crate::views::devices::RACK_WIDTH - 2.0,
        "closing the Rack must return its column to the arrangement"
    );
    assert_eq!(harness.app.selection_for_tests(), &selection);
    assert_eq!(harness.project().tracks()[1].effects(), effects);
    let Track::Midi(midi) = &harness.project().tracks()[1] else {
        panic!("expected the instrument track");
    };
    assert_eq!(midi.plugin_instrument(), instrument.as_ref());
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn rack_status_control_shortcut_and_track_double_click_restore_the_chain() {
    let mut harness = Harness::with_vst3_devices();
    harness.select_track(2);
    let revision = harness.app.document_for_tests().revision();
    close_rack(&mut harness);

    let toggle = harness.rect(Id::new("status-rack-toggle"));
    harness.click(toggle.center());
    harness.steps(2);
    harness.assert_visible(Id::new(("device-chain", 2)), true);
    assert_eq!(
        harness.app.selection_for_tests().tracks,
        BTreeSet::from([2])
    );
    for visible in [false, true] {
        harness.key(Key::R, Modifiers::COMMAND | Modifiers::ALT);
        harness.steps(2);
        harness.assert_visible(Id::new("rack-close"), visible);
        harness.assert_visible(Id::new(("device-chain", 2)), visible);
        assert_eq!(
            harness.app.selection_for_tests().tracks,
            BTreeSet::from([2])
        );
    }

    close_rack(&mut harness);
    harness.dispatch(Action::ShowMixer);
    harness.open_track_devices(1);
    harness.assert_visible(Id::new("rack-close"), true);
    harness.assert_visible(Id::new(("device-chain", 1)), true);
    assert_eq!(harness.app.detail_view_for_tests(), "devices");
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn rack_can_close_with_no_selection_multiple_tracks_or_a_bus() {
    let mut harness = Harness::with_vst3_devices();
    let mut project = harness.project().clone();
    project.add_bus_track("Bus");
    let bus_index = project.tracks().len() - 1;
    let path = harness._files.0.join("rack-bus.symp");
    sympoie::project_file::save_project_directory(&project, &path).unwrap();
    harness.dispatch(Action::OpenPath(path));
    let revision = harness.app.document_for_tests().revision();

    for tracks in [
        BTreeSet::new(),
        BTreeSet::from([1, 2]),
        BTreeSet::from([bus_index]),
    ] {
        harness.dispatch(Action::SelectTracks(tracks.clone()));
        harness.dispatch(Action::ShowDevices);
        harness.assert_visible(Id::new("rack-close"), true);
        close_rack(&mut harness);
        assert_eq!(harness.app.selection_for_tests().tracks, tracks);
    }
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn rack_visibility_is_independent_of_the_drawer_and_library() {
    let mut harness = Harness::with_midi_clip();
    let revision = harness.app.document_for_tests().revision();
    assert_eq!(harness.app.detail_view_for_tests(), "piano");
    close_rack(&mut harness);
    harness.assert_visible(Id::new("piano-grid"), true);
    harness.assert_visible(Id::new("browser-search"), true);
    harness.dispatch(Action::ToggleBrowser);
    harness.assert_visible(Id::new("browser-search"), false);
    harness.assert_visible(Id::new("piano-grid"), true);
    harness.assert_visible(Id::new("rack-close"), false);

    let arrange = harness.labelled("Arrange");
    harness.click(arrange.center());
    harness.steps(2);
    harness.assert_visible(Id::new("piano-grid"), false);
    harness.assert_visible(Id::new("rack-close"), false);
    harness.assert_visible(Id::new("browser-search"), false);

    harness.dispatch(Action::ShowMixer);
    for visible in [true, false] {
        harness.dispatch(Action::ToggleRack);
        harness.assert_visible(Id::new("rack-close"), visible);
        harness.assert_visible(Id::new("browser-search"), false);
        assert_eq!(harness.app.detail_view_for_tests(), "mixer");
    }
    harness.dispatch(Action::CloseDrawer);
    harness.assert_visible(Id::new("rack-close"), false);
    assert_eq!(harness.app.detail_view_for_tests(), "devices");
    harness.dispatch(Action::ToggleBrowser);
    harness.assert_visible(Id::new("browser-search"), true);
    harness.assert_visible(Id::new("rack-close"), false);
    harness.dispatch(Action::ShowDevices);
    harness.assert_visible(Id::new("rack-close"), true);
    harness.assert_visible(Id::new("browser-search"), true);
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn view_menu_can_hide_and_show_the_rack() {
    let mut harness = Harness::with_instruments();
    harness.select_track(1);
    let selection = harness.app.selection_for_tests().clone();
    let shortcut = harness.ctx.format_shortcut(&egui::KeyboardShortcut::new(
        Modifiers::COMMAND | Modifiers::ALT,
        Key::R,
    ));
    for (label, visible) in [("Hide Rack", false), ("Show Rack", true)] {
        let view = harness.labelled("View");
        harness.click(view.center());
        let item = harness.labelled(&format!("{label} {shortcut}"));
        harness.click(item.center());
        harness.key(Key::Escape, Modifiers::NONE);
        harness.steps(2);
        harness.assert_visible(Id::new("rack-close"), visible);
        harness.assert_visible(Id::new(("device-chain", 1)), visible);
        assert_eq!(harness.app.selection_for_tests(), &selection);
    }
    assert!(!harness.app.document_for_tests().is_dirty());
}

#[test]
fn opening_builtin_controls_reveals_the_rack_only_for_the_device_chain() {
    use sympoie::builtin::BuiltinKind;

    let mut harness = Harness::with_empty_project();
    harness.dispatch(Action::AddTrack(crate::document::edit::TrackKind::Midi));
    close_rack(&mut harness);
    harness.dispatch(Action::LoadVst3 {
        track: 1,
        plugin: BuiltinKind::Oscil.descriptor(),
    });
    harness.steps(3);
    harness.assert_visible(Id::new("rack-close"), true);
    harness.assert_visible(Id::new(("device-chain", 1)), true);
    let Track::Midi(midi) = &harness.project().tracks()[1] else {
        panic!("expected the instrument track");
    };
    let instrument = midi.plugin_instrument().expect("Oscil loaded").id;
    let revision = harness.app.document_for_tests().revision();
    close_rack(&mut harness);
    harness.dispatch(Action::OpenNativeVst3Editor(instrument));
    harness.assert_visible(Id::new("rack-close"), true);
    harness.assert_visible(Id::new(("device-chain", 1)), true);
    assert_eq!(harness.app.document_for_tests().revision(), revision);

    close_rack(&mut harness);
    harness.dispatch(Action::LoadMixerVst3 {
        track: 1,
        plugin: BuiltinKind::Eq.descriptor(),
        before: None,
    });
    harness.steps(3);
    harness.assert_visible(Id::new("rack-close"), false);
    assert_eq!(harness.app.detail_view_for_tests(), "mixer");
    let effect = harness.project().tracks()[1].mixer_effects()[0].id;
    let revision = harness.app.document_for_tests().revision();
    harness.dispatch(Action::CloseDrawer);
    harness.dispatch(Action::OpenNativeVst3Editor(effect));
    harness.assert_visible(Id::new("rack-close"), false);
    harness.assert_visible(Id::new("mixer-builtin-panel"), true);
    assert_eq!(harness.app.detail_view_for_tests(), "mixer");
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(harness.app.vst3_editors_for_tests().is_empty());
}
