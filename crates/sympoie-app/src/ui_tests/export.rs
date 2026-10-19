use super::*;
use crate::export_config::{ExportConfig, ExportDepth, ExportFormat, ExportSource};
use crate::views::arrangement;

fn wait_for_export(harness: &mut Harness) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while harness.app.exporting_for_tests() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
        harness.steps(1);
    }
    assert!(
        !harness.app.exporting_for_tests(),
        "export did not complete"
    );
    harness.steps(1);
}

fn folder(harness: &Harness, name: &str) -> PathBuf {
    let folder = harness._files.0.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn audio_clip_name(harness: &Harness, track: usize) -> Option<String> {
    let Track::Audio(audio) = &harness.project().tracks()[track] else {
        panic!("not an audio track")
    };
    audio.clips()[0].name.clone()
}

fn begin_clip_rename(harness: &mut Harness, track: usize) {
    let clip = harness.clip_rect(track, 0);
    harness.right_click(clip_title(clip));
    let rename = harness.labelled("Rename");
    harness.click(rename.center());
    harness.steps(2);
    assert!(harness.ctx.egui_wants_keyboard_input());
}

#[test]
fn clips_are_renamed_from_their_context_menu_in_one_undo_step() {
    let mut harness = Harness::with_two_clips();
    begin_clip_rename(&mut harness, 1);
    harness.step(vec![Event::Text("Kick take 2".to_owned())]);
    harness.key(Key::Enter, Modifiers::NONE);
    harness.steps(2);
    assert_eq!(audio_clip_name(&harness, 1).as_deref(), Some("Kick take 2"));
    assert_eq!(audio_clip_name(&harness, 2), None);
    harness.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(audio_clip_name(&harness, 1), None);
    harness.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(audio_clip_name(&harness, 1).as_deref(), Some("Kick take 2"));

    // Escape keeps the old name; clearing the text restores the default label.
    begin_clip_rename(&mut harness, 1);
    harness.key(Key::A, Modifiers::COMMAND);
    harness.step(vec![Event::Text("Discarded".to_owned())]);
    harness.key(Key::Escape, Modifiers::NONE);
    harness.steps(2);
    assert_eq!(audio_clip_name(&harness, 1).as_deref(), Some("Kick take 2"));
    begin_clip_rename(&mut harness, 1);
    harness.key(Key::A, Modifiers::COMMAND);
    harness.key(Key::Backspace, Modifiers::NONE);
    harness.key(Key::Enter, Modifiers::NONE);
    harness.steps(2);
    assert_eq!(audio_clip_name(&harness, 1), None);
}

#[test]
fn named_clips_keep_their_names_through_split_and_duplicate() {
    let mut harness = Harness::with_midi_clip();
    let clip = ClipRef { track: 1, index: 0 };
    harness.dispatch(Action::RenameClip {
        clip,
        name: "  Chorus  ".into(),
    });
    harness.dispatch(Action::DuplicateClip(clip));
    harness.dispatch(Action::Seek(5760));
    harness.dispatch(Action::SplitClip(clip));
    let names: Vec<_> = harness
        .midi_clips(1)
        .iter()
        .map(|clip| clip.name.clone())
        .collect();
    assert!(names.len() >= 3, "{names:?}");
    assert!(
        names.iter().all(|name| name.as_deref() == Some("Chorus")),
        "{names:?}"
    );
    // The clip shows its name in place of the note count.
    let clip = &harness.midi_clips(1)[0];
    assert_eq!(arrangement::midi_clip_label(clip), "Chorus");
    let unnamed = MidiClip {
        name: None,
        ..clip.clone()
    };
    assert!(arrangement::midi_clip_label(&unnamed).ends_with(" notes"));
}

#[test]
fn midi_and_audio_clips_export_from_their_context_menus() {
    let mut harness = Harness::with_two_clips();
    let clip = harness.clip_rect(1, 0);
    harness.right_click(clip_title(clip));
    harness.labelled("Export Audio File…");
    harness.key(Key::Escape, Modifiers::NONE);

    // The audio clip as it plays: trimmed, with its gain.
    harness.dispatch(Action::TrimClipStart {
        clip: ClipRef { track: 1, index: 0 },
        start: 960,
    });
    harness.dispatch(Action::SetClipGain {
        clip: ClipRef { track: 1, index: 0 },
        gain: 0.5,
    });
    let path = harness._files.0.join("kick clip.wav");
    harness
        .app
        .export_clip_audio_for_tests(ClipRef { track: 1, index: 0 }, path.clone());
    wait_for_export(&mut harness);
    let audio = sympoie::wav::read_wav(&path).unwrap();
    let Track::Audio(track) = &harness.project().tracks()[1] else {
        panic!("not an audio track")
    };
    assert_eq!(audio.len() as u64, track.clips()[0].frame_count);
    assert!(audio.len() < 48_000);
    assert!(audio.samples().iter().all(|frame| *frame == [0.125; 2]));
    let path = harness._files.0.join("kick clip.flac");
    harness
        .app
        .export_clip_audio_for_tests(ClipRef { track: 1, index: 0 }, path.clone());
    wait_for_export(&mut harness);
    assert_eq!(&std::fs::read(&path).unwrap()[..4], b"fLaC");

    let mut harness = Harness::with_midi_clip();
    let clip = harness.clip_rect(1, 0);
    harness.right_click(clip_title(clip));
    harness.labelled("Export MIDI File…");
    harness.key(Key::Escape, Modifiers::NONE);
    let path = harness._files.0.join("piano.mid");
    harness
        .app
        .export_clip_midi_for_tests(ClipRef { track: 1, index: 0 }, &path);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"MThd");
    assert_eq!(&bytes[14..18], b"MTrk");
    // Track name meta event with the track's name, as the clip has none.
    assert_eq!(&bytes[22..26], &[0, 0xff, 0x03, 5]);
    assert_eq!(&bytes[26..31], b"Piano");
    // Three notes: three note-ons on their channels.
    let ons = bytes
        .windows(3)
        .filter(|event| matches!(event, [0x93, 60, 87] | [0x95, 64, 106] | [0x9f, 67, 52]))
        .count();
    assert_eq!(ons, 3);
}

fn card(harness: &Harness, index: usize) -> Rect {
    harness.rect(Id::new(("export-card", index)))
}

fn add_config(harness: &mut Harness, choice: &str) {
    let add = harness.labelled("+ Config");
    harness.click(add.center());
    let choice = harness.labelled(choice);
    harness.click(choice.center());
    harness.steps(1);
}

#[test]
fn the_export_window_shows_cards_that_open_their_settings_and_render_together() {
    let mut harness = Harness::with_two_clips();
    harness.key(Key::R, Modifiers::COMMAND | Modifiers::SHIFT);
    harness.steps(2);
    assert!(harness.app.export_window_for_tests().open);
    assert_eq!(harness.app.export_window_for_tests().configs.len(), 1);
    card(&harness, 0);

    // + Config offers the default: 48 kHz 24-bit WAV.
    add_config(&mut harness, "Default");
    let window = harness.app.export_window_for_tests();
    assert_eq!(window.configs.len(), 2);
    assert_eq!(window.configs[1], ExportConfig::default());
    assert_eq!(window.configs[1].format_line(), "WAV · 24-bit · 48 kHz");
    assert_eq!(window.editing, None);

    // A card opens its settings; Done goes back to the cards.
    let second = card(&harness, 1);
    harness.click(second.center());
    harness.steps(1);
    assert_eq!(harness.app.export_window_for_tests().editing, Some(1));
    harness.assert_visible(Id::new(("export-card", 0)), false);
    harness.labelled("Triangular dither");
    let done = harness.labelled("Done");
    harness.click(done.center());
    harness.steps(1);
    assert_eq!(harness.app.export_window_for_tests().editing, None);

    let out = folder(&harness, "exports");
    {
        let window = harness.app.export_window_for_tests();
        window.configs[0] = ExportConfig {
            name: "Master".into(),
            folder: Some(out.clone()),
            file_name: "Song".into(),
            ..ExportConfig::default()
        };
        window.configs[1] = ExportConfig {
            name: "Tracks".into(),
            source: ExportSource::Tracks,
            format: ExportFormat::Flac,
            depth: ExportDepth::Int16,
            sample_rate: 44_100,
            file_name: "{track}".into(),
            folder: Some(out.clone()),
            ..ExportConfig::default()
        };
        window.configs.push(ExportConfig {
            name: "Preview".into(),
            format: ExportFormat::Mp3,
            mp3_bitrate: 192,
            folder: Some(out.clone()),
            file_name: "Song preview".into(),
            ..ExportConfig::default()
        });
        window.configs.push(ExportConfig {
            name: "Off".into(),
            folder: Some(out.clone()),
            file_name: "Never".into(),
            ..ExportConfig::default()
        });
    }
    harness.steps(1);
    // Cards sit side by side without overlapping, four to a row.
    let cards: Vec<_> = (0..4).map(|index| card(&harness, index)).collect();
    for (index, rect) in cards.iter().enumerate() {
        assert!(rect.width() > 100.0 && rect.height() > 100.0);
        for other in &cards[index + 1..] {
            assert!(!rect.intersects(*other), "{rect:?} overlaps {other:?}");
        }
    }
    assert!(
        cards
            .windows(2)
            .all(|pair| pair[1].left() > pair[0].right())
    );
    assert!((cards[0].top() - cards[3].top()).abs() < 1.0);
    add_config(&mut harness, "Default");
    let wrapped = card(&harness, 4);
    assert!(
        wrapped.top() > cards[0].bottom(),
        "the fifth card starts a new row"
    );
    harness.app.export_window_for_tests().configs.pop();
    harness.steps(1);
    // The switch on a card turns it off without opening it.
    let last = card(&harness, 3);
    harness.click(last.min + vec2(17.0, 15.0));
    harness.steps(1);
    let window = harness.app.export_window_for_tests();
    assert!(!window.configs[3].enabled);
    assert_eq!(window.editing, None);

    let render = harness.labelled("Render");
    harness.click(render.center());
    assert!(harness.app.exporting_for_tests());
    wait_for_export(&mut harness);
    let mut files: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["Song preview.mp3", "Song.wav", "kick.flac", "tone.flac"]
    );
    let bytes = std::fs::read(out.join("Song.wav")).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 24);
    assert_eq!(&std::fs::read(out.join("kick.flac")).unwrap()[..4], b"fLaC");
    let mp3 = std::fs::read(out.join("Song preview.mp3")).unwrap();
    assert_eq!(mp3[0], 0xff, "an MPEG frame");
    assert_eq!(harness.app.notice_for_tests(), Some("Exported 4 files"));
    let remembered = &harness.app.settings_for_tests().export.last;
    assert_eq!(remembered.len(), 4);
    assert_eq!(remembered[2].mp3_bitrate, 192);
    assert!(!remembered[3].enabled);
}

#[test]
fn presets_are_saved_from_a_card_and_added_back_with_plus_config() {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::ShowExport);
    harness.app.export_window_for_tests().configs[0].name = "Mastering 96k".into();
    harness.app.export_window_for_tests().configs[0].sample_rate = 96_000;
    let first = card(&harness, 0);
    harness.click(first.center());
    harness.steps(1);
    let save = harness.labelled("Save as Preset");
    harness.click(save.center());
    harness.steps(1);
    let saved = &harness.app.settings_for_tests().export.presets;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "Mastering 96k");

    let remove = harness.labelled("Remove");
    harness.click(remove.center());
    harness.steps(1);
    let window = harness.app.export_window_for_tests();
    assert!(window.configs.is_empty());
    assert_eq!(window.editing, None);
    add_config(&mut harness, "Mastering 96k");
    let window = harness.app.export_window_for_tests();
    assert_eq!(window.configs.len(), 1);
    assert_eq!(window.configs[0].sample_rate, 96_000);
}

#[test]
fn a_stems_project_export_writes_a_project_of_audio_tracks() {
    let mut harness = Harness::with_instruments();
    harness.dispatch(Action::InsertMidiClip { track: 1, start: 0 });
    harness.dispatch(Action::ShowExport);
    let out = folder(&harness, "stems");
    harness.app.export_window_for_tests().configs = vec![ExportConfig {
        name: "Stems".into(),
        source: ExportSource::StemsProject,
        file_name: "Session Stems".into(),
        folder: Some(out.clone()),
        ..ExportConfig::default()
    }];
    harness.dispatch(Action::ExportAll);
    wait_for_export(&mut harness);
    let stems = sympoie::project_json::load_project_folder(out.join("Session Stems.symp"))
        .unwrap_or_else(|error| panic!("{error}; {:?}", harness.app.notice_for_tests()));
    let original = harness.project();
    assert_eq!(stems.tracks().len(), original.tracks().len());
    for (before, after) in original.tracks().iter().zip(stems.tracks()) {
        assert_eq!(before.name(), after.name());
        if matches!(before, Track::Audio(_) | Track::Midi(_)) {
            assert!(
                matches!(after, Track::Audio(_)),
                "{} is a stem",
                before.name()
            );
        }
    }
    // Exporting again never replaces the earlier project.
    harness.dispatch(Action::ExportAll);
    wait_for_export(&mut harness);
    assert!(out.join("Session Stems 2.symp").is_dir());
}

#[test]
fn the_cross_on_a_card_removes_that_configuration_only() {
    let mut harness = Harness::with_two_clips();
    harness.dispatch(Action::ShowExport);
    add_config(&mut harness, "Default");
    add_config(&mut harness, "Default");
    {
        let window = harness.app.export_window_for_tests();
        for (config, name) in window.configs.iter_mut().zip(["One", "Two", "Three"]) {
            config.name = name.into();
        }
    }
    harness.steps(1);
    let middle = card(&harness, 1);
    harness.click(egui::pos2(middle.right() - 16.0, middle.top() + 17.0));
    harness.steps(1);
    let window = harness.app.export_window_for_tests();
    let names: Vec<_> = window
        .configs
        .iter()
        .map(|config| config.name.as_str())
        .collect();
    assert_eq!(names, ["One", "Three"]);
    assert_eq!(window.editing, None, "removing does not open a card");
}
