use eframe::egui::{self, Align, Id, Layout, Rect, Response, RichText, Ui, pos2, vec2};
use sympoie::project::Project;
use sympoie::time::TimeSignature;
use sympoie::transport::PlaybackState;

use crate::actions::Action;
use crate::audio::{EngineStatus, OutputInfo};
use crate::state::LoopRegion;
use crate::ui::analog;
use crate::ui::format;
use crate::ui::theme;
use crate::ui::widgets::{self, Icon, MeterState};

pub struct Inputs<'a> {
    pub project: &'a Project,
    pub status: &'a EngineStatus,
    pub loop_region: LoopRegion,
    pub follow: bool,
    pub playhead_tick: f64,
    pub output: &'a OutputInfo,
    pub recording: bool,
}

const KEYS_WIDTH: f32 = 180.0;
const DISPLAY_WIDTH: f32 = 520.0;
const DRAG_PER_STEP: f32 = 8.0;

pub fn show(ui: &mut Ui, inputs: &Inputs<'_>, meter: &mut MeterState, actions: &mut Vec<Action>) {
    let full = ui.max_rect();
    let rail = full.expand2(vec2(8.0, 6.0));
    analog::gradient_rect(ui.painter(), rail, theme::BG_RAISED, theme::BG_PANEL);
    ui.painter().hline(
        rail.x_range(),
        rail.top() + 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(14)),
    );
    ui.painter().hline(
        rail.x_range(),
        rail.bottom() - 0.5,
        egui::Stroke::new(1.0, theme::BORDER),
    );
    let keys = Rect::from_min_max(full.min, pos2(full.left() + KEYS_WIDTH, full.bottom()));
    let display_left = (full.center().x - DISPLAY_WIDTH / 2.0).max(keys.right() + 12.0);
    let display = Rect::from_min_max(
        pos2(display_left, full.top() - 3.0),
        pos2(display_left + DISPLAY_WIDTH, full.bottom() + 3.0),
    );
    let right = Rect::from_min_max(pos2(display.right() + 12.0, full.top()), full.max);

    widgets::inside(ui, keys, Layout::left_to_right(Align::Center), |ui| {
        keys_section(ui, inputs, actions);
    });
    analog::well(ui.painter(), display, 4.0);
    ui.painter().rect_filled(
        display.shrink(1.0),
        3.0,
        egui::Color32::from_rgb(0x0d, 0x0d, 0x0d),
    );
    widgets::inside(
        ui,
        display.shrink2(vec2(10.0, 0.0)),
        Layout::left_to_right(Align::Center),
        |ui| display_section(ui, inputs, actions),
    );
    analog::glass(ui.painter(), display.shrink(1.0));
    if right.width() > 60.0 {
        widgets::inside(ui, right, Layout::right_to_left(Align::Center), |ui| {
            output_section(ui, inputs, meter);
        });
    }
    ui.advance_cursor_after_rect(full);
}

fn legend(ui: &mut Ui, text: &str) {
    let painter = ui.painter().clone();
    let galley = crate::ui::rack::spaced(&painter, text, theme::prop(8.5), theme::TEXT_FAINT, 0.6);
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    painter.galley(rect.min, galley, theme::TEXT_FAINT);
}

fn keys_section(ui: &mut Ui, inputs: &Inputs<'_>, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.x = 3.0;
    let playing = inputs.status.state == PlaybackState::Playing;
    let can_play = inputs.status.render_error.is_none();
    let play = ui.add_enabled_ui(can_play || playing, |ui| {
        if playing {
            widgets::transport_button(ui, Icon::Play, Some(theme::PLAY), "Pause (Space)")
        } else {
            widgets::transport_button(ui, Icon::Play, None, "Play from start marker (Space)")
        }
    });
    if play.inner.clicked() {
        actions.push(Action::TogglePlay);
    }
    if let Some(reason) = &inputs.status.render_error {
        play.response
            .on_disabled_hover_text(format!("Playback unavailable: {reason}"));
    }
    if widgets::transport_button(
        ui,
        Icon::Stop,
        None,
        "Stop; press again to return to the start (Home)",
    )
    .clicked()
    {
        actions.push(Action::Stop);
    }
    if widgets::transport_button(
        ui,
        Icon::Record,
        inputs.recording.then_some(theme::ERROR),
        if inputs.recording {
            "Stop recording (Q)"
        } else {
            "Record (Q)"
        },
    )
    .clicked()
    {
        actions.push(Action::ToggleRecord);
    }
    ui.add_space(10.0);
    if widgets::icon_button(
        ui,
        Icon::Loop,
        inputs.loop_region.enabled,
        theme::ACCENT,
        "Loop switch (L). Drag in the loop strip under the bar numbers to set the range.",
    )
    .clicked()
    {
        actions.push(Action::ToggleLoop);
    }
    if widgets::icon_button(
        ui,
        Icon::Follow,
        inputs.follow,
        theme::ACCENT,
        "Follow: keep the play position in view (F)",
    )
    .clicked()
    {
        actions.push(Action::ToggleFollow);
    }
}

fn display_section(ui: &mut Ui, inputs: &Inputs<'_>, actions: &mut Vec<Action>) {
    let project = inputs.project;
    let meter = project.time_signature();
    ui.spacing_mut().item_spacing.x = 5.0;

    legend(ui, "Pos");
    let tick = inputs.playhead_tick.max(0.0).round() as u64;
    let seconds = inputs.playhead_tick * 60.0 / (project.bpm() * sympoie::TICKS_PER_QUARTER as f64);
    widgets::value_box(ui, &format::bar_beat(meter, tick), 70.0, theme::TEXT)
        .on_hover_text("Arrangement position (bar.beat.sixteenth)");
    ui.label(
        RichText::new(format::clock(seconds))
            .font(theme::mono(10.0))
            .color(theme::TEXT_DIM),
    );
    ui.add_space(8.0);

    legend(ui, "Loop");
    let region = inputs.loop_region;
    let bar = meter.ticks_per_bar();
    let length = region.end.saturating_sub(region.start);
    let start_response = widgets::value_box(
        ui,
        &format::bar_beat(meter, region.start),
        62.0,
        if region.enabled {
            theme::ACCENT
        } else {
            theme::TEXT
        },
    )
    .on_hover_text("Loop start. Drag up or down to move the loop by bars.");
    let steps = drag_steps(ui, &start_response, Id::new("loop-start-drag"));
    if steps != 0 {
        let start = offset(region.start, steps, bar);
        actions.push(Action::SetLoop {
            start,
            end: start + length,
            enable: false,
        });
    }
    let length_response = widgets::value_box(
        ui,
        &loop_length(meter, length),
        62.0,
        if region.enabled {
            theme::ACCENT
        } else {
            theme::TEXT
        },
    )
    .on_hover_text("Loop length (bars.beats.sixteenths). Drag up or down to change it.");
    let steps = drag_steps(ui, &length_response, Id::new("loop-length-drag"));
    if steps != 0 {
        let length = offset(length, steps, bar).max(meter.ticks_per_beat());
        actions.push(Action::SetLoop {
            start: region.start,
            end: region.start + length,
            enable: false,
        });
    }
    ui.add_space(8.0);
    tempo_section(ui, project, actions);
}

fn tempo_section(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.x = 4.0;

    legend(ui, "Bpm");
    let mut bpm = project.bpm();
    let response = readout_control(ui, |ui| {
        ui.add(
            egui::DragValue::new(&mut bpm)
                .speed(0.1)
                .range(20.0..=999.0)
                .fixed_decimals(2),
        )
    })
    .on_hover_text("Tempo in BPM. Clips keep their bar positions; audio is not stretched.");
    if response.changed() {
        actions.push(Action::SetBpm(bpm));
    }
    ui.add_space(6.0);

    legend(ui, "Metre");
    let meter_now = project.time_signature();
    let mut numerator = meter_now.numerator();
    let numerator_response = readout_control(ui, |ui| {
        ui.add(
            egui::DragValue::new(&mut numerator)
                .range(1..=32)
                .speed(0.05),
        )
    })
    .on_hover_text("Time signature: beats per bar");
    if numerator_response.changed()
        && let Ok(next) = TimeSignature::new(numerator, meter_now.denominator())
    {
        actions.push(Action::SetTimeSignature(next));
    }
    ui.label(RichText::new("/").color(theme::TEXT_DIM));
    let menu_style = ui.style().clone();
    readout_control(ui, |ui| {
        egui::ComboBox::from_id_salt("meter-denominator")
            .width(30.0)
            .selected_text(meter_now.denominator().to_string())
            .show_ui(ui, |ui| {
                ui.set_style(menu_style);
                for denominator in [1, 2, 4, 8, 16, 32] {
                    if ui
                        .selectable_label(
                            meter_now.denominator() == denominator,
                            denominator.to_string(),
                        )
                        .clicked()
                        && let Ok(next) = TimeSignature::new(meter_now.numerator(), denominator)
                    {
                        actions.push(Action::SetTimeSignature(next));
                    }
                }
            })
            .response
    })
    .on_hover_text("Time signature: beat unit");
}

fn readout_control(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> Response) -> Response {
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .scope(|ui| {
            let widgets = &mut ui.visuals_mut().widgets;
            for state in [
                &mut widgets.inactive,
                &mut widgets.hovered,
                &mut widgets.active,
                &mut widgets.open,
            ] {
                state.bg_fill = egui::Color32::TRANSPARENT;
                state.weak_bg_fill = egui::Color32::TRANSPARENT;
                state.bg_stroke = egui::Stroke::NONE;
            }
            add(ui)
        })
        .inner;
    ui.painter().set(
        background,
        analog::readout(
            response.rect,
            response.hovered() || response.has_focus() || response.dragged(),
        ),
    );
    response
}

fn output_section(ui: &mut Ui, inputs: &Inputs<'_>, meter: &mut MeterState) {
    ui.spacing_mut().item_spacing.x = 6.0;
    widgets::meter(ui, meter, vec2(96.0, 12.0), false);
    let output = inputs.output;
    let color = if output.is_device {
        theme::TEXT_DIM
    } else {
        theme::WARN
    };
    let text = if output.is_device {
        format!(
            "{} · {} kHz",
            output.description,
            output.sample_rate as f32 / 1000.0
        )
    } else {
        output.description.clone()
    };
    let hint = if output.is_device {
        format!("Audio output: {}", output.detail)
    } else {
        format!(
            "{}.\nPlayback follows a silent clock; WAV export still works.",
            output.detail
        )
    };
    ui.add(egui::Label::new(RichText::new(text).size(10.5).color(color)).truncate())
        .on_hover_text(hint);
}

fn loop_length(meter: TimeSignature, ticks: u64) -> String {
    let bar = meter.ticks_per_bar().max(1);
    let beat = meter.ticks_per_beat().max(1);
    let bars = ticks / bar;
    let beats = (ticks % bar) / beat;
    let sixteenths = (ticks % beat) / format::TICKS_PER_SIXTEENTH;
    format!("{bars}.{beats}.{sixteenths}")
}

fn offset(value: u64, steps: i64, step: u64) -> u64 {
    let delta = steps.unsigned_abs().saturating_mul(step);
    if steps >= 0 {
        value.saturating_add(delta)
    } else {
        value.saturating_sub(delta)
    }
}

fn drag_steps(ui: &Ui, response: &Response, id: Id) -> i64 {
    if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(id, 0.0_f32));
    }
    if !response.dragged() {
        return 0;
    }
    let mut carried: f32 = ui.data(|data| data.get_temp(id)).unwrap_or(0.0);
    carried -= response.drag_delta().y;
    let steps = (carried / DRAG_PER_STEP).trunc();
    carried -= steps * DRAG_PER_STEP;
    ui.data_mut(|data| data.insert_temp(id, carried));
    steps as i64
}
