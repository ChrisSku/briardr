// OAuth usage fetch and statusline tap.
// Source 1 (opt-in): Claude Code's own sign-in, read from the macOS Keychain, used for one read-only
//   request to Anthropic's usage endpoint. Briardr never refreshes or stores the token.
// Source 2 (opt-in): a status-line tap that records the rate_limits Claude Code already passes to
//   status-line commands. Installed only after the user sees the exact settings change.
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::codex::UsageError;
use super::types::{js_num, ProviderReport, UsageWindow};
use crate::config::{read_json, write_file_atomic, ConfigError};
use crate::http;

pub const CLAUDE_USAGE_URL_DEFAULT: &str = "https://api.anthropic.com/api/oauth/usage";

/// `BRIARDR_CLAUDE_USAGE_URL` or the default.
pub fn claude_usage_url() -> String {
    match std::env::var("BRIARDR_CLAUDE_USAGE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => CLAUDE_USAGE_URL_DEFAULT.to_string(),
    }
}

/// Reads the Claude Code OAuth access token from the Keychain. macOS may ask the user to allow it.
pub async fn read_claude_token() -> Result<String, UsageError> {
    if let Ok(token) = std::env::var("BRIARDR_TEST_CLAUDE_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let mut cmd = tokio::process::Command::new("security");
    cmd.args([
        "find-generic-password",
        "-s",
        "Claude Code-credentials",
        "-w",
    ])
    .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(30), cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            parse_keychain(&String::from_utf8_lossy(&out.stdout), now_ms() as f64)
        }
        _ => Err(not_signed_in(
            "Claude Code’s sign-in was not found in the Keychain (or access was denied)",
        )),
    }
}

/// The Keychain item is either `{claudeAiOauth: {...}}` or the OAuth object itself.
fn parse_keychain(stdout: &str, now_ms: f64) -> Result<String, UsageError> {
    let unreadable = || not_signed_in("Claude Code’s sign-in could not be read");
    let root: Value = serde_json::from_str(stdout.trim()).map_err(|_| unreadable())?;
    let oauth = match root.get("claudeAiOauth") {
        Some(v) if js_truthy(v) => v,
        _ => &root,
    };
    let token = match oauth.get("accessToken") {
        Some(Value::String(t)) if !t.is_empty() => t.clone(),
        _ => return Err(unreadable()),
    };
    if let Some(expires) = oauth.get("expiresAt") {
        if js_truthy(expires) && js_number(expires) < now_ms {
            return Err(not_signed_in(
                "Claude Code’s sign-in has expired. Open Claude Code on the Mac once to refresh it.",
            ));
        }
    }
    Ok(token)
}

fn not_signed_in(message: &str) -> UsageError {
    UsageError::new("not_signed_in", message)
}

fn label_for(key: &str) -> String {
    match key {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        "seven_day_opus" => "Weekly · Opus".into(),
        "seven_day_sonnet" => "Weekly · Sonnet".into(),
        "seven_day_oauth_apps" => "Weekly · Apps".into(),
        _ => key.replace('_', " "),
    }
}

fn window_order(id: &str) -> u8 {
    match id {
        "five_hour" => 0,
        "seven_day" => 1,
        _ => 2,
    }
}

fn claude_report(
    source: &str,
    windows: Vec<UsageWindow>,
    spend: Value,
    captured_at: Value,
) -> ProviderReport {
    ProviderReport {
        provider: "claude".into(),
        source: source.into(),
        windows,
        credits: None,
        reset_credits: None,
        spend: Some(spend),
        plan: None,
        limit_reached: None,
        captured_at,
    }
}

pub fn parse_claude_oauth(body: &Value, now: u64) -> ProviderReport {
    let entries: Vec<(String, &Value)> = match body {
        Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v))
            .collect(),
        _ => Vec::new(),
    };
    let mut windows = Vec::new();
    for (key, v) in entries {
        let Some(used) = v.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        windows.push(UsageWindow {
            label: label_for(&key),
            id: key,
            used_percent: js_num(used),
            resets_at: oauth_reset(v.get("resets_at")),
            window_minutes: None,
        });
    }
    windows.sort_by_key(|w| window_order(&w.id));
    let spend = match body.get("extra_usage") {
        Some(extra) if extra.get("is_enabled").is_some_and(js_truthy) => json!({
            "usedUsd": or_null(extra.get("used_credits")),
            "limitUsd": or_null(extra.get("monthly_limit")),
            "period": "monthly",
        }),
        _ => Value::Null,
    };
    claude_report("oauth", windows, spend, json!(now))
}

fn oauth_reset(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) if !s.is_empty() => parse_date_ms(s)
            .filter(|t| *t != 0)
            .map_or(Value::Null, Value::from),
        _ => Value::Null,
    }
}

pub async fn fetch_claude_oauth() -> Result<ProviderReport, UsageError> {
    let token = read_claude_token().await?;
    let client = http::client(Duration::from_secs(15))
        .map_err(|_| UsageError::new("network", "Could not reach Anthropic"))?;
    let res = client
        .get(claude_usage_url())
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(USER_AGENT, "briardr/0.1")
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| {
            let category = if e.is_timeout() { "timeout" } else { "network" };
            UsageError::new(category, "Could not reach Anthropic")
        })?;
    let status = res.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        let ra = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map_or(0.0, js_number_str);
        let secs = if ra.is_finite() && ra > 0.0 {
            ra
        } else {
            300.0
        };
        return Err(UsageError {
            category: "rate_limited".into(),
            message: "Anthropic asked Briardr to wait before checking again".into(),
            retry_after_ms: Some((secs * 1000.0) as u64),
        });
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(not_signed_in(
            "Claude Code’s sign-in was rejected. Open Claude Code on the Mac to sign in again.",
        ));
    }
    if !status.is_success() {
        return Err(UsageError::new(
            "unavailable",
            format!("Anthropic returned {}", status.as_u16()),
        ));
    }
    let body = res
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .ok_or_else(|| UsageError::new("error", "Usage could not be read"))?;
    Ok(parse_claude_oauth(&body, now_ms()))
}

// ---------- Status-line tap ----------

fn statusline_reset(v: Option<&Value>) -> Value {
    match v {
        Some(r) if js_truthy(r) => js_num(js_number(r) * 1000.0),
        _ => Value::Null,
    }
}

pub fn parse_statusline_capture(cap: &Value) -> Option<ProviderReport> {
    let rate_limits = cap.get("rate_limits").filter(|r| js_truthy(r))?;
    let captured_at = cap.get("capturedAt").filter(|c| js_truthy(c))?;
    let mut windows = Vec::new();
    for key in ["five_hour", "seven_day"] {
        let Some(w) = rate_limits.get(key) else {
            continue;
        };
        if let Some(used) = w.get("used_percentage").and_then(Value::as_f64) {
            windows.push(UsageWindow {
                id: key.into(),
                label: label_for(key),
                used_percent: js_num(used),
                resets_at: statusline_reset(w.get("resets_at")),
                window_minutes: None,
            });
        }
    }
    let spend_limit = rate_limits.get("spend_limit");
    if let Some(used) = spend_limit
        .and_then(|s| s.get("used_percentage"))
        .and_then(Value::as_f64)
    {
        let s = spend_limit.unwrap_or(&Value::Null);
        windows.push(UsageWindow {
            id: "spend_limit".into(),
            label: "Spend limit".into(),
            used_percent: js_num(used),
            resets_at: statusline_reset(s.get("resets_at")),
            window_minutes: None,
        });
    }
    let spend = match spend_limit.filter(|s| js_truthy(s)) {
        Some(s) => json!({
            "usedUsd": or_null(s.get("used_usd")),
            "limitUsd": or_null(s.get("limit_usd")),
            "period": or_null(s.get("period")),
        }),
        None => Value::Null,
    };
    Some(claude_report(
        "statusline",
        windows,
        spend,
        captured_at.clone(),
    ))
}

/// The status-line tap's record of what it changed, kept in the state directory.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TapRecord {
    settings_path: String,
    /// The exact statusLine value before Briardr touched it; absent when there was none.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    original: Option<Value>,
    had_status_line: bool,
    backup_path: Option<String>,
    installed_bytes_hash: String,
    installed_at: u64,
}

/// Deserializes a present key, including `null`, as `Some`; a missing key stays `None` via `default`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Debug, Error)]
pub enum TapError {
    #[error("The status-line tap is already installed")]
    AlreadyInstalled,
    #[error("Claude’s settings changed since you reviewed them. Review the change again.")]
    SettingsChanged,
    #[error("A Briardr tap is already configured")]
    AlreadyConfigured,
    #[error("Claude’s settings are not a JSON object")]
    NotAnObject,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Write(#[from] ConfigError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Restored {
    Backup,
    Field,
    None,
}

/// Exactly what installing would change, for the user to review.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TapPreview {
    pub settings_path: String,
    pub exists: bool,
    pub before: Value,
    pub after: Value,
    pub fingerprint: String,
    pub chained: Value,
}

// The tap: records only rate_limits (never conversation content), then runs the original status
// line with the same input and passes its output through unchanged.
const TAP_SOURCE: &str = r#"#!/usr/bin/env node
import fs from 'node:fs';
import { spawn } from 'node:child_process';
const [recordFile, captureFile] = process.argv.slice(2);
const chunks = [];
process.stdin.on('data', (c) => chunks.push(c));
process.stdin.on('end', () => {
  const input = Buffer.concat(chunks);
  try {
    const j = JSON.parse(input.toString('utf8'));
    if (j && j.rate_limits) {
      const tmp = captureFile + '.' + process.pid + '.tmp';
      fs.writeFileSync(tmp, JSON.stringify({ capturedAt: Date.now(), rate_limits: j.rate_limits }), { mode: 0o600 });
      fs.renameSync(tmp, captureFile);
    }
  } catch {}
  let original = null;
  try { original = JSON.parse(fs.readFileSync(recordFile, 'utf8')).original; } catch {}
  const cmd = original && typeof original === 'object' ? original.command : null;
  if (!cmd) return;
  const child = spawn('/bin/sh', ['-c', cmd], { stdio: ['pipe', 'inherit', 'inherit'] });
  child.on('exit', (code) => process.exit(code ?? 0));
  child.stdin.on('error', () => {});
  child.stdin.end(input);
});
"#;

#[derive(Debug)]
pub struct StatuslineTap {
    pub claude_dir: PathBuf,
    pub state_dir: PathBuf,
    pub bin_dir: PathBuf,
    /// The interpreter the status-line command runs the tap with (the running Bun).
    pub interpreter: PathBuf,
}

impl StatuslineTap {
    pub fn new(claude_dir: PathBuf, state_dir: PathBuf, bin_dir: PathBuf) -> Self {
        StatuslineTap {
            claude_dir,
            state_dir,
            bin_dir,
            interpreter: find_bun(),
        }
    }
    pub fn settings_path(&self) -> PathBuf {
        self.claude_dir.join("settings.json")
    }
    fn record_file(&self) -> PathBuf {
        self.state_dir.join("statusline-tap.json")
    }
    fn capture_file(&self) -> PathBuf {
        self.state_dir.join("statusline-rate-limits.json")
    }
    fn tap_script(&self) -> PathBuf {
        self.bin_dir.join("briardr-statusline-tap.mjs")
    }
    pub fn record(&self) -> Option<TapRecord> {
        read_json(&self.record_file(), None)
    }
    pub fn installed(&self) -> bool {
        self.record().is_some()
    }
    fn command(&self) -> String {
        [
            &self.interpreter,
            &self.tap_script(),
            &self.record_file(),
            &self.capture_file(),
        ]
        .iter()
        .map(|p| shell_quote(&p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
    }
    pub fn preview(&self) -> Result<TapPreview, TapError> {
        let path = self.settings_path();
        let existing = read_optional(&path)?;
        let exists = existing.is_some();
        let bytes = existing.unwrap_or_default();
        let settings = parse_settings(&bytes)?;
        let before = settings.get("statusLine").cloned();
        Ok(TapPreview {
            settings_path: path.to_string_lossy().into_owned(),
            exists,
            after: tap_status_line(before.as_ref(), self.command()),
            fingerprint: sha256_hex(&bytes),
            chained: or_null(before.as_ref().and_then(|b| b.get("command"))),
            before: before.unwrap_or(Value::Null),
        })
    }
    pub fn install(&self, fingerprint: &str) -> Result<(), TapError> {
        if self.installed() {
            return Err(TapError::AlreadyInstalled);
        }
        let path = self.settings_path();
        let existing = read_optional(&path)?;
        let exists = existing.is_some();
        let bytes = existing.unwrap_or_default();
        if sha256_hex(&bytes) != fingerprint {
            return Err(TapError::SettingsChanged);
        }
        let mut settings = parse_settings(&bytes)?;
        let configured = settings
            .get("statusLine")
            .and_then(|s| s.get("command"))
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("briardr-statusline-tap"));
        if configured {
            return Err(TapError::AlreadyConfigured);
        }
        let mode = if exists {
            fs::metadata(&path)?.permissions().mode() & 0o777
        } else {
            0o600
        };
        let backup_path = if exists {
            Some(write_backup(&path, &bytes)?)
        } else {
            None
        };
        self.write_tap_script()?;
        let had_status_line = settings.contains_key("statusLine");
        let original = settings.get("statusLine").cloned();
        let status_line = tap_status_line(original.as_ref(), self.command());
        settings.insert("statusLine".into(), status_line);
        let out = format!("{}\n", serde_json::to_string_pretty(&settings)?);
        let record = TapRecord {
            settings_path: path.to_string_lossy().into_owned(),
            original,
            had_status_line,
            backup_path: backup_path.map(|p| p.to_string_lossy().into_owned()),
            installed_bytes_hash: sha256_hex(out.as_bytes()),
            installed_at: now_ms(),
        };
        // Record first so the tap can always find the chained command, then swap the settings atomically.
        write_file_atomic(
            &self.record_file(),
            serde_json::to_string_pretty(&record)?.as_bytes(),
            0o600,
        )?;
        fs::create_dir_all(&self.claude_dir)?;
        write_file_atomic(&path, out.as_bytes(), mode)?;
        Ok(())
    }
    /// Restore the original exactly. If settings are untouched since install, restore the backup byte for byte.
    pub fn remove(&self) -> Result<Restored, TapError> {
        let Some(record) = self.record() else {
            return Ok(Restored::None);
        };
        let path = PathBuf::from(&record.settings_path);
        let restored = match read_optional(&path)? {
            Some(bytes) if sha256_hex(&bytes) == record.installed_bytes_hash => {
                match record
                    .backup_path
                    .as_deref()
                    .map(Path::new)
                    .filter(|p| p.exists())
                {
                    Some(backup) => {
                        let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
                        write_file_atomic(&path, &fs::read(backup)?, mode)?;
                    }
                    // There was no settings file before Briardr.
                    None => fs::remove_file(&path)?,
                }
                Restored::Backup
            }
            Some(bytes) => {
                // The user changed other settings since; put back only the statusLine value.
                let mut settings = parse_settings(&bytes)?;
                let original = if record.had_status_line {
                    record.original.clone()
                } else {
                    None
                };
                match original {
                    Some(value) => settings.insert("statusLine".into(), value),
                    None => settings.remove("statusLine"),
                };
                let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
                let out = format!("{}\n", serde_json::to_string_pretty(&settings)?);
                write_file_atomic(&path, out.as_bytes(), mode)?;
                Restored::Field
            }
            None => Restored::None,
        };
        remove_if_exists(&self.record_file())?;
        Ok(restored)
    }
    pub fn latest(&self) -> Option<ProviderReport> {
        parse_statusline_capture(&read_json(&self.capture_file(), Value::Null))
    }
    fn write_tap_script(&self) -> Result<(), TapError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.bin_dir)?;
        write_file_atomic(&self.tap_script(), TAP_SOURCE.as_bytes(), 0o700)?;
        Ok(())
    }
}

/// The Bun executable that runs the tap script (the running Bun):
/// `BRIARDR_BUN`, else the first `bun` on PATH (resolved), else plain "bun".
fn find_bun() -> PathBuf {
    if let Some(p) = std::env::var_os("BRIARDR_BUN").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join("bun"))
        .find(|p| p.is_file())
        .map(|p| fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|| PathBuf::from("bun"))
}

/// Copies the settings file next to itself with the same mode; the name is never reused.
fn write_backup(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let backup = PathBuf::from(format!(
        "{}.briardr-tap-backup-{}",
        path.to_string_lossy(),
        now_ms()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&backup)?;
    file.write_all(bytes)?;
    file.set_permissions(fs::metadata(path)?.permissions())?;
    Ok(backup)
}

fn tap_status_line(original: Option<&Value>, command: String) -> Value {
    let mut map = match original.and_then(Value::as_object) {
        Some(m) => m.clone(),
        None => Map::new(),
    };
    map.insert("type".into(), json!("command"));
    map.insert("command".into(), json!(command));
    Value::Object(map)
}

/// Parses settings bytes; empty bytes count as `{}`.
fn parse_settings(bytes: &[u8]) -> Result<Map<String, Value>, TapError> {
    let text = String::from_utf8_lossy(bytes);
    let value: Value = if text.is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text)?
    };
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(TapError::NotAnObject),
    }
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn or_null(v: Option<&Value>) -> Value {
    match v {
        Some(v) if !v.is_null() => v.clone(),
        _ => Value::Null,
    }
}

/// JavaScript truthiness for a JSON value.
fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// JavaScript `Number(value)` for a JSON value; arrays and objects are treated as NaN.
fn js_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => js_number_str(s),
        Value::Array(_) | Value::Object(_) => f64::NAN,
    }
}

/// JavaScript `Number(string)` for the forms Briardr can receive: decimal and 0x/0o/0b integers.
fn js_number_str(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    let lower = t.to_ascii_lowercase();
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(digits) = lower.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).map_or(f64::NAN, |n| n as f64);
        }
    }
    // Rust accepts spellings such as "inf" and "NaN" that JavaScript's Number() does not.
    if !t
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

fn take_digits(b: &[u8], p: &mut usize, n: usize) -> Option<i64> {
    let digits = b.get(*p..*p + n)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *p += n;
    Some(
        digits
            .iter()
            .fold(0, |acc, d| acc * 10 + i64::from(d - b'0')),
    )
}

fn eat(b: &[u8], p: &mut usize, c: u8) -> bool {
    if b.get(*p) == Some(&c) {
        *p += 1;
        true
    } else {
        false
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `Date.parse` for the ISO 8601 forms the usage endpoint sends. Date-only and offset-less times are
/// read as UTC. Returns epoch milliseconds.
fn parse_date_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let mut p = 0;
    let year = take_digits(b, &mut p, 4)?;
    let (mut month, mut day) = (1, 1);
    if eat(b, &mut p, b'-') {
        month = take_digits(b, &mut p, 2)?;
        if eat(b, &mut p, b'-') {
            day = take_digits(b, &mut p, 2)?;
        }
    }
    // hour, minute, second, millisecond
    let mut time = [0i64; 4];
    let mut offset_min = 0;
    if p < b.len() && (b[p] == b'T' || b[p] == b't') {
        p += 1;
        time[0] = take_digits(b, &mut p, 2)?;
        if !eat(b, &mut p, b':') {
            return None;
        }
        time[1] = take_digits(b, &mut p, 2)?;
        if eat(b, &mut p, b':') {
            time[2] = take_digits(b, &mut p, 2)?;
            if eat(b, &mut p, b'.') {
                let start = p;
                while p < b.len() && b[p].is_ascii_digit() {
                    p += 1;
                }
                if p == start {
                    return None;
                }
                let fraction = &b[start..p];
                time[3] = (0..3).fold(0, |acc, i| {
                    acc * 10 + fraction.get(i).map_or(0, |d| i64::from(d - b'0'))
                });
            }
        }
        let zulu = eat(b, &mut p, b'Z') || eat(b, &mut p, b'z');
        if !zulu && p < b.len() && (b[p] == b'+' || b[p] == b'-') {
            let sign = if b[p] == b'-' { -1 } else { 1 };
            p += 1;
            let hours = take_digits(b, &mut p, 2)?;
            if !eat(b, &mut p, b':') {
                return None;
            }
            let minutes = take_digits(b, &mut p, 2)?;
            offset_min = sign * (hours * 60 + minutes);
        }
    }
    if p != b.len() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let [hour, minute, second, millis] = time;
    if hour > 24
        || minute > 59
        || second > 59
        || (hour == 24 && (minute, second, millis) != (0, 0, 0))
    {
        return None;
    }
    let days = days_from_civil(year, month, 1) + day - 1;
    let local = ((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis;
    Some(local - offset_min * 60_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const FIVE_HOUR_FIXTURE: &str =
        include_str!("../../fixtures/usage/parseClaudeOauth.five-hour-and-weekly.json");
    const EMPTY_FIXTURE: &str = include_str!("../../fixtures/usage/parseClaudeOauth.empty.json");

    #[test]
    fn oauth_fixtures_match_ts_output() {
        for text in [EMPTY_FIXTURE, FIVE_HOUR_FIXTURE] {
            let fixture: Value = serde_json::from_str(text).unwrap();
            let now = fixture["input"][1].as_u64().unwrap();
            let report = parse_claude_oauth(&fixture["input"][0], now);
            assert_eq!(serde_json::to_value(&report).unwrap(), fixture["output"]);
        }
    }

    #[test]
    fn statusline_capture_reads_windows_and_spend() {
        let cap = json!({
            "capturedAt": 1790000000000u64,
            "rate_limits": {
                "five_hour": { "used_percentage": 12, "resets_at": 1791741600 },
                "seven_day": { "used_percentage": 41.5 },
                "spend_limit": { "used_percentage": 3, "used_usd": 1.5, "limit_usd": 50, "period": "monthly" }
            }
        });
        let v = serde_json::to_value(parse_statusline_capture(&cap).unwrap()).unwrap();
        assert_eq!(v["source"], "statusline");
        assert_eq!(v["capturedAt"], 1790000000000u64);
        assert_eq!(
            v["windows"],
            json!([
                { "id": "five_hour", "label": "5-hour", "usedPercent": 12, "resetsAt": 1791741600000u64 },
                { "id": "seven_day", "label": "Weekly", "usedPercent": 41.5, "resetsAt": null },
                { "id": "spend_limit", "label": "Spend limit", "usedPercent": 3, "resetsAt": null }
            ])
        );
        assert_eq!(
            v["spend"],
            json!({ "usedUsd": 1.5, "limitUsd": 50, "period": "monthly" })
        );
    }

    #[test]
    fn statusline_capture_requires_rate_limits_and_timestamp() {
        assert!(
            parse_statusline_capture(&json!({ "capturedAt": 1, "rate_limits": null })).is_none()
        );
        assert!(parse_statusline_capture(&json!({ "rate_limits": {} })).is_none());
        assert!(parse_statusline_capture(&json!(null)).is_none());
        let cap =
            json!({ "capturedAt": 1, "rate_limits": { "five_hour": { "used_percentage": 1 } } });
        let v = serde_json::to_value(parse_statusline_capture(&cap).unwrap()).unwrap();
        assert_eq!(v["spend"], Value::Null);
    }

    #[test]
    fn date_parsing_matches_date_parse() {
        assert_eq!(parse_date_ms("2026-10-15T00:00:00Z"), Some(1792022400000));
        assert_eq!(parse_date_ms("2026-10-11T18:00:00Z"), Some(1791741600000));
        assert_eq!(
            parse_date_ms("2026-10-11T20:00:00+02:00"),
            Some(1791741600000)
        );
        assert_eq!(
            parse_date_ms("2026-10-11T18:00:00.123Z").map(|t| t % 1000),
            Some(123)
        );
        assert_eq!(parse_date_ms("2026-02-31"), parse_date_ms("2026-03-03"));
        assert_eq!(parse_date_ms("not a date"), None);
        assert_eq!(parse_date_ms("2026-13-01"), None);
    }

    #[test]
    fn js_number_conversions() {
        assert_eq!(js_number_str("42"), 42.0);
        assert_eq!(js_number_str(""), 0.0);
        assert_eq!(js_number_str("0x1A"), 26.0);
        assert_eq!(js_number_str(" 7 "), 7.0);
        assert!(js_number_str("abc").is_nan());
        assert!(js_number_str("inf").is_nan());
    }

    #[test]
    fn keychain_item_forms_and_errors() {
        let now = 2000.0;
        let wrapped = r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":3000}}"#;
        assert_eq!(parse_keychain(wrapped, now).unwrap(), "abc");
        assert_eq!(
            parse_keychain(r#"{"accessToken":"plain"}"#, now).unwrap(),
            "plain"
        );
        let expired = parse_keychain(r#"{"accessToken":"t","expiresAt":1000}"#, now).unwrap_err();
        assert_eq!(
            expired.message,
            "Claude Code’s sign-in has expired. Open Claude Code on the Mac once to refresh it."
        );
        assert_eq!(expired.category, "not_signed_in");
        for bad in ["", "not json", r#"{"other":1}"#, r#"{"accessToken":""}"#] {
            let err = parse_keychain(bad, now).unwrap_err();
            assert_eq!(err.message, "Claude Code’s sign-in could not be read");
        }
    }

    fn http_response(status: &str, retry_after: Option<&str>, body: &str) -> String {
        let extra = retry_after
            .map(|v| format!("Retry-After: {v}\r\n"))
            .unwrap_or_default();
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Serves one canned response on a local port and returns the URL to request.
    async fn serve(response: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut seen = Vec::new();
                let mut buf = [0u8; 1024];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{addr}/api/oauth/usage")
    }

    async fn fetch_via(response: String) -> Result<ProviderReport, UsageError> {
        std::env::set_var("BRIARDR_CLAUDE_USAGE_URL", serve(response).await);
        fetch_claude_oauth().await
    }

    #[tokio::test]
    async fn fetch_and_url_follow_the_environment() {
        // The environment is shared by every test in the process, so all env-dependent cases run here.
        const TOKEN: &str = "test-token-do-not-leak";
        std::env::remove_var("BRIARDR_CLAUDE_USAGE_URL");
        assert_eq!(claude_usage_url(), CLAUDE_USAGE_URL_DEFAULT);
        std::env::set_var("BRIARDR_TEST_CLAUDE_TOKEN", TOKEN);

        let body = r#"{"five_hour":{"utilization":12,"resets_at":"2026-10-11T18:00:00Z"},"seven_day":{"utilization":41.5}}"#;
        let report = fetch_via(http_response("200 OK", None, body))
            .await
            .unwrap();
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["windows"][0]["resetsAt"], 1791741600000u64);
        assert_eq!(v["windows"][1]["usedPercent"], 41.5);

        let cases = [
            (
                http_response("429 Too Many Requests", Some("30"), "{}"),
                "rate_limited",
                Some(30_000),
            ),
            (
                http_response("429 Too Many Requests", None, "{}"),
                "rate_limited",
                Some(300_000),
            ),
            (
                http_response("401 Unauthorized", None, "{}"),
                "not_signed_in",
                None,
            ),
            (
                http_response("403 Forbidden", None, "{}"),
                "not_signed_in",
                None,
            ),
            (
                http_response("500 Internal Server Error", None, "{}"),
                "unavailable",
                None,
            ),
            (http_response("200 OK", None, "not json"), "error", None),
        ];
        for (response, category, retry_ms) in cases {
            let err = fetch_via(response).await.unwrap_err();
            assert_eq!(err.category, category);
            assert_eq!(err.retry_after_ms, retry_ms);
            assert!(!err.message.contains(TOKEN));
        }

        std::env::remove_var("BRIARDR_TEST_CLAUDE_TOKEN");
    }

    /// A scratch directory with a `claude` settings folder, and room for the tap's state and bin folders.
    struct ClaudeHome(Scratch);

    impl ClaudeHome {
        fn new(name: &str) -> Self {
            let dir = Scratch::new(&format!("claude-{name}"));
            fs::create_dir_all(dir.join("claude")).unwrap();
            ClaudeHome(dir)
        }
        fn tap(&self) -> StatuslineTap {
            StatuslineTap::new(
                self.0.join("claude"),
                self.0.join("state"),
                self.0.join("bin"),
            )
        }
        fn settings(&self) -> PathBuf {
            self.0.join("claude").join("settings.json")
        }
    }

    const ORIGINAL: &str = r#"{
  "theme": "dark",
  "statusLine": { "type": "command", "command": "printf 'orig:%s' \"$(cat | wc -c | tr -d ' ')\"", "padding": 1 }
}
"#;

    #[test]
    fn tap_install_then_remove_restores_backup_byte_for_byte() {
        let dir = ClaudeHome::new("backup");
        let tap = dir.tap();
        fs::write(dir.settings(), ORIGINAL).unwrap();

        assert!(matches!(
            tap.install("stale"),
            Err(TapError::SettingsChanged)
        ));
        let preview = tap.preview().unwrap();
        assert!(preview.exists);
        assert_eq!(
            preview.chained,
            json!("printf 'orig:%s' \"$(cat | wc -c | tr -d ' ')\"")
        );
        tap.install(&preview.fingerprint).unwrap();
        assert!(tap.installed());
        let mode = fs::metadata(tap.record_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(fs::read_to_string(dir.settings())
            .unwrap()
            .contains("briardr-statusline-tap"));

        assert!(matches!(
            tap.install(&preview.fingerprint),
            Err(TapError::AlreadyInstalled)
        ));
        assert_eq!(tap.remove().unwrap(), Restored::Backup);
        assert_eq!(fs::read_to_string(dir.settings()).unwrap(), ORIGINAL);
        assert!(!tap.installed());
    }

    #[test]
    fn tap_remove_restores_only_the_status_line_when_settings_changed() {
        let dir = ClaudeHome::new("field");
        let tap = dir.tap();
        fs::write(dir.settings(), r#"{"model":"opus"}"#).unwrap();
        let preview = tap.preview().unwrap();
        tap.install(&preview.fingerprint).unwrap();
        fs::write(dir.settings(), r#"{"model":"sonnet"}"#).unwrap();

        assert_eq!(tap.remove().unwrap(), Restored::Field);
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(dir.settings()).unwrap()).unwrap();
        assert_eq!(settings, json!({ "model": "sonnet" }));
    }

    #[test]
    fn tap_without_settings_file_removes_the_file_again() {
        let dir = ClaudeHome::new("missing");
        let tap = dir.tap();
        fs::remove_dir_all(dir.0.join("claude")).unwrap();
        let preview = tap.preview().unwrap();
        assert!(!preview.exists);
        assert_eq!(preview.before, Value::Null);
        assert_eq!(preview.fingerprint, sha256_hex(b""));

        tap.install(&preview.fingerprint).unwrap();
        assert!(dir.settings().exists());
        assert_eq!(tap.remove().unwrap(), Restored::Backup);
        assert!(!dir.settings().exists());
    }

    #[test]
    fn tap_refuses_settings_that_already_point_at_briardr() {
        let dir = ClaudeHome::new("configured");
        let tap = dir.tap();
        let settings = r#"{"statusLine":{"type":"command","command":"'node' '/x/briardr-statusline-tap.mjs'"}}"#;
        fs::write(dir.settings(), settings).unwrap();
        let preview = tap.preview().unwrap();
        assert!(matches!(
            tap.install(&preview.fingerprint),
            Err(TapError::AlreadyConfigured)
        ));
        assert!(!tap.installed());
    }

    #[test]
    fn tap_source_ends_with_the_script_body() {
        assert!(TAP_SOURCE.starts_with("#!/usr/bin/env node\n"));
        assert!(TAP_SOURCE.ends_with("});\n"));
        assert!(TAP_SOURCE.contains("capturedAt: Date.now(), rate_limits: j.rate_limits"));
    }
}
