//! Calor's interface, drawn in egui and shared by Sympoie's built-in Calor and the Calor
//! plugin for other hosts.
//!
//! Everything is drawn here, with no host styling, in the graphite look Calor shares with
//! Reson. The full panel has two pages under its header.
//!
//! - **Bands**: a strip across the spectrum shows each band as the shape its crossovers
//!   give it, in its own colour, with the crossovers to drag and each band's solo and
//!   mute. Click a band to pick it: below the strip, its transfer curve and the harmonics a
//!   sine at the input's level comes out with (the fundamental light, the odd harmonics
//!   copper, the even ones jade), its model, style and channels, and a deck with its
//!   knobs. The global Tone, Mix and Output sit beside them.
//! - **Modulation**: the LFO and the envelope follower, each drawn live, and the four
//!   slots that route them.
//!
//! The compact card, made for Sympoie's rack, has the strip, the picked band's glasses
//! and its main knobs. Both layouts are drawn at a design size and scaled to fit. A host
//! connects the interface to Calor through [`Controls`], and hands over Calor's
//! [`Telemetry`] for the meters and the modulation's live view.

mod format;
mod graphs;
mod knob;
mod modpage;
mod paint;
mod strip;
mod widgets;

use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
pub use sympoie_calor::Telemetry;
use sympoie_calor::params::{AUTO_GAIN, COUNT, MIX, OUTPUT, Slot, TONE, band_parameter};
use sympoie_calor::{STYLES, Transfer, matching_style};

use paint::{
    BAND_COLORS, EMBER, FAINT, HAIRLINE, LABEL, PANEL_BOTTOM, PANEL_TOP, RULE, TEAL, TEXT, fade,
    sans, spaced,
};

/// How the interface reads and changes Calor's parameters. Values are in physical units
/// (dB, Hz, %), as `sympoie_calor::PARAMETERS` describes them.
pub trait Controls {
    fn value(&self, id: u32) -> f64;
    /// A change begins: a drag starts, or a single edit is about to be made.
    fn begin(&mut self, id: u32);
    fn set(&mut self, id: u32, value: f64);
    /// The change that `begin` opened is complete.
    fn end(&mut self, id: u32);
    fn bypassed(&self) -> bool;
    fn set_bypassed(&mut self, bypassed: bool);
}

/// A parameter's value as the interface writes it, for a host's own displays.
pub fn format_value(spec: &sympoie_calor::Spec, value: f64) -> String {
    format::value(spec, value)
}

/// Reads a value typed for a parameter: a number, optionally with its unit or a `k`, a
/// choice by name, or `on` and `off` for a switch. Anything else is `None`.
pub fn parse_value(spec: &sympoie_calor::Spec, text: &str) -> Option<f64> {
    format::parse(spec, text)
}

/// The full panel's design size; it scales to whatever space it is given.
pub const PANEL_SIZE: Vec2 = vec2(900.0, 500.0);
/// The compact card's design size, for Sympoie's rack.
pub const CARD_SIZE: Vec2 = vec2(444.0, 172.0);

/// A knob for one of a band's controls: slot, label on the panel, label on the card, and
/// what it does.
type BandKnob = (Slot, &'static str, &'static str, &'static str);

const SATURATE: [BandKnob; 3] = [
    (
        Slot::Drive,
        "Drive",
        "Drive",
        "How hard the signal hits the curve.",
    ),
    (
        Slot::Hardness,
        "Hardness",
        "Hard",
        "Where the curve starts to bend: early and soft, or late and sharp.",
    ),
    (
        Slot::Bias,
        "Bias",
        "Bias",
        "How far the curve leans off centre, and so how early the even harmonics come in.",
    ),
];
const HARMONIC_KNOBS: [BandKnob; 2] = [
    (
        Slot::Odd,
        "Odd",
        "Odd",
        "How much of the odd harmonics, and the squash that comes with them, you hear.",
    ),
    (
        Slot::Even,
        "Even",
        "Even",
        "How much of the even harmonics the lean makes: none, the curve's own, or more.",
    ),
];
const MOTION_KNOBS: [BandKnob; 3] = [
    (
        Slot::Dynamics,
        "Dynamics",
        "Dyn",
        "Right compresses the band, pumping at the top; left expands it towards a gate.",
    ),
    (
        Slot::Feedback,
        "Feedback",
        "Fdbk",
        "Feeds the band's output back in through a ringing filter, up to singing.",
    ),
    (
        Slot::FeedbackFrequency,
        "Ring",
        "Ring",
        "Where the feedback rings.",
    ),
];
const EMPHASIS_KNOBS: [BandKnob; 2] = [
    (
        Slot::Emphasis,
        "Emphasis",
        "Emph",
        "Lifts the treble into the curve and takes it back out after, so it saturates first; below zero the lows do.",
    ),
    (
        Slot::EmphasisFrequency,
        "Freq",
        "Freq",
        "Where the emphasis begins.",
    ),
];
const BAND_LEVEL_KNOBS: [BandKnob; 2] = [
    (
        Slot::Mix,
        "Mix",
        "Mix",
        "How much of this band's saturated sound you hear against its dry signal.",
    ),
    (
        Slot::Level,
        "Level",
        "Level",
        "This band's level after Calor.",
    ),
];

/// The accent of a band knob's arc: copper for Odd and jade for Even, as the harmonics
/// glass draws them, and the band's own colour for the rest.
fn accent(slot: Slot, band: usize) -> Color32 {
    match slot {
        Slot::Odd => EMBER,
        Slot::Even => TEAL,
        _ => BAND_COLORS[band],
    }
}

/// The larger knobs.
fn hero(slot: Slot) -> bool {
    matches!(slot, Slot::Drive | Slot::Odd | Slot::Even)
}

/// Every parameter's value, indexed by id.
fn values(controls: &dyn Controls) -> [f64; COUNT] {
    std::array::from_fn(|id| controls.value(id as u32))
}

/// Maps design coordinates into the space the layout was given.
#[derive(Clone, Copy)]
pub(crate) struct Place {
    origin: Pos2,
    pub(crate) s: f32,
}

impl Place {
    fn fit(space: Rect, design: Vec2) -> Self {
        let s = (space.width() / design.x)
            .min(space.height() / design.y)
            .max(0.1);
        let size = design * s;
        Self {
            origin: space.center() - size / 2.0,
            s,
        }
    }

    pub(crate) fn at(self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x, y) * self.s
    }

    pub(crate) fn rect(self, x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect::from_min_size(self.at(x, y), vec2(width, height) * self.s)
    }
}

/// Which page the panel shows: 0 for the bands, 1 for modulation.
fn page(ui: &Ui, salt: Id) -> usize {
    ui.ctx()
        .data_mut(|data| data.get_temp::<usize>(salt.with("page")))
        .unwrap_or(0)
}

/// The full panel, filling the space `ui` offers: Sympoie's Calor window, or the plugin's.
/// `salt` keeps this panel's interface state apart from any other Calor's.
pub fn panel(ui: &mut Ui, controls: &mut dyn Controls, telemetry: Option<&Telemetry>, salt: Id) {
    let space = ui.available_rect_before_wrap();
    ui.allocate_rect(space, Sense::hover());
    let painter = ui.painter_at(space);
    paint::gradient_rect(&painter, space, 0.0, PANEL_TOP, PANEL_BOTTOM);
    let place = Place::fit(space, PANEL_SIZE);
    let s = place.s;
    let live = !controls.bypassed();
    let levels = graphs::levels(ui, salt, telemetry);

    // Header: the name, the page switch, the meters, Auto gain and power.
    let mark = spaced(
        &painter,
        place.at(28.0, 30.0),
        Align2::LEFT_CENTER,
        "Calor",
        sans(20.0 * s),
        TEXT,
        0.3 * s,
    );
    spaced(
        &painter,
        pos2(mark.right() + 12.0 * s, mark.center().y + 1.5 * s),
        Align2::LEFT_CENTER,
        "Harmonic Saturator",
        sans(10.5 * s),
        LABEL,
        0.2 * s,
    );
    let current = page(ui, salt);
    if let Some(next) = widgets::segmented(
        ui,
        salt.with("page"),
        place.rect(292.0, 18.0, 180.0, 24.0),
        s,
        &["Bands", "Modulation"],
        current,
    ) {
        ui.ctx()
            .data_mut(|data| data.insert_temp(salt.with("page"), next));
    }
    graphs::meter(
        &painter,
        place.rect(492.0, 16.0, 176.0, 12.0),
        levels.input_db,
        "IN",
        s,
    );
    graphs::meter(
        &painter,
        place.rect(492.0, 32.0, 176.0, 12.0),
        levels.output_db,
        "OUT",
        s,
    );
    widgets::toggle(
        ui,
        controls,
        salt.with("auto gain"),
        place.rect(688.0, 18.0, 104.0, 24.0),
        s,
        AUTO_GAIN,
        "Auto Gain",
        EMBER,
    )
    .on_hover_text(if controls.value(AUTO_GAIN) >= 0.5 {
        "Auto gain is on: a sine at −12 dBFS comes out at the level it went in, so you hear the colour and not the loudness"
    } else {
        "Auto gain is off: Drive makes the sound louder as well as hotter"
    });
    widgets::power(ui, controls, salt, place.at(866.0, 30.0), 12.0 * s);
    if !live {
        widgets::bypassed_badge(&painter, place.at(866.0, 50.0), Align2::CENTER_TOP, s);
    }

    if page(ui, salt) == 1 {
        modpage::page(ui, controls, telemetry, &levels, salt, place);
    } else {
        bands_page(ui, controls, &levels, salt, place, live);
    }
    if telemetry.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// The bands page: the strip, the picked band's glasses and character, the global
/// output, and the picked band's knobs.
fn bands_page(
    ui: &mut Ui,
    controls: &mut dyn Controls,
    levels: &graphs::Levels,
    salt: Id,
    place: Place,
    live: bool,
) {
    let s = place.s;
    let painter = ui.painter().clone();
    strip::strip(
        ui,
        controls,
        salt,
        place.rect(20.0, 56.0, 860.0, 116.0),
        s,
        false,
        live,
    );
    let count = strip::count(controls);
    let band = strip::selected(ui, salt, count);
    let transfer = Transfer::for_band(&values(controls), band);
    let amplitude = graphs::display_level(ui, salt, levels);
    let reach = (levels.input_db > -90.0).then(|| 10.0_f64.powf(f64::from(levels.input_db) / 20.0));
    graphs::transfer(
        &painter,
        place.rect(20.0, 182.0, 156.0, 156.0),
        &transfer,
        reach,
        live,
        s,
        false,
    );
    graphs::harmonics(
        ui,
        &painter,
        place.rect(186.0, 182.0, 320.0, 156.0),
        &transfer,
        amplitude,
        live,
        salt,
        s,
        false,
    );

    // The band's character: its name and range, model, style and channels.
    let character = place.rect(516.0, 182.0, 214.0, 156.0);
    graphs::glass(&painter, character, s);
    let color = if live { BAND_COLORS[band] } else { FAINT };
    painter.circle_filled(place.at(530.0, 199.0), 5.0 * s, color);
    painter.text(
        place.at(541.0, 199.0),
        Align2::LEFT_CENTER,
        format!("Band {}", band + 1),
        sans(11.0 * s),
        TEXT,
    );
    painter.text(
        place.at(718.0, 199.0),
        Align2::RIGHT_CENTER,
        strip::range(controls, band, count),
        sans(9.0 * s),
        LABEL,
    );
    widgets::choice_menu(
        ui,
        controls,
        salt,
        place.rect(526.0, 216.0, 194.0, 24.0),
        s,
        band_parameter(band, Slot::Model),
        "Model",
    );
    let value = |slot: Slot| controls.value(band_parameter(band, slot));
    let matched = matching_style(value);
    let names: Vec<&str> = STYLES.iter().map(|style| style.name).collect();
    if let Some(index) = widgets::menu(
        ui,
        salt.with(("style", band)),
        place.rect(526.0, 248.0, 194.0, 24.0),
        s,
        "Style",
        matched.map_or("Custom", |index| STYLES[index].name),
        &names,
        matched,
    ) {
        for &(slot, setting) in STYLES[index].settings {
            widgets::set(controls, band_parameter(band, slot), setting);
        }
    }
    widgets::choice_segments(
        ui,
        controls,
        salt,
        place.rect(526.0, 280.0, 194.0, 24.0),
        s,
        band_parameter(band, Slot::Channels),
    );
    // The band's output, from −60 to +6 dBFS like the meters.
    let out = widgets::heading(&painter, place.at(528.0, 321.0), "Out", s);
    widgets::bar(
        &painter,
        Rect::from_min_max(
            pos2(out.right() + 8.0 * s, place.at(0.0, 318.5).y),
            place.at(720.0, 323.5),
        ),
        ((levels.bands[band] + 60.0) / 66.0).clamp(0.0, 1.0),
        fade(color, 0.85),
        s,
    );

    // The global output.
    let output = place.rect(740.0, 182.0, 140.0, 156.0);
    graphs::glass(&painter, output, s);
    widgets::heading(&painter, place.at(752.0, 199.0), "Output", s);
    let small = knob::Look {
        radius: 15.0 * s,
        accent: EMBER,
        hero: false,
        text: 0.85 * s,
    };
    for (id, label, help, x) in [
        (TONE, "Tone", "Tilts the result darker or brighter.", 763.0),
        (
            MIX,
            "Mix",
            "How much of the saturated sound you hear.",
            810.0,
        ),
        (OUTPUT, "Output", "Level after Calor.", 857.0),
    ] {
        knob::knob(
            ui,
            controls,
            salt,
            id,
            place.at(x, 270.0),
            small,
            label,
            help,
        );
    }

    // The deck: the picked band's knobs.
    let normal = knob::Look {
        radius: 19.0 * s,
        accent: EMBER,
        hero: false,
        text: s,
    };
    let groups: [(&str, f32, f32, &[BandKnob], f32); 5] = [
        ("Saturate", 24.0, 232.0, &SATURATE, 70.0),
        ("Harmonics", 248.0, 392.0, &HARMONIC_KNOBS, 74.0),
        ("Dynamics · Feedback", 408.0, 604.0, &MOTION_KNOBS, 64.0),
        ("Emphasis", 620.0, 736.0, &EMPHASIS_KNOBS, 58.0),
        ("Band", 752.0, 876.0, &BAND_LEVEL_KNOBS, 58.0),
    ];
    for (title, left, right, knobs, spacing) in groups {
        let heading = widgets::heading(&painter, place.at(left, 366.0), title, s);
        painter.hline(
            (heading.right() + 10.0 * s)..=place.at(right, 0.0).x,
            heading.center().y,
            Stroke::new(1.0, RULE),
        );
        let first = left + (right - left - spacing * (knobs.len() - 1) as f32) / 2.0;
        for (index, &(slot, label, _, help)) in knobs.iter().enumerate() {
            let look = knob::Look {
                radius: if hero(slot) { 24.0 * s } else { normal.radius },
                accent: if live { accent(slot, band) } else { FAINT },
                hero: hero(slot),
                ..normal
            };
            knob::knob(
                ui,
                controls,
                salt,
                band_parameter(band, slot),
                place.at(first + spacing * index as f32, 432.0),
                look,
                label,
                help,
            );
        }
    }
}

/// The compact card for Sympoie's rack: the strip, the picked band's glasses, Auto gain
/// and the band count, and the picked band's main knobs with the global Mix and Output.
pub fn card(ui: &mut Ui, controls: &mut dyn Controls, telemetry: Option<&Telemetry>, salt: Id) {
    let width = ui.available_width();
    let s = (width / CARD_SIZE.x).clamp(0.5, 1.5);
    let (space, _) = ui.allocate_exact_size(CARD_SIZE * s, Sense::hover());
    let painter = ui.painter_at(space);
    paint::gradient_rect(&painter, space, 8.0 * s, PANEL_TOP, PANEL_BOTTOM);
    painter.rect_stroke(
        space,
        8.0 * s,
        Stroke::new(1.0, HAIRLINE),
        StrokeKind::Inside,
    );
    let place = Place {
        origin: space.min,
        s,
    };
    let live = !controls.bypassed();
    let levels = graphs::levels(ui, salt, telemetry);
    strip::strip(
        ui,
        controls,
        salt,
        place.rect(4.0, 4.0, 436.0, 46.0),
        s,
        true,
        live,
    );
    let count = strip::count(controls);
    let band = strip::selected(ui, salt, count);
    let transfer = Transfer::for_band(&values(controls), band);
    let amplitude = graphs::display_level(ui, salt, &levels);
    let reach = (levels.input_db > -90.0).then(|| 10.0_f64.powf(f64::from(levels.input_db) / 20.0));
    graphs::transfer(
        &painter,
        place.rect(4.0, 54.0, 58.0, 58.0),
        &transfer,
        reach,
        live,
        s,
        true,
    );
    graphs::harmonics(
        ui,
        &painter,
        place.rect(66.0, 54.0, 300.0, 58.0),
        &transfer,
        amplitude,
        live,
        salt,
        s,
        true,
    );
    widgets::toggle(
        ui,
        controls,
        salt.with("auto gain"),
        place.rect(370.0, 56.0, 70.0, 22.0),
        s,
        AUTO_GAIN,
        "Auto",
        EMBER,
    )
    .on_hover_text("Auto gain: level with the dry signal at −12 dBFS");
    widgets::stepper(
        ui,
        controls,
        salt,
        place.rect(370.0, 86.0, 70.0, 22.0),
        s,
        sympoie_calor::BANDS,
        "",
    );
    let look = knob::Look {
        radius: 11.0 * s,
        accent: EMBER,
        hero: false,
        text: 0.82 * s,
    };
    let knobs: [(u32, &str, &str, Color32, f32); 10] = [
        (
            band_parameter(band, Slot::Drive),
            "Drive",
            SATURATE[0].3,
            BAND_COLORS[band],
            24.0,
        ),
        (
            band_parameter(band, Slot::Hardness),
            "Hard",
            SATURATE[1].3,
            BAND_COLORS[band],
            64.0,
        ),
        (
            band_parameter(band, Slot::Bias),
            "Bias",
            SATURATE[2].3,
            BAND_COLORS[band],
            104.0,
        ),
        (
            band_parameter(band, Slot::Odd),
            "Odd",
            HARMONIC_KNOBS[0].3,
            EMBER,
            152.0,
        ),
        (
            band_parameter(band, Slot::Even),
            "Even",
            HARMONIC_KNOBS[1].3,
            TEAL,
            196.0,
        ),
        (
            band_parameter(band, Slot::Dynamics),
            "Dyn",
            MOTION_KNOBS[0].3,
            BAND_COLORS[band],
            244.0,
        ),
        (
            band_parameter(band, Slot::Feedback),
            "Fdbk",
            MOTION_KNOBS[1].3,
            BAND_COLORS[band],
            284.0,
        ),
        (
            band_parameter(band, Slot::Emphasis),
            "Emph",
            EMPHASIS_KNOBS[0].3,
            BAND_COLORS[band],
            324.0,
        ),
        (
            MIX,
            "Mix",
            "How much of the saturated sound you hear.",
            EMBER,
            372.0,
        ),
        (OUTPUT, "Out", "Level after Calor.", EMBER, 414.0),
    ];
    for (id, label, help, color, x) in knobs {
        let slot = sympoie_calor::band_slot(id).map(|(_, slot)| slot);
        let big = slot.is_some_and(hero);
        let look = knob::Look {
            radius: if big { 12.5 * s } else { look.radius },
            accent: if live { color } else { FAINT },
            hero: big,
            ..look
        };
        knob::knob(
            ui,
            controls,
            salt,
            id,
            place.at(x, 146.0),
            look,
            label,
            help,
        );
    }
    for x in [128.0, 220.0, 348.0] {
        painter.vline(
            place.at(x, 0.0).x,
            place.at(0.0, 128.0).y..=place.at(0.0, 166.0).y,
            Stroke::new(1.0, RULE),
        );
    }
    if !live {
        widgets::bypassed_badge(&painter, place.at(434.0, 10.0), Align2::RIGHT_TOP, s);
    }
    if telemetry.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sympoie_calor::{
        BANDS, CROSSOVER_BASE, DRIVE, LFO_RATE, MOD_SOURCE, PARAMETERS, mod_parameter,
    };

    /// Parameters held in memory, recording every gesture.
    struct Memory {
        values: [f64; COUNT],
        bypassed: bool,
        gestures: Vec<(char, u32)>,
    }

    impl Controls for Memory {
        fn value(&self, id: u32) -> f64 {
            self.values[id as usize]
        }
        fn begin(&mut self, id: u32) {
            self.gestures.push(('b', id));
        }
        fn set(&mut self, id: u32, value: f64) {
            self.values[id as usize] = value;
            self.gestures.push(('s', id));
        }
        fn end(&mut self, id: u32) {
            self.gestures.push(('e', id));
        }
        fn bypassed(&self) -> bool {
            self.bypassed
        }
        fn set_bypassed(&mut self, bypassed: bool) {
            self.bypassed = bypassed;
        }
    }

    fn frame(ctx: &egui::Context, memory: &mut Memory, events: Vec<egui::Event>, card: bool) {
        let size = if card { CARD_SIZE } else { PANEL_SIZE };
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let telemetry = Telemetry::default();
        let mut output = ctx.run_ui(raw, |ui| {
            let salt = Id::new("calor");
            if card {
                super::card(ui, memory, Some(&telemetry), salt);
            } else {
                panel(ui, memory, Some(&telemetry), salt);
            }
        });
        output.textures_delta.clear();
    }

    fn press(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn memory() -> Memory {
        Memory {
            values: sympoie_calor::params::defaults(),
            bypassed: false,
            gestures: Vec::new(),
        }
    }

    fn centre(ctx: &egui::Context, key: Id) -> Pos2 {
        ctx.read_response(key)
            .unwrap_or_else(|| panic!("nothing drawn for {key:?}"))
            .rect
            .center()
    }

    /// Two quiet frames. egui answers `read_response` from the last two passes, so after
    /// these both show what the interface draws now.
    fn settle(ctx: &egui::Context, memory: &mut Memory, card: bool) {
        frame(ctx, memory, vec![], card);
        frame(ctx, memory, vec![], card);
    }

    /// Clicks whatever is drawn for `key`.
    fn click(ctx: &egui::Context, memory: &mut Memory, key: Id, card: bool) {
        settle(ctx, memory, card);
        let at = centre(ctx, key);
        frame(
            ctx,
            memory,
            vec![egui::Event::PointerMoved(at), press(at, true)],
            card,
        );
        frame(ctx, memory, vec![press(at, false)], card);
        settle(ctx, memory, card);
    }

    /// Drags whatever is drawn for `key` by `by`.
    fn drag(ctx: &egui::Context, memory: &mut Memory, key: Id, by: Vec2, card: bool) {
        settle(ctx, memory, card);
        let at = centre(ctx, key);
        frame(
            ctx,
            memory,
            vec![egui::Event::PointerMoved(at), press(at, true)],
            card,
        );
        frame(
            ctx,
            memory,
            vec![egui::Event::PointerMoved(at + by * 0.5)],
            card,
        );
        frame(ctx, memory, vec![egui::Event::PointerMoved(at + by)], card);
        frame(ctx, memory, vec![press(at + by, false)], card);
        settle(ctx, memory, card);
    }

    #[test]
    fn dragging_a_knob_is_one_gesture_and_the_toggles_flip() {
        for card in [false, true] {
            let ctx = egui::Context::default();
            let mut memory = Memory {
                values: sympoie_calor::params::defaults(),
                bypassed: false,
                gestures: Vec::new(),
            };
            frame(&ctx, &mut memory, vec![], card);
            frame(&ctx, &mut memory, vec![], card);
            let salt = Id::new("calor");
            let drive = ctx
                .read_response(salt.with(("knob", DRIVE)))
                .unwrap()
                .rect
                .center();
            frame(
                &ctx,
                &mut memory,
                vec![egui::Event::PointerMoved(drive), press(drive, true)],
                card,
            );
            frame(
                &ctx,
                &mut memory,
                vec![egui::Event::PointerMoved(drive - vec2(0.0, 30.0))],
                card,
            );
            frame(
                &ctx,
                &mut memory,
                vec![press(drive - vec2(0.0, 30.0), false)],
                card,
            );
            assert!(memory.values[DRIVE as usize] > PARAMETERS[DRIVE as usize].default);
            assert_eq!(memory.gestures.first(), Some(&('b', DRIVE)));
            assert_eq!(memory.gestures.last(), Some(&('e', DRIVE)));
            assert!(memory.gestures.iter().all(|&(_, id)| id == DRIVE));

            let auto = ctx
                .read_response(salt.with("auto gain"))
                .unwrap()
                .rect
                .center();
            frame(
                &ctx,
                &mut memory,
                vec![egui::Event::PointerMoved(auto), press(auto, true)],
                card,
            );
            frame(&ctx, &mut memory, vec![press(auto, false)], card);
            assert_eq!(memory.values[AUTO_GAIN as usize], 0.0, "card: {card}");
        }
        // The power button bypasses, on the panel.
        let ctx = egui::Context::default();
        let mut memory = Memory {
            values: sympoie_calor::params::defaults(),
            bypassed: false,
            gestures: Vec::new(),
        };
        frame(&ctx, &mut memory, vec![], false);
        frame(&ctx, &mut memory, vec![], false);
        let power = ctx
            .read_response(Id::new("calor").with("power"))
            .unwrap()
            .rect
            .center();
        frame(
            &ctx,
            &mut memory,
            vec![egui::Event::PointerMoved(power), press(power, true)],
            false,
        );
        frame(&ctx, &mut memory, vec![press(power, false)], false);
        assert!(memory.bypassed);
    }

    #[test]
    fn picking_a_band_shows_its_controls_on_the_panel_and_the_card() {
        let salt = Id::new("calor");
        for card in [false, true] {
            let ctx = egui::Context::default();
            let mut memory = memory();
            memory.values[BANDS as usize] = 3.0;
            frame(&ctx, &mut memory, vec![], card);
            click(&ctx, &mut memory, salt.with(("band", 1)), card);
            let drive = band_parameter(1, Slot::Drive);
            assert!(ctx.read_response(salt.with(("knob", DRIVE))).is_none());
            drag(
                &ctx,
                &mut memory,
                salt.with(("knob", drive)),
                vec2(0.0, -30.0),
                card,
            );
            assert!(memory.values[drive as usize] > PARAMETERS[drive as usize].default);
            assert_eq!(
                memory.values[DRIVE as usize],
                PARAMETERS[DRIVE as usize].default
            );
            // With fewer bands, the pick falls back to the last one left.
            memory.values[BANDS as usize] = 1.0;
            settle(&ctx, &mut memory, card);
            assert!(ctx.read_response(salt.with(("knob", DRIVE))).is_some());
        }
    }

    #[test]
    fn crossovers_drag_but_keep_clear_of_their_neighbours() {
        let salt = Id::new("calor");
        let ctx = egui::Context::default();
        let mut memory = memory();
        memory.values[BANDS as usize] = 3.0;
        let low = memory.values[CROSSOVER_BASE as usize];
        let high = memory.values[CROSSOVER_BASE as usize + 1];
        drag(
            &ctx,
            &mut memory,
            salt.with(("crossover", 0)),
            vec2(30.0, 0.0),
            false,
        );
        let moved = memory.values[CROSSOVER_BASE as usize];
        assert!(moved > low * 1.2, "{low} → {moved}");
        assert_eq!(memory.gestures.first(), Some(&('b', CROSSOVER_BASE)));
        assert_eq!(memory.gestures.last(), Some(&('e', CROSSOVER_BASE)));
        // Dragged past the next crossover, it stops short of it.
        drag(
            &ctx,
            &mut memory,
            salt.with(("crossover", 0)),
            vec2(800.0, 0.0),
            false,
        );
        let stopped = memory.values[CROSSOVER_BASE as usize];
        assert!((stopped - high / 1.25).abs() < 1.0e-6, "{stopped}");
        assert_eq!(memory.values[CROSSOVER_BASE as usize + 1], high);
    }

    #[test]
    fn solo_mute_and_the_band_count_set_their_parameters() {
        let salt = Id::new("calor");
        let ctx = egui::Context::default();
        let mut memory = memory();
        memory.values[BANDS as usize] = 2.0;
        click(&ctx, &mut memory, salt.with(("solo", 1)), false);
        assert_eq!(memory.values[band_parameter(1, Slot::Solo) as usize], 1.0);
        click(&ctx, &mut memory, salt.with(("mute", 0)), false);
        assert_eq!(memory.values[band_parameter(0, Slot::Mute) as usize], 1.0);
        click(&ctx, &mut memory, salt.with(("step", BANDS, "+")), false);
        assert_eq!(memory.values[BANDS as usize], 3.0);
        // The card has the count too.
        click(&ctx, &mut memory, salt.with(("step", BANDS, "−")), true);
        assert_eq!(memory.values[BANDS as usize], 2.0);
    }

    #[test]
    fn the_modulation_page_sets_its_sources_and_routing() {
        let salt = Id::new("calor");
        let ctx = egui::Context::default();
        let mut memory = memory();
        click(&ctx, &mut memory, salt.with("page").with(1), false);
        let source = mod_parameter(2, MOD_SOURCE);
        click(
            &ctx,
            &mut memory,
            salt.with(("segments", source)).with(1),
            false,
        );
        assert_eq!(memory.values[source as usize], 1.0);
        drag(
            &ctx,
            &mut memory,
            salt.with(("knob", LFO_RATE)),
            vec2(0.0, -30.0),
            false,
        );
        assert!(memory.values[LFO_RATE as usize] > PARAMETERS[LFO_RATE as usize].default);
        // Back on the bands page, the band knobs are there again.
        click(&ctx, &mut memory, salt.with("page").with(0), false);
        assert!(ctx.read_response(salt.with(("knob", DRIVE))).is_some());
        assert!(ctx.read_response(salt.with(("knob", LFO_RATE))).is_none());
    }
}
