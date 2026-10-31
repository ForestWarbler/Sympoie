//! Reson's parameters: one table of ids, names, units, ranges and defaults, shared by
//! every host that runs it, and the sensitivity curve its bands and cuts draw.

pub const DEPTH: u32 = 0;
pub const WIDTH: u32 = 1;
pub const THRESHOLD: u32 = 2;
pub const ATTACK: u32 = 3;
pub const RELEASE: u32 = 4;
pub const KNEE: u32 = 5;
pub const STEREO: u32 = 6;
pub const MIX: u32 = 7;
pub const OUTPUT: u32 = 8;
pub const DELTA: u32 = 9;
pub const LOW_CUT: u32 = 10;
pub const HIGH_CUT: u32 = 11;
/// First sensitivity band. Each band is a frequency, a gain and a Q, in that order.
pub const BAND_BASE: u32 = 12;
pub const BAND_STRIDE: u32 = 3;
pub const BANDS: usize = 5;
pub const BAND_FREQUENCY: u32 = 0;
pub const BAND_GAIN: u32 = 1;
pub const BAND_Q: u32 = 2;
/// How much the cuts leave the harmonics of the notes Reson hears alone, after the bands.
pub const FOCUS: u32 = BAND_BASE + BANDS as u32 * BAND_STRIDE;
/// How far Reson turns down the noise floor around the notes, in dB.
pub const CLEAN: u32 = FOCUS + 1;
/// Number of parameters; their ids run from 0 to `COUNT - 1`.
pub const COUNT: usize = CLEAN as usize + 1;

pub const STEREO_MODES: [&str; 3] = ["Link", "Dual", "M/S"];

/// Low and high cut settings at which the cuts are switched off.
pub const LOW_CUT_OFF: f64 = 20.0;
pub const HIGH_CUT_OFF: f64 = 20_000.0;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandShape {
    LowShelf,
    Bell,
    HighShelf,
}

pub const BAND_SHAPES: [BandShape; BANDS] = [
    BandShape::LowShelf,
    BandShape::Bell,
    BandShape::Bell,
    BandShape::Bell,
    BandShape::HighShelf,
];

const fn band(
    index: u32,
    names: [&'static str; 3],
    frequency: f64,
    gain: f64,
    q: f64,
) -> [Spec; 3] {
    let base = BAND_BASE + index * BAND_STRIDE;
    [
        parameter(
            base + BAND_FREQUENCY,
            names[0],
            "Hz",
            (20.0, 20_000.0),
            frequency,
            Scale::Log,
            0,
        ),
        parameter(
            base + BAND_GAIN,
            names[1],
            "dB",
            (-12.0, 12.0),
            gain,
            Scale::Linear,
            0,
        ),
        parameter(base + BAND_Q, names[2], "", (0.3, 8.0), q, Scale::Log, 0),
    ]
}

const fn specs() -> [Spec; COUNT] {
    let main = [
        parameter(DEPTH, "Depth", "dB", (0.0, 20.0), 6.0, Scale::Linear, 0),
        parameter(
            WIDTH,
            "Width",
            "oct",
            (1.0 / 64.0, 0.25),
            1.0 / 12.0,
            Scale::Log,
            0,
        ),
        parameter(
            THRESHOLD,
            "Threshold",
            "dB",
            (0.0, 15.0),
            5.0,
            Scale::Linear,
            0,
        ),
        parameter(ATTACK, "Attack", "ms", (0.5, 100.0), 4.0, Scale::Log, 0),
        parameter(RELEASE, "Release", "ms", (5.0, 1000.0), 60.0, Scale::Log, 0),
        parameter(KNEE, "Knee", "dB", (0.0, 24.0), 12.0, Scale::Linear, 0),
        parameter(STEREO, "Stereo", "", (0.0, 2.0), 0.0, Scale::Linear, 2),
        parameter(MIX, "Mix", "%", (0.0, 100.0), 100.0, Scale::Linear, 0),
        parameter(OUTPUT, "Output", "dB", (-12.0, 12.0), 0.0, Scale::Linear, 0),
        parameter(DELTA, "Delta", "", (0.0, 1.0), 0.0, Scale::Linear, 1),
        parameter(
            LOW_CUT,
            "Low Cut",
            "Hz",
            (LOW_CUT_OFF, 2000.0),
            LOW_CUT_OFF,
            Scale::Log,
            0,
        ),
        parameter(
            HIGH_CUT,
            "High Cut",
            "Hz",
            (1000.0, HIGH_CUT_OFF),
            HIGH_CUT_OFF,
            Scale::Log,
            0,
        ),
    ];
    let bands = [
        band(
            0,
            ["Low Shelf Frequency", "Low Shelf Gain", "Low Shelf Q"],
            250.0,
            -10.0,
            0.7,
        ),
        band(
            1,
            ["Bell 1 Frequency", "Bell 1 Gain", "Bell 1 Q"],
            500.0,
            0.0,
            1.0,
        ),
        band(
            2,
            ["Bell 2 Frequency", "Bell 2 Gain", "Bell 2 Q"],
            2000.0,
            0.0,
            1.0,
        ),
        band(
            3,
            ["Bell 3 Frequency", "Bell 3 Gain", "Bell 3 Q"],
            6000.0,
            0.0,
            1.0,
        ),
        band(
            4,
            ["High Shelf Frequency", "High Shelf Gain", "High Shelf Q"],
            10_000.0,
            0.0,
            0.7,
        ),
    ];
    let mut result = [main[0]; COUNT];
    let mut index = 0;
    while index < main.len() {
        result[index] = main[index];
        index += 1;
    }
    let mut band = 0;
    while band < BANDS {
        let mut item = 0;
        while item < 3 {
            result[index] = bands[band][item];
            index += 1;
            item += 1;
        }
        band += 1;
    }
    result[index] = parameter(FOCUS, "Focus", "%", (0.0, 100.0), 0.0, Scale::Linear, 0);
    result[index + 1] = parameter(CLEAN, "Clean", "dB", (0.0, 24.0), 0.0, Scale::Linear, 0);
    result
}

/// Every parameter, indexed by id.
pub const PARAMETERS: [Spec; COUNT] = specs();

/// The names of a stepped parameter's settings.
pub fn choices(id: u32) -> Option<&'static [&'static str]> {
    match id {
        STEREO => Some(&STEREO_MODES),
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

/// How strongly Reson works at each frequency: the sum of its shelf and bell bands in dB,
/// as a factor on Depth, faded out below the low cut and above the high cut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sensitivity {
    low_cut: f64,
    high_cut: f64,
    bands: [(f64, f64, f64); BANDS],
}

impl Sensitivity {
    /// The curve for a set of parameter values in physical units, indexed by id.
    pub fn from_values(values: &[f64; COUNT]) -> Self {
        Self {
            low_cut: values[LOW_CUT as usize],
            high_cut: values[HIGH_CUT as usize],
            bands: std::array::from_fn(|band| {
                let base = (BAND_BASE + band as u32 * BAND_STRIDE) as usize;
                (values[base], values[base + 1], values[base + 2])
            }),
        }
    }

    /// The bands' combined gain at `hz`, in dB, leaving out the cuts.
    pub fn bands_db(&self, hz: f64) -> f64 {
        if !(hz.is_finite() && hz > 0.0) {
            return 0.0;
        }
        self.bands
            .iter()
            .zip(BAND_SHAPES)
            .map(|(&(frequency, gain, q), shape)| {
                if gain == 0.0 {
                    return 0.0;
                }
                let octaves = (hz / frequency).log2();
                match shape {
                    BandShape::Bell => {
                        // A bell as wide as the matching EQ band, measured at half its gain.
                        let width = 2.0 * (0.5 / q).asinh() / std::f64::consts::LN_2;
                        let sigma = width / 2.354_820_045;
                        gain * (-0.5 * (octaves / sigma).powi(2)).exp()
                    }
                    BandShape::LowShelf => gain / (1.0 + (4.0 * q * octaves).exp()),
                    BandShape::HighShelf => gain / (1.0 + (-4.0 * q * octaves).exp()),
                }
            })
            .sum()
    }

    /// Factor on Depth at `hz`.
    pub fn weight(&self, hz: f64) -> f64 {
        if !(hz.is_finite() && hz > 0.0) {
            return 0.0;
        }
        let mut weight = amplitude(self.bands_db(hz));
        if self.low_cut > LOW_CUT_OFF {
            weight /= 1.0 + (self.low_cut / hz).powi(4);
        }
        if self.high_cut < HIGH_CUT_OFF {
            weight /= 1.0 + (hz / self.high_cut).powi(4);
        }
        weight
    }

    /// The sensitivity curve at `hz` in dB, cuts included.
    pub fn db(&self, hz: f64) -> f64 {
        decibels(self.weight(hz))
    }
}
