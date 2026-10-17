use eframe::egui::{self, Color32, Rect, Response, RichText, Sense, Stroke, Ui, pos2, vec2};
use sympoie::project::Project;
use sympoie::track::{AudioOutput, MixerSettings, TrackId};

use crate::actions::Action;
use crate::document::{edit, rows};
use crate::ui::format;
use crate::ui::theme;

pub use crate::document::rows::track_title;

pub fn mixer_controls(ui: &mut Ui, project: &Project, index: usize, actions: &mut Vec<Action>) {
    let track = &project.tracks()[index];
    let mixer = *track.mixer();
    let color = theme::track_color(project, index);
    let mut db = format::gain_to_db(mixer.gain()).max(-70.0);
    let (gain_response, gain_bar) = header_value(
        ui,
        68.0,
        egui::DragValue::new(&mut db)
            .speed(0.2)
            .range(-70.0..=6.0)
            .custom_formatter(|value, _| {
                if value <= -69.95 {
                    "-∞ dB".to_owned()
                } else {
                    format!("{value:+.1} dB")
                }
            })
            .custom_parser(|text| {
                let text = text.trim().trim_end_matches("dB").trim();
                if text.starts_with("-inf") || text.starts_with("-∞") {
                    Some(-70.0)
                } else {
                    text.parse().ok()
                }
            }),
    );
    let gain_response = gain_response
        .on_hover_text("Track volume. Drag, or double-click to type. The bar marks 0 dB.");
    let gain = if db <= -69.95 {
        0.0
    } else {
        format::db_to_gain(db)
    };
    value_bar(
        ui,
        gain_bar,
        0.0,
        format::gain_to_fader(gain),
        format::gain_to_fader(1.0),
        color,
    );
    if gain_response.changed()
        && let Ok(next) = MixerSettings::new(gain, mixer.pan(), mixer.output())
    {
        actions.push(Action::SetMixer {
            track: index,
            mixer: next.with_muted(mixer.muted()).with_soloed(mixer.soloed()),
        });
    }
    let mut pan = f64::from(mixer.pan()) * 100.0;
    let (pan_response, pan_bar) = header_value(
        ui,
        44.0,
        egui::DragValue::new(&mut pan)
            .speed(0.5)
            .range(-100.0..=100.0)
            .custom_formatter(|value, _| format::pan(value as f32 / 100.0))
            .custom_parser(|text| format::parse_pan(text).map(|pan| f64::from(pan) * 100.0)),
    );
    let pan_response = pan_response.on_hover_text(
        "Track panning. Drag, or double-click to type. The bar extends left or right from center.",
    );
    value_bar(
        ui,
        pan_bar,
        0.5,
        (pan as f32 / 100.0 + 1.0) * 0.5,
        0.5,
        color,
    );
    if pan_response.changed()
        && let Ok(next) = MixerSettings::new(mixer.gain(), (pan / 100.0) as f32, mixer.output())
    {
        actions.push(Action::SetMixer {
            track: index,
            mixer: next.with_muted(mixer.muted()).with_soloed(mixer.soloed()),
        });
    }
    route_combo(ui, project, index, ui.available_width().max(48.0), actions);
}

fn header_value(ui: &mut Ui, width: f32, value: egui::DragValue<'_>) -> (Response, Rect) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    let response = ui.place(rect.with_max_y(rect.top() + 18.0), value);
    let rail = Rect::from_min_max(
        pos2(rect.left() + 2.0, rect.bottom() - 3.0),
        pos2(rect.right() - 2.0, rect.bottom()),
    );
    (response, rail)
}

fn value_bar(ui: &Ui, rail: Rect, origin: f32, value: f32, reference: f32, color: Color32) {
    let x = |fraction: f32| egui::lerp(rail.x_range(), fraction.clamp(0.0, 1.0));
    let (start, end) = (x(origin), x(value));
    let painter = ui.painter();
    painter.rect_filled(rail, 1.0, theme::BG_DEEP);
    if (end - start).abs() > 0.1 {
        painter.rect_filled(
            Rect::from_x_y_ranges(start.min(end)..=start.max(end), rail.y_range()),
            0.0,
            color,
        );
    }
    painter.vline(x(reference), rail.y_range(), Stroke::new(1.0, theme::TEXT));
}

fn route_choices(project: &Project, index: usize) -> Vec<(AudioOutput, String)> {
    if index == 0 {
        return vec![
            (AudioOutput::Output, "Hardware Out".to_owned()),
            (AudioOutput::Disconnected, "None".to_owned()),
        ];
    }
    let labels = rows::track_labels(project);
    let mut candidates = vec![(AudioOutput::Master, labels[0].clone())];
    for (candidate, id) in project.track_ids().enumerate() {
        let output = match id {
            TrackId::Group(group) => AudioOutput::Group(group),
            TrackId::Bus(bus) => AudioOutput::Bus(bus),
            _ => continue,
        };
        if candidate != index {
            candidates.push((output, labels[candidate].clone()));
        }
    }
    let mixer = *project.tracks()[index].mixer();
    let mut choices: Vec<_> = candidates
        .into_iter()
        .filter(|(output, _)| {
            edit::set_mixer(&mut project.clone(), index, mixer.with_output(*output)).is_ok()
        })
        .collect();
    choices.push((AudioOutput::Disconnected, "None".to_owned()));
    choices
}

fn output_label(project: &Project, output: AudioOutput) -> String {
    match output {
        AudioOutput::Master => rows::track_label(project, 0),
        AudioOutput::Group(group) => routed_track_label(project, TrackId::Group(group)),
        AudioOutput::Bus(bus) => routed_track_label(project, TrackId::Bus(bus)),
        AudioOutput::Output => "Hardware Out".to_owned(),
        AudioOutput::Disconnected => "None".to_owned(),
    }
}

fn routed_track_label(project: &Project, destination: TrackId) -> String {
    project
        .track_ids()
        .position(|id| id == destination)
        .map_or_else(
            || "Missing track".to_owned(),
            |index| rows::track_label(project, index),
        )
}

pub fn route_combo(
    ui: &mut Ui,
    project: &Project,
    index: usize,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let mixer = *project.tracks()[index].mixer();
    let current = output_label(project, mixer.output());
    egui::ComboBox::from_id_salt(("route", index))
        .width(width)
        .selected_text(RichText::new(current).color(
            if mixer.output() == AudioOutput::Disconnected {
                theme::WARN
            } else {
                theme::TEXT
            },
        ))
        .truncate()
        .show_ui(ui, |ui| {
            for (output, label) in route_choices(project, index) {
                if ui
                    .selectable_label(output == mixer.output(), label.as_str())
                    .clicked()
                    && output != mixer.output()
                {
                    actions.push(Action::SetMixer {
                        track: index,
                        mixer: mixer.with_output(output),
                    });
                }
            }
        })
        .response
        .on_hover_text("Output: where this track's audio goes");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use sympoie::plugin::{
        PluginDescriptor, PluginInstance, PluginKind, SidechainRoute, SidechainTap,
    };

    #[test]
    fn route_menu_includes_buses_and_excludes_audio_and_sidechain_cycles() {
        let mut project = Project::new();
        let audio = project.add_audio_track("Audio");
        let group = project.add_group_track("Group");
        let first = project.add_bus_track("First bus");
        let second = project.add_bus_track("Second bus");
        let mixer = *project.tracks()[3].mixer();
        project
            .set_bus_track_mixer(first, mixer.with_output(AudioOutput::Bus(second)))
            .unwrap();

        let outputs = |project: &Project, index| {
            route_choices(project, index)
                .into_iter()
                .map(|(output, _)| output)
                .collect::<Vec<_>>()
        };
        assert!(outputs(&project, 1).contains(&AudioOutput::Group(group)));
        assert!(outputs(&project, 1).contains(&AudioOutput::Bus(first)));
        assert!(outputs(&project, 2).contains(&AudioOutput::Bus(second)));
        assert!(!outputs(&project, 3).contains(&AudioOutput::Group(group)));
        assert!(!outputs(&project, 3).contains(&AudioOutput::Bus(first)));
        assert!(!outputs(&project, 4).contains(&AudioOutput::Bus(first)));

        let mut effect = PluginInstance::new(
            PluginDescriptor {
                path: PathBuf::from("/missing-route-test.vst3"),
                class_id: "0123456789ABCDEF".to_owned(),
                name: "Sidechain compressor".to_owned(),
                vendor: "Tests".to_owned(),
                kind: PluginKind::Effect,
            },
            vec![],
            vec![],
        );
        effect.sidechain = Some(SidechainRoute {
            source_track: 4,
            bus_index: 1,
            gain: 1.0,
            tap: SidechainTap::PostFader,
        });
        project.add_mixer_effect(audio.into(), effect).unwrap();
        assert!(!outputs(&project, 1).contains(&AudioOutput::Bus(first)));
        assert!(!outputs(&project, 1).contains(&AudioOutput::Bus(second)));
        assert!(outputs(&project, 1).contains(&AudioOutput::Master));
    }
}
