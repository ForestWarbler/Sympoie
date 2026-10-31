use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, InnerResponse, Painter, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, Style, Ui, Vec2, pos2, vec2,
};

use crate::ui::{analog, format, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Stop,
    Record,
    Loop,
    Follow,
}

pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.height().min(rect.width()) * 0.5;
    let stroke = Stroke::new(1.5, color);
    match icon {
        Icon::Play => {
            let points = vec![
                pos2(c.x - s * 0.55, c.y - s * 0.8),
                pos2(c.x + s * 0.75, c.y),
                pos2(c.x - s * 0.55, c.y + s * 0.8),
            ];
            painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 1.2)), 0.0, color);
        }
        Icon::Record => {
            painter.circle_filled(c, s * 0.7, color);
        }
        Icon::Loop => {
            let body = Rect::from_center_size(c, vec2(s * 1.9, s * 1.15));
            let radius = body.height() / 2.0;
            let half_circle = |center: Pos2, from: f32| -> Vec<Pos2> {
                (0..=8)
                    .map(|step| {
                        let angle = (from + 180.0 * step as f32 / 8.0).to_radians();
                        center + radius * vec2(angle.cos(), angle.sin())
                    })
                    .collect()
            };
            let left = pos2(body.left() + radius, c.y);
            let right = pos2(body.right() - radius, c.y);
            painter.add(Shape::line(half_circle(right, -90.0), stroke));
            painter.add(Shape::line(half_circle(left, 90.0), stroke));
            painter.line_segment(
                [pos2(left.x, body.top()), pos2(right.x + 0.5, body.top())],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(left.x - 0.5, body.bottom()),
                    pos2(right.x, body.bottom()),
                ],
                stroke,
            );
            let head = (s * 0.45).max(2.5);
            for (tip, direction) in [
                (pos2(c.x + head * 0.6, body.top()), 1.0),
                (pos2(c.x - head * 0.6, body.bottom()), -1.0),
            ] {
                painter.add(Shape::convex_polygon(
                    vec![
                        tip + vec2(direction * head * 0.7, 0.0),
                        tip + vec2(-direction * head * 0.5, -head * 0.75),
                        tip + vec2(-direction * head * 0.5, head * 0.75),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Follow => {
            let page = Rect::from_center_size(c, vec2(s * 1.6, s * 1.3));
            painter.rect_stroke(page, 1.5, stroke, StrokeKind::Inside);
            painter.vline(
                c.x - s * 0.3,
                (page.top() + 1.5)..=(page.bottom() - 1.5),
                Stroke::new(1.2, color),
            );
            let points = vec![
                pos2(c.x - s * 0.05, c.y - s * 0.32),
                pos2(c.x + s * 0.45, c.y),
                pos2(c.x - s * 0.05, c.y + s * 0.32),
            ];
            painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
        }
    }
}

pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    active: bool,
    accent: Color32,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(TRANSPORT_KEY, Sense::click());
    if ui.is_rect_visible(rect) {
        paint_transport_cap(ui, rect, &response, icon, active.then_some(accent));
    }
    response.on_hover_text(tooltip)
}

/// A transport key. Play lights its glyph while playing; the other
/// keys fill with their colour while engaged.
pub fn transport_button(ui: &mut Ui, icon: Icon, lit: Option<Color32>, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(TRANSPORT_KEY, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            lit.is_some(),
            tooltip,
        )
    });
    if ui.is_rect_visible(rect) {
        paint_transport_cap(ui, rect, &response, icon, lit);
    }
    response.on_hover_text(tooltip)
}

pub const TRANSPORT_KEY: Vec2 = vec2(30.0, 22.0);

/// Dark ink on light fills, white ink on dark ones, for key legends. The cut sits
/// above the WCAG midpoint so the blue and red keys keep white legends.
pub fn ink_on(fill: Color32) -> Color32 {
    if luminance(fill) > 0.27 {
        theme::TEXT_ON_COLOR
    } else {
        Color32::WHITE
    }
}

/// Ink for a name set on a track colour (region strips, name plates): dark across the
/// palette, so a track's names read alike everywhere, unless the colour is too dark.
pub fn track_ink(color: Color32) -> Color32 {
    if luminance(color) > 0.18 {
        theme::TEXT_ON_COLOR
    } else {
        Color32::WHITE
    }
}

fn luminance(color: Color32) -> f32 {
    let linear = egui::Rgba::from(color);
    0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
}

pub const FIELD_RADIUS: f32 = 3.0;
const FIELD_RIM: Color32 = Color32::from_black_alpha(190);
const FIELD_RIM_HOT: Color32 = Color32::from_gray(0x5a);

/// The dark inset field behind a painted value.
pub fn field(painter: &Painter, rect: Rect, hot: bool) {
    let (fill, rim) = if hot {
        (theme::BG_WIDGET_HOVER, FIELD_RIM_HOT)
    } else {
        (theme::BG_WIDGET, FIELD_RIM)
    };
    painter.rect(
        rect,
        FIELD_RADIUS,
        fill,
        Stroke::new(1.0, rim),
        StrokeKind::Inside,
    );
}

/// Dresses egui drag values and combo boxes in `style` as [`field`]s.
pub fn field_visuals(style: &mut Style) {
    let widgets = &mut style.visuals.widgets;
    for (state, fill, rim) in [
        (&mut widgets.inactive, theme::BG_WIDGET, FIELD_RIM),
        (&mut widgets.hovered, theme::BG_WIDGET_HOVER, FIELD_RIM_HOT),
        (&mut widgets.active, theme::BG_DEEP, theme::SELECT),
        (&mut widgets.open, theme::BG_WIDGET_HOVER, FIELD_RIM_HOT),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, rim);
        state.corner_radius = CornerRadius::same(FIELD_RADIUS as u8);
    }
}

fn paint_transport_cap(ui: &Ui, rect: Rect, response: &Response, icon: Icon, lit: Option<Color32>) {
    let painter = ui.painter();
    let glows = icon == Icon::Play && lit.is_some();
    let fill = lit.filter(|_| !glows);
    let cap = analog::key(
        painter,
        rect.shrink2(vec2(1.0, 1.0)),
        4.0,
        fill,
        response.hovered(),
        glows || response.is_pointer_button_down_on(),
    );
    let glyph = cap.shrink2(vec2(7.0, 4.0));
    let color = match (lit, fill) {
        (Some(light), None) => {
            painter.circle_filled(glyph.center(), glyph.height(), light.gamma_multiply(0.10));
            light
        }
        (Some(_), Some(fill)) => ink_on(fill),
        (None, _) if icon == Icon::Record => theme::RECORD,
        (None, _) if response.hovered() => Color32::WHITE,
        (None, _) => theme::TEXT,
    };
    paint_icon(painter, glyph, icon, color);
}

/// The legend ink of a key: contrasting while lit, white while hovered.
pub fn key_ink(light: Option<Color32>, hovered: bool) -> Color32 {
    match light {
        Some(light) => ink_on(light),
        None if hovered => Color32::WHITE,
        None => theme::TEXT,
    }
}

pub fn toggle(ui: &mut Ui, text: &str, on: bool, color: Color32, size: Vec2) -> Response {
    toggle_with(ui, text, theme::prop(10.0), on, color, size)
}

/// A [`toggle`] whose legend is a lone symbol such as ø, drawn larger so it reads.
pub fn glyph_toggle(ui: &mut Ui, glyph: &str, on: bool, color: Color32, size: Vec2) -> Response {
    toggle_with(ui, glyph, theme::prop(12.0), on, color, size)
}

fn toggle_with(
    ui: &mut Ui,
    text: &str,
    font: egui::FontId,
    on: bool,
    color: Color32,
    size: Vec2,
) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), theme::TEXT);
    let size = vec2(size.x.max(galley.size().x + 12.0), size.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, text)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let cap = analog::key(
            painter,
            rect.shrink2(vec2(1.0, 1.0)),
            3.5,
            on.then_some(color),
            response.hovered(),
            response.is_pointer_button_down_on(),
        );
        let ink = key_ink(on.then_some(color), response.hovered());
        painter.text(cap.center(), Align2::CENTER_CENTER, text, font, ink);
    }
    response
}

pub fn activator(ui: &mut Ui, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
    if ui.is_rect_visible(rect) {
        analog::round_switch(
            ui.painter(),
            rect.center(),
            5.0,
            on.then_some(theme::ACCENT),
            response.hovered(),
            response.is_pointer_button_down_on(),
        );
    }
    response.on_hover_text(if on {
        "Device on. Click to bypass."
    } else {
        "Device bypassed. Click to switch it on."
    })
}

/// The raised strip behind panel titles.
pub fn title_bar(painter: &egui::Painter, rect: Rect) {
    analog::gradient_rect(
        painter,
        rect,
        theme::BG_RAISED,
        Color32::from_rgb(0x32, 0x32, 0x33),
    );
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(16)),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(150)),
    );
}

pub fn color_bar(painter: &egui::Painter, rect: Rect, color: Color32, text: &str) {
    title_bar(painter, rect);
    let tab = Rect::from_min_size(
        rect.min + vec2(0.0, 1.0),
        vec2(3.0, (rect.height() - 2.0).max(0.0)),
    );
    painter.rect_filled(tab, 0.0, color);
    fitted_text(
        painter,
        Rect::from_min_max(
            pos2(tab.right() + 7.0, rect.top()),
            rect.max - vec2(5.0, 0.0),
        ),
        text,
        theme::prop(10.5),
        theme::TEXT,
        Align2::LEFT_CENTER,
    );
}

/// The playhead's head in a ruler: a small pentagon pointing down at `x`.
pub fn playhead_head(painter: &Painter, x: f32, top: f32, color: Color32) {
    painter.add(Shape::convex_polygon(
        vec![
            pos2(x - 4.0, top),
            pos2(x + 4.0, top),
            pos2(x + 4.0, top + 2.0),
            pos2(x, top + 6.0),
            pos2(x - 4.0, top + 2.0),
        ],
        color,
        Stroke::new(1.0, Color32::from_black_alpha(110)),
    ));
}

/// A blue focus ring around a text field. egui draws focus with `selection.stroke`,
/// which also colours selected text, so the ring is painted over it instead.
pub fn focus_ring(painter: &Painter, rect: Rect, radius: f32) {
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::SELECT),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(2.0, theme::SELECT.gamma_multiply(0.45)),
        StrokeKind::Outside,
    );
}

/// Paints the [`focus_ring`] around a text edit while it has focus.
pub fn ring_focus(ui: &Ui, response: &Response) {
    if response.has_focus() {
        let radius = ui.style().interact(response).corner_radius.nw;
        focus_ring(ui.painter(), response.rect, f32::from(radius));
    }
}

/// Menu rows that light up blue with white text. Row text starts past a gutter as
/// wide as a radio button, where [`choice`] puts its tick. Apply it as a popup's
/// style (`Popup::style`, `ComboBox::popup_style`) rather than through
/// `ui.style_mut()`, so that submenus inherit it.
pub fn menu_rows(style: &mut Style) {
    egui::menu::menu_style(style);
    let gutter = style.spacing.icon_width + style.spacing.icon_spacing;
    style.spacing.button_padding = vec2(gutter, 2.0);
    style.spacing.interact_size.y = 20.0;
    style.spacing.item_spacing.y = 1.0;
    style.visuals.weak_text_color = Some(theme::TEXT_DIM);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, theme::HAIRLINE);
    menu_highlight(style);
}

/// The blue highlight of hovered and open menu rows and titles.
pub fn menu_highlight(style: &mut Style) {
    // Darkened just enough to keep white text on it legible.
    let fill = theme::SELECT.lerp_to_gamma(Color32::BLACK, 0.2);
    let widgets = &mut style.visuals.widgets;
    widgets.inactive.fg_stroke.color = theme::TEXT;
    for state in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        state.weak_bg_fill = fill;
        state.bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        state.corner_radius = CornerRadius::same(4);
    }
}

/// [`menu_rows`] as a style modifier, built once: popups ask for one every frame.
pub fn menu_style() -> egui::style::StyleModifier {
    static STYLE: std::sync::LazyLock<egui::style::StyleModifier> =
        std::sync::LazyLock::new(|| menu_rows.into());
    STYLE.clone()
}

/// [`Response::context_menu`] with the app's menu rows.
pub fn context_menu(
    response: &Response,
    add_contents: impl FnOnce(&mut Ui),
) -> Option<InnerResponse<()>> {
    egui::Popup::context_menu(response)
        .style(menu_style())
        .show(add_contents)
}

/// A menu row whose current choice carries a tick in the [`menu_rows`] gutter.
pub fn choice(ui: &mut Ui, enabled: bool, selected: bool, text: &str) -> Response {
    ticked_row(
        ui,
        enabled,
        selected,
        text,
        egui::WidgetType::SelectableLabel,
    )
}

/// A [`choice`] among exclusive modes, announced as a radio button.
pub fn radio_choice(ui: &mut Ui, selected: bool, text: &str) -> Response {
    ticked_row(ui, true, selected, text, egui::WidgetType::RadioButton)
}

fn ticked_row(
    ui: &mut Ui,
    enabled: bool,
    selected: bool,
    text: &str,
    kind: egui::WidgetType,
) -> Response {
    let response = ui.add_enabled(enabled, egui::Button::new(text));
    response.widget_info(|| egui::WidgetInfo::selected(kind, enabled, selected, text));
    if selected && ui.is_rect_visible(response.rect) {
        let ink = ui.style().interact(&response).text_color();
        let tick = if enabled {
            ink
        } else {
            ui.visuals().disable(ink)
        };
        let gutter = ui.spacing().button_padding.x;
        let center = pos2(
            response.rect.left() + gutter / 2.0,
            response.rect.center().y,
        );
        ui.painter().add(Shape::line(
            vec![
                center + vec2(-3.5, 0.0),
                center + vec2(-1.0, 2.5),
                center + vec2(3.5, -3.0),
            ],
            Stroke::new(1.5, tick),
        ));
    }
    response
}

/// [`choice`] for a value, as `Ui::selectable_value`: a click picks `value`, and
/// the response is marked changed only when that changes `current`.
pub fn choice_value<T: PartialEq>(ui: &mut Ui, current: &mut T, value: T, text: &str) -> Response {
    let mut response = choice(ui, true, *current == value, text);
    if response.clicked() && *current != value {
        *current = value;
        response.mark_changed();
    }
    response
}

/// A checkbox that fills blue while ticked.
pub fn checkbox(ui: &mut Ui, enabled: bool, checked: &mut bool, text: &str) -> Response {
    let widgets = ui.visuals().widgets.clone();
    if *checked {
        blue_tick(&mut ui.visuals_mut().widgets);
    }
    let response = ui.add_enabled(enabled, egui::Checkbox::new(checked, text));
    ui.visuals_mut().widgets = widgets;
    response
}

/// Fills a ticked checkbox blue behind a white tick.
pub fn blue_tick(widgets: &mut egui::style::Widgets) {
    let lit = theme::SELECT.lerp_to_gamma(Color32::WHITE, 0.15);
    for (state, fill) in [
        (&mut widgets.inactive, theme::SELECT),
        (&mut widgets.hovered, lit),
        (&mut widgets.active, theme::SELECT),
    ] {
        state.bg_fill = fill;
        state.fg_stroke = Stroke::new(2.0, Color32::WHITE);
    }
}

/// Lays keys edge to edge in a shared recess.
pub fn key_group(ui: &mut Ui, keys: impl FnOnce(&mut Ui)) {
    let tray = ui.painter().add(Shape::Noop);
    let group = ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        keys(ui);
    });
    let rect = group.response.rect;
    ui.painter().set(
        tray,
        Shape::Vec(vec![
            Shape::rect_filled(rect, 4.5, Color32::from_black_alpha(80)),
            Shape::line_segment(
                [
                    pos2(rect.left() + 4.0, rect.bottom() + 0.5),
                    pos2(rect.right() - 4.0, rect.bottom() + 0.5),
                ],
                Stroke::new(1.0, Color32::from_white_alpha(9)),
            ),
        ]),
    );
}

/// A dim caption inside a menu, lined up with the rows past their gutter. Text with
/// its own colour keeps it.
pub fn menu_caption(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    let gutter = ui.spacing().button_padding.x;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(gutter as i8, 1))
        .show(ui, |ui| {
            ui.visuals_mut().override_text_color = Some(theme::TEXT_DIM);
            ui.label(text)
        })
        .inner
}

fn arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, stroke: Stroke) {
    let steps = 24;
    let points: Vec<Pos2> = (0..=steps)
        .map(|step| {
            let t = from + (to - from) * step as f32 / steps as f32;
            center + radius * vec2(t.sin(), -t.cos())
        })
        .collect();
    painter.add(Shape::line(points, stroke));
}

const KNOB_RANGE: f32 = 2.36;

fn paint_knob(painter: &egui::Painter, center: Pos2, radius: f32, value: f32, bipolar: bool) {
    arc(
        painter,
        center,
        radius,
        -KNOB_RANGE,
        KNOB_RANGE,
        Stroke::new(2.0, Color32::from_black_alpha(140)),
    );
    let angle = -KNOB_RANGE + value.clamp(0.0, 1.0) * 2.0 * KNOB_RANGE;
    let origin = if bipolar { 0.0 } else { -KNOB_RANGE };
    if (angle - origin).abs() > 0.01 {
        let (from, to) = (angle.min(origin), angle.max(origin));
        arc(
            painter,
            center,
            radius,
            from,
            to,
            Stroke::new(4.0, theme::ACCENT.gamma_multiply(0.16)),
        );
        arc(
            painter,
            center,
            radius,
            from,
            to,
            Stroke::new(2.0, theme::ACCENT),
        );
    }
    if bipolar {
        let tick = center + vec2(0.0, -(radius + 1.0));
        painter.line_segment(
            [tick, tick - vec2(0.0, 2.0)],
            Stroke::new(1.0, theme::TEXT_DIM),
        );
    }
    analog::knob_cap(painter, center, (radius - 3.0).max(3.0), angle, false);
}

pub fn pan_knob(ui: &mut Ui, value: &mut f32, diameter: f32) -> Response {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::splat(diameter), Sense::click_and_drag());
    if response.dragged() {
        let delta = response.drag_delta();
        let speed = if ui.input(|i| i.modifiers.shift) {
            0.002
        } else {
            0.01
        };
        let next = (*value + (delta.x - delta.y) * speed).clamp(-1.0, 1.0);
        if next != *value {
            *value = next;
            response.mark_changed();
        }
    }
    if response.double_clicked() && *value != 0.0 {
        *value = 0.0;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        paint_knob(
            ui.painter(),
            rect.center(),
            diameter * 0.5 - 2.0,
            (*value + 1.0) * 0.5,
            true,
        );
    }
    let hint = format!("Pan {}", format::pan(*value));
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(hint)
}

pub fn knob(ui: &mut Ui, value: &mut f32, label: &str, display: &str) -> Response {
    let size = vec2(48.0, 58.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if response.dragged() {
        let speed = if ui.input(|i| i.modifiers.shift) {
            0.001
        } else {
            0.006
        };
        let delta = response.drag_delta();
        let next = (*value + (delta.x - delta.y) * speed).clamp(0.0, 1.0);
        if next != *value {
            *value = next;
            response.mark_changed();
        }
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        fitted_text(
            painter,
            Rect::from_min_size(rect.min, vec2(rect.width(), 13.0)),
            label,
            theme::prop(10.0),
            theme::TEXT_DIM,
            Align2::CENTER_CENTER,
        );
        let center = pos2(rect.center().x, rect.top() + 29.0);
        paint_knob(painter, center, 12.5, *value, false);
        let color = if response.hovered() || response.dragged() {
            Color32::WHITE
        } else {
            theme::TEXT
        };
        fitted_text(
            painter,
            Rect::from_min_max(pos2(rect.left(), rect.bottom() - 13.0), rect.max),
            display,
            theme::prop(10.0),
            color,
            Align2::CENTER_CENTER,
        );
    }
    response.on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
}

/// The travel of a fader drawn in `rect`: its cap's centre stays inside it.
pub fn fader_track(rect: Rect) -> Rect {
    Rect::from_center_size(rect.center(), vec2(7.0, (rect.height() - 24.0).max(1.0)))
}

fn fader_y(track: Rect, gain: f32) -> f32 {
    egui::lerp(track.bottom()..=track.top(), format::gain_to_fader(gain))
}

/// A vertical fader centred in its rect, without a scale; see [`fader_scale`].
pub fn gain_fader(ui: &mut Ui, gain: &mut f32, size: Vec2, color: Color32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let track = fader_track(rect);
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let delta = -response.drag_delta().y / track.height() * if fine { 0.2 } else { 1.0 };
        let position = (format::gain_to_fader(*gain) + delta).clamp(0.0, 1.0);
        let next = format::fader_to_gain(position);
        if next != *gain {
            *gain = next;
            response.mark_changed();
        }
    }
    if response.double_clicked() && *gain != 1.0 {
        *gain = 1.0;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let center_x = track.center().x;
        analog::well(painter, track.expand2(vec2(1.0, 2.0)), 3.0);
        painter.vline(
            center_x + 1.0,
            track.y_range(),
            Stroke::new(1.0, Color32::from_gray(24)),
        );
        let y = fader_y(track, *gain);
        let hot = if response.hovered() || response.dragged() {
            Color32::from_gray(0xf4)
        } else {
            Color32::from_gray(0xd0)
        };
        let cap = Rect::from_center_size(pos2(center_x, y), vec2(28.0, 20.0));
        painter.rect_filled(
            cap.translate(vec2(0.0, 2.0)).expand(1.0),
            2.5,
            Color32::from_black_alpha(160),
        );
        painter.rect_filled(cap, 2.5, Color32::from_gray(0x45));
        let face = cap.shrink(1.0);
        analog::gradient_rect(painter, face, hot, Color32::from_gray(0x79));
        painter.rect_stroke(
            cap,
            2.5,
            Stroke::new(1.0, Color32::from_gray(0x12)),
            StrokeKind::Inside,
        );
        painter.hline(
            face.x_range(),
            face.top(),
            Stroke::new(1.0, Color32::from_white_alpha(160)),
        );
        for offset in [-5.0, 5.0] {
            let range = (cap.left() + 4.0)..=(cap.right() - 4.0);
            painter.hline(
                range.clone(),
                y + offset,
                Stroke::new(1.0, Color32::from_black_alpha(100)),
            );
            painter.hline(
                range,
                y + offset + 1.0,
                Stroke::new(1.0, Color32::from_white_alpha(110)),
            );
        }
        painter.hline(
            face.x_range(),
            y,
            Stroke::new(3.0, Color32::from_gray(0x26)),
        );
        painter.hline(
            (cap.left() + 3.0)..=(cap.right() - 3.0),
            y,
            Stroke::new(1.0, color),
        );
    }
    let hint = format!(
        "Volume {}\nShift: fine adjustment · Double-click: 0 dB",
        format::db(*gain)
    );
    response
        .on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(hint)
}

/// The dB scale for a [`gain_fader`] drawn in `fader`, painted in the column `rect`
/// beside it. Labels that would crowd the one above are left out; 0 dB always shows.
pub fn fader_scale(painter: &egui::Painter, rect: Rect, fader: Rect) {
    let track = fader_track(fader);
    let marks = [
        (6.0, "+6"),
        (0.0, "0"),
        (-6.0, "6"),
        (-12.0, "12"),
        (-24.0, "24"),
        (-48.0, "48"),
    ];
    let mut last: Option<f32> = None;
    let mut ticks = Vec::with_capacity(marks.len() + 1);
    for (db, label) in marks {
        let y = fader_y(track, format::db_to_gain(db));
        let room = last.is_none_or(|above| y - above >= 11.0);
        if db == 0.0 || room {
            ticks.push((y, label, db == 0.0));
            last = Some(y);
        }
    }
    if last.is_none_or(|above| track.bottom() - above >= 11.0) {
        ticks.push((track.bottom(), "∞", false));
    }
    for (y, label, unity) in ticks {
        let ink = if unity {
            theme::TEXT
        } else {
            theme::TEXT_FAINT
        };
        let y = y.round() + 0.5;
        painter.hline(
            rect.left()..=(rect.left() + if unity { 5.0 } else { 3.0 }),
            y,
            Stroke::new(1.0, ink),
        );
        painter.hline(
            (rect.right() - if unity { 5.0 } else { 3.0 })..=rect.right(),
            y,
            Stroke::new(1.0, ink),
        );
        painter.text(
            pos2(rect.center().x, y),
            Align2::CENTER_CENTER,
            label,
            theme::mono(8.0),
            ink,
        );
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MeterState {
    level: [f32; 2],
    hold: [f32; 2],
    hold_age: [f32; 2],
    pub clipped: bool,
}

impl MeterState {
    pub fn update(&mut self, peaks: [f32; 2], dt: f32) {
        for (channel, peak) in peaks.into_iter().enumerate() {
            let fallen = self.level[channel] * 10_f32.powf(-1.3 * dt);
            self.level[channel] = peak.max(fallen);
            self.hold_age[channel] += dt;
            if peak >= self.hold[channel] || self.hold_age[channel] > 1.5 {
                self.hold[channel] = peak.max(fallen);
                self.hold_age[channel] = 0.0;
            }
            if peak > 1.0 {
                self.clipped = true;
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.level
            .iter()
            .chain(&self.hold)
            .any(|level| *level > 1e-4)
    }
}

const METER_FLOOR_DB: f32 = -60.0;
const METER_CEIL_DB: f32 = 6.0;

fn meter_fraction(level: f32) -> f32 {
    let db = format::gain_to_db(level).max(METER_FLOOR_DB);
    ((db - METER_FLOOR_DB) / (METER_CEIL_DB - METER_FLOOR_DB)).clamp(0.0, 1.0)
}

fn meter_color(fraction: f32) -> Color32 {
    let zero = meter_fraction(1.0);
    let minus_six = meter_fraction(0.5);
    if fraction > zero {
        theme::ERROR
    } else if fraction > minus_six {
        theme::WARN
    } else {
        theme::OK
    }
}

pub fn meter(ui: &mut Ui, state: &mut MeterState, size: Vec2, vertical: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    paint_meter(ui, state, rect, response, vertical)
}

pub fn meter_at(
    ui: &mut Ui,
    state: &mut MeterState,
    rect: Rect,
    id: Id,
    vertical: bool,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    paint_meter(ui, state, rect, response, vertical)
}

fn paint_meter(
    ui: &mut Ui,
    state: &mut MeterState,
    rect: Rect,
    response: Response,
    vertical: bool,
) -> Response {
    if response.clicked() {
        state.clipped = false;
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let light = 5.0;
    let gap = 1.0;
    let (bars_rect, light_rect) = if vertical {
        (
            Rect::from_min_max(pos2(rect.left(), rect.top() + light + gap), rect.max),
            Rect::from_min_size(rect.min, vec2(rect.width(), light)),
        )
    } else {
        (
            Rect::from_min_max(rect.min, pos2(rect.right() - light - gap, rect.bottom())),
            Rect::from_min_size(
                pos2(rect.right() - light, rect.top()),
                vec2(light, rect.height()),
            ),
        )
    };
    analog::lamp(painter, light_rect.shrink(0.5), theme::ERROR, state.clipped);
    analog::well(painter, bars_rect, 1.5);

    let inner = bars_rect.shrink(1.0);
    let length = if vertical {
        inner.height()
    } else {
        inner.width()
    };
    let size = if length >= 120.0 { 3.0 } else { 2.0 };
    let pitch = size + 1.0;
    let count = ((length + 1.0) / pitch).floor().max(1.0) as usize;
    let margin = ((length - (count as f32 * pitch - 1.0)) / 2.0).max(0.0);
    let mut glows = egui::Mesh::default();
    let mut leds = egui::Mesh::default();
    for channel in 0..2 {
        let lane = if vertical {
            let width = (inner.width() - gap) / 2.0;
            Rect::from_min_size(
                pos2(inner.left() + channel as f32 * (width + gap), inner.top()),
                vec2(width, inner.height()),
            )
        } else {
            let height = (inner.height() - gap) / 2.0;
            Rect::from_min_size(
                pos2(inner.left(), inner.top() + channel as f32 * (height + gap)),
                vec2(inner.width(), height),
            )
        };
        let level = state.level[channel];
        let fraction = meter_fraction(level);
        let hold = (state.hold[channel] > 1e-5).then(|| {
            ((meter_fraction(state.hold[channel]) * count as f32) as usize).min(count - 1)
        });
        for index in 0..count {
            let from = index as f32 / count as f32;
            let start = margin + index as f32 * pitch;
            let led = if vertical {
                Rect::from_min_max(
                    pos2(lane.left(), lane.bottom() - start - size),
                    pos2(lane.right(), lane.bottom() - start),
                )
            } else {
                Rect::from_min_max(
                    pos2(lane.left() + start, lane.top()),
                    pos2(lane.left() + start + size, lane.bottom()),
                )
            };
            let color = meter_color((index as f32 + 0.5) / count as f32);
            let lit = (level > 1e-5 && fraction > from) || hold == Some(index);
            if lit {
                analog::led_glow(&mut glows, led, color);
            }
            analog::led(&mut leds, led, color, lit, vertical);
        }
    }
    painter.add(Shape::mesh(glows));
    painter.add(Shape::mesh(leds));
    analog::glass(painter, inner);
    response.on_hover_text(format!(
        "Peak level — L: {}, R: {}\nClick to reset the clip light.",
        format::db(state.level[0]),
        format::db(state.level[1]),
    ))
}

pub fn inside<R>(
    ui: &mut Ui,
    rect: Rect,
    layout: egui::Layout,
    contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    contents(&mut child)
}

pub fn fitted_text(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: egui::FontId,
    color: Color32,
    align: Align2,
) {
    let Some(galley) = fit_galley(painter, rect.width(), text, &font, color) else {
        return;
    };
    let position = align
        .anchor_size(align.pos_in_rect(&rect), galley.size())
        .min;
    painter.galley(position, galley, color);
}

/// Lays out `text`, eliding it with "…" until it is at most `width` wide.
///
/// This runs for every visible clip label on every frame, so it must stay
/// cheap even when thousands of labels are too wide: it bails out before any
/// layout when there is no room, and finds the longest fitting prefix with a
/// binary search instead of trying every length.
fn fit_galley(
    painter: &egui::Painter,
    width: f32,
    text: &str,
    font: &egui::FontId,
    color: Color32,
) -> Option<std::sync::Arc<egui::Galley>> {
    if width.is_nan() || width <= 0.0 {
        return None;
    }
    let layout = |shown: String| painter.layout_no_wrap(shown, font.clone(), color);
    let full = layout(text.to_owned());
    if full.size().x <= width {
        return Some(full);
    }
    let count = text.chars().count();
    if count <= 1 {
        return None;
    }
    let elided = |keep: usize| {
        let end = text
            .char_indices()
            .nth(keep)
            .map_or(text.len(), |(index, _)| index);
        layout(format!("{}…", text[..end].trim_end()))
    };
    let mut best = elided(1);
    if best.size().x > width {
        return None;
    }
    let (mut low, mut high) = (1, count - 1);
    while low < high {
        let middle = (low + high).div_ceil(2);
        let candidate = elided(middle);
        if candidate.size().x <= width {
            low = middle;
            best = candidate;
        } else {
            high = middle - 1;
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The previous linear elision, kept as the reference behaviour.
    fn linear_fit(painter: &egui::Painter, width: f32, text: &str) -> Option<String> {
        let font = theme::prop(10.0);
        let mut shown = text.to_owned();
        let mut galley = painter.layout_no_wrap(shown.clone(), font.clone(), Color32::WHITE);
        while galley.size().x > width && shown.chars().count() > 1 {
            shown.pop();
            while shown.ends_with(char::is_whitespace) {
                shown.pop();
            }
            galley = painter.layout_no_wrap(format!("{shown}…"), font.clone(), Color32::WHITE);
        }
        (galley.size().x <= width).then(|| galley.text().to_owned())
    }

    #[test]
    fn fitted_labels_fit_and_appear_exactly_when_the_linear_search_found_room() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let painter = ui.painter();
            let font = theme::prop(10.0);
            for text in [
                "NT Closed Hat",
                "Bass  Phase Current",
                "a b",
                "x",
                "Ünïcødé ✓",
            ] {
                let mut width = -8.0;
                while width < 160.0 {
                    let fitted = fit_galley(painter, width, text, &font, Color32::WHITE);
                    let reference = linear_fit(painter, width, text);
                    assert_eq!(fitted.is_some(), reference.is_some(), "{text:?} at {width}");
                    if let Some(galley) = fitted {
                        assert!(galley.size().x <= width, "{text:?} at {width}");
                    }
                    width += 0.5;
                }
            }
            assert_eq!(
                fit_galley(painter, 1000.0, "NT Closed Hat", &font, Color32::WHITE)
                    .map(|galley| galley.text().to_owned()),
                Some("NT Closed Hat".to_owned())
            );
            assert!(fit_galley(painter, -4.0, "NT Closed Hat", &font, Color32::WHITE).is_none());
            assert!(
                fit_galley(painter, f32::NAN, "NT Closed Hat", &font, Color32::WHITE).is_none()
            );
        });
        output.textures_delta.clear();
    }
}
