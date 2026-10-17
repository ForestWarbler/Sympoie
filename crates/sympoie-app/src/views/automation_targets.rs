use std::collections::HashSet;

use eframe::egui::{self, Align2, Id, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use sympoie::automation::{AutomationTarget, SubtractiveParameter};
use sympoie::plugin::PluginInstance;
use sympoie::track::Track;

use super::RED;
use crate::ui::{theme, widgets};

fn plugins(track: &Track) -> impl Iterator<Item = &PluginInstance> {
    let instrument = match track {
        Track::Midi(midi) => midi.plugin_instrument(),
        _ => None,
    };
    instrument.into_iter().chain(track.all_effects())
}

fn plugin_label(track: &Track, plugin: &PluginInstance) -> String {
    let mut count = 0;
    let mut ordinal = 0;
    for instance in plugins(track).filter(|p| p.descriptor.name == plugin.descriptor.name) {
        count += 1;
        if instance.id == plugin.id {
            ordinal = count;
        }
    }
    if count > 1 {
        format!("{} ({ordinal})", plugin.descriptor.name)
    } else {
        plugin.descriptor.name.clone()
    }
}

fn selected_label(track: &Track, target: AutomationTarget) -> String {
    match target {
        AutomationTarget::TrackGain => "Track · Volume".into(),
        AutomationTarget::TrackPan => "Track · Pan".into(),
        AutomationTarget::PluginParameter { plugin, parameter } => plugins(track)
            .find(|p| p.id == plugin)
            .and_then(|p| {
                p.parameters
                    .iter()
                    .find(|info| info.id == parameter)
                    .map(|info| format!("{} · {}", plugin_label(track, p), info.name))
            })
            .unwrap_or_else(|| target.label(track)),
        _ => target.label(track),
    }
}

pub(super) fn selector(
    ui: &mut Ui,
    width: f32,
    index: usize,
    track: &Track,
    target: &mut AutomationTarget,
) -> Response {
    let label = selected_label(track, *target);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    let response = ui.interact(rect, Id::new(("automation-target", index)), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), &label)
    });
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect,
        visuals.corner_radius,
        visuals.weak_bg_fill,
        visuals.bg_stroke,
        StrokeKind::Inside,
    );
    widgets::fitted_text(
        ui.painter(),
        rect.shrink(4.0).with_max_x(rect.right() - 16.0),
        &label,
        theme::prop(10.5),
        RED,
        Align2::LEFT_CENTER,
    );
    let arrow = pos2(rect.right() - 8.0, rect.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            arrow + vec2(-3.0, -1.5),
            arrow + vec2(3.0, -1.5),
            arrow + vec2(0.0, 2.0),
        ],
        RED,
        Stroke::NONE,
    ));

    let popup_id = Id::new(("automation-target-menu", index));
    if response.clicked() && !egui::Popup::is_id_open(ui.ctx(), popup_id) {
        for plugin in plugins(track) {
            ui.data_mut(|data| {
                data.remove::<String>(Id::new(("automation-target-search", index, plugin.id)));
            });
        }
    }
    let mut chosen = None;
    egui::Popup::menu(&response)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(180.0);
            let automated: HashSet<_> = track
                .automation()
                .iter()
                .filter(|lane| !lane.points().is_empty())
                .map(|lane| lane.target())
                .collect();
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    ui.push_id("track", |ui| {
                        ui.menu_button("Track", |ui| {
                            for (candidate, name) in [
                                (AutomationTarget::TrackGain, "Volume"),
                                (AutomationTarget::TrackPan, "Pan"),
                            ] {
                                if parameter(ui, name, candidate, *target, &automated).clicked() {
                                    chosen = Some(candidate);
                                }
                            }
                        });
                    });
                    if let Track::Midi(midi) = track
                        && midi.instrument().is_some()
                    {
                        ui.push_id("subtractive", |ui| {
                            ui.menu_button("Subtractive", |ui| {
                                for value in SubtractiveParameter::ALL {
                                    let candidate = AutomationTarget::Subtractive(value);
                                    if parameter(ui, value.label(), candidate, *target, &automated)
                                        .clicked()
                                    {
                                        chosen = Some(candidate);
                                    }
                                }
                            });
                        });
                    }
                    for plugin in plugins(track) {
                        ui.push_id(plugin.id, |ui| {
                            ui.menu_button(plugin_label(track, plugin), |ui| {
                                if let Some(candidate) =
                                    plugin_menu(ui, index, plugin, *target, &automated)
                                {
                                    chosen = Some(candidate);
                                }
                            });
                        });
                    }
                });
        });
    if let Some(candidate) = chosen {
        *target = candidate;
        egui::Popup::close_id(ui.ctx(), popup_id);
    }
    response.on_hover_text(format!(
        "{label}\nChoose any track or device parameter to draw its envelope."
    ))
}

fn parameter(
    ui: &mut Ui,
    name: &str,
    candidate: AutomationTarget,
    selected: AutomationTarget,
    automated: &HashSet<AutomationTarget>,
) -> Response {
    let label = if automated.contains(&candidate) {
        format!("● {name}")
    } else {
        name.to_owned()
    };
    ui.push_id(candidate, |ui| {
        ui.selectable_label(candidate == selected, label)
    })
    .inner
}

fn plugin_menu(
    ui: &mut Ui,
    index: usize,
    plugin: &PluginInstance,
    selected: AutomationTarget,
    automated: &HashSet<AutomationTarget>,
) -> Option<AutomationTarget> {
    ui.set_width(280.0);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    let search_id = Id::new(("automation-target-search", index, plugin.id));
    let mut search = ui
        .data(|data| data.get_temp::<String>(search_id))
        .unwrap_or_default();
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut search)
                .id(search_id)
                .hint_text("Search parameters")
                .desired_width(f32::INFINITY),
        )
        .changed();
    ui.data_mut(|data| data.insert_temp(search_id, search.clone()));
    let query = search.trim().to_lowercase();
    let parameters: Vec<_> = plugin
        .parameters
        .iter()
        .filter(|parameter| {
            !parameter.read_only
                && (query.is_empty() || parameter.name.to_lowercase().contains(&query))
        })
        .collect();
    ui.separator();
    if parameters.is_empty() {
        ui.weak(if query.is_empty() {
            "No writable parameters"
        } else {
            "No matching parameters"
        });
        return None;
    }
    let mut chosen = None;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("parameters", plugin.id))
        .max_height(320.0);
    if changed {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show_rows(
        ui,
        ui.spacing().interact_size.y,
        parameters.len(),
        |ui, rows| {
            for info in &parameters[rows] {
                let candidate = AutomationTarget::PluginParameter {
                    plugin: plugin.id,
                    parameter: info.id,
                };
                let description = if info.unit.is_empty() {
                    info.name.clone()
                } else {
                    format!("{} ({})", info.name, info.unit)
                };
                let response = parameter(ui, &info.name, candidate, selected, automated)
                    .on_hover_text(description);
                if response.clicked() {
                    chosen = Some(candidate);
                }
            }
        },
    );
    chosen
}
