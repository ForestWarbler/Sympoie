use eframe::egui::{
    self, Align, Align2, Grid, Id, Layout, Rect, Response, RichText, Sense, Ui, pos2, vec2,
};
use sympoie::builtin::BuiltinKind;
use sympoie::instrument::{Instrument, SubtractiveSettings, Waveform};
use sympoie::plugin::{PluginInstance, PluginKind, SidechainRoute, SidechainTap};
use sympoie::project::Project;
use sympoie::track::Track;
use sympoie::transport::TransportCommand;

use crate::actions::Action;
use crate::document::rows;
use crate::plugins::{PluginBusInfo, PluginRuntime};
use crate::state::Selection;
use crate::ui::rack;
use crate::ui::theme;
use crate::ui::widgets;
use crate::views::builtin_devices;
use crate::views::device_drag::DeviceDrag;
use crate::views::track_controls::mixer_controls;

pub const MODULE_WIDTH: f32 = 460.0;
pub const RACK_WIDTH: f32 = MODULE_WIDTH + 2.0 * CHAIN_PADDING + 10.0;
const INSTRUMENT_WIDTH: f32 = MODULE_WIDTH;
const DEVICE_WIDTH: f32 = MODULE_WIDTH;
const DROP_HEIGHT: f32 = 54.0;
const TITLE_HEIGHT: f32 = 21.0;
const RACK_HEADER: f32 = 26.0;
const OUTPUT_HEIGHT: f32 = 62.0;

pub const PANEL_HEIGHT: f32 = 214.0;
const CHAIN_PADDING: f32 = 6.0;
const CARD_BORDER_WIDTH: f32 = 1.0;

#[cfg(test)]
#[path = "device_drag_tests.rs"]
mod drag_tests;

#[cfg(test)]
#[path = "subtractive_tests.rs"]
mod subtractive_tests;

pub fn show(
    ui: &mut Ui,
    project: &Project,
    selection: &Selection,
    runtime: &PluginRuntime,
    actions: &mut Vec<Action>,
) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, theme::BG_DEEP);
    for x in [full.left() + 2.5, full.right() - 2.5] {
        rack::rule_vertical(ui.painter(), x, full.top()..=full.bottom());
    }
    let index = selection
        .tracks
        .first()
        .copied()
        .filter(|&index| selection.tracks.len() == 1 && index < project.tracks().len());
    let header = Rect::from_min_size(full.min, vec2(full.width(), RACK_HEADER));
    rack_header(ui, header, project, index, actions);
    let Some(index) = index else {
        ui.painter().text(
            full.with_min_y(header.bottom()).center(),
            Align2::CENTER_CENTER,
            "Select one track to see its rack",
            theme::prop(11.5),
            theme::TEXT_DIM,
        );
        ui.advance_cursor_after_rect(full);
        return;
    };
    let track = &project.tracks()[index];
    if matches!(track, Track::Bus(_)) {
        let body = full.with_min_y(header.bottom());
        widgets::inside(ui, body, Layout::top_down(Align::Center), |ui| {
            ui.add_space(24.0);
            ui.label("Bus effects are managed in the Mixer.");
            if ui.button("Open Mixer").clicked() {
                actions.push(Action::ShowMixer);
            }
        });
        ui.advance_cursor_after_rect(full);
        return;
    }
    let output = Rect::from_min_max(pos2(full.left(), full.bottom() - OUTPUT_HEIGHT), full.max);
    output_section(ui, output, project, index, actions);
    let body = Rect::from_min_max(
        pos2(full.left(), header.bottom()),
        pos2(full.right(), output.top()),
    );
    ui.interact(body, Id::new(("device-chain", index)), Sense::hover());
    let chain_rect = body.shrink2(vec2(CHAIN_PADDING, 4.0));
    let mut chain_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(chain_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    chain_ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
    egui::ScrollArea::vertical()
        .id_salt(("device-chain-scroll", index))
        .auto_shrink([false, false])
        .show(&mut chain_ui, |ui| {
            let mut instrument = None;
            let mut effects = Vec::new();
            ui.spacing_mut().item_spacing.y = 6.0;
            if let Track::Midi(midi) = track {
                instrument = Some(if let Some(plugin) = midi.plugin_instrument() {
                    external_card(ui, project, runtime, index, plugin, None, 0.0, actions).rect
                } else {
                    instrument_card(
                        ui,
                        index,
                        midi.instrument(),
                        midi.instrument_bypassed(),
                        0.0,
                        actions,
                    )
                    .rect
                });
                if !track.effects().is_empty() {
                    signal_arrow(ui);
                }
            }
            for (position, plugin) in track.effects().iter().enumerate() {
                let response = external_card(
                    ui,
                    project,
                    runtime,
                    index,
                    plugin,
                    Some((position, track.effects().len())),
                    0.0,
                    actions,
                );
                effects.push((plugin.id, response.rect));
            }
            let end = drop_zone(ui, index, track).rect;
            chain_drop_target(ui, project, index, instrument, &effects, end, actions);
        });
    ui.advance_cursor_after_rect(full);
}

fn rack_header(
    ui: &mut Ui,
    rect: Rect,
    project: &Project,
    index: Option<usize>,
    actions: &mut Vec<Action>,
) {
    let close_rect =
        Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(20.0, 20.0));
    let painter = ui.painter();
    widgets::color_bar(
        painter,
        rect,
        index.map_or(theme::BG_RAISED, |index| theme::track_color(project, index)),
        "",
    );
    let label = rack::print(
        painter,
        pos2(close_rect.left() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        "Rack",
        rack::INK_FAINT,
    );
    if let Some(index) = index {
        widgets::fitted_text(
            painter,
            Rect::from_min_max(
                pos2(rect.left() + 10.0, rect.top()),
                pos2(label.left() - 8.0, rect.bottom()),
            ),
            &rows::track_label(project, index),
            theme::prop(10.5),
            theme::TEXT,
            Align2::LEFT_CENTER,
        );
    }
    let close = rack::glyph_button(
        ui,
        close_rect,
        Id::new("rack-close"),
        rack::Glyph::Close,
        true,
    );
    close.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Hide Rack"));
    if close.on_hover_text("Hide Rack").clicked() {
        actions.push(Action::ToggleRack);
    }
}

fn signal_arrow(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(MODULE_WIDTH, 6.0), Sense::hover());
    let c = rect.center();
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            c + vec2(-5.0, -3.0),
            c + vec2(5.0, -3.0),
            c + vec2(0.0, 3.0),
        ],
        rack::INK_FAINT,
        egui::Stroke::NONE,
    ));
}

fn output_section(
    ui: &mut Ui,
    rect: Rect,
    project: &Project,
    index: usize,
    actions: &mut Vec<Action>,
) {
    let plate = rect.shrink2(vec2(CHAIN_PADDING, 4.0));
    ui.painter().add(rack::faceplate(plate, None, false));
    let track = &project.tracks()[index];
    let mixer = *track.mixer();
    let inner = plate.shrink2(vec2(8.0, 5.0));
    rack::print(
        ui.painter(),
        inner.left_top(),
        Align2::LEFT_TOP,
        "Output",
        rack::INK_FAINT,
    );
    let row = Rect::from_min_max(pos2(inner.left(), inner.top() + 14.0), inner.max);
    widgets::inside(ui, row, Layout::left_to_right(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let mute = widgets::toggle(ui, "M", mixer.muted(), theme::WARN, vec2(23.0, 20.0))
            .on_hover_text("Mute track");
        ui.interact(mute.rect, Id::new(("rack-mute", index)), Sense::hover());
        if mute.clicked() {
            actions.push(Action::SetMixer {
                track: index,
                mixer: mixer.with_muted(!mixer.muted()),
            });
        }
        let solo = widgets::toggle(ui, "S", mixer.soloed(), theme::ACCENT, vec2(23.0, 20.0))
            .on_hover_text("Solo track");
        ui.interact(solo.rect, Id::new(("rack-solo", index)), Sense::hover());
        if solo.clicked() {
            actions.push(Action::SetSolo {
                track: index,
                soloed: !mixer.soloed(),
            });
        }
        ui.add_space(6.0);
        ui.push_id("rack-output", |ui| {
            mixer_controls(ui, project, index, actions);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut Ui,
    track: usize,
    kind: &'static str,
    width: f32,
    height: f32,
    accent: Option<egui::Color32>,
    dimmed: bool,
    contents: impl FnOnce(&mut Ui),
) -> Response {
    let plate = ui.painter().add(egui::Shape::Noop);
    let response = egui::Frame::new()
        .stroke(egui::Stroke::new(
            CARD_BORDER_WIDTH,
            egui::Color32::TRANSPARENT,
        ))
        .inner_margin(0)
        .show(ui, |ui| {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt((kind, track))
                    .layout(Layout::top_down(Align::Min)),
                |ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    ui.set_width(width);
                    ui.set_min_height(height);
                    contents(ui);
                },
            );
        })
        .response;
    ui.painter()
        .set(plate, rack::faceplate(response.rect, accent, dimmed));
    ui.interact(response.rect, Id::new((kind, track)), Sense::hover())
}

fn title_bar(
    ui: &mut Ui,
    name: &str,
    note: &str,
    enabled: Option<bool>,
) -> (Option<Response>, Rect) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, TITLE_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect.shrink2(vec2(1.0, 0.0)).with_min_y(rect.top() + 3.0),
        0.0,
        egui::Color32::from_black_alpha(34),
    );
    painter.hline(
        (rect.left() + 1.0)..=(rect.right() - 1.0),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(120)),
    );
    painter.hline(
        (rect.left() + 1.0)..=(rect.right() - 1.0),
        rect.bottom() + 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(12)),
    );
    let mut activator = None;
    let mut name_left = rect.left() + 8.0;
    let middle = rect.center().y + 1.0;
    if let Some(on) = enabled {
        let area = Rect::from_center_size(pos2(rect.left() + 12.0, middle), vec2(14.0, 14.0));
        activator = Some(widgets::inside(
            ui,
            area,
            Layout::left_to_right(Align::Center),
            |ui| widgets::activator(ui, on),
        ));
        name_left = area.right() + 5.0;
    }
    let painter = ui.painter();
    let note_width = if note.is_empty() {
        0.0
    } else {
        painter
            .layout_no_wrap(note.to_owned(), theme::prop(9.0), rack::INK_FAINT)
            .size()
            .x
            + 8.0
    };
    let ink = if enabled == Some(false) {
        rack::INK_FAINT
    } else {
        rack::INK_BRIGHT
    };
    let available = rect.right() - 6.0 - note_width - name_left;
    let galley = rack::spaced(painter, name, theme::prop(9.5), ink, 1.0);
    if galley.size().x <= available {
        painter.galley(pos2(name_left, middle - galley.size().y / 2.0), galley, ink);
    } else {
        widgets::fitted_text(
            painter,
            Rect::from_min_max(
                pos2(name_left, rect.top() + 2.0),
                pos2(name_left + available.max(0.0), rect.bottom()),
            ),
            name,
            theme::prop(10.0),
            ink,
            Align2::LEFT_CENTER,
        );
    }
    if note_width > 0.0 {
        painter.text(
            pos2(rect.right() - 6.0, middle),
            Align2::RIGHT_CENTER,
            note,
            theme::prop(9.0),
            rack::INK_FAINT,
        );
    }
    (activator, rect)
}

fn body(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 5.0);
            contents(ui);
        });
}

fn drop_zone(ui: &mut Ui, index: usize, track: &Track) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(MODULE_WIDTH, DROP_HEIGHT), Sense::hover());
    let response = ui.interact(
        rect,
        Id::new(("device-empty-effects", index)),
        Sense::hover(),
    );
    let painter = ui.painter();
    let slot = rect.shrink(1.0);
    painter.rect_filled(slot, 3.0, egui::Color32::from_black_alpha(40));
    let dash = egui::Stroke::new(
        1.0,
        if response.hovered() {
            rack::INK
        } else {
            rack::INK_FAINT
        },
    );
    let corners = [
        slot.left_top(),
        slot.right_top(),
        slot.right_bottom(),
        slot.left_bottom(),
        slot.left_top(),
    ];
    for pair in corners.windows(2) {
        painter.extend(egui::Shape::dashed_line(
            &[pair[0], pair[1]],
            dash,
            4.0,
            3.0,
        ));
    }
    let center = slot.center();
    let plus = center - vec2(70.0, 0.0);
    painter.line_segment([plus - vec2(6.0, 0.0), plus + vec2(6.0, 0.0)], dash);
    painter.line_segment([plus - vec2(0.0, 6.0), plus + vec2(0.0, 6.0)], dash);
    rack::print(
        painter,
        plus + vec2(14.0, -1.0),
        Align2::LEFT_BOTTOM,
        if track.effects().is_empty() {
            "Drop an effect into the rack"
        } else {
            "Add another effect"
        },
        rack::INK,
    );
    rack::print(
        painter,
        plus + vec2(14.0, 2.0),
        Align2::LEFT_TOP,
        "Signal flows top to bottom",
        rack::INK_FAINT,
    );
    response
}

fn move_effect_to_gap(
    project: &Project,
    track: usize,
    id: u64,
    before: Option<u64>,
) -> Option<Action> {
    let effects = project.tracks().get(track)?.effects();
    let from = effects.iter().position(|plugin| plugin.id == id)?;
    let gap = match before {
        Some(id) => effects.iter().position(|plugin| plugin.id == id)?,
        None => effects.len(),
    };
    let to = gap - usize::from(from < gap);
    (from != to).then_some(Action::MoveEffect { track, from, to })
}

fn insertion_gap(effects: &[(u64, Rect)], end: Rect, pointer_y: f32) -> (Option<u64>, f32) {
    effects
        .iter()
        .find(|(_, rect)| pointer_y < rect.center().y)
        .map_or((None, end.top() - 3.0), |(id, rect)| {
            (Some(*id), rect.top() - 3.0)
        })
}

fn chain_drop_target(
    ui: &mut Ui,
    project: &Project,
    track: usize,
    instrument: Option<Rect>,
    effects: &[(u64, Rect)],
    end: Rect,
    actions: &mut Vec<Action>,
) {
    let Some(payload) = egui::DragAndDrop::payload::<DeviceDrag>(ui.ctx()) else {
        return;
    };
    let viewport = ui.clip_rect().intersect(Rect::from_x_y_ranges(
        end.x_range(),
        ui.clip_rect().y_range(),
    ));
    if !ui.rect_contains_pointer(viewport) {
        return;
    }
    let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    let (before, marker_y) = insertion_gap(effects, end, pointer.y);
    let (action, replaces_instrument) = match payload.as_ref() {
        DeviceDrag::Browser(device) if device.can_load_on(project, track) => {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
            (
                Some(device.load_action(track, before)),
                device.is_instrument(),
            )
        }
        DeviceDrag::Effect { track: source, id }
            if *source == track
                && project.tracks()[track]
                    .effects()
                    .iter()
                    .any(|plugin| plugin.id == *id) =>
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            (move_effect_to_gap(project, track, *id, before), false)
        }
        _ => {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
            return;
        }
    };
    if replaces_instrument {
        let target = instrument
            .filter(|rect| rect.intersects(viewport))
            .unwrap_or(viewport);
        ui.painter().rect_stroke(
            target,
            2.0,
            egui::Stroke::new(2.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    } else {
        let y = marker_y.clamp(viewport.top() + 1.0, viewport.bottom() - 1.0);
        ui.painter().line_segment(
            [pos2(end.left(), y), pos2(end.right(), y)],
            egui::Stroke::new(3.0, theme::ACCENT),
        );
    }
    if ui.input(|input| input.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ui.ctx());
        if let Some(action) = action {
            actions.push(action);
        }
    } else {
        let edge = 28.0;
        let speed = if pointer.y < viewport.top() + edge {
            1.0 - (pointer.y - viewport.top()) / edge
        } else if pointer.y > viewport.bottom() - edge {
            -1.0 + (viewport.bottom() - pointer.y) / edge
        } else {
            0.0
        };
        if speed != 0.0 {
            let delta = speed * 560.0 * ui.input(|input| input.stable_dt.min(0.05));
            ui.scroll_with_delta_animation(vec2(0.0, delta), egui::style::ScrollAnimation::none());
            ui.ctx().request_repaint();
        }
    }
}

#[derive(Clone, Copy)]
enum Taper {
    Linear,
    Log,
    Cubic,
}

const KNOBS: [(&str, &str, f32, f32, Taper); 7] = [
    ("cutoff", "Filter", 20.0, 20_000.0, Taper::Log),
    ("resonance", "Res", 0.5, 10.0, Taper::Linear),
    ("gain", "Volume", 0.0, 1.0, Taper::Linear),
    ("attack", "Attack", 0.0, 10.0, Taper::Cubic),
    ("decay", "Decay", 0.0, 10.0, Taper::Cubic),
    ("sustain", "Sustain", 0.0, 1.0, Taper::Linear),
    ("release", "Release", 0.0, 10.0, Taper::Cubic),
];

fn to_knob(value: f32, min: f32, max: f32, taper: Taper) -> f32 {
    let norm = match taper {
        Taper::Linear => (value - min) / (max - min),
        Taper::Log => (value.max(min) / min).ln() / (max / min).ln(),
        Taper::Cubic => (value / max).max(0.0).cbrt(),
    };
    norm.clamp(0.0, 1.0)
}

fn from_knob(norm: f32, min: f32, max: f32, taper: Taper) -> f32 {
    let value = match taper {
        Taper::Linear => min + norm * (max - min),
        Taper::Log => min * (max / min).powf(norm),
        Taper::Cubic => max * norm * norm * norm,
    };
    value.clamp(min, max)
}

fn knob_text(key: &str, value: f32) -> String {
    match key {
        "cutoff" if value >= 1000.0 => format!("{:.2} kHz", value / 1000.0),
        "cutoff" => format!("{value:.0} Hz"),
        "resonance" => format!("{value:.2}"),
        "sustain" | "gain" => format!("{:.0} %", value * 100.0),
        _ if value < 1.0 => format!("{:.0} ms", value * 1000.0),
        _ => format!("{value:.2} s"),
    }
}

fn knob_field<'a>(settings: &'a mut SubtractiveSettings, key: &str) -> &'a mut f32 {
    match key {
        "cutoff" => &mut settings.cutoff_hz,
        "resonance" => &mut settings.resonance,
        "attack" => &mut settings.attack_seconds,
        "decay" => &mut settings.decay_seconds,
        "sustain" => &mut settings.sustain,
        "release" => &mut settings.release_seconds,
        _ => &mut settings.gain,
    }
}

fn instrument_card(
    ui: &mut Ui,
    track: usize,
    instrument: Option<&Instrument>,
    bypassed: bool,
    height: f32,
    actions: &mut Vec<Action>,
) -> Response {
    card(
        ui,
        track,
        "device-instrument",
        INSTRUMENT_WIDTH,
        height,
        None,
        bypassed && instrument.is_some(),
        |ui| {
            if let Some(Instrument::Subtractive(current)) = instrument {
                let (activator, _) = title_bar(ui, "Legacy instrument", "", Some(!bypassed));
                if let Some(response) = activator {
                    instrument_activator(
                        ui,
                        response,
                        track,
                        bypassed,
                        "device-instrument-activator",
                        actions,
                    );
                }
                body(ui, |ui| subtractive_panel(ui, track, current, actions));
            } else {
                title_bar(ui, "Instrument", "", None);
                body(ui, |ui| {
                    ui.label(RichText::new("No instrument").color(theme::TEXT));
                    ui.label(
                        RichText::new("Add an instrument to hear MIDI notes.")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(4.0);
                    if button(ui, track, "add-instrument", "Add Oscil").clicked() {
                        actions.push(Action::LoadVst3 {
                            track,
                            plugin: BuiltinKind::Oscil.descriptor(),
                        });
                    }
                    if button(ui, track, "add-sampler", "Add Sampler").clicked() {
                        actions.push(Action::LoadVst3 {
                            track,
                            plugin: BuiltinKind::Sampler.descriptor(),
                        });
                    }
                    if button(ui, track, "browse-instruments", "Choose Instrument…").clicked() {
                        actions.push(Action::BrowsePlugins {
                            track,
                            kind: PluginKind::Instrument,
                        });
                    }
                });
            }
        },
    )
}

fn instrument_activator(
    ui: &mut Ui,
    response: Response,
    track: usize,
    bypassed: bool,
    name: &'static str,
    actions: &mut Vec<Action>,
) {
    ui.interact(response.rect, Id::new((name, track)), Sense::hover());
    if response
        .on_hover_text(if bypassed {
            "Turn legacy instrument on"
        } else {
            "Turn legacy instrument off"
        })
        .clicked()
    {
        actions.push(Action::SetInstrumentBypassed {
            track,
            bypassed: !bypassed,
        });
    }
}

fn subtractive_panel(
    ui: &mut Ui,
    track: usize,
    current: &SubtractiveSettings,
    actions: &mut Vec<Action>,
) {
    let mut settings = *current;
    let mut edited = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Osc").color(theme::TEXT_DIM));
        egui::ComboBox::from_id_salt(("device-waveform", track))
            .width(70.0)
            .selected_text(waveform_name(settings.waveform))
            .show_ui(ui, |ui| {
                for waveform in [Waveform::Sine, Waveform::Saw, Waveform::Square] {
                    if ui
                        .selectable_value(&mut settings.waveform, waveform, waveform_name(waveform))
                        .changed()
                    {
                        edited = Some("waveform");
                    }
                }
            });
    });
    let defaults = SubtractiveSettings::default();
    for row in [&KNOBS[..3], &KNOBS[3..]] {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for &(key, label, min, max, taper) in row {
                let value = *knob_field(&mut settings, key);
                let mut norm = to_knob(value, min, max, taper);
                let response = widgets::knob(ui, &mut norm, label, &knob_text(key, value));
                if response.double_clicked() {
                    let mut reset = defaults;
                    *knob_field(&mut settings, key) = *knob_field(&mut reset, key);
                    edited = Some(key);
                } else if response.changed() {
                    *knob_field(&mut settings, key) = from_knob(norm, min, max, taper);
                    edited = Some(key);
                }
                response.on_hover_text(format!(
                    "{label}: {}. Drag up or down; Shift for fine control; double-click to reset.",
                    knob_text(key, *knob_field(&mut settings, key))
                ));
            }
        });
    }
    if let Some(parameter) = edited {
        actions.push(Action::SetInstrumentParameter {
            track,
            instrument: Instrument::Subtractive(settings),
            parameter,
        });
    }
    ui.horizontal_wrapped(|ui| {
        if button(ui, track, "open-plugin-editor", "Open Editor").clicked() {
            actions.push(Action::OpenPluginEditor(track));
        }
        if button(ui, track, "browse-instruments", "Replace…")
            .on_hover_text("Replace this instrument")
            .clicked()
        {
            actions.push(Action::BrowsePlugins {
                track,
                kind: PluginKind::Instrument,
            });
        }
        if button(ui, track, "remove-plugin", "Remove").clicked() {
            actions.push(Action::SetInstrument {
                track,
                instrument: None,
            });
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn external_card(
    ui: &mut Ui,
    project: &Project,
    runtime: &PluginRuntime,
    track: usize,
    plugin: &PluginInstance,
    effect: Option<(usize, usize)>,
    height: f32,
    actions: &mut Vec<Action>,
) -> Response {
    if let Some(kind) = BuiltinKind::from_descriptor(&plugin.descriptor) {
        return builtin_card(
            ui, project, runtime, track, plugin, kind, effect, height, actions,
        );
    }
    ui.push_id(("vst3-card", plugin.id), |ui| {
        card(
            ui,
            plugin.id as usize,
            "device-vst3",
            DEVICE_WIDTH,
            height,
            None,
            plugin.bypassed,
            |ui| {
                let builtin = BuiltinKind::from_descriptor(&plugin.descriptor);
                let kind = if builtin.is_some() {
                    "Built-in"
                } else if effect.is_some() {
                    "VST3"
                } else {
                    "VST3 Instrument"
                };
                let note = runtime
                    .latency_samples(plugin.id)
                    .filter(|latency| *latency > 0)
                    .map_or_else(
                        || kind.to_owned(),
                        |latency| {
                            format!(
                                "{:.1} ms",
                                f64::from(latency) * 1000.0 / f64::from(sympoie::SAMPLE_RATE)
                            )
                        },
                    );
                let (activator, title) =
                    title_bar(ui, &plugin.descriptor.name, &note, Some(!plugin.bypassed));
                if activator.is_some_and(|response| response.clicked()) {
                    actions.push(Action::SetVst3Bypassed {
                        id: plugin.id,
                        bypassed: !plugin.bypassed,
                    });
                }
                if effect.is_some() {
                    let handle = ui.interact(
                        Rect::from_min_max(title.min + vec2(23.0, 0.0), title.max),
                        Id::new(("device-effect-drag", plugin.id)),
                        Sense::drag(),
                    );
                    handle.dnd_set_drag_payload(DeviceDrag::Effect {
                        track,
                        id: plugin.id,
                    });
                    handle.on_hover_text("Drag to reorder this effect");
                }
                body(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&plugin.descriptor.vendor)
                                .small()
                                .color(theme::TEXT_DIM),
                        )
                        .truncate(),
                    );
                    if plugin.bypassed {
                        ui.label(RichText::new("Bypassed").small().color(theme::WARN));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Open Editor").clicked() {
                            actions.push(Action::OpenNativeVst3Editor(plugin.id));
                        }
                        if builtin.is_none() && ui.button("Parameters").clicked() {
                            actions.push(Action::OpenVst3Editor(plugin.id));
                        }
                    });
                    if effect.is_some() && builtin != Some(BuiltinKind::Eq) {
                        let label = if plugin.sidechain.is_some() {
                            "Sidechain •"
                        } else {
                            "Sidechain"
                        };
                        let button = ui.button(label);
                        if button.clicked() {
                            actions.push(Action::OpenVst3Sidechain(plugin.id));
                        }
                        ui.interact(
                            button.rect,
                            Id::new(("vst3-sidechain", plugin.id)),
                            Sense::hover(),
                        );
                    }
                    ui.horizontal(|ui| {
                        if let Some((position, count)) = effect {
                            if ui
                                .add_enabled(position > 0, egui::Button::new("↑"))
                                .on_hover_text("Move up")
                                .clicked()
                            {
                                actions.push(Action::MoveEffect {
                                    track,
                                    from: position,
                                    to: position - 1,
                                });
                            }
                            if ui
                                .add_enabled(position + 1 < count, egui::Button::new("↓"))
                                .on_hover_text("Move down")
                                .clicked()
                            {
                                actions.push(Action::MoveEffect {
                                    track,
                                    from: position,
                                    to: position + 1,
                                });
                            }
                        }
                        if ui.button("Remove").clicked() {
                            actions.push(Action::RemoveVst3 {
                                track,
                                id: plugin.id,
                            });
                        }
                    });
                });
            },
        )
    })
    .inner
}

#[allow(clippy::too_many_arguments)]
fn builtin_card(
    ui: &mut Ui,
    project: &Project,
    runtime: &PluginRuntime,
    track: usize,
    plugin: &PluginInstance,
    kind: BuiltinKind,
    effect: Option<(usize, usize)>,
    height: f32,
    actions: &mut Vec<Action>,
) -> Response {
    let telemetry = runtime.builtin_telemetry(plugin.id);
    let response = ui
        .push_id(("builtin-card", plugin.id), |ui| {
            card(
                ui,
                plugin.id as usize,
                "device-vst3",
                MODULE_WIDTH,
                height,
                Some(builtin_devices::accent(kind)),
                plugin.bypassed,
                |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let (activator, title) =
                        title_bar(ui, &plugin.descriptor.name, "", Some(!plugin.bypassed));
                    if activator.is_some_and(|response| response.clicked()) {
                        actions.push(Action::SetVst3Bypassed {
                            id: plugin.id,
                            bypassed: !plugin.bypassed,
                        });
                    }
                    let glyph = |offset: f32| {
                        Rect::from_center_size(
                            pos2(title.right() - offset, title.center().y + 1.0),
                            vec2(16.0, 16.0),
                        )
                    };
                    let mut controls_left = glyph(13.0).left();
                    let remove = rack::glyph_button(
                        ui,
                        glyph(13.0),
                        Id::new(("builtin-remove", plugin.id)),
                        rack::Glyph::Close,
                        true,
                    );
                    if remove.on_hover_text("Remove device").clicked() {
                        actions.push(Action::RemoveVst3 {
                            track,
                            id: plugin.id,
                        });
                    }
                    if let Some((position, count)) = effect {
                        for (offset, glyph_kind, allowed, to, hint) in [
                            (
                                49.0,
                                rack::Glyph::Up,
                                position > 0,
                                position.saturating_sub(1),
                                "Move up",
                            ),
                            (
                                33.0,
                                rack::Glyph::Down,
                                position + 1 < count,
                                position + 1,
                                "Move down",
                            ),
                        ] {
                            let response = rack::glyph_button(
                                ui,
                                glyph(offset),
                                Id::new(("builtin-move", plugin.id, offset as u32)),
                                glyph_kind,
                                allowed,
                            );
                            controls_left = controls_left.min(glyph(offset).left());
                            if response.on_hover_text(hint).clicked() {
                                actions.push(Action::MoveEffect {
                                    track,
                                    from: position,
                                    to,
                                });
                            }
                        }
                    }
                    if kind == BuiltinKind::Comp {
                        let chip = Rect::from_min_max(
                            pos2(controls_left - 36.0, title.top() + 3.0),
                            pos2(controls_left - 4.0, title.bottom() - 2.0),
                        );
                        controls_left = chip.left();
                        widgets::inside(ui, chip, Layout::left_to_right(Align::Center), |ui| {
                            inline_sidechain_button(ui, project, runtime, track, plugin, actions)
                        });
                    }
                    let handle = ui.interact(
                        Rect::from_min_max(
                            title.min + vec2(23.0, 0.0),
                            pos2(controls_left - 2.0, title.bottom()),
                        ),
                        Id::new(("device-effect-drag", plugin.id)),
                        if effect.is_some() {
                            Sense::click_and_drag()
                        } else {
                            Sense::click()
                        },
                    );
                    if effect.is_some() {
                        handle.dnd_set_drag_payload(DeviceDrag::Effect {
                            track,
                            id: plugin.id,
                        });
                        if handle.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                        }
                    }
                    handle.context_menu(|ui| {
                        if ui.button("Open in window").clicked() {
                            actions.push(Action::OpenNativeVst3Editor(plugin.id));
                            ui.close();
                        }
                        if ui.button("Reset controls").clicked() {
                            actions.push(Action::ResetBuiltinParameters { id: plugin.id });
                            ui.close();
                        }
                    });
                    let side = ((MODULE_WIDTH - builtin_devices::card_width(kind)) / 2.0 + 8.0)
                        .max(8.0) as i8;
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: side,
                            right: side,
                            top: 1,
                            bottom: 6,
                        })
                        .show(ui, |ui| {
                            builtin_devices::compact(
                                ui,
                                plugin,
                                kind,
                                telemetry.as_deref(),
                                actions,
                            )
                        });
                },
            )
        })
        .inner;
    let focus_key = Id::new("builtin-chain-focus");
    if ui.ctx().data_mut(|data| data.get_temp::<u64>(focus_key)) == Some(plugin.id) {
        ui.ctx().data_mut(|data| data.remove_temp::<u64>(focus_key));
        response.scroll_to_me(Some(Align::Center));
    }
    response
}

pub fn inline_sidechain_button(
    ui: &mut Ui,
    project: &Project,
    runtime: &PluginRuntime,
    track: usize,
    plugin: &PluginInstance,
    actions: &mut Vec<Action>,
) {
    let buses = runtime.sidechain_inputs(plugin.id);
    let response = rack::led_switch(
        ui,
        "SC",
        plugin.sidechain.is_some(),
        builtin_devices::accent(BuiltinKind::Comp),
    );
    ui.interact(
        response.rect,
        Id::new(("builtin-sidechain-menu", plugin.id)),
        Sense::hover(),
    );
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| sidechain_controls(ui, project, track, plugin, &buses, actions));
    response.on_hover_text(if plugin.sidechain.is_some() {
        "Sidechain source routed. Click to change it."
    } else {
        "Route another track into the detector"
    });
}

pub fn show_sidechain_editor(
    ctx: &egui::Context,
    project: &Project,
    runtime: &PluginRuntime,
    id: u64,
    actions: &mut Vec<Action>,
) {
    let Some((destination, plugin)) =
        project
            .tracks()
            .iter()
            .enumerate()
            .find_map(|(index, track)| {
                track
                    .all_effects()
                    .find(|plugin| plugin.id == id)
                    .map(|plugin| (index, plugin))
            })
    else {
        return;
    };
    let buses = runtime.sidechain_inputs(id);
    let mut open = true;
    egui::Window::new(format!("{} · Sidechain", plugin.descriptor.name))
        .id(Id::new(("sidechain-window", id)))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(310.0)
        .show(ctx, |ui| {
            sidechain_controls(ui, project, destination, plugin, &buses, actions)
        });
    if !open {
        actions.push(Action::CloseVst3Sidechain(id));
    }
}

fn sidechain_controls(
    ui: &mut Ui,
    project: &Project,
    destination: usize,
    plugin: &PluginInstance,
    buses: &[PluginBusInfo],
    actions: &mut Vec<Action>,
) {
    ui.set_min_width(260.0);
    ui.label(RichText::new("External sidechain").strong());
    let labels = rows::track_labels(project);
    let mut route = plugin.sidechain;
    let source_name = route
        .as_ref()
        .filter(|route| route.source_track < project.tracks().len())
        .map_or("None", |route| labels[route.source_track].as_str());
    let source = egui::ComboBox::from_id_salt(("sidechain-source", plugin.id))
        .selected_text(source_name)
        .show_ui(ui, |ui| {
            let none = ui.selectable_label(route.is_none(), "None");
            ui.interact(
                none.rect,
                Id::new(("sidechain-source-none", plugin.id)),
                Sense::hover(),
            );
            if none.clicked() {
                route = None;
            }
            ui.add_enabled_ui(!buses.is_empty() || route.is_some(), |ui| {
                for (index, label) in labels.iter().enumerate() {
                    if index == destination {
                        continue;
                    }
                    let selected = route
                        .as_ref()
                        .is_some_and(|route| route.source_track == index);
                    let response = ui.selectable_label(selected, label);
                    ui.interact(
                        response.rect,
                        Id::new(("sidechain-source-option", plugin.id, index)),
                        Sense::hover(),
                    );
                    if response.clicked() {
                        if let Some(route) = &mut route {
                            route.source_track = index;
                        } else if let Some(bus) = buses.first() {
                            route = Some(SidechainRoute {
                                source_track: index,
                                bus_index: bus.index,
                                tap: SidechainTap::PostEffects,
                                gain: 1.0,
                            });
                        }
                    }
                }
            });
        });
    ui.interact(
        source.response.rect,
        Id::new(("sidechain-source", plugin.id)),
        Sense::hover(),
    );
    if let Some(route) = &mut route {
        let selected_bus = buses
            .iter()
            .find(|bus| bus.index == route.bus_index)
            .map_or_else(
                || format!("Input {} (unavailable)", route.bus_index),
                |bus| bus.name.clone(),
            );
        ui.add_enabled_ui(!buses.is_empty(), |ui| {
            egui::ComboBox::from_id_salt(("sidechain-bus", plugin.id))
                .selected_text(selected_bus)
                .show_ui(ui, |ui| {
                    for bus in buses {
                        let layout = if bus.channels == 1 { "mono" } else { "stereo" };
                        ui.selectable_value(
                            &mut route.bus_index,
                            bus.index,
                            format!("{} ({layout})", bus.name),
                        );
                    }
                });
        });
        egui::ComboBox::from_id_salt(("sidechain-tap", plugin.id))
            .selected_text(sidechain_tap_label(route.tap))
            .show_ui(ui, |ui| {
                for tap in [
                    SidechainTap::PreEffects,
                    SidechainTap::PostEffects,
                    SidechainTap::PostFader,
                ] {
                    ui.selectable_value(&mut route.tap, tap, sidechain_tap_label(tap));
                }
            });
        let mut gain = route.gain;
        let gain_response = ui.add(
            egui::Slider::new(&mut gain, 0.0..=4.0)
                .text("Send gain")
                .max_decimals(2),
        );
        ui.interact(
            gain_response.rect,
            Id::new(("sidechain-gain", plugin.id)),
            Sense::hover(),
        );
        if gain_response.changed() {
            actions.push(Action::SetVst3SidechainGain {
                id: plugin.id,
                gain,
            });
        }
        ui.label(
            RichText::new("Send a copy of the source to this effect.")
                .small()
                .color(theme::TEXT_DIM),
        );
    }
    if buses.is_empty() {
        ui.label(
            RichText::new("No sidechain input is currently available for this device.")
                .small()
                .color(theme::TEXT_DIM),
        );
    }
    if route != plugin.sidechain {
        actions.push(Action::SetVst3Sidechain {
            id: plugin.id,
            route,
        });
    }
}

fn sidechain_tap_label(tap: SidechainTap) -> &'static str {
    match tap {
        SidechainTap::PreEffects => "Before effects",
        SidechainTap::PostEffects => "After effects, before fader",
        SidechainTap::PostFader => "After fader",
    }
}

pub fn vst3_editor_viewport_id(id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("vst3-parameters", id))
}

#[cfg(test)]
pub fn show_vst3_editor(
    ctx: &egui::Context,
    project: &Project,
    id: u64,
    actions: &mut Vec<Action>,
) {
    show_parameter_editor(ctx, project, None, id, actions);
}

pub fn show_vst3_editor_with_runtime(
    ctx: &egui::Context,
    project: &Project,
    runtime: &PluginRuntime,
    id: u64,
    actions: &mut Vec<Action>,
) {
    show_parameter_editor(ctx, project, Some(runtime), id, actions);
}

fn show_parameter_editor(
    ctx: &egui::Context,
    project: &Project,
    runtime: Option<&PluginRuntime>,
    id: u64,
    actions: &mut Vec<Action>,
) {
    let Some(plugin) = project.plugin(id) else {
        return;
    };
    let builtin = BuiltinKind::from_descriptor(&plugin.descriptor);
    let telemetry = runtime.and_then(|runtime| runtime.builtin_telemetry(id));
    ctx.show_viewport_immediate(
        vst3_editor_viewport_id(id),
        egui::ViewportBuilder::default()
            .with_active(false)
            .with_title(if builtin.is_some() {
                plugin.descriptor.name.clone()
            } else {
                format!("{} · Parameters", plugin.descriptor.name)
            })
            .with_inner_size(builtin.map_or([540.0, 560.0], builtin_devices::editor_size))
            .with_min_inner_size(
                builtin.map_or([380.0, 280.0], builtin_devices::minimum_editor_size),
            ),
        |ui, class| {
            if class != egui::ViewportClass::EmbeddedWindow {
                editor_shortcuts(ui.ctx(), actions);
                if ui.input(|i| i.viewport().close_requested()) {
                    actions.push(Action::CloseVst3Editor(id));
                }
            }
            let mut contents = |ui: &mut Ui| {
                if let Some(kind) = builtin {
                    builtin_devices::show(ui, plugin, kind, telemetry.as_deref(), actions);
                    return;
                }
                ui.horizontal(|ui| {
                    if ui.button("Open Plugin UI").clicked() {
                        actions.push(Action::OpenNativeVst3Editor(id));
                    }
                    let close = ui.button("Close");
                    ui.interact(
                        close.rect,
                        Id::new(("close-vst3-editor", id)),
                        Sense::hover(),
                    );
                    if close.clicked() {
                        actions.push(Action::CloseVst3Editor(id));
                    }
                });
                ui.label(RichText::new(&plugin.descriptor.name).strong());
                let search_id = Id::new(("vst3-parameter-search", id));
                let mut search = ui
                    .ctx()
                    .data_mut(|data| data.get_temp::<String>(search_id).unwrap_or_default());
                ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .id(search_id)
                        .hint_text("Filter parameters"),
                );
                ui.ctx()
                    .data_mut(|data| data.insert_temp(search_id, search.clone()));
                let search = search.to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt(("vst3-parameters", id))
                    .show(ui, |ui| {
                        for parameter in &plugin.parameters {
                            if !parameter.name.to_lowercase().contains(&search) {
                                continue;
                            }
                            ui.push_id(parameter.id, |ui| {
                                let mut value = parameter.value;
                                ui.horizontal(|ui| {
                                    ui.label(&parameter.name);
                                    let mut slider =
                                        egui::Slider::new(&mut value, 0.0..=1.0).max_decimals(4);
                                    if parameter.steps > 0 {
                                        slider = slider.step_by(1.0 / f64::from(parameter.steps));
                                    }
                                    let response = ui.add_enabled(!parameter.read_only, slider);
                                    ui.interact(
                                        response.rect,
                                        Id::new(("vst3-parameter", id, parameter.id)),
                                        Sense::hover(),
                                    );
                                    if !parameter.read_only && response.changed() {
                                        actions.push(Action::SetVst3Parameter {
                                            id,
                                            parameter: parameter.id,
                                            value,
                                        });
                                    }
                                });
                            });
                        }
                    });
            };
            if class == egui::ViewportClass::EmbeddedWindow {
                contents(ui);
            } else {
                egui::CentralPanel::default().show(ui, contents);
            }
        },
    );
}

pub fn editor_viewport_id(track: usize) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("plugin-editor", track))
}

pub fn show_editor(
    ctx: &egui::Context,
    project: &Project,
    track: usize,
    actions: &mut Vec<Action>,
) {
    let Some(Track::Midi(midi)) = project.tracks().get(track) else {
        return;
    };
    let Some(Instrument::Subtractive(current)) = midi.instrument() else {
        return;
    };
    let title = format!("Legacy instrument · {}", rows::track_label(project, track));
    ctx.show_viewport_immediate(
        editor_viewport_id(track),
        egui::ViewportBuilder::default()
            .with_active(false)
            .with_title(title)
            .with_inner_size([460.0, 430.0])
            .with_min_inner_size([340.0, 280.0]),
        |ui, class| {
            if class == egui::ViewportClass::EmbeddedWindow {
                editor_contents(ui, track, current, midi.instrument_bypassed(), actions);
            } else {
                editor_shortcuts(ui.ctx(), actions);
                if ui.input(|input| input.viewport().close_requested()) {
                    actions.push(Action::ClosePluginEditor(track));
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    editor_contents(ui, track, current, midi.instrument_bypassed(), actions);
                });
            }
        },
    );
}

fn editor_shortcuts(ctx: &egui::Context, actions: &mut Vec<Action>) {
    if !ctx.input(|input| input.focused) {
        return;
    }
    let command = egui::Modifiers::COMMAND;
    for (modifiers, action) in [
        (command | egui::Modifiers::SHIFT, Action::SaveAs),
        (command, Action::Save),
    ] {
        if ctx.input_mut(|input| input.consume_key(modifiers, egui::Key::S)) {
            actions.push(action);
        }
    }
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    for (modifiers, key, action) in [
        (command | egui::Modifiers::SHIFT, egui::Key::Z, Action::Redo),
        (command, egui::Key::Z, Action::Undo),
        (command, egui::Key::Y, Action::Redo),
        (egui::Modifiers::NONE, egui::Key::Space, Action::TogglePlay),
        (
            egui::Modifiers::NONE,
            egui::Key::Home,
            Action::Transport(TransportCommand::ReturnToStart),
        ),
        (
            egui::Modifiers::NONE,
            egui::Key::Enter,
            Action::Transport(TransportCommand::ReturnToStart),
        ),
        (egui::Modifiers::NONE, egui::Key::L, Action::ToggleLoop),
        (egui::Modifiers::NONE, egui::Key::F, Action::ToggleFollow),
    ] {
        if ctx.input_mut(|input| input.consume_key(modifiers, key)) {
            actions.push(action);
        }
    }
}

fn editor_contents(
    ui: &mut Ui,
    track: usize,
    current: &SubtractiveSettings,
    bypassed: bool,
    actions: &mut Vec<Action>,
) {
    ui.push_id(("plugin-editor", track), |ui| {
        ui.horizontal(|ui| {
            let response = widgets::activator(ui, !bypassed);
            instrument_activator(
                ui,
                response,
                track,
                bypassed,
                "editor-instrument-activator",
                actions,
            );
            ui.heading("Legacy instrument");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, track, "close-plugin-editor", "Close Editor").clicked() {
                    actions.push(Action::ClosePluginEditor(track));
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt(("plugin-editor-scroll", track))
            .auto_shrink([false, false])
            .show(ui, |ui| instrument_controls(ui, track, current, actions));
    });
}

fn button(ui: &mut Ui, track: usize, name: &'static str, label: &str) -> egui::Response {
    let response = ui.button(label);
    ui.interact(response.rect, Id::new((name, track)), Sense::hover());
    response
}

fn instrument_controls(
    ui: &mut Ui,
    track: usize,
    current: &SubtractiveSettings,
    actions: &mut Vec<Action>,
) {
    let mut settings = *current;
    let mut edited = None;
    ui.add_space(6.0);
    Grid::new(("synth-controls", track))
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Oscillator");
            let waveform = egui::ComboBox::from_id_salt(("synth-waveform", track))
                .selected_text(waveform_name(settings.waveform))
                .show_ui(ui, |ui| {
                    for waveform in [Waveform::Sine, Waveform::Saw, Waveform::Square] {
                        if ui
                            .selectable_value(
                                &mut settings.waveform,
                                waveform,
                                waveform_name(waveform),
                            )
                            .changed()
                        {
                            edited = Some("waveform");
                        }
                    }
                });
            ui.interact(
                waveform.response.rect,
                Id::new(("plugin-parameter", track, "waveform")),
                Sense::hover(),
            );
            ui.end_row();
            let mut slider = |label, value: &mut f32, range, logarithmic, unit, key| {
                ui.label(label);
                let response = ui.add(
                    egui::Slider::new(value, range)
                        .logarithmic(logarithmic)
                        .suffix(unit),
                );
                ui.interact(
                    response.rect,
                    Id::new(("plugin-parameter", track, key)),
                    Sense::hover(),
                );
                if response.changed() {
                    edited = Some(key);
                }
                ui.end_row();
            };
            slider(
                "Cutoff",
                &mut settings.cutoff_hz,
                20.0..=20_000.0,
                true,
                " Hz",
                "cutoff",
            );
            slider(
                "Resonance",
                &mut settings.resonance,
                0.5..=10.0,
                false,
                " Q",
                "resonance",
            );
            slider(
                "Attack",
                &mut settings.attack_seconds,
                0.0..=10.0,
                true,
                " s",
                "attack",
            );
            slider(
                "Decay",
                &mut settings.decay_seconds,
                0.0..=10.0,
                true,
                " s",
                "decay",
            );
            slider(
                "Sustain",
                &mut settings.sustain,
                0.0..=1.0,
                false,
                "",
                "sustain",
            );
            slider(
                "Release",
                &mut settings.release_seconds,
                0.0..=10.0,
                true,
                " s",
                "release",
            );
            slider(
                "Output gain",
                &mut settings.gain,
                0.0..=1.0,
                false,
                "",
                "gain",
            );
        });
    if let Some(parameter) = edited {
        actions.push(Action::SetInstrumentParameter {
            track,
            instrument: Instrument::Subtractive(settings),
            parameter,
        });
    }
    if button(ui, track, "reset-instrument", "Reset Instrument").clicked()
        && *current != SubtractiveSettings::default()
    {
        actions.push(Action::SetInstrument {
            track,
            instrument: Some(Instrument::Subtractive(SubtractiveSettings::default())),
        });
    }
    ui.label(
        RichText::new("Polyphonic oscillator → low-pass filter → amplitude envelope")
            .size(10.5)
            .color(theme::TEXT_FAINT),
    );
}

fn waveform_name(waveform: Waveform) -> &'static str {
    match waveform {
        Waveform::Sine => "Sine",
        Waveform::Saw => "Saw",
        Waveform::Square => "Square",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidechain_source_picker_uses_the_plugins_actual_auxiliary_bus() {
        let ctx = egui::Context::default();
        let mut project = Project::new();
        project.add_audio_track("Kick");
        project.add_audio_track("Bass");
        let mut plugin = PluginInstance::new(
            sympoie::plugin::PluginDescriptor {
                path: "/test/Compressor.vst3".into(),
                class_id: "test-compressor".into(),
                name: "Compressor".into(),
                vendor: "Test".into(),
                kind: PluginKind::Effect,
            },
            Vec::new(),
            Vec::new(),
        );
        plugin.id = 7;
        let buses = [PluginBusInfo {
            index: 3,
            name: "External detector".into(),
            channels: 2,
        }];
        let frame = |events| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 600.0),
                    )),
                    events,
                    focused: true,
                    ..Default::default()
                },
                |ui| sidechain_controls(ui, &project, 2, &plugin, &buses, &mut actions),
            );
            output.textures_delta.clear();
            actions
        };
        frame(Vec::new());
        let source = ctx
            .read_response(Id::new(("sidechain-source", plugin.id)))
            .unwrap()
            .rect
            .center();
        for pressed in [true, false] {
            frame(vec![
                egui::Event::PointerMoved(source),
                egui::Event::PointerButton {
                    pos: source,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        frame(Vec::new());
        let kick = ctx
            .read_response(Id::new(("sidechain-source-option", plugin.id, 1_usize)))
            .unwrap()
            .rect
            .center();
        frame(vec![
            egui::Event::PointerMoved(kick),
            egui::Event::PointerButton {
                pos: kick,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let actions = frame(vec![egui::Event::PointerButton {
            pos: kick,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(
            actions,
            [Action::SetVst3Sidechain {
                id: plugin.id,
                route: Some(SidechainRoute {
                    source_track: 1,
                    bus_index: 3,
                    tap: SidechainTap::PostEffects,
                    gain: 1.0
                }),
            }]
        );
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn input(events: Vec<egui::Event>, focused: bool) -> egui::RawInput {
        egui::RawInput {
            focused,
            events,
            ..egui::RawInput::default()
        }
    }

    #[test]
    fn native_editor_dispatches_save_history_and_transport_shortcuts_once() {
        use egui::{Key, Modifiers};
        let command = Modifiers::COMMAND;
        let cases = [
            (command, Key::S, Action::Save),
            (command | Modifiers::SHIFT, Key::S, Action::SaveAs),
            (command, Key::Z, Action::Undo),
            (command | Modifiers::SHIFT, Key::Z, Action::Redo),
            (command, Key::Y, Action::Redo),
            (Modifiers::NONE, Key::Space, Action::TogglePlay),
            (
                Modifiers::NONE,
                Key::Home,
                Action::Transport(TransportCommand::ReturnToStart),
            ),
            (
                Modifiers::NONE,
                Key::Enter,
                Action::Transport(TransportCommand::ReturnToStart),
            ),
            (Modifiers::NONE, Key::L, Action::ToggleLoop),
            (Modifiers::NONE, Key::F, Action::ToggleFollow),
        ];
        for (modifiers, key, expected) in cases {
            let ctx = egui::Context::default();
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(input(vec![key_event(key, modifiers)], true), |ui| {
                editor_shortcuts(ui.ctx(), &mut actions);
                editor_shortcuts(ui.ctx(), &mut actions);
            });
            output.textures_delta.clear();
            assert_eq!(actions, vec![expected], "{modifiers:?} + {key:?}");
        }
    }

    #[test]
    fn native_editor_preserves_text_shortcuts_but_allows_saving_while_typing() {
        use egui::{Key, Modifiers};
        let ctx = egui::Context::default();
        let text_id = Id::new("plugin-numeric-entry");
        let mut text = "4000".to_owned();
        let mut output = ctx.run_ui(input(Vec::new(), true), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id(text_id))
                .request_focus();
        });
        output.textures_delta.clear();
        let command = Modifiers::COMMAND;
        let text_keys = [
            (Modifiers::NONE, Key::Space),
            (Modifiers::NONE, Key::Home),
            (Modifiers::NONE, Key::Enter),
            (Modifiers::NONE, Key::L),
            (Modifiers::NONE, Key::F),
            (command, Key::Z),
            (command | Modifiers::SHIFT, Key::Z),
            (command, Key::Y),
        ];
        let mut events: Vec<_> = text_keys
            .iter()
            .map(|(modifiers, key)| key_event(*key, *modifiers))
            .collect();
        events.push(key_event(Key::S, command));
        events.push(key_event(Key::S, command | Modifiers::SHIFT));
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(input(events, true), |ui| {
            assert!(ui.ctx().egui_wants_keyboard_input());
            editor_shortcuts(ui.ctx(), &mut actions);
            let remaining = ui.input(|input| input.events.clone());
            for (modifiers, key) in text_keys {
                assert!(remaining.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: event_key,
                        modifiers: event_modifiers,
                        pressed: true,
                        ..
                    } if *event_key == key && *event_modifiers == modifiers
                )));
            }
            ui.add(egui::TextEdit::singleline(&mut text).id(text_id));
        });
        output.textures_delta.clear();
        assert_eq!(actions, vec![Action::SaveAs, Action::Save]);
    }

    #[test]
    fn native_editor_ignores_shortcuts_when_unfocused() {
        let ctx = egui::Context::default();
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(
            input(
                vec![key_event(egui::Key::S, egui::Modifiers::COMMAND)],
                false,
            ),
            |ui| editor_shortcuts(ui.ctx(), &mut actions),
        );
        output.textures_delta.clear();
        assert!(actions.is_empty());
    }
}
