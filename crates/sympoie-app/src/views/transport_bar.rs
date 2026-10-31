use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CursorIcon, FontId, Galley, Id, Layout, Painter, Rangef, Rect, Response,
    RichText, Sense, Shape, Stroke, Style, TextStyle, Ui, UiBuilder, pos2, vec2,
};
use sympoie::project::Project;
use sympoie::time::TimeSignature;
use sympoie::transport::PlaybackState;

use crate::actions::Action;
use crate::audio::{EngineStatus, OutputInfo};
use crate::state::LoopRegion;
use crate::ui::format;
use crate::ui::widgets::{self, Icon, MeterState};
use crate::ui::{analog, rack, theme};

pub struct Inputs<'a> {
    pub project: &'a Project,
    pub status: &'a EngineStatus,
    pub loop_region: LoopRegion,
    pub follow: bool,
    pub playhead_tick: f64,
    pub output: &'a OutputInfo,
    pub recording: bool,
}

const KEYS_WIDTH: f32 = 180.0;
/// First-pass guess for the display; after that it is fitted to its fields.
const DISPLAY_WIDTH: f32 = 500.0;
const DISPLAY_PADDING: f32 = 14.0;
const DRAG_PER_STEP: f32 = 8.0;
/// Space between neighbouring fields, and on each side of a divider.
const FIELD_GAP: f32 = 12.0;
/// Space between the columns of one readout, such as bar and beat.
const COLUMN_GAP: f32 = 6.0;
/// Side padding of the egui controls on the display.
const CONTROL_PADDING: f32 = 4.0;
const CAPTION_TRACKING: f32 = 0.7;
/// How tall the mono font's figures stand, as a share of its size.
const FIGURE_HEIGHT: f32 = 0.73;

pub fn show(ui: &mut Ui, inputs: &Inputs<'_>, meter: &mut MeterState, actions: &mut Vec<Action>) {
    let full = ui.max_rect();
    let rail = full.expand2(vec2(10.0, 5.0));
    let painter = ui.painter();
    analog::gradient_rect(painter, rail, theme::CONTROL_TOP, theme::CONTROL_BOTTOM);
    painter.hline(
        rail.x_range(),
        rail.top() + 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
    // Keys and meter sit on whole pixels so their edges stay crisp.
    let row = Rect::from_min_size(
        pos2(
            full.left(),
            (full.center().y - widgets::TRANSPORT_KEY.y / 2.0).round(),
        ),
        vec2(full.width(), widgets::TRANSPORT_KEY.y),
    );
    let keys = Rect::from_min_max(row.min, pos2(row.left() + KEYS_WIDTH, row.bottom()));
    let width_id = Id::new("transport-display-width");
    let display_width = ui
        .data(|data| data.get_temp::<f32>(width_id))
        .unwrap_or(DISPLAY_WIDTH);
    let display_left = (full.center().x - display_width / 2.0)
        .max(keys.right() + 12.0)
        .round();
    let display = Rect::from_min_max(
        pos2(display_left, (full.top() - 2.0).round()),
        pos2(display_left + display_width, (full.bottom() + 2.0).round()),
    );
    let right = Rect::from_min_max(pos2(display.right() + 16.0, row.top()), row.max);

    widgets::inside(ui, keys, Layout::left_to_right(Align::Center), |ui| {
        keys_section(ui, inputs, actions);
    });
    analog::lcd(ui.painter(), display, 5.0);
    let content_width = widgets::inside(
        ui,
        display.shrink2(vec2(DISPLAY_PADDING, 0.0)),
        Layout::left_to_right(Align::Center),
        |ui| {
            display_section(ui, inputs, actions);
            ui.min_rect().width()
        },
    );
    // The fields don't fit a fixed width (the clock grows past an hour, bars past
    // 999), so the display follows them, redrawing at once rather than overflowing.
    let fitted = (content_width + 2.0 * DISPLAY_PADDING).ceil();
    if (fitted - display_width).abs() > 0.5 {
        ui.data_mut(|data| data.insert_temp(width_id, fitted));
        ui.ctx().request_discard("transport display resized");
    }
    if right.width() > 60.0 {
        widgets::inside(ui, right, Layout::right_to_left(Align::Center), |ui| {
            output_section(ui, inputs, meter);
        });
    }
    ui.advance_cursor_after_rect(full);
}

/// The shallow recess a cluster of keys sits in.
fn tray(keys: Rect) -> Shape {
    let rect = keys.expand(2.0);
    Shape::Vec(vec![
        Shape::rect_filled(rect, 6.0, Color32::from_black_alpha(45)),
        Shape::hline(
            (rect.left() + 6.0)..=(rect.right() - 6.0),
            rect.bottom() + 0.5,
            Stroke::new(1.0, Color32::from_white_alpha(16)),
        ),
    ])
}

fn keys_section(ui: &mut Ui, inputs: &Inputs<'_>, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.x = 2.0;
    let transport_tray = ui.painter().add(Shape::Noop);
    let playing = inputs.status.state == PlaybackState::Playing;
    let can_play = inputs.status.render_error.is_none();
    let play = ui.add_enabled_ui(can_play || playing, |ui| {
        if playing {
            widgets::transport_button(ui, Icon::Play, Some(theme::PLAY), "Pause (Space)")
        } else {
            widgets::transport_button(ui, Icon::Play, None, "Play from start marker (Space)")
        }
    });
    let first = play.inner.rect;
    if play.inner.clicked() {
        actions.push(Action::TogglePlay);
    }
    if let Some(reason) = &inputs.status.render_error {
        play.response
            .on_disabled_hover_text(format!("Playback unavailable: {reason}"));
    }
    if widgets::transport_button(
        ui,
        Icon::Stop,
        None,
        "Stop; press again to return to the start (Home)",
    )
    .clicked()
    {
        actions.push(Action::Stop);
    }
    let record = widgets::transport_button(
        ui,
        Icon::Record,
        inputs.recording.then_some(theme::RECORD),
        if inputs.recording {
            "Stop recording (Q)"
        } else {
            "Record (Q)"
        },
    );
    if record.clicked() {
        actions.push(Action::ToggleRecord);
    }
    ui.painter()
        .set(transport_tray, tray(first.union(record.rect)));
    ui.add_space(12.0);
    let modes_tray = ui.painter().add(Shape::Noop);
    let cycle = widgets::icon_button(
        ui,
        Icon::Loop,
        inputs.loop_region.enabled,
        theme::LOOP,
        "Loop switch (L). Drag in the loop strip under the bar numbers to set the range.",
    );
    if cycle.clicked() {
        actions.push(Action::ToggleLoop);
    }
    let follow = widgets::icon_button(
        ui,
        Icon::Follow,
        inputs.follow,
        theme::SELECT,
        "Follow: keep the play position in view (F)",
    );
    if follow.clicked() {
        actions.push(Action::ToggleFollow);
    }
    ui.painter()
        .set(modes_tray, tray(cycle.rect.union(follow.rect)));
}

/// The display's two lines of text: every value shares one baseline and every
/// caption another, whatever their sizes.
#[derive(Clone, Copy)]
struct Lines {
    value: f32,
    caption: f32,
    span: Rangef,
}

impl Lines {
    fn new(display: Rect) -> Self {
        let caption = (display.bottom() - 8.0).round();
        Self {
            value: caption - 10.0,
            caption,
            span: display.y_range(),
        }
    }

    /// Where a value in `font` lights up under the pointer: its figures with the
    /// same room above as below.
    fn band(self, x: Rangef, font: &FontId) -> Rect {
        let figures = (FIGURE_HEIGHT * font.size).round();
        Rect::from_x_y_ranges(x, (self.value - figures - 3.0)..=(self.value + 3.0))
    }
}

fn display_section(ui: &mut Ui, inputs: &Inputs<'_>, actions: &mut Vec<Action>) {
    let project = inputs.project;
    let meter = project.time_signature();
    let menu_style = ui.style().clone();
    let lines = Lines::new(ui.max_rect());
    ui.spacing_mut().item_spacing.x = 0.0;

    let tick = inputs.playhead_tick.max(0.0).round() as u64;
    let position = meter.position_at(tick);
    let (bar, beat, division) = (
        position.bar.to_string(),
        position.beat.to_string(),
        (position.tick / format::TICKS_PER_SIXTEENTH + 1).to_string(),
    );
    readout(
        ui,
        lines,
        &theme::mono(16.0),
        theme::LCD_TEXT,
        &[(&bar, "Bar", 3), (&beat, "Beat", 1), (&division, "Div", 2)],
    )
    .on_hover_text("Arrangement position (bar.beat.sixteenth)");
    ui.add_space(FIELD_GAP);

    let seconds = inputs.playhead_tick * 60.0 / (project.bpm() * sympoie::TICKS_PER_QUARTER as f64);
    let clock = format::clock(seconds);
    let font = theme::mono(12.5);
    let digit = digit_width(ui, &font);
    field(ui, lines, &font, "Time", |ui, caption| {
        let width = (clock.chars().count().max(9) as f32 * digit).max(caption);
        ui.add_sized(
            vec2(width, ui.available_height()),
            egui::Label::new(RichText::new(clock).color(dimmed(theme::LCD_TEXT, 0.22))),
        );
    });
    divider(ui, lines);

    let region = inputs.loop_region;
    let bar = meter.ticks_per_bar();
    let length = region.end.saturating_sub(region.start);
    let loop_color = if region.enabled {
        theme::LOOP
    } else {
        dimmed(theme::LCD_TEXT, 0.45)
    };
    let font = theme::mono(14.0);
    let start_text = format::bar_beat(meter, region.start);
    let start_response = readout(ui, lines, &font, loop_color, &[(&start_text, "Loop", 6)])
        .on_hover_text("Loop start. Drag up or down to move the loop by bars.");
    let steps = drag_steps(ui, &start_response, Id::new("loop-start-drag"));
    if steps != 0 {
        let start = offset(region.start, steps, bar);
        actions.push(Action::SetLoop {
            start,
            end: start + length,
            enable: false,
        });
    }
    ui.add_space(FIELD_GAP);
    let length_text = loop_length(meter, length);
    let length_response = readout(ui, lines, &font, loop_color, &[(&length_text, "Length", 6)])
        .on_hover_text("Loop length (bars.beats.sixteenths). Drag up or down to change it.");
    let steps = drag_steps(ui, &length_response, Id::new("loop-length-drag"));
    if steps != 0 {
        let length = offset(length, steps, bar).max(meter.ticks_per_beat());
        actions.push(Action::SetLoop {
            start: region.start,
            end: region.start + length,
            enable: false,
        });
    }
    divider(ui, lines);
    tempo_section(ui, project, lines, menu_style, actions);
}

fn tempo_section(
    ui: &mut Ui,
    project: &Project,
    lines: Lines,
    menu_style: Arc<Style>,
    actions: &mut Vec<Action>,
) {
    let font = theme::mono(16.0);
    let digit = digit_width(ui, &font);
    let mut bpm = project.bpm();
    let response = field(ui, lines, &font, "Tempo", |ui, caption| {
        let width = (6.0 * digit).max(caption) + 2.0 * CONTROL_PADDING;
        readout_control(ui, lines, &font, |ui| {
            ui.add_sized(
                vec2(width, ui.available_height()),
                egui::DragValue::new(&mut bpm)
                    .speed(0.1)
                    .range(20.0..=999.0)
                    .fixed_decimals(2),
            )
        })
    })
    .on_hover_text("Tempo in BPM. Clips keep their bar positions; audio is not stretched.");
    if response.changed() {
        actions.push(Action::SetBpm(bpm));
    }
    ui.add_space(FIELD_GAP);

    let meter_now = project.time_signature();
    let mut numerator = meter_now.numerator();
    let slash_font = theme::mono(14.0);
    let slash = ui
        .painter()
        .fonts_mut(|fonts| fonts.glyph_width(&slash_font, '/'));
    let denominator_text = meter_now.denominator().to_string();
    field(ui, lines, &font, "Signature", |ui, caption| {
        // "4/4" is narrower than its caption, so it is centred under it.
        let numerator_width = numerator.to_string().len() as f32 * digit;
        let fields = numerator_width
            + slash
            + denominator_text.len() as f32 * digit
            + 4.0 * CONTROL_PADDING
            + ui.spacing().icon_spacing
            + ui.spacing().icon_width;
        ui.add_space(((caption - fields) / 2.0).max(0.0).floor());
        // Typing a new value opens a field as wide as the digits.
        ui.spacing_mut().interact_size.x = numerator_width + 2.0 * CONTROL_PADDING;
        let numerator_response = readout_control(ui, lines, &font, |ui| {
            ui.add(
                egui::DragValue::new(&mut numerator)
                    .range(1..=32)
                    .speed(0.05),
            )
        })
        .on_hover_text("Time signature: beats per bar");
        if numerator_response.changed()
            && let Ok(next) = TimeSignature::new(numerator, meter_now.denominator())
        {
            actions.push(Action::SetTimeSignature(next));
        }
        on_baseline(ui, lines, &slash_font, |ui| {
            ui.label(RichText::new("/").color(theme::LCD_LABEL));
        });
        readout_control(ui, lines, &font, |ui| {
            egui::ComboBox::from_id_salt("meter-denominator")
                .width(0.0)
                .selected_text(denominator_text)
                .icon(|ui, rect, visuals, _| {
                    let c = rect.center();
                    ui.painter().add(Shape::convex_polygon(
                        vec![
                            c + vec2(-2.5, -1.0),
                            c + vec2(2.5, -1.0),
                            c + vec2(0.0, 2.0),
                        ],
                        visuals.fg_stroke.color.lerp_to_gamma(theme::LCD, 0.4),
                        Stroke::NONE,
                    ));
                })
                .show_ui(ui, |ui| {
                    ui.set_style(menu_style);
                    widgets::menu_rows(ui.style_mut());
                    // The button is a single digit wide; let the menu fit "16" and "32".
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.set_min_width(36.0);
                    for denominator in [1, 2, 4, 8, 16, 32] {
                        if widgets::choice(
                            ui,
                            true,
                            meter_now.denominator() == denominator,
                            &denominator.to_string(),
                        )
                        .clicked()
                            && let Ok(next) = TimeSignature::new(meter_now.numerator(), denominator)
                        {
                            actions.push(Action::SetTimeSignature(next));
                        }
                    }
                })
                .response
        })
        .on_hover_text("Time signature: beat unit");
    });
}

/// Values painted straight onto the display, one column per `(text, caption,
/// digits)`, each at least `digits` wide with its caption centred underneath.
fn readout(
    ui: &mut Ui,
    lines: Lines,
    font: &FontId,
    color: Color32,
    columns: &[(&str, &str, usize)],
) -> Response {
    let painter = ui.painter().clone();
    let digit = digit_width(ui, font);
    let cells: Vec<_> = columns
        .iter()
        .map(|&(text, caption, digits)| {
            let value = painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER);
            let caption = caption_galley(&painter, caption);
            let width = (digits as f32 * digit)
                .max(value.size().x)
                .max(caption.size().x)
                .ceil();
            (value, caption, width)
        })
        .collect();
    let width = cells.iter().map(|(_, _, width)| width).sum::<f32>()
        + COLUMN_GAP * cells.len().saturating_sub(1) as f32;
    let left = ui.cursor().left();
    let response = ui.allocate_rect(
        lines.band(Rangef::new(left, left + width), font),
        Sense::click_and_drag(),
    );
    if ui.is_rect_visible(response.rect) {
        let lit = response.hovered() || response.dragged();
        if lit {
            painter.add(highlight(
                response.rect.expand2(vec2(CONTROL_PADDING, 0.0)),
                response.dragged(),
            ));
        }
        let ink = if lit {
            color.lerp_to_gamma(Color32::WHITE, 0.25)
        } else {
            color
        };
        let mut x = left;
        for (value, caption, width) in cells {
            let center = x + width / 2.0;
            painter.galley(
                pos2(center - value.size().x / 2.0, lines.value - ascent(&value)),
                value,
                ink,
            );
            paint_caption(&painter, lines, center, caption);
            x += width + COLUMN_GAP;
        }
    }
    response.on_hover_and_drag_cursor(CursorIcon::ResizeVertical)
}

/// Lays out egui widgets as display text in `font`, sitting on the value baseline,
/// with `caption` centred under them. `add` gets the caption's width.
fn field<R>(
    ui: &mut Ui,
    lines: Lines,
    font: &FontId,
    caption: &str,
    add: impl FnOnce(&mut Ui, f32) -> R,
) -> R {
    let painter = ui.painter().clone();
    let caption = caption_galley(&painter, caption);
    let (inner, used) = on_baseline(ui, lines, font, |ui| add(ui, caption.size().x));
    let missing = caption.size().x - used.span();
    if missing > 0.0 {
        ui.add_space(missing);
    }
    paint_caption(
        &painter,
        lines,
        used.min + used.span().max(caption.size().x) / 2.0,
        caption,
    );
    inner
}

/// Runs `add` in a row of egui widgets styled as display text in `font`, with the
/// text sitting on the value baseline. Also returns the span the widgets took.
fn on_baseline<R>(
    ui: &mut Ui,
    lines: Lines,
    font: &FontId,
    add: impl FnOnce(&mut Ui) -> R,
) -> (R, Rangef) {
    let sample = ui
        .painter()
        .layout_no_wrap("0".to_owned(), font.clone(), Color32::PLACEHOLDER);
    let row = Rect::from_min_size(
        pos2(ui.cursor().left(), lines.value - ascent(&sample)),
        vec2(ui.available_width(), sample.size().y),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Min)),
        |ui| {
            lcd_style(ui.style_mut(), font, row.height());
            // Like the painted values, controls reach only as far as their band. This
            // also keeps the selection of a value being typed off the caption below.
            let band = lines.band(row.x_range(), font);
            ui.set_clip_rect(ui.clip_rect().intersect(band));
            let inner = add(ui);
            (inner, ui.min_rect().x_range())
        },
    )
    .inner
}

/// Turns egui's raised keys into plain display text, one `height` tall row of `font`.
fn lcd_style(style: &mut Style, font: &FontId, height: f32) {
    style.override_font_id = Some(font.clone());
    style.text_styles.insert(TextStyle::Button, font.clone());
    style.spacing.button_padding = vec2(CONTROL_PADDING, 0.0);
    style.spacing.interact_size = vec2(0.0, height);
    style.spacing.icon_width = 6.0;
    style.spacing.icon_spacing = 2.0;
    // A value being typed is marked by its lit band, not a field with a focus ring.
    // The ring's colour stays: egui also uses it for the selected text.
    style.visuals.text_edit_bg_color = Some(Color32::TRANSPARENT);
    style.visuals.selection.stroke.width = 0.0;
    let widgets = &mut style.visuals.widgets;
    for (state, ink) in [
        (&mut widgets.inactive, theme::LCD_TEXT),
        (&mut widgets.hovered, Color32::WHITE),
        (&mut widgets.active, Color32::WHITE),
        (&mut widgets.open, Color32::WHITE),
    ] {
        state.bg_fill = Color32::TRANSPARENT;
        state.weak_bg_fill = Color32::TRANSPARENT;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke.color = ink;
        state.expansion = 0.0;
    }
}

/// An egui control on the display, lit like the painted values while it is
/// hovered, dragged, edited or open.
fn readout_control(
    ui: &mut Ui,
    lines: Lines,
    font: &FontId,
    add: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    let background = ui.painter().add(Shape::Noop);
    let response = add(ui);
    let held = response.dragged()
        || response.has_focus()
        || egui::ComboBox::is_open(ui.ctx(), response.id);
    if held || response.hovered() {
        ui.painter().set(
            background,
            highlight(lines.band(response.rect.x_range(), font), held),
        );
    }
    response
}

fn highlight(rect: Rect, held: bool) -> Shape {
    Shape::rect_filled(
        rect,
        4.0,
        Color32::from_white_alpha(if held { 16 } else { 10 }),
    )
}

fn divider(ui: &mut Ui, lines: Lines) {
    ui.add_space(FIELD_GAP);
    let (_, rect) = ui.allocate_space(vec2(2.0, 0.0));
    let y = (lines.span.min + 8.0)..=(lines.span.max - 8.0);
    let painter = ui.painter();
    painter.vline(
        rect.left() + 0.5,
        y.clone(),
        Stroke::new(1.0, Color32::from_black_alpha(160)),
    );
    painter.vline(
        rect.left() + 1.5,
        y,
        Stroke::new(1.0, Color32::from_white_alpha(10)),
    );
    ui.add_space(FIELD_GAP);
}

fn caption_galley(painter: &Painter, text: &str) -> Arc<Galley> {
    rack::spaced(
        painter,
        text,
        theme::prop(8.0),
        theme::LCD_LABEL,
        CAPTION_TRACKING,
    )
}

fn paint_caption(painter: &Painter, lines: Lines, center: f32, caption: Arc<Galley>) {
    // Tracking also follows the last letter, so it is left out of the centring.
    let width = caption.size().x - CAPTION_TRACKING;
    painter.galley(
        pos2(center - width / 2.0, lines.caption - ascent(&caption)),
        caption,
        theme::LCD_LABEL,
    );
}

/// How far below a galley's top its first baseline sits.
fn ascent(galley: &Galley) -> f32 {
    galley
        .rows
        .first()
        .and_then(|row| row.row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
        .unwrap_or(galley.size().y)
}

fn digit_width(ui: &Ui, font: &FontId) -> f32 {
    ui.painter().fonts_mut(|fonts| fonts.glyph_width(font, '0'))
}

fn dimmed(color: Color32, amount: f32) -> Color32 {
    color.lerp_to_gamma(theme::LCD, amount)
}

fn output_section(ui: &mut Ui, inputs: &Inputs<'_>, meter: &mut MeterState) {
    ui.spacing_mut().item_spacing.x = 8.0;
    widgets::meter(ui, meter, vec2(120.0, 14.0), false);
    let output = inputs.output;
    let color = if output.is_device {
        theme::TEXT_DIM
    } else {
        theme::WARN
    };
    let text = if output.is_device {
        format!(
            "{} · {} kHz",
            output.description,
            output.sample_rate as f32 / 1000.0
        )
    } else {
        output.description.clone()
    };
    let hint = if output.is_device {
        format!("Audio output: {}", output.detail)
    } else {
        format!(
            "{}.\nPlayback follows a silent clock; WAV export still works.",
            output.detail
        )
    };
    ui.add(egui::Label::new(RichText::new(text).size(10.5).color(color)).truncate())
        .on_hover_text(hint);
}

fn loop_length(meter: TimeSignature, ticks: u64) -> String {
    let bar = meter.ticks_per_bar().max(1);
    let beat = meter.ticks_per_beat().max(1);
    let bars = ticks / bar;
    let beats = (ticks % bar) / beat;
    let sixteenths = (ticks % beat) / format::TICKS_PER_SIXTEENTH;
    format!("{bars}.{beats}.{sixteenths}")
}

fn offset(value: u64, steps: i64, step: u64) -> u64 {
    let delta = steps.unsigned_abs().saturating_mul(step);
    if steps >= 0 {
        value.saturating_add(delta)
    } else {
        value.saturating_sub(delta)
    }
}

fn drag_steps(ui: &Ui, response: &Response, id: Id) -> i64 {
    if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(id, 0.0_f32));
    }
    if !response.dragged() {
        return 0;
    }
    let mut carried: f32 = ui.data(|data| data.get_temp(id)).unwrap_or(0.0);
    carried -= response.drag_delta().y;
    let steps = (carried / DRAG_PER_STEP).trunc();
    carried -= steps * DRAG_PER_STEP;
    ui.data_mut(|data| data.insert_temp(id, carried));
    steps as i64
}
