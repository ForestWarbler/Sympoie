use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};
use sympoie::project::Project;
use sympoie::track::Track;

pub const BG_DEEP: Color32 = Color32::from_rgb(0x13, 0x13, 0x13);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
pub const BG_RAISED: Color32 = Color32::from_rgb(0x2d, 0x2d, 0x2d);
pub const BG_WIDGET: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const BG_WIDGET_HOVER: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
pub const LANE: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
pub const LANE_ALT: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
pub const LANE_SELECTED: Color32 = Color32::from_rgb(0x27, 0x27, 0x27);
pub const HEADER: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
pub const HEADER_SELECTED: Color32 = Color32::from_rgb(0x37, 0x37, 0x37);
pub const BORDER: Color32 = Color32::from_rgb(0x0b, 0x0b, 0x0b);
pub const HAIRLINE: Color32 = Color32::from_rgb(0x36, 0x36, 0x36);

pub const TEXT: Color32 = Color32::from_rgb(0xd0, 0xd0, 0xd0);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8d, 0x8d, 0x8d);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x58, 0x58, 0x58);
pub const TEXT_ON_COLOR: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);

pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0xaa, 0x3b);
pub const PLAY: Color32 = Color32::from_rgb(0x57, 0xd3, 0x5a);
pub const SELECTION: Color32 = Color32::from_rgb(0xd8, 0xdc, 0xe2);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
pub const LOOP: Color32 = Color32::from_rgb(0xb0, 0xb0, 0xb0);
pub const LOOP_OFF: Color32 = Color32::from_rgb(0x52, 0x52, 0x52);
pub const OK: Color32 = Color32::from_rgb(0x3f, 0xd4, 0x5c);
pub const WARN: Color32 = Color32::from_rgb(0xff, 0xd2, 0x2e);
pub const ERROR: Color32 = Color32::from_rgb(0xff, 0x4c, 0x4c);

pub fn grid_bar() -> Color32 {
    Color32::from_white_alpha(20)
}

pub fn grid_beat() -> Color32 {
    Color32::from_white_alpha(10)
}

pub fn grid_fine() -> Color32 {
    Color32::from_white_alpha(6)
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
        spacing.menu_margin = Margin::same(4);
        spacing.window_margin = Margin::same(10);
        spacing.combo_height = 320.0;

        let visuals = &mut style.visuals;
        visuals.dark_mode = true;
        visuals.panel_fill = BG_PANEL;
        visuals.window_fill = Color32::from_rgb(0x28, 0x28, 0x28);
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = CornerRadius::same(4);
        visuals.menu_corner_radius = CornerRadius::same(3);
        visuals.popup_shadow = egui::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        visuals.extreme_bg_color = Color32::from_rgb(0x0e, 0x0e, 0x0e);
        visuals.text_edit_bg_color = Some(Color32::from_rgb(0x0e, 0x0e, 0x0e));
        visuals.faint_bg_color = LANE_ALT;
        visuals.code_bg_color = BG_DEEP;
        visuals.override_text_color = None;
        visuals.hyperlink_color = ACCENT;
        visuals.warn_fg_color = WARN;
        visuals.error_fg_color = ERROR;
        visuals.selection.bg_fill = Color32::from_rgb(0x48, 0x4b, 0x50);
        visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0xf0, 0xf0, 0xf0));
        visuals.striped = true;
        visuals.slider_trailing_fill = true;

        let radius = CornerRadius::same(2);
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = BG_PANEL;
        widgets.noninteractive.weak_bg_fill = BG_PANEL;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.noninteractive.corner_radius = radius;
        for (state, fill, rim, text) in [
            (&mut widgets.inactive, BG_WIDGET, BORDER, TEXT),
            (
                &mut widgets.hovered,
                BG_WIDGET_HOVER,
                Color32::from_rgb(0x53, 0x53, 0x53),
                Color32::WHITE,
            ),
            (
                &mut widgets.active,
                tint(BG_WIDGET_HOVER, Color32::WHITE, 0.1),
                Color32::from_rgb(0x80, 0x80, 0x80),
                Color32::WHITE,
            ),
            (
                &mut widgets.open,
                BG_WIDGET_HOVER,
                Color32::from_rgb(0x53, 0x53, 0x53),
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
