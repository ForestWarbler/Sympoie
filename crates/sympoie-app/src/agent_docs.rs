use std::fmt::Write as _;
use std::path::Path;

use sympoie::builtin::{BuiltinKind, Scale};

const MARKER: &str = "sympoie:generated";

pub fn write(folder: &Path, exe: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    let header = format!(
        "<!-- {MARKER}: Sympoie keeps this file up to date while agent mode is on. \
         Delete this line to keep your own version. -->\n"
    );
    refresh(
        folder,
        "AGENTS.md",
        &format!("{header}{}", agents_md()),
        |text| text.contains(MARKER),
        &mut notes,
    );
    refresh(
        folder,
        "CLAUDE.md",
        &format!(
            "{header}# Sympoie project\n\nThis folder is a Sympoie music project. Its format, \
             tools and workflow are described in AGENTS.md:\n\n@AGENTS.md\n"
        ),
        |text| text.contains(MARKER),
        &mut notes,
    );
    let exe_text = exe.to_string_lossy();
    let folder_text = folder.to_string_lossy();
    let path = folder.join(".mcp.json");
    let mut config = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) if value.is_object() => Some(value),
            _ => {
                notes.push(".mcp.json could not be read, so it was left alone".to_owned());
                None
            }
        },
        Err(_) => Some(serde_json::json!({})),
    };
    if let Some(config) = &mut config {
        let servers = config
            .as_object_mut()
            .expect("checked above")
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(servers) = servers.as_object_mut() {
            servers.insert(
                "sympoie".to_owned(),
                serde_json::json!({
                    "command": exe_text,
                    "args": ["--mcp", folder_text],
                }),
            );
            let text = serde_json::to_string_pretty(config).unwrap_or_default() + "\n";
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                let _ = std::fs::write(&path, text);
            }
        }
    }
    let toml = format!(
        "# {MARKER}: Sympoie keeps this file up to date while agent mode is on.\n\
         # Delete this line to keep your own version.\n\
         [mcp_servers.sympoie]\ncommand = {}\nargs = [\"--mcp\", {}]\n",
        toml_string(&exe_text),
        toml_string(&folder_text)
    );
    let _ = std::fs::create_dir_all(folder.join(".codex"));
    refresh(
        folder,
        ".codex/config.toml",
        &toml,
        |text| text.contains(MARKER),
        &mut notes,
    );
    notes
}

fn refresh(
    folder: &Path,
    name: &str,
    content: &str,
    ours: impl Fn(&str) -> bool,
    notes: &mut Vec<String>,
) {
    let path = folder.join(name);
    match std::fs::read_to_string(&path) {
        Ok(existing) if existing == content => {}
        Ok(existing) if !ours(&existing) => {
            notes.push(format!(
                "{name} was written by someone else, so it was left alone"
            ));
        }
        _ => {
            let _ = std::fs::write(&path, content);
        }
    }
}

fn toml_string(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            character if character.is_control() => {
                let _ = write!(quoted, "\\u{:04X}", character as u32);
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn agents_md() -> String {
    let mut text = String::from(AGENTS_GUIDE);
    text.push_str("\n## Built-in device parameters\n\n");
    text.push_str(
        "Write these in their own units as `\"value\"`, by `id` (the `name` is optional and \
         only for reading). Values outside the range are rejected. Stepped parameters take \
         whole steps; where a parameter's values stand for choices, they are listed.\n",
    );
    for kind in BuiltinKind::ALL {
        let descriptor = kind.descriptor();
        let role = match descriptor.kind {
            sympoie::plugin::PluginKind::Instrument => "instrument",
            sympoie::plugin::PluginKind::Effect => "effect",
        };
        let _ = write!(
            text,
            "\n### {} (`\"builtin\": \"{}\"`, {role})\n\n| id | name | unit | min | max | default | scale or choices |\n| --- | --- | --- | --- | --- | --- | --- |\n",
            kind.name(),
            kind.name().to_ascii_lowercase()
        );
        for spec in sympoie::builtin::parameters(kind) {
            let scale = if let Some(labels) = sympoie::builtin::choices(kind, spec.id) {
                labels
                    .iter()
                    .enumerate()
                    .map(|(index, label)| format!("{} = {label}", spec.min + index as f64))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else if spec.steps > 0 {
                "whole numbers".to_owned()
            } else {
                match spec.scale {
                    Scale::Linear => "linear".to_owned(),
                    Scale::Log => "logarithmic".to_owned(),
                }
            };
            let _ = writeln!(
                text,
                "| {} | {} | {} | {} | {} | {} | {scale} |",
                spec.id, spec.name, spec.unit, spec.min, spec.max, spec.default
            );
        }
    }
    text
}

const AGENTS_GUIDE: &str = r##"# Working on this Sympoie project

This folder is a music project for Sympoie, a digital audio workstation. The
person is working on it in Sympoie while you work on its files. When they
have **agent mode** on (the AGENT switch in Sympoie's status bar), Sympoie:

- saves the folder about a second after they stop editing, so the files
  always show what they see;
- loads your changes about half a second after you stop writing, as one
  "Agent Edit" step they can undo with ⌘Z;
- refuses to load files with errors and shows the error, without
  overwriting your files, until you fix them.

So: read the files, edit them in place, then check your work.

## Tools

The `sympoie` MCP server (configured in `.mcp.json` and `.codex/config.toml`)
gives you:

- `get_context` — what the person has selected, the playhead, the loop,
  tempo, every track's ID, name and file. Call it first, and whenever they
  say "this", "here" or "the selection".
- `check_project` — loads the files exactly as Sympoie does and returns the
  first error with its file and field. Call it after every edit.
- `analyze_audio` — renders a range (optionally soloing tracks) and reports
  peak/RMS levels, clipping, silence, a low/mid/high balance and per-bar
  levels. You cannot listen; measure instead.
- `transport` — play, stop, seek, or set the loop so the person can listen.

## Files

```text
project.json         tempo, metre, grid, ID counters, track order
tracks/<id>-<name>.json   one file per track
samples/*.wav        audio (48 kHz stereo); don't edit
state/*.bin          opaque plug-in state; don't edit
```

`project.json`:

```json
{
  "format": "sympoie-project",
  "version": 1,
  "sample_rate": 48000,
  "ticks_per_quarter": 960,
  "tempo": 120.0,
  "time_signature": [4, 4],
  "snap": "1/16",
  "next_id": 14,
  "next_device_id": 5,
  "tracks": ["tracks/1-master.json", "tracks/2-drums.json", "tracks/7-bass.json"]
}
```

The `tracks` list is the track order; the master is always first. A track
file not listed there is ignored.

A MIDI track file:

```json
{
  "id": 7,
  "type": "midi",
  "name": "Bass",
  "color": "#e0a040",
  "mixer": {"gain": 0.8, "pan": 0.0, "mute": false, "solo": false, "output": "master"},
  "instrument": {"id": 3, "builtin": "oscil", "parameters": [{"id": 20, "value": 1200.0}]},
  "devices": [{"id": 4, "builtin": "comp", "parameters": [{"id": 0, "value": -24.0}]}],
  "clips": [
    {"id": 12, "start_tick": 0, "length_ticks": 3840, "notes": [
      {"start": 0, "length": 480, "key": 36, "velocity": 100},
      {"start": 960, "length": 480, "key": 43}
    ]}
  ]
}
```

Track `type` is `audio`, `midi`, `group`, `bus` or `master`. Audio tracks
have audio clips instead of notes:

```json
{"id": 21, "sample": "samples/loop.wav", "start_tick": 0, "source_start_frame": 0,
 "frame_count": 96000, "gain": 1.0}
```

Any clip can have a `"name"`, shown on the clip in place of its note count
or track name; leave it out for the default label.

## Rules

- **Time is in ticks: 960 per quarter note.** In 4/4 a bar is 3840 ticks,
  an eighth note 480, a sixteenth 240. Bar N starts at (N − 1) × ticks per
  bar. Ticks per bar = 3840 × beats ÷ beat unit (so 6/8 is 2880).
- **Notes are relative to their clip.** A note's `start` counts from the
  clip's content start: a note at `"start": 0` in a clip at
  `"start_tick": 7680` plays at bar 3 in 4/4. `key` is a MIDI note number
  (60 = middle C, 36 = C2), `velocity` 1–127 (default 100), `channel` 0–15
  (default 0). Notes outside `0..length_ticks` (shifted by `offset_ticks`)
  are kept but silent.
- **Slides and pitch bend.** A note's optional `"slide"` (ticks, default 0)
  glides into it from the previous note's pitch over that time without
  restarting its envelope, on instruments that support it (Ferro). A clip's
  optional `"bend"` is a list of `{"tick": T, "value": V}` points in the same
  clip-relative ticks as notes, with strictly increasing ticks; `value` runs
  −8192 to 8191 (0 = no bend, the ends = the instrument's bend range). The
  bend ramps linearly between points and holds before the first and after
  the last; it returns to 0 when the clip ends.
- **Ferro's Engine** (parameter 70): 0 plays the realtime engine; 1 renders
  the track's notes in the background with the slower, more accurate offline
  engine and plays that render. Bounce and export always use the offline
  engine.
- **Slicer slices.** Slices (parameter 50) says how many slices are active;
  slice n (counting from 0) starts at parameter 100 + n, as a fraction of the
  whole sample from 0 to 1, and ends where the next later slice starts. Slice
  n plays on key Root key (parameter 20) + n, so keep starts in ascending
  order to keep the keys in time order. The sample itself is loaded in Sympoie.
- **IDs are permanent.** Keep existing `id`s exactly. Leave `id` out of new
  tracks, clips and devices and Sympoie assigns one. Never reuse an ID.
- **References use track IDs**, never positions: an `output` of
  `{"group": 6}` or `{"bus": 9}`, `"sends": [{"bus": 9, "gain": 0.5}]`, and
  a device's `"sidechain": {"source": 4}`.
- **Levels.** Track `gain` and send `gain` are linear: 1.0 is unity,
  0.5 is about −6 dB, 0 is silent. `pan` is −1 (left) to 1 (right).
- **Automation** lanes hold `[tick, value]` points with values normalized
  0–1. For `"target": "gain"`, 0.5 is unity and 1.0 is +6 dB; for `"pan"`,
  0.5 is centre. A device parameter's target is
  `{"device": ID, "parameter": ID}`, normalized over its range. An
  automated control still works as a trim: a lane's optional `"anchor"` is
  the control value (normalized like the lane) at which the lane plays as
  written. Moving the control away shifts the whole lane by the difference,
  or for `gain` scales it, capped to 0–1. Left out, `anchor` is the
  control's current value, so the lane plays exactly as written.
- **External plug-ins** (`"plugin": {...}`) store `"normalized"` 0–1
  parameter values; their sound also depends on `state`, which you can't
  edit. Prefer built-in devices for anything you need to set exactly.
- **Unknown fields are errors**, so a typo is reported rather than ignored.
  Most fields have defaults and can be left out.
- **To add a track**: write `tracks/<name>.json` (letters, digits, `-`,
  `_`) and add its path to `tracks` in `project.json`. To remove one, take
  it out of the list.
- Don't edit `samples/`, `state/` or `.sympoie/`. A WAV you put in
  `samples/` can be used by an audio clip; it's converted to 48 kHz stereo.
- After editing, call `check_project`, then `analyze_audio` on what you
  changed. If the person has unsaved edits when your change arrives,
  Sympoie asks them which version to keep.
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_guides_and_merges_mcp_config_without_touching_other_files() {
        let folder =
            std::env::temp_dir().join(format!("sympoie-agent-docs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(".mcp.json"),
            r#"{"mcpServers": {"other": {"command": "x"}}}"#,
        )
        .unwrap();
        std::fs::write(folder.join("CLAUDE.md"), "My own notes").unwrap();
        let exe = Path::new("/Applications/Sympoie \"Beta\".app/sympoie");
        let notes = write(&folder, exe);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("CLAUDE.md"));
        assert_eq!(
            std::fs::read_to_string(folder.join("CLAUDE.md")).unwrap(),
            "My own notes"
        );

        let agents = std::fs::read_to_string(folder.join("AGENTS.md")).unwrap();
        assert!(agents.contains("960 per quarter note"));
        assert!(agents.contains("### Oscil (`\"builtin\": \"oscil\"`, instrument)"));
        assert!(agents.contains("| 20 | Cutoff | Hz |"));
        assert!(
            agents.contains("0 = Low-pass, 1 = High-pass, 2 = Band-pass"),
            "{agents}"
        );
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(folder.join(".mcp.json")).unwrap())
                .unwrap();
        assert_eq!(config["mcpServers"]["other"]["command"], "x");
        assert_eq!(
            config["mcpServers"]["sympoie"]["command"],
            exe.to_string_lossy().as_ref()
        );
        assert_eq!(config["mcpServers"]["sympoie"]["args"][0], "--mcp");
        let toml = std::fs::read_to_string(folder.join(".codex/config.toml")).unwrap();
        assert!(toml.contains(r#"command = "/Applications/Sympoie \"Beta\".app/sympoie""#));

        std::fs::write(folder.join("AGENTS.md"), "old <!-- sympoie:generated -->").unwrap();
        write(&folder, exe);
        assert!(
            std::fs::read_to_string(folder.join("AGENTS.md"))
                .unwrap()
                .contains("## Rules")
        );
        std::fs::remove_dir_all(folder).unwrap();
    }
}
