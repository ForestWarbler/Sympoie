use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use sympoie::builtin::BuiltinKind;
use sympoie::plugin::{PluginDescriptor, PluginKind};
use sympoie::project::Project;

use super::browser_folders::LibraryFolders;
use crate::actions::Action;
use crate::document::rows::track_label;
use crate::state::Selection;
use crate::ui::theme;
use crate::ui::widgets;
use crate::views::device_drag::{BrowserDevice, DeviceDrag};

#[path = "browser_custom.rs"]
mod custom;

const CATEGORY_HEIGHT: f32 = 26.0;
const ROW_HEIGHT: f32 = 18.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
enum Category {
    #[default]
    Instruments,
    AudioEffects,
    Plugins,
    Custom,
    Samples,
}

impl Category {
    const ALL: [Self; 5] = [
        Self::Instruments,
        Self::AudioEffects,
        Self::Plugins,
        Self::Custom,
        Self::Samples,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Instruments => "Instruments",
            Self::AudioEffects => "Effects",
            Self::Plugins => "Plug-ins",
            Self::Custom => "Custom",
            Self::Samples => "Samples",
        }
    }
}

#[derive(Debug, Default)]
pub struct BrowserState {
    category: Category,
    query: String,
    selected: Option<Id>,
    expanded_vendors: BTreeSet<String>,
    search_collapsed_vendors: BTreeSet<String>,
    folders: LibraryFolders,
    expanded_folders: BTreeSet<PathBuf>,
}

pub struct Inputs<'a> {
    pub project: &'a Project,
    pub selection: &'a Selection,
    pub catalog: &'a [PluginDescriptor],
    pub scan_status: Option<String>,
    pub loading: bool,
    pub mixer: bool,
}

#[derive(Clone, Copy)]
enum Item<'a> {
    Builtin(BuiltinKind),
    Plugin(&'a PluginDescriptor),
}

impl Item<'_> {
    fn name(&self) -> &str {
        match self {
            Item::Builtin(kind) => super::builtin_devices::name(*kind),
            Item::Plugin(plugin) => &plugin.name,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Item::Builtin(_) => "Sympoie",
            Item::Plugin(plugin) => vendor_name(plugin),
        }
    }

    fn id(&self) -> Id {
        match self {
            Item::Builtin(kind) => {
                Id::new(("browser-builtin", super::builtin_devices::name(*kind)))
            }
            Item::Plugin(plugin) => Id::new(("browser-plugin", &plugin.path, &plugin.class_id)),
        }
    }

    fn is_instrument(&self) -> bool {
        match self {
            Item::Builtin(kind) => kind.descriptor().kind == PluginKind::Instrument,
            Item::Plugin(plugin) => plugin.kind == PluginKind::Instrument,
        }
    }

    fn device(&self) -> BrowserDevice {
        match self {
            Item::Builtin(kind) => BrowserDevice::Plugin(kind.descriptor()),
            Item::Plugin(plugin) => BrowserDevice::Plugin((*plugin).clone()),
        }
    }
}

fn items<'a>(category: Category, catalog: &'a [PluginDescriptor], query: &str) -> Vec<Item<'a>> {
    let query = query.trim().to_lowercase();
    let mut items = Vec::new();
    for kind in BuiltinKind::ALL {
        if matches!(
            (category, kind.descriptor().kind),
            (Category::Instruments, PluginKind::Instrument)
                | (Category::AudioEffects, PluginKind::Effect)
        ) {
            items.push(Item::Builtin(kind));
        }
    }
    let mut plugins: Vec<&PluginDescriptor> = catalog
        .iter()
        .filter(|plugin| match category {
            Category::Instruments => plugin.kind == PluginKind::Instrument,
            Category::AudioEffects => plugin.kind == PluginKind::Effect,
            Category::Plugins => true,
            Category::Custom | Category::Samples => false,
        })
        .collect();
    plugins.sort_by_cached_key(|plugin| {
        (
            vendor_name(plugin).to_lowercase(),
            plugin.name.to_lowercase(),
            plugin.name.clone(),
            plugin.path.clone(),
            plugin.class_id.clone(),
        )
    });
    items.extend(plugins.into_iter().map(Item::Plugin));
    items.retain(|item| {
        query.is_empty()
            || format!("{} {}", item.name(), item.detail())
                .to_lowercase()
                .contains(&query)
    });
    items
}

fn vendor_name(plugin: &PluginDescriptor) -> &str {
    let vendor = plugin.vendor.trim();
    if vendor.is_empty() {
        "Unknown Vendor"
    } else {
        vendor
    }
}

struct VendorGroup<'a> {
    key: String,
    name: &'a str,
    items: Vec<Item<'a>>,
}

fn vendor_groups<'a>(items: &[Item<'a>]) -> Vec<VendorGroup<'a>> {
    let mut groups = BTreeMap::<String, VendorGroup<'a>>::new();
    for item in items {
        let Item::Plugin(plugin) = item else {
            continue;
        };
        let name = vendor_name(plugin);
        let key = name.to_lowercase();
        let group = groups.entry(key.clone()).or_insert_with(|| VendorGroup {
            key,
            name,
            items: Vec::new(),
        });
        group.name = group.name.min(name);
        group.items.push(*item);
    }
    groups.into_values().collect()
}

impl BrowserState {
    pub fn load_folders(&mut self) -> Result<(), String> {
        self.folders.load_user_preferences()
    }

    pub fn add_folder(&mut self, path: PathBuf) -> Result<(), String> {
        self.folders.add_root(path.clone())?;
        let path = path.canonicalize().unwrap_or(path);
        self.expanded_folders.insert(path);
        self.category = Category::Custom;
        self.query.clear();
        self.selected = None;
        Ok(())
    }

    pub fn remove_folder(&mut self, path: &Path) -> Result<(), String> {
        self.folders.remove_root(path)?;
        self.expanded_folders
            .retain(|folder| !folder.starts_with(path));
        self.selected = None;
        Ok(())
    }

    fn vendor_is_open(&self, key: &str) -> bool {
        if self.query.trim().is_empty() {
            self.expanded_vendors.contains(key)
        } else {
            !self.search_collapsed_vendors.contains(key)
        }
    }

    fn toggle_vendor(&mut self, key: &str) {
        let set = if self.query.trim().is_empty() {
            &mut self.expanded_vendors
        } else {
            &mut self.search_collapsed_vendors
        };
        if !set.remove(key) {
            set.insert(key.to_owned());
        }
    }
}

fn vendor_row(ui: &mut Ui, state: &mut BrowserState, group: &VendorGroup<'_>) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let response = ui.interact(
        rect,
        Id::new(("browser-vendor", &group.key)),
        Sense::click(),
    );
    if response.clicked() {
        state.toggle_vendor(&group.key);
        if !state.vendor_is_open(&group.key)
            && group
                .items
                .iter()
                .any(|item| state.selected == Some(item.id()))
        {
            state.selected = None;
        }
    }
    let open = state.vendor_is_open(&group.key);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, group.name)
    });
    custom::row_highlight(ui.painter(), rect, false, response.hovered());
    let center = pos2(rect.left() + 11.0, rect.center().y);
    let points = if open {
        vec![
            center + vec2(-3.5, -2.0),
            center + vec2(3.5, -2.0),
            center + vec2(0.0, 2.5),
        ]
    } else {
        vec![
            center + vec2(-2.0, -3.5),
            center + vec2(2.5, 0.0),
            center + vec2(-2.0, 3.5),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        theme::TEXT_DIM,
        Stroke::NONE,
    ));
    let folder = Rect::from_center_size(
        pos2(rect.left() + 25.0, rect.center().y + 1.0),
        vec2(10.0, 7.0),
    );
    ui.painter().rect_stroke(
        folder,
        1.0,
        Stroke::new(1.0, theme::TEXT_DIM),
        egui::StrokeKind::Inside,
    );
    ui.painter().line_segment(
        [
            folder.left_top() + vec2(0.0, -2.0),
            folder.left_top() + vec2(5.0, -2.0),
        ],
        Stroke::new(1.0, theme::TEXT_DIM),
    );
    let count = ui.painter().layout_no_wrap(
        group.items.len().to_string(),
        theme::prop(9.5),
        theme::TEXT_DIM,
    );
    let count_left = rect.right() - count.size().x - 10.0;
    ui.painter().galley(
        pos2(count_left, rect.center().y - count.size().y / 2.0),
        count,
        theme::TEXT_DIM,
    );
    widgets::fitted_text(
        ui.painter(),
        Rect::from_min_max(
            pos2(rect.left() + 36.0, rect.top()),
            pos2(count_left - 8.0, rect.bottom()),
        ),
        group.name,
        theme::prop(11.0),
        theme::TEXT,
        Align2::LEFT_CENTER,
    );
    response.on_hover_text(format!(
        "{} · {} plug-ins\nClick to {}",
        group.name,
        group.items.len(),
        if open { "collapse" } else { "expand" }
    ));
    open
}

fn device_row(
    ui: &mut Ui,
    state: &mut BrowserState,
    item: Item<'_>,
    nested: bool,
    inputs: &Inputs<'_>,
    actions: &mut Vec<Action>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let response = ui.interact(rect, item.id(), Sense::click_and_drag());
    if response.clicked() || response.drag_started() {
        state.selected = Some(item.id());
    }
    if !inputs.loading {
        response.dnd_set_drag_payload(DeviceDrag::Browser(item.device()));
    }
    let selected = state.selected == Some(item.id());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            item.name(),
        )
    });
    custom::row_highlight(ui.painter(), rect, selected, response.hovered());
    let indent = if nested { 18.0 } else { 0.0 };
    let color = if selected {
        egui::Color32::WHITE
    } else {
        theme::TEXT_DIM
    };
    let glyph = Rect::from_center_size(
        pos2(rect.left() + indent + 12.0, rect.center().y),
        vec2(7.0, 7.0),
    );
    if item.is_instrument() {
        ui.painter().rect_filled(glyph, 1.0, color);
    } else {
        ui.painter().rect_stroke(
            glyph,
            1.0,
            Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
    }
    widgets::fitted_text(
        ui.painter(),
        Rect::from_min_max(
            pos2(rect.left() + indent + 22.0, rect.top()),
            rect.max - vec2(4.0, 0.0),
        ),
        item.name(),
        theme::prop(11.0),
        if selected {
            egui::Color32::WHITE
        } else {
            theme::TEXT
        },
        Align2::LEFT_CENTER,
    );
    let response = response.on_hover_text(format!(
        "{} · {}\nDouble-click to load onto the selected track\nDrag onto a track or into its effect chain",
        item.name(),
        item.detail()
    ));
    if response.double_clicked()
        && !response.dragged()
        && !response.drag_stopped()
        && !inputs.loading
        && let Ok(action) = load(item, target(inputs), inputs.project)
    {
        actions.push(match action {
            Action::LoadVst3 { track, plugin }
                if inputs.mixer && plugin.kind == PluginKind::Effect =>
            {
                Action::LoadMixerVst3 {
                    track,
                    plugin,
                    before: None,
                }
            }
            action => action,
        });
    }
}

fn target(inputs: &Inputs<'_>) -> Option<usize> {
    (inputs.selection.tracks.len() == 1)
        .then(|| inputs.selection.tracks.first().copied())
        .flatten()
        .filter(|index| *index < inputs.project.tracks().len())
}

fn load(item: Item<'_>, track: Option<usize>, project: &Project) -> Result<Action, String> {
    let Some(track) = track else {
        return Err("Select a track to load devices onto it".to_owned());
    };
    if track >= project.tracks().len() {
        return Err("Select a track to load devices onto it".to_owned());
    }
    let device = item.device();
    if !device.can_load_on(project, track) {
        return Err("Instruments load onto MIDI tracks".to_owned());
    }
    Ok(device.load_action(track, None))
}

pub fn show(ui: &mut Ui, state: &mut BrowserState, inputs: &Inputs<'_>, actions: &mut Vec<Action>) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, theme::BG_PANEL);

    let search = Rect::from_min_max(
        full.min + vec2(6.0, 6.0),
        pos2(full.right() - 6.0, full.top() + 26.0),
    );
    widgets::inside(
        ui,
        search,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .id(Id::new("browser-search"))
                    .hint_text(if state.category == Category::Custom {
                        "Filter files in open folders"
                    } else if state.category == Category::Samples {
                        "Search samples"
                    } else {
                        "Search name or vendor"
                    })
                    .margin(egui::Margin {
                        left: 22,
                        right: 6,
                        top: 2,
                        bottom: 2,
                    })
                    .desired_width(search.width()),
            );
            custom::decorate_search(ui.painter(), &response);
            if response.changed() {
                state.selected = None;
                state.search_collapsed_vendors.clear();
            }
        },
    );

    let footer_height = 38.0;
    let body = Rect::from_min_max(
        pos2(full.left(), search.bottom() + 6.0),
        pos2(full.right(), full.bottom() - footer_height),
    );
    let categories = body.with_max_y(body.top() + CATEGORY_HEIGHT * 2.0);
    let list = Rect::from_min_max(pos2(body.left(), categories.bottom() + 1.0), body.max);
    ui.painter().hline(
        full.x_range(),
        body.top() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    ui.painter().hline(
        body.x_range(),
        categories.bottom() + 0.5,
        Stroke::new(1.0, theme::BORDER),
    );

    for (row, bank) in Category::ALL.chunks(3).enumerate() {
        let labels: Vec<&str> = bank.iter().map(|category| category.label()).collect();
        let current = bank
            .iter()
            .position(|category| *category == state.category)
            .unwrap_or(bank.len());
        let strip = Rect::from_min_size(
            categories.min + vec2(0.0, row as f32 * CATEGORY_HEIGHT),
            vec2(categories.width(), CATEGORY_HEIGHT),
        );
        let chosen = widgets::inside(
            ui,
            strip.shrink2(vec2(6.0, 5.0)),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                crate::ui::rack::selector_bank(
                    ui,
                    &labels,
                    current,
                    |_| theme::SELECT,
                    |index| Id::new(("browser-category", bank[index])),
                )
            },
        );
        if chosen != current {
            if matches!(state.category, Category::Custom | Category::Samples)
                || matches!(bank[chosen], Category::Custom | Category::Samples)
            {
                state.query.clear();
                state.search_collapsed_vendors.clear();
            }
            state.category = bank[chosen];
            state.selected = None;
        }
    }

    let footer = Rect::from_min_max(pos2(full.left(), body.bottom()), full.max);
    if matches!(state.category, Category::Custom | Category::Samples) {
        custom::show(ui, state, list, footer, inputs, actions);
        ui.advance_cursor_after_rect(full);
        return;
    }

    let track = target(inputs);
    let shown = items(state.category, inputs.catalog, &state.query);
    if state
        .selected
        .is_some_and(|id| !shown.iter().any(|item| item.id() == id))
    {
        state.selected = None;
    }
    let mut list_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(list)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("browser-items")
        .auto_shrink([false, false])
        .show(&mut list_ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            for item in shown.iter().filter(|item| matches!(item, Item::Builtin(_))) {
                device_row(ui, state, *item, false, inputs, actions);
            }
            for group in vendor_groups(&shown) {
                if vendor_row(ui, state, &group) {
                    for item in group.items {
                        device_row(ui, state, item, true, inputs, actions);
                    }
                }
            }
            if shown.is_empty() {
                let message = if inputs.scan_status.is_some() {
                    "Scanning plug-ins…"
                } else if state.query.trim().is_empty() {
                    "No VST3 plug-ins found"
                } else {
                    "No matches"
                };
                custom::message(ui, message);
            }
        });

    ui.painter().hline(
        footer.x_range(),
        footer.top() + 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    let hint = if inputs.loading {
        "Loading plug-in…".to_owned()
    } else if let Some(status) = &inputs.scan_status {
        status.clone()
    } else {
        match state
            .selected
            .and_then(|id| shown.iter().find(|item| item.id() == id).copied())
            .map(|item| load(item, track, inputs.project))
        {
            Some(Err(reason)) => reason,
            _ => track.map_or_else(
                || "Select a track to load devices onto it".to_owned(),
                |track| {
                    let name = if track == 0 {
                        "Master".to_owned()
                    } else {
                        track_label(inputs.project, track)
                    };
                    if inputs.mixer && state.category != Category::Instruments {
                        format!("Double-click adds effects to {name}'s Mixer inserts")
                    } else {
                        format!("Double-click loads onto {name}")
                    }
                },
            ),
        }
    };
    widgets::fitted_text(
        ui.painter(),
        Rect::from_min_size(
            footer.min + vec2(8.0, 3.0),
            vec2(footer.width() - 16.0, 14.0),
        ),
        &hint,
        theme::prop(10.0),
        theme::TEXT_DIM,
        Align2::LEFT_CENTER,
    );
    widgets::inside(
        ui,
        Rect::from_min_max(
            pos2(footer.left() + 6.0, footer.top() + 17.0),
            footer.max - vec2(6.0, 2.0),
        ),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            if ui
                .small_button("Manage Plug-Ins…")
                .on_hover_text("Rescan, add VST3 folders, and see plug-ins that failed to load")
                .clicked()
            {
                let kind = if state.category == Category::Instruments {
                    PluginKind::Instrument
                } else {
                    PluginKind::Effect
                };
                actions.push(Action::BrowsePlugins {
                    track: track.unwrap_or(0),
                    kind,
                });
            }
        },
    );
    drag_preview(ui);
    ui.advance_cursor_after_rect(full);
}

fn drag_preview(ui: &Ui) {
    let Some(payload) = egui::DragAndDrop::payload::<DeviceDrag>(ui.ctx()) else {
        return;
    };
    let DeviceDrag::Browser(device) = payload.as_ref() else {
        return;
    };
    ui.ctx().output_mut(|output| {
        if output.cursor_icon != egui::CursorIcon::Copy {
            output.cursor_icon = egui::CursorIcon::NotAllowed;
        }
    });
    let Some(pointer) = ui.ctx().pointer_hover_pos() else {
        return;
    };
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        Id::new("browser-device-drag-preview"),
    ));
    let text = painter.layout_no_wrap(device.name().to_owned(), theme::prop(11.0), theme::TEXT);
    let rect = Rect::from_min_size(pointer + vec2(14.0, 16.0), text.size() + vec2(16.0, 10.0));
    painter.add(ui.visuals().popup_shadow.as_shape(rect, 6.0));
    painter.rect(
        rect,
        6.0,
        theme::BG_RAISED,
        Stroke::new(1.0, theme::SELECT),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + vec2(8.0, 5.0), text, theme::TEXT);
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod interaction_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plugin(name: &str, vendor: &str, kind: PluginKind) -> PluginDescriptor {
        PluginDescriptor {
            path: PathBuf::from(format!("/test/{name}.vst3")),
            class_id: name.to_owned(),
            name: name.to_owned(),
            vendor: vendor.to_owned(),
            kind,
        }
    }

    #[test]
    fn categories_filter_by_kind_and_search() {
        let catalog = [
            plugin("Reverb", "Acme", PluginKind::Effect),
            plugin("Bass Synth", "Zeta", PluginKind::Instrument),
            plugin("Comp", "Acme", PluginKind::Effect),
        ];
        let names = |category, query| -> Vec<String> {
            items(category, &catalog, query)
                .iter()
                .map(|item| item.name().to_owned())
                .collect()
        };
        assert_eq!(
            names(Category::Instruments, ""),
            ["Oscil", "Sampler", "Slicer", "Rain", "Ferro", "Bass Synth"]
        );
        assert_eq!(
            names(Category::AudioEffects, ""),
            ["EQ", "Comp", "Vari-Mu", "Reson", "Calor", "Comp", "Reverb"]
        );
        assert_eq!(
            names(Category::Plugins, ""),
            ["Comp", "Reverb", "Bass Synth"]
        );
        assert_eq!(names(Category::Plugins, "acme"), ["Comp", "Reverb"]);
    }

    #[test]
    fn vendor_folders_normalize_metadata_and_sort_their_devices() {
        let catalog = [
            plugin("Reverb", " acme ", PluginKind::Effect),
            plugin("Synth", "Zeta", PluginKind::Instrument),
            plugin("Compressor", "Acme", PluginKind::Effect),
            plugin("Mystery", "  ", PluginKind::Effect),
            plugin("Another", "", PluginKind::Effect),
        ];
        let shown = items(Category::Plugins, &catalog, "");
        let groups = vendor_groups(&shown);
        assert_eq!(
            groups
                .iter()
                .map(|group| (group.key.as_str(), group.name, group.items.len()))
                .collect::<Vec<_>>(),
            [
                ("acme", "Acme", 2),
                ("unknown vendor", "Unknown Vendor", 2),
                ("zeta", "Zeta", 1)
            ]
        );
        assert_eq!(
            groups[0].items.iter().map(Item::name).collect::<Vec<_>>(),
            ["Compressor", "Reverb"]
        );

        let filtered = items(Category::Plugins, &catalog, " ACME ");
        let groups = vendor_groups(&filtered);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
        let filtered = items(Category::Plugins, &catalog, "reverb");
        let groups = vendor_groups(&filtered);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items[0].name(), "Reverb");
        assert_eq!(
            items(Category::Plugins, &catalog, "unknown vendor").len(),
            2
        );
        assert!(items(Category::Plugins, &catalog, "missing").is_empty());
        let instruments = items(Category::Instruments, &catalog, "");
        assert!(matches!(instruments[0], Item::Builtin(BuiltinKind::Oscil)));
        assert_eq!(vendor_groups(&instruments).len(), 1);
    }

    #[test]
    fn instruments_only_load_onto_midi_tracks() {
        let mut project = Project::new();
        project.add_audio_track("Audio");
        project.add_midi_track("Keys");
        assert!(load(Item::Builtin(BuiltinKind::Oscil), Some(1), &project).is_err());
        assert!(load(Item::Builtin(BuiltinKind::Oscil), None, &project).is_err());
        assert!(matches!(
            load(Item::Builtin(BuiltinKind::Oscil), Some(2), &project),
            Ok(Action::LoadVst3 { track: 2, .. })
        ));
        let effect = plugin("Comp", "Acme", PluginKind::Effect);
        assert!(matches!(
            load(Item::Plugin(&effect), Some(1), &project),
            Ok(Action::LoadVst3 { track: 1, .. })
        ));
    }
}
