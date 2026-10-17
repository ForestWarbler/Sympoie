use sympoie::automation::{AutomationTarget, SubtractiveParameter};

pub(super) fn format(target: AutomationTarget, value: f64) -> String {
    target.format_value(value)
}

pub(super) fn hint(target: AutomationTarget) -> &'static str {
    match target {
        AutomationTarget::TrackGain => "dB (up to +6.02; −∞ for silence)",
        AutomationTarget::TrackPan => "−100 to +100; C for center; or 25L / 25R",
        AutomationTarget::Subtractive(SubtractiveParameter::Waveform) => {
            "Sine (0), Saw (1), or Square (2)"
        }
        AutomationTarget::Subtractive(SubtractiveParameter::Cutoff) => "Hz (20–20000)",
        AutomationTarget::Subtractive(SubtractiveParameter::Resonance) => "Q (0.5–10)",
        AutomationTarget::Subtractive(
            SubtractiveParameter::Attack
            | SubtractiveParameter::Decay
            | SubtractiveParameter::Release,
        ) => "Seconds (0–10)",
        _ => "Percent (0–100)",
    }
}

pub(super) fn parse(target: AutomationTarget, text: &str) -> Option<f64> {
    let text = text.trim().to_lowercase().replace('−', "-");
    let value = match target {
        AutomationTarget::TrackGain => {
            let text = without_unit(&text, "db");
            if matches!(text, "-inf" | "-infinity" | "-∞") {
                return Some(0.0);
            }
            let db = number(text)?;
            if db > 20.0 * 2.0_f64.log10() {
                return None;
            }
            10.0_f64.powf(db / 20.0) / 2.0
        }
        AutomationTarget::TrackPan => {
            if matches!(text.as_str(), "c" | "center" | "centre") {
                return Some(0.5);
            }
            let signed = if let Some(left) = text.strip_suffix('l') {
                -bounded(without_unit(left, "%"), 0.0, 100.0)?
            } else if let Some(right) = text.strip_suffix('r') {
                bounded(without_unit(right, "%"), 0.0, 100.0)?
            } else {
                bounded(without_unit(&text, "%"), -100.0, 100.0)?
            };
            (signed / 100.0 + 1.0) / 2.0
        }
        AutomationTarget::Subtractive(SubtractiveParameter::Waveform) => {
            return match text.as_str() {
                "sine" | "0" => Some(0.0),
                "saw" | "1" => Some(0.5),
                "square" | "2" => Some(1.0),
                _ => None,
            };
        }
        AutomationTarget::Subtractive(SubtractiveParameter::Cutoff) => {
            let hz = bounded(without_unit(&text, "hz"), 20.0, 20_000.0)?;
            (hz / 20.0).ln() / 1000.0_f64.ln()
        }
        AutomationTarget::Subtractive(SubtractiveParameter::Resonance) => {
            (bounded(without_unit(&text, "q"), 0.5, 10.0)? - 0.5) / 9.5
        }
        AutomationTarget::Subtractive(
            SubtractiveParameter::Attack
            | SubtractiveParameter::Decay
            | SubtractiveParameter::Release,
        ) => bounded(without_unit(&text, "s"), 0.0, 10.0)? / 10.0,
        _ => bounded(without_unit(&text, "%"), 0.0, 100.0)? / 100.0,
    };
    (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(value)
}

fn without_unit<'a>(text: &'a str, unit: &str) -> &'a str {
    let text = text.trim();
    text.strip_suffix(unit).unwrap_or(text).trim()
}

fn number(text: &str) -> Option<f64> {
    let value = text.parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

fn bounded(text: &str, minimum: f64, maximum: f64) -> Option<f64> {
    let value = number(text)?;
    (minimum..=maximum).contains(&value).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use AutomationTarget::{PluginParameter, Subtractive, TrackGain, TrackPan};
    use SubtractiveParameter::{
        Attack, Cutoff, Decay, Gain, Release, Resonance, Sustain, Waveform,
    };

    #[test]
    fn track_gain_uses_decibels_and_explicit_silence() {
        assert_eq!(parse(TrackGain, "0 dB"), Some(0.5));
        assert_eq!(parse(TrackGain, "−∞ dB"), Some(0.0));
        assert_eq!(parse(TrackGain, "-inf"), Some(0.0));
        assert_eq!(parse(TrackGain, " -INFINITY DB "), Some(0.0));
        let quarter = parse(TrackGain, "-6.020599913279624").unwrap();
        assert!((quarter - 0.25).abs() < 1e-15);
        assert_eq!(parse(TrackGain, "6.021"), None);
    }

    #[test]
    fn pan_accepts_center_signed_values_and_directions() {
        for center in ["C", "center", "0", "0L", "0 R"] {
            assert_eq!(parse(TrackPan, center), Some(0.5));
        }
        for left in ["-25", "−25%", "25L", "25 % l"] {
            assert_eq!(parse(TrackPan, left), Some(0.375));
        }
        for right in ["+25", "25", "25R", "25 % R"] {
            assert_eq!(parse(TrackPan, right), Some(0.625));
        }
        assert_eq!(parse(TrackPan, "100L"), Some(0.0));
        assert_eq!(parse(TrackPan, "100R"), Some(1.0));
        for invalid in ["101", "-101", "101L", "-25L", "left", "25LR"] {
            assert_eq!(parse(TrackPan, invalid), None, "{invalid}");
        }
    }

    #[test]
    fn instrument_values_use_the_parameter_units() {
        assert_eq!(parse(Subtractive(Cutoff), "20 Hz"), Some(0.0));
        assert_eq!(parse(Subtractive(Cutoff), "20000"), Some(1.0));
        let cutoff = parse(Subtractive(Cutoff), "200 Hz").unwrap();
        assert!((cutoff - 1.0 / 3.0).abs() < 1e-15);
        assert_eq!(parse(Subtractive(Resonance), "5.25 Q"), Some(0.5));
        for parameter in [Attack, Decay, Release] {
            assert_eq!(parse(Subtractive(parameter), "2.5 s"), Some(0.25));
        }
        for target in [
            Subtractive(Sustain),
            Subtractive(Gain),
            PluginParameter {
                plugin: 7,
                parameter: 2,
            },
        ] {
            assert_eq!(parse(target, "75%"), Some(0.75));
            assert_eq!(parse(target, "75"), Some(0.75));
            assert_eq!(parse(target, "-1"), None);
            assert_eq!(parse(target, "101%"), None);
        }
        for (parameter, text) in [
            (Cutoff, "19"),
            (Cutoff, "20001"),
            (Resonance, "0.49"),
            (Resonance, "10.1"),
            (Attack, "-1"),
            (Release, "10.1"),
        ] {
            assert_eq!(parse(Subtractive(parameter), text), None);
        }
    }

    #[test]
    fn waveform_accepts_names_and_discrete_indices_only() {
        for (name, index, value) in [("SINE", "0", 0.0), ("Saw", "1", 0.5), ("square", "2", 1.0)] {
            assert_eq!(parse(Subtractive(Waveform), name), Some(value));
            assert_eq!(parse(Subtractive(Waveform), index), Some(value));
        }
        for text in ["-1", "3", "0.5", "triangle"] {
            assert_eq!(parse(Subtractive(Waveform), text), None);
        }
    }

    #[test]
    fn malformed_or_nonfinite_values_are_rejected() {
        for target in [
            TrackGain,
            TrackPan,
            Subtractive(Cutoff),
            Subtractive(Resonance),
            Subtractive(Attack),
            Subtractive(Sustain),
        ] {
            for text in [
                "",
                " ",
                "NaN",
                "inf",
                "+inf",
                "∞",
                "1e999",
                "1 2",
                "10 bananas",
            ] {
                assert_eq!(parse(target, text), None, "{target:?}: {text}");
            }
        }
    }

    #[test]
    fn formatted_values_are_valid_input_including_endpoints() {
        for target in [
            TrackGain,
            TrackPan,
            Subtractive(Cutoff),
            Subtractive(Resonance),
            Subtractive(Attack),
            Subtractive(Sustain),
        ] {
            for value in [0.0, 0.0123456789012345, 0.25, 0.5, 0.75, 1.0] {
                let text = format(target, value);
                assert!(parse(target, &text).is_some(), "{target:?}: {text}");
            }
        }
    }
}
