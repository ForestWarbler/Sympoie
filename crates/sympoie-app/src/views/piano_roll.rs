use std::collections::BTreeSet;

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Grid, Id, Key, KeyboardShortcut, Layout, Modifiers,
    Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2,
};
use sympoie::TICKS_PER_QUARTER;
use sympoie::clip::MidiNote;
use sympoie::project::Project;
use sympoie::time::{SnapGrid, TimeSignature};

use crate::actions::Action;
use crate::document::edit::{self, ClipRef};
use crate::ui::format;
use crate::ui::theme;
use crate::ui::widgets;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragKind {
    Move,
    Start,
    End,
    Velocity,
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
    ui.painter().rect_filled(side, 0.0, theme::BG_RAISED);
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
                toolbar(ui, state, &clip.notes, extent, color, actions);
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
    ui.painter().hline(
        divider.x_range(),
        divider.center().y,
        Stroke::new(1.0, theme::BORDER),
    );
    ui.painter().hline(
        divider.center().x - 16.0..=divider.center().x + 16.0,
        divider.center().y,
        Stroke::new(
            2.0,
            if divider_response.hovered() || divider_response.dragged() {
                theme::ACCENT
            } else {
                theme::TEXT_DIM
            },
        ),
    );
    let ruler = Rect::from_min_max(
        pos2(area.left() + KEY_WIDTH, area.top()),
        pos2(area.right(), area.top() + RULER_HEIGHT),
    );
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
        ruler,
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
    if let Some(pointer) = pointer {
        if ui.input(|input| input.pointer.primary_pressed())
            && (response.contains_pointer() || velocity_response.contains_pointer())
        {
            state.focused = true;
            state.notice = None;
            let in_velocity = velocity.contains(pointer);
            let hit = clip
                .notes
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
                .map(|(index, _)| index);
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
                    let kind = if in_velocity {
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
            let anchor_tick = if drag.kind == DragKind::End {
                note_end(&original)
            } else {
                original.start_tick
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
                DragKind::Start | DragKind::End => CursorIcon::ResizeHorizontal,
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
    paint_velocity(ui, geo, velocity, area.left(), notes, selected, color);
    paint_region(ui, geo, ruler, velocity, region, extent);
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
            ui.painter().line_segment(
                [pos2(x, ruler.top()), pos2(x, velocity.bottom())],
                Stroke::new(1.0, theme::PLAYHEAD),
            );
        }
    }
    let cursor_x = geo.x(state.cursor_tick);
    if grid.x_range().contains(cursor_x) {
        ui.painter().line_segment(
            [
                pos2(cursor_x, ruler.bottom() - 5.0),
                pos2(cursor_x, ruler.bottom()),
            ],
            Stroke::new(2.0, theme::TEXT),
        );
    }
    let scrollbar =
        Rect::from_min_max(pos2(grid.left(), area.bottom() - 22.0), area.right_bottom());
    ui.scope_builder(egui::UiBuilder::new().max_rect(scrollbar), |ui| {
        ui.add_sized(
            scrollbar.size(),
            egui::Slider::new(&mut state.scroll_tick, 0.0..=max_scroll).show_value(false),
        );
    });
    ui.painter().text(
        pos2(area.left() + 5.0, area.bottom() - SCROLLBAR_HEIGHT / 2.0),
        Align2::LEFT_CENTER,
        "Velocity",
        theme::prop(10.0),
        theme::TEXT_DIM,
    );
}

fn toolbar(
    ui: &mut Ui,
    state: &mut PianoRollState,
    notes: &[MidiNote],
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
                if widgets::toggle(ui, "Draw", state.draw, theme::ACCENT, vec2(38.0, 18.0))
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
                .spacing([8.0, 5.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Grid").color(theme::TEXT_DIM));
                    egui::ComboBox::from_id_salt("piano-snap")
                        .selected_text(format::grid_label(state.grid))
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            for grid in format::GRIDS {
                                ui.selectable_value(
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
            ui.separator();

            let source = state
                .numeric
                .as_ref()
                .map_or(notes, |edit| edit.preview.as_slice());
            let first = state.selected.first().copied();
            let mut velocity = first.map_or(state.default_velocity, |index| source[index].velocity);
            let mut channel =
                first.map_or(state.default_channel + 1, |index| source[index].channel + 1);
            ui.label(
                RichText::new(if first.is_some() {
                    "Selection"
                } else {
                    "New notes"
                })
                .color(theme::TEXT),
            );
            let velocity_response = ui
                .horizontal(|ui| {
                    ui.label(RichText::new("Velocity").color(theme::TEXT_DIM));
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
                    ui.label(RichText::new("Channel").color(theme::TEXT_DIM));
                    ui.push_id("piano-channel", |ui| {
                        ui.add(egui::DragValue::new(&mut channel).range(1..=16).speed(0.1))
                    })
                    .inner
                })
                .inner;
            let changed = velocity_response.changed() || channel_response.changed();
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
                }
            }
            let mut responses = vec![velocity_response, channel_response];
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
                        ui.label(RichText::new("Start").color(theme::TEXT_DIM));
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
                        ui.label(RichText::new("Length").color(theme::TEXT_DIM));
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
                if first.is_some() && ui.button("Delete notes").clicked() {
                    state.delete(notes, actions);
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

fn velocity_shifted(notes: &[MidiNote], selected: &BTreeSet<usize>, delta: i16) -> Vec<MidiNote> {
    let mut result = notes.to_vec();
    for index in selected {
        result[*index].velocity = (i16::from(notes[*index].velocity) + delta).clamp(1, 127) as u8;
    }
    result
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
    for key in 0..=127u8 {
        let y = geo.y(key);
        if y + geo.row_height <= geo.grid.top() || y >= geo.grid.bottom() {
            continue;
        }
        let black = matches!(key % 12, 1 | 3 | 6 | 8 | 10);
        let row = Rect::from_min_size(
            pos2(geo.grid.left(), y),
            vec2(geo.grid.width(), geo.row_height),
        );
        painter.rect_filled(row, 0.0, if black { theme::LANE } else { theme::LANE_ALT });
        painter.line_segment(
            [row.left_bottom(), row.right_bottom()],
            Stroke::new(
                1.0,
                if key % 12 == 0 {
                    Color32::from_gray(0x44)
                } else {
                    Color32::from_gray(0x1c)
                },
            ),
        );
        let key_rect = Rect::from_min_max(
            pos2(left, y),
            pos2(geo.grid.left() - 1.0, y + geo.row_height - 1.0),
        );
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
        let pressed = response.is_pointer_button_down_on();
        let light = if pressed {
            theme::ACCENT
        } else {
            Color32::from_gray(0xc8)
        };
        keyboard.rect_filled(key_rect, 0.0, light);
        if black {
            keyboard.rect_filled(
                key_rect.with_max_x(left + (key_rect.width() * 0.62)),
                0.0,
                if pressed {
                    theme::ACCENT
                } else {
                    Color32::from_gray(0x1a)
                },
            );
        }
        if key % 12 == 0 {
            keyboard.text(
                pos2(geo.grid.left() - 4.0, y + geo.row_height / 2.0),
                Align2::RIGHT_CENTER,
                format::note_name(key),
                theme::prop(9.5),
                theme::TEXT_ON_COLOR,
            );
        }
    }
    let ruler_painter = ui.painter().with_clip_rect(ruler);
    ruler_painter.rect_filled(ruler, 0.0, theme::BG_PANEL);
    let mut step = grid.interval(meter).unwrap_or(TICKS_PER_QUARTER / 4);
    while step as f64 * geo.pixels_per_tick < 8.0 {
        let Some(next) = step.checked_mul(2) else {
            break;
        };
        step = next;
    }
    let visible_end = geo.tick(geo.grid.right()).min(length);
    let start = geo.tick(geo.grid.left()) / step * step;
    let mut tick = start;
    while tick <= visible_end {
        let x = geo.x(tick);
        let bar = tick.is_multiple_of(meter.ticks_per_bar());
        painter.line_segment(
            [pos2(x, geo.grid.top()), pos2(x, geo.grid.bottom())],
            Stroke::new(
                1.0,
                if bar {
                    theme::grid_bar()
                } else {
                    theme::grid_beat()
                },
            ),
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
        ruler_painter.text(
            pos2(x + 4.0, ruler.center().y),
            Align2::LEFT_CENTER,
            format::bar_beat(meter, tick),
            theme::mono(10.0),
            theme::TEXT_DIM,
        );
        let Some(next) = tick.checked_add(ruler_step) else {
            break;
        };
        tick = next;
    }
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(left, ruler.top()), pos2(ruler.left(), ruler.bottom())),
        0.0,
        theme::BG_PANEL,
    );
    pressed_key
}

fn paint_notes(
    ui: &mut Ui,
    geo: Geometry,
    notes: &[MidiNote],
    selected: &BTreeSet<usize>,
    color: Color32,
) {
    let painter = ui.painter().with_clip_rect(geo.grid);
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
        let fill = if selected.contains(&index) {
            theme::tint(color, Color32::WHITE, 0.45)
        } else {
            theme::tint(
                theme::LANE,
                color,
                0.45 + 0.55 * f32::from(note.velocity) / 127.0,
            )
        };
        painter.rect(
            rect,
            0.0,
            fill,
            Stroke::new(
                1.0,
                if selected.contains(&index) {
                    Color32::WHITE
                } else {
                    Color32::from_black_alpha(160)
                },
            ),
            StrokeKind::Inside,
        );
        if rect.width() > 32.0 {
            painter.with_clip_rect(rect.intersect(geo.grid)).text(
                pos2(rect.left() + 4.0, rect.center().y),
                Align2::LEFT_CENTER,
                format::note_name(note.key),
                theme::prop(9.5),
                theme::TEXT_ON_COLOR,
            );
        }
        response.on_hover_text(format!(
            "{} · tick {} · length {} · velocity {} · channel {}",
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
    let painter = ui.painter().with_clip_rect(ruler);
    for (x, toward) in [(start, 1.0), (end, -1.0)] {
        painter.vline(x, ruler.y_range(), Stroke::new(1.0, theme::LOOP));
        let flag = vec![
            pos2(x, ruler.top() + 2.0),
            pos2(x + 7.0 * toward, ruler.top() + 6.0),
            pos2(x, ruler.top() + 10.0),
        ];
        painter.add(egui::Shape::convex_polygon(flag, theme::LOOP, Stroke::NONE));
    }
}

fn velocity_range_height(lane: Rect) -> f32 {
    (lane.height() - VELOCITY_PADDING * 2.0).max(1.0)
}

fn velocity_y(lane: Rect, value: u8) -> f32 {
    lane.bottom() - VELOCITY_PADDING - f32::from(value) / 127.0 * velocity_range_height(lane)
}

fn paint_velocity(
    ui: &Ui,
    geo: Geometry,
    lane: Rect,
    left: f32,
    notes: &[MidiNote],
    selected: &BTreeSet<usize>,
    track_color: Color32,
) {
    let painter = ui.painter().with_clip_rect(lane);
    painter.rect_filled(lane, 0.0, theme::BG_DEEP);
    let axis = Rect::from_min_max(pos2(left, lane.top()), lane.left_bottom());
    let axis_painter = ui.painter().with_clip_rect(axis);
    axis_painter.rect_filled(axis, 0.0, theme::BG_PANEL);
    axis_painter.vline(
        axis.right() - 0.5,
        velocity_y(lane, 127)..=velocity_y(lane, 0),
        Stroke::new(1.0, theme::BORDER),
    );
    let ticks: &[u8] = if lane.height() >= 96.0 {
        &[0, 32, 64, 96, 127]
    } else {
        &[0, 64, 127]
    };
    for &value in ticks {
        let y = velocity_y(lane, value);
        painter.hline(lane.x_range(), y, Stroke::new(1.0, theme::grid_beat()));
        axis_painter.hline(
            axis.right() - 5.0..=axis.right(),
            y,
            Stroke::new(1.0, theme::TEXT_DIM),
        );
        axis_painter.text(
            pos2(axis.right() - 8.0, y),
            Align2::RIGHT_CENTER,
            value.to_string(),
            theme::mono(9.0),
            theme::TEXT_DIM,
        );
    }
    for (index, note) in notes.iter().enumerate() {
        let x = geo.x(note.start_tick);
        let y = velocity_y(lane, note.velocity);
        let color = if selected.contains(&index) {
            theme::ACCENT
        } else {
            track_color
        };
        painter.line_segment(
            [pos2(x, velocity_y(lane, 0)), pos2(x, y)],
            Stroke::new(2.0, color),
        );
        painter.circle_filled(pos2(x, y), 3.0, color);
    }
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
