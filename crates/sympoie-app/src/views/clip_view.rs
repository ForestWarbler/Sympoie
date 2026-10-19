use eframe::egui::{
    self, Align, Align2, Color32, Grid, Layout, Mesh, Rect, RichText, Sense, Shape, Stroke, Ui,
    pos2, vec2,
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
use crate::ui::theme;
use crate::ui::waveform::WaveformCache;
use crate::ui::widgets;

const PROPERTIES_WIDTH: f32 = 236.0;

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
    ui.painter().rect_filled(side, 0.0, theme::BG_RAISED);
    ui.painter().vline(
        side.right() + 0.5,
        side.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );
    widgets::inside(ui, side, Layout::top_down(Align::Min), |ui| {
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

fn row(ui: &mut Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(label).color(theme::TEXT_DIM));
    ui.label(value);
    ui.end_row();
}

fn properties(ui: &mut Ui, project: &Project, clip: ClipRef, actions: &mut Vec<Action>) {
    let meter = project.time_signature();
    let tempo = project.tempo();
    let Some((start, end)) = edit::clip_span(project, clip) else {
        return;
    };
    let track = &project.tracks()[clip.track];
    let color = theme::track_color(project, clip.track);
    ui.spacing_mut().item_spacing.y = 4.0;
    let title = match edit::clip_name(project, clip) {
        Some(name) => format!("{} · {name}", track_label(project, clip.track)),
        None => track_label(project, clip.track),
    };
    box_title(ui, &title, color);
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            Grid::new("clip-grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    row(
                        ui,
                        "Start",
                        RichText::new(format::bar_beat_tick(meter, start)).font(theme::mono(11.0)),
                    );
                    row(
                        ui,
                        "End",
                        RichText::new(format::bar_beat_tick(meter, end)).font(theme::mono(11.0)),
                    );
                    let seconds = tempo.ticks_to_seconds(start).unwrap_or(0.0);
                    row(ui, "Time", format::clock(seconds));
                });
        });

    match track {
        Track::Audio(_) => {
            let Some(audio) = edit::audio_clip(project, clip).cloned() else {
                return;
            };
            box_title(ui, "Sample", theme::LOOP);
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    let rate = f64::from(SAMPLE_RATE);
                    Grid::new("sample-grid")
                        .num_columns(2)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
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
                            ui.label(RichText::new("Gain").color(theme::TEXT_DIM));
                            let mut db = format::gain_to_db(audio.gain.abs()).max(-70.0);
                            ui.horizontal(|ui| {
                                let response = ui.add(
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
                                if widgets::toggle(
                                    ui,
                                    "ø",
                                    inverted,
                                    theme::ACCENT,
                                    vec2(20.0, 18.0),
                                )
                                .on_hover_text("Invert polarity")
                                .clicked()
                                {
                                    actions.push(Action::SetClipGain {
                                        clip,
                                        gain: -audio.gain,
                                    });
                                }
                            });
                            ui.end_row();
                            let pitch_label = ui.label(RichText::new("Pitch").color(theme::TEXT_DIM));
                            ui.horizontal(|ui| {
                                let mut semitones = audio.pitch_semitones;
                                let response = ui.add(
                                    egui::DragValue::new(&mut semitones)
                                        .speed(0.1)
                                        .range(-24.0..=24.0)
                                        .max_decimals(2)
                                        .suffix(" st"),
                                ).labelled_by(pitch_label.id)
                                    .on_hover_text("Pitch in semitones; duration stays unchanged. Double-click to type.");
                                if response.changed() {
                                    actions.push(Action::SetClipPitch { clip, semitones });
                                }
                                if ui.small_button("Reset").on_hover_text("Reset pitch to 0 semitones").clicked() {
                                    actions.push(Action::ResetClipPitch(clip));
                                }
                            });
                            ui.end_row();
                            let algorithm_label = ui.label(RichText::new("Algorithm").color(theme::TEXT_DIM));
                            pitch_options(ui, &audio, clip, algorithm_label.id, actions);
                            ui.end_row();
                            ui.label(RichText::new("Process").color(theme::TEXT_DIM));
                            if ui.button("Reverse").on_hover_text("Reverse the visible audio region (R); keep hidden audio").clicked() {
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
            box_title(ui, "Notes", theme::LOOP);
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    let bars = midi.length_ticks as f64 / meter.ticks_per_bar() as f64;
                    Grid::new("notes-grid")
                        .num_columns(2)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            row(ui, "Length", format!("{bars:.2} bars"));
                            let shown = midi.audible_notes().count();
                            let hidden = midi.notes.len() - shown;
                            let notes = if hidden > 0 {
                                format!("{shown} ({hidden} outside the clip)")
                            } else {
                                shown.to_string()
                            };
                            row(ui, "Notes", notes);
                            if midi.offset_ticks > 0 {
                                let offset =
                                    midi.offset_ticks as f64 / meter.ticks_per_bar() as f64;
                                row(ui, "Start offset", format!("{offset:.2} bars"));
                            }
                        });
                    if ui.button("Edit Notes").clicked() {
                        actions.push(Action::OpenPianoRoll(clip));
                    }
                });
        }
        _ => {}
    }

    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
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
    ui.vertical(|ui| {
        let mut algorithm = audio.pitch_algorithm;
        let mut preserve_formants = audio.preserve_formants;
        egui::ComboBox::from_id_salt(("pitch-algorithm", clip))
            .width(145.0)
            .selected_text(algorithm.label())
            .show_ui(ui, |ui| {
                for choice in [
                    PitchAlgorithm::Signalsmith,
                    PitchAlgorithm::RubberBandR3,
                    PitchAlgorithm::Builtin,
                ] {
                    ui.selectable_value(&mut algorithm, choice, choice.label());
                }
            })
            .response
            .labelled_by(algorithm_label);
        ui.add_enabled(
            algorithm.supports_formants(),
            egui::Checkbox::new(&mut preserve_formants, "Preserve Formants"),
        )
        .on_hover_text("Keep vocal resonances closer to the original while changing pitch");
        if algorithm != audio.pitch_algorithm || preserve_formants != audio.preserve_formants {
            actions.push(Action::SetClipPitchOptions {
                clip,
                algorithm,
                preserve_formants,
            });
        }
    });
}

fn sample_display(
    ui: &mut Ui,
    rect: Rect,
    project: &Project,
    clip: ClipRef,
    waveforms: &mut WaveformCache,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::LANE);
    let color = theme::track_color(project, clip.track);
    let Some(audio) = edit::audio_clip(project, clip).cloned() else {
        if let Some(midi) = edit::midi_clip(project, clip) {
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
        return;
    };
    let Some((overview, buffer)) = waveforms.get(project, audio.asset) else {
        return;
    };
    let gain = audio.gain.abs().min(4.0);
    let invert = audio.gain < 0.0;
    let halves = [
        Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y)),
        Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.max),
    ];
    for half in halves {
        painter.hline(
            half.x_range(),
            half.center().y,
            Stroke::new(1.0, Color32::from_white_alpha(18)),
        );
    }
    painter.hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, theme::BORDER),
    );
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
                    color,
                );
            }
        }
        x += 1.0;
    }
    painter.add(Shape::mesh(mesh));
    painter.text(
        rect.left_top() + vec2(6.0, 4.0),
        Align2::LEFT_TOP,
        format::duration(audio.frame_count as f64 / f64::from(SAMPLE_RATE)),
        theme::prop(10.0),
        theme::TEXT_DIM,
    );
}
