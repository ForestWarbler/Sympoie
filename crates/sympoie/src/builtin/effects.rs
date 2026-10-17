use std::f64::consts::TAU;
use std::sync::{Arc, atomic::Ordering};

use crate::instrument::NoteEvent;
use crate::plugin::{PluginInstance, PluginProcessContext, PluginProcessor, PluginSidechainInput};
use crate::{Frame, SAMPLE_RATE};

use super::{ParameterSpec, Scale, Telemetry};

const RATE: f64 = SAMPLE_RATE as f64;
const SMOOTH: f64 = 1.0 / (RATE * 0.010);
const FFT_SIZE: usize = 8192;
const FFT_HOP: usize = 2048;

const fn parameter(
    id: u32,
    name: &'static str,
    unit: &'static str,
    range: (f64, f64),
    default: f64,
    scale: Scale,
    steps: u32,
) -> ParameterSpec {
    ParameterSpec {
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

const fn eq_specs() -> [ParameterSpec; 32] {
    let names = [
        [
            "Band 1 Enable",
            "Band 1 Type",
            "Band 1 Frequency",
            "Band 1 Gain",
            "Band 1 Q",
        ],
        [
            "Band 2 Enable",
            "Band 2 Type",
            "Band 2 Frequency",
            "Band 2 Gain",
            "Band 2 Q",
        ],
        [
            "Band 3 Enable",
            "Band 3 Type",
            "Band 3 Frequency",
            "Band 3 Gain",
            "Band 3 Q",
        ],
        [
            "Band 4 Enable",
            "Band 4 Type",
            "Band 4 Frequency",
            "Band 4 Gain",
            "Band 4 Q",
        ],
        [
            "Band 5 Enable",
            "Band 5 Type",
            "Band 5 Frequency",
            "Band 5 Gain",
            "Band 5 Q",
        ],
        [
            "Band 6 Enable",
            "Band 6 Type",
            "Band 6 Frequency",
            "Band 6 Gain",
            "Band 6 Q",
        ],
    ];
    let frequencies = [60.0, 180.0, 600.0, 2000.0, 6000.0, 12000.0];
    let mut result = [parameter(0, "", "", (0.0, 1.0), 0.0, Scale::Linear, 0); 32];
    let mut band = 0;
    while band < 6 {
        let base = band as u32 * 5;
        let items = [
            parameter(base, names[band][0], "", (0.0, 1.0), 1.0, Scale::Linear, 1),
            parameter(
                base + 1,
                names[band][1],
                "",
                (0.0, 5.0),
                if band == 0 {
                    1.0
                } else if band == 5 {
                    2.0
                } else {
                    0.0
                },
                Scale::Linear,
                5,
            ),
            parameter(
                base + 2,
                names[band][2],
                "Hz",
                (20.0, 20000.0),
                frequencies[band],
                Scale::Log,
                0,
            ),
            parameter(
                base + 3,
                names[band][3],
                "dB",
                (-24.0, 24.0),
                0.0,
                Scale::Linear,
                0,
            ),
            parameter(
                base + 4,
                names[band][4],
                "",
                (0.1, 18.0),
                std::f64::consts::FRAC_1_SQRT_2,
                Scale::Log,
                0,
            ),
        ];
        let mut index = 0;
        while index < 5 {
            result[band * 5 + index] = items[index];
            index += 1;
        }
        band += 1;
    }
    result[30] = parameter(30, "Output", "dB", (-24.0, 24.0), 0.0, Scale::Linear, 0);
    result[31] = parameter(31, "Mix", "%", (0.0, 100.0), 100.0, Scale::Linear, 0);
    result
}

const EQ_PARAMETERS: [ParameterSpec; 32] = eq_specs();
pub fn eq_parameters() -> Vec<ParameterSpec> {
    EQ_PARAMETERS.to_vec()
}

const COMP_PARAMETERS: [ParameterSpec; 11] = [
    parameter(0, "Threshold", "dB", (-60.0, 0.0), -18.0, Scale::Linear, 0),
    parameter(1, "Ratio", ":1", (1.0, 20.0), 4.0, Scale::Log, 0),
    parameter(2, "Knee", "dB", (0.0, 24.0), 6.0, Scale::Linear, 0),
    parameter(3, "Attack", "ms", (0.05, 200.0), 10.0, Scale::Log, 0),
    parameter(4, "Release", "ms", (5.0, 2000.0), 120.0, Scale::Log, 0),
    parameter(5, "Makeup", "dB", (-12.0, 24.0), 0.0, Scale::Linear, 0),
    parameter(6, "Mix", "%", (0.0, 100.0), 100.0, Scale::Linear, 0),
    parameter(7, "Output", "dB", (-24.0, 12.0), 0.0, Scale::Linear, 0),
    parameter(8, "Detector HP", "Hz", (20.0, 1000.0), 20.0, Scale::Log, 0),
    parameter(9, "Detector", "", (0.0, 1.0), 0.0, Scale::Linear, 1),
    parameter(
        10,
        "External Sidechain",
        "",
        (0.0, 1.0),
        0.0,
        Scale::Linear,
        1,
    ),
];
pub fn comp_parameters() -> Vec<ParameterSpec> {
    COMP_PARAMETERS.to_vec()
}

fn physical_values<const N: usize>(
    instance: &PluginInstance,
    specs: &[ParameterSpec; N],
) -> [f64; N] {
    std::array::from_fn(|i| {
        instance
            .parameters
            .iter()
            .find(|p| p.id == specs[i].id)
            .map_or(specs[i].default, |p| specs[i].denormalize(p.value))
    })
}

fn amplitude(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}
fn decibels(level: f64) -> f64 {
    20.0 * level.max(1.0e-6).log10()
}
fn clean(sample: f64) -> f64 {
    if sample.is_finite() { sample } else { 0.0 }
}

fn smooth(current: &mut f64, target: f64) {
    let difference = target - *current;
    if difference.abs() < 1.0e-12 {
        *current = target;
    } else {
        *current += SMOOTH * difference;
    }
}

fn output_sample(value: f64) -> f32 {
    clean(value).clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}

#[derive(Debug, Clone, Copy)]
struct Complex {
    re: f64,
    im: f64,
}
impl Complex {
    const ZERO: Self = Self { re: 0.0, im: 0.0 };
    const ONE: Self = Self { re: 1.0, im: 0.0 };
    fn add(self, other: Self) -> Self {
        Self {
            re: self.re + other.re,
            im: self.im + other.im,
        }
    }
    fn sub(self, other: Self) -> Self {
        Self {
            re: self.re - other.re,
            im: self.im - other.im,
        }
    }
    fn mul(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
    fn scaled(self, scale: f64) -> Self {
        Self {
            re: self.re * scale,
            im: self.im * scale,
        }
    }
    fn power(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
    fn div(self, other: Self) -> Self {
        self.mul(Self {
            re: other.re,
            im: -other.im,
        })
        .scaled(1.0 / other.power().max(1.0e-30))
    }
}

#[derive(Debug, Clone, Copy)]
struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}
impl Coefficients {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }
    fn smooth(&mut self, target: Self) {
        smooth(&mut self.b0, target.b0);
        smooth(&mut self.b1, target.b1);
        smooth(&mut self.b2, target.b2);
        smooth(&mut self.a1, target.a1);
        smooth(&mut self.a2, target.a2);
    }
    fn response(self, hz: f64) -> Complex {
        let w = -TAU * hz.clamp(0.0, RATE / 2.0) / RATE;
        let z = Complex {
            re: w.cos(),
            im: w.sin(),
        };
        let z2 = z.mul(z);
        Complex::ONE
            .scaled(self.b0)
            .add(z.scaled(self.b1))
            .add(z2.scaled(self.b2))
            .div(Complex::ONE.add(z.scaled(self.a1)).add(z2.scaled(self.a2)))
    }
}

fn band_coefficients(values: &[f64]) -> Coefficients {
    if values[0] < 0.5 {
        return Coefficients::IDENTITY;
    }
    let kind = values[1].round() as u32;
    let w = TAU * values[2].clamp(20.0, 20000.0) / RATE;
    let cosine = w.cos();
    let alpha = w.sin() / (2.0 * values[4].clamp(0.1, 18.0));
    let a = 10.0_f64.powf(values[3].clamp(-24.0, 24.0) / 40.0);
    let beta = 2.0 * a.sqrt() * alpha;
    match kind {
        1 => Coefficients::new(
            [
                a * (a + 1.0 - (a - 1.0) * cosine + beta),
                2.0 * a * (a - 1.0 - (a + 1.0) * cosine),
                a * (a + 1.0 - (a - 1.0) * cosine - beta),
            ],
            [
                a + 1.0 + (a - 1.0) * cosine + beta,
                -2.0 * (a - 1.0 + (a + 1.0) * cosine),
                a + 1.0 + (a - 1.0) * cosine - beta,
            ],
        ),
        2 => Coefficients::new(
            [
                a * (a + 1.0 + (a - 1.0) * cosine + beta),
                -2.0 * a * (a - 1.0 + (a + 1.0) * cosine),
                a * (a + 1.0 + (a - 1.0) * cosine - beta),
            ],
            [
                a + 1.0 - (a - 1.0) * cosine + beta,
                2.0 * (a - 1.0 - (a + 1.0) * cosine),
                a + 1.0 - (a - 1.0) * cosine - beta,
            ],
        ),
        3 => Coefficients::new(
            [(1.0 + cosine) / 2.0, -(1.0 + cosine), (1.0 + cosine) / 2.0],
            [1.0 + alpha, -2.0 * cosine, 1.0 - alpha],
        ),
        4 => Coefficients::new(
            [(1.0 - cosine) / 2.0, 1.0 - cosine, (1.0 - cosine) / 2.0],
            [1.0 + alpha, -2.0 * cosine, 1.0 - alpha],
        ),
        5 => Coefficients::new(
            [1.0, -2.0 * cosine, 1.0],
            [1.0 + alpha, -2.0 * cosine, 1.0 - alpha],
        ),
        _ => Coefficients::new(
            [1.0 + alpha * a, -2.0 * cosine, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cosine, 1.0 - alpha / a],
        ),
    }
}

pub fn eq_response_db(instance: &PluginInstance, hz: f64) -> f64 {
    EqResponse::new(instance).db(hz)
}

#[derive(Debug, Clone, Copy)]
pub struct EqResponse {
    coefficients: [Coefficients; 6],
    wet: f64,
    output: f64,
}

impl EqResponse {
    pub fn new(instance: &PluginInstance) -> Self {
        let values = physical_values(instance, &EQ_PARAMETERS);
        Self {
            coefficients: std::array::from_fn(|band| {
                band_coefficients(&values[band * 5..band * 5 + 5])
            }),
            wet: (values[31] / 100.0).clamp(0.0, 1.0),
            output: values[30],
        }
    }

    pub fn db(&self, hz: f64) -> f64 {
        if !hz.is_finite() {
            return 0.0;
        }
        let mut response = Complex::ONE;
        for coefficients in self.coefficients {
            response = response.mul(coefficients.response(hz));
        }
        decibels(
            response
                .scaled(self.wet)
                .add(Complex::ONE.scaled(1.0 - self.wet))
                .power()
                .sqrt(),
        ) + self.output
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct FilterState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}
impl FilterState {
    fn process(&mut self, input: f64, c: Coefficients) -> f64 {
        let output =
            c.b0 * input + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = if output.abs() < 1.0e-25 {
            0.0
        } else {
            clean(output)
        };
        self.y1
    }
}

#[derive(Debug)]
struct Analyzer {
    ring: Box<[Frame]>,
    data: Box<[Complex]>,
    window: Box<[f64]>,
    twiddles: Box<[Complex]>,
    reversed: Box<[usize]>,
    ranges: [(usize, usize); 128],
    display: [f64; 128],
    normalization: f64,
    position: usize,
    filled: usize,
    since_fft: usize,
}

impl Analyzer {
    fn new() -> Self {
        let window: Box<[f64]> = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (TAU * i as f64 / FFT_SIZE as f64).cos())
            .collect();
        let sum: f64 = window.iter().sum();
        let ranges = std::array::from_fn(|i| {
            let low = 20.0 * 1000.0_f64.powf((i as f64 - 0.5) / 127.0);
            let high = 20.0 * 1000.0_f64.powf((i as f64 + 0.5) / 127.0);
            let first = (low * FFT_SIZE as f64 / RATE)
                .round()
                .clamp(1.0, FFT_SIZE as f64 / 2.0 - 1.0) as usize;
            let last = (high * FFT_SIZE as f64 / RATE)
                .round()
                .clamp(first as f64, FFT_SIZE as f64 / 2.0 - 1.0) as usize;
            (first, last)
        });
        Self {
            ring: vec![[0.0; 2]; FFT_SIZE].into_boxed_slice(),
            data: vec![Complex::ZERO; FFT_SIZE].into_boxed_slice(),
            window,
            twiddles: (0..FFT_SIZE / 2)
                .map(|i| {
                    let w = -TAU * i as f64 / FFT_SIZE as f64;
                    Complex {
                        re: w.cos(),
                        im: w.sin(),
                    }
                })
                .collect(),
            reversed: (0..FFT_SIZE)
                .map(|i| i.reverse_bits() >> (usize::BITS - FFT_SIZE.trailing_zeros()))
                .collect(),
            ranges,
            display: [-120.0; 128],
            normalization: 1.0 / (sum * sum),
            position: 0,
            filled: 0,
            since_fft: 0,
        }
    }
    fn reset(&mut self) {
        self.ring.fill([0.0; 2]);
        self.display.fill(-120.0);
        self.position = 0;
        self.filled = 0;
        self.since_fft = 0;
    }
    fn push(&mut self, frame: Frame, telemetry: &Telemetry) {
        self.ring[self.position] = frame;
        self.position = (self.position + 1) & (FFT_SIZE - 1);
        self.filled = (self.filled + 1).min(FFT_SIZE);
        self.since_fft += 1;
        if self.filled < FFT_SIZE || self.since_fft < FFT_HOP {
            return;
        }
        self.since_fft = 0;
        for i in 0..FFT_SIZE {
            let frame = self.ring[(self.position + i) & (FFT_SIZE - 1)];
            self.data[self.reversed[i]] = Complex {
                re: f64::from(frame[0]) * self.window[i],
                im: f64::from(frame[1]) * self.window[i],
            };
        }
        let mut size = 2;
        while size <= FFT_SIZE {
            let half = size / 2;
            for base in (0..FFT_SIZE).step_by(size) {
                for j in 0..half {
                    let odd = self.data[base + j + half].mul(self.twiddles[j * FFT_SIZE / size]);
                    let even = self.data[base + j];
                    self.data[base + j] = even.add(odd);
                    self.data[base + j + half] = even.sub(odd);
                }
            }
            size *= 2;
        }
        for (i, &(first, last)) in self.ranges.iter().enumerate() {
            let mut power: f64 = 0.0;
            for k in first..=last {
                power = power.max(
                    (self.data[k].power() + self.data[FFT_SIZE - k].power()) * self.normalization,
                );
            }
            let db = 10.0 * power.max(1.0e-12).log10();
            self.display[i] +=
                (db - self.display[i]) * if db > self.display[i] { 0.85 } else { 0.22 };
            telemetry.spectrum[i].store((self.display[i] as f32).to_bits(), Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Default)]
struct Meters {
    input: f64,
    output: f64,
    reduction: f64,
}
impl Meters {
    fn frame(&mut self, input: [f64; 2], output: [f64; 2], reduction: f64) {
        const DECAY: f64 = 1.0 - 1.0 / (RATE * 0.300);
        self.input = (self.input * DECAY).max(input[0].abs().max(input[1].abs()));
        self.output = (self.output * DECAY).max(output[0].abs().max(output[1].abs()));
        self.reduction = (self.reduction * DECAY).max(reduction);
    }
    fn publish(&self, telemetry: &Telemetry) {
        telemetry
            .input_db
            .store((decibels(self.input) as f32).to_bits(), Ordering::Relaxed);
        telemetry
            .output_db
            .store((decibels(self.output) as f32).to_bits(), Ordering::Relaxed);
        telemetry
            .reduction_db
            .store((self.reduction as f32).to_bits(), Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct EqProcessor {
    values: [f64; 32],
    coefficients: [Coefficients; 6],
    target: [Coefficients; 6],
    states: [[FilterState; 2]; 6],
    gain: f64,
    mix: f64,
    telemetry: Option<Arc<Telemetry>>,
    analyzer: Option<Analyzer>,
    meters: Meters,
}

pub fn create_eq(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    let values = physical_values(instance, &EQ_PARAMETERS);
    let coefficients =
        std::array::from_fn(|band| band_coefficients(&values[band * 5..band * 5 + 5]));
    let analyzer = telemetry.as_ref().map(|_| Analyzer::new());
    Ok(Box::new(EqProcessor {
        values,
        coefficients,
        target: coefficients,
        states: [[FilterState::default(); 2]; 6],
        gain: amplitude(values[30]),
        mix: values[31] / 100.0,
        telemetry,
        analyzer,
        meters: Meters::default(),
    }))
}

impl PluginProcessor for EqProcessor {
    fn reset(&mut self) {
        self.states = [[FilterState::default(); 2]; 6];
        self.coefficients = self.target;
        self.gain = amplitude(self.values[30]);
        self.mix = self.values[31] / 100.0;
        self.meters = Meters::default();
        if let Some(analyzer) = &mut self.analyzer {
            analyzer.reset();
        }
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
            for value in &telemetry.spectrum {
                value.store((-120.0_f32).to_bits(), Ordering::Relaxed);
            }
        }
    }
    fn event(&mut self, _event: NoteEvent) {}
    fn set_parameter(&mut self, id: u32, normalized: f64) {
        if !normalized.is_finite() {
            return;
        }
        if let Some(spec) = EQ_PARAMETERS.get(id as usize) {
            self.values[id as usize] = spec.denormalize(normalized.clamp(0.0, 1.0));
            if id < 30 {
                let band = id as usize / 5;
                self.target[band] = band_coefficients(&self.values[band * 5..band * 5 + 5]);
            }
        }
    }
    fn process(&mut self, frames: &mut [Frame], _context: PluginProcessContext) {
        let gain_target = amplitude(self.values[30]);
        let mix_target = self.values[31] / 100.0;
        for frame in frames {
            let input = [clean(f64::from(frame[0])), clean(f64::from(frame[1]))];
            let mut output = input;
            for band in 0..6 {
                self.coefficients[band].smooth(self.target[band]);
                for (channel, sample) in output.iter_mut().enumerate() {
                    *sample = self.states[band][channel].process(*sample, self.coefficients[band]);
                }
            }
            smooth(&mut self.gain, gain_target);
            smooth(&mut self.mix, mix_target);
            for channel in 0..2 {
                output[channel] = clean(
                    (input[channel] * (1.0 - self.mix) + output[channel] * self.mix) * self.gain,
                );
                frame[channel] = output_sample(output[channel]);
            }
            if let Some(telemetry) = &self.telemetry {
                self.meters.frame(input, output, 0.0);
                if let Some(analyzer) = &mut self.analyzer {
                    analyzer.push(*frame, telemetry);
                }
            }
        }
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }
}

fn reduction_db(input_db: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = input_db - threshold;
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee > 0.0 && over > -knee * 0.5 && over < knee * 0.5 {
        slope * (over + knee * 0.5).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

#[derive(Debug)]
struct CompProcessor {
    target: [f64; 11],
    current: [f64; 11],
    detector_input: [f64; 2],
    detector_output: [f64; 2],
    rms_power: [f64; 2],
    reduction: f64,
    telemetry: Option<Arc<Telemetry>>,
    meters: Meters,
}

pub fn create_comp(
    instance: &PluginInstance,
    telemetry: Option<Arc<Telemetry>>,
) -> Result<Box<dyn PluginProcessor>, String> {
    let target = physical_values(instance, &COMP_PARAMETERS);
    Ok(Box::new(CompProcessor {
        target,
        current: target,
        detector_input: [0.0; 2],
        detector_output: [0.0; 2],
        rms_power: [0.0; 2],
        reduction: 0.0,
        telemetry,
        meters: Meters::default(),
    }))
}

impl CompProcessor {
    fn run(&mut self, frames: &mut [Frame], sidechain: Option<PluginSidechainInput<'_>>) {
        let external = sidechain.filter(|input| input.bus_index == 1);
        for (index, frame) in frames.iter_mut().enumerate() {
            for i in 0..9 {
                smooth(&mut self.current[i], self.target[i]);
            }
            let input = [clean(f64::from(frame[0])), clean(f64::from(frame[1]))];
            let detector = if self.target[10] >= 0.5 {
                external
                    .and_then(|bus| bus.frames.get(index))
                    .copied()
                    .unwrap_or([0.0; 2])
                    .map(|sample| clean(f64::from(sample)))
            } else {
                input
            };
            let hp = (-TAU * self.current[8] / RATE).exp();
            let mut peak: f64 = 0.0;
            for (channel, sample) in detector.into_iter().enumerate() {
                let filtered =
                    hp * (self.detector_output[channel] + sample - self.detector_input[channel]);
                self.detector_input[channel] = sample;
                self.detector_output[channel] = if filtered.abs() < 1.0e-25 {
                    0.0
                } else {
                    filtered
                };
                peak = peak.max(filtered.abs());
                self.rms_power[channel] +=
                    (filtered * filtered - self.rms_power[channel]) / (RATE * 0.010);
                if self.rms_power[channel] < 1.0e-25 {
                    self.rms_power[channel] = 0.0;
                }
            }
            let detected = if self.target[9] >= 0.5 {
                self.rms_power[0].max(self.rms_power[1]).max(0.0).sqrt()
            } else {
                peak
            };
            let desired = reduction_db(
                decibels(detected),
                self.current[0],
                self.current[1],
                self.current[2],
            );
            let time_ms = if desired > self.reduction {
                self.current[3]
            } else {
                self.current[4]
            };
            let envelope = 1.0 - (-1000.0 / (RATE * time_ms)).exp();
            self.reduction += envelope * (desired - self.reduction);
            if self.reduction < 1.0e-12 {
                self.reduction = 0.0;
            }
            let gain = amplitude(self.current[5] - self.reduction);
            let mix = self.current[6] / 100.0;
            let trim = amplitude(self.current[7]);
            let combined = ((1.0 - mix) + mix * gain) * trim;
            let output = [clean(input[0] * combined), clean(input[1] * combined)];
            *frame = [output_sample(output[0]), output_sample(output[1])];
            if self.telemetry.is_some() {
                self.meters.frame(input, output, self.reduction);
            }
        }
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }
}

impl PluginProcessor for CompProcessor {
    fn reset(&mut self) {
        self.current = self.target;
        self.detector_input = [0.0; 2];
        self.detector_output = [0.0; 2];
        self.rms_power = [0.0; 2];
        self.reduction = 0.0;
        self.meters = Meters::default();
        if let Some(telemetry) = &self.telemetry {
            self.meters.publish(telemetry);
        }
    }
    fn event(&mut self, _event: NoteEvent) {}
    fn process(&mut self, frames: &mut [Frame], _context: PluginProcessContext) {
        self.run(frames, None);
    }
    fn process_with_sidechain(
        &mut self,
        frames: &mut [Frame],
        sidechain: Option<PluginSidechainInput<'_>>,
        _context: PluginProcessContext,
    ) {
        self.run(frames, sidechain);
    }
    fn set_parameter(&mut self, id: u32, normalized: f64) {
        if normalized.is_finite()
            && let Some(spec) = COMP_PARAMETERS.get(id as usize)
        {
            self.target[id as usize] = spec.denormalize(normalized.clamp(0.0, 1.0));
        }
    }
}

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;
