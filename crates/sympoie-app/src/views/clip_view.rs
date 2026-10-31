use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Align, Align2, Color32, Grid, Layout, Margin, Mesh, Painter, Rect, RichText, Sense,
    Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};
use sympoie::SAMPLE_RATE;
use sympoie::clip::AudioClip;
use sympoie::pitch::PitchAlgorithm;
use sympoie::project::Project;
use sympoie::track::Track;

use crate::actions::Action;
use crate::document::edit::{self, ClipRef};
use crate::document::rows::track_label;
use crate::state::Selection;
use crate::ui::format;
use crate::ui::rack;
use crate::ui::theme;
use crate::ui::waveform::WaveformCache;
use crate::ui::widgets;

const PROPERTIES_WIDTH: f32 = 236.0;
/// The inspector's value fields and the keys beside them share one slim height.
const FIELD_HEIGHT: f32 = 17.0;
/// Every section uses this label column, so the values line up down the inspector. It
/// fits the widest label, "Start offset".
const LABEL_WIDTH: f32 = 62.0;
const DISPLAY_RADIUS: f32 = 5.0;

pub fn show(
    ui: &mut Ui,
    project: &Project,
    selection: &Selection,
    waveforms: &mut WaveformCache,
    actions: &mut Vec<Action>,
) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, theme::BG_PANEL);
    let Some(clip) = selection
        .clip
        .filter(|clip| edit::clip_span(project, *clip).is_some())
    else {
        ui.painter().text(
            full.center(),
            Align2::CENTER_CENTER,
            "Select a clip to see its properties here",
            theme::prop(11.5),
            theme::TEXT_DIM,
        );
        ui.advance_cursor_after_rect(full);
        return;
    };
    let side = Rect::from_min_max(
        full.min,
        pos2(
            (full.left() + PROPERTIES_WIDTH).min(full.right()),
            full.bottom(),
        ),
    );
    let display = Rect::from_min_max(
        pos2(side.right() + 4.0, full.top() + 4.0),
        full.max - vec2(4.0, 4.0),
    );
    widgets::inside(ui, side, Layout::top_down(Align::Min), |ui| {
        // A short drawer cuts the inspector off, so the scroll handle shows without a hover.
        ui.spacing_mut().scroll.dormant_handle_opacity = 0.4;
        egui::ScrollArea::vertical()
            .id_salt("clip-properties")
            .auto_shrink([false, false])
            .show(ui, |ui| properties(ui, project, clip, actions));
    });
    if display.width() > 40.0 && display.height() > 30.0 {
        sample_display(ui, display, project, clip, waveforms);
    }
    ui.advance_cursor_after_rect(full);
}

fn box_title(ui: &mut Ui, text: &str, color: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    widgets::color_bar(ui.painter(), rect, color, text);
}

fn caption(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let painter = ui.painter();
    let galley = rack::spaced(painter, text, theme::prop(8.5), theme::TEXT_FAINT, 0.9);
    let left = rect.left() + 10.0;
    let rule = left + galley.size().x + 6.0;
    painter.galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::TEXT_FAINT,
    );
    if rule < rect.right() - 10.0 {
        painter.hline(
            rule..=(rect.right() - 10.0),
            rect.center().y.round() + 0.5,
            Stroke::new(1.0, theme::HAIRLINE),
        );
    }
}

fn section<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::NONE
        .inner_margin(Margin {
            left: 10,
            right: 10,
            top: 5,
            bottom: 5,
        })
        .show(ui, contents)
        .inner
}

fn grid(id: &str) -> Grid {
    Grid::new(id)
        .num_columns(2)
        .striped(false)
        .min_col_width(LABEL_WIDTH)
        .spacing([8.0, 3.0])
}

fn label(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).size(11.0).color(theme::TEXT_DIM))
}

fn row(ui: &mut Ui, text: &str, value: String) {
    label(ui, text);
    field(
        ui,
        RichText::new(value)
            .font(theme::mono(11.0))
            .color(theme::TEXT),
    );
    ui.end_row();
}

/// Read-only values are plain text, so that only the editable ones look like fields. The
/// fields' padding sets them in, so every value starts at the same x.
fn field(ui: &mut Ui, value: impl Into<egui::WidgetText>) {
    let width = ui.available_width().max(0.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, FIELD_HEIGHT), Sense::hover());
    let padding = ui.spacing().button_padding.x;
    widgets::inside(
        ui,
        rect.shrink2(vec2(padding, 0.0)),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add(egui::Label::new(value).truncate());
        },
    );
}

/// The count is a number, so it is mono; the hidden-note remark is prose and must fit.
fn note_count(shown: usize, hidden: usize) -> LayoutJob {
    let style = |font_id, color| TextFormat {
        font_id,
        color,
        valign: Align::Center,
        ..TextFormat::default()
    };
    let mut job = LayoutJob::default();
    job.append(
        &shown.to_string(),
        0.0,
        style(theme::mono(11.0), theme::TEXT),
    );
    if hidden > 0 {
        job.append(
            &format!(" ({hidden} outside the clip)"),
            0.0,
            style(theme::prop(11.0), theme::TEXT_DIM),
        );
    }
    job
}

fn value_field(ui: &mut Ui, width: f32, value: egui::DragValue<'_>) -> egui::Response {
    let layout = Layout::left_to_right(Align::Center).with_main_align(Align::Min);
    ui.with_layout(layout, |ui| {
        let style = ui.style_mut();
        style.spacing.interact_size.x = width;
        style.override_font_id = Some(theme::mono(11.0));
        style.drag_value_text_style = egui::TextStyle::Monospace;
        widgets::field_visuals(style);
        let response = ui.add(value);
        if response.has_focus() {
            widgets::focus_ring(ui.painter(), response.rect, widgets::FIELD_RADIUS);
        }
        response
    })
    .inner
}

fn properties(ui: &mut Ui, project: &Project, clip: ClipRef, actions: &mut Vec<Action>) {
    let meter = project.time_signature();
    let tempo = project.tempo();
    let Some((start, end)) = edit::clip_span(project, clip) else {
        return;
    };
    let track = &project.tracks()[clip.track];
    let color = theme::track_color(project, clip.track);
    let spacing = ui.spacing_mut();
    spacing.item_spacing.y = 3.0;
    spacing.interact_size.y = FIELD_HEIGHT;
    spacing.button_padding.y = 1.0;
    let title = match edit::clip_name(project, clip) {
        Some(name) => format!("{} · {name}", track_label(project, clip.track)),
        None => track_label(project, clip.track),
    };
    box_title(ui, &title, color);
    section(ui, |ui| {
        grid("clip-grid").show(ui, |ui| {
            row(ui, "Start", format::bar_beat_tick(meter, start));
            row(ui, "End", format::bar_beat_tick(meter, end));
            let seconds = tempo.ticks_to_seconds(start).unwrap_or(0.0);
            row(ui, "Time", format::clock(seconds));
        });
    });

    match track {
        Track::Audio(_) => {
            let Some(audio) = edit::audio_clip(project, clip).cloned() else {
                return;
            };
            caption(ui, "Sample");
            section(ui, |ui| {
                let rate = f64::from(SAMPLE_RATE);
                grid("sample-grid").show(ui, |ui| {
                    row(
                        ui,
                        "Length",
                        format::duration(audio.frame_count as f64 / rate),
                    );
                    row(
                        ui,
                        "Offset",
                        format::duration(audio.source_start as f64 / rate),
                    );
                    if let Ok(asset) = project.audio_asset(audio.asset) {
                        row(ui, "Source", format::duration(asset.len() as f64 / rate));
                    }
                    label(ui, "Gain");
                    let mut db = format::gain_to_db(audio.gain.abs()).max(-70.0);
                    ui.horizontal(|ui| {
                        let key = vec2(22.0, FIELD_HEIGHT);
                        let width = ui.available_width() - key.x - ui.spacing().item_spacing.x;
                        let response = value_field(
                            ui,
                            width,
                            egui::DragValue::new(&mut db)
                                .speed(0.2)
                                .range(-70.0..=24.0)
                                .custom_formatter(|value, _| {
                                    if value <= -69.95 {
                                        "-∞ dB".to_owned()
                                    } else {
                                        format!("{value:+.1} dB")
                                    }
                                }),
                        );
                        if response.changed() {
                            let magnitude = if db <= -69.95 {
                                0.0
                            } else {
                                format::db_to_gain(db)
                            };
                            let gain = if audio.gain < 0.0 {
                                -magnitude
                            } else {
                                magnitude
                            };
                            actions.push(Action::SetClipGain { clip, gain });
                        }
                        let inverted = audio.gain < 0.0;
                        // The toggle draws its key 1 px inside its rect, so the rect reaches
                        // 1 px past the row to make the key as tall as the field.
                        let slot = ui.allocate_space(key).1.expand2(vec2(0.0, 1.0));
                        let polarity =
                            widgets::inside(ui, slot, Layout::left_to_right(Align::Center), |ui| {
                                widgets::glyph_toggle(ui, "ø", inverted, theme::ACCENT, slot.size())
                            });
                        if polarity.on_hover_text("Invert polarity").clicked() {
                            actions.push(Action::SetClipGain {
                                clip,
                                gain: -audio.gain,
                            });
                        }
                    });
                    ui.end_row();
                    let pitch_label = label(ui, "Pitch");
                    ui.horizontal(|ui| {
                        let key = vec2(46.0, FIELD_HEIGHT);
                        let width = ui.available_width() - key.x - ui.spacing().item_spacing.x;
                        let mut semitones = audio.pitch_semitones;
                        let response = value_field(
                            ui,
                            width,
                            egui::DragValue::new(&mut semitones)
                                .speed(0.1)
                                .range(-24.0..=24.0)
                                .max_decimals(2)
                                .suffix(" st"),
                        )
                        .labelled_by(pitch_label.id)
                        .on_hover_text(
                            "Pitch in semitones; duration stays unchanged. Double-click to type.",
                        );
                        if response.changed() {
                            actions.push(Action::SetClipPitch { clip, semitones });
                        }
                        if ui
                            .add_sized(key, egui::Button::new("Reset").small())
                            .on_hover_text("Reset pitch to 0 semitones")
                            .clicked()
                        {
                            actions.push(Action::ResetClipPitch(clip));
                        }
                    });
                    ui.end_row();
                    let algorithm_label = label(ui, "Algorithm");
                    pitch_options(ui, &audio, clip, algorithm_label.id, actions);
                    ui.end_row();
                    label(ui, "Process");
                    if ui
                        .button("Reverse")
                        .on_hover_text("Reverse the visible audio region (R); keep hidden audio")
                        .clicked()
                    {
                        actions.push(Action::ReverseClip(clip));
                    }
                    ui.end_row();
                });
            });
        }
        Track::Midi(_) => {
            let Some(midi) = edit::midi_clip(project, clip) else {
                return;
            };
            caption(ui, "Notes");
            section(ui, |ui| {
                let bars = midi.length_ticks as f64 / meter.ticks_per_bar() as f64;
                grid("notes-grid").show(ui, |ui| {
                    row(ui, "Length", format!("{bars:.2} bars"));
                    let shown = midi.audible_notes().count();
                    label(ui, "Notes");
                    field(ui, note_count(shown, midi.notes.len() - shown));
                    ui.end_row();
                    if midi.offset_ticks > 0 {
                        let offset = midi.offset_ticks as f64 / meter.ticks_per_bar() as f64;
                        row(ui, "Start offset", format!("{offset:.2} bars"));
                    }
                    ui.allocate_space(egui::Vec2::ZERO);
                    if ui.button("Edit Notes").clicked() {
                        actions.push(Action::OpenPianoRoll(clip));
                    }
                    ui.end_row();
                });
            });
        }
        _ => {}
    }

    egui::Frame::NONE
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Split")
                    .on_hover_text("Split at the play position")
                    .clicked()
                {
                    actions.push(Action::SplitClip(clip));
                }
                if ui.button("Duplicate").clicked() {
                    actions.push(Action::DuplicateClip(clip));
                }
                if ui.button("Loop").on_hover_text("Loop this clip").clicked() {
                    actions.push(Action::LoopClip(clip));
                }
                if ui
                    .button(RichText::new("Delete").color(theme::ERROR))
                    .clicked()
                {
                    actions.push(Action::DeleteClip(clip));
                }
            });
        });
}

fn pitch_options(
    ui: &mut Ui,
    audio: &AudioClip,
    clip: ClipRef,
    algorithm_label: egui::Id,
    actions: &mut Vec<Action>,
) {
    let mut algorithm = audio.pitch_algorithm;
    let mut preserve_formants = audio.preserve_formants;
    ui.scope(|ui| {
        widgets::field_visuals(ui.style_mut());
        egui::ComboBox::from_id_salt(("pitch-algorithm", clip))
            .popup_style(widgets::menu_style())
            .width(ui.available_width())
            .selected_text(algorithm.label())
            .show_ui(ui, |ui| {
                for choice in [
                    PitchAlgorithm::Signalsmith,
                    PitchAlgorithm::RubberBandR3,
                    PitchAlgorithm::Builtin,
                ] {
                    widgets::choice_value(ui, &mut algorithm, choice, choice.label());
                }
            })
            .response
            .labelled_by(algorithm_label);
    });
    // The checkbox takes its own row, so the label stays level with the menu.
    ui.end_row();
    ui.allocate_space(egui::Vec2::ZERO);
    widgets::checkbox(
        ui,
        algorithm.supports_formants(),
        &mut preserve_formants,
        "Preserve Formants",
    )
    .on_hover_text("Keep vocal resonances closer to the original while changing pitch");
    if algorithm != audio.pitch_algorithm || preserve_formants != audio.preserve_formants {
        actions.push(Action::SetClipPitchOptions {
            clip,
            algorithm,
            preserve_formants,
        });
    }
}

fn sample_display(
    ui: &Ui,
    rect: Rect,
    project: &Project,
    clip: ClipRef,
    waveforms: &mut WaveformCache,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, DISPLAY_RADIUS, theme::LANE);
    let color = theme::track_color(project, clip.track);
    if let Some(audio) = edit::audio_clip(project, clip) {
        waveform(&painter, rect, color, audio, project, waveforms);
    } else if let Some(midi) = edit::midi_clip(project, clip) {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            format!(
                "{} notes · double-click the clip or choose Edit Notes",
                midi.audible_notes().count()
            ),
            theme::prop(11.0),
            theme::TEXT_DIM,
        );
    }
    painter.rect_stroke(
        rect,
        DISPLAY_RADIUS,
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
}

fn waveform(
    painter: &Painter,
    rect: Rect,
    color: Color32,
    audio: &AudioClip,
    project: &Project,
    waveforms: &mut WaveformCache,
) {
    let Some((overview, buffer)) = waveforms.get(project, audio.asset) else {
        return;
    };
    painter.rect_filled(rect, DISPLAY_RADIUS, theme::tint(theme::LANE, color, 0.07));
    let gain = audio.gain.abs().min(4.0);
    let invert = audio.gain < 0.0;
    let halves = [
        Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y)),
        Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.max),
    ];
    for half in halves {
        painter.hline(
            half.x_range(),
            half.center().y.round() + 0.5,
            Stroke::new(1.0, Color32::from_white_alpha(16)),
        );
    }
    painter.hline(
        rect.x_range(),
        rect.center().y.round() + 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    let ink = theme::tint(color, Color32::WHITE, 0.25);
    let count = audio.frame_count.max(1) as f64;
    let width = f64::from(rect.width().max(1.0));
    let frame_at = |x: f32| -> u64 {
        audio.source_start + (f64::from(x - rect.left()) / width * count).max(0.0) as u64
    };
    let limit = audio.source_start + audio.frame_count;
    let mut mesh = Mesh::default();
    let mut x = rect.left();
    while x < rect.right() {
        let from = frame_at(x).min(limit);
        let to = frame_at(x + 1.0).clamp(from + 1, limit.max(from + 1));
        if let Some(extent) = overview.extent(buffer.samples(), from, to) {
            for (half, low, high) in [
                (halves[0], extent[0], extent[1]),
                (halves[1], extent[2], extent[3]),
            ] {
                let (low, high) = if invert { (-high, -low) } else { (low, high) };
                let center = half.center().y;
                let scale = half.height() * 0.45;
                let top = center - (high * gain).clamp(-1.0, 1.0) * scale;
                let bottom = center - (low * gain).clamp(-1.0, 1.0) * scale;
                mesh.add_colored_rect(
                    Rect::from_x_y_ranges(
                        x..=x + 1.0,
                        top.min(center - 0.5)..=bottom.max(center + 0.5),
                    ),
                    ink,
                );
            }
        }
        x += 1.0;
    }
    painter.add(Shape::mesh(mesh));
    let length = painter.layout_no_wrap(
        format::duration(audio.frame_count as f64 / f64::from(SAMPLE_RATE)),
        theme::mono(10.0),
        theme::TEXT_DIM,
    );
    let chip = Rect::from_min_size(rect.min + vec2(6.0, 6.0), length.size() + vec2(10.0, 2.0));
    painter.rect_filled(chip, 3.0, Color32::from_black_alpha(110));
    painter.galley(chip.min + vec2(5.0, 1.0), length, theme::TEXT_DIM);
}
