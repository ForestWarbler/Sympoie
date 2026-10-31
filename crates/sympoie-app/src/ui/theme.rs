//! Sympoie's look: a charcoal studio with analog touches.
//!
//! Surfaces step up in brightness from the deep backdrop to the raised control bar.
//! Colour carries meaning: blue selects, yellow cycles and solos, green plays, red
//! records, and warm amber marks analog values such as knob arcs and lit readouts.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};
use sympoie::project::Project;
use sympoie::track::Track;

// Surfaces, darkest first.
/// The backdrop behind panels, and the deepest wells.
pub const BG_DEEP: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1b);
/// The empty arrangement area below the last track.
pub const WORKSPACE: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x20);
/// Dark input fields: text edits and inset readouts.
pub const BG_WIDGET: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x1e);
pub const BG_WIDGET_HOVER: Color32 = Color32::from_rgb(0x25, 0x25, 0x26);
/// Track lanes, and the black-key rows of the piano roll.
pub const LANE: Color32 = Color32::from_rgb(0x27, 0x27, 0x28);
/// The white-key rows of the piano roll and striped list rows.
pub const LANE_ALT: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2f);
/// The lane of a selected track.
pub const LANE_SELECTED: Color32 = Color32::from_rgb(0x2f, 0x31, 0x35);
/// Side panels: the browser, inspectors and the mixer.
pub const BG_PANEL: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2b);
/// Track headers.
pub const HEADER: Color32 = Color32::from_rgb(0x34, 0x34, 0x35);
/// A selected track header.
pub const HEADER_SELECTED: Color32 = Color32::from_rgb(0x47, 0x48, 0x4b);
/// Raised surfaces: title bars, drag chips and hovered rows.
pub const BG_RAISED: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3b);
/// The control bar's gradient, top to bottom.
pub const CONTROL_TOP: Color32 = Color32::from_rgb(0x47, 0x47, 0x48);
pub const CONTROL_BOTTOM: Color32 = Color32::from_rgb(0x38, 0x38, 0x39);
/// The time ruler above the lanes.
pub const RULER: Color32 = Color32::from_rgb(0x31, 0x31, 0x32);
/// The transport display, its digits and its captions.
pub const LCD: Color32 = Color32::from_rgb(0x15, 0x16, 0x16);
pub const LCD_TEXT: Color32 = Color32::from_rgb(0xec, 0xea, 0xe4);
pub const LCD_LABEL: Color32 = Color32::from_rgb(0x84, 0x84, 0x86);
/// Separators between panels and around controls.
pub const BORDER: Color32 = Color32::from_rgb(0x12, 0x12, 0x13);
/// Light dividers inside a panel.
pub const HAIRLINE: Color32 = Color32::from_rgb(0x41, 0x41, 0x43);

pub const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdd);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9b, 0x9b, 0x9e);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x68, 0x68, 0x6b);
pub const TEXT_ON_COLOR: Color32 = Color32::from_rgb(0x15, 0x15, 0x16);

/// Warm amber for analog values: knob arcs, device power and lit readouts.
pub const ACCENT: Color32 = Color32::from_rgb(0xf2, 0xa6, 0x3d);
/// Selection, focus and engaged view buttons.
pub const SELECT: Color32 = Color32::from_rgb(0x3d, 0x8e, 0xf0);
/// The fill behind selected list rows and selected text.
pub const SELECT_FILL: Color32 = Color32::from_rgb(0x2b, 0x58, 0x96);
pub const PLAY: Color32 = Color32::from_rgb(0x4c, 0xc8, 0x5c);
pub const RECORD: Color32 = Color32::from_rgb(0xf2, 0x46, 0x3e);
pub const MUTE: Color32 = Color32::from_rgb(0x4d, 0xae, 0xe6);
pub const SOLO: Color32 = Color32::from_rgb(0xec, 0xc3, 0x3e);
/// The cycle (loop) region and switch.
pub const LOOP: Color32 = Color32::from_rgb(0xea, 0xc0, 0x3c);
pub const LOOP_OFF: Color32 = Color32::from_rgb(0x6a, 0x66, 0x5a);
/// Time selections, drawn translucent over lanes.
pub const SELECTION: Color32 = Color32::from_rgb(0xe2, 0xe6, 0xee);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xf4, 0xf4, 0xf4);
pub const OK: Color32 = Color32::from_rgb(0x4a, 0xd0, 0x6a);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xc4, 0x3a);
pub const ERROR: Color32 = Color32::from_rgb(0xff, 0x4d, 0x42);

pub fn grid_bar() -> Color32 {
    Color32::from_white_alpha(26)
}

pub fn grid_beat() -> Color32 {
    Color32::from_white_alpha(12)
}

pub fn grid_fine() -> Color32 {
    Color32::from_white_alpha(6)
}

/// Laid over every other bar so neighbouring bars read dark, bright, dark, bright.
pub fn bar_band() -> Color32 {
    Color32::from_white_alpha(14)
}

pub const PALETTE: [u32; 44] = [
    0xe8a39c, 0xe8ba9c, 0xe8d19c, 0xe5e89c, 0xafe89c, 0x9ce8d5, 0x9cd5e8, 0x9cb5e8, 0xbb9ce8,
    0xe89cc8, 0xd6d6d6, 0xd7756a, 0xd7966a, 0xd7b76a, 0xd4d76a, 0x85d76a, 0x6ad7bc, 0x6abcd7,
    0x6a8ed7, 0x986ad7, 0xd76aaa, 0xaaaaaa, 0xc24a3d, 0xc2723d, 0xc29a3d, 0xbdc23d, 0x5ec23d,
    0x3dc2a1, 0x3da1c2, 0x3d69c2, 0x743dc2, 0xc23d8b, 0x7e7e7e, 0x8c3a31, 0x8c5531, 0x8c7031,
    0x898c31, 0x488c31, 0x318c75, 0x31758c, 0x314f8c, 0x57318c, 0x8c3166, 0x525252,
];

pub const PALETTE_COLUMNS: usize = 11;

const DEFAULT_COLORS: [u32; 10] = [
    0xd7756a, 0x6abcd7, 0xd7b76a, 0x986ad7, 0x85d76a, 0xd76aaa, 0x6ad7bc, 0xd7966a, 0x6a8ed7,
    0xd4d76a,
];

pub const MASTER_COLOR: Color32 = Color32::from_rgb(0xb4, 0xb4, 0xb4);

pub fn rgb(color: u32) -> Color32 {
    let [_, r, g, b] = color.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

pub fn default_track_color(index: usize) -> Color32 {
    if index == 0 {
        MASTER_COLOR
    } else {
        rgb(DEFAULT_COLORS[(index - 1) % DEFAULT_COLORS.len()])
    }
}

pub fn track_color(project: &Project, index: usize) -> Color32 {
    match project.tracks().get(index).and_then(Track::color) {
        Some(color) => rgb(color),
        None => default_track_color(index),
    }
}

pub fn tint(base: Color32, color: Color32, amount: f32) -> Color32 {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    Color32::from_rgb(
        mix(base.r(), color.r()),
        mix(base.g(), color.g()),
        mix(base.b(), color.b()),
    )
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn prop(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, family, bytes) in [
        (
            "Inter Medium",
            FontFamily::Proportional,
            include_bytes!("../../assets/fonts/Inter-Medium.ttf").as_slice(),
        ),
        (
            "JetBrains Mono Medium",
            FontFamily::Monospace,
            include_bytes!("../../assets/fonts/JetBrainsMono-Medium.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_static(bytes).into());
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.to_owned());
    }
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("Hack".to_owned());
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Small, prop(9.5)),
            (TextStyle::Body, prop(11.5)),
            (TextStyle::Button, prop(11.5)),
            (TextStyle::Heading, prop(14.0)),
            (TextStyle::Monospace, mono(11.0)),
        ]
        .into();
        let spacing = &mut style.spacing;
        spacing.item_spacing = egui::vec2(5.0, 4.0);
        spacing.button_padding = egui::vec2(6.0, 2.0);
        spacing.interact_size.y = 18.0;
        spacing.menu_margin = Margin::same(5);
        spacing.window_margin = Margin::same(12);
        spacing.combo_height = 320.0;

        let visuals = &mut style.visuals;
        visuals.dark_mode = true;
        visuals.panel_fill = BG_PANEL;
        visuals.window_fill = Color32::from_rgb(0x2c, 0x2c, 0x2d);
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = CornerRadius::same(8);
        visuals.window_shadow = egui::Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(130),
        };
        visuals.menu_corner_radius = CornerRadius::same(6);
        visuals.popup_shadow = egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        visuals.extreme_bg_color = Color32::from_rgb(0x18, 0x18, 0x19);
        visuals.text_edit_bg_color = Some(BG_WIDGET);
        visuals.faint_bg_color = LANE_ALT;
        visuals.code_bg_color = BG_DEEP;
        visuals.override_text_color = None;
        visuals.hyperlink_color = SELECT;
        visuals.warn_fg_color = WARN;
        visuals.error_fg_color = ERROR;
        visuals.selection.bg_fill = SELECT_FILL;
        visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0xf4, 0xf6, 0xfa));
        visuals.striped = true;
        visuals.slider_trailing_fill = true;

        let radius = CornerRadius::same(4);
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = BG_PANEL;
        widgets.noninteractive.weak_bg_fill = BG_PANEL;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.noninteractive.corner_radius = radius;
        for (state, fill, rim, text) in [
            (
                &mut widgets.inactive,
                Color32::from_rgb(0x44, 0x44, 0x46),
                Color32::from_rgb(0x1b, 0x1b, 0x1c),
                TEXT,
            ),
            (
                &mut widgets.hovered,
                Color32::from_rgb(0x4e, 0x4e, 0x50),
                Color32::from_rgb(0x22, 0x22, 0x23),
                Color32::WHITE,
            ),
            (
                &mut widgets.active,
                Color32::from_rgb(0x2e, 0x2e, 0x30),
                Color32::from_rgb(0x0f, 0x0f, 0x10),
                Color32::WHITE,
            ),
            (
                &mut widgets.open,
                Color32::from_rgb(0x4e, 0x4e, 0x50),
                Color32::from_rgb(0x22, 0x22, 0x23),
                Color32::WHITE,
            ),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, rim);
            state.fg_stroke = Stroke::new(1.0, text);
            state.corner_radius = radius;
            state.expansion = 0.0;
        }
    });
}
