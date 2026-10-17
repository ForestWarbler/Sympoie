use super::*;

const MIN_THUMB_WIDTH: f32 = 28.0;
const GRIP_WIDTH: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragMode {
    Pan { grab: f32 },
    Start,
    End,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OverviewDrag {
    mode: DragMode,
    origin: f32,
    scroll: f64,
    visible: f64,
    total: f64,
    rect: Rect,
    lanes_width: f32,
}

pub(super) fn overview_end_tick(project: &Project, state: &ArrangementState, playhead: f64) -> f64 {
    padded_end(project, state, required_end(project, state, playhead))
}

fn required_end(project: &Project, state: &ArrangementState, playhead: f64) -> f64 {
    content_end_tick(project)
        .max(state.playback_start as f64)
        .max(playhead)
        .max(state.scroll_tick + state.visible_ticks())
}

fn padded_end(project: &Project, state: &ArrangementState, end: f64) -> f64 {
    let bar = project.time_signature().ticks_per_bar() as f64;
    let padding = (bar * 8.0).max(state.visible_ticks() * 0.5);
    ((end + padding) / bar).ceil() * bar
}

pub(super) fn overview_thumb(rect: Rect, scroll: f64, visible: f64, total: f64) -> Rect {
    let width = rect.width().max(0.0);
    let thumb_width =
        ((visible / total * f64::from(width)) as f32).clamp(MIN_THUMB_WIDTH.min(width), width);
    let max_scroll = (total - visible).max(0.0);
    let fraction = if max_scroll > 0.0 {
        (scroll / max_scroll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let left = rect.left() + (width - thumb_width) * fraction as f32;
    Rect::from_min_max(
        pos2(left, rect.top() + 1.0),
        pos2((left + thumb_width).min(rect.right()), rect.bottom() - 1.0),
    )
}

fn drag_mode(thumb: Rect, pointer: Pos2) -> DragMode {
    if thumb.contains(pointer) {
        let grip = GRIP_WIDTH.min(thumb.width() / 4.0);
        if pointer.x < thumb.left() + grip {
            DragMode::Start
        } else if pointer.x > thumb.right() - grip {
            DragMode::End
        } else {
            DragMode::Pan {
                grab: pointer.x - thumb.left(),
            }
        }
    } else {
        DragMode::Pan {
            grab: thumb.width() / 2.0,
        }
    }
}

fn pan_position(rect: Rect, thumb_width: f32, pointer: f32, grab: f32, max_scroll: f64) -> f64 {
    let travel = rect.width() - thumb_width;
    if travel <= 0.0 {
        return 0.0;
    }
    f64::from(((pointer - rect.left() - grab) / travel).clamp(0.0, 1.0)) * max_scroll
}

impl OverviewDrag {
    fn apply(self, state: &mut ArrangementState, pointer: f32) {
        match self.mode {
            DragMode::Pan { grab } => {
                let thumb = overview_thumb(self.rect, self.scroll, self.visible, self.total);
                state.scroll_tick = pan_position(
                    self.rect,
                    thumb.width(),
                    pointer,
                    grab,
                    (self.total - self.visible).max(0.0),
                );
            }
            DragMode::Start | DragMode::End => {
                let delta = f64::from(pointer - self.origin)
                    / f64::from(self.rect.width().max(1.0))
                    * self.total;
                let width = f64::from(state.lanes_width);
                let min_span = width / state::MAX_ZOOM;
                let max_span = width / state::MIN_ZOOM;
                let right = self.scroll + self.visible;
                let span = match self.mode {
                    DragMode::Start => (self.visible - delta).clamp(min_span, max_span.min(right)),
                    _ => (self.visible + delta)
                        .clamp(min_span, max_span.min(self.total - self.scroll)),
                };
                state.pixels_per_tick = (width / span).clamp(state::MIN_ZOOM, state::MAX_ZOOM);
                state.scroll_tick = if self.mode == DragMode::Start {
                    (right - state.visible_ticks()).max(0.0)
                } else {
                    self.scroll
                };
            }
        }
        state.follow = false;
    }
}

pub(super) fn overview(
    ui: &mut Ui,
    rect: Rect,
    state: &mut ArrangementState,
    inputs: &Inputs<'_>,
    playhead: f64,
    interaction: &mut Interaction,
) {
    let project = inputs.project;
    if interaction
        .overview_drag
        .is_some_and(|drag| drag.rect != rect || drag.lanes_width != state.lanes_width)
    {
        interaction.overview_drag = None;
    }
    if interaction.overview_drag.is_none() {
        let required = required_end(project, state, playhead).max(if inputs.loop_region.enabled {
            inputs.loop_region.end as f64
        } else {
            0.0
        });
        if required > state.overview_end_tick + state.overview_end_tick.abs() * 1e-12 {
            state.overview_end_tick = overview_end_tick(project, state, playhead)
                .max(padded_end(project, state, required));
        }
    }
    let mut total = interaction
        .overview_drag
        .map_or(state.overview_end_tick, |drag| drag.total)
        .max(state.visible_ticks());
    let thumb = overview_thumb(rect, state.scroll_tick, state.visible_ticks(), total);
    let response = ui
        .interact(rect, Id::new("arrangement-overview"), Sense::click_and_drag())
        .on_hover_text(
            "Drag the frame to pan · Drag either edge to zoom\nDouble-click to fit the project\nScroll horizontally to explore further · Ctrl/Cmd+wheel or pinch to zoom",
        );

    if response.is_pointer_button_down_on()
        && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary))
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        interaction.overview_drag = Some(OverviewDrag {
            mode: drag_mode(thumb, origin),
            origin: origin.x,
            scroll: state.scroll_tick,
            visible: state.visible_ticks(),
            total,
            rect,
            lanes_width: state.lanes_width,
        });
        state.follow = false;
    }
    if (response.dragged_by(egui::PointerButton::Primary) || response.drag_stopped())
        && let (Some(drag), Some(pointer)) =
            (interaction.overview_drag, response.interact_pointer_pos())
    {
        drag.apply(state, pointer.x);
    }
    if response.double_clicked() {
        let end =
            content_end_tick(project).max(project.time_signature().ticks_per_bar() as f64 * 8.0);
        state.fit(0, end.ceil() as u64);
        state.follow = false;
        interaction.overview_drag = None;
        total = overview_end_tick(project, state, playhead).max(padded_end(
            project,
            state,
            if inputs.loop_region.enabled {
                inputs.loop_region.end as f64
            } else {
                0.0
            },
        ));
        state.overview_end_tick = total;
    } else if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
        && !thumb.contains(pointer)
    {
        state.scroll_tick = pan_position(
            rect,
            thumb.width(),
            pointer.x,
            thumb.width() / 2.0,
            (total - state.visible_ticks()).max(0.0),
        );
        state.follow = false;
    }

    let thumb = overview_thumb(rect, state.scroll_tick, state.visible_ticks(), total);
    let mode = interaction.overview_drag.map(|drag| drag.mode).or_else(|| {
        response
            .hover_pos()
            .map(|pointer| drag_mode(thumb, pointer))
    });
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(match mode {
            Some(DragMode::Start | DragMode::End) => CursorIcon::ResizeHorizontal,
            _ if response.dragged() => CursorIcon::Grabbing,
            _ => CursorIcon::Grab,
        });
    }
    if !ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary)) {
        interaction.overview_drag = None;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BG_DEEP);
    let x = |tick: f64| rect.left() + (tick / total * f64::from(rect.width())) as f32;

    let tracks = project.tracks().len().saturating_sub(1).max(1);
    let band = ((rect.height() - 4.0) / tracks as f32).clamp(1.0, 4.0);
    for (index, track) in project.tracks().iter().enumerate().skip(1) {
        let y = rect.top() + 2.0 + (index - 1) as f32 * band;
        if y + band > rect.bottom() - 1.0 {
            break;
        }
        let paint_span = |start: f64, end: f64, color| {
            painter.rect_filled(
                Rect::from_x_y_ranges(x(start)..=x(end).max(x(start) + 1.0), y..=y + band),
                0.0,
                color,
            );
        };
        let color = theme::track_color(project, index).gamma_multiply(0.75);
        match track {
            Track::Audio(audio) => {
                for clip in audio.clips() {
                    paint_span(clip.start_tick as f64, audio_end_tick(project, clip), color);
                }
            }
            Track::Midi(midi) => {
                for clip in midi.clips() {
                    paint_span(
                        clip.start_tick as f64,
                        (clip.start_tick + clip.length_ticks) as f64,
                        color,
                    );
                }
            }
            _ => {}
        }
        if inputs.recording
            && inputs.armed_tracks.contains(&index)
            && matches!(track, Track::Audio(_) | Track::Midi(_))
            && let Some(start) = inputs.recording_start_tick
        {
            paint_span(start as f64, playhead, theme::ERROR);
        }
    }

    painter.rect(
        thumb,
        2.0,
        Color32::from_white_alpha(if response.hovered() || response.dragged() {
            26
        } else {
            14
        }),
        Stroke::new(1.0, theme::TEXT_DIM),
        StrokeKind::Inside,
    );
    let grip = GRIP_WIDTH.min(thumb.width() / 4.0);
    for edge in [thumb.left() + grip / 2.0, thumb.right() - grip / 2.0] {
        painter.vline(
            edge,
            thumb.top() + 3.0..=thumb.bottom() - 3.0,
            Stroke::new(1.5, theme::TEXT_DIM),
        );
    }
    painter.vline(
        x(state.playback_start as f64),
        rect.y_range(),
        Stroke::new(1.0, theme::ACCENT),
    );
    if inputs.status.state == PlaybackState::Playing {
        painter.vline(
            x(playhead),
            rect.y_range(),
            Stroke::new(1.0, theme::PLAYHEAD),
        );
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
}
