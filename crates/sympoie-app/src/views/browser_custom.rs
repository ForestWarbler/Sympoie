use super::*;
use crate::views::browser_folders::{FolderEntry, is_audio_file};

pub(super) fn show(
    ui: &mut Ui,
    state: &mut BrowserState,
    list: Rect,
    footer: Rect,
    inputs: &Inputs<'_>,
    actions: &mut Vec<Action>,
) {
    let mut list_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(list)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    list_ui.set_clip_rect(ui.clip_rect().intersect(list));
    egui::ScrollArea::vertical()
        .id_salt(("browser-files", state.category))
        .auto_shrink([false, false])
        .show(&mut list_ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            if state.category == Category::Samples {
                message(ui, "No built-in samples yet");
            } else if state.folders.roots().is_empty() {
                message(ui, "Add folders to browse your own files.");
            } else {
                for root in state.folders.roots().to_vec() {
                    folder(ui, state, &root, &root, 0, inputs, actions);
                }
            }
        });

    ui.painter().hline(
        footer.x_range(),
        footer.top() + 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    if state.category == Category::Samples {
        return;
    }
    widgets::fitted_text(
        ui.painter(),
        Rect::from_min_size(
            footer.min + vec2(8.0, 3.0),
            vec2(footer.width() - 16.0, 14.0),
        ),
        "Double-click WAV files to import",
        theme::prop(10.0),
        theme::TEXT_DIM,
        Align2::LEFT_CENTER,
    );
    widgets::inside(
        ui,
        Rect::from_min_max(footer.min + vec2(6.0, 17.0), footer.max - vec2(6.0, 2.0)),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            if ui.small_button("Add Folder…").clicked() {
                actions.push(Action::AddLibraryFolder);
            }
            if ui
                .small_button("Refresh")
                .on_hover_text("Reload folder contents from disk")
                .clicked()
            {
                state.folders.refresh();
                state.selected = None;
            }
        },
    );
}

fn message(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).size(11.0).color(theme::TEXT_DIM));
}

fn row_rect(ui: &mut Ui, depth: usize) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let indent = (depth as f32 * 12.0).min((rect.width() - 90.0).max(0.0));
    rect.with_min_x(rect.left() + indent)
}

fn folder(
    ui: &mut Ui,
    state: &mut BrowserState,
    root: &Path,
    path: &Path,
    depth: usize,
    inputs: &Inputs<'_>,
    actions: &mut Vec<Action>,
) {
    let rect = row_rect(ui, depth);
    let is_root = path == root;
    let remove_rect = Rect::from_min_max(pos2(rect.right() - ROW_HEIGHT, rect.top()), rect.max);
    let label_rect = if is_root {
        rect.with_max_x(remove_rect.left())
    } else {
        rect
    };
    let id = Id::new(("browser-folder", root, path));
    let response = ui.interact(label_rect, id, Sense::click());
    if response.clicked() {
        if !state.expanded_folders.remove(path) {
            state.expanded_folders.insert(path.to_owned());
        }
        state.selected = Some(id);
    }
    let open = state.expanded_folders.contains(path);
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::CollapsingHeader,
            true,
            open,
            name.as_ref(),
        )
    });
    if state.selected == Some(id) {
        ui.painter()
            .rect_filled(label_rect, 0.0, ui.visuals().selection.bg_fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(label_rect, 0.0, theme::BG_WIDGET_HOVER);
    }
    let center = pos2(rect.left() + 9.0, rect.center().y);
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
    let glyph = Rect::from_center_size(
        pos2(rect.left() + 23.0, rect.center().y + 1.0),
        vec2(10.0, 7.0),
    );
    ui.painter().rect_stroke(
        glyph,
        0.0,
        Stroke::new(1.0, theme::TEXT_DIM),
        egui::StrokeKind::Inside,
    );
    ui.painter().line_segment(
        [
            glyph.left_top() + vec2(0.0, -2.0),
            glyph.left_top() + vec2(5.0, -2.0),
        ],
        Stroke::new(1.0, theme::TEXT_DIM),
    );
    widgets::fitted_text(
        ui.painter(),
        label_rect
            .with_min_x(rect.left() + 34.0)
            .shrink2(vec2(2.0, 0.0)),
        &name,
        theme::prop(11.0),
        theme::TEXT,
        Align2::LEFT_CENTER,
    );
    response.on_hover_text(path.display().to_string());
    if is_root {
        let remove = ui.interact(
            remove_rect,
            Id::new(("browser-remove-folder", root)),
            Sense::click(),
        );
        remove.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Remove folder from Custom")
        });
        if remove.hovered() {
            ui.painter()
                .rect_filled(remove_rect, 0.0, theme::BG_WIDGET_HOVER);
        }
        ui.painter().text(
            remove_rect.center(),
            Align2::CENTER_CENTER,
            "×",
            theme::prop(12.0),
            theme::TEXT_DIM,
        );
        if remove
            .on_hover_text("Remove from Custom (keeps files on disk)")
            .clicked()
        {
            actions.push(Action::RemoveLibraryFolder(root.to_owned()));
        }
    }
    if !open {
        return;
    }
    let entries = match state.folders.entries(path) {
        Ok(entries) => entries.to_vec(),
        Err(error) => {
            message(ui, "Folder unavailable — try Refresh");
            ui.label(egui::RichText::new(error).small().color(theme::TEXT_DIM));
            return;
        }
    };
    let query = state.query.trim().to_lowercase();
    let mut visible = 0;
    for entry in entries {
        if entry.is_dir {
            folder(ui, state, root, &entry.path, depth + 1, inputs, actions);
        } else if query.is_empty() || entry.name.to_lowercase().contains(&query) {
            file(ui, state, root, &entry, depth + 1, inputs, actions);
        } else {
            continue;
        }
        visible += 1;
    }
    if visible == 0 {
        let rect = row_rect(ui, depth + 1);
        widgets::fitted_text(
            ui.painter(),
            rect.shrink2(vec2(8.0, 0.0)),
            if query.is_empty() {
                "Empty folder"
            } else {
                "No matching files"
            },
            theme::prop(10.0),
            theme::TEXT_DIM,
            Align2::LEFT_CENTER,
        );
    }
}

fn file(
    ui: &mut Ui,
    state: &mut BrowserState,
    root: &Path,
    entry: &FolderEntry,
    depth: usize,
    inputs: &Inputs<'_>,
    actions: &mut Vec<Action>,
) {
    let path = entry.path.as_path();
    let name = entry.name.as_str();
    let rect = row_rect(ui, depth);
    let id = Id::new(("browser-file", root, path));
    let response = ui.interact(rect, id, Sense::click());
    if response.clicked() {
        state.selected = Some(id);
    }
    let selected = state.selected == Some(id);
    let supported = is_audio_file(path);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name)
    });
    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, theme::BG_WIDGET_HOVER);
    }
    widgets::fitted_text(
        ui.painter(),
        rect.with_min_x(rect.left() + 20.0).shrink2(vec2(4.0, 0.0)),
        name,
        theme::prop(11.0),
        if supported {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
        Align2::LEFT_CENTER,
    );
    let hint = if supported {
        "Double-click to import audio at the playhead"
    } else {
        "Only WAV audio can currently be imported"
    };
    if response
        .on_hover_text(format!("{}\n{hint}", path.display()))
        .double_clicked()
        && supported
    {
        actions.push(Action::ImportAudio {
            paths: vec![path.to_owned()],
            track: target(inputs).filter(|&track| {
                matches!(
                    inputs.project.tracks()[track],
                    sympoie::track::Track::Audio(_)
                )
            }),
            start: None,
        });
    }
}
