pub mod bridge;
pub mod server;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub const DISCOVERY: &str = ".sympoie/agent.json";

pub fn discovery_path(folder: &Path) -> PathBuf {
    folder.join(DISCOVERY)
}

pub fn tools() -> Value {
    json!([
        {
            "name": "get_context",
            "description": "What the person is doing in Sympoie right now: the selected tracks, clip and time range (with their IDs and track files), the playhead and loop, playback state, tempo and metre, every track with its ID, type, name and file, and whether the last outside edit loaded. Call this first, and whenever the person refers to 'this', 'here' or 'the selection'.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "check_project",
            "description": "Load the project's files exactly as Sympoie will and report whether they are valid. On failure the error names the file and field, such as 'tracks/7-bass.json: clips[0].notes[3].key: must be at most 127'. Call it after editing project files. Works even when Sympoie is closed.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "analyze_audio",
            "description": "Render part of the project as it is open in Sympoie (including unsaved edits and plug-ins) and measure it: peak and RMS levels per channel in dBFS, clipped samples, silence, a low/mid/high balance, and the level of each bar. Use it to check your work, since you cannot listen. Defaults to the loop when it is on, otherwise the whole project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "start_tick": {"type": "integer", "minimum": 0, "description": "Start, in ticks (960 per quarter note)."},
                    "end_tick": {"type": "integer", "minimum": 1, "description": "End, in ticks, exclusive."},
                    "tracks": {"type": "array", "items": {"type": "integer"}, "description": "Track IDs to solo; the rest are silent. Leave out to hear the whole mix."},
                    "save_wav": {"type": "string", "description": "Also save the render as a 32-bit float WAV at this path inside the project folder, under renders/, such as 'renders/bass-check.wav'."}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "transport",
            "description": "Control playback so the person can listen: play, pause, stop, move the start marker, or set the loop.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["play", "pause", "stop", "seek", "set_loop", "clear_loop"]},
                    "tick": {"type": "integer", "minimum": 0, "description": "For seek: where the next playback starts."},
                    "start_tick": {"type": "integer", "minimum": 0, "description": "For set_loop."},
                    "end_tick": {"type": "integer", "minimum": 1, "description": "For set_loop, exclusive."}
                },
                "required": ["action"],
                "additionalProperties": false
            }
        }
    ])
}

pub fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut parts = String::new();
    for salt in 0..2_u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |time| time.as_nanos()),
        );
        hasher.write_u32(std::process::id());
        parts.push_str(&format!("{:016x}", hasher.finish()));
    }
    parts
}
