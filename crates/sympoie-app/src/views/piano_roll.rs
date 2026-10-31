use std::collections::BTreeSet;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, Grid, Id, Key, KeyboardShortcut,
    Layout, Mesh, Modifiers, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder,
    pos2, vec2,
};
use sympoie::TICKS_PER_QUARTER;
use sympoie::clip::{BEND_MAX, BEND_MIN, BendPoint, MidiNote, bend_value_at, thin_bend};
use sympoie::project::Project;
use sympoie::time::{SnapGrid, TimeSignature};

use crate::actions::Action;
use crate::document::edit::{self, ClipRef};
use crate::ui::analog;
use crate::ui::bars;
use crate::ui::format;
use crate::ui::rack;
use crate::ui::theme;
use crate::ui::widgets;

const IVORY_BACK: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
const IVORY_FRONT: Color32 = Color32::from_rgb(0xec, 0xec, 0xec);
const KEY_SEAM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9c);
const EBONY_TOP: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2b);
const EBONY_BOTTOM: Color32 = Color32::from_rgb(0x11, 0x11, 0x12);
const EBONY_RADIUS: CornerRadius = CornerRadius {
    nw: 0,
    ne: 3,
    sw: 0,
    se: 3,
};
/// Width of the caption column in the side panel, so the values beside it line up.
const LABEL_WIDTH: f32 = 66.0;

const KEY_WIDTH: f32 = 62.0;
const SIDE_WIDTH: f32 = 172.0;
const RULER_HEIGHT: f32 = 24.0;
const VELOCITY_HEIGHT: f32 = 48.0;
const VELOCITY_MIN_HEIGHT: f32 = 48.0;
const VELOCITY_PADDING: f32 = 6.0;
const NOTE_GRID_MIN_HEIGHT: f32 = 48.0;
const VELOCITY_DIVIDER_HEIGHT: f32 = 7.0;
const SCROLLBAR_HEIGHT: f32 = 24.0;
const EDGE_WIDTH: f32 = 5.0;
/// Radius around a slide handle or bend point that picks it up.
const HANDLE_RADIUS: f32 = 6.0;
/// Freehand bend strokes are thinned to points that stay within this many 14-bit steps.
const BEND_STROKE_TOLERANCE: f64 = 48.0;

/// What the lane under the note grid edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lane {
    #[default]
    Velocity,
    Bend,
}

#[derive(Debug, Clone)]
enum BendDrag {
    /// Moving one breakpoint.
    Point {
        index: usize,
        original: Vec<BendPoint>,
        preview: Vec<BendPoint>,
    },
    /// Drawing freehand: `stroke` holds the raw points in drawing order.
    Draw {
        original: Vec<BendPoint>,
        stroke: Vec<BendPoint>,
        preview: Vec<BendPoint>,
    },
}

impl BendDrag {
    fn original(&self) -> &[BendPoint] {
        match self {
            Self::Point { original, .. } | Self::Draw { original, .. } => original,
        }
    }

    fn preview(&self) -> &[BendPoint] {
        match self {
            Self::Point { preview, .. } | Self::Draw { preview, .. } => preview,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragKind {
    Move,
    Start,
    End,
    Velocity,
    /// Dragging the end of a note's slide.
    Slide,
}

#[derive(Debug, Clone)]
struct NoteDrag {
    kind: DragKind,
    copy: bool,
    origin: Pos2,
    anchor: usize,
    original: Vec<MidiNote>,
    preview: Vec<MidiNote>,
}

impl NoteDrag {
    fn copied_notes(&self, selected: &BTreeSet<usize>) -> Option<Vec<MidiNote>> {
        if !self.copy || self.preview == self.original {
            return None;
        }
        let mut notes = self.original.clone();
        notes.extend(selected.iter().map(|index| self.preview[*index]));
        Some(notes)
    }
}

#[derive(Debug, Clone)]
struct Marquee {
    origin: Pos2,
    current: Pos2,
    original: BTreeSet<usize>,
    create_note_on_release: bool,
}

#[derive(Debug, Clone)]
struct NumericEdit {
    original: Vec<MidiNote>,
    preview: Vec<MidiNote>,
}

pub struct PianoRollState {
    pub clip: Option<ClipRef>,
    pub focused: bool,
    selected: BTreeSet<usize>,
    grid: SnapGrid,
    pixels_per_tick: f64,
    scroll_tick: f64,
    top_key: f32,
    row_height: f32,
    velocity_height: f32,
    default_duration: u64,
    default_velocity: u8,
    default_channel: u8,
    cursor_tick: u64,
    draw: bool,
    fit_pending: bool,
    panel_rect: Option<Rect>,
    drag: Option<NoteDrag>,
    marquee: Option<Marquee>,
    numeric: Option<NumericEdit>,
    lane: Lane,
    bend_drag: Option<BendDrag>,
    clipboard: Vec<MidiNote>,
    notice: Option<String>,
    pending_selection: Option<BTreeSet<usize>>,
    held_audition: bool,
    pending_audition_release: bool,
    last_empty_click: Option<(Pos2, f64)>,
}

impl Default for PianoRollState {
    fn default() -> Self {
        Self {
            clip: None,
            focused: false,
            selected: BTreeSet::new(),
            grid: SnapGrid::Sixteenth,
            pixels_per_tick: 0.12,
            scroll_tick: 0.0,
            top_key: 72.0,
            row_height: 16.0,
            velocity_height: VELOCITY_HEIGHT,
            default_duration: TICKS_PER_QUARTER / 4,
            default_velocity: 100,
            default_channel: 0,
            cursor_tick: 0,
            draw: false,
            fit_pending: true,
            panel_rect: None,
            drag: None,
            marquee: None,
            numeric: None,
            lane: Lane::Velocity,
            bend_drag: None,
            clipboard: Vec::new(),
            notice: None,
            pending_selection: None,
            held_audition: false,
            pending_audition_release: false,
            last_empty_click: None,
        }
    }
}

impl PianoRollState {
    pub fn open(&mut self, clip: ClipRef, project: &Project) {
        let Some(midi) = edit::midi_clip(project, clip) else {
            return;
        };
        if self.clip != Some(clip) {
            self.invalidate();
            self.scroll_tick = midi.offset_ticks as f64;
            self.cursor_tick = midi.offset_ticks;
            self.top_key = midi
                .notes
                .iter()
                .map(|note| note.key)
                .max()
                .map_or(72.0, |key| f32::from(key.saturating_add(1).min(127)));
            self.fit_pending = true;
        }
        self.clip = Some(clip);
        self.focused = true;
    }

    pub fn close(&mut self) {
        self.clip = None;
        self.focused = false;
        self.panel_rect = None;
        self.invalidate();
    }

    pub fn invalidate(&mut self) {
        self.cancel_audition();
        self.selected.clear();
        self.drag = None;
        self.bend_drag = None;
        self.marquee = None;
        self.numeric = None;
        self.notice = None;
        self.pending_selection = None;
    }

    pub fn cancel_audition(&mut self) {
        self.last_empty_click = None;
        if let Some(marquee) = &mut self.marquee {
            marquee.create_note_on_release = false;
        }
        self.release_audition();
    }

    fn release_audition(&mut self) {
        self.pending_audition_release |= self.held_audition;
        self.held_audition = false;
    }

    pub fn flush_audition_release(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let (primary_down, interrupted) = ctx.input(|input| {
            (
                input.pointer.primary_down(),
                !input.focused
                    || input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::PointerGone)),
            )
        });
        if interrupted {
            self.drag = None;
            self.marquee = None;
        }
        if interrupted || !self.focused || self.clip.is_none() {
            self.cancel_audition();
        } else if !primary_down {
            self.release_audition();
        }
        if std::mem::take(&mut self.pending_audition_release) {
            actions.push(Action::ReleaseMidiAudition);
        }
    }

    fn begin_audition(
        &mut self,
        track: usize,
        notes: impl IntoIterator<Item = MidiNote>,
        actions: &mut Vec<Action>,
    ) {
        let notes: Vec<_> = notes.into_iter().collect();
        if notes.is_empty() {
            return;
        }
        self.last_empty_click = None;
        if std::mem::take(&mut self.pending_audition_release) {
            actions.push(Action::ReleaseMidiAudition);
        }
        self.held_audition = true;
        actions.push(Action::BeginMidiAudition { track, notes });
    }

    pub fn notes_committed(&mut self) {
        if let Some(selected) = self.pending_selection.take() {
            self.selected = selected;
        }
    }

    fn emit(
        &self,
        original: &[MidiNote],
        notes: Vec<MidiNote>,
        label: &'static str,
        actions: &mut Vec<Action>,
    ) {
        if notes != original
            && let Some(clip) = self.clip
        {
            actions.push(Action::EditMidiNotes {
                clip,
                before: original.to_vec(),
                notes,
                label,
            });
        }
    }

    fn copy(&mut self, notes: &[MidiNote]) {
        self.clipboard = self.selected.iter().map(|index| notes[*index]).collect();
        if let Some(start) = self.clipboard.iter().map(|note| note.start_tick).min() {
            for note in &mut self.clipboard {
                note.start_tick -= start;
            }
        }
    }

    fn audition_selection(&self, notes: &[MidiNote], actions: &mut Vec<Action>) {
        if let Some(clip) = self.clip {
            audition(
                clip.track,
                self.selected.iter().map(|index| notes[*index]),
                actions,
            );
        }
    }

    fn copy_to_clipboard(&mut self, ctx: &egui::Context, notes: &[MidiNote]) {
        self.copy(notes);
        if !self.clipboard.is_empty() {
            ctx.copy_text(clipboard_text(&self.clipboard));
        }
    }

    fn delete(&mut self, notes: &[MidiNote], actions: &mut Vec<Action>) {
        let remaining = notes
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.selected.contains(index))
            .map(|(_, note)| *note)
            .collect();
        self.emit(notes, remaining, "Delete MIDI notes", actions);
        self.selected.clear();
    }

    fn paste(
        &mut self,
        notes: &[MidiNote],
        length: u64,
        label: &'static str,
        actions: &mut Vec<Action>,
    ) {
        let Some(span) = self.clipboard.iter().map(note_end).max() else {
            return;
        };
        if span > length {
            self.notice = Some("The copied notes are longer than this clip.".into());
            return;
        }
        let start = self.cursor_tick.min(length - span);
        let mut result = notes.to_vec();
        let selected = (result.len()..result.len() + self.clipboard.len()).collect();
        result.extend(self.clipboard.iter().map(|note| MidiNote {
            start_tick: start + note.start_tick,
            ..*note
        }));
        self.pending_selection = Some(selected);
        self.cursor_tick = (start + span).min(length);
        self.emit(notes, result, label, actions);
    }
}

fn consumed(ctx: &egui::Context, modifiers: Modifiers, key: Key) -> bool {
    ctx.input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(modifiers, key)))
}

fn clipboard_shortcut(ctx: &egui::Context, key: Key) -> bool {
    consumed(ctx, Modifiers::COMMAND, key) || consumed(ctx, Modifiers::CTRL, key)
}

pub fn shortcuts(
    ctx: &egui::Context,
    state: &mut PianoRollState,
    project: &Project,
    actions: &mut Vec<Action>,
) {
    if ctx.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Key { pressed: true, .. }))
    }) {
        state.last_empty_click = None;
    }
    if ctx.input(|input| input.pointer.any_pressed())
        && let Some(pointer) = ctx.input(|input| input.pointer.interact_pos())
    {
        state.focused = state.panel_rect.is_some_and(|rect| rect.contains(pointer));
    }
    if !state.focused || ctx.egui_wants_keyboard_input() {
        return;
    }
    let Some(clip) = state.clip.and_then(|clip| edit::midi_clip(project, clip)) else {
        return;
    };
    state.selected.retain(|index| *index < clip.notes.len());
    let notes = &clip.notes;
    let (mut native_copy, mut native_cut, mut native_paste) = (false, false, false);
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            match event {
                egui::Event::Copy => native_copy = true,
                egui::Event::Cut => native_cut = true,
                egui::Event::Paste(text) => {
                    native_paste =
                        !state.clipboard.is_empty() && *text == clipboard_text(&state.clipboard);
                }
                _ => return true,
            }
            false
        })
    });
    let command = Modifiers::COMMAND;
    if consumed(ctx, Modifiers::NONE, Key::Escape) {
        state.invalidate();
        return;
    }
    if state.drag.is_some() || state.numeric.is_some() || state.marquee.is_some() {
        return;
    }
    if consumed(ctx, command, Key::A) {
        state.selected = (0..notes.len()).collect();
        state.audition_selection(notes, actions);
    }
    if clipboard_shortcut(ctx, Key::C) || native_copy {
        state.copy_to_clipboard(ctx, notes);
    }
    if clipboard_shortcut(ctx, Key::X) || native_cut {
        state.copy_to_clipboard(ctx, notes);
        state.delete(notes, actions);
        return;
    }
    let extent = clip.content_end();
    if clipboard_shortcut(ctx, Key::V) || native_paste {
        state.paste(notes, extent, "Paste MIDI notes", actions);
        return;
    }
    if consumed(ctx, command, Key::D) {
        if let Some(end) = state
            .selected
            .iter()
            .map(|index| note_end(&notes[*index]))
            .max()
        {
            let saved = state.clipboard.clone();
            state.copy(notes);
            state.cursor_tick = end;
            let span = state.clipboard.iter().map(note_end).max().unwrap_or(0);
            if span <= extent.saturating_sub(end) {
                state.paste(notes, extent, "Duplicate MIDI notes", actions);
            } else {
                state.notice =
                    Some("No room after these notes; shorten them or extend the clip.".into());
            }
            state.clipboard = saved;
        }
        return;
    }
    if consumed(ctx, Modifiers::NONE, Key::Delete) || consumed(ctx, Modifiers::NONE, Key::Backspace)
    {
        state.delete(notes, actions);
        return;
    }
    for (key, pitch) in [(Key::ArrowUp, 12), (Key::ArrowDown, -12)] {
        if consumed(ctx, Modifiers::SHIFT, key) {
            let result = shifted(notes, &state.selected, 0, pitch, extent);
            state.emit(notes, result.clone(), "Transpose MIDI notes", actions);
            if result != *notes {
                state.audition_selection(&result, actions);
            }
            return;
        }
    }
    let step = state.grid.interval(project.time_signature()).unwrap_or(1);
    for (key, dt, pitch) in [
        (Key::ArrowLeft, -i128::from(step), 0),
        (Key::ArrowRight, i128::from(step), 0),
        (Key::ArrowUp, 0, 1),
        (Key::ArrowDown, 0, -1),
    ] {
        if consumed(ctx, Modifiers::NONE, key) {
            let result = shifted(notes, &state.selected, dt, pitch, extent);
            state.emit(notes, result.clone(), "Move MIDI notes", actions);
            if pitch != 0 && result != *notes {
                state.audition_selection(&result, actions);
            }
            return;
        }
    }
    for (key, multiplier) in [
        (Key::Plus, 1.4),
        (Key::Equals, 1.4),
        (Key::Minus, 1.0 / 1.4),
    ] {
        if consumed(ctx, Modifiers::NONE, key) {
            state.pixels_per_tick = (state.pixels_per_tick * multiplier).clamp(0.0001, 4.0);
        }
    }
    if consumed(ctx, Modifiers::NONE, Key::Z) {
        state.fit_pending = true;
    }
}

#[derive(Clone, Copy)]
struct Geometry {
    grid: Rect,
    pixels_per_tick: f64,
    scroll_tick: f64,
    top_key: f32,
    row_height: f32,
}

impl Geometry {
    fn x(self, tick: u64) -> f32 {
        self.grid.left() + ((tick as f64 - self.scroll_tick) * self.pixels_per_tick) as f32
    }
    fn tick(self, x: f32) -> u64 {
        (self.scroll_tick + f64::from(x - self.grid.left()) / self.pixels_per_tick)
            .max(0.0)
            .round() as u64
    }
    fn y(self, key: u8) -> f32 {
        self.grid.top() + (self.top_key - f32::from(key)) * self.row_height
    }
    fn key(self, y: f32) -> u8 {
        (self.top_key - ((y - self.grid.top()) / self.row_height).floor()).clamp(0.0, 127.0) as u8
    }
    fn note_rect(self, note: &MidiNote) -> Rect {
        Rect::from_min_max(
            pos2(self.x(note.start_tick), self.y(note.key) + 1.0),
            pos2(
                self.x(note_end(note)).max(self.x(note.start_tick) + 3.0),
                self.y(note.key) + self.row_height - 1.0,
            ),
        )
    }
}

pub fn show(
    ui: &mut Ui,
    state: &mut PianoRollState,
    project: &Project,
    playhead_tick: u64,
    actions: &mut Vec<Action>,
) {
    let Some(clip_ref) = state.clip else { return };
    let Some(clip) = edit::midi_clip(project, clip_ref) else {
        state.close();
        return;
    };
    let previous_empty_click = ui
        .input(|input| input.pointer.any_pressed())
        .then(|| state.last_empty_click.take())
        .flatten();
    state.panel_rect = Some(ui.max_rect());
    state.selected.retain(|index| *index < clip.notes.len());
    let region = (clip.offset_ticks, clip.offset_end());
    let extent = clip.content_end();
    let color = theme::track_color(project, clip_ref.track);

    let full = ui.available_rect_before_wrap();
    let side = Rect::from_min_max(
        full.min,
        pos2((full.left() + SIDE_WIDTH).min(full.right()), full.bottom()),
    );
    ui.painter().rect_filled(side, 0.0, theme::BG_PANEL);
    ui.painter().vline(
        side.right() + 0.5,
        side.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );
    widgets::inside(ui, side, Layout::top_down(Align::Min), |ui| {
        egui::ScrollArea::vertical()
            .id_salt("piano-properties")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                toolbar(
                    ui,
                    state,
                    &clip.notes,
                    &clip.pitch_bend,
                    extent,
                    color,
                    actions,
                );
            });
    });
    let editor = Rect::from_min_max(pos2(side.right() + 1.0, full.top()), full.max);
    ui.advance_cursor_after_rect(full);
    let mut editor_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(editor)
            .layout(Layout::top_down(Align::Min)),
    );
    let ui = &mut editor_ui;
    // The hint line sits in a header strip level with the side panel's title bar.
    let strip = Rect::from_min_size(editor.min, vec2(editor.width(), 18.0));
    widgets::title_bar(ui.painter(), strip);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let text = state.notice.as_deref().unwrap_or("Double-click to add · drag to move / resize edges · Cmd/Ctrl-drag to copy · Shift-click or drag to select · Alt: no snap · wheel: pitch · Shift-wheel: time");
        ui.add(egui::Label::new(RichText::new(text).small().color(theme::TEXT_DIM)).truncate());
    });
    let available = ui.available_size();
    let (_, area) = ui.allocate_space(available);
    let max_velocity_height = area.height()
        - RULER_HEIGHT
        - SCROLLBAR_HEIGHT
        - VELOCITY_DIVIDER_HEIGHT
        - NOTE_GRID_MIN_HEIGHT;
    if area.width() <= KEY_WIDTH || max_velocity_height < VELOCITY_MIN_HEIGHT {
        return;
    }
    state.velocity_height = state
        .velocity_height
        .clamp(VELOCITY_MIN_HEIGHT, max_velocity_height);
    let divider_rect = |height| {
        let bottom = area.bottom() - SCROLLBAR_HEIGHT - height;
        Rect::from_min_max(
            pos2(area.left(), bottom - VELOCITY_DIVIDER_HEIGHT),
            pos2(area.right(), bottom),
        )
    };
    let divider_response = ui
        .interact(
            divider_rect(state.velocity_height),
            Id::new("piano-velocity-divider"),
            Sense::drag(),
        )
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text("Drag to resize the velocity lane");
    if divider_response.dragged() {
        state.focused = true;
        state.velocity_height = (state.velocity_height - divider_response.drag_delta().y)
            .clamp(VELOCITY_MIN_HEIGHT, max_velocity_height);
    }
    let divider = divider_rect(state.velocity_height);
    ui.painter().rect_filled(divider, 0.0, theme::BG_PANEL);
    for y in [divider.top() + 0.5, divider.bottom() - 0.5] {
        ui.painter()
            .hline(divider.x_range(), y, Stroke::new(1.0, theme::BORDER));
    }
    ui.painter().rect_filled(
        Rect::from_center_size(divider.center(), vec2(30.0, 3.0)),
        1.5,
        if divider_response.dragged() {
            theme::SELECT
        } else if divider_response.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_FAINT
        },
    );
    let ruler = Rect::from_min_max(
        pos2(area.left() + KEY_WIDTH, area.top()),
        pos2(area.right(), area.top() + RULER_HEIGHT),
    );
    // Drawn up to the header strip, over the item spacing under the hint line.
    let ruler_band = ruler.with_min_y(strip.bottom().min(ruler.top()));
    let grid = Rect::from_min_max(
        pos2(ruler.left(), ruler.bottom()),
        pos2(area.right(), divider.top()),
    );
    let velocity = Rect::from_min_max(
        pos2(grid.left(), divider.bottom()),
        pos2(grid.right(), area.bottom() - SCROLLBAR_HEIGHT),
    );
    if state.fit_pending {
        state.pixels_per_tick = f64::from(grid.width() - 8.0) / clip.length_ticks as f64;
        state.scroll_tick = region.0 as f64;
        if clip.notes.is_empty() {
            state.top_key = (60.0 + grid.height() / state.row_height / 2.0)
                .round()
                .min(127.0);
        }
        state.fit_pending = false;
    }
    let hovered = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|point| area.contains(point));
    if hovered && state.drag.is_none() && state.marquee.is_none() {
        let (scroll, modifiers, zoom) = ui.input(|input| {
            (
                input.smooth_scroll_delta,
                input.modifiers,
                input.zoom_delta(),
            )
        });
        if scroll != egui::Vec2::ZERO || zoom != 1.0 {
            state.last_empty_click = None;
        }
        if modifiers.command || zoom != 1.0 {
            let factor = if zoom != 1.0 {
                f64::from(zoom)
            } else {
                (f64::from(scroll.y) * 0.01).exp()
            };
            state.pixels_per_tick = (state.pixels_per_tick * factor).clamp(0.0001, 4.0);
        } else if modifiers.shift {
            state.scroll_tick -= f64::from(scroll.x + scroll.y) / state.pixels_per_tick;
        } else {
            state.scroll_tick -= f64::from(scroll.x) / state.pixels_per_tick;
            state.top_key = (state.top_key + scroll.y / state.row_height)
                .round()
                .clamp(0.0, 127.0);
        }
        ui.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
    }
    let visible_ticks = f64::from(grid.width()) / state.pixels_per_tick;
    let max_scroll = (extent as f64 - visible_ticks).max(0.0);
    state.scroll_tick = state.scroll_tick.clamp(0.0, max_scroll);
    state.top_key = state
        .top_key
        .max((grid.height() / state.row_height).ceil().min(127.0));
    let geo = Geometry {
        grid,
        pixels_per_tick: state.pixels_per_tick,
        scroll_tick: state.scroll_tick,
        top_key: state.top_key,
        row_height: state.row_height,
    };
    if let Some(key) = show_grid(
        ui,
        geo,
        ruler_band,
        area.left(),
        extent,
        project.time_signature(),
        state.grid,
    ) {
        state.focused = true;
        state.begin_audition(
            clip_ref.track,
            [MidiNote {
                start_tick: 0,
                duration_ticks: TICKS_PER_QUARTER,
                key,
                velocity: state.default_velocity,
                channel: state.default_channel,
                slide_ticks: 0,
            }],
            actions,
        );
    }
    let ruler_response = ui.interact(ruler, Id::new("piano-ruler"), Sense::click_and_drag());
    if (ruler_response.clicked() || ruler_response.dragged())
        && let Some(pointer) = ruler_response.interact_pointer_pos()
    {
        state.focused = true;
        state.cursor_tick = snapped(
            geo.tick(pointer.x),
            state.grid,
            project.time_signature(),
            ui,
        )
        .min(extent);
        let inside = state.cursor_tick.clamp(region.0, region.1) - region.0;
        actions.push(Action::Seek(clip.start_tick.saturating_add(inside)));
    }
    let response = ui.interact(grid, Id::new("piano-grid"), Sense::click_and_drag());
    let velocity_response = ui.interact(
        velocity,
        Id::new("piano-velocity-lane"),
        Sense::click_and_drag(),
    );
    let pointer = ui.input(|input| input.pointer.interact_pos());
    // Command can be pressed or released mid-drag; whatever is held at the drop decides.
    if let Some(drag) = &mut state.drag {
        drag.copy = drag.kind == DragKind::Move && ui.input(|input| input.modifiers.command);
    }
    if let Some(pointer) = pointer
        && state.lane == Lane::Bend
    {
        bend_interaction(
            ui,
            state,
            clip,
            geo,
            velocity,
            &velocity_response,
            pointer,
            extent,
            actions,
        );
    }
    if let Some(pointer) = pointer {
        let bend_lane = state.lane == Lane::Bend && velocity.contains(pointer);
        if ui.input(|input| input.pointer.primary_pressed())
            && !bend_lane
            && state.bend_drag.is_none()
            && (response.contains_pointer() || velocity_response.contains_pointer())
        {
            state.focused = true;
            state.notice = None;
            let in_velocity = velocity.contains(pointer);
            let slide_hit = (!in_velocity)
                .then(|| {
                    state.selected.iter().copied().find(|index| {
                        slide_handle(geo, &clip.notes[*index]).distance(pointer) <= HANDLE_RADIUS
                    })
                })
                .flatten();
            let hit = slide_hit.or_else(|| {
                clip.notes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, note)| {
                        if in_velocity {
                            (geo.x(note.start_tick) - pointer.x).abs() <= 6.0
                        } else {
                            geo.note_rect(note).contains(pointer)
                        }
                    })
                    .map(|(index, _)| index)
            });
            if let Some(index) = hit {
                let shift = ui.input(|input| input.modifiers.shift);
                if shift && state.selected.contains(&index) {
                    state.selected.remove(&index);
                } else {
                    if !state.selected.contains(&index) && !shift {
                        state.selected.clear();
                    }
                    state.selected.insert(index);
                    state.begin_audition(clip_ref.track, [clip.notes[index]], actions);
                    let rect = geo.note_rect(&clip.notes[index]);
                    let kind = if slide_hit.is_some() {
                        DragKind::Slide
                    } else if in_velocity {
                        DragKind::Velocity
                    } else if pointer.x >= rect.right() - EDGE_WIDTH.min(rect.width() / 3.0) {
                        DragKind::End
                    } else if pointer.x <= rect.left() + EDGE_WIDTH.min(rect.width() / 3.0) {
                        DragKind::Start
                    } else {
                        DragKind::Move
                    };
                    state.drag = Some(NoteDrag {
                        kind,
                        copy: kind == DragKind::Move && ui.input(|input| input.modifiers.command),
                        origin: pointer,
                        anchor: index,
                        original: clip.notes.clone(),
                        preview: clip.notes.clone(),
                    });
                }
            } else if !in_velocity {
                state.cursor_tick = effective_grid(state.grid, ui)
                    .floor(geo.tick(pointer.x), project.time_signature())
                    .min(extent);
                let original = if ui.input(|input| input.modifiers.shift) {
                    state.selected.clone()
                } else {
                    BTreeSet::new()
                };
                state.selected = original.clone();
                let input_options = ui.ctx().options(|options| options.input_options);
                let double_press = previous_empty_click.is_some_and(|(position, time)| {
                    ui.input(|input| input.time) - time < input_options.max_double_click_delay
                        && pointer.distance_sq(position)
                            < input_options.max_click_dist * input_options.max_click_dist
                });
                let create_note_on_release = state.draw || double_press;
                state.marquee = Some(Marquee {
                    origin: pointer,
                    current: pointer,
                    original,
                    create_note_on_release,
                });
                if create_note_on_release {
                    state.begin_audition(
                        clip_ref.track,
                        [MidiNote {
                            start_tick: state.cursor_tick,
                            duration_ticks: state.default_duration,
                            key: geo.key(pointer.y),
                            velocity: state.default_velocity,
                            channel: state.default_channel,
                            slide_ticks: 0,
                        }],
                        actions,
                    );
                }
            }
        }
        let mut retuned_notes = None;
        if let Some(drag) = &mut state.drag {
            let previous_key = drag.preview[drag.anchor].key;
            let movement = pointer - drag.origin;
            let original = drag.original[drag.anchor];
            let delta = (f64::from(movement.x) / geo.pixels_per_tick).round() as i128;
            let anchor_tick = match drag.kind {
                DragKind::End => note_end(&original),
                DragKind::Slide => original.start_tick + u64::from(original.slide_ticks),
                _ => original.start_tick,
            };
            let target = (i128::from(anchor_tick) + delta).clamp(0, i128::from(u64::MAX)) as u64;
            let dt = if movement.x.abs() < 2.0 {
                0
            } else {
                i128::from(snapped(target, state.grid, project.time_signature(), ui))
                    - i128::from(anchor_tick)
            };
            drag.preview = match drag.kind {
                DragKind::Move => shifted(
                    &drag.original,
                    &state.selected,
                    dt,
                    (-movement.y / geo.row_height).round() as i16,
                    extent,
                ),
                DragKind::Start | DragKind::End => resized(
                    &drag.original,
                    &state.selected,
                    dt,
                    drag.kind == DragKind::Start,
                    extent,
                ),
                DragKind::Velocity => velocity_shifted(
                    &drag.original,
                    &state.selected,
                    (-movement.y * 127.0 / velocity_range_height(velocity)).round() as i16,
                ),
                DragKind::Slide => slid(&drag.original, &state.selected, dt),
            };
            if drag.kind == DragKind::Move
                && drag.preview[drag.anchor].key != previous_key
                && state.held_audition
                && ui.input(|input| input.pointer.primary_down())
            {
                retuned_notes = Some(
                    state
                        .selected
                        .iter()
                        .map(|index| drag.preview[*index])
                        .collect::<Vec<_>>(),
                );
            }
            ui.ctx().set_cursor_icon(match drag.kind {
                DragKind::Move if drag.copy => CursorIcon::Copy,
                DragKind::Move => CursorIcon::Grabbing,
                DragKind::Start | DragKind::End | DragKind::Slide => CursorIcon::ResizeHorizontal,
                DragKind::Velocity => CursorIcon::ResizeVertical,
            });
        }
        if let Some(notes) = retuned_notes {
            state.begin_audition(clip_ref.track, notes, actions);
        }
        let mut cancel_new_note_preview = false;
        if let Some(marquee) = &mut state.marquee {
            marquee.current = pointer;
            if pointer.distance(marquee.origin) > 3.0 {
                cancel_new_note_preview = marquee.create_note_on_release;
                let rect = Rect::from_two_pos(marquee.origin, pointer);
                state.selected = marquee.original.clone();
                state.selected.extend(
                    clip.notes
                        .iter()
                        .enumerate()
                        .filter(|(_, note)| geo.note_rect(note).intersects(rect))
                        .map(|(index, _)| index),
                );
            }
        }
        if cancel_new_note_preview {
            state.cancel_audition();
        }
    }
    if ui.input(|input| input.pointer.primary_released()) {
        if let Some(drag) = state.drag.take() {
            let copied_notes = drag.copied_notes(&state.selected);
            let copying = copied_notes.is_some();
            if copying {
                state.pending_selection = Some(
                    (drag.original.len()..drag.original.len() + state.selected.len()).collect(),
                );
            }
            state.emit(
                &drag.original,
                copied_notes.unwrap_or(drag.preview),
                match drag.kind {
                    DragKind::Move if copying => "Duplicate MIDI notes",
                    DragKind::Move => "Move MIDI notes",
                    DragKind::Start | DragKind::End => "Resize MIDI notes",
                    DragKind::Velocity => "Set MIDI velocity",
                    DragKind::Slide => "Set MIDI note slide",
                },
                actions,
            );
        }
        let empty_click = state
            .marquee
            .as_ref()
            .is_some_and(|marquee| marquee.current.distance(marquee.origin) <= 3.0);
        let create_note = state
            .marquee
            .as_ref()
            .is_some_and(|marquee| marquee.create_note_on_release);
        state.marquee = None;
        if empty_click
            && create_note
            && let Some(pointer) = pointer
            && grid.contains(pointer)
        {
            let start = state.cursor_tick.min(extent - 1);
            let note = MidiNote {
                start_tick: start,
                duration_ticks: state.default_duration.min(extent - start).max(1),
                key: geo.key(pointer.y),
                velocity: state.default_velocity,
                channel: state.default_channel,
                slide_ticks: 0,
            };
            let mut notes = clip.notes.clone();
            state.pending_selection = Some([notes.len()].into());
            notes.push(note);
            state.emit(&clip.notes, notes, "Create MIDI note", actions);
        } else if empty_click && response.clicked() {
            state.last_empty_click =
                pointer.map(|position| (position, ui.input(|input| input.time)));
        }
    }
    let copied_notes = state
        .drag
        .as_ref()
        .and_then(|drag| drag.copied_notes(&state.selected));
    let copied_selection: BTreeSet<_> = copied_notes
        .as_ref()
        .map(|notes| (clip.notes.len()..notes.len()).collect())
        .unwrap_or_default();
    let selected = if copied_notes.is_some() {
        &copied_selection
    } else {
        &state.selected
    };
    let notes = copied_notes
        .as_ref()
        .or_else(|| {
            state
                .drag
                .as_ref()
                .map(|drag| &drag.preview)
                .or_else(|| state.numeric.as_ref().map(|edit| &edit.preview))
        })
        .unwrap_or(&clip.notes);
    paint_notes(ui, geo, notes, selected, color);
    match state.lane {
        Lane::Velocity => paint_velocity(
            ui,
            geo,
            velocity,
            area.left(),
            notes,
            selected,
            color,
            project.time_signature(),
        ),
        Lane::Bend => {
            let points = state
                .bend_drag
                .as_ref()
                .map_or(clip.pitch_bend.as_slice(), BendDrag::preview);
            let hovered = pointer.filter(|pointer| velocity.contains(*pointer));
            paint_bend(
                ui,
                geo,
                velocity,
                area.left(),
                points,
                hovered,
                color,
                project.time_signature(),
            );
        }
    }
    paint_region(ui, geo, ruler_band, velocity, region, extent, color);
    if let Some(marquee) = &state.marquee
        && marquee.current.distance(marquee.origin) > 3.0
    {
        let rect = Rect::from_two_pos(marquee.origin, marquee.current).intersect(grid);
        ui.painter().rect(
            rect,
            0.0,
            theme::SELECTION.gamma_multiply(0.10),
            Stroke::new(1.0, theme::SELECTION),
            StrokeKind::Inside,
        );
    }
    if playhead_tick >= clip.start_tick
        && playhead_tick <= clip.start_tick.saturating_add(clip.length_ticks)
    {
        let x = geo.x(playhead_tick - clip.start_tick + region.0);
        if x >= grid.left() && x <= grid.right() {
            let (x, top) = (x.round() + 0.5, ruler_band.top());
            ui.painter().vline(
                x,
                top..=velocity.bottom(),
                Stroke::new(1.0, theme::PLAYHEAD),
            );
            widgets::playhead_head(
                &ui.painter().with_clip_rect(ruler_band),
                x,
                top,
                theme::PLAYHEAD,
            );
        }
    }
    let cursor_x = geo.x(state.cursor_tick);
    if grid.x_range().contains(cursor_x) {
        // Kept whole at the edges: it starts at the region start, which Fit puts at the left.
        let x = (cursor_x.round() + 0.5).clamp(ruler.left() + 4.0, ruler.right() - 4.0);
        ui.painter()
            .with_clip_rect(ruler)
            .add(Shape::convex_polygon(
                vec![
                    pos2(x - 4.0, ruler.bottom() - 6.0),
                    pos2(x + 4.0, ruler.bottom() - 6.0),
                    pos2(x, ruler.bottom() - 1.0),
                ],
                theme::TEXT,
                Stroke::NONE,
            ));
    }
    let scrollbar =
        Rect::from_min_max(pos2(grid.left(), area.bottom() - 22.0), area.right_bottom());
    // egui lays the slider out `slider_width` wide, not across the row, so the dark track
    // under it is sized from the slider's response.
    let track = ui.painter().add(Shape::Noop);
    let slider = ui
        .scope_builder(egui::UiBuilder::new().max_rect(scrollbar), |ui| {
            // A scroll position, not a level: a grey knob in a dark track, no fill.
            ui.spacing_mut().slider_rail_height = 0.0;
            let visuals = ui.visuals_mut();
            visuals.slider_trailing_fill = false;
            for (widget, gray) in [
                (&mut visuals.widgets.inactive, 0x5a),
                (&mut visuals.widgets.hovered, 0x6c),
                (&mut visuals.widgets.active, 0x7a),
            ] {
                widget.bg_fill = Color32::from_gray(gray);
                widget.fg_stroke = Stroke::new(1.0, Color32::from_gray(0x16));
            }
            ui.add_sized(
                scrollbar.size(),
                egui::Slider::new(&mut state.scroll_tick, 0.0..=max_scroll).show_value(false),
            )
        })
        .inner;
    ui.painter().set(
        track,
        Shape::rect_filled(
            Rect::from_x_y_ranges(
                slider.rect.x_range(),
                (scrollbar.center().y - 3.0)..=(scrollbar.center().y + 3.0),
            ),
            3.0,
            theme::BG_WIDGET,
        ),
    );
    lane_switch(
        ui,
        state,
        Rect::from_min_max(
            pos2(area.left() + 3.0, area.bottom() - SCROLLBAR_HEIGHT + 3.0),
            pos2(grid.left() - 3.0, area.bottom() - 3.0),
        ),
    );
}

/// Velocity / Bend tabs under the keyboard.
fn lane_switch(ui: &mut Ui, state: &mut PianoRollState, rect: Rect) {
    let half = rect.width() / 2.0;
    for (index, (lane, label, hint)) in [
        (Lane::Velocity, "Vel", "Edit note velocities in the lane"),
        (Lane::Bend, "Bend", "Draw the clip's pitch bend in the lane"),
    ]
    .into_iter()
    .enumerate()
    {
        let tab = Rect::from_min_size(
            rect.min + vec2(index as f32 * half, 0.0),
            vec2(half - 1.0, rect.height()),
        );
        let response = ui.interact(tab, Id::new(("piano-lane", index)), Sense::click());
        let on = state.lane == lane;
        let cap = analog::key(
            ui.painter(),
            tab.shrink2(vec2(0.5, 0.0)),
            3.5,
            on.then_some(theme::SELECT),
            response.hovered(),
            response.is_pointer_button_down_on(),
        );
        // 9 px keeps "Bend" clear of the rim on these 26 px keys.
        ui.painter().text(
            cap.center(),
            Align2::CENTER_CENTER,
            label,
            theme::prop(9.0),
            widgets::key_ink(on.then_some(theme::SELECT), response.hovered()),
        );
        if response.on_hover_text(hint).clicked() {
            state.lane = lane;
            state.bend_drag = None;
            state.focused = true;
        }
    }
}

fn toolbar(
    ui: &mut Ui,
    state: &mut PianoRollState,
    notes: &[MidiNote],
    bend: &[BendPoint],
    length: u64,
    color: Color32,
    actions: &mut Vec<Action>,
) {
    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
    let width = ui.available_width();
    let (title, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    widgets::color_bar(ui.painter(), title, color, "Notes");
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 5.0);
            ui.label(
                RichText::new(format!(
                    "{} notes · {} selected",
                    notes.len(),
                    state.selected.len()
                ))
                .small()
                .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                if widgets::toggle(ui, "Draw", state.draw, theme::SELECT, vec2(38.0, 18.0))
                    .on_hover_text("Single-click empty space to create a note")
                    .clicked()
                {
                    state.draw = !state.draw;
                }
                if ui.button("−").on_hover_text("Zoom out").clicked() {
                    state.pixels_per_tick = (state.pixels_per_tick / 1.4).max(0.0001);
                }
                if ui.button("+").on_hover_text("Zoom in").clicked() {
                    state.pixels_per_tick = (state.pixels_per_tick * 1.4).min(4.0);
                }
                if ui.button("Fit").on_hover_text("Fit the clip").clicked() {
                    state.fit_pending = true;
                }
            });
            Grid::new("piano-settings")
                .num_columns(2)
                .spacing([4.0, 5.0])
                .min_col_width(LABEL_WIDTH)
                .striped(false)
                .show(ui, |ui| {
                    widgets::field_visuals(ui.style_mut());
                    ui.label(RichText::new("Grid").color(theme::TEXT_DIM));
                    egui::ComboBox::from_id_salt("piano-snap")
                        .popup_style(widgets::menu_style())
                        .selected_text(format::grid_label(state.grid))
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            for grid in format::GRIDS {
                                widgets::choice_value(
                                    ui,
                                    &mut state.grid,
                                    grid,
                                    format::grid_label(grid),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label(RichText::new("New length").color(theme::TEXT_DIM));
                    ui.add(
                        egui::DragValue::new(&mut state.default_duration)
                            .range(1..=length)
                            .speed(1),
                    )
                    .on_hover_text("Length of new notes, in ticks");
                    ui.end_row();
                });
            ui.add_space(4.0);

            let source = state
                .numeric
                .as_ref()
                .map_or(notes, |edit| edit.preview.as_slice());
            let first = state.selected.first().copied();
            let mut velocity = first.map_or(state.default_velocity, |index| source[index].velocity);
            let mut channel =
                first.map_or(state.default_channel + 1, |index| source[index].channel + 1);
            caption(
                ui,
                if first.is_some() {
                    "Selection"
                } else {
                    "New notes"
                },
            );
            let velocity_response = ui
                .horizontal(|ui| {
                    field_label(ui, "Velocity");
                    widgets::field_visuals(ui.style_mut());
                    ui.push_id("piano-velocity", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut velocity)
                                .range(1..=127)
                                .speed(0.2),
                        )
                    })
                    .inner
                })
                .inner;
            let channel_response = ui
                .horizontal(|ui| {
                    field_label(ui, "Channel");
                    widgets::field_visuals(ui.style_mut());
                    ui.push_id("piano-channel", |ui| {
                        ui.add(egui::DragValue::new(&mut channel).range(1..=16).speed(0.1))
                    })
                    .inner
                })
                .inner;
            let mut slide = first.map_or(0, |index| source[index].slide_ticks);
            let slide_response = ui
                .horizontal(|ui| {
                    field_label(ui, "Slide");
                    widgets::field_visuals(ui.style_mut());
                    ui.add_enabled_ui(first.is_some(), |ui| {
                        ui.push_id("piano-slide", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut slide)
                                    .range(0..=u32::try_from(length).unwrap_or(u32::MAX))
                                    .speed(2),
                            )
                        })
                        .inner
                    })
                    .inner
                })
                .inner
                .on_hover_text(
                    "Glide into the note from the previous pitch over this many ticks, without restarting its envelope (instruments that support slides, like Ferro)",
                );
            let changed = velocity_response.changed()
                || channel_response.changed()
                || slide_response.changed();
            if first.is_none() {
                state.default_velocity = velocity;
                state.default_channel = channel - 1;
            } else if changed {
                let edit = state.numeric.get_or_insert_with(|| NumericEdit {
                    original: notes.to_vec(),
                    preview: notes.to_vec(),
                });
                for index in &state.selected {
                    if velocity_response.changed() {
                        edit.preview[*index].velocity = velocity;
                    }
                    if channel_response.changed() {
                        edit.preview[*index].channel = channel - 1;
                    }
                    if slide_response.changed() {
                        let note = &mut edit.preview[*index];
                        note.slide_ticks =
                            slide.min(u32::try_from(note.duration_ticks).unwrap_or(u32::MAX));
                    }
                }
            }
            let mut responses = vec![velocity_response, channel_response, slide_response];
            if state.selected.len() == 1 {
                let index = *state.selected.first().unwrap();
                let note = state
                    .numeric
                    .as_ref()
                    .map_or(notes[index], |edit| edit.preview[index]);
                let mut start = note.start_tick;
                let mut duration = note.duration_ticks;
                ui.label(RichText::new(format::note_name(note.key)).strong());
                let start_response = ui
                    .horizontal(|ui| {
                        field_label(ui, "Start");
                        widgets::field_visuals(ui.style_mut());
                        ui.push_id("piano-start", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut start)
                                    .range(0..=length - duration)
                                    .speed(1),
                            )
                        })
                        .inner
                    })
                    .inner;
                let duration_response = ui
                    .horizontal(|ui| {
                        field_label(ui, "Length");
                        widgets::field_visuals(ui.style_mut());
                        ui.push_id("piano-duration", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut duration)
                                    .range(1..=length - start)
                                    .speed(1),
                            )
                        })
                        .inner
                    })
                    .inner;
                if start_response.changed() || duration_response.changed() {
                    let edit = state.numeric.get_or_insert_with(|| NumericEdit {
                        original: notes.to_vec(),
                        preview: notes.to_vec(),
                    });
                    edit.preview[index].start_tick = start;
                    edit.preview[index].duration_ticks = duration;
                }
                responses.extend([start_response, duration_response]);
            }
            if state.numeric.is_some()
                && responses.iter().any(|response| {
                    response.drag_stopped()
                        || response.lost_focus()
                        || (response.changed() && !response.dragged() && !response.has_focus())
                })
            {
                let edit = state.numeric.take().unwrap();
                state.emit(
                    &edit.original,
                    edit.preview,
                    "Edit MIDI note properties",
                    actions,
                );
            }
            ui.horizontal_wrapped(|ui| {
                if first.is_some() {
                    let sliding = state
                        .selected
                        .iter()
                        .all(|index| notes[*index].slide_ticks > 0);
                    let label = if sliding { "No slide" } else { "Slide" };
                    if ui
                        .button(label)
                        .on_hover_text("Toggle a sixteenth-note slide into the selected notes")
                        .clicked()
                    {
                        let mut edited = notes.to_vec();
                        for index in &state.selected {
                            let note = &mut edited[*index];
                            note.slide_ticks = if sliding {
                                0
                            } else {
                                (TICKS_PER_QUARTER / 4).min(note.duration_ticks) as u32
                            };
                        }
                        state.emit(notes, edited, "Set MIDI note slide", actions);
                    }
                }
                if first.is_some() && ui.button("Delete notes").clicked() {
                    state.delete(notes, actions);
                }
                if state.lane == Lane::Bend
                    && !bend.is_empty()
                    && ui
                        .button("Clear bend")
                        .on_hover_text("Remove this clip's pitch bend")
                        .clicked()
                {
                    emit_bend(state, bend, Vec::new(), "Clear pitch bend", actions);
                }
                if ui
                    .button("Close")
                    .on_hover_text("Close the note editor")
                    .clicked()
                {
                    actions.push(Action::ClosePianoRoll);
                }
            });
        });
}

/// A small spaced caption over a group of fields, with a groove running to the right.
fn caption(ui: &mut Ui, text: &str) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    let painter = ui.painter();
    let galley = rack::spaced(painter, text, theme::prop(8.5), theme::TEXT_FAINT, 0.8);
    let end = rect.left() + galley.size().x;
    painter.galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        theme::TEXT_FAINT,
    );
    if end + 12.0 < rect.right() {
        let y = rect.center().y.round() - 0.5;
        let run = (end + 6.0)..=rect.right();
        painter.hline(run.clone(), y, Stroke::new(1.0, theme::BORDER));
        painter.hline(
            run,
            y + 1.0,
            Stroke::new(1.0, Color32::from_white_alpha(10)),
        );
    }
}

/// A field's name in the side panel, padded to [`LABEL_WIDTH`].
fn field_label(ui: &mut Ui, text: &str) {
    let size = vec2(LABEL_WIDTH, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(size);
        ui.label(RichText::new(text).color(theme::TEXT_DIM));
    });
}

fn audition(track: usize, notes: impl IntoIterator<Item = MidiNote>, actions: &mut Vec<Action>) {
    let notes: Vec<_> = notes.into_iter().collect();
    if !notes.is_empty() {
        actions.push(Action::AuditionMidiNotes { track, notes });
    }
}

fn effective_grid(grid: SnapGrid, ui: &Ui) -> SnapGrid {
    if ui.input(|input| input.modifiers.alt) {
        SnapGrid::Off
    } else {
        grid
    }
}
fn snapped(tick: u64, grid: SnapGrid, meter: TimeSignature, ui: &Ui) -> u64 {
    effective_grid(grid, ui).snap(tick, meter).unwrap_or(tick)
}
fn note_end(note: &MidiNote) -> u64 {
    note.start_tick + note.duration_ticks
}

fn clipboard_text(notes: &[MidiNote]) -> String {
    let mut text =
        String::from("Sympoie MIDI notes\nstart_tick\tduration_ticks\tkey\tvelocity\tchannel\n");
    for note in notes {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            note.start_tick,
            note.duration_ticks,
            note.key,
            note.velocity,
            note.channel + 1
        ));
    }
    text
}

fn shifted(
    notes: &[MidiNote],
    selected: &BTreeSet<usize>,
    ticks: i128,
    keys: i16,
    length: u64,
) -> Vec<MidiNote> {
    let mut result = notes.to_vec();
    let Some(first) = selected.first() else {
        return result;
    };
    let mut min_tick = notes[*first].start_tick;
    let mut max_end = note_end(&notes[*first]);
    let mut min_key = notes[*first].key;
    let mut max_key = min_key;
    for index in selected {
        min_tick = min_tick.min(notes[*index].start_tick);
        max_end = max_end.max(note_end(&notes[*index]));
        min_key = min_key.min(notes[*index].key);
        max_key = max_key.max(notes[*index].key);
    }
    let ticks = ticks.clamp(-i128::from(min_tick), i128::from(length - max_end));
    let keys = keys.clamp(-i16::from(min_key), 127 - i16::from(max_key));
    for index in selected {
        result[*index].start_tick = (i128::from(result[*index].start_tick) + ticks) as u64;
        result[*index].key = (i16::from(result[*index].key) + keys) as u8;
    }
    result
}

fn resized(
    notes: &[MidiNote],
    selected: &BTreeSet<usize>,
    delta: i128,
    left: bool,
    length: u64,
) -> Vec<MidiNote> {
    let mut result = notes.to_vec();
    let mut lower = i128::MIN;
    let mut upper = i128::MAX;
    for index in selected {
        let note = notes[*index];
        if left {
            lower = lower.max(-i128::from(note.start_tick));
            upper = upper.min(i128::from(note.duration_ticks) - 1);
        } else {
            lower = lower.max(1 - i128::from(note.duration_ticks));
            upper = upper.min(i128::from(length - note_end(&note)));
        }
    }
    let delta = delta.clamp(lower, upper);
    for index in selected {
        let note = &mut result[*index];
        if left {
            note.start_tick = (i128::from(note.start_tick) + delta) as u64;
            note.duration_ticks = (i128::from(note.duration_ticks) - delta) as u64;
        } else {
            note.duration_ticks = (i128::from(note.duration_ticks) + delta) as u64;
        }
    }
    result
}

/// Where a note's slide handle sits: at the top edge, where the slide reaches the note's
/// pitch (its start when it does not slide).
fn slide_handle(geo: Geometry, note: &MidiNote) -> Pos2 {
    let rect = geo.note_rect(note);
    let x = geo.x(note.start_tick + u64::from(note.slide_ticks));
    pos2(x.min(rect.right() - 2.0).max(rect.left() + 2.0), rect.top())
}

fn slid(notes: &[MidiNote], selected: &BTreeSet<usize>, delta: i128) -> Vec<MidiNote> {
    let mut result = notes.to_vec();
    for index in selected {
        let note = &mut result[*index];
        let slide = (i128::from(note.slide_ticks) + delta)
            .clamp(0, i128::from(note.duration_ticks.min(u64::from(u32::MAX))));
        note.slide_ticks = slide as u32;
    }
    result
}

/// For each note, the key it slides from: the note that started most recently before it.
fn slide_sources(notes: &[MidiNote]) -> Vec<Option<u8>> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|index| (notes[*index].start_tick, *index));
    let mut sources = vec![None; notes.len()];
    for pair in order.windows(2) {
        let (before, after) = (notes[pair[0]], notes[pair[1]]);
        if before.start_tick < after.start_tick {
            sources[pair[1]] = Some(before.key);
        }
    }
    sources
}

const BEND_PADDING: f32 = 6.0;

fn bend_y(lane: Rect, value: i16) -> f32 {
    let half = (lane.height() / 2.0 - BEND_PADDING).max(1.0);
    lane.center().y - sympoie::clip::bend_normalized(value) * half
}

fn bend_value(lane: Rect, y: f32) -> i16 {
    let half = (lane.height() / 2.0 - BEND_PADDING).max(1.0);
    let offset = lane.center().y - y;
    // A small detent around the centre makes returning to no bend easy.
    if offset.abs() <= 3.0 {
        return 0;
    }
    let normalized = (offset / half).clamp(-1.0, 1.0);
    if normalized < 0.0 {
        (normalized * -f32::from(BEND_MIN)).round() as i16
    } else {
        (normalized * f32::from(BEND_MAX)).round() as i16
    }
}

/// Replaces the original points under a freehand stroke with the stroke, thinned.
fn merge_stroke(original: &[BendPoint], stroke: &[BendPoint]) -> Vec<BendPoint> {
    let Some(from) = stroke.iter().map(|point| point.tick).min() else {
        return original.to_vec();
    };
    let to = stroke.iter().map(|point| point.tick).max().unwrap_or(from);
    let mut drawn: Vec<BendPoint> = Vec::with_capacity(stroke.len());
    let mut sorted = stroke.to_vec();
    sorted.sort_by_key(|point| point.tick);
    for point in sorted {
        match drawn.last_mut() {
            Some(last) if last.tick == point.tick => last.value = point.value,
            _ => drawn.push(point),
        }
    }
    let drawn = thin_bend(&drawn, BEND_STROKE_TOLERANCE);
    let mut merged: Vec<BendPoint> = original
        .iter()
        .filter(|point| point.tick < from || point.tick > to)
        .copied()
        .chain(drawn)
        .collect();
    merged.sort_by_key(|point| point.tick);
    merged.dedup_by_key(|point| point.tick);
    merged
}

#[allow(clippy::too_many_arguments)]
fn bend_interaction(
    ui: &Ui,
    state: &mut PianoRollState,
    clip: &sympoie::clip::MidiClip,
    geo: Geometry,
    lane: Rect,
    response: &egui::Response,
    pointer: Pos2,
    extent: u64,
    actions: &mut Vec<Action>,
) {
    let points = &clip.pitch_bend;
    let hit = |points: &[BendPoint]| {
        points
            .iter()
            .enumerate()
            .filter(|(_, point)| {
                pos2(geo.x(point.tick), bend_y(lane, point.value)).distance(pointer)
                    <= HANDLE_RADIUS
            })
            .min_by(|a, b| {
                let distance = |point: &BendPoint| (geo.x(point.tick) - pointer.x).abs();
                distance(a.1).total_cmp(&distance(b.1))
            })
            .map(|(index, _)| index)
    };
    if response.double_clicked()
        && let Some(index) = hit(points)
    {
        let mut edited = points.clone();
        edited.remove(index);
        state.bend_drag = None;
        emit_bend(state, points, edited, "Delete pitch bend point", actions);
        return;
    }
    let tick = geo.tick(pointer.x).min(extent);
    let value = bend_value(lane, pointer.y);
    if ui.input(|input| input.pointer.primary_pressed()) && lane.contains(pointer) {
        state.focused = true;
        state.bend_drag = Some(match hit(points) {
            Some(index) => BendDrag::Point {
                index,
                original: points.clone(),
                preview: points.clone(),
            },
            None => {
                let stroke = vec![BendPoint { tick, value }];
                BendDrag::Draw {
                    original: points.clone(),
                    preview: merge_stroke(points, &stroke),
                    stroke,
                }
            }
        });
    } else if ui.input(|input| input.pointer.primary_down()) {
        match &mut state.bend_drag {
            Some(BendDrag::Point {
                index,
                original,
                preview,
            }) => {
                let lower = index
                    .checked_sub(1)
                    .map_or(0, |previous| original[previous].tick + 1);
                let upper = original
                    .get(*index + 1)
                    .map_or(extent, |next| next.tick.saturating_sub(1));
                preview[*index] = BendPoint {
                    tick: tick.clamp(lower, upper.max(lower)),
                    value,
                };
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
            Some(BendDrag::Draw {
                original,
                stroke,
                preview,
            }) => {
                if stroke.last().is_none_or(|last| {
                    (geo.x(last.tick) - pointer.x).abs() >= 2.0 || last.value != value
                }) {
                    stroke.push(BendPoint { tick, value });
                    *preview = merge_stroke(original, stroke);
                }
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            None => {}
        }
    }
    if ui.input(|input| input.pointer.primary_released())
        && let Some(drag) = state.bend_drag.take()
    {
        let label = match drag {
            BendDrag::Point { .. } => "Move pitch bend point",
            BendDrag::Draw { .. } => "Draw pitch bend",
        };
        emit_bend(
            state,
            drag.original(),
            drag.preview().to_vec(),
            label,
            actions,
        );
    }
}

fn emit_bend(
    state: &PianoRollState,
    original: &[BendPoint],
    points: Vec<BendPoint>,
    label: &'static str,
    actions: &mut Vec<Action>,
) {
    if points != original
        && let Some(clip) = state.clip
    {
        actions.push(Action::EditMidiPitchBend {
            clip,
            before: original.to_vec(),
            points,
            label,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_bend(
    ui: &Ui,
    geo: Geometry,
    lane: Rect,
    left: f32,
    points: &[BendPoint],
    hovered: Option<Pos2>,
    track_color: Color32,
    meter: TimeSignature,
) {
    let painter = ui.painter().with_clip_rect(lane);
    painter.rect_filled(lane, 0.0, theme::BG_DEEP);
    paint_bar_bands(&painter, geo, lane, meter);
    let (axis_painter, axis) = lane_axis(ui, lane, left);
    for (value, label) in [
        (BEND_MAX, "+"),
        (BEND_MAX / 2, ""),
        (0, "0"),
        (BEND_MIN / 2, ""),
        (BEND_MIN, "−"),
    ] {
        let y = bend_y(lane, value);
        painter.hline(
            lane.x_range(),
            y,
            Stroke::new(
                1.0,
                if value == 0 {
                    Color32::from_white_alpha(34)
                } else {
                    theme::grid_beat()
                },
            ),
        );
        if !label.is_empty() {
            axis_tick(&axis_painter, axis, y, label);
        }
    }
    if points.is_empty() {
        painter.text(
            lane.center(),
            Align2::CENTER_CENTER,
            "Drag to draw pitch bend",
            theme::prop(10.0),
            theme::TEXT_DIM,
        );
        return;
    }
    // The curve holds its first and last values beyond the points.
    let mut line = vec![pos2(lane.left(), bend_y(lane, points[0].value))];
    line.extend(
        points
            .iter()
            .map(|point| pos2(geo.x(point.tick), bend_y(lane, point.value))),
    );
    line.push(pos2(
        lane.right(),
        bend_y(lane, points[points.len() - 1].value),
    ));
    let zero = bend_y(lane, 0);
    let shade = track_color.gamma_multiply(0.18);
    let mut fill = Mesh::default();
    let mut span = |from: Pos2, to: Pos2| {
        let first = fill.vertices.len() as u32;
        for point in [from, to, pos2(to.x, zero), pos2(from.x, zero)] {
            fill.colored_vertex(point, shade);
        }
        fill.add_triangle(first, first + 1, first + 2);
        fill.add_triangle(first, first + 2, first + 3);
    };
    for pair in line.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        // A segment crossing the centre is split there, so each part fills toward it.
        if (from.y - zero) * (to.y - zero) < 0.0 {
            let cross = pos2(
                from.x + (to.x - from.x) * (zero - from.y) / (to.y - from.y),
                zero,
            );
            span(from, cross);
            span(cross, to);
        } else {
            span(from, to);
        }
    }
    painter.add(Shape::mesh(fill));
    painter.add(Shape::line(line, Stroke::new(1.5, track_color)));
    for point in points {
        let center = pos2(geo.x(point.tick), bend_y(lane, point.value));
        let hot = hovered.is_some_and(|pointer| pointer.distance(center) <= HANDLE_RADIUS);
        let radius = if hot { 4.5 } else { 3.0 };
        painter.circle_filled(
            center,
            radius,
            if hot { theme::SELECT } else { track_color },
        );
        painter.circle_stroke(
            center,
            radius,
            Stroke::new(1.0, Color32::from_black_alpha(160)),
        );
    }
    if let Some(pointer) = hovered {
        let tick = geo.tick(pointer.x);
        let value = bend_value_at(points, tick);
        painter.text(
            pos2(lane.right() - 6.0, lane.top() + 4.0),
            Align2::RIGHT_TOP,
            format!("{:+.0}%", sympoie::clip::bend_normalized(value) * 100.0),
            theme::mono(9.0),
            theme::TEXT_DIM,
        );
    }
}

fn velocity_shifted(notes: &[MidiNote], selected: &BTreeSet<usize>, delta: i16) -> Vec<MidiNote> {
    let mut result = notes.to_vec();
    for index in selected {
        result[*index].velocity = (i16::from(notes[*index].velocity) + delta).clamp(1, 127) as u8;
    }
    result
}

/// Shades every other bar of the clip, counting from its first bar like the ruler.
fn paint_bar_bands(painter: &egui::Painter, geo: Geometry, lane: Rect, meter: TimeSignature) {
    let from = geo.scroll_tick + f64::from(lane.left() - geo.grid.left()) / geo.pixels_per_tick;
    let to = geo.scroll_tick + f64::from(lane.right() - geo.grid.left()) / geo.pixels_per_tick;
    bars::paint_bands(
        painter,
        lane,
        meter.ticks_per_bar(),
        geo.pixels_per_tick,
        from..=to,
        |tick| geo.x(tick),
    );
}

fn show_grid(
    ui: &Ui,
    geo: Geometry,
    ruler: Rect,
    left: f32,
    length: u64,
    meter: TimeSignature,
    grid: SnapGrid,
) -> Option<u8> {
    let mut pressed_key = None;
    let painter = ui.painter().with_clip_rect(geo.grid);
    painter.rect_filled(geo.grid, 0.0, theme::LANE);
    let keyboard_rect = Rect::from_min_max(pos2(left, geo.grid.top()), geo.grid.left_bottom());
    let keyboard = ui.painter().with_clip_rect(keyboard_rect);
    let keys_right = geo.grid.left() - 1.0;
    horizontal_gradient(
        &keyboard,
        keyboard_rect.with_max_x(keys_right),
        IVORY_BACK,
        IVORY_FRONT,
    );
    let visible = |key: u8| {
        let y = geo.y(key);
        y + geo.row_height > geo.grid.top() && y < geo.grid.bottom()
    };
    let key_rect = |key: u8| {
        let y = geo.y(key);
        Rect::from_min_max(pos2(left, y), pos2(keys_right, y + geo.row_height - 1.0))
    };
    let ebony = |key_rect: Rect| key_rect.with_max_x(left + (key_rect.width() * 0.62));
    let mut held = None;
    for key in (0..=127u8).filter(|key| visible(*key)) {
        let y = geo.y(key);
        let row = Rect::from_min_size(
            pos2(geo.grid.left(), y),
            vec2(geo.grid.width(), geo.row_height),
        );
        painter.rect_filled(
            row,
            0.0,
            if is_black(key) {
                theme::LANE
            } else {
                theme::LANE_ALT
            },
        );
        painter.hline(
            row.x_range(),
            row.bottom(),
            Stroke::new(
                1.0,
                if key % 12 == 0 {
                    Color32::from_white_alpha(40)
                } else {
                    Color32::from_black_alpha(70)
                },
            ),
        );
        let key_rect = key_rect(key);
        let response = ui
            .interact(
                key_rect.intersect(keyboard_rect),
                Id::new(("piano-key", key)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("{} · Hold to audition", format::note_name(key)));
        if response.contains_pointer() && ui.input(|input| input.pointer.primary_pressed()) {
            pressed_key = Some(key);
        }
        if response.is_pointer_button_down_on() {
            held = Some(key);
            // A held ebony key lights only its body, drawn with the others below.
            if !is_black(key) {
                horizontal_gradient(
                    &keyboard,
                    key_rect,
                    theme::SELECT.lerp_to_gamma(Color32::BLACK, 0.12),
                    theme::SELECT.lerp_to_gamma(Color32::WHITE, 0.12),
                );
            }
        }
        keyboard.hline(
            left..=keys_right,
            key_rect.bottom() + 0.5,
            Stroke::new(1.0, KEY_SEAM),
        );
        if is_black(key) {
            keyboard.rect_filled(
                ebony(key_rect).translate(vec2(1.0, 1.0)),
                EBONY_RADIUS,
                Color32::from_black_alpha(46),
            );
        } else if key % 12 == 0 {
            keyboard.text(
                pos2(keys_right - 4.0, key_rect.center().y),
                Align2::RIGHT_CENTER,
                format::note_name(key),
                theme::prop(9.0),
                if held == Some(key) {
                    widgets::ink_on(theme::SELECT)
                } else {
                    theme::TEXT_ON_COLOR
                },
            );
        }
    }
    // The ebony keys share one mesh, placed under their outlines and highlights.
    let bodies = keyboard.add(Shape::Noop);
    let mut mesh = Mesh::default();
    for key in (0..=127u8).filter(|key| is_black(*key) && visible(*key)) {
        let body = ebony(key_rect(key));
        let (top, bottom) = if held == Some(key) {
            (
                theme::SELECT.lerp_to_gamma(Color32::WHITE, 0.12),
                theme::SELECT.lerp_to_gamma(Color32::BLACK, 0.35),
            )
        } else {
            (EBONY_TOP, EBONY_BOTTOM)
        };
        rounded_right_gradient(&mut mesh, body, f32::from(EBONY_RADIUS.ne), top, bottom);
        keyboard.hline(
            (body.left() + 1.0)..=(body.right() - 3.0),
            body.top() + 1.5,
            Stroke::new(1.0, Color32::from_white_alpha(22)),
        );
        keyboard.rect_stroke(
            body,
            EBONY_RADIUS,
            Stroke::new(1.0, Color32::from_black_alpha(200)),
            StrokeKind::Inside,
        );
    }
    keyboard.set(bodies, Shape::mesh(mesh));
    keyboard.vline(
        geo.grid.left() - 0.5,
        keyboard_rect.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );
    paint_bar_bands(&painter, geo, geo.grid, meter);

    let ruler_painter = ui.painter().with_clip_rect(ruler);
    ruler_painter.rect_filled(ruler, 0.0, theme::RULER);
    ruler_painter.hline(
        ruler.x_range(),
        ruler.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(8)),
    );
    let bar = meter.ticks_per_bar();
    let beat = meter.ticks_per_beat();
    // Beat lines show whatever the snap; finer lines follow it. Lines stay at least 8 px apart.
    let mut step = grid
        .interval(meter)
        .unwrap_or(TICKS_PER_QUARTER / 4)
        .min(beat);
    while step as f64 * geo.pixels_per_tick < 8.0 {
        let next = if step < beat {
            Some(step.saturating_mul(2).min(beat))
        } else if step < bar {
            Some(bar)
        } else {
            step.checked_mul(2)
        };
        let Some(next) = next else {
            break;
        };
        step = next;
    }
    let visible_end = geo.tick(geo.grid.right()).min(length);
    let start = geo.tick(geo.grid.left()) / step * step;
    let mut tick = start;
    while tick <= visible_end {
        let x = geo.x(tick);
        let (line, mark, ink) = if tick.is_multiple_of(bar) {
            (theme::grid_bar(), 8.0, theme::TEXT_FAINT)
        } else if tick.is_multiple_of(beat) {
            (theme::grid_beat(), 4.0, theme::TEXT_FAINT)
        } else {
            (theme::grid_fine(), 2.0, Color32::from_gray(0x4c))
        };
        painter.vline(x, geo.grid.y_range(), Stroke::new(1.0, line));
        ruler_painter.vline(
            x,
            (ruler.bottom() - mark)..=ruler.bottom(),
            Stroke::new(1.0, ink),
        );
        let Some(next) = tick.checked_add(step) else {
            break;
        };
        tick = next;
    }
    let ruler_step = format::ruler_step(meter, geo.pixels_per_tick, 65.0);
    let mut tick = geo.tick(geo.grid.left()) / ruler_step * ruler_step;
    while tick <= visible_end {
        let x = geo.x(tick);
        let position = meter.position_at(tick);
        let (label, font, ink) = if position.beat == 1 && position.tick == 0 {
            (position.bar.to_string(), theme::prop(10.5), theme::TEXT)
        } else if position.tick == 0 {
            (
                format!("{}.{}", position.bar, position.beat),
                theme::prop(9.5),
                theme::TEXT_DIM,
            )
        } else {
            (
                format::bar_beat(meter, tick),
                theme::prop(9.5),
                theme::TEXT_DIM,
            )
        };
        // Clear of the region flags and playhead head above and the edit cursor below.
        ruler_painter.text(
            pos2(x + 4.0, ruler.bottom() - 12.0),
            Align2::LEFT_CENTER,
            label,
            font,
            ink,
        );
        let Some(next) = tick.checked_add(ruler_step) else {
            break;
        };
        tick = next;
    }
    let corner = Rect::from_min_max(pos2(left, ruler.top()), pos2(ruler.left(), ruler.bottom()));
    ui.painter().rect_filled(corner, 0.0, theme::RULER);
    ui.painter().hline(
        left..=ruler.right(),
        ruler.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    ui.painter().vline(
        corner.right() - 0.5,
        corner.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );
    pressed_key
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

fn horizontal_gradient(painter: &egui::Painter, rect: Rect, left: Color32, right: Color32) {
    let mut mesh = Mesh::default();
    for (point, color) in [
        (rect.left_top(), left),
        (rect.right_top(), right),
        (rect.right_bottom(), right),
        (rect.left_bottom(), left),
    ] {
        mesh.colored_vertex(point, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

/// Adds `rect`, rounded on its right side and shaded from `top` to `bottom`, to `mesh`.
fn rounded_right_gradient(mesh: &mut Mesh, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    let radius = radius.min(rect.height() / 2.0).min(rect.width()).max(0.0);
    let shade = |y: f32| {
        top.lerp_to_gamma(
            bottom,
            ((y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0),
        )
    };
    let hub = mesh.vertices.len() as u32;
    mesh.colored_vertex(rect.center(), shade(rect.center().y));
    mesh.colored_vertex(rect.left_top(), shade(rect.top()));
    for (center, from) in [
        (pos2(rect.right() - radius, rect.top() + radius), -90.0_f32),
        (pos2(rect.right() - radius, rect.bottom() - radius), 0.0),
    ] {
        for step in 0..=3 {
            let angle = (from + 30.0 * step as f32).to_radians();
            let point = center + radius * vec2(angle.cos(), angle.sin());
            mesh.colored_vertex(point, shade(point.y));
        }
    }
    mesh.colored_vertex(rect.left_bottom(), shade(rect.bottom()));
    let count = mesh.vertices.len() as u32 - hub - 1;
    for index in 0..count {
        mesh.add_triangle(hub, hub + 1 + index, hub + 1 + (index + 1) % count);
    }
}

fn paint_notes(
    ui: &mut Ui,
    geo: Geometry,
    notes: &[MidiNote],
    selected: &BTreeSet<usize>,
    color: Color32,
) {
    let painter = ui.painter().with_clip_rect(geo.grid);
    // Slides first, so notes draw over the start of their ramps.
    let sources = slide_sources(notes);
    for (index, note) in notes.iter().enumerate() {
        if note.slide_ticks == 0 {
            continue;
        }
        let rect = geo.note_rect(note);
        let from_y =
            sources[index].map_or(rect.center().y, |key| geo.y(key) + geo.row_height / 2.0);
        let (x0, x1) = (
            rect.left(),
            geo.x(note.start_tick + u64::from(note.slide_ticks)),
        );
        if x1 < geo.grid.left() || x0 > geo.grid.right() {
            continue;
        }
        // Drawn with the RC curve the synth glides along.
        let curve: Vec<Pos2> = (0..=16)
            .map(|step| {
                let t = step as f32 / 16.0;
                let shape = (1.0 - (-4.6 * t).exp()) / (1.0 - (-4.6_f32).exp());
                pos2(
                    x0 + (x1 - x0) * t,
                    from_y + (rect.center().y - from_y) * shape,
                )
            })
            .collect();
        painter.add(egui::Shape::line(
            curve,
            Stroke::new(2.0, theme::tint(color, Color32::WHITE, 0.6)),
        ));
    }
    for (index, note) in notes.iter().enumerate() {
        let rect = geo.note_rect(note);
        if !rect.intersects(geo.grid) {
            continue;
        }
        let response = ui.interact(
            rect.intersect(geo.grid),
            Id::new(("piano-note", index)),
            Sense::hover(),
        );
        let chosen = selected.contains(&index);
        let fill = if chosen {
            theme::tint(color, Color32::WHITE, 0.45)
        } else {
            theme::tint(
                theme::LANE,
                color,
                0.45 + 0.55 * f32::from(note.velocity) / 127.0,
            )
        };
        let outline = if chosen {
            Color32::WHITE
        } else {
            theme::tint(fill, Color32::BLACK, 0.5)
        };
        // A note too short to show its corners stays a plain quad, which is cheaper.
        let radius = if rect.width() >= 6.0 { 2.0 } else { 0.0 };
        painter.rect(
            rect,
            radius,
            fill,
            Stroke::new(1.0, outline),
            StrokeKind::Inside,
        );
        if rect.width() > 6.0 {
            painter.hline(
                (rect.left() + 2.0)..=(rect.right() - 2.0),
                rect.top() + 1.5,
                Stroke::new(1.0, Color32::from_white_alpha(40)),
            );
        }
        if rect.width() > 32.0 {
            painter.with_clip_rect(rect.intersect(geo.grid)).text(
                pos2(rect.left() + 4.0, rect.center().y),
                Align2::LEFT_CENTER,
                format::note_name(note.key),
                theme::prop(9.5),
                theme::TEXT_ON_COLOR,
            );
        }
        if chosen {
            let handle = slide_handle(geo, note);
            painter.circle_filled(handle, 3.5, Color32::WHITE);
            painter.circle_stroke(
                handle,
                3.5,
                Stroke::new(1.0, Color32::from_black_alpha(200)),
            );
        }
        let slide = if note.slide_ticks > 0 {
            format!(" · slide {}", note.slide_ticks)
        } else {
            String::new()
        };
        response.on_hover_text(format!(
            "{} · tick {} · length {} · velocity {} · channel {}{slide}",
            format::note_name(note.key),
            note.start_tick,
            note.duration_ticks,
            note.velocity,
            note.channel + 1
        ));
    }
}

fn paint_region(
    ui: &Ui,
    geo: Geometry,
    ruler: Rect,
    velocity: Rect,
    region: (u64, u64),
    extent: u64,
    color: Color32,
) {
    let shade = Color32::from_black_alpha(110);
    let (start, end) = (geo.x(region.0), geo.x(region.1));
    let past = geo.x(extent.max(region.1));
    for lane in [geo.grid, velocity] {
        let painter = ui.painter().with_clip_rect(lane);
        if start > lane.left() {
            painter.rect_filled(lane.with_max_x(start), 0.0, shade);
        }
        if end < lane.right() {
            painter.rect_filled(lane.with_min_x(end), 0.0, shade);
        }
        if past < lane.right() {
            painter.rect_filled(lane.with_min_x(past), 0.0, shade);
        }
    }
    // The clip's own bounds take its colour; yellow is kept for the cycle.
    let mark = theme::tint(color, Color32::WHITE, 0.35);
    let painter = ui.painter().with_clip_rect(ruler);
    for (x, toward) in [(start, 1.0), (end, -1.0)] {
        painter.vline(x, ruler.y_range(), Stroke::new(1.0, mark));
        let flag = vec![
            pos2(x, ruler.top() + 2.0),
            pos2(x + 7.0 * toward, ruler.top() + 6.0),
            pos2(x, ruler.top() + 10.0),
        ];
        painter.add(egui::Shape::convex_polygon(flag, mark, Stroke::NONE));
    }
}

fn velocity_range_height(lane: Rect) -> f32 {
    (lane.height() - VELOCITY_PADDING * 2.0).max(1.0)
}

fn velocity_y(lane: Rect, value: u8) -> f32 {
    lane.bottom() - VELOCITY_PADDING - f32::from(value) / 127.0 * velocity_range_height(lane)
}

#[allow(clippy::too_many_arguments)]
fn paint_velocity(
    ui: &Ui,
    geo: Geometry,
    lane: Rect,
    left: f32,
    notes: &[MidiNote],
    selected: &BTreeSet<usize>,
    track_color: Color32,
    meter: TimeSignature,
) {
    let painter = ui.painter().with_clip_rect(lane);
    painter.rect_filled(lane, 0.0, theme::BG_DEEP);
    paint_bar_bands(&painter, geo, lane, meter);
    let (axis_painter, axis) = lane_axis(ui, lane, left);
    let ticks: &[u8] = if lane.height() >= 96.0 {
        &[0, 32, 64, 96, 127]
    } else {
        &[0, 64, 127]
    };
    for &value in ticks {
        let y = velocity_y(lane, value);
        painter.hline(lane.x_range(), y, Stroke::new(1.0, theme::grid_beat()));
        axis_tick(&axis_painter, axis, y, &value.to_string());
    }
    let base = velocity_y(lane, 0);
    let head = |index: &usize| {
        let note = notes.get(*index)?;
        let x = geo.x(note.start_tick);
        ((lane.left() - 4.0)..=(lane.right() + 4.0))
            .contains(&x)
            .then(|| pos2(x, velocity_y(lane, note.velocity)))
    };
    let mut selection = selected.iter().copied().peekable();
    let (mut plain, mut chosen) = (Vec::new(), Vec::new());
    for index in 0..notes.len() {
        let picked = selection.next_if_eq(&index).is_some();
        if let Some(head) = head(&index) {
            if picked { &mut chosen } else { &mut plain }.push(head);
        }
    }
    // Selected stems last, so they stay on top where notes share a start.
    lollipops(&painter, plain.iter().copied(), base, track_color);
    lollipops(&painter, chosen.iter().copied(), base, theme::SELECT);
}

/// Stems, then rims, then fills, so the heads of a chord merge under one outline.
fn lollipops(
    painter: &egui::Painter,
    heads: impl Iterator<Item = Pos2> + Clone,
    base: f32,
    color: Color32,
) {
    for head in heads.clone() {
        painter.line_segment([pos2(head.x, base), head], Stroke::new(2.0, color));
    }
    // Radii up to 3.3 stay pre-rendered discs at 2x; larger ones become paths.
    let rim = theme::tint(color, Color32::BLACK, 0.6);
    for head in heads.clone() {
        painter.circle_filled(head, 3.3, rim);
    }
    for head in heads {
        painter.circle_filled(head, 2.3, color);
    }
}

/// Fills the value axis left of a lane, under the keyboard, closes the lane's bottom
/// edge and returns the axis painter.
fn lane_axis(ui: &Ui, lane: Rect, left: f32) -> (egui::Painter, Rect) {
    let axis = Rect::from_min_max(pos2(left, lane.top()), lane.left_bottom());
    let painter = ui.painter().with_clip_rect(axis);
    painter.rect_filled(axis, 0.0, theme::BG_PANEL);
    painter.vline(
        axis.right() - 0.5,
        axis.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );
    // Bright bars would otherwise run straight into the panel below.
    ui.painter().hline(
        left..=lane.right(),
        lane.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    (painter, axis)
}

fn axis_tick(painter: &egui::Painter, axis: Rect, y: f32, label: &str) {
    painter.hline(
        (axis.right() - 5.0)..=(axis.right() - 1.0),
        y,
        Stroke::new(1.0, theme::TEXT_FAINT),
    );
    painter.text(
        pos2(axis.right() - 8.0, y),
        Align2::RIGHT_CENTER,
        label,
        theme::mono(9.0),
        theme::TEXT_DIM,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start_tick: u64, duration_ticks: u64, key: u8) -> MidiNote {
        MidiNote {
            start_tick,
            duration_ticks,
            key,
            velocity: 79,
            channel: 6,
            slide_ticks: 0,
        }
    }

    #[test]
    fn group_move_clamps_once_and_preserves_intervals_and_expression() {
        let notes = vec![note(10, 20, 0), note(50, 40, 100), note(0, 1, 127)];
        let selected = [0, 1].into();
        let moved = shifted(&notes, &selected, -100, -12, 100);
        assert_eq!(moved[0], note(0, 20, 0));
        assert_eq!(moved[1], note(40, 40, 100));
        assert_eq!(moved[2], notes[2]);
        let moved = shifted(&notes, &selected, 100, 80, 100);
        assert_eq!(moved[0], note(20, 20, 27));
        assert_eq!(moved[1], note(60, 40, 127));
    }

    #[test]
    fn resizing_keeps_every_note_positive_and_in_clip() {
        let notes = vec![note(10, 20, 60), note(50, 40, 64)];
        let selected = [0, 1].into();
        let left = resized(&notes, &selected, 100, true, 100);
        assert_eq!(left, vec![note(29, 1, 60), note(69, 21, 64)]);
        let right = resized(&notes, &selected, 100, false, 100);
        assert_eq!(right, vec![note(10, 30, 60), note(50, 50, 64)]);
        let shorter = resized(&notes, &selected, -100, false, 100);
        assert_eq!(shorter, vec![note(10, 1, 60), note(50, 21, 64)]);
    }

    #[test]
    fn clipboard_keeps_relative_time_channel_velocity_and_overlap() {
        let notes = vec![note(100, 50, 60), note(110, 50, 67)];
        let mut state = PianoRollState {
            selected: [0, 1].into(),
            ..Default::default()
        };
        state.copy(&notes);
        assert_eq!(state.clipboard, vec![note(0, 50, 60), note(10, 50, 67)]);
    }

    struct PianoHarness {
        ctx: egui::Context,
        state: PianoRollState,
        project: Project,
        time: f64,
    }

    impl PianoHarness {
        fn new(key: u8) -> Self {
            Self::with_notes(vec![note(960, 480, key)])
        }

        fn with_notes(notes: Vec<MidiNote>) -> Self {
            let key = notes.iter().map(|note| note.key).max().unwrap_or(60);
            let mut project = Project::new();
            let track = project.add_midi_track("Piano");
            project
                .add_midi_clip(
                    track,
                    sympoie::clip::MidiClip {
                        offset_ticks: 0,
                        start_tick: 0,
                        length_ticks: 3840,
                        notes,
                        name: None,
                        pitch_bend: Vec::new(),
                    },
                )
                .unwrap();
            let mut state = PianoRollState::default();
            state.open(ClipRef { track: 1, index: 0 }, &project);
            state.top_key = f32::from(key.saturating_add(4).min(127));
            let mut harness = Self {
                ctx: egui::Context::default(),
                state,
                project,
                time: 0.0,
            };
            harness.step(Vec::new());
            harness.step(Vec::new());
            harness
        }

        fn step(&mut self, events: Vec<egui::Event>) -> Vec<Action> {
            self.step_with_modifiers(events, Modifiers::NONE)
        }

        fn step_with_modifiers(
            &mut self,
            mut events: Vec<egui::Event>,
            modifiers: Modifiers,
        ) -> Vec<Action> {
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 400.0))),
                time: Some(self.time),
                focused: true,
                events,
                ..Default::default()
            };
            self.time += 1.0 / 60.0;
            let mut actions = Vec::new();
            let mut output = self.ctx.run_ui(input, |ui| {
                self.state.flush_audition_release(ui.ctx(), &mut actions);
                shortcuts(ui.ctx(), &mut self.state, &self.project, &mut actions);
                show(ui, &mut self.state, &self.project, 0, &mut actions);
                self.state.flush_audition_release(ui.ctx(), &mut actions);
            });
            output.textures_delta.clear();
            actions
        }

        fn flush_only(&mut self, events: Vec<egui::Event>, focused: bool) -> Vec<Action> {
            let mut actions = Vec::new();
            let input = egui::RawInput {
                focused,
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| {
                self.state.flush_audition_release(ui.ctx(), &mut actions);
            });
            output.textures_delta.clear();
            actions
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) -> Vec<Action> {
            self.step_with_modifiers(
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
            )
        }

        fn button(&mut self, pos: Pos2, pressed: bool) -> Vec<Action> {
            self.button_with_modifiers(pos, pressed, Modifiers::NONE)
        }

        fn button_with_modifiers(
            &mut self,
            pos: Pos2,
            pressed: bool,
            modifiers: Modifiers,
        ) -> Vec<Action> {
            self.step_with_modifiers(
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                }],
                modifiers,
            )
        }
    }

    fn rect_of(harness: &PianoHarness, id: Id) -> Rect {
        harness
            .ctx
            .read_response(id)
            .unwrap_or_else(|| panic!("no widget {id:?}"))
            .rect
    }

    fn drag(harness: &mut PianoHarness, from: Pos2, to: Pos2) -> Vec<Action> {
        harness.step(vec![egui::Event::PointerMoved(from)]);
        harness.button(from, true);
        let mut actions = Vec::new();
        for step in 1..=6 {
            let point = from + (to - from) * (step as f32 / 6.0);
            actions.extend(harness.step(vec![egui::Event::PointerMoved(point)]));
        }
        actions.extend(harness.button(to, false));
        actions
    }

    #[test]
    fn dragging_the_slide_handle_sets_a_slide_without_moving_the_note() {
        let mut harness = PianoHarness::new(60);
        let note_rect = rect_of(&harness, Id::new(("piano-note", 0)));
        harness.button(note_rect.center(), true);
        harness.button(note_rect.center(), false);
        harness.step(Vec::new());
        let handle = pos2(note_rect.left() + 2.0, note_rect.top());
        let actions = drag(
            &mut harness,
            handle,
            handle + vec2(note_rect.width() / 2.0, 0.0),
        );
        let edited = actions
            .iter()
            .find_map(|action| match action {
                Action::EditMidiNotes { notes, label, .. } => Some((notes.clone(), *label)),
                _ => None,
            })
            .expect("the slide drag commits an edit");
        assert_eq!(edited.1, "Set MIDI note slide");
        let note = edited.0[0];
        assert_eq!(
            (note.start_tick, note.duration_ticks, note.key),
            (960, 480, 60)
        );
        assert!(
            (200..=280).contains(&note.slide_ticks),
            "slide {}",
            note.slide_ticks
        );
    }

    #[test]
    fn drawing_in_the_bend_lane_commits_a_thinned_curve() {
        let mut harness = PianoHarness::new(60);
        let tab = rect_of(&harness, Id::new(("piano-lane", 1_usize)));
        harness.button(tab.center(), true);
        harness.button(tab.center(), false);
        harness.step(Vec::new());
        assert_eq!(harness.state.lane, Lane::Bend);
        let lane = rect_of(&harness, Id::new("piano-velocity-lane"));
        let from = pos2(lane.left() + 40.0, lane.center().y);
        let to = pos2(lane.left() + 240.0, lane.top() + 4.0);
        let actions = drag(&mut harness, from, to);
        let points = actions
            .iter()
            .find_map(|action| match action {
                Action::EditMidiPitchBend { points, before, .. } => {
                    assert!(before.is_empty());
                    Some(points.clone())
                }
                _ => None,
            })
            .expect("drawing commits a bend");
        assert!(points.len() >= 2);
        assert!(points.windows(2).all(|pair| pair[0].tick < pair[1].tick));
        assert_eq!(points[0].value, 0);
        assert!(points.last().unwrap().value > 7000);
        // A straight stroke thins to its corners, not one point per pixel.
        assert!(points.len() <= 4, "{points:?}");
    }

    #[test]
    fn bend_lane_maps_values_symmetrically_and_snaps_to_centre() {
        let lane = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 112.0));
        for value in [BEND_MIN, -4096, 0, 4096, BEND_MAX] {
            let back = bend_value(lane, bend_y(lane, value));
            assert!(
                (i32::from(back) - i32::from(value)).abs() <= 100,
                "{value} -> {back}"
            );
        }
        assert_eq!(bend_value(lane, lane.center().y + 2.0), 0);
    }

    #[test]
    fn strokes_replace_only_the_points_they_cover() {
        let original = vec![
            BendPoint {
                tick: 0,
                value: 100,
            },
            BendPoint {
                tick: 500,
                value: 200,
            },
            BendPoint {
                tick: 2000,
                value: 300,
            },
        ];
        let stroke = vec![
            BendPoint {
                tick: 900,
                value: 0,
            },
            BendPoint {
                tick: 400,
                value: 1000,
            },
        ];
        let merged = merge_stroke(&original, &stroke);
        assert_eq!(
            merged,
            vec![
                BendPoint {
                    tick: 0,
                    value: 100
                },
                BendPoint {
                    tick: 400,
                    value: 1000
                },
                BendPoint {
                    tick: 900,
                    value: 0
                },
                BendPoint {
                    tick: 2000,
                    value: 300
                },
            ]
        );
    }

    #[test]
    fn command_drag_copies_selected_notes_and_keeps_originals_visible() {
        let original = vec![note(960, 480, 60), note(120, 240, 65), note(1680, 240, 64)];
        for modifiers in [
            Modifiers {
                command: true,
                mac_cmd: true,
                ..Modifiers::NONE
            },
            Modifiers {
                command: true,
                ctrl: true,
                ..Modifiers::NONE
            },
        ] {
            let mut harness = PianoHarness::with_notes(original.clone());
            harness.state.selected = [0, 2].into();
            let original_rect = harness
                .ctx
                .read_response(Id::new(("piano-note", 0_usize)))
                .unwrap()
                .rect;
            let start = original_rect.center();
            harness.step(vec![egui::Event::PointerMoved(start)]);
            harness.button_with_modifiers(start, true, modifiers);
            let end = start
                + vec2(
                    (480.0 * harness.state.pixels_per_tick) as f32,
                    -harness.state.row_height,
                );
            harness.step_with_modifiers(vec![egui::Event::PointerMoved(end)], modifiers);
            assert_eq!(
                harness
                    .ctx
                    .read_response(Id::new(("piano-note", 0_usize)))
                    .unwrap()
                    .rect,
                original_rect
            );
            assert!(
                harness
                    .ctx
                    .read_response(Id::new(("piano-note", 3_usize)))
                    .is_some()
            );
            assert_eq!(
                edit::midi_clip(&harness.project, harness.state.clip.unwrap())
                    .unwrap()
                    .notes,
                original
            );
            let actions = harness.button_with_modifiers(end, false, modifiers);
            let [
                Action::ReleaseMidiAudition,
                Action::EditMidiNotes {
                    clip,
                    before,
                    notes,
                    label,
                },
            ] = actions.as_slice()
            else {
                panic!("expected release and one copy edit, got {actions:?}");
            };
            let mut expected = original.clone();
            expected.extend([note(1440, 480, 61), note(2160, 240, 65)]);
            assert_eq!(before, &original);
            assert_eq!(notes, &expected);
            assert_eq!(*label, "Duplicate MIDI notes");
            assert_eq!(harness.state.selected, BTreeSet::from([0, 2]));
            let track = edit::midi_id(&harness.project, clip.track).unwrap();
            harness
                .project
                .set_midi_clip_notes(track, clip.index, notes.clone())
                .unwrap();
            harness.state.notes_committed();
            assert_eq!(harness.state.selected, BTreeSet::from([3, 4]));
        }
    }

    #[test]
    fn command_state_at_drop_decides_between_moving_and_duplicating_notes() {
        let command = Modifiers {
            command: true,
            mac_cmd: true,
            ..Modifiers::NONE
        };
        let original = vec![note(960, 480, 60), note(1680, 240, 64)];
        for release_before_drop in [false, true] {
            let mut harness = PianoHarness::with_notes(original.clone());
            let start = harness
                .ctx
                .read_response(Id::new(("piano-note", 0_usize)))
                .unwrap()
                .rect
                .center();
            harness.step(vec![egui::Event::PointerMoved(start)]);
            harness.button_with_modifiers(start, true, Modifiers::NONE);
            let end = start + vec2((480.0 * harness.state.pixels_per_tick) as f32, 0.0);
            harness.step(vec![egui::Event::PointerMoved(end)]);
            let copied = |harness: &PianoHarness| {
                let drag = harness.state.drag.as_ref().unwrap();
                drag.copied_notes(&harness.state.selected)
            };
            assert_eq!(copied(&harness), None);

            harness.step_with_modifiers(Vec::new(), command);
            let mut duplicated = original.clone();
            duplicated.push(note(1440, 480, 60));
            assert_eq!(copied(&harness), Some(duplicated.clone()));

            let drop_modifiers = if release_before_drop {
                harness.step_with_modifiers(Vec::new(), Modifiers::NONE);
                assert_eq!(copied(&harness), None);
                Modifiers::NONE
            } else {
                command
            };
            let actions = harness.button_with_modifiers(end, false, drop_modifiers);
            let [
                Action::ReleaseMidiAudition,
                Action::EditMidiNotes {
                    before,
                    notes,
                    label,
                    ..
                },
            ] = actions.as_slice()
            else {
                panic!("expected release and one edit, got {actions:?}");
            };
            assert_eq!(before, &original);
            if release_before_drop {
                assert_eq!(notes, &[note(1440, 480, 60), note(1680, 240, 64)]);
                assert_eq!(*label, "Move MIDI notes");
            } else {
                assert_eq!(notes, &duplicated);
                assert_eq!(*label, "Duplicate MIDI notes");
            }
        }
    }

    #[test]
    fn command_click_cancel_and_resize_do_not_duplicate_notes() {
        let modifiers = Modifiers {
            command: true,
            mac_cmd: true,
            ..Modifiers::NONE
        };
        for operation in ["click", "cancel", "resize"] {
            let mut harness = PianoHarness::new(60);
            let rect = harness
                .ctx
                .read_response(Id::new(("piano-note", 0_usize)))
                .unwrap()
                .rect;
            let start = if operation == "resize" {
                pos2(rect.right() - 1.0, rect.center().y)
            } else {
                rect.center()
            };
            harness.step(vec![egui::Event::PointerMoved(start)]);
            harness.button_with_modifiers(start, true, modifiers);
            let end = if operation == "click" {
                start
            } else {
                start + vec2(30.0, 0.0)
            };
            harness.step_with_modifiers(vec![egui::Event::PointerMoved(end)], modifiers);
            if operation == "cancel" {
                assert_eq!(
                    harness.key(Key::Escape, Modifiers::NONE),
                    [Action::ReleaseMidiAudition]
                );
            }
            let actions = harness.button_with_modifiers(end, false, modifiers);
            if operation == "resize" {
                let [
                    Action::ReleaseMidiAudition,
                    Action::EditMidiNotes {
                        before,
                        notes,
                        label,
                        ..
                    },
                ] = actions.as_slice()
                else {
                    panic!("expected one resize, got {actions:?}");
                };
                assert_eq!(notes.len(), before.len());
                assert!(notes[0].duration_ticks > before[0].duration_ticks);
                assert_eq!(*label, "Resize MIDI notes");
            } else if operation == "cancel" {
                assert!(actions.is_empty());
            } else {
                assert_eq!(actions, [Action::ReleaseMidiAudition]);
            }
            assert!(harness.state.pending_selection.is_none());
        }
    }

    #[test]
    fn clipboard_shortcuts_accept_native_command_and_literal_control() {
        for modifiers in [
            Modifiers {
                command: true,
                mac_cmd: true,
                ..Modifiers::NONE
            },
            Modifiers {
                command: true,
                ctrl: true,
                ..Modifiers::NONE
            },
            Modifiers::CTRL,
        ] {
            let mut harness =
                PianoHarness::with_notes(vec![note(960, 480, 60), note(1200, 240, 64)]);
            harness.state.selected = [0, 1].into();
            assert!(harness.key(Key::C, modifiers).is_empty());
            harness.state.cursor_tick = 1920;
            let actions = harness.key(Key::V, modifiers);
            let [
                Action::EditMidiNotes {
                    before,
                    notes,
                    label,
                    ..
                },
            ] = actions.as_slice()
            else {
                panic!("expected one paste edit, got {actions:?}");
            };
            assert_eq!(notes[..2], before[..]);
            assert_eq!(notes[2..], [note(1920, 480, 60), note(2160, 240, 64)]);
            assert_eq!(*label, "Paste MIDI notes");
            assert_eq!(
                harness.state.pending_selection,
                Some(BTreeSet::from([2, 3]))
            );
        }
        let mut harness = PianoHarness::new(60);
        harness.state.selected.insert(0);
        assert!(harness.step(vec![egui::Event::Copy]).is_empty());
        let text = clipboard_text(&harness.state.clipboard);
        assert!(matches!(
            harness.step(vec![egui::Event::Paste(text)]).as_slice(),
            [Action::EditMidiNotes { .. }]
        ));
    }

    #[test]
    fn piano_keys_audition_immediately_without_editing_or_changing_selection() {
        let mut harness = PianoHarness::new(60);
        harness.state.selected.insert(0);
        harness.state.default_velocity = 83;
        harness.state.default_channel = 3;
        for key in [60_u8, 61, 60] {
            harness.state.focused = false;
            let point = harness
                .ctx
                .read_response(Id::new(("piano-key", key)))
                .unwrap()
                .rect
                .center();
            assert!(
                harness
                    .step(vec![egui::Event::PointerMoved(point)])
                    .is_empty()
            );
            assert_eq!(
                harness.button(point, true),
                [Action::BeginMidiAudition {
                    track: 1,
                    notes: vec![MidiNote {
                        start_tick: 0,
                        duration_ticks: TICKS_PER_QUARTER,
                        key,
                        velocity: 83,
                        channel: 3,
                        slide_ticks: 0,
                    }],
                }]
            );
            assert!(harness.state.focused);
            assert_eq!(harness.state.selected, BTreeSet::from([0]));
            assert!(harness.state.drag.is_none());
            assert!(harness.state.marquee.is_none());
            assert!(harness.step(Vec::new()).is_empty());
            assert_eq!(harness.button(point, false), [Action::ReleaseMidiAudition]);
        }
    }

    #[test]
    fn piano_keys_ignore_secondary_clicks_and_only_hit_visible_rows() {
        let mut harness = PianoHarness::new(127);
        for (top_key, key) in [(127.0, 127_u8), (20.5, 21_u8)] {
            harness.state.top_key = top_key;
            harness.step(Vec::new());
            harness.step(Vec::new());
            let key_rect = harness
                .ctx
                .read_response(Id::new(("piano-key", key)))
                .unwrap()
                .rect;
            let grid = harness
                .ctx
                .read_response(Id::new("piano-grid"))
                .unwrap()
                .rect;
            assert!(key_rect.top() >= grid.top());
            assert!(key_rect.bottom() <= grid.bottom());
            let point = key_rect.center();
            harness.step(vec![egui::Event::PointerMoved(point)]);
            for pressed in [true, false] {
                assert!(
                    harness
                        .step(vec![egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Secondary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        }])
                        .is_empty()
                );
            }
            assert!(matches!(
                harness.button(point, true).as_slice(),
                [Action::BeginMidiAudition { notes, .. }] if notes[0].key == key
            ));
            harness.button(point, false);
        }
    }

    #[test]
    fn piano_key_releases_outside_the_editor_without_retriggering() {
        let mut harness = PianoHarness::new(60);
        let point = harness
            .ctx
            .read_response(Id::new(("piano-key", 60_u8)))
            .unwrap()
            .rect
            .center();
        harness.step(vec![egui::Event::PointerMoved(point)]);
        assert!(matches!(
            harness.button(point, true).as_slice(),
            [Action::BeginMidiAudition { .. }]
        ));
        for _ in 0..90 {
            assert!(harness.step(Vec::new()).is_empty());
            assert!(harness.state.held_audition);
        }
        let outside = pos2(1200.0, 500.0);
        assert!(
            harness
                .step(vec![egui::Event::PointerMoved(outside)])
                .is_empty()
        );
        assert_eq!(
            harness.button(outside, false),
            [Action::ReleaseMidiAudition]
        );
        assert!(!harness.state.held_audition);
        assert!(harness.step(Vec::new()).is_empty());
    }

    #[test]
    fn held_preview_releases_on_editor_cancellation_or_window_interruption() {
        for cancel in [
            "close",
            "invalidate",
            "selection_focus",
            "switch_clip",
            "escape",
            "pointer_gone",
            "window_focus",
        ] {
            let mut harness = PianoHarness::new(60);
            let point = harness
                .ctx
                .read_response(Id::new(("piano-note", 0_usize)))
                .unwrap()
                .rect
                .center();
            harness.step(vec![egui::Event::PointerMoved(point)]);
            assert!(matches!(
                harness.button(point, true).as_slice(),
                [Action::BeginMidiAudition { .. }]
            ));
            let actions = match cancel {
                "escape" => harness.key(Key::Escape, Modifiers::NONE),
                "pointer_gone" => harness.flush_only(vec![egui::Event::PointerGone], true),
                "window_focus" => harness.flush_only(Vec::new(), false),
                _ => {
                    match cancel {
                        "close" => harness.state.close(),
                        "invalidate" => harness.state.invalidate(),
                        "selection_focus" => harness.state.focused = false,
                        "switch_clip" => {
                            let track = edit::midi_id(&harness.project, 1).unwrap();
                            harness
                                .project
                                .add_midi_clip(
                                    track,
                                    sympoie::clip::MidiClip {
                                        offset_ticks: 0,
                                        start_tick: 3840,
                                        length_ticks: 3840,
                                        notes: vec![note(0, 240, 67)],
                                        name: None,
                                        pitch_bend: Vec::new(),
                                    },
                                )
                                .unwrap();
                            harness
                                .state
                                .open(ClipRef { track: 1, index: 1 }, &harness.project);
                        }
                        _ => unreachable!(),
                    }
                    harness.flush_only(Vec::new(), true)
                }
            };
            assert_eq!(actions, [Action::ReleaseMidiAudition], "{cancel}");
            assert!(!harness.state.held_audition, "{cancel}");
            assert!(harness.flush_only(Vec::new(), true).is_empty(), "{cancel}");
        }
    }

    #[test]
    fn quick_key_tap_in_one_frame_still_begins_and_releases_the_gate() {
        let mut harness = PianoHarness::new(60);
        let point = harness
            .ctx
            .read_response(Id::new(("piano-key", 60_u8)))
            .unwrap()
            .rect
            .center();
        harness.step(vec![egui::Event::PointerMoved(point)]);
        let actions = harness.step(
            [true, false]
                .into_iter()
                .map(|pressed| egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                })
                .collect(),
        );
        assert!(matches!(
            actions.as_slice(),
            [
                Action::BeginMidiAudition { .. },
                Action::ReleaseMidiAudition
            ]
        ));
    }

    #[test]
    fn drawing_holds_a_preview_and_does_not_retrigger_after_note_creation() {
        let mut harness = PianoHarness::new(60);
        harness.state.draw = true;
        let grid = harness
            .ctx
            .read_response(Id::new("piano-grid"))
            .unwrap()
            .rect;
        let point = grid.min + vec2(20.0, harness.state.row_height / 2.0);
        harness.step(vec![egui::Event::PointerMoved(point)]);
        assert!(matches!(
            harness.button(point, true).as_slice(),
            [Action::BeginMidiAudition { .. }]
        ));
        assert!(harness.step(Vec::new()).is_empty());
        assert!(matches!(
            harness.button(point, false).as_slice(),
            [Action::ReleaseMidiAudition, Action::EditMidiNotes { .. }]
        ));
        assert!(harness.step(Vec::new()).is_empty());
    }

    #[test]
    fn double_click_creation_previews_on_second_press_until_release() {
        let mut harness = PianoHarness::new(60);
        let grid = harness
            .ctx
            .read_response(Id::new("piano-grid"))
            .unwrap()
            .rect;
        let point = grid.min + vec2(20.0, harness.state.row_height / 2.0);
        harness.step(vec![egui::Event::PointerMoved(point)]);
        assert!(harness.button(point, true).is_empty());
        assert!(harness.button(point, false).is_empty());
        assert!(matches!(
            harness.button(point, true).as_slice(),
            [Action::BeginMidiAudition { .. }]
        ));
        for _ in 0..90 {
            assert!(harness.step(Vec::new()).is_empty());
            assert!(harness.state.held_audition);
        }
        assert_eq!(
            edit::midi_clip(&harness.project, harness.state.clip.unwrap())
                .unwrap()
                .notes
                .len(),
            1
        );
        assert!(matches!(
            harness.button(point, false).as_slice(),
            [Action::ReleaseMidiAudition, Action::EditMidiNotes { notes, label, .. }]
                if notes.len() == 2 && *label == "Create MIDI note"
        ));
        assert!(harness.step(Vec::new()).is_empty());
    }

    #[test]
    fn empty_click_preview_candidate_expires_or_cancels_before_a_later_press() {
        for interruption in ["time", "distance", "cancel", "other_press", "marquee"] {
            let mut harness = PianoHarness::new(60);
            let grid = harness
                .ctx
                .read_response(Id::new("piano-grid"))
                .unwrap()
                .rect;
            let point = grid.min + vec2(20.0, harness.state.row_height / 2.0);
            harness.step(vec![egui::Event::PointerMoved(point)]);
            assert!(harness.button(point, true).is_empty());
            if interruption == "marquee" {
                harness.step(vec![egui::Event::PointerMoved(point + vec2(0.0, 20.0))]);
            }
            assert!(harness.button(point, false).is_empty());
            let mut second = point;
            match interruption {
                "time" => harness.time += 1.0,
                "distance" => second += vec2(10.0, 0.0),
                "cancel" => harness.state.cancel_audition(),
                "other_press" => {
                    for pressed in [true, false] {
                        harness.step(vec![egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Secondary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        }]);
                    }
                }
                "marquee" => {}
                _ => unreachable!(),
            }
            assert!(harness.button(second, true).is_empty(), "{interruption}");
            assert!(harness.button(second, false).is_empty(), "{interruption}");
        }
    }

    #[test]
    fn marquee_selection_does_not_start_preview_on_release() {
        let mut harness = PianoHarness::new(60);
        let rect = harness
            .ctx
            .read_response(Id::new(("piano-note", 0_usize)))
            .unwrap()
            .rect;
        let start = rect.min - vec2(8.0, 8.0);
        let end = rect.max + vec2(8.0, 8.0);
        harness.step(vec![egui::Event::PointerMoved(start)]);
        assert!(harness.button(start, true).is_empty());
        assert!(
            harness
                .step(vec![egui::Event::PointerMoved(end)])
                .is_empty()
        );
        assert_eq!(harness.state.selected, BTreeSet::from([0]));
        assert!(harness.button(end, false).is_empty());
    }

    #[test]
    fn piano_selection_and_pitch_drag_audition_only_when_pitch_changes() {
        let mut harness = PianoHarness::new(60);
        let start = harness
            .ctx
            .read_response(Id::new(("piano-note", 0_usize)))
            .unwrap()
            .rect
            .center();
        harness.step(vec![egui::Event::PointerMoved(start)]);
        assert_eq!(
            harness.button(start, true),
            [Action::BeginMidiAudition {
                track: 1,
                notes: vec![note(960, 480, 60)]
            }]
        );
        assert!(harness.step(Vec::new()).is_empty());
        let horizontal = start + vec2(30.0, 0.0);
        assert!(
            harness
                .step(vec![egui::Event::PointerMoved(horizontal)])
                .is_empty()
        );
        let higher = horizontal - vec2(0.0, harness.state.row_height);
        let actions = harness.step(vec![egui::Event::PointerMoved(higher)]);
        let [Action::BeginMidiAudition { track, notes }] = actions.as_slice() else {
            panic!("expected a single audition, got {actions:?}");
        };
        assert_eq!(*track, 1);
        assert_eq!(notes[0].key, 61);
        assert_eq!(notes[0].velocity, 79);
        assert_eq!(notes[0].channel, 6);
        assert!(harness.step(Vec::new()).is_empty());
        assert!(matches!(
            harness.button(higher, false).as_slice(),
            [Action::ReleaseMidiAudition, Action::EditMidiNotes { .. },]
        ));
    }

    #[test]
    fn note_release_at_a_new_pitch_commits_without_restarting_preview() {
        let mut harness = PianoHarness::new(60);
        let start = harness
            .ctx
            .read_response(Id::new(("piano-note", 0_usize)))
            .unwrap()
            .rect
            .center();
        harness.step(vec![egui::Event::PointerMoved(start)]);
        harness.button(start, true);
        let higher = start - vec2(0.0, harness.state.row_height);
        let actions = harness.button(higher, false);
        assert!(matches!(
            actions.as_slice(),
            [Action::ReleaseMidiAudition, Action::EditMidiNotes { notes, .. }]
                if notes[0].key == 61
        ));
    }

    #[test]
    fn piano_pitch_shortcuts_audition_new_pitch_and_ignore_clamped_moves() {
        for (key, modifiers, expected) in [
            (Key::ArrowUp, Modifiers::NONE, 61),
            (Key::ArrowDown, Modifiers::NONE, 59),
            (Key::ArrowUp, Modifiers::SHIFT, 72),
            (Key::ArrowDown, Modifiers::SHIFT, 48),
        ] {
            let mut harness = PianoHarness::new(60);
            harness.state.selected.insert(0);
            let actions = harness.key(key, modifiers);
            let [
                Action::EditMidiNotes { notes, .. },
                Action::AuditionMidiNotes {
                    track,
                    notes: auditioned,
                },
            ] = actions.as_slice()
            else {
                panic!("expected edit then audition, got {actions:?}");
            };
            assert_eq!(*track, 1);
            assert_eq!(notes, auditioned);
            assert_eq!(auditioned, &[note(960, 480, expected)]);
        }
        let mut harness = PianoHarness::new(127);
        harness.state.selected.insert(0);
        assert!(harness.key(Key::ArrowUp, Modifiers::SHIFT).is_empty());
        let mut harness = PianoHarness::new(0);
        harness.state.selected.insert(0);
        assert!(harness.key(Key::ArrowDown, Modifiers::SHIFT).is_empty());
    }
}
