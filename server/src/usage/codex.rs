// JSON-RPC over `codex app-server`, allow-listed methods only.
// Reading usage only ever sends initialize, initialized and account/rateLimits/read. Using a reset
// credit sends initialize, initialized and account/rateLimitResetCredit/consume. Every method is
// checked against the allowed set before anything is spawned or written.
use std::io::ErrorKind;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

use super::types::{js_num, ProviderReport, UsageWindow};

pub const CODEX_ALLOWED_METHODS: &[&str] =
    &["initialize", "initialized", "account/rateLimits/read"];
pub const CODEX_RESET_METHODS: &[&str] = &[
    "initialize",
    "initialized",
    "account/rateLimitResetCredit/consume",
];

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const RESET_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error, Clone, PartialEq)]
#[error("{message}")]
pub struct UsageError {
    pub category: String,
    pub message: String,
    pub retry_after_ms: Option<u64>,
}

impl UsageError {
    pub fn new(category: impl Into<String>, message: impl Into<String>) -> Self {
        UsageError {
            category: category.into(),
            message: message.into(),
            retry_after_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetOutcome {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

impl ResetOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResetOutcome::Reset => "reset",
            ResetOutcome::NothingToReset => "nothingToReset",
            ResetOutcome::NoCredit => "noCredit",
            ResetOutcome::AlreadyRedeemed => "alreadyRedeemed",
        }
    }
}

pub async fn read_codex_rate_limits(codex_path: &str) -> Result<Value, UsageError> {
    read_within(codex_path, READ_TIMEOUT).await
}

async fn read_within(codex_path: &str, timeout: Duration) -> Result<Value, UsageError> {
    codex_request(
        codex_path,
        CODEX_ALLOWED_METHODS,
        "account/rateLimits/read",
        json!({"excludeResetCreditDetails": false}),
        timeout,
        "report usage",
    )
    .await
}

/// Use one limit reset credit (the backend picks the next available one).
pub async fn consume_codex_reset_credit(codex_path: &str) -> Result<ResetOutcome, UsageError> {
    consume_within(codex_path, RESET_TIMEOUT).await
}

async fn consume_within(codex_path: &str, timeout: Duration) -> Result<ResetOutcome, UsageError> {
    let r = codex_request(
        codex_path,
        CODEX_RESET_METHODS,
        "account/rateLimitResetCredit/consume",
        json!({"idempotencyKey": new_idempotency_key()}),
        timeout,
        "use a reset",
    )
    .await?;
    match r.get("outcome").and_then(Value::as_str) {
        Some("reset") => Ok(ResetOutcome::Reset),
        Some("nothingToReset") => Ok(ResetOutcome::NothingToReset),
        Some("noCredit") => Ok(ResetOutcome::NoCredit),
        Some("alreadyRedeemed") => Ok(ResetOutcome::AlreadyRedeemed),
        _ => Err(UsageError::new("error", "Codex gave an unexpected answer")),
    }
}

/// Start `codex app-server`, initialize, send one request, and return its result.
async fn codex_request(
    codex_path: &str,
    allowed: &[&str],
    method: &str,
    params: Value,
    timeout: Duration,
    what: &str,
) -> Result<Value, UsageError> {
    for m in ["initialize", "initialized", method] {
        check_allowed(allowed, m)?;
    }
    let mut child = Command::new(codex_path)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(spawn_error)?;
    let outcome =
        match tokio::time::timeout(timeout, exchange(&mut child, allowed, method, params, what))
            .await
        {
            Ok(result) => result,
            Err(_) => Err(UsageError::new("timeout", "Codex did not answer in time")),
        };
    let _ = child.kill().await;
    outcome
}

fn spawn_error(e: std::io::Error) -> UsageError {
    match e.kind() {
        ErrorKind::InvalidInput => UsageError::new("not_installed", "Codex is not installed"),
        ErrorKind::NotFound => UsageError::new("not_installed", "Could not start Codex"),
        _ => UsageError::new("error", "Could not start Codex"),
    }
}

async fn exchange(
    child: &mut Child,
    allowed: &[&str],
    method: &str,
    params: Value,
    what: &str,
) -> Result<Value, UsageError> {
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(UsageError::new("error", "Could not start Codex"));
    };
    let mut stdout = BufReader::new(stdout);
    send(
        &mut stdin,
        allowed,
        "initialize",
        json!({
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "briardr", "title": "Briardr", "version": crate::app::VERSION}},
        }),
    )
    .await?;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = stdout.read_until(b'\n', &mut line).await.unwrap_or(0);
        // A partial last line is never parsed.
        if n == 0 || line.last() != Some(&b'\n') {
            return Err(UsageError::new(
                "unavailable",
                format!("Codex exited before it could {what}"),
            ));
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if id_is(&msg, 1.0) {
            if truthy(msg.get("error").unwrap_or(&Value::Null)) {
                return Err(UsageError::new("error", "Codex refused the connection"));
            }
            send(
                &mut stdin,
                allowed,
                "initialized",
                json!({"method": "initialized"}),
            )
            .await?;
            send(
                &mut stdin,
                allowed,
                method,
                json!({"id": 2, "method": method, "params": params.clone()}),
            )
            .await?;
        } else if id_is(&msg, 2.0) {
            if let Some(error) = msg.get("error").filter(|e| truthy(e)) {
                return Err(rpc_error(error, what));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

async fn send(
    stdin: &mut ChildStdin,
    allowed: &[&str],
    method: &str,
    msg: Value,
) -> Result<(), UsageError> {
    check_allowed(allowed, method)?;
    let mut bytes = msg.to_string();
    bytes.push('\n');
    // Write errors are ignored; a dead child ends the read.
    let _ = stdin.write_all(bytes.as_bytes()).await;
    Ok(())
}

fn check_allowed(allowed: &[&str], method: &str) -> Result<(), UsageError> {
    if allowed.contains(&method) {
        Ok(())
    } else {
        Err(UsageError::new(
            "error",
            format!("Refusing to send {method} to Codex"),
        ))
    }
}

fn rpc_error(error: &Value, what: &str) -> UsageError {
    let message = error
        .get("message")
        .filter(|m| truthy(m))
        .map(js_string)
        .unwrap_or_default()
        .to_lowercase();
    if ["auth", "login", "sign"]
        .iter()
        .any(|k| message.contains(k))
    {
        UsageError::new("not_signed_in", "Codex is not signed in")
    } else if ["rate", "429", "too many"]
        .iter()
        .any(|k| message.contains(k))
    {
        UsageError {
            category: "rate_limited".into(),
            message: "Codex is rate limited. Try again in a minute.".into(),
            retry_after_ms: Some(60_000),
        }
    } else {
        UsageError::new("error", format!("Codex could not {what}"))
    }
}

fn new_idempotency_key() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// JS truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `x === n` for a JSON number.
fn id_is(msg: &Value, n: f64) -> bool {
    msg.get("id").and_then(Value::as_f64) == Some(n)
}

/// `!!x` for an optional value.
fn flag(v: Option<&Value>) -> bool {
    v.is_some_and(truthy)
}

/// `x ?? null` for an optional value, with numbers normalized as JSON.stringify would write them.
fn or_null(v: Option<&Value>) -> Value {
    match v {
        None | Some(Value::Null) => Value::Null,
        Some(x) => js_value(x),
    }
}

fn js_value(v: &Value) -> Value {
    match v {
        Value::Number(n) => n.as_f64().map_or(Value::Null, js_num),
        Value::Array(a) => Value::Array(a.iter().map(js_value).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, x)| (k.clone(), js_value(x))).collect())
        }
        other => other.clone(),
    }
}

/// JS ToNumber for the scalar values Codex sends; `None` (undefined) is NaN.
fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse().unwrap_or(f64::NAN)
            }
        }
        Some(_) => f64::NAN,
    }
}

/// JS ToString for the values that can reach a label.
fn js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js_number_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|x| {
                if x.is_null() {
                    String::new()
                } else {
                    js_string(x)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

fn js_number_string(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if n == 0.0 {
        "0".into()
    } else {
        n.to_string()
    }
}

/// Epoch seconds to epoch ms, or null when absent, not a number, or not positive.
fn sec(v: Option<&Value>) -> Value {
    match v.and_then(Value::as_f64) {
        Some(n) if n > 0.0 => js_num(n * 1000.0),
        _ => Value::Null,
    }
}

fn string_or_null(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) => Value::String(s.clone()),
        _ => Value::Null,
    }
}

fn window_label(mins: Option<&Value>, fallback: &str) -> String {
    let Some(mins) = mins.filter(|m| truthy(m)) else {
        return fallback.to_string();
    };
    if mins.as_f64() == Some(300.0) {
        return "5-hour".into();
    }
    if mins.as_f64() == Some(10080.0) {
        return "Weekly".into();
    }
    let n = to_number(Some(mins));
    if n % 1440.0 == 0.0 {
        format!("{}-day", js_number_string(n / 1440.0))
    } else if n % 60.0 == 0.0 {
        format!("{}-hour", js_number_string(n / 60.0))
    } else {
        format!("{}-minute", js_string(mins))
    }
}

fn push_windows(
    windows: &mut Vec<UsageWindow>,
    snap: Option<&Value>,
    prefix: &str,
    id_prefix: &str,
) {
    let Some(snap) = snap else {
        return;
    };
    for (key, fallback) in [("primary", "5-hour"), ("secondary", "Weekly")] {
        let Some(w) = snap.get(key) else {
            continue;
        };
        let Some(used) = w.get("usedPercent").and_then(Value::as_f64) else {
            continue;
        };
        let mins = w.get("windowDurationMins");
        windows.push(UsageWindow {
            id: format!("{id_prefix}{key}"),
            label: format!("{prefix}{}", window_label(mins, fallback)),
            used_percent: js_num(used),
            resets_at: sec(w.get("resetsAt")),
            window_minutes: Some(or_null(mins)),
        });
    }
}

/// Own entries in JS `Object.entries` terms: object members, or indexes of an array.
fn entries(v: &Value) -> Vec<(String, &Value)> {
    match v {
        Value::Object(o) => o.iter().map(|(k, x)| (k.clone(), x)).collect(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, x)| (i.to_string(), x))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn parse_codex(result: &Value, now: u64) -> ProviderReport {
    let mut windows = Vec::new();
    let main = result.get("rateLimits");
    push_windows(&mut windows, main, "", "");
    let main_limit_id = main.and_then(|m| m.get("limitId")).filter(|v| truthy(v));
    if let Some(by_id) = result.get("rateLimitsByLimitId").filter(|v| truthy(v)) {
        for (id, snap) in entries(by_id) {
            if main_limit_id.and_then(Value::as_str) == Some(id.as_str()) {
                continue;
            }
            if id == "codex" && main_limit_id.is_none() {
                continue;
            }
            let name = match snap.get("limitName").filter(|v| truthy(v)) {
                Some(n) => js_string(n),
                None => id.clone(),
            };
            push_windows(
                &mut windows,
                Some(snap),
                &format!("{name} · "),
                &format!("{id}:"),
            );
        }
    }
    let credits = match main.and_then(|m| m.get("credits")).filter(|c| truthy(c)) {
        Some(c) => json!({
            "hasCredits": flag(c.get("hasCredits")),
            "unlimited": flag(c.get("unlimited")),
            "balance": or_null(c.get("balance")),
        }),
        None => Value::Null,
    };
    let reset_credits = match result.get("rateLimitResetCredits").filter(|r| truthy(r)) {
        Some(r) => parse_reset_credits(r),
        None => Value::Null,
    };
    ProviderReport {
        provider: "codex".into(),
        source: "app-server".into(),
        windows,
        credits: Some(credits),
        reset_credits: Some(reset_credits),
        spend: None,
        plan: Some(or_null(main.and_then(|m| m.get("planType")))),
        limit_reached: Some(or_null(main.and_then(|m| m.get("rateLimitReachedType")))),
        captured_at: Value::from(now),
    }
}

/// A credit is listed when it is truthy and its status is absent or "available".
fn listed_credit(c: &Value) -> bool {
    truthy(c)
        && match c.get("status") {
            None => true,
            Some(s) => s.as_str() == Some("available"),
        }
}

fn parse_reset_credits(r: &Value) -> Value {
    let credits = match r.get("credits") {
        Some(Value::Array(items)) => Value::Array(
            items
                .iter()
                .filter(|c| listed_credit(c))
                .map(|c| {
                    json!({
                        "title": string_or_null(c.get("title")),
                        "description": string_or_null(c.get("description")),
                        "grantedAt": sec(c.get("grantedAt")),
                        "expiresAt": sec(c.get("expiresAt")),
                    })
                })
                .collect(),
        ),
        _ => Value::Null,
    };
    let n = to_number(r.get("availableCount"));
    let available = if n.is_nan() || n == 0.0 { 0.0 } else { n };
    json!({"available": js_num(available), "credits": credits})
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::scratch::Scratch;

    const FIXTURE: &str =
        include_str!("../../fixtures/usage/parseCodex.primary-and-secondary.json");

    const SCRIPT: &str = r#"#!/bin/sh
LOG='__LOG__'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{"userAgent":"fake"}}' ;;
    *'"id":2,'*) printf '%s\n' '__REPLY__' ;;
  esac
done
"#;

    const READ_REPLY: &str = r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":1790000000}}}}"#;

    /// A throwaway directory holding one fake `codex` script and its stdin log.
    struct FakeCodex {
        _dir: Scratch,
        path: String,
        log: PathBuf,
    }

    impl FakeCodex {
        fn new(script: &str, reply: &str) -> Self {
            let dir = Scratch::new("codex");
            let log = dir.join("stdin.log");
            let body = script
                .replace("__LOG__", &log.to_string_lossy())
                .replace("__REPLY__", reply);
            let path = dir.join("codex");
            std::fs::write(&path, body).expect("test script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("test script mode");
            FakeCodex {
                _dir: dir,
                path: path.to_string_lossy().into_owned(),
                log,
            }
        }

        fn lines(&self) -> Vec<Value> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).expect("logged line is JSON"))
                .collect()
        }

        fn sent(&self) -> Vec<String> {
            self.lines()
                .iter()
                .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
                .collect()
        }
    }

    fn parse_input(input: &Value) -> Value {
        serde_json::to_value(parse_codex(input, 0)).expect("report serializes")
    }

    #[test]
    fn fixture_matches_ts_output() {
        let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture");
        let now = fixture["input"][1].as_u64().expect("now");
        let report = parse_codex(&fixture["input"][0], now);
        assert_eq!(
            serde_json::to_value(report).expect("json"),
            fixture["output"]
        );
    }

    #[test]
    fn main_limit_id_skips_same_codex_entry() {
        let input = json!({
            "rateLimits": {"limitId": "codex", "primary": {"usedPercent": 1, "windowDurationMins": 300}},
            "rateLimitsByLimitId": {
                "codex": {"limitId": "codex", "primary": {"usedPercent": 9, "windowDurationMins": 300}},
                "codex_bengalfox": {"limitId": "codex_bengalfox", "limitName": "GPT-5.3-Codex-Spark", "primary": {"usedPercent": 3, "windowDurationMins": 300, "resetsAt": 1790000000}}
            }
        });
        let r = parse_input(&input);
        let labels: Vec<&str> = r["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["5-hour", "GPT-5.3-Codex-Spark · 5-hour"]);
        assert_eq!(r["windows"][1]["id"], "codex_bengalfox:primary");
        assert_eq!(r["windows"][1]["resetsAt"], 1790000000000u64);
    }

    #[test]
    fn codex_entry_is_skipped_only_without_main_limit_id() {
        let by_id = json!({
            "codex": {"primary": {"usedPercent": 9, "windowDurationMins": 300}},
            "other": {"primary": {"usedPercent": 4, "windowDurationMins": 60}}
        });
        let no_main_limit = parse_input(&json!({"rateLimitsByLimitId": by_id.clone()}));
        assert_eq!(no_main_limit["windows"].as_array().unwrap().len(), 1);
        assert_eq!(no_main_limit["windows"][0]["id"], "other:primary");

        let other_main =
            parse_input(&json!({"rateLimits": {"limitId": "other"}, "rateLimitsByLimitId": by_id}));
        let labels: Vec<&str> = other_main["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["codex · 5-hour"]);
    }

    #[test]
    fn credits_are_reported_as_sent() {
        let r = parse_input(
            &json!({"rateLimits": {"credits": {"hasCredits": true, "unlimited": 0, "balance": "12.50"}}}),
        );
        assert_eq!(
            r["credits"],
            json!({"hasCredits": true, "unlimited": false, "balance": "12.50"})
        );
        let r = parse_input(&json!({"rateLimits": {"credits": {}}}));
        assert_eq!(
            r["credits"],
            json!({"hasCredits": false, "unlimited": false, "balance": null})
        );
        let r = parse_input(&json!({"rateLimits": {}}));
        assert_eq!(r["credits"], Value::Null);
    }

    #[test]
    fn reset_credits_keep_only_available_or_unmarked_entries() {
        let input = json!({"rateLimitResetCredits": {
            "availableCount": "2",
            "credits": [
                {"title": "A", "status": "available", "grantedAt": 100, "expiresAt": 200},
                {"title": "B", "description": "d"},
                {"title": "C", "status": "used"},
                {"title": "D", "status": null},
                {"title": 5}
            ]
        }});
        let r = parse_input(&input);
        assert_eq!(
            r["resetCredits"],
            json!({
                "available": 2,
                "credits": [
                    {"title": "A", "description": null, "grantedAt": 100000, "expiresAt": 200000},
                    {"title": "B", "description": "d", "grantedAt": null, "expiresAt": null},
                    {"title": null, "description": null, "grantedAt": null, "expiresAt": null}
                ]
            })
        );
    }

    #[test]
    fn reset_credits_count_defaults_to_zero() {
        let r = parse_input(&json!({"rateLimitResetCredits": {"availableCount": "nope"}}));
        assert_eq!(r["resetCredits"], json!({"available": 0, "credits": null}));
        let r =
            parse_input(&json!({"rateLimitResetCredits": {"availableCount": 0, "credits": "x"}}));
        assert_eq!(r["resetCredits"], json!({"available": 0, "credits": null}));
    }

    #[test]
    fn non_number_used_percent_is_skipped() {
        let r = parse_input(&json!({"rateLimits": {
            "primary": {"usedPercent": "50", "windowDurationMins": 300},
            "secondary": {"usedPercent": 7.5, "windowDurationMins": 10080}
        }}));
        assert_eq!(r["windows"].as_array().unwrap().len(), 1);
        assert_eq!(r["windows"][0]["id"], "secondary");
        assert_eq!(r["windows"][0]["usedPercent"], 7.5);
    }

    #[test]
    fn window_labels_follow_duration() {
        let label = |mins: Value| {
            let r = parse_input(
                &json!({"rateLimits": {"primary": {"usedPercent": 1, "windowDurationMins": mins}}}),
            );
            r["windows"][0]["label"].as_str().unwrap().to_string()
        };
        assert_eq!(label(json!(300)), "5-hour");
        assert_eq!(label(json!(10080)), "Weekly");
        assert_eq!(label(json!(90)), "90-minute");
        assert_eq!(label(json!(120)), "2-hour");
        assert_eq!(label(json!(1440)), "1-day");
        assert_eq!(label(json!(300.0)), "5-hour");
    }

    #[test]
    fn missing_duration_uses_fallback_label_and_null_minutes() {
        let r = parse_input(&json!({"rateLimits": {
            "primary": {"usedPercent": 1, "resetsAt": -5},
            "secondary": {"usedPercent": 2, "windowDurationMins": 0}
        }}));
        assert_eq!(r["windows"][0]["label"], "5-hour");
        assert_eq!(r["windows"][0]["windowMinutes"], Value::Null);
        assert_eq!(r["windows"][0]["resetsAt"], Value::Null);
        assert_eq!(r["windows"][1]["label"], "Weekly");
        assert_eq!(r["windows"][1]["windowMinutes"], 0);
    }

    #[test]
    fn reset_outcome_strings_match_ts() {
        assert_eq!(ResetOutcome::Reset.as_str(), "reset");
        assert_eq!(ResetOutcome::NothingToReset.as_str(), "nothingToReset");
        assert_eq!(ResetOutcome::NoCredit.as_str(), "noCredit");
        assert_eq!(ResetOutcome::AlreadyRedeemed.as_str(), "alreadyRedeemed");
    }

    #[tokio::test]
    async fn read_sends_only_the_three_allowed_methods() {
        let fake = FakeCodex::new(SCRIPT, READ_REPLY);
        let res = read_within(&fake.path, Duration::from_secs(10))
            .await
            .expect("read");
        assert_eq!(res["rateLimits"]["primary"]["usedPercent"], 42);
        assert_eq!(
            fake.sent(),
            vec!["initialize", "initialized", "account/rateLimits/read"]
        );
        let read = fake.lines().pop().expect("read request");
        assert_eq!(read["params"], json!({"excludeResetCreditDetails": false}));
    }

    #[tokio::test]
    async fn consume_sends_reset_sequence_with_idempotency_key() {
        let fake = FakeCodex::new(SCRIPT, r#"{"id":2,"result":{"outcome":"nothingToReset"}}"#);
        let outcome = consume_within(&fake.path, Duration::from_secs(10))
            .await
            .expect("consume");
        assert_eq!(outcome, ResetOutcome::NothingToReset);
        assert_eq!(
            fake.sent(),
            vec![
                "initialize",
                "initialized",
                "account/rateLimitResetCredit/consume"
            ]
        );
        let key = fake.lines()[2]["params"]["idempotencyKey"]
            .as_str()
            .expect("key")
            .to_string();
        assert_eq!(key.len(), 36);
        assert_eq!(key.as_bytes()[14], b'4');
    }

    #[tokio::test]
    async fn unexpected_reset_outcome_is_an_error() {
        let fake = FakeCodex::new(SCRIPT, r#"{"id":2,"result":{"outcome":"maybe"}}"#);
        let err = consume_within(&fake.path, Duration::from_secs(10))
            .await
            .expect_err("error");
        assert_eq!(err.category, "error");
        assert_eq!(err.message, "Codex gave an unexpected answer");
    }

    #[tokio::test]
    async fn refused_methods_spawn_nothing() {
        let marker = FakeCodex::new("#!/bin/sh\n: > '__LOG__'\n", "");
        let spawned = marker.log.clone();
        for method in [
            "thread/start",
            "turn/start",
            "account/rateLimitResetCredit/consume",
        ] {
            let err = codex_request(
                &marker.path,
                CODEX_ALLOWED_METHODS,
                method,
                json!({}),
                Duration::from_secs(5),
                "report usage",
            )
            .await
            .expect_err("refused");
            assert_eq!(err.category, "error");
            assert_eq!(err.message, format!("Refusing to send {method} to Codex"));
        }
        assert!(!spawned.exists(), "a refused method must not start Codex");
    }

    #[tokio::test]
    async fn missing_binary_is_not_installed() {
        let err = read_within(
            "/nonexistent/briardr-codex-test/codex",
            Duration::from_secs(5),
        )
        .await
        .expect_err("missing");
        assert_eq!(err.category, "not_installed");
    }

    #[tokio::test]
    async fn silent_child_times_out() {
        let fake = FakeCodex::new("#!/bin/sh\nexec sleep 30\n", "");
        let err = read_within(&fake.path, Duration::from_millis(300))
            .await
            .expect_err("timeout");
        assert_eq!(err.category, "timeout");
        assert_eq!(err.message, "Codex did not answer in time");
    }

    #[tokio::test]
    async fn child_that_exits_is_unavailable() {
        let fake = FakeCodex::new("#!/bin/sh\nexit 0\n", "");
        let err = read_within(&fake.path, Duration::from_secs(10))
            .await
            .expect_err("exited");
        assert_eq!(err.category, "unavailable");
        assert_eq!(err.message, "Codex exited before it could report usage");
    }

    #[tokio::test]
    async fn json_rpc_errors_are_mapped() {
        let cases = [
            (
                r#"{"id":2,"error":{"code":-1,"message":"Please login again"}}"#,
                "not_signed_in",
                "Codex is not signed in",
                None,
            ),
            (
                r#"{"id":2,"error":{"code":429,"message":"Too many requests"}}"#,
                "rate_limited",
                "Codex is rate limited. Try again in a minute.",
                Some(60_000),
            ),
            (
                r#"{"id":2,"error":{"code":-1,"message":"boom"}}"#,
                "error",
                "Codex could not report usage",
                None,
            ),
        ];
        for (reply, category, message, retry) in cases {
            let fake = FakeCodex::new(SCRIPT, reply);
            let err = read_within(&fake.path, Duration::from_secs(10))
                .await
                .expect_err("rpc error");
            assert_eq!(err.category, category);
            assert_eq!(err.message, message);
            assert_eq!(err.retry_after_ms, retry);
        }
    }
}
