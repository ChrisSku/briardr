// Workspace/project/stream model from herdr state.
// Field names serialize as camelCase to match the existing JSON exactly.
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Blocked,
    Working,
    Done,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Agent,
    Shell,
    Editor,
    Program,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub number: u64,
    pub focused: bool,
    pub active_project_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub number: u64,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub id: String,
    pub workspace_id: String,
    pub project_id: String,
    pub terminal_id: String,
    /// Descriptive name: the user's stream name wins over everything else.
    pub name: String,
    /// The user-assigned stream name (Herdr pane label), if any.
    pub label: Option<String>,
    pub kind: StreamKind,
    /// Tool identifier: claude, codex, nvim, zsh, ...
    pub tool: String,
    /// Agent lifecycle state as reported by Herdr; None for shells, editors and programs.
    pub state: Option<AgentState>,
    pub state_change_seq: Option<u64>,
    /// When Briardr observed the current state begin (server clock, ms).
    pub state_since: Option<u64>,
    pub cwd: Option<String>,
    pub folder: Option<String>,
    pub focused: bool,
    pub title: Option<String>,
    pub rows: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub workspaces: Vec<Workspace>,
    pub projects: Vec<Project>,
    pub streams: Vec<Stream>,
    pub focused_stream_id: Option<String>,
    pub herdr_version: Option<String>,
    /// Server time when this model was read from Herdr.
    pub as_of: u64,
}

pub fn tool_label(tool: &str) -> String {
    let known = match tool {
        "claude" => Some("Claude"),
        "codex" => Some("Codex"),
        "nvim" => Some("Neovim"),
        "vim" => Some("Vim"),
        "hx" => Some("Helix"),
        "emacs" => Some("Emacs"),
        _ => None,
    };
    if let Some(label) = known {
        return label.to_string();
    }
    let mut chars = tool.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

const EDITORS: &[&str] = &[
    "nvim", "vim", "vi", "hx", "helix", "emacs", "nano", "micro", "kak", "kakoune", "pico", "joe",
    "mg",
];
const SHELLS: &[&str] = &[
    "zsh", "bash", "fish", "sh", "nu", "dash", "tcsh", "csh", "ksh", "pwsh", "xonsh", "elvish",
];

pub fn classify_process(name: Option<&str>) -> (StreamKind, String) {
    let raw = name.unwrap_or("");
    let n = raw.strip_prefix('-').unwrap_or(raw).to_lowercase();
    if n.is_empty() {
        return (StreamKind::Shell, "shell".to_string());
    }
    if EDITORS.contains(&n.as_str()) {
        return (StreamKind::Editor, n);
    }
    if SHELLS.contains(&n.as_str()) {
        return (StreamKind::Shell, n);
    }
    (StreamKind::Program, n)
}

/// Like JS `items || []`: a missing or non-array field reads as empty.
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A JSON field, treating `null` and absence alike (like JS `??`).
fn field<'a>(obj: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    obj.and_then(|o| o.get(key)).filter(|v| !v.is_null())
}

/// A non-empty JSON string (like JS `||` on strings).
fn non_empty(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Node's `path.basename` for POSIX paths: trailing slashes are ignored.
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or("")
}

fn agent_state(raw: &str) -> Option<AgentState> {
    match raw {
        "blocked" => Some(AgentState::Blocked),
        "working" => Some(AgentState::Working),
        "done" => Some(AgentState::Done),
        "idle" => Some(AgentState::Idle),
        "unknown" => Some(AgentState::Unknown),
        _ => None,
    }
}

/// `process_names` maps pane id to foreground process name. `prev` is the previous model, if any.
pub fn normalize(
    snap: &Value,
    process_names: &HashMap<String, String>,
    prev: Option<&Model>,
    herdr_version: Option<&str>,
    now: u64,
) -> Model {
    let mut agents_by_pane: HashMap<&str, &Value> = HashMap::new();
    for a in items(snap, "agents") {
        if let Some(id) = a.get("pane_id").and_then(Value::as_str) {
            agents_by_pane.insert(id, a);
        }
    }
    let prev_streams: HashMap<&str, &Stream> = prev
        .map(|m| m.streams.iter().map(|s| (s.id.as_str(), s)).collect())
        .unwrap_or_default();
    let mut rows_by_pane: HashMap<&str, Option<u64>> = HashMap::new();
    for l in items(snap, "layouts") {
        for p in items(l, "panes") {
            if let Some(id) = p.get("pane_id").and_then(Value::as_str) {
                let height = field(p.get("rect"), "height").and_then(Value::as_u64);
                rows_by_pane.insert(id, height);
            }
        }
    }

    let mut streams: Vec<Stream> = Vec::new();
    for p in items(snap, "panes") {
        let pane_id = p.get("pane_id").and_then(Value::as_str).unwrap_or("");
        let a = agents_by_pane.get(pane_id).copied();
        let agent = non_empty(field(a, "agent")).or(non_empty(p.get("agent")));
        let (kind, tool) = match agent {
            Some(agent) => (StreamKind::Agent, agent.to_string()),
            None => classify_process(process_names.get(pane_id).map(String::as_str)),
        };
        let raw_state = field(a, "agent_status")
            .or_else(|| field(Some(p), "agent_status"))
            .and_then(Value::as_str);
        let state = if kind == StreamKind::Agent {
            Some(
                raw_state
                    .and_then(agent_state)
                    .unwrap_or(AgentState::Unknown),
            )
        } else {
            None
        };
        let label = field(Some(p), "label")
            .or_else(|| field(a, "label"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let title = field(a, "title")
            .or_else(|| field(Some(p), "title"))
            .and_then(Value::as_str);
        let display = field(a, "display_agent")
            .or_else(|| field(Some(p), "display_agent"))
            .and_then(Value::as_str);
        let name = label
            .clone()
            .or_else(|| title.filter(|s| !s.is_empty()).map(String::from))
            .or_else(|| display.filter(|s| !s.is_empty()).map(String::from))
            .or_else(|| non_empty(field(a, "name")).map(String::from))
            .unwrap_or_else(|| match kind {
                StreamKind::Agent | StreamKind::Editor => tool_label(&tool),
                StreamKind::Program => tool.clone(),
                StreamKind::Shell => "Shell".to_string(),
            });
        let cwd = non_empty(field(a, "foreground_cwd"))
            .or(non_empty(p.get("foreground_cwd")))
            .or(non_empty(p.get("cwd")));
        let before = prev_streams.get(pane_id);
        let state_since = match before {
            Some(b) if b.state == state => b.state_since,
            _ if state.is_some() => Some(now),
            _ => None,
        };
        streams.push(Stream {
            id: pane_id.to_string(),
            workspace_id: p
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            project_id: p
                .get("tab_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            terminal_id: p
                .get("terminal_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name,
            label,
            kind,
            tool,
            state,
            state_change_seq: field(a, "state_change_seq").and_then(Value::as_u64),
            state_since,
            cwd: cwd.map(String::from),
            folder: cwd.map(|c| basename(c).to_string()),
            focused: p.get("focused").and_then(Value::as_bool).unwrap_or(false),
            title: non_empty(p.get("terminal_title_stripped")).map(String::from),
            rows: rows_by_pane.get(pane_id).copied().flatten(),
        });
    }

    let mut first_cwd: HashMap<&str, &str> = HashMap::new();
    for s in &streams {
        if let Some(cwd) = &s.cwd {
            first_cwd
                .entry(s.workspace_id.as_str())
                .or_insert(cwd.as_str());
        }
    }

    let workspaces = items(snap, "workspaces")
        .iter()
        .map(|w| {
            let id = w.get("workspace_id").and_then(Value::as_str).unwrap_or("");
            let number = w.get("number").and_then(Value::as_u64).unwrap_or(0);
            Workspace {
                id: id.to_string(),
                name: non_empty(w.get("label"))
                    .map_or_else(|| format!("Workspace {number}"), String::from),
                number,
                focused: w.get("focused").and_then(Value::as_bool).unwrap_or(false),
                active_project_id: non_empty(w.get("active_tab_id")).map(String::from),
                cwd: first_cwd.get(id).map(|c| c.to_string()),
            }
        })
        .collect();
    let projects = items(snap, "tabs")
        .iter()
        .map(|t| {
            let number = t.get("number").and_then(Value::as_u64).unwrap_or(0);
            Project {
                id: t
                    .get("tab_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                workspace_id: t
                    .get("workspace_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                name: non_empty(t.get("label"))
                    .map_or_else(|| format!("Project {number}"), String::from),
                number,
                focused: t.get("focused").and_then(Value::as_bool).unwrap_or(false),
            }
        })
        .collect();

    Model {
        workspaces,
        projects,
        streams,
        focused_stream_id: non_empty(snap.get("focused_pane_id")).map(String::from),
        herdr_version: non_empty(snap.get("version"))
            .map(String::from)
            .or_else(|| herdr_version.map(String::from)),
        as_of: now,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::{json, Value};

    use super::*;

    fn fixture_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/herdr")
    }

    /// Loads every `<prefix>*.json` fixture, sorted by file name, as (name, parsed JSON).
    fn load_fixtures(prefix: &str) -> Vec<(String, Value)> {
        let mut names: Vec<String> = fs::read_dir(fixture_dir())
            .expect("fixture dir readable")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.starts_with(prefix) && n.ends_with(".json"))
            .collect();
        names.sort();
        assert!(!names.is_empty(), "no fixtures for {prefix}");
        names
            .into_iter()
            .map(|n| {
                let text = fs::read_to_string(fixture_dir().join(&n)).expect("fixture readable");
                (n, serde_json::from_str(&text).expect("fixture is JSON"))
            })
            .collect()
    }

    #[test]
    fn model_tool_label_matches_fixtures() {
        for (name, fixture) in load_fixtures("toolLabel.") {
            let tool = fixture["input"][0].as_str().expect("tool string");
            assert_eq!(json!(tool_label(tool)), fixture["output"], "{name}");
        }
    }

    #[test]
    fn model_classify_process_matches_fixtures() {
        for (name, fixture) in load_fixtures("classifyProcess.") {
            let input = fixture["input"][0].as_str();
            let (kind, tool) = classify_process(input);
            let got = json!({ "kind": kind, "tool": tool });
            assert_eq!(got, fixture["output"], "{name}");
        }
    }

    #[test]
    fn model_normalize_matches_fixtures() {
        for (name, fixture) in load_fixtures("normalize.") {
            let input = fixture["input"].as_array().expect("input array");
            let process_names: HashMap<String, String> = input[1]
                .as_object()
                .expect("process names object")
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect();
            let prev: Option<Model> = match &input[2] {
                Value::Null => None,
                v => Some(serde_json::from_value(v.clone()).expect("prev model")),
            };
            let herdr_version = input[3].as_str();
            let now = input[4].as_u64().expect("now");
            let model = normalize(&input[0], &process_names, prev.as_ref(), herdr_version, now);
            assert_eq!(
                serde_json::to_value(&model).expect("model JSON"),
                fixture["output"],
                "{name}"
            );
        }
    }
}
