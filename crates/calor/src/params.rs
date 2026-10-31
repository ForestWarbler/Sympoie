//! Calor's parameters: one table of ids, names, units, ranges and defaults, shared by
//! every host that runs it.
//!
//! Ids are forever: hosts store automation and saved sets under them, so parameters are
//! only ever added at the end. The first eleven are Calor's first set: the curve and the
//! emphasis of the first band, and the global Tone, Mix, Output and Auto Gain. After them
//! come the band layout, the first band's later controls, the other five bands, and
//! modulation. Every one of them starts where it leaves the sound as the first set made
//! it: one band, no dynamics, no feedback, the plain curve, nothing modulated.

pub const DRIVE: u32 = 0;
pub const HARDNESS: u32 = 1;
pub const BIAS: u32 = 2;
pub const ODD: u32 = 3;
pub const EVEN: u32 = 4;
pub const EMPHASIS: u32 = 5;
pub const EMPHASIS_FREQUENCY: u32 = 6;
pub const TONE: u32 = 7;
pub const MIX: u32 = 8;
pub const OUTPUT: u32 = 9;
pub const AUTO_GAIN: u32 = 10;
/// How many bands the signal is split into, 1 to [`MAX_BANDS`].
pub const BANDS: u32 = 11;
pub const CROSSOVER_SLOPE: u32 = 12;
/// The first of the crossover frequencies, lowest first.
pub const CROSSOVER_BASE: u32 = 13;
/// The first band's controls added with the bands, from [`Slot::Mix`] on.
const FIRST_BAND_LATER: u32 = 18;
/// The first control of the second band; each band after the first has [`BAND_STRIDE`]
/// controls in [`Slot`] order.
pub const BAND_BASE: u32 = 27;
pub const BAND_STRIDE: u32 = 16;
pub const LFO_RATE: u32 = 107;
pub const LFO_SHAPE: u32 = 108;
pub const ENVELOPE_ATTACK: u32 = 109;
pub const ENVELOPE_RELEASE: u32 = 110;
/// The first modulation slot; each has a source, a target, a band and an amount.
pub const MOD_BASE: u32 = 111;
pub const MOD_STRIDE: u32 = 4;
pub const MOD_SOURCE: u32 = 0;
pub const MOD_TARGET: u32 = 1;
pub const MOD_BAND: u32 = 2;
pub const MOD_AMOUNT: u32 = 3;
/// Number of parameters; their ids run from 0 to `COUNT - 1`.
pub const COUNT: usize = 127;

pub const MAX_BANDS: usize = 6;
pub const CROSSOVERS: usize = MAX_BANDS - 1;
pub const MOD_SLOTS: usize = 4;

/// Where each crossover sits until it is moved, in Hz. Two bands split the lows from the
/// rest; more bands add a split higher up each time.
pub const CROSSOVER_DEFAULTS: [f64; CROSSOVERS] = [150.0, 800.0, 3000.0, 7000.0, 13_000.0];

pub const MODELS: [&str; 5] = ["Curve", "Tube", "Tape", "Transformer", "Fold"];
pub const CHANNEL_MODES: [&str; 3] = ["Stereo", "Mid", "Side"];
pub const SLOPES: [&str; 3] = ["12 dB/oct", "24 dB/oct", "48 dB/oct"];
pub const BAND_COUNTS: [&str; MAX_BANDS] = ["1", "2", "3", "4", "5", "6"];
pub const LFO_SHAPES: [&str; 4] = ["Sine", "Triangle", "Square", "Saw"];
pub const MOD_SOURCES: [&str; 3] = ["Off", "LFO", "Envelope"];
pub const MOD_TARGETS: [&str; 13] = [
    "Drive", "Hardness", "Bias", "Odd", "Even", "Emphasis", "Dynamics", "Feedback", "Band Mix",
    "Level", "Mix", "Tone", "Output",
];
pub const MOD_BANDS: [&str; MAX_BANDS + 1] = [
    "All bands",
    "Band 1",
    "Band 2",
    "Band 3",
    "Band 4",
    "Band 5",
    "Band 6",
];

/// One of a band's controls, in the order each band after the first lays them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Drive,
    Hardness,
    Bias,
    Odd,
    Even,
    Emphasis,
    EmphasisFrequency,
    Mix,
    Level,
    Dynamics,
    Feedback,
    FeedbackFrequency,
    Model,
    Channels,
    Solo,
    Mute,
}

impl Slot {
    pub const ALL: [Slot; BAND_STRIDE as usize] = [
        Slot::Drive,
        Slot::Hardness,
        Slot::Bias,
        Slot::Odd,
        Slot::Even,
        Slot::Emphasis,
        Slot::EmphasisFrequency,
        Slot::Mix,
        Slot::Level,
        Slot::Dynamics,
        Slot::Feedback,
        Slot::FeedbackFrequency,
        Slot::Model,
        Slot::Channels,
        Slot::Solo,
        Slot::Mute,
    ];
}

/// The id of one of a band's controls; bands count from 0.
pub const fn band_parameter(band: usize, slot: Slot) -> u32 {
    let slot = slot as u32;
    if band == 0 {
        if slot < Slot::Mix as u32 {
            slot
        } else {
            FIRST_BAND_LATER + slot - Slot::Mix as u32
        }
    } else {
        BAND_BASE + (band as u32 - 1) * BAND_STRIDE + slot
    }
}

/// The band and control an id belongs to, for a band's controls.
pub fn band_slot(id: u32) -> Option<(usize, Slot)> {
    let slot = |index: u32| Slot::ALL[index as usize];
    match id {
        id if id < Slot::Mix as u32 => Some((0, slot(id))),
        id if (FIRST_BAND_LATER..BAND_BASE).contains(&id) => {
            Some((0, slot(id - FIRST_BAND_LATER + Slot::Mix as u32)))
        }
        id if (BAND_BASE..LFO_RATE).contains(&id) => {
            let offset = id - BAND_BASE;
            Some((
                1 + (offset / BAND_STRIDE) as usize,
                slot(offset % BAND_STRIDE),
            ))
        }
        _ => None,
    }
}

/// The id of one of a modulation slot's controls; slots count from 0.
pub const fn mod_parameter(slot: usize, field: u32) -> u32 {
    MOD_BASE + slot as u32 * MOD_STRIDE + field
}

/// How a parameter's range maps onto a control's travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Linear,
    Log,
}

/// One parameter: its id, how it is named and shown, its range and default in physical
/// units, how the range maps onto 0–1, and how many steps it has (0 for continuous).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub id: u32,
    pub name: &'static str,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub scale: Scale,
    pub steps: u32,
}

impl Spec {
    /// The 0–1 position of a physical value. Values outside the range are clamped, and
    /// a value that is no number reads as the default.
    pub fn normalize(&self, physical: f64) -> f64 {
        let physical = if physical.is_finite() {
            physical.clamp(self.min, self.max)
        } else {
            self.default
        };
        let value = match self.scale {
            Scale::Linear => (physical - self.min) / (self.max - self.min),
            Scale::Log => (physical / self.min).ln() / (self.max / self.min).ln(),
        };
        self.quantize(value)
    }

    /// The physical value at a 0–1 position.
    pub fn denormalize(&self, normalized: f64) -> f64 {
        if !normalized.is_finite() {
            return self.default;
        }
        let value = self.quantize(normalized);
        match self.scale {
            Scale::Linear => self.min + value * (self.max - self.min),
            Scale::Log => self.min * (self.max / self.min).powf(value),
        }
    }

    fn quantize(&self, value: f64) -> f64 {
        let value = value.clamp(0.0, 1.0);
        if self.steps == 0 {
            value
        } else {
            (value * self.steps as f64).round() / self.steps as f64
        }
    }
}

const fn parameter(
    id: u32,
    name: &'static str,
    unit: &'static str,
    range: (f64, f64),
    default: f64,
    scale: Scale,
    steps: u32,
) -> Spec {
    Spec {
        id,
        name,
        unit,
        min: range.0,
        max: range.1,
        default,
        scale,
        steps,
    }
}

/// A band's control in `slot` under `name`, with the range and default every band shares.
const fn band_spec(band: usize, slot: Slot, name: &'static str) -> Spec {
    let id = band_parameter(band, slot);
    match slot {
        Slot::Drive => parameter(id, name, "dB", (0.0, 36.0), 6.0, Scale::Linear, 0),
        Slot::Hardness => parameter(id, name, "%", (0.0, 100.0), 30.0, Scale::Linear, 0),
        Slot::Bias => parameter(id, name, "%", (0.0, 100.0), 30.0, Scale::Linear, 0),
        Slot::Odd => parameter(id, name, "%", (0.0, 100.0), 100.0, Scale::Linear, 0),
        Slot::Even => parameter(id, name, "%", (0.0, 300.0), 100.0, Scale::Linear, 0),
        Slot::Emphasis => parameter(id, name, "dB", (-12.0, 12.0), 0.0, Scale::Linear, 0),
        Slot::EmphasisFrequency => {
            parameter(id, name, "Hz", (200.0, 8000.0), 2000.0, Scale::Log, 0)
        }
        Slot::Mix => parameter(id, name, "%", (0.0, 100.0), 100.0, Scale::Linear, 0),
        Slot::Level => parameter(id, name, "dB", (-24.0, 24.0), 0.0, Scale::Linear, 0),
        Slot::Dynamics => parameter(id, name, "%", (-100.0, 100.0), 0.0, Scale::Linear, 0),
        Slot::Feedback => parameter(id, name, "%", (0.0, 100.0), 0.0, Scale::Linear, 0),
        Slot::FeedbackFrequency => {
            parameter(id, name, "Hz", (40.0, 12_000.0), 1000.0, Scale::Log, 0)
        }
        Slot::Model => parameter(id, name, "", (0.0, 4.0), 0.0, Scale::Linear, 4),
        Slot::Channels => parameter(id, name, "", (0.0, 2.0), 0.0, Scale::Linear, 2),
        Slot::Solo | Slot::Mute => parameter(id, name, "", (0.0, 1.0), 0.0, Scale::Linear, 1),
    }
}

/// The names of a band's controls after the first band's, in [`Slot`] order.
macro_rules! band_names {
    ($band:literal) => {
        [
            concat!("Band ", $band, " Drive"),
            concat!("Band ", $band, " Hardness"),
            concat!("Band ", $band, " Bias"),
            concat!("Band ", $band, " Odd"),
            concat!("Band ", $band, " Even"),
            concat!("Band ", $band, " Emphasis"),
            concat!("Band ", $band, " Emphasis Frequency"),
            concat!("Band ", $band, " Mix"),
            concat!("Band ", $band, " Level"),
            concat!("Band ", $band, " Dynamics"),
            concat!("Band ", $band, " Feedback"),
            concat!("Band ", $band, " Feedback Frequency"),
            concat!("Band ", $band, " Model"),
            concat!("Band ", $band, " Channels"),
            concat!("Band ", $band, " Solo"),
            concat!("Band ", $band, " Mute"),
        ]
    };
}

/// The first band keeps the plain names it has always had.
const FIRST_BAND_NAMES: [&str; BAND_STRIDE as usize] = [
    "Drive",
    "Hardness",
    "Bias",
    "Odd",
    "Even",
    "Emphasis",
    "Emphasis Frequency",
    "Band Mix",
    "Level",
    "Dynamics",
    "Feedback",
    "Feedback Frequency",
    "Model",
    "Channels",
    "Solo",
    "Mute",
];
const BAND_NAMES: [[&str; BAND_STRIDE as usize]; MAX_BANDS] = [
    FIRST_BAND_NAMES,
    band_names!(2),
    band_names!(3),
    band_names!(4),
    band_names!(5),
    band_names!(6),
];
const MOD_NAMES: [[&str; MOD_STRIDE as usize]; MOD_SLOTS] = [
    ["Mod 1 Source", "Mod 1 Target", "Mod 1 Band", "Mod 1 Amount"],
    ["Mod 2 Source", "Mod 2 Target", "Mod 2 Band", "Mod 2 Amount"],
    ["Mod 3 Source", "Mod 3 Target", "Mod 3 Band", "Mod 3 Amount"],
    ["Mod 4 Source", "Mod 4 Target", "Mod 4 Band", "Mod 4 Amount"],
];
const CROSSOVER_NAMES: [&str; CROSSOVERS] = [
    "Crossover 1",
    "Crossover 2",
    "Crossover 3",
    "Crossover 4",
    "Crossover 5",
];

const fn specs() -> [Spec; COUNT] {
    let mut result = [parameter(0, "", "", (0.0, 1.0), 0.0, Scale::Linear, 0); COUNT];
    // Every band's controls, the first band's included.
    let mut band = 0;
    while band < MAX_BANDS {
        let mut slot = 0;
        while slot < BAND_STRIDE as usize {
            let spec = band_spec(band, Slot::ALL[slot], BAND_NAMES[band][slot]);
            result[spec.id as usize] = spec;
            slot += 1;
        }
        band += 1;
    }
    result[TONE as usize] = parameter(TONE, "Tone", "dB", (-6.0, 6.0), 0.0, Scale::Linear, 0);
    result[MIX as usize] = parameter(MIX, "Mix", "%", (0.0, 100.0), 100.0, Scale::Linear, 0);
    result[OUTPUT as usize] =
        parameter(OUTPUT, "Output", "dB", (-24.0, 24.0), 0.0, Scale::Linear, 0);
    result[AUTO_GAIN as usize] = parameter(
        AUTO_GAIN,
        "Auto Gain",
        "",
        (0.0, 1.0),
        1.0,
        Scale::Linear,
        1,
    );
    result[BANDS as usize] = parameter(BANDS, "Bands", "", (1.0, 6.0), 1.0, Scale::Linear, 5);
    result[CROSSOVER_SLOPE as usize] = parameter(
        CROSSOVER_SLOPE,
        "Crossover Slope",
        "",
        (0.0, 2.0),
        1.0,
        Scale::Linear,
        2,
    );
    let mut crossover = 0;
    while crossover < CROSSOVERS {
        let id = CROSSOVER_BASE + crossover as u32;
        result[id as usize] = parameter(
            id,
            CROSSOVER_NAMES[crossover],
            "Hz",
            (20.0, 20_000.0),
            CROSSOVER_DEFAULTS[crossover],
            Scale::Log,
            0,
        );
        crossover += 1;
    }
    result[LFO_RATE as usize] =
        parameter(LFO_RATE, "LFO Rate", "Hz", (0.05, 20.0), 1.0, Scale::Log, 0);
    result[LFO_SHAPE as usize] = parameter(
        LFO_SHAPE,
        "LFO Shape",
        "",
        (0.0, 3.0),
        0.0,
        Scale::Linear,
        3,
    );
    result[ENVELOPE_ATTACK as usize] = parameter(
        ENVELOPE_ATTACK,
        "Envelope Attack",
        "ms",
        (0.5, 200.0),
        10.0,
        Scale::Log,
        0,
    );
    result[ENVELOPE_RELEASE as usize] = parameter(
        ENVELOPE_RELEASE,
        "Envelope Release",
        "ms",
        (10.0, 2000.0),
        150.0,
        Scale::Log,
        0,
    );
    let mut slot = 0;
    while slot < MOD_SLOTS {
        let names = MOD_NAMES[slot];
        let source = mod_parameter(slot, MOD_SOURCE);
        let target = mod_parameter(slot, MOD_TARGET);
        let band = mod_parameter(slot, MOD_BAND);
        let amount = mod_parameter(slot, MOD_AMOUNT);
        result[source as usize] =
            parameter(source, names[0], "", (0.0, 2.0), 0.0, Scale::Linear, 2);
        result[target as usize] =
            parameter(target, names[1], "", (0.0, 12.0), 0.0, Scale::Linear, 12);
        result[band as usize] = parameter(band, names[2], "", (0.0, 6.0), 0.0, Scale::Linear, 6);
        result[amount as usize] = parameter(
            amount,
            names[3],
            "%",
            (-100.0, 100.0),
            0.0,
            Scale::Linear,
            0,
        );
        slot += 1;
    }
    result
}

/// Every parameter, indexed by id.
pub const PARAMETERS: [Spec; COUNT] = specs();

/// The names of a stepped parameter's settings; switches (Auto Gain, Solo, Mute) have
/// none and read Off and On.
pub fn choices(id: u32) -> Option<&'static [&'static str]> {
    if let Some((_, slot)) = band_slot(id) {
        return match slot {
            Slot::Model => Some(&MODELS),
            Slot::Channels => Some(&CHANNEL_MODES),
            _ => None,
        };
    }
    match id {
        BANDS => Some(&BAND_COUNTS),
        CROSSOVER_SLOPE => Some(&SLOPES),
        LFO_SHAPE => Some(&LFO_SHAPES),
        id if id >= MOD_BASE && (id as usize) < COUNT => match (id - MOD_BASE) % MOD_STRIDE {
            MOD_SOURCE => Some(&MOD_SOURCES),
            MOD_TARGET => Some(&MOD_TARGETS),
            MOD_BAND => Some(&MOD_BANDS),
            _ => None,
        },
        _ => None,
    }
}

/// Every parameter at its default.
pub fn defaults() -> [f64; COUNT] {
    std::array::from_fn(|id| PARAMETERS[id].default)
}

pub(crate) fn amplitude(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

pub(crate) fn decibels(level: f64) -> f64 {
    20.0 * level.max(1.0e-6).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_has_its_parameter_and_the_first_set_is_unchanged() {
        for (id, spec) in PARAMETERS.iter().enumerate() {
            assert_eq!(spec.id as usize, id, "{}", spec.name);
            assert!(!spec.name.is_empty(), "{id}");
        }
        // The first set's ids, names, ranges and defaults, as hosts stored them.
        let first = [
            (DRIVE, "Drive", 0.0, 36.0, 6.0),
            (HARDNESS, "Hardness", 0.0, 100.0, 30.0),
            (BIAS, "Bias", 0.0, 100.0, 30.0),
            (ODD, "Odd", 0.0, 100.0, 100.0),
            (EVEN, "Even", 0.0, 300.0, 100.0),
            (EMPHASIS, "Emphasis", -12.0, 12.0, 0.0),
            (
                EMPHASIS_FREQUENCY,
                "Emphasis Frequency",
                200.0,
                8000.0,
                2000.0,
            ),
            (TONE, "Tone", -6.0, 6.0, 0.0),
            (MIX, "Mix", 0.0, 100.0, 100.0),
            (OUTPUT, "Output", -24.0, 24.0, 0.0),
            (AUTO_GAIN, "Auto Gain", 0.0, 1.0, 1.0),
        ];
        for (id, name, min, max, default) in first {
            let spec = PARAMETERS[id as usize];
            assert_eq!(
                (spec.name, spec.min, spec.max, spec.default),
                (name, min, max, default)
            );
        }
        assert_eq!(PARAMETERS[EMPHASIS_FREQUENCY as usize].scale, Scale::Log);
        assert_eq!(PARAMETERS[AUTO_GAIN as usize].steps, 1);
    }

    #[test]
    fn every_band_control_maps_to_one_id_and_back() {
        let mut seen = std::collections::BTreeSet::new();
        for band in 0..MAX_BANDS {
            for slot in Slot::ALL {
                let id = band_parameter(band, slot);
                assert!(seen.insert(id), "{band} {slot:?}");
                assert_eq!(band_slot(id), Some((band, slot)));
                let first = PARAMETERS[band_parameter(0, slot) as usize];
                let spec = PARAMETERS[id as usize];
                assert_eq!(
                    (spec.min, spec.max, spec.default, spec.steps),
                    (first.min, first.max, first.default, first.steps)
                );
            }
        }
        assert_eq!(seen.len(), MAX_BANDS * Slot::ALL.len());
        assert_eq!(band_parameter(0, Slot::Drive), DRIVE);
        assert_eq!(
            band_parameter(0, Slot::EmphasisFrequency),
            EMPHASIS_FREQUENCY
        );
        assert_eq!(band_slot(TONE), None);
        assert_eq!(band_slot(MIX), None);
        assert_eq!(band_slot(LFO_RATE), None);
        assert_eq!(
            PARAMETERS[band_parameter(3, Slot::Bias) as usize].name,
            "Band 4 Bias"
        );
        assert_eq!(mod_parameter(MOD_SLOTS - 1, MOD_AMOUNT) as usize, COUNT - 1);
    }

    #[test]
    fn every_choice_list_matches_its_steps() {
        for spec in PARAMETERS {
            if let Some(names) = choices(spec.id) {
                assert_eq!(names.len() as u32, spec.steps + 1, "{}", spec.name);
            }
        }
        assert_eq!(choices(band_parameter(2, Slot::Model)), Some(&MODELS[..]));
        assert_eq!(
            choices(mod_parameter(1, MOD_TARGET)),
            Some(&MOD_TARGETS[..])
        );
        assert_eq!(choices(band_parameter(0, Slot::Solo)), None);
    }
}
