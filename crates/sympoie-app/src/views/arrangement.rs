use std::collections::BTreeSet;

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Id, Key, KeyboardShortcut, Layout, Mesh, Modifiers,
    Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2,
    vec2,
};
use sympoie::TICKS_PER_QUARTER;
use sympoie::clip::{AudioClip, MidiClip};
use sympoie::pitch::PitchAlgorithm;
use sympoie::project::Project;
use sympoie::time::{SnapGrid, TimeSignature};
use sympoie::track::Track;
use sympoie::transport::PlaybackState;

use crate::actions::Action;
use crate::audio::EngineStatus;
use crate::audio::record_input::InputCatalog;
use crate::document::edit::{self, ClipRef, Grid, TrackKind};
use crate::document::rows::{self, Row};
use crate::state::{
    self, ArrangementState, GridDensity, GridMode, LoopRegion, Selection, TimeRange,
};
use crate::ui::analog;
use crate::ui::format;
use crate::ui::theme;
use crate::ui::waveform::WaveformCache;
use crate::ui::widgets;
use crate::views::device_drag::DeviceDrag;
use crate::views::track_controls::{mixer_controls, track_title};

#[path = "automation.rs"]
mod automation;

#[path = "recording_inputs.rs"]
mod recording_inputs;

#[path = "arrangement_navigation.rs"]
mod navigation;
use navigation::{OverviewDrag, overview};
#[cfg(test)]
use navigation::{overview_end_tick, overview_thumb};

const HEADER_WIDTH: f32 = 252.0;
const OVERVIEW_HEIGHT: f32 = 18.0;
const NUMBERS_HEIGHT: f32 = 20.0;
const LOOP_BAND: f32 = 14.0;
const TOP_HEIGHT: f32 = OVERVIEW_HEIGHT + NUMBERS_HEIGHT + LOOP_BAND;
const MASTER_HEIGHT: f32 = 60.0;
const EDGE_GRAB: f32 = 6.0;
const CLIP_HEADER: f32 = 13.0;
const TITLE_HEIGHT: f32 = 18.0;

pub struct Inputs<'a> {
    pub project: &'a Project,
    pub status: &'a EngineStatus,
    pub selection: &'a Selection,
    pub loop_region: LoopRegion,
    pub armed_tracks: &'a BTreeSet<usize>,
    pub input_catalog: &'a InputCatalog,
    pub recording: bool,
    pub recording_start_tick: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropTarget {
    pub track: Option<usize>,
    pub start: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ClipMode {
    Move,
    TrimStart,
    TrimEnd,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipDrag {
    clip: ClipRef,
    mode: ClipMode,
    copy: bool,
    grab_offset: f64,
    start: u64,
    end: u64,
}

impl ClipDrag {
    fn snap_on(self, modifiers: Modifiers) -> bool {
        !modifiers.alt && (self.mode == ClipMode::Move || !modifiers.command)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LoopDrag {
    Create { anchor: u64 },
    Start,
    End,
    Move { grab_offset: f64, length: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Ghost {
    track: usize,
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Copy)]
struct Timeline {
    left: f32,
    right: f32,
    scroll_tick: f64,
    pixels_per_tick: f64,
    grid: Grid,
}

impl Timeline {
    fn x(&self, tick: f64) -> f32 {
        self.left + ((tick - self.scroll_tick) * self.pixels_per_tick) as f32
    }

    fn tick(&self, x: f32) -> f64 {
        (self.scroll_tick + f64::from(x - self.left) / self.pixels_per_tick).max(0.0)
    }

    fn visible(&self) -> (f64, f64) {
        (self.tick(self.left), self.tick(self.right))
    }

    fn snapped(&self, x: f32, snap_on: bool) -> u64 {
        edit::snap(self.grid, self.tick(x).round() as u64, snap_on)
    }

    fn visible_grid_interval(&self, meter: TimeSignature) -> u64 {
        let bar = meter.ticks_per_bar();
        let mut step = self.grid.interval().unwrap_or_else(|| {
            state::adaptive_interval(
                meter,
                self.pixels_per_tick,
                GridDensity::Medium.min_pixels(),
            )
        });
        while step as f64 * self.pixels_per_tick < 4.0 {
            step = if step < bar {
                let next = step * 2;
                if next < bar && bar.is_multiple_of(next) {
                    next
                } else {
                    bar
                }
            } else {
                step * 2
            };
        }
        step
    }

    fn grid_line_x(&self, tick: u64) -> f32 {
        self.x(tick as f64).round() + 0.5
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RangeDrag {
    anchor: u64,
    track: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct RangeMove {
    range: TimeRange,
    grab_offset: f64,
}

impl RangeMove {
    fn target(&self, timeline: &Timeline, x: f32, snap_on: bool) -> u64 {
        let raw = (timeline.tick(x) - self.grab_offset).max(0.0).round() as u64;
        edit::snap(timeline.grid, raw, snap_on)
    }
}

#[derive(Debug, Clone, Default)]
struct Interaction {
    automation: automation::Editor,
    track_resize: Option<TrackResize>,
    track_drag: Option<usize>,
    clip_drag: Option<ClipDrag>,
    loop_drag: Option<LoopDrag>,
    loop_preview: Option<(u64, u64)>,
    overview_drag: Option<OverviewDrag>,
    released: Option<(ClipDrag, Pos2)>,
    range_drag: Option<RangeDrag>,
    range_move: Option<RangeMove>,
    range_released: Option<(RangeMove, Pos2)>,
    menu_at: Option<(usize, u64)>,
    /// The clip being renamed in place, by its stable id, and the text so far.
    clip_rename: Option<(u64, String)>,
}

#[derive(Debug, Clone, Copy)]
struct TrackResize {
    index: usize,
    start_y: f32,
    start_height: f32,
    previous_height: Option<f32>,
}

fn interaction_id() -> Id {
    Id::new("sympoie-arrangement-interaction")
}

pub fn cancel_interactions(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<Interaction>(interaction_id()));
}

pub fn resize_shortcuts(ctx: &egui::Context, state: &mut ArrangementState) {
    let Some(mut interaction) = ctx.data(|data| data.get_temp::<Interaction>(interaction_id()))
    else {
        return;
    };
    let Some(resize) = interaction.track_resize else {
        return;
    };
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        restore_track_height(state, resize);
        interaction.track_resize = None;
        ctx.data_mut(|data| data.insert_temp(interaction_id(), interaction));
        ctx.stop_dragging();
    }
}

fn ignore_grid(ui: &Ui) -> bool {
    ui.input(|i| i.modifiers.alt || i.modifiers.command)
}

fn audio_end_tick(project: &Project, clip: &AudioClip) -> f64 {
    clip.start_tick as f64 + clip.frame_count as f64 * project.bpm() / 3000.0
}

pub fn playhead_tick(
    project: &Project,
    status: &EngineStatus,
    loop_region: LoopRegion,
    playback_start: u64,
) -> f64 {
    if status.state != PlaybackState::Playing {
        return playback_start as f64;
    }
    let mut tick = status.position_frame as f64 * project.bpm() / 3000.0;
    if loop_region.enabled
        && status.state == PlaybackState::Playing
        && tick >= loop_region.end as f64
        && loop_region.end > loop_region.start
    {
        tick = loop_region.start as f64 + (tick - loop_region.end as f64);
    }
    tick
}

struct Areas {
    full: Rect,
    headers: Rect,
    overview: Rect,
    numbers: Rect,
    band: Rect,
    corner: Rect,
    rows: Rect,
    master: Rect,
}

impl Areas {
    fn new(full: Rect, automation: bool) -> Self {
        let split = (full.left() + HEADER_WIDTH).min(full.right() - 1.0);
        let top = full.top();
        let overview =
            Rect::from_min_max(pos2(split, top), pos2(full.right(), top + OVERVIEW_HEIGHT));
        let numbers = Rect::from_min_max(
            pos2(split, overview.bottom()),
            pos2(full.right(), overview.bottom() + NUMBERS_HEIGHT),
        );
        let band = Rect::from_min_max(
            pos2(split, numbers.bottom()),
            pos2(full.right(), top + TOP_HEIGHT),
        );
        let corner = Rect::from_min_max(full.min, pos2(split, top + TOP_HEIGHT));
        let master_height = if automation { 88.0 } else { MASTER_HEIGHT };
        let master = Rect::from_min_max(pos2(full.left(), full.bottom() - master_height), full.max);
        let rows = Rect::from_min_max(
            pos2(full.left(), top + TOP_HEIGHT),
            pos2(full.right(), master.top()),
        );
        Self {
            full,
            headers: Rect::from_min_max(full.min, pos2(split, full.bottom())),
            overview,
            numbers,
            band,
            corner,
            rows,
            master,
        }
    }

    fn split(&self) -> f32 {
        self.headers.right()
    }

    fn lanes(&self) -> Rect {
        Rect::from_min_max(pos2(self.split(), self.numbers.top()), self.full.max)
    }
}

pub fn show(
    ui: &mut Ui,
    state: &mut ArrangementState,
    inputs: &Inputs<'_>,
    waveforms: &mut WaveformCache,
    track_meters: &mut [widgets::MeterState],
    main_meter: &mut widgets::MeterState,
    actions: &mut Vec<Action>,
) -> Option<DropTarget> {
    let project = inputs.project;
    let full = ui.available_rect_before_wrap();
    let areas = Areas::new(full, state.automation);
    let lanes_left = areas.split();
    let lanes_right = full.right();
    ui.painter().rect_filled(full, 0.0, theme::BG_DEEP);
    ui.painter().rect_filled(
        areas.rows.with_min_x(lanes_left),
        0.0,
        Color32::from_rgb(0x1c, 0x1c, 0x1c),
    );
    ui.painter()
        .rect_filled(areas.rows.with_max_x(lanes_left), 0.0, theme::BG_PANEL);
    state.lanes_width = (lanes_right - lanes_left).max(1.0);

    let mut interaction: Interaction = ui
        .data(|d| d.get_temp(interaction_id()))
        .unwrap_or_default();
    let ruler_area = Rect::from_min_max(areas.overview.min, areas.band.max);
    if interaction.overview_drag.is_none() {
        handle_wheel(ui, state, areas.lanes().union(ruler_area), ruler_area);
    }
    let playhead = playhead_tick(
        project,
        inputs.status,
        inputs.loop_region,
        state.playback_start,
    );
    if interaction.overview_drag.is_none() {
        state.follow_playhead(playhead, inputs.status);
    }
    update_track_resize(ui, state, &mut interaction);
    if state.automation {
        interaction.automation.keyboard(ui, project, state, actions);
        interaction.clip_drag = None;
        interaction.range_drag = None;
        interaction.range_move = None;
        interaction.released = None;
        interaction.range_released = None;
    }

    let loop_shown = interaction
        .loop_preview
        .map_or(inputs.loop_region, |(start, end)| LoopRegion {
            start,
            end,
            enabled: true,
        });

    overview(
        ui,
        areas.overview,
        state,
        inputs,
        playhead,
        &mut interaction,
    );
    let timeline = Timeline {
        left: lanes_left,
        right: lanes_right,
        scroll_tick: state.scroll_tick,
        pixels_per_tick: state.pixels_per_tick,
        grid: state.grid(project),
    };
    ruler(
        ui,
        &areas,
        &timeline,
        project,
        inputs,
        &mut interaction,
        loop_shown,
        state.automation,
        actions,
    );

    let display = rows::display_rows(project, &state.folded);
    let mut row_rects: Vec<(Row, Rect)> = Vec::with_capacity(display.len());
    let mut drop_target = None;

    let mut rows_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(areas.rows)
            .layout(Layout::top_down(Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("arrangement-rows")
        .auto_shrink([false, false])
        .scroll_bar_rect(areas.rows.with_max_x(lanes_left))
        .show(&mut rows_ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let width = ui.available_width();
            for row in &display {
                let row_height = state.track_height(row.index);
                let (rect, _) = ui.allocate_exact_size(vec2(width, row_height), Sense::hover());
                row_rects.push((*row, rect));
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let lane = rect.with_min_x(lanes_left);
                let selected = inputs.selection.tracks.contains(&row.index);
                let fill = if selected {
                    theme::LANE_SELECTED
                } else {
                    theme::LANE
                };
                ui.painter().rect_filled(lane, 0.0, fill);
                ui.painter().hline(
                    lane.x_range(),
                    lane.bottom() - 0.5,
                    Stroke::new(1.0, theme::BORDER),
                );
                if !state.automation {
                    lane_background(
                        ui,
                        lane,
                        *row,
                        &display,
                        &timeline,
                        state.grid,
                        state.playback_start,
                        inputs,
                        &mut interaction,
                        actions,
                    );
                }
                track_header(
                    ui,
                    rect.with_max_x(lanes_left),
                    *row,
                    state,
                    inputs,
                    track_meters.get_mut(row.index),
                    &mut interaction,
                    actions,
                );
            }

            update_track_drag(
                ui,
                project,
                &row_rects,
                areas.rows.with_max_x(lanes_left),
                &mut interaction,
                actions,
            );

            let lanes_clip = ui.clip_rect().intersect(Rect::from_min_max(
                pos2(lanes_left, f32::NEG_INFINITY),
                pos2(lanes_right, f32::INFINITY),
            ));
            let painter = ui.painter().with_clip_rect(lanes_clip);
            let rows_extent = row_rects
                .first()
                .zip(row_rects.last())
                .map(|(first, last)| (first.1.top(), last.1.bottom()));
            if let Some((top, bottom)) = rows_extent {
                paint_grid(
                    &painter,
                    &timeline,
                    project.time_signature(),
                    top..=bottom,
                    [theme::grid_bar(), theme::grid_beat(), theme::grid_fine()],
                );
                if loop_shown.enabled {
                    let x0 = timeline.x(loop_shown.start as f64);
                    let x1 = timeline.x(loop_shown.end as f64);
                    painter.rect_filled(
                        Rect::from_x_y_ranges(x0..=x1, top..=bottom),
                        0.0,
                        Color32::from_white_alpha(5),
                    );
                }
            }
            for (row, rect) in &row_rects {
                if ui.is_rect_visible(*rect) {
                    let lane = rect.with_min_x(lanes_left);
                    clips(
                        ui,
                        &painter,
                        lane,
                        *row,
                        &timeline,
                        project,
                        inputs,
                        waveforms,
                        &mut interaction,
                        !state.automation,
                        actions,
                    );
                    if state.automation {
                        painter.rect_filled(lane, 0.0, Color32::from_black_alpha(150));
                        automation::lane(
                            ui,
                            lane,
                            row.index,
                            state,
                            project,
                            &timeline,
                            &mut interaction.automation,
                            actions,
                        );
                    }
                    if inputs.recording
                        && inputs.armed_tracks.contains(&row.index)
                        && matches!(
                            project.tracks()[row.index],
                            Track::Audio(_) | Track::Midi(_)
                        )
                        && let Some(start) = inputs.recording_start_tick
                    {
                        paint_recording(&painter, lane, &timeline, start, playhead);
                    }
                }
            }

            if let Some(range) = inputs
                .selection
                .range
                .as_ref()
                .filter(|_| !state.automation)
            {
                paint_time_selection(&painter, &timeline, range, &row_rects);
            }
            update_range_drag(ui, &timeline, inputs, &row_rects, &mut interaction, actions);
            if let Some((moving, pointer)) = interaction.range_released.take() {
                let to = moving.target(&timeline, pointer.x, !ignore_grid(ui));
                if to != moving.range.start {
                    actions.push(Action::MoveTimeSelection { to });
                }
            }
            if let Some(moving) = &interaction.range_move
                && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
            {
                let to = moving.target(&timeline, pointer.x, !ignore_grid(ui));
                let x0 = timeline.x(to as f64);
                let x1 = timeline.x((to + moving.range.len()) as f64);
                for (row, rect) in &row_rects {
                    if moving.range.tracks.contains(&row.index) {
                        painter.rect(
                            Rect::from_x_y_ranges(x0..=x1, rect.top() + 1.0..=rect.bottom() - 2.0),
                            0.0,
                            Color32::from_white_alpha(18),
                            Stroke::new(1.0, Color32::WHITE),
                            StrokeKind::Inside,
                        );
                    }
                }
                if let Some((_, rect)) = row_rects
                    .iter()
                    .find(|(row, _)| moving.range.tracks.contains(&row.index))
                {
                    painter.text(
                        pos2(x0 + 5.0, rect.top() + 3.0),
                        Align2::LEFT_TOP,
                        format::bar_beat(project.time_signature(), to),
                        theme::mono(10.5),
                        Color32::WHITE,
                    );
                }
            }

            for (row, rect) in &row_rects {
                device_drop_target(ui, *rect, project, row.index, actions);
            }

            if let Some((drag, pointer)) = interaction.released.take() {
                let snap_on = drag.snap_on(ui.input(|input| input.modifiers));
                if let Some(ghost) = ghost(project, drag, &timeline, pointer, &row_rects, snap_on) {
                    commit(drag, ghost, actions);
                }
            }

            if let Some(drag) = interaction.clip_drag
                && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
            {
                let snap_on = drag.snap_on(ui.input(|input| input.modifiers));
                if let Some(ghost) = ghost(project, drag, &timeline, pointer, &row_rects, snap_on)
                    && let Some((_, rect)) =
                        row_rects.iter().find(|(row, _)| row.index == ghost.track)
                {
                    let ghost_rect = Rect::from_x_y_ranges(
                        timeline.x(ghost.start as f64)..=timeline.x(ghost.end as f64),
                        rect.top() + 1.0..=rect.bottom() - 2.0,
                    );
                    painter.rect(
                        ghost_rect,
                        0.0,
                        Color32::from_white_alpha(18),
                        Stroke::new(1.0, Color32::WHITE),
                        StrokeKind::Inside,
                    );
                    let (anchor, edge) = match drag.mode {
                        ClipMode::TrimEnd => (Align2::RIGHT_TOP, ghost.end),
                        _ => (Align2::LEFT_TOP, ghost.start),
                    };
                    let position = match drag.mode {
                        ClipMode::TrimEnd => ghost_rect.right_top() + vec2(-5.0, 2.0),
                        _ => ghost_rect.left_top() + vec2(5.0, 2.0),
                    };
                    painter.text(
                        position,
                        anchor,
                        format::bar_beat(project.time_signature(), edge),
                        theme::mono(10.5),
                        Color32::WHITE,
                    );
                }
            }

            let hovering_files = ui.input(|i| !i.raw.hovered_files.is_empty());
            if let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
                && pointer.x > lanes_left
                && areas.rows.contains(pointer)
            {
                let start = timeline.snapped(pointer.x, true);
                let track = row_rects
                    .iter()
                    .find(|(row, rect)| {
                        rect.y_range().contains(pointer.y)
                            && matches!(project.tracks().get(row.index), Some(Track::Audio(_)))
                    })
                    .map(|(row, _)| row.index);
                drop_target = Some(DropTarget { track, start });
                if hovering_files {
                    let highlight = match track
                        .and_then(|t| row_rects.iter().find(|(row, _)| row.index == t))
                    {
                        Some((_, rect)) => rect.with_min_x(lanes_left),
                        None => Rect::from_min_max(
                            pos2(
                                lanes_left,
                                row_rects
                                    .last()
                                    .map_or(areas.rows.top(), |(_, r)| r.bottom()),
                            ),
                            pos2(lanes_right, areas.rows.bottom()),
                        ),
                    };
                    painter.rect_filled(highlight, 0.0, theme::SELECTION.gamma_multiply(0.10));
                    painter.vline(
                        timeline.x(start as f64),
                        highlight.y_range(),
                        Stroke::new(2.0, theme::SELECTION),
                    );
                }
            }
        });

    let below_top = row_rects
        .last()
        .map_or(areas.rows.top(), |(_, rect)| rect.bottom());
    let below = Rect::from_min_max(
        pos2(lanes_left, below_top.max(areas.rows.top())),
        pos2(lanes_right, areas.rows.bottom()),
    );
    if below.height() > 0.0 {
        let response = ui.interact(below, Id::new("arrangement-below"), Sense::click());
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            actions.push(Action::SelectClip(None));
            actions.push(Action::Seek(timeline.snapped(pointer.x, !ignore_grid(ui))));
        }
        response.context_menu(|ui| {
            for (kind, label) in [
                (TrackKind::Audio, "Insert Audio Track"),
                (TrackKind::Midi, "Insert MIDI Track"),
                (TrackKind::Group, "Insert Group Track"),
            ] {
                if ui.button(label).clicked() {
                    actions.push(Action::AddTrack(kind));
                }
            }
            ui.separator();
            grid_menu(ui, state.grid, project, actions);
        });
    }
    if display.is_empty() {
        empty_state(ui, below, actions);
    } else if below.height() >= 34.0 {
        ui.painter_at(below).text(
            below.center(),
            Align2::CENTER_CENTER,
            "Drop audio files or devices here",
            theme::prop(11.0),
            theme::TEXT_FAINT,
        );
    }

    master_row(
        ui,
        areas.master,
        &timeline,
        inputs,
        state,
        main_meter,
        actions,
    );
    if state.automation {
        automation::lane(
            ui,
            areas.master.with_min_x(lanes_left),
            0,
            state,
            project,
            &timeline,
            &mut interaction.automation,
            actions,
        );
        interaction.automation.finish(ui, actions);
    }
    device_drop_target(ui, areas.master, project, 0, actions);

    let overlay = ui.painter().with_clip_rect(areas.lanes());
    let start_x = timeline.x(state.playback_start as f64).round() + 0.5;
    if (lanes_left..=lanes_right).contains(&start_x) {
        overlay.vline(
            start_x,
            areas.numbers.top()..=areas.master.bottom(),
            Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.65)),
        );
        let start_head = Rect::from_min_max(
            pos2(start_x - 5.0, areas.numbers.top()),
            pos2(start_x + 5.0, areas.numbers.top() + 8.0),
        );
        ui.interact(start_head, Id::new("playback-start"), Sense::hover())
            .on_hover_text("Next playback starts here");
        overlay.add(Shape::convex_polygon(
            vec![
                start_head.left_top(),
                start_head.right_top(),
                start_head.center_bottom(),
            ],
            theme::ACCENT,
            Stroke::NONE,
        ));
    }
    if inputs.status.state == PlaybackState::Playing {
        let x = timeline.x(playhead).round() + 0.5;
        overlay.vline(
            x,
            areas.numbers.top()..=areas.master.bottom(),
            Stroke::new(1.0, theme::PLAYHEAD),
        );
        let head = vec![
            pos2(x - 5.0, areas.numbers.top()),
            pos2(x + 5.0, areas.numbers.top()),
            pos2(x, areas.numbers.top() + 7.0),
        ];
        overlay.add(Shape::convex_polygon(head, theme::PLAYHEAD, Stroke::NONE));
    }

    ui.painter().vline(
        lanes_left - 0.5,
        full.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );

    if !ui.input(|i| i.pointer.any_down()) {
        interaction.track_drag = None;
        interaction.clip_drag = None;
        interaction.loop_drag = None;
        interaction.loop_preview = None;
        interaction.overview_drag = None;
        interaction.range_drag = None;
        interaction.range_move = None;
    }
    ui.data_mut(|d| d.insert_temp(interaction_id(), interaction));
    ui.advance_cursor_after_rect(full);
    drop_target
}

fn device_drop_target(
    ui: &Ui,
    rect: Rect,
    project: &Project,
    track: usize,
    actions: &mut Vec<Action>,
) {
    if !ui.rect_contains_pointer(rect) {
        return;
    }
    let Some(payload) = egui::DragAndDrop::payload::<DeviceDrag>(ui.ctx()) else {
        return;
    };
    let DeviceDrag::Browser(device) = payload.as_ref() else {
        return;
    };
    if !device.can_load_on(project, track) {
        ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
        return;
    }
    ui.ctx().set_cursor_icon(CursorIcon::Copy);
    ui.painter()
        .rect_filled(rect, 0.0, theme::SELECTION.gamma_multiply(0.10));
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        0.0,
        Stroke::new(2.0, theme::SELECTION),
        StrokeKind::Inside,
    );
    if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
        egui::DragAndDrop::take_payload::<DeviceDrag>(ui.ctx());
        actions.push(device.load_action(track, None));
    }
}

fn handle_wheel(ui: &mut Ui, state: &mut ArrangementState, lanes: Rect, ruler: Rect) {
    if !ui.rect_contains_pointer(lanes) {
        return;
    }
    let (scroll, zoom, pointer) =
        ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
    let Some(pointer) = pointer else { return };
    if zoom != 1.0 {
        state.follow = false;
        state.zoom_around(f64::from(zoom), (pointer.x - lanes.left()).max(0.0));
    }
    let over_ruler = ruler.contains(pointer);
    let horizontal = if over_ruler {
        scroll.x + scroll.y
    } else {
        scroll.x
    };
    if horizontal != 0.0 {
        state.follow = false;
        state.scroll_tick =
            (state.scroll_tick - f64::from(horizontal) / state.pixels_per_tick).max(0.0);
        ui.input_mut(|i| {
            i.smooth_scroll_delta.x = 0.0;
            if over_ruler {
                i.smooth_scroll_delta.y = 0.0;
            }
        });
    }
}

fn content_end_tick(project: &Project) -> f64 {
    let audio = project.duration_frames().unwrap_or(0) as f64 * project.bpm() / 3000.0;
    let midi = project
        .tracks()
        .iter()
        .filter_map(|track| match track {
            Track::Midi(midi) => midi
                .clips()
                .iter()
                .map(|clip| clip.start_tick + clip.length_ticks)
                .max(),
            _ => None,
        })
        .max()
        .unwrap_or(0) as f64;
    audio.max(midi)
}

#[allow(clippy::too_many_arguments)]
fn ruler(
    ui: &mut Ui,
    areas: &Areas,
    timeline: &Timeline,
    project: &Project,
    inputs: &Inputs<'_>,
    interaction: &mut Interaction,
    loop_shown: LoopRegion,
    automation: bool,
    actions: &mut Vec<Action>,
) {
    let meter = project.time_signature();
    let numbers = areas.numbers;
    let band = areas.band;

    let corner = areas.corner;
    ui.painter_at(corner)
        .rect_filled(corner, 0.0, theme::BG_PANEL);
    ui.painter().hline(
        corner.x_range(),
        corner.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    let reset_rect = Rect::from_min_size(
        pos2(corner.right() - 102.0, corner.top() + 5.0),
        vec2(94.0, 18.0),
    );
    let reset = ui.put(
        reset_rect,
        egui::Button::new(RichText::new("Reset Heights").size(10.0)),
    );
    ui.interact(reset.rect, Id::new("reset-track-heights"), Sense::hover());
    if reset
        .on_hover_text("Restore all arrangement track heights to Normal")
        .clicked()
    {
        actions.push(Action::ResetTrackHeights);
    }
    widgets::inside(
        ui,
        corner.shrink2(vec2(8.0, 6.0)),
        Layout::left_to_right(Align::Max),
        |ui| {
            automation::toggle(ui, automation, actions);
            let tracks = project.tracks().len() - 1;
            ui.label(
                RichText::new(format!(
                    "{tracks} track{}",
                    if tracks == 1 { "" } else { "s" }
                ))
                .size(10.0)
                .color(theme::TEXT_FAINT),
            );
        },
    );

    let painter = ui.painter_at(numbers);
    painter.rect_filled(numbers, 0.0, theme::BG_PANEL);
    if let Some(range) = &inputs.selection.range {
        let span = Rect::from_x_y_ranges(
            timeline.x(range.start as f64)..=timeline.x(range.end as f64),
            numbers.y_range(),
        );
        painter.rect_filled(span, 0.0, theme::SELECTION.gamma_multiply(0.22));
    }
    let (from, to) = timeline.visible();
    let step = format::ruler_step(meter, timeline.pixels_per_tick, 48.0);
    let bar = meter.ticks_per_bar();
    let first = (from as u64 / step) * step;
    let mut tick = first;
    while (tick as f64) <= to + step as f64 {
        let x = timeline.x(tick as f64).round() + 0.5;
        let is_bar = tick.is_multiple_of(bar);
        let height = if is_bar { 8.0 } else { 4.0 };
        painter.vline(
            x,
            numbers.bottom() - height..=numbers.bottom(),
            Stroke::new(1.0, theme::TEXT_FAINT),
        );
        let label = if step >= bar {
            format!("{}", tick / bar + 1)
        } else {
            format::bar_beat(meter, tick).rsplit_once('.').map_or_else(
                String::new,
                |(head, sixteenth)| {
                    if sixteenth == "1" {
                        head.to_owned()
                    } else {
                        format!("{head}.{sixteenth}")
                    }
                },
            )
        };
        painter.text(
            pos2(x + 3.0, numbers.top() + 2.0),
            Align2::LEFT_TOP,
            label,
            theme::prop(if is_bar { 10.5 } else { 9.5 }),
            if is_bar { theme::TEXT } else { theme::TEXT_DIM },
        );
        tick += step;
    }
    painter.hline(
        numbers.x_range(),
        numbers.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );

    let painter = ui.painter_at(band);
    painter.rect_filled(band, 0.0, theme::LANE);
    painter.hline(
        band.x_range(),
        band.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    let x0 = timeline.x(loop_shown.start as f64);
    let x1 = timeline.x(loop_shown.end as f64);
    let brace = Rect::from_x_y_ranges(x0..=x1, band.top() + 2.0..=band.bottom() - 3.0);
    let brace_color = if loop_shown.enabled {
        theme::LOOP
    } else {
        theme::LOOP_OFF
    };
    painter.rect_filled(brace, 0.0, brace_color);
    for edge in [brace.left(), brace.right() - 2.0] {
        painter.rect_filled(
            Rect::from_min_max(pos2(edge, brace.top()), pos2(edge + 2.0, band.bottom())),
            0.0,
            brace_color,
        );
    }

    let snap_on = !ignore_grid(ui);
    let band_response = ui
        .interact(band, Id::new("loop-band"), Sense::click_and_drag())
        .on_hover_text("Drag to set the loop. Double-click to switch looping on or off.");
    let near = |x: f32, edge: f32| (x - edge).abs() <= EDGE_GRAB;
    if let Some(pointer) = band_response.hover_pos() {
        if near(pointer.x, x0) || near(pointer.x, x1) {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        } else if pointer.x > x0 && pointer.x < x1 {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }
    if band_response.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        let region = inputs.loop_region;
        let tick = timeline.tick(origin.x);
        interaction.loop_drag = Some(if near(origin.x, x0) {
            LoopDrag::Start
        } else if near(origin.x, x1) {
            LoopDrag::End
        } else if origin.x > x0 && origin.x < x1 {
            LoopDrag::Move {
                grab_offset: tick - region.start as f64,
                length: region.end - region.start,
            }
        } else {
            LoopDrag::Create {
                anchor: edit::snap(timeline.grid, tick.round() as u64, snap_on),
            }
        });
    }
    if let Some(drag) = interaction.loop_drag
        && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
    {
        let tick = timeline.snapped(pointer.x, snap_on);
        let region = inputs.loop_region;
        let minimum = timeline
            .grid
            .interval()
            .filter(|_| snap_on)
            .unwrap_or(TICKS_PER_QUARTER / 4);
        let (start, end) = match drag {
            LoopDrag::Create { anchor } => (anchor.min(tick), anchor.max(tick)),
            LoopDrag::Start => (tick.min(region.end.saturating_sub(minimum)), region.end),
            LoopDrag::End => (region.start, tick.max(region.start + minimum)),
            LoopDrag::Move {
                grab_offset,
                length,
            } => {
                let raw = (timeline.tick(pointer.x) - grab_offset).max(0.0).round() as u64;
                let start = edit::snap(timeline.grid, raw, snap_on);
                (start, start + length)
            }
        };
        interaction.loop_preview = Some((start, end.max(start + minimum)));
    }
    if band_response.drag_stopped() {
        if let Some((start, end)) = interaction.loop_preview.take() {
            actions.push(Action::SetLoop {
                start,
                end,
                enable: true,
            });
        }
        interaction.loop_drag = None;
    }
    if band_response.double_clicked() {
        actions.push(Action::ToggleLoop);
    }

    let seek = ui
        .interact(numbers, Id::new("ruler-seek"), Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Set next playback start");
    if (seek.clicked() || seek.dragged())
        && let Some(pointer) = seek.interact_pointer_pos()
    {
        let tick = timeline.snapped(pointer.x, snap_on);
        actions.push(Action::Seek(tick));
    }
}

fn paint_grid(
    painter: &egui::Painter,
    timeline: &Timeline,
    meter: TimeSignature,
    y: std::ops::RangeInclusive<f32>,
    [bar_color, beat_color, fine_color]: [Color32; 3],
) {
    let bounds = painter.clip_rect();
    let from = timeline.tick(bounds.left().max(timeline.left));
    let to = timeline.tick(bounds.right().min(timeline.right));
    let bar = meter.ticks_per_bar();
    let beat = meter.ticks_per_beat();
    let step = timeline.visible_grid_interval(meter);
    let mut tick = (from as u64 / step) * step;
    while (tick as f64) <= to {
        let color = if tick.is_multiple_of(bar) {
            bar_color
        } else if tick.is_multiple_of(beat) {
            beat_color
        } else {
            fine_color
        };
        painter.vline(
            timeline.grid_line_x(tick),
            y.clone(),
            Stroke::new(1.0, color),
        );
        tick += step;
    }
}

#[allow(clippy::too_many_arguments)]
fn lane_background(
    ui: &mut Ui,
    lane: Rect,
    row: Row,
    display: &[Row],
    timeline: &Timeline,
    grid_mode: GridMode,
    playback_start: u64,
    inputs: &Inputs<'_>,
    interaction: &mut Interaction,
    actions: &mut Vec<Action>,
) {
    let project = inputs.project;
    let response = ui.interact(lane, Id::new(("lane", row.index)), Sense::click_and_drag());
    let snap_on = !ignore_grid(ui);
    let shift = ui.input(|i| i.modifiers.shift);
    if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        if response.double_clicked()
            && let Some(clip) = clip_at_tick(project, row.index, timeline.tick(pointer.x))
        {
            open_clip(project, clip, actions);
        } else {
            let tick = timeline.snapped(pointer.x, snap_on);
            match inputs.selection.range.as_ref().filter(|_| shift) {
                Some(range) => {
                    let extended = extend_range(range, row.index, tick, display);
                    actions.push(Action::SetTimeSelection(Some(extended)));
                }
                None => {
                    actions.push(Action::SelectClip(None));
                    actions.push(Action::SelectTracks(BTreeSet::from([row.index])));
                    actions.push(Action::Seek(tick));
                }
            }
            if response.double_clicked() && matches!(project.tracks()[row.index], Track::Midi(_)) {
                let tick = SnapGrid::Bar.floor(
                    timeline.tick(pointer.x).round() as u64,
                    project.time_signature(),
                );
                actions.push(Action::InsertMidiClip {
                    track: row.index,
                    start: tick,
                });
            }
        }
    }
    if response.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        interaction.range_drag = Some(RangeDrag {
            anchor: timeline.snapped(origin.x, snap_on),
            track: row.index,
        });
        actions.push(Action::SelectClip(None));
    }
    if response.secondary_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let tick = timeline.grid.floor(timeline.tick(pointer.x).round() as u64);
        interaction.menu_at = Some((row.index, tick));
    }
    let menu_tick = interaction
        .menu_at
        .filter(|(track, _)| *track == row.index)
        .map_or_else(
            || {
                playhead_tick(project, inputs.status, inputs.loop_region, playback_start).round()
                    as u64
            },
            |(_, tick)| tick,
        );
    response.context_menu(|ui| {
        lane_menu(ui, row.index, menu_tick, grid_mode, inputs, actions);
    });
}

fn clip_at_tick(project: &Project, track: usize, tick: f64) -> Option<ClipRef> {
    let index = match &project.tracks()[track] {
        Track::Audio(audio) => audio.clips().iter().rposition(|clip| {
            tick >= clip.start_tick as f64 && tick < audio_end_tick(project, clip)
        }),
        Track::Midi(midi) => midi.clips().iter().rposition(|clip| {
            tick >= clip.start_tick as f64 && tick < (clip.start_tick + clip.length_ticks) as f64
        }),
        _ => None,
    }?;
    Some(ClipRef { track, index })
}

fn open_clip(project: &Project, clip: ClipRef, actions: &mut Vec<Action>) {
    actions.push(Action::SelectClip(Some(clip)));
    actions.push(if edit::midi_clip(project, clip).is_some() {
        Action::OpenPianoRoll(clip)
    } else {
        Action::ShowClipView
    });
}

fn lane_menu(
    ui: &mut Ui,
    track: usize,
    tick: u64,
    grid_mode: GridMode,
    inputs: &Inputs<'_>,
    actions: &mut Vec<Action>,
) {
    let project = inputs.project;
    let ctx = ui.ctx().clone();
    let command = |key: Key| ctx.format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, key));
    let range = inputs.selection.range.as_ref();
    match project.tracks()[track] {
        Track::Audio(_) => {
            if ui.button("Import Audio Here…").clicked() {
                actions.push(Action::ChooseAudioToImport {
                    track: Some(track),
                    start: Some(tick),
                });
            }
        }
        Track::Midi(_) => {
            let fills_selection = range.is_some_and(|range| {
                range.tracks.len() == 1
                    && range.tracks.contains(&track)
                    && edit::range_is_empty(project, track, range.start, range.end)
            });
            let insert = egui::Button::new("Insert MIDI Clip").shortcut_text(command(Key::M));
            if ui.add(insert).clicked() {
                actions.push(if fills_selection {
                    Action::InsertMidiClipInSelection
                } else {
                    Action::InsertMidiClip { track, start: tick }
                });
            }
        }
        _ => {
            ui.label(RichText::new("Groups mix their member tracks").color(theme::TEXT_DIM));
        }
    }
    if range.is_some() {
        ui.separator();
        if ui
            .add_enabled(
                inputs.selection.has_audio(project),
                egui::Button::new("Reverse Selection").shortcut_text("R"),
            )
            .clicked()
        {
            actions.push(Action::ReverseSelection);
        }
        if ui
            .add(egui::Button::new("Split at Selection").shortcut_text(command(Key::E)))
            .clicked()
        {
            actions.push(Action::SplitTimeSelection);
        }
        if ui
            .add(egui::Button::new("Delete Selection").shortcut_text("Del"))
            .clicked()
        {
            actions.push(Action::DeleteTimeSelection);
        }
    }
    ui.separator();
    grid_menu(ui, grid_mode, project, actions);
}

pub fn grid_menu(ui: &mut Ui, current: GridMode, project: &Project, actions: &mut Vec<Action>) {
    let meter = project.time_signature();
    let label = |grid: SnapGrid| match grid.interval(meter) {
        Some(interval) => state::interval_label(meter, interval),
        None => "Off".to_owned(),
    };
    for density in GridDensity::ALL {
        let mode = GridMode::Adaptive(density);
        if ui
            .radio(current == mode, format!("{} Grid", density.label()))
            .clicked()
        {
            actions.push(Action::SetGridMode(mode));
            ui.close();
        }
    }
    let saved = project.snap_grid();
    let fixed = current == GridMode::Fixed && saved != SnapGrid::Off;
    let title = if fixed {
        format!("Fixed Grid ({})", label(saved))
    } else {
        "Fixed Grid".to_owned()
    };
    ui.menu_button(title, |ui| {
        for grid in &format::GRIDS[1..] {
            if ui.radio(fixed && saved == *grid, label(*grid)).clicked() {
                actions.push(Action::SetGridMode(GridMode::Fixed));
                if saved != *grid {
                    actions.push(Action::SetSnap(*grid));
                }
                ui.close();
            }
        }
    });
    let off = current == GridMode::Fixed && saved == SnapGrid::Off;
    if ui
        .radio(off, "Off")
        .on_hover_text("Edits ignore the grid")
        .clicked()
    {
        actions.push(Action::SetGridMode(GridMode::Fixed));
        if saved != SnapGrid::Off {
            actions.push(Action::SetSnap(SnapGrid::Off));
        }
        ui.close();
    }
}

fn tracks_between(rows: &[(Row, Rect)], track: usize, y: f32) -> BTreeSet<usize> {
    let Some(anchor) = rows.iter().position(|(row, _)| row.index == track) else {
        return BTreeSet::from([track]);
    };
    let current = match rows.iter().position(|(_, rect)| rect.y_range().contains(y)) {
        Some(position) => position,
        None if rows.first().is_some_and(|(_, rect)| y < rect.top()) => 0,
        None => rows.len() - 1,
    };
    let (from, to) = (anchor.min(current), anchor.max(current));
    rows[from..=to].iter().map(|(row, _)| row.index).collect()
}

fn extend_range(range: &TimeRange, track: usize, tick: u64, display: &[Row]) -> TimeRange {
    let (start, end) = if tick < range.start {
        (tick, range.end)
    } else {
        (range.start, tick.max(range.start + 1))
    };
    let position = |index: usize| display.iter().position(|row| row.index == index);
    let positions: Vec<usize> = range
        .tracks
        .iter()
        .copied()
        .chain([track])
        .filter_map(position)
        .collect();
    let tracks = match (positions.iter().min(), positions.iter().max()) {
        (Some(&from), Some(&to)) => display[from..=to].iter().map(|row| row.index).collect(),
        _ => BTreeSet::from([track]),
    };
    TimeRange { start, end, tracks }
}

fn update_range_drag(
    ui: &Ui,
    timeline: &Timeline,
    inputs: &Inputs<'_>,
    rows: &[(Row, Rect)],
    interaction: &mut Interaction,
    actions: &mut Vec<Action>,
) {
    let Some(drag) = interaction.range_drag else {
        return;
    };
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    let x = pointer.x.clamp(timeline.left, timeline.right);
    let tick = timeline.snapped(x, !ignore_grid(ui));
    let (start, end) = (drag.anchor.min(tick), drag.anchor.max(tick));
    let range = (start < end).then(|| TimeRange {
        start,
        end,
        tracks: tracks_between(rows, drag.track, pointer.y),
    });
    if inputs.selection.range != range {
        actions.push(Action::SetTimeSelection(range));
    }
    if ui.input(|i| i.pointer.primary_released()) {
        interaction.range_drag = None;
        actions.push(Action::Seek(start));
    }
}

fn paint_time_selection(
    painter: &egui::Painter,
    timeline: &Timeline,
    range: &TimeRange,
    rows: &[(Row, Rect)],
) {
    let x0 = timeline.x(range.start as f64).round();
    let x1 = timeline.x(range.end as f64).round();
    for (row, rect) in rows {
        if !range.tracks.contains(&row.index) {
            continue;
        }
        let area = Rect::from_x_y_ranges(x0..=x1, rect.top()..=rect.bottom() - 1.0);
        painter.rect_filled(area, 0.0, theme::SELECTION.gamma_multiply(0.20));
        for x in [x0 + 0.5, x1 - 0.5] {
            painter.vline(x, area.y_range(), Stroke::new(1.0, theme::SELECTION));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn clips(
    ui: &mut Ui,
    painter: &egui::Painter,
    lane: Rect,
    row: Row,
    timeline: &Timeline,
    project: &Project,
    inputs: &Inputs<'_>,
    waveforms: &mut WaveformCache,
    interaction: &mut Interaction,
    editable: bool,
    actions: &mut Vec<Action>,
) {
    let color = theme::track_color(project, row.index);
    // Command can be pressed or released mid-drag; whatever is held at the drop decides.
    if let Some(drag) = &mut interaction.clip_drag {
        drag.copy = drag.mode == ClipMode::Move && ui.input(|i| i.modifiers.command);
    }
    let body = Rect::from_min_max(lane.min + vec2(0.0, 1.0), lane.max - vec2(0.0, 2.0));
    let selection = inputs
        .selection
        .range
        .as_ref()
        .filter(|range| range.tracks.contains(&row.index));
    let in_selection =
        |x: f32| selection.is_some_and(|range| range.contains(row.index, timeline.tick(x)));
    if matches!(project.tracks()[row.index], Track::Group(_)) {
        group_summary(painter, body, row.index, timeline, project);
        return;
    }
    let count = match &project.tracks()[row.index] {
        Track::Audio(track) => track.clips().len(),
        Track::Midi(track) => track.clips().len(),
        _ => 0,
    };
    let (view_from, view_to) = timeline.visible();
    for index in 0..count {
        let clip = ClipRef {
            track: row.index,
            index,
        };
        let (start, end) = match &project.tracks()[row.index] {
            Track::Audio(track) => {
                let audio = &track.clips()[index];
                (audio.start_tick as f64, audio_end_tick(project, audio))
            }
            Track::Midi(track) => {
                let midi = &track.clips()[index];
                (
                    midi.start_tick as f64,
                    (midi.start_tick + midi.length_ticks) as f64,
                )
            }
            _ => continue,
        };
        if end < view_from || start > view_to {
            continue;
        }
        let x0 = timeline.x(start);
        let x1 = timeline.x(end).max(x0 + 3.0);
        let rect = Rect::from_x_y_ranges(x0..=x1, body.y_range());
        let header = rect.with_max_y((rect.top() + CLIP_HEADER).min(rect.bottom()));
        let selected = inputs.selection.clip == Some(clip);
        let uid = project.tracks()[row.index].clip_uids()[index];

        ui.interact(
            rect.intersect(lane),
            Id::new(("clip", row.index, index)),
            Sense::hover(),
        );
        let response = ui.interact(
            header.intersect(lane),
            Id::new(("clip-title", row.index, index)),
            if editable {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            },
        );
        let edge = |pointer: Pos2| {
            if rect.width() > EDGE_GRAB * 3.0 && pointer.x >= rect.right() - EDGE_GRAB {
                ClipMode::TrimEnd
            } else if rect.width() > EDGE_GRAB * 3.0 && pointer.x <= rect.left() + EDGE_GRAB {
                ClipMode::TrimStart
            } else {
                ClipMode::Move
            }
        };
        if let Some(pointer) = response.hover_pos().filter(|_| editable) {
            let cursor = match edge(pointer) {
                _ if in_selection(pointer.x) => CursorIcon::Grab,
                ClipMode::Move => CursorIcon::Grab,
                _ => CursorIcon::ResizeHorizontal,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
        if response.clicked() {
            if response.double_clicked() {
                open_clip(project, clip, actions);
            } else {
                actions.push(Action::SelectClip(Some(clip)));
            }
        }
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            && header.contains(origin)
            && !ui.input(|i| i.modifiers.command)
            && let Some(range) = selection.filter(|_| in_selection(origin.x))
        {
            interaction.range_move = Some(RangeMove {
                range: range.clone(),
                grab_offset: timeline.tick(origin.x) - range.start as f64,
            });
        } else if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            && header.contains(origin)
        {
            let span = edit::clip_span(project, clip).unwrap_or((start as u64, end as u64));
            interaction.clip_drag = Some(ClipDrag {
                clip,
                mode: edge(origin),
                copy: edge(origin) == ClipMode::Move && ui.input(|i| i.modifiers.command),
                grab_offset: timeline.tick(origin.x) - start,
                start: span.0,
                end: span.1,
            });
            actions.push(Action::SelectClip(Some(clip)));
        }
        if response.dragged()
            && interaction
                .clip_drag
                .is_some_and(|drag| drag.mode != ClipMode::Move)
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        } else if response.dragged() && interaction.clip_drag.is_some_and(|drag| drag.copy) {
            ui.ctx().set_cursor_icon(CursorIcon::Copy);
        } else if response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        if response.drag_stopped()
            && let Some(pointer) = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()))
        {
            if let Some(drag) = interaction.clip_drag.take() {
                interaction.released = Some((drag, pointer));
            }
            if let Some(moving) = interaction.range_move.take() {
                interaction.range_released = Some((moving, pointer));
            }
        }
        if editable {
            let mut rename = false;
            response.context_menu(|ui| {
                let reverse_selection = selection
                    .is_some_and(|range| start < range.end as f64 && end > range.start as f64)
                    && inputs.selection.has_audio(project);
                rename = clip_menu(ui, project, clip, reverse_selection, actions);
            });
            if rename {
                let current = edit::clip_name(project, clip).unwrap_or_default();
                interaction.clip_rename = Some((uid, current.to_owned()));
            }
        }

        if !ui.is_rect_visible(rect) {
            continue;
        }
        let moving_original = interaction
            .clip_drag
            .is_some_and(|drag| drag.clip == clip && !drag.copy);
        let fade = if moving_original { 0.45 } else { 1.0 };
        let (fill, ink, label_fill) = if selected {
            (
                theme::tint(theme::LANE, color, 0.30),
                theme::tint(color, Color32::WHITE, 0.25),
                theme::tint(color, Color32::WHITE, 0.15),
            )
        } else {
            (
                theme::tint(theme::LANE, color, 0.14),
                color.gamma_multiply(0.92),
                theme::tint(color, Color32::BLACK, 0.12),
            )
        };
        let painted_fill = fill.gamma_multiply(fade);
        painter.rect_filled(rect, 2.0, painted_fill);
        painter.rect_filled(
            header,
            egui::CornerRadius {
                nw: 2,
                ne: 2,
                sw: 0,
                se: 0,
            },
            label_fill.gamma_multiply(fade),
        );
        painter.hline(
            header.x_range(),
            header.bottom() - 0.5,
            Stroke::new(1.0, Color32::from_black_alpha(90)),
        );
        let content = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max)
            .shrink2(vec2(0.0, 2.0));
        let visible_x = (x0.max(lane.left()))..=(x1.min(lane.right()));
        let text_rect =
            Rect::from_x_y_ranges((x0.max(lane.left()) + 4.0)..=(x1 - 3.0), header.y_range());
        let renaming = interaction
            .clip_rename
            .as_ref()
            .is_some_and(|(renamed, _)| *renamed == uid);
        if renaming {
            rename_clip_inline(ui, header.intersect(lane), clip, interaction, actions);
        }
        let fitted_label = |label: &str| {
            if !renaming {
                widgets::fitted_text(
                    painter,
                    text_rect,
                    label,
                    theme::prop(10.0),
                    theme::TEXT_ON_COLOR,
                    Align2::LEFT_CENTER,
                );
            }
        };
        match &project.tracks()[row.index] {
            Track::Audio(track) => {
                let audio = &track.clips()[index];
                if let Some((overview, buffer)) = waveforms.get(project, audio.asset) {
                    paint_waveform(
                        painter,
                        content,
                        visible_x,
                        audio,
                        overview,
                        buffer,
                        timeline,
                        project.bpm(),
                        ink,
                    );
                }
                let mut label = audio
                    .name
                    .clone()
                    .unwrap_or_else(|| track_title(&project.tracks()[row.index]));
                if (audio.gain - 1.0).abs() > 1e-4 {
                    label = format!("{label}  {}", format::db(audio.gain));
                    if audio.gain < 0.0 {
                        label.push_str(" ø");
                    }
                }
                fitted_label(&label);
            }
            Track::Midi(track) => {
                let midi = &track.clips()[index];
                paint_notes(painter, content, midi, timeline, ink);
                fitted_label(&midi_clip_label(midi));
            }
            _ => {}
        }
        let lane_fill = if inputs.selection.tracks.contains(&row.index) {
            theme::LANE_SELECTED
        } else {
            theme::LANE
        };
        let grid_color = if lane_fill.blend(painted_fill).intensity() >= 0.5 {
            Color32::from_black_alpha
        } else {
            Color32::from_white_alpha
        };
        let grid_rect = rect.with_min_y(header.bottom()).intersect(lane);
        if grid_rect.is_positive() {
            paint_grid(
                &painter.with_clip_rect(grid_rect),
                timeline,
                project.time_signature(),
                grid_rect.top()..=grid_rect.bottom(),
                [96, 64, 40].map(grid_color),
            );
        }
        if selected {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, theme::tint(color, Color32::WHITE, 0.6)),
                StrokeKind::Inside,
            );
        } else if response.hovered() {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, Color32::from_white_alpha(110)),
                StrokeKind::Inside,
            );
        } else {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, Color32::from_black_alpha(120)),
                StrokeKind::Inside,
            );
        }
    }
}

fn group_summary(
    painter: &egui::Painter,
    body: Rect,
    group: usize,
    timeline: &Timeline,
    project: &Project,
) {
    let members: Vec<usize> = rows::subtree(project, group)
        .into_iter()
        .filter(|index| matches!(project.tracks()[*index], Track::Audio(_) | Track::Midi(_)))
        .collect();
    if members.is_empty() {
        return;
    }
    let band = (body.height() / members.len() as f32).clamp(2.0, 10.0);
    let top = body.center().y - band * members.len() as f32 / 2.0;
    for (slot, index) in members.iter().enumerate() {
        let y = top + slot as f32 * band;
        let spans: Vec<(f64, f64)> = match &project.tracks()[*index] {
            Track::Audio(track) => track
                .clips()
                .iter()
                .map(|clip| (clip.start_tick as f64, audio_end_tick(project, clip)))
                .collect(),
            Track::Midi(track) => track
                .clips()
                .iter()
                .map(|clip| {
                    (
                        clip.start_tick as f64,
                        (clip.start_tick + clip.length_ticks) as f64,
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        for (start, end) in spans {
            let rect = Rect::from_x_y_ranges(
                timeline.x(start)..=timeline.x(end),
                y + 1.0..=y + band - 1.0,
            );
            painter.rect_filled(
                rect,
                0.0,
                theme::track_color(project, *index).gamma_multiply(0.6),
            );
        }
    }
}

/// A MIDI clip's name, or how many notes it plays when it has none.
pub(crate) fn midi_clip_label(clip: &MidiClip) -> String {
    clip.name.clone().unwrap_or_else(|| {
        let count = clip.audible_notes().count();
        format!("{count} note{}", if count == 1 { "" } else { "s" })
    })
}

fn rename_clip_inline(
    ui: &mut Ui,
    header: Rect,
    clip: ClipRef,
    interaction: &mut Interaction,
    actions: &mut Vec<Action>,
) {
    let mut commit = None;
    widgets::inside(
        ui,
        header.shrink2(vec2(2.0, 0.0)),
        Layout::left_to_right(Align::Center),
        |ui| {
            if let Some((_, text)) = &mut interaction.clip_rename {
                let edit = ui.add(
                    egui::TextEdit::singleline(text)
                        .id(Id::new("clip-rename"))
                        .hint_text("Clip name")
                        .font(theme::prop(10.0))
                        .margin(vec2(3.0, 0.0))
                        .desired_width(header.width() - 6.0),
                );
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                if edit.lost_focus() {
                    // egui consumes the Escape that ends editing, so look at the raw input.
                    let cancelled = ui.input(|i| {
                        i.raw.events.iter().any(|event| {
                            matches!(
                                event,
                                egui::Event::Key {
                                    key: egui::Key::Escape,
                                    pressed: true,
                                    ..
                                }
                            )
                        })
                    });
                    commit = Some((!cancelled).then(|| text.clone()));
                }
            }
        },
    );
    if let Some(result) = commit {
        if let Some(name) = result {
            actions.push(Action::RenameClip { clip, name });
        }
        interaction.clip_rename = None;
    }
}

/// Returns whether the user chose to rename the clip.
fn clip_menu(
    ui: &mut Ui,
    project: &Project,
    clip: ClipRef,
    reverse_selection: bool,
    actions: &mut Vec<Action>,
) -> bool {
    if edit::midi_clip(project, clip).is_some() {
        if ui.button("Edit Notes").clicked() {
            actions.push(Action::OpenPianoRoll(clip));
        }
    } else if ui.button("Edit Clip").clicked() {
        actions.push(Action::SelectClip(Some(clip)));
        actions.push(Action::ShowClipView);
    }
    let rename = ui.button("Rename").clicked();
    ui.separator();
    if ui.button("Split").clicked() {
        actions.push(Action::SplitClip(clip));
    }
    if ui.button("Duplicate").clicked() {
        actions.push(Action::DuplicateClip(clip));
    }
    if ui.button("Loop Selection").clicked() {
        actions.push(Action::LoopClip(clip));
    }
    if let Some(audio) = edit::audio_clip(project, clip) {
        ui.separator();
        if ui
            .add(egui::Button::new("Reverse").shortcut_text("R"))
            .clicked()
        {
            actions.push(if reverse_selection {
                Action::ReverseSelection
            } else {
                Action::ReverseClip(clip)
            });
        }
        ui.menu_button("Pitch", |ui| {
            let mut semitones = audio.pitch_semitones;
            if ui
                .add(
                    egui::DragValue::new(&mut semitones)
                        .speed(0.1)
                        .range(-24.0..=24.0)
                        .max_decimals(2)
                        .suffix(" st"),
                )
                .on_hover_text("Semitones; duration stays unchanged")
                .changed()
            {
                actions.push(Action::SetClipPitch { clip, semitones });
            }
            if ui.button("Reset Pitch").clicked() {
                actions.push(Action::ResetClipPitch(clip));
            }
            ui.separator();
            ui.label("Algorithm");
            let mut algorithm = audio.pitch_algorithm;
            let mut preserve_formants = audio.preserve_formants;
            for choice in [
                PitchAlgorithm::Signalsmith,
                PitchAlgorithm::RubberBandR3,
                PitchAlgorithm::Builtin,
            ] {
                ui.selectable_value(&mut algorithm, choice, choice.label());
            }
            ui.add_enabled(
                algorithm.supports_formants(),
                egui::Checkbox::new(&mut preserve_formants, "Preserve Formants"),
            );
            if algorithm != audio.pitch_algorithm || preserve_formants != audio.preserve_formants {
                actions.push(Action::SetClipPitchOptions {
                    clip,
                    algorithm,
                    preserve_formants,
                });
            }
        });
        if ui.button("Reset Clip Gain").clicked() {
            actions.push(Action::SetClipGain { clip, gain: 1.0 });
        }
        if ui.button("Invert Polarity").clicked() {
            actions.push(Action::SetClipGain {
                clip,
                gain: -audio.gain,
            });
        }
    }
    ui.separator();
    if edit::midi_clip(project, clip).is_some() {
        if ui.button("Export MIDI File…").clicked() {
            actions.push(Action::ExportClipMidi(clip));
        }
    } else if ui.button("Export Audio File…").clicked() {
        actions.push(Action::ExportClipAudio(clip));
    }
    ui.separator();
    if ui.button("Delete").clicked() {
        actions.push(Action::DeleteClip(clip));
    }
    rename
}

fn ghost(
    project: &Project,
    drag: ClipDrag,
    timeline: &Timeline,
    pointer: Pos2,
    rows: &[(Row, Rect)],
    snap_on: bool,
) -> Option<Ghost> {
    let raw = timeline.tick(pointer.x);
    match drag.mode {
        ClipMode::Move => {
            let start = edit::snap(
                timeline.grid,
                (raw - drag.grab_offset).max(0.0).round() as u64,
                snap_on,
            );
            let origin_kind = std::mem::discriminant(&project.tracks()[drag.clip.track]);
            let track = rows
                .iter()
                .find(|(_, rect)| rect.y_range().contains(pointer.y))
                .map(|(row, _)| row.index)
                .filter(|index| std::mem::discriminant(&project.tracks()[*index]) == origin_kind)
                .unwrap_or(drag.clip.track);
            Some(Ghost {
                track,
                start,
                end: start + (drag.end - drag.start),
            })
        }
        ClipMode::TrimEnd => edit::trim_end_target(
            project,
            drag.clip,
            raw.round() as u64,
            timeline.grid,
            snap_on,
        )
        .map(|end| Ghost {
            track: drag.clip.track,
            start: drag.start,
            end,
        }),
        ClipMode::TrimStart => edit::trim_start_target(
            project,
            drag.clip,
            raw.round() as u64,
            timeline.grid,
            snap_on,
        )
        .map(|start| Ghost {
            track: drag.clip.track,
            start,
            end: drag.end,
        }),
    }
}

fn commit(drag: ClipDrag, ghost: Ghost, actions: &mut Vec<Action>) {
    let clip = drag.clip;
    match drag.mode {
        ClipMode::Move if drag.copy => {
            actions.push(Action::CopyClipTo {
                clip,
                to_track: ghost.track,
                start: ghost.start,
            });
        }
        ClipMode::Move if ghost.start != drag.start || ghost.track != clip.track => {
            actions.push(Action::MoveClip {
                clip,
                to_track: ghost.track,
                start: ghost.start,
            });
        }
        ClipMode::TrimEnd if ghost.end != drag.end => {
            actions.push(Action::TrimClipEnd {
                clip,
                end: ghost.end,
            });
        }
        ClipMode::TrimStart if ghost.start != drag.start => {
            actions.push(Action::TrimClipStart {
                clip,
                start: ghost.start,
            });
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_waveform(
    painter: &egui::Painter,
    rect: Rect,
    visible_x: std::ops::RangeInclusive<f32>,
    clip: &AudioClip,
    overview: &crate::ui::waveform::Overview,
    buffer: &sympoie::project::AudioBuffer,
    timeline: &Timeline,
    bpm: f64,
    color: Color32,
) {
    if rect.height() < 4.0 {
        return;
    }
    let ticks_per_frame = bpm / 3000.0;
    let stereo = rect.height() >= 50.0;
    let gain = clip.gain.abs().min(4.0);
    let invert = clip.gain < 0.0;
    let mut mesh = Mesh::default();
    let mut x = visible_x.start().floor();
    let end_x = *visible_x.end();
    let frame_at = |x: f32| -> u64 {
        let tick = timeline.tick(x) - clip.start_tick as f64;
        clip.source_start + (tick / ticks_per_frame).max(0.0) as u64
    };
    let limit = clip.source_start + clip.frame_count;
    let half = |lane: Rect, low: f32, high: f32| {
        let (low, high) = if invert { (-high, -low) } else { (low, high) };
        let center = lane.center().y;
        let scale = lane.height() * 0.5;
        let top = center - (high * gain).clamp(-1.0, 1.0) * scale;
        let bottom = center - (low * gain).clamp(-1.0, 1.0) * scale;
        (top.min(center - 0.5), bottom.max(center + 0.5))
    };
    while x < end_x {
        let from = frame_at(x).min(limit);
        let to = frame_at(x + 1.0).clamp(from + 1, limit.max(from + 1));
        if let Some(extent) = overview.extent(buffer.samples(), from, to) {
            if stereo {
                let upper = Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y));
                let lower = Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.max);
                for (lane, low, high) in
                    [(upper, extent[0], extent[1]), (lower, extent[2], extent[3])]
                {
                    let (top, bottom) = half(lane.shrink2(vec2(0.0, 1.0)), low, high);
                    mesh.add_colored_rect(Rect::from_x_y_ranges(x..=x + 1.0, top..=bottom), color);
                }
            } else {
                let (top, bottom) = half(rect, extent[0].min(extent[2]), extent[1].max(extent[3]));
                mesh.add_colored_rect(Rect::from_x_y_ranges(x..=x + 1.0, top..=bottom), color);
            }
        }
        x += 1.0;
    }
    if stereo {
        painter.hline(
            rect.x_range(),
            rect.center().y,
            Stroke::new(1.0, color.gamma_multiply(0.25)),
        );
    }
    painter.add(Shape::mesh(mesh));
}

fn paint_notes(
    painter: &egui::Painter,
    rect: Rect,
    clip: &MidiClip,
    timeline: &Timeline,
    color: Color32,
) {
    paint_bend(painter, rect, clip, timeline, color);
    let notes: Vec<_> = clip.audible_notes().collect();
    let (Some(low), Some(high)) = (
        notes.iter().map(|note| note.key).min(),
        notes.iter().map(|note| note.key).max(),
    ) else {
        return;
    };
    let low = low.saturating_sub(1);
    let high = high.saturating_add(1).min(127);
    let span = f32::from(high - low).max(6.0);
    let row = (rect.height() / (span + 1.0)).clamp(1.5, 6.0);
    let visible = painter.clip_rect().x_range();
    for note in &notes {
        let x0 = timeline.x((clip.start_tick + note.start_tick) as f64);
        let x1 = timeline
            .x((clip.start_tick + note.start_tick + note.duration_ticks) as f64)
            .max(x0 + 1.5);
        if x1 < visible.min || x0 > visible.max {
            continue;
        }
        let y = egui::lerp(
            rect.bottom() - row..=rect.top(),
            f32::from(note.key - low) / span,
        );
        let alpha = 0.55 + 0.45 * f32::from(note.velocity) / 127.0;
        painter.rect_filled(
            Rect::from_x_y_ranges(x0..=x1, y..=y + row),
            0.0,
            color.gamma_multiply(alpha),
        );
    }
}

/// The clip's pitch bend as a faint line around the clip's middle.
fn paint_bend(
    painter: &egui::Painter,
    rect: Rect,
    clip: &MidiClip,
    timeline: &Timeline,
    color: Color32,
) {
    let points = clip.audible_bend();
    if points.iter().all(|point| point.value == 0) {
        return;
    }
    let half = rect.height() * 0.4;
    let y = |value: i16| rect.center().y - sympoie::clip::bend_normalized(value) * half;
    let x = |tick: u64| timeline.x((clip.start_tick + tick) as f64);
    let end = x(clip.length_ticks);
    let mut line = vec![pos2(x(0), y(points[0].value))];
    line.extend(
        points
            .iter()
            .map(|point| pos2(x(point.tick), y(point.value))),
    );
    line.push(pos2(end, y(points[points.len() - 1].value)));
    for point in &mut line {
        point.x = point.x.clamp(x(0), end);
    }
    painter.add(Shape::line(
        line,
        Stroke::new(1.0, color.gamma_multiply(0.45)),
    ));
}

fn kind_name(track: &Track) -> &'static str {
    match track {
        Track::Audio(_) => "Audio",
        Track::Midi(_) => "MIDI",
        Track::Group(_) => "Group",
        Track::Bus(_) => "Bus",
        Track::Master(_) => "Master",
    }
}

fn track_drop_target(
    project: &Project,
    source: usize,
    row_rects: &[(Row, Rect)],
    pointer: Pos2,
) -> Option<edit::TrackDrop> {
    if source == 0 || source >= project.tracks().len() {
        return None;
    }
    let (row, rect) = row_rects
        .iter()
        .find(|(_, rect)| rect.y_range().contains(pointer.y))
        .or_else(|| row_rects.first().filter(|(_, rect)| pointer.y < rect.top()))
        .or_else(|| {
            row_rects
                .last()
                .filter(|(_, rect)| pointer.y > rect.bottom())
        })?;
    let edge = 5.0_f32.min(rect.height() / 4.0);
    let target = if matches!(project.tracks()[row.index], Track::Group(_))
        && pointer.y > rect.top() + edge
        && pointer.y < rect.bottom() - edge
    {
        edit::TrackDrop::IntoGroup(row.index)
    } else {
        let parents = rows::parents(project);
        let mut sibling = row.index;
        while parents[sibling] != parents[source] {
            sibling = parents[sibling]?;
        }
        if sibling != row.index || pointer.y >= rect.center().y {
            edit::TrackDrop::After(sibling)
        } else {
            edit::TrackDrop::Before(sibling)
        }
    };
    edit::can_drop_track(project, source, target).then_some(target)
}

fn update_track_drag(
    ui: &mut Ui,
    project: &Project,
    row_rects: &[(Row, Rect)],
    viewport: Rect,
    interaction: &mut Interaction,
    actions: &mut Vec<Action>,
) {
    let Some(source) = interaction.track_drag else {
        return;
    };
    if ui.input(|input| input.key_pressed(Key::Escape)) {
        interaction.track_drag = None;
        return;
    }
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let target = pointer
        .filter(|pointer| viewport.contains(*pointer))
        .and_then(|pointer| track_drop_target(project, source, row_rects, pointer));
    ui.ctx().set_cursor_icon(if target.is_some() {
        CursorIcon::Grabbing
    } else {
        CursorIcon::NotAllowed
    });
    if let Some(target) = target {
        let painter = ui.painter().with_clip_rect(viewport);
        match target {
            edit::TrackDrop::IntoGroup(index) => {
                if let Some((_, rect)) = row_rects.iter().find(|(row, _)| row.index == index) {
                    let header = rect
                        .with_min_x(viewport.left())
                        .with_max_x(viewport.right());
                    painter.rect_filled(header, 0.0, theme::ACCENT.gamma_multiply(0.18));
                    painter.rect_stroke(
                        header.shrink(1.0),
                        0.0,
                        Stroke::new(2.0, theme::ACCENT),
                        StrokeKind::Inside,
                    );
                }
            }
            edit::TrackDrop::Before(index) | edit::TrackDrop::After(index) => {
                let before = matches!(target, edit::TrackDrop::Before(_));
                let parents = rows::parents(project);
                let row = if before {
                    row_rects.iter().find(|(row, _)| row.index == index)
                } else {
                    row_rects.iter().rev().find(|(row, _)| {
                        row.index == index || rows::is_descendant(&parents, row.index, index)
                    })
                };
                if let Some((row, rect)) = row {
                    let y = if before { rect.top() } else { rect.bottom() };
                    let depth = row_rects
                        .iter()
                        .find(|(row, _)| row.index == index)
                        .map_or(row.depth, |(row, _)| row.depth);
                    let x = viewport.left() + depth as f32 * 10.0;
                    painter.hline(x..=viewport.right(), y, Stroke::new(3.0, theme::ACCENT));
                }
            }
        }
    }
    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
        interaction.track_drag = None;
        if let Some(target) = target {
            actions.push(Action::MoveTrack {
                track: source,
                target,
            });
        }
    } else if let Some(pointer) = pointer.filter(|pointer| viewport.contains(*pointer)) {
        let edge = 24.0;
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

#[allow(clippy::too_many_arguments)]
fn track_header(
    ui: &mut Ui,
    rect: Rect,
    row: Row,
    state: &mut ArrangementState,
    inputs: &Inputs<'_>,
    meter: Option<&mut widgets::MeterState>,
    interaction: &mut Interaction,
    actions: &mut Vec<Action>,
) {
    let project = inputs.project;
    let track = &project.tracks()[row.index];
    let selected = inputs.selection.tracks.contains(&row.index);
    let color = theme::track_color(project, row.index);
    let painter = ui.painter_at(rect);
    let (top, bottom) = if selected {
        (
            theme::tint(theme::HEADER_SELECTED, color, 0.08),
            theme::tint(theme::HEADER, color, 0.05),
        )
    } else {
        (
            theme::HEADER,
            theme::tint(theme::HEADER, theme::BG_PANEL, 0.7),
        )
    };
    analog::gradient_rect(&painter, rect, top, bottom);
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    let indent = row.depth as f32 * 10.0;
    if row.depth > 0 {
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(indent, rect.height())),
            0.0,
            theme::BG_PANEL,
        );
        painter.vline(
            rect.left() + indent - 4.5,
            rect.y_range(),
            Stroke::new(1.0, theme::HAIRLINE),
        );
    }

    let background = ui.interact(rect, Id::new(("header", row.index)), Sense::click());
    if background.double_clicked() {
        open_track_devices(row.index, actions);
    } else if background.clicked() {
        select_header(ui, row.index, inputs.selection, actions);
    }
    background
        .context_menu(|ui| track_menu(ui, project, row.index, inputs.selection, state, actions));

    let title = Rect::from_min_max(
        rect.min + vec2(indent, 0.0),
        pos2(rect.right(), rect.top() + TITLE_HEIGHT),
    );
    let title_fill = if selected {
        theme::tint(color, Color32::WHITE, 0.2)
    } else {
        color
    };
    widgets::color_bar(&painter, title, title_fill, "");
    if selected {
        painter.vline(
            title.left() + 4.5,
            (title.top() + 1.0)..=(rect.bottom() - 1.0),
            Stroke::new(1.0, title_fill.gamma_multiply(0.6)),
        );
    }
    let foldable = row.has_children || matches!(track, Track::Group(_));
    let name_left = if foldable {
        let folded = state.folded.contains(&row.index);
        let button = Rect::from_min_size(title.min + vec2(4.0, 0.0), vec2(16.0, title.height()));
        if fold_button(ui, button, row.index, folded).clicked() {
            actions.push(Action::ToggleFold(row.index));
        }
        button.right() - 3.0
    } else {
        title.left() + 4.0
    };
    let mute = Rect::from_min_max(
        pos2(title.right() - 40.0, title.top() + 1.0),
        title.max - vec2(22.0, 1.0),
    );
    mute_button(ui, mute, row.index, track, actions);
    solo_button(
        ui,
        mute.translate(vec2(20.0, 0.0)),
        row.index,
        track,
        actions,
    );
    let controls_left = if matches!(track, Track::Audio(_) | Track::Midi(_)) {
        let arm = mute.translate(vec2(-20.0, 0.0));
        arm_button(ui, arm, row.index, track, inputs, actions);
        arm.left()
    } else {
        mute.left()
    };
    let name_rect = Rect::from_min_max(
        pos2(name_left, title.top()),
        pos2(controls_left - 2.0, title.bottom()),
    );
    let renaming = state
        .renaming
        .as_ref()
        .is_some_and(|(index, _)| *index == row.index);
    if renaming {
        let mut commit = None;
        widgets::inside(
            ui,
            name_rect.shrink2(vec2(2.0, 0.0)),
            Layout::left_to_right(Align::Center),
            |ui| {
                if let Some((_, text)) = &mut state.renaming {
                    let edit = ui.add(
                        egui::TextEdit::singleline(text).desired_width(name_rect.width() - 8.0),
                    );
                    if !edit.has_focus() && !edit.lost_focus() {
                        edit.request_focus();
                    }
                    if edit.lost_focus() {
                        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        commit = Some((!cancelled).then(|| text.clone()));
                    }
                }
            },
        );
        if let Some(result) = commit {
            if let Some(name) = result {
                actions.push(Action::RenameTrack(row.index, name));
            }
            state.renaming = None;
        }
    } else {
        widgets::fitted_text(
            &painter,
            name_rect.shrink2(vec2(5.0, 0.0)),
            &rows::track_label(project, row.index),
            theme::prop(11.0),
            if selected {
                Color32::WHITE
            } else {
                theme::TEXT
            },
            Align2::LEFT_CENTER,
        );
        let name = ui
            .interact(
                name_rect,
                Id::new(("track-name", row.index)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::Grab)
            .on_hover_text(format!(
                "{}\nDrag to reorder · Double-click to show its Rack · Right-click to rename",
                track.name()
            ));
        if name.drag_started_by(egui::PointerButton::Primary) {
            interaction.track_drag = Some(row.index);
        }
        if name.double_clicked() {
            open_track_devices(row.index, actions);
        } else if name.clicked() {
            select_header(ui, row.index, inputs.selection, actions);
        }
        name.context_menu(|ui| {
            track_menu(ui, project, row.index, inputs.selection, state, actions)
        });
    }

    let control_indent = indent.min(30.0);
    if rect.height() >= TITLE_HEIGHT + 22.0 && matches!(track, Track::Audio(_) | Track::Midi(_)) {
        let input_rect = Rect::from_min_max(
            pos2(rect.left() + control_indent + 4.0, title.bottom() + 2.0),
            pos2(rect.left() + control_indent + 41.0, title.bottom() + 20.0),
        );
        recording_inputs::selector(ui, input_rect, row.index, track, inputs, actions);
    } else if rect.height() >= TITLE_HEIGHT + 22.0 {
        painter.text(
            pos2(
                rect.left() + control_indent + 6.0,
                if rect.height() >= 56.0 {
                    title.bottom() + 11.0
                } else {
                    rect.bottom() - 11.5
                },
            ),
            Align2::LEFT_CENTER,
            kind_name(track),
            theme::prop(10.0),
            theme::TEXT_DIM,
        );
    }
    if rect.height() >= 56.0 {
        let controls = Rect::from_min_max(
            pos2(rect.left() + control_indent + 44.0, title.bottom() + 2.0),
            pos2(rect.right() - 4.0, title.bottom() + 24.0),
        );
        widgets::inside(ui, controls, Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            mixer_controls(ui, project, row.index, actions);
        });
    }
    if state.automation {
        automation::selector(ui, rect, indent, row.index, state, project, actions);
    }
    if let Some(meter) = meter {
        header_meter(ui, rect, indent, row.index, track.name(), meter);
    }
    track_resize_handle(ui, rect, row.index, state, interaction);
}

fn track_resize_handle(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    state: &mut ArrangementState,
    interaction: &mut Interaction,
) {
    let edge = rect.with_min_y(rect.bottom() - 3.0);
    let response = ui
        .interact(
            edge,
            Id::new(("track-height-resize", index)),
            Sense::click_and_drag(),
        )
        .on_hover_cursor(CursorIcon::ResizeVertical)
        .on_hover_text("Drag to resize this track · Double-click to restore its height");
    if response.double_clicked() {
        state.track_heights.remove(&index);
        interaction.track_resize = None;
    } else if interaction.track_resize.is_none()
        && response.is_pointer_button_down_on()
        && ui.input(|input| input.pointer.primary_down())
    {
        interaction.track_resize = Some(TrackResize {
            index,
            start_y: ui
                .input(|input| input.pointer.press_origin())
                .map_or(rect.bottom(), |pos| pos.y),
            start_height: rect.height(),
            previous_height: state.track_heights.get(&index).copied(),
        });
        interaction.track_drag = None;
    } else if interaction.track_resize.is_none()
        && let Some((start, end)) = ui.input(|input| {
            if !input.focused
                || input.key_pressed(Key::Escape)
                || !input.pointer.primary_pressed()
                || !input.pointer.primary_released()
                || input.pointer.primary_down()
            {
                return None;
            }
            let mut press = None;
            for event in &input.events {
                if let egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    ..
                } = event
                {
                    if *pressed {
                        press = Some(*pos);
                    } else if let Some(start) = press.take()
                        && edge.intersect(ui.clip_rect()).contains(start)
                        && (pos.y - start.y).abs() > 3.0
                    {
                        return Some((start, *pos));
                    }
                }
            }
            None
        })
        && ui.ctx().layer_id_at(start) == Some(ui.layer_id())
    {
        let minimum = if state.automation { 92.0 } else { 46.0 };
        state.set_track_height(index, (rect.height() + end.y - start.y).max(minimum));
        interaction.track_drag = None;
        ui.ctx().request_repaint();
    }
    if response.hovered() || response.dragged() {
        ui.painter().hline(
            edge.x_range(),
            rect.bottom() - 1.0,
            Stroke::new(2.0, theme::SELECTION),
        );
    }
}

fn update_track_resize(ui: &Ui, state: &mut ArrangementState, interaction: &mut Interaction) {
    let Some(resize) = interaction.track_resize else {
        return;
    };
    let (cancelled, pointer, down) = ui.input(|input| {
        (
            !input.focused || input.key_pressed(Key::Escape),
            input.pointer.latest_pos(),
            input.pointer.primary_down(),
        )
    });
    if cancelled {
        restore_track_height(state, resize);
        interaction.track_resize = None;
        ui.ctx().stop_dragging();
        return;
    }
    if let Some(pointer) = pointer {
        let minimum = if state.automation { 92.0 } else { 46.0 };
        let height = (resize.start_height + pointer.y - resize.start_y).max(minimum);
        if (height - resize.start_height).abs() < 0.01 {
            restore_track_height(state, resize);
        } else {
            state.set_track_height(resize.index, height);
        }
    }
    if !down {
        interaction.track_resize = None;
    }
}

fn restore_track_height(state: &mut ArrangementState, resize: TrackResize) {
    if let Some(height) = resize.previous_height {
        state.track_heights.insert(resize.index, height);
    } else {
        state.track_heights.remove(&resize.index);
    }
}

fn open_track_devices(index: usize, actions: &mut Vec<Action>) {
    actions.push(Action::SelectClip(None));
    actions.push(Action::SelectTracks(BTreeSet::from([index])));
    actions.push(Action::ShowDevices);
}

fn header_meter(
    ui: &mut Ui,
    rect: Rect,
    indent: f32,
    index: usize,
    name: &str,
    meter: &mut widgets::MeterState,
) {
    let full = rect.height() >= 56.0;
    let indent = indent.min(30.0);
    let meter_top = if full {
        (rect.bottom() - 16.0).max(rect.top() + 44.0)
    } else {
        rect.bottom() - 16.0
    };
    let meter_rect = Rect::from_min_max(
        pos2(rect.left() + indent + 44.0, meter_top),
        pos2(rect.right() - 6.0, meter_top + 9.0),
    );
    if full {
        ui.painter_at(rect).text(
            pos2(rect.left() + indent + 6.0, meter_rect.center().y),
            Align2::LEFT_CENTER,
            "Peak",
            theme::prop(9.0),
            theme::TEXT_FAINT,
        );
    }
    widgets::meter_at(
        ui,
        meter,
        meter_rect,
        Id::new(("track-meter", index)),
        false,
    )
    .on_hover_text(format!("{name} · output after effects, volume and pan"));
}

fn select_header(ui: &Ui, index: usize, selection: &Selection, actions: &mut Vec<Action>) {
    let modifiers = ui.input(|i| i.modifiers);
    let mut tracks = selection.tracks.clone();
    tracks.remove(&0);
    if modifiers.shift || modifiers.command {
        if !tracks.remove(&index) {
            tracks.insert(index);
        }
    } else {
        tracks = BTreeSet::from([index]);
    }
    actions.push(Action::SelectClip(None));
    actions.push(Action::SelectTracks(tracks));
}

fn fold_button(ui: &mut Ui, rect: Rect, index: usize, folded: bool) -> Response {
    let response = ui.interact(rect, Id::new(("fold", index)), Sense::click());
    let c = rect.center();
    let color = if response.hovered() {
        Color32::WHITE
    } else {
        theme::TEXT_DIM
    };
    let points = if folded {
        vec![
            pos2(c.x - 2.5, c.y - 4.0),
            pos2(c.x + 3.5, c.y),
            pos2(c.x - 2.5, c.y + 4.0),
        ]
    } else {
        vec![
            pos2(c.x - 4.0, c.y - 2.5),
            pos2(c.x + 4.0, c.y - 2.5),
            pos2(c.x, c.y + 3.5),
        ]
    };
    ui.painter()
        .add(Shape::convex_polygon(points, color, Stroke::NONE));
    response.on_hover_text(if folded {
        "Show members"
    } else {
        "Hide members"
    })
}

fn track_menu(
    ui: &mut Ui,
    project: &Project,
    index: usize,
    selection: &Selection,
    state: &mut ArrangementState,
    actions: &mut Vec<Action>,
) {
    if ui
        .button(if project.tracks()[index].mixer().muted() {
            "Unmute Track"
        } else {
            "Mute Track"
        })
        .clicked()
    {
        let mixer = *project.tracks()[index].mixer();
        actions.push(Action::SetMixer {
            track: index,
            mixer: mixer.with_muted(!mixer.muted()),
        });
        ui.close();
    }
    if ui.button("Bounce Track in Place").clicked() {
        actions.push(Action::BounceTrack {
            track: index,
            duplicate: false,
        });
        ui.close();
    }
    if ui.button("Duplicate and Bounce Track").clicked() {
        actions.push(Action::BounceTrack {
            track: index,
            duplicate: true,
        });
        ui.close();
    }
    ui.separator();
    if ui.button("Rename").clicked() {
        state.renaming = Some((index, project.tracks()[index].name().to_owned()));
        ui.close();
    }
    ui.menu_button("Change Color", |ui| {
        color_palette(ui, project, index, selection, actions);
    });
    ui.separator();
    let mut group = selection.tracks.clone();
    group.insert(index);
    let label = if group.len() > 1 {
        format!("Group {} Tracks", group.len())
    } else {
        "Group Track".to_owned()
    };
    if ui.button(label).clicked() {
        actions.push(Action::SelectTracks(group));
        actions.push(Action::GroupSelected);
    }
    if matches!(project.tracks()[index], Track::Group(_)) && ui.button("Ungroup").clicked() {
        actions.push(Action::Ungroup(index));
    }
    if rows::parents(project)[index].is_some() && ui.button("Exit Group").clicked() {
        actions.push(Action::ExitGroup(index));
        ui.close();
    }
    ui.separator();
    for (kind, label) in [
        (TrackKind::Audio, "Insert Audio Track"),
        (TrackKind::Midi, "Insert MIDI Track"),
        (TrackKind::Group, "Insert Group Track"),
    ] {
        if ui.button(label).clicked() {
            actions.push(Action::AddTrack(kind));
        }
    }
    ui.separator();
    let delete = if matches!(project.tracks()[index], Track::Group(_)) {
        "Delete Group and Members"
    } else {
        "Delete"
    };
    if ui
        .button(RichText::new(delete).color(theme::ERROR))
        .clicked()
    {
        actions.push(Action::DeleteTrack(index));
    }
}

fn paint_recording(painter: &egui::Painter, lane: Rect, timeline: &Timeline, start: u64, end: f64) {
    let left = timeline.x(start as f64);
    let right = timeline.x(end.max(start as f64)).max(left + 2.0);
    let rect = Rect::from_min_max(
        pos2(left, lane.top() + 2.0),
        pos2(right, lane.bottom() - 2.0),
    );
    let painter = painter.with_clip_rect(painter.clip_rect().intersect(lane));
    painter.rect_filled(rect, 2.0, theme::ERROR.gamma_multiply(0.26));
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, theme::ERROR),
        StrokeKind::Inside,
    );
    widgets::fitted_text(
        &painter,
        rect.intersect(lane).shrink2(vec2(5.0, 0.0)),
        "Recording…",
        theme::prop(10.5),
        theme::TEXT,
        Align2::LEFT_TOP,
    );
}

fn arm_button(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    track: &Track,
    inputs: &Inputs<'_>,
    actions: &mut Vec<Action>,
) {
    let armed = inputs.armed_tracks.contains(&index);
    let response = ui.interact(rect, Id::new(("track-arm", index)), Sense::click());
    let label = format!(
        "{} {} for recording",
        if armed { "Disarm" } else { "Arm" },
        track.name(),
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, armed, &label));
    let painter = ui.painter().with_clip_rect(rect.expand2(vec2(3.0, 1.0)));
    let cap = analog::push_button(
        &painter,
        rect.shrink2(vec2(1.0, 1.5)),
        2.5,
        armed.then_some(theme::ERROR),
        response.hovered(),
        response.is_pointer_button_down_on(),
    );
    widgets::paint_icon(
        &painter,
        cap.shrink2(vec2(3.0, 1.0)),
        widgets::Icon::Record,
        if armed {
            theme::TEXT_ON_COLOR
        } else {
            theme::ERROR
        },
    );
    if armed && inputs.recording {
        painter.rect_stroke(cap, 2.0, Stroke::new(1.0, theme::TEXT), StrokeKind::Inside);
    }
    if response.clicked() {
        actions.push(Action::ToggleTrackArm(index));
    }
    response.on_hover_text(format!(
        "{label}\n{} · Press Q to start recording",
        if matches!(track, Track::Midi(_)) {
            "MIDI input"
        } else {
            "Audio input"
        },
    ));
}

fn mute_button(ui: &mut Ui, rect: Rect, index: usize, track: &Track, actions: &mut Vec<Action>) {
    let muted = track.mixer().muted();
    let response = ui.interact(rect, Id::new(("track-mute", index)), Sense::click());
    let painter = ui.painter().with_clip_rect(rect.expand2(vec2(3.0, 1.0)));
    let cap = analog::push_button(
        &painter,
        rect.shrink2(vec2(1.0, 1.5)),
        2.5,
        muted.then_some(theme::WARN),
        response.hovered(),
        response.is_pointer_button_down_on(),
    );
    analog::legend(&painter, cap, "M", theme::prop(9.5), muted);
    if response.clicked() {
        actions.push(Action::SetMixer {
            track: index,
            mixer: track.mixer().with_muted(!muted),
        });
    }
    response.on_hover_text(if muted { "Unmute track" } else { "Mute track" });
}

fn solo_button(ui: &mut Ui, rect: Rect, index: usize, track: &Track, actions: &mut Vec<Action>) {
    let soloed = track.mixer().soloed();
    let response = ui.interact(rect, Id::new(("track-solo", index)), Sense::click());
    let painter = ui.painter().with_clip_rect(rect.expand2(vec2(3.0, 1.0)));
    let cap = analog::push_button(
        &painter,
        rect.shrink2(vec2(1.0, 1.5)),
        2.5,
        soloed.then_some(theme::ACCENT),
        response.hovered(),
        response.is_pointer_button_down_on(),
    );
    analog::legend(&painter, cap, "S", theme::prop(9.5), soloed);
    if response.clicked() {
        actions.push(Action::SetSolo {
            track: index,
            soloed: !soloed,
        });
    }
    response.on_hover_text(if soloed { "Unsolo track" } else { "Solo track" });
}

fn color_palette(
    ui: &mut Ui,
    project: &Project,
    index: usize,
    selection: &Selection,
    actions: &mut Vec<Action>,
) {
    const SWATCH: f32 = 18.0;
    const GAP: f32 = 3.0;
    const NEUTRAL_GAP: f32 = 8.0;
    let tracks: BTreeSet<usize> = if selection.tracks.contains(&index) {
        selection
            .tracks
            .iter()
            .copied()
            .filter(|track| *track != 0)
            .collect()
    } else {
        BTreeSet::from([index])
    };
    if tracks.len() > 1 {
        ui.label(
            RichText::new(format!("Color {} Tracks", tracks.len()))
                .size(10.5)
                .color(theme::TEXT_DIM),
        );
    }
    let current = project.tracks()[index].color();
    let columns = theme::PALETTE_COLUMNS;
    let rows = theme::PALETTE.len().div_ceil(columns);
    let size = vec2(
        columns as f32 * (SWATCH + GAP) - GAP + NEUTRAL_GAP,
        rows as f32 * (SWATCH + GAP) - GAP,
    );
    let (area, _) = ui.allocate_exact_size(size, Sense::hover());
    let neutral_start = (columns - 1) as f32 * (SWATCH + GAP) + NEUTRAL_GAP;
    ui.painter().vline(
        area.left() + neutral_start - (GAP + NEUTRAL_GAP) / 2.0,
        area.y_range(),
        Stroke::new(1.0, theme::HAIRLINE),
    );
    for (slot, &rgb) in theme::PALETTE.iter().enumerate() {
        let column = slot % columns;
        let min = area.min
            + vec2(
                column as f32 * (SWATCH + GAP)
                    + if column == columns - 1 {
                        NEUTRAL_GAP
                    } else {
                        0.0
                    },
                (slot / columns) as f32 * (SWATCH + GAP),
            );
        let swatch = Rect::from_min_size(min, Vec2::splat(SWATCH));
        let response = ui
            .interact(swatch, Id::new(("track-color", slot)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("#{rgb:06X}"));
        let painter = ui.painter();
        painter.rect_filled(swatch, 1.0, theme::rgb(rgb));
        if current == Some(rgb) {
            painter.rect_stroke(
                swatch,
                1.0,
                Stroke::new(1.0, Color32::BLACK),
                StrokeKind::Inside,
            );
            painter.rect_stroke(
                swatch.shrink(1.0),
                0.0,
                Stroke::new(1.0, Color32::WHITE),
                StrokeKind::Inside,
            );
        } else if response.hovered() {
            painter.rect_stroke(
                swatch,
                1.0,
                Stroke::new(1.0, Color32::WHITE),
                StrokeKind::Outside,
            );
        }
        if response.clicked() {
            actions.push(Action::SetTrackColor {
                tracks: tracks.clone(),
                color: Some(rgb),
            });
            ui.close();
        }
    }
    ui.add_space(2.0);
    if ui
        .add_enabled(
            tracks
                .iter()
                .any(|track| project.tracks()[*track].color().is_some()),
            egui::Button::new("Default Color"),
        )
        .on_hover_text("Use the colour given by the track's position")
        .clicked()
    {
        actions.push(Action::SetTrackColor {
            tracks,
            color: None,
        });
        ui.close();
    }
}

fn empty_state(ui: &mut Ui, area: Rect, actions: &mut Vec<Action>) {
    if area.height() < 90.0 || area.width() < 320.0 {
        return;
    }
    let block = Rect::from_center_size(area.center(), vec2(320.0, 70.0));
    ui.painter_at(area).text(
        block.center_top() + vec2(0.0, 8.0),
        Align2::CENTER_TOP,
        "Drop audio files or devices here",
        theme::prop(14.0),
        theme::TEXT_DIM,
    );
    ui.painter_at(area).text(
        block.center_top() + vec2(0.0, 30.0),
        Align2::CENTER_TOP,
        "WAV, AIFF, FLAC or MP3 files",
        theme::prop(10.5),
        theme::TEXT_FAINT,
    );
    widgets::inside(
        ui,
        Rect::from_min_max(pos2(block.left(), block.bottom() - 18.0), block.max),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(16.0);
            if ui.button("Import Audio…").clicked() {
                actions.push(Action::ChooseAudioToImport {
                    track: None,
                    start: None,
                });
            }
            if ui.button("Insert Audio Track").clicked() {
                actions.push(Action::AddTrack(TrackKind::Audio));
            }
            if ui.button("Open…").clicked() {
                actions.push(Action::OpenProject);
            }
        },
    );
}

fn master_row(
    ui: &mut Ui,
    rect: Rect,
    timeline: &Timeline,
    inputs: &Inputs<'_>,
    state: &mut ArrangementState,
    meter: &mut widgets::MeterState,
    actions: &mut Vec<Action>,
) {
    let project = inputs.project;
    let header = rect.with_max_x(timeline.left);
    let lane = rect.with_min_x(timeline.left);
    let selected = inputs.selection.tracks.contains(&0);
    let response = ui.interact(header, Id::new(("header", 0)), Sense::click());
    if response.double_clicked() {
        open_track_devices(0, actions);
    } else if response.clicked() {
        actions.push(Action::SelectTracks(BTreeSet::from([0])));
    }
    let painter = ui.painter_at(rect);
    analog::gradient_rect(
        &painter,
        header,
        if selected {
            theme::HEADER_SELECTED
        } else {
            theme::HEADER
        },
        theme::tint(theme::HEADER, theme::BG_PANEL, 0.7),
    );
    painter.rect_filled(lane, 0.0, theme::LANE);
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(2.0, theme::BORDER),
    );
    let title = Rect::from_min_max(
        header.min + vec2(0.0, 1.0),
        pos2(header.right(), header.top() + TITLE_HEIGHT + 1.0),
    );
    widgets::color_bar(
        &painter,
        title,
        if selected {
            theme::tint(theme::MASTER_COLOR, Color32::WHITE, 0.3)
        } else {
            theme::MASTER_COLOR
        },
        &rows::track_label(project, 0),
    );
    mute_button(
        ui,
        Rect::from_min_max(
            pos2(title.right() - 40.0, title.top() + 1.0),
            title.max - vec2(22.0, 1.0),
        ),
        0,
        &project.tracks()[0],
        actions,
    );
    solo_button(
        ui,
        Rect::from_min_max(
            pos2(title.right() - 20.0, title.top() + 1.0),
            title.max - vec2(2.0, 1.0),
        ),
        0,
        &project.tracks()[0],
        actions,
    );
    let controls = Rect::from_min_max(
        pos2(header.left() + 44.0, title.bottom() + 1.0),
        pos2(header.right() - 4.0, title.bottom() + 23.0),
    );
    widgets::inside(ui, controls, Layout::left_to_right(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        mixer_controls(ui, project, 0, actions);
    });
    if state.automation {
        automation::selector(ui, header, 0.0, 0, state, project, actions);
    }
    header_meter(ui, header, 0.0, 0, "Master", meter);
    let end = project.duration_frames().unwrap_or(0) as f64 / f64::from(sympoie::SAMPLE_RATE);
    if end > 0.0 && !state.automation {
        painter.text(
            lane.left_center() + vec2(10.0, 0.0),
            Align2::LEFT_CENTER,
            format!("Content length {}", format::duration(end)),
            theme::prop(10.5),
            theme::TEXT_FAINT,
        );
    }
}

#[cfg(test)]
#[path = "arrangement_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "arrangement_navigation_tests.rs"]
mod navigation_tests;
