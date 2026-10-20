use super::*;
use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput};
use std::path::PathBuf;
use std::sync::Arc;

use crate::audio::EngineStatus;
use crate::plugins::PluginRuntime;
use crate::state::{ArrangementState, LoopRegion};
use crate::ui::waveform::WaveformCache;
use crate::views::{arrangement, devices};
use sympoie::plugin::PluginInstance;
use sympoie::transport::PlaybackState;

struct Harness {
    ctx: egui::Context,
    state: BrowserState,
    project: Project,
    selection: Selection,
    catalog: Vec<PluginDescriptor>,
    actions: Vec<Action>,
    loading: bool,
    time: f64,
    arrangement: ArrangementState,
    waveforms: WaveformCache,
    device_runtime: Option<Arc<PluginRuntime>>,
    cursor: egui::CursorIcon,
}

fn plugin(name: &str, vendor: &str, kind: PluginKind) -> PluginDescriptor {
    PluginDescriptor {
        path: PathBuf::from(format!("/test/{vendor}/{name}.vst3")),
        class_id: name.to_owned(),
        name: name.to_owned(),
        vendor: vendor.to_owned(),
        kind,
    }
}

fn plugin_id(plugin: &PluginDescriptor) -> Id {
    Id::new(("browser-plugin", &plugin.path, &plugin.class_id))
}

fn vendor_id(vendor: &str) -> Id {
    Id::new(("browser-vendor", vendor))
}

impl Harness {
    fn new(catalog: Vec<PluginDescriptor>) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut project = Project::new();
        project.add_midi_track("Keys");
        project.add_audio_track("Audio");
        let mut harness = Self {
            ctx,
            state: BrowserState {
                category: Category::Plugins,
                ..BrowserState::default()
            },
            project,
            selection: Selection {
                tracks: [1].into(),
                ..Selection::default()
            },
            catalog,
            actions: Vec::new(),
            loading: false,
            time: 0.0,
            arrangement: ArrangementState::default(),
            waveforms: WaveformCache::default(),
            device_runtime: None,
            cursor: egui::CursorIcon::Default,
        };
        harness.settle();
        harness
    }

    fn step(&mut self, events: Vec<Event>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                vec2(
                    1100.0
                        + if self.device_runtime.is_some() {
                            devices::RACK_WIDTH
                        } else {
                            0.0
                        },
                    700.0,
                ),
            )),
            time: Some(self.time),
            focused: true,
            events,
            ..RawInput::default()
        };
        self.time += 1.0 / 60.0;
        let inputs = Inputs {
            project: &self.project,
            selection: &self.selection,
            catalog: &self.catalog,
            scan_status: None,
            loading: self.loading,
            mixer: false,
        };
        let mut output = self.ctx.run_ui(raw, |ui| {
            let full = ui.available_rect_before_wrap();
            let mut upper = full;
            if let Some(runtime) = &self.device_runtime {
                let device_rect = full.with_min_x(full.right() - devices::RACK_WIDTH);
                let mut device_ui = ui.new_child(egui::UiBuilder::new().max_rect(device_rect));
                devices::show(
                    &mut device_ui,
                    &self.project,
                    &self.selection,
                    runtime,
                    &mut self.actions,
                );
                upper = upper.with_max_x(device_rect.left());
            }
            let mut browser_ui = ui
                .new_child(egui::UiBuilder::new().max_rect(upper.with_max_x(full.left() + 300.0)));
            show(&mut browser_ui, &mut self.state, &inputs, &mut self.actions);
            let arrangement_rect = upper.with_min_x(full.left() + 300.0);
            let mut arrangement_ui =
                ui.new_child(egui::UiBuilder::new().max_rect(arrangement_rect));
            let status = EngineStatus {
                state: PlaybackState::Stopped,
                position_frame: 0,
                render_error: None,
            };
            arrangement::show(
                &mut arrangement_ui,
                &mut self.arrangement,
                &arrangement::Inputs {
                    project: &self.project,
                    selection: &self.selection,
                    status: &status,
                    loop_region: LoopRegion::default(),
                    armed_tracks: &Default::default(),
                    input_catalog: &Default::default(),
                    recording: false,
                    recording_start_tick: None,
                },
                &mut self.waveforms,
                &mut vec![crate::ui::widgets::MeterState::default(); self.project.tracks().len()],
                &mut crate::ui::widgets::MeterState::default(),
                &mut self.actions,
            );
        });
        self.cursor = output.platform_output.cursor_icon;
        output.textures_delta.clear();
    }

    fn settle(&mut self) {
        self.step(Vec::new());
        self.step(Vec::new());
    }

    fn visible(&self, id: Id) -> bool {
        self.ctx.read_response(id).is_some()
    }

    fn center(&self, id: Id) -> Pos2 {
        self.ctx
            .read_response(id)
            .unwrap_or_else(|| panic!("missing browser widget {id:?}"))
            .rect
            .center()
    }

    fn click_at(&mut self, position: Pos2) {
        self.step(vec![Event::PointerMoved(position)]);
        for pressed in [true, false] {
            self.step(vec![Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        self.settle();
    }

    fn click(&mut self, id: Id) {
        self.time += 0.7;
        self.click_at(self.center(id));
    }

    fn double_click(&mut self, id: Id) {
        self.time += 0.7;
        let position = self.center(id);
        self.click_at(position);
        self.click_at(position);
    }

    fn start_drag(&mut self, id: Id, destination: Pos2) {
        let source = self.center(id);
        self.step(vec![Event::PointerMoved(source)]);
        self.step(vec![Event::PointerButton {
            pos: source,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.step(vec![Event::PointerMoved(source + vec2(10.0, 0.0))]);
        self.step(vec![Event::PointerMoved(destination)]);
    }

    fn release(&mut self, destination: Pos2) {
        self.step(vec![Event::PointerButton {
            pos: destination,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.settle();
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        for pressed in [true, false] {
            self.step(vec![Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }]);
        }
    }

    fn search(&mut self, query: &str) {
        self.click(Id::new("browser-search"));
        self.key(Key::A, Modifiers::COMMAND);
        if query.is_empty() {
            self.key(Key::Backspace, Modifiers::NONE);
        } else {
            self.step(vec![Event::Text(query.to_owned())]);
        }
        self.settle();
        assert_eq!(self.state.query, query);
    }
}

#[test]
fn vendor_rows_expand_without_loading_and_device_double_click_loads() {
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    let reverb_id = plugin_id(&reverb);
    let mut harness = Harness::new(vec![reverb.clone()]);
    assert!(harness.visible(vendor_id("acme")));
    assert!(!harness.visible(reverb_id));

    harness.click(vendor_id("acme"));
    assert!(harness.visible(reverb_id));
    assert!(harness.actions.is_empty());

    harness.click(vendor_id("acme"));
    assert!(!harness.visible(reverb_id));
    assert!(harness.actions.is_empty());

    harness.click(vendor_id("acme"));
    harness.double_click(reverb_id);
    assert_eq!(
        harness.actions,
        [Action::LoadVst3 {
            track: 1,
            plugin: reverb,
        }]
    );
}

#[test]
fn searching_reveals_matches_and_restores_manual_folder_expansion() {
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    let delay = plugin("Delay", "Zeta", PluginKind::Effect);
    let reverb_id = plugin_id(&reverb);
    let delay_id = plugin_id(&delay);
    let mut harness = Harness::new(vec![reverb, delay]);
    harness.click(vendor_id("acme"));

    harness.search("delay");
    assert!(!harness.visible(vendor_id("acme")));
    assert!(harness.visible(delay_id));
    harness.click(vendor_id("zeta"));
    assert!(!harness.visible(delay_id));

    harness.search("zeta");
    assert!(harness.visible(delay_id));

    harness.search("");
    assert!(harness.visible(reverb_id));
    assert!(harness.visible(vendor_id("zeta")));
    assert!(!harness.visible(delay_id));
    assert!(harness.actions.is_empty());
}

#[test]
fn catalog_updates_preserve_selected_device_identity() {
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    let reverb_id = plugin_id(&reverb);
    let mut harness = Harness::new(vec![reverb.clone()]);
    harness.click(vendor_id("acme"));
    harness.click(reverb_id);
    assert_eq!(harness.state.selected, Some(reverb_id));

    harness
        .catalog
        .insert(0, plugin("Compressor", "Acme", PluginKind::Effect));
    harness
        .catalog
        .push(plugin("Echo", "Aardvark", PluginKind::Effect));
    harness.settle();
    assert_eq!(harness.state.selected, Some(reverb_id));
    harness.double_click(reverb_id);
    assert_eq!(
        harness.actions,
        [Action::LoadVst3 {
            track: 1,
            plugin: reverb,
        }]
    );

    harness.catalog.retain(|item| plugin_id(item) != reverb_id);
    harness.settle();
    assert_eq!(harness.state.selected, None);
}

#[test]
fn nested_instruments_keep_loading_and_track_type_guards() {
    let synth = plugin("Synth", "Acme", PluginKind::Instrument);
    let synth_id = plugin_id(&synth);
    let mut harness = Harness::new(vec![synth.clone()]);
    harness.click(vendor_id("acme"));

    harness.selection.tracks = [2].into();
    harness.settle();
    harness.double_click(synth_id);
    assert!(
        harness.actions.is_empty(),
        "audio tracks reject instruments"
    );

    harness.selection.tracks.clear();
    harness.settle();
    harness.double_click(synth_id);
    assert!(
        harness.actions.is_empty(),
        "a load needs one selected track"
    );

    harness.selection.tracks = [1].into();
    harness.loading = true;
    harness.settle();
    harness.double_click(synth_id);
    assert!(harness.actions.is_empty(), "concurrent loads are blocked");

    harness.loading = false;
    harness.settle();
    harness.double_click(synth_id);
    assert_eq!(
        harness.actions,
        [Action::LoadVst3 {
            track: 1,
            plugin: synth,
        }]
    );
}

#[test]
fn browser_effect_drag_appends_to_lane_header_and_master_without_selected_track() {
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    for (track, lane) in [(1, false), (2, false), (2, true), (0, false), (0, true)] {
        let mut harness = Harness::new(vec![reverb.clone()]);
        harness.selection.tracks.clear();
        harness.click(vendor_id("acme"));
        let mut destination = harness.center(Id::new(("header", track)));
        if lane {
            destination.x = 420.0;
        }
        harness.start_drag(plugin_id(&reverb), destination);
        assert!(harness.actions.is_empty(), "hovering must not load devices");
        assert!(matches!(
            egui::DragAndDrop::payload::<DeviceDrag>(&harness.ctx).as_deref(),
            Some(DeviceDrag::Browser(BrowserDevice::Plugin(plugin))) if plugin == &reverb
        ));
        harness.release(destination);
        assert_eq!(
            harness.actions,
            [Action::LoadVst3 {
                track,
                plugin: reverb.clone(),
            }]
        );
        assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
    }
}

#[test]
fn browser_instrument_drag_only_loads_on_midi_tracks() {
    let synth = plugin("Synth", "Acme", PluginKind::Instrument);
    for track in [0, 1, 2] {
        let mut harness = Harness::new(vec![synth.clone()]);
        harness.selection.tracks = [2].into();
        harness.click(vendor_id("acme"));
        let destination = harness.center(Id::new(("header", track)));
        harness.start_drag(plugin_id(&synth), destination);
        harness.release(destination);
        if track == 1 {
            assert_eq!(
                harness.actions,
                [Action::LoadVst3 {
                    track: 1,
                    plugin: synth.clone(),
                }]
            );
        } else {
            assert!(harness.actions.is_empty());
        }
    }
}

#[test]
fn browser_drag_cancel_and_loading_do_not_load() {
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    let mut harness = Harness::new(vec![reverb.clone()]);
    harness.click(vendor_id("acme"));
    let destination = harness.center(Id::new(("header", 1)));
    harness.start_drag(plugin_id(&reverb), destination);
    harness.key(Key::Escape, Modifiers::NONE);
    harness.release(destination);
    assert!(harness.actions.is_empty());

    harness.loading = true;
    harness.start_drag(plugin_id(&reverb), destination);
    assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
    harness.release(destination);
    assert!(harness.actions.is_empty());
}

#[test]
fn browser_oscil_drag_replaces_instrument_on_midi_track() {
    let mut harness = Harness::new(Vec::new());
    harness.state.category = Category::Instruments;
    harness.settle();
    let destination = harness.center(Id::new(("header", 1)));
    harness.start_drag(Id::new(("browser-builtin", "Oscil")), destination);
    harness.release(destination);
    assert!(matches!(
        harness.actions.as_slice(),
        [Action::LoadVst3 { track: 1, plugin }] if BuiltinKind::from_descriptor(plugin) == Some(BuiltinKind::Oscil)
    ));
}

#[test]
fn browser_device_drop_accepts_group_and_occupied_clip_lane() {
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    let mut harness = Harness::new(vec![reverb.clone()]);
    harness.project.add_group_track("Bus");
    let midi = harness.project.add_midi_track("Occupied");
    harness
        .project
        .add_midi_clip(
            midi,
            sympoie::clip::MidiClip {
                offset_ticks: 0,
                start_tick: 0,
                length_ticks: 9600,
                notes: Vec::new(),
                name: None,
                pitch_bend: Vec::new(),
            },
        )
        .unwrap();
    harness.settle();
    harness.click(vendor_id("acme"));
    for track in [3, 4] {
        let mut destination = harness.center(Id::new(("header", track)));
        if track == 4 {
            destination.x = 420.0;
        }
        harness.start_drag(plugin_id(&reverb), destination);
        harness.release(destination);
        assert_eq!(
            harness.actions,
            [Action::LoadVst3 {
                track,
                plugin: reverb.clone(),
            }]
        );
        harness.actions.clear();
    }
}

#[test]
fn browser_drag_into_effect_chain_inserts_before_the_target_instance_once() {
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    let mut harness = Harness::new(vec![reverb.clone()]);
    harness.device_runtime = Some(PluginRuntime::new());
    let track = harness.project.track_ids().nth(1).unwrap();
    let existing = plugin("Same name", "Acme", PluginKind::Effect);
    let first = harness
        .project
        .add_effect(track, PluginInstance::new(existing.clone(), vec![], vec![]))
        .unwrap();
    let second = harness
        .project
        .add_effect(track, PluginInstance::new(existing, vec![], vec![]))
        .unwrap();
    assert_ne!(first, second);
    harness.settle();
    harness.click(vendor_id("acme"));
    let card = harness
        .ctx
        .read_response(Id::new(("device-vst3", second as usize)))
        .unwrap()
        .rect;
    let destination = pos2(card.center().x, card.top() + 10.0);
    harness.start_drag(plugin_id(&reverb), destination);
    assert_eq!(harness.cursor, egui::CursorIcon::Copy);
    assert!(harness.actions.is_empty(), "hovering must not load devices");
    assert!(matches!(
        egui::DragAndDrop::payload::<DeviceDrag>(&harness.ctx).as_deref(),
        Some(DeviceDrag::Browser(BrowserDevice::Plugin(plugin))) if plugin == &reverb
    ));
    harness.release(destination);
    assert_eq!(
        harness.actions,
        [Action::InsertVst3Before {
            track: 1,
            plugin: reverb,
            before: second,
        }],
        "the browser gesture inserts once before the intended instance, without appending"
    );
    assert!(!egui::DragAndDrop::has_any_payload(&harness.ctx));
}

struct TemporaryLibraryFolder(PathBuf);

impl TemporaryLibraryFolder {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "sympoie-browser-ui-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path.canonicalize().unwrap()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create test folder: {error}"),
            }
        }
    }

    fn file(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"test contents").unwrap();
        path
    }

    fn folder_id(&self, path: &Path) -> Id {
        Id::new(("browser-folder", self.0.as_path(), path))
    }

    fn file_id(&self, path: &Path) -> Id {
        Id::new(("browser-file", self.0.as_path(), path))
    }
}

impl Drop for TemporaryLibraryFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn custom_and_samples_categories_hide_devices_and_preserve_custom_folders() {
    let folder = TemporaryLibraryFolder::new();
    let audio = folder.file("Kick.wav");
    let reverb = plugin("Reverb", "Acme", PluginKind::Effect);
    let reverb_id = plugin_id(&reverb);
    let mut harness = Harness::new(vec![reverb]);
    harness.click(vendor_id("acme"));
    harness.click(reverb_id);

    harness.state.add_folder(folder.0.clone()).unwrap();
    harness.settle();
    assert_eq!(harness.state.category, Category::Custom);
    assert!(harness.visible(folder.file_id(&audio)));
    assert!(!harness.visible(vendor_id("acme")));
    assert!(!harness.visible(reverb_id));

    harness.click(folder.file_id(&audio));
    harness.search("kick");
    harness.click(Id::new(("browser-category", Category::Samples)));
    assert_eq!(harness.state.category, Category::Samples);
    assert!(harness.state.query.is_empty());
    assert!(harness.state.selected.is_none());
    assert!(!harness.visible(folder.folder_id(&folder.0)));
    assert!(!harness.visible(folder.file_id(&audio)));
    assert!(!harness.visible(vendor_id("acme")));
    assert!(!harness.visible(reverb_id));

    harness.click(Id::new(("browser-category", Category::Custom)));
    assert!(harness.visible(folder.file_id(&audio)));
    harness.click(Id::new(("browser-category", Category::Plugins)));
    assert!(harness.visible(reverb_id));
    assert!(!harness.visible(folder.file_id(&audio)));
    assert!(harness.actions.is_empty());
}

#[test]
fn custom_folder_expansion_and_case_insensitive_filter_keep_directories_visible() {
    let folder = TemporaryLibraryFolder::new();
    let kick = folder.file("Drums/KiCk.WAV");
    let snare = folder.file("Drums/Snare.wav");
    let notes = folder.file("Readme.txt");
    let drums = folder.0.join("Drums");
    let mut harness = Harness::new(Vec::new());
    harness.state.add_folder(folder.0.clone()).unwrap();
    harness.settle();
    assert!(harness.visible(folder.folder_id(&drums)));
    assert!(harness.visible(folder.file_id(&notes)));
    assert!(!harness.visible(folder.file_id(&kick)));

    harness.click(folder.folder_id(&drums));
    assert!(harness.visible(folder.file_id(&kick)));
    assert!(harness.visible(folder.file_id(&snare)));
    harness.search("  KICK  ");
    assert!(harness.visible(folder.folder_id(&drums)));
    assert!(harness.visible(folder.file_id(&kick)));
    assert!(!harness.visible(folder.file_id(&snare)));
    assert!(!harness.visible(folder.file_id(&notes)));

    harness.click(folder.folder_id(&drums));
    assert!(!harness.visible(folder.file_id(&kick)));
    harness.search("");
    assert!(harness.visible(folder.folder_id(&drums)));
    assert!(harness.visible(folder.file_id(&notes)));
    assert!(!harness.visible(folder.file_id(&kick)));
    harness.click(folder.folder_id(&drums));
    assert!(harness.visible(folder.file_id(&snare)));
    harness.click(folder.folder_id(&folder.0));
    assert!(!harness.visible(folder.folder_id(&drums)));
    assert!(!harness.visible(folder.file_id(&notes)));
    harness.click(folder.folder_id(&folder.0));
    assert!(harness.visible(folder.file_id(&kick)));
    assert!(harness.actions.is_empty());
}

#[test]
fn custom_audio_double_click_loads_the_selected_sampler_or_a_new_sampler_track() {
    let folder = TemporaryLibraryFolder::new();
    let audio = folder.file("Kick.WAV");
    let unsupported = folder.file("Readme.txt");
    let mut harness = Harness::new(Vec::new());
    harness.state.add_folder(folder.0.clone()).unwrap();
    harness.settle();

    // Without a Sampler on the selected track, a new Sampler track is made.
    for selection in [vec![2], vec![1], vec![], vec![1, 2]] {
        harness.selection.tracks = selection.into_iter().collect();
        harness.settle();
        harness.double_click(folder.file_id(&audio));
        assert_eq!(
            harness.actions,
            [Action::LoadSampleIntoSampler {
                path: audio.clone(),
                track: None,
            }]
        );
        harness.actions.clear();
    }
    let keys = harness.project.track_ids().nth(1).unwrap();
    let sympoie::track::TrackId::Midi(keys) = keys else {
        panic!("track 1 is the MIDI track")
    };
    harness
        .project
        .set_plugin_instrument(
            keys,
            Some(sympoie::builtin::BuiltinKind::Sampler.instance()),
        )
        .unwrap();
    harness.selection.tracks = [1].into();
    harness.settle();
    harness.double_click(folder.file_id(&audio));
    assert_eq!(
        harness.actions,
        [Action::LoadSampleIntoSampler {
            path: audio.clone(),
            track: Some(1),
        }]
    );
    harness.actions.clear();

    harness.double_click(folder.file_id(&unsupported));
    assert!(harness.actions.is_empty());
    assert_eq!(harness.state.selected, Some(folder.file_id(&unsupported)));
}

#[test]
fn removing_a_custom_root_only_removes_its_bookmark() {
    let folder = TemporaryLibraryFolder::new();
    let audio = folder.file("Drums/Kick.wav");
    let drums = folder.0.join("Drums");
    let mut harness = Harness::new(Vec::new());
    harness.state.add_folder(folder.0.clone()).unwrap();
    harness.settle();
    harness.click(folder.folder_id(&drums));
    harness.click(folder.file_id(&audio));
    harness.click(Id::new(("browser-remove-folder", folder.0.as_path())));
    assert_eq!(
        harness.actions,
        [Action::RemoveLibraryFolder(folder.0.clone())]
    );

    harness.state.remove_folder(&folder.0).unwrap();
    harness.settle();
    assert!(harness.state.folders.roots().is_empty());
    assert!(harness.state.expanded_folders.is_empty());
    assert!(harness.state.selected.is_none());
    assert!(!harness.visible(folder.folder_id(&folder.0)));
    assert!(!harness.visible(folder.file_id(&audio)));
    assert!(folder.0.is_dir());
    assert!(drums.is_dir());
    assert_eq!(std::fs::read(&audio).unwrap(), b"test contents");
}

#[test]
fn custom_footer_requests_a_folder_and_refreshes_cached_contents() {
    let folder = TemporaryLibraryFolder::new();
    let existing = folder.file("Kick.wav");
    let mut harness = Harness::new(Vec::new());
    harness.state.add_folder(folder.0.clone()).unwrap();
    harness.settle();
    assert!(harness.visible(folder.file_id(&existing)));

    let footer_y = harness.ctx.content_rect().bottom() - 11.0;
    harness.click_at(pos2(20.0, footer_y));
    assert_eq!(harness.actions, [Action::AddLibraryFolder]);
    harness.actions.clear();

    let added = folder.file("Snare.wav");
    harness.settle();
    assert!(!harness.visible(folder.file_id(&added)));
    harness.click(folder.file_id(&existing));
    harness.click_at(pos2(105.0, footer_y));
    assert!(harness.visible(folder.file_id(&added)));
    assert!(harness.state.selected.is_none());
    assert!(harness.actions.is_empty());
}

#[test]
fn samples_stays_empty_during_scanning_and_categories_fit_minimum_library_width() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let mut state = BrowserState {
        category: Category::Samples,
        ..BrowserState::default()
    };
    let project = Project::new();
    let selection = Selection::default();
    let catalog = vec![plugin("Reverb", "Acme", PluginKind::Effect)];
    let inputs = Inputs {
        project: &project,
        selection: &selection,
        catalog: &catalog,
        scan_status: Some("Checking vendor plug-ins".to_owned()),
        loading: true,
        mixer: false,
    };
    let mut actions = Vec::new();
    let mut bounds = Rect::NOTHING;
    let mut painted_labels = Vec::new();
    for frame in 0..2 {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(220.0, 500.0))),
                time: Some(f64::from(frame) / 60.0),
                ..RawInput::default()
            },
            |ui| {
                bounds = ui.available_rect_before_wrap();
                show(ui, &mut state, &inputs, &mut actions);
            },
        );
        fn collect_labels(shape: &egui::Shape, labels: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => labels.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_labels(shape, labels);
                    }
                }
                _ => {}
            }
        }
        painted_labels.clear();
        for shape in &output.shapes {
            collect_labels(&shape.shape, &mut painted_labels);
        }
        output.textures_delta.clear();
    }
    for category in Category::ALL {
        let response = ctx
            .read_response(Id::new(("browser-category", category)))
            .unwrap();
        assert!(
            bounds.contains_rect(response.rect),
            "{} is outside the minimum-width Library: {:?}",
            category.label(),
            response.rect
        );
    }
    assert!(
        painted_labels
            .iter()
            .any(|text| text == "No built-in samples yet")
    );
    assert!(
        !painted_labels
            .iter()
            .any(|text| text.contains("plug-ins") || text.contains("Loading"))
    );
    assert!(ctx.read_response(vendor_id("acme")).is_none());
    assert!(ctx.read_response(plugin_id(&catalog[0])).is_none());
    for kind in BuiltinKind::ALL {
        assert!(ctx.read_response(Item::Builtin(kind).id()).is_none());
    }
    assert!(actions.is_empty());
}
