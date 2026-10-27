//! Slice display and controls for the Slicer.
//!
//! The screen shows the sample cut into its slices, shaded alternately, with a numbered
//! flag at each slice start. Drag a flag to move that slice's start, right-click it to
//! remove the slice, double-click the waveform to cut a new slice there, and press a slice
//! to hear it. Slices are kept in time order, so keys rise from the root with the sample.

use super::*;
use crate::ui::format::note_name;
use sympoie::builtin::instruments::{
    ATTACK, GAIN, MAX_SLICES, RELEASE, SLICE_BASE, SLICE_CHOKE, SLICE_CUTOFF, SLICE_DETECT,
    SLICE_DETECTORS, SLICE_DIVISIONS, SLICE_FADE, SLICE_MIN_LENGTH, SLICE_MODE, SLICE_RESONANCE,
    SLICE_REVERSE, SLICE_ROOT, SLICE_SENSITIVITY, SLICE_TRANSPOSE, SLICE_TUNE, slice_regions,
    slice_starts,
};

fn selected_id(plugin: u64) -> Id {
    Id::new(("slicer-selected", plugin))
}

/// The slice last pressed or edited, in key order.
fn selected(ui: &Ui, plugin: u64, count: usize) -> usize {
    ui.ctx()
        .data(|data| data.get_temp::<usize>(selected_id(plugin)))
        .unwrap_or(0)
        .min(count.saturating_sub(1))
}

fn select(ui: &Ui, plugin: u64, slice: usize) {
    ui.ctx()
        .data_mut(|data| data.insert_temp(selected_id(plugin), slice));
}

/// The key that plays `slice`, if it falls on the keyboard.
fn slice_key(deck: &Deck, slice: usize) -> Option<u8> {
    u8::try_from(deck.value(SLICE_ROOT).round() as usize + slice)
        .ok()
        .filter(|key| *key <= 127)
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn toolbar(ui: &mut Ui, deck: &mut Deck, loaded: &Result<Arc<SamplePreview>, String>, card: bool) {
    let id = deck.plugin.id;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let load = rack::led_switch(ui, "Load", false, deck.accent);
        ui.interact(load.rect, Id::new(("slicer-load", id)), Sense::hover());
        if load
            .on_hover_text("Load a sample (WAV, AIFF, FLAC or MP3) and slice it")
            .clicked()
        {
            deck.actions.push(Action::LoadSamplerSample { id });
        }
        let analyze = rack::led_switch(ui, "Analyze", false, deck.accent);
        ui.interact(
            analyze.rect,
            Id::new(("slicer-analyze", id)),
            Sense::hover(),
        );
        let detection = SLICE_DETECTORS[(deck.value(SLICE_DETECT).round() as usize).min(2)];
        if analyze
            .on_hover_text(format!(
                "Slice the sample automatically by {} ({}), replacing the current slices",
                detection.to_lowercase(),
                match detection {
                    "Transient" => "sudden rises in level",
                    "Volume" => "sound rising out of quiet",
                    _ => "equal divisions",
                }
            ))
            .clicked()
            && loaded.is_ok()
        {
            deck.actions.push(Action::AnalyzeSlices { id });
        }
        if card {
            let panel = rack::led_switch(ui, "Panel", false, deck.accent);
            ui.interact(panel.rect, Id::new(("slicer-panel", id)), Sense::hover());
            if panel
                .on_hover_text("Open the Slicer in its own window to edit slices")
                .clicked()
            {
                deck.actions.push(Action::OpenNativeVst3Editor(id));
            }
        } else {
            let clear = rack::led_switch(ui, "Clear", false, deck.accent);
            ui.interact(clear.rect, Id::new(("slicer-clear", id)), Sense::hover());
            if clear
                .on_hover_text("Remove every slice but one that plays the whole sample")
                .clicked()
                && slice_starts(deck.plugin) != [0.0]
            {
                select(ui, id, 0);
                deck.actions.push(Action::SetSlices {
                    id,
                    starts: vec![0.0],
                    label: "Clear Slices",
                });
            }
        }
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), rack::ROW), Sense::hover());
        ui.painter().add(crate::ui::analog::readout(rect, false));
        let (name, detail) = match loaded {
            Ok(sample) => {
                let count = slice_starts(deck.plugin).len();
                (
                    sample.name.clone(),
                    format!(
                        "{count} slice{} · {:.2}s",
                        plural(count),
                        sample.frames as f64 / f64::from(sample.sample_rate)
                    ),
                )
            }
            Err(_) if deck.plugin.state.is_empty() => {
                ("No sample — load an audio file".to_owned(), String::new())
            }
            Err(error) => (error.clone(), String::new()),
        };
        let detail_width = if detail.is_empty() {
            0.0
        } else {
            ui.painter()
                .layout_no_wrap(detail.clone(), rack::value_font(), rack::INK)
                .size()
                .x
                + 8.0
        };
        widgets::fitted_text(
            ui.painter(),
            Rect::from_min_max(
                rect.min + vec2(5.0, 0.0),
                rect.max - vec2(5.0 + detail_width, 0.0),
            ),
            &name,
            theme::prop(9.5),
            match loaded {
                Ok(_) => rack::INK_BRIGHT,
                Err(_) if deck.plugin.state.is_empty() => rack::INK,
                Err(_) => theme::WARN,
            },
            Align2::LEFT_CENTER,
        );
        if !detail.is_empty() {
            ui.painter().text(
                rect.right_center() - vec2(5.0, 0.0),
                Align2::RIGHT_CENTER,
                detail,
                rack::value_font(),
                rack::INK,
            );
        }
    });
}

/// The waveform with its slices; `large` adds the key of each slice under it.
fn slice_screen(
    ui: &mut Ui,
    rect: Rect,
    deck: &mut Deck,
    loaded: &Result<Arc<SamplePreview>, String>,
    large: bool,
) {
    let plugin = deck.plugin.id;
    let color = deck.accent;
    let screen = rack::screen(ui.painter(), rect);
    let rail = if large { 14.0 } else { 11.0 };
    let footer = if large { 13.0 } else { 4.0 };
    let inner = Rect::from_min_max(screen.min + vec2(8.0, rail), screen.max - vec2(8.0, footer));
    ui.painter()
        .hline(inner.x_range(), inner.center().y, rack::graticule());
    let Ok(sample) = loaded else {
        rack::print(
            ui.painter(),
            inner.center(),
            Align2::CENTER_CENTER,
            "No sample loaded",
            rack::INK_FAINT,
        );
        rack::screen_glass(ui.painter(), rect);
        return;
    };
    let starts = slice_starts(deck.plugin);
    let regions = slice_regions(&starts);
    let count = starts.len();
    let current = selected(ui, plugin, count);
    let x_of = |value: f64| inner.left() + value as f32 * inner.width();
    let value_at = |x: f32| f64::from(((x - inner.left()) / inner.width()).clamp(0.0, 1.0));
    let slice_at = |value: f64| {
        regions
            .iter()
            .position(|&(start, end)| (start..end).contains(&value))
    };
    // Slices in time order, and each slice's place in it.
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| starts[a].total_cmp(&starts[b]));
    let mut rank = vec![0; count];
    for (place, &slice) in order.iter().enumerate() {
        rank[slice] = place;
    }
    // Neighbouring slices may come no closer than this.
    let gap = (32.0 / sample.frames.max(1) as f64).min(0.001);

    let (from, to) = regions[current];
    ui.painter().rect_filled(
        Rect::from_x_y_ranges(x_of(from)..=x_of(to), inner.y_range()),
        0.0,
        color.gamma_multiply(0.14),
    );
    for (index, peak) in sample.peaks.iter().enumerate() {
        let position = index as f64 / sample.peaks.len().max(1) as f64;
        let stroke = match slice_at(position) {
            None => color.gamma_multiply(0.22),
            Some(slice) if rank[slice].is_multiple_of(2) => color,
            Some(_) => color.gamma_multiply(0.6),
        };
        let x = x_of(position);
        ui.painter().line_segment(
            [
                pos2(x, inner.center().y - peak[1] * inner.height() * 0.48),
                pos2(x, inner.center().y - peak[0] * inner.height() * 0.48),
            ],
            Stroke::new(1.0, stroke),
        );
    }

    let body = ui.interact(inner, Id::new(("slicer-body", plugin)), Sense::click());
    let held_id = Id::new(("slicer-audition", plugin));
    let held = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(held_id))
        .unwrap_or(false);
    let down = body.is_pointer_button_down_on() && ui.input(|input| input.pointer.primary_down());
    if down
        && !held
        && let Some(pointer) = body.interact_pointer_pos()
        && let Some(slice) = slice_at(value_at(pointer.x))
    {
        select(ui, plugin, slice);
        if let Some(key) = slice_key(deck, slice) {
            deck.actions
                .push(Action::AuditionInstrumentKey { id: plugin, key });
        }
    }
    if held && !down {
        deck.actions.push(Action::ReleaseMidiAudition);
    }
    ui.ctx().data_mut(|data| data.insert_temp(held_id, down));
    if body.double_clicked()
        && let Some(pointer) = body.interact_pointer_pos()
    {
        let at = value_at(pointer.x);
        if count < MAX_SLICES && at < 1.0 - gap && starts.iter().all(|s| (s - at).abs() > gap) {
            let mut added = starts.clone();
            added.push(at);
            added.sort_by(f64::total_cmp);
            if let Some(slice) = added.iter().position(|&start| start == at) {
                select(ui, plugin, slice);
            }
            deck.actions.push(Action::SetSlices {
                id: plugin,
                starts: added,
                label: "Add Slice",
            });
        }
    }
    body.on_hover_text(if count < MAX_SLICES {
        "Press a slice to hear it; double-click to cut a new slice here"
    } else {
        "Press a slice to hear it (a Slicer holds at most 64 slices)"
    });

    for (place, &slice) in order.iter().enumerate() {
        let x = x_of(starts[slice]);
        let next = order
            .get(place + 1)
            .map_or(inner.right(), |&s| x_of(starts[s]));
        let label = (slice + 1).to_string();
        let flag_width = 5.0 + 5.0 * label.len() as f32;
        let roomy = next - x >= flag_width + 2.0;
        let flag = Rect::from_min_max(
            pos2(x, screen.top() + 1.0),
            pos2(x + if roomy { flag_width } else { 4.0 }, inner.top() - 1.0),
        );
        let grab = flag.union(Rect::from_min_max(
            pos2(x - 4.0, inner.top()),
            pos2(x + 4.0, inner.top() + inner.height() * 0.3),
        ));
        let response = ui.interact(
            grab,
            Id::new(("slicer-marker", plugin, slice)),
            Sense::click_and_drag(),
        );
        let hot = response.hovered() || response.dragged() || slice == current;
        let marker = if hot {
            color
        } else {
            color.gamma_multiply(0.75)
        };
        ui.painter().vline(
            x,
            inner.y_range(),
            Stroke::new(if hot { 1.5 } else { 1.0 }, marker),
        );
        ui.painter().rect_filled(flag, 1.5, marker);
        if roomy {
            ui.painter().text(
                flag.center(),
                Align2::CENTER_CENTER,
                &label,
                theme::prop(8.0),
                theme::TEXT_ON_COLOR,
            );
        }
        if large
            && next - x >= 26.0
            && let Some(key) = slice_key(deck, slice)
        {
            rack::print(
                ui.painter(),
                pos2((x + next) / 2.0, screen.bottom() - footer / 2.0 - 1.0),
                Align2::CENTER_CENTER,
                &note_name(key),
                if slice == current {
                    rack::INK_BRIGHT
                } else {
                    rack::INK_FAINT
                },
            );
        }
        if response.drag_started() || response.clicked() {
            select(ui, plugin, slice);
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let lower = place
                .checked_sub(1)
                .map_or(0.0, |before| starts[order[before]] + gap);
            let upper = order
                .get(place + 1)
                .map_or(1.0 - gap, |&after| starts[after] - gap);
            let value = value_at(pointer.x).clamp(lower, upper.max(lower));
            if value != starts[slice] {
                deck.set(SLICE_BASE + slice as u32, value);
            }
        }
        if response.secondary_clicked() && count > 1 {
            let mut remaining = starts.clone();
            remaining.remove(slice);
            remaining.sort_by(f64::total_cmp);
            select(ui, plugin, slice.min(count - 2));
            deck.actions.push(Action::SetSlices {
                id: plugin,
                starts: remaining,
                label: "Remove Slice",
            });
        }
        let key = slice_key(deck, slice).map_or_else(|| "no key".to_owned(), note_name);
        response
            .on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text(format!(
                "Slice {} ({key}). Drag to move its start; right-click to remove it.",
                slice + 1
            ));
    }
    rack::screen_glass(ui.painter(), rect);
}

pub(super) fn compact(ui: &mut Ui, deck: &mut Deck) {
    let loaded = preview(ui, deck.plugin);
    let width = ui.available_width();
    toolbar(ui, deck, &loaded, true);
    let (plot, _) = ui.allocate_exact_size(vec2(width, 62.0), Sense::hover());
    slice_screen(ui, plot, deck, &loaded, false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        deck.choose(ui, SLICE_MODE, &sympoie::builtin::PLAYBACK_MODES, 72.0);
        deck.choose(ui, SLICE_DETECT, &SLICE_DETECTORS, 78.0);
        deck.switch(ui, SLICE_CHOKE, "Choke");
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (id, label) in [
            (SLICE_ROOT, "Root"),
            (SLICE_TRANSPOSE, "Semi"),
            (SLICE_SENSITIVITY, "Sens"),
            (SLICE_MIN_LENGTH, "Min"),
            (SLICE_FADE, "Fade"),
            (ATTACK, "Attack"),
            (RELEASE, "Release"),
        ] {
            deck.knob(ui, id, label);
        }
        let gap = ui.available_width() - rack::KNOB_CELL.x - 9.0;
        if gap > 0.0 {
            ui.add_space(gap);
        }
        divider(ui, rack::KNOB_CELL.y);
        deck.knob(ui, GAIN, "Gain");
    });
}

pub(super) fn editor(ui: &mut Ui, deck: &mut Deck) {
    let loaded = preview(ui, deck.plugin);
    let full = 3.0 * SECTION + 2.0 * SECTION_GAP;
    ui.allocate_ui_with_layout(
        vec2(full, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(full);
            ui.spacing_mut().item_spacing.y = GAP;
            toolbar(ui, deck, &loaded, false);
            let (plot, _) = ui.allocate_exact_size(vec2(full, 170.0), Sense::hover());
            slice_screen(ui, plot, deck, &loaded, true);
            let (line, _) = ui.allocate_exact_size(vec2(full, 12.0), Sense::hover());
            rack::print(
                ui.painter(),
                line.left_center(),
                Align2::LEFT_CENTER,
                &slice_summary(ui, deck, &loaded),
                rack::INK_FAINT,
            );
        },
    );
    ui.horizontal_top(|ui| {
        section(ui, "Slicing", SECTION, |ui| {
            deck.choose(ui, SLICE_DETECT, &SLICE_DETECTORS, 90.0);
            match deck.value(SLICE_DETECT).round() as u32 {
                2 => deck.knobs(ui, &[(SLICE_DIVISIONS, "Divisions")]),
                _ => deck.knobs(
                    ui,
                    &[(SLICE_SENSITIVITY, "Sens"), (SLICE_MIN_LENGTH, "Min slice")],
                ),
            }
        });
        section(ui, "Playback", SECTION, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                deck.choose(ui, SLICE_MODE, &sympoie::builtin::PLAYBACK_MODES, 72.0);
                deck.switch(ui, SLICE_REVERSE, "Reverse");
                deck.switch(ui, SLICE_CHOKE, "Choke");
            });
            deck.knobs(
                ui,
                &[
                    (SLICE_ROOT, "Root"),
                    (SLICE_TRANSPOSE, "Semi"),
                    (SLICE_TUNE, "Tune"),
                    (SLICE_FADE, "Fade"),
                ],
            );
        });
        section(ui, "Filter & amp", SECTION, |ui| {
            deck.knobs(ui, &[(SLICE_CUTOFF, "Cutoff"), (SLICE_RESONANCE, "Res")]);
            envelope_row(ui, deck, ATTACK);
        });
    });
}

fn slice_summary(ui: &Ui, deck: &Deck, loaded: &Result<Arc<SamplePreview>, String>) -> String {
    let Ok(sample) = loaded else {
        return "Load a sample, then Analyze to slice it by transient, volume or grid".to_owned();
    };
    let starts = slice_starts(deck.plugin);
    let count = starts.len();
    let slice = selected(ui, deck.plugin.id, count);
    let (start, end) = slice_regions(&starts)[slice];
    let seconds = (end - start) * sample.frames as f64 / f64::from(sample.sample_rate);
    let keys = match (slice_key(deck, 0), slice_key(deck, count - 1)) {
        (Some(first), Some(last)) if count > 1 => {
            format!("keys {}–{}", note_name(first), note_name(last))
        }
        (Some(first), _) => format!("key {}", note_name(first)),
        _ => "no keys".to_owned(),
    };
    format!(
        "Slice {} of {count} · {} · {seconds:.3}s   |   {count} slice{} on {keys}",
        slice + 1,
        slice_key(deck, slice).map_or_else(|| "no key".to_owned(), note_name),
        plural(count),
    )
}
