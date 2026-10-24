// Usage reports for Claude and Codex.
// - Reports persist with their original capture time, across restarts, offline, and after failures.
// - Concurrent requests share one in-flight read per provider.
// - A 3-minute cache; failures back off exponentially; rate-limit cooldowns always hold, even for
//   manual refresh (which otherwise bypasses the cache and failure backoff).
// - Using a Codex limit reset credit happens only on an explicit, confirmed request from the user.
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::config::{read_json, write_file_atomic};
use crate::diag::Diagnostics;

pub mod claude;
pub mod codex;
pub mod types;

use claude::StatuslineTap;
use codex::{ResetOutcome, UsageError};
use types::{LastError, ProviderReport, ProviderStatus};

/// Cache window for provider snapshots.
pub const CACHE: Duration = Duration::from_secs(3 * 60);
const CACHE_MS: u64 = 3 * 60 * 1000;
const MAX_BACKOFF_MS: u64 = 30 * 60 * 1000;

pub type ReportFuture = Pin<Box<dyn Future<Output = Result<ProviderReport, UsageError>> + Send>>;
pub type ResetFuture = Pin<Box<dyn Future<Output = Result<ResetOutcome, UsageError>> + Send>>;

/// Settings and (for tests) replacements for the network reads.
pub struct UsageDeps {
    pub claude_enabled: Box<dyn Fn() -> bool + Send + Sync>,
    pub codex_enabled: Box<dyn Fn() -> bool + Send + Sync>,
    pub codex_path: Box<dyn Fn() -> String + Send + Sync>,
    pub fetch_claude: Option<Box<dyn Fn() -> ReportFuture + Send + Sync>>,
    pub fetch_codex: Option<Box<dyn Fn() -> ReportFuture + Send + Sync>>,
    pub consume_codex_reset: Option<Box<dyn Fn() -> ResetFuture + Send + Sync>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn name(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }
}

/// usage.json: `{claude, codex}`; a partial or missing file starts from the defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    claude: ProviderStatus,
    #[serde(default)]
    codex: ProviderStatus,
}

struct State {
    status: Saved,
    /// A read in progress per provider; waiters see `true` when it is done.
    inflight: [Option<watch::Receiver<bool>>; 2],
}

pub struct Usage {
    file: PathBuf,
    state: Mutex<State>,
    deps: UsageDeps,
    diag: Arc<Diagnostics>,
    pub tap: StatuslineTap,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn slot(p: Provider) -> usize {
    match p {
        Provider::Claude => 0,
        Provider::Codex => 1,
    }
}

impl Usage {
    pub fn new(file: PathBuf, tap: StatuslineTap, deps: UsageDeps, diag: Arc<Diagnostics>) -> Self {
        let status = read_json::<Saved>(&file, Saved::default());
        Usage {
            file,
            state: Mutex::new(State {
                status,
                inflight: [None, None],
            }),
            deps,
            diag,
            tap,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn status_mut(st: &mut State, p: Provider) -> &mut ProviderStatus {
        match p {
            Provider::Claude => &mut st.status.claude,
            Provider::Codex => &mut st.status.codex,
        }
    }

    fn save(&self) {
        let data = {
            let st = self.lock();
            serde_json::to_string_pretty(&st.status).unwrap_or_default()
        };
        let _ = write_file_atomic(&self.file, data.as_bytes(), 0o600);
    }

    fn event(&self, ev: &str, v: Value) {
        let fields: BTreeMap<String, Value> = match v {
            Value::Object(m) => m.into_iter().collect(),
            _ => BTreeMap::new(),
        };
        self.diag.event(ev, &fields);
    }

    /// `{claude, codex}` with each provider's status plus `tapReport` and `tapInstalled`.
    pub fn snapshot(&self) -> Value {
        let tap_installed = self.tap.installed();
        let tap_report = self.tap.latest();
        let (claude, codex) = {
            let st = self.lock();
            (st.status.claude.clone(), st.status.codex.clone())
        };
        let with = |s: ProviderStatus,
                    enabled: bool,
                    tap_report: Option<ProviderReport>,
                    installed: bool| {
            let mut v = serde_json::to_value(s).unwrap_or_else(|_| json!({}));
            if let Value::Object(m) = &mut v {
                m.insert("enabled".into(), json!(enabled));
                m.insert("tapReport".into(), json!(tap_report));
                m.insert("tapInstalled".into(), json!(installed));
            }
            v
        };
        json!({
            "claude": with(claude, (self.deps.claude_enabled)() || tap_installed, tap_report, tap_installed),
            "codex": with(codex, (self.deps.codex_enabled)(), None, false),
        })
    }

    pub async fn refresh(&self, force: bool) -> Value {
        tokio::join!(
            self.refresh_one(Provider::Claude, force),
            self.refresh_one(Provider::Codex, force)
        );
        self.snapshot()
    }

    /// Use one Codex limit reset credit, then read usage again so the new numbers show.
    pub async fn use_codex_reset_credit(&self) -> Result<ResetOutcome, UsageError> {
        if !(self.deps.codex_enabled)() {
            return Err(UsageError::new(
                "not_enabled",
                "Codex usage is turned off in Settings",
            ));
        }
        let outcome = match &self.deps.consume_codex_reset {
            Some(f) => f().await?,
            None => codex::consume_codex_reset_credit(&(self.deps.codex_path)()).await?,
        };
        let category: String = outcome
            .as_str()
            .chars()
            .flat_map(|c| {
                if c.is_ascii_uppercase() {
                    vec!['_', c.to_ascii_lowercase()]
                } else {
                    vec![c]
                }
            })
            .collect();
        self.event("usage_reset_credit", json!({ "category": category }));
        // A reset changes the limits; read them now.
        self.lock().status.codex.rate_limited_until = None;
        self.refresh_one(Provider::Codex, true).await;
        Ok(outcome)
    }

    fn enabled(&self, p: Provider) -> bool {
        match p {
            Provider::Claude => (self.deps.claude_enabled)(),
            Provider::Codex => (self.deps.codex_enabled)(),
        }
    }

    async fn fetch(&self, p: Provider) -> Result<ProviderReport, UsageError> {
        match p {
            Provider::Claude => match &self.deps.fetch_claude {
                Some(f) => f().await,
                None => claude::fetch_claude_oauth().await,
            },
            Provider::Codex => match &self.deps.fetch_codex {
                Some(f) => f().await,
                None => {
                    let r = codex::read_codex_rate_limits(&(self.deps.codex_path)()).await?;
                    Ok(codex::parse_codex(&r, now_ms()))
                }
            },
        }
    }

    async fn refresh_one(&self, p: Provider, force: bool) {
        if !self.enabled(p) {
            return;
        }
        enum Next {
            Skip,
            Wait(watch::Receiver<bool>),
            Run(watch::Sender<bool>),
        }
        let next = {
            let mut st = self.lock();
            let now = now_ms();
            let s = Self::status_mut(&mut st, p);
            let fresh = s.report.is_some()
                && now.saturating_sub(s.last_attempt_at.unwrap_or(0)) < CACHE_MS
                && s.last_error.is_none();
            // A rate-limit cooldown always holds; the cache and failure backoff only without force.
            let rate_limited = s.rate_limited_until.is_some_and(|t| now < t);
            let cooling = s.cooldown_until.is_some_and(|t| now < t);
            if rate_limited || (!force && (fresh || cooling)) {
                Next::Skip
            } else if let Some(rx) = st.inflight[slot(p)].clone() {
                Next::Wait(rx)
            } else {
                let (tx, rx) = watch::channel(false);
                st.inflight[slot(p)] = Some(rx);
                Self::status_mut(&mut st, p).last_attempt_at = Some(now);
                Next::Run(tx)
            }
        };
        let done_tx = match next {
            Next::Skip => return,
            Next::Wait(mut rx) => {
                let _ = rx.wait_for(|done| *done).await;
                return;
            }
            Next::Run(tx) => tx,
        };
        let t0 = now_ms();
        let result = self.fetch(p).await;
        {
            let mut st = self.lock();
            let s = Self::status_mut(&mut st, p);
            match result {
                Ok(report) => {
                    s.report = Some(report);
                    s.last_error = None;
                    s.failures = 0;
                    s.cooldown_until = None;
                    s.rate_limited_until = None;
                    drop(st);
                    self.event(
                        "usage_ok",
                        json!({"provider": p.name(), "ms": now_ms().saturating_sub(t0)}),
                    );
                }
                Err(e) => {
                    let now = now_ms();
                    s.failures += 1;
                    s.last_error = Some(LastError {
                        category: e.category.clone(),
                        message: e.message.clone(),
                        at: now,
                    });
                    let backoff = MAX_BACKOFF_MS
                        .min(30_000u64.saturating_mul(1u64 << (s.failures - 1).min(32)));
                    s.cooldown_until = Some(now + backoff);
                    if e.category == "rate_limited" {
                        s.rate_limited_until =
                            Some(now + e.retry_after_ms.filter(|ms| *ms > 0).unwrap_or(300_000));
                    }
                    drop(st);
                    self.event(
                        "usage_fail",
                        json!({"provider": p.name(), "category": e.category, "backoff_ms": backoff}),
                    );
                }
            }
        }
        self.save();
        self.lock().inflight[slot(p)] = None;
        let _ = done_tx.send(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn report(at: u64) -> ProviderReport {
        ProviderReport {
            provider: "claude".into(),
            source: "oauth".into(),
            windows: vec![],
            credits: None,
            reset_credits: None,
            spend: Some(Value::Null),
            plan: None,
            limit_reached: None,
            captured_at: json!(at),
        }
    }

    fn usage(dir: &Scratch, calls: Arc<AtomicU32>, fail: Option<UsageError>) -> Usage {
        let diag = Arc::new(Diagnostics::new(dir.join("logs"), dir.join("salt")).unwrap());
        let tap = StatuslineTap::new(dir.join("claude"), dir.join("state"), dir.join("bin"));
        let deps = UsageDeps {
            claude_enabled: Box::new(|| true),
            codex_enabled: Box::new(|| false),
            codex_path: Box::new(|| "codex".into()),
            fetch_claude: Some(Box::new(move || {
                let calls = calls.clone();
                let fail = fail.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    match fail {
                        Some(e) => Err(e),
                        None => Ok(report(1)),
                    }
                })
            })),
            fetch_codex: None,
            consume_codex_reset: None,
        };
        Usage::new(dir.join("usage.json"), tap, deps, diag)
    }

    #[tokio::test]
    async fn caches_and_shares_inflight_reads() {
        let dir = Scratch::new("usage-cache");
        let calls = Arc::new(AtomicU32::new(0));
        let u = usage(&dir, calls.clone(), None);
        tokio::join!(u.refresh(false), u.refresh(true));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "concurrent reads share one fetch"
        );
        u.refresh(false).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fresh report is served from cache"
        );
        u.refresh(true).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "manual refresh bypasses the cache"
        );
        let saved: Value =
            serde_json::from_slice(&std::fs::read(dir.join("usage.json")).unwrap()).unwrap();
        assert_eq!(saved["claude"]["report"]["capturedAt"], json!(1));
        assert_eq!(saved["codex"]["failures"], json!(0));
    }

    #[tokio::test]
    async fn rate_limits_hold_even_for_manual_refresh() {
        let dir = Scratch::new("usage-rl");
        let calls = Arc::new(AtomicU32::new(0));
        let err = UsageError {
            category: "rate_limited".into(),
            message: "wait".into(),
            retry_after_ms: Some(60_000),
        };
        let u = usage(&dir, calls.clone(), Some(err));
        let snap = u.refresh(true).await;
        assert_eq!(snap["claude"]["failures"], json!(1));
        assert_eq!(
            snap["claude"]["lastError"]["category"],
            json!("rate_limited")
        );
        assert_eq!(snap["claude"]["tapInstalled"], json!(false));
        let until = snap["claude"]["rateLimitedUntil"].as_u64().unwrap();
        let cooldown = snap["claude"]["cooldownUntil"].as_u64().unwrap();
        let attempt = snap["claude"]["lastAttemptAt"].as_u64().unwrap();
        assert!(until >= attempt + 60_000 && cooldown >= attempt + 30_000);
        u.refresh(true).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reset_credit_refused_when_codex_is_off() {
        let dir = Scratch::new("usage-reset");
        let u = usage(&dir, Arc::new(AtomicU32::new(0)), None);
        let e = u.use_codex_reset_credit().await.unwrap_err();
        assert_eq!(e.category, "not_enabled");
    }
}
