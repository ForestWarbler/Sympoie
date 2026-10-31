//! Faceplate for Ferro, drawn as classic synthesizer hardware rather than the shared rack
//! skin: walnut end cheeks, a black panel with cream silkscreen, knurled knobs with
//! numbered silver skirts, chicken-head range and waveform selectors, rocker switches,
//! and a lower strip of slide faders and lit push buttons after the Japanese polysynths.

use super::varimu::{chicken_head, circle, fan, hash, jewel, screw, silk};
use super::*;
use crate::ui::analog;
use eframe::egui::{CursorIcon, Mesh, Painter, Pos2, Shape, Vec2};
use sympoie::builtin::RenderState;
use sympoie::builtin::ferro::{
    AMP_ATTACK, BEND_RANGE, CHORUS, CONTOUR, CUTOFF, DRIFT, DRIVE, EMPHASIS, ENGINE, FEEDBACK,
    FILTER_ATTACK, FILTER_VELOCITY, GAIN, GLIDE, GLIDE_CURVE, GLIDE_MODE, HPF, KEY_TRACK,
    KEY_TRACKS, LFO_DELAY, LFO_RATE, LFO_SHAPE, LFO_SHAPES, MAX_VOICES, MOD_FILTER, MOD_PITCH,
    MOD_PWM, MOD_SOURCE, MOD_SOURCES, NOISE, NOISE_COLOR, OSC_BASE, OSC_LEVEL, OSC_ON, OSC_RANGE,
    OSC_STRIDE, OSC_TUNE, OSC_WAVE, OSC3_KEYBOARD, PRESETS, RANGES, SUB, TUNE, VELOCITY,
    VOICE_MODE, VOICES,
};

const PANEL_TOP: Color32 = Color32::from_rgb(0x1d, 0x1c, 0x1b);
const PANEL_BOTTOM: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0e);
const STRIP_TOP: Color32 = Color32::from_rgb(0x2b, 0x2a, 0x28);
const STRIP_BOTTOM: Color32 = Color32::from_rgb(0x1e, 0x1d, 0x1c);
const CREAM: Color32 = Color32::from_rgb(0xf0, 0xe6, 0xcc);
const CREAM_DIM: Color32 = Color32::from_rgb(0x9c, 0x95, 0x84);
const WALNUT_LIGHT: Color32 = Color32::from_rgb(0x8a, 0x55, 0x2e);
const WALNUT_DARK: Color32 = Color32::from_rgb(0x4a, 0x2a, 0x15);
const AMBER: Color32 = Color32::from_rgb(0xff, 0xa6, 0x38);
const RED: Color32 = Color32::from_rgb(0xe8, 0x4a, 0x32);
const ORANGE: Color32 = Color32::from_rgb(0xf0, 0x86, 0x2a);
const YELLOW: Color32 = Color32::from_rgb(0xf2, 0xc8, 0x3a);
const ROCKER_BLUE: Color32 = Color32::from_rgb(0x3d, 0x6f, 0xb8);
const ROCKER_WHITE: Color32 = Color32::from_rgb(0xe8, 0xe4, 0xd8);

const KNOB_SWEEP: f32 = 2.36;
const WAVE_LABELS: [&str; 6] = ["Tri", "Shark", "Saw", "Sq", "Wide", "Narr"];
const CHORUS_LABELS: [&str; 2] = ["I", "II"];
/// Short legends for the three-way selectors, which sit close together.
const GLIDE_LEGENDS: [&str; 3] = ["Off", "On", "Leg"];
const VOICE_LEGENDS: [&str; 3] = ["Poly", "Mono", "Leg"];

fn osc(index: u32, offset: u32) -> u32 {
    OSC_BASE + index * OSC_STRIDE + offset
}

// --- Editor ---------------------------------------------------------------------------

pub(super) fn editor(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    const DESIGN: Vec2 = vec2(1180.0, 440.0);
    let available = ui.available_rect_before_wrap();
    ui.allocate_rect(available, Sense::hover());
    let s = (available.width() / DESIGN.x)
        .min(available.height() / DESIGN.y)
        .clamp(0.45, 2.0);
    let rect = Rect::from_center_size(available.center(), DESIGN * s);
    let rect = rect.translate(vec2(
        (available.left() - rect.left()).max(0.0),
        (available.top() - rect.top()).max(0.0),
    ));
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    let painter = ui.painter().clone();
    let window = ui.max_rect().expand(8.0).intersect(ui.clip_rect());
    painter.rect_filled(window, 0.0, PANEL_BOTTOM);

    let cheek = 34.0;
    walnut(
        &painter,
        Rect::from_min_max(at(0.0, 0.0), at(cheek, DESIGN.y)),
        s,
        1,
    );
    walnut(
        &painter,
        Rect::from_min_max(at(DESIGN.x - cheek, 0.0), at(DESIGN.x, DESIGN.y)),
        s,
        2,
    );
    let panel = Rect::from_min_max(at(cheek, 0.0), at(DESIGN.x - cheek, 304.0));
    let strip = Rect::from_min_max(at(cheek, 304.0), at(DESIGN.x - cheek, DESIGN.y));
    brushed(&painter, panel, PANEL_TOP, PANEL_BOTTOM, s);
    brushed(&painter, strip, STRIP_TOP, STRIP_BOTTOM, s);
    painter.hline(
        strip.x_range(),
        strip.top(),
        Stroke::new(2.0 * s, Color32::from_black_alpha(200)),
    );
    for corner in [
        at(44.0, 10.0),
        at(1136.0, 10.0),
        at(44.0, 430.0),
        at(1136.0, 430.0),
    ] {
        screw(&painter, corner, 3.0 * s, 0.7);
    }

    title(ui, deck, at(54.0, 12.0), s, true);
    let reset = plate_button(
        ui,
        Rect::from_min_size(at(1060.0, 18.0), vec2(56.0, 18.0) * s),
        Id::new(("ferro-reset", deck.plugin.id)),
        "Reset",
        s,
    );
    if reset
        .on_hover_text("Reset every control to its default")
        .clicked()
    {
        deck.actions
            .push(Action::ResetBuiltinParameters { id: deck.plugin.id });
    }
    preset_display(
        ui,
        deck,
        Rect::from_min_size(at(800.0, 16.0), vec2(250.0, 24.0) * s),
        s,
    );

    // Section headers sit in a cream band, the way the panel legends do on the hardware.
    for (label, from, to) in [
        ("Controllers", 44.0, 236.0),
        ("Oscillator bank", 246.0, 516.0),
        ("Mixer", 526.0, 706.0),
        ("Modifiers", 716.0, 1040.0),
        ("Output", 1050.0, 1136.0),
    ] {
        header_band(&painter, at(from, 58.0), at(to, 72.0), label, s);
    }

    // Controllers.
    let r = 15.0 * s;
    knob(ui, deck, TUNE, at(80.0, 110.0), r, s, "Tune");
    knob(ui, deck, GLIDE, at(140.0, 110.0), r, s, "Glide");
    knob(ui, deck, GLIDE_CURVE, at(200.0, 110.0), r, s, "Curve");
    stepped(
        ui,
        deck,
        GLIDE_MODE,
        at(80.0, 185.0),
        11.0 * s,
        s,
        &GLIDE_LEGENDS,
        "Glide mode",
    );
    stepped(
        ui,
        deck,
        VOICE_MODE,
        at(140.0, 185.0),
        11.0 * s,
        s,
        &VOICE_LEGENDS,
        "Voice",
    );
    knob(ui, deck, VOICES, at(200.0, 185.0), r, s, "Voices");
    knob(ui, deck, BEND_RANGE, at(80.0, 255.0), r, s, "Bend");
    knob(ui, deck, DRIFT, at(140.0, 255.0), r, s, "Drift");
    divider(&painter, at(241.0, 78.0), at(241.0, 296.0), s);

    // Oscillator bank: range, tune and waveform for each VCO.
    for index in 0..3 {
        let y = 104.0 + index as f32 * 66.0;
        silk(
            &painter,
            at(256.0, y),
            Align2::LEFT_CENTER,
            &format!("{}", index + 1),
            11.0 * s,
            CREAM,
        );
        stepped(
            ui,
            deck,
            osc(index, OSC_RANGE),
            at(312.0, y),
            11.0 * s,
            s,
            &RANGES,
            "Range",
        );
        knob(
            ui,
            deck,
            osc(index, OSC_TUNE),
            at(392.0, y),
            13.0 * s,
            s,
            "Tune",
        );
        stepped(
            ui,
            deck,
            osc(index, OSC_WAVE),
            at(468.0, y),
            11.0 * s,
            s,
            &WAVE_LABELS,
            "Wave",
        );
    }
    rocker(
        ui,
        deck,
        OSC3_KEYBOARD,
        Rect::from_min_size(at(256.0, 282.0), vec2(20.0, 14.0) * s),
        s,
        "Osc 3 keyboard",
    );
    silk(
        &painter,
        at(280.0, 289.0),
        Align2::LEFT_CENTER,
        "Osc 3 ctrl",
        6.0 * s,
        CREAM_DIM,
    );
    divider(&painter, at(521.0, 78.0), at(521.0, 296.0), s);

    // Mixer: a level and a rocker for each source.
    for index in 0..3 {
        let y = 110.0 + index as f32 * 70.0;
        knob(
            ui,
            deck,
            osc(index, OSC_LEVEL),
            at(566.0, y),
            13.0 * s,
            s,
            &format!("Osc {}", index + 1),
        );
        rocker(
            ui,
            deck,
            osc(index, OSC_ON),
            Rect::from_min_size(at(592.0, y - 8.0), vec2(20.0, 16.0) * s),
            s,
            &format!("Osc {} on", index + 1),
        );
    }
    knob(ui, deck, SUB, at(660.0, 110.0), 13.0 * s, s, "Sub");
    knob(ui, deck, NOISE, at(660.0, 180.0), 13.0 * s, s, "Noise");
    rocker(
        ui,
        deck,
        NOISE_COLOR,
        Rect::from_min_size(at(684.0, 172.0), vec2(16.0, 16.0) * s),
        s,
        "Noise color: white or pink",
    );
    knob(
        ui,
        deck,
        FEEDBACK,
        at(660.0, 250.0),
        13.0 * s,
        s,
        "Feedback",
    );
    divider(&painter, at(711.0, 78.0), at(711.0, 296.0), s);

    // Modifiers: the filter and its contour, then the loudness contour.
    knob(ui, deck, CUTOFF, at(756.0, 106.0), 18.0 * s, s, "Cutoff");
    knob(ui, deck, EMPHASIS, at(826.0, 110.0), r, s, "Emphasis");
    knob(ui, deck, CONTOUR, at(884.0, 110.0), r, s, "Contour");
    knob(ui, deck, DRIVE, at(942.0, 110.0), r, s, "Drive");
    stepped(
        ui,
        deck,
        KEY_TRACK,
        at(1004.0, 110.0),
        11.0 * s,
        s,
        &KEY_TRACKS,
        "Keyboard",
    );
    contour_row(
        ui,
        deck,
        FILTER_ATTACK,
        FILTER_VELOCITY,
        196.0,
        "Filter contour",
        at,
        s,
    );
    contour_row(
        ui,
        deck,
        AMP_ATTACK,
        VELOCITY,
        264.0,
        "Loudness contour",
        at,
        s,
    );
    divider(&painter, at(1045.0, 78.0), at(1045.0, 296.0), s);

    // Output.
    knob(ui, deck, GAIN, at(1093.0, 116.0), 19.0 * s, s, "Volume");
    power(ui, deck, at(1093.0, 205.0), s);
    voice_lamps(&painter, at(1062.0, 262.0), s, telemetry, 2);

    // Lower strip.
    for (label, from, to, color) in [
        ("LFO", 44.0, 236.0, RED),
        ("Modulation", 246.0, 516.0, ORANGE),
        ("HPF", 526.0, 606.0, YELLOW),
        ("Chorus", 616.0, 786.0, YELLOW),
        ("Engine", 796.0, 1136.0, AMBER),
    ] {
        strip_band(&painter, at(from, 314.0), at(to, 326.0), label, color, s);
    }
    fader(
        ui,
        deck,
        LFO_RATE,
        Rect::from_min_size(at(56.0, 336.0), vec2(26.0, 84.0) * s),
        s,
        "Rate",
    );
    fader(
        ui,
        deck,
        LFO_DELAY,
        Rect::from_min_size(at(94.0, 336.0), vec2(26.0, 84.0) * s),
        s,
        "Delay",
    );
    button_row(
        ui,
        deck,
        LFO_SHAPE,
        &LFO_SHAPES,
        at(132.0, 340.0),
        vec2(48.0, 16.0),
        2,
        s,
        RED,
    );
    button_row(
        ui,
        deck,
        MOD_SOURCE,
        &MOD_SOURCES,
        at(256.0, 340.0),
        vec2(48.0, 16.0),
        1,
        s,
        ORANGE,
    );
    fader(
        ui,
        deck,
        MOD_PITCH,
        Rect::from_min_size(at(326.0, 336.0), vec2(26.0, 84.0) * s),
        s,
        "Pitch",
    );
    fader(
        ui,
        deck,
        MOD_FILTER,
        Rect::from_min_size(at(386.0, 336.0), vec2(26.0, 84.0) * s),
        s,
        "Filter",
    );
    fader(
        ui,
        deck,
        MOD_PWM,
        Rect::from_min_size(at(446.0, 336.0), vec2(26.0, 84.0) * s),
        s,
        "PWM",
    );
    fader(
        ui,
        deck,
        HPF,
        Rect::from_min_size(at(552.0, 336.0), vec2(26.0, 84.0) * s),
        s,
        "HPF",
    );
    chorus_buttons(ui, deck, at(636.0, 352.0), s);
    engine(
        ui,
        deck,
        Rect::from_min_size(at(808.0, 334.0), vec2(320.0, 90.0) * s),
        s,
        telemetry,
    );

    ui.ctx().request_repaint_after(Duration::from_millis(33));
}

// --- Compact rack card ----------------------------------------------------------------

pub(super) fn compact(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    const DESIGN: Vec2 = vec2(432.0, 164.0);
    let s = (ui.available_width() / DESIGN.x).clamp(0.5, 1.0);
    let (rect, _) = ui.allocate_exact_size(DESIGN * s, Sense::hover());
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    let painter = ui.painter().clone();
    let cheek = 12.0;
    walnut(
        &painter,
        Rect::from_min_max(at(0.0, 0.0), at(cheek, DESIGN.y)),
        s,
        3,
    );
    walnut(
        &painter,
        Rect::from_min_max(at(DESIGN.x - cheek, 0.0), at(DESIGN.x, DESIGN.y)),
        s,
        4,
    );
    let panel = Rect::from_min_max(at(cheek, 0.0), at(DESIGN.x - cheek, DESIGN.y));
    brushed(&painter, panel, PANEL_TOP, PANEL_BOTTOM, s);
    title(ui, deck, at(20.0, 6.0), s, false);
    voice_lamps(&painter, at(300.0, 14.0), s, telemetry, 1);
    let open = plate_button(
        ui,
        Rect::from_min_size(at(372.0, 8.0), vec2(46.0, 16.0) * s),
        Id::new(("ferro-open-panel", deck.plugin.id)),
        "Panel",
        s,
    );
    if open
        .on_hover_text("Open the full Ferro panel in its own window")
        .clicked()
    {
        deck.actions
            .push(Action::OpenNativeVst3Editor(deck.plugin.id));
    }

    let r = 13.0 * s;
    for (id, x, label) in [
        (CUTOFF, 44.0, "Cutoff"),
        (EMPHASIS, 102.0, "Emphasis"),
        (CONTOUR, 160.0, "Contour"),
        (GLIDE, 218.0, "Glide"),
        (DRIFT, 276.0, "Drift"),
        (GAIN, 334.0, "Volume"),
    ] {
        knob_styled(ui, deck, id, at(x, 64.0), r, s, label, false);
    }
    stepped(
        ui,
        deck,
        VOICE_MODE,
        at(390.0, 64.0),
        10.0 * s,
        s,
        &VOICE_LEGENDS,
        "Voice",
    );
    chorus_buttons_compact(ui, deck, at(24.0, 122.0), s);
    engine_compact(
        ui,
        deck,
        Rect::from_min_size(at(150.0, 116.0), vec2(262.0, 36.0) * s),
        s,
        telemetry,
    );
    ui.ctx().request_repaint_after(Duration::from_millis(33));
}

// --- Pieces of the panel --------------------------------------------------------------

fn title(ui: &mut Ui, deck: &mut Deck, origin: Pos2, s: f32, large: bool) {
    let painter = ui.painter();
    let ink = if deck.plugin.bypassed {
        CREAM_DIM
    } else {
        CREAM
    };
    silk(
        painter,
        origin,
        Align2::LEFT_TOP,
        "Sympoie",
        if large { 7.0 } else { 6.0 } * s,
        AMBER,
    );
    silk(
        painter,
        origin + vec2(0.0, if large { 9.0 } else { 8.0 } * s),
        Align2::LEFT_TOP,
        "Ferro",
        if large { 24.0 } else { 15.0 } * s,
        ink,
    );
    if large {
        silk(
            painter,
            origin + vec2(118.0 * s, 22.0 * s),
            Align2::LEFT_TOP,
            "Analog synthesizer · model F-1",
            7.0 * s,
            CREAM_DIM,
        );
    }
}

fn header_band(painter: &Painter, from: Pos2, to: Pos2, label: &str, s: f32) {
    let rect = Rect::from_min_max(from, to);
    painter.rect_filled(rect, 1.5 * s, CREAM.gamma_multiply(0.92));
    silk(
        painter,
        rect.center(),
        Align2::CENTER_CENTER,
        &label.to_uppercase(),
        6.5 * s,
        Color32::from_gray(0x18),
    );
}

fn strip_band(painter: &Painter, from: Pos2, to: Pos2, label: &str, color: Color32, s: f32) {
    let rect = Rect::from_min_max(from, to);
    painter.rect_filled(
        Rect::from_min_max(rect.left_bottom() - vec2(0.0, 2.0 * s), rect.right_bottom()),
        0.0,
        color.gamma_multiply(0.85),
    );
    silk(
        painter,
        rect.left_center() - vec2(0.0, 1.5 * s),
        Align2::LEFT_CENTER,
        &label.to_uppercase(),
        6.5 * s,
        CREAM,
    );
}

fn divider(painter: &Painter, from: Pos2, to: Pos2, s: f32) {
    painter.line_segment(
        [from, to],
        Stroke::new(1.0 * s.max(0.7), Color32::from_black_alpha(200)),
    );
    painter.line_segment(
        [from + vec2(1.0, 0.0), to + vec2(1.0, 0.0)],
        Stroke::new(1.0, Color32::from_white_alpha(12)),
    );
}

#[allow(clippy::too_many_arguments)]
fn contour_row(
    ui: &mut Ui,
    deck: &mut Deck,
    base: u32,
    velocity: u32,
    y: f32,
    label: &str,
    at: impl Fn(f32, f32) -> Pos2,
    s: f32,
) {
    silk(
        ui.painter(),
        at(1036.0, y - 27.0),
        Align2::RIGHT_CENTER,
        label,
        6.0 * s,
        CREAM_DIM,
    );
    let r = 13.0 * s;
    for (offset, x, name) in [
        (0, 756.0, "Attack"),
        (1, 812.0, "Decay"),
        (2, 868.0, "Sustain"),
        (3, 924.0, "Release"),
    ] {
        knob(ui, deck, base + offset, at(x, y), r, s, name);
    }
    knob(ui, deck, velocity, at(992.0, y), r, s, "Velocity");
}

fn power(ui: &mut Ui, deck: &mut Deck, center: Pos2, s: f32) {
    let plugin = deck.plugin;
    let response = ui.interact(
        Rect::from_center_size(center, Vec2::splat(26.0 * s)),
        Id::new(("ferro-power", plugin.id)),
        Sense::click(),
    );
    jewel(
        ui.painter(),
        center,
        7.5 * s,
        if plugin.bypassed { 0.0 } else { 1.0 },
    );
    silk(
        ui.painter(),
        center + vec2(0.0, 14.0 * s),
        Align2::CENTER_TOP,
        "Power",
        6.5 * s,
        CREAM,
    );
    if response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Power: switch the instrument in or out")
        .clicked()
    {
        deck.actions.push(Action::SetVst3Bypassed {
            id: plugin.id,
            bypassed: !plugin.bypassed,
        });
    }
}

fn voice_level(telemetry: Option<&Telemetry>, voice: usize) -> f32 {
    telemetry.map_or(0.0, |telemetry| {
        f32::from_bits(telemetry.ferro_voices[voice].load(Ordering::Relaxed)).clamp(0.0, 1.0)
    })
}

/// One small lamp per voice card, lit by its loudness contour.
fn voice_lamps(
    painter: &Painter,
    origin: Pos2,
    s: f32,
    telemetry: Option<&Telemetry>,
    rows: usize,
) {
    let per_row = MAX_VOICES / rows;
    for voice in 0..MAX_VOICES {
        let column = voice % per_row;
        let row = voice / per_row;
        let center = origin + vec2(column as f32 * 9.0, row as f32 * 10.0) * s;
        let level = voice_level(telemetry, voice);
        if level > 0.0 {
            painter.circle_filled(center, 4.5 * s, RED.gamma_multiply(0.25 * level));
        }
        painter.circle_filled(
            center,
            2.6 * s,
            Color32::from_rgb(0x30, 0x0c, 0x08)
                .lerp_to_gamma(RED.lerp_to_gamma(Color32::WHITE, 0.3), level),
        );
        painter.circle_stroke(
            center,
            2.6 * s,
            Stroke::new(0.6, Color32::from_black_alpha(200)),
        );
    }
}

fn preset_display(ui: &mut Ui, deck: &mut Deck, rect: Rect, s: f32) {
    let plugin = deck.plugin.id;
    let matching = builtin::matching_preset(deck.plugin);
    ui.painter().add(analog::readout(rect, false));
    let arrows = [
        (
            Rect::from_min_size(rect.min, vec2(22.0 * s, rect.height())),
            "◀",
            -1_i32,
        ),
        (
            Rect::from_min_size(
                rect.right_top() - vec2(22.0 * s, 0.0),
                vec2(22.0 * s, rect.height()),
            ),
            "▶",
            1,
        ),
    ];
    let name = matching.map_or("Custom", |index| PRESETS[index].name);
    let label = match matching {
        Some(index) => format!("{:02} {}", index + 1, name),
        None => name.to_owned(),
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::mono(10.0 * s),
        AMBER,
    );
    for (area, glyph, direction) in arrows {
        let response = ui.interact(
            area,
            Id::new(("ferro-preset", plugin, direction)),
            Sense::click(),
        );
        ui.painter().text(
            area.center(),
            Align2::CENTER_CENTER,
            glyph,
            theme::mono(8.0 * s),
            if response.hovered() { AMBER } else { CREAM_DIM },
        );
        if response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Factory presets. Moving any control makes the sound Custom.")
            .clicked()
        {
            let count = PRESETS.len() as i32;
            let from = matching.map_or(if direction > 0 { -1 } else { 0 }, |index| index as i32);
            let preset = (from + direction).rem_euclid(count) as usize;
            deck.actions
                .push(Action::LoadBuiltinPreset { id: plugin, preset });
        }
    }
}

fn render_status(telemetry: Option<&Telemetry>, rendering: bool) -> (RenderState, f32) {
    match telemetry {
        Some(telemetry) if rendering => (telemetry.render_state(), telemetry.render_progress()),
        _ => (RenderState::Live, 0.0),
    }
}

fn status_text(state: RenderState, progress: f32) -> String {
    match state {
        RenderState::Live => "Realtime engine".to_owned(),
        RenderState::Rendering => format!("Rendering {:.0}%", progress * 100.0),
        RenderState::Ready => "Playing analog render".to_owned(),
        RenderState::Failed => "Render failed · playing live".to_owned(),
    }
}

/// The Live / Render switch with its status lamp.
fn engine(ui: &mut Ui, deck: &mut Deck, rect: Rect, s: f32, telemetry: Option<&Telemetry>) {
    let rendering = deck.value(ENGINE) >= 0.5;
    let painter = ui.painter().clone();
    let width = 72.0 * s;
    for (index, label) in ["Live", "Render"].iter().enumerate() {
        let lit = rendering == (index == 1);
        let area = Rect::from_min_size(
            rect.min + vec2(index as f32 * (width + 10.0 * s), 18.0 * s),
            vec2(width, 22.0 * s),
        );
        let response = lit_button(
            ui,
            deck.param_id(ENGINE).with(index),
            area,
            s,
            label,
            lit,
            AMBER,
        );
        if response
            .on_hover_text(if index == 0 {
                "Live: play the realtime engine (2× oversampled)"
            } else {
                "Render: render the track's notes in the background with the accurate offline engine (8× oversampled, settled filter, continuous glides) and play that instead"
            })
            .clicked()
            && !lit
        {
            deck.set(ENGINE, index as f64);
        }
    }
    let (state, progress) = render_status(telemetry, rendering);
    let lamp = rect.min + vec2(180.0 * s, 29.0 * s);
    let (color, glow) = match state {
        RenderState::Live => (Color32::from_rgb(0x58, 0xd0, 0x6a), 1.0),
        RenderState::Rendering => {
            let time = ui.input(|input| input.time) as f32;
            (AMBER, 0.45 + 0.55 * (time * 5.0).sin().abs())
        }
        RenderState::Ready => (AMBER, 1.0),
        RenderState::Failed => (RED, 1.0),
    };
    painter.circle_filled(lamp, 8.0 * s, color.gamma_multiply(0.22 * glow));
    painter.circle_filled(lamp, 4.0 * s, Color32::BLACK.lerp_to_gamma(color, glow));
    painter.circle_stroke(
        lamp,
        4.0 * s,
        Stroke::new(0.8, Color32::from_black_alpha(220)),
    );
    silk(
        &painter,
        lamp + vec2(12.0 * s, 0.0),
        Align2::LEFT_CENTER,
        &status_text(state, progress),
        7.0 * s,
        CREAM,
    );
    // Progress bar under the switch.
    let bar = Rect::from_min_size(
        rect.min + vec2(0.0, 52.0 * s),
        vec2(rect.width() - 6.0 * s, 5.0 * s),
    );
    painter.rect_filled(bar, 2.0 * s, Color32::from_black_alpha(160));
    let filled = match state {
        RenderState::Rendering => progress,
        RenderState::Ready => 1.0,
        _ => 0.0,
    };
    if filled > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(bar.min, vec2(bar.width() * filled, bar.height())),
            2.0 * s,
            AMBER.gamma_multiply(0.85),
        );
    }
    silk(
        &painter,
        rect.min + vec2(0.0, 66.0 * s),
        Align2::LEFT_TOP,
        if rendering {
            "Edits re-render automatically · live input stays realtime"
        } else {
            "Switch to Render for the accurate offline engine"
        },
        6.0 * s,
        CREAM_DIM,
    );
}

fn engine_compact(ui: &mut Ui, deck: &mut Deck, rect: Rect, s: f32, telemetry: Option<&Telemetry>) {
    let rendering = deck.value(ENGINE) >= 0.5;
    let area = Rect::from_min_size(rect.min + vec2(0.0, 8.0 * s), vec2(60.0 * s, 20.0 * s));
    let response = lit_button(
        ui,
        deck.param_id(ENGINE),
        area,
        s,
        if rendering { "Render" } else { "Live" },
        rendering,
        AMBER,
    );
    if response
        .on_hover_text(
            "Engine: Live plays the realtime engine; Render plays a background offline render",
        )
        .clicked()
    {
        deck.set(ENGINE, if rendering { 0.0 } else { 1.0 });
    }
    let (state, progress) = render_status(telemetry, rendering);
    silk(
        ui.painter(),
        rect.min + vec2(70.0 * s, 18.0 * s),
        Align2::LEFT_CENTER,
        &status_text(state, progress),
        6.5 * s,
        if state == RenderState::Failed {
            RED
        } else {
            CREAM_DIM
        },
    );
}

fn chorus_buttons(ui: &mut Ui, deck: &mut Deck, origin: Pos2, s: f32) {
    let mode = deck.value(CHORUS).round() as u32;
    for (index, label) in CHORUS_LABELS.iter().enumerate() {
        let bit = 1 << index;
        let lit = mode & bit != 0;
        let area = Rect::from_min_size(
            origin + vec2(index as f32 * 70.0, 0.0) * s,
            vec2(56.0, 30.0) * s,
        );
        let response = lit_button(
            ui,
            deck.param_id(CHORUS).with(index),
            area,
            s,
            label,
            lit,
            YELLOW,
        );
        if response
            .on_hover_text("Chorus I and II; press both for the fast I+II setting")
            .clicked()
        {
            deck.set(CHORUS, f64::from(mode ^ bit));
        }
    }
}

fn chorus_buttons_compact(ui: &mut Ui, deck: &mut Deck, origin: Pos2, s: f32) {
    let mode = deck.value(CHORUS).round() as u32;
    silk(
        ui.painter(),
        origin - vec2(0.0, 9.5 * s),
        Align2::LEFT_BOTTOM,
        "Chorus",
        6.0 * s,
        CREAM_DIM,
    );
    for (index, label) in CHORUS_LABELS.iter().enumerate() {
        let bit = 1 << index;
        let lit = mode & bit != 0;
        let area = Rect::from_min_size(
            origin + vec2(index as f32 * 46.0, 0.0) * s,
            vec2(38.0, 22.0) * s,
        );
        if lit_button(
            ui,
            deck.param_id(CHORUS).with(index),
            area,
            s,
            label,
            lit,
            YELLOW,
        )
        .on_hover_text("Chorus I and II; press both for I+II")
        .clicked()
        {
            deck.set(CHORUS, f64::from(mode ^ bit));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn button_row(
    ui: &mut Ui,
    deck: &mut Deck,
    id: u32,
    labels: &[&str],
    origin: Pos2,
    size: Vec2,
    columns: usize,
    s: f32,
    color: Color32,
) {
    let selected = deck.value(id).round() as usize;
    let name = deck.spec(id).map_or("", |spec| spec.name);
    for (index, label) in labels.iter().enumerate() {
        let column = index % columns;
        let row = index / columns;
        let area = Rect::from_min_size(
            origin + vec2(column as f32 * (size.x + 4.0), row as f32 * (size.y + 16.0)) * s,
            size * s,
        );
        let lit = index == selected;
        if lit_button(
            ui,
            deck.param_id(id).with(index),
            area,
            s,
            label,
            lit,
            color,
        )
        .on_hover_text(format!("{name}: {label}"))
        .clicked()
            && !lit
        {
            deck.set(id, index as f64);
        }
    }
}

// --- Widgets --------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn knob(ui: &mut Ui, deck: &mut Deck, id: u32, center: Pos2, radius: f32, s: f32, label: &str) {
    knob_styled(ui, deck, id, center, radius, s, label, true);
}

/// A knurled black knob over a silver skirt, with the panel printed 0–10 around it.
#[allow(clippy::too_many_arguments)]
fn knob_styled(
    ui: &mut Ui,
    deck: &mut Deck,
    id: u32,
    center: Pos2,
    radius: f32,
    s: f32,
    label: &str,
    numbers: bool,
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
        let fine = ui.input(|input| input.modifiers.shift);
        let speed = if fine { 0.0012 } else { 0.0055 } / s.max(0.5);
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
    if numbers {
        for tick in 0..=10 {
            let t = -KNOB_SWEEP + 2.0 * KNOB_SWEEP * tick as f32 / 10.0;
            let direction = vec2(t.sin(), -t.cos());
            painter.line_segment(
                [
                    center + direction * radius * 1.38,
                    center + direction * radius * 1.5,
                ],
                Stroke::new(0.9 * s.max(0.7), CREAM_DIM),
            );
            let printed = if radius >= 17.0 * s {
                tick % 2 == 0
            } else {
                tick % 5 == 0
            };
            if printed {
                painter.text(
                    center + direction * radius * 1.78,
                    Align2::CENTER_CENTER,
                    tick.to_string(),
                    theme::mono((radius * 0.36).max(5.5)),
                    CREAM_DIM,
                );
            }
        }
    }
    // Silver skirt with its own pointer line, then the knurled cap.
    let skirt = radius * 1.3;
    painter.circle_filled(
        center + vec2(0.0, 1.5 * s),
        skirt + 0.5,
        Color32::from_black_alpha(120),
    );
    let light = center - vec2(0.35, 0.5) * skirt;
    fan(painter, &circle(center, skirt, 32), light, |p| {
        let d = ((p - light).length() / (skirt * 1.9)).clamp(0.0, 1.0);
        Color32::from_gray(0xe4).lerp_to_gamma(Color32::from_gray(0x6a), d)
    });
    painter.circle_stroke(
        center,
        skirt,
        Stroke::new(0.8, Color32::from_black_alpha(200)),
    );
    let angle = -KNOB_SWEEP + normalized * 2.0 * KNOB_SWEEP;
    let direction = vec2(angle.sin(), -angle.cos());
    painter.line_segment(
        [
            center + direction * radius * 1.02,
            center + direction * skirt * 0.97,
        ],
        Stroke::new(1.4 * s.max(0.7), Color32::from_gray(0x15)),
    );
    let cap = radius;
    let ridges = 24;
    let outline: Vec<Pos2> = (0..ridges * 2)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / (ridges * 2) as f32 + angle;
            let r = if i % 2 == 0 { cap } else { cap * 0.93 };
            center + r * vec2(a.cos(), a.sin())
        })
        .collect();
    let (top, rim) = if hot {
        (Color32::from_gray(0x4a), Color32::from_gray(0x0c))
    } else {
        (Color32::from_gray(0x38), Color32::from_gray(0x08))
    };
    let shine = center - vec2(0.3, 0.45) * cap;
    fan(painter, &outline, shine, |p| {
        let d = ((p - shine).length() / (cap * 1.7)).clamp(0.0, 1.0);
        top.lerp_to_gamma(rim, d)
    });
    painter.circle_filled(center, cap * 0.62, Color32::from_gray(0x16));
    painter.circle_stroke(
        center,
        cap * 0.62,
        Stroke::new(0.7, Color32::from_white_alpha(18)),
    );
    painter.line_segment(
        [
            center + direction * cap * 0.2,
            center + direction * cap * 0.92,
        ],
        Stroke::new(1.6 * s.max(0.7), CREAM),
    );
    let label_y = if numbers {
        radius * 2.05
    } else {
        radius * 1.45
    };
    silk(
        painter,
        center + vec2(0.0, label_y),
        Align2::CENTER_TOP,
        label,
        6.5 * s,
        CREAM,
    );
    if hot {
        painter.text(
            center - vec2(0.0, if numbers { radius * 2.05 } else { radius * 1.5 }),
            Align2::CENTER_BOTTOM,
            compact_value(&spec, value),
            theme::mono(7.5 * s),
            AMBER,
        );
    }
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(format!(
            "{}: {}. Drag; Shift for fine control; double-click to reset; right-click to type a value.",
            spec.name,
            display(&spec, value)
        ));
}

/// A chicken-head selector with its positions printed around it; drag, click to step, or
/// click a legend.
#[allow(clippy::too_many_arguments)]
fn stepped(
    ui: &mut Ui,
    deck: &mut Deck,
    id: u32,
    center: Pos2,
    radius: f32,
    s: f32,
    labels: &[&str],
    title: &str,
) {
    let Some(spec) = deck.spec(id).copied() else {
        return;
    };
    let count = labels.len();
    let selected = (deck.value(id).round().max(0.0) as usize).min(count - 1);
    let span = if count <= 3 { 1.2_f32 } else { 2.1 };
    let angle = |index: usize| -span + 2.0 * span * index as f32 / (count - 1).max(1) as f32;
    let key = deck.param_id(id);
    let response = ui.interact(
        Rect::from_center_size(center, Vec2::splat(radius * 2.4)),
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
        next = Some(spec.default.round() as usize);
    }
    let painter = ui.painter().clone();
    let font = theme::mono((6.4 * s).max(5.0));
    for (index, label) in labels.iter().enumerate() {
        let a = angle(index);
        let direction = vec2(a.sin(), -a.cos());
        let on = index == selected;
        painter.line_segment(
            [
                center + direction * radius * 1.2,
                center + direction * radius * 1.4,
            ],
            Stroke::new(1.0 * s.max(0.7), if on { AMBER } else { CREAM_DIM }),
        );
        let spot = center + direction * radius * 2.0;
        let hit = ui.interact(
            Rect::from_center_size(spot, vec2(22.0, 11.0) * s),
            key.with(("position", index)),
            Sense::click(),
        );
        painter.text(
            spot,
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if on {
                AMBER
            } else if hit.hovered() {
                CREAM
            } else {
                CREAM_DIM
            },
        );
        if hit
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("{}: {label}", spec.name))
            .clicked()
        {
            next = Some(index);
        }
    }
    let hot = response.hovered() || response.dragged();
    chicken_head(&painter, center, radius, angle(selected), s, hot);
    silk(
        &painter,
        center + vec2(0.0, radius * 2.55),
        Align2::CENTER_TOP,
        title,
        6.0 * s,
        CREAM,
    );
    if let Some(index) = next
        && index != selected
    {
        deck.set(id, index as f64);
    }
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(format!(
            "{}: {}. Drag, click to step, or click a position.",
            spec.name, labels[selected]
        ));
}

/// A two-way paddle rocker in the hardware's blue and white.
fn rocker(ui: &mut Ui, deck: &mut Deck, id: u32, rect: Rect, s: f32, tooltip: &str) {
    let on = deck.value(id) >= 0.5;
    let response = ui.interact(rect, deck.param_id(id), Sense::click());
    if response.clicked() {
        deck.set(id, if on { 0.0 } else { 1.0 });
    }
    let painter = ui.painter();
    painter.rect_filled(
        rect.expand(1.5 * s),
        2.0 * s,
        Color32::from_black_alpha(220),
    );
    let oscillator_switch = (OSC_BASE..OSC_BASE + 3 * OSC_STRIDE).contains(&id)
        && (id - OSC_BASE) % OSC_STRIDE == OSC_ON;
    let color = if oscillator_switch {
        ROCKER_BLUE
    } else {
        ROCKER_WHITE
    };
    let (lit_half, dark_half) = {
        let (top, bottom) = rect.split_top_bottom_at_fraction(0.5);
        if on { (top, bottom) } else { (bottom, top) }
    };
    analog::gradient_rect(
        painter,
        lit_half,
        color.lerp_to_gamma(Color32::WHITE, 0.25),
        color,
    );
    analog::gradient_rect(
        painter,
        dark_half,
        color.lerp_to_gamma(Color32::BLACK, 0.55),
        color.lerp_to_gamma(Color32::BLACK, 0.35),
    );
    painter.hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(0.8, Color32::from_black_alpha(180)),
    );
    painter.rect_stroke(
        rect,
        1.5 * s,
        Stroke::new(0.8, Color32::from_black_alpha(220)),
        egui::StrokeKind::Inside,
    );
    let state = if on { "on" } else { "off" };
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!("{tooltip}: {state}. Click to switch."));
}

/// A slide fader: a slotted track with a square cap; steps snap for stepped parameters.
fn fader(ui: &mut Ui, deck: &mut Deck, id: u32, rect: Rect, s: f32, label: &str) {
    let Some(spec) = deck.spec(id).copied() else {
        return;
    };
    let value = deck.value(id);
    let normalized = spec.normalize(value) as f32;
    let track = Rect::from_min_max(
        rect.min + vec2(0.0, 8.0 * s),
        rect.max - vec2(0.0, 14.0 * s),
    );
    let response = ui.interact(
        track.expand(3.0 * s),
        deck.param_id(id),
        Sense::click_and_drag(),
    );
    if response.dragged() {
        let fine = ui.input(|input| input.modifiers.shift);
        let speed = if fine { 0.25 } else { 1.0 } / track.height().max(1.0);
        let next = (normalized - response.drag_delta().y * speed).clamp(0.0, 1.0);
        if (next - normalized).abs() > 1.0e-6 {
            deck.set(id, spec.denormalize(f64::from(next)));
        }
    } else if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let next = 1.0 - ((pointer.y - track.top()) / track.height()).clamp(0.0, 1.0);
        deck.set(id, spec.denormalize(f64::from(next)));
    }
    if response.double_clicked() {
        deck.set(id, spec.default);
    }
    deck.entry(&response, &spec, value);
    let painter = ui.painter();
    let ticks = if spec.steps > 0 { spec.steps } else { 10 };
    for tick in 0..=ticks {
        let y = track.bottom() - track.height() * tick as f32 / ticks as f32;
        let major = spec.steps > 0 || tick % 5 == 0;
        painter.hline(
            (track.left() + 2.0 * s)..=(track.left() + if major { 7.0 } else { 5.0 } * s),
            y,
            Stroke::new(0.8, CREAM_DIM),
        );
    }
    let slot = Rect::from_center_size(track.center(), vec2(4.0 * s, track.height() + 4.0 * s));
    painter.rect_filled(slot, 2.0 * s, Color32::from_gray(0x05));
    painter.rect_stroke(
        slot,
        2.0 * s,
        Stroke::new(0.8, Color32::from_white_alpha(16)),
        egui::StrokeKind::Outside,
    );
    let y = track.bottom() - track.height() * normalized;
    let cap = Rect::from_center_size(pos2(track.center().x, y), vec2(16.0, 9.0) * s);
    let hot = response.hovered() || response.dragged();
    painter.rect_filled(
        cap.translate(vec2(0.0, 1.5 * s)),
        1.5 * s,
        Color32::from_black_alpha(140),
    );
    analog::gradient_rect(
        painter,
        cap,
        Color32::from_gray(if hot { 0x5c } else { 0x48 }),
        Color32::from_gray(0x16),
    );
    painter.hline(
        (cap.left() + 1.5 * s)..=(cap.right() - 1.5 * s),
        cap.center().y,
        Stroke::new(1.4 * s.max(0.7), CREAM),
    );
    silk(
        painter,
        pos2(rect.center().x, rect.bottom() - 10.0 * s),
        Align2::CENTER_TOP,
        label,
        6.0 * s,
        CREAM,
    );
    let shown = if let Some(choices) = builtin::choices(BuiltinKind::Ferro, id) {
        choices[(value.round() as usize).min(choices.len() - 1)].to_owned()
    } else {
        compact_value(&spec, value)
    };
    if hot {
        painter.text(
            pos2(rect.center().x, rect.top()),
            Align2::CENTER_BOTTOM,
            shown,
            theme::mono(7.5 * s),
            AMBER,
        );
    }
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(format!(
            "{}: {}. Drag; Shift for fine control; double-click to reset; right-click to type a value.",
            spec.name,
            display(&spec, value)
        ));
}

/// A rectangular push button with a lamp above it.
fn lit_button(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    s: f32,
    label: &str,
    lit: bool,
    color: Color32,
) -> egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    let pressed = response.is_pointer_button_down_on();
    let lamp = Rect::from_center_size(
        pos2(rect.center().x, rect.top() - 6.0 * s),
        vec2(10.0, 3.5) * s,
    );
    analog::lamp(ui.painter(), lamp, color, lit);
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
        if lit { CREAM } else { CREAM_DIM },
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
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
        CREAM,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

// --- Finishes -------------------------------------------------------------------------

/// Oiled walnut: a warm gradient with long wavy grain lines and a few darker streaks. The
/// grain comes from a fixed hash so it never shimmers.
fn walnut(painter: &Painter, rect: Rect, s: f32, seed: i32) {
    analog::gradient_rect(painter, rect, WALNUT_LIGHT, WALNUT_DARK);
    let lines = ((rect.width() / (2.2 * s.max(0.6))) as i32).max(3);
    for line in 0..lines {
        let h = hash(line, seed);
        let x0 = rect.left() + rect.width() * (line as f32 + 0.5) / lines as f32;
        let wave = 1.0 + (h & 0x7) as f32 * 0.4;
        let phase = ((h >> 4) & 0xff) as f32 / 40.0;
        let dark = h & 0x0100 != 0;
        let color = if dark {
            Color32::from_black_alpha(40 + ((h >> 12) & 0x1f) as u8)
        } else {
            Color32::from_rgba_unmultiplied(0xc0, 0x80, 0x48, 18 + ((h >> 12) & 0xf) as u8)
        };
        let steps = 24;
        let points: Vec<Pos2> = (0..=steps)
            .map(|step| {
                let t = step as f32 / steps as f32;
                let y = rect.top() + rect.height() * t;
                let x =
                    x0 + wave * s * (t * 9.0 + phase).sin() + 0.6 * s * (t * 31.0 + phase).sin();
                pos2(x.clamp(rect.left(), rect.right()), y)
            })
            .collect();
        painter.add(Shape::line(
            points,
            Stroke::new(if dark { 1.0 } else { 0.7 } * s.max(0.6), color),
        ));
    }
    // Rounded front edge and a soft shadow where the cheek meets the panel.
    painter.rect_stroke(
        rect,
        2.0 * s,
        Stroke::new(1.0, Color32::from_black_alpha(160)),
        egui::StrokeKind::Inside,
    );
    let mut mesh = Mesh::default();
    let shade = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 6.0 * s));
    mesh.add_colored_rect(shade, Color32::from_white_alpha(10));
    painter.add(Shape::mesh(mesh));
}

/// Black anodised panel: a vertical gradient with faint horizontal brushing.
fn brushed(painter: &Painter, rect: Rect, top: Color32, bottom: Color32, s: f32) {
    analog::gradient_rect(painter, rect, top, bottom);
    let pitch = (2.0 * s).max(1.5);
    let rows = (rect.height() / pitch) as i32;
    let mut mesh = Mesh::default();
    for row in 0..rows {
        let h = hash(row, 77);
        if h & 3 != 0 {
            continue;
        }
        let y = rect.top() + row as f32 * pitch;
        let alpha = 3 + (h >> 8 & 0x7) as u8;
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(rect.left(), y), pos2(rect.right(), y + 0.6)),
            Color32::from_white_alpha(alpha),
        );
    }
    painter.add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    fn frame(
        ctx: &egui::Context,
        plugin: &PluginInstance,
        size: Vec2,
        events: Vec<Event>,
        editor_view: bool,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let telemetry = Telemetry::default();
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                if editor_view {
                    super::super::super::show(
                        ui,
                        plugin,
                        BuiltinKind::Ferro,
                        Some(&telemetry),
                        &mut actions,
                    );
                } else {
                    super::super::super::compact(
                        ui,
                        plugin,
                        BuiltinKind::Ferro,
                        Some(&telemetry),
                        &mut actions,
                    );
                }
            },
        );
        output.textures_delta.clear();
        actions
    }

    /// The control for a parameter; button groups key each button by its position.
    fn widget(ctx: &egui::Context, plugin: &PluginInstance, parameter: u32) -> Rect {
        let id = Id::new(("builtin-inline-param", plugin.id, parameter));
        ctx.read_response(id)
            .or_else(|| ctx.read_response(id.with(0_usize)))
            .unwrap_or_else(|| panic!("parameter {parameter} has no widget"))
            .rect
    }

    #[test]
    fn every_parameter_has_a_control_on_the_faceplate() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let plugin = BuiltinKind::Ferro.instance();
        for _ in 0..3 {
            assert!(frame(&ctx, &plugin, vec2(1180.0, 440.0), Vec::new(), true).is_empty());
        }
        for spec in builtin::parameters(BuiltinKind::Ferro) {
            widget(&ctx, &plugin, spec.id);
        }
    }

    #[test]
    fn compact_card_fits_the_chain_panel_and_stays_quiet() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let plugin = BuiltinKind::Ferro.instance();
        let width = card_width(BuiltinKind::Ferro);
        for _ in 0..3 {
            assert!(frame(&ctx, &plugin, vec2(width, 400.0), Vec::new(), false).is_empty());
        }
        for parameter in [CUTOFF, EMPHASIS, CONTOUR, GLIDE, GAIN, VOICE_MODE] {
            let rect = widget(&ctx, &plugin, parameter);
            assert!(
                rect.bottom() <= super::super::BODY_HEIGHT + 4.0,
                "{parameter} at {rect:?}"
            );
            assert!(rect.right() <= width, "{parameter} at {rect:?}");
        }
    }

    #[test]
    fn dragging_the_cutoff_knob_changes_only_the_cutoff() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let plugin = BuiltinKind::Ferro.instance();
        frame(&ctx, &plugin, vec2(1180.0, 440.0), Vec::new(), true);
        let knob = widget(&ctx, &plugin, CUTOFF).center();
        let press = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &ctx,
            &plugin,
            vec2(1180.0, 440.0),
            vec![Event::PointerMoved(knob), press(knob, true)],
            true,
        );
        let moved = knob - vec2(0.0, 30.0);
        let actions = frame(
            &ctx,
            &plugin,
            vec2(1180.0, 440.0),
            vec![Event::PointerMoved(moved)],
            true,
        );
        frame(
            &ctx,
            &plugin,
            vec2(1180.0, 440.0),
            vec![press(moved, false)],
            true,
        );
        assert!(!actions.is_empty());
        assert!(actions.iter().all(|action| matches!(
            action,
            Action::SetVst3Parameter { parameter, value, .. } if *parameter == CUTOFF && *value > 0.5
        )));
    }

    #[test]
    fn engine_button_switches_to_render() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let plugin = BuiltinKind::Ferro.instance();
        frame(&ctx, &plugin, vec2(1180.0, 440.0), Vec::new(), true);
        let button = ctx
            .read_response(Id::new(("builtin-inline-param", plugin.id, ENGINE)).with(1_usize))
            .unwrap()
            .rect
            .center();
        let click = |pressed| Event::PointerButton {
            pos: button,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &ctx,
            &plugin,
            vec2(1180.0, 440.0),
            vec![Event::PointerMoved(button), click(true)],
            true,
        );
        let actions = frame(&ctx, &plugin, vec2(1180.0, 440.0), vec![click(false)], true);
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::SetVst3Parameter { parameter: ENGINE, value, .. } if *value == 1.0
        )));
    }
}
