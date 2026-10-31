//! Note detection for Focus.
//!
//! Focus leaves the notes alone and cuts only resonances. To know where the notes are,
//! every frame is searched for the fundamentals that best explain its spectral peaks, and
//! the harmonics of each note found are protected.
//!
//! 1. A peak is a local maximum that rises at least 6 dB above the valleys either side of
//!    it; the further it rises, the stronger it is.
//! 2. Every candidate fundamental from 60 Hz to 1.2 kHz, 36 to an octave, is ranked by the
//!    strength of the peaks at its harmonics below 5 kHz, each weighted down with its
//!    frequency the way Klapuri's harmonic summation does, times the share of the peak
//!    strength in its range that those harmonics explain, times the share of its harmonics
//!    that are there at all. A candidate at a multiple of the real fundamental leaves the
//!    harmonics in between unexplained; one below it finds many of its own missing.
//! 3. The best-ranked candidate has its fundamental fitted loosely to the exact frequencies
//!    of its lowest four harmonic peaks. Then its first twelve partials are followed
//!    tightly, each expected where a fit of the partials found so far puts it, within 0.4 %
//!    or 0.4 bins: a note's partials line up that closely (stretched a little, as a stiff
//!    string's are), while peaks of noise that happen to fall near a candidate's harmonics
//!    scatter.
//!    A candidate is a note only if more than half of its partials are followed (somewhat
//!    fewer beside a note found already, whose partials pull nearby ones aside, though it
//!    must still bring three partials of its own), stand clear of their valleys, include
//!    odd-numbered ones (a candidate an octave below a real note finds only the even
//!    ones), and sit, on average, within a quarter of a bin of where they are expected.
//!    Otherwise the search moves on to the next candidate.
//! 4. A note's peaks are taken out of the running and the search repeats for up to
//!    [`MAX_NOTES`], so chords work too. A note counts once it has been found in two
//!    searches running.
//!
//! A note's harmonics are then followed upwards one by one, each expected where a fit of
//! the stretch of the partials found so far puts it, so the stretched partials of a stiff
//! string are still matched. Each is protected down to the level of the harmonics either
//! side of it, the second loudest of the nearest two on each side: one that sits on its
//! note's spectral envelope is never cut, nor is the loud half of a timbre whose harmonics
//! alternate loud and soft, as a clarinet's do, while one a resonance has lifted above its
//! neighbours can still be brought back towards them. That is how Focus keeps cutting
//! resonances in pitched material, where a resonance mostly shows as a harmonic that is
//! too loud. A lone sine has no harmonics and is never a note, so a whistle, a hum or
//! feedback is always cut.

use crate::detector::POWER_FLOOR;
use crate::layout::Layout;
use crate::spectral::log2;

/// Most notes found in one frame.
pub(super) const MAX_NOTES: usize = 4;
const DB_PER_LOG2: f32 = 3.010_3;

const LOWEST_HZ: f32 = 60.0;
const PER_OCTAVE: f32 = 36.0;
/// Candidates from 60 Hz to just past 1.2 kHz.
const CANDIDATES: usize = 157;
/// At most this many harmonics rank a candidate, all below [`SCORE_LIMIT_HZ`].
const SCORED: usize = 20;
const SCORE_LIMIT_HZ: f32 = 5000.0;
/// Above this a ranked harmonic is looked for two bins either side instead of one: the
/// candidates are up to half a step out of tune, and that grows with the harmonic.
const WIDE_ABOVE_HZ: f32 = 2400.0;
/// Klapuri's harmonic weights, `(f0 + ALPHA) / (h·f0 + BETA)`.
const ALPHA_HZ: f32 = 27.0;
const BETA_HZ: f32 = 320.0;
/// Least share of a candidate's harmonics present, and of the peak strength in its range
/// it explains, for it to be tried.
const COVERAGE: f32 = 0.4;
const EXPLAINED: f32 = 0.15;

/// How far a maximum must rise above its valleys to be a peak, and how far for full
/// strength, in dB.
const PEAK_MIN_DB: f32 = 6.0;
const STRENGTH_FULL_DB: f32 = 30.0;
/// Least strength of a harmonic that counts as present when ranking.
const PRESENT: f32 = 0.3;
/// Partials a note needs.
const MIN_HARMONICS: usize = 3;
/// How far apart the same note may be in two searches running.
const CONFIRM: f32 = 0.03;
/// Candidates a search may turn down before it gives up.
const ATTEMPTS: usize = 8;

/// The loose fit takes the first few harmonics, looking for each within a share of its
/// frequency and at least some bins either side.
const LOOSE_HARMONICS: usize = 4;
const LOOSE_SHARE: f32 = 0.02;
const LOOSE_BINS: f32 = 1.2;
/// The tight fit follows up to a dozen partials below the limit, each within a share of
/// its frequency and at least some bins of where it is expected.
const TIGHT_HARMONICS: usize = 12;
const TIGHT_SHARE: f32 = 0.004;
const TIGHT_BINS: f32 = 0.4;
const FIT_LIMIT_HZ: f32 = 4000.0;
/// Partials up to this harmonic fix the fundamental; higher ones may be stretched.
const FIX_HARMONICS: usize = 8;
/// Share of a note's partials the tight fit must follow, and how far, on average, they may
/// sit from where they are expected, in bins. Beside a note found already in the same frame
/// a note's partials are pulled aside by that note's nearby partials, so the fit looks a
/// little wider and asks for less; noise alone never gets that far.
const LINE_UP_SHARE: f32 = 0.55;
const RESIDUAL_BINS: f32 = 0.25;
const BESIDE_BINS: f32 = 0.7;
const BESIDE_SHARE: f32 = 0.45;
const BESIDE_RESIDUAL_BINS: f32 = 0.4;
/// Least mean strength of the partials a note's tight fit follows: 12 dB of prominence,
/// less for low notes, whose partials sit closer than [`CLEAR_SPACING_BINS`] and cannot
/// dig valleys as deep between them.
const CLEAR: f32 = 0.4;
const CLEAR_SPACING_BINS: f32 = 6.0;
/// The stiffest string expected, as its inharmonicity coefficient `B`: high piano strings
/// reach about this. A stretch is only fitted once this many partials are in.
const MAX_STIFFNESS: f64 = 0.004;
const STRETCH_PARTIALS: usize = 4;

/// Harmonics are followed and protected up to this frequency.
const PROTECT_LIMIT_HZ: f32 = 16_000.0;
const MAX_HARMONICS: usize = 272;
/// How far a harmonic may stand above nearly all of its neighbours, in dB, before any of it
/// may be cut.
const MARGIN_DB: f32 = 3.0;
/// How fast a note's harmonics are taken to fall, in dB per octave: as 1/h, about as most
/// notes' do.
const HARMONIC_TILT_DB: f32 = 6.020_6;
/// Bins either side of a harmonic's peak that share its protection: its main lobe.
const LOBE: usize = 2;

#[derive(Debug, Clone, Copy, Default)]
struct Peak {
    bin: u16,
    /// Exact frequency of the maximum, in Hz.
    hz: f32,
    /// Level in dB, and strength from 0 to 1.
    level: f32,
    strength: f32,
}

/// Fundamentals of the notes found in one frame, in Hz.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Notes {
    pub(super) count: usize,
    pub(super) fundamentals: [f32; MAX_NOTES],
}

impl Notes {
    pub(super) fn found(&self) -> &[f32] {
        &self.fundamentals[..self.count]
    }

    fn push(&mut self, fundamental: f32) {
        if self.count < MAX_NOTES {
            self.fundamentals[self.count] = fundamental;
            self.count += 1;
        }
    }

    fn has(&self, fundamental: f32) -> bool {
        self.found()
            .iter()
            .any(|&other| (fundamental / other - 1.0).abs() < CONFIRM)
    }
}

/// Finds notes and their harmonics in a frame's power spectrum, without allocating.
#[derive(Debug)]
pub(super) struct Tracker {
    levels: Box<[f32]>,
    /// Strength of the peak at each bin, zero elsewhere. Peaks of notes already found are
    /// taken out.
    strength: Box<[f32]>,
    /// Strengths spread over one and two bins either side for tolerant look-ups: of the
    /// peaks still in the running, and of every peak in the frame.
    near: [Box<[f32]>; 2],
    near_all: [Box<[f32]>; 2],
    /// Running sum of `strength`, for the total peak strength in a range.
    total: Box<[f32]>,
    /// Index of the peak at each bin, plus one; zero where there is none.
    peak_at: Box<[u16]>,
    peaks: Box<[Peak]>,
    peak_count: usize,
    /// Scratch for the peak search: local maxima and the valleys before them.
    maxima: Box<[u16]>,
    valleys: Box<[f32]>,
    /// For every candidate and ranked harmonic: the bin of the harmonic, its weight, and
    /// whether to look two bins wide. A zero bin ends a candidate's list.
    harmonic: Box<[u16]>,
    weight: Box<[f32]>,
    wide: Box<[bool]>,
    /// The bins a candidate's harmonics range over: half a fundamental either side.
    range: Box<[(u16, u16)]>,
    fundamentals: Box<[f32]>,
    /// Where each harmonic of the note being followed was expected, and the peak, plus
    /// one, matched to it.
    expected: Box<[f32]>,
    matched: Box<[u16]>,
    /// Width of a bin in Hz, the Nyquist bin, and the number of bins.
    bin_hz: f32,
    half: usize,
    bins: usize,
}

fn bin_of(hz: f32, bin_hz: f32, half: usize) -> usize {
    ((hz / bin_hz).round() as usize).clamp(1, half - 1)
}

/// Spreads peak strengths over one and two bins either side.
fn dilate(strength: &[f32], near: &mut [Box<[f32]>; 2]) {
    let [one, two] = near;
    let bins = strength.len();
    for k in 0..bins {
        let (from, to) = (k.saturating_sub(1), (k + 1).min(bins - 1));
        one[k] = strength[from..=to]
            .iter()
            .fold(0.0, |most, &value| value.max(most));
    }
    for k in 0..bins {
        let (from, to) = (k.saturating_sub(1), (k + 1).min(bins - 1));
        two[k] = one[from..=to]
            .iter()
            .fold(0.0, |most, &value| value.max(most));
    }
}

/// The partials of a note so far, fitted to a stiff string's stretch: the `h`-th partial of
/// a string of fundamental `f0` and stiffness `B` lies near `h·f0·(1 + B·h²/2)`, so its
/// frequency over `h` is a straight line in `h²`. A weighted least-squares fit of that line
/// predicts the next partial, follows a stiff string's stretch, and shrugs off any one
/// partial pulled aside by a neighbour.
#[derive(Debug, Clone, Copy)]
struct Stretch {
    fundamental: f64,
    weight: f64,
    x: f64,
    y: f64,
    xx: f64,
    xy: f64,
    count: usize,
}

impl Stretch {
    fn new(fundamental: f32) -> Self {
        Self {
            fundamental: f64::from(fundamental),
            weight: 0.0,
            x: 0.0,
            y: 0.0,
            xx: 0.0,
            xy: 0.0,
            count: 0,
        }
    }

    fn add(&mut self, harmonic: usize, hz: f32, strength: f32) {
        let number = harmonic as f64;
        let (x, y, weight) = (number * number, f64::from(hz) / number, f64::from(strength));
        self.weight += weight;
        self.x += weight * x;
        self.y += weight * y;
        self.xx += weight * x * x;
        self.xy += weight * x * y;
        self.count += 1;
    }

    /// The fitted fundamental and stretch, `f0·B/2`. The stretch needs a few partials to go
    /// on, and strings only ever stretch, up to the stiffest a string gets.
    fn line(&self) -> (f64, f64) {
        if self.count == 0 {
            return (self.fundamental, 0.0);
        }
        let mean = self.y / self.weight;
        if self.count < STRETCH_PARTIALS {
            return (mean, 0.0);
        }
        let spread = self.weight * self.xx - self.x * self.x;
        if spread <= 1.0e-9 * self.weight * self.xx {
            return (mean, 0.0);
        }
        let slope = ((self.weight * self.xy - self.x * self.y) / spread)
            .clamp(0.0, 0.5 * MAX_STIFFNESS * mean);
        ((self.y - slope * self.x) / self.weight, slope)
    }

    /// Where the `harmonic`-th partial is expected, and the spacing of the partials there.
    fn predict(&self, harmonic: usize) -> (f32, f32) {
        let (fundamental, slope) = self.line();
        let number = harmonic as f64;
        let x = number * number;
        (
            (number * (fundamental + slope * x)) as f32,
            (fundamental + 3.0 * slope * x) as f32,
        )
    }
}

impl Tracker {
    pub(super) fn new(layout: &Layout) -> Self {
        let (bin_hz, half, bin_count) = (layout.bin_hz() as f32, layout.half(), layout.bins());
        let bin_of = |hz: f32| bin_of(hz, bin_hz, half);
        let slots = CANDIDATES * SCORED;
        let mut harmonic = vec![0; slots].into_boxed_slice();
        let mut weight = vec![0.0; slots].into_boxed_slice();
        let mut wide = vec![false; slots].into_boxed_slice();
        let mut range = vec![(0, 0); CANDIDATES].into_boxed_slice();
        let fundamentals: Box<[f32]> = (0..CANDIDATES)
            .map(|candidate| LOWEST_HZ * (candidate as f32 / PER_OCTAVE).exp2())
            .collect();
        for (candidate, &fundamental) in fundamentals.iter().enumerate() {
            let mut top = fundamental;
            for index in 0..SCORED {
                let hz = (index + 1) as f32 * fundamental;
                if hz > SCORE_LIMIT_HZ {
                    break;
                }
                let slot = candidate * SCORED + index;
                harmonic[slot] = bin_of(hz) as u16;
                weight[slot] = (fundamental + ALPHA_HZ) / (hz + BETA_HZ);
                wide[slot] = hz > WIDE_ABOVE_HZ;
                top = hz;
            }
            range[candidate] = (
                bin_of(0.5 * fundamental) as u16,
                bin_of(top + 0.5 * fundamental) as u16,
            );
        }
        let bins = || vec![0.0; bin_count].into_boxed_slice();
        Self {
            levels: bins(),
            strength: bins(),
            near: std::array::from_fn(|_| bins()),
            near_all: std::array::from_fn(|_| bins()),
            total: vec![0.0; bin_count + 1].into_boxed_slice(),
            peak_at: vec![0; bin_count].into_boxed_slice(),
            peaks: vec![Peak::default(); half / 2 + 1].into_boxed_slice(),
            peak_count: 0,
            maxima: vec![0; half].into_boxed_slice(),
            valleys: vec![0.0; half].into_boxed_slice(),
            harmonic,
            weight,
            wide,
            range,
            fundamentals,
            expected: vec![0.0; MAX_HARMONICS + 3].into_boxed_slice(),
            matched: vec![0; MAX_HARMONICS + 3].into_boxed_slice(),
            bin_hz,
            half,
            bins: bin_count,
        }
    }

    /// The bin nearest `hz`, kept off DC and Nyquist.
    fn bin(&self, hz: f32) -> usize {
        bin_of(hz, self.bin_hz, self.half)
    }

    /// Finds the notes in a frame's power spectrum and writes how many dB of cut Focus
    /// allows at every bin: infinity away from the notes, zero on an ordinary harmonic.
    /// `heard` holds what the previous search found and is updated; only notes found by
    /// both count, and those are returned.
    pub(super) fn track(&mut self, power: &[f32], heard: &mut Notes, allowed: &mut [f32]) -> Notes {
        self.find_peaks(&power[..self.bins]);
        dilate(&self.strength, &mut self.near_all);
        let mut found = Notes::default();
        let mut attempts = 0;
        // Each round ranks the candidates once, against the peaks still in the running, and
        // tries the best of them in turn until one is a note.
        'rounds: while found.count < MAX_NOTES && attempts < ATTEMPTS {
            self.spread();
            let (ranked, count) = self.ranked();
            for &candidate in &ranked[..count] {
                if attempts == ATTEMPTS {
                    break 'rounds;
                }
                if let Some(fundamental) =
                    self.refine(self.fundamentals[candidate], found.count > 0)
                    && !found.has(fundamental)
                {
                    found.push(fundamental);
                    self.take_out(fundamental);
                    continue 'rounds;
                }
                attempts += 1;
            }
            break;
        }
        let allowed = &mut allowed[..self.bins];
        allowed.fill(f32::INFINITY);
        let mut confirmed = Notes::default();
        for &fundamental in found.found() {
            if heard.has(fundamental) {
                confirmed.push(fundamental);
                self.protect(fundamental, allowed);
            }
        }
        *heard = found;
        confirmed
    }

    fn find_peaks(&mut self, power: &[f32]) {
        for (level, &power) in self.levels.iter_mut().zip(power) {
            *level = DB_PER_LOG2 * log2(power.max(POWER_FLOOR));
        }
        self.strength.fill(0.0);
        self.peak_at.fill(0);
        self.peak_count = 0;
        let levels = &self.levels;
        // Each maximum's valley before it is the lowest level since the maximum before.
        let mut count = 0;
        let mut lowest = levels[0];
        for k in 1..self.half {
            let level = levels[k];
            if level >= levels[k - 1] && level > levels[k + 1] {
                self.maxima[count] = k as u16;
                self.valleys[count] = lowest;
                count += 1;
                lowest = level;
            } else {
                lowest = lowest.min(level);
            }
        }
        let last_valley = lowest.min(levels[self.half]);
        for index in 0..count {
            let k = usize::from(self.maxima[index]);
            let after = if index + 1 < count {
                self.valleys[index + 1]
            } else {
                last_valley
            };
            let prominence = levels[k] - self.valleys[index].max(after);
            if prominence < PEAK_MIN_DB {
                continue;
            }
            let (before, at, next) = (levels[k - 1], levels[k], levels[k + 1]);
            let curvature = before - 2.0 * at + next;
            let offset = if curvature < -1.0e-6 {
                (0.5 * (before - next) / curvature).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let strength = (prominence / STRENGTH_FULL_DB).min(1.0);
            self.peaks[self.peak_count] = Peak {
                bin: k as u16,
                hz: (k as f32 + offset) * self.bin_hz,
                level: at - 0.25 * (before - next) * offset,
                strength,
            };
            self.peak_count += 1;
            self.peak_at[k] = self.peak_count as u16;
            self.strength[k] = strength;
        }
    }

    /// Spreads and sums up the strengths of the peaks still in the running.
    fn spread(&mut self) {
        let mut sum = 0.0;
        self.total[0] = 0.0;
        for (k, &strength) in self.strength.iter().enumerate() {
            sum += strength;
            self.total[k + 1] = sum;
        }
        dilate(&self.strength, &mut self.near);
    }

    /// The best few candidate fundamentals worth trying, best first, and how many there are.
    fn ranked(&self) -> ([usize; ATTEMPTS], usize) {
        let mut best = [(0.0_f32, 0_usize); ATTEMPTS];
        let mut count = 0;
        for candidate in 0..CANDIDATES {
            let Some(score) = self.score(candidate) else {
                continue;
            };
            if count == ATTEMPTS && score <= best[ATTEMPTS - 1].0 {
                continue;
            }
            let mut slot = count.min(ATTEMPTS - 1);
            while slot > 0 && best[slot - 1].0 < score {
                best[slot] = best[slot - 1];
                slot -= 1;
            }
            best[slot] = (score, candidate);
            count = (count + 1).min(ATTEMPTS);
        }
        (best.map(|(_, candidate)| candidate), count)
    }

    /// How well a candidate fundamental explains the peaks still in the running, if it is
    /// worth trying at all. Whether its harmonics are there counts every peak, so a note
    /// that shares partials with one found before it is not starved of them.
    fn score(&self, candidate: usize) -> Option<f32> {
        let (mut weighted, mut explained) = (0.0, 0.0);
        let (mut ranked, mut present) = (0_usize, 0_usize);
        for slot in candidate * SCORED..(candidate + 1) * SCORED {
            let bin = usize::from(self.harmonic[slot]);
            if bin == 0 {
                break;
            }
            let wide = usize::from(self.wide[slot]);
            let strength = self.near[wide][bin];
            weighted += self.weight[slot] * strength;
            explained += strength;
            ranked += 1;
            if self.near_all[wide][bin] >= PRESENT {
                present += 1;
            }
        }
        if ranked == 0 || explained <= 0.0 {
            return None;
        }
        let (from, to) = self.range[candidate];
        let total = self.total[usize::from(to) + 1] - self.total[usize::from(from)];
        let share = (explained / total.max(1.0e-6)).min(1.0);
        let coverage = present as f32 / ranked as f32;
        (present >= MIN_HARMONICS && coverage >= COVERAGE && share >= EXPLAINED)
            .then_some(weighted * share * coverage)
    }

    /// The strongest peak whose exact frequency lies within `reach` bins of `hz`, as an
    /// index into `peaks`, among those still in the running or among all of them.
    fn peak_near(&self, hz: f32, reach: f32, running: bool) -> Option<usize> {
        let centre = hz / self.bin_hz;
        // A peak's bin is within half a bin of its exact frequency.
        let from = ((centre - reach - 0.5).floor().max(1.0)) as usize;
        let to = ((centre + reach + 0.5).ceil() as usize).min(self.half - 1);
        let mut best: Option<(f32, usize)> = None;
        for k in from..=to {
            let index = usize::from(self.peak_at[k]);
            if index == 0 {
                continue;
            }
            let peak = self.peaks[index - 1];
            if (peak.hz - hz).abs() > reach * self.bin_hz {
                continue;
            }
            let strength = if running {
                self.strength[k]
            } else {
                peak.strength
            };
            if strength > 0.0 && best.is_none_or(|(most, _)| strength > most) {
                best = Some((strength, index - 1));
            }
        }
        best.map(|(_, index)| index)
    }

    /// Fits a fundamental near `rough` to the exact frequencies of its lowest harmonic peaks.
    fn fit(&self, rough: f32) -> Option<f32> {
        let (mut sum, mut norm) = (0.0, 0.0);
        for harmonic in 1..=LOOSE_HARMONICS {
            let hz = harmonic as f32 * rough;
            let reach = (LOOSE_SHARE * hz / self.bin_hz).max(LOOSE_BINS);
            if let Some(index) = self.peak_near(hz, reach, false) {
                let peak = self.peaks[index];
                let number = harmonic as f32;
                sum += peak.strength * number * peak.hz;
                norm += peak.strength * number * number;
            }
        }
        (norm > 0.0).then(|| sum / norm)
    }

    /// The fundamental of the note near `rough`, if its partials line up like a note's.
    /// `beside` says a note has been found in the frame already. Partials a note shares
    /// with notes found before it still count.
    fn refine(&self, rough: f32, beside: bool) -> Option<f32> {
        let loose = self.fit(rough)?;
        // Beside a note found already, partials are pulled aside by its partials nearby.
        let (tight_bins, line_up_share, residual_bins) = if beside {
            (BESIDE_BINS, BESIDE_SHARE, BESIDE_RESIDUAL_BINS)
        } else {
            (TIGHT_BINS, LINE_UP_SHARE, RESIDUAL_BINS)
        };
        let mut stretch = Stretch::new(loose);
        let (mut matched, mut looked, mut odd, mut fresh) = (0, 0, 0, 0);
        let (mut off, mut weight) = (0.0, 0.0);
        let (mut sum, mut norm) = (0.0, 0.0);
        for harmonic in 1..=TIGHT_HARMONICS {
            let (expected, _) = stretch.predict(harmonic);
            if expected > FIT_LIMIT_HZ {
                break;
            }
            looked += 1;
            let reach = (TIGHT_SHARE * expected / self.bin_hz).max(tight_bins);
            let Some(index) = self.peak_near(expected, reach, false) else {
                continue;
            };
            let peak = self.peaks[index];
            matched += 1;
            odd += harmonic % 2;
            if self.strength[usize::from(peak.bin)] > 0.0 {
                fresh += 1;
            }
            off += peak.strength * (peak.hz - expected).abs();
            weight += peak.strength;
            // Only a partial that lands close joins the fit, as when following.
            if (peak.hz - expected).abs() <= 0.5 * reach * self.bin_hz {
                stretch.add(harmonic, peak.hz, peak.strength);
            }
            if harmonic <= FIX_HARMONICS {
                let number = harmonic as f32;
                sum += peak.strength * number * peak.hz;
                norm += peak.strength * number * number;
            }
        }
        // A further note must bring partials of its own, not just borrow those of the notes
        // found before it.
        let line_up = fresh >= MIN_HARMONICS
            && matched as f32 >= line_up_share * looked as f32
            && 3 * odd >= matched - odd;
        let clear_enough = CLEAR * (loose / self.bin_hz / CLEAR_SPACING_BINS).min(1.0);
        let clear = weight >= clear_enough * matched as f32;
        let residual = off / weight.max(1.0e-6) / self.bin_hz;
        (line_up && clear && residual <= residual_bins && norm > 0.0).then(|| sum / norm)
    }

    /// Follows the harmonics of the note at `fundamental` up to `limit` Hz, each expected
    /// where the stretch fitted to the partials found so far puts it, and records where each
    /// was expected and the peak (plus one) matched to it. Only a partial that lands close
    /// to where it was expected joins the fit: one pulled aside by another note's nearby
    /// partial still counts as the note's, but does not lead the rest astray. Returns the
    /// highest harmonic followed.
    fn follow(&mut self, fundamental: f32, limit: f32, running: bool) -> usize {
        let mut stretch = Stretch::new(fundamental);
        let mut highest = 0;
        for harmonic in 1..=MAX_HARMONICS {
            let (expected, spacing) = stretch.predict(harmonic);
            if expected > limit || expected / self.bin_hz > (self.half - LOBE - 1) as f32 {
                break;
            }
            // Tight enough that a note never claims the partials of another note nearby.
            let reach = (0.1 * spacing / self.bin_hz).max(1.0);
            let found = self.peak_near(expected, reach, running);
            self.expected[harmonic] = expected;
            self.matched[harmonic] = found.map_or(0, |index| index as u16 + 1);
            if let Some(index) = found {
                let peak = self.peaks[index];
                if (peak.hz - expected).abs() <= 0.5 * reach * self.bin_hz {
                    stretch.add(harmonic, peak.hz, peak.strength);
                }
            }
            highest = harmonic;
        }
        self.matched[highest + 1] = 0;
        self.matched[highest + 2] = 0;
        highest
    }

    /// Takes the peaks of the note at `fundamental` out of the running.
    fn take_out(&mut self, fundamental: f32) {
        let highest = self.follow(fundamental, PROTECT_LIMIT_HZ, true);
        for harmonic in 1..=highest {
            let matched = usize::from(self.matched[harmonic]);
            if matched > 0 {
                self.strength[usize::from(self.peaks[matched - 1].bin)] = 0.0;
            }
        }
    }

    /// Protects every harmonic of the note at `fundamental` down to its neighbours: the
    /// second loudest of the nearest two partials on each side, each read as if the
    /// partials fell as 1/h, or the loudest when only two are there. A resonance that lifts
    /// two partials next to each other cannot hide behind the other one, while a note whose
    /// partials alternate loud and soft keeps its loud ones. The fundamental is held against
    /// its second and third partials the same way. A partial that makes no peak of its own,
    /// merged into a neighbour's, is protected where it was expected, up to the note's
    /// highest partial that clearly stands out: above a note's last partials, the peaks of
    /// whatever noise lies under them land near where further partials would be expected
    /// as readily as anything.
    fn protect(&mut self, fundamental: f32, allowed: &mut [f32]) {
        let highest = self.follow(fundamental, PROTECT_LIMIT_HZ, false);
        let top = (1..=highest)
            .rev()
            .find(|&harmonic| {
                let matched = usize::from(self.matched[harmonic]);
                matched > 0 && self.peaks[matched - 1].strength >= PRESENT
            })
            .unwrap_or(0);
        let level =
            |matched: u16| (matched > 0).then(|| self.peaks[usize::from(matched) - 1].level);
        for harmonic in 1..=top {
            let matched = self.matched[harmonic];
            let Some(here) = level(matched) else {
                let bin = self.bin(self.expected[harmonic]);
                for limit in &mut allowed[bin - 1..=bin + 1] {
                    *limit = limit.min(0.0);
                }
                continue;
            };
            // Symmetric pairs only, so a note falling faster than 1/h does not pull the
            // level down; the fundamental has only partials above it.
            let pairs = if harmonic == 1 {
                [(2, 3), (0, 0)]
            } else {
                [
                    (harmonic - 1, harmonic + 1),
                    (harmonic.saturating_sub(2), harmonic + 2),
                ]
            };
            let mut around = [0.0; 4];
            let mut count = 0;
            for neighbour in pairs
                .into_iter()
                .filter(|&(below, _)| below >= 1)
                .flat_map(|(below, above)| [below, above])
            {
                if let Some(level) = level(self.matched[neighbour]) {
                    let tilt = HARMONIC_TILT_DB * log2(neighbour as f32 / harmonic as f32);
                    around[count] = level + tilt;
                    count += 1;
                }
            }
            let around = &mut around[..count];
            around.sort_by(f32::total_cmp);
            let envelope = match count {
                0 => here,
                1 | 2 => around[count - 1],
                count => around[count - 2],
            };
            let cut = (here - envelope - MARGIN_DB).max(0.0);
            let bin = usize::from(self.peaks[usize::from(matched) - 1].bin);
            for limit in &mut allowed[bin - LOBE..=bin + LOBE] {
                *limit = limit.min(cut);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectral::Fft;
    use std::f64::consts::TAU;

    const RATE: f64 = 48_000.0;
    const FRAME: usize = 2048;
    const HALF: usize = FRAME / 2;
    const BINS: usize = HALF + 1;

    fn layout() -> Layout {
        Layout::new(RATE)
    }

    fn bin_of(hz: f32) -> usize {
        super::bin_of(hz, (RATE / FRAME as f64) as f32, HALF)
    }

    /// Hann-windowed power spectrum of a frame of `signal`.
    fn spectrum(signal: impl Fn(usize) -> f64) -> Vec<f32> {
        let fft = Fft::new(FRAME);
        let mut re: Vec<f32> = (0..FRAME)
            .map(|n| {
                let window = 0.5 - 0.5 * (TAU * n as f64 / FRAME as f64).cos();
                (signal(n) * window) as f32
            })
            .collect();
        let mut im = vec![0.0; FRAME];
        fft.forward(&mut re, &mut im);
        (0..BINS).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
    }

    /// A note with partials falling as `1/h^tilt`, its fundamental scaled by
    /// `fundamental_gain`.
    fn note(f0: f64, tilt: f64, fundamental_gain: f64) -> impl Fn(usize) -> f64 {
        move |n| {
            let t = n as f64 / RATE;
            (1..)
                .take_while(|&h| f64::from(h) * f0 < 12_000.0)
                .map(|h| {
                    let gain = if h == 1 { fundamental_gain } else { 1.0 };
                    gain * (TAU * f64::from(h) * f0 * t + f64::from(h)).sin()
                        / f64::from(h).powf(tilt)
                })
                .sum::<f64>()
                * 0.1
        }
    }

    fn noise(seed: u32) -> impl Fn(usize) -> f64 {
        move |n| {
            let mut x = (n as u32).wrapping_mul(2_654_435_761) ^ seed;
            x ^= x >> 15;
            x = x.wrapping_mul(2_246_822_519);
            x ^= x >> 13;
            f64::from(x) / f64::from(u32::MAX) - 0.5
        }
    }

    /// Notes and allowed cuts after two searches of the same frame, so they are confirmed.
    fn notes_in(power: &[f32]) -> (Notes, Vec<f32>) {
        let mut tracker = Tracker::new(&layout());
        let mut heard = Notes::default();
        let mut allowed = vec![0.0; BINS];
        tracker.track(power, &mut heard, &mut allowed);
        let notes = tracker.track(power, &mut heard, &mut allowed);
        (notes, allowed)
    }

    fn has(notes: &Notes, f0: f64) -> bool {
        notes
            .found()
            .iter()
            .any(|&found| (f64::from(found) / f0 - 1.0).abs() < 0.003)
    }

    #[test]
    fn single_notes_are_found_to_within_a_few_cents() {
        for f0 in [82.41, 98.0, 147.0, 220.0, 330.0, 523.25, 880.0, 1100.0] {
            for tilt in [0.5, 1.0, 1.5] {
                let (notes, _) = notes_in(&spectrum(note(f0, tilt, 1.0)));
                assert!(
                    notes.count == 1 && has(&notes, f0),
                    "{f0} Hz, tilt {tilt}: {:?}",
                    notes.found()
                );
            }
        }
    }

    #[test]
    fn a_weak_fundamental_does_not_fool_it_an_octave_up() {
        for f0 in [110.0, 196.0, 262.0] {
            let quiet = |n| note(f0, 1.0, 0.03)(n) + 0.002 * noise(9)(n);
            let (notes, _) = notes_in(&spectrum(quiet));
            assert!(has(&notes, f0), "{f0} Hz: {:?}", notes.found());
        }
    }

    #[test]
    fn chords_keep_their_partials_protected() {
        let chords: [&[f64]; 3] = [
            &[261.63, 329.63, 392.0],
            &[110.0, 277.18],
            &[146.83, 220.0, 369.99],
        ];
        for chord in chords {
            let voices: Vec<_> = chord.iter().map(|&f0| note(f0, 1.0, 1.0)).collect();
            let (notes, allowed) =
                notes_in(&spectrum(|n| voices.iter().map(|voice| voice(n)).sum()));
            for &f0 in chord {
                let partials: Vec<usize> = (1..=16)
                    .map(|h| bin_of((f64::from(h) * f0) as f32))
                    .collect();
                let protected = partials
                    .iter()
                    .filter(|&&bin| {
                        allowed[bin - 1..=bin + 1]
                            .iter()
                            .any(|limit| limit.is_finite())
                    })
                    .count();
                assert!(
                    protected * 5 >= partials.len() * 4,
                    "{f0} Hz in {chord:?}: {protected} of {} partials protected; notes {:?}",
                    partials.len(),
                    notes.found()
                );
            }
        }
        // Two notes apart are each named.
        let voices = [note(110.0, 1.0, 1.0), note(277.18, 1.0, 1.0)];
        let (notes, _) = notes_in(&spectrum(|n| voices.iter().map(|voice| voice(n)).sum()));
        assert!(
            has(&notes, 110.0) && has(&notes, 277.18),
            "{:?}",
            notes.found()
        );
    }

    #[test]
    fn noise_and_lone_sines_are_no_notes() {
        // Peaks of noise line up with some candidate's partials once in a long while; the
        // tight fit turns nearly all of those down.
        let mistaken: usize = (0..200)
            .map(|seed| notes_in(&spectrum(noise(seed * 7919))).0.count)
            .sum();
        assert!(
            mistaken <= 6,
            "{mistaken} notes found in 200 frames of noise"
        );
        let (notes, _) = notes_in(&spectrum(|n| (TAU * 1000.0 * n as f64 / RATE).sin()));
        assert_eq!(notes.count, 0);
        let (notes, _) = notes_in(&spectrum(|n| note(220.0, 1.0, 1.0)(n) + 0.05 * noise(3)(n)));
        assert!(
            notes.count == 1 && has(&notes, 220.0),
            "{:?}",
            notes.found()
        );
    }

    #[test]
    fn a_note_counts_only_once_found_twice() {
        let power = spectrum(note(220.0, 1.0, 1.0));
        let mut tracker = Tracker::new(&layout());
        let mut heard = Notes::default();
        let mut allowed = vec![0.0; BINS];
        assert_eq!(tracker.track(&power, &mut heard, &mut allowed).count, 0);
        assert!(allowed.iter().all(|limit| limit.is_infinite()));
        assert_eq!(tracker.track(&power, &mut heard, &mut allowed).count, 1);
    }

    #[test]
    fn ordinary_harmonics_are_fully_protected_and_a_lifted_one_partly() {
        let f0 = 220.0;
        let (_, allowed) = notes_in(&spectrum(note(f0, 1.0, 1.0)));
        for h in 1..=40 {
            let bin = bin_of(h as f32 * f0 as f32);
            assert_eq!(allowed[bin], 0.0, "harmonic {h}");
        }
        assert!(allowed[bin_of(330.0)].is_infinite(), "between harmonics");
        // Lift the 11th harmonic 12 dB, as a resonance would.
        let lifted = move |n: usize| {
            let t = n as f64 / RATE;
            note(f0, 1.0, 1.0)(n) + 0.1 * 3.0 * (TAU * 11.0 * f0 * t + 11.0).sin() / 11.0
        };
        let (_, allowed) = notes_in(&spectrum(lifted));
        let cut = allowed[bin_of(11.0 * f0 as f32)];
        assert!(cut > 7.0 && cut < 10.0, "{cut}");
        assert_eq!(allowed[bin_of(10.0 * f0 as f32)], 0.0);
    }

    #[test]
    fn the_loud_half_of_alternating_harmonics_is_protected() {
        // A clarinet-like tone: odd harmonics falling as 1/h, even ones 20 dB below them.
        let f0 = 196.0;
        let hollow = move |n: usize| {
            let t = n as f64 / RATE;
            (1..50)
                .map(|h| {
                    let level = if h % 2 == 1 { 1.0 } else { 0.1 };
                    let h = f64::from(h);
                    level * (TAU * h * f0 * t + h).sin() / h
                })
                .sum::<f64>()
                * 0.1
        };
        let (notes, allowed) = notes_in(&spectrum(hollow));
        assert!(notes.count == 1 && has(&notes, f0), "{:?}", notes.found());
        for h in 1..=40 {
            let bin = bin_of(h as f32 * f0 as f32);
            assert_eq!(allowed[bin], 0.0, "harmonic {h}");
        }
    }

    #[test]
    fn stretched_partials_are_still_followed() {
        // A stiff string, whose partials stretch as h·f0·√(1 + B·h²).
        let (f0, stretch) = (196.0, 4.0e-4);
        let string = move |n: usize| {
            let t = n as f64 / RATE;
            (1..40)
                .map(|h| {
                    let h = f64::from(h);
                    (TAU * h * f0 * (1.0 + stretch * h * h).sqrt() * t + h).sin() / h
                })
                .sum::<f64>()
                * 0.1
        };
        let (notes, allowed) = notes_in(&spectrum(string));
        // A stiff string has no single exact fundamental; its partials are what matter.
        assert!(
            notes.count == 1 && (f64::from(notes.found()[0]) / f0 - 1.0).abs() < 0.015,
            "{:?}",
            notes.found()
        );
        for h in [20.0, 30.0] {
            let hz = h * f0 * (1.0_f64 + stretch * h * h).sqrt();
            assert_eq!(allowed[bin_of(hz as f32)], 0.0, "partial {h} at {hz:.0} Hz");
        }
    }
}
