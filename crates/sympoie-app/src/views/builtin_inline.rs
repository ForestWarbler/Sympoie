use super::*;
use crate::ui::rack;

#[cfg(test)]
const BODY_HEIGHT: f32 = 172.0;

const GAP: f32 = 4.0;

#[path = "builtin_calor.rs"]
mod calor;
#[path = "builtin_ferro.rs"]
mod ferro;
#[path = "builtin_rain.rs"]
mod rain;
#[path = "builtin_reson.rs"]
mod reson;
#[path = "builtin_slicer.rs"]
mod slicer;
#[path = "builtin_varimu.rs"]
mod varimu;

pub(super) fn scope(
    ui: &mut Ui,
    plugin: &PluginInstance,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    varimu::scope(ui, plugin, telemetry, actions);
}

pub(super) fn show(
    ui: &mut Ui,
    plugin: &PluginInstance,
    kind: BuiltinKind,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    let specs = builtin::parameters(kind);
    ui.spacing_mut().item_spacing = vec2(0.0, GAP);
    let mut deck = Deck {
        plugin,
        specs: &specs,
        accent: accent(kind),
        actions,
    };
    match kind {
        BuiltinKind::Oscil => oscil(ui, &mut deck),
        BuiltinKind::Sampler => sampler(ui, &mut deck),
        BuiltinKind::Slicer => slicer::compact(ui, &mut deck),
        BuiltinKind::Eq => eq(ui, &mut deck, telemetry),
        BuiltinKind::Comp => comp(ui, &mut deck, telemetry),
        BuiltinKind::VariMu => varimu::compact(ui, &mut deck, telemetry),
        BuiltinKind::Rain => rain::compact(ui, &mut deck, telemetry),
        BuiltinKind::Ferro => ferro::compact(ui, &mut deck, telemetry),
        BuiltinKind::Reson => reson::compact(ui, &mut deck, telemetry),
        BuiltinKind::Calor => calor::compact(ui, &mut deck, telemetry),
    }
    if matches!(
        kind,
        BuiltinKind::Eq
            | BuiltinKind::Comp
            | BuiltinKind::VariMu
            | BuiltinKind::Rain
            | BuiltinKind::Reson
            | BuiltinKind::Calor
    ) {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
}

struct Deck<'a> {
    plugin: &'a PluginInstance,
    specs: &'a [ParameterSpec],
    accent: Color32,
    actions: &'a mut Vec<Action>,
}

impl Deck<'_> {
    fn spec(&self, id: u32) -> Option<&ParameterSpec> {
        self.specs.iter().find(|spec| spec.id == id)
    }

    fn value(&self, id: u32) -> f64 {
        builtin::value(self.plugin, id)
    }

    fn param_id(&self, id: u32) -> Id {
        Id::new(("builtin-inline-param", self.plugin.id, id))
    }

    fn set(&mut self, id: u32, value: f64) {
        if let Some(spec) = self.specs.iter().find(|spec| spec.id == id) {
            set(self.plugin, spec, value, self.actions);
        }
    }

    fn knob(&mut self, ui: &mut Ui, id: u32, label: &str) {
        let Some(spec) = self.spec(id).cloned() else {
            return;
        };
        let value = self.value(id);
        let mut normalized = spec.normalize(value) as f32;
        let bipolar = spec.min < 0.0 && (spec.min + spec.max).abs() < 1e-9;
        let response = rack::knob(
            ui,
            &mut normalized,
            bipolar,
            label,
            &compact_value(&spec, value),
            self.accent,
        );
        ui.interact(response.rect, self.param_id(id), Sense::hover());
        if response.double_clicked() {
            self.set(id, spec.default);
        } else if response.changed() {
            self.set(id, spec.denormalize(f64::from(normalized)));
        }
        self.entry(&response, &spec, value);
        response.on_hover_text(format!(
            "{}: {}. Drag; Shift for fine control; double-click to reset; right-click to type a value.",
            spec.name,
            display(&spec, value)
        ));
    }

    fn entry(&mut self, response: &egui::Response, spec: &ParameterSpec, value: f64) {
        let text_id = self.param_id(spec.id).with("entry");
        if response.secondary_clicked() {
            let text = display(spec, value);
            response
                .ctx
                .data_mut(|data| data.insert_temp(text_id, text));
        }
        let mut commit = None;
        egui::Popup::context_menu(response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(RichText::new(spec.name).small().color(rack::INK));
                let mut text = ui
                    .ctx()
                    .data_mut(|data| data.get_temp::<String>(text_id))
                    .unwrap_or_default();
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(110.0)
                        .font(theme::mono(11.0)),
                );
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                let parsed = parse_value(spec, &text);
                if edit.lost_focus() {
                    if ui.input(|input| input.key_pressed(egui::Key::Enter))
                        && let Some(parsed) = parsed
                    {
                        commit = Some(parsed);
                    }
                    ui.close();
                } else if text.trim().is_empty() || parsed.is_some() {
                    ui.label(
                        RichText::new("Enter to apply · Esc to cancel")
                            .small()
                            .color(rack::INK_FAINT),
                    );
                } else {
                    ui.label(
                        RichText::new("Not a valid value")
                            .small()
                            .color(theme::WARN),
                    );
                }
                ui.ctx().data_mut(|data| data.insert_temp(text_id, text));
            });
        if let Some(value) = commit {
            self.set(spec.id, value.clamp(spec.min, spec.max));
        }
    }

    fn knobs(&mut self, ui: &mut Ui, parameters: &[(u32, &str)]) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for &(id, label) in parameters {
                self.knob(ui, id, label);
            }
        });
    }

    fn choose(&mut self, ui: &mut Ui, id: u32, choices: &[&str], width: f32) -> egui::Response {
        let selected = self.value(id).round() as usize;
        let plugin = self.plugin.id;
        let (response, picked) = rack::chooser(ui, width, choices, selected, |value| {
            Id::new(("builtin-inline-option", plugin, id, value))
        });
        ui.interact(response.rect, self.param_id(id), Sense::hover());
        if let Some(value) = picked
            && value != selected
        {
            self.set(id, value as f64);
        }
        response
    }

    fn switch(&mut self, ui: &mut Ui, id: u32, label: &str) {
        let on = self.value(id) >= 0.5;
        let response = rack::led_switch(ui, label, on, self.accent);
        ui.interact(response.rect, self.param_id(id), Sense::hover());
        if response.clicked() {
            self.set(id, if on { 0.0 } else { 1.0 });
        }
        if let Some(spec) = self.spec(id) {
            response.on_hover_text(spec.name);
        }
    }

    fn pages(&self, ui: &mut Ui, labels: &[&str]) -> usize {
        let id = Id::new(("builtin-inline-page", self.plugin.id));
        let selected = ui
            .ctx()
            .data_mut(|data| data.get_temp::<usize>(id).unwrap_or(0))
            .min(labels.len() - 1);
        let plugin = self.plugin.id;
        let accent = self.accent;
        let selected = rack::selector_bank(
            ui,
            labels,
            selected,
            |_| accent,
            |index| Id::new(("builtin-inline-tab", plugin, index)),
        );
        ui.ctx().data_mut(|data| data.insert_temp(id, selected));
        selected
    }
}

fn compact_value(spec: &ParameterSpec, value: f64) -> String {
    match spec.unit {
        "Hz" if value >= 1000.0 => format!("{:.1}kHz", value / 1000.0),
        "Hz" if value < 1.0 => format!("{value:.2}Hz"),
        "Hz" if value < 10.0 => format!("{value:.1}Hz"),
        "Hz" => format!("{value:.0}Hz"),
        "s" if value < 0.001 => format!("{:.1}ms", value * 1000.0),
        "s" if value < 1.0 => format!("{:.0}ms", value * 1000.0),
        "s" => format!("{value:.1}s"),
        "ms" if value < 1.0 => format!("{value:.2}ms"),
        "ms" => format!("{value:.0}ms"),
        "dB" => format!("{value:.1}dB"),
        "cent" => format!("{value:.0}ct"),
        "st" => format!("{value:.0}st"),
        // A range of less than an octave needs the finer steps.
        "oct" if spec.max < 1.0 => format!("{value:.2}oct"),
        "oct" => format!("{value:.1}oct"),
        "MIDI" => format!("{value:.0}"),
        "%" => format!("{value:.0}%"),
        "" if spec.min >= 0.0 && spec.max <= 1.0 => format!("{:.0}%", value * 100.0),
        _ => format!("{value:.2}{}", spec.unit),
    }
}

fn column<R>(ui: &mut Ui, width: f32, contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            contents(ui)
        },
    )
    .inner
}

fn divider(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, height), Sense::hover());
    rack::rule_vertical(
        ui.painter(),
        rect.center().x - 0.5,
        rect.top()..=rect.bottom(),
    );
}

const OSC_LEFT: f32 = 202.0;
const OSC_RIGHT: f32 = 200.0;

fn oscil(ui: &mut Ui, deck: &mut Deck) {
    let selected = deck.pages(
        ui,
        &["Osc 1", "Osc 2", "Osc 3", "Mod env", "LFOs", "Matrix"],
    );
    if selected == 5 {
        matrix(ui, deck);
        return;
    }
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        column(ui, OSC_LEFT, |ui| match selected {
            0..=2 => {
                let base = 100 + selected as u32 * 10;
                deck.choose(ui, base, &sympoie::builtin::OSCIL_SHAPES, 74.0);
                let (rect, _) = ui.allocate_exact_size(vec2(OSC_LEFT, 38.0), Sense::hover());
                let wave = (deck.value(base).round() as u32, deck.value(base + 5));
                let silent = deck.value(base + 3) <= -59.9;
                wave_screen(
                    ui,
                    rect,
                    wave.0,
                    wave.1,
                    if silent { rack::INK_FAINT } else { deck.accent },
                );
                deck.knobs(
                    ui,
                    &[
                        (base + 1, "Coarse"),
                        (base + 2, "Fine"),
                        (base + 3, "Level"),
                        (base + 4, "Pan"),
                        (base + 5, "Width"),
                    ],
                );
            }
            3 => {
                rack::section_label(ui, "Mod envelope");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (id, label) in [
                        (30, "Attack"),
                        (31, "Decay"),
                        (32, "Sustain"),
                        (33, "Release"),
                    ] {
                        deck.knob(ui, id, label);
                    }
                    let (rect, _) = ui.allocate_exact_size(
                        vec2(OSC_LEFT - 160.0, rack::KNOB_CELL.y),
                        Sense::hover(),
                    );
                    envelope_screen(
                        ui,
                        Rect::from_min_max(rect.min + vec2(3.0, 10.0), rect.max - vec2(0.0, 6.0)),
                        deck.plugin,
                        30,
                        deck.accent,
                    );
                });
                rack::section_label(ui, "Env → filter");
                deck.knobs(ui, &[(23, "Amount"), (24, "Key trk"), (25, "Drive")]);
            }
            _ => {
                lfo_block(ui, deck, 0, OSC_LEFT);
                lfo_block(ui, deck, 1, OSC_LEFT);
            }
        });
        divider(ui, 128.0);
        column(ui, OSC_RIGHT, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (label, _) = ui.allocate_exact_size(vec2(36.0, rack::ROW), Sense::hover());
                rack::print(
                    ui.painter(),
                    label.left_center(),
                    Align2::LEFT_CENTER,
                    "Filter",
                    rack::INK_FAINT,
                );
                deck.choose(ui, 22, &sympoie::builtin::FILTER_TYPES, 82.0);
            });
            deck.knobs(
                ui,
                &[
                    (20, "Cutoff"),
                    (21, "Res"),
                    (0, "Gain"),
                    (1, "Pan"),
                    (2, "Vel"),
                ],
            );
            rack::section_label(ui, "Amp envelope");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (id, label) in [
                    (10, "Attack"),
                    (11, "Decay"),
                    (12, "Sustain"),
                    (13, "Release"),
                ] {
                    deck.knob(ui, id, label);
                }
                let (rect, _) = ui.allocate_exact_size(
                    vec2(OSC_RIGHT - 160.0, rack::KNOB_CELL.y),
                    Sense::hover(),
                );
                envelope_screen(
                    ui,
                    Rect::from_min_max(rect.min + vec2(3.0, 10.0), rect.max - vec2(0.0, 6.0)),
                    deck.plugin,
                    10,
                    deck.accent,
                );
            });
        });
    });
}

/// One LFO: a label, shape and retrigger on top; rate and depths beside a small preview.
fn lfo_block(ui: &mut Ui, deck: &mut Deck, lfo: usize, width: f32) {
    let base = sympoie::builtin::instruments::LFO_BASES[lfo];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (label, _) = ui.allocate_exact_size(vec2(36.0, rack::ROW), Sense::hover());
        rack::print(
            ui.painter(),
            label.left_center(),
            Align2::LEFT_CENTER,
            &format!("LFO {}", lfo + 1),
            rack::INK_FAINT,
        );
        deck.choose(ui, base + 3, &sympoie::builtin::LFO_SHAPES, 74.0);
        deck.switch(ui, base + 4, "Retrig");
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        deck.knobs(
            ui,
            &[(base, "Rate"), (base + 1, "Pitch"), (base + 2, "Filter")],
        );
        let (rect, _) = ui.allocate_exact_size(
            vec2(width - 3.0 * rack::KNOB_CELL.x, rack::KNOB_CELL.y),
            Sense::hover(),
        );
        let silent =
            deck.value(base + 1) == 0.0 && deck.value(base + 2) == 0.0 && !lfo_routed(deck, lfo);
        lfo_screen(
            ui,
            Rect::from_min_max(rect.min + vec2(3.0, 8.0), rect.max - vec2(0.0, 6.0)),
            deck.value(base + 3).round() as u32,
            deck.value(base),
            if silent { rack::INK_FAINT } else { deck.accent },
        );
    });
}

/// Whether any modulation matrix route uses the LFO as its source.
fn lfo_routed(deck: &Deck, lfo: usize) -> bool {
    use sympoie::builtin::instruments::{
        MATRIX_AMOUNT, MATRIX_BASE, MATRIX_ROUTES, MATRIX_SOURCE, MATRIX_STRIDE,
    };
    (0..MATRIX_ROUTES as u32).any(|route| {
        let base = MATRIX_BASE + route * MATRIX_STRIDE;
        deck.value(base + MATRIX_SOURCE).round() as usize == 3 + lfo
            && deck.value(base + MATRIX_AMOUNT) != 0.0
    })
}

fn matrix(ui: &mut Ui, deck: &mut Deck) {
    use sympoie::builtin::instruments::{
        MATRIX_AMOUNT, MATRIX_BASE, MATRIX_ROUTES, MATRIX_SOURCE, MATRIX_SOURCE_LABELS,
        MATRIX_STRIDE, MATRIX_TARGET, MATRIX_TARGET_LABELS, matrix_target_depth,
    };
    const COLUMNS: [(&str, f32); 5] = [
        ("", 14.0),
        ("Source", 84.0),
        ("Target", 128.0),
        ("Amount", 64.0),
        ("Depth", 80.0),
    ];
    let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    let mut x = header.left();
    for (title, width) in COLUMNS {
        rack::print(
            ui.painter(),
            pos2(x, header.center().y),
            Align2::LEFT_CENTER,
            title,
            rack::INK_FAINT,
        );
        x += width + 6.0;
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    for row in 0..MATRIX_ROUTES {
        let base = MATRIX_BASE + row as u32 * MATRIX_STRIDE;
        let source = deck.value(base + MATRIX_SOURCE).round() as u32;
        let target = deck.value(base + MATRIX_TARGET).round() as u32;
        let amount = deck.value(base + MATRIX_AMOUNT);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (slot, _) = ui.allocate_exact_size(vec2(COLUMNS[0].1, rack::ROW), Sense::hover());
            rack::led(ui.painter(), slot.center() - vec2(2.0, 0.0), 2.3, deck.accent, source != 0);
            deck.choose(ui, base + MATRIX_SOURCE, &MATRIX_SOURCE_LABELS, COLUMNS[1].1);
            let target_response =
                deck.choose(ui, base + MATRIX_TARGET, &MATRIX_TARGET_LABELS, COLUMNS[2].1);
            let (depth, unit) = matrix_target_depth(target);
            target_response.on_hover_text(format!(
                "Full depth: {depth} {unit}. Negative amount reverses modulation; LFOs span both directions."
            ));
            ui.push_id(("matrix-amount", deck.plugin.id, row), |ui| {
                crate::ui::widgets::field_visuals(ui.style_mut());
                let mut percent = amount * 100.0;
                let response = ui.add_sized(
                    vec2(COLUMNS[3].1, rack::ROW),
                    egui::DragValue::new(&mut percent)
                        .range(-100.0..=100.0)
                        .speed(0.5)
                        .fixed_decimals(1)
                        .suffix("%"),
                );
                ui.interact(response.rect, deck.param_id(base + MATRIX_AMOUNT), Sense::hover());
                if response.changed() {
                    deck.set(base + MATRIX_AMOUNT, percent / 100.0);
                }
                response.on_hover_text(
                    "Modulation depth. Negative values invert the source. Drag or double-click to type an exact percentage.",
                );
            });
            let depth_text = if source == 0 {
                "—".to_owned()
            } else if source >= 3 {
                format!("±{:.1} {unit}", depth * amount.abs())
            } else {
                format!("{:+.1} {unit}", depth * amount)
            };
            let (rect, response) = ui.allocate_exact_size(vec2(COLUMNS[4].1, rack::ROW), Sense::hover());
            widgets::fitted_text(
                ui.painter(),
                rect,
                &depth_text,
                rack::value_font(),
                if source == 0 { rack::INK_FAINT } else { rack::INK },
                Align2::LEFT_CENTER,
            );
            response.on_hover_text(format!(
                "At 100%: {depth} {unit}. Envelopes range from 0 to 1; LFOs range from −1 to +1. The target stays within its valid range."
            ));
        });
    }
}

fn wave_screen(ui: &Ui, rect: Rect, shape: u32, width: f64, color: Color32) {
    let painter = ui.painter();
    let inner = rack::screen(painter, rect);
    painter.hline(inner.x_range(), inner.center().y, rack::graticule());
    let points = (0..=160)
        .map(|index| {
            let x = f64::from(index) / 160.0;
            let y = sympoie::builtin::instruments::wave_shape(shape, x * 2.0, width);
            pos2(
                inner.left() + 4.0 + x as f32 * (inner.width() - 8.0),
                inner.center().y - y as f32 * (inner.height() / 2.0 - 5.0),
            )
        })
        .collect();
    rack::trace(painter, points, color);
    rack::screen_glass(painter, rect);
}

fn lfo_screen(ui: &Ui, rect: Rect, shape: u32, rate: f64, color: Color32) {
    let painter = ui.painter();
    let inner = rack::screen(painter, rect);
    painter.hline(inner.x_range(), inner.center().y, rack::graticule());
    let cycles = (1.0 + rate.max(0.01).log2() * 0.5).clamp(1.0, 6.0);
    let points = (0..=200)
        .map(|index| {
            let x = f64::from(index) / 200.0;
            let phase = (x * cycles).fract();
            let y = match shape {
                1 => 1.0 - 4.0 * (phase - 0.5).abs(),
                2 => {
                    if phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                _ => (phase * std::f64::consts::TAU).sin(),
            };
            pos2(
                inner.left() + 4.0 + x as f32 * (inner.width() - 8.0),
                inner.center().y - y as f32 * (inner.height() / 2.0 - 5.0),
            )
        })
        .collect();
    rack::trace(painter, points, color);
    rack::screen_glass(painter, rect);
}

fn envelope_screen(ui: &Ui, rect: Rect, plugin: &PluginInstance, base: u32, color: Color32) {
    let painter = ui.painter();
    let inner = rack::screen(painter, rect);
    let times = [
        builtin::value(plugin, base),
        builtin::value(plugin, base + 1),
        builtin::value(plugin, base + 3),
    ]
    .map(|value| value.max(0.0).sqrt() as f32 + 0.08);
    let total = times.iter().sum::<f32>() + 0.5;
    let area = inner.shrink2(vec2(3.0, 4.0));
    let x = |time: f32| area.left() + area.width() * time / total;
    let sustain = area.bottom() - builtin::value(plugin, base + 2) as f32 * area.height();
    rack::trace(
        painter,
        vec![
            area.left_bottom(),
            pos2(x(times[0]), area.top()),
            pos2(x(times[0] + times[1]), sustain),
            pos2(x(total - times[2]), sustain),
            area.right_bottom(),
        ],
        color,
    );
    rack::screen_glass(painter, rect);
}

fn sampler(ui: &mut Ui, deck: &mut Deck) {
    let loaded = preview(ui, deck.plugin);
    let width = ui.available_width();
    sample_bar(ui, deck, &loaded);
    let (plot, _) = ui.allocate_exact_size(vec2(width, 62.0), Sense::hover());
    waveform_in(ui, plot, deck.plugin, deck.specs, &loaded, deck.actions);
    let mut selected = 0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        selected = deck.pages(ui, &["Play", "Loop", "Amp env"]);
        ui.add_space(4.0);
        deck.choose(ui, 23, &sympoie::builtin::PLAYBACK_MODES, 72.0);
        deck.switch(ui, 24, "Reverse");
    });
    let params: &[(u32, &str)] = match selected {
        0 => &[
            (20, "Root"),
            (21, "Semi"),
            (22, "Tune"),
            (30, "Start"),
            (31, "End"),
            (40, "Cutoff"),
            (41, "Res"),
        ],
        1 => &[
            (32, "Loop in"),
            (33, "Loop out"),
            (34, "X-fade"),
            (40, "Cutoff"),
            (41, "Res"),
        ],
        _ => &[
            (10, "Attack"),
            (11, "Decay"),
            (12, "Sustain"),
            (13, "Release"),
            (1, "Pan"),
            (2, "Vel"),
        ],
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for &(id, label) in params {
            deck.knob(ui, id, label);
        }
        let gap = ui.available_width() - rack::KNOB_CELL.x - 9.0;
        if gap > 0.0 {
            ui.add_space(gap);
        }
        divider(ui, rack::KNOB_CELL.y);
        deck.knob(ui, 0, "Gain");
    });
}

fn eq(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    let key = Id::new(("eq-inline-selected", deck.plugin.id));
    let mut band = ui
        .ctx()
        .data_mut(|data| data.get_temp::<usize>(key).unwrap_or(0))
        .min(5);
    eq_plot(
        ui,
        deck.plugin,
        deck.specs,
        telemetry,
        100.0,
        Some(&mut band),
        deck.actions,
    );
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        column(ui, 132.0, |ui| {
            let plugin = deck.plugin.id;
            let labels = ["1", "2", "3", "4", "5", "6"];
            band = rack::selector_bank(
                ui,
                &labels,
                band,
                |index| BAND_COLORS[index],
                |index| Id::new(("eq-inline-select", plugin, index)),
            );
            let base = band as u32 * 5;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                deck.switch(ui, base, "On");
                let width = ui.available_width().min(86.0);
                deck.choose(ui, base + 1, &sympoie::builtin::EQ_BAND_TYPES, width);
            });
            let (rect, _) = ui.allocate_exact_size(vec2(132.0, 9.0), Sense::hover());
            rack::print(
                ui.painter(),
                rect.left_center(),
                Align2::LEFT_CENTER,
                "Drag nodes · dbl-click",
                rack::INK_FAINT,
            );
        });
        ui.add_space(4.0);
        let base = band as u32 * 5;
        let saved = deck.accent;
        deck.accent = BAND_COLORS[band];
        for (id, label) in [(base + 2, "Freq"), (base + 3, "Gain"), (base + 4, "Q")] {
            deck.knob(ui, id, label);
        }
        deck.accent = saved;
        let gap = ui.available_width() - 2.0 * rack::KNOB_CELL.x - 9.0;
        if gap > 0.0 {
            ui.add_space(gap);
        }
        divider(ui, rack::KNOB_CELL.y);
        deck.knob(ui, 30, "Output");
        deck.knob(ui, 31, "Mix");
    });
    ui.ctx().data_mut(|data| data.insert_temp(key, band));
}

fn comp(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    let read = |value: &std::sync::atomic::AtomicU32| f32::from_bits(value.load(Ordering::Relaxed));
    let (input, output, reduction) = telemetry.map_or((-120.0, -120.0, 0.0), |t| {
        (read(&t.input_db), read(&t.output_db), read(&t.reduction_db))
    });
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let side = 2.0 * rack::KNOB_CELL.y + GAP + rack::ROW + GAP;
        let (rect, _) = ui.allocate_exact_size(vec2(side - 6.0, side), Sense::hover());
        comp_curve(ui, rect, deck.plugin, input, deck.accent);
        ui.add_space(6.0);
        column(ui, 5.0 * rack::KNOB_CELL.x, |ui| {
            deck.knobs(
                ui,
                &[
                    (0, "Thresh"),
                    (1, "Ratio"),
                    (2, "Knee"),
                    (3, "Attack"),
                    (4, "Release"),
                ],
            );
            deck.knobs(
                ui,
                &[(5, "Makeup"), (6, "Mix"), (7, "Output"), (8, "Det HP")],
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                deck.choose(ui, 9, &sympoie::builtin::DETECTORS, 56.0)
                    .on_hover_text("Detector: Peak or RMS");
                deck.switch(ui, 10, "Ext sidechain");
            });
        });
        ui.add_space(4.0);
        let (meters, response) = ui.allocate_exact_size(vec2(34.0, side), Sense::hover());
        comp_meters(ui, meters, input, reduction, output, deck.accent);
        response.on_hover_text(format!(
            "In {} · GR −{reduction:.1} dB · Out {}",
            level_text(input),
            level_text(output)
        ));
    });
}

fn level_text(db: f32) -> String {
    if db <= -90.0 {
        "−∞".to_owned()
    } else {
        format!("{db:.1} dB")
    }
}

fn comp_meters(ui: &Ui, rect: Rect, input: f32, reduction: f32, output: f32, accent: Color32) {
    let painter = ui.painter();
    let level = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let signal = |position: f32| {
        if position > 0.9 {
            theme::ERROR
        } else if position > 0.75 {
            theme::WARN
        } else {
            theme::OK
        }
    };
    let ladder_height = rect.height() - 22.0;
    for (index, (label, fraction, hanging)) in [
        ("I", level(input), false),
        ("G", (reduction / 24.0).clamp(0.0, 1.0), true),
        ("O", level(output), false),
    ]
    .into_iter()
    .enumerate()
    {
        let x = rect.left() + index as f32 * 12.0;
        let well = Rect::from_min_size(pos2(x, rect.top() + 10.0), vec2(9.0, ladder_height));
        rack::print(
            painter,
            pos2(well.center().x, rect.top()),
            Align2::CENTER_TOP,
            label,
            rack::INK_FAINT,
        );
        if hanging {
            rack::ladder(painter, well, fraction, true, |_| accent);
        } else {
            rack::ladder(painter, well, fraction, false, signal);
        }
    }
    painter.text(
        pos2(rect.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        if reduction > 0.05 {
            format!("−{reduction:.1}")
        } else {
            "0.0".to_owned()
        },
        rack::value_font(),
        if reduction > 0.05 { accent } else { rack::INK },
    );
}

fn comp_curve(ui: &Ui, outer: Rect, plugin: &PluginInstance, input: f32, color: Color32) {
    let painter = ui.painter();
    let inner = rack::screen(painter, outer);
    let graph = inner.shrink2(vec2(6.0, 6.0));
    let point = |x: f64, y: f64| {
        pos2(
            graph.left() + ((x + 60.0) / 60.0) as f32 * graph.width(),
            graph.bottom() - ((y.clamp(-60.0, 0.0) + 60.0) / 60.0) as f32 * graph.height(),
        )
    };
    for db in [-48, -36, -24, -12] {
        let p = point(f64::from(db), f64::from(db));
        painter.vline(p.x, graph.y_range(), rack::graticule());
        painter.hline(graph.x_range(), p.y, rack::graticule());
    }
    painter.line_segment(
        [graph.left_bottom(), graph.right_top()],
        Stroke::new(1.0, Color32::from_white_alpha(30)),
    );
    let threshold = builtin::value(plugin, 0);
    let ratio = builtin::value(plugin, 1);
    let knee = builtin::value(plugin, 2);
    let t = point(threshold, threshold);
    painter.vline(
        t.x,
        graph.y_range(),
        Stroke::new(1.0, color.gamma_multiply(0.25)),
    );
    let points = (0..=120)
        .map(|i| {
            let db = -60.0 + f64::from(i) / 2.0;
            point(db, compression_output(db, threshold, ratio, knee))
        })
        .collect();
    rack::trace(painter, points, color);
    if input > -60.0 {
        let dot = point(
            f64::from(input.min(0.0)),
            compression_output(f64::from(input), threshold, ratio, knee),
        );
        painter.circle_filled(dot, 4.5, Color32::from_white_alpha(30));
        painter.circle_filled(dot, 2.5, rack::INK_BRIGHT);
    }
    rack::print(
        painter,
        inner.left_top() + vec2(5.0, 4.0),
        Align2::LEFT_TOP,
        "In → out",
        rack::INK_FAINT,
    );
    rack::screen_glass(painter, outer);
}

const SECTION: f32 = 5.0 * 40.0 + 16.0;
const SECTION_GAP: f32 = 8.0;

pub(super) fn editor(
    ui: &mut Ui,
    plugin: &PluginInstance,
    kind: BuiltinKind,
    telemetry: Option<&Telemetry>,
    actions: &mut Vec<Action>,
) {
    let specs = builtin::parameters(kind);
    let mut deck = Deck {
        plugin,
        specs: &specs,
        accent: accent(kind),
        actions,
    };
    if kind == BuiltinKind::VariMu {
        varimu::editor(ui, &mut deck, telemetry);
        return;
    }
    if kind == BuiltinKind::Ferro {
        ferro::editor(ui, &mut deck, telemetry);
        return;
    }
    if kind == BuiltinKind::Reson {
        reson::editor(ui, &mut deck, telemetry);
        return;
    }
    if kind == BuiltinKind::Calor {
        calor::editor(ui, &mut deck, telemetry);
        return;
    }
    ui.spacing_mut().item_spacing = vec2(SECTION_GAP, SECTION_GAP);
    header(ui, &mut deck, kind);
    egui::ScrollArea::both()
        .id_salt(("builtin-editor-scroll", plugin.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(SECTION_GAP, SECTION_GAP);
            match kind {
                BuiltinKind::Oscil => oscil_editor(ui, &mut deck),
                BuiltinKind::Sampler => sampler_editor(ui, &mut deck),
                BuiltinKind::Slicer => slicer::editor(ui, &mut deck),
                BuiltinKind::Eq => eq_editor(ui, &mut deck, telemetry),
                BuiltinKind::Comp => comp_editor(ui, &mut deck, telemetry),
                BuiltinKind::Rain => rain::editor(ui, &mut deck, telemetry),
                BuiltinKind::VariMu
                | BuiltinKind::Ferro
                | BuiltinKind::Reson
                | BuiltinKind::Calor => {
                    unreachable!("the Vari-Mu, Ferro, Reson and Calor draw their own faceplates")
                }
            }
        });
}

fn header(ui: &mut Ui, deck: &mut Deck, kind: BuiltinKind) {
    let plugin = deck.plugin;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.set_min_height(rack::KNOB_CELL.y);
        let power = widgets::activator(ui, !plugin.bypassed);
        if power.clicked() {
            deck.actions.push(Action::SetVst3Bypassed {
                id: plugin.id,
                bypassed: !plugin.bypassed,
            });
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let painter = ui.painter().clone();
            let title = rack::spaced(
                &painter,
                name(kind),
                theme::prop(15.0),
                if plugin.bypassed {
                    rack::INK
                } else {
                    deck.accent
                },
                2.5,
            );
            let (rect, _) = ui.allocate_exact_size(title.size(), Sense::hover());
            painter.galley(rect.min, title, rack::INK_BRIGHT);
            let subtitle = match kind {
                BuiltinKind::Oscil => "Three-oscillator synthesizer",
                BuiltinKind::Sampler => "Polyphonic sample instrument",
                BuiltinKind::Slicer => "Sample slicer, one slice per key",
                BuiltinKind::Eq => "Six-band equalizer",
                BuiltinKind::Comp => "Compressor",
                BuiltinKind::VariMu => "Variable-mu tube compressor",
                BuiltinKind::Rain => "Random arpeggiator with a soft synth",
                BuiltinKind::Ferro => "Analog synthesizer",
                BuiltinKind::Reson => "Resonance suppressor",
                BuiltinKind::Calor => "Harmonic saturator",
            };
            let (rect, _) = ui.allocate_exact_size(vec2(200.0, 10.0), Sense::hover());
            rack::print(
                &painter,
                rect.left_center(),
                Align2::LEFT_CENTER,
                subtitle,
                rack::INK_FAINT,
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let output: &[(u32, &str)] = match kind {
                BuiltinKind::Oscil | BuiltinKind::Sampler | BuiltinKind::Slicer => {
                    &[(0, "Gain"), (1, "Pan"), (2, "Vel")]
                }
                BuiltinKind::Eq => &[(31, "Mix"), (30, "Output")],
                BuiltinKind::Comp => &[(7, "Output"), (6, "Mix"), (5, "Makeup")],
                BuiltinKind::VariMu
                | BuiltinKind::Ferro
                | BuiltinKind::Reson
                | BuiltinKind::Calor => &[],
                BuiltinKind::Rain => &[(0, "Gain")],
            };
            for &(id, label) in output {
                deck.knob(ui, id, label);
            }
            ui.add_space(8.0);
            let reset = rack::led_switch(ui, "Reset", false, deck.accent);
            if reset
                .on_hover_text("Reset every control to its default")
                .clicked()
            {
                deck.actions
                    .push(Action::ResetBuiltinParameters { id: plugin.id });
            }
        });
    });
}

fn section<R>(ui: &mut Ui, title: &str, width: f32, contents: impl FnOnce(&mut Ui) -> R) -> R {
    let slot = ui.painter().add(egui::Shape::Noop);
    let inner = ui.allocate_ui_with_layout(
        vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 6,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.spacing_mut().item_spacing = vec2(0.0, GAP);
                    rack::section_label(ui, title);
                    contents(ui)
                })
                .inner
        },
    );
    let rect = inner.response.rect;
    ui.painter().set(
        slot,
        egui::Shape::Vec(vec![
            egui::Shape::rect_filled(rect, 3.0, Color32::from_black_alpha(46)),
            egui::Shape::rect_stroke(
                rect,
                3.0,
                Stroke::new(1.0, Color32::from_black_alpha(110)),
                egui::StrokeKind::Inside,
            ),
            egui::Shape::line_segment(
                [
                    rect.left_bottom() + vec2(3.0, 0.5),
                    rect.right_bottom() + vec2(-3.0, 0.5),
                ],
                Stroke::new(1.0, Color32::from_white_alpha(14)),
            ),
        ]),
    );
    inner.inner
}

fn envelope_row(ui: &mut Ui, deck: &mut Deck, base: u32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (offset, label) in [(0, "Attack"), (1, "Decay"), (2, "Sustain"), (3, "Release")] {
            deck.knob(ui, base + offset, label);
        }
        let (rect, _) = ui.allocate_exact_size(vec2(40.0, rack::KNOB_CELL.y), Sense::hover());
        envelope_screen(
            ui,
            Rect::from_min_max(rect.min + vec2(3.0, 10.0), rect.max - vec2(0.0, 6.0)),
            deck.plugin,
            base,
            deck.accent,
        );
    });
}

fn oscil_editor(ui: &mut Ui, deck: &mut Deck) {
    let width = SECTION - 16.0;
    ui.horizontal_top(|ui| {
        for index in 0..3 {
            let base = 100 + 10 * index;
            ui.push_id(("oscillator", index), |ui| {
                section(ui, &format!("Oscillator {}", index + 1), SECTION, |ui| {
                    deck.choose(ui, base, &sympoie::builtin::OSCIL_SHAPES, 80.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                    let silent = deck.value(base + 3) <= -59.9;
                    wave_screen(
                        ui,
                        rect,
                        deck.value(base).round() as u32,
                        deck.value(base + 5),
                        if silent { rack::INK_FAINT } else { deck.accent },
                    );
                    deck.knobs(
                        ui,
                        &[
                            (base + 1, "Coarse"),
                            (base + 2, "Fine"),
                            (base + 3, "Level"),
                            (base + 4, "Pan"),
                            (base + 5, "Width"),
                        ],
                    );
                });
            });
        }
    });
    ui.horizontal_top(|ui| {
        section(ui, "Filter", SECTION, |ui| {
            deck.choose(ui, 22, &sympoie::builtin::FILTER_TYPES, 90.0);
            deck.knobs(
                ui,
                &[
                    (20, "Cutoff"),
                    (21, "Res"),
                    (23, "Env amt"),
                    (24, "Key trk"),
                    (25, "Drive"),
                ],
            );
        });
        section(ui, "Filter envelope", SECTION, |ui| {
            envelope_row(ui, deck, 30)
        });
        section(ui, "Amp envelope", SECTION, |ui| envelope_row(ui, deck, 10));
    });
    ui.horizontal_top(|ui| {
        section(ui, "LFOs", SECTION, |ui| {
            lfo_block(ui, deck, 0, width);
            ui.add_space(4.0);
            lfo_block(ui, deck, 1, width);
        });
        section(ui, "Modulation matrix", 2.0 * SECTION + SECTION_GAP, |ui| {
            matrix(ui, deck)
        });
    });
}

fn sampler_editor(ui: &mut Ui, deck: &mut Deck) {
    let loaded = preview(ui, deck.plugin);
    let full = 3.0 * SECTION + 2.0 * SECTION_GAP;
    ui.allocate_ui_with_layout(
        vec2(full, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(full);
            ui.spacing_mut().item_spacing.y = GAP;
            sample_bar(ui, deck, &loaded);
            let (plot, _) = ui.allocate_exact_size(vec2(full, 150.0), Sense::hover());
            waveform_in(ui, plot, deck.plugin, deck.specs, &loaded, deck.actions);
        },
    );
    ui.horizontal_top(|ui| {
        section(ui, "Playback", SECTION, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                deck.choose(ui, 23, &sympoie::builtin::PLAYBACK_MODES, 80.0);
                deck.switch(ui, 24, "Reverse");
            });
            deck.knobs(ui, &[(20, "Root"), (21, "Semi"), (22, "Tune")]);
        });
        section(ui, "Region", SECTION, |ui| {
            deck.knobs(ui, &[(30, "Start"), (31, "End"), (34, "X-fade")]);
            deck.knobs(ui, &[(32, "Loop in"), (33, "Loop out")]);
        });
        section(ui, "Filter & amp", SECTION, |ui| {
            deck.knobs(ui, &[(40, "Cutoff"), (41, "Res")]);
            envelope_row(ui, deck, 10);
        });
    });
}

fn sample_bar(ui: &mut Ui, deck: &mut Deck, loaded: &Result<Arc<SamplePreview>, String>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let response = rack::led_switch(ui, "Load", false, deck.accent);
        ui.interact(
            response.rect,
            Id::new(("sampler-inline-load", deck.plugin.id)),
            Sense::hover(),
        );
        if response
            .on_hover_text("Load a sample (WAV, AIFF, FLAC or MP3)")
            .clicked()
        {
            deck.actions
                .push(Action::LoadSamplerSample { id: deck.plugin.id });
        }
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), rack::ROW), Sense::hover());
        ui.painter().add(crate::ui::analog::readout(rect, false));
        let (name, detail) = match loaded {
            Ok(sample) => (
                sample.name.clone(),
                format!(
                    "{:.2}s · {:.1}k",
                    sample.frames as f64 / f64::from(sample.sample_rate),
                    f64::from(sample.sample_rate) / 1000.0
                ),
            ),
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

fn eq_editor(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    const BAND: f32 = 3.0 * 40.0 + 16.0;
    let full = 6.0 * BAND + 5.0 * SECTION_GAP;
    ui.allocate_ui_with_layout(
        vec2(full, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(full);
            eq_plot(
                ui,
                deck.plugin,
                deck.specs,
                telemetry,
                230.0,
                None,
                deck.actions,
            );
        },
    );
    ui.horizontal_top(|ui| {
        for (band, color) in BAND_COLORS.into_iter().enumerate() {
            let base = band as u32 * 5;
            ui.push_id(("band", band), |ui| {
                section(ui, &format!("Band {}", band + 1), BAND, |ui| {
                    let saved = deck.accent;
                    deck.accent = color;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        deck.switch(ui, base, "On");
                        let width = ui.available_width();
                        deck.choose(ui, base + 1, &sympoie::builtin::EQ_BAND_TYPES, width);
                    });
                    deck.knobs(
                        ui,
                        &[(base + 2, "Freq"), (base + 3, "Gain"), (base + 4, "Q")],
                    );
                    deck.accent = saved;
                });
            });
        }
    });
}

fn comp_editor(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    let read = |value: &std::sync::atomic::AtomicU32| f32::from_bits(value.load(Ordering::Relaxed));
    let (input, output, reduction) = telemetry.map_or((-120.0, -120.0, 0.0), |t| {
        (read(&t.input_db), read(&t.output_db), read(&t.reduction_db))
    });
    ui.horizontal_top(|ui| {
        let side = 214.0;
        let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
        comp_curve(ui, rect, deck.plugin, input, deck.accent);
        let (meters, response) = ui.allocate_exact_size(vec2(36.0, side), Sense::hover());
        comp_meters(ui, meters, input, reduction, output, deck.accent);
        response.on_hover_text(format!(
            "In {} · GR −{reduction:.1} dB · Out {}",
            level_text(input),
            level_text(output)
        ));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SECTION_GAP;
            section(ui, "Dynamics", SECTION, |ui| {
                deck.knobs(
                    ui,
                    &[
                        (0, "Thresh"),
                        (1, "Ratio"),
                        (2, "Knee"),
                        (3, "Attack"),
                        (4, "Release"),
                    ],
                );
            });
            section(ui, "Detector", SECTION, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    deck.choose(ui, 9, &sympoie::builtin::DETECTORS, 56.0);
                    deck.switch(ui, 10, "Ext sidechain");
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    deck.knob(ui, 8, "HP");
                    ui.vertical(|ui| {
                        ui.add_space(8.0);
                        let routed = deck.plugin.sidechain.is_some();
                        let source = rack::led_switch(ui, "Source…", routed, deck.accent);
                        if source
                            .on_hover_text("Choose the track that drives the detector")
                            .clicked()
                        {
                            deck.actions.push(Action::OpenVst3Sidechain(deck.plugin.id));
                        }
                        rack::print(
                            ui.painter(),
                            ui.cursor().left_top() + vec2(0.0, 6.0),
                            Align2::LEFT_TOP,
                            if routed { "Source routed" } else { "No source" },
                            rack::INK_FAINT,
                        );
                    });
                });
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_inline_page_fits_the_fixed_chain_panel() {
        for kind in BuiltinKind::ALL {
            let page_count: usize = match kind {
                BuiltinKind::Oscil => 6,
                BuiltinKind::Sampler => 3,
                BuiltinKind::Slicer => 1,
                BuiltinKind::Eq => 6,
                BuiltinKind::Rain => 3,
                BuiltinKind::Comp
                | BuiltinKind::VariMu
                | BuiltinKind::Ferro
                | BuiltinKind::Reson
                | BuiltinKind::Calor => 1,
            };
            let mut reachable = std::collections::BTreeSet::new();
            for tab in 0..page_count {
                let ctx = egui::Context::default();
                theme::install(&ctx);
                let mut instance = kind.instance();
                instance.id = 71;
                ctx.data_mut(|data| {
                    data.insert_temp(Id::new(("builtin-inline-page", instance.id)), tab);
                    data.insert_temp(Id::new(("eq-inline-selected", instance.id)), tab);
                });
                let mut actions = Vec::new();
                let mut size = vec2(0.0, 0.0);
                for frame in 0..3 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                pos2(0.0, 0.0),
                                vec2(card_width(kind) - 16.0, BODY_HEIGHT),
                            )),
                            time: Some(frame as f64 / 60.0),
                            ..Default::default()
                        },
                        |ui| {
                            compact(ui, &instance, kind, None, &mut actions);
                            size = ui.min_size();
                        },
                    );
                    output.textures_delta.clear();
                }
                assert!(
                    size.x <= card_width(kind) - 16.0 + 1.0,
                    "{} page {tab} too wide: {size:?}",
                    name(kind)
                );
                assert!(
                    size.y <= BODY_HEIGHT,
                    "{} page {tab} too tall: {size:?}",
                    name(kind)
                );
                assert!(actions.is_empty());
                for spec in builtin::parameters(kind) {
                    if let Some(response) =
                        ctx.read_response(Id::new(("builtin-inline-param", instance.id, spec.id)))
                    {
                        assert!(
                            response.rect.bottom() <= BODY_HEIGHT + 1.0,
                            "{} parameter {} falls below card: {:?}",
                            name(kind),
                            spec.id,
                            response.rect
                        );
                        assert!(
                            response.rect.right() <= card_width(kind) - 15.0,
                            "{} parameter {} falls outside card: {:?}",
                            name(kind),
                            spec.id,
                            response.rect
                        );
                        reachable.insert(spec.id);
                    }
                }
            }
            // Devices with a full panel keep only their main controls on the card.
            for spec in builtin::parameters(kind)
                .into_iter()
                .filter(|_| !has_panel(kind))
            {
                assert!(
                    reachable.contains(&spec.id),
                    "{} parameter {} has no inline control",
                    name(kind),
                    spec.name
                );
            }
        }
    }

    #[test]
    fn inline_knob_drag_changes_sound_without_opening_an_editor() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::Oscil.instance();
        instance.id = 31;
        let mut actions = Vec::new();
        let mut time = 0.0;
        let mut frame = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        pos2(0.0, 0.0),
                        vec2(444.0, BODY_HEIGHT),
                    )),
                    focused: true,
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| compact(ui, &instance, BuiltinKind::Oscil, None, &mut actions),
            );
            output.textures_delta.clear();
            time += 1.0 / 60.0;
        };
        frame(vec![]);
        frame(vec![]);
        let point = ctx
            .read_response(Id::new(("builtin-inline-param", instance.id, 20_u32)))
            .unwrap()
            .rect
            .center();
        frame(vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        frame(vec![egui::Event::PointerMoved(point - vec2(0.0, 18.0))]);
        assert!(!actions.is_empty());
        assert!(actions.iter().all(|a| matches!(a, Action::SetVst3Parameter { id: 31, parameter: 20, value } if value.is_finite() && (0.0..=1.0).contains(value))));
    }

    #[test]
    fn eq_band_click_selects_its_inline_controls() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::Eq.instance();
        instance.id = 32;
        let mut actions = Vec::new();
        let mut time = 0.0;
        let mut frame = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        pos2(0.0, 0.0),
                        vec2(444.0, BODY_HEIGHT),
                    )),
                    focused: true,
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| compact(ui, &instance, BuiltinKind::Eq, None, &mut actions),
            );
            output.textures_delta.clear();
            time += 1.0 / 60.0;
        };
        frame(vec![]);
        frame(vec![]);
        let point = ctx
            .read_response(Id::new(("eq-inline-node", instance.id, 4_usize)))
            .unwrap()
            .rect
            .center();
        frame(vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        frame(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        frame(vec![]);
        assert_eq!(
            ctx.data_mut(
                |data| data.get_temp::<usize>(Id::new(("eq-inline-selected", instance.id)))
            ),
            Some(4)
        );
        assert!(
            ctx.read_response(Id::new(("builtin-inline-param", instance.id, 22_u32)))
                .is_some()
        );
        assert!(actions.is_empty());
    }

    struct MatrixHarness {
        ctx: egui::Context,
        instance: PluginInstance,
        actions: Vec<Action>,
        time: f64,
    }

    impl MatrixHarness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut instance = BuiltinKind::Oscil.instance();
            instance.id = 45;
            ctx.data_mut(|data| {
                data.insert_temp(Id::new(("builtin-inline-page", instance.id)), 5_usize)
            });
            let mut harness = Self {
                ctx,
                instance,
                actions: Vec::new(),
                time: 0.0,
            };
            harness.frame(vec![]);
            harness.frame(vec![]);
            harness
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let first_action = self.actions.len();
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(460.0, 600.0))),
                    focused: true,
                    time: Some(self.time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(
                        Rect::from_min_size(pos2(0.0, 0.0), vec2(444.0, BODY_HEIGHT)),
                    ));
                    compact(
                        &mut child,
                        &self.instance,
                        BuiltinKind::Oscil,
                        None,
                        &mut self.actions,
                    );
                },
            );
            output.textures_delta.clear();
            for action in &self.actions[first_action..] {
                if let Action::SetVst3Parameter {
                    parameter, value, ..
                } = action
                {
                    self.instance
                        .parameters
                        .iter_mut()
                        .find(|spec| spec.id == *parameter)
                        .unwrap()
                        .value = *value;
                }
            }
            self.time += 1.0 / 60.0;
        }

        fn click(&mut self, id: Id) {
            let point = self
                .ctx
                .read_response(id)
                .unwrap_or_else(|| panic!("missing control {id:?}"))
                .rect
                .center();
            self.frame(vec![egui::Event::PointerMoved(point)]);
            for pressed in [true, false] {
                self.frame(vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
        }

        fn select(&mut self, parameter: u32, option: usize) {
            self.click(Id::new((
                "builtin-inline-param",
                self.instance.id,
                parameter,
            )));
            self.frame(vec![]);
            self.click(Id::new((
                "builtin-inline-option",
                self.instance.id,
                parameter,
                option,
            )));
            self.frame(vec![]);
        }
    }

    #[test]
    fn matrix_source_and_target_selectors_edit_only_the_chosen_route() {
        use sympoie::builtin::instruments::{MATRIX_BASE, MATRIX_ROUTES, MATRIX_STRIDE};
        let mut harness = MatrixHarness::new();
        let base = MATRIX_BASE + (MATRIX_ROUTES - 1) as u32 * MATRIX_STRIDE;
        harness.select(base, 3);
        harness.select(base + 1, 16);
        assert_eq!(builtin::value(&harness.instance, base), 3.0);
        assert_eq!(builtin::value(&harness.instance, base + 1), 16.0);
        assert_eq!(builtin::value(&harness.instance, MATRIX_BASE), 0.0);
        assert_eq!(harness.actions.len(), 2);
        assert!(harness.actions.iter().all(|action| matches!(action, Action::SetVst3Parameter { id: 45, parameter, .. } if *parameter == base || *parameter == base + 1)));
    }

    #[test]
    fn matrix_amount_accepts_an_exact_negative_percentage() {
        use sympoie::builtin::instruments::{MATRIX_AMOUNT, MATRIX_BASE};
        let mut harness = MatrixHarness::new();
        let id = Id::new((
            "builtin-inline-param",
            harness.instance.id,
            MATRIX_BASE + MATRIX_AMOUNT,
        ));
        harness.click(id);
        harness.click(id);
        let command = egui::Modifiers {
            command: true,
            ..Default::default()
        };
        harness.frame(vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: command,
            },
            egui::Event::Text("-25".to_owned()),
        ]);
        harness.frame(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(
            (builtin::value(&harness.instance, MATRIX_BASE + MATRIX_AMOUNT) + 0.25).abs()
                < 0.000001
        );
        assert!(harness.actions.iter().all(|action| matches!(action, Action::SetVst3Parameter { id: 45, parameter, .. } if *parameter == MATRIX_BASE + MATRIX_AMOUNT)));
    }

    #[test]
    fn dense_readouts_keep_physical_units_and_small_envelope_times() {
        let specs = builtin::parameters(BuiltinKind::Oscil);
        let spec = |id| specs.iter().find(|spec| spec.id == id).unwrap();
        assert_eq!(compact_value(spec(20), 6000.0), "6.0kHz");
        assert_eq!(compact_value(spec(10), 0.0001), "0.1ms");
        assert_eq!(compact_value(spec(0), -12.0), "-12.0dB");
        assert_eq!(compact_value(spec(101), 12.0), "12st");
        assert_eq!(compact_value(spec(102), -7.0), "-7ct");
        assert_eq!(compact_value(spec(12), 0.8), "80%");
    }
}
