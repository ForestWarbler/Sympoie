//! The processor: framing, analysis, filtering and the overlap-add back to audio.

use std::sync::Arc;

use crate::clean;
use crate::detector::{self, MAX_REDUCTION_DB, Settings, Shape, Spare};
use crate::display::{Cuts, Display};
use crate::layout::Layout;
use crate::params::{
    ATTACK, BAND_BASE, CLEAN, COUNT, DELTA, DEPTH, FOCUS, HIGH_CUT, KNEE, LOW_CUT, MIX, OUTPUT,
    PARAMETERS, RELEASE, STEREO, Sensitivity, THRESHOLD, WIDTH, amplitude, decibels,
};
use crate::pitch;
use crate::spectral::{self, Fft, MinimumPhase};
use crate::telemetry::{self, Telemetry};

/// Frames quieter than this (about −150 dBFS) are not analysed.
const SILENT_PEAK: f32 = 3.0e-8;
/// Inputs are clamped far above full scale so the spectra stay finite.
const INPUT_LIMIT: f32 = 1.0e9;
/// log2 of the linear gain per dB of cut.
const LOG2_GAIN_PER_DB: f32 = -0.166_096_4;
/// How much louder, as a power ratio, the newest two hops must be than the frame before
/// them to count as an attack, and how many hops Clean then holds off: long enough for the
/// analysis window to hear the attack itself.
const ATTACK_RATIO: f32 = 4.0;
const ATTACK_HOLD_HOPS: u32 = 6;
/// Hops between note searches while Focus or Clean is up (about 5 ms).
const TRACK_HOPS: u32 = 2;
/// How fast a cut lets go of a harmonic once it belongs to a note, in ms, so a new note
/// is free of the cuts its harmonics fell under before it began.
const SPARE_RELEASE_MS: f64 = 5.0;
/// Smoothing of Output and Delta, and how long the meters hold their peaks, in seconds.
const SMOOTH_SECONDS: f64 = 0.010;
const METER_SECONDS: f64 = 0.300;

/// The cut, in dB, that a reduction of `cut` dB becomes once Mix blends it with unity.
pub(crate) fn mixed_cut(cut: f32, mix: f32) -> f32 {
    -decibels(f64::from(
        1.0 - mix * (1.0 - (cut * LOG2_GAIN_PER_DB).exp2()),
    )) as f32
}

/// Peak input and output levels and the deepest cut, held and falling back slowly.
#[derive(Debug, Default)]
struct Meters {
    input: f64,
    output: f64,
    reduction: f64,
}

impl Meters {
    fn frame(&mut self, decay: f64, input: [f32; 2], output: [f32; 2], reduction: f32) {
        let peak = |frame: [f32; 2]| f64::from(frame[0].abs().max(frame[1].abs()));
        self.input = (self.input * decay).max(peak(input));
        self.output = (self.output * decay).max(peak(output));
        self.reduction = (self.reduction * decay).max(f64::from(reduction));
    }

    fn publish(&self, telemetry: &Telemetry) {
        telemetry::set(&telemetry.input_db, decibels(self.input) as f32);
        telemetry::set(&telemetry.output_db, decibels(self.output) as f32);
        telemetry::set(&telemetry.reduction_db, self.reduction as f32);
    }
}

/// Reson running at one sample rate on a stereo signal.
#[derive(Debug)]
pub struct Reson {
    layout: Layout,
    values: [f64; COUNT],
    pub(crate) fft: Fft,
    minimum: MinimumPhase,
    pub(crate) shape: Shape,
    scratch: detector::Scratch,
    weights: Box<[f32]>,
    pub(crate) weights_stale: bool,
    /// The last frame of input samples of each channel; `write` is the oldest.
    history: [Box<[f32]>; 2],
    write: usize,
    /// Loudest sample and energy of each hop in `history`, both channels together, gathered
    /// as the samples arrive; `hops[newest]` is the hop just completed.
    hops: Box<[(f32, f32)]>,
    newest: usize,
    /// The loudest sample and energy of the hop being filled.
    filling: (f32, f32),
    /// Samples taken since the last frame.
    fill: usize,
    /// Overlap-add accumulators for the two hops a frame writes.
    overlap: [Box<[f32]>; 2],
    /// The hop being played out: processed and dry, both a latency behind the input.
    wet: [Box<[f32]>; 2],
    dry: [Box<[f32]>; 2],
    re: Box<[f32]>,
    im: Box<[f32]>,
    /// Unpacked spectra of the two analysed channels (left/right or mid/side).
    spectra: [[Box<[f32]>; 2]; 2],
    pub(crate) power: [Box<[f32]>; 2],
    goal: Box<[f32]>,
    pub(crate) reduction: [Box<[f32]>; 2],
    gain: [Box<[f32]>; 2],
    phase: [Box<[f32]>; 2],
    /// Finds the notes Focus and Clean spare.
    tracker: pitch::Tracker,
    /// Cut Focus allows at each bin of each analysed channel, in dB; infinite away from the
    /// notes, and the notes themselves.
    allowed: [Box<[f32]>; 2],
    /// Notes each analysed channel's last search found, and those confirmed by the search
    /// before.
    heard: [pitch::Notes; 2],
    notes: [pitch::Notes; 2],
    track_countdown: u32,
    /// Clean's noise floor and cuts for each analysed channel, and the hops left before it
    /// may cut again after an attack.
    pub(crate) cleaners: [clean::Cleaner; 2],
    attack_hold: u32,
    window: Box<[f32]>,
    stereo: u32,
    output: f32,
    delta: f32,
    /// Per-sample share of the way Output and Delta move, and of the way a meter falls.
    smooth: f32,
    meter_decay: f64,
    /// Deepest cut of the last frame, in dB, as heard through Mix.
    pub(crate) deepest: f32,
    telemetry: Option<Arc<Telemetry>>,
    display: Option<Box<Display>>,
    meters: Meters,
}

fn buffer(length: usize) -> Box<[f32]> {
    vec![0.0; length].into_boxed_slice()
}

impl Reson {
    /// Reson at `sample_rate` Hz with every parameter at its default. With `telemetry`, it
    /// publishes its spectra, cuts and meters there for an interface to draw.
    pub fn new(sample_rate: f64, telemetry: Option<Arc<Telemetry>>) -> Self {
        let layout = Layout::new(sample_rate);
        let (frame, hop, bins) = (layout.frame, layout.hop, layout.bins());
        let values = crate::params::defaults();
        let display = telemetry.as_ref().map(|_| Box::new(Display::new(&layout)));
        let mut reson = Self {
            layout,
            values,
            fft: Fft::new(frame),
            minimum: MinimumPhase::new(frame),
            shape: Shape::new(&layout, values[WIDTH as usize] as f32),
            scratch: detector::Scratch::new(&layout),
            weights: buffer(bins),
            weights_stale: true,
            history: [buffer(frame), buffer(frame)],
            write: 0,
            hops: vec![(0.0, 0.0); layout.hops_per_frame()].into_boxed_slice(),
            newest: 0,
            filling: (0.0, 0.0),
            fill: 0,
            overlap: [buffer(2 * hop), buffer(2 * hop)],
            wet: [buffer(hop), buffer(hop)],
            dry: [buffer(hop), buffer(hop)],
            re: buffer(frame),
            im: buffer(frame),
            spectra: std::array::from_fn(|_| [buffer(bins), buffer(bins)]),
            power: [buffer(bins), buffer(bins)],
            goal: buffer(bins),
            reduction: [buffer(bins), buffer(bins)],
            gain: [buffer(bins), buffer(bins)],
            phase: [buffer(bins), buffer(bins)],
            tracker: pitch::Tracker::new(&layout),
            allowed: [buffer(bins), buffer(bins)],
            heard: Default::default(),
            notes: Default::default(),
            track_countdown: 0,
            cleaners: [clean::Cleaner::new(&layout), clean::Cleaner::new(&layout)],
            attack_hold: 0,
            window: (0..2 * hop)
                .map(|i| {
                    let s = (std::f64::consts::PI * i as f64 / (2 * hop) as f64).sin();
                    (s * s) as f32
                })
                .collect(),
            stereo: 0,
            output: 1.0,
            delta: 0.0,
            smooth: (1.0 / (layout.rate * SMOOTH_SECONDS)) as f32,
            meter_decay: 1.0 - 1.0 / (layout.rate * METER_SECONDS),
            deepest: 0.0,
            telemetry,
            display,
            meters: Meters::default(),
        };
        reson.reset();
        reson
    }

    /// How the audio is cut into frames at this sample rate.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Delay of the output behind the input, in samples.
    pub fn latency(&self) -> u32 {
        self.layout.latency()
    }

    /// A parameter's value in physical units.
    pub fn value(&self, id: u32) -> f64 {
        self.values[id as usize]
    }

    /// Sets a parameter in physical units. Values outside its range are clamped, stepped
    /// parameters snap to their nearest step, and anything that is no number or no
    /// parameter is ignored.
    pub fn set_parameter(&mut self, id: u32, value: f64) {
        let Some(spec) = PARAMETERS.get(id as usize) else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        let value = if spec.steps > 0 {
            spec.denormalize(spec.normalize(value))
        } else {
            value.clamp(spec.min, spec.max)
        };
        if self.values[id as usize] != value {
            self.values[id as usize] = value;
            self.weights_stale |=
                id == LOW_CUT || id == HIGH_CUT || (BAND_BASE..FOCUS).contains(&id);
        }
    }

    /// Deepest cut of the last frame, in dB, as heard through Mix.
    pub fn deepest_cut(&self) -> f32 {
        self.deepest
    }

    /// Clears the signal path: buffered audio, cuts and meters. Output and Delta jump to
    /// their settings. Clean's noise floor stays, since it belongs to the material, and so
    /// does the display, which keeps running through the resets Sympoie performs at every
    /// loop wrap.
    pub fn reset(&mut self) {
        for buffers in [
            &mut self.history,
            &mut self.overlap,
            &mut self.wet,
            &mut self.dry,
            &mut self.reduction,
        ] {
            for buffer in buffers {
                buffer.fill(0.0);
            }
        }
        for cleaner in &mut self.cleaners {
            cleaner.clear();
        }
        self.hops.fill((0.0, 0.0));
        self.filling = (0.0, 0.0);
        self.write = 0;
        self.fill = 0;
        self.stereo = self.value(STEREO).round() as u32;
        self.output = amplitude(self.value(OUTPUT)) as f32;
        self.delta = if self.value(DELTA) >= 0.5 { 1.0 } else { 0.0 };
        self.deepest = 0.0;
        self.meters = Meters::default();
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }

    /// Processes interleaved stereo frames in place.
    pub fn process(&mut self, frames: &mut [[f32; 2]]) {
        let (output, delta) = self.output_targets();
        for frame in frames {
            *frame = self.step(*frame, output, delta);
        }
        self.publish_meters();
    }

    /// Processes a stereo signal held as one slice per channel, in place. The slices must
    /// be the same length.
    pub fn process_channels(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (output, delta) = self.output_targets();
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            [*left, *right] = self.step([*left, *right], output, delta);
        }
        self.publish_meters();
    }

    fn output_targets(&self) -> (f32, f32) {
        let output = amplitude(self.value(OUTPUT)) as f32;
        let delta = if self.value(DELTA) >= 0.5 { 1.0 } else { 0.0 };
        (output, delta)
    }

    fn publish_meters(&self) {
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }

    /// Takes one input frame and hands back the output frame a latency behind it.
    #[inline]
    fn step(&mut self, frame: [f32; 2], output: f32, delta: f32) -> [f32; 2] {
        let input = frame.map(|sample| {
            if sample.is_finite() {
                sample.clamp(-INPUT_LIMIT, INPUT_LIMIT)
            } else {
                0.0
            }
        });
        for (history, sample) in self.history.iter_mut().zip(input) {
            history[self.write] = sample;
        }
        self.write = (self.write + 1) & (self.layout.frame - 1);
        let (loudest, energy) = &mut self.filling;
        *loudest = loudest.max(input[0].abs()).max(input[1].abs());
        *energy += input[0] * input[0] + input[1] * input[1];
        self.output += self.smooth * (output - self.output);
        self.delta += self.smooth * (delta - self.delta);
        let result = [0, 1].map(|channel| {
            let wet = self.wet[channel][self.fill];
            let dry = self.dry[channel][self.fill];
            let sample = (wet + self.delta * (dry - 2.0 * wet)) * self.output;
            if sample.is_finite() { sample } else { 0.0 }
        });
        if self.telemetry.is_some() {
            let dry = [self.dry[0][self.fill], self.dry[1][self.fill]];
            self.meters
                .frame(self.meter_decay, dry, result, self.deepest);
        }
        self.fill += 1;
        if self.fill == self.layout.hop {
            self.fill = 0;
            self.newest = (self.newest + 1) % self.hops.len();
            self.hops[self.newest] = std::mem::take(&mut self.filling);
            self.hop();
        }
        result
    }

    fn refresh_weights(&mut self) {
        let sensitivity = Sensitivity::from_values(&self.values);
        for (k, weight) in self.weights.iter_mut().enumerate() {
            *weight = sensitivity.weight(self.layout.hz(k)) as f32;
        }
        self.weights_stale = false;
    }

    pub(crate) fn refresh_tables(&mut self) {
        let width = self.value(WIDTH) as f32;
        if (width - self.shape.width()).abs() > 1.0e-6 {
            self.shape.rebuild(width);
        }
        if self.weights_stale {
            self.refresh_weights();
        }
    }

    fn mix(&self) -> f32 {
        (self.value(MIX) / 100.0).clamp(0.0, 1.0) as f32
    }

    /// Takes one hop: analyses the newest frame, filters it, and stages the next hop of
    /// output.
    fn hop(&mut self) {
        self.refresh_tables();
        let (frame, start) = (self.layout.frame, self.write);
        for (history, buffer) in self.history.iter().zip([&mut self.re, &mut self.im]) {
            buffer[..frame - start].copy_from_slice(&history[start..]);
            buffer[frame - start..].copy_from_slice(&history[..start]);
        }
        // The loudest sample of the frame and of its quietest hop, and the energy of the
        // two hops this frame hands to the output against that of the hops before them.
        let count = self.hops.len();
        let (mut peak, mut quietest, mut earlier) = (0.0_f32, f32::INFINITY, 0.0_f32);
        for &(loudest, energy) in self.hops.iter() {
            peak = peak.max(loudest);
            quietest = quietest.min(loudest);
            earlier += energy;
        }
        let newest = self.hops[self.newest].1 + self.hops[(self.newest + count - 1) % count].1;
        let earlier = (earlier - newest).max(0.0);
        let attack =
            newest / 2.0 > ATTACK_RATIO * earlier / (count - 2) as f32 && peak >= SILENT_PEAK;
        self.attack_hold = if attack {
            ATTACK_HOLD_HOPS
        } else {
            self.attack_hold.saturating_sub(1)
        };
        let stereo = self.value(STEREO).round() as u32;
        if stereo != self.stereo {
            // Coming out of link both channels start from the shared cuts.
            if self.stereo == 0 {
                let (first, second) = self.reduction.split_at_mut(1);
                second[0].copy_from_slice(&first[0]);
                let (first, second) = self.cleaners.split_at_mut(1);
                second[0].copy_from(&first[0]);
            }
            self.stereo = stereo;
            self.track_countdown = 0;
        }
        let filtered = if peak < SILENT_PEAK {
            self.release_silently();
            self.heard = Default::default();
            self.notes = Default::default();
            if let (Some(display), Some(telemetry)) = (&mut self.display, &self.telemetry) {
                display.idle(telemetry);
            }
            false
        } else {
            // A frame that reaches into digital silence, as one does where a clip starts,
            // would teach Clean a floor far below the real one.
            self.analyse_and_filter(quietest >= SILENT_PEAK)
        };
        self.overlap_add(filtered);
    }

    /// Lets every cut release while the input is silent.
    fn release_silently(&mut self) {
        let release = self.layout.hop_rate(self.value(RELEASE));
        self.goal.fill(0.0);
        let mut deepest: f32 = 0.0;
        for state in &mut self.reduction {
            deepest = deepest.max(detector::follow(state, &self.goal, 1.0, release));
        }
        for cleaner in &mut self.cleaners {
            cleaner.release();
        }
        self.deepest = mixed_cut(self.deepest_with_clean(2, deepest), self.mix());
    }

    /// Deepest cut, Clean's included, of the first `channels` analysed channels, in dB,
    /// given the deepest resonance cut among them.
    fn deepest_with_clean(&self, channels: usize, resonance: f32) -> f32 {
        let mut deepest = resonance;
        for (reduction, cleaner) in self.reduction.iter().zip(&self.cleaners).take(channels) {
            if cleaner.cutting() {
                for (&cut, &cleaned) in reduction.iter().zip(cleaner.state.iter()) {
                    deepest = deepest.max(cut + cleaned);
                }
            }
        }
        deepest.min(MAX_REDUCTION_DB)
    }

    /// Runs the detector and, when anything needs cutting, filters the frame in place.
    /// Clean learns its noise floor from the frame if `learn`. Returns whether `re`/`im`
    /// now hold the filtered frame.
    fn analyse_and_filter(&mut self, learn: bool) -> bool {
        let (bins, half) = (self.layout.bins(), self.layout.half());
        self.fft.forward(&mut self.re, &mut self.im);
        let stereo = self.stereo;
        let [[left_re, left_im], [right_re, right_im]] = &mut self.spectra;
        for k in 0..bins {
            let (left, right) = spectral::unpack(&self.re, &self.im, k);
            let (left, right) = if stereo == 2 {
                (
                    [0.5 * (left[0] + right[0]), 0.5 * (left[1] + right[1])],
                    [0.5 * (left[0] - right[0]), 0.5 * (left[1] - right[1])],
                )
            } else {
                (left, right)
            };
            left_re[k] = left[0];
            left_im[k] = left[1];
            right_re[k] = right[0];
            right_im[k] = right[1];
        }
        // Detection uses the Hann-windowed spectrum, which is the rectangular one
        // convolved with (−¼, ½, −¼).
        for (channel, power) in self.power.iter_mut().enumerate() {
            let [re, im] = &self.spectra[channel];
            for (k, power) in power.iter_mut().enumerate() {
                // Across DC and Nyquist the neighbour is the mirror image, a conjugate.
                let (below_re, below_im) = if k == 0 {
                    (re[1], -im[1])
                } else {
                    (re[k - 1], im[k - 1])
                };
                let (above_re, above_im) = if k == half {
                    (re[half - 1], -im[half - 1])
                } else {
                    (re[k + 1], im[k + 1])
                };
                let x = 0.5 * re[k] - 0.25 * (below_re + above_re);
                let y = 0.5 * im[k] - 0.25 * (below_im + above_im);
                *power = x * x + y * y;
            }
        }
        let linked = stereo == 0;
        if linked {
            let (first, second) = self.power.split_at_mut(1);
            for (a, &b) in first[0].iter_mut().zip(second[0].iter()) {
                *a = a.max(b);
            }
        }
        let settings = Settings {
            depth: self.value(DEPTH) as f32,
            threshold: self.value(THRESHOLD) as f32,
            knee: self.value(KNEE) as f32,
        };
        let attack = self.layout.hop_rate(self.value(ATTACK));
        let release = self.layout.hop_rate(self.value(RELEASE));
        let channels = if linked { 1 } else { 2 };
        let focus = (self.value(FOCUS) / 100.0).clamp(0.0, 1.0) as f32;
        let clean = self.value(CLEAN).max(0.0) as f32;
        // Focus and Clean both work from the notes.
        let listening = focus > 0.0 || clean > 0.0;
        let track = listening && self.track_countdown == 0;
        if listening {
            self.track_countdown = if track {
                TRACK_HOPS - 1
            } else {
                self.track_countdown - 1
            };
        } else {
            self.track_countdown = 0;
            self.heard = Default::default();
            self.notes = Default::default();
        }
        let spare_release = self.layout.hop_rate(SPARE_RELEASE_MS);
        let mut deepest: f32 = 0.0;
        for channel in 0..channels {
            if track {
                self.notes[channel] = self.tracker.track(
                    &self.power[channel],
                    &mut self.heard[channel],
                    &mut self.allowed[channel],
                );
            }
            let spare = (focus > 0.0).then(|| Spare {
                allowed: &self.allowed[channel],
                focus,
            });
            detector::reductions(
                &self.power[channel],
                &self.weights,
                &self.shape,
                settings,
                spare,
                &mut self.scratch,
                &mut self.goal,
            );
            let state = &mut self.reduction[channel];
            deepest = deepest.max(detector::follow(state, &self.goal, attack, release));
            if focus > 0.0 {
                // A note's harmonics come out from under old cuts quickly.
                for ((state, &goal), &allowed) in state
                    .iter_mut()
                    .zip(self.goal.iter())
                    .zip(self.allowed[channel].iter())
                {
                    if allowed.is_finite() && *state > goal {
                        *state -= spare_release * (*state - goal);
                    }
                }
            }
            let cleaner = &mut self.cleaners[channel];
            if clean > 0.0 {
                cleaner.update(
                    &self.power[channel],
                    &self.allowed[channel],
                    &self.weights,
                    clean,
                    learn,
                    self.attack_hold > 0,
                );
            } else {
                cleaner.release();
            }
        }
        let deepest = self.deepest_with_clean(channels, deepest);
        let mix = self.mix();
        self.deepest = mixed_cut(deepest, mix);
        let filtered = deepest > 0.0 && mix > 0.0;
        if filtered {
            self.filter(channels, mix);
        }
        if let (Some(display), Some(telemetry)) = (&mut self.display, &self.telemetry) {
            if !linked {
                let (first, second) = self.power.split_at_mut(1);
                for (a, &b) in first[0].iter_mut().zip(second[0].iter()) {
                    *a = a.max(b);
                }
            }
            let second = if linked { 0 } else { 1 };
            let mut notes = [0.0; pitch::MAX_NOTES];
            let mut count = 0;
            for &note in self.notes[0]
                .found()
                .iter()
                .chain(self.notes[second].found())
            {
                if count < notes.len()
                    && !notes[..count]
                        .iter()
                        .any(|&seen| (note / seen - 1.0).abs() < 0.01)
                {
                    notes[count] = note;
                    count += 1;
                }
            }
            let cuts = Cuts {
                resonance: [&self.reduction[0][..], &self.reduction[second][..]],
                cleaning: [
                    &self.cleaners[0].state[..],
                    &self.cleaners[second].state[..],
                ],
                mix,
            };
            display.update(
                telemetry,
                &self.power[0],
                filtered.then_some(cuts),
                listening.then(|| [&self.allowed[0][..], &self.allowed[second][..]]),
                &notes[..count],
            );
        }
        filtered
    }

    /// Builds the minimum-phase filter of each analysed channel from its cuts and applies
    /// it to the frame's spectrum, leaving the filtered frame in `re`/`im`.
    fn filter(&mut self, channels: usize, mix: f32) {
        let (frame, bins, half) = (self.layout.frame, self.layout.bins(), self.layout.half());
        let stereo = self.stereo;
        let linked = channels == 1;
        for channel in 0..channels {
            let (gain, phase) = (&mut self.gain[channel], &mut self.phase[channel]);
            let cleaned = &self.cleaners[channel].state;
            for (((gain, log), &cut), &cleaned) in gain
                .iter_mut()
                .zip(phase.iter_mut())
                .zip(self.reduction[channel].iter())
                .zip(cleaned.iter())
            {
                let cut = (cut + cleaned).min(MAX_REDUCTION_DB);
                let mixed = if mix >= 1.0 {
                    spectral::exp2(cut * LOG2_GAIN_PER_DB)
                } else {
                    1.0 - mix * (1.0 - spectral::exp2(cut * LOG2_GAIN_PER_DB))
                };
                *gain = mixed;
                *log = std::f32::consts::LN_2 * spectral::log2(mixed);
            }
        }
        {
            let [first, second] = &mut self.phase;
            self.minimum.phases(
                &self.fft,
                &mut first[..],
                (!linked).then_some(&mut second[..]),
            );
        }
        if linked {
            let (gain, phase) = (&self.gain[0], &self.phase[0]);
            for k in 0..bins {
                let (sin, cos) = spectral::sin_cos(phase[k]);
                let (hr, hi) = (gain[k] * cos, gain[k] * sin);
                let (zr, zi) = (self.re[k], self.im[k]);
                self.re[k] = zr * hr - zi * hi;
                self.im[k] = zr * hi + zi * hr;
                if k > 0 && k < half {
                    // The mirrored bin takes the conjugate response.
                    let mirror = frame - k;
                    let (zr, zi) = (self.re[mirror], self.im[mirror]);
                    self.re[mirror] = zr * hr + zi * hi;
                    self.im[mirror] = zi * hr - zr * hi;
                }
            }
        } else {
            let [[a_re, a_im], [b_re, b_im]] = &self.spectra;
            for k in 0..bins {
                let response = |channel: usize| {
                    let (sin, cos) = spectral::sin_cos(self.phase[channel][k]);
                    (self.gain[channel][k] * cos, self.gain[channel][k] * sin)
                };
                let multiply = |(xr, xi): (f32, f32), (hr, hi): (f32, f32)| {
                    [xr * hr - xi * hi, xr * hi + xi * hr]
                };
                let first = multiply((a_re[k], a_im[k]), response(0));
                let second = multiply((b_re[k], b_im[k]), response(1));
                let (left, right) = if stereo == 2 {
                    (
                        [first[0] + second[0], first[1] + second[1]],
                        [first[0] - second[0], first[1] - second[1]],
                    )
                } else {
                    (first, second)
                };
                let (left, right) = if k == 0 || k == half {
                    ([left[0], 0.0], [right[0], 0.0])
                } else {
                    (left, right)
                };
                spectral::pack(&mut self.re, &mut self.im, k, left, right);
            }
        }
        self.fft.inverse(&mut self.re, &mut self.im);
    }

    /// Adds this frame's last two hops to the output and stages the hop that is complete.
    fn overlap_add(&mut self, filtered: bool) {
        let (frame, hop) = (self.layout.frame, self.layout.hop);
        let (tail, mask) = (frame - 2 * hop, frame - 1);
        let scale = 1.0 / frame as f32;
        for channel in 0..2 {
            let overlap = &mut self.overlap[channel];
            if filtered {
                let samples = if channel == 0 { &self.re } else { &self.im };
                for ((sum, &window), &sample) in overlap
                    .iter_mut()
                    .zip(self.window.iter())
                    .zip(&samples[tail..])
                {
                    *sum += window * scale * sample;
                }
            } else {
                let history = &self.history[channel];
                for (i, (sum, &window)) in overlap.iter_mut().zip(self.window.iter()).enumerate() {
                    *sum += window * history[(self.write + tail + i) & mask];
                }
            }
            self.wet[channel].copy_from_slice(&overlap[..hop]);
            overlap.copy_within(hop.., 0);
            overlap[hop..].fill(0.0);
            let dry = &mut self.dry[channel];
            let history = &self.history[channel];
            for (i, dry) in dry.iter_mut().enumerate() {
                *dry = history[(self.write + tail + i) & mask];
            }
        }
    }
}
