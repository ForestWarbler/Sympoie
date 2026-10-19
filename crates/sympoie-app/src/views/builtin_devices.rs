use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, Id, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use sympoie::builtin::instruments::{SamplePreview, sample_preview, sample_state_fingerprint};
use sympoie::builtin::{self, BuiltinKind, ParameterSpec, Telemetry};
use sympoie::plugin::PluginInstance;

use crate::actions::Action;
use crate::ui::{rack, theme, widgets};

pub fn name(kind: BuiltinKind) -> &'static str {
    match kind {
        BuiltinKind::Oscil => "Oscil",
        BuiltinKind::Sampler => "Sampler",
        BuiltinKind::Eq => "EQ",
        BuiltinKind::Comp => "Comp",
        BuiltinKind::VariMu => "Vari-Mu",
        BuiltinKind::Rain => "Rain",
        BuiltinKind::Ferro => "Ferro",
    }
}

pub fn editor_size(kind: BuiltinKind) -> [f32; 2] {
    match kind {
        BuiltinKind::Oscil => [700.0, 500.0],
        BuiltinKind::Sampler => [700.0, 410.0],
        BuiltinKind::Eq => [890.0, 450.0],
        BuiltinKind::Comp => [530.0, 330.0],
        BuiltinKind::VariMu => [720.0, 400.0],
        BuiltinKind::Rain => [700.0, 520.0],
        BuiltinKind::Ferro => [1180.0, 440.0],
    }
}

pub fn minimum_editor_size(kind: BuiltinKind) -> [f32; 2] {
    match kind {
        BuiltinKind::Oscil => [690.0, 300.0],
        BuiltinKind::Sampler => [690.0, 300.0],
        BuiltinKind::Eq => [880.0, 300.0],
        BuiltinKind::Comp => [520.0, 300.0],
        BuiltinKind::VariMu => [600.0, 330.0],
        BuiltinKind::Rain => [690.0, 300.0],
        BuiltinKind::Ferro => [826.0, 308.0],
    }
}

pub fn accent(kind: BuiltinKind) -> Color32 {
    match kind {
        BuiltinKind::Oscil => theme::ACCENT,
        BuiltinKind::Sampler => Color32::from_rgb(113, 210, 184),
        BuiltinKind::Eq => Color32::from_rgb(114, 187, 239),
        BuiltinKind::Comp => Color32::from_rgb(208, 157, 238),
        BuiltinKind::VariMu => Color32::from_rgb(232, 168, 84),
        BuiltinKind::Rain => Color32::from_rgb(104, 192, 234),
        BuiltinKind::Ferro => Color32::from_rgb(255, 166, 56),
    }
}

fn set(plugin: &PluginInstance, spec: &ParameterSpec, value: f64, actions: &mut Vec<Action>) {
    actions.push(Action::SetVst3Parameter {
        id: plugin.id,
        parameter: spec.id,
        value: spec.normalize(value),
    });
}

fn display(spec: &ParameterSpec, value: f64) -> String {
    match spec.unit {
        "Hz" if value >= 1000.0 => format!("{:.2} kHz", value / 1000.0),
        "Hz" if value < 10.0 => format!("{value:.2} Hz"),
        "Hz" => format!("{value:.0} Hz"),
        "s" if value < 0.01 => format!("{:.2} ms", value * 1000.0),
        "s" if value < 1.0 => format!("{:.0} ms", value * 1000.0),
        "s" => format!("{value:.2} s"),
        "dB" => format!("{value:.1} dB"),
        "%" => format!("{value:.0} %"),
        "ms" => format!("{value:.1} ms"),
        _ if spec.steps > 0 => format!("{value:.0} {}", spec.unit),
        _ => format!("{value:.2} {}", spec.unit),
    }
}

fn parse_value(spec: &ParameterSpec, text: &str) -> Option<f64> {
    let text = text.trim();
    let numeric_end = text
        .find(|c: char| c.is_alphabetic() && c != 'e' && c != 'E' || c == '%')
        .unwrap_or(text.len());
    let number = text[..numeric_end].trim().parse::<f64>().ok()?;
    let unit = text[numeric_end..].trim().to_ascii_lowercase();
    let multiplier = match (spec.unit, unit.as_str()) {
        ("Hz", "khz") => 1000.0,
        ("s", "ms") => 0.001,
        ("ms", "s") => 1000.0,
        (_, "") => 1.0,
        (expected, actual) if expected.to_ascii_lowercase() == actual => 1.0,
        _ => return None,
    };
    let value = number * multiplier;
    value.is_finite().then_some(value)
}

#[path = "builtin_inline.rs"]
mod inline;

/// Seconds after the last spectrum refresh before the display starts to fade, and how long
/// the fade takes.
const SPECTRUM_HOLD: f64 = 0.25;
const SPECTRUM_FADE: f64 = 0.5;

/// A telemetry spectrum ready to draw. Devices keep their spectra through resets (the engine
/// resets them at every loop wrap), so when refreshes stop because playback stopped the
/// display fades to `floor` instead of freezing, like an analyzer hearing silence.
fn display_spectrum(
    ui: &Ui,
    id: Id,
    telemetry: &Telemetry,
    bands: &[std::sync::atomic::AtomicU32; 128],
    floor: f32,
) -> [f32; 128] {
    let now = ui.input(|input| input.time);
    let updates = telemetry.spectrum_updates.load(Ordering::Relaxed);
    let key = id.with("spectrum-age");
    // A spectrum never seen refreshing is treated as long stale.
    let (seen, since) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(u32, f64)>(key))
        .unwrap_or((updates, f64::NEG_INFINITY));
    let since = if updates == seen { since } else { now };
    ui.ctx()
        .data_mut(|data| data.insert_temp(key, (updates, since)));
    let fade = (((now - since) - SPECTRUM_HOLD) / SPECTRUM_FADE).clamp(0.0, 1.0) as f32;
    std::array::from_fn(|band| {
        let db = f32::from_bits(bands[band].load(Ordering::Relaxed)).max(floor);
        db + (floor - db) * fade
    })
}

/// Whether the device has a full faceplate that opens in its own window; the others show
/// all their controls on the rack card.
pub fn has_panel(kind: BuiltinKind) -> bool {
    kind == BuiltinKind::Ferro
}

/// Whether the device has a before/after scope window.
pub fn has_scope(kind: BuiltinKind) -> bool {
    kind == BuiltinKind::VariMu
}

pub fn scope(
    ui: &mut Ui,
    plugin: &PluginInstance,
    kind: BuiltinKind,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    if !has_scope(kind) {
        return;
    }
    ui.push_id(("builtin-scope", plugin.id), |ui| {
        inline::scope(ui, plugin, telemetry, actions)
    });
    ui.ctx().request_repaint_after(Duration::from_millis(16));
}

pub fn card_width(kind: BuiltinKind) -> f32 {
    match kind {
        BuiltinKind::Oscil => 428.0,
        BuiltinKind::Sampler => 424.0,
        BuiltinKind::Eq => 460.0,
        BuiltinKind::Comp => 372.0,
        BuiltinKind::VariMu => 448.0,
        BuiltinKind::Rain => 428.0,
        BuiltinKind::Ferro => 448.0,
    }
}

pub fn compact(
    ui: &mut Ui,
    plugin: &PluginInstance,
    kind: BuiltinKind,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    ui.push_id(("builtin-inline", plugin.id), |ui| {
        inline::show(ui, plugin, kind, telemetry, actions)
    });
}

pub fn show(
    ui: &mut Ui,
    plugin: &PluginInstance,
    kind: BuiltinKind,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    if !matches!(kind, BuiltinKind::VariMu | BuiltinKind::Ferro) {
        let plate = ui.max_rect().expand(8.0).intersect(ui.clip_rect());
        ui.painter()
            .add(rack::faceplate(plate, Some(accent(kind)), false));
    }
    ui.push_id(("builtin-editor", plugin.id), |ui| {
        inline::editor(ui, plugin, kind, telemetry, actions)
    });
    if matches!(
        kind,
        BuiltinKind::Eq | BuiltinKind::Comp | BuiltinKind::VariMu | BuiltinKind::Rain
    ) {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
}

#[derive(Clone)]
struct CachedPreview {
    fingerprint: Option<u64>,
    state_len: usize,
    preview: Result<Arc<SamplePreview>, String>,
}

fn preview(ui: &Ui, plugin: &PluginInstance) -> Result<Arc<SamplePreview>, String> {
    let id = Id::new(("sampler-preview", plugin.id));
    let fingerprint = sample_state_fingerprint(&plugin.state);
    ui.ctx().data_mut(|data| {
        let cached = data.get_temp::<CachedPreview>(id).filter(|cache| {
            fingerprint.is_some()
                && cache.fingerprint == fingerprint
                && cache.state_len == plugin.state.len()
        });
        let cached = cached.unwrap_or_else(|| CachedPreview {
            fingerprint,
            state_len: plugin.state.len(),
            preview: sample_preview(&plugin.state).and_then(|sample| {
                if sample.frames == 0 {
                    Err("No sample loaded".to_owned())
                } else {
                    Ok(Arc::new(sample))
                }
            }),
        });
        let result = cached.preview.clone();
        data.insert_temp(id, cached);
        result
    })
}

fn waveform_in(
    ui: &mut Ui,
    rect: Rect,
    plugin: &PluginInstance,
    specs: &[ParameterSpec],
    loaded: &Result<Arc<SamplePreview>, String>,
    actions: &mut Vec<Action>,
) {
    let color = accent(BuiltinKind::Sampler);
    let screen = rack::screen(ui.painter(), rect);
    let rail = (rect.height() * 0.16).clamp(9.0, 14.0);
    let inner = Rect::from_min_max(screen.min + vec2(8.0, rail), screen.max - vec2(8.0, rail));
    ui.painter()
        .hline(inner.x_range(), inner.center().y, rack::graticule());
    if let Ok(sample) = loaded {
        let looping = builtin::value(plugin, 23).round() == 2.0;
        let start = builtin::value(plugin, 30) as f32;
        let end = builtin::value(plugin, 31) as f32;
        let x_of = |value: f32| inner.left() + value * inner.width();
        for (index, peak) in sample.peaks.iter().enumerate() {
            let position = index as f32 / sample.peaks.len().max(1) as f32;
            let x = x_of(position);
            let inside = (start..=end).contains(&position);
            ui.painter().line_segment(
                [
                    pos2(x, inner.center().y - peak[1] * inner.height() * 0.48),
                    pos2(x, inner.center().y - peak[0] * inner.height() * 0.48),
                ],
                Stroke::new(
                    1.0,
                    if inside {
                        color
                    } else {
                        color.gamma_multiply(0.28)
                    },
                ),
            );
        }
        if looping {
            let from = x_of(builtin::value(plugin, 32) as f32);
            let to = x_of(builtin::value(plugin, 33) as f32);
            ui.painter().rect_filled(
                Rect::from_x_y_ranges(from.min(to)..=from.max(to), inner.y_range()),
                0.0,
                theme::ACCENT.gamma_multiply(0.08),
            );
        }
        for (id, label, marker_color) in [
            (30, "S", color),
            (31, "E", color),
            (32, "L", theme::ACCENT),
            (33, "R", theme::ACCENT),
        ] {
            let loop_handle = id >= 32;
            if loop_handle && !looping {
                continue;
            }
            let x = x_of(builtin::value(plugin, id) as f32);
            let flag = if loop_handle {
                Rect::from_min_max(
                    pos2(x - 5.0, inner.bottom()),
                    pos2(x + 5.0, screen.bottom()),
                )
            } else {
                Rect::from_min_max(pos2(x - 5.0, screen.top()), pos2(x + 5.0, inner.top()))
            };
            let grab = if loop_handle {
                Rect::from_min_max(
                    pos2(x - 6.0, inner.center().y),
                    pos2(x + 6.0, screen.bottom()),
                )
            } else {
                Rect::from_min_max(pos2(x - 6.0, screen.top()), pos2(x + 6.0, inner.center().y))
            };
            let response = ui.interact(
                grab,
                Id::new(("sampler-marker", plugin.id, id)),
                Sense::drag(),
            );
            let hot = response.hovered() || response.dragged();
            ui.painter().vline(
                x,
                inner.y_range(),
                Stroke::new(if hot { 1.6 } else { 1.0 }, marker_color),
            );
            ui.painter().rect_filled(
                flag,
                1.5,
                marker_color.gamma_multiply(if hot { 1.0 } else { 0.8 }),
            );
            ui.painter().text(
                flag.center(),
                Align2::CENTER_CENTER,
                label,
                theme::prop(8.0),
                theme::TEXT_ON_COLOR,
            );
            if response.dragged()
                && let Some(pointer) = response.interact_pointer_pos()
                && let Some(spec) = specs.iter().find(|spec| spec.id == id)
            {
                set(
                    plugin,
                    spec,
                    f64::from(((pointer.x - inner.left()) / inner.width()).clamp(0.0, 1.0)),
                    actions,
                );
            }
            response
                .on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal)
                .on_hover_text(match id {
                    30 => "Sample start",
                    31 => "Sample end",
                    32 => "Loop start",
                    _ => "Loop end",
                });
        }
    } else {
        rack::print(
            ui.painter(),
            inner.center(),
            Align2::CENTER_CENTER,
            "No sample loaded",
            rack::INK_FAINT,
        );
    }
    rack::screen_glass(ui.painter(), rect);
}

const BAND_COLORS: [Color32; 6] = [
    Color32::from_rgb(245, 130, 130),
    Color32::from_rgb(239, 185, 104),
    Color32::from_rgb(204, 215, 111),
    Color32::from_rgb(114, 211, 171),
    Color32::from_rgb(115, 185, 237),
    Color32::from_rgb(193, 149, 233),
];

fn frequency_x(rect: Rect, hz: f64) -> f32 {
    rect.left() + ((hz.clamp(20.0, 20000.0) / 20.0).ln() / 1000.0_f64.ln()) as f32 * rect.width()
}

fn x_frequency(rect: Rect, x: f32) -> f64 {
    20.0 * 1000.0_f64.powf(f64::from(
        ((x - rect.left()) / rect.width()).clamp(0.0, 1.0),
    ))
}

fn eq_plot(
    ui: &mut Ui,
    plugin: &PluginInstance,
    specs: &[ParameterSpec],
    telemetry: Option<&Telemetry>,
    height: f32,
    mut selected: Option<&mut usize>,
    actions: &mut Vec<Action>,
) {
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let compact = selected.is_some();
    let screen = rack::screen(ui.painter(), outer);
    let graph = if compact {
        Rect::from_min_max(screen.min + vec2(22.0, 6.0), screen.max - vec2(8.0, 13.0))
    } else {
        Rect::from_min_max(screen.min + vec2(30.0, 10.0), screen.max - vec2(30.0, 20.0))
    };
    let label_font = theme::mono(if compact { 7.5 } else { 8.5 });
    let gain_y =
        |gain: f64| graph.center().y - (gain.clamp(-24.0, 24.0) / 48.0) as f32 * graph.height();
    for hz in [
        20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
    ] {
        let x = frequency_x(graph, hz);
        ui.painter().vline(x, graph.y_range(), rack::graticule());
        if compact && matches!(hz as u32, 20 | 20000 | 200 | 2000 | 5000) {
            continue;
        }
        ui.painter().text(
            pos2(x, graph.bottom() + 2.0),
            Align2::CENTER_TOP,
            if hz >= 1000.0 {
                format!("{}k", hz / 1000.0)
            } else {
                format!("{hz:.0}")
            },
            label_font.clone(),
            rack::INK_FAINT,
        );
    }
    for gain in [-24_i32, -12, 0, 12, 24] {
        let y = gain_y(f64::from(gain));
        ui.painter().hline(
            graph.x_range(),
            y,
            if gain == 0 {
                Stroke::new(1.0, Color32::from_white_alpha(34))
            } else {
                rack::graticule()
            },
        );
        if compact && gain.abs() == 24 {
            continue;
        }
        ui.painter().text(
            pos2(graph.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{gain:+}"),
            label_font.clone(),
            rack::INK_FAINT,
        );
    }
    if !compact {
        for level in [-90, -60, -30, 0] {
            let y = graph.bottom() - ((level + 90) as f32 / 90.0) * graph.height();
            ui.painter().text(
                pos2(graph.right() + 4.0, y),
                Align2::LEFT_CENTER,
                level.to_string(),
                label_font.clone(),
                rack::INK_FAINT,
            );
        }
    }
    if let Some(telemetry) = telemetry {
        let spectrum = display_spectrum(
            ui,
            Id::new(("eq-spectrum", plugin.id, compact)),
            telemetry,
            &telemetry.spectrum,
            -120.0,
        );
        let points: Vec<_> = spectrum
            .iter()
            .enumerate()
            .map(|(i, &db)| {
                pos2(
                    graph.left() + i as f32 / 127.0 * graph.width(),
                    graph.bottom() - (db.clamp(-90.0, 0.0) + 90.0) / 90.0 * graph.height(),
                )
            })
            .collect();
        for points in points.windows(2) {
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    pos2(points[0].x, graph.bottom()),
                    points[0],
                    points[1],
                    pos2(points[1].x, graph.bottom()),
                ],
                accent(BuiltinKind::Eq).gamma_multiply(0.12),
                Stroke::NONE,
            ));
        }
        ui.painter().add(egui::Shape::line(
            points,
            Stroke::new(1.0, accent(BuiltinKind::Eq).gamma_multiply(0.5)),
        ));
    }
    let curve = sympoie::builtin::effects::EqResponse::new(plugin);
    let response = (0..=350)
        .map(|i| {
            let hz = 20.0 * 1000.0_f64.powf(f64::from(i) / 350.0);
            pos2(frequency_x(graph, hz), gain_y(curve.db(hz)))
        })
        .collect();
    rack::trace(ui.painter(), response, accent(BuiltinKind::Eq));
    for (band, color) in BAND_COLORS.into_iter().enumerate() {
        let base = band as u32 * 5;
        let enabled = builtin::value(plugin, base) >= 0.5;
        let shape = builtin::value(plugin, base + 1).round() as u32;
        let point = pos2(
            frequency_x(graph, builtin::value(plugin, base + 2)),
            gain_y(if shape < 3 {
                builtin::value(plugin, base + 3)
            } else {
                0.0
            }),
        );
        let response = ui.interact(
            Rect::from_center_size(point, vec2(20.0, 20.0)),
            Id::new((
                if selected.is_some() {
                    "eq-inline-node"
                } else {
                    "eq-band-node"
                },
                plugin.id,
                band,
            )),
            Sense::click_and_drag(),
        );
        let node = if compact { 6.0 } else { 8.0 };
        let hot = response.hovered() || response.dragged();
        if enabled {
            ui.painter().circle_filled(
                point,
                node + 2.5,
                color.gamma_multiply(if hot { 0.3 } else { 0.15 }),
            );
        }
        ui.painter().circle_filled(
            point,
            node,
            if enabled {
                color
            } else {
                Color32::from_gray(0x1c)
            },
        );
        ui.painter().circle_stroke(
            point,
            node,
            Stroke::new(
                1.0,
                if enabled {
                    Color32::from_black_alpha(160)
                } else {
                    color.gamma_multiply(0.7)
                },
            ),
        );
        ui.painter().text(
            point + vec2(0.0, 0.5),
            Align2::CENTER_CENTER,
            (band + 1).to_string(),
            theme::mono(if compact { 7.5 } else { 9.0 }),
            if enabled { theme::TEXT_ON_COLOR } else { color },
        );
        if (response.clicked() || response.dragged())
            && let Some(selected) = selected.as_deref_mut()
        {
            *selected = band;
        }
        if selected
            .as_deref()
            .is_some_and(|selected| *selected == band)
        {
            ui.painter()
                .circle_stroke(point, node + 2.5, Stroke::new(1.0, rack::INK_BRIGHT));
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            if let Some(spec) = specs.iter().find(|spec| spec.id == base + 2) {
                set(plugin, spec, x_frequency(graph, pointer.x), actions);
            }
            if shape < 3
                && let Some(spec) = specs.iter().find(|spec| spec.id == base + 3)
            {
                set(
                    plugin,
                    spec,
                    f64::from((graph.center().y - pointer.y) / graph.height()) * 48.0,
                    actions,
                );
            }
        }
        if response.double_clicked()
            && let Some(spec) = specs.iter().find(|spec| spec.id == base)
        {
            set(plugin, spec, if enabled { 0.0 } else { 1.0 }, actions);
        }
        response.on_hover_text(format!(
            "Band {} · Drag frequency / gain; double-click to enable or disable.",
            band + 1
        ));
    }
    rack::screen_glass(ui.painter(), outer);
}

fn compression_output(input: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = input - threshold;
    if knee > 0.0 && over.abs() < knee / 2.0 {
        input + (1.0 / ratio - 1.0) * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        threshold + over / ratio
    } else {
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_spectrum_fades_to_the_floor_and_returns_on_the_next_refresh() {
        let ctx = egui::Context::default();
        let telemetry = Telemetry::default();
        telemetry.spectrum[10].store((-30.0_f32).to_bits(), Ordering::Relaxed);
        let read = |time: f64| {
            let mut band = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    band =
                        display_spectrum(ui, Id::new("t"), &telemetry, &telemetry.spectrum, -120.0)
                            [10];
                },
            );
            output.textures_delta.clear();
            band
        };
        // Never refreshed while watched: left over from an earlier run, so hidden.
        assert_eq!(read(0.0), -120.0);
        telemetry.spectrum_updates.store(1, Ordering::Relaxed);
        assert_eq!(read(1.0), -30.0);
        assert_eq!(
            read(1.2),
            -30.0,
            "a gap shorter than the hold keeps the display"
        );
        let fading = read(1.5);
        assert!(fading < -30.0 && fading > -120.0, "{fading}");
        assert_eq!(read(2.0), -120.0);
        telemetry.spectrum_updates.store(2, Ordering::Relaxed);
        assert_eq!(read(2.1), -30.0);
    }

    #[test]
    fn eq_frequency_axis_round_trips() {
        let rect = Rect::from_min_size(pos2(43.0, 5.0), vec2(830.0, 200.0));
        for hz in [20.0, 67.0, 440.0, 1000.0, 12400.0, 20000.0] {
            assert!((x_frequency(rect, frequency_x(rect, hz)) / hz - 1.0).abs() < 0.000001);
        }
    }

    #[test]
    fn compression_curve_has_continuous_soft_knee_and_expected_ratio() {
        assert_eq!(compression_output(-40.0, -20.0, 4.0, 6.0), -40.0);
        assert_eq!(compression_output(0.0, -20.0, 4.0, 6.0), -15.0);
        for edge in [-23.0, -17.0] {
            assert!(
                (compression_output(edge - 0.000001, -20.0, 4.0, 6.0)
                    - compression_output(edge + 0.000001, -20.0, 4.0, 6.0))
                .abs()
                    < 0.000003
            );
        }
    }

    #[test]
    fn typed_values_accept_the_displayed_units() {
        let specs = builtin::parameters(BuiltinKind::Oscil);
        let cutoff = specs.iter().find(|s| s.id == 20).unwrap();
        let attack = specs.iter().find(|s| s.id == 10).unwrap();
        assert_eq!(parse_value(cutoff, "2.40 kHz"), Some(2400.0));
        assert_eq!(parse_value(cutoff, "2400"), Some(2400.0));
        assert_eq!(parse_value(attack, "5.00 ms"), Some(0.005));
        assert_eq!(parse_value(attack, "0.5 s"), Some(0.5));
        assert_eq!(parse_value(cutoff, "NaN"), None);
        assert_eq!(parse_value(attack, "10 kHz"), None);
    }

    #[test]
    fn dedicated_editors_fit_minimum_width_without_spurious_edits() {
        for kind in BuiltinKind::ALL {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let size = minimum_editor_size(kind);
            let mut instance = kind.instance();
            instance.id = 1;
            let mut actions = Vec::new();
            let mut width = 0.0;
            for frame in 0..3 {
                let raw = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(size[0], size[1]))),
                    time: Some(frame as f64 / 60.0),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(raw, |ui| {
                    show(
                        ui,
                        &instance,
                        kind,
                        Some(&Telemetry::default()),
                        &mut actions,
                    );
                    width = ui.min_rect().width();
                });
                output.textures_delta.clear();
            }
            assert!(
                width <= size[0] + 1.0,
                "{} editor overflows: {width} > {}",
                name(kind),
                size[0]
            );
            assert!(actions.is_empty());
        }
    }

    #[test]
    fn eq_node_drag_edits_frequency_and_gain_on_the_same_band() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::Eq.instance();
        instance.id = 17;
        let mut actions = Vec::new();
        let mut time = 0.0;
        let mut frame = |events| {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(980.0, 570.0))),
                time: Some(time),
                focused: true,
                events,
                ..Default::default()
            };
            time += 1.0 / 60.0;
            let mut output = ctx.run_ui(raw, |ui| {
                show(ui, &instance, BuiltinKind::Eq, None, &mut actions)
            });
            output.textures_delta.clear();
        };
        frame(vec![]);
        frame(vec![]);
        let start = ctx
            .read_response(Id::new(("eq-band-node", 17_u64, 2_usize)))
            .unwrap()
            .rect
            .center();
        frame(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        frame(vec![egui::Event::PointerMoved(start + vec2(45.0, -25.0))]);
        assert!(actions.iter().any(|a| matches!(a, Action::SetVst3Parameter { id: 17, parameter: 12, value } if *value > 0.5)));
        assert!(actions.iter().any(|a| matches!(a, Action::SetVst3Parameter { id: 17, parameter: 13, value } if *value > 0.5)));
        assert!(actions.iter().all(|a| matches!(
            a,
            Action::SetVst3Parameter {
                id: 17,
                parameter: 12 | 13,
                ..
            }
        )));
    }

    #[test]
    fn sample_preview_updates_when_same_allocation_gets_new_audio() {
        let path =
            std::env::temp_dir().join(format!("sympoie-waveform-cache-{}.wav", std::process::id()));
        let make_state = |value| {
            let file = std::fs::File::create(&path).unwrap();
            let mut writer = sympoie::wav::WavWriter::new(file, 16).unwrap();
            writer.write_frames(&[[value; 2]; 16]).unwrap();
            writer.finish().unwrap();
            sympoie::builtin::instruments::sample_state_from_wav(&path).unwrap()
        };
        let mut instance = BuiltinKind::Sampler.instance();
        instance.id = 22;
        instance.state = make_state(0.25);
        let updated = make_state(0.75);
        std::fs::remove_file(path).unwrap();
        assert_eq!(instance.state.len(), updated.len());
        let ctx = egui::Context::default();
        let mut observed = Vec::new();
        for state in [None, Some(updated)] {
            if let Some(state) = state {
                instance.state.copy_from_slice(&state);
            }
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                observed.push(preview(ui, &instance).unwrap().peaks[0][1]);
            });
            output.textures_delta.clear();
        }
        assert!((observed[0] - 0.25).abs() < 0.001);
        assert!((observed[1] - 0.75).abs() < 0.001);
    }
}
