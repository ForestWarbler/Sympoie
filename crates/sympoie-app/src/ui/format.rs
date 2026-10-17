use sympoie::TICKS_PER_QUARTER;
use sympoie::time::{SnapGrid, TimeSignature};

pub const TICKS_PER_SIXTEENTH: u64 = TICKS_PER_QUARTER / 4;

pub fn bar_beat(meter: TimeSignature, tick: u64) -> String {
    let position = meter.position_at(tick);
    let sixteenth = position.tick / TICKS_PER_SIXTEENTH + 1;
    format!("{}.{}.{}", position.bar, position.beat, sixteenth)
}

pub fn bar_beat_tick(meter: TimeSignature, tick: u64) -> String {
    let position = meter.position_at(tick);
    format!("{}.{}.{:03}", position.bar, position.beat, position.tick)
}

pub fn clock(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let (hours, rest) = (millis / 3_600_000, millis % 3_600_000);
    let (minutes, rest) = (rest / 60_000, rest % 60_000);
    let (secs, millis) = (rest / 1000, rest % 1000);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}.{millis:03}")
    } else {
        format!("{minutes}:{secs:02}.{millis:03}")
    }
}

pub fn duration(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.2} s")
    } else {
        let minutes = (seconds / 60.0).floor();
        format!("{}:{:04.1}", minutes as u64, seconds - minutes * 60.0)
    }
}

pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * gain.log10()
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    if db == f32::NEG_INFINITY {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

pub fn db(gain: f32) -> String {
    let value = gain_to_db(gain.abs());
    if value == f32::NEG_INFINITY || value < -99.95 {
        "-∞ dB".to_owned()
    } else if value.abs() < 0.05 {
        "0.0 dB".to_owned()
    } else {
        format!("{value:+.1} dB")
    }
}

pub fn pan(pan: f32) -> String {
    let percent = (pan * 100.0).round() as i32;
    match percent {
        0 => "C".to_owned(),
        p if p < 0 => format!("{}L", -p),
        p => format!("{p}R"),
    }
}

pub fn parse_pan(text: &str) -> Option<f32> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() || text == "c" || text == "center" {
        return Some(0.0);
    }
    let (sign, digits) = if let Some(rest) = text.strip_suffix('l') {
        (-1.0, rest)
    } else if let Some(rest) = text.strip_prefix('l') {
        (-1.0, rest)
    } else if let Some(rest) = text.strip_suffix('r') {
        (1.0, rest)
    } else if let Some(rest) = text.strip_prefix('r') {
        (1.0, rest)
    } else {
        (1.0, text.as_str())
    };
    let value: f32 = digits.trim().trim_end_matches('%').parse().ok()?;
    Some((sign * value / 100.0).clamp(-1.0, 1.0))
}

pub fn fader_to_gain(position: f32) -> f32 {
    let position = position.clamp(0.0, 1.0);
    2.0 * position * position * position
}

pub fn gain_to_fader(gain: f32) -> f32 {
    (gain.max(0.0) / 2.0).cbrt().clamp(0.0, 1.0)
}

pub const MAX_FADER_GAIN: f32 = 2.0;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn note_name(key: u8) -> String {
    let octave = i32::from(key / 12) - 1;
    format!("{}{octave}", NOTE_NAMES[usize::from(key % 12)])
}

pub fn grid_label(grid: SnapGrid) -> &'static str {
    match grid {
        SnapGrid::Off => "Off",
        SnapGrid::Bar => "Bar",
        SnapGrid::Quarter => "1/4",
        SnapGrid::Eighth => "1/8",
        SnapGrid::Sixteenth => "1/16",
    }
}

pub const GRIDS: [SnapGrid; 5] = [
    SnapGrid::Off,
    SnapGrid::Bar,
    SnapGrid::Quarter,
    SnapGrid::Eighth,
    SnapGrid::Sixteenth,
];

pub fn ruler_step(meter: TimeSignature, pixels_per_tick: f64, min_pixels: f64) -> u64 {
    let bar = meter.ticks_per_bar();
    let beat = meter.ticks_per_beat();
    let mut candidates = vec![TICKS_PER_SIXTEENTH, TICKS_PER_QUARTER / 2, beat];
    let mut bars = 1;
    while bars <= 4096 {
        candidates.push(bar * bars);
        bars *= 2;
    }
    candidates
        .into_iter()
        .filter(|step| *step <= bar || step % bar == 0)
        .find(|step| *step as f64 * pixels_per_tick >= min_pixels)
        .unwrap_or(bar * 4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_one_based_bars_beats_and_sixteenths() {
        let four = TimeSignature::default();
        assert_eq!(bar_beat(four, 0), "1.1.1");
        assert_eq!(bar_beat(four, 960 + 240), "1.2.2");
        assert_eq!(bar_beat(four, 3840), "2.1.1");
        let six_eight = TimeSignature::new(6, 8).unwrap();
        assert_eq!(bar_beat(six_eight, 480 * 5 + 240), "1.6.2");
        assert_eq!(bar_beat_tick(four, 961), "1.2.001");
    }

    #[test]
    fn clock_and_duration_formatting() {
        assert_eq!(clock(0.0), "0:00.000");
        assert_eq!(clock(61.2345), "1:01.235");
        assert_eq!(clock(3725.5), "1:02:05.500");
        assert_eq!(duration(2.5), "2.50 s");
        assert_eq!(duration(64.0), "1:04.0");
    }

    #[test]
    fn decibels_and_faders_round_trip() {
        assert_eq!(db(1.0), "0.0 dB");
        assert_eq!(db(0.0), "-∞ dB");
        assert_eq!(db(0.5), "-6.0 dB");
        assert_eq!(db(-2.0), "+6.0 dB");
        assert!((db_to_gain(gain_to_db(0.25)) - 0.25).abs() < 1e-6);
        assert_eq!(db_to_gain(f32::NEG_INFINITY), 0.0);
        for gain in [0.0, 0.1, 1.0, 1.7, 2.0] {
            assert!((fader_to_gain(gain_to_fader(gain)) - gain).abs() < 1e-5);
        }
        assert!((gain_to_fader(1.0) - 0.7937).abs() < 1e-3);
    }

    #[test]
    fn pan_text_round_trips() {
        for value in [-1.0, -0.25, 0.0, 0.5, 1.0] {
            assert_eq!(parse_pan(&pan(value)), Some(value));
        }
        assert_eq!(parse_pan("-30"), Some(-0.3));
        assert_eq!(parse_pan("L100"), Some(-1.0));
        assert_eq!(parse_pan("200r"), Some(1.0));
        assert_eq!(parse_pan("left"), None);
    }

    #[test]
    fn note_names_use_middle_c_four() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(127), "G9");
    }

    #[test]
    fn ruler_steps_grow_with_zoom_out() {
        let meter = TimeSignature::default();
        assert_eq!(ruler_step(meter, 1.0, 60.0), 240);
        assert_eq!(ruler_step(meter, 0.05, 60.0), 3840);
        assert_eq!(ruler_step(meter, 0.01, 60.0), 7680);
    }
}
