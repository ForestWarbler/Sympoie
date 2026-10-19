use super::*;
use sympoie::track::{AudioOutput, MixerSettings, TrackId};

fn sends_fixture() -> Harness {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::AddBusTrack);
    harness.dispatch(Action::RenameTrack(3, "Reverb".into()));
    harness.dispatch(Action::AddBusTrack);
    harness.dispatch(Action::RenameTrack(4, "Delay".into()));
    harness
}

fn send_levels(harness: &Harness, track: usize) -> Vec<(String, f32)> {
    harness.project().tracks()[track]
        .sends()
        .iter()
        .map(|send| {
            let bus = harness
                .project()
                .track_ids()
                .position(|id| id == TrackId::Bus(send.destination))
                .unwrap();
            (harness.project().tracks()[bus].name().to_owned(), send.gain)
        })
        .collect()
}

#[test]
fn sends_keep_the_main_route_and_have_independent_history_and_saved_levels() {
    let mut harness = sends_fixture();
    let original = *harness.project().tracks()[1].mixer();
    harness.dispatch(Action::SetTrackSend {
        track: 1,
        bus: 3,
        gain: 0.0,
    });
    harness.dispatch(Action::SetTrackSend {
        track: 1,
        bus: 4,
        gain: 0.0,
    });
    for gain in [0.1, 0.2, 0.4] {
        harness.dispatch(Action::SetTrackSend {
            track: 1,
            bus: 3,
            gain,
        });
    }
    for gain in [0.2, 0.3] {
        harness.dispatch(Action::SetTrackSend {
            track: 1,
            bus: 4,
            gain,
        });
    }
    assert_eq!(*harness.project().tracks()[1].mixer(), original);
    assert_eq!(
        send_levels(&harness, 1),
        vec![("Reverb".into(), 0.4), ("Delay".into(), 0.3)]
    );
    harness.dispatch(Action::Undo);
    assert_eq!(
        send_levels(&harness, 1),
        vec![("Reverb".into(), 0.4), ("Delay".into(), 0.0)]
    );
    harness.dispatch(Action::Undo);
    assert_eq!(
        send_levels(&harness, 1),
        vec![("Reverb".into(), 0.0), ("Delay".into(), 0.0)]
    );
    harness.dispatch(Action::Redo);
    harness.dispatch(Action::Redo);
    harness.dispatch(Action::RemoveTrackSend { track: 1, bus: 3 });
    assert_eq!(send_levels(&harness, 1), vec![("Delay".into(), 0.3)]);
    harness.dispatch(Action::Undo);
    let path = harness._files.0.join("sends.symp");
    sympoie::project_file::save_project_directory(harness.project(), &path).unwrap();
    harness.dispatch(Action::OpenPath(path));
    harness.dispatch(Action::ShowMixer);
    assert_eq!(
        send_levels(&harness, 1),
        vec![("Reverb".into(), 0.4), ("Delay".into(), 0.3)]
    );
    assert_eq!(*harness.project().tracks()[1].mixer(), original);
    for bus in [3usize, 4] {
        assert!(
            harness
                .ctx
                .read_response(Id::new(("mixer-send-level", 1usize, bus)))
                .is_some()
        );
    }
}

#[test]
fn deleting_a_send_bus_removes_only_its_sends_and_undo_restores_them() {
    let mut harness = sends_fixture();
    for (track, bus, gain) in [(1, 3, 0.4), (1, 4, 0.2), (2, 3, 0.7)] {
        harness.dispatch(Action::SetTrackSend { track, bus, gain });
    }
    harness.dispatch(Action::DeleteTrack(3));
    assert_eq!(harness.project().tracks().len(), 4);
    assert_eq!(send_levels(&harness, 1), vec![("Delay".into(), 0.2)]);
    assert!(send_levels(&harness, 2).is_empty());
    for track in [1, 2] {
        assert_eq!(
            harness.project().tracks()[track].mixer().output(),
            AudioOutput::Master
        );
        assert!(
            matches!(&harness.project().tracks()[track], Track::Audio(track) if track.clips().len() == 1)
        );
    }
    harness.dispatch(Action::Undo);
    assert_eq!(
        send_levels(&harness, 1),
        vec![("Reverb".into(), 0.4), ("Delay".into(), 0.2)]
    );
    assert_eq!(send_levels(&harness, 2), vec![("Reverb".into(), 0.7)]);
    harness.dispatch(Action::SetTrackSend {
        track: 3,
        bus: 4,
        gain: 0.5,
    });
    let revision = harness.app.document_for_tests().revision();
    harness.dispatch(Action::SetTrackSend {
        track: 4,
        bus: 3,
        gain: 0.0,
    });
    assert_eq!(harness.app.document_for_tests().revision(), revision);
    assert!(send_levels(&harness, 4).is_empty());
    assert!(harness.app.notice_for_tests().unwrap().contains("cycle"));
}

#[test]
fn bounce_keeps_sends_editable_and_does_not_bake_bus_returns_into_the_clip() {
    let mut harness = sends_fixture();
    for bus in [3, 4] {
        harness.dispatch(Action::SetTrackSend {
            track: 1,
            bus,
            gain: 0.5,
        });
        harness.dispatch(Action::SetMixer {
            track: bus,
            mixer: MixerSettings::new(0.5, 0.0, AudioOutput::Master).unwrap(),
        });
    }
    let original = harness.project().tracks()[1].sends().to_vec();
    harness.dispatch(Action::BounceTrack {
        track: 1,
        duplicate: false,
    });
    harness.finish_bounce();
    assert_eq!(harness.project().tracks()[1].sends(), original);
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!("bounced track missing")
    };
    assert_eq!(track.clips().len(), 1);
    let clip = track.clips()[0].clone();
    let audio = harness.project().audio_asset(clip.asset).unwrap();
    assert_eq!(audio.samples()[0], [0.25; 2]);
    assert_eq!(harness.project().tracks()[3].name(), "Reverb");
    assert_eq!(harness.project().tracks()[4].name(), "Delay");
    harness.dispatch(Action::SetTrackSend {
        track: 1,
        bus: 3,
        gain: 0.1,
    });
    assert_eq!(
        send_levels(&harness, 1),
        vec![("Reverb".into(), 0.1), ("Delay".into(), 0.5)]
    );
}
