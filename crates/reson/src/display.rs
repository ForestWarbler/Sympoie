//! Turns a frame's spectra, cuts and notes into the bands an interface draws.

use crate::detector::MAX_REDUCTION_DB;
use crate::layout::Layout;
use crate::processor::mixed_cut;
use crate::telemetry::{self, REDUCTION_BANDS, SPECTRUM_BANDS, Telemetry};

/// Where one display band reads the spectrum: the loudest of a run of bins, or, where a
/// band is narrower than a bin, an interpolation between two of them.
#[derive(Debug, Clone, Copy)]
enum Reach {
    Bins(usize, usize),
    Between(usize, f32),
}

/// Reaches of `count` bands log-spaced from 20 Hz to 20 kHz.
fn reaches(layout: &Layout, count: usize) -> Box<[Reach]> {
    let span = 1000.0_f64;
    let half_step = span.powf(0.5 / (count - 1) as f64);
    let half = layout.half();
    let bin = |hz: f64| hz / layout.bin_hz();
    (0..count)
        .map(|band| {
            let centre = 20.0 * span.powf(band as f64 / (count - 1) as f64);
            let first = bin(centre / half_step).ceil() as usize;
            let last = (bin(centre * half_step).floor() as usize).min(half);
            if first >= 1 && last >= first {
                Reach::Bins(first, last)
            } else {
                let position = bin(centre).clamp(1.0, half as f64 - 1.0);
                Reach::Between(position.floor() as usize, position.fract() as f32)
            }
        })
        .collect()
}

fn read(values: &[f32], reach: Reach) -> f32 {
    match reach {
        Reach::Bins(first, last) => values[first..=last]
            .iter()
            .fold(f32::NEG_INFINITY, |peak, &value| peak.max(value)),
        Reach::Between(bin, fraction) => values[bin] + (values[bin + 1] - values[bin]) * fraction,
    }
}

/// How wide a stretch of the spectrum each band of the drawn spectra looks over, in
/// octaves. Bands are a thirteenth of an octave apart, so neighbouring windows overlap and
/// the trace reads as an envelope rather than a comb.
const SMOOTHING_OCTAVES: f64 = 1.0 / 6.0;

/// Where one band of the drawn spectra reads the spectrum: the bins within
/// [`SMOOTHING_OCTAVES`] of its centre, each weighted, in dB, by a window that is flat over
/// the middle half and tapers to nothing at the edges; or, where that window is too narrow
/// to hold three bins, an interpolation between the two around its centre.
///
/// The band reads its loudest weighted bin, so a tone keeps its level at the nearest band
/// (the flat middle is wider than the bands' spacing) and falls away smoothly to either
/// side, and the comb of a note's harmonics reads as their envelope.
#[derive(Debug, Clone)]
enum Smooth {
    Window { first: usize, weights: Box<[f32]> },
    Between(usize, f32),
}

/// The windows of `count` bands log-spaced from 20 Hz to 20 kHz.
fn windows(layout: &Layout, count: usize) -> Box<[Smooth]> {
    let span = 1000.0_f64;
    let half = layout.half();
    let edge = 2.0_f64.powf(SMOOTHING_OCTAVES / 2.0);
    let bin = |hz: f64| hz / layout.bin_hz();
    (0..count)
        .map(|band| {
            let centre = 20.0 * span.powf(band as f64 / (count - 1) as f64);
            let first = (bin(centre / edge).ceil() as usize).max(1);
            let last = (bin(centre * edge).floor() as usize).min(half);
            if last >= first + 2 {
                let weights = (first..=last)
                    .map(|k| {
                        // −1 at the window's lower edge, 1 at its upper one.
                        let x = ((layout.hz(k) / centre).log2() / (SMOOTHING_OCTAVES / 2.0))
                            .abs()
                            .min(1.0);
                        let weight = if x <= 0.5 {
                            1.0
                        } else {
                            0.5 + 0.5 * (std::f64::consts::PI * (x - 0.5) / 0.5).cos()
                        };
                        (10.0 * weight.max(1.0e-6).log10()) as f32
                    })
                    .collect();
                Smooth::Window { first, weights }
            } else {
                let position = bin(centre).clamp(1.0, half as f64 - 1.0);
                Smooth::Between(position.floor() as usize, position.fract() as f32)
            }
        })
        .collect()
}

/// A band of the drawn spectra read from per-bin levels in dB.
fn read_smooth(values: &[f32], window: &Smooth) -> f32 {
    match window {
        Smooth::Window { first, weights } => values[*first..*first + weights.len()]
            .iter()
            .zip(weights.iter())
            .fold(f32::NEG_INFINITY, |peak, (&value, &weight)| {
                peak.max(value + weight)
            }),
        &Smooth::Between(bin, fraction) => values[bin] + (values[bin + 1] - values[bin]) * fraction,
    }
}

/// The mean of the bins a display band reads, for quantities that matter across the band
/// rather than at its loudest bin.
fn read_mean(values: &[f32], reach: Reach) -> f32 {
    match reach {
        Reach::Bins(first, last) => {
            values[first..=last].iter().sum::<f32>() / (last - first + 1) as f32
        }
        Reach::Between(..) => read(values, reach),
    }
}

/// Hops between display refreshes (about 21 ms).
const DISPLAY_HOPS: u32 = 8;
/// Shares of the way the drawn spectra move towards a new reading at each refresh, rising
/// and falling: they rise within a few refreshes and fall back over about 150 ms.
const SPECTRUM_RISE: f32 = 0.5;
const SPECTRUM_FALL: f32 = 0.13;

/// The cuts of a frame's two analysed channels, in dB, and the Mix they are heard at.
#[derive(Clone, Copy)]
pub(crate) struct Cuts<'a> {
    pub(crate) resonance: [&'a [f32]; 2],
    pub(crate) cleaning: [&'a [f32]; 2],
    pub(crate) mix: f32,
}

/// Before/after spectra and the cut curves, only built when an interface is watching.
#[derive(Debug)]
pub(crate) struct Display {
    spectrum: Box<[Smooth]>,
    curve: Box<[Reach]>,
    /// Converts the detector's raw power to dB where a full-scale sine reads 0 dB: such a
    /// sine centred on a bin reads `(frame / 4)²`.
    power_db: f32,
    input: [f32; SPECTRUM_BANDS],
    output: [f32; SPECTRUM_BANDS],
    reduction: [f32; REDUCTION_BANDS],
    cleaning: [f32; REDUCTION_BANDS],
    spared: [f32; REDUCTION_BANDS],
    /// Per-bin scratch: input level, output level, resonance cut and Clean's cut, all in
    /// dB, and whether the notes' harmonics are spared at the bin.
    levels: [Box<[f32]>; 5],
    silence: Box<[f32]>,
    countdown: u32,
}

impl Display {
    pub(crate) fn new(layout: &Layout) -> Self {
        let bins = || vec![0.0; layout.bins()].into_boxed_slice();
        Self {
            spectrum: windows(layout, SPECTRUM_BANDS),
            curve: reaches(layout, REDUCTION_BANDS),
            power_db: -20.0 * (layout.frame as f32 / 4.0).log10(),
            input: [-120.0; SPECTRUM_BANDS],
            output: [-120.0; SPECTRUM_BANDS],
            reduction: [0.0; REDUCTION_BANDS],
            cleaning: [0.0; REDUCTION_BANDS],
            spared: [0.0; REDUCTION_BANDS],
            levels: std::array::from_fn(|_| bins()),
            silence: bins(),
            countdown: 0,
        }
    }

    fn ease(shown: &mut f32, value: f32, rise: f32, fall: f32) {
        *shown += (value - *shown) * if value > *shown { rise } else { fall };
    }

    /// Publishes a frame's spectra, cuts and spared notes every few hops. `cuts` holds the
    /// cuts of the two analysed channels, `None` when the frame passed through untouched;
    /// `spared` holds the cut Focus allows at each bin of the two, `None` when the notes are
    /// not being followed; `notes` holds the fundamentals found.
    pub(crate) fn update(
        &mut self,
        telemetry: &Telemetry,
        power: &[f32],
        cuts: Option<Cuts<'_>>,
        spared: Option<[&[f32]; 2]>,
        notes: &[f32],
    ) {
        if self.countdown > 0 {
            self.countdown -= 1;
            return;
        }
        self.countdown = DISPLAY_HOPS - 1;
        let [input, output, cut, cleaned, kept] = &mut self.levels;
        for k in 0..input.len() {
            let level = 10.0 * power[k].max(1.0e-30).log10() + self.power_db;
            input[k] = level;
            let (resonance, total) = cuts.map_or((0.0, 0.0), |cuts| {
                let [first, second] = cuts.resonance;
                let [first_clean, second_clean] = cuts.cleaning;
                let resonance = first[k].max(second[k]);
                let total = (first[k] + first_clean[k])
                    .max(second[k] + second_clean[k])
                    .min(MAX_REDUCTION_DB);
                if cuts.mix >= 1.0 {
                    (resonance, total)
                } else {
                    (mixed_cut(resonance, cuts.mix), mixed_cut(total, cuts.mix))
                }
            });
            output[k] = level - total;
            cut[k] = resonance;
            cleaned[k] = (total - resonance).max(0.0);
            kept[k] = match spared {
                Some([first, second]) if first[k].is_finite() || second[k].is_finite() => 1.0,
                _ => 0.0,
            };
        }
        for (band, window) in self.spectrum.iter().enumerate() {
            let (rise, fall) = (SPECTRUM_RISE, SPECTRUM_FALL);
            Self::ease(
                &mut self.input[band],
                read_smooth(input, window),
                rise,
                fall,
            );
            Self::ease(
                &mut self.output[band],
                read_smooth(output, window),
                rise,
                fall,
            );
            telemetry::set(&telemetry.input_spectrum[band], self.input[band]);
            telemetry::set(&telemetry.spectrum[band], self.output[band]);
        }
        for (band, &reach) in self.curve.iter().enumerate() {
            Self::ease(&mut self.reduction[band], read(cut, reach), 0.8, 0.4);
            telemetry::set(&telemetry.reduction_spectrum[band], self.reduction[band]);
            let cleaning = read_mean(cleaned, reach);
            Self::ease(&mut self.cleaning[band], cleaning, 0.6, 0.25);
            telemetry::set(&telemetry.cleaning_spectrum[band], self.cleaning[band]);
            Self::ease(&mut self.spared[band], read(kept, reach), 0.6, 0.25);
            telemetry::set(&telemetry.spared_spectrum[band], self.spared[band]);
        }
        for (slot, cell) in telemetry.notes.iter().enumerate() {
            telemetry::set(cell, notes.get(slot).copied().unwrap_or(0.0));
        }
        telemetry
            .spectrum_updates
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Lets the curve and spectra fall back after silence, without spending an FFT.
    pub(crate) fn idle(&mut self, telemetry: &Telemetry) {
        let silence = std::mem::take(&mut self.silence);
        self.update(telemetry, &silence, None, None, &[]);
        self.silence = silence;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-bin levels in dB: `floor` everywhere but `peak` dB at the given bins.
    fn levels(layout: &Layout, floor: f32, peaks: &[usize]) -> Vec<f32> {
        let mut levels = vec![floor; layout.bins()];
        for &bin in peaks {
            levels[bin] = 0.0;
        }
        levels
    }

    #[test]
    fn a_tone_reads_its_level_at_its_nearest_band_wherever_it_falls() {
        let layout = Layout::new(48_000.0);
        let windows = windows(&layout, SPECTRUM_BANDS);
        // Every bin from 700 Hz, where the windows begin, to the top band at 20 kHz.
        for bin in 30..=853 {
            let levels = levels(&layout, -100.0, &[bin]);
            let loudest = windows
                .iter()
                .map(|window| read_smooth(&levels, window))
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(loudest > -0.01, "bin {bin}: {loudest} dB");
        }
    }

    #[test]
    fn a_comb_of_harmonics_reads_as_its_envelope() {
        let layout = Layout::new(48_000.0);
        // A 220 Hz note's harmonics, each in its nearest bin, 60 dB over the floor.
        let peaks: Vec<usize> = (1..=70)
            .map(|harmonic| (220.0 * f64::from(harmonic) / layout.bin_hz()).round() as usize)
            .collect();
        let levels = levels(&layout, -60.0, &peaks);
        // Where the harmonics are closer together than a window's flat middle is wide
        // (above about 3 kHz for this note); below, the trace dips between them.
        let in_band = |band: usize| {
            let hz = 20.0 * 1000.0_f64.powf(band as f64 / (SPECTRUM_BANDS - 1) as f64);
            (3000.0..14_000.0).contains(&hz)
        };
        let windows = windows(&layout, SPECTRUM_BANDS);
        let smooth: Vec<f32> = (0..SPECTRUM_BANDS)
            .filter(|&band| in_band(band))
            .map(|band| read_smooth(&levels, &windows[band]))
            .collect();
        assert!(smooth.iter().all(|&level| level > -1.0), "{smooth:?}");
        // Low down, where the harmonics are far apart, they still read one by one: the
        // band halfway between the second and the third falls to the floor.
        let between = (0..SPECTRUM_BANDS)
            .min_by_key(|&band| {
                let hz = 20.0 * 1000.0_f64.powf(band as f64 / (SPECTRUM_BANDS - 1) as f64);
                ((hz / 550.0).log2().abs() * 1.0e6) as u64
            })
            .unwrap();
        assert!(read_smooth(&levels, &windows[between]) < -30.0);
    }
}
