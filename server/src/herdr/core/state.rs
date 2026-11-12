// Live, cached view of herdr-core. One connection subscribes to every event: its snapshot and the
// events after it keep the terminal records (herdr-core's schema/README.md, Events), and the Herdr
// app's layout document is read again whenever it changes. The model is rebuilt from both after
// each change and published only when it differs. A lost or refused connection retries with backoff.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::conn::{socket_path, Chunk, CoreConn};
use super::model::normalize;
use crate::diag::Diagnostics;
use crate::herdr::model::Model;
use crate::herdr::rpc::{HerdrError, HerdrStatus};
use crate::herdr::state::{transitions, HerdrEvent, HerdrHealth, HerdrReader};

const MIN_RETRY: Duration = Duration::from_millis(500);
const MAX_RETRY: Duration = Duration::from_secs(15);
/// The Herdr app's layout document.
pub const LAYOUT_DOC: &str = "herdr-app";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTarget {
    pub socket: PathBuf,
    pub timeout: Duration,
}

impl CoreTarget {
    pub fn of(cfg: &crate::config::BriardrConfig) -> Self {
        CoreTarget {
            socket: socket_path(&cfg.herdr_core_session, &cfg.herdr_core_socket, &|k| {
                std::env::var(k).ok()
            }),
            timeout: Duration::from_millis(cfg.herdr_timeout_ms),
        }
    }
}

/// One connection's hello, for Settings and `briardr status`. Never starts a core.
pub async fn core_status(t: &CoreTarget) -> HerdrStatus {
    match CoreConn::open(&t.socket, t.timeout).await {
        Ok((_, hello)) => HerdrStatus {
            ok: true,
            running: true,
            version: hello
                .get("version")
                .and_then(Value::as_str)
                .map(String::from),
            socket: Some(t.socket.clone()),
            problem: None,
        },
        Err(e) => HerdrStatus {
            ok: false,
            running: false,
            version: None,
            socket: None,
            problem: Some(problem_of(&e).to_string()),
        },
    }
}

fn problem_of(e: &HerdrError) -> &'static str {
    match e.code.as_str() {
        "not_running" => "not_running",
        "timeout" => "timeout",
        "incompatible" => "incompatible",
        _ => "unreachable",
    }
}

struct Inner {
    target: CoreTarget,
    conn: Option<CoreConn>,
    /// The core's terminal records, in its order.
    terminals: Vec<Value>,
    /// Events up to this one are in `terminals`.
    seq: u64,
    doc: Option<Value>,
    model: Option<Model>,
    health: HerdrHealth,
    task: Option<JoinHandle<()>>,
}

struct Shared {
    inner: Mutex<Inner>,
    diag: Arc<Diagnostics>,
    events: broadcast::Sender<HerdrEvent>,
}

/// Cheap to clone. Clones share one state.
#[derive(Clone)]
pub struct CoreState {
    shared: Arc<Shared>,
}

impl CoreState {
    pub fn new(
        target: CoreTarget,
        diag: Arc<Diagnostics>,
        events: broadcast::Sender<HerdrEvent>,
    ) -> Self {
        CoreState {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    target,
                    conn: None,
                    terminals: Vec::new(),
                    seq: 0,
                    doc: None,
                    model: None,
                    health: HerdrHealth {
                        ok: false,
                        problem: None,
                        version: None,
                        last_ok_at: None,
                    },
                    task: None,
                }),
                diag,
                events,
            }),
        }
    }

    pub fn target(&self) -> CoreTarget {
        self.lock().target.clone()
    }

    pub async fn start(&self) {
        let this = self.clone();
        let task = tokio::spawn(async move { this.run().await });
        if let Some(old) = self.lock().task.replace(task) {
            old.abort();
        }
    }

    pub fn stop(&self) {
        let mut i = self.lock();
        if let Some(task) = i.task.take() {
            task.abort();
        }
        i.conn = None;
    }

    pub async fn retarget(&self, target: CoreTarget) {
        self.stop();
        {
            let mut i = self.lock();
            i.target = target;
            i.terminals.clear();
            i.doc = None;
            i.model = None;
        }
        self.start().await;
    }

    /// One request on the live connection.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, HerdrError> {
        let conn = self.lock().conn.clone().ok_or_else(|| HerdrError {
            code: "unreachable".to_string(),
            message: "herdr-core is not reachable".to_string(),
        })?;
        conn.call(method, params).await
    }

    /// Reads the terminals and the layout document again, so the model shows a change just made.
    pub async fn refresh(&self) -> Result<(), HerdrError> {
        let list = self.request("terminal.list", json!({})).await?;
        let doc = self.read_doc().await;
        let terminals = list
            .get("terminals")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        {
            let mut i = self.lock();
            // Finished records stay, so a late event for one still applies.
            i.terminals
                .retain(|t| t.get("state").and_then(Value::as_str) != Some("live"));
            i.terminals.extend(terminals);
            i.doc = doc;
        }
        self.rebuild();
        Ok(())
    }

    async fn read_doc(&self) -> Option<Value> {
        let r = self
            .request("doc.get", json!({"name": LAYOUT_DOC}))
            .await
            .ok()?;
        r.get("value").filter(|v| v.is_object()).cloned()
    }

    async fn run(self) {
        let mut delay = MIN_RETRY;
        loop {
            match self.connection().await {
                Ok(()) => delay = MIN_RETRY,
                Err(e) => {
                    self.patch_health(|h| {
                        h.ok = false;
                        h.problem = Some(problem_of(&e).to_string());
                    });
                }
            }
            self.log(
                "herdr_retry",
                &[("delay_ms", json!(delay.as_millis() as u64))],
            );
            sleep(delay).await;
            delay = (delay * 2).min(MAX_RETRY);
        }
    }

    /// One connection, from hello until it ends. Errors only before it was up.
    async fn connection(&self) -> Result<(), HerdrError> {
        let target = self.target();
        let (conn, hello) = CoreConn::open(&target.socket, target.timeout).await?;
        let (sub, mut events) = conn.open_stream("events.subscribe", json!({})).await?;
        {
            let mut i = self.lock();
            i.conn = Some(conn.clone());
            i.terminals = list(&sub, "terminals");
            i.seq = sub.get("seq").and_then(Value::as_u64).unwrap_or(0);
        }
        // Subscribed first, then read: a change in between arrives as an event.
        let doc = self.read_doc().await;
        self.lock().doc = doc;
        let version = hello
            .get("version")
            .and_then(Value::as_str)
            .map(String::from);
        self.patch_health(|h| {
            h.ok = true;
            h.problem = None;
            h.version = version;
            h.last_ok_at = Some(now_ms());
        });
        self.rebuild();
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let bytes = match events.recv().await {
                Chunk::Data(bytes) => bytes,
                Chunk::Resize(..) => continue,
                Chunk::Closed(_) | Chunk::Lost => break,
            };
            events.credit(bytes.len());
            pending.extend_from_slice(&bytes);
            let mut changed = Changed::default();
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let Ok(msg) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                let mut i = self.lock();
                let Inner { terminals, seq, .. } = &mut *i;
                changed.merge(apply(terminals, seq, &msg));
                if changed.terminals || changed.doc {
                    i.health.last_ok_at = Some(now_ms());
                }
            }
            if changed.doc {
                let doc = self.read_doc().await;
                self.lock().doc = doc;
            }
            if changed.terminals || changed.doc {
                self.rebuild();
            }
        }
        self.lock().conn = None;
        self.patch_health(|h| {
            h.ok = false;
            h.problem = Some("unreachable".to_string());
        });
        Ok(())
    }

    /// Publishes the model when it changed.
    fn rebuild(&self) {
        let (transitions, model) = {
            let mut i = self.lock();
            let mut next = normalize(
                &i.terminals,
                i.doc.as_ref(),
                i.model.as_ref(),
                i.health.version.as_deref(),
                now_ms(),
            );
            if let Some(prev) = &i.model {
                let as_of = next.as_of;
                next.as_of = prev.as_of;
                if *prev == next {
                    return;
                }
                next.as_of = as_of;
            }
            let t = transitions(i.model.as_ref(), &next, now_ms());
            i.model = Some(next.clone());
            (t, next)
        };
        for t in transitions {
            let _ = self.shared.events.send(HerdrEvent::Transition(t));
        }
        let _ = self.shared.events.send(HerdrEvent::Model(model));
    }

    fn patch_health(&self, patch: impl FnOnce(&mut HerdrHealth)) {
        let after = {
            let mut i = self.lock();
            let before = i.health.clone();
            patch(&mut i.health);
            let after = i.health.clone();
            if after.ok == before.ok && after.problem == before.problem {
                return;
            }
            after
        };
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

impl HerdrReader for CoreState {
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

fn list(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// What one notification changed.
#[derive(Debug, Default, PartialEq)]
pub struct Changed {
    pub terminals: bool,
    /// The layout document changed (or events were missed): read it again.
    pub doc: bool,
}

impl Changed {
    fn merge(&mut self, other: Changed) {
        self.terminals |= other.terminals;
        self.doc |= other.doc;
    }
}

/// Applies one notification to the records, as herdr-core's schema/README.md (Events) says.
pub fn apply(terminals: &mut Vec<Value>, seq: &mut u64, msg: &Value) -> Changed {
    let params = msg.get("params").unwrap_or(&Value::Null);
    let at = params.get("seq").and_then(Value::as_u64).unwrap_or(0);
    match msg.get("method").and_then(Value::as_str) {
        Some("missed") => {
            *terminals = list(params, "terminals");
            *seq = at;
            return Changed {
                terminals: true,
                doc: true,
            };
        }
        Some("event") if at > *seq => *seq = at,
        _ => return Changed::default(),
    }
    let data = params.get("data").unwrap_or(&Value::Null);
    let kind = params.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "doc" {
        // Another machine's documents come with `machine`; only this one's layout is read.
        let ours = params.get("machine").is_none_or(Value::is_null)
            && data.get("name").and_then(Value::as_str) == Some(LAYOUT_DOC);
        return Changed {
            terminals: false,
            doc: ours,
        };
    }
    let Some(address) = params.get("terminal").and_then(Value::as_str) else {
        return Changed::default();
    };
    let index = terminals
        .iter()
        .position(|t| t.get("address").and_then(Value::as_str) == Some(address));
    match (kind, index) {
        ("created" | "restarted", Some(i)) => terminals[i] = data.clone(),
        ("created" | "restarted", None) => terminals.push(data.clone()),
        ("labels", None) => match data.get("record") {
            Some(record) if record.is_object() => terminals.push(record.clone()),
            _ => return Changed::default(),
        },
        ("removed", Some(i)) => {
            terminals.remove(i);
        }
        (_, None) => return Changed::default(),
        (_, Some(i)) => {
            let Some(t) = terminals[i].as_object_mut() else {
                return Changed::default();
            };
            let mut set = |key: &str, v: Option<&Value>| {
                t.insert(key.to_string(), v.cloned().unwrap_or(Value::Null));
            };
            match kind {
                "lost" => {
                    for key in ["reason", "exit_code", "signal", "finished_ms"] {
                        set(key, data.get(key));
                    }
                    set("pid", None);
                    set("foreground", None);
                }
                "finished" => {
                    for key in ["reason", "exit_code", "signal", "finished_ms"] {
                        set(key, data.get(key));
                    }
                    set("state", Some(&json!("finished")));
                    set("foreground", None);
                }
                "keep" => set("keep", data.get("keep")),
                "status" => {
                    set("status", data.get("status"));
                    set("records", data.get("records"));
                    set("reported", data.get("reported"));
                }
                "turn" => set("turn", data.get("turn")),
                "title" => set("title", data.get("title")),
                "labels" => set("labels", data.get("labels")),
                "size" => {
                    set("cols", data.get("cols"));
                    set("rows", data.get("rows"));
                }
                "foreground" => set("foreground", Some(data)),
                _ => return Changed::default(),
            }
        }
    }
    Changed {
        terminals: true,
        doc: false,
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

    fn event(seq: u64, kind: &str, terminal: &str, data: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": "event",
            "params": {"seq": seq, "type": kind, "terminal": terminal, "data": data}})
    }

    fn record(address: &str) -> Value {
        json!({"address": address, "state": "live", "labels": {}, "turn": 0, "title": "",
            "foreground": {"name": "zsh"}, "pid": 9})
    }

    #[test]
    fn events_apply_in_order_and_stale_ones_are_dropped() {
        let mut ts = vec![record("t1")];
        let mut seq = 10;
        let changed = apply(&mut ts, &mut seq, &event(11, "created", "t2", record("t2")));
        assert_eq!(
            changed,
            Changed {
                terminals: true,
                doc: false
            }
        );
        apply(
            &mut ts,
            &mut seq,
            &event(12, "title", "t2", json!({"title": "vim"})),
        );
        apply(
            &mut ts,
            &mut seq,
            &event(
                13,
                "status",
                "t1",
                json!({"status": {"state": "working"}, "records": {}, "reported": true}),
            ),
        );
        apply(
            &mut ts,
            &mut seq,
            &event(14, "turn", "t1", json!({"turn": 3, "state": "done"})),
        );
        apply(
            &mut ts,
            &mut seq,
            &event(15, "labels", "t1", json!({"labels": {"herdr.seen": "3"}})),
        );
        apply(
            &mut ts,
            &mut seq,
            &event(16, "foreground", "t2", Value::Null),
        );
        assert_eq!(ts[1]["title"], "vim");
        assert_eq!(ts[1]["foreground"], Value::Null);
        assert_eq!(ts[0]["status"]["state"], "working");
        assert_eq!(
            (ts[0]["turn"].clone(), ts[0]["labels"]["herdr.seen"].clone()),
            (json!(3), json!("3"))
        );
        assert_eq!(seq, 16);
        let stale = apply(&mut ts, &mut seq, &event(16, "removed", "t1", json!({})));
        assert_eq!(stale, Changed::default());
        apply(
            &mut ts,
            &mut seq,
            &event(
                17,
                "finished",
                "t2",
                json!({"reason": "exited", "exit_code": 0, "signal": null, "finished_ms": 5}),
            ),
        );
        assert_eq!(ts[1]["state"], "finished");
        apply(&mut ts, &mut seq, &event(18, "removed", "t2", json!({})));
        assert_eq!(ts.len(), 1);
    }

    #[test]
    fn lost_keeps_the_record_live_without_a_program() {
        let mut ts = vec![record("t1")];
        let mut seq = 0;
        apply(
            &mut ts,
            &mut seq,
            &event(
                1,
                "lost",
                "t1",
                json!({"reason": "signaled", "exit_code": null, "signal": 9, "finished_ms": null}),
            ),
        );
        assert_eq!(ts[0]["state"], "live");
        assert_eq!(
            (ts[0]["pid"].clone(), ts[0]["reason"].clone()),
            (Value::Null, json!("signaled"))
        );
    }

    #[test]
    fn doc_events_and_missed_ask_for_the_document() {
        let mut ts = vec![record("t1")];
        let mut seq = 3;
        let doc = json!({"jsonrpc": "2.0", "method": "event",
            "params": {"seq": 4, "type": "doc", "data": {"name": "herdr-app", "version": 2}}});
        assert_eq!(
            apply(&mut ts, &mut seq, &doc),
            Changed {
                terminals: false,
                doc: true
            }
        );
        let remote = json!({"jsonrpc": "2.0", "method": "event",
            "params": {"seq": 5, "type": "doc", "machine": "box", "data": {"name": "herdr-app", "version": 2}}});
        assert_eq!(apply(&mut ts, &mut seq, &remote), Changed::default());
        let missed = json!({"jsonrpc": "2.0", "method": "missed",
            "params": {"seq": 40, "terminals": [record("t7")]}});
        assert_eq!(
            apply(&mut ts, &mut seq, &missed),
            Changed {
                terminals: true,
                doc: true
            }
        );
        assert_eq!((ts[0]["address"].clone(), seq), (json!("t7"), 40));
        let heartbeat = json!({"jsonrpc": "2.0", "method": "heartbeat", "params": {"seq": 40}});
        assert_eq!(apply(&mut ts, &mut seq, &heartbeat), Changed::default());
    }

    #[test]
    fn a_label_event_with_a_record_adds_a_terminal_newly_in_view() {
        let mut ts = vec![];
        let mut seq = 0;
        apply(
            &mut ts,
            &mut seq,
            &event(
                1,
                "labels",
                "t5",
                json!({"labels": {}, "record": record("t5")}),
            ),
        );
        assert_eq!(ts.len(), 1);
    }
}
