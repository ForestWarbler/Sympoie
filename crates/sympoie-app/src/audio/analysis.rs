use std::path::Path;

use serde_json::{Value, json};
use sympoie::Frame;
use sympoie::project::Project;
use sympoie::render::Renderer;

use crate::plugins::PluginRuntime;

const BLOCK: usize = 4096;
pub const MAX_FRAMES: u64 = 48_000 * 600;
const MAX_SEGMENTS: usize = 128;

/// Renders and measures a range on the calling (worker) thread. `plugins` must come from
/// `PluginRuntime::prepare_offline` on the main thread, which also services and releases it.
pub fn analyze(
    project: &Project,
    plugins: &PluginRuntime,
    start_tick: u64,
    end_tick: u64,
    wav: Option<&Path>,
) -> Result<Value, String> {
    let tempo = project.tempo();
    let start = tempo
        .ticks_to_frames(start_tick)
        .map_err(|error| error.to_string())?;
    let end = tempo
        .ticks_to_frames(end_tick)
        .map_err(|error| error.to_string())?;
    if end <= start {
        return Err("the range is empty; end_tick must be after start_tick".into());
    }
    let frames = end - start;
    if frames > MAX_FRAMES {
        return Err("the range is longer than ten minutes; analyze it in parts".into());
    }
    let _pitch = project
        .prepare_audio_pitch(|| false)
        .map_err(|error| error.to_string())?;
    let mut renderer =
        Renderer::with_plugin_host(project, plugins).map_err(|error| error.to_string())?;

    let bar = project.time_signature().ticks_per_bar();
    let first_bar = start_tick / bar;
    let last_bar = end_tick.div_ceil(bar);
    let bars = (last_bar - first_bar).max(1);
    let per_segment = bars.div_ceil(MAX_SEGMENTS as u64);
    let mut edges = Vec::new();
    let mut tick = first_bar * bar;
    while tick < end_tick {
        let next = (tick + per_segment * bar).min(end_tick);
        edges.push((tick.max(start_tick), next));
        tick = next;
    }
    let edge_frames: Vec<(u64, u64)> = edges
        .iter()
        .map(|&(from, to)| {
            Ok((
                tempo.ticks_to_frames(from)?.max(start) - start,
                tempo.ticks_to_frames(to)?.min(end) - start,
            ))
        })
        .collect::<Result<_, sympoie::time::TimeError>>()
        .map_err(|error| error.to_string())?;

    let mut writer = match wav {
        Some(path) => Some(
            sympoie::wav::FloatWavWriter::new(
                std::io::BufWriter::new(
                    std::fs::File::create(path)
                        .map_err(|error| format!("cannot create {}: {error}", path.display()))?,
                ),
                frames,
            )
            .map_err(|error| error.to_string())?,
        ),
        None => None,
    };
    let mut meter = Meter::default();
    let mut segments = vec![Meter::default(); edges.len()];
    let mut bands = Bands::default();
    let mut block = vec![[0.0; 2]; BLOCK];
    let mut done = 0;
    let mut segment = 0;
    while done < frames {
        let errors = plugins.take_errors();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let count = (frames - done).min(BLOCK as u64) as usize;
        renderer.render_block(start + done, &mut block[..count]);
        if let Some(error) = renderer.latency_error() {
            return Err(error.to_string());
        }
        for (offset, frame) in block[..count].iter().enumerate() {
            let position = done + offset as u64;
            while segment + 1 < edge_frames.len() && position >= edge_frames[segment].1 {
                segment += 1;
            }
            meter.add(*frame);
            if let Some(meter) = segments.get_mut(segment) {
                meter.add(*frame);
            }
            bands.add(*frame);
        }
        if let Some(writer) = &mut writer {
            writer
                .write_frames(&block[..count])
                .map_err(|error| error.to_string())?;
        }
        done += count as u64;
    }
    if let Some(writer) = writer {
        writer.finish().map_err(|error| error.to_string())?;
    }

    let seconds = frames as f64 / f64::from(sympoie::SAMPLE_RATE);
    Ok(json!({
        "start_tick": start_tick,
        "end_tick": end_tick,
        "seconds": round(seconds, 3),
        "peak_db": [db(meter.peak[0]), db(meter.peak[1])],
        "rms_db": [db(meter.rms(0)), db(meter.rms(1))],
        "clipped_samples": meter.clipped,
        "silent": meter.peak[0].max(meter.peak[1]) < 1e-5,
        "bands_rms_db": {
            "low_below_250_hz": db(bands.rms(0)),
            "mid_250_to_4000_hz": db(bands.rms(1)),
            "high_above_4000_hz": db(bands.rms(2)),
        },
        "segments": edges.iter().zip(&segments).map(|(&(from, to), meter)| json!({
            "start_tick": from,
            "end_tick": to,
            "bar": from / bar + 1,
            "peak_db": db(meter.peak[0].max(meter.peak[1])),
            "rms_db": db(((meter.rms(0).powi(2) + meter.rms(1).powi(2)) / 2.0).sqrt()),
        })).collect::<Vec<_>>(),
        "levels": "dBFS; 0 is full scale and -120 means silence. Peaks above 0 clip on export.",
    }))
}

#[derive(Clone, Default)]
struct Meter {
    peak: [f64; 2],
    squares: [f64; 2],
    count: u64,
    clipped: u64,
}

impl Meter {
    fn add(&mut self, frame: Frame) {
        for (channel, &sample) in frame.iter().enumerate() {
            let level = f64::from(sample).abs();
            self.peak[channel] = self.peak[channel].max(level);
            self.squares[channel] += level * level;
            if level > 1.0 {
                self.clipped += 1;
            }
        }
        self.count += 1;
    }

    fn rms(&self, channel: usize) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            (self.squares[channel] / self.count as f64).sqrt()
        }
    }
}

struct Bands {
    low: f64,
    below_high: f64,
    squares: [f64; 3],
    count: u64,
}

impl Default for Bands {
    fn default() -> Self {
        Self {
            low: 0.0,
            below_high: 0.0,
            squares: [0.0; 3],
            count: 0,
        }
    }
}

impl Bands {
    fn add(&mut self, frame: Frame) {
        let rate = f64::from(sympoie::SAMPLE_RATE);
        let coefficient = |hz: f64| 1.0 - (-2.0 * std::f64::consts::PI * hz / rate).exp();
        let mono = (f64::from(frame[0]) + f64::from(frame[1])) / 2.0;
        self.low += coefficient(250.0) * (mono - self.low);
        self.below_high += coefficient(4000.0) * (mono - self.below_high);
        let bands = [self.low, self.below_high - self.low, mono - self.below_high];
        for (square, band) in self.squares.iter_mut().zip(bands) {
            *square += band * band;
        }
        self.count += 1;
    }

    fn rms(&self, band: usize) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            (self.squares[band] / self.count as f64).sqrt()
        }
    }
}

fn db(level: f64) -> f64 {
    if level <= 1e-6 {
        -120.0
    } else {
        round(20.0 * level.log10(), 1)
    }
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}
