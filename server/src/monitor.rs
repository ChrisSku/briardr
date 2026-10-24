// Turns stream state transitions into debounced push alerts.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::config::{read_json, write_file_atomic, BriardrConfig};
use crate::diag::Diagnostics;
use crate::herdr::model::{AgentState, Model, StreamKind};
use crate::herdr::state::{HerdrEvent, HerdrReader, Transition};
use crate::push::Push;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    NeedsYou,
    Finished,
    Ready,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::NeedsYou => "needs_you",
            AlertKind::Finished => "finished",
            AlertKind::Ready => "ready",
        }
    }
}

pub fn alert_kind_for(
    from: Option<AgentState>,
    to: Option<AgentState>,
    kind: StreamKind,
) -> Option<AlertKind> {
    if kind != StreamKind::Agent {
        return None;
    }
    match to {
        Some(AgentState::Blocked) => Some(AlertKind::NeedsYou),
        Some(AgentState::Done) => Some(AlertKind::Finished),
        // done→idle means someone looked at the finished work: not news.
        Some(AgentState::Idle)
            if from != Some(AgentState::Done) && from != Some(AgentState::Idle) =>
        {
            Some(AlertKind::Ready)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlertPayload {
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
    pub stream_id: String,
    pub tag: String,
    /// Agents blocked on the user right now (the Agents list's "Needs you"), for the icon badge.
    pub attention: u32,
}

type SendFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Where alerts go: Web Push in production, a recorder in tests.
pub trait AlertSink: Send + Sync {
    fn send(&self, payload: AlertPayload, ttl_secs: u32) -> SendFuture<'_>;
}

impl AlertSink for Push {
    fn send(&self, payload: AlertPayload, ttl_secs: u32) -> SendFuture<'_> {
        Box::pin(async move {
            Push::send_counted(
                self,
                payload.kind.as_str(),
                &payload.title,
                &payload.body,
                Some(payload.stream_id),
                &payload.tag,
                None,
                ttl_secs,
                Some(payload.attention),
            )
            .await;
        })
    }
}

pub type ConfigFn = Arc<dyn Fn() -> BriardrConfig + Send + Sync>;

#[derive(Debug)]
pub struct PendingAlert {
    pub key: String,
    pub stream_id: String,
    pub kind: AlertKind,
    pub timer: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Serialize, Deserialize, Default)]
struct MonitorState {
    sent: Vec<String>,
}

struct State {
    /// Insertion order, for trimming; `sent_set` answers membership.
    sent: Vec<String>,
    sent_set: HashSet<String>,
    pending: HashMap<String, PendingAlert>,
    baselined: bool,
    /// Outcome per transition key (exposed for tests).
    log: Vec<(String, &'static str)>,
}

struct Inner {
    reader: Arc<dyn HerdrReader>,
    sink: Arc<dyn AlertSink>,
    cfg: ConfigFn,
    file: PathBuf,
    diag: Arc<Diagnostics>,
    state: Mutex<State>,
    /// Keeps state-file writes in order, so an older snapshot never lands after a newer one.
    save_lock: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct Monitor {
    inner: Arc<Inner>,
}

fn state_name(state: AgentState) -> &'static str {
    match state {
        AgentState::Blocked => "blocked",
        AgentState::Working => "working",
        AgentState::Done => "done",
        AgentState::Idle => "idle",
        AgentState::Unknown => "unknown",
    }
}

fn alert_key(terminal_id: &str, state: AgentState, seq: Option<u64>) -> String {
    let seq = seq.map_or_else(|| "na".to_string(), |n| n.to_string());
    format!("{terminal_id}|{}|{seq}", state_name(state))
}

/// Adds a key in insertion order, keeping the last 1000 once more than 2000 are remembered.
fn remember(st: &mut State, key: String) -> bool {
    if !st.sent_set.insert(key.clone()) {
        return false;
    }
    st.sent.push(key);
    if st.sent.len() > 2000 {
        let excess = st.sent.len() - 1000;
        st.sent.drain(..excess);
        st.sent_set = st.sent.iter().cloned().collect();
    }
    true
}

/// Cuts to `max` UTF-16 code units, as JS `slice` does. A surrogate pair split by the cut becomes U+FFFD.
fn utf16_cut(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        let n = c.len_utf16();
        if units + n > max {
            if units < max {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(c);
        units += n;
    }
    out
}

impl Monitor {
    pub fn new(
        reader: Arc<dyn HerdrReader>,
        sink: Arc<dyn AlertSink>,
        cfg: ConfigFn,
        state_file: PathBuf,
        diag: Arc<Diagnostics>,
    ) -> Monitor {
        let mut sent = Vec::new();
        let mut sent_set = HashSet::new();
        for key in read_json::<MonitorState>(&state_file, MonitorState::default()).sent {
            if sent_set.insert(key.clone()) {
                sent.push(key);
            }
        }
        Monitor {
            inner: Arc::new(Inner {
                reader,
                sink,
                cfg,
                file: state_file,
                diag,
                state: Mutex::new(State {
                    sent,
                    sent_set,
                    pending: HashMap::new(),
                    baselined: false,
                    log: Vec::new(),
                }),
                save_lock: tokio::sync::Mutex::new(()),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Subscribes to Herdr and baselines from the current model. Must run inside the tokio runtime.
    pub fn start(&self) {
        let mut events = self.inner.reader.events();
        let snapshot = self.inner.reader.model();
        let me = self.clone();
        tokio::spawn(async move {
            if let Some(m) = snapshot {
                me.baseline(&m).await;
            }
            loop {
                match events.recv().await {
                    Ok(HerdrEvent::Model(m)) => me.baseline(&m).await,
                    Ok(HerdrEvent::Transition(t)) => {
                        if me.state().baselined {
                            me.on_transition(&t);
                        }
                    }
                    Ok(HerdrEvent::Health(_)) => {}
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    /// States that exist when the monitor first sees the model are remembered, not alerted.
    async fn baseline(&self, m: &Model) {
        let added = {
            let mut st = self.state();
            if st.baselined {
                return;
            }
            st.baselined = true;
            let mut added = false;
            for s in &m.streams {
                if let Some(state) = s.state {
                    added |= remember(
                        &mut st,
                        alert_key(&s.terminal_id, state, s.state_change_seq),
                    );
                }
            }
            added
        };
        self.inner.diag.event(
            "monitor_baseline",
            &BTreeMap::from([("streams".to_string(), json!(m.streams.len()))]),
        );
        if added {
            self.save().await;
        }
    }

    /// Writes the sent list, owner-only. Returns false on failure; the alert is then not sent.
    async fn save(&self) -> bool {
        let _guard = self.inner.save_lock.lock().await;
        let data = {
            let st = self.state();
            match serde_json::to_vec(&MonitorState {
                sent: st.sent.clone(),
            }) {
                Ok(data) => data,
                Err(_) => return false,
            }
        };
        let file = self.inner.file.clone();
        matches!(
            tokio::task::spawn_blocking(move || write_file_atomic(&file, &data, 0o600)).await,
            Ok(Ok(()))
        )
    }

    fn log(&self, key: String, outcome: &'static str) {
        self.state().log.push((key, outcome));
    }

    /// Schedules, cancels or skips the alert for one transition. Call only after the baseline.
    pub fn on_transition(&self, t: &Transition) {
        let cfg = (self.inner.cfg)().notifications;
        let mut st = self.state();
        // Any change for a stream cancels its pending alert: the old state is no longer true.
        if let Some(p) = st.pending.remove(&t.stream_id) {
            if let Some(timer) = &p.timer {
                timer.abort();
            }
            st.log.push((p.key.clone(), "cancelled"));
            self.inner.diag.event(
                "alert_cancelled",
                &BTreeMap::from([("kind".to_string(), json!(p.kind.as_str()))]),
            );
        }
        let (Some(kind), Some(to)) = (alert_kind_for(t.from, t.to, t.kind), t.to) else {
            return;
        };
        let enabled = match kind {
            AlertKind::NeedsYou => cfg.needs_you,
            AlertKind::Finished => cfg.finished,
            AlertKind::Ready => cfg.ready,
        };
        let key = alert_key(&t.terminal_id, to, t.state_change_seq);
        if !enabled {
            st.log.push((key, "disabled"));
            return;
        }
        if st.sent_set.contains(&key) {
            st.log.push((key, "duplicate"));
            return;
        }
        let me = self.clone();
        let fire_t = t.clone();
        let fire_key = key.clone();
        let delay = Duration::from_millis(cfg.debounce_ms);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            me.fire(fire_t, kind, fire_key).await;
        });
        st.pending.insert(
            t.stream_id.clone(),
            PendingAlert {
                key,
                stream_id: t.stream_id.clone(),
                kind,
                timer: Some(timer),
            },
        );
    }

    async fn fire(&self, t: Transition, kind: AlertKind, key: String) {
        {
            let mut st = self.state();
            // Only the alert still pending under this key may fire.
            if !st.pending.get(&t.stream_id).is_some_and(|p| p.key == key) {
                return;
            }
            st.pending.remove(&t.stream_id);
        }
        // Re-check against the latest model: only alert if the state is still true right now.
        let live = self.inner.reader.model().is_some_and(|m| {
            m.streams
                .iter()
                .any(|s| s.id == t.stream_id && s.state == t.to && s.terminal_id == t.terminal_id)
        });
        if !live {
            self.log(key, "cancelled");
            return;
        }
        {
            let mut st = self.state();
            if st.sent_set.contains(&key) {
                return;
            }
            remember(&mut st, key.clone());
        }
        if !self.save().await {
            return;
        }
        let payload = self.payload(kind, &t.stream_id);
        self.log(key, "sent");
        self.inner.diag.event(
            "alert_fire",
            &BTreeMap::from([("kind".to_string(), json!(kind.as_str()))]),
        );
        let ttl_secs = if kind == AlertKind::NeedsYou {
            6 * 3600
        } else {
            3600
        };
        self.inner.sink.send(payload, ttl_secs).await;
    }

    /// Notification text. Names only when the setting allows and the stream is known; never terminal text.
    pub fn payload(&self, kind: AlertKind, stream_id: &str) -> AlertPayload {
        let model = self.inner.reader.model();
        let include_names = (self.inner.cfg)().notifications.include_names;
        let tag = format!("stream:{stream_id}");
        let attention = model.as_ref().map_or(0, |m| {
            m.streams
                .iter()
                .filter(|s| s.kind == StreamKind::Agent && s.state == Some(AgentState::Blocked))
                .count() as u32
        });
        let stream = model
            .as_ref()
            .and_then(|m| m.streams.iter().find(|s| s.id == stream_id));
        match stream {
            Some(s) if include_names => {
                let verb = match kind {
                    AlertKind::NeedsYou => "Needs you",
                    AlertKind::Finished => "Finished",
                    AlertKind::Ready => "Ready for input",
                };
                let ws = model
                    .as_ref()
                    .and_then(|m| m.workspaces.iter().find(|w| w.id == s.workspace_id))
                    .map(|w| w.name.as_str());
                let proj = model
                    .as_ref()
                    .and_then(|m| m.projects.iter().find(|p| p.id == s.project_id))
                    .map(|p| p.name.as_str());
                let body = [ws, proj]
                    .into_iter()
                    .flatten()
                    .filter(|n| !n.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                AlertPayload {
                    kind,
                    title: utf16_cut(&format!("{verb}: {}", s.name), 120),
                    body: utf16_cut(&body, 160),
                    stream_id: stream_id.to_string(),
                    tag,
                    attention,
                }
            }
            _ => {
                let title = match kind {
                    AlertKind::NeedsYou => "An agent needs you",
                    AlertKind::Finished => "An agent finished",
                    AlertKind::Ready => "An agent is ready for input",
                };
                AlertPayload {
                    kind,
                    title: title.to_string(),
                    body: "Open Briardr to see which one.".to_string(),
                    stream_id: stream_id.to_string(),
                    tag,
                    attention,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::model::{Project, Stream, Workspace};
    use crate::herdr::state::HerdrHealth;
    use crate::scratch::Scratch;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tokio::sync::broadcast;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stream(id: &str, state: AgentState, seq: u64) -> Stream {
        Stream {
            id: id.to_string(),
            workspace_id: "w1".to_string(),
            project_id: "w1:t1".to_string(),
            terminal_id: format!("term_{id}"),
            name: format!("agent {id}"),
            label: None,
            kind: StreamKind::Agent,
            tool: "claude".to_string(),
            state: Some(state),
            state_change_seq: Some(seq),
            state_since: Some(1),
            cwd: Some("/x".to_string()),
            folder: Some("x".to_string()),
            focused: false,
            title: Some("SECRET TERMINAL TITLE".to_string()),
            rows: Some(40),
        }
    }

    struct FakeReader {
        model: Mutex<Option<Model>>,
        events: broadcast::Sender<HerdrEvent>,
    }

    impl FakeReader {
        fn new() -> Arc<Self> {
            let (events, _) = broadcast::channel(256);
            Arc::new(FakeReader {
                model: Mutex::new(None),
                events,
            })
        }

        fn model_with(streams: Vec<Stream>) -> Model {
            Model {
                workspaces: vec![Workspace {
                    id: "w1".to_string(),
                    name: "api".to_string(),
                    number: 1,
                    focused: true,
                    active_project_id: None,
                    cwd: Some("/x".to_string()),
                }],
                projects: vec![Project {
                    id: "w1:t1".to_string(),
                    workspace_id: "w1".to_string(),
                    name: "backend".to_string(),
                    number: 1,
                    focused: true,
                }],
                streams,
                focused_stream_id: None,
                herdr_version: Some("0.9.3".to_string()),
                as_of: 1,
            }
        }

        fn set(&self, streams: Vec<Stream>) {
            let model = Self::model_with(streams);
            *lock(&self.model) = Some(model.clone());
            let _ = self.events.send(HerdrEvent::Model(model));
        }

        /// Fake model: it already shows the new state when the transition arrives.
        fn move_to(&self, id: &str, to: AgentState, seq: u64) {
            let mut streams = lock(&self.model)
                .as_ref()
                .map(|m| m.streams.clone())
                .unwrap_or_default();
            let from = streams.iter().find(|s| s.id == id).and_then(|s| s.state);
            streams.retain(|s| s.id != id);
            streams.push(stream(id, to, seq));
            let model = Self::model_with(streams);
            *lock(&self.model) = Some(model.clone());
            let _ = self.events.send(HerdrEvent::Transition(Transition {
                stream_id: id.to_string(),
                terminal_id: format!("term_{id}"),
                from,
                to: Some(to),
                kind: StreamKind::Agent,
                tool: "claude".to_string(),
                state_change_seq: Some(seq),
                at: 1,
            }));
            let _ = self.events.send(HerdrEvent::Model(model));
        }
    }

    impl HerdrReader for FakeReader {
        fn model(&self) -> Option<Model> {
            lock(&self.model).clone()
        }

        fn health(&self) -> HerdrHealth {
            HerdrHealth {
                ok: true,
                problem: None,
                version: Some("0.9.3".to_string()),
                last_ok_at: Some(1),
            }
        }

        fn events(&self) -> broadcast::Receiver<HerdrEvent> {
            self.events.subscribe()
        }
    }

    #[derive(Default)]
    struct Recorder {
        sent: Mutex<Vec<(AlertPayload, u32)>>,
    }

    impl Recorder {
        fn sent(&self) -> Vec<(AlertPayload, u32)> {
            lock(&self.sent).clone()
        }
    }

    impl AlertSink for Recorder {
        fn send(&self, payload: AlertPayload, ttl_secs: u32) -> SendFuture<'_> {
            Box::pin(async move {
                lock(&self.sent).push((payload, ttl_secs));
            })
        }
    }

    struct Setup {
        reader: Arc<FakeReader>,
        sink: Arc<Recorder>,
        cfg: Arc<Mutex<BriardrConfig>>,
        mon: Monitor,
    }

    fn setup(file: PathBuf, debounce_ms: u64) -> Setup {
        let reader = FakeReader::new();
        let sink = Arc::new(Recorder::default());
        let mut config = crate::config::default_config(&HashMap::new(), Path::new("/home/test"));
        config.notifications.debounce_ms = debounce_ms;
        config.notifications.ready = true;
        let cfg = Arc::new(Mutex::new(config));
        let cfg_fn: ConfigFn = {
            let cfg = cfg.clone();
            Arc::new(move || lock(&cfg).clone())
        };
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let diag = Arc::new(
            Diagnostics::new(dir.join("diag"), dir.join("salt"))
                .unwrap_or_else(|e| panic!("cannot start diagnostics: {e}")),
        );
        let mon = Monitor::new(reader.clone(), sink.clone(), cfg_fn, file, diag);
        Setup {
            reader,
            sink,
            cfg,
            mon,
        }
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    /// Waits for the nth alert. Each one waits for two synced writes, which can take a while on CI disks.
    async fn sent_at_least(sink: &Recorder, n: usize) -> Vec<(AlertPayload, u32)> {
        for _ in 0..100 {
            let sent = sink.sent();
            if sent.len() >= n {
                return sent;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        sink.sent()
    }

    #[test]
    fn alert_kind_fixtures() {
        for name in [
            "alertKindFor.agent-finished",
            "alertKindFor.agent-needs-you",
            "alertKindFor.non-agent",
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/monitor")
                .join(format!("{name}.json"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));
            let fixture: Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad fixture {name}: {e}"));
            let want = fixture["output"].as_str().map(str::to_string);
            for case in fixture["input"].as_array().into_iter().flatten() {
                let kind = if case["kind"] == "agent" {
                    StreamKind::Agent
                } else {
                    StreamKind::Shell
                };
                let from: Option<AgentState> = serde_json::from_value(case["from"].clone())
                    .unwrap_or_else(|e| panic!("bad from in {name}: {e}"));
                let to: Option<AgentState> = serde_json::from_value(case["to"].clone())
                    .unwrap_or_else(|e| panic!("bad to in {name}: {e}"));
                let got = alert_kind_for(from, to, kind).map(|k| k.as_str().to_string());
                assert_eq!(got, want, "{name}");
            }
        }
    }

    #[test]
    fn utf16_cut_counts_code_units() {
        assert_eq!(utf16_cut("abcdef", 4), "abcd");
        assert_eq!(utf16_cut("ab\u{1F600}", 3), "ab\u{FFFD}");
        assert_eq!(utf16_cut("ab\u{1F600}", 4), "ab\u{1F600}");
    }

    #[tokio::test]
    async fn states_that_exist_at_start_are_skipped() {
        let dir = Scratch::new("monitor-baseline");
        let s = setup(dir.join("monitor.json"), 60);
        s.reader.set(vec![
            stream("a", AgentState::Blocked, 1),
            stream("b", AgentState::Done, 1),
        ]);
        s.mon.start();
        // Herdr re-reporting the same states (same sequence numbers) must not alert.
        s.reader.move_to("a", AgentState::Blocked, 1);
        s.reader.move_to("b", AgentState::Done, 1);
        settle().await;
        assert!(s.sink.sent().is_empty());
    }

    #[tokio::test]
    async fn alerts_after_the_debounce_and_cancels_one_that_stops_being_true() {
        let dir = Scratch::new("monitor-debounce");
        let s = setup(dir.join("monitor.json"), 60);
        s.reader.set(vec![stream("a", AgentState::Working, 1)]);
        s.mon.start();
        s.reader.move_to("a", AgentState::Blocked, 2);
        s.reader.move_to("a", AgentState::Working, 3); // answered within the debounce window
        settle().await;
        assert!(s.sink.sent().is_empty(), "cancelled before delivery");
        s.reader.move_to("a", AgentState::Blocked, 4);
        let sent = sent_at_least(&s.sink, 1).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.kind, AlertKind::NeedsYou);
        assert_eq!(sent[0].0.stream_id, "a");
        assert_eq!(sent[0].1, 6 * 3600);
    }

    #[tokio::test]
    async fn deduplicates_across_monitor_restarts() {
        let dir = Scratch::new("monitor-restart");
        let file = dir.join("monitor.json");
        let one = setup(file.clone(), 60);
        one.reader.set(vec![stream("a", AgentState::Working, 1)]);
        one.mon.start();
        one.reader.move_to("a", AgentState::Done, 5);
        assert_eq!(sent_at_least(&one.sink, 1).await.len(), 1);
        // A restarted monitor that sees the same transition (e.g. replayed after reconnect) stays quiet.
        let two = setup(file, 60);
        two.reader.set(vec![stream("a", AgentState::Working, 4)]);
        two.mon.start();
        two.reader.move_to("a", AgentState::Done, 5);
        settle().await;
        assert!(two.sink.sent().is_empty());
    }

    #[tokio::test]
    async fn payloads_never_contain_terminal_text_and_names_can_be_removed() {
        let dir = Scratch::new("monitor-payload");
        let s = setup(dir.join("monitor.json"), 60);
        s.reader.set(vec![stream("a", AgentState::Working, 1)]);
        s.mon.start();
        s.reader.move_to("a", AgentState::Blocked, 9);
        let sent = sent_at_least(&s.sink, 1).await;
        let p = &sent[0].0;
        assert_eq!(p.title, "Needs you: agent a");
        assert_eq!(p.body, "api \u{00B7} backend");
        assert!(
            !format!("{p:?}").contains("SECRET"),
            "terminal title/text never included"
        );
        lock(&s.cfg).notifications.include_names = false;
        s.reader.move_to("a", AgentState::Done, 10);
        let sent = sent_at_least(&s.sink, 2).await;
        let p = &sent[1].0;
        assert_eq!(p.title, "An agent finished");
        assert_eq!(p.body, "Open Briardr to see which one.");
        let debug = format!("{p:?}");
        assert!(!debug.contains("agent a") && !debug.contains("api"));
    }

    #[tokio::test]
    async fn alerts_carry_how_many_agents_need_attention_now() {
        let dir = Scratch::new("monitor-attention");
        let s = setup(dir.join("monitor.json"), 60);
        s.reader.set(vec![
            stream("a", AgentState::Working, 1),
            stream("b", AgentState::Blocked, 1),
            stream("c", AgentState::Done, 1),
        ]);
        s.mon.start();
        s.reader.move_to("a", AgentState::Blocked, 2);
        assert_eq!(sent_at_least(&s.sink, 1).await[0].0.attention, 2);
        s.reader.move_to("b", AgentState::Done, 2);
        assert_eq!(sent_at_least(&s.sink, 2).await[1].0.attention, 1);
    }

    #[tokio::test]
    async fn per_kind_settings_are_honoured() {
        let dir = Scratch::new("monitor-per-kind");
        let s = setup(dir.join("monitor.json"), 60);
        lock(&s.cfg).notifications.finished = false;
        s.reader.set(vec![stream("a", AgentState::Working, 1)]);
        s.mon.start();
        s.reader.move_to("a", AgentState::Done, 2);
        settle().await;
        assert!(s.sink.sent().is_empty());
    }
}
