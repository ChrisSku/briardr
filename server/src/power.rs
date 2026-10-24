// Mac battery and Always on via pmset/caffeinate/sudo, low-battery
// sleep, and Sleep Mac. On other hosts nothing is run: the summary carries the host name only.
//
// The controller takes time as a `now` argument (ms) and has no timers of its own. The server calls
// `poll` every 60 s, `tick` every 5 s, and `release` on exit. Commands go through `Host`, so
// tests never touch the Mac's real sleep settings.
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{read_json, write_file_atomic};
use crate::diag::Diagnostics;
use crate::platform::{HostCapabilities, HostPlatform};

/// At or below this level on battery, Always on pauses and Briardr lets the Mac sleep.
pub const LOW_BATTERY_SLEEP: u32 = 10;
pub const LOW_BATTERY_LEVELS: [u32; 2] = [20, LOW_BATTERY_SLEEP];
/// Time for the 10% notification to go out before the Mac sleeps; plugging in meanwhile cancels.
const LOW_BATTERY_SLEEP_DELAY_MS: u64 = 30_000;
/// Gap between the sleep request's response and the sleep command, so the response has the best chance to arrive.
const SLEEP_LEAD_MS: u64 = 600;
/// If the Mac did not sleep after all, no wake comes: Always on resumes this long after the sleep command.
const RESUME_AFTER_SLEEP_MS: u64 = 120_000;
/// Retry the sudoers rule for `pmset disablesleep` this often, rather than on every poll.
const LID_RETRY_MS: u64 = 10 * 60_000;
/// Wall-clock jump beyond the tick interval that means the Mac was asleep.
const WAKE_GAP_MS: u64 = 30_000;
const CMD_TIMEOUT: Duration = Duration::from_secs(5);
const EXCLUDED_OWNERS: [&str; 5] = [
    "powerd",
    "kernel_task",
    "WindowServer",
    "coreaudiod",
    "sharingd",
];
const SLEEP_REQUEST_LIMIT: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    pub present: bool,
    pub percent: Option<u32>,
    /// "ac" | "battery" | "unknown"
    pub source: String,
    pub charging: bool,
    pub charged: bool,
    /// Minutes remaining (on battery) or until full (charging); None when unknown.
    pub minutes: Option<u32>,
    /// "remaining" | "until_full" | "calculating" | "unavailable"
    pub time_kind: String,
}

impl Battery {
    fn none() -> Self {
        Battery {
            present: false,
            percent: None,
            source: "unknown".into(),
            charging: false,
            charged: false,
            minutes: None,
            time_kind: "unavailable".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwake {
    /// Always on is turned on in Settings.
    pub wanted: bool,
    /// Briardr is holding the Mac awake right now (idle sleep).
    pub briardr: bool,
    /// Briardr has disabled system sleep, so the Mac also stays awake with the lid closed.
    pub lid_closed: bool,
    /// Always on is wanted but the sudoers rule for `pmset disablesleep` is missing.
    pub setup_needed: bool,
    /// Paused at 10% or less on battery.
    pub low_battery: bool,
    pub others: Vec<String>,
    pub sleep_disabled: bool,
}

impl KeepAwake {
    fn none() -> Self {
        KeepAwake {
            wanted: false,
            briardr: false,
            lid_closed: false,
            setup_needed: false,
            low_battery: false,
            others: Vec::new(),
            sleep_disabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MacSummary {
    pub name: String,
    pub battery: Battery,
    pub keep_awake: KeepAwake,
    pub as_of: u64,
}

/// What `nextBatteryAlert` decides: the level to alert for (if any) and the levels alerted so far.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AlertDecision {
    pub level: Option<u32>,
    pub alerted: Vec<u32>,
}

/// A low-battery notification the server should send.
#[derive(Debug, Clone, PartialEq)]
pub struct LowBatteryAlert {
    pub level: Option<u32>,
    pub percent: Option<u32>,
    pub sleeping: bool,
}

/// Things the server reacts to (push, live updates) that come out of `poll` and `tick`.
#[derive(Debug, Clone, PartialEq)]
pub enum PowerEvent {
    LowBattery(LowBatteryAlert),
    /// The wall clock jumped: the Mac was asleep for this long.
    Wake {
        gap_ms: u64,
    },
    /// Owners of keep-awake that were present before sleep and are gone after wake.
    KeepAwakeMissing(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct PollReport {
    pub summary: MacSummary,
    pub events: Vec<PowerEvent>,
}

/// The commands Power runs. The real one shells out; tests substitute a fake.
pub trait Host: Send {
    /// `pmset -g batt`
    fn pmset_batt(&mut self) -> String;
    /// `pmset -g assertions`
    fn pmset_assertions(&mut self) -> String;
    /// `pmset -g`
    fn pmset_settings(&mut self) -> String;
    /// `scutil --get ComputerName` ("" when unavailable).
    fn computer_name(&mut self) -> String;
    fn hostname(&mut self) -> String;
    /// `sudo -n /usr/bin/pmset disablesleep 1|0`; true when it succeeded.
    fn set_sleep_disabled(&mut self, on: bool) -> bool;
    /// `caffeinate -i -w <pid>`: holds idle sleep and lets go when Briardr exits.
    fn hold_idle_sleep(&mut self) -> Option<Box<dyn Hold>>;
    /// `pmset sleepnow`
    fn sleep_now(&mut self);
}

/// A running caffeinate.
pub trait Hold: Send {
    fn is_running(&mut self) -> bool;
    fn stop(&mut self);
}

/// The real commands. Tests use a fake `Host` instead.
pub struct SystemHost;

impl Host for SystemHost {
    fn pmset_batt(&mut self) -> String {
        capture("pmset", &["-g", "batt"]).1
    }
    fn pmset_assertions(&mut self) -> String {
        capture("pmset", &["-g", "assertions"]).1
    }
    fn pmset_settings(&mut self) -> String {
        capture("pmset", &["-g"]).1
    }
    fn computer_name(&mut self) -> String {
        capture("scutil", &["--get", "ComputerName"]).1
    }
    fn hostname(&mut self) -> String {
        capture("hostname", &[]).1.trim().to_string()
    }
    fn set_sleep_disabled(&mut self, on: bool) -> bool {
        let value = if on { "1" } else { "0" };
        run_quiet(
            "/usr/bin/sudo",
            &["-n", "/usr/bin/pmset", "disablesleep", value],
        )
    }
    fn hold_idle_sleep(&mut self) -> Option<Box<dyn Hold>> {
        let pid = std::process::id().to_string();
        let child = Command::new("caffeinate")
            .args(["-i", "-w", &pid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        Some(Box::new(CaffeinateHold(child)))
    }
    fn sleep_now(&mut self) {
        let _ = Command::new("pmset")
            .arg("sleepnow")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct CaffeinateHold(Child);

impl Hold for CaffeinateHold {
    fn is_running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs a command with a deadline. Returns whether it exited 0, and its stdout ("" if it could not run).
fn capture(cmd: &str, args: &[&str]) -> (bool, String) {
    let Ok(mut child) = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return (false, String::new());
    };
    // Read on a thread so a chatty command cannot fill the pipe and stall until the deadline.
    let pipe = child.stdout.take();
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut out);
        }
        String::from_utf8_lossy(&out).into_owned()
    });
    let ok = wait_with_deadline(&mut child);
    (ok, reader.join().unwrap_or_default())
}

fn run_quiet(cmd: &str, args: &[&str]) -> bool {
    match Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => wait_with_deadline(&mut child),
        Err(_) => false,
    }
}

fn wait_with_deadline(child: &mut Child) -> bool {
    let deadline = Instant::now() + CMD_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// `\n`-split lines, the way the regexes see them.
pub fn parse_pmset_batt(text: &str) -> Battery {
    let source = if text.contains("Now drawing from 'AC Power'") {
        "ac"
    } else if text.contains("Now drawing from 'Battery Power'") {
        "battery"
    } else {
        "unknown"
    };
    let Some((percent, state, rest)) = internal_battery(text) else {
        return Battery {
            present: false,
            percent: None,
            source: source.into(),
            charging: false,
            charged: source == "ac",
            minutes: None,
            time_kind: "unavailable".into(),
        };
    };
    let charging = state == "charging";
    let charged = state == "charged" || state == "finishing charge";
    let mut minutes = remaining_minutes(&rest);
    let mut time_kind = "unavailable";
    if rest.to_ascii_lowercase().contains("no estimate") {
        minutes = None;
        time_kind = "calculating";
    } else if minutes.is_some() {
        time_kind = if charging {
            "until_full"
        } else if source == "battery" {
            "remaining"
        } else {
            "unavailable"
        };
    }
    if minutes == Some(0) && !charged {
        minutes = None;
        time_kind = "calculating";
    }
    Battery {
        present: true,
        percent: Some(percent),
        source: source.into(),
        charging,
        charged,
        minutes,
        time_kind: time_kind.into(),
    }
}

/// The `InternalBattery` line: `\t<percent>%; <state>; <rest of line>`. Returns (percent, lowercased state, rest).
fn internal_battery(text: &str) -> Option<(u32, String, String)> {
    let mut from = 0;
    while let Some(rel) = text[from..].find("InternalBattery") {
        let start = from + rel;
        let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        for (tab, _) in text[start..line_end].match_indices('\t') {
            let at = start + tab + 1;
            let digits = text[at..].bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || !text[at + digits..].starts_with("%;") {
                continue;
            }
            let Ok(percent) = text[at..at + digits].parse::<u32>() else {
                continue;
            };
            let tail = text[at + digits + 2..].trim_start();
            let Some(semi) = tail.find(';') else {
                continue;
            };
            let state = tail[..semi].trim().to_ascii_lowercase();
            let rest = tail[semi + 1..]
                .trim_start()
                .split('\n')
                .next()
                .unwrap_or("");
            return Some((percent, state, rest.to_string()));
        }
        from = start + "InternalBattery".len();
    }
    None
}

/// `<h>:<mm> remaining` as minutes, leftmost match.
fn remaining_minutes(rest: &str) -> Option<u32> {
    let b = rest.as_bytes();
    let run_end = |from: usize| from + b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    for i in 0..b.len() {
        if !b[i].is_ascii_digit() {
            continue;
        }
        let h_end = run_end(i);
        if b.get(h_end) != Some(&b':') {
            continue;
        }
        let m_end = run_end(h_end + 1);
        if m_end == h_end + 1 || !rest[m_end..].starts_with(" remaining") {
            continue;
        }
        let (Ok(h), Ok(m)) = (
            rest[i..h_end].parse::<u32>(),
            rest[h_end + 1..m_end].parse::<u32>(),
        ) else {
            return None;
        };
        return Some(h * 60 + m);
    }
    None
}

/// Owners of PreventUserIdleSystemSleep / PreventSystemSleep assertions (excluding the OS itself), in order.
pub fn parse_keep_awake_owners(text: &str) -> Vec<String> {
    let mut owners: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let Some(name) = keep_awake_owner(line) else {
            continue;
        };
        if EXCLUDED_OWNERS.contains(&name) || owners.iter().any(|o| o == name) {
            continue;
        }
        owners.push(name.to_string());
    }
    owners
}

/// `pid <n>(<name>): ... PreventUserIdleSystemSleep|PreventSystemSleep`, all on one line.
fn keep_awake_owner(line: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(rel) = line[from..].find("pid ") {
        let digits_at = from + rel + 4;
        let digits = line[digits_at..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        from = digits_at;
        if digits == 0 {
            continue;
        }
        let paren = digits_at + digits;
        if !line[paren..].starts_with('(') {
            continue;
        }
        let name_start = paren + 1;
        let Some(close) = line[name_start..].find(')') else {
            continue;
        };
        let name = &line[name_start..name_start + close];
        let after = name_start + close + 1;
        if name.is_empty() || !line[after..].starts_with(':') {
            continue;
        }
        let tail = &line[after + 1..];
        if tail.contains("PreventUserIdleSystemSleep") || tail.contains("PreventSystemSleep") {
            return Some(name);
        }
    }
    None
}

/// Decide which low-battery level (if any) to alert for. Escalates and never repeats a level within one discharge.
pub fn next_battery_alert(b: &Battery, alerted: &[u32]) -> AlertDecision {
    let keep = || AlertDecision {
        level: None,
        alerted: alerted.to_vec(),
    };
    if !b.present {
        return keep();
    }
    let Some(percent) = b.percent else {
        return keep();
    };
    if b.source != "battery" || b.charging {
        // Back on power: reset once comfortably charged so the next discharge alerts again.
        let alerted = if percent > 25 || b.source == "ac" {
            Vec::new()
        } else {
            alerted.to_vec()
        };
        return AlertDecision {
            level: None,
            alerted,
        };
    }
    let due = LOW_BATTERY_LEVELS
        .iter()
        .copied()
        .filter(|l| percent <= *l && !alerted.contains(l));
    // Escalate straight to the most severe crossed level.
    let Some(level) = due.min() else {
        return keep();
    };
    let mut out: Vec<u32> = Vec::new();
    for l in alerted
        .iter()
        .copied()
        .chain(LOW_BATTERY_LEVELS.iter().copied().filter(|l| *l >= level))
    {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    AlertDecision {
        level: Some(level),
        alerted: out,
    }
}

/// Always on pauses at 10% or less on battery and stays paused until the battery is above 10% again.
pub fn low_battery_pause(b: &Battery, paused: bool) -> bool {
    if !b.present {
        return false;
    }
    match b.percent {
        Some(p) if p <= LOW_BATTERY_SLEEP => paused || b.source == "battery",
        _ => false,
    }
}

fn is_sleep_disabled(settings: &str) -> bool {
    settings.lines().any(|line| {
        line.trim_start()
            .strip_prefix("SleepDisabled")
            .is_some_and(|rest| {
                rest.starts_with(char::is_whitespace) && rest.trim_start().starts_with('1')
            })
    })
}

/// Host name without a local domain suffix (macOS and Linux differ on which suffixes appear).
fn strip_domain(name: &str) -> String {
    for suffix in [".local", ".fritz.box", ".lan", ".home"] {
        if let Some(s) = name.strip_suffix(suffix) {
            return s.to_string();
        }
    }
    name.to_string()
}

fn valid_request_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct PowerState {
    alerted: Vec<u32>,
    sleep_requests: BTreeMap<String, u64>,
    keep_awake_before_sleep: Option<KeepAwakeBeforeSleep>,
    /// Briardr set `pmset disablesleep 1` and owes the Mac a `disablesleep 0`, across restarts and crashes.
    disabled_sleep: bool,
    low_battery: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct KeepAwakeBeforeSleep {
    others: Vec<String>,
}

pub struct Power {
    state_file: PathBuf,
    state: PowerState,
    caps: HostCapabilities,
    diag: Arc<Diagnostics>,
    host: Box<dyn Host>,
    caffeinate: Option<Box<dyn Hold>>,
    wanted: bool,
    /// Between Sleep Mac and the next wake: hold nothing so the Mac can actually sleep.
    paused_for_sleep: bool,
    setup_needed: bool,
    lid_retry_at: u64,
    low_battery_sleep_at: Option<u64>,
    pending_sleep_at: Option<u64>,
    resume_at: Option<u64>,
    last_tick: u64,
    name_cache: Option<String>,
    summary: Option<MacSummary>,
}

impl Power {
    /// The state file is read here; a missing or unreadable one starts from defaults.
    pub fn new(
        state_file: PathBuf,
        caps: HostCapabilities,
        diag: Arc<Diagnostics>,
        host: Box<dyn Host>,
    ) -> Self {
        let state = read_json(&state_file, PowerState::default());
        Power {
            state_file,
            state,
            caps,
            diag,
            host,
            caffeinate: None,
            wanted: false,
            paused_for_sleep: false,
            setup_needed: false,
            lid_retry_at: 0,
            low_battery_sleep_at: None,
            pending_sleep_at: None,
            resume_at: None,
            last_tick: 0,
            name_cache: None,
            summary: None,
        }
    }

    /// A controller that runs the real commands.
    pub fn system(state_file: PathBuf, caps: HostCapabilities, diag: Arc<Diagnostics>) -> Self {
        Self::new(state_file, caps, diag, Box::new(SystemHost))
    }

    /// Whether this host has any power integration at all.
    pub fn supported(&self) -> bool {
        self.caps.battery || self.caps.keep_awake || self.caps.sleep
    }

    pub fn summary(&self) -> Option<&MacSummary> {
        self.summary.as_ref()
    }

    /// Set the Always on wish from the config, then take the first reading.
    pub fn start(&mut self, keep_awake: bool, now: u64) -> PollReport {
        self.wanted = keep_awake && self.caps.keep_awake;
        self.last_tick = now;
        self.poll(now)
    }

    /// Read the battery and the Mac's sleep state, and bring what Briardr holds in line with the settings.
    pub fn poll(&mut self, now: u64) -> PollReport {
        if !self.supported() {
            let summary = self.host_only_summary(now);
            return PollReport {
                summary,
                events: Vec::new(),
            };
        }
        self.reap_caffeinate();
        let battery = parse_pmset_batt(&self.host.pmset_batt());
        let asserts = self.host.pmset_assertions();
        let mut settings = self.host.pmset_settings();
        if self.state.disabled_sleep && !is_sleep_disabled(&settings) {
            // Cleared behind Briardr's back (another app quitting, a second Briardr): apply() sets it again.
            self.state.disabled_sleep = false;
            self.save();
        }
        let paused = low_battery_pause(&battery, self.state.low_battery);
        let pausing = paused && !self.state.low_battery;
        let decision = next_battery_alert(&battery, &self.state.alerted);
        if paused != self.state.low_battery || decision.alerted != self.state.alerted {
            if paused != self.state.low_battery {
                let ev = if paused {
                    "always_on_low_battery_pause"
                } else {
                    "always_on_low_battery_resume"
                };
                self.event(ev, json!({ "percent": battery.percent }));
            }
            self.state.low_battery = paused;
            self.state.alerted = decision.alerted.clone();
            self.save();
        }
        let sleeping = pausing && self.wanted;
        if sleeping && is_sleep_disabled(&settings) && !self.state.disabled_sleep {
            // Another app disabled sleep; at 10% the Mac sleeps anyway, as Always on promises.
            self.host.set_sleep_disabled(false);
        }
        if self.apply(now) || sleeping {
            settings = self.host.pmset_settings();
        }
        if sleeping {
            self.schedule_low_battery_sleep(now);
        }
        let others: Vec<String> = parse_keep_awake_owners(&asserts)
            .into_iter()
            .filter(|o| o != "caffeinate" || self.caffeinate.is_none())
            .collect();
        let summary = MacSummary {
            name: self.computer_name(),
            battery: battery.clone(),
            keep_awake: KeepAwake {
                wanted: self.wanted,
                briardr: self.caffeinate.is_some(),
                lid_closed: self.state.disabled_sleep,
                setup_needed: self.wanted && self.setup_needed,
                low_battery: self.state.low_battery,
                others,
                sleep_disabled: is_sleep_disabled(&settings),
            },
            as_of: now,
        };
        let mut events = Vec::new();
        if decision.level.is_some() || sleeping {
            events.push(PowerEvent::LowBattery(LowBatteryAlert {
                level: if sleeping {
                    Some(LOW_BATTERY_SLEEP)
                } else {
                    decision.level
                },
                percent: battery.percent,
                sleeping,
            }));
        }
        self.summary = Some(summary.clone());
        PollReport { summary, events }
    }

    /// Timed work and wake detection. Call every 5 s.
    pub fn tick(&mut self, now: u64) -> Vec<PowerEvent> {
        let mut events = Vec::new();
        let gap = now.saturating_sub(self.last_tick);
        self.last_tick = now;
        if gap > WAKE_GAP_MS {
            events.extend(self.on_wake(now, gap));
        }
        if self.low_battery_sleep_at.is_some_and(|at| now >= at) {
            self.low_battery_sleep_at = None;
            self.low_battery_sleep();
        }
        if self.pending_sleep_at.is_some_and(|at| now >= at) {
            self.pending_sleep_at = None;
            self.paused_for_sleep = true;
            self.apply(now);
            self.host.sleep_now();
            self.resume_at = Some(now + RESUME_AFTER_SLEEP_MS);
        }
        if self.resume_at.is_some_and(|at| now >= at) {
            self.resume_at = None;
            self.resume_after_sleep(now);
        }
        events
    }

    /// The Settings switch for Always on. Takes effect now.
    pub fn set_keep_awake(&mut self, on: bool, now: u64) {
        if !self.caps.keep_awake {
            return;
        }
        self.wanted = on;
        self.lid_retry_at = 0;
        self.apply(now);
    }

    /// Give back everything Briardr holds, before the process exits.
    pub fn release(&mut self) {
        if let Some(mut hold) = self.caffeinate.take() {
            hold.stop();
        }
        self.low_battery_sleep_at = None;
        // If this fails, the next start sees disabled_sleep and clears it.
        if self.caps.keep_awake && self.state.disabled_sleep && self.host.set_sleep_disabled(false)
        {
            self.state.disabled_sleep = false;
            self.save();
        }
    }

    /// Sleep the Mac once per request ID. The reply is returned before the command runs (see `tick`);
    /// a repeated request ID never re-runs the command.
    pub fn request_sleep(&mut self, request_id: &str, now: u64) -> Value {
        if !self.caps.sleep {
            return json!({ "ok": false, "error": "unsupported" });
        }
        if !valid_request_id(request_id) {
            return json!({ "ok": false, "error": "invalid_request" });
        }
        if let Some(at) = self.state.sleep_requests.get(request_id) {
            return json!({ "ok": true, "duplicate": true, "at": at });
        }
        // Briardr can lift its own disablesleep, not another app's.
        let others_disabled_sleep = self
            .summary
            .as_ref()
            .is_some_and(|s| s.keep_awake.sleep_disabled);
        if others_disabled_sleep && !self.state.disabled_sleep {
            return json!({ "ok": false, "error": "sleep_disabled" });
        }
        self.state
            .sleep_requests
            .insert(request_id.to_string(), now);
        // Keep the idempotency record small: the most recent requests only.
        let mut ids: Vec<(String, u64)> = self.state.sleep_requests.clone().into_iter().collect();
        ids.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        ids.truncate(SLEEP_REQUEST_LIMIT);
        self.state.sleep_requests = ids.into_iter().collect();
        let others = self
            .summary
            .as_ref()
            .map(|s| s.keep_awake.others.clone())
            .unwrap_or_default();
        self.state.keep_awake_before_sleep = Some(KeepAwakeBeforeSleep { others });
        self.save();
        self.event("mac_sleep_requested", json!({}));
        self.pending_sleep_at = Some(now + SLEEP_LEAD_MS);
        json!({ "ok": true, "duplicate": false })
    }

    fn save(&self) {
        if let Ok(text) = serde_json::to_string(&self.state) {
            let _ = write_file_atomic(&self.state_file, text.as_bytes(), 0o600);
        }
    }

    fn event(&self, name: &str, fields: Value) {
        let map: BTreeMap<String, Value> = match fields {
            Value::Object(m) => m.into_iter().collect(),
            _ => BTreeMap::new(),
        };
        self.diag.event(name, &map);
    }

    /// Forget a caffeinate that has exited on its own.
    fn reap_caffeinate(&mut self) {
        if self.caffeinate.as_mut().is_some_and(|c| !c.is_running()) {
            self.caffeinate = None;
        }
    }

    /// Bring what Briardr holds in line with what it should hold. Returns true if anything changed.
    fn apply(&mut self, now: u64) -> bool {
        self.reap_caffeinate();
        let on = self.wanted && !self.state.low_battery && !self.paused_for_sleep;
        let mut changed = false;
        if on {
            if self.caffeinate.is_none() {
                if let Some(hold) = self.host.hold_idle_sleep() {
                    self.caffeinate = Some(hold);
                    changed = true;
                }
            }
        } else if let Some(mut hold) = self.caffeinate.take() {
            hold.stop();
            changed = true;
        }
        if on && !self.state.disabled_sleep && now >= self.lid_retry_at {
            if self.host.set_sleep_disabled(true) {
                self.state.disabled_sleep = true;
                self.setup_needed = false;
                self.save();
                self.event("always_on_lid", json!({ "on": true }));
                changed = true;
            } else {
                // No sudoers rule: idle keep-awake still works. Retry later rather than on every poll.
                if !self.setup_needed {
                    self.event("always_on_lid_unavailable", json!({}));
                }
                self.setup_needed = true;
                self.lid_retry_at = now + LID_RETRY_MS;
            }
        } else if !on && self.state.disabled_sleep && self.host.set_sleep_disabled(false) {
            self.state.disabled_sleep = false;
            self.save();
            self.event("always_on_lid", json!({ "on": false }));
            changed = true;
        }
        changed
    }

    fn schedule_low_battery_sleep(&mut self, now: u64) {
        if self.low_battery_sleep_at.is_none() {
            self.low_battery_sleep_at = Some(now + LOW_BATTERY_SLEEP_DELAY_MS);
        }
    }

    /// Runs when the grace period ends. The battery is read again: plugging in cancels the sleep.
    fn low_battery_sleep(&mut self) {
        let b = parse_pmset_batt(&self.host.pmset_batt());
        if !self.state.low_battery || b.source != "battery" {
            self.event("low_battery_sleep_cancelled", json!({}));
            return;
        }
        self.event("low_battery_sleep", json!({ "percent": b.percent }));
        self.host.sleep_now();
    }

    fn resume_after_sleep(&mut self, now: u64) {
        if !self.paused_for_sleep {
            return;
        }
        self.paused_for_sleep = false;
        self.apply(now);
    }

    fn on_wake(&mut self, now: u64, gap_ms: u64) -> Vec<PowerEvent> {
        let gap_s = (gap_ms + 500) / 1000;
        self.event("mac_wake_detected", json!({ "gap_s": gap_s }));
        let before = self.state.keep_awake_before_sleep.take();
        if before.is_some() {
            self.save();
        }
        self.resume_after_sleep(now);
        let report = self.poll(now);
        let mut events = vec![PowerEvent::Wake { gap_ms }];
        if let Some(before) = before {
            let missing: Vec<String> = before
                .others
                .into_iter()
                .filter(|o| !report.summary.keep_awake.others.contains(o))
                .collect();
            if !missing.is_empty() {
                events.push(PowerEvent::KeepAwakeMissing(missing));
            }
        }
        events.extend(report.events);
        events
    }

    fn computer_name(&mut self) -> String {
        if let Some(name) = &self.name_cache {
            return name.clone();
        }
        let mac_name = if self.caps.platform == HostPlatform::Macos {
            self.host.computer_name().trim().to_string()
        } else {
            String::new()
        };
        let name = if mac_name.is_empty() {
            strip_domain(&self.host.hostname())
        } else {
            mac_name
        };
        self.name_cache = Some(name.clone());
        name
    }

    /// A host without power integration: its name, no battery, nothing held.
    fn host_only_summary(&mut self, now: u64) -> MacSummary {
        let summary = MacSummary {
            name: self.host.hostname(),
            battery: Battery::none(),
            keep_awake: KeepAwake::none(),
            as_of: now,
        };
        self.summary = Some(summary.clone());
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::capabilities;
    use crate::scratch::Scratch;
    use std::fs;
    use std::sync::Mutex;

    fn fixture_cases(func: &str) -> Vec<(String, Value)> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/power");
        let mut out: Vec<(String, Value)> = fs::read_dir(&dir)
            .expect("power fixtures exist")
            .map(|e| e.expect("entry").path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&format!("{func}.")))
            })
            .map(|p| {
                let text = fs::read_to_string(&p).expect("read fixture");
                let v: Value = serde_json::from_str(&text).expect("fixture json");
                (p.display().to_string(), v)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(!out.is_empty(), "no fixtures for {func}");
        out
    }

    #[test]
    fn parse_pmset_batt_matches_fixtures() {
        for (path, f) in fixture_cases("parsePmsetBatt") {
            let text = f["input"][0].as_str().expect("text input");
            let got = serde_json::to_value(parse_pmset_batt(text)).unwrap();
            assert_eq!(got, f["output"], "{path}");
        }
    }

    #[test]
    fn parse_keep_awake_owners_matches_fixtures() {
        for (path, f) in fixture_cases("parseKeepAwakeOwners") {
            let text = f["input"][0].as_str().expect("text input");
            let got = serde_json::to_value(parse_keep_awake_owners(text)).unwrap();
            assert_eq!(got, f["output"], "{path}");
        }
    }

    #[test]
    fn next_battery_alert_matches_fixtures() {
        for (path, f) in fixture_cases("nextBatteryAlert") {
            let b: Battery = serde_json::from_value(f["input"][0].clone()).unwrap();
            let alerted: Vec<u32> = serde_json::from_value(f["input"][1].clone()).unwrap();
            let got = serde_json::to_value(next_battery_alert(&b, &alerted)).unwrap();
            assert_eq!(got, f["output"], "{path}");
        }
    }

    #[test]
    fn low_battery_pause_matches_fixtures() {
        for (path, f) in fixture_cases("lowBatteryPause") {
            let b: Battery = serde_json::from_value(f["input"][0].clone()).unwrap();
            let paused = f["input"][1].as_bool().expect("bool input");
            assert_eq!(
                Value::Bool(low_battery_pause(&b, paused)),
                f["output"],
                "{path}"
            );
        }
    }

    #[derive(Default)]
    struct World {
        batt: String,
        asserts: String,
        settings: String,
        sudo_ok: bool,
        name: String,
        calls: Vec<String>,
    }

    type Shared = Arc<Mutex<World>>;

    struct FakeHost(Shared);
    struct FakeHold(Shared);

    impl Hold for FakeHold {
        fn is_running(&mut self) -> bool {
            true
        }
        fn stop(&mut self) {
            self.0.lock().unwrap().calls.push("caffeinate-stop".into());
        }
    }

    impl Host for FakeHost {
        fn pmset_batt(&mut self) -> String {
            let mut w = self.0.lock().unwrap();
            w.calls.push("pmset-batt".into());
            w.batt.clone()
        }
        fn pmset_assertions(&mut self) -> String {
            let mut w = self.0.lock().unwrap();
            w.calls.push("pmset-assertions".into());
            w.asserts.clone()
        }
        fn pmset_settings(&mut self) -> String {
            let mut w = self.0.lock().unwrap();
            w.calls.push("pmset-settings".into());
            w.settings.clone()
        }
        fn computer_name(&mut self) -> String {
            let mut w = self.0.lock().unwrap();
            w.calls.push("scutil".into());
            w.name.clone()
        }
        fn hostname(&mut self) -> String {
            let mut w = self.0.lock().unwrap();
            w.calls.push("hostname".into());
            "host.local".into()
        }
        fn set_sleep_disabled(&mut self, on: bool) -> bool {
            let mut w = self.0.lock().unwrap();
            w.calls.push(format!("sudo-disablesleep-{}", on as u8));
            if w.sudo_ok {
                w.settings = format!("SleepDisabled\t{}\n", on as u8);
            }
            w.sudo_ok
        }
        fn hold_idle_sleep(&mut self) -> Option<Box<dyn Hold>> {
            self.0.lock().unwrap().calls.push("caffeinate-start".into());
            Some(Box::new(FakeHold(self.0.clone())))
        }
        fn sleep_now(&mut self) {
            self.0.lock().unwrap().calls.push("sleepnow".into());
        }
    }

    fn batt(percent: u32, ac: bool) -> String {
        if ac {
            format!("Now drawing from 'AC Power'\n -InternalBattery-0 (id=1234567)\t{percent}%; charging; 1:05 remaining present: true")
        } else {
            format!("Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1234567)\t{percent}%; discharging; 2:15 remaining present: true")
        }
    }

    fn world(batt: String, settings: &str, sudo_ok: bool) -> Shared {
        Arc::new(Mutex::new(World {
            batt,
            asserts: "No assertions.".into(),
            settings: settings.into(),
            sudo_ok,
            name: "Studio".into(),
            calls: Vec::new(),
        }))
    }

    fn count(w: &Shared, call: &str) -> usize {
        w.lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.as_str() == call)
            .count()
    }

    /// The Power under test, and its scratch directory: keep that until the test ends.
    fn setup(
        name: &str,
        platform: HostPlatform,
        w: Shared,
        state: Option<&str>,
    ) -> (Scratch, Power) {
        let dir = Scratch::new(&format!("power-{name}"));
        let diag = Arc::new(Diagnostics::new(dir.join("logs"), dir.join("salt")).unwrap());
        let state_file = dir.join("power.json");
        if let Some(text) = state {
            fs::write(&state_file, text).unwrap();
        }
        let power = Power::new(
            state_file,
            capabilities(platform),
            diag,
            Box::new(FakeHost(w)),
        );
        (dir, power)
    }

    #[test]
    fn linux_host_runs_no_power_commands() {
        let w = world(String::new(), "", false);
        let (_dir, mut p) = setup("linux", HostPlatform::Linux, w.clone(), None);
        let report = p.start(true, 0);
        assert!(!report.summary.battery.present);
        assert_eq!(report.summary.name, "host.local");
        p.set_keep_awake(true, 0);
        assert_eq!(
            p.request_sleep("abcdefgh", 0),
            json!({ "ok": false, "error": "unsupported" })
        );
        p.tick(120_000);
        p.release();
        let calls = w.lock().unwrap().calls.clone();
        assert!(!calls.is_empty());
        assert!(calls.iter().all(|c| c == "hostname"), "{calls:?}");
    }

    #[test]
    fn always_on_holds_idle_sleep_and_lid_closed_until_switched_off() {
        let w = world(batt(80, true), "SleepDisabled\t0\n", true);
        let (_dir, mut p) = setup("awake", HostPlatform::Macos, w.clone(), None);
        p.start(true, 0);
        let s = p.summary().unwrap().clone();
        assert_eq!(s.name, "Studio");
        assert!(s.keep_awake.wanted && s.keep_awake.briardr && s.keep_awake.lid_closed);
        assert!(s.keep_awake.sleep_disabled && !s.keep_awake.setup_needed);
        p.set_keep_awake(false, 1_000);
        assert_eq!(count(&w, "caffeinate-stop"), 1);
        assert_eq!(count(&w, "sudo-disablesleep-0"), 1);
    }

    #[test]
    fn missing_sudoers_rule_keeps_idle_hold_and_retries_later() {
        let w = world(batt(80, true), "SleepDisabled\t0\n", false);
        let (_dir, mut p) = setup("sudoers", HostPlatform::Macos, w.clone(), None);
        p.start(true, 0);
        let s = p.summary().unwrap();
        assert!(s.keep_awake.setup_needed && s.keep_awake.briardr);
        assert!(!s.keep_awake.lid_closed);
        p.poll(300_000);
        assert_eq!(count(&w, "sudo-disablesleep-1"), 1);
        p.poll(600_000);
        assert_eq!(count(&w, "sudo-disablesleep-1"), 2);
    }

    #[test]
    fn low_battery_pauses_then_sleeps_after_the_grace_period() {
        let w = world(batt(9, false), "SleepDisabled\t0\n", true);
        let (_dir, mut p) = setup("low", HostPlatform::Macos, w.clone(), None);
        let report = p.start(true, 0);
        assert!(report.summary.keep_awake.low_battery);
        assert!(!report.summary.keep_awake.briardr);
        assert!(report
            .events
            .contains(&PowerEvent::LowBattery(LowBatteryAlert {
                level: Some(LOW_BATTERY_SLEEP),
                percent: Some(9),
                sleeping: true,
            })));
        p.tick(29_999);
        assert_eq!(count(&w, "sleepnow"), 0);
        p.tick(30_000);
        assert_eq!(count(&w, "sleepnow"), 1);
    }

    #[test]
    fn plugging_in_cancels_the_low_battery_sleep() {
        let w = world(batt(9, false), "SleepDisabled\t0\n", true);
        let (_dir, mut p) = setup("plugged", HostPlatform::Macos, w.clone(), None);
        p.start(true, 0);
        w.lock().unwrap().batt = batt(9, true);
        p.tick(30_000);
        assert_eq!(count(&w, "sleepnow"), 0);
    }

    #[test]
    fn sleep_request_runs_once_per_id() {
        let w = world(batt(80, true), "SleepDisabled\t0\n", true);
        let (_dir, mut p) = setup("sleep", HostPlatform::Macos, w.clone(), None);
        p.start(false, 0);
        assert_eq!(
            p.request_sleep("bad id!", 0),
            json!({ "ok": false, "error": "invalid_request" })
        );
        assert_eq!(
            p.request_sleep("req-123456", 1_000),
            json!({ "ok": true, "duplicate": false })
        );
        assert_eq!(
            p.request_sleep("req-123456", 1_500),
            json!({ "ok": true, "duplicate": true, "at": 1000 })
        );
        p.tick(1_600);
        p.tick(2_000);
        p.tick(3_000);
        assert_eq!(count(&w, "sleepnow"), 1);
    }

    #[test]
    fn sleep_is_refused_when_another_app_disabled_sleep() {
        let w = world(batt(80, true), "SleepDisabled\t1\n", true);
        let (_dir, mut p) = setup("refused", HostPlatform::Macos, w.clone(), None);
        p.start(false, 0);
        assert_eq!(
            p.request_sleep("req-123456", 0),
            json!({ "ok": false, "error": "sleep_disabled" })
        );
        assert_eq!(count(&w, "sleepnow"), 0);
    }

    #[test]
    fn release_gives_back_disablesleep_and_caffeinate() {
        let w = world(batt(80, true), "SleepDisabled\t0\n", true);
        let (_dir, mut p) = setup("release", HostPlatform::Macos, w.clone(), None);
        p.start(true, 0);
        p.release();
        assert_eq!(count(&w, "caffeinate-stop"), 1);
        assert_eq!(count(&w, "sudo-disablesleep-0"), 1);
    }

    #[test]
    fn leftover_disablesleep_from_a_crash_is_cleared_on_start() {
        let w = world(batt(80, true), "SleepDisabled\t0\n", true);
        let (_dir, mut p) = setup(
            "crash",
            HostPlatform::Macos,
            w.clone(),
            Some(r#"{"disabledSleep":true}"#),
        );
        let report = p.start(false, 0);
        assert!(!report.summary.keep_awake.lid_closed);
    }

    #[test]
    fn keep_awake_owner_missing_after_wake_is_reported() {
        let w = world(batt(80, true), "SleepDisabled\t0\n", true);
        w.lock().unwrap().asserts =
            "pid 77(Herdr): PreventUserIdleSystemSleep named: \"herdr\"".into();
        let (_dir, mut p) = setup("wake", HostPlatform::Macos, w.clone(), None);
        p.start(false, 0);
        p.request_sleep("req-123456", 1_000);
        p.tick(1_600);
        w.lock().unwrap().asserts = "No assertions.".into();
        let events = p.tick(1_600 + 60_000);
        assert!(events.contains(&PowerEvent::Wake { gap_ms: 60_000 }));
        assert!(events.contains(&PowerEvent::KeepAwakeMissing(vec!["Herdr".into()])));
    }
}
