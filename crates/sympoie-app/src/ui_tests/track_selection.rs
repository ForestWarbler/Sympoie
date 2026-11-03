use super::*;
use crate::document::edit::TrackKind;

/// Five audio tracks named "T1" to "T5" at indices 1 to 5.
fn five_tracks() -> Harness {
    let mut harness = Harness::with_empty_project();
    while harness.project().tracks().len() > 1 {
        let last = harness.project().tracks().len() - 1;
        harness.dispatch(Action::DeleteTrack(last));
    }
    for n in 1..=5 {
        harness.dispatch(Action::AddTrack(TrackKind::Audio));
        harness.dispatch(Action::RenameTrack(n, format!("T{n}")));
    }
    harness.dispatch(Action::SelectTracks(BTreeSet::new()));
    assert_eq!(names(&harness), ["T1", "T2", "T3", "T4", "T5"]);
    harness
}

fn names(harness: &Harness) -> Vec<String> {
    harness.project().tracks()[1..]
        .iter()
        .map(|track| track.name().to_owned())
        .collect()
}

fn click_header(harness: &mut Harness, track: usize, modifiers: Modifiers) {
    let header = harness.rect(Id::new(("header", track)));
    harness.click_with(header.left_bottom() + vec2(6.0, -4.0), modifiers);
    harness.steps(1);
}

fn selected(harness: &Harness) -> BTreeSet<usize> {
    harness.app.selection_for_tests().tracks.clone()
}

#[test]
fn shift_click_selects_the_range_down_from_the_anchor_and_keeps_the_anchor() {
    let mut harness = five_tracks();
    harness.select_track(2);
    click_header(&mut harness, 4, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([2, 3, 4]));
    // The anchor stays on 2, so the range shrinks and flips around it.
    click_header(&mut harness, 3, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([2, 3]));
    click_header(&mut harness, 1, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([1, 2]));
    click_header(&mut harness, 5, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([2, 3, 4, 5]));
}

#[test]
fn shift_click_selects_the_range_up_from_the_anchor() {
    let mut harness = five_tracks();
    harness.select_track(5);
    click_header(&mut harness, 2, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([2, 3, 4, 5]));
    // Shift-clicking the anchor itself leaves only it.
    click_header(&mut harness, 5, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([5]));
}

#[test]
fn command_click_toggles_tracks_and_moves_the_anchor() {
    let mut harness = five_tracks();
    harness.select_track(1);
    click_header(&mut harness, 3, Modifiers::COMMAND);
    assert_eq!(selected(&harness), BTreeSet::from([1, 3]));
    click_header(&mut harness, 1, Modifiers::COMMAND);
    assert_eq!(selected(&harness), BTreeSet::from([3]));
    click_header(&mut harness, 4, Modifiers::COMMAND);
    assert_eq!(selected(&harness), BTreeSet::from([3, 4]));
    // The last command-click (4) is the new anchor.
    click_header(&mut harness, 2, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([2, 3, 4]));
    // A plain click still selects only that track.
    harness.select_track(5);
}

#[test]
fn shift_click_after_selecting_elsewhere_extends_from_the_selection() {
    let mut harness = five_tracks();
    harness.select_track(1);
    // e.g. a mixer strip click: the header anchor (1) is no longer selected.
    harness.dispatch(Action::SelectTracks(BTreeSet::from([4])));
    click_header(&mut harness, 2, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([2, 3, 4]));
    // With nothing selected a shift-click behaves like a plain click.
    harness.dispatch(Action::SelectTracks(BTreeSet::new()));
    click_header(&mut harness, 3, Modifiers::SHIFT);
    assert_eq!(selected(&harness), BTreeSet::from([3]));
}

#[test]
fn backspace_deletes_every_selected_track_in_one_undo_step() {
    let mut harness = five_tracks();
    harness.select_track(2);
    click_header(&mut harness, 4, Modifiers::SHIFT);
    for key in [Key::Backspace, Key::Delete] {
        harness.key(key, Modifiers::NONE);
        harness.steps(1);
        assert_eq!(names(&harness), ["T1", "T5"]);
        assert!(selected(&harness).is_empty());

        harness.dispatch(Action::Undo);
        assert_eq!(names(&harness), ["T1", "T2", "T3", "T4", "T5"]);
        harness.dispatch(Action::Redo);
        assert_eq!(names(&harness), ["T1", "T5"]);
        harness.dispatch(Action::Undo);
        assert_eq!(names(&harness), ["T1", "T2", "T3", "T4", "T5"]);
        harness.dispatch(Action::SelectTracks(BTreeSet::from([2, 3, 4])));
    }
}

#[test]
fn delete_key_with_nothing_selected_or_only_the_master_deletes_nothing() {
    let mut harness = five_tracks();
    harness.key(Key::Backspace, Modifiers::NONE);
    harness.dispatch(Action::SelectTracks(BTreeSet::from([0])));
    harness.key(Key::Delete, Modifiers::NONE);
    harness.steps(1);
    assert_eq!(names(&harness), ["T1", "T2", "T3", "T4", "T5"]);
    assert_eq!(harness.project().tracks().len(), 6);
}

#[test]
fn track_menu_delete_removes_the_whole_selection_and_undo_restores_it() {
    let mut harness = five_tracks();
    harness.select_track(1);
    click_header(&mut harness, 3, Modifiers::COMMAND);
    click_header(&mut harness, 5, Modifiers::COMMAND);
    assert_eq!(selected(&harness), BTreeSet::from([1, 3, 5]));
    let header = harness.rect(Id::new(("header", 3)));
    harness.right_click(header.left_bottom() + vec2(6.0, -4.0));
    let delete = harness.labelled("Delete 3 Tracks");
    harness.click(delete.center());
    harness.steps(1);
    assert_eq!(names(&harness), ["T2", "T4"]);

    harness.dispatch(Action::Undo);
    assert_eq!(names(&harness), ["T1", "T2", "T3", "T4", "T5"]);
}

#[test]
fn track_menu_delete_on_an_unselected_track_removes_only_that_track() {
    let mut harness = five_tracks();
    harness.select_track(1);
    click_header(&mut harness, 2, Modifiers::SHIFT);
    let header = harness.rect(Id::new(("header", 4)));
    harness.right_click(header.left_bottom() + vec2(6.0, -4.0));
    let delete = harness.labelled("Delete");
    harness.click(delete.center());
    harness.steps(1);
    assert_eq!(names(&harness), ["T1", "T2", "T3", "T5"]);
}

#[test]
fn deleting_the_selection_through_the_action_is_one_undo_step() {
    let mut harness = five_tracks();
    harness.dispatch(Action::DeleteTracks(BTreeSet::from([1, 2, 5])));
    assert_eq!(names(&harness), ["T3", "T4"]);
    harness.dispatch(Action::Undo);
    assert_eq!(names(&harness), ["T1", "T2", "T3", "T4", "T5"]);
}
