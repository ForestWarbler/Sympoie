//! The style library: starting points for a band, each a model and the settings that make
//! it speak. A style is applied by setting the band's controls, so nothing about it is
//! stored or automated on its own, and every setting stays free to change afterwards.

use crate::params::{MODELS, Slot};

/// One style: a name, a short description, and the band settings it makes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub name: &'static str,
    pub description: &'static str,
    pub settings: &'static [(Slot, f64)],
}

const fn model(name: &str) -> f64 {
    let mut index = 0;
    while index < MODELS.len() {
        if const_eq(MODELS[index], name) {
            return index as f64;
        }
        index += 1;
    }
    panic!("no such model");
}

const fn const_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut index = 0;
    while index < a.len() {
        if a[index] != b[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// Every style, in the order a menu lists them.
pub const STYLES: [Style; 12] = [
    Style {
        name: "Gentle Warmth",
        description: "A little weight and glow; barely there until you push it.",
        settings: &[
            (Slot::Model, model("Curve")),
            (Slot::Drive, 4.0),
            (Slot::Hardness, 20.0),
            (Slot::Bias, 30.0),
            (Slot::Odd, 70.0),
            (Slot::Even, 120.0),
            (Slot::Emphasis, 0.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Warm Tube",
        description: "Even harmonics that bloom on the loud notes.",
        settings: &[
            (Slot::Model, model("Tube")),
            (Slot::Drive, 9.0),
            (Slot::Hardness, 25.0),
            (Slot::Bias, 55.0),
            (Slot::Odd, 85.0),
            (Slot::Even, 160.0),
            (Slot::Emphasis, -2.0),
            (Slot::EmphasisFrequency, 1500.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Hot Tube",
        description: "A tube stage driven into its grid: thick, forward, a little dirty.",
        settings: &[
            (Slot::Model, model("Tube")),
            (Slot::Drive, 16.0),
            (Slot::Hardness, 35.0),
            (Slot::Bias, 70.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 200.0),
            (Slot::Emphasis, 0.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Clean Tape",
        description: "Soft, round level control with a gentle lift in the lows.",
        settings: &[
            (Slot::Model, model("Tape")),
            (Slot::Drive, 6.0),
            (Slot::Hardness, 30.0),
            (Slot::Bias, 15.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 40.0),
            (Slot::Emphasis, 3.0),
            (Slot::EmphasisFrequency, 3000.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Old Tape",
        description: "A worn machine run hot: squashed highs, a pumping body.",
        settings: &[
            (Slot::Model, model("Tape")),
            (Slot::Drive, 12.0),
            (Slot::Hardness, 20.0),
            (Slot::Bias, 25.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 70.0),
            (Slot::Emphasis, 6.0),
            (Slot::EmphasisFrequency, 2500.0),
            (Slot::Dynamics, 15.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Iron Transformer",
        description: "Lows that thicken and growl while the highs stay clean.",
        settings: &[
            (Slot::Model, model("Transformer")),
            (Slot::Drive, 12.0),
            (Slot::Hardness, 40.0),
            (Slot::Bias, 20.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 60.0),
            (Slot::Emphasis, 0.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Console Glue",
        description: "A mix bus run warm, with a touch of compression.",
        settings: &[
            (Slot::Model, model("Curve")),
            (Slot::Drive, 6.0),
            (Slot::Hardness, 45.0),
            (Slot::Bias, 10.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 30.0),
            (Slot::Emphasis, 0.0),
            (Slot::Dynamics, 25.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Crunch",
        description: "Edge and bite in the upper mids.",
        settings: &[
            (Slot::Model, model("Curve")),
            (Slot::Drive, 20.0),
            (Slot::Hardness, 70.0),
            (Slot::Bias, 35.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 100.0),
            (Slot::Emphasis, 3.0),
            (Slot::EmphasisFrequency, 1800.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Fuzz",
        description: "Clipped flat and leaning hard, with a hint of ringing.",
        settings: &[
            (Slot::Model, model("Curve")),
            (Slot::Drive, 30.0),
            (Slot::Hardness, 95.0),
            (Slot::Bias, 60.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 150.0),
            (Slot::Emphasis, 0.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 20.0),
            (Slot::FeedbackFrequency, 800.0),
        ],
    },
    Style {
        name: "Folded",
        description: "A wavefolder: metallic overtones that keep growing with Drive.",
        settings: &[
            (Slot::Model, model("Fold")),
            (Slot::Drive, 12.0),
            (Slot::Hardness, 0.0),
            (Slot::Bias, 10.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 50.0),
            (Slot::Emphasis, 0.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Even Exciter",
        description: "Only even harmonics, lifted in the air band, the clean signal untouched.",
        settings: &[
            (Slot::Model, model("Curve")),
            (Slot::Drive, 10.0),
            (Slot::Hardness, 30.0),
            (Slot::Bias, 60.0),
            (Slot::Odd, 0.0),
            (Slot::Even, 220.0),
            (Slot::Emphasis, 6.0),
            (Slot::EmphasisFrequency, 4000.0),
            (Slot::Dynamics, 0.0),
            (Slot::Feedback, 0.0),
        ],
    },
    Style {
        name: "Screamer",
        description: "Hard drive and heavy feedback that sings; tighten it with Dynamics.",
        settings: &[
            (Slot::Model, model("Curve")),
            (Slot::Drive, 24.0),
            (Slot::Hardness, 80.0),
            (Slot::Bias, 40.0),
            (Slot::Odd, 100.0),
            (Slot::Even, 100.0),
            (Slot::Emphasis, 0.0),
            (Slot::Dynamics, -20.0),
            (Slot::Feedback, 70.0),
            (Slot::FeedbackFrequency, 1200.0),
        ],
    },
];

/// The style a band's settings currently match, if any. `value` reads the band's control
/// in a slot.
pub fn matching(value: impl Fn(Slot) -> f64) -> Option<usize> {
    STYLES.iter().position(|style| {
        style
            .settings
            .iter()
            .all(|&(slot, setting)| (value(slot) - setting).abs() < 1.0e-6)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{PARAMETERS, band_parameter};

    #[test]
    fn every_style_sets_valid_values_and_can_be_recognised() {
        for (index, style) in STYLES.iter().enumerate() {
            for &(slot, value) in style.settings {
                let spec = PARAMETERS[band_parameter(0, slot) as usize];
                assert!(
                    (spec.min..=spec.max).contains(&value),
                    "{}: {slot:?} {value}",
                    style.name
                );
            }
            let mut values = crate::params::defaults();
            for &(slot, value) in style.settings {
                values[band_parameter(2, slot) as usize] = value;
            }
            let found = matching(|slot| values[band_parameter(2, slot) as usize]);
            assert_eq!(found, Some(index), "{}", style.name);
        }
        let defaults = crate::params::defaults();
        assert_eq!(
            matching(|slot| defaults[band_parameter(0, slot) as usize]),
            None
        );
    }
}
