// WebSocket ↔ Herdr terminal stream bridge.
// Safety rules enforced here:
//  - One WebSocket is one connection generation. Input belongs to the connection it arrived on and
//    is never queued across connections, so nothing is replayed after a reconnect.
//  - Input is accepted only while this connection holds control and its stream is live. Input that
//    arrives while connecting, watching, or after losing control is rejected and dropped.
//  - Taking control from another client only happens when the client explicitly asks (takeover=1),
//    which the UI sends only after the user confirmed Take Over.
//  - Closing the socket (leaving, locking, backgrounding, network loss) releases control.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::app::{js_number, now_ms, Query};
use crate::diag::Diagnostics;
use crate::herdr::model::Model;
use crate::herdr::state::{HerdrReader, HerdrState};
use crate::herdr::stream::{
    CloseCategory, Direction, Frame, SessionEvent, StreamMode, TerminalSession,
};

pub const PROTOCOL: u32 = 1;
const MAX_INPUT_CHARS: usize = 128 * 1024;
const HEARTBEAT: Duration = Duration::from_secs(10);
/// A pane's input worker exits after this long without input.
const QUEUE_IDLE: Duration = Duration::from_secs(60);

pub fn encode_frame(f: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(10 + f.bytes.len());
    out.push(1);
    out.push(u8::from(f.full));
    out.extend_from_slice(&(f.seq as u32).to_be_bytes());
    out.extend_from_slice(&f.width.to_be_bytes());
    out.extend_from_slice(&f.height.to_be_bytes());
    out.extend_from_slice(&f.bytes);
    out
}

/// Remove control characters that could escape a bracketed paste or inject keystrokes.
pub fn sanitize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|&c| {
            !matches!(c, '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}' | '\u{80}'..='\u{9f}')
        })
        .collect()
}

/// `KEY_RE`: modifiers joined with `+`, then one key Herdr can encode for the program's mode.
fn valid_key(k: &str) -> bool {
    const MODS: [&str; 7] = ["ctrl", "control", "alt", "shift", "meta", "super", "cmd"];
    const NAMED: [&str; 12] = [
        "up",
        "down",
        "left",
        "right",
        "enter",
        "esc",
        "escape",
        "tab",
        "backspace",
        "space",
        "minus",
        "plus",
    ];
    let mut rest = k;
    'strip: loop {
        for m in MODS {
            if let Some(r) = rest.strip_prefix(m).and_then(|r| r.strip_prefix('+')) {
                rest = r;
                continue 'strip;
            }
        }
        break;
    }
    let single = |s: &str| {
        let mut cs = s.chars();
        matches!((cs.next(), cs.next()), (Some(c), None)
            if c.is_ascii_lowercase() || c.is_ascii_digit() || "@[]\\/.,;'`=".contains(c))
    };
    let function = |s: &str| {
        s.strip_prefix('f').is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "10" | "11" | "12"
            )
        })
    };
    single(rest) || function(rest) || NAMED.contains(&rest)
}

/// Keys sent as their standard sequences.
fn raw_key(k: &str) -> Option<&'static str> {
    Some(match k {
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "delete" => "\x1b[3~",
        "insert" => "\x1b[2~",
        _ => return None,
    })
}

/// JS string length (UTF-16 code units).
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn close_code(c: CloseCategory) -> u16 {
    match c {
        CloseCategory::Busy => 9,
        CloseCategory::TakenOver => 10,
        CloseCategory::Released => 11,
        CloseCategory::PaneGone => 4,
        CloseCategory::Exited => 12,
        CloseCategory::Error => 13,
    }
}

fn public_reason(c: CloseCategory, host: &str) -> String {
    match c {
        CloseCategory::Busy => "Another client is controlling this stream.".into(),
        CloseCategory::TakenOver => "Another client took control of this stream.".into(),
        CloseCategory::Released => format!("Control was handed back to {host}."),
        CloseCategory::PaneGone => "That stream no longer exists.".into(),
        CloseCategory::Exited => "The terminal connection ended.".into(),
        CloseCategory::Error => "The terminal connection failed.".into(),
    }
}

/// What the hub asks a connection's socket task to send.
enum Out {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

struct Conn {
    id: String,
    device_id: String,
    pane_id: String,
    client_session: String,
    mode: StreamMode,
    session: TerminalSession,
    tx: mpsc::UnboundedSender<Out>,
}

impl Conn {
    fn send_text(&self, v: Value) {
        let _ = self.tx.send(Out::Text(v.to_string()));
    }
}

/// One queued input call; checked again when its turn comes.
struct Job {
    conn: Arc<Conn>,
    id: Option<Value>,
    method: &'static str,
    params: Value,
}

pub struct TerminalHub {
    herdr: HerdrState,
    diag: Arc<Diagnostics>,
    /// "the Mac" or "the server", for the `released` reason.
    host: String,
    conns: Mutex<HashMap<String, Arc<Conn>>>,
    /// One FIFO worker per pane: input to a pane runs in arrival order, across connections.
    queues: Mutex<HashMap<String, mpsc::UnboundedSender<Job>>>,
    /// Every input call made to Herdr, for tests.
    sent: Mutex<Vec<(String, String)>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn fields(v: Value) -> BTreeMap<String, Value> {
    match v {
        Value::Object(m) => m.into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

impl TerminalHub {
    pub fn new(herdr: HerdrState, diag: Arc<Diagnostics>, host: String) -> Self {
        TerminalHub {
            herdr,
            diag,
            host,
            conns: Mutex::new(HashMap::new()),
            queues: Mutex::new(HashMap::new()),
            sent: Mutex::new(Vec::new()),
        }
    }

    fn event(&self, ev: &str, v: Value) {
        self.diag.event(ev, &fields(v));
    }

    /// Close every socket for `device_id` with `code` (default 4401).
    pub fn close_device(&self, device_id: &str, code: u16) {
        for c in lock(&self.conns)
            .values()
            .filter(|c| c.device_id == device_id)
        {
            let _ = c.tx.send(Out::Close(code, "revoked".into()));
        }
    }

    pub fn controller_count(&self) -> usize {
        lock(&self.conns)
            .values()
            .filter(|c| c.mode == StreamMode::Control)
            .count()
    }

    /// Number of input calls made to Herdr (the test hook's `hub.sent.length`).
    pub fn sent_count(&self) -> usize {
        lock(&self.sent).len()
    }

    fn is_current(&self, c: &Conn) -> bool {
        lock(&self.conns)
            .get(&c.id)
            .is_some_and(|x| std::ptr::eq(Arc::as_ptr(x), c))
    }

    /// Runs one /ws/term connection until it closes.
    pub async fn attach(self: &Arc<Self>, mut ws: WebSocket, device_id: String, q: Query) {
        let pane_id = q.get("pane").unwrap_or("").to_string();
        let mode = if q.get("mode") == Some("control") {
            StreamMode::Control
        } else {
            StreamMode::Observe
        };
        let takeover = mode == StreamMode::Control && q.get("takeover") == Some("1");
        let client_session: String = {
            let s = q.get("session").unwrap_or("");
            String::from_utf16_lossy(&s.encode_utf16().take(64).collect::<Vec<_>>())
        };
        let dim = |k: &str| {
            let n = js_number(q.get(k).unwrap_or("null"));
            (n.is_finite() && n != 0.0).then(|| n.round() as i64)
        };
        let (cols, rows) = (dim("cols"), dim("rows"));
        if js_number(q.get("v").unwrap_or("null")) != f64::from(PROTOCOL) {
            close(&mut ws, 4400, "protocol").await;
            return;
        }
        let exists = self
            .herdr
            .model()
            .is_some_and(|m: Model| m.streams.iter().any(|s| s.id == pane_id));
        if !exists {
            let msg = json!({"t": "closed", "category": "pane_gone", "reason": "That stream no longer exists."});
            let _ = ws.send(Message::Text(msg.to_string().into())).await;
            close(&mut ws, 4404, "pane_gone").await;
            return;
        }
        // A reconnect from the same screen supersedes its own previous connection first; that is not
        // taking control from another client.
        if mode == StreamMode::Control && !client_session.is_empty() {
            let prev = {
                let mut conns = lock(&self.conns);
                let id = conns
                    .values()
                    .find(|c| {
                        c.device_id == device_id
                            && c.pane_id == pane_id
                            && c.client_session == client_session
                            && c.mode == StreamMode::Control
                    })
                    .map(|c| c.id.clone());
                id.and_then(|id| conns.remove(&id))
            };
            if let Some(prev) = prev {
                prev.session.release().await;
                let _ = prev.tx.send(Out::Close(4000, "superseded".into()));
            }
        }
        // Peek never resizes the Mac's pane: observers ask for no size at all.
        let size = match (mode, cols, rows) {
            (StreamMode::Control, Some(c), Some(r)) => Some((c, r)),
            _ => None,
        };
        let target = self.herdr.herdr_target();
        let (session, mut events) = TerminalSession::start(&target, &pane_id, mode, size, takeover);
        let (tx, mut out_rx) = mpsc::unbounded_channel();
        let mut id_bytes = [0u8; 6];
        rand::fill(&mut id_bytes);
        let conn = Arc::new(Conn {
            id: hex::encode(id_bytes),
            device_id,
            pane_id: pane_id.clone(),
            client_session,
            mode,
            session,
            tx,
        });
        lock(&self.conns).insert(conn.id.clone(), conn.clone());
        let t0 = now_ms();
        self.event(
            "term_open",
            json!({"mode": mode.as_str(), "takeover": takeover, "stream": self.diag.anon(&pane_id)}),
        );
        conn.send_text(
            json!({"t": "hello", "conn": conn.id, "mode": mode.as_str(), "protocol": PROTOCOL}),
        );

        let mut alive = true;
        let mut live_pending = false;
        let mut events_open = true;
        let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        loop {
            tokio::select! {
                msg = ws.recv() => match msg {
                    Some(Ok(Message::Text(text))) => {
                        alive = true;
                        if let Ok(m) = serde_json::from_str::<Value>(text.as_str()) {
                            self.on_message(&conn, &m);
                        }
                    }
                    Some(Ok(Message::Binary(_))) | Some(Ok(Message::Pong(_))) => alive = true,
                    Some(Ok(Message::Ping(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                },
                ev = events.recv(), if events_open => match ev {
                    // The live message carries the first frame's size, so it waits for that frame.
                    Some(SessionEvent::Live) => live_pending = true,
                    Some(SessionEvent::Frame(f)) => {
                        if live_pending {
                            live_pending = false;
                            self.event("term_live", json!({"mode": mode.as_str(), "ms": now_ms().saturating_sub(t0)}));
                            conn.send_text(json!({"t": "live", "mode": mode.as_str(), "width": f.width, "height": f.height}));
                        }
                        let _ = conn.tx.send(Out::Binary(encode_frame(&f)));
                    }
                    Some(SessionEvent::Warning(code)) => conn.send_text(json!({"t": "warning", "code": code})),
                    Some(SessionEvent::Closed { category, .. }) => {
                        self.event("term_closed", json!({"mode": mode.as_str(), "category": category.as_str()}));
                        conn.send_text(json!({"t": "closed", "category": category.as_str(), "reason": public_reason(category, &self.host)}));
                        let removed = {
                            let mut conns = lock(&self.conns);
                            let current = conns.get(&conn.id).is_some_and(|x| Arc::ptr_eq(x, &conn));
                            if current {
                                conns.remove(&conn.id);
                            }
                            current
                        };
                        if removed {
                            let _ = conn.tx.send(Out::Close(4000 + close_code(category), category.as_str().into()));
                        }
                    }
                    None => events_open = false,
                },
                out = out_rx.recv() => match out {
                    Some(Out::Text(s)) => {
                        if ws.send(Message::Text(s.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Out::Binary(b)) => {
                        if ws.send(Message::Binary(b.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Out::Close(code, reason)) => {
                        close(&mut ws, code, &reason).await;
                        break;
                    }
                    None => break,
                },
                _ = beat.tick() => {
                    if !alive {
                        self.event("term_heartbeat_timeout", json!({"mode": mode.as_str()}));
                        break; // terminate: no close handshake
                    }
                    alive = false;
                    if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        // The socket is gone: this connection generation ends and control goes back.
        {
            let mut conns = lock(&self.conns);
            if conns.get(&conn.id).is_some_and(|x| Arc::ptr_eq(x, &conn)) {
                conns.remove(&conn.id);
            }
        }
        conn.session.release().await;
        self.event(
            "term_socket_closed",
            json!({"mode": mode.as_str(), "held_ms": now_ms().saturating_sub(t0)}),
        );
    }

    fn on_message(self: &Arc<Self>, c: &Arc<Conn>, msg: &Value) {
        let num = |k: &str| match msg.get(k) {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
            Some(Value::String(s)) => js_number(s),
            Some(Value::Bool(b)) => f64::from(u8::from(*b)),
            Some(Value::Null) => 0.0,
            _ => f64::NAN,
        };
        match msg.get("t").and_then(Value::as_str) {
            Some("ping") => {
                let mut pong = Map::new();
                pong.insert("t".into(), json!("pong"));
                if let Some(id) = msg.get("id") {
                    pong.insert("id".into(), id.clone());
                }
                c.send_text(Value::Object(pong));
            }
            Some("release") => {
                self.event("term_release", json!({}));
                let c = c.clone();
                tokio::spawn(async move { c.session.release().await });
            }
            Some("resize") => {
                let (cols, rows) = (num("cols"), num("rows"));
                // NaN would reach Herdr as null (JSON has no NaN); such a resize is dropped here instead.
                if c.mode == StreamMode::Control && cols.is_finite() && rows.is_finite() {
                    c.session.resize(cols.round() as i64, rows.round() as i64);
                }
            }
            Some("scroll") => {
                if c.mode != StreamMode::Control || !c.session.is_live() {
                    return self.reject(c, msg.get("id"), "not_controlling");
                }
                let direction = match msg.get("direction").and_then(Value::as_str) {
                    Some("up") => Direction::Up,
                    Some("down") => Direction::Down,
                    _ => return,
                };
                let (column, row) = (num("column"), num("row"));
                if column.is_finite() && row.is_finite() {
                    c.session
                        .scroll(direction, column.round() as i64, row.round() as i64);
                }
            }
            Some("input") => self.input(c, msg),
            _ => {}
        }
    }

    fn reject(&self, c: &Conn, id: Option<&Value>, reason: &str) {
        let id = id.filter(|v| v.is_number()).cloned().unwrap_or(Value::Null);
        c.send_text(json!({"t": "input_rejected", "id": id, "reason": reason}));
    }

    fn can_input(&self, c: &Conn) -> Option<&'static str> {
        if c.mode != StreamMode::Control {
            return Some("watching");
        }
        if c.session.is_connecting() {
            return Some("connecting");
        }
        if !c.session.is_live() || !self.is_current(c) {
            return Some("not_controlling");
        }
        None
    }

    fn input(self: &Arc<Self>, c: &Arc<Conn>, m: &Value) {
        if let Some(why) = self.can_input(c) {
            return self.reject(c, m.get("id"), why);
        }
        let pane = c.pane_id.as_str();
        let text_call = |text: &str| ("pane.send_text", json!({"pane_id": pane, "text": text}));
        let call = if let Some(text) = m
            .get("text")
            .and_then(Value::as_str)
            .filter(|t| js_len(t) <= MAX_INPUT_CHARS)
        {
            Some(text_call(text))
        } else if let Some(keys) = m.get("keys").and_then(Value::as_array).filter(|k| {
            k.len() <= 64
                && k.iter()
                    .all(|k| k.as_str().is_some_and(|k| valid_key(&k.to_lowercase())))
        }) {
            let keys: Vec<String> = keys
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_lowercase)
                .collect();
            Some(("pane.send_keys", json!({"pane_id": pane, "keys": keys})))
        } else if let Some(raw) = m.get("key").and_then(Value::as_str).and_then(raw_key) {
            Some(text_call(raw))
        } else if let Some(paste) = m.get("paste").filter(|p| {
            p.get("text")
                .and_then(Value::as_str)
                .is_some_and(|t| js_len(t) <= MAX_INPUT_CHARS)
        }) {
            let text = sanitize_paste(paste.get("text").and_then(Value::as_str).unwrap_or(""));
            // send_input applies the program's live bracketed-paste mode; send_text sends it unwrapped.
            if paste.get("bracketed").is_some_and(crate::app::js_truthy) {
                Some(("pane.send_input", json!({"pane_id": pane, "text": text})))
            } else {
                Some(text_call(&text))
            }
        } else {
            None
        };
        let Some((method, params)) = call else {
            return self.reject(c, m.get("id"), "invalid");
        };
        let job = Job {
            conn: c.clone(),
            id: m.get("id").cloned(),
            method,
            params,
        };
        self.enqueue(job);
    }

    /// Hands the job to the pane's worker, starting one if needed. Enqueueing happens under the
    /// queue lock, so a worker that is retiring never drops a job.
    fn enqueue(self: &Arc<Self>, job: Job) {
        let pane = job.conn.pane_id.clone();
        let mut queues = lock(&self.queues);
        let job = match queues.get(&pane) {
            Some(tx) => match tx.send(job) {
                Ok(()) => return,
                // The worker is gone (it can only end by removing its entry): start a new one.
                Err(mpsc::error::SendError(job)) => job,
            },
            None => job,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(job);
        queues.insert(pane.clone(), tx);
        let hub = self.clone();
        tokio::spawn(async move { hub.run_queue(pane, rx).await });
    }

    async fn run_queue(self: Arc<Self>, pane: String, mut rx: mpsc::UnboundedReceiver<Job>) {
        loop {
            let job = match tokio::time::timeout(QUEUE_IDLE, rx.recv()).await {
                Ok(Some(job)) => job,
                Ok(None) => return,
                Err(_) => {
                    let mut queues = lock(&self.queues);
                    if rx.is_empty() {
                        queues.remove(&pane);
                        return;
                    }
                    continue;
                }
            };
            // Re-check at execution time: control may have been lost while this waited in line.
            if let Some(late) = self.can_input(&job.conn) {
                self.reject(&job.conn, job.id.as_ref(), late);
                continue;
            }
            {
                let mut sent = lock(&self.sent);
                sent.push((job.conn.id.clone(), job.method.to_string()));
                if sent.len() > 1000 {
                    sent.drain(..500);
                }
            }
            match self.herdr.request(job.method, job.params).await {
                Ok(_) => {
                    let mut ok = Map::new();
                    ok.insert("t".into(), json!("input_ok"));
                    if let Some(id) = job.id {
                        ok.insert("id".into(), id);
                    }
                    job.conn.send_text(Value::Object(ok));
                }
                Err(e) => {
                    let reason = if e.code.is_empty() {
                        "failed"
                    } else {
                        e.code.as_str()
                    };
                    self.reject(&job.conn, job.id.as_ref(), reason);
                }
            }
        }
    }
}

async fn close(ws: &mut WebSocket, code: u16, reason: &str) {
    let _ = ws
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_encode_like_ts() {
        let f = Frame {
            seq: 0x1_0000_0002,
            width: 80,
            height: 24,
            full: true,
            bytes: vec![0xaa, 0xbb],
        };
        assert_eq!(
            encode_frame(&f),
            vec![1, 1, 0, 0, 0, 2, 0, 80, 0, 24, 0xaa, 0xbb]
        );
    }

    #[test]
    fn paste_is_sanitized() {
        assert_eq!(
            sanitize_paste("a\r\nb\tc\x1b[201~\u{9b}d\r"),
            "a\nb\tc[201~d"
        );
        assert_eq!(sanitize_paste("é ✓\n"), "é ✓\n");
    }

    #[test]
    fn keys_follow_key_re() {
        for k in [
            "a",
            "0",
            "f1",
            "f12",
            "ctrl+c",
            "ctrl+shift+tab",
            "alt+.",
            "cmd+[",
            "escape",
            "plus",
            "super+`",
        ] {
            assert!(valid_key(k), "{k}");
        }
        for k in [
            "",
            "f0",
            "f13",
            "f01",
            "ctrl+",
            "ctrl+home",
            "ab",
            "ctrl-c",
            "+a",
            "A",
            "pageup",
        ] {
            assert!(!valid_key(k), "{k}");
        }
        assert_eq!(raw_key("pageup"), Some("\x1b[5~"));
        assert_eq!(raw_key("constructor"), None);
    }

    #[test]
    fn close_codes_and_reasons() {
        assert_eq!(close_code(CloseCategory::PaneGone), 4);
        assert_eq!(close_code(CloseCategory::Error), 13);
        assert_eq!(
            public_reason(CloseCategory::Released, "the Mac"),
            "Control was handed back to the Mac."
        );
    }
}
