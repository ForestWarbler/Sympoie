use eframe::egui::{
    self, Align, Align2, Color32, Id, Layout, Painter, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use sympoie::project::Project;
use sympoie::track::{BusTrackId, TrackId};

use crate::actions::Action;
use crate::document::rows;
use crate::ui::{analog, format, theme, widgets};

const ROW_HEIGHT: f32 = 21.0;
const SEND_HINT: &str = "Send a copy to a bus, after channel volume, pan and mute. The channel's Output stays unchanged.";

fn destinations(project: &Project, source: usize) -> Vec<(usize, BusTrackId)> {
    let Some(source_id) = project.track_ids().nth(source) else {
        return Vec::new();
    };
    if matches!(source_id, TrackId::Master(_)) {
        return Vec::new();
    }
    let sends = project.tracks()[source].sends();
    project
        .track_ids()
        .enumerate()
        .filter_map(|(index, id)| {
            let TrackId::Bus(bus) = id else {
                return None;
            };
            if sends.iter().any(|send| send.destination == bus) {
                return None;
            }
            project
                .clone()
                .set_track_send(source_id, bus, 0.0)
                .ok()
                .map(|()| (index, bus))
        })
        .collect()
}

pub(super) fn show(
    ui: &mut Ui,
    project: &Project,
    source: usize,
    width: f32,
    height: f32,
    ink: Color32,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(super::caption("SENDS", 8.5, 0.8, ink))
            .on_hover_text(SEND_HINT);
        if source != 0 {
            let add = ui.small_button("+").on_hover_text("Add a send to a bus");
            ui.interact(
                add.rect,
                Id::new(("mixer-send-add", source)),
                Sense::hover(),
            );
            egui::Popup::menu(&add)
                .style(widgets::menu_style())
                .id(Id::new(("mixer-send-menu", source)))
                .show(|ui| {
                    let choices = destinations(project, source);
                    if choices.is_empty() {
                        ui.label(RichText::new("No available buses").color(theme::TEXT_DIM));
                        ui.label(
                            RichText::new("Add a bus using + Bus above.")
                                .small()
                                .color(theme::TEXT_FAINT),
                        );
                    }
                    for (bus_index, _) in choices {
                        let choice = ui.button(rows::track_label(project, bus_index));
                        ui.interact(
                            choice.rect,
                            Id::new(("mixer-send-target", source, bus_index)),
                            Sense::hover(),
                        );
                        if choice.clicked() {
                            actions.push(Action::SetTrackSend {
                                track: source,
                                bus: bus_index,
                                gain: 0.0,
                            });
                            ui.close();
                        }
                    }
                });
        } else {
            ui.add_space(0.0);
            ui.set_min_height(ui.spacing().interact_size.y.min(18.0));
        }
    });
    let (area, mut child) =
        super::list_container(ui, width, height, Id::new(("mixer-sends", source)));
    egui::ScrollArea::vertical()
        .id_salt(("mixer-send-scroll", source))
        .auto_shrink([false, false])
        .max_height(area.height())
        .min_scrolled_height(area.height())
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let sends = project.tracks()[source].sends();
            if sends.is_empty() {
                let (empty, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
                widgets::fitted_text(
                    ui.painter(),
                    empty.shrink2(vec2(3.0, 0.0)),
                    "—",
                    theme::prop(10.0),
                    theme::TEXT_FAINT,
                    Align2::LEFT_CENTER,
                );
                response.on_hover_text(SEND_HINT);
            }
            for send in sends {
                let Some(bus) = project
                    .track_ids()
                    .position(|id| id == TrackId::Bus(send.destination))
                else {
                    continue;
                };
                send_row(ui, project, source, bus, send.gain, actions);
            }
        });
}

fn send_row(
    ui: &mut Ui,
    project: &Project,
    source: usize,
    bus: usize,
    gain: f32,
    actions: &mut Vec<Action>,
) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let remove_rect = Rect::from_min_max(pos2(row.right() - 12.0, row.top()), row.max);
    let level_rect = Rect::from_min_max(
        pos2(remove_rect.left() - 58.0, row.top()),
        pos2(remove_rect.left() - 2.0, row.bottom()),
    );
    let name_rect = Rect::from_min_max(row.min, pos2(level_rect.left() - 2.0, row.bottom()));
    let name = rows::track_title(&project.tracks()[bus]);
    slot_plate(ui.painter(), row);
    widgets::fitted_text(
        ui.painter(),
        name_rect.with_min_x(name_rect.left() + 5.0),
        &name,
        theme::prop(9.5),
        theme::TEXT,
        Align2::LEFT_CENTER,
    );
    ui.interact(
        name_rect,
        Id::new(("mixer-send-name", source, bus)),
        Sense::hover(),
    )
    .on_hover_text(format!("{}\n{SEND_HINT}", rows::track_label(project, bus)));
    widgets::inside(ui, level_rect, Layout::left_to_right(Align::Center), |ui| {
        ui.push_id(("mixer-send-value", source, bus), |ui| {
            super::lcd_value_style(ui, 9.5);
            ui.spacing_mut().button_padding.x = 3.0;
            ui.spacing_mut().interact_size.x = 48.0;
            let field = ui.painter().add(Shape::Noop);
            let mut db = format::gain_to_db(gain).max(-70.0);
            let response = ui.add_sized(
                level_rect.size(),
                egui::DragValue::new(&mut db)
                    .speed(0.2)
                    .range(-70.0..=6.0)
                    .custom_formatter(|value, _| {
                        if value <= -69.95 {
                            "−∞ dB".to_owned()
                        } else {
                            format!("{value:.1} dB")
                        }
                    })
                    .custom_parser(|text| {
                        let text = text.trim().trim_end_matches("dB").trim().replace('−', "-");
                        if text == "-∞" || text.eq_ignore_ascii_case("-inf") {
                            Some(-70.0)
                        } else {
                            text.parse().ok()
                        }
                    }),
            );
            ui.painter().set(
                field,
                analog::readout(
                    level_rect.shrink2(vec2(0.0, 2.5)),
                    response.hovered() || response.dragged(),
                ),
            );
            ui.interact(
                response.rect,
                Id::new(("mixer-send-level", source, bus)),
                Sense::hover(),
            );
            if response.changed() {
                actions.push(Action::SetTrackSend {
                    track: source,
                    bus,
                    gain: if db <= -69.95 {
                        0.0
                    } else {
                        format::db_to_gain(db)
                    },
                });
            }
            response.on_hover_text(format!(
                "Send to {name}: {}\nDrag to adjust; double-click to type.\n{SEND_HINT}",
                format::db(gain)
            ));
        });
    });
    let remove = ui.interact(
        remove_rect,
        Id::new(("mixer-send-remove", source, bus)),
        Sense::click(),
    );
    if remove.hovered() {
        ui.painter().rect_filled(
            remove_rect.shrink2(vec2(0.5, 3.0)),
            2.5,
            Color32::from_white_alpha(20),
        );
    }
    let icon = remove_rect.shrink2(vec2(3.0, 7.0));
    let stroke = Stroke::new(
        1.0,
        if remove.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_FAINT
        },
    );
    ui.painter()
        .line_segment([icon.left_top(), icon.right_bottom()], stroke);
    ui.painter()
        .line_segment([icon.left_bottom(), icon.right_top()], stroke);
    if remove
        .on_hover_text(format!("Remove send to {name}"))
        .clicked()
    {
        actions.push(Action::RemoveTrackSend { track: source, bus });
    }
}

fn slot_plate(painter: &Painter, rect: Rect) {
    let rect = rect.shrink2(vec2(0.0, 0.5));
    painter.rect_filled(rect, super::SLOT_RADIUS, Color32::from_gray(0x37));
    painter.hline(
        (rect.left() + 3.0)..=(rect.right() - 3.0),
        rect.top() + 1.5,
        Stroke::new(1.0, Color32::from_white_alpha(14)),
    );
    painter.rect_stroke(
        rect,
        super::SLOT_RADIUS,
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
}
