pub mod automation;
pub mod bounce;
pub mod builtin;
pub mod clip;
pub mod instrument;
pub mod json;
pub mod pitch;
pub mod plugin;
pub mod project;
pub mod project_file;
pub mod project_json;
pub mod render;
pub mod time;
pub mod track;
pub mod transport;
pub mod wav;

pub const SAMPLE_RATE: u32 = 48_000;

pub const TICKS_PER_QUARTER: u64 = 960;

pub type Frame = [f32; 2];
