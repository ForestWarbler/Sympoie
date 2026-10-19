//! Pond display and controls for the rain drum.
//!
//! The screen is a cross-section of a pond: a drizzle whose density follows the Rate
//! control falls through the sky, and every drop the device actually lands — read from its
//! telemetry — splashes on the water line and sends out rings that fade over the Decay
//! time. The notes being held sit under the water as drum heads at their pitch position
//! (low on the left, high on the right, which is also where the drop pans), and the one a
//! drop strikes lights up.

use super::*;
use crate::ui::format::note_name;
use eframe::egui::Shape;
use sympoie::builtin::rain::{
    ATTACK, CHIRP, CUTOFF, DECAY, DETUNE, DIVISION, DIVISIONS, DRIFT, DROP_RATE, FILTER_ENV, GAIN,
    JITTER, NOISE, OCTAVES, ORDER, ORDERS, OVERTONE, PRESETS, RESONANCE, SHAPE, SPACE, SPREAD,
    SYNC, TONE, TRANSPOSE, VARIATION, VELOCITY, key_position,
};

const ARP_ROW: [(u32, &str); 5] = [
    (DROP_RATE, "Rate"),
    (JITTER, "Jitter"),
    (VARIATION, "Vary"),
    (OCTAVES, "Octaves"),
    (SPREAD, "Spread"),
];
const VOICE_ROW: [(u32, &str); 5] = [
    (ATTACK, "Attack"),
    (DECAY, "Decay"),
    (DETUNE, "Detune"),
    (DRIFT, "Drift"),
    (CHIRP, "Chirp"),
];
const COLOR_ROW: [(u32, &str); 5] = [
    (OVERTONE, "Overtone"),
    (NOISE, "Noise"),
    (TRANSPOSE, "Transp"),
    (VELOCITY, "Vel"),
    (GAIN, "Gain"),
];
const FILTER_ROW: [(u32, &str); 3] = [(CUTOFF, "Cutoff"), (RESONANCE, "Res"), (FILTER_ENV, "Env")];
const SPACE_ROW: [(u32, &str); 2] = [(SPACE, "Space"), (TONE, "Tone")];

const PRESET_WIDTH: f32 = 118.0;

/// Preset chooser: shows the matching factory preset, or "Custom" once a control has moved.
fn presets(ui: &mut Ui, deck: &mut Deck, width: f32) {
    let names: Vec<&str> = PRESETS.iter().map(|preset| preset.name).collect();
    let matching = builtin::matching_preset(deck.plugin);
    let mut labels = names.clone();
    if matching.is_none() {
        labels.push("Custom");
    }
    let selected = matching.unwrap_or(names.len());
    let plugin = deck.plugin.id;
    let (response, picked) = rack::chooser(ui, width, &labels, selected, |index| {
        Id::new(("rain-preset-option", plugin, index))
    });
    ui.interact(
        response.rect,
        Id::new(("rain-preset", plugin)),
        Sense::hover(),
    );
    if let Some(preset) = picked
        && preset < names.len()
        && Some(preset) != matching
    {
        deck.actions
            .push(Action::LoadBuiltinPreset { id: plugin, preset });
    }
    response.on_hover_text("Factory presets. Moving any control makes the sound Custom.");
}

fn arp_switches(ui: &mut Ui, deck: &mut Deck) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        deck.choose(ui, ORDER, &ORDERS, 66.0)
            .on_hover_text("Order: which held note each trigger plays");
        deck.switch(ui, SYNC, "Sync");
        deck.choose(ui, DIVISION, &DIVISIONS, 56.0)
            .on_hover_text("Step length when synced to the tempo");
    });
}

pub(super) fn compact(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    let mut page = 0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        page = deck.pages(ui, &["Arp", "Synth", "Filter"]);
        presets(ui, deck, PRESET_WIDTH);
    });
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let height = rack::ROW + GAP + 2.0 * rack::KNOB_CELL.y + GAP;
        let (rect, _) = ui.allocate_exact_size(vec2(170.0, height), Sense::hover());
        pond(ui, rect, "compact", deck.plugin, deck.accent, telemetry);
        divider(ui, height);
        column(ui, 5.0 * rack::KNOB_CELL.x, |ui| match page {
            0 => {
                arp_switches(ui, deck);
                deck.knobs(ui, &ARP_ROW);
                deck.knobs(ui, &COLOR_ROW[3..]);
            }
            1 => {
                deck.choose(ui, SHAPE, &sympoie::builtin::OSC_SHAPES, 74.0)
                    .on_hover_text("Oscillator shape");
                deck.knobs(ui, &VOICE_ROW);
                deck.knobs(ui, &COLOR_ROW[..3]);
            }
            _ => {
                rack::section_label(ui, "Filter & space");
                deck.knobs(ui, &FILTER_ROW);
                deck.knobs(ui, &SPACE_ROW);
            }
        });
    });
}

pub(super) fn editor(ui: &mut Ui, deck: &mut Deck, telemetry: Option<&Telemetry>) {
    let full = 3.0 * SECTION + 2.0 * SECTION_GAP;
    ui.allocate_ui_with_layout(
        vec2(full, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(full);
            ui.spacing_mut().item_spacing.y = GAP;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (label, _) = ui.allocate_exact_size(vec2(40.0, rack::ROW), Sense::hover());
                rack::print(
                    ui.painter(),
                    label.left_center(),
                    Align2::LEFT_CENTER,
                    "Preset",
                    rack::INK_FAINT,
                );
                presets(ui, deck, 150.0);
            });
            let (rect, _) = ui.allocate_exact_size(vec2(full, 200.0), Sense::hover());
            pond(ui, rect, "editor", deck.plugin, deck.accent, telemetry);
        },
    );
    ui.horizontal_top(|ui| {
        section(ui, "Arpeggiator", SECTION, |ui| {
            arp_switches(ui, deck);
            deck.knobs(ui, &ARP_ROW);
        });
        section(ui, "Voice", SECTION, |ui| {
            deck.choose(ui, SHAPE, &sympoie::builtin::OSC_SHAPES, 80.0);
            deck.knobs(ui, &VOICE_ROW);
        });
        section(ui, "Colour", SECTION, |ui| {
            ui.add_space(rack::ROW);
            deck.knobs(ui, &COLOR_ROW[..4]);
        });
    });
    ui.horizontal_top(|ui| {
        section(ui, "Filter", SECTION, |ui| deck.knobs(ui, &FILTER_ROW));
        section(ui, "Space", SECTION, |ui| deck.knobs(ui, &SPACE_ROW));
    });
}

/// Seconds a drizzle streak takes to fall from the top of the sky to the water.
const FALL: f64 = 0.55;
/// Seconds the splash spark and strike flash last.
const SPARK: f64 = 0.16;
const RINGS: usize = 3;
const RING_SPACING: f64 = 0.1;

#[derive(Clone, Copy)]
struct Ripple {
    born: f64,
    position: f32,
    key: u8,
    intensity: f32,
}

#[derive(Clone, Copy)]
struct Streak {
    born: f64,
    position: f32,
    length: f32,
}

#[derive(Clone, Default)]
struct Pond {
    /// Drop-log entries already animated; `None` until the display has seen the log once,
    /// so drops that landed before it opened are not replayed.
    seen: Option<u32>,
    last: Option<f64>,
    ripples: Vec<Ripple>,
    streaks: Vec<Streak>,
    /// Fractional streaks owed to the drizzle at the current rate.
    debt: f64,
    rng: u32,
}

impl Pond {
    fn random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }
}

fn held_keys(telemetry: Option<&Telemetry>) -> Vec<u8> {
    let Some(telemetry) = telemetry else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for (word, mask) in telemetry.rain_held.iter().enumerate() {
        let bits = mask.load(Ordering::Relaxed);
        for bit in 0..64 {
            if bits & (1 << bit) != 0 {
                keys.push((word * 64 + bit) as u8);
            }
        }
    }
    keys
}

fn ellipse(center: egui::Pos2, radii: egui::Vec2) -> Vec<egui::Pos2> {
    (0..40)
        .map(|index| {
            let angle = index as f32 / 40.0 * std::f32::consts::TAU;
            center + vec2(angle.cos() * radii.x, angle.sin() * radii.y)
        })
        .collect()
}

fn pond(
    ui: &mut Ui,
    outer: Rect,
    view: &str,
    plugin: &PluginInstance,
    accent: Color32,
    telemetry: Option<&Telemetry>,
) {
    let painter = ui.painter().clone();
    let screen = rack::screen(&painter, outer);
    let now = ui.input(|input| input.time);
    let key = Id::new(("rain-pond", plugin.id, view));
    let mut pond = ui
        .ctx()
        .data_mut(|data| data.get_temp::<Pond>(key))
        .unwrap_or_default();
    let dt = pond.last.map_or(0.0, |last| (now - last).clamp(0.0, 0.1));
    pond.last = Some(now);
    let rate = builtin::value(plugin, DROP_RATE);
    let life = builtin::value(plugin, DECAY).clamp(0.3, 2.5);
    let held = held_keys(telemetry);

    if let Some(telemetry) = telemetry {
        let written = telemetry.rain_drops.written();
        if let Some(seen) = pond.seen {
            for drop in telemetry.rain_drops.since(seen) {
                pond.ripples.push(Ripple {
                    born: now,
                    position: drop.position,
                    key: drop.key,
                    intensity: drop.intensity.clamp(0.2, 1.0),
                });
            }
        }
        pond.seen = Some(written);
    }
    pond.debt = (pond.debt + rate.min(40.0) * dt).min(8.0);
    while pond.debt >= 1.0 {
        pond.debt -= 1.0;
        let position = pond.random();
        let length = 0.6 + pond.random() * 0.4;
        pond.streaks.push(Streak {
            born: now,
            position,
            length,
        });
    }
    pond.streaks.retain(|streak| now - streak.born < FALL);
    pond.ripples
        .retain(|ripple| now - ripple.born < life + RINGS as f64 * RING_SPACING);

    let inset = 10.0;
    let x_of = |position: f32| screen.left() + inset + position * (screen.width() - 2.0 * inset);
    let water_y = (screen.top() + screen.height() * 0.6).round() + 0.5;
    let scale = (screen.width() / 170.0).clamp(1.0, 2.5);

    // Water.
    painter.rect_filled(
        Rect::from_min_max(pos2(screen.left(), water_y), screen.max),
        0.0,
        accent.gamma_multiply(0.07),
    );
    painter.hline(
        screen.x_range(),
        water_y,
        Stroke::new(1.0, accent.gamma_multiply(0.55)),
    );
    painter.hline(
        screen.x_range(),
        water_y + 1.0,
        Stroke::new(1.0, accent.gamma_multiply(0.12)),
    );

    // Drizzle: cosmetic, its density follows the Rate control.
    let drizzle = if held.is_empty() { 0.35 } else { 0.6 };
    for streak in &pond.streaks {
        let progress = ((now - streak.born) / FALL) as f32;
        let x = x_of(streak.position).round() + 0.5;
        let y = screen.top() + 2.0 + progress * (water_y - screen.top() - 2.0);
        let length = (6.0 + 8.0 * streak.length) * scale.sqrt();
        painter.line_segment(
            [pos2(x, (y - length).max(screen.top() + 1.0)), pos2(x, y)],
            Stroke::new(
                1.0,
                rack::INK.gamma_multiply(drizzle * (0.3 + 0.7 * progress)),
            ),
        );
    }

    // Landed drops: a spark at impact and rings spreading over the decay time.
    for ripple in &pond.ripples {
        let age = now - ripple.born;
        let center = pos2(x_of(ripple.position), water_y);
        for ring in 0..RINGS {
            let ring_age = age - ring as f64 * RING_SPACING;
            if ring_age < 0.0 {
                continue;
            }
            let fade = (1.0 - ring_age / life).clamp(0.0, 1.0) as f32;
            if fade <= 0.0 {
                continue;
            }
            let radius = (3.0 + ring_age as f32 * 34.0) * scale * (0.6 + 0.4 * ripple.intensity);
            let alpha = fade * ripple.intensity * (0.85 - ring as f32 * 0.22);
            painter.add(Shape::closed_line(
                ellipse(center, vec2(radius, radius * 0.28)),
                Stroke::new(1.0, accent.gamma_multiply(alpha)),
            ));
        }
        if age < SPARK {
            let burst = 1.0 - (age / SPARK) as f32;
            let height = (4.0 + 14.0 * ripple.intensity) * scale * burst;
            painter.line_segment(
                [center, center - vec2(0.0, height)],
                Stroke::new(1.5, accent.lerp_to_gamma(Color32::WHITE, 0.4 * burst)),
            );
            painter.circle_filled(center, 2.0 + 1.5 * burst, accent.gamma_multiply(burst));
        }
    }

    // Drum heads for the held notes, each at its pitch position.
    let head_y = water_y + 7.0;
    let head = vec2(3.5, 6.0) * scale.sqrt();
    for &key in &held {
        let x = x_of(key_position(key) as f32);
        let flash = pond
            .ripples
            .iter()
            .filter(|ripple| ripple.key == key)
            .map(|ripple| (1.0 - ((now - ripple.born) / SPARK) as f32).clamp(0.0, 1.0))
            .fold(0.0_f32, f32::max);
        let color = accent
            .gamma_multiply(0.55 + 0.45 * flash)
            .lerp_to_gamma(Color32::WHITE, 0.5 * flash);
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x, head_y),
                pos2(x + head.x, head_y + head.y),
                pos2(x - head.x, head_y + head.y),
            ],
            color,
            Stroke::NONE,
        ));
        painter.vline(
            x,
            (water_y + 1.0)..=head_y,
            Stroke::new(1.0, accent.gamma_multiply(0.35 + 0.5 * flash)),
        );
    }

    // Legend: the drop rate, and the chord the rain is playing.
    rack::print(
        &painter,
        screen.left_top() + vec2(5.0, 4.0),
        Align2::LEFT_TOP,
        &format!("{rate:.1} drops/s"),
        rack::INK_FAINT,
    );
    let legend = if held.is_empty() {
        "Hold notes to strike".to_owned()
    } else {
        let mut names: Vec<_> = held.iter().take(8).map(|&key| note_name(key)).collect();
        if held.len() > 8 {
            names.push(format!("+{}", held.len() - 8));
        }
        names.join(" ")
    };
    widgets::fitted_text(
        &painter,
        Rect::from_min_max(
            pos2(screen.left() + 5.0, screen.bottom() - 13.0),
            pos2(screen.right() - 5.0, screen.bottom() - 3.0),
        ),
        &legend,
        rack::value_font(),
        if held.is_empty() {
            rack::INK_FAINT
        } else {
            rack::INK_BRIGHT
        },
        Align2::CENTER_CENTER,
    );
    rack::screen_glass(&painter, outer);
    ui.ctx().data_mut(|data| data.insert_temp(key, pond));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use sympoie::builtin::rain::RainDrop;

    fn frame(ctx: &egui::Context, time: f64, instance: &PluginInstance, telemetry: &Telemetry) {
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(700.0, 460.0))),
                time: Some(time),
                ..Default::default()
            },
            |ui| {
                super::super::super::show(
                    ui,
                    instance,
                    BuiltinKind::Rain,
                    Some(telemetry),
                    &mut actions,
                )
            },
        );
        output.textures_delta.clear();
        assert!(actions.is_empty());
    }

    #[test]
    fn pond_animates_only_drops_landed_while_it_was_watching() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut instance = BuiltinKind::Rain.instance();
        instance.id = 9;
        let telemetry = Telemetry::default();
        for _ in 0..5 {
            telemetry.rain_drops.push(RainDrop {
                position: 0.5,
                key: 60,
                intensity: 1.0,
            });
        }
        frame(&ctx, 0.0, &instance, &telemetry);
        let key = Id::new(("rain-pond", 9_u64, "editor"));
        let pond = ctx.data_mut(|data| data.get_temp::<Pond>(key)).unwrap();
        assert!(pond.ripples.is_empty(), "earlier drops are not replayed");
        assert_eq!(pond.seen, Some(5));
        telemetry.rain_drops.push(RainDrop {
            position: 0.2,
            key: 64,
            intensity: 0.8,
        });
        frame(&ctx, 0.05, &instance, &telemetry);
        let pond = ctx.data_mut(|data| data.get_temp::<Pond>(key)).unwrap();
        assert_eq!(pond.ripples.len(), 1);
        assert_eq!(pond.ripples[0].key, 64);
        for tick in 0..6 {
            frame(&ctx, 10.0 + f64::from(tick) * 0.05, &instance, &telemetry);
        }
        let pond = ctx.data_mut(|data| data.get_temp::<Pond>(key)).unwrap();
        assert!(pond.ripples.is_empty(), "rings fade after the decay time");
        assert!(!pond.streaks.is_empty(), "the drizzle keeps falling");
    }

    #[test]
    fn held_keys_decode_both_halves_of_the_mask() {
        let telemetry = Telemetry::default();
        telemetry.rain_held[0].store(1 << 60, Ordering::Relaxed);
        telemetry.rain_held[1].store((1 << 0) | (1 << 3), Ordering::Relaxed);
        assert_eq!(held_keys(Some(&telemetry)), vec![60, 64, 67]);
        assert!(held_keys(None).is_empty());
        let _ = AtomicU64::new(0);
    }
}
