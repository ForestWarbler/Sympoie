//! How parameter values are written out, and read back when typed in.

use sympoie_calor::{Spec, choices};

/// A value as a knob shows it.
pub(crate) fn value(spec: &Spec, value: f64) -> String {
    if let Some(names) = choices(spec.id) {
        let index = (value - spec.min).round().max(0.0) as usize;
        return names.get(index).copied().unwrap_or_default().to_owned();
    }
    match spec.unit {
        "Hz" => hertz(value),
        "dB" if spec.min < 0.0 => format!("{value:+.1} dB"),
        "dB" => format!("{value:.1} dB"),
        "%" if spec.min < 0.0 => format!("{value:+.0}%"),
        "%" => format!("{value:.0}%"),
        "ms" if value < 10.0 => format!("{value:.1} ms"),
        "ms" => format!("{value:.0} ms"),
        _ if spec.steps == 1 => if value >= 0.5 { "On" } else { "Off" }.to_owned(),
        _ => format!("{value:.2}"),
    }
}

pub(crate) fn hertz(value: f64) -> String {
    if value >= 10_000.0 {
        format!("{:.1} kHz", value / 1000.0)
    } else if value >= 1000.0 {
        format!("{:.2} kHz", value / 1000.0)
    } else if value >= 20.0 {
        format!("{value:.0} Hz")
    } else {
        format!("{value:.2} Hz")
    }
}

/// A level in dB with a true minus sign, as the readouts show it.
pub(crate) fn decibels(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    match text.strip_prefix('-') {
        Some(rest) if rest.chars().any(|c| c.is_ascii_digit() && c != '0') => format!("−{rest}"),
        Some(rest) => rest.to_owned(),
        None => text,
    }
}

/// Reads a typed value: a number, optionally with its unit or a `k` for thousands, or
/// `on` and `off` for a switch. Anything else is `None`.
pub(crate) fn parse(spec: &Spec, text: &str) -> Option<f64> {
    if let Some(names) = choices(spec.id) {
        return names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(text.trim()))
            .map(|index| spec.min + index as f64);
    }
    let text = text.trim().to_ascii_lowercase();
    if spec.steps == 1 {
        match text.as_str() {
            "on" | "1" => return Some(1.0),
            "off" | "0" => return Some(0.0),
            _ => {}
        }
    }
    let text = text
        .trim_end_matches(spec.unit.to_ascii_lowercase().as_str())
        .trim_end_matches("hz")
        .trim_end_matches("db")
        .trim_end_matches("ms")
        .trim_end_matches('%')
        .trim();
    let (text, scale) = match text.strip_suffix('k') {
        Some(rest) => (rest.trim(), 1000.0),
        None => (text, 1.0),
    };
    let value: f64 = text.replace('−', "-").parse().ok()?;
    value.is_finite().then_some(value * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sympoie_calor::{
        AUTO_GAIN, BANDS, DRIVE, EMPHASIS_FREQUENCY, EVEN, LFO_RATE, OUTPUT, PARAMETERS, Slot,
        TONE, band_parameter, mod_parameter,
    };

    fn spec(id: u32) -> &'static Spec {
        &PARAMETERS[id as usize]
    }

    #[test]
    fn values_read_the_way_they_are_spoken() {
        assert_eq!(value(spec(DRIVE), 6.0), "6.0 dB");
        assert_eq!(value(spec(OUTPUT), 3.0), "+3.0 dB");
        assert_eq!(value(spec(TONE), -1.5), "-1.5 dB");
        assert_eq!(value(spec(EVEN), 150.0), "150%");
        assert_eq!(value(spec(EMPHASIS_FREQUENCY), 2400.0), "2.40 kHz");
        assert_eq!(value(spec(EMPHASIS_FREQUENCY), 800.0), "800 Hz");
        assert_eq!(value(spec(AUTO_GAIN), 1.0), "On");
        assert_eq!(
            value(spec(band_parameter(2, Slot::Model)), 3.0),
            "Transformer"
        );
        assert_eq!(value(spec(BANDS), 4.0), "4");
        assert_eq!(
            value(spec(band_parameter(1, Slot::Dynamics)), -40.0),
            "-40%"
        );
        assert_eq!(value(spec(LFO_RATE), 1.5), "1.50 Hz");
        assert_eq!(value(spec(mod_parameter(0, 1)), 12.0), "Output");
        assert_eq!(decibels(-18.26, 1), "−18.3");
        assert_eq!(decibels(-0.01, 1), "0.0");
        assert_eq!(decibels(3.0, 0), "3");
    }

    #[test]
    fn typed_values_take_units_thousands_and_switch_words() {
        assert_eq!(parse(spec(EMPHASIS_FREQUENCY), "2.4k"), Some(2400.0));
        assert_eq!(parse(spec(EMPHASIS_FREQUENCY), "2.4 kHz"), Some(2400.0));
        assert_eq!(parse(spec(DRIVE), "9 dB"), Some(9.0));
        assert_eq!(parse(spec(OUTPUT), "−3"), Some(-3.0));
        assert_eq!(parse(spec(EVEN), "150%"), Some(150.0));
        assert_eq!(parse(spec(AUTO_GAIN), "off"), Some(0.0));
        assert_eq!(
            parse(spec(band_parameter(0, Slot::Model)), "tape"),
            Some(2.0)
        );
        assert_eq!(parse(spec(BANDS), "3"), Some(3.0));
        assert_eq!(parse(spec(DRIVE), "loud"), None);
    }
}
