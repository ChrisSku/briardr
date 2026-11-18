// Create, rename and close workspace/project/stream; history, info.
// Every Herdr creation passes focus:false so the Mac's focused pane never changes. herdr-core's
// are in `core`.
mod core;

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use thiserror::Error;
use tokio::time::sleep;

use crate::herdr::backend::{Backend, Herdr};
use crate::herdr::model::{tool_label, Model, Stream, StreamKind};
use crate::herdr::rpc::{herdr_agent_kinds, HerdrError};
use crate::herdr::state::{HerdrReader, HerdrState};

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct ActionError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

fn err(status: u16, code: &str, message: impl Into<String>) -> ActionError {
    ActionError {
        status,
        code: code.to_string(),
        message: message.into(),
    }
}

/// Expands a leading `~` or `~/` to `home`, joined and normalized the way Node's `path.join` does.
pub fn expand_home(path: &str, home: &str) -> String {
    if path == "~" {
        return home.to_string();
    }
    match path.strip_prefix("~/") {
        Some(rest) => join(home, rest),
        None => path.to_string(),
    }
}

/// JS `String(v)` for a JSON value. Callers handle `undefined` themselves.
pub(crate) fn js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e21 => (f as i128).to_string(),
            Some(f) if n.is_f64() => f.to_string(),
            _ => n.to_string(),
        },
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| {
                if i.is_null() {
                    String::new()
                } else {
                    js_string(i)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// JS truthiness of a JSON value.
pub(crate) fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// JS `trim()` whitespace: Unicode Zs plus tab, line feeds, and U+FEFF. Rust's `trim` also strips U+0085.
fn is_js_space(c: char) -> bool {
    c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}')
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// The longest prefix of `s` within `max` UTF-16 code units, as JS `slice(0, max)` cuts.
fn utf16_prefix(s: &str, max: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &s[..i];
        }
    }
    s
}

/// Label text: control characters removed, trimmed, cut to `max` UTF-16 code units, empty becomes None.
fn clean_text(raw: &str, max: usize) -> Option<String> {
    let stripped: String = raw
        .chars()
        .filter(|&c| c > '\u{1f}' && c != '\u{7f}')
        .collect();
    let out = utf16_prefix(js_trim(&stripped), max);
    (!out.is_empty()).then(|| out.to_string())
}

/// A label as Herdr takes it: at most 60 UTF-16 code units, None when there's none.
fn clean_label(name: Option<&str>) -> Option<String> {
    name.and_then(|n| clean_text(n, 60))
}

/// Herdr has no way to clear a workspace's or tab's label, so these need a name.
fn required_label(name: Option<&str>) -> Result<String, ActionError> {
    clean_label(name).ok_or_else(|| err(400, "invalid_name", "Enter a name."))
}

/// Node's `path.normalize` for POSIX paths: collapses separators, `.` and `..`, and keeps a trailing slash.
fn normalize(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if !absolute => parts.push(".."),
                _ => {}
            },
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if absolute {
        out = format!("/{out}");
    }
    if out.is_empty() {
        return if absolute {
            "/".to_string()
        } else {
            ".".to_string()
        };
    }
    if p.ends_with('/') && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// Node's `path.join` for two segments.
fn join(a: &str, b: &str) -> String {
    let parts: Vec<&str> = [a, b].into_iter().filter(|s| !s.is_empty()).collect();
    normalize(&parts.join("/"))
}

/// Node's `path.resolve` for one segment: absolute against the process working directory, lexically normalized.
fn resolve(p: &str) -> String {
    let joined = if p.starts_with('/') {
        p.to_string()
    } else {
        let cwd = std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "/".to_string());
        format!("{cwd}/{p}")
    };
    let trimmed = normalize(&joined);
    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn home_dir() -> String {
    std::env::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// JS `Number(v)` as used for pane rectangles: anything that isn't a number becomes NaN.
fn num(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// The value at `path`, or None where JS would read `undefined` (a missing key or a null parent).
fn field(v: &Value, path: &[&str]) -> Option<Value> {
    path.iter().try_fold(v, |cur, k| cur.get(*k)).cloned()
}

/// Sets `key` only when the value is defined, as JSON.stringify drops `undefined` keys.
fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(v) = value {
        map.insert(key.to_string(), v);
    }
}

/// `POST /api/workspaces`: a workspace in a folder (the home folder when none is given), optionally named.
#[derive(Debug, Deserialize)]
pub struct NewWorkspace {
    pub name: Option<String>,
    /// A path on the host; `~` and `~/…` are the home folder.
    pub folder: Option<String>,
}

/// `POST /api/projects`: a project (Herdr's tab) in a workspace, optionally named.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProject {
    /// None, or one that doesn't exist, is `workspace_gone`.
    pub workspace_id: Option<String>,
    pub name: Option<String>,
}

/// `POST /api/streams`: a shell or an agent in a project, optionally named (Herdr TUI), and an agent's
/// first message.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewStream {
    pub project_id: String,
    /// "shell", or one of the agents `Actions::agents` offers.
    pub kind: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Handed to the agent as an argument when it starts, never typed into its terminal.
    #[serde(default)]
    pub prompt: Option<String>,
}

/// `GET /api/agents`: what a new stream can start, and how long its prompt can be.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Agents {
    pub agents: Vec<AgentChoice>,
    pub max_prompt: usize,
}

/// An agent a new stream can start.
#[derive(Debug, PartialEq, Serialize)]
pub struct AgentChoice {
    pub kind: String,
    pub name: String,
    /// Whether it takes a first message when it starts.
    pub prompt: bool,
}

/// A prompt's longest length, in UTF-16 code units as the sheet's field counts them.
pub const MAX_PROMPT: usize = 2000;

/// The agents Briardr has always started, first in the list.
const FIRST_AGENTS: [&str; 2] = ["claude", "codex"];

/// Herdr TUI's agent kinds as the sheet offers them, Claude and Codex first; none listed: those two.
fn agent_choices(kinds: Vec<String>) -> Vec<AgentChoice> {
    let kinds = if kinds.is_empty() {
        FIRST_AGENTS.map(String::from).to_vec()
    } else {
        kinds
    };
    let (mut first, rest): (Vec<String>, Vec<String>) = kinds
        .into_iter()
        .partition(|k| FIRST_AGENTS.contains(&k.as_str()));
    first.sort_by_key(|k| FIRST_AGENTS.iter().position(|f| f == k));
    first
        .into_iter()
        .chain(rest)
        .map(|kind| AgentChoice {
            name: tool_label(&kind),
            prompt: takes_prompt(&kind),
            kind,
        })
        .collect()
}

/// Whether an agent takes a first message when it starts: Claude Code and Codex read one as their
/// prompt argument. Other agents' arguments differ (OpenCode's first is a folder), so they start without.
fn takes_prompt(kind: &str) -> bool {
    matches!(kind, "claude" | "codex")
}

/// A first message as those agents' arguments: after `--`, so that one starting with a dash stays a prompt.
fn prompt_args(prompt: &str) -> Vec<String> {
    vec!["--".to_string(), prompt.to_string()]
}

/// A prompt as it's handed over: trimmed, and None when empty. Refused for what takes none, when too
/// long, and with control characters, which Herdr TUI refuses in arguments (a line break included).
fn clean_prompt(kind: &str, prompt: Option<&str>) -> Result<Option<String>, ActionError> {
    let Some(prompt) = prompt.map(js_trim).filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    if !takes_prompt(kind) {
        return Err(err(
            400,
            "prompt_not_supported",
            "Only Claude and Codex start with a prompt.",
        ));
    }
    if prompt.encode_utf16().count() > MAX_PROMPT {
        return Err(err(
            400,
            "prompt_too_long",
            format!("A prompt can be at most {MAX_PROMPT} characters."),
        ));
    }
    if prompt.chars().any(char::is_control) {
        return Err(err(400, "invalid_prompt", "Write the prompt on one line."));
    }
    Ok(Some(prompt.to_string()))
}

/// The shells in which Herdr's quoting of agent arguments (POSIX single quotes) keeps any text text.
fn quotes_arguments(shell: &str) -> bool {
    let name = shell.rsplit('/').next().unwrap_or(shell);
    matches!(
        name.trim_start_matches('-'),
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "mksh"
    )
}

/// What `pane.process_info` says about a pane's processes.
#[derive(Deserialize)]
struct ProcessInfo {
    shell_pid: u64,
    foreground_processes: Vec<PaneProcess>,
}

#[derive(Deserialize)]
struct PaneProcess {
    pid: u64,
    name: String,
}

/// Agent display name from a label, kind, and names already taken.
pub fn agent_name_from(label: Option<&str>, kind: &str, taken: &HashSet<String>) -> String {
    let source = label.filter(|l| !l.is_empty()).unwrap_or(kind);
    // The regex replacements below only ever produce ASCII, so byte slicing is safe.
    let mut cleaned = String::new();
    let mut in_run = false;
    for c in source.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            cleaned.push(c);
            in_run = false;
        } else if !in_run {
            cleaned.push('-');
            in_run = true;
        }
    }
    let cleaned = cleaned.trim_start_matches(|c: char| !c.is_ascii_lowercase());
    let cleaned = cleaned.trim_end_matches('-');
    let cleaned = &cleaned[..cleaned.len().min(24)];
    let mut base = if cleaned.is_empty() { kind } else { cleaned }.to_string();
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base = kind.to_string();
    }
    let mut name = base.clone();
    let mut i = 2;
    while taken.contains(&name) {
        name = utf16_prefix(&format!("{base}-{i}"), 32).to_string();
        i += 1;
    }
    name
}

/// The HTTP error for a Herdr failure, mapped the same way on every route.
/// `host` is the host as messages name it ("the Mac", "the server").
pub fn from_herdr(e: &HerdrError, host: &str) -> ActionError {
    let (status, message) = match e.code.as_str() {
        "timeout" => (504, "Herdr didn’t answer in time.".to_string()),
        "unreachable" => (503, format!("Herdr isn’t reachable on {host}.")),
        _ => (502, utf16_prefix(&e.message, 200).to_string()),
    };
    err(status, &format!("herdr_{}", e.code), message)
}

pub struct Actions {
    herdr: Herdr,
    /// The host as messages name it, e.g. "the Mac" or "the server".
    host: String,
}

impl Actions {
    pub fn new(herdr: Herdr, host: String) -> Self {
        Actions { herdr, host }
    }

    fn internal(&self) -> ActionError {
        err(
            500,
            "internal",
            format!("Something went wrong on {}.", self.host),
        )
    }

    /// Herdr TUI, for the actions herdr-core has no form of.
    fn tui(&self) -> Result<HerdrState, ActionError> {
        match self.herdr.backend() {
            Backend::Tui(h) => Ok(h),
            Backend::Core(_) => Err(err(
                501,
                "not_supported",
                "This isn’t available with the Herdr app.",
            )),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, ActionError> {
        self.tui()?
            .request(method, params)
            .await
            .map_err(|e| from_herdr(&e, &self.host))
    }

    async fn core_call(&self, method: &str, params: Value) -> Result<Value, ActionError> {
        let Backend::Core(c) = self.herdr.backend() else {
            return Err(self.internal());
        };
        c.request(method, params)
            .await
            .map_err(|e| from_herdr(&e, &self.host))
    }

    async fn refresh(&self) -> Result<(), ActionError> {
        self.herdr
            .refresh()
            .await
            .map_err(|e| from_herdr(&e, &self.host))
    }

    async fn focus_ids(&self) -> Result<String, ActionError> {
        let s = self.call("session.snapshot", json!({})).await?;
        let snap = s
            .get("snapshot")
            .filter(|v| v.is_object())
            .ok_or_else(|| self.internal())?;
        let part = |k: &str| {
            snap.get(k)
                .map_or_else(|| "undefined".to_string(), js_string)
        };
        Ok(format!(
            "{}|{}|{}",
            part("focused_workspace_id"),
            part("focused_tab_id"),
            part("focused_pane_id")
        ))
    }

    /// The name of a pane's shell once it waits at its prompt (alone in the foreground, as Herdr wants
    /// it for starting an agent), or None if it doesn't by the deadline.
    async fn pane_shell(&self, pane_id: &Option<Value>, deadline: Instant) -> Option<String> {
        loop {
            let info = self
                .call("pane.process_info", json!({ "pane_id": pane_id }))
                .await
                .ok()
                .and_then(|r| {
                    serde_json::from_value::<ProcessInfo>(r["process_info"].clone()).ok()
                });
            let shell = info.and_then(|i| match i.foreground_processes.as_slice() {
                [only] if only.pid == i.shell_pid => Some(only.name.clone()),
                _ => None,
            });
            if shell.is_some() || Instant::now() >= deadline {
                return shell;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn create_workspace(&self, req: NewWorkspace) -> Result<Value, ActionError> {
        let label = clean_label(req.name.as_deref());
        let typed = match req.folder.as_deref().map(js_trim) {
            None | Some("") => "~",
            Some(t) => t,
        };
        let cwd = resolve(&expand_home(typed, &home_dir()));
        match tokio::fs::metadata(&cwd).await {
            Err(_) => {
                return Err(err(
                    400,
                    "folder_missing",
                    format!("That folder doesn’t exist on {}.", self.host),
                ))
            }
            Ok(m) if !m.is_dir() => {
                return Err(err(
                    400,
                    "not_a_folder",
                    "That path is a file, not a folder.",
                ))
            }
            Ok(_) => {}
        }
        if let Some(c) = self.core() {
            return self.core_create_workspace(&c, label, cwd).await;
        }
        let before = self.focus_ids().await?;
        let r = self
            .call(
                "workspace.create",
                json!({ "label": label, "cwd": cwd, "focus": false }),
            )
            .await?;
        let after = self.focus_ids().await?;
        self.refresh().await?;
        let mut out = Map::new();
        put(
            &mut out,
            "workspaceId",
            field(&r, &["workspace", "workspace_id"]),
        );
        put(&mut out, "projectId", field(&r, &["tab", "tab_id"]));
        put(&mut out, "streamId", field(&r, &["root_pane", "pane_id"]));
        out.insert("focusUnchanged".to_string(), json!(before == after));
        Ok(Value::Object(out))
    }

    pub async fn create_project(&self, req: NewProject) -> Result<Value, ActionError> {
        let model = self.herdr.model();
        let ws = model
            .as_ref()
            .and_then(|m| {
                m.workspaces
                    .iter()
                    .find(|w| req.workspace_id.as_deref() == Some(w.id.as_str()))
            })
            .ok_or_else(|| err(404, "workspace_gone", "That workspace no longer exists."))?;
        let label = clean_label(req.name.as_deref());
        if let Some(c) = self.core() {
            return self.core_create_project(&c, ws, label).await;
        }
        let before = self.focus_ids().await?;
        let cwd = ws.cwd.as_deref().filter(|c| !c.is_empty());
        let r = self
            .call(
                "tab.create",
                json!({
                    "workspace_id": ws.id,
                    "label": label,
                    "focus": false,
                    "cwd": cwd,
                }),
            )
            .await?;
        let after = self.focus_ids().await?;
        self.refresh().await?;
        let mut out = Map::new();
        put(&mut out, "projectId", field(&r, &["tab", "tab_id"]));
        put(&mut out, "streamId", field(&r, &["root_pane", "pane_id"]));
        out.insert("focusUnchanged".to_string(), json!(before == after));
        Ok(Value::Object(out))
    }

    /// The agents a new stream can start: Herdr TUI's kinds, Claude and Codex first. herdr-core has no
    /// list of agents (the Herdr app keeps its agent menu to itself), so there it's the two Briardr has
    /// always started, which are also what Herdr TUI gets when its CLI doesn't list its kinds.
    pub async fn agents(&self) -> Agents {
        let kinds = match self.herdr.backend() {
            Backend::Tui(h) => herdr_agent_kinds(&h.herdr_target()).await,
            Backend::Core(_) => Vec::new(),
        };
        Agents {
            agents: agent_choices(kinds),
            max_prompt: MAX_PROMPT,
        }
    }

    pub async fn create_stream(&self, req: NewStream) -> Result<Value, ActionError> {
        let kind = req.kind;
        if kind != "shell" && !self.agents().await.agents.iter().any(|a| a.kind == kind) {
            return Err(err(
                400,
                "invalid_kind",
                "Choose a shell or an agent Herdr can start.",
            ));
        }
        let prompt = clean_prompt(&kind, req.prompt.as_deref())?;
        let gone = || err(404, "project_gone", "That project no longer exists.");
        let model = self.herdr.model().ok_or_else(gone)?;
        let project = model
            .projects
            .iter()
            .find(|p| p.id == req.project_id)
            .ok_or_else(gone)?;
        let in_project: Vec<&Stream> = model
            .streams
            .iter()
            .filter(|s| s.project_id == project.id)
            .collect();
        // Herdr app terminals have no names: the app places a new one beside the tab's last.
        if self.core().is_some() {
            let cwd = in_project
                .iter()
                .find_map(|s| s.cwd.clone())
                .or_else(|| {
                    let ws = model
                        .workspaces
                        .iter()
                        .find(|w| w.id == project.workspace_id);
                    ws.and_then(|w| w.cwd.clone())
                })
                .filter(|c| !c.is_empty());
            return self
                .core_create_stream(project, cwd, &kind, prompt.as_deref())
                .await;
        }
        let first = in_project
            .first()
            .map(|s| s.id.clone())
            .ok_or_else(|| err(409, "project_empty", "That project has no panes to split."))?;
        let layout = self
            .call("pane.layout", json!({ "pane_id": first }))
            .await?;
        let rects: &[Value] = layout
            .get("layout")
            .and_then(|l| l.get("panes"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        // The largest pane by area. A tie keeps the earlier pane, and NaN areas lose to any pane.
        let mut biggest: Option<(&Value, f64)> = None;
        for pane in rects {
            let area = num(pane.pointer("/rect/width")) * num(pane.pointer("/rect/height"));
            biggest = match biggest {
                Some((kept, kept_area)) if kept_area >= area => Some((kept, kept_area)),
                _ => Some((pane, area)),
            };
        }
        let biggest_pane = biggest.map(|(p, _)| p);
        let target = biggest_pane
            .and_then(|p| p.get("pane_id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(first);
        let (width, height) = match biggest_pane
            .and_then(|p| p.get("rect"))
            .filter(|r| js_truthy(r))
        {
            Some(rect) => (num(rect.get("width")), num(rect.get("height"))),
            None => (80.0, 24.0),
        };
        // Terminal cells are about twice as tall as wide: split side by side only when there's room.
        let direction = if width >= height * 2.2 {
            "right"
        } else {
            "down"
        };
        let cwd = model
            .streams
            .iter()
            .find(|s| s.id == target)
            .and_then(|s| s.cwd.clone())
            .filter(|c| !c.is_empty());
        let before = self.focus_ids().await?;
        let r = self
            .call(
                "pane.split",
                json!({ "target_pane_id": target, "direction": direction, "focus": false, "cwd": cwd }),
            )
            .await?;
        let pane_id = field(&r, &["pane", "pane_id"]);
        let label = clean_label(req.name.as_deref());
        if let Some(label) = &label {
            self.call("pane.rename", json!({ "pane_id": pane_id, "label": label }))
                .await?;
        }
        let mut note: Option<String> = None;
        if kind != "shell" {
            let agents = self.call("agent.list", json!({})).await?;
            let taken: HashSet<String> = agents
                .get("agents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.get("name"))
                .filter_map(Value::as_str)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect();
            let name = agent_name_from(label.as_deref(), &kind, &taken);
            let agent_label = tool_label(&kind);
            let deadline = Instant::now() + Duration::from_secs(8);
            // Herdr types the agent's command into the pane's shell, its arguments quoted for POSIX
            // shells: in another shell (fish, say) a prompt's backslash could end the quotes.
            let args = match &prompt {
                None => Vec::new(),
                Some(p) => match self.pane_shell(&pane_id, deadline).await {
                    Some(shell) if quotes_arguments(&shell) => prompt_args(p),
                    _ => {
                        note = Some(format!("{agent_label} started without your prompt: Briardr hands one over only in sh, bash, zsh, dash or ksh."));
                        Vec::new()
                    }
                },
            };
            loop {
                let params = json!({ "name": name, "kind": kind, "pane_id": pane_id, "args": args, "timeout_ms": 30_000 });
                let Err(e) = self.tui()?.request("agent.start", params).await else {
                    break;
                };
                if e.code == "agent_not_ready" {
                    note = Some(format!("{agent_label} started and is waiting for you (for example a trust prompt)."));
                    break;
                }
                if Instant::now() < deadline
                    && ["not_available", "busy", "not_ready", "shell"]
                        .iter()
                        .any(|p| e.code.contains(p))
                {
                    sleep(Duration::from_millis(400)).await;
                    continue;
                }
                let code = if e.code.is_empty() {
                    "error"
                } else {
                    e.code.as_str()
                };
                note = Some(format!(
                    "The pane was created, but {agent_label} could not be started ({code})."
                ));
                break;
            }
        }
        let after = self.focus_ids().await?;
        self.refresh().await?;
        let mut out = Map::new();
        put(&mut out, "streamId", pane_id);
        out.insert("note".to_string(), json!(note));
        out.insert("focusUnchanged".to_string(), json!(before == after));
        Ok(Value::Object(out))
    }

    /// A stream without a name (None or empty) falls back to its tool's.
    pub async fn rename(&self, stream_id: &str, name: Option<&str>) -> Result<Value, ActionError> {
        let known = self
            .herdr
            .model()
            .is_some_and(|m| m.streams.iter().any(|s| s.id == stream_id));
        if !known {
            return Err(err(404, "stream_gone", "That stream no longer exists."));
        }
        if self.core().is_some() {
            return Err(err(
                501,
                "not_supported",
                "Terminals in the Herdr app have no names.",
            ));
        }
        self.call(
            "pane.rename",
            json!({ "pane_id": stream_id, "label": clean_label(name) }),
        )
        .await?;
        self.refresh().await?;
        Ok(Value::Null)
    }

    pub async fn rename_workspace(
        &self,
        id: &str,
        name: Option<&str>,
    ) -> Result<Value, ActionError> {
        if !self.has(|m| m.workspaces.iter().any(|w| w.id == id)) {
            return Err(err(
                404,
                "workspace_gone",
                "That workspace no longer exists.",
            ));
        }
        let label = required_label(name)?;
        if let Some(c) = self.core() {
            return self.core_rename(&c, id, &label).await;
        }
        self.call(
            "workspace.rename",
            json!({ "workspace_id": id, "label": label }),
        )
        .await?;
        self.refresh().await?;
        Ok(Value::Null)
    }

    pub async fn rename_project(&self, id: &str, name: Option<&str>) -> Result<Value, ActionError> {
        if !self.has(|m| m.projects.iter().any(|p| p.id == id)) {
            return Err(err(404, "project_gone", "That project no longer exists."));
        }
        let label = required_label(name)?;
        if let Some(c) = self.core() {
            return self.core_rename(&c, id, &label).await;
        }
        self.call("tab.rename", json!({ "tab_id": id, "label": label }))
            .await?;
        self.refresh().await?;
        Ok(Value::Null)
    }

    /// Closing stops what runs inside on the host; the client confirms first. Linked worktree
    /// workspaces are never closed along with this one: Herdr refuses, and says so.
    pub async fn close_workspace(&self, id: &str) -> Result<Value, ActionError> {
        if !self.has(|m| m.workspaces.iter().any(|w| w.id == id)) {
            return Err(err(
                404,
                "workspace_gone",
                "That workspace no longer exists.",
            ));
        }
        if self.core().is_some() {
            return self.core_close(|s| s.workspace_id == id).await;
        }
        self.call("workspace.close", json!({ "workspace_id": id }))
            .await?;
        self.refresh().await?;
        Ok(Value::Null)
    }

    pub async fn close_project(&self, id: &str) -> Result<Value, ActionError> {
        if !self.has(|m| m.projects.iter().any(|p| p.id == id)) {
            return Err(err(404, "project_gone", "That project no longer exists."));
        }
        if self.core().is_some() {
            return self.core_close(|s| s.project_id == id).await;
        }
        self.call("tab.close", json!({ "tab_id": id })).await?;
        self.refresh().await?;
        Ok(Value::Null)
    }

    pub async fn close_stream(&self, id: &str) -> Result<Value, ActionError> {
        if !self.has(|m| m.streams.iter().any(|s| s.id == id)) {
            return Err(err(404, "stream_gone", "That stream no longer exists."));
        }
        if self.core().is_some() {
            return self.core_close(|s| s.id == id).await;
        }
        self.call("pane.close", json!({ "pane_id": id })).await?;
        self.refresh().await?;
        Ok(Value::Null)
    }

    fn has(&self, found: impl FnOnce(&Model) -> bool) -> bool {
        self.herdr.model().is_some_and(|m| found(&m))
    }

    /// The last `lines` lines: 1000 when none are asked for (or 0), and from 100 to 20 000.
    pub async fn history(&self, stream_id: &str, lines: Option<i64>) -> Result<Value, ActionError> {
        let n = match lines {
            None | Some(0) => 1000,
            Some(n) => n.clamp(100, 20_000) as u64,
        };
        if let Backend::Core(_) = self.herdr.backend() {
            let r = self
                .core_call(
                    "terminal.read",
                    json!({ "terminal": stream_id, "lines": n }),
                )
                .await?;
            let text = r.get("text").and_then(Value::as_str).unwrap_or("");
            let truncated = r.get("complete").and_then(Value::as_bool) == Some(false);
            return Ok(
                json!({ "text": text, "truncated": truncated, "lines": n, "asOf": now_ms() }),
            );
        }
        let r = self
            .call(
                "pane.read",
                json!({ "pane_id": stream_id, "source": "recent_unwrapped", "lines": n, "format": "text" }),
            )
            .await?;
        let read = r.get("read");
        let text = match read.and_then(|x| x.get("text")) {
            Some(t) if js_truthy(t) => js_string(t),
            _ => String::new(),
        };
        let truncated = read.and_then(|x| x.get("truncated")).is_some_and(js_truthy);
        Ok(json!({ "text": text, "truncated": truncated, "lines": n, "asOf": now_ms() }))
    }

    pub async fn info(&self, stream_id: &str) -> Result<Value, ActionError> {
        let model = self.herdr.model();
        let stream = model
            .as_ref()
            .and_then(|m| m.streams.iter().find(|s| s.id == stream_id))
            .ok_or_else(|| err(404, "stream_gone", "That stream no longer exists."))?;
        let mut agent: Option<Value> = None;
        if let (StreamKind::Agent, Backend::Tui(h)) = (stream.kind, self.herdr.backend()) {
            agent = h
                .request("agent.get", json!({ "target": stream_id }))
                .await
                .ok()
                .and_then(|r| r.get("agent").cloned())
                .filter(js_truthy);
        }
        let agent_json = match agent {
            None => Value::Null,
            Some(a) => {
                let or_null = |key: &str| {
                    a.get(key)
                        .filter(|v| !v.is_null())
                        .cloned()
                        .unwrap_or(Value::Null)
                };
                let session = a
                    .get("agent_session")
                    .and_then(|s| s.get("value"))
                    .is_some_and(js_truthy);
                let mut out = Map::new();
                out.insert("name".to_string(), or_null("name"));
                put(&mut out, "agent", a.get("agent").cloned());
                put(&mut out, "status", a.get("agent_status").cloned());
                out.insert("stateChangeSeq".to_string(), or_null("state_change_seq"));
                out.insert(
                    "session".to_string(),
                    if session {
                        json!("present")
                    } else {
                        Value::Null
                    },
                );
                Value::Object(out)
            }
        };
        let stream_json = serde_json::to_value(stream).map_err(|_| self.internal())?;
        Ok(json!({ "stream": stream_json, "agent": agent_json, "asOf": now_ms() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Diagnostics;
    use crate::herdr::rpc::HerdrTarget;
    use crate::scratch::Scratch;
    use std::sync::Arc;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// Actions over a Herdr target whose socket never exists, so any Herdr call fails as unreachable.
    fn offline(host: &str) -> (Actions, Scratch) {
        let dir = Scratch::new("actions");
        let diag =
            Arc::new(Diagnostics::new(dir.join("logs"), dir.join("salt")).expect("diagnostics"));
        let target = HerdrTarget {
            herdr_path: "herdr-not-installed".to_string(),
            session: String::new(),
            timeout: Duration::from_secs(1),
        };
        (
            Actions::new(
                Herdr::new(crate::herdr::backend::Target::Tui(target), diag),
                host.to_string(),
            ),
            dir,
        )
    }

    #[test]
    fn expand_home_handles_tilde_forms_only() {
        assert_eq!(expand_home("~", "/home/k"), "/home/k");
        assert_eq!(expand_home("~/", "/home/k"), "/home/k");
        assert_eq!(expand_home("~/notes/../x/", "/home/k"), "/home/k/x/");
        assert_eq!(expand_home("~/a//b", "/home/k"), "/home/k/a/b");
        assert_eq!(expand_home("~user/x", "/home/k"), "~user/x");
        assert_eq!(expand_home("/abs/~/x", "/home/k"), "/abs/~/x");
        assert_eq!(expand_home("~/x", ""), "x");
    }

    #[test]
    fn resolve_is_absolute_and_lexical() {
        assert_eq!(resolve("/a/./b/../c/"), "/a/c");
        assert_eq!(resolve("/../.."), "/");
        assert_eq!(resolve("/"), "/");
        assert!(resolve("relative/dir").starts_with('/'));
    }

    #[test]
    fn labels_lose_control_characters_are_trimmed_and_capped() {
        assert_eq!(
            clean_label(Some("  Hi\tthere\n")),
            Some("Hithere".to_string())
        );
        assert_eq!(clean_label(None), None);
        assert_eq!(clean_label(Some("")), None);
        assert_eq!(clean_label(Some("   ")), None);
        assert_eq!(clean_label(Some("\u{1}\u{7f}")), None);
        assert_eq!(clean_label(Some("\u{a0}x\u{feff}")), Some("x".to_string()));
        // U+0085 is not whitespace to the browser, so it survives trimming.
        assert_eq!(clean_label(Some("\u{85}x")), Some("\u{85}x".to_string()));
        assert_eq!(clean_text("abcdef", 3), Some("abc".to_string()));
        assert_eq!(clean_text("😀abc", 2), Some("😀".to_string()));
        assert_eq!(clean_text("😀abc", 3), Some("😀a".to_string()));
        assert_eq!(clean_label(Some(&"x".repeat(61))), Some("x".repeat(60)));
        assert_eq!(
            required_label(Some(" \u{7} ")).unwrap_err().code,
            "invalid_name"
        );
    }

    #[test]
    fn workspace_and_project_requests_are_typed() {
        let w: NewWorkspace =
            serde_json::from_value(json!({"name": "api", "folder": "~/src"})).unwrap();
        assert_eq!(
            (w.name.as_deref(), w.folder.as_deref()),
            (Some("api"), Some("~/src"))
        );
        let p: NewProject = serde_json::from_value(json!({"workspaceId": null})).unwrap();
        assert_eq!((p.workspace_id, p.name), (None, None));
        assert!(serde_json::from_value::<NewWorkspace>(json!({"folder": 42})).is_err());
        assert!(serde_json::from_value::<NewProject>(json!({"workspaceId": 1})).is_err());
    }

    #[test]
    fn agent_name_from_slugs_the_label() {
        let none = set(&[]);
        assert_eq!(
            agent_name_from(Some("Fix login bug"), "claude", &none),
            "fix-login-bug"
        );
        assert_eq!(
            agent_name_from(Some("  Deploy: prod!  "), "codex", &none),
            "deploy-prod"
        );
        assert_eq!(
            agent_name_from(Some("snake_case-ok"), "codex", &none),
            "snake_case-ok"
        );
    }

    #[test]
    fn agent_name_from_skips_digit_and_punctuation_leading_labels() {
        let none = set(&[]);
        // Leading non-letters are dropped, not replaced by the kind.
        assert_eq!(
            agent_name_from(Some("2fa tokens"), "claude", &none),
            "fa-tokens"
        );
        assert_eq!(agent_name_from(Some("!!! hello"), "claude", &none), "hello");
        assert_eq!(agent_name_from(Some("___x"), "codex", &none), "x");
        // Nothing left after cleaning falls back to the kind.
        assert_eq!(agent_name_from(Some("123"), "claude", &none), "claude");
        assert_eq!(agent_name_from(Some("é"), "codex", &none), "codex");
    }

    #[test]
    fn agent_name_from_uses_kind_for_empty_labels() {
        let none = set(&[]);
        assert_eq!(agent_name_from(None, "claude", &none), "claude");
        assert_eq!(agent_name_from(Some(""), "codex", &none), "codex");
    }

    #[test]
    fn agent_name_from_avoids_collisions() {
        let taken = set(&["fix-login-bug", "fix-login-bug-2"]);
        assert_eq!(
            agent_name_from(Some("Fix login bug"), "claude", &taken),
            "fix-login-bug-3"
        );
        let free_base = set(&["claude"]);
        assert_eq!(agent_name_from(None, "claude", &free_base), "claude-2");
    }

    #[test]
    fn agent_name_from_caps_base_at_24_and_names_at_32() {
        let none = set(&[]);
        let long = "a".repeat(40);
        let base = "a".repeat(24);
        assert_eq!(agent_name_from(Some(&long), "claude", &none), base);
        let taken: HashSet<String> = std::iter::once(base.clone())
            .chain((2..1001).map(|i| format!("{base}-{i}")))
            .collect();
        let name = agent_name_from(Some(&long), "claude", &taken);
        assert_eq!(name, format!("{base}-1001"));
        assert!(name.len() <= 32);
    }

    #[test]
    fn agent_choices_put_claude_and_codex_first_and_say_which_take_a_prompt() {
        let kinds = ["pi", "claude", "codex", "gemini", "agy"].map(String::from);
        let choices = agent_choices(kinds.to_vec());
        let kinds: Vec<&str> = choices.iter().map(|a| a.kind.as_str()).collect();
        assert_eq!(kinds, ["claude", "codex", "pi", "gemini", "agy"]);
        assert_eq!(
            choices[0],
            AgentChoice {
                kind: "claude".into(),
                name: "Claude".into(),
                prompt: true
            }
        );
        assert_eq!((choices[2].name.as_str(), choices[2].prompt), ("Pi", false));
        let none = agent_choices(Vec::new());
        assert_eq!(
            none.iter().map(|a| a.kind.as_str()).collect::<Vec<_>>(),
            ["claude", "codex"],
            "herdr-core, or a Herdr whose CLI lists no kinds"
        );
        let without_codex = agent_choices(vec!["gemini".into(), "claude".into()]);
        assert_eq!(without_codex[0].kind, "claude");
        assert_eq!(without_codex.len(), 2, "only what Herdr starts");
    }

    #[test]
    fn prompts_are_trimmed_capped_on_one_line_and_only_for_agents_that_take_one() {
        assert_eq!(
            clean_prompt("claude", Some("  fix the login bug ")).unwrap(),
            Some("fix the login bug".into())
        );
        assert_eq!(clean_prompt("codex", Some("   ")).unwrap(), None);
        assert_eq!(clean_prompt("shell", None).unwrap(), None);
        let code = |kind: &str, p: &str| clean_prompt(kind, Some(p)).unwrap_err().code;
        assert_eq!(code("shell", "ls"), "prompt_not_supported");
        assert_eq!(code("gemini", "hi"), "prompt_not_supported");
        assert_eq!(code("claude", "one\ntwo"), "invalid_prompt");
        assert_eq!(code("claude", "esc \u{1b}[A"), "invalid_prompt");
        let longest = "é".repeat(MAX_PROMPT);
        assert!(clean_prompt("codex", Some(&longest)).is_ok());
        assert_eq!(code("codex", &format!("{longest}x")), "prompt_too_long");
        assert_eq!(
            code("codex", &"😀".repeat(MAX_PROMPT / 2 + 1)),
            "prompt_too_long",
            "UTF-16 units, as the field counts"
        );
        assert_eq!(
            prompt_args("-p go"),
            ["--", "-p go"],
            "after --, a leading dash stays the prompt's"
        );
    }

    #[test]
    fn prompts_go_only_through_shells_that_keep_quoted_text_text() {
        for shell in ["zsh", "-zsh", "bash", "/bin/sh", "dash", "ksh", "mksh"] {
            assert!(quotes_arguments(shell), "{shell}");
        }
        for shell in [
            "fish", "-fish", "tcsh", "csh", "nu", "xonsh", "elvish", "pwsh", "",
        ] {
            assert!(!quotes_arguments(shell), "{shell}");
        }
    }

    #[test]
    fn a_new_stream_request_is_typed() {
        let req: NewStream =
            serde_json::from_value(json!({"projectId": "w1:t1", "kind": "claude", "prompt": "hi"}))
                .unwrap();
        assert_eq!(
            (
                req.project_id.as_str(),
                req.kind.as_str(),
                req.name,
                req.prompt.as_deref()
            ),
            ("w1:t1", "claude", None, Some("hi"))
        );
        assert!(
            serde_json::from_value::<NewStream>(json!({"projectId": "w1:t1", "kind": 3})).is_err()
        );
        assert!(serde_json::from_value::<NewStream>(json!({"kind": "shell"})).is_err());
    }

    fn new_stream(kind: &str, prompt: Option<&str>) -> NewStream {
        NewStream {
            project_id: "p-1".into(),
            kind: kind.into(),
            name: None,
            prompt: prompt.map(String::from),
        }
    }

    #[tokio::test]
    async fn create_stream_checks_the_kind_and_the_prompt_before_herdr() {
        let (actions, _scratch) = offline("the server");
        let code = async |req| actions.create_stream(req).await.unwrap_err().code;
        assert_eq!(
            code(new_stream("gemini", None)).await,
            "invalid_kind",
            "a Herdr whose CLI lists no kinds: Claude and Codex"
        );
        assert_eq!(
            code(new_stream("shell", Some("hi"))).await,
            "prompt_not_supported"
        );
        assert_eq!(
            code(new_stream("claude", Some("a\nb"))).await,
            "invalid_prompt"
        );
        assert_eq!(code(new_stream("claude", Some("go"))).await, "project_gone");
    }

    #[tokio::test]
    async fn create_workspace_rejects_missing_folder_before_herdr() {
        let (actions, _scratch) = offline("the server");
        let missing = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/no-such-folder-for-actions-test"
        );
        let e = actions
            .create_workspace(NewWorkspace {
                name: Some("Work".into()),
                folder: Some(missing.into()),
            })
            .await
            .unwrap_err();
        assert_eq!(e.status, 400);
        assert_eq!(e.code, "folder_missing");
        assert_eq!(e.message, "That folder doesn’t exist on the server.");
    }

    #[tokio::test]
    async fn create_workspace_rejects_a_file_before_herdr() {
        let (actions, _scratch) = offline("the Mac");
        let file = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let e = actions
            .create_workspace(NewWorkspace {
                name: None,
                folder: Some(file.into()),
            })
            .await
            .unwrap_err();
        assert_eq!(e.status, 400);
        assert_eq!(e.code, "not_a_folder");
        assert_eq!(e.message, "That path is a file, not a folder.");
    }

    #[tokio::test]
    async fn lookups_fail_as_gone_before_herdr() {
        let (actions, _scratch) = offline("the server");
        let e = actions
            .create_project(NewProject {
                workspace_id: Some("ws-1".into()),
                name: None,
            })
            .await
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "workspace_gone"));
        let e = actions
            .create_stream(new_stream("shell", None))
            .await
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "project_gone"));
        let e = actions
            .create_stream(new_stream("vim", None))
            .await
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (400, "invalid_kind"));
        let e = actions.rename("pane-1", Some("x")).await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "stream_gone"));
        let e = actions.info("pane-1").await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "stream_gone"));
        let e = actions
            .rename_workspace("w-1", Some("x"))
            .await
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "workspace_gone"));
        let e = actions.rename_project("p-1", Some("x")).await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "project_gone"));
        let e = actions.close_workspace("w-1").await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "workspace_gone"));
        let e = actions.close_project("p-1").await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "project_gone"));
        let e = actions.close_stream("pane-1").await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "stream_gone"));
    }

    #[test]
    fn herdr_failures_map_to_http_statuses() {
        let unreachable = from_herdr(
            &HerdrError {
                code: "unreachable".to_string(),
                message: "Herdr is not reachable".to_string(),
            },
            "the Mac",
        );
        assert_eq!(unreachable.status, 503);
        assert_eq!(unreachable.code, "herdr_unreachable");
        assert_eq!(unreachable.message, "Herdr isn’t reachable on the Mac.");

        let timeout = from_herdr(
            &HerdrError {
                code: "timeout".to_string(),
                message: "slow".to_string(),
            },
            "the Mac",
        );
        assert_eq!(
            (timeout.status, timeout.code.as_str()),
            (504, "herdr_timeout")
        );
        assert_eq!(timeout.message, "Herdr didn’t answer in time.");

        let other = from_herdr(
            &HerdrError {
                code: "invalid".to_string(),
                message: "x".repeat(300),
            },
            "the Mac",
        );
        assert_eq!((other.status, other.code.as_str()), (502, "herdr_invalid"));
        assert_eq!(other.message, "x".repeat(200));
    }
}
