use std::collections::BTreeSet;

use eframe::egui::{
    self, Align, Align2, Color32, Id, Layout, Rect, Response, RichText, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, pos2, vec2,
};
use sympoie::builtin::BuiltinKind;
use sympoie::plugin::{PluginInstance, PluginKind};
use sympoie::project::Project;
use sympoie::track::MixerSettings;

use crate::actions::Action;
use crate::document::rows;
use crate::plugins::PluginRuntime;
use crate::state::Selection;
use crate::ui::widgets::{self, MeterState};
use crate::ui::{analog, format, rack, theme};
use crate::views::builtin_devices;
use crate::views::device_drag::{BrowserDevice, DeviceDrag};
use crate::views::track_controls::route_combo;

const STRIP_WIDTH: f32 = 122.0;
const TITLE_HEIGHT: f32 = 22.0;
const HEADER_HEIGHT: f32 = 28.0;
const SLOT_HEIGHT: f32 = 21.0;
const LIST_PADDING: f32 = 4.0;
const MIN_GAIN_DB: f64 = -70.0;

#[path = "mixer_sends.rs"]
mod sends;

#[cfg(test)]
#[path = "mixer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mixer_builtin_tests.rs"]
mod builtin_tests;

#[derive(Clone)]
struct MixerEffectDrag {
    track: usize,
    id: u64,
}

#[cfg(test)]
pub fn show(
    ui: &mut Ui,
    project: &Project,
    selection: &Selection,
    folded: &std::collections::HashSet<usize>,
    meter: &mut MeterState,
    actions: &mut Vec<Action>,
) {
    show_inner(
        ui,
        project,
        selection,
        folded,
        meter,
        &mut [],
        None,
        actions,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn show_with_runtime(
    ui: &mut Ui,
    project: &Project,
    selection: &Selection,
    folded: &std::collections::HashSet<usize>,
    meter: &mut MeterState,
    track_meters: &mut [MeterState],
    runtime: &PluginRuntime,
    actions: &mut Vec<Action>,
) {
    show_inner(
        ui,
        project,
        selection,
        folded,
        meter,
        track_meters,
        Some(runtime),
        actions,
    );
}

#[allow(clippy::too_many_arguments)]
fn show_inner(
    ui: &mut Ui,
    project: &Project,
    selection: &Selection,
    folded: &std::collections::HashSet<usize>,
    meter: &mut MeterState,
    track_meters: &mut [MeterState],
    runtime: Option<&PluginRuntime>,
    actions: &mut Vec<Action>,
) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, theme::BG_DEEP);
    let header = Rect::from_min_size(full.min, vec2(full.width(), HEADER_HEIGHT));
    analog::gradient_rect(ui.painter(), header, theme::BG_RAISED, theme::BG_PANEL);
    ui.painter().hline(
        header.x_range(),
        header.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    widgets::inside(
        ui,
        header.shrink2(vec2(8.0, 3.0)),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(
                RichText::new("MIXER")
                    .size(10.0)
                    .strong()
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(8.0);
            let add = ui.button("+ Bus").on_hover_text(
                "Add a mixer bus. Route channels to it with their Output; buses only appear in the Mixer.",
            );
            ui.interact(add.rect, Id::new("mixer-add-bus"), Sense::hover());
            if add.clicked() {
                actions.push(Action::AddBusTrack);
            }
            if header.width() > 520.0 {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Mix inserts stay on the channel when bouncing a track.")
                        .size(10.0)
                        .color(theme::TEXT_FAINT),
                );
            }
        },
    );
    let mut channels = full.with_min_y(header.bottom());
    let inspector = selected_builtin(ui, project, selection).map(|(track, plugin, kind)| {
        let available = (channels.width() - 2.0 * STRIP_WIDTH - 28.0)
            .max(160.0)
            .min(channels.width() * 0.68);
        let width = builtin_devices::card_width(kind).min(available);
        let area = channels.with_min_x(channels.right() - width);
        channels.set_right(area.left() - 8.0);
        (area, track, plugin, kind)
    });
    let master_rect = Rect::from_min_max(
        pos2(channels.right() - STRIP_WIDTH - 20.0, channels.top()),
        channels.max,
    );
    let strips_rect = channels.with_max_x(master_rect.left() - 4.0);

    let mut strips_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(strips_rect)
            .layout(Layout::left_to_right(Align::Min)),
    );
    egui::ScrollArea::horizontal()
        .id_salt("mixer-strips")
        .auto_shrink([false, false])
        .show(&mut strips_ui, |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let rows = rows::mixer_rows(project, folded);
            if rows.is_empty() {
                ui.add_space(16.0);
                ui.label(
                    RichText::new("Tracks and buses appear here as channel strips.")
                        .color(theme::TEXT_FAINT),
                );
            }
            for row in rows {
                let selected = selection.tracks.contains(&row.index);
                let meter = track_meters.get_mut(row.index);
                strip(ui, project, row.index, selected, meter, actions);
            }
        });

    let mut master_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(master_rect)
            .layout(Layout::left_to_right(Align::Min)),
    );
    strip(
        &mut master_ui,
        project,
        0,
        selection.tracks.contains(&0),
        Some(meter),
        actions,
    );
    if let Some((area, track, plugin, kind)) = inspector {
        builtin_inspector(ui, area, project, track, plugin, kind, runtime, actions);
    }
    ui.advance_cursor_after_rect(full);
}

fn inspector_id() -> Id {
    Id::new("mixer-builtin-inspector")
}

fn selected_builtin<'a>(
    ui: &Ui,
    project: &'a Project,
    selection: &Selection,
) -> Option<(usize, &'a PluginInstance, BuiltinKind)> {
    let id = ui.ctx().data(|data| data.get_temp::<u64>(inspector_id()))?;
    let existing = project
        .tracks()
        .iter()
        .enumerate()
        .find_map(|(track, source)| {
            source
                .mixer_effects()
                .iter()
                .find(|plugin| plugin.id == id)
                .and_then(|plugin| {
                    BuiltinKind::from_descriptor(&plugin.descriptor)
                        .map(|kind| (track, plugin, kind))
                })
        });
    let Some((track, plugin, kind)) = existing else {
        ui.ctx().data_mut(|data| data.remove::<u64>(inspector_id()));
        return None;
    };
    (selection.tracks.len() == 1 && selection.tracks.contains(&track))
        .then_some((track, plugin, kind))
}

fn open_builtin(ui: &Ui, track: usize, id: u64, actions: &mut Vec<Action>) {
    ui.ctx()
        .data_mut(|data| data.insert_temp(inspector_id(), id));
    actions.push(Action::SelectClip(None));
    actions.push(Action::SelectTracks(BTreeSet::from([track])));
}

#[allow(clippy::too_many_arguments)]
fn builtin_inspector(
    ui: &mut Ui,
    area: Rect,
    project: &Project,
    track: usize,
    plugin: &PluginInstance,
    kind: BuiltinKind,
    runtime: Option<&PluginRuntime>,
    actions: &mut Vec<Action>,
) {
    let panel =
        area.with_max_y((area.top() + crate::views::devices::PANEL_HEIGHT).min(area.bottom()));
    ui.painter().add(rack::faceplate(
        panel,
        Some(builtin_devices::accent(kind)),
        plugin.bypassed,
    ));
    ui.interact(panel, Id::new("mixer-builtin-panel"), Sense::hover());
    let header = panel
        .with_max_y(panel.top() + HEADER_HEIGHT)
        .shrink2(vec2(8.0, 2.0));
    widgets::inside(ui, header, Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let routing_width =
            if matches!(kind, BuiltinKind::Comp | BuiltinKind::VariMu) && runtime.is_some() {
                84.0
            } else {
                0.0
            };
        ui.add_sized(
            vec2((header.width() - 74.0 - routing_width).max(20.0), 20.0),
            egui::Label::new(
                RichText::new(format!(
                    "{} · {}",
                    plugin.descriptor.name,
                    project.tracks()[track].name()
                ))
                .strong()
                .size(11.0)
                .color(theme::TEXT),
            )
            .truncate(),
        );
        if matches!(kind, BuiltinKind::Comp | BuiltinKind::VariMu)
            && let Some(runtime) = runtime
        {
            crate::views::devices::inline_sidechain_button(
                ui, project, runtime, track, plugin, actions,
            );
        }
        let enabled = widgets::toggle(ui, "ON", !plugin.bypassed, theme::ACCENT, vec2(34.0, 20.0));
        ui.interact(
            enabled.rect,
            Id::new(("mixer-builtin-enabled", plugin.id)),
            Sense::hover(),
        );
        if enabled
            .on_hover_text("Enable / bypass this mixer effect")
            .clicked()
        {
            actions.push(Action::SetVst3Bypassed {
                id: plugin.id,
                bypassed: !plugin.bypassed,
            });
        }
        let close = ui.small_button("×").on_hover_text("Close controls");
        ui.interact(close.rect, Id::new("mixer-builtin-close"), Sense::hover());
        if close.clicked() {
            ui.ctx().data_mut(|data| data.remove::<u64>(inspector_id()));
        }
    });
    let body = Rect::from_min_max(
        panel.min + vec2(8.0, HEADER_HEIGHT + 2.0),
        panel.max - vec2(8.0, 4.0),
    );
    let telemetry = runtime.and_then(|runtime| runtime.builtin_telemetry(plugin.id));
    widgets::inside(ui, body, Layout::top_down(Align::Min), |ui| {
        egui::ScrollArea::both()
            .id_salt(("mixer-builtin-controls", plugin.id))
            .auto_shrink([false, false])
            .max_height(body.height())
            .show(ui, |ui| {
                ui.set_width(builtin_devices::card_width(kind) - 16.0);
                builtin_devices::compact(ui, plugin, kind, telemetry.as_deref(), actions);
            });
    });
}

fn strip(
    ui: &mut Ui,
    project: &Project,
    index: usize,
    selected: bool,
    meter: Option<&mut MeterState>,
    actions: &mut Vec<Action>,
) {
    let height = ui.available_height();
    let width = STRIP_WIDTH + if meter.is_some() { 20.0 } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let response = ui.interact(rect, Id::new(("mixer-strip", index)), Sense::click());
    if response.clicked() {
        actions.push(Action::SelectClip(None));
        actions.push(Action::SelectTracks(BTreeSet::from([index])));
    }
    if response.secondary_clicked() {
        ui.ctx()
            .data_mut(|data| data.remove::<String>(Id::new(("mixer-rename-buffer", index))));
    }
    egui::Popup::context_menu(&response)
        .id(Id::new(("mixer-channel-menu", index)))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| channel_menu(ui, project, index, actions));
    let track = &project.tracks()[index];
    let color = theme::track_color(project, index);
    let painter = ui.painter_at(rect);
    painter.add(rack::faceplate(rect, None, false));
    if selected {
        painter.rect_filled(rect.shrink(1.0), 3.0, color.gamma_multiply(0.05));
        painter.rect_stroke(
            rect.shrink(0.5),
            3.0,
            Stroke::new(1.0, color.gamma_multiply(0.55)),
            StrokeKind::Inside,
        );
    }
    let title = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE_HEIGHT)).shrink(1.0);
    widgets::color_bar(
        &painter,
        title,
        if selected {
            theme::tint(color, Color32::WHITE, 0.2)
        } else {
            color
        },
        &rows::track_label(project, index),
    );
    ui.interact(title, Id::new(("mixer-title", index)), Sense::hover())
        .on_hover_text(format!(
            "{}\nRight-click to rename or delete this channel.",
            track.name()
        ));
    painter.text(
        rect.center_bottom() - vec2(0.0, 4.0),
        Align2::CENTER_BOTTOM,
        if index == 0 {
            "MASTER"
        } else {
            rows::kind_label(track)
        },
        theme::prop(9.5),
        theme::TEXT_FAINT,
    );

    let inner = Rect::from_min_max(
        pos2(rect.left() + 6.0, title.bottom() + 5.0),
        pos2(rect.right() - 6.0, rect.bottom() - 18.0),
    );
    widgets::inside(ui, inner, Layout::top_down(Align::Center), |ui| {
        ui.push_id(("mixer-channel-controls", index), |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let processing_height = (inner.height() * 0.4).clamp(82.0, 180.0);
            let inserts_height = (processing_height - 22.0) * 0.6;
            let sends_height = (processing_height - 22.0) * 0.4;
            mixer_inserts(ui, project, index, inner.width(), inserts_height, actions);
            sends::show(ui, project, index, inner.width(), sends_height, actions);
            ui.add_space(2.0);
            ui.separator();
            let route = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(RichText::new("To").size(10.0).color(theme::TEXT_DIM));
                route_combo(ui, project, index, inner.width() - 22.0, actions);
            });
            route.response.on_hover_text("Output");

            let mixer = *track.mixer();
            ui.allocate_ui_with_layout(
                vec2(inner.width(), 25.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let mute =
                        widgets::toggle(ui, "M", mixer.muted(), theme::WARN, vec2(23.0, 20.0))
                            .on_hover_text("Mute channel");
                    ui.interact(mute.rect, Id::new(("mixer-mute", index)), Sense::hover());
                    if mute.clicked() {
                        actions.push(Action::SetMixer {
                            track: index,
                            mixer: mixer.with_muted(!mixer.muted()),
                        });
                    }
                    let solo =
                        widgets::toggle(ui, "S", mixer.soloed(), theme::ACCENT, vec2(23.0, 20.0))
                            .on_hover_text("Solo channel");
                    ui.interact(solo.rect, Id::new(("mixer-solo", index)), Sense::hover());
                    if solo.clicked() {
                        actions.push(Action::SetMixer {
                            track: index,
                            mixer: mixer.with_soloed(!mixer.soloed()),
                        });
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (value_rect, _) =
                            ui.allocate_exact_size(vec2(27.0, 25.0), Sense::hover());
                        let mut pan = mixer.pan();
                        let knob = widgets::pan_knob(ui, &mut pan, 25.0);
                        ui.interact(knob.rect, Id::new(("mixer-pan", index)), Sense::hover());
                        if knob.changed()
                            && let Ok(next) = MixerSettings::new(mixer.gain(), pan, mixer.output())
                        {
                            actions.push(Action::SetMixer {
                                track: index,
                                mixer: next.with_muted(mixer.muted()).with_soloed(mixer.soloed()),
                            });
                        }
                        widgets::fitted_text(
                            ui.painter(),
                            value_rect,
                            &format::pan(pan),
                            theme::prop(9.5),
                            theme::TEXT_DIM,
                            Align2::RIGHT_CENTER,
                        );
                    });
                },
            );
            gain_readout(ui, index, mixer, actions);

            let fader_height = (ui.available_height() - 1.0).max(42.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let fader_width = 62.0;
                let controls_width = fader_width + if meter.is_some() { 19.0 } else { 0.0 };
                ui.add_space(((inner.width() - controls_width) / 2.0).max(0.0));
                let mut gain = mixer.gain();
                let fader =
                    widgets::gain_fader(ui, &mut gain, vec2(fader_width, fader_height), color);
                ui.interact(fader.rect, Id::new(("mixer-fader", index)), Sense::hover());
                if fader.changed() {
                    push_gain(actions, index, mixer, gain);
                }
                if let Some(meter) = meter {
                    let response = widgets::meter(ui, meter, vec2(13.0, fader_height), true);
                    ui.interact(
                        response.rect,
                        Id::new(("mixer-meter", index)),
                        Sense::hover(),
                    );
                }
            });
        });
    });
    ui.painter().vline(
        rect.right() + 0.5,
        rect.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );
}

fn gain_readout(ui: &mut Ui, index: usize, mixer: MixerSettings, actions: &mut Vec<Action>) {
    let cancel_id = Id::new(("mixer-gain-cancel", index));
    let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let cancelled = ui
        .data_mut(|data| data.remove_temp::<bool>(cancel_id))
        .unwrap_or(false)
        || escaped;
    let previous_db = f64::from(format::gain_to_db(mixer.gain())).max(MIN_GAIN_DB);
    let mut db = previous_db;
    let max_db = f64::from(format::gain_to_db(format::MAX_FADER_GAIN));
    let response = ui.add_sized(
        vec2(74.0, 20.0),
        egui::DragValue::new(&mut db)
            .speed(0.2)
            .range(MIN_GAIN_DB..=max_db)
            .clamp_existing_to_range(false)
            .update_while_editing(false)
            .custom_formatter(|value, _| {
                if value <= MIN_GAIN_DB {
                    "−∞ dB".to_owned()
                } else {
                    format!("{value:+.1} dB")
                }
            })
            .custom_parser(|text| (!cancelled).then(|| parse_gain_db(text)).flatten()),
    );
    ui.interact(
        response.rect,
        Id::new(("mixer-gain", index)),
        Sense::hover(),
    );
    if escaped && response.lost_focus() {
        ui.data_mut(|data| data.insert_temp(cancel_id, true));
    }
    if response.changed() && db != previous_db {
        let gain = if db <= MIN_GAIN_DB {
            0.0
        } else {
            format::db_to_gain(db as f32)
        };
        push_gain(actions, index, mixer, gain);
    }
    response.on_hover_text(
        "Channel volume. Drag to adjust; double-click to type dB. Use −∞ to silence.",
    );
}

fn parse_gain_db(text: &str) -> Option<f64> {
    let normalized = text.trim().replace('−', "-").to_ascii_lowercase();
    let value = normalized.trim_end_matches("db").trim();
    if matches!(value, "-∞" | "-inf" | "-infinity") {
        return Some(MIN_GAIN_DB);
    }
    value.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn list_container(ui: &mut Ui, width: f32, height: f32, id: Id) -> (Rect, Ui) {
    let height = height.max(SLOT_HEIGHT + 2.0 * LIST_PADDING);
    let (area, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.interact(area, id, Sense::hover());
    analog::well(ui.painter(), area, 3.0);
    let viewport = area.shrink(LIST_PADDING);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(viewport)
            .layout(Layout::top_down(Align::Min)),
    );
    child.spacing_mut().scroll = egui::style::ScrollStyle {
        bar_width: 4.0,
        floating_width: 3.0,
        floating_allocated_width: 6.0,
        bar_inner_margin: 2.0,
        ..egui::style::ScrollStyle::thin()
    };
    (viewport, child)
}

fn channel_menu(ui: &mut Ui, project: &Project, index: usize, actions: &mut Vec<Action>) {
    let rename_id = Id::new(("mixer-rename-buffer", index));
    let mut name = ui
        .ctx()
        .data_mut(|data| data.get_temp::<String>(rename_id))
        .unwrap_or_else(|| project.tracks()[index].name().to_owned());
    ui.label("Channel name");
    let edit = ui.add(
        egui::TextEdit::singleline(&mut name)
            .desired_width(160.0)
            .id(Id::new(("mixer-channel-name", index))),
    );
    ui.ctx()
        .data_mut(|data| data.insert_temp(rename_id, name.clone()));
    let enter = (edit.has_focus() || edit.lost_focus())
        && ui.input(|input| input.key_pressed(egui::Key::Enter));
    if ui.button("Rename").clicked() || enter {
        actions.push(Action::RenameTrack(index, name));
        ui.ctx().data_mut(|data| data.remove::<String>(rename_id));
        ui.close();
    }
    if index != 0 {
        ui.separator();
        let delete = ui.button("Delete channel");
        ui.interact(
            delete.rect,
            Id::new(("mixer-delete-channel", index)),
            Sense::hover(),
        );
        if delete.clicked() {
            actions.push(Action::DeleteTrack(index));
            ui.close();
        }
    }
}

fn mixer_inserts(
    ui: &mut Ui,
    project: &Project,
    track: usize,
    width: f32,
    height: f32,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(RichText::new("MIXER FX").size(9.0).color(theme::TEXT_DIM))
            .on_hover_text("Mixing inserts run after the track's Rack and before channel volume. Bouncing preserves them.");
        let add = ui.small_button("+").on_hover_text("Add a mixer effect");
        ui.interact(add.rect, Id::new(("mixer-effect-plus", track)), Sense::hover());
        if add.clicked() {
            actions.push(Action::BrowseMixerEffects { track });
        }
    });
    let (area, mut child) =
        list_container(ui, width, height, Id::new(("mixer-insert-list", track)));
    egui::ScrollArea::vertical()
        .id_salt(("mixer-inserts", track))
        .auto_shrink([false, false])
        .max_height(area.height())
        .min_scrolled_height(area.height())
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let effects = project.tracks()[track].mixer_effects();
            let mut slots = Vec::with_capacity(effects.len());
            for (position, plugin) in effects.iter().enumerate() {
                let response = effect_slot(ui, track, plugin, position, effects.len(), actions);
                slots.push((plugin.id, response.rect));
            }
            let (end, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), SLOT_HEIGHT), Sense::click());
            ui.interact(end, Id::new(("mixer-effect-add", track)), Sense::hover());
            ui.painter().rect(
                end,
                2.0,
                theme::BG_WIDGET,
                Stroke::new(1.0, theme::BORDER),
                StrokeKind::Inside,
            );
            widgets::fitted_text(
                ui.painter(),
                end.shrink2(vec2(4.0, 0.0)),
                "+ Audio FX",
                theme::prop(10.0),
                theme::TEXT_FAINT,
                Align2::CENTER_CENTER,
            );
            if response
                .on_hover_text("Add a mixer effect, or drop an audio effect here")
                .clicked()
            {
                actions.push(Action::BrowseMixerEffects { track });
            }
            insert_drop_target(ui, project, track, &slots, end, area, actions);
        });
}

fn effect_slot(
    ui: &mut Ui,
    track: usize,
    plugin: &PluginInstance,
    position: usize,
    count: usize,
    actions: &mut Vec<Action>,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), SLOT_HEIGHT), Sense::hover());
    let button = Rect::from_min_size(rect.min + vec2(3.0, 3.5), vec2(14.0, 14.0));
    let name_rect = Rect::from_min_max(pos2(button.right() + 3.0, rect.top()), rect.max);
    let response = ui.interact(
        name_rect,
        Id::new(("mixer-effect", plugin.id)),
        Sense::click_and_drag(),
    );
    let fill = if plugin.bypassed {
        theme::BG_WIDGET
    } else {
        Color32::from_rgb(48, 69, 81)
    };
    ui.painter().rect(
        rect,
        2.0,
        fill,
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    let activator = ui.interact(
        button,
        Id::new(("mixer-effect-bypass", plugin.id)),
        Sense::click(),
    );
    analog::round_switch(
        ui.painter(),
        button.center(),
        4.0,
        (!plugin.bypassed).then_some(theme::ACCENT),
        activator.hovered(),
        activator.is_pointer_button_down_on(),
    );
    if activator
        .on_hover_text("Enable / bypass this mixer effect")
        .clicked()
    {
        actions.push(Action::SetVst3Bypassed {
            id: plugin.id,
            bypassed: !plugin.bypassed,
        });
    }
    widgets::fitted_text(
        ui.painter(),
        name_rect.shrink2(vec2(3.0, 0.0)),
        &plugin.descriptor.name,
        theme::prop(10.0),
        if plugin.bypassed {
            theme::TEXT_DIM
        } else {
            theme::TEXT
        },
        Align2::LEFT_CENTER,
    );
    let builtin = BuiltinKind::from_descriptor(&plugin.descriptor).is_some();
    if builtin && response.double_clicked() {
        open_builtin(ui, track, plugin.id, actions);
    } else if response.clicked() {
        if builtin {
            actions.push(Action::SelectClip(None));
            actions.push(Action::SelectTracks(BTreeSet::from([track])));
        } else {
            actions.push(Action::OpenNativeVst3Editor(plugin.id));
        }
    }
    response.dnd_set_drag_payload(MixerEffectDrag {
        track,
        id: plugin.id,
    });
    response.context_menu(|ui| {
        ui.label(&plugin.descriptor.name);
        let open = ui.button(if builtin {
            "Open controls"
        } else {
            "Open editor"
        });
        ui.interact(
            open.rect,
            Id::new(("mixer-effect-open", plugin.id)),
            Sense::hover(),
        );
        if open.clicked() {
            if builtin {
                open_builtin(ui, track, plugin.id, actions);
            } else {
                actions.push(Action::OpenNativeVst3Editor(plugin.id));
            }
            ui.close();
        }
        if !builtin && ui.button("Parameters").clicked() {
            actions.push(Action::OpenVst3Editor(plugin.id));
            ui.close();
        }
        if !builtin && ui.button("Sidechain…").clicked() {
            actions.push(Action::OpenVst3Sidechain(plugin.id));
            ui.close();
        }
        if ui
            .button(if plugin.bypassed { "Enable" } else { "Bypass" })
            .clicked()
        {
            actions.push(Action::SetVst3Bypassed {
                id: plugin.id,
                bypassed: !plugin.bypassed,
            });
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(position > 0, egui::Button::new("Move up"))
            .clicked()
        {
            actions.push(Action::MoveMixerEffect {
                track,
                from: position,
                to: position - 1,
            });
            ui.close();
        }
        if ui
            .add_enabled(position + 1 < count, egui::Button::new("Move down"))
            .clicked()
        {
            actions.push(Action::MoveMixerEffect {
                track,
                from: position,
                to: position + 1,
            });
            ui.close();
        }
        ui.separator();
        if ui.button("Remove effect").clicked() {
            actions.push(Action::RemoveVst3 {
                track,
                id: plugin.id,
            });
            ui.close();
        }
    });
    response
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text(format!(
            "{}{}\n{} · Drag: reorder · Right-click: options",
            plugin.descriptor.name,
            if plugin.bypassed { " (bypassed)" } else { "" },
            if builtin {
                "Double-click: show controls"
            } else {
                "Click: open editor"
            },
        ))
}

fn move_effect_to_gap(
    project: &Project,
    track: usize,
    id: u64,
    before: Option<u64>,
) -> Option<Action> {
    let effects = project.tracks().get(track)?.mixer_effects();
    let from = effects.iter().position(|plugin| plugin.id == id)?;
    let gap = match before {
        Some(id) => effects.iter().position(|plugin| plugin.id == id)?,
        None => effects.len(),
    };
    let to = gap - usize::from(from < gap);
    (from != to).then_some(Action::MoveMixerEffect { track, from, to })
}

fn insert_drop_target(
    ui: &mut Ui,
    project: &Project,
    track: usize,
    slots: &[(u64, Rect)],
    end: Rect,
    area: Rect,
    actions: &mut Vec<Action>,
) {
    let browser = egui::DragAndDrop::payload::<DeviceDrag>(ui.ctx());
    let reorder = egui::DragAndDrop::payload::<MixerEffectDrag>(ui.ctx());
    if browser.is_none() && reorder.is_none() {
        return;
    }
    let viewport = area.intersect(ui.clip_rect());
    if !ui.rect_contains_pointer(viewport) {
        return;
    }
    let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    let (before, marker_y) = slots
        .iter()
        .find(|(_, rect)| pointer.y < rect.center().y)
        .map_or((None, end.top() - 1.5), |(id, rect)| {
            (Some(*id), rect.top() - 1.5)
        });
    let action = if let Some(payload) = browser {
        match payload.as_ref() {
            DeviceDrag::Browser(BrowserDevice::Plugin(plugin))
                if plugin.kind == PluginKind::Effect =>
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
                Some(Action::LoadMixerVst3 {
                    track,
                    plugin: plugin.clone(),
                    before,
                })
            }
            _ => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
                return;
            }
        }
    } else if let Some(payload) = reorder {
        if payload.track != track
            || !project.tracks()[track]
                .mixer_effects()
                .iter()
                .any(|effect| effect.id == payload.id)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
            return;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        move_effect_to_gap(project, track, payload.id, before)
    } else {
        return;
    };
    let y = marker_y.clamp(viewport.top() + 1.0, viewport.bottom() - 1.0);
    ui.painter()
        .hline(viewport.x_range(), y, Stroke::new(2.0, theme::ACCENT));
    if ui.input(|input| input.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ui.ctx());
        if let Some(action) = action {
            actions.push(action);
        }
    } else {
        let edge = 15.0;
        let speed = if pointer.y < viewport.top() + edge {
            1.0 - (pointer.y - viewport.top()) / edge
        } else if pointer.y > viewport.bottom() - edge {
            -1.0 + (viewport.bottom() - pointer.y) / edge
        } else {
            0.0
        };
        if speed != 0.0 {
            let delta = speed * 280.0 * ui.input(|input| input.stable_dt.min(0.05));
            ui.scroll_with_delta_animation(vec2(0.0, delta), egui::style::ScrollAnimation::none());
            ui.ctx().request_repaint();
        }
    }
}

fn push_gain(actions: &mut Vec<Action>, index: usize, mixer: MixerSettings, gain: f32) {
    if let Ok(next) = MixerSettings::new(
        gain.min(format::MAX_FADER_GAIN),
        mixer.pan(),
        mixer.output(),
    ) {
        actions.push(Action::SetMixer {
            track: index,
            mixer: next.with_muted(mixer.muted()).with_soloed(mixer.soloed()),
        });
    }
}
