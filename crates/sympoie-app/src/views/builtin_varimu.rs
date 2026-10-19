//! Faceplate for the Vari-Mu. Unlike the shared rack skin it is drawn as a piece of
//! broadcast hardware: hammertone enamel, cream silkscreen, bakelite chicken-head knobs, a
//! stepped time-constant rotary, bat-handle toggles and a backlit needle meter.

use super::*;
use crate::ui::analog;
use eframe::egui::{CursorIcon, Mesh, Painter, Pos2, Shape, Vec2};
use sympoie::builtin::varimu::{
    ATTACK, COLOR, DETECTOR_HP, EXTERNAL, INPUT, MIX, OUTPUT, RATIO, RELEASE, STEREO, THRESHOLD,
    TIME_CONSTANT, TIME_CONSTANTS, VARIABLE_TIME,
};

const ENAMEL_TOP: Color32 = Color32::from_rgb(0x3e, 0x4b, 0x47);
const ENAMEL_BOTTOM: Color32 = Color32::from_rgb(0x2a, 0x34, 0x31);
const SILK: Color32 = Color32::from_rgb(0xea, 0xe0, 0xc6);
const SILK_DIM: Color32 = Color32::from_rgb(0x95, 0x95, 0x84);
const BRASS: Color32 = Color32::from_rgb(0xc8, 0xa2, 0x5c);
const FACE: Color32 = Color32::from_rgb(0xf6, 0xe7, 0xb9);
const FACE_EDGE: Color32 = Color32::from_rgb(0xc4, 0xa9, 0x6c);
const FACE_OFF: Color32 = Color32::from_rgb(0x5e, 0x55, 0x43);
const PRINT: Color32 = Color32::from_rgb(0x2b, 0x24, 0x1b);
const RED: Color32 = Color32::from_rgb(0xb4, 0x30, 0x27);
const AMBER: Color32 = Color32::from_rgb(0xff, 0xa6, 0x38);

const KNOB_SWEEP: f32 = 2.36;

#[path = "builtin_varimu_scope.rs"]
mod scope_view;

pub(super) fn scope(
    ui: &mut Ui,
    plugin: &PluginInstance,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    scope_view::show(ui, plugin, telemetry, actions);
}

fn scope_button(ui: &mut Ui, deck: &mut Deck, rect: Rect, s: f32) {
    let response = plate_button(
        ui,
        rect,
        Id::new(("varimu-open-scope", deck.plugin.id)),
        "Scope",
        s,
    );
    if response
        .on_hover_text("Open the before/after waveform and spectrum scope")
        .clicked()
    {
        deck.actions.push(Action::OpenBuiltinScope(deck.plugin.id));
    }
}
const RATIO_LABELS: [&str; 3] = ["Gentle", "Comp", "Limit"];
const STEREO_LABELS: [&str; 3] = ["Link", "Dual", "M·S"];

pub(super) fn compact(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    const DESIGN: Vec2 = vec2(432.0, 164.0);
    let s = (ui.available_width() / DESIGN.x).clamp(0.5, 1.0);
    let (rect, _) = ui.allocate_exact_size(DESIGN * s, Sense::hover());
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    let painter = ui.painter().clone();
    plate(&painter, rect, s);
    for corner in [
        at(6.0, 6.0),
        at(426.0, 6.0),
        at(6.0, 158.0),
        at(426.0, 158.0),
    ] {
        screw(&painter, corner, 2.6 * s, 0.6);
    }

    let (reduction, output) = levels(telemetry);
    let mode = Id::new(("varimu-meter-mode", deck.plugin.id));
    let showing_output = meter_block(
        ui,
        deck,
        Rect::from_min_size(at(14.0, 10.0), vec2(126.0, 78.0) * s),
        Rect::from_min_size(at(14.0, 98.0), vec2(48.0, 30.0) * s),
        mode,
        s,
        (reduction, output),
    );
    toggle_param(
        ui,
        deck,
        EXTERNAL,
        Rect::from_min_size(at(68.0, 98.0), vec2(42.0, 30.0) * s),
        s,
        &["Int", "Ext"],
    );
    jewel(&painter, at(126.0, 113.0), 5.5 * s, glow(deck, reduction));
    silk(
        &painter,
        at(14.0, 136.0),
        Align2::LEFT_TOP,
        "Vari-Mu",
        11.0 * s,
        SILK,
    );
    silk(
        &painter,
        at(14.0, 150.0),
        Align2::LEFT_TOP,
        if showing_output {
            "Output · VU"
        } else {
            "Tube limiter"
        },
        6.0 * s,
        SILK_DIM,
    );
    scope_button(
        ui,
        deck,
        Rect::from_min_size(at(100.0, 137.0), vec2(40.0, 14.0) * s),
        s,
    );

    for (id, x, label) in [
        (INPUT, 178.0, "Input"),
        (THRESHOLD, 232.0, "Threshold"),
        (OUTPUT, 286.0, "Output"),
    ] {
        knob(
            ui,
            deck,
            id,
            at(x, 50.0),
            15.0 * s,
            s,
            label,
            Style::Compact,
            true,
        );
    }
    selector(ui, deck, TIME_CONSTANT, at(370.0, 46.0), 13.0 * s, s, false);
    silk(
        &painter,
        at(370.0, 76.0),
        Align2::CENTER_TOP,
        "Time const",
        6.0 * s,
        SILK,
    );

    rule(
        &painter,
        at(152.0, 100.0).y,
        at(152.0, 0.0).x..=at(424.0, 0.0).x,
        s,
    );
    silk(
        &painter,
        at(175.0, 105.0),
        Align2::CENTER_TOP,
        "Ratio",
        6.0 * s,
        SILK,
    );
    toggle_param(
        ui,
        deck,
        RATIO,
        Rect::from_min_size(at(152.0, 116.0), vec2(48.0, 40.0) * s),
        s,
        &RATIO_LABELS,
    );
    let variable = deck.value(TIME_CONSTANT).round() as usize == VARIABLE_TIME;
    for (id, x, label, active) in [
        (ATTACK, 222.0, "Attack", variable),
        (RELEASE, 258.0, "Release", variable),
        (DETECTOR_HP, 294.0, "SC HP", true),
        (COLOR, 330.0, "Color", true),
        (MIX, 366.0, "Mix", true),
    ] {
        knob(
            ui,
            deck,
            id,
            at(x, 134.0),
            9.5 * s,
            s,
            label,
            Style::Compact,
            active,
        );
    }
    silk(
        &painter,
        at(410.0, 105.0),
        Align2::CENTER_TOP,
        "Stereo",
        6.0 * s,
        SILK,
    );
    toggle_param(
        ui,
        deck,
        STEREO,
        Rect::from_min_size(at(388.0, 116.0), vec2(44.0, 40.0) * s),
        s,
        &STEREO_LABELS,
    );
}

pub(super) fn editor(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    const DESIGN: Vec2 = vec2(700.0, 380.0);
    let available = ui.available_rect_before_wrap();
    ui.allocate_rect(available, Sense::hover());
    let s = (available.width() / DESIGN.x)
        .min(available.height() / DESIGN.y)
        .clamp(0.5, 2.0);
    let rect = Rect::from_center_size(available.center(), DESIGN * s);
    let rect = rect.translate(vec2(
        (available.left() - rect.left()).max(0.0),
        (available.top() - rect.top()).max(0.0),
    ));
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    let painter = ui.painter().clone();
    let window = ui.max_rect().expand(8.0).intersect(ui.clip_rect());
    plate(&painter, window, s);
    rack_ears(&painter, window, s);

    let plugin = deck.plugin;
    silk(
        &painter,
        at(24.0, 16.0),
        Align2::LEFT_TOP,
        "Sympoie",
        7.5 * s,
        BRASS,
    );
    silk(
        &painter,
        at(24.0, 27.0),
        Align2::LEFT_TOP,
        "Vari-Mu",
        22.0 * s,
        SILK,
    );
    silk(
        &painter,
        at(24.0, 56.0),
        Align2::LEFT_TOP,
        "Variable-mu tube limiting amplifier",
        7.0 * s,
        SILK_DIM,
    );
    let (reduction, output) = levels(telemetry);
    let lamp = at(640.0, 34.0);
    let power = ui.interact(
        Rect::from_center_size(lamp, Vec2::splat(26.0 * s)),
        Id::new(("varimu-power", plugin.id)),
        Sense::click(),
    );
    jewel(&painter, lamp, 8.0 * s, glow(deck, reduction));
    silk(
        &painter,
        at(640.0, 50.0),
        Align2::CENTER_TOP,
        "Power",
        6.5 * s,
        SILK,
    );
    if power
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Power: switch the unit in or out")
        .clicked()
    {
        deck.actions.push(Action::SetVst3Bypassed {
            id: plugin.id,
            bypassed: !plugin.bypassed,
        });
    }
    scope_button(
        ui,
        deck,
        Rect::from_min_size(at(484.0, 26.0), vec2(56.0, 18.0) * s),
        s,
    );
    let reset = plate_button(
        ui,
        Rect::from_min_size(at(548.0, 26.0), vec2(56.0, 18.0) * s),
        Id::new(("varimu-reset", plugin.id)),
        "Reset",
        s,
    );
    if reset
        .on_hover_text("Reset every control to its default")
        .clicked()
    {
        deck.actions
            .push(Action::ResetBuiltinParameters { id: plugin.id });
    }
    rule(
        &painter,
        at(0.0, 76.0).y,
        at(24.0, 0.0).x..=at(676.0, 0.0).x,
        s,
    );

    let mode = Id::new(("varimu-meter-mode", plugin.id));
    let showing_output = meter_block(
        ui,
        deck,
        Rect::from_min_size(at(240.0, 90.0), vec2(220.0, 128.0) * s),
        Rect::from_min_size(at(262.0, 228.0), vec2(56.0, 32.0) * s),
        mode,
        s,
        (reduction, output),
    );
    let readout = Rect::from_min_size(at(338.0, 234.0), vec2(100.0, 20.0) * s);
    painter.add(analog::readout(readout, false));
    let (text, color) = if showing_output {
        (format!("OUT {}", level_text(output)), SILK)
    } else if reduction > 0.05 {
        (format!("GR −{reduction:.1} dB"), AMBER)
    } else {
        ("GR 0.0 dB".to_owned(), SILK)
    };
    painter.text(
        readout.center(),
        Align2::CENTER_CENTER,
        text,
        theme::mono(9.0 * s),
        color,
    );

    for (id, x, label) in [(INPUT, 78.0, "Input"), (THRESHOLD, 168.0, "Threshold")] {
        knob(
            ui,
            deck,
            id,
            at(x, 150.0),
            28.0 * s,
            s,
            label,
            Style::Editor,
            true,
        );
    }
    selector(ui, deck, TIME_CONSTANT, at(526.0, 150.0), 24.0 * s, s, true);
    silk(
        &painter,
        at(526.0, 198.0),
        Align2::CENTER_TOP,
        "Time constant",
        7.5 * s,
        SILK,
    );
    knob(
        ui,
        deck,
        OUTPUT,
        at(632.0, 150.0),
        28.0 * s,
        s,
        "Output",
        Style::Editor,
        true,
    );

    let bay = Rect::from_min_max(at(24.0, 274.0), at(676.0, 366.0));
    recess(&painter, bay, s);
    for (x, title) in [(34.0, "Detector"), (312.0, "Stereo & key"), (536.0, "Tone")] {
        silk(
            &painter,
            at(x, 282.0),
            Align2::LEFT_TOP,
            title,
            6.5 * s,
            BRASS,
        );
    }
    for x in [300.0, 524.0] {
        rule_vertical(
            &painter,
            at(x, 0.0).x,
            at(0.0, 284.0).y..=at(0.0, 358.0).y,
            s,
        );
    }
    toggle_param(
        ui,
        deck,
        RATIO,
        Rect::from_min_size(at(36.0, 302.0), vec2(62.0, 44.0) * s),
        s,
        &RATIO_LABELS,
    );
    let variable = deck.value(TIME_CONSTANT).round() as usize == VARIABLE_TIME;
    for (id, x, label, active) in [
        (ATTACK, 132.0, "Attack", variable),
        (RELEASE, 192.0, "Release", variable),
        (DETECTOR_HP, 252.0, "SC HP", true),
        (COLOR, 578.0, "Color", true),
        (MIX, 638.0, "Mix", true),
    ] {
        knob(
            ui,
            deck,
            id,
            at(x, 326.0),
            15.0 * s,
            s,
            label,
            Style::Compact,
            active,
        );
    }
    toggle_param(
        ui,
        deck,
        STEREO,
        Rect::from_min_size(at(314.0, 302.0), vec2(56.0, 44.0) * s),
        s,
        &STEREO_LABELS,
    );
    toggle_param(
        ui,
        deck,
        EXTERNAL,
        Rect::from_min_size(at(380.0, 302.0), vec2(50.0, 44.0) * s),
        s,
        &["Int", "Ext"],
    );
    let routed = plugin.sidechain.is_some();
    let source = plate_button(
        ui,
        Rect::from_min_size(at(440.0, 312.0), vec2(72.0, 18.0) * s),
        Id::new(("varimu-source", plugin.id)),
        "Source…",
        s,
    );
    silk(
        &painter,
        at(476.0, 336.0),
        Align2::CENTER_TOP,
        if routed { "Key routed" } else { "No key" },
        6.0 * s,
        if routed { BRASS } else { SILK_DIM },
    );
    if source
        .on_hover_text("Choose the track that keys the detector")
        .clicked()
    {
        deck.actions.push(Action::OpenVst3Sidechain(plugin.id));
    }
}

fn levels(telemetry: Option<&Telemetry>) -> (f32, f32) {
    let read = |value: &std::sync::atomic::AtomicU32| f32::from_bits(value.load(Ordering::Relaxed));
    telemetry.map_or((0.0, -120.0), |t| {
        (read(&t.reduction_db).max(0.0), read(&t.output_db))
    })
}

fn glow(deck: &Deck, reduction: f32) -> f32 {
    if deck.plugin.bypassed {
        0.0
    } else {
        0.8 + 0.2 * (reduction / 12.0).clamp(0.0, 1.0)
    }
}

/// Meter with its GR / OUT switch; returns whether it shows output level.
fn meter_block(
    ui: &mut Ui,
    deck: &Deck,
    face: Rect,
    switch: Rect,
    mode: Id,
    s: f32,
    (reduction, output): (f32, f32),
) -> bool {
    let mut showing_output = ui
        .ctx()
        .data_mut(|data| data.get_temp::<bool>(mode))
        .unwrap_or(false);
    if let Some(picked) = toggle(
        ui,
        mode,
        switch,
        s,
        &["GR", "Out"],
        usize::from(showing_output),
    ) {
        showing_output = picked == 1;
        ui.ctx()
            .data_mut(|data| data.insert_temp(mode, showing_output));
    }
    let target = if showing_output {
        // 0 VU sits at −18 dBFS, the usual alignment for tracking levels.
        vu_position(output + 18.0)
    } else {
        10.0_f32.powf(-reduction / 20.0)
    };
    let position = needle(ui, mode.with("needle"), target);
    let response = ui.interact(face, mode.with("face"), Sense::hover());
    meter(
        ui.painter(),
        face,
        s,
        position,
        showing_output,
        glow(deck, reduction),
    );
    response.on_hover_text(format!(
        "Gain reduction −{reduction:.1} dB · Output {}",
        level_text(output)
    ));
    showing_output
}

fn vu_position(vu: f32) -> f32 {
    10.0_f32.powf((vu - 3.0) / 20.0)
}

/// Mechanical needle: a damped spring tuned to VU ballistics (about 300 ms to settle
/// with a slight overshoot).
fn needle(ui: &Ui, id: Id, target: f32) -> f32 {
    const OMEGA: f32 = 13.0;
    const ZETA: f32 = 0.78;
    let now = ui.input(|input| input.time);
    let (mut position, mut velocity, last) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(f32, f32, f64)>(id))
        .unwrap_or((target, 0.0, now));
    let mut remaining = (now - last).clamp(0.0, 0.1) as f32;
    while remaining > 0.0 {
        let step = remaining.min(0.004);
        let acceleration = OMEGA * OMEGA * (target - position) - 2.0 * ZETA * OMEGA * velocity;
        velocity += acceleration * step;
        position += velocity * step;
        remaining -= step;
    }
    if !(-0.03..=1.06).contains(&position) {
        position = position.clamp(-0.03, 1.06);
        velocity = 0.0;
    }
    ui.ctx()
        .data_mut(|data| data.insert_temp(id, (position, velocity, now)));
    position
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Compact,
    Editor,
}

#[allow(clippy::too_many_arguments)]
fn knob(
    ui: &mut Ui,
    deck: &mut Deck,
    id: u32,
    center: Pos2,
    radius: f32,
    s: f32,
    label: &str,
    style: Style,
    active: bool,
) {
    let Some(spec) = deck.spec(id).copied() else {
        return;
    };
    let value = deck.value(id);
    let normalized = spec.normalize(value) as f32;
    let response = ui.interact(
        Rect::from_center_size(center, Vec2::splat(radius * 2.6)),
        deck.param_id(id),
        Sense::click_and_drag(),
    );
    if response.dragged() {
        let speed = if ui.input(|input| input.modifiers.shift) {
            0.0012
        } else {
            0.0055
        };
        let delta = response.drag_delta();
        let next = (normalized + (delta.x - delta.y) * speed).clamp(0.0, 1.0);
        if (next - normalized).abs() > 1.0e-6 {
            deck.set(id, spec.denormalize(f64::from(next)));
        }
    }
    if response.double_clicked() {
        deck.set(id, spec.default);
    }
    deck.entry(&response, &spec, value);
    let hot = response.hovered() || response.dragged();
    let painter = ui.painter();
    let angle = -KNOB_SWEEP + normalized * 2.0 * KNOB_SWEEP;
    for tick in 0..=10 {
        let t = -KNOB_SWEEP + 2.0 * KNOB_SWEEP * tick as f32 / 10.0;
        let direction = vec2(t.sin(), -t.cos());
        let major = tick % 5 == 0;
        painter.line_segment(
            [
                center + direction * radius * 1.16,
                center + direction * radius * if major { 1.34 } else { 1.26 },
            ],
            Stroke::new(if major { 1.2 } else { 0.8 } * s.max(0.7), SILK_DIM),
        );
    }
    if style == Style::Editor {
        for t in [0.0, 0.5, 1.0] {
            let a = -KNOB_SWEEP + 2.0 * KNOB_SWEEP * t;
            painter.text(
                center + vec2(a.sin(), -a.cos()) * radius * 1.56,
                Align2::CENTER_CENTER,
                legend(&spec, spec.denormalize(f64::from(t))),
                theme::mono(7.5 * s),
                SILK_DIM,
            );
        }
    }
    chicken_head(painter, center, radius, angle, s, hot);
    let ink = if active { SILK } else { SILK_DIM };
    let readout = compact_value(&spec, value);
    let size = if style == Style::Editor { 7.5 } else { 6.0 } * s;
    if style == Style::Editor {
        silk(
            painter,
            center + vec2(0.0, radius * 1.62),
            Align2::CENTER_TOP,
            label,
            size,
            ink,
        );
        painter.text(
            center + vec2(0.0, radius * 1.62 + 11.0 * s),
            Align2::CENTER_TOP,
            readout,
            theme::mono(8.0 * s),
            if hot { AMBER } else { SILK_DIM },
        );
    } else {
        silk(
            painter,
            center - vec2(0.0, radius * 1.36 + 1.0 * s),
            Align2::CENTER_BOTTOM,
            label,
            size,
            ink,
        );
        painter.text(
            center + vec2(0.0, radius * 1.32 + 1.5 * s),
            Align2::CENTER_TOP,
            readout,
            theme::mono(6.8 * s),
            if hot { AMBER } else { SILK_DIM },
        );
    }
    let note = if active {
        ""
    } else {
        " Used when Time constant is set to Var."
    };
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(format!(
            "{}: {}.{note} Drag; Shift for fine control; double-click to reset; right-click to type a value.",
            spec.name,
            display(&spec, value)
        ));
}

/// Stepped rotary for the time constant; click a legend to jump straight to it.
fn selector(ui: &mut Ui, deck: &mut Deck, id: u32, center: Pos2, radius: f32, s: f32, large: bool) {
    let Some(spec) = deck.spec(id).copied() else {
        return;
    };
    let count = TIME_CONSTANTS.len();
    let selected = (deck.value(id).round().max(0.0) as usize).min(count - 1);
    let span = 2.1_f32;
    let angle = |index: usize| -span + 2.0 * span * index as f32 / (count - 1) as f32;
    let key = deck.param_id(id);
    let response = ui.interact(
        Rect::from_center_size(center, Vec2::splat(radius * 2.3)),
        key,
        Sense::click_and_drag(),
    );
    let travel = key.with("travel");
    if response.drag_started() {
        ui.ctx().data_mut(|data| data.insert_temp(travel, 0.0_f32));
    }
    let mut next = None;
    if response.dragged() {
        let delta = response.drag_delta();
        let mut moved = ui
            .ctx()
            .data_mut(|data| data.get_temp::<f32>(travel))
            .unwrap_or(0.0)
            + delta.x
            - delta.y;
        let notch = 16.0 * s;
        let mut position = selected as i32;
        while moved >= notch {
            moved -= notch;
            position += 1;
        }
        while moved <= -notch {
            moved += notch;
            position -= 1;
        }
        let position = position.clamp(0, count as i32 - 1) as usize;
        if position != selected {
            next = Some(position);
        }
        ui.ctx().data_mut(|data| data.insert_temp(travel, moved));
    } else if response.clicked() {
        next = Some((selected + 1) % count);
    }
    if response.double_clicked() {
        next = Some(spec.default as usize);
    }
    let painter = ui.painter().clone();
    let font = theme::mono(if large { 8.5 } else { 6.8 } * s);
    for (index, label) in TIME_CONSTANTS.iter().enumerate() {
        let a = angle(index);
        let direction = vec2(a.sin(), -a.cos());
        let on = index == selected;
        painter.line_segment(
            [
                center + direction * radius * 1.14,
                center + direction * radius * 1.3,
            ],
            Stroke::new(1.1 * s.max(0.7), if on { BRASS } else { SILK_DIM }),
        );
        let spot = center + direction * radius * 1.62;
        let hit = ui.interact(
            Rect::from_center_size(spot, vec2(16.0, 12.0) * s),
            key.with(("position", index)),
            Sense::click(),
        );
        let hot = hit.hovered();
        painter.text(
            spot,
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if on {
                AMBER
            } else if hot {
                SILK
            } else {
                SILK_DIM
            },
        );
        if hit.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            next = Some(index);
        }
    }
    let hot = response.hovered() || response.dragged();
    chicken_head(&painter, center, radius, angle(selected), s, hot);
    if let Some(index) = next
        && index != selected
    {
        deck.set(id, index as f64);
    }
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(format!(
            "Time constant {}: {}. 1–4 fixed; 5 and 6 recover fast from a lone peak but slowly after sustained limiting; Var uses Attack and Release. Drag, click to step, or click a number.",
            TIME_CONSTANTS[selected],
            time_constant_text(selected)
        ));
}

fn time_constant_text(position: usize) -> &'static str {
    [
        "attack 0.2 ms, release 0.3 s",
        "attack 0.2 ms, release 0.8 s",
        "attack 0.4 ms, release 2 s",
        "attack 0.4 ms, release 5 s",
        "attack 0.4 ms, release 2 s / 10 s program",
        "attack 0.2 ms, release 0.3 s / 12 s program",
        "attack and release from the controls",
    ][position.min(VARIABLE_TIME)]
}

fn toggle_param(ui: &mut Ui, deck: &mut Deck, id: u32, rect: Rect, s: f32, labels: &[&str]) {
    let selected = (deck.value(id).round().max(0.0) as usize).min(labels.len() - 1);
    let key = deck.param_id(id);
    if let Some(picked) = toggle(ui, key, rect, s, labels, selected)
        && picked != selected
    {
        deck.set(id, picked as f64);
    }
}

/// Bat-handle toggle with stacked legends; click the lever to step, or a legend to pick.
fn toggle(
    ui: &mut Ui,
    key: Id,
    rect: Rect,
    s: f32,
    labels: &[&str],
    selected: usize,
) -> Option<usize> {
    let count = labels.len();
    let response = ui.interact(rect, key, Sense::click());
    let mut picked = response.clicked().then_some((selected + 1) % count);
    let painter = ui.painter().clone();
    let nut = pos2(rect.left() + 9.0 * s, rect.center().y);
    let pitch = if count == 3 { 11.0 } else { 8.0 } * s;
    let row = |index: usize| rect.center().y + (index as f32 - (count - 1) as f32 / 2.0) * pitch;
    for (index, label) in labels.iter().enumerate() {
        let spot = pos2(rect.left() + 20.0 * s, row(index));
        let hit = ui.interact(
            Rect::from_min_max(
                pos2(spot.x - 2.0 * s, spot.y - pitch / 2.0),
                pos2(rect.right(), spot.y + pitch / 2.0),
            ),
            key.with(("legend", index)),
            Sense::click(),
        );
        let on = index == selected;
        silk(
            &painter,
            spot,
            Align2::LEFT_CENTER,
            label,
            6.0 * s,
            if on || hit.hovered() { SILK } else { SILK_DIM },
        );
        if on {
            painter.circle_filled(spot - vec2(4.0 * s, 0.0), 1.3 * s, AMBER);
        }
        if hit.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            picked = Some(index);
        }
    }
    let hot = response.hovered();
    // Hex nut.
    let hex: Vec<Pos2> = (0..6)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 6.0 + 0.52;
            nut + 6.0 * s * vec2(a.cos(), a.sin())
        })
        .collect();
    painter.add(Shape::convex_polygon(
        hex.iter().map(|p| *p + vec2(0.0, 1.0 * s)).collect(),
        Color32::from_black_alpha(110),
        Stroke::NONE,
    ));
    fan(&painter, &hex, nut - vec2(2.0, 2.5) * s, |p| {
        let d = ((p - nut).length() / (7.0 * s)).clamp(0.0, 1.0);
        Color32::from_gray(0xd0).lerp_to_gamma(Color32::from_gray(0x55), d)
    });
    painter.add(Shape::closed_line(
        hex,
        Stroke::new(0.8, Color32::from_black_alpha(170)),
    ));
    painter.circle_filled(nut, 3.4 * s, Color32::from_gray(0x2a));
    // Lever: up, centre (pointing at the viewer) or down.
    let middle = count == 3 && selected == 1;
    let tip = if middle {
        nut
    } else {
        let up = selected == 0;
        nut + vec2(0.0, if up { -1.0 } else { 1.0 } * 11.0 * s)
    };
    if !middle {
        let base = nut;
        let side = vec2(1.6 * s, 0.0);
        painter.add(Shape::convex_polygon(
            vec![base - side, base + side, tip + side * 1.3, tip - side * 1.3],
            if hot {
                Color32::from_gray(0xe2)
            } else {
                Color32::from_gray(0xc4)
            },
            Stroke::new(0.8, Color32::from_black_alpha(160)),
        ));
    }
    let ball = if middle { 3.6 } else { 2.9 } * s;
    fan(
        &painter,
        &circle(tip, ball, 18),
        tip - vec2(1.0, 1.0) * s,
        |p| {
            let d = ((p - tip).length() / (ball * 1.6)).clamp(0.0, 1.0);
            Color32::from_gray(0xff).lerp_to_gamma(Color32::from_gray(0x70), d)
        },
    );
    painter.circle_stroke(tip, ball, Stroke::new(0.8, Color32::from_black_alpha(160)));
    response.on_hover_cursor(CursorIcon::PointingHand);
    picked
}

fn plate_button(ui: &mut Ui, rect: Rect, id: Id, label: &str, s: f32) -> egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    let pressed = response.is_pointer_button_down_on();
    let cap = analog::push_button(
        ui.painter(),
        rect,
        2.0 * s,
        None,
        response.hovered(),
        pressed,
    );
    silk(
        ui.painter(),
        cap.center(),
        Align2::CENTER_CENTER,
        label,
        6.5 * s,
        SILK,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

fn legend(spec: &ParameterSpec, value: f64) -> String {
    match spec.unit {
        "dB" if value.abs() < 0.05 => "0".to_owned(),
        "dB" if spec.min < 0.0 && spec.max > 0.0 => format!("{value:+.0}"),
        "Hz" if value >= 1000.0 => format!("{:.0}k", value / 1000.0),
        "ms" if value < 1.0 => format!("{value:.1}"),
        _ => format!("{value:.0}"),
    }
}

fn level_text(db: f32) -> String {
    if db <= -90.0 {
        "−∞".to_owned()
    } else {
        format!("{db:.1} dB")
    }
}

// --- Drawing --------------------------------------------------------------------------

pub(super) fn circle(center: Pos2, radius: f32, steps: usize) -> Vec<Pos2> {
    (0..steps)
        .map(|step| {
            let angle = std::f32::consts::TAU * step as f32 / steps as f32;
            center + radius * vec2(angle.cos(), angle.sin())
        })
        .collect()
}

pub(super) fn fan(painter: &Painter, outline: &[Pos2], hub: Pos2, shade: impl Fn(Pos2) -> Color32) {
    if outline.len() < 3 {
        return;
    }
    let mut mesh = Mesh::default();
    mesh.colored_vertex(hub, shade(hub));
    for point in outline {
        mesh.colored_vertex(*point, shade(*point));
    }
    let count = outline.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % count);
    }
    painter.add(Shape::mesh(mesh));
}

pub(super) fn silk(
    painter: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    size: f32,
    color: Color32,
) {
    let galley = rack::spaced(painter, text, theme::prop(size), color, size * 0.16);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
}

pub(super) fn hash(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

/// Hammertone enamel: a soft vertical gradient with a fixed speckle so the finish does
/// not shimmer from frame to frame.
fn plate(painter: &Painter, rect: Rect, s: f32) {
    painter.rect_filled(rect, 4.0 * s, ENAMEL_BOTTOM);
    analog::gradient_rect(painter, rect.shrink(1.0), ENAMEL_TOP, ENAMEL_BOTTOM);
    let pitch = (6.0 * s).max(3.5);
    let mut mesh = Mesh::default();
    let columns = (rect.width() / pitch) as i32;
    let rows = (rect.height() / pitch) as i32;
    for row in 0..rows {
        for column in 0..columns {
            let h = hash(column, row);
            let jitter = vec2((h & 0xff) as f32 / 255.0, ((h >> 8) & 0xff) as f32 / 255.0);
            let center = rect.min + (vec2(column as f32, row as f32) + jitter) * pitch;
            let size = (0.8 + ((h >> 16) & 0x7) as f32 * 0.25) * s.max(0.7);
            let color = if h & 0x0100_0000 != 0 {
                Color32::from_white_alpha(5 + ((h >> 20) & 0x7) as u8)
            } else {
                Color32::from_black_alpha(14 + ((h >> 20) & 0xf) as u8)
            };
            mesh.add_colored_rect(Rect::from_center_size(center, Vec2::splat(size)), color);
        }
    }
    painter.add(Shape::mesh(mesh));
    painter.hline(
        (rect.left() + 3.0)..=(rect.right() - 3.0),
        rect.top() + 1.5,
        Stroke::new(1.0, Color32::from_white_alpha(26)),
    );
    painter.rect_stroke(
        rect,
        4.0 * s,
        Stroke::new(1.0, Color32::from_black_alpha(230)),
        egui::StrokeKind::Inside,
    );
}

fn rack_ears(painter: &Painter, window: Rect, s: f32) {
    let width = 22.0 * s;
    for left in [true, false] {
        let ear = if left {
            Rect::from_min_max(window.min, pos2(window.left() + width, window.bottom()))
        } else {
            Rect::from_min_max(pos2(window.right() - width, window.top()), window.max)
        };
        painter.rect_filled(ear, 0.0, Color32::from_black_alpha(40));
        let edge = if left { ear.right() } else { ear.left() };
        painter.vline(
            edge,
            ear.y_range(),
            Stroke::new(1.0, Color32::from_black_alpha(120)),
        );
        painter.vline(
            edge + 1.0,
            ear.y_range(),
            Stroke::new(1.0, Color32::from_white_alpha(14)),
        );
        for y in [ear.top() + 28.0 * s, ear.bottom() - 28.0 * s] {
            let slot = Rect::from_center_size(pos2(ear.center().x, y), vec2(9.0, 16.0) * s);
            painter.rect_filled(slot, 4.5 * s, Color32::from_gray(0x0c));
            painter.rect_stroke(
                slot,
                4.5 * s,
                Stroke::new(1.0, Color32::from_white_alpha(18)),
                egui::StrokeKind::Outside,
            );
            screw(painter, slot.center(), 3.4 * s, 1.1);
        }
    }
}

fn recess(painter: &Painter, rect: Rect, s: f32) {
    painter.rect_filled(rect, 3.0 * s, Color32::from_black_alpha(52));
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(140)),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
}

fn rule(painter: &Painter, y: f32, x: std::ops::RangeInclusive<f32>, s: f32) {
    painter.hline(
        x.clone(),
        y,
        Stroke::new(1.0 * s.max(0.7), BRASS.gamma_multiply(0.55)),
    );
    painter.hline(x, y + 1.0, Stroke::new(1.0, Color32::from_black_alpha(90)));
}

fn rule_vertical(painter: &Painter, x: f32, y: std::ops::RangeInclusive<f32>, s: f32) {
    painter.vline(
        x,
        y.clone(),
        Stroke::new(1.0 * s.max(0.7), Color32::from_black_alpha(120)),
    );
    painter.vline(x + 1.0, y, Stroke::new(1.0, Color32::from_white_alpha(14)));
}

pub(super) fn screw(painter: &Painter, center: Pos2, radius: f32, angle: f32) {
    painter.circle_filled(
        center + vec2(0.0, 0.8),
        radius + 0.6,
        Color32::from_black_alpha(120),
    );
    let hub = center - vec2(0.3, 0.4) * radius;
    fan(painter, &circle(center, radius, 16), hub, |p| {
        let d = ((p - hub).length() / (radius * 1.7)).clamp(0.0, 1.0);
        Color32::from_gray(0xc0).lerp_to_gamma(Color32::from_gray(0x4c), d)
    });
    let direction = vec2(angle.cos(), angle.sin()) * radius * 0.8;
    painter.line_segment(
        [center - direction, center + direction],
        Stroke::new((radius * 0.32).max(0.8), Color32::from_gray(0x22)),
    );
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(0.7, Color32::from_black_alpha(170)),
    );
}

/// Bakelite chicken-head: a hub with a long pointer and a short tail, both tangent to it
/// so the outline stays convex, with a cream inlay along the pointer.
pub(super) fn chicken_head(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    angle: f32,
    s: f32,
    hot: bool,
) {
    let hub = radius * 0.58;
    let front = radius * 1.1;
    let back = radius * 0.8;
    // Shaft washer under the head.
    painter.circle_filled(
        center + vec2(0.0, 1.5 * s.max(0.6)),
        hub * 1.12,
        Color32::from_black_alpha(110),
    );
    let heading = vec2(angle.sin(), -angle.cos());
    let theta = heading.y.atan2(heading.x);
    let front_spread = (hub / front).acos();
    let back_spread = (hub / back).acos();
    let mut outline = vec![center + heading * front];
    let arc = |from: f32, to: f32, outline: &mut Vec<Pos2>| {
        for step in 0..=10 {
            let a = from + (to - from) * step as f32 / 10.0;
            outline.push(center + hub * vec2(a.cos(), a.sin()));
        }
    };
    arc(
        theta + front_spread,
        theta + std::f32::consts::PI - back_spread,
        &mut outline,
    );
    outline.push(center - heading * back);
    arc(
        theta + std::f32::consts::PI + back_spread,
        theta + std::f32::consts::TAU - front_spread,
        &mut outline,
    );
    painter.add(Shape::convex_polygon(
        outline
            .iter()
            .map(|p| *p + vec2(1.0, 2.4) * s.max(0.6))
            .collect(),
        Color32::from_black_alpha(95),
        Stroke::NONE,
    ));
    let light = center + vec2(-0.35, -0.5) * hub;
    let (top, rim) = if hot {
        (
            Color32::from_rgb(0x5e, 0x57, 0x50),
            Color32::from_rgb(0x14, 0x12, 0x10),
        )
    } else {
        (
            Color32::from_rgb(0x4a, 0x44, 0x3e),
            Color32::from_rgb(0x10, 0x0f, 0x0d),
        )
    };
    fan(painter, &outline, light, |p| {
        let d = ((p - light).length() / (front * 1.5)).clamp(0.0, 1.0);
        top.lerp_to_gamma(rim, d)
    });
    painter.add(Shape::closed_line(
        outline,
        Stroke::new(1.0, Color32::from_black_alpha(220)),
    ));
    painter.line_segment(
        [
            center + heading * hub * 0.15,
            center + heading * (front - 2.8 * s.max(0.6)),
        ],
        Stroke::new(1.7 * s.max(0.6), SILK),
    );
    painter.circle_filled(
        light,
        (hub * 0.22).max(1.0),
        Color32::from_white_alpha(if hot { 40 } else { 26 }),
    );
}

pub(super) fn jewel(painter: &Painter, center: Pos2, radius: f32, glow: f32) {
    if glow > 0.0 {
        for (grow, alpha) in [(2.4, 0.08), (1.7, 0.16), (1.3, 0.24)] {
            painter.circle_filled(center, radius * grow, AMBER.gamma_multiply(alpha * glow));
        }
    }
    let bezel = radius * 1.3;
    fan(
        painter,
        &circle(center, bezel, 24),
        center - vec2(0.3, 0.5) * bezel,
        |p| {
            let d = ((p - center).length() / bezel).clamp(0.0, 1.0);
            Color32::from_gray(0xd8).lerp_to_gamma(Color32::from_gray(0x44), d)
        },
    );
    let dark = Color32::from_rgb(0x4a, 0x24, 0x0c);
    let core = dark.lerp_to_gamma(AMBER.lerp_to_gamma(Color32::WHITE, 0.35), glow);
    let edge = dark.lerp_to_gamma(Color32::from_rgb(0xc0, 0x5a, 0x10), glow);
    fan(painter, &circle(center, radius, 24), center, |p| {
        let d = ((p - center).length() / radius).clamp(0.0, 1.0);
        core.lerp_to_gamma(edge, d)
    });
    for facet in 0..6 {
        let a = std::f32::consts::TAU * facet as f32 / 6.0;
        painter.line_segment(
            [center, center + radius * 0.9 * vec2(a.cos(), a.sin())],
            Stroke::new(0.6, Color32::from_black_alpha(40)),
        );
    }
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(0.8, Color32::from_black_alpha(180)),
    );
    painter.circle_filled(
        center - vec2(0.35, 0.4) * radius,
        radius * 0.22,
        Color32::from_white_alpha(130),
    );
}

/// Backlit moving-coil meter. `position` runs 0..1 across the arc; gain reduction reads
/// from the rest position at the right, output reads as a standard VU scale.
fn meter(painter: &Painter, rect: Rect, s: f32, position: f32, output: bool, glow: f32) {
    painter.rect_filled(
        rect.expand(2.0 * s),
        6.0 * s,
        Color32::from_black_alpha(200),
    );
    fan(
        painter,
        &[
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ],
        rect.center() - vec2(0.0, rect.height() * 0.3),
        |p| {
            let t = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            Color32::from_gray(0x3a).lerp_to_gamma(Color32::from_gray(0x10), t)
        },
    );
    let face = rect.shrink(4.0 * s);
    let lit = FACE_OFF.lerp_to_gamma(FACE, glow);
    let shade = FACE_OFF
        .lerp_to_gamma(Color32::from_rgb(0x2a, 0x25, 0x1c), 0.5)
        .lerp_to_gamma(FACE_EDGE, glow);
    let hub = pos2(face.center().x, face.top() + face.height() * 0.6);
    let reach = (face.width() * 0.5).hypot(face.height() * 0.6);
    let outline: Vec<Pos2> = (0..32)
        .map(|i| {
            let t = i as f32 / 8.0;
            let side = (t as usize).min(3);
            let f = t - side as f32;
            match side {
                0 => face.left_top() + vec2(face.width() * f, 0.0),
                1 => face.right_top() + vec2(0.0, face.height() * f),
                2 => face.right_bottom() - vec2(face.width() * f, 0.0),
                _ => face.left_bottom() - vec2(0.0, face.height() * f),
            }
        })
        .collect();
    fan(painter, &outline, hub, |p| {
        lit.lerp_to_gamma(shade, ((p - hub).length() / reach).clamp(0.0, 1.0))
    });

    let painter = painter.with_clip_rect(face.intersect(painter.clip_rect()));
    let h = face.height();
    let pivot = pos2(face.center().x, face.bottom() + h * 0.42);
    let radius = h * 1.12;
    let sweep = 0.66_f32;
    let ink = PRINT.lerp_to_gamma(Color32::from_rgb(0x1a, 0x18, 0x14), 1.0 - glow);
    let at = |t: f32, r: f32| {
        let a = -sweep + 2.0 * sweep * t;
        pivot + r * vec2(a.sin(), -a.cos())
    };
    let arc = |from: f32, to: f32, r: f32| -> Vec<Pos2> {
        (0..=40)
            .map(|i| at(from + (to - from) * i as f32 / 40.0, r))
            .collect()
    };
    let line = 1.0 * s.max(0.7);
    painter.add(Shape::line(arc(0.0, 1.0, radius), Stroke::new(line, ink)));
    let red_from = vu_position(0.0);
    if output {
        painter.add(Shape::line(
            arc(red_from, 1.0, radius + 2.0 * s),
            Stroke::new(3.2 * s, RED),
        ));
    }
    let marks: &[(f32, &str)] = if output {
        &[
            (-20.0, "20"),
            (-10.0, "10"),
            (-7.0, "7"),
            (-5.0, "5"),
            (-3.0, "3"),
            (-2.0, ""),
            (-1.0, ""),
            (0.0, "0"),
            (1.0, ""),
            (2.0, ""),
            (3.0, "+3"),
        ]
    } else {
        &[
            (20.0, "20"),
            (14.0, ""),
            (10.0, "10"),
            (8.0, ""),
            (6.0, "6"),
            (4.0, "4"),
            (3.0, "3"),
            (2.0, "2"),
            (1.0, "1"),
            (0.0, "0"),
        ]
    };
    let numbers = theme::prop((6.6 * h / 70.0).clamp(5.0, 12.0));
    for &(value, label) in marks {
        let t = if output {
            vu_position(value)
        } else {
            10.0_f32.powf(-value / 20.0)
        };
        let major = !label.is_empty();
        let color = if output && value > 0.0 { RED } else { ink };
        painter.line_segment(
            [
                at(t, radius),
                at(t, radius + if major { 6.0 } else { 3.5 } * s),
            ],
            Stroke::new(line, color),
        );
        if major {
            painter.text(
                at(t, radius + 12.5 * s),
                Align2::CENTER_CENTER,
                label,
                numbers.clone(),
                color,
            );
        }
    }
    let caption = if output { "VU" } else { "Gain reduction" };
    let caption_size = if output { 15.0 } else { 7.0 } * (h / 70.0).clamp(0.6, 1.9);
    let galley = rack::spaced(&painter, caption, theme::prop(caption_size), ink, 0.8);
    let caption = Align2::CENTER_CENTER
        .anchor_size(pos2(face.center().x, face.top() + h * 0.64), galley.size());
    painter.galley(caption.min, galley, ink);
    let brand = rack::spaced(
        &painter,
        "Sympoie",
        theme::prop(5.5 * (h / 70.0).clamp(0.7, 1.9)),
        ink.gamma_multiply(0.7),
        0.6,
    );
    painter.galley(
        pos2(
            face.left() + 5.0 * s,
            face.bottom() - brand.size().y - 3.0 * s,
        ),
        brand,
        ink,
    );

    let angle = -sweep + 2.0 * sweep * position;
    let direction = vec2(angle.sin(), -angle.cos());
    let start = pivot + direction * (h * 0.5);
    let end = pivot + direction * (radius + 9.0 * s);
    painter.line_segment(
        [start + vec2(1.5, 1.5) * s, end + vec2(1.5, 1.5) * s],
        Stroke::new(1.3 * s.max(0.7), Color32::from_black_alpha(45)),
    );
    painter.line_segment([start, end], Stroke::new(1.3 * s.max(0.7), PRINT));
    let cover: Vec<Pos2> = (0..=20)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 20.0);
            pos2(
                face.center().x + face.width() * 0.13 * a.cos(),
                face.bottom() + h * 0.17 * a.sin(),
            )
        })
        .collect();
    painter.add(Shape::convex_polygon(
        cover,
        Color32::from_gray(0x16),
        Stroke::new(1.0, Color32::from_black_alpha(160)),
    ));
    analog::glass(&painter, face);
    painter.rect_stroke(
        face,
        2.0 * s,
        Stroke::new(1.0, Color32::from_black_alpha(160)),
        egui::StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(
        ctx: &egui::Context,
        instance: &PluginInstance,
        id: Id,
        aim: impl Fn(Rect) -> Pos2,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let frame = |events, actions: &mut Vec<Action>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(432.0, 172.0))),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| super::super::super::compact(ui, instance, BuiltinKind::VariMu, None, actions),
            );
            output.textures_delta.clear();
        };
        frame(vec![], &mut actions);
        frame(vec![], &mut actions);
        let point = aim(ctx.read_response(id).expect("control").rect);
        frame(vec![egui::Event::PointerMoved(point)], &mut actions);
        for pressed in [true, false] {
            frame(
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                &mut actions,
            );
        }
        actions
    }

    #[test]
    fn time_constant_legend_and_ratio_toggle_select_their_positions() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::VariMu.instance();
        instance.id = 5;
        let param = |id: u32| Id::new(("builtin-inline-param", 5_u64, id));
        let spec = |id: u32| {
            builtin::parameters(BuiltinKind::VariMu)
                .into_iter()
                .find(|spec| spec.id == id)
                .unwrap()
        };
        let actions = click(
            &ctx,
            &instance,
            param(TIME_CONSTANT).with(("position", 6_usize)),
            |rect| rect.center(),
        );
        assert!(matches!(
            actions.as_slice(),
            [Action::SetVst3Parameter { id: 5, parameter: TIME_CONSTANT, value }]
                if spec(TIME_CONSTANT).denormalize(*value) == VARIABLE_TIME as f64
        ));
        // Clicking the lever steps from Compress to Limit.
        let actions = click(&ctx, &instance, param(RATIO), |rect| {
            rect.left_center() + vec2(9.0, 0.0)
        });
        assert!(matches!(
            actions.as_slice(),
            [Action::SetVst3Parameter { id: 5, parameter: RATIO, value }]
                if spec(RATIO).denormalize(*value) == 2.0
        ));
    }
}
