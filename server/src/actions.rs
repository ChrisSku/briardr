// Create workspace/project/stream, rename, history, info.
// Every Herdr creation passes focus:false so the Mac's focused pane never changes.
use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use thiserror::Error;
use tokio::time::sleep;

use crate::herdr::model::{Stream, StreamKind};
use crate::herdr::rpc::HerdrError;
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
fn clean_label(v: &Value, max: usize) -> Option<String> {
    let raw = if v.is_null() {
        String::new()
    } else {
        js_string(v)
    };
    let stripped: String = raw
        .chars()
        .filter(|&c| c > '\u{1f}' && c != '\u{7f}')
        .collect();
    let out = utf16_prefix(js_trim(&stripped), max);
    (!out.is_empty()).then(|| out.to_string())
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
    herdr: HerdrState,
    /// The host as messages name it, e.g. "the Mac" or "the server".
    host: String,
}

impl Actions {
    pub fn new(herdr: HerdrState, host: String) -> Self {
        Actions { herdr, host }
    }

    fn internal(&self) -> ActionError {
        err(
            500,
            "internal",
            format!("Something went wrong on {}.", self.host),
        )
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, ActionError> {
        self.herdr
            .request(method, params)
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

    pub async fn create_workspace(
        &self,
        label_in: &Value,
        cwd_in: &Value,
    ) -> Result<Value, ActionError> {
        let label = clean_label(label_in, 60);
        let typed = if js_truthy(cwd_in) {
            js_string(cwd_in)
        } else {
            String::new()
        };
        let typed = match js_trim(&typed) {
            "" => "~",
            t => t,
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

    pub async fn create_project(
        &self,
        workspace_id: &Value,
        label_in: &Value,
    ) -> Result<Value, ActionError> {
        let model = self.herdr.model();
        let ws = model
            .as_ref()
            .and_then(|m| {
                m.workspaces
                    .iter()
                    .find(|w| workspace_id.as_str() == Some(w.id.as_str()))
            })
            .ok_or_else(|| err(404, "workspace_gone", "That workspace no longer exists."))?;
        let before = self.focus_ids().await?;
        let cwd = ws.cwd.as_deref().filter(|c| !c.is_empty());
        let r = self
            .call(
                "tab.create",
                json!({
                    "workspace_id": ws.id,
                    "label": clean_label(label_in, 60),
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

    pub async fn create_stream(
        &self,
        project_id: &Value,
        kind_in: &Value,
        label_in: &Value,
    ) -> Result<Value, ActionError> {
        let kind = js_string(kind_in);
        if !matches!(kind.as_str(), "shell" | "claude" | "codex") {
            return Err(err(
                400,
                "invalid_kind",
                "Choose a shell, Claude, or Codex.",
            ));
        }
        let gone = || err(404, "project_gone", "That project no longer exists.");
        let model = self.herdr.model().ok_or_else(gone)?;
        let project = model
            .projects
            .iter()
            .find(|p| project_id.as_str() == Some(p.id.as_str()))
            .ok_or_else(gone)?;
        let in_project: Vec<&Stream> = model
            .streams
            .iter()
            .filter(|s| s.project_id == project.id)
            .collect();
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
        let label = clean_label(label_in, 60);
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
            let agent_label = if kind == "claude" { "Claude" } else { "Codex" };
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let params =
                    json!({ "name": name, "kind": kind, "pane_id": pane_id, "timeout_ms": 30_000 });
                let Err(e) = self.herdr.request("agent.start", params).await else {
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

    pub async fn rename(&self, stream_id: &str, label_in: &Value) -> Result<Value, ActionError> {
        let known = self
            .herdr
            .model()
            .is_some_and(|m| m.streams.iter().any(|s| s.id == stream_id));
        if !known {
            return Err(err(404, "stream_gone", "That stream no longer exists."));
        }
        self.call(
            "pane.rename",
            json!({ "pane_id": stream_id, "label": clean_label(label_in, 60) }),
        )
        .await?;
        self.refresh().await?;
        Ok(Value::Null)
    }

    /// `lines` is the JS number from the request (NaN when missing or not a number).
    pub async fn history(&self, stream_id: &str, lines: f64) -> Result<Value, ActionError> {
        // Math.max(100, Math.min(20000, Math.round(lines) || 1000)); JS rounds .5 up.
        let rounded = (lines + 0.5).floor();
        let n = if rounded.is_nan() || rounded == 0.0 {
            1000
        } else {
            rounded.clamp(100.0, 20000.0) as u64
        };
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
        if stream.kind == StreamKind::Agent {
            agent = self
                .herdr
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
    use std::path::PathBuf;
    use std::sync::Arc;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// Removes the scratch directory when the test ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Actions over a Herdr target whose socket never exists, so any Herdr call fails as unreachable.
    fn offline(host: &str) -> (Actions, Scratch) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "briardr-actions-test-{}-{nanos}",
            std::process::id()
        ));
        let diag =
            Arc::new(Diagnostics::new(dir.join("logs"), dir.join("salt")).expect("diagnostics"));
        let target = HerdrTarget {
            herdr_path: "herdr-not-installed".to_string(),
            session: String::new(),
            timeout: Duration::from_secs(1),
        };
        (
            Actions::new(HerdrState::new(target, diag), host.to_string()),
            Scratch(dir),
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
    fn clean_label_strips_controls_trims_and_caps() {
        assert_eq!(
            clean_label(&json!("  Hi\tthere\n"), 60),
            Some("Hithere".to_string())
        );
        assert_eq!(clean_label(&Value::Null, 60), None);
        assert_eq!(clean_label(&json!(""), 60), None);
        assert_eq!(clean_label(&json!("   "), 60), None);
        assert_eq!(clean_label(&json!("\u{1}\u{7f}"), 60), None);
        assert_eq!(
            clean_label(&json!("\u{a0}x\u{feff}"), 60),
            Some("x".to_string())
        );
        // U+0085 is not JS whitespace, so it survives trimming.
        assert_eq!(
            clean_label(&json!("\u{85}x"), 60),
            Some("\u{85}x".to_string())
        );
        assert_eq!(clean_label(&json!("abcdef"), 3), Some("abc".to_string()));
        assert_eq!(clean_label(&json!("😀abc"), 2), Some("😀".to_string()));
        assert_eq!(clean_label(&json!("😀abc"), 3), Some("😀a".to_string()));
    }

    #[test]
    fn clean_label_stringifies_like_js() {
        assert_eq!(clean_label(&json!(42), 60), Some("42".to_string()));
        assert_eq!(clean_label(&json!(1.0), 60), Some("1".to_string()));
        assert_eq!(clean_label(&json!(1.5), 60), Some("1.5".to_string()));
        assert_eq!(clean_label(&json!(true), 60), Some("true".to_string()));
        assert_eq!(
            clean_label(&json!(["a", null, 2]), 60),
            Some("a,,2".to_string())
        );
        assert_eq!(
            clean_label(&json!({}), 60),
            Some("[object Object]".to_string())
        );
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

    #[tokio::test]
    async fn create_workspace_rejects_missing_folder_before_herdr() {
        let (actions, _scratch) = offline("the server");
        let missing = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/no-such-folder-for-actions-test"
        );
        let e = actions
            .create_workspace(&json!("Work"), &json!(missing))
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
            .create_workspace(&Value::Null, &json!(file))
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
            .create_project(&json!("ws-1"), &Value::Null)
            .await
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "workspace_gone"));
        let e = actions
            .create_stream(&json!("p-1"), &json!("shell"), &Value::Null)
            .await
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "project_gone"));
        let e = actions
            .create_stream(&json!("p-1"), &json!("vim"), &Value::Null)
            .await
            .unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (400, "invalid_kind"));
        let e = actions.rename("pane-1", &json!("x")).await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (404, "stream_gone"));
        let e = actions.info("pane-1").await.unwrap_err();
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
