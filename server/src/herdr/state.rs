// Live, cached view of the Herdr session.
// Strategy (per Herdr's socket docs): subscribe first, then read authoritative snapshots; treat events
// as invalidation signals; serialize refreshes; resubscribe whenever the pane set changes or the
// subscription ends. Agent status events are applied immediately so the UI and monitor react fast.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::sleep;

use super::model::{normalize, AgentState, Model, Stream, StreamKind};
use super::rpc::{
    herdr_request, herdr_status, herdr_subscribe, HerdrError, HerdrStatus, HerdrTarget, SubEvent,
};
use crate::diag::Diagnostics;

const LIFECYCLE: [&str; 17] = [
    "workspace.created",
    "workspace.updated",
    "workspace.renamed",
    "workspace.closed",
    "workspace.moved",
    "workspace.reordered",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.moved",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.moved",
    "pane.exited",
    "pane.agent_detected",
    "layout.updated",
];
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const MIN_RETRY: Duration = Duration::from_millis(500);
const MAX_RETRY: Duration = Duration::from_secs(15);
const RESUBSCRIBE_DELAY: Duration = Duration::from_millis(300);
const EVENT_DEBOUNCE: Duration = Duration::from_millis(120);
const EVENT_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HerdrHealth {
    pub ok: bool,
    pub problem: Option<String>,
    pub version: Option<String>,
    pub last_ok_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub stream_id: String,
    pub terminal_id: String,
    pub from: Option<AgentState>,
    pub to: Option<AgentState>,
    pub kind: StreamKind,
    pub tool: String,
    pub state_change_seq: Option<u64>,
    pub at: u64,
}

/// What the rest of the server hears from Herdr state: the `model`, `transition` and `health` events.
#[derive(Debug, Clone, PartialEq)]
pub enum HerdrEvent {
    Model(Model),
    Transition(Transition),
    Health(HerdrHealth),
}

/// The read-only surface. The notification monitor only ever receives this.
pub trait HerdrReader: Send + Sync {
    fn model(&self) -> Option<Model>;
    fn health(&self) -> HerdrHealth;
    fn events(&self) -> broadcast::Receiver<HerdrEvent>;
}

struct Inner {
    target: HerdrTarget,
    socket: Option<PathBuf>,
    model: Option<Model>,
    health: HerdrHealth,
    /// Id of the subscription whose events count (the newest one). Older ones close on their first event.
    sub_id: Option<u64>,
    /// Pane set the current subscription was built for.
    sub_panes: String,
    next_sub_id: u64,
    /// Close handles for every subscription still open (a new one starts before the old one is closed).
    subs: HashMap<u64, Box<dyn FnOnce() + Send>>,
    retry_delay: Duration,
    retry_task: Option<JoinHandle<()>>,
    debounce: Option<JoinHandle<()>>,
    timers: Vec<JoinHandle<()>>,
    stopped: bool,
    process_names: HashMap<String, String>,
}

struct Shared {
    inner: Mutex<Inner>,
    /// Serializes refreshes: a refresh never overlaps another one.
    refresh_lock: tokio::sync::Mutex<()>,
    diag: Arc<Diagnostics>,
    events: broadcast::Sender<HerdrEvent>,
}

/// Cheap to clone. Clones share one state, so spawned tasks can hold their own handle.
#[derive(Clone)]
pub struct HerdrState {
    shared: Arc<Shared>,
}

impl HerdrState {
    pub fn new(target: HerdrTarget, diag: Arc<Diagnostics>) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        HerdrState {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    target,
                    socket: None,
                    model: None,
                    health: HerdrHealth {
                        ok: false,
                        problem: None,
                        version: None,
                        last_ok_at: None,
                    },
                    sub_id: None,
                    sub_panes: String::new(),
                    next_sub_id: 0,
                    subs: HashMap::new(),
                    retry_delay: MIN_RETRY,
                    retry_task: None,
                    debounce: None,
                    timers: Vec::new(),
                    stopped: false,
                    process_names: HashMap::new(),
                }),
                refresh_lock: tokio::sync::Mutex::new(()),
                diag,
                events,
            }),
        }
    }

    pub fn herdr_target(&self) -> HerdrTarget {
        self.lock().target.clone()
    }

    pub fn socket(&self) -> Option<PathBuf> {
        self.lock().socket.clone()
    }

    pub async fn start(&self) {
        self.lock().stopped = false;
        self.connect().await;
        let this = self.clone();
        let timer = tokio::spawn(async move {
            loop {
                sleep(REFRESH_INTERVAL).await;
                this.request_refresh(Duration::ZERO);
            }
        });
        self.lock().timers.push(timer);
    }

    pub fn stop(&self) {
        let (timers, retry, debounce, closes) = {
            let mut i = self.lock();
            i.stopped = true;
            i.sub_id = None;
            let closes: Vec<Box<dyn FnOnce() + Send>> = i.subs.drain().map(|(_, c)| c).collect();
            (
                std::mem::take(&mut i.timers),
                i.retry_task.take(),
                i.debounce.take(),
                closes,
            )
        };
        for t in timers {
            t.abort();
        }
        if let Some(t) = retry {
            t.abort();
        }
        if let Some(t) = debounce {
            t.abort();
        }
        for close in closes {
            close();
        }
    }

    /// Point Briardr at a different Herdr executable or session at runtime.
    pub async fn retarget(&self, target: HerdrTarget) {
        self.stop();
        {
            let mut i = self.lock();
            i.target = target;
            i.socket = None;
            i.model = None;
            i.sub_panes.clear();
        }
        self.start().await;
    }

    /// Sends one request to Herdr over a fresh connection.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, HerdrError> {
        let (socket, timeout) = {
            let i = self.lock();
            (i.socket.clone(), i.target.timeout)
        };
        let Some(socket) = socket else {
            return Err(HerdrError {
                code: "unreachable".to_string(),
                message: "Herdr is not reachable".to_string(),
            });
        };
        match herdr_request(&socket, method, params, timeout).await {
            Ok(r) => Ok(r),
            Err(e) => {
                if matches!(e.code.as_str(), "unreachable" | "timeout" | "socket") {
                    let problem = if e.code == "timeout" {
                        "timeout"
                    } else {
                        "unreachable"
                    };
                    self.patch_health(|h| {
                        h.ok = false;
                        h.problem = Some(problem.to_string());
                    });
                    self.schedule_retry();
                }
                Err(e)
            }
        }
    }

    /// Coalesces refresh requests: while one is pending, further requests are dropped.
    pub fn request_refresh(&self, delay: Duration) {
        let mut i = self.lock();
        if i.stopped || i.debounce.is_some() {
            return;
        }
        let this = self.clone();
        let handle = tokio::spawn(async move {
            sleep(delay).await;
            this.lock().debounce = None;
            let _ = this.refresh().await;
        });
        i.debounce = Some(handle);
    }

    /// Serialized authoritative refresh. Callers queue behind a running refresh.
    pub async fn refresh(&self) -> Result<(), HerdrError> {
        let _guard = self.shared.refresh_lock.lock().await;
        self.refresh_once().await
    }

    async fn refresh_once(&self) -> Result<(), HerdrError> {
        let t0 = now_ms();
        let res = self.request("session.snapshot", json!({})).await?;
        let Some(snap) = res.get("snapshot").filter(|s| s.is_object()) else {
            return Err(HerdrError {
                code: "invalid".to_string(),
                message: "Herdr snapshot missing".to_string(),
            });
        };
        let agent_panes: HashSet<&str> = list(snap, "agents")
            .iter()
            .filter_map(|a| a.get("pane_id").and_then(Value::as_str))
            .collect();
        // Foreground process names tell shells, editors and other programs apart. Agents don't need it.
        let others: Vec<String> = list(snap, "panes")
            .iter()
            .filter(|p| {
                !p.get("pane_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| agent_panes.contains(id))
            })
            .filter(|p| !p.get("agent").is_some_and(truthy))
            .filter_map(|p| p.get("pane_id").and_then(Value::as_str).map(str::to_string))
            .collect();
        let mut lookups = JoinSet::new();
        for pane in others {
            let this = self.clone();
            lookups.spawn(async move {
                let r = this
                    .request("pane.process_info", json!({ "pane_id": pane }))
                    .await
                    .ok()?;
                let procs: Vec<Value> = r
                    .get("process_info")
                    .and_then(|p| p.get("foreground_processes"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let fg = procs.last();
                let name = fg
                    .and_then(|f| {
                        f.get("argv0")
                            .filter(|v| truthy(v))
                            .or_else(|| f.get("name"))
                    })
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some((pane, name))
            });
        }
        while let Some(done) = lookups.join_next().await {
            if let Ok(Some((pane, name))) = done {
                self.lock().process_names.insert(pane, name);
            }
        }
        let (process_names, prev, version) = {
            let i = self.lock();
            (
                i.process_names.clone(),
                i.model.clone(),
                i.health.version.clone(),
            )
        };
        let next = normalize(
            snap,
            &process_names,
            prev.as_ref(),
            version.as_deref(),
            now_ms(),
        );
        let streams = next.streams.len();
        let panes = pane_signature(&next.streams);
        self.apply_model(next);
        self.patch_health(|h| {
            h.ok = true;
            h.problem = None;
            h.last_ok_at = Some(now_ms());
        });
        self.log(
            "herdr_refresh",
            &[("ms", json!(now_ms() - t0)), ("streams", json!(streams))],
        );
        if panes != self.lock().sub_panes {
            self.resubscribe();
        }
        Ok(())
    }

    async fn connect(&self) {
        let target = {
            let i = self.lock();
            if i.stopped {
                return;
            }
            i.target.clone()
        };
        let HerdrStatus {
            ok,
            socket,
            version,
            problem,
            ..
        } = herdr_status(&target).await;
        let socket = match (ok, socket) {
            (true, Some(socket)) => socket,
            _ => {
                self.patch_health(|h| {
                    h.ok = false;
                    h.problem = problem;
                    h.version = version;
                });
                self.schedule_retry();
                return;
            }
        };
        self.lock().socket = Some(socket);
        self.lock().health.version = version;
        match self.refresh().await {
            Ok(()) => {
                self.resubscribe();
                self.lock().retry_delay = MIN_RETRY;
            }
            Err(_) => self.schedule_retry(),
        }
    }

    fn schedule_retry(&self) {
        let mut i = self.lock();
        if i.stopped || i.retry_task.is_some() {
            return;
        }
        let delay = i.retry_delay;
        i.retry_delay = (i.retry_delay * 2).min(MAX_RETRY);
        let this = self.clone();
        let handle = tokio::spawn(async move {
            sleep(delay).await;
            this.lock().retry_task = None;
            this.connect().await;
        });
        i.retry_task = Some(handle);
        drop(i);
        self.log(
            "herdr_retry",
            &[("delay_ms", json!(delay.as_millis() as u64))],
        );
    }

    fn resubscribe(&self) {
        let mut i = self.lock();
        if i.stopped {
            return;
        }
        let Some(socket) = i.socket.clone() else {
            return;
        };
        let mut panes: Vec<String> = i
            .model
            .as_ref()
            .map(|m| m.streams.iter().map(|s| s.id.clone()).collect())
            .unwrap_or_default();
        panes.sort();
        let mut subs: Vec<Value> = LIFECYCLE.iter().map(|t| json!({ "type": t })).collect();
        for p in &panes {
            subs.push(json!({ "type": "pane.agent_status_changed", "pane_id": p }));
        }
        i.sub_panes = panes.join(",");
        i.next_sub_id += 1;
        let id = i.next_sub_id;
        let old = i.sub_id;
        let sub = herdr_subscribe(&socket, subs);
        i.subs.insert(id, sub.close);
        i.sub_id = Some(id);
        drop(i);
        tokio::spawn(self.clone().pump(id, old, sub.events));
    }

    /// Reads one subscription's events until it ends.
    async fn pump(self, id: u64, old: Option<u64>, mut events: UnboundedReceiver<SubEvent>) {
        while let Some(ev) = events.recv().await {
            match ev {
                SubEvent::Started => {
                    if let Some(old) = old {
                        self.close_sub(old);
                    }
                    self.request_refresh(Duration::ZERO);
                }
                SubEvent::Event { name, data } => self.on_event(&name, &data),
                SubEvent::End(reason) => {
                    self.handle_end(id, &reason);
                    return;
                }
            }
        }
    }

    fn handle_end(&self, id: u64, reason: &str) {
        {
            let mut i = self.lock();
            i.subs.remove(&id);
            if i.sub_id != Some(id) || i.stopped {
                return;
            }
            i.sub_id = None;
            i.sub_panes.clear();
        }
        self.log("herdr_sub_end", &[("reason", json!(reason))]);
        // A rejected subscription (e.g. a pane vanished) or a lost history both mean: re-read, re-subscribe.
        let this = self.clone();
        tokio::spawn(async move {
            sleep(RESUBSCRIBE_DELAY).await;
            let stopped = this.lock().stopped;
            if stopped {
                return;
            }
            match this.refresh().await {
                Ok(()) => {
                    let idle = this.lock().sub_id.is_none();
                    if idle {
                        this.resubscribe();
                    }
                }
                Err(_) => this.schedule_retry(),
            }
        });
    }

    fn close_sub(&self, id: u64) {
        let close = self.lock().subs.remove(&id);
        if let Some(close) = close {
            close();
        }
    }

    fn on_event(&self, name: &str, data: &Value) {
        if name == "pane.agent_status_changed" {
            let next = {
                let i = self.lock();
                i.model.as_ref().and_then(|m| {
                    let mut next = m.clone();
                    apply_agent_status(&mut next, data, now_ms()).then_some(next)
                })
            };
            if let Some(next) = next {
                self.apply_model(next);
            }
        }
        self.request_refresh(EVENT_DEBOUNCE);
    }

    fn apply_model(&self, next: Model) {
        let (transitions, model) = {
            let mut i = self.lock();
            let transitions = transitions(i.model.as_ref(), &next, now_ms());
            i.model = Some(next.clone());
            (transitions, next)
        };
        for t in transitions {
            let _ = self.shared.events.send(HerdrEvent::Transition(t));
        }
        let _ = self.shared.events.send(HerdrEvent::Model(model));
    }

    fn patch_health(&self, patch: impl FnOnce(&mut HerdrHealth)) {
        let mut i = self.lock();
        let before = i.health.clone();
        patch(&mut i.health);
        let after = i.health.clone();
        if after.ok == before.ok && after.problem == before.problem {
            return;
        }
        let ev = if after.ok { "herdr_up" } else { "herdr_down" };
        self.log(ev, &[("problem", json!(after.problem))]);
        let _ = self.shared.events.send(HerdrEvent::Health(after));
    }

    fn log(&self, ev: &str, fields: &[(&str, Value)]) {
        let map: BTreeMap<String, Value> = fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        self.shared.diag.event(ev, &map);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl HerdrReader for HerdrState {
    fn model(&self) -> Option<Model> {
        self.lock().model.clone()
    }

    fn health(&self) -> HerdrHealth {
        self.lock().health.clone()
    }

    fn events(&self) -> broadcast::Receiver<HerdrEvent> {
        self.shared.events.subscribe()
    }
}

/// Transitions between two models. A stream counts as changed when its state differs, or when its terminal
/// was replaced (then `from` is null).
pub fn transitions(prev: Option<&Model>, next: &Model, at: u64) -> Vec<Transition> {
    let before: HashMap<&str, &Stream> = prev
        .map(|m| m.streams.iter().map(|s| (s.id.as_str(), s)).collect())
        .unwrap_or_default();
    next.streams
        .iter()
        .filter_map(|s| {
            let old = before.get(s.id.as_str()).copied();
            let same_terminal = old.is_some_and(|o| o.terminal_id == s.terminal_id);
            let old_state = old.and_then(|o| o.state);
            if old_state == s.state && (old.is_none() || same_terminal) {
                return None;
            }
            Some(Transition {
                stream_id: s.id.clone(),
                terminal_id: s.terminal_id.clone(),
                from: if same_terminal { old_state } else { None },
                to: s.state,
                kind: s.kind,
                tool: s.tool.clone(),
                state_change_seq: s.state_change_seq,
                at,
            })
        })
        .collect()
}

/// Applies a `pane.agent_status_changed` event to the model. Returns false when nothing changed.
/// An unrecognized status reads as `unknown`, the same as `normalize`.
pub fn apply_agent_status(model: &mut Model, data: &Value, now: u64) -> bool {
    let Some(pane) = data.get("pane_id").and_then(Value::as_str) else {
        return false;
    };
    let state = data
        .get("agent_status")
        .and_then(|v| serde_json::from_value::<AgentState>(v.clone()).ok())
        .unwrap_or(AgentState::Unknown);
    let Some(stream) = model.streams.iter_mut().find(|s| s.id == pane) else {
        return false;
    };
    if stream.state == Some(state) {
        return false;
    }
    stream.state = Some(state);
    stream.kind = StreamKind::Agent;
    if let Some(agent) = data
        .get("agent")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty())
    {
        stream.tool = agent.to_string();
    }
    stream.state_since = Some(now);
    model.as_of = now;
    true
}

fn pane_signature(streams: &[Stream]) -> String {
    let mut ids: Vec<&str> = streams.iter().map(|s| s.id.as_str()).collect();
    ids.sort();
    ids.join(",")
}

fn list<'a>(snap: &'a Value, key: &str) -> &'a [Value] {
    snap.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// JavaScript truthiness for the JSON values Herdr sends.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        _ => true,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stream(id: &str, terminal: &str, state: Option<AgentState>) -> Stream {
        serde_json::from_value(json!({
            "id": id,
            "workspaceId": "w",
            "projectId": "p",
            "terminalId": terminal,
            "name": "n",
            "label": null,
            "kind": "agent",
            "tool": "claude",
            "state": state,
            "stateChangeSeq": 1,
            "stateSince": 1,
            "cwd": null,
            "folder": null,
            "focused": false,
            "title": null,
            "rows": null,
        }))
        .unwrap()
    }

    fn model(streams: Vec<Stream>) -> Model {
        Model {
            workspaces: vec![],
            projects: vec![],
            streams,
            focused_stream_id: None,
            herdr_version: None,
            as_of: 1,
        }
    }

    #[test]
    fn first_model_reports_every_stream_with_null_from() {
        let next = model(vec![stream("a", "t1", Some(AgentState::Working))]);
        let t = transitions(None, &next, 9);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].from, None);
        assert_eq!(t[0].to, Some(AgentState::Working));
        assert_eq!(t[0].at, 9);
    }

    #[test]
    fn same_state_same_terminal_is_no_transition() {
        let prev = model(vec![stream("a", "t1", Some(AgentState::Idle))]);
        let next = model(vec![stream("a", "t1", Some(AgentState::Idle))]);
        assert!(transitions(Some(&prev), &next, 9).is_empty());
    }

    #[test]
    fn replaced_terminal_reports_from_null() {
        let prev = model(vec![stream("a", "t1", Some(AgentState::Idle))]);
        let next = model(vec![stream("a", "t2", Some(AgentState::Idle))]);
        let t = transitions(Some(&prev), &next, 9);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].from, None);
    }

    #[test]
    fn agent_status_event_updates_stream() {
        let mut m = model(vec![stream("a", "t1", Some(AgentState::Idle))]);
        let data = json!({ "pane_id": "a", "agent_status": "blocked", "agent": "codex" });
        assert!(apply_agent_status(&mut m, &data, 50));
        assert_eq!(m.streams[0].state, Some(AgentState::Blocked));
        assert_eq!(m.streams[0].tool, "codex");
        assert_eq!(m.streams[0].state_since, Some(50));
        assert_eq!(m.as_of, 50);
    }

    #[test]
    fn agent_status_event_ignores_same_state_and_unknown_pane() {
        let mut m = model(vec![stream("a", "t1", Some(AgentState::Idle))]);
        assert!(!apply_agent_status(
            &mut m,
            &json!({ "pane_id": "a", "agent_status": "idle" }),
            50
        ));
        assert!(!apply_agent_status(
            &mut m,
            &json!({ "pane_id": "zz", "agent_status": "working" }),
            50
        ));
    }
}
