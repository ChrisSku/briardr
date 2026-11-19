// JSON-RPC over `codex app-server`, allow-listed methods only.
// Reading usage only ever sends initialize, initialized and account/rateLimits/read. Using a reset
// credit sends initialize, initialized and account/rateLimitResetCredit/consume. Every method is
// checked against the allowed set before anything is spawned or written.
use std::io::ErrorKind;
use std::process::Stdio;
use std::time::Duration;

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

use super::types::{
    epoch_ms, Credits, Members, ProviderReport, ResetCredit, ResetCredits, UsageWindow,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// The answer to `account/rateLimits/read`. Older Codex versions leave out the keys newer ones added
/// (the limits by id, the reset credits, a snapshot's id, name and plan).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitsRead {
    rate_limits: Option<RateLimitSnapshot>,
    rate_limits_by_limit_id: Option<Members<RateLimitSnapshot>>,
    rate_limit_reset_credits: Option<ResetCreditsSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitSnapshot {
    limit_id: Option<String>,
    limit_name: Option<String>,
    primary: Option<RateLimitWindow>,
    secondary: Option<RateLimitWindow>,
    credits: Option<Credits>,
    plan_type: Option<String>,
    rate_limit_reached_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitWindow {
    /// A whole number from current versions, a fraction from older ones.
    used_percent: Option<f64>,
    window_duration_mins: Option<u64>,
    /// Epoch seconds.
    resets_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetCreditsSummary {
    available_count: Option<u64>,
    /// None when Codex knows only the count.
    credits: Option<Vec<ResetCreditDetail>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetCreditDetail {
    /// "available", "redeeming", "redeemed" or "unknown"; older versions list only available ones,
    /// without it.
    status: Option<String>,
    title: Option<String>,
    description: Option<String>,
    /// Epoch seconds.
    granted_at: Option<i64>,
    /// Epoch seconds.
    expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ConsumeResult {
    outcome: ResetOutcome,
}

/// A JSON-RPC message to `codex app-server`: a request when it has an id, else a notification.
#[derive(Serialize)]
struct Message<'a, P> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<P>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    client_info: ClientInfo,
}

#[derive(Serialize)]
struct ClientInfo {
    name: &'static str,
    title: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadParams {
    exclude_reset_credit_details: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConsumeParams {
    idempotency_key: String,
}

/// The id of a line from `codex app-server`. Answers to Briardr's requests have a number id; its
/// notifications have none, and its own requests may have a string one, which doesn't read.
#[derive(Deserialize)]
struct Envelope {
    id: Option<u64>,
}

/// An answer to one of Briardr's requests.
#[derive(Deserialize)]
struct Answer<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    #[serde(default)]
    message: String,
}

pub async fn read_codex_rate_limits(codex_path: &str) -> Result<RateLimitsRead, UsageError> {
    read_within(codex_path, READ_TIMEOUT).await
}

async fn read_within(codex_path: &str, timeout: Duration) -> Result<RateLimitsRead, UsageError> {
    codex_request(
        codex_path,
        CODEX_ALLOWED_METHODS,
        "account/rateLimits/read",
        ReadParams {
            exclude_reset_credit_details: false,
        },
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
    let r: ConsumeResult = codex_request(
        codex_path,
        CODEX_RESET_METHODS,
        "account/rateLimitResetCredit/consume",
        ConsumeParams {
            idempotency_key: new_idempotency_key(),
        },
        timeout,
        "use a reset",
    )
    .await?;
    Ok(r.outcome)
}

/// Start `codex app-server`, initialize, send one request, and return its result.
async fn codex_request<P: Serialize, T: DeserializeOwned>(
    codex_path: &str,
    allowed: &[&str],
    method: &str,
    params: P,
    timeout: Duration,
    what: &str,
) -> Result<T, UsageError> {
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

async fn exchange<P: Serialize, T: DeserializeOwned>(
    child: &mut Child,
    allowed: &[&str],
    method: &str,
    params: P,
    what: &str,
) -> Result<T, UsageError> {
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(UsageError::new("error", "Could not start Codex"));
    };
    let mut stdout = BufReader::new(stdout);
    let initialize = InitializeParams {
        client_info: ClientInfo {
            name: "briardr",
            title: "Briardr",
            version: crate::app::VERSION,
        },
    };
    send(&mut stdin, allowed, Some(1), "initialize", Some(initialize)).await?;
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
        let Ok(Envelope { id: Some(id) }) = serde_json::from_slice(&line) else {
            continue;
        };
        if id == 1 {
            match serde_json::from_slice::<Answer<IgnoredAny>>(&line) {
                Ok(Answer { error: None, .. }) => {}
                _ => return Err(UsageError::new("error", "Codex refused the connection")),
            }
            send::<()>(&mut stdin, allowed, None, "initialized", None).await?;
            send(&mut stdin, allowed, Some(2), method, Some(&params)).await?;
        } else if id == 2 {
            return match serde_json::from_slice::<Answer<T>>(&line) {
                Ok(Answer {
                    error: Some(error), ..
                }) => Err(rpc_error(&error, what)),
                Ok(Answer {
                    result: Some(result),
                    ..
                }) => Ok(result),
                _ => Err(UsageError::new("error", "Codex gave an unexpected answer")),
            };
        }
    }
}

async fn send<P: Serialize>(
    stdin: &mut ChildStdin,
    allowed: &[&str],
    id: Option<u64>,
    method: &str,
    params: Option<P>,
) -> Result<(), UsageError> {
    check_allowed(allowed, method)?;
    let message = Message { id, method, params };
    let mut bytes = serde_json::to_vec(&message).unwrap_or_default();
    bytes.push(b'\n');
    // Write errors are ignored; a dead child ends the read.
    let _ = stdin.write_all(&bytes).await;
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

fn rpc_error(error: &RpcError, what: &str) -> UsageError {
    let message = error.message.to_lowercase();
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

fn window_label(mins: Option<u64>, fallback: &str) -> String {
    match mins {
        None | Some(0) => fallback.to_string(),
        Some(300) => "5-hour".into(),
        Some(10080) => "Weekly".into(),
        Some(m) if m % 1440 == 0 => format!("{}-day", m / 1440),
        Some(m) if m % 60 == 0 => format!("{}-hour", m / 60),
        Some(m) => format!("{m}-minute"),
    }
}

fn push_windows(
    windows: &mut Vec<UsageWindow>,
    snap: &RateLimitSnapshot,
    prefix: &str,
    id_prefix: &str,
) {
    for (key, fallback, window) in [
        ("primary", "5-hour", &snap.primary),
        ("secondary", "Weekly", &snap.secondary),
    ] {
        let Some(w) = window else {
            continue;
        };
        let Some(used) = w.used_percent else {
            continue;
        };
        windows.push(UsageWindow {
            id: format!("{id_prefix}{key}"),
            label: format!("{prefix}{}", window_label(w.window_duration_mins, fallback)),
            used_percent: used,
            resets_at: w.resets_at.and_then(|s| epoch_ms(s as f64)),
            window_minutes: Some(w.window_duration_mins),
        });
    }
}

pub fn parse_codex(read: &RateLimitsRead, now: u64) -> ProviderReport {
    let mut windows = Vec::new();
    let main = read.rate_limits.as_ref();
    if let Some(main) = main {
        push_windows(&mut windows, main, "", "");
    }
    // The main snapshot is listed by its id too; `codex` is its id when it doesn't say.
    let main_id = main.and_then(|m| m.limit_id.as_deref()).unwrap_or("codex");
    for (id, snap) in read.rate_limits_by_limit_id.iter().flat_map(|m| &m.0) {
        if id == main_id {
            continue;
        }
        let name = snap
            .limit_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(id);
        push_windows(&mut windows, snap, &format!("{name} · "), &format!("{id}:"));
    }
    let reset_credits = read
        .rate_limit_reset_credits
        .as_ref()
        .map(|r| ResetCredits {
            available: r.available_count.unwrap_or(0),
            credits: r.credits.as_ref().map(|credits| {
                credits
                    .iter()
                    .filter(|c| c.status.as_deref().is_none_or(|s| s == "available"))
                    .map(|c| ResetCredit {
                        title: c.title.clone(),
                        description: c.description.clone(),
                        granted_at: c.granted_at.and_then(|s| epoch_ms(s as f64)),
                        expires_at: c.expires_at.and_then(|s| epoch_ms(s as f64)),
                    })
                    .collect()
            }),
        });
    ProviderReport {
        provider: "codex".into(),
        source: "app-server".into(),
        windows,
        credits: Some(main.and_then(|m| m.credits.clone())),
        reset_credits: Some(reset_credits),
        spend: None,
        plan: Some(main.and_then(|m| m.plan_type.clone())),
        limit_reached: Some(main.and_then(|m| m.rate_limit_reached_type.clone())),
        captured_at: now,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{json, Value};

    use super::*;
    use crate::scratch::{write_executable, Scratch};

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
            write_executable(&path, &body);
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
        let read: RateLimitsRead = serde_json::from_value(input.clone()).expect("answer reads");
        serde_json::to_value(parse_codex(&read, 0)).expect("report serializes")
    }

    #[test]
    fn fixture_gives_its_report() {
        let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture");
        let now = fixture["input"][1].as_u64().expect("now");
        let read: RateLimitsRead =
            serde_json::from_value(fixture["input"][0].clone()).expect("answer reads");
        assert_eq!(
            serde_json::to_value(parse_codex(&read, now)).expect("json"),
            fixture["output"]
        );
    }

    #[test]
    fn current_answers_read_with_every_key() {
        let r = parse_input(&json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": {
                "limitId": "codex", "limitName": null, "normalModelSlug": null,
                "primary": {"usedPercent": 100, "windowDurationMins": 300, "resetsAt": 1790003600},
                "secondary": null,
                "credits": {"hasCredits": false, "unlimited": false, "balance": null},
                "individualLimit": null, "spendControlReached": null,
                "planType": "team", "rateLimitReachedType": "rate_limit_reached"
            },
            "rateLimitsByLimitId": null,
            "rateLimitResetCredits": null,
            "accountId": null,
            "rateLimitUpsell": null
        }));
        assert_eq!(
            r,
            json!({
                "provider": "codex", "source": "app-server",
                "windows": [{"id": "primary", "label": "5-hour", "usedPercent": 100, "resetsAt": 1790003600000u64, "windowMinutes": 300}],
                "credits": {"hasCredits": false, "unlimited": false, "balance": null},
                "resetCredits": null,
                "plan": "team",
                "limitReached": "rate_limit_reached",
                "capturedAt": 0
            })
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
            &json!({"rateLimits": {"credits": {"hasCredits": true, "unlimited": false, "balance": "12.50"}}}),
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
            "availableCount": 2,
            "credits": [
                {"id": "a", "resetType": "codexRateLimits", "status": "available", "grantedAt": 100, "expiresAt": 200, "title": "A", "description": null},
                {"title": "B", "description": "d"},
                {"id": "c", "status": "redeemed", "grantedAt": 100, "title": "C"},
                {"id": "d", "status": "redeeming", "grantedAt": 100, "title": "D"}
            ]
        }});
        let r = parse_input(&input);
        assert_eq!(
            r["resetCredits"],
            json!({
                "available": 2,
                "credits": [
                    {"title": "A", "description": null, "grantedAt": 100000, "expiresAt": 200000},
                    {"title": "B", "description": "d", "grantedAt": null, "expiresAt": null}
                ]
            })
        );
    }

    #[test]
    fn reset_credits_count_defaults_to_zero() {
        let r = parse_input(&json!({"rateLimitResetCredits": {"credits": null}}));
        assert_eq!(r["resetCredits"], json!({"available": 0, "credits": null}));
        let r =
            parse_input(&json!({"rateLimitResetCredits": {"availableCount": 0, "credits": []}}));
        assert_eq!(r["resetCredits"], json!({"available": 0, "credits": []}));
    }

    #[test]
    fn older_answers_read_without_the_newer_keys() {
        let r = parse_input(&json!({"rateLimits": {
            "primary": {"usedPercent": 12.5, "windowDurationMins": 300, "resetsAt": 1790003600},
            "secondary": {"usedPercent": 7.0, "windowDurationMins": 10080, "resetsAt": null},
            "credits": null
        }}));
        let ids: Vec<&str> = r["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["primary", "secondary"]);
        assert_eq!(r["windows"][0]["usedPercent"], 12.5);
        assert_eq!(r["windows"][1]["usedPercent"], json!(7));
        assert_eq!(r["windows"][1]["resetsAt"], Value::Null);
        assert_eq!(r["plan"], Value::Null);
        assert_eq!(r["resetCredits"], Value::Null);
    }

    #[test]
    fn window_without_used_percent_is_skipped() {
        let r = parse_input(&json!({"rateLimits": {
            "primary": {"windowDurationMins": 300},
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
        assert_eq!(label(json!(4320)), "3-day");
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
    fn reset_outcomes_are_named_as_codex_names_them() {
        for outcome in [
            ResetOutcome::Reset,
            ResetOutcome::NothingToReset,
            ResetOutcome::NoCredit,
            ResetOutcome::AlreadyRedeemed,
        ] {
            let read: ResetOutcome = serde_json::from_value(json!(outcome.as_str())).unwrap();
            assert_eq!(read, outcome);
        }
        assert_eq!(ResetOutcome::NothingToReset.as_str(), "nothingToReset");
    }

    #[tokio::test]
    async fn read_sends_only_the_three_allowed_methods() {
        let fake = FakeCodex::new(SCRIPT, READ_REPLY);
        let res = read_within(&fake.path, Duration::from_secs(10))
            .await
            .expect("read");
        let report = parse_codex(&res, 0);
        assert_eq!(report.windows[0].used_percent, 42.0);
        assert_eq!(report.windows[0].resets_at, Some(1_790_000_000_000));
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
    async fn answers_of_another_shape_are_errors() {
        for reply in [
            r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":"50"}}}}"#,
            r#"{"id":2,"result":null}"#,
            r#"{"id":2}"#,
        ] {
            let fake = FakeCodex::new(SCRIPT, reply);
            let err = read_within(&fake.path, Duration::from_secs(10))
                .await
                .expect_err("error");
            assert_eq!(err.category, "error", "{reply}");
            assert_eq!(err.message, "Codex gave an unexpected answer", "{reply}");
        }
    }

    #[tokio::test]
    async fn notifications_and_codex_requests_are_passed_over() {
        let script = SCRIPT.replace(
            "printf '%s\\n' '__REPLY__'",
            r#"printf '%s\n' '{"method":"account/updated","params":{}}' '{"id":"s1","method":"item/approve"}' '__REPLY__'"#,
        );
        assert_ne!(script, SCRIPT);
        let fake = FakeCodex::new(&script, READ_REPLY);
        let res = read_within(&fake.path, Duration::from_secs(10))
            .await
            .expect("read");
        assert_eq!(parse_codex(&res, 0).windows.len(), 1);
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
            let err = codex_request::<_, IgnoredAny>(
                &marker.path,
                CODEX_ALLOWED_METHODS,
                method,
                (),
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
