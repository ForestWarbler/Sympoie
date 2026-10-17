use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{server, tools};

const PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = "Sympoie is a music editor. The project is a folder of JSON files \
(project.json and tracks/*.json); AGENTS.md in the folder explains the format. Edit those files \
directly: while the person has agent mode on, Sympoie loads your changes within a second, as one \
undoable step. Call get_context to see what the person has selected, check_project after editing, \
and analyze_audio to measure the result.";

pub fn project_folder(argument: Option<&str>) -> Option<PathBuf> {
    if let Some(folder) = argument {
        return Some(PathBuf::from(folder));
    }
    let mut directory = std::env::current_dir().ok()?;
    loop {
        if directory.join(sympoie::project_json::MANIFEST).is_file() {
            return Some(directory);
        }
        if !directory.pop() {
            return None;
        }
    }
}

pub fn run(folder: Option<PathBuf>, input: impl BufRead, mut output: impl Write) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = handle(folder.as_deref(), &line) else {
            continue;
        };
        if writeln!(output, "{reply}")
            .and_then(|()| output.flush())
            .is_err()
        {
            break;
        }
    }
}

fn handle(folder: Option<&Path>, line: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => {
            return Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": format!("parse error: {error}")}
            }));
        }
    };
    let id = message.get("id").cloned()?;
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "sympoie", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => Ok(call_tool(folder, &params)),
        _ => Err(json!({"code": -32601, "message": format!("unknown method {method}")})),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    })
}

fn call_tool(folder: Option<&Path>, params: &Value) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let outcome = match folder {
        None => Err(
            "No Sympoie project found. Run this server inside a project folder \
                     (one with project.json), or pass the folder: sympoie --mcp FOLDER"
                .to_owned(),
        ),
        Some(folder) => match name {
            "check_project" => Ok(check(folder)),
            "get_context" | "analyze_audio" | "transport" => server::call(folder, name, &arguments),
            _ => Err(format!("unknown tool {name}")),
        },
    };
    match outcome {
        Ok(value) => json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default()}],
            "structuredContent": value,
            "isError": false,
        }),
        Err(error) => json!({
            "content": [{"type": "text", "text": error}],
            "isError": true,
        }),
    }
}

pub fn check(folder: &Path) -> Value {
    match sympoie::project_json::load_project_folder(folder) {
        Ok(project) => json!({
            "valid": true,
            "tracks": project.tracks().len(),
            "tempo": project.bpm(),
            "message": "The project loads. If agent mode is on, Sympoie has loaded (or is loading) these files.",
        }),
        Err(error) => json!({"valid": false, "error": error.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange(folder: Option<&Path>, messages: &[Value]) -> Vec<Value> {
        let input: String = messages
            .iter()
            .map(|message| format!("{message}\n"))
            .collect();
        let mut output = Vec::new();
        run(folder.map(Path::to_owned), input.as_bytes(), &mut output);
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn speaks_mcp_and_checks_projects_without_the_editor() {
        let folder =
            std::env::temp_dir().join(format!("sympoie-mcp-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let mut project = sympoie::project::Project::new();
        project.add_midi_track("Keys");
        sympoie::project_json::save_project_folder(&project, &folder).unwrap();
        let replies = exchange(
            Some(&folder),
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                                  "clientInfo": {"name": "test", "version": "1"}}}),
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                       "params": {"name": "check_project", "arguments": {}}}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                       "params": {"name": "get_context", "arguments": {}}}),
                json!({"jsonrpc": "2.0", "id": 5, "method": "nonsense"}),
            ],
        );
        assert_eq!(replies.len(), 5, "the notification gets no reply");
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "sympoie");
        let names: Vec<_> = replies[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["get_context", "check_project", "analyze_audio", "transport"]
        );
        assert_eq!(replies[2]["result"]["structuredContent"]["valid"], true);
        assert_eq!(replies[2]["result"]["structuredContent"]["tracks"], 2);
        assert_eq!(replies[3]["result"]["isError"], true);
        assert!(
            replies[3]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("AGENT switch")
        );
        assert_eq!(replies[4]["error"]["code"], -32601);

        let track = folder.join(sympoie::project_json::track_file(&project.tracks()[1]));
        std::fs::write(
            &track,
            "{\"type\": \"midi\", \"clips\": [{\"start_tick\": 0}]}",
        )
        .unwrap();
        let replies = exchange(
            Some(&folder),
            &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": {"name": "check_project"}})],
        );
        let result = &replies[0]["result"]["structuredContent"];
        assert_eq!(result["valid"], false);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("length_ticks (missing)")
        );
        std::fs::remove_dir_all(folder).unwrap();
    }
}
