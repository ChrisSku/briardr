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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Number, Value};
use tokio::sync::mpsc;

use crate::app::{now_ms, Query};
use crate::diag::Diagnostics;
use crate::herdr::backend::{Backend, Herdr};
use crate::herdr::core::stream::CoreSession;
use crate::herdr::model::Model;
use crate::herdr::rpc::HerdrError;
use crate::herdr::state::HerdrReader;
use crate::herdr::stream::{
    CloseCategory, Direction, Frame, Input, MouseAction, MouseButton, SessionEvent, StreamMode,
    TerminalSession,
};

pub const PROTOCOL: u32 = 1;

/// Whether a socket's client speaks this protocol (`?v=1`); others are closed with 4400.
pub fn speaks_protocol(q: &Query) -> bool {
    q.get("v").and_then(|v| v.parse().ok()) == Some(PROTOCOL)
}
const MAX_INPUT_CHARS: usize = 128 * 1024;
/// A terminal's reply to a query is short.
const MAX_REPLY_CHARS: usize = 4096;
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

/// A key the client may send: modifiers joined with `+`, then one key Herdr can encode for the program's
/// mode.
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

/// A text's length as the client counts it, in UTF-16 code units.
fn utf16_len(s: &str) -> usize {
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

/// What a client sends on /ws/term, by its `t`. Anything else is ignored.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields = nullable))]
#[serde(tag = "t", rename_all = "snake_case")]
pub(crate) enum TermRequest {
    Ping {
        id: Option<Number>,
    },
    /// Hands control back to the host now, before the socket closes.
    Release,
    /// The controller's size, in cells. A watcher's is ignored: Peek never resizes.
    Resize {
        cols: i64,
        rows: i64,
    },
    Scroll {
        id: Option<Number>,
        direction: Direction,
        column: i64,
        row: i64,
    },
    Mouse {
        id: Option<Number>,
        action: MouseAction,
        button: MouseButton,
        column: i64,
        row: i64,
        modifiers: i64,
    },
    Input(InputRequest),
    /// The browser terminal's own reply to a program's query (herdr-core streams only).
    Reply {
        data: String,
    },
}

/// One `input`: the first of `text`, `keys`, `key` and `paste` that is valid is sent.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields = nullable))]
pub(crate) struct InputRequest {
    /// Echoed in `input_ok` or `input_rejected`.
    id: Option<Number>,
    text: Option<String>,
    /// Named keys with modifiers (`valid_key`).
    keys: Option<Vec<String>>,
    /// A key sent as its standard sequence (`raw_key`).
    key: Option<String>,
    paste: Option<PasteRequest>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields = nullable))]
pub(crate) struct PasteRequest {
    text: Option<String>,
    /// Wrapped as a bracketed paste when the program turned that on.
    bracketed: Option<bool>,
}

/// Enough of a message that doesn't parse to refuse it when it was an input.
#[derive(Deserialize)]
struct Tagged {
    t: String,
    id: Option<Value>,
}

/// What Briardr sends on /ws/term, by its `t` (frames go as binary messages, `encode_frame`).
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "t", rename_all = "snake_case")]
pub(crate) enum TermMessage<'a> {
    /// `raw`: the stream is the program's own output, so the browser's terminal keeps its scrollback.
    Hello {
        conn: &'a str,
        mode: StreamMode,
        protocol: u32,
        raw: bool,
    },
    /// The stream is live, at the size of its first frame.
    Live {
        mode: StreamMode,
        width: u16,
        height: u16,
    },
    /// Whether the program takes the mouse now.
    Modes {
        mouse: bool,
    },
    Warning {
        code: &'static str,
    },
    Closed {
        category: CloseCategory,
        reason: String,
    },
    InputOk {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        id: Option<Number>,
    },
    InputRejected {
        id: Option<Number>,
        reason: &'a str,
    },
    Pong {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        id: Option<Number>,
    },
}

/// What the hub asks a connection's socket task to send.
enum Out {
    Text(String),
    /// A frame, and the stream bytes to credit once it is sent.
    Binary(Vec<u8>, usize),
    Close(u16, String),
}

/// A pane's stream from Herdr TUI or herdr-core. herdr-core's is the program's raw output, so the
/// browser's terminal keeps the scrollback and answers the program's queries.
trait PaneStream: Send + Sync {
    fn is_connecting(&self) -> bool;
    fn is_live(&self) -> bool;
    fn resize(&self, cols: i64, rows: i64);
    fn scroll(&self, direction: Direction, column: i64, row: i64);
    fn mouse(
        &self,
        action: MouseAction,
        button: MouseButton,
        column: i64,
        row: i64,
        modifiers: i64,
    );
    fn release(&self) -> BoxFuture<'_, ()>;
    /// Whether the stream is the program's own output (herdr-core).
    fn raw(&self) -> bool {
        false
    }
    /// Writes input on the stream itself, in order: true when it was written. None for a stream that
    /// takes input as Herdr calls instead, which go through the pane's queue (Herdr TUI).
    fn write(&self, _input: &Input) -> Option<bool> {
        None
    }
    /// The browser terminal's reply to a program's query.
    fn reply(&self, _bytes: &[u8]) {}
    /// The viewer has `bytes` more of the stream; herdr-core sends more as they're written out.
    fn credit(&self, _bytes: usize) {}
}

impl PaneStream for TerminalSession {
    fn is_connecting(&self) -> bool {
        TerminalSession::is_connecting(self)
    }
    fn is_live(&self) -> bool {
        TerminalSession::is_live(self)
    }
    fn resize(&self, cols: i64, rows: i64) {
        TerminalSession::resize(self, cols, rows);
    }
    fn scroll(&self, direction: Direction, column: i64, row: i64) {
        TerminalSession::scroll(self, direction, column, row);
    }
    fn mouse(
        &self,
        action: MouseAction,
        button: MouseButton,
        column: i64,
        row: i64,
        modifiers: i64,
    ) {
        TerminalSession::mouse(self, action, button, column, row, modifiers);
    }
    fn release(&self) -> BoxFuture<'_, ()> {
        Box::pin(TerminalSession::release(self))
    }
}

impl PaneStream for CoreSession {
    fn is_connecting(&self) -> bool {
        CoreSession::is_connecting(self)
    }
    fn is_live(&self) -> bool {
        CoreSession::is_live(self)
    }
    fn resize(&self, cols: i64, rows: i64) {
        CoreSession::resize(self, cols, rows);
    }
    fn scroll(&self, direction: Direction, column: i64, row: i64) {
        CoreSession::scroll(self, direction, column, row);
    }
    fn mouse(
        &self,
        action: MouseAction,
        button: MouseButton,
        column: i64,
        row: i64,
        modifiers: i64,
    ) {
        CoreSession::mouse(self, action, button, column, row, modifiers);
    }
    fn release(&self) -> BoxFuture<'_, ()> {
        Box::pin(CoreSession::release(self))
    }
    fn raw(&self) -> bool {
        true
    }
    fn write(&self, input: &Input) -> Option<bool> {
        Some(self.input(input))
    }
    fn reply(&self, bytes: &[u8]) {
        CoreSession::reply(self, bytes);
    }
    fn credit(&self, bytes: usize) {
        CoreSession::credit(self, bytes);
    }
}

struct Conn {
    id: String,
    device_id: String,
    pane_id: String,
    client_session: String,
    mode: StreamMode,
    session: Box<dyn PaneStream>,
    tx: mpsc::UnboundedSender<Out>,
}

impl Conn {
    fn send(&self, m: &TermMessage) {
        if let Ok(text) = serde_json::to_string(m) {
            let _ = self.tx.send(Out::Text(text));
        }
    }
}

/// One queued input call; checked again when its turn comes.
struct Job {
    conn: Arc<Conn>,
    id: Option<Number>,
    method: &'static str,
    params: Value,
}

pub struct TerminalHub {
    herdr: Herdr,
    diag: Arc<Diagnostics>,
    /// "the Mac" or "the server", for the `released` reason.
    host: String,
    conns: Mutex<HashMap<String, Arc<Conn>>>,
    /// Control connections whose socket closed and whose control Herdr doesn't have back yet.
    releasing: AtomicUsize,
    /// One FIFO worker per pane: input to a pane runs in arrival order, across connections.
    queues: Mutex<HashMap<String, mpsc::UnboundedSender<Job>>>,
    /// Input calls made to Herdr, for the tests (`/__test/hub/sent`).
    #[cfg(feature = "test-hooks")]
    sent: AtomicUsize,
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
    pub fn new(herdr: Herdr, diag: Arc<Diagnostics>, host: String) -> Self {
        TerminalHub {
            herdr,
            diag,
            host,
            conns: Mutex::new(HashMap::new()),
            releasing: AtomicUsize::new(0),
            queues: Mutex::new(HashMap::new()),
            #[cfg(feature = "test-hooks")]
            sent: AtomicUsize::new(0),
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

    /// Ends every stream: Briardr now talks to another Herdr, whose streams these aren't.
    pub fn close_all(&self) {
        let reason = public_reason(CloseCategory::PaneGone, &self.host);
        for c in lock(&self.conns).values() {
            c.send(&TermMessage::Closed {
                category: CloseCategory::PaneGone,
                reason: reason.clone(),
            });
            let code = 4000 + close_code(CloseCategory::PaneGone);
            let _ = c.tx.send(Out::Close(code, "pane_gone".into()));
        }
    }

    /// Control connections open, or closed with their control still on its way back to Herdr: until
    /// then, another client asking for control only gets to watch (the test hook's `hub.controllers`).
    pub fn controller_count(&self) -> usize {
        let open = lock(&self.conns)
            .values()
            .filter(|c| c.mode == StreamMode::Control)
            .count();
        open + self.releasing.load(Ordering::SeqCst)
    }

    /// Number of input calls made to Herdr (the test hook's `hub.sent.length`).
    #[cfg(feature = "test-hooks")]
    pub fn sent_count(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }

    fn is_current(&self, c: &Conn) -> bool {
        lock(&self.conns)
            .get(&c.id)
            .is_some_and(|x| std::ptr::eq(Arc::as_ptr(x), c))
    }

    /// Runs one /ws/term connection until it closes.
    pub async fn attach(self: &Arc<Self>, mut ws: WebSocket, device_id: String, q: Query) {
        let pane_id = q.get("pane").unwrap_or("").to_string();
        let backend = self.herdr.backend();
        let mode = if q.get("mode") == Some("control") {
            StreamMode::Control
        } else {
            StreamMode::Observe
        };
        // herdr-core lets every viewer type, so no one is ever busy and taking over does nothing.
        let takeover = mode == StreamMode::Control && q.get("takeover") == Some("1");
        let client_session: String = {
            let s = q.get("session").unwrap_or("");
            String::from_utf16_lossy(&s.encode_utf16().take(64).collect::<Vec<_>>())
        };
        let dim = |k: &str| {
            q.get(k)
                .and_then(|n| n.parse::<i64>().ok())
                .filter(|&n| n != 0)
        };
        let (cols, rows) = (dim("cols"), dim("rows"));
        if !speaks_protocol(&q) {
            close(&mut ws, 4400, "protocol").await;
            return;
        }
        let exists = self
            .herdr
            .model()
            .is_some_and(|m: Model| m.streams.iter().any(|s| s.id == pane_id));
        if !exists {
            let msg = TermMessage::Closed {
                category: CloseCategory::PaneGone,
                reason: public_reason(CloseCategory::PaneGone, &self.host),
            };
            if let Ok(msg) = serde_json::to_string(&msg) {
                let _ = ws.send(Message::Text(msg.into())).await;
            }
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
        let (session, mut events): (Box<dyn PaneStream>, _) = match &backend {
            Backend::Tui(h) => {
                let (s, e) =
                    TerminalSession::start(&h.herdr_target(), &pane_id, mode, size, takeover);
                (Box::new(s), e)
            }
            Backend::Core(c) => {
                let (s, e) = CoreSession::start(&c.target(), &pane_id, mode, size);
                (Box::new(s), e)
            }
        };
        let raw = session.raw();
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
        conn.send(&TermMessage::Hello {
            conn: &conn.id,
            mode,
            protocol: PROTOCOL,
            raw,
        });

        let mut alive = true;
        let mut live_pending = false;
        let mut events_open = true;
        let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        loop {
            tokio::select! {
                msg = ws.recv() => match msg {
                    Some(Ok(Message::Text(text))) => {
                        alive = true;
                        self.on_message(&conn, text.as_str());
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
                            conn.send(&TermMessage::Live { mode, width: f.width, height: f.height });
                        }
                        let _ = conn.tx.send(Out::Binary(encode_frame(&f), f.credit));
                    }
                    Some(SessionEvent::Mouse(mouse)) => conn.send(&TermMessage::Modes { mouse }),
                    Some(SessionEvent::Warning(code)) => conn.send(&TermMessage::Warning { code }),
                    Some(SessionEvent::Closed { category, .. }) => {
                        self.event("term_closed", json!({"mode": mode.as_str(), "category": category.as_str()}));
                        conn.send(&TermMessage::Closed { category, reason: public_reason(category, &self.host) });
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
                    Some(Out::Binary(b, credit)) => {
                        if ws.send(Message::Binary(b.into())).await.is_err() {
                            break;
                        }
                        conn.session.credit(credit);
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
        let control = mode == StreamMode::Control;
        if control {
            self.releasing.fetch_add(1, Ordering::SeqCst);
        }
        {
            let mut conns = lock(&self.conns);
            if conns.get(&conn.id).is_some_and(|x| Arc::ptr_eq(x, &conn)) {
                conns.remove(&conn.id);
            }
        }
        conn.session.release().await;
        if control {
            self.releasing.fetch_sub(1, Ordering::SeqCst);
        }
        self.event(
            "term_socket_closed",
            json!({"mode": mode.as_str(), "held_ms": now_ms().saturating_sub(t0)}),
        );
    }

    fn on_message(self: &Arc<Self>, c: &Arc<Conn>, text: &str) {
        let msg = match serde_json::from_str::<TermRequest>(text) {
            Ok(msg) => msg,
            // An input that doesn't parse is refused, like one with nothing valid to send.
            Err(_) => {
                if let Ok(Tagged { t, id }) = serde_json::from_str(text) {
                    if t == "input" {
                        let id = id.and_then(|id| serde_json::from_value(id).ok());
                        let why = self.can_input(c).unwrap_or("invalid");
                        self.reject(c, id, why);
                    }
                }
                return;
            }
        };
        match msg {
            TermRequest::Ping { id } => c.send(&TermMessage::Pong { id }),
            TermRequest::Release => {
                self.event("term_release", json!({}));
                let c = c.clone();
                tokio::spawn(async move { c.session.release().await });
            }
            TermRequest::Resize { cols, rows } => {
                if c.mode == StreamMode::Control {
                    c.session.resize(cols, rows);
                }
            }
            TermRequest::Scroll {
                id,
                direction,
                column,
                row,
            } => {
                if c.mode != StreamMode::Control || !c.session.is_live() {
                    return self.reject(c, id, "not_controlling");
                }
                c.session.scroll(direction, column, row);
            }
            TermRequest::Mouse {
                id,
                action,
                button,
                column,
                row,
                modifiers,
            } => {
                if let Some(why) = self.can_input(c) {
                    return self.reject(c, id, why);
                }
                c.session.mouse(action, button, column, row, modifiers);
            }
            TermRequest::Input(input) => self.input(c, input),
            TermRequest::Reply { data } => {
                if utf16_len(&data) <= MAX_REPLY_CHARS && self.can_input(c).is_none() {
                    c.session.reply(data.as_bytes());
                }
            }
        }
    }

    fn reject(&self, c: &Conn, id: Option<Number>, reason: &str) {
        c.send(&TermMessage::InputRejected { id, reason });
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

    fn input(self: &Arc<Self>, c: &Arc<Conn>, req: InputRequest) {
        let id = req.id.clone();
        if let Some(why) = self.can_input(c) {
            return self.reject(c, id, why);
        }
        let Some(input) = parse_input(req) else {
            return self.reject(c, id, "invalid");
        };
        // herdr-core: written on this connection's own stream, in order, so there's nothing to queue.
        match c.session.write(&input) {
            Some(written) => {
                self.record_sent();
                if written {
                    c.send(&TermMessage::InputOk { id });
                } else {
                    self.reject(c, id, "not_controlling");
                }
            }
            None => {
                let (method, params) = tui_call(&c.pane_id, input);
                self.enqueue(Job {
                    conn: c.clone(),
                    id,
                    method,
                    params,
                });
            }
        }
    }

    /// Counts an input call made to Herdr, for the tests.
    fn record_sent(&self) {
        #[cfg(feature = "test-hooks")]
        self.sent.fetch_add(1, Ordering::SeqCst);
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
                self.reject(&job.conn, job.id, late);
                continue;
            }
            self.record_sent();
            let sent = match self.herdr.backend() {
                Backend::Tui(h) => h.request(job.method, job.params).await,
                Backend::Core(_) => Err(HerdrError {
                    code: "not_controlling".to_string(),
                    message: String::new(),
                }),
            };
            match sent {
                Ok(_) => job.conn.send(&TermMessage::InputOk { id: job.id }),
                Err(e) => {
                    let reason = if e.code.is_empty() {
                        "failed"
                    } else {
                        e.code.as_str()
                    };
                    self.reject(&job.conn, job.id, reason);
                }
            }
        }
    }
}

/// One `input` message's text, keys, key or paste, checked; None when it is none of them.
fn parse_input(req: InputRequest) -> Option<Input> {
    let short = |t: &String| utf16_len(t) <= MAX_INPUT_CHARS;
    if let Some(text) = req.text.filter(short) {
        return Some(Input::Text(text));
    }
    let keys = req
        .keys
        .map(|k| k.iter().map(|k| k.to_lowercase()).collect::<Vec<_>>());
    if let Some(keys) = keys.filter(|k| k.len() <= 64 && k.iter().all(|k| valid_key(k))) {
        return Some(Input::Keys(keys));
    }
    if let Some(raw) = req.key.as_deref().and_then(raw_key) {
        return Some(Input::Raw(raw));
    }
    let paste = req.paste?;
    let text = paste.text.filter(short)?;
    Some(Input::Paste {
        text: sanitize_paste(&text),
        bracketed: paste.bracketed == Some(true),
    })
}

/// The Herdr TUI call that sends `input` to `pane`.
fn tui_call(pane: &str, input: Input) -> (&'static str, Value) {
    match input {
        Input::Text(text) => ("pane.send_text", json!({"pane_id": pane, "text": text})),
        Input::Keys(keys) => ("pane.send_keys", json!({"pane_id": pane, "keys": keys})),
        Input::Raw(raw) => ("pane.send_text", json!({"pane_id": pane, "text": raw})),
        // send_input applies the program's live bracketed-paste mode; send_text sends it unwrapped.
        Input::Paste {
            text,
            bracketed: true,
        } => ("pane.send_input", json!({"pane_id": pane, "text": text})),
        Input::Paste { text, .. } => ("pane.send_text", json!({"pane_id": pane, "text": text})),
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
    fn frames_have_a_ten_byte_header() {
        let f = Frame {
            seq: 0x1_0000_0002,
            width: 80,
            height: 24,
            full: true,
            bytes: vec![0xaa, 0xbb],
            credit: 0,
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
    fn keys_are_modifiers_and_one_key_herdr_encodes() {
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

    fn request(v: Value) -> Option<TermRequest> {
        serde_json::from_value(v).ok()
    }

    #[test]
    fn messages_from_the_client_are_typed() {
        assert!(matches!(
            request(json!({"t": "ping", "id": 1_700_000_000_000u64})),
            Some(TermRequest::Ping { id: Some(id) }) if id.to_string() == "1700000000000"
        ));
        assert!(matches!(
            request(json!({"t": "release", "extra": 1})),
            Some(TermRequest::Release)
        ));
        assert!(matches!(
            request(json!({"t": "resize", "cols": 80, "rows": 24})),
            Some(TermRequest::Resize { cols: 80, rows: 24 })
        ));
        assert!(matches!(
            request(
                json!({"t": "mouse", "action": "drag", "button": "left", "column": 4, "row": 2, "modifiers": 0})
            ),
            Some(TermRequest::Mouse {
                action: MouseAction::Drag,
                button: MouseButton::Left,
                column: 4,
                ..
            })
        ));
        // What the client never sends is dropped: an unknown action, a number as text, a fraction.
        for bad in [
            json!({"t": "mouse", "action": "wiggle", "button": "left", "column": 1, "row": 1, "modifiers": 0}),
            json!({"t": "mouse", "action": "down", "button": "left", "column": "x", "row": 1, "modifiers": 0}),
            json!({"t": "resize", "cols": "80", "rows": 24}),
            json!({"t": "resize", "cols": 80.5, "rows": 24}),
            json!({"t": "scroll", "direction": "left", "column": 1, "row": 1}),
            json!({"t": "hello"}),
        ] {
            assert!(request(bad.clone()).is_none(), "{bad}");
        }
    }

    fn input(v: Value) -> Option<Input> {
        match request(v) {
            Some(TermRequest::Input(req)) => parse_input(req),
            other => panic!("not an input: {other:?}"),
        }
    }

    #[test]
    fn an_input_sends_the_first_of_text_keys_key_and_paste_that_is_valid() {
        assert_eq!(
            input(json!({"t": "input", "id": 1, "text": "ls"})),
            Some(Input::Text("ls".into()))
        );
        assert_eq!(
            input(json!({"t": "input", "keys": ["Ctrl+C"]})),
            Some(Input::Keys(vec!["ctrl+c".into()]))
        );
        assert_eq!(
            input(json!({"t": "input", "keys": ["ctrl+home"], "key": "home"})),
            Some(Input::Raw("\x1b[H")),
            "a key Herdr can't encode isn't sent as keys"
        );
        assert_eq!(
            input(json!({"t": "input", "paste": {"text": "a\x1bb", "bracketed": true}})),
            Some(Input::Paste {
                text: "ab".into(),
                bracketed: true
            })
        );
        assert_eq!(
            input(json!({"t": "input", "paste": {"text": "x"}})),
            Some(Input::Paste {
                text: "x".into(),
                bracketed: false
            })
        );
        let long = "x".repeat(MAX_INPUT_CHARS + 1);
        assert_eq!(input(json!({"t": "input", "text": long})), None);
        assert_eq!(input(json!({"t": "input", "key": "constructor"})), None);
        assert!(
            request(json!({"t": "input", "id": 2, "text": 5})).is_none(),
            "refused as invalid (`Tagged`)"
        );
    }

    #[test]
    fn messages_to_the_client_are_tagged_by_t() {
        let json = |m: TermMessage| serde_json::to_value(m).unwrap();
        assert_eq!(
            json(TermMessage::Hello {
                conn: "c1",
                mode: StreamMode::Control,
                protocol: PROTOCOL,
                raw: false
            }),
            json!({"t": "hello", "conn": "c1", "mode": "control", "protocol": 1, "raw": false})
        );
        assert_eq!(
            json(TermMessage::Live {
                mode: StreamMode::Observe,
                width: 80,
                height: 24
            }),
            json!({"t": "live", "mode": "observe", "width": 80, "height": 24})
        );
        assert_eq!(
            json(TermMessage::Closed {
                category: CloseCategory::TakenOver,
                reason: "Taken over.".into()
            }),
            json!({"t": "closed", "category": "taken_over", "reason": "Taken over."})
        );
        assert_eq!(
            json(TermMessage::InputOk { id: None }),
            json!({"t": "input_ok"})
        );
        assert_eq!(
            json(TermMessage::InputRejected {
                id: None,
                reason: "watching"
            }),
            json!({"t": "input_rejected", "id": null, "reason": "watching"})
        );
        assert_eq!(
            json(TermMessage::Pong { id: Some(7.into()) }),
            json!({"t": "pong", "id": 7})
        );
    }

    #[test]
    fn sockets_speak_protocol_1() {
        assert!(speaks_protocol(&Query::parse(Some("v=1&pane=w1:p1"))));
        for q in ["v=2", "v=1.0", "v=", "pane=w1:p1"] {
            assert!(!speaks_protocol(&Query::parse(Some(q))), "{q}");
        }
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
