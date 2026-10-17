use sympoie::time::{MusicalPosition, SnapGrid, Tempo, TimeError, TimeSignature};

#[test]
fn default_tempo_converts_quarters_seconds_and_frames() {
    let tempo = Tempo::default();
    assert_eq!(tempo.bpm(), 120.0);
    assert_eq!(tempo.ticks_to_seconds(960), Ok(0.5));
    assert_eq!(tempo.seconds_to_ticks(0.5), Ok(960));
    assert_eq!(tempo.ticks_to_frames(960), Ok(24_000));
    assert_eq!(tempo.frames_to_ticks(24_000), Ok(960));

    let slow = Tempo::new(60.0).unwrap();
    assert_eq!(slow.ticks_to_seconds(960), Ok(1.0));
    assert_eq!(slow.ticks_to_frames(960), Ok(48_000));
}

#[test]
fn conversion_rounding_chooses_the_later_position_on_ties() {
    let tempo = Tempo::new(120.0).unwrap();
    assert_eq!(tempo.seconds_to_ticks(0.5 / 1920.0), Ok(1));
    assert_eq!(tempo.frames_to_ticks(12), Ok(0));
    assert_eq!(tempo.frames_to_ticks(13), Ok(1));
    assert_eq!(Tempo::new(6000.0).unwrap().ticks_to_frames(1), Ok(1));
    assert_eq!(Tempo::new(1500.0).unwrap().frames_to_ticks(1), Ok(1));
}

#[test]
fn absolute_conversion_avoids_accumulated_beat_rounding() {
    let tempo = Tempo::new(137.0).unwrap();
    let beat_frames = tempo.ticks_to_frames(960).unwrap();
    let hundred_beats = tempo.ticks_to_frames(96_000).unwrap();
    assert_eq!(hundred_beats, 2_102_190);
    assert_ne!(hundred_beats, beat_frames * 100);
}

#[test]
fn tempo_and_seconds_reject_invalid_values() {
    for bpm in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(Tempo::new(bpm), Err(TimeError::InvalidTempo));
    }
    for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            Tempo::default().seconds_to_ticks(seconds),
            Err(TimeError::InvalidSeconds)
        );
    }
    assert_eq!(Tempo::default().seconds_to_ticks(0.0), Ok(0));
}

#[test]
fn conversions_report_overflow_instead_of_saturating() {
    let tempo = Tempo::default();
    assert_eq!(
        tempo.ticks_to_frames(u64::MAX),
        Err(TimeError::ConversionOverflow)
    );
    assert_eq!(
        tempo.seconds_to_ticks(f64::MAX),
        Err(TimeError::ConversionOverflow)
    );
    assert_eq!(
        Tempo::new(f64::MAX).unwrap().frames_to_ticks(1),
        Err(TimeError::ConversionOverflow)
    );
    let tiny = Tempo::new(f64::from_bits(1)).unwrap();
    assert_eq!(tiny.ticks_to_seconds(1), Err(TimeError::ConversionOverflow));
    assert_eq!(tiny.ticks_to_frames(1), Err(TimeError::ConversionOverflow));
    assert_eq!(tiny.ticks_to_frames(0), Ok(0));
    assert_eq!(tiny.frames_to_ticks(u64::MAX), Ok(0));

    let one_tick_per_second = Tempo::new(0.0625).unwrap();
    let first_unrepresentable = u64::MAX as f64;
    assert_eq!(
        one_tick_per_second.seconds_to_ticks(first_unrepresentable),
        Err(TimeError::ConversionOverflow)
    );
    let previous_float = f64::from_bits(first_unrepresentable.to_bits() - 1);
    assert_eq!(
        one_tick_per_second.seconds_to_ticks(previous_float),
        Ok(u64::MAX - 2047)
    );
}

#[test]
fn meter_changes_bar_lengths_without_changing_tempo_unit() {
    let common = TimeSignature::default();
    assert_eq!((common.numerator(), common.denominator()), (4, 4));
    assert_eq!(common.ticks_per_beat(), 960);
    assert_eq!(common.ticks_per_bar(), 3840);

    let three_four = TimeSignature::new(3, 4).unwrap();
    let six_eight = TimeSignature::new(6, 8).unwrap();
    assert_eq!(three_four.ticks_per_beat(), 960);
    assert_eq!(six_eight.ticks_per_beat(), 480);
    assert_eq!(three_four.ticks_per_bar(), 2880);
    assert_eq!(six_eight.ticks_per_bar(), 2880);
    assert_eq!(
        Tempo::default().ticks_to_seconds(six_eight.ticks_per_bar()),
        Ok(1.5)
    );
}

#[test]
fn meter_accepts_only_exact_tick_note_values() {
    for (numerator, denominator) in [(0, 4), (4, 0), (4, 3), (4, 512), (4, 32768)] {
        assert_eq!(
            TimeSignature::new(numerator, denominator),
            Err(TimeError::InvalidTimeSignature)
        );
    }
    assert_eq!(TimeSignature::new(1, 256).unwrap().ticks_per_beat(), 15);
    assert_eq!(
        TimeSignature::new(u16::MAX, 1).unwrap().ticks_per_bar(),
        u64::from(u16::MAX) * 3840
    );
}

#[test]
fn bar_beat_tick_positions_are_one_based_and_round_trip() {
    let meter = TimeSignature::new(6, 8).unwrap();
    let position = MusicalPosition {
        bar: 2,
        beat: 3,
        tick: 17,
    };
    assert_eq!(meter.tick_at(position), Ok(3857));
    assert_eq!(meter.position_at(3857), position);
    assert_eq!(
        meter.position_at(0),
        MusicalPosition {
            bar: 1,
            beat: 1,
            tick: 0,
        }
    );
    for meter in [meter, TimeSignature::new(1, 256).unwrap()] {
        assert_eq!(meter.tick_at(meter.position_at(u64::MAX)), Ok(u64::MAX));
    }
}

#[test]
fn invalid_or_overflowing_musical_positions_are_rejected() {
    let meter = TimeSignature::default();
    for (bar, beat, tick) in [(0, 1, 0), (1, 0, 0), (1, 5, 0), (1, 1, 960)] {
        assert_eq!(
            meter.tick_at(MusicalPosition { bar, beat, tick }),
            Err(TimeError::InvalidMusicalPosition)
        );
    }
    assert_eq!(
        meter.tick_at(MusicalPosition {
            bar: u64::MAX,
            beat: 1,
            tick: 0,
        }),
        Err(TimeError::ConversionOverflow)
    );
}

#[test]
fn snapping_obeys_note_divisions_and_later_ties() {
    let meter = TimeSignature::default();
    assert_eq!(SnapGrid::default(), SnapGrid::Off);
    assert_eq!(SnapGrid::Off.snap(731, meter), Ok(731));
    for (grid, interval) in [
        (SnapGrid::Bar, 3840),
        (SnapGrid::Quarter, 960),
        (SnapGrid::Eighth, 480),
        (SnapGrid::Sixteenth, 240),
    ] {
        assert_eq!(grid.snap(interval / 2 - 1, meter), Ok(0));
        assert_eq!(grid.snap(interval / 2, meter), Ok(interval));
        assert_eq!(grid.snap(interval, meter), Ok(interval));
        assert_eq!(grid.snap(interval * 2 - 1, meter), Ok(interval * 2));
    }
}

#[test]
fn grid_intervals_and_floors_match_snapping() {
    let three_four = TimeSignature::new(3, 4).unwrap();
    assert_eq!(SnapGrid::Off.interval(three_four), None);
    assert_eq!(SnapGrid::Bar.interval(three_four), Some(2880));
    assert_eq!(SnapGrid::Eighth.interval(three_four), Some(480));
    assert_eq!(SnapGrid::Bar.floor(5000, three_four), 2880);
    assert_eq!(SnapGrid::Sixteenth.floor(5000, three_four), 4800);
    assert_eq!(SnapGrid::Off.floor(5000, three_four), 5000);
}

#[test]
fn only_bar_grid_changes_with_meter() {
    let common = TimeSignature::default();
    let six_eight = TimeSignature::new(6, 8).unwrap();
    assert_eq!(SnapGrid::Bar.snap(1600, common), Ok(0));
    assert_eq!(SnapGrid::Bar.snap(1600, six_eight), Ok(2880));
    assert_eq!(SnapGrid::Quarter.snap(600, six_eight), Ok(960));
    assert_eq!(SnapGrid::Eighth.snap(600, six_eight), Ok(480));

    let odd_tick_bar = TimeSignature::new(1, 256).unwrap();
    assert_eq!(SnapGrid::Bar.snap(7, odd_tick_bar), Ok(0));
    assert_eq!(SnapGrid::Bar.snap(8, odd_tick_bar), Ok(15));
}

#[test]
fn snapping_preserves_disabled_positions_and_checks_upper_boundary() {
    let meter = TimeSignature::default();
    assert_eq!(SnapGrid::Off.snap(u64::MAX, meter), Ok(u64::MAX));
    let last_grid_line = u64::MAX - u64::MAX % 240;
    assert_eq!(
        SnapGrid::Sixteenth.snap(last_grid_line, meter),
        Ok(last_grid_line)
    );

    let fifteen_tick_bar = TimeSignature::new(1, 256).unwrap();
    assert_eq!(SnapGrid::Bar.snap(u64::MAX, fifteen_tick_bar), Ok(u64::MAX));
    let fourteen_quarter_bar = TimeSignature::new(14, 4).unwrap();
    assert_eq!(
        SnapGrid::Bar.snap(u64::MAX, fourteen_quarter_bar),
        Err(TimeError::ConversionOverflow)
    );
}
