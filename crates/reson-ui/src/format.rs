//! How parameter values are written out, and read back when typed in.

use sympoie_reson::{Spec, params};

/// A value as a knob shows it.
pub(crate) fn value(spec: &Spec, value: f64) -> String {
    match spec.unit {
        "Hz" => hertz(value),
        "dB" if spec.min < 0.0 => format!("{value:+.1} dB"),
        "dB" => format!("{value:.1} dB"),
        "ms" if value < 10.0 => format!("{value:.1} ms"),
        "ms" => format!("{value:.0} ms"),
        "%" => format!("{value:.0}%"),
        "oct" => octaves(value),
        _ if spec.id == params::STEREO => params::STEREO_MODES
            .get(value.round() as usize)
            .copied()
            .unwrap_or_default()
            .to_owned(),
        _ if spec.steps == 1 => if value >= 0.5 { "On" } else { "Off" }.to_owned(),
        _ => format!("{value:.2}"),
    }
}

pub(crate) fn hertz(value: f64) -> String {
    if value >= 10_000.0 {
        format!("{:.1} kHz", value / 1000.0)
    } else if value >= 1000.0 {
        format!("{:.2} kHz", value / 1000.0)
    } else {
        format!("{value:.0} Hz")
    }
}

/// A width as a fraction of an octave, the way filter bandwidths are spoken of.
fn octaves(value: f64) -> String {
    let parts = 1.0 / value.max(1.0e-6);
    if (parts - parts.round()).abs() < 0.05 {
        format!("1/{:.0} oct", parts.round())
    } else {
        format!("1/{parts:.1} oct")
    }
}

/// Reads a typed value: a number, optionally with its unit, a `k` for thousands, or for a
/// width a fraction such as `1/12`. Anything else is `None`.
pub(crate) fn parse(spec: &Spec, text: &str) -> Option<f64> {
    let text = text.trim().to_ascii_lowercase();
    let text = text
        .trim_end_matches(spec.unit.to_ascii_lowercase().as_str())
        .trim_end_matches("oct")
        .trim_end_matches("hz")
        .trim_end_matches("db")
        .trim_end_matches("ms")
        .trim_end_matches('%')
        .trim();
    if let Some((numerator, denominator)) = text.split_once('/') {
        let numerator: f64 = numerator.trim().parse().ok()?;
        let denominator: f64 = denominator.trim().parse().ok()?;
        return (denominator != 0.0).then(|| numerator / denominator);
    }
    let (text, scale) = match text.strip_suffix('k') {
        Some(rest) => (rest.trim(), 1000.0),
        None => (text, 1.0),
    };
    let value: f64 = text.parse().ok()?;
    value.is_finite().then_some(value * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sympoie_reson::PARAMETERS;

    #[test]
    fn values_read_the_way_they_are_spoken() {
        let spec = |id: u32| &PARAMETERS[id as usize];
        assert_eq!(value(spec(params::WIDTH), 1.0 / 12.0), "1/12 oct");
        assert_eq!(value(spec(params::WIDTH), 0.1), "1/10 oct");
        assert_eq!(value(spec(params::OUTPUT), 3.0), "+3.0 dB");
        assert_eq!(value(spec(params::DEPTH), 9.0), "9.0 dB");
        assert_eq!(value(spec(params::BAND_BASE), 2400.0), "2.40 kHz");
        assert_eq!(value(spec(params::STEREO), 2.0), "M/S");
        assert_eq!(value(spec(params::ATTACK), 4.0), "4.0 ms");
    }

    #[test]
    fn typed_values_take_units_thousands_and_fractions() {
        let spec = |id: u32| &PARAMETERS[id as usize];
        assert_eq!(parse(spec(params::BAND_BASE), "2.4k"), Some(2400.0));
        assert_eq!(parse(spec(params::BAND_BASE), "2.4 kHz"), Some(2400.0));
        assert_eq!(parse(spec(params::WIDTH), "1/12"), Some(1.0 / 12.0));
        assert_eq!(parse(spec(params::DEPTH), "9 dB"), Some(9.0));
        assert_eq!(parse(spec(params::MIX), "50%"), Some(50.0));
        assert_eq!(parse(spec(params::DEPTH), "loud"), None);
    }
}
