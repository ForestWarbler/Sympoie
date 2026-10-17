use eframe::egui::{self, Align2, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2};
use sympoie::track::{AudioInputChannels, AudioRecordingInput, MidiRecordingInput, Track};

use super::Inputs;
use crate::actions::Action;
use crate::audio::record_input::{AudioInputDevice, InputCatalog};
use crate::ui::{theme, widgets};

pub(super) fn selector(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    track: &Track,
    inputs: &Inputs<'_>,
    actions: &mut Vec<Action>,
) {
    let (short, description, unavailable) = match track {
        Track::Audio(track) => audio_label(track.recording_input(), inputs.input_catalog),
        Track::Midi(track) => midi_label(track.recording_input(), inputs.input_catalog),
        _ => return,
    };
    let enabled = !inputs.recording;
    let response = ui.interact(
        rect,
        Id::new(("track-record-input", index)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ComboBox,
            enabled,
            format!("{} recording input: {description}", track.name()),
        )
    });
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        2.0,
        if enabled && response.hovered() {
            theme::HEADER_SELECTED
        } else {
            theme::BG_PANEL
        },
    );
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(0.5, theme::BORDER),
        StrokeKind::Inside,
    );
    let color = if !enabled {
        theme::TEXT_FAINT
    } else if unavailable {
        theme::WARN
    } else {
        theme::TEXT
    };
    widgets::fitted_text(
        &painter,
        rect.shrink(2.0).with_max_x(rect.right() - 8.0),
        &short,
        theme::prop(10.0),
        color,
        Align2::CENTER_CENTER,
    );
    let center = pos2(rect.right() - 5.0, rect.center().y);
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(center.x - 2.5, center.y - 1.0),
            pos2(center.x + 2.5, center.y - 1.0),
            pos2(center.x, center.y + 2.0),
        ],
        color,
        Stroke::NONE,
    ));
    if enabled {
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(220.0);
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    match track {
                        Track::Audio(track) => audio_menu(
                            ui,
                            index,
                            track.recording_input(),
                            inputs.input_catalog,
                            actions,
                        ),
                        Track::Midi(track) => midi_menu(
                            ui,
                            index,
                            track.recording_input(),
                            inputs.input_catalog,
                            actions,
                        ),
                        _ => {}
                    }
                    ui.separator();
                    if ui.button("Refresh inputs").clicked() {
                        actions.push(Action::RefreshRecordingInputs);
                        ui.close();
                    }
                    if let Some(error) = &inputs.input_catalog.error {
                        ui.label(egui::RichText::new(error).color(theme::WARN));
                    }
                });
        });
    } else {
        egui::Popup::close_id(ui.ctx(), egui::Popup::default_response_id(&response));
    }
    response.on_hover_text(format!(
        "Recording input\n{description}\n{}",
        if enabled {
            "Click to choose a device and input channel."
        } else {
            "Stop recording to change the input."
        },
    ));
}

fn selected_audio_device<'a>(
    input: &AudioRecordingInput,
    catalog: &'a InputCatalog,
) -> Option<&'a AudioInputDevice> {
    catalog.audio.iter().find(|device| match &input.device {
        Some(id) => &device.id == id,
        None => device.is_default,
    })
}

fn audio_channels_label(channels: AudioInputChannels) -> String {
    match channels {
        AudioInputChannels::Mono(channel) => format!("Mono {}", u32::from(channel) + 1),
        AudioInputChannels::Stereo(left, right) => {
            format!("Stereo {}/{}", u32::from(left) + 1, u32::from(right) + 1)
        }
    }
}

fn audio_label(input: &AudioRecordingInput, catalog: &InputCatalog) -> (String, String, bool) {
    let device = selected_audio_device(input, catalog);
    let short = match input.channels {
        AudioInputChannels::Mono(channel) => (u32::from(channel) + 1).to_string(),
        AudioInputChannels::Stereo(left, right) => {
            format!("{}/{}", u32::from(left) + 1, u32::from(right) + 1)
        }
    };
    let name = device.map_or_else(
        || {
            if input.device.is_some() {
                "Selected device unavailable".to_owned()
            } else {
                "System default input".to_owned()
            }
        },
        |device| {
            if input.device.is_none() {
                format!("System default: {}", device.name)
            } else {
                device.name.clone()
            }
        },
    );
    let channel_available = device.is_none_or(|device| match input.channels {
        AudioInputChannels::Mono(channel) => channel < device.channels,
        AudioInputChannels::Stereo(left, right) => {
            left < device.channels && right < device.channels
        }
    });
    let description = format!(
        "{name} · {}{}{}",
        audio_channels_label(input.channels),
        if matches!(input.channels, AudioInputChannels::Mono(_)) {
            " · same signal in left and right"
        } else {
            ""
        },
        if channel_available {
            ""
        } else {
            " · channel unavailable"
        },
    );
    (
        short,
        description,
        (device.is_none() && input.device.is_some()) || !channel_available,
    )
}

fn midi_label(input: &MidiRecordingInput, catalog: &InputCatalog) -> (String, String, bool) {
    let device = input
        .device
        .as_ref()
        .and_then(|id| catalog.midi.iter().find(|device| &device.id == id));
    let missing = input.device.is_some() && device.is_none();
    let name = device.map_or(
        if missing {
            "Selected port unavailable"
        } else {
            "All MIDI inputs"
        },
        |device| device.name.as_str(),
    );
    let short = input
        .channel
        .map_or_else(|| "All".to_owned(), |channel| (channel + 1).to_string());
    let channel = input.channel.map_or_else(
        || "All channels".to_owned(),
        |channel| format!("Channel {}", channel + 1),
    );
    (short, format!("{name} · {channel}"), missing)
}

fn audio_menu(
    ui: &mut Ui,
    index: usize,
    input: &AudioRecordingInput,
    catalog: &InputCatalog,
    actions: &mut Vec<Action>,
) {
    ui.label("Audio input");
    if input.device.is_some() && selected_audio_device(input, catalog).is_none() {
        ui.colored_label(theme::WARN, "Selected device unavailable");
    }
    let default = catalog.audio.iter().find(|device| device.is_default);
    let label = default.map_or_else(
        || "System default input".to_owned(),
        |device| format!("System default: {}", device.name),
    );
    if ui.selectable_label(input.device.is_none(), label).clicked() {
        select_audio_device(ui, index, input, None, default, actions);
    }
    for device in &catalog.audio {
        if ui
            .selectable_label(input.device.as_ref() == Some(&device.id), &device.name)
            .clicked()
        {
            select_audio_device(
                ui,
                index,
                input,
                Some(device.id.clone()),
                Some(device),
                actions,
            );
        }
    }
    ui.separator();
    ui.label("Input channels");
    if let Some(device) = selected_audio_device(input, catalog) {
        for channel in 0..device.channels {
            select_audio_channels(ui, index, input, AudioInputChannels::Mono(channel), actions);
        }
        for left in (0..device.channels.saturating_sub(1)).step_by(2) {
            select_audio_channels(
                ui,
                index,
                input,
                AudioInputChannels::Stereo(left, left + 1),
                actions,
            );
        }
    } else {
        ui.label(format!("{} (saved)", audio_channels_label(input.channels)));
        ui.label("Connect an audio input and refresh inputs.");
    }
}

fn select_audio_device(
    ui: &mut Ui,
    index: usize,
    input: &AudioRecordingInput,
    id: Option<String>,
    device: Option<&AudioInputDevice>,
    actions: &mut Vec<Action>,
) {
    let mut next = input.clone();
    next.device = id;
    if let Some(device) = device {
        let fits = match next.channels {
            AudioInputChannels::Mono(channel) => channel < device.channels,
            AudioInputChannels::Stereo(left, right) => {
                left < device.channels && right < device.channels
            }
        };
        if !fits {
            next.channels = AudioInputChannels::Mono(0);
        }
    }
    if &next != input {
        actions.push(Action::SetAudioRecordingInput {
            track: index,
            input: next,
        });
    }
    ui.close();
}

fn select_audio_channels(
    ui: &mut Ui,
    index: usize,
    input: &AudioRecordingInput,
    channels: AudioInputChannels,
    actions: &mut Vec<Action>,
) {
    let response = ui.selectable_label(input.channels == channels, audio_channels_label(channels));
    if response.clicked() {
        if input.channels != channels {
            actions.push(Action::SetAudioRecordingInput {
                track: index,
                input: AudioRecordingInput {
                    channels,
                    ..input.clone()
                },
            });
        }
        ui.close();
    }
    if matches!(channels, AudioInputChannels::Mono(_)) {
        response.on_hover_text("Records this input equally into the left and right channels.");
    }
}

fn midi_menu(
    ui: &mut Ui,
    index: usize,
    input: &MidiRecordingInput,
    catalog: &InputCatalog,
    actions: &mut Vec<Action>,
) {
    ui.label("MIDI input");
    if input
        .device
        .as_ref()
        .is_some_and(|id| !catalog.midi.iter().any(|device| &device.id == id))
    {
        ui.colored_label(theme::WARN, "Selected port unavailable");
    }
    if ui
        .selectable_label(input.device.is_none(), "All MIDI inputs")
        .clicked()
    {
        if input.device.is_some() {
            actions.push(Action::SetMidiRecordingInput {
                track: index,
                input: MidiRecordingInput {
                    device: None,
                    ..input.clone()
                },
            });
        }
        ui.close();
    }
    for device in &catalog.midi {
        if ui
            .selectable_label(input.device.as_ref() == Some(&device.id), &device.name)
            .clicked()
        {
            if input.device.as_ref() != Some(&device.id) {
                actions.push(Action::SetMidiRecordingInput {
                    track: index,
                    input: MidiRecordingInput {
                        device: Some(device.id.clone()),
                        ..input.clone()
                    },
                });
            }
            ui.close();
        }
    }
    ui.separator();
    ui.label("MIDI channel");
    for channel in std::iter::once(None).chain((0..16).map(Some)) {
        let label = channel.map_or_else(
            || "All channels".to_owned(),
            |channel| format!("Channel {}", channel + 1),
        );
        if ui
            .selectable_label(input.channel == channel, label)
            .clicked()
        {
            if input.channel != channel {
                actions.push(Action::SetMidiRecordingInput {
                    track: index,
                    input: MidiRecordingInput {
                        channel,
                        ..input.clone()
                    },
                });
            }
            ui.close();
        }
    }
}
