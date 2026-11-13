// One terminal's stream from herdr-core (`terminal.attach`), as the same session events Herdr TUI's
// stream gives: the first paint and then the program's raw output arrive as frames at the terminal's
// size. Observe asks the core to send that size (RESIZE) before every repaint and never types or
// resizes. Control types, resizes and focuses. herdr-core takes bytes, so keys, pastes and the mouse
// are encoded here for the program's modes, read from its output (`vt.rs`, `keys.rs`). A controller
// gets no RESIZE, so it follows the terminal's size events: the Herdr app sets the size again while
// it is the one in use. Each attach has its own connection, closed with the session.
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::conn::{Channel, ChannelSender, Chunk, CoreConn, AUTOMATIC_INPUT, DATA, FOCUS, RESIZE};
use super::keys;
use super::state::CoreTarget;
use super::vt::{Modes, Vt};
use crate::herdr::stream::{
    clamp_dim, CloseCategory, Direction, Frame, Input, MouseAction, MouseButton, SessionEvent,
    StreamMode,
};

/// Scrollback lines sent with the first paint.
const HISTORY: u64 = 500;
/// Clears the screen and scrollback before the first paint, so a reattach doesn't repeat history.
const RESET: &[u8] = b"\x1bc";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connecting,
    Live,
    Closed,
}

/// Where input goes once attached.
struct Link {
    channel: ChannelSender,
    /// The core and this attach take a terminal's own replies as such (AUTOMATIC_INPUT).
    automatic: bool,
}

struct Core {
    phase: Mutex<Phase>,
    events: mpsc::UnboundedSender<SessionEvent>,
    link: Mutex<Option<Link>>,
    vt: Mutex<Vt>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Core {
    fn phase(&self) -> Phase {
        *lock(&self.phase)
    }

    fn close(&self, category: CloseCategory, reason: impl Into<String>) {
        {
            let mut phase = lock(&self.phase);
            if *phase == Phase::Closed {
                return;
            }
            *phase = Phase::Closed;
        }
        let _ = self.events.send(SessionEvent::Closed {
            category,
            reason: reason.into(),
        });
    }
}

pub struct CoreSession {
    pub pane_id: String,
    pub mode: StreamMode,
    core: Arc<Core>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

impl CoreSession {
    /// Attaches to `terminal`; control asks for `size` (cols, rows), or keeps the terminal's.
    /// A failure is reported as a `Closed` event, not an error.
    pub fn start(
        target: &CoreTarget,
        terminal: &str,
        mode: StreamMode,
        size: Option<(i64, i64)>,
    ) -> (Self, mpsc::UnboundedReceiver<SessionEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        let (stop, stop_rx) = oneshot::channel();
        let core = Arc::new(Core {
            phase: Mutex::new(Phase::Connecting),
            events,
            link: Mutex::new(None),
            vt: Mutex::new(Vt::default()),
        });
        tokio::spawn(drive(
            target.clone(),
            terminal.to_string(),
            mode,
            size,
            core.clone(),
            stop_rx,
        ));
        let session = CoreSession {
            pane_id: terminal.to_string(),
            mode,
            core,
            stop: Mutex::new(Some(stop)),
        };
        (session, rx)
    }

    pub fn is_connecting(&self) -> bool {
        self.core.phase() == Phase::Connecting
    }

    pub fn is_live(&self) -> bool {
        self.core.phase() == Phase::Live
    }

    fn modes(&self) -> Modes {
        lock(&self.core.vt).modes.clone()
    }

    /// Sends a frame on the attach channel, if this session controls a live stream.
    fn send(&self, kind: u8, payload: &[u8]) -> bool {
        if self.mode != StreamMode::Control || !self.is_live() {
            return false;
        }
        match lock(&self.core.link).as_ref() {
            Some(link) => {
                if !payload.is_empty() {
                    link.channel.send(kind, payload);
                }
                true
            }
            None => false,
        }
    }

    /// Types one input; false if this session can't type now. A key the program can't be sent
    /// (Command without the kitty keyboard protocol) is left out.
    pub fn input(&self, input: &Input) -> bool {
        self.send(DATA, &keys::input(input, &self.modes()))
    }

    /// The browser terminal's own reply to a query the program asked: not typing, so it doesn't
    /// make this viewer the one whose size counts.
    pub fn reply(&self, bytes: &[u8]) -> bool {
        let automatic = lock(&self.core.link).as_ref().is_some_and(|l| l.automatic);
        self.send(if automatic { AUTOMATIC_INPUT } else { DATA }, bytes)
    }

    pub fn resize(&self, cols: i64, rows: i64) {
        let (cols, rows) = (clamp_dim(cols, 2, 500), clamp_dim(rows, 2, 300));
        self.send(RESIZE, &[cols.to_le_bytes(), rows.to_le_bytes()].concat());
    }

    pub fn scroll(&self, direction: Direction, column: i64, row: i64) {
        if let Some(bytes) = keys::scroll(direction, column, row, &self.modes()) {
            self.send(DATA, &bytes);
        }
    }

    pub fn mouse(
        &self,
        action: MouseAction,
        button: MouseButton,
        column: i64,
        row: i64,
        modifiers: i64,
    ) {
        if let Some(bytes) = keys::mouse(action, button, column, row, modifiers, &self.modes()) {
            self.send(DATA, &bytes);
        }
    }

    /// The viewer wrote out a frame that carried `bytes` of this stream.
    pub fn credit(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        if let Some(link) = lock(&self.core.link).as_ref() {
            link.channel.credit(bytes);
        }
    }

    /// Detaches. The terminal keeps this viewer's size until the Herdr app is used again.
    pub async fn release(&self) {
        self.kill();
    }

    pub fn kill(&self) {
        if let Some(stop) = lock(&self.stop).take() {
            let _ = stop.send(());
        }
        self.core
            .close(CloseCategory::Released, "closed by briardr");
    }
}

impl Drop for CoreSession {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A terminal record's (or a size event's) grid.
fn grid(v: Option<&Value>) -> Option<(u16, u16)> {
    let dim = |k: &str| {
        v?.get(k)
            .and_then(Value::as_u64)
            .map(|n| n.min(u64::from(u16::MAX)) as u16)
    };
    Some((dim("cols")?, dim("rows")?))
}

async fn drive(
    target: CoreTarget,
    terminal: String,
    mode: StreamMode,
    size: Option<(i64, i64)>,
    core: Arc<Core>,
    mut stop: oneshot::Receiver<()>,
) {
    let control = mode == StreamMode::Control;
    let attached = async {
        let (conn, hello) = CoreConn::open(&target.socket, target.timeout).await?;
        // Subscribed first, so no size change after the attach goes unseen.
        let (sizes, current) = if control {
            let (snapshot, ch) = conn
                .open_stream("events.subscribe", json!({"terminal": terminal}))
                .await?;
            (Some(ch), grid(snapshot.pointer("/terminals/0")))
        } else {
            (None, None)
        };
        // An observer's size is never applied; the core sends the terminal's own before painting.
        let (cols, rows) = match size {
            Some((c, r)) => (clamp_dim(c, 2, 500), clamp_dim(r, 2, 300)),
            None => current.unwrap_or((80, 24)),
        };
        let params = json!({"terminal": terminal, "cols": cols, "rows": rows, "history": HISTORY,
            "observe": !control, "resize_notifications": !control});
        let (result, channel) = conn.open_stream("terminal.attach", params).await?;
        let offered = hello
            .get("features")
            .and_then(Value::as_array)
            .is_some_and(|f| f.contains(&json!("automatic_input")));
        let automatic = offered && result.get("automatic_input") == Some(&Value::Bool(true));
        Ok::<_, crate::herdr::rpc::HerdrError>((conn, result, channel, sizes, automatic))
    };
    let (conn, result, channel, sizes, automatic) = tokio::select! {
        r = attached => match r {
            Ok(v) => v,
            Err(e) => {
                let category = if e.code == "not_found" { CloseCategory::PaneGone } else { CloseCategory::Error };
                return core.close(category, e.message);
            }
        },
        _ = &mut stop => return,
    };
    if control {
        channel.send(FOCUS, &[1]); // the person is looking at it here now
    }
    *lock(&core.link) = Some(Link {
        channel: channel.sender(),
        automatic,
    });
    let size = grid(result.get("terminal")).unwrap_or((0, 0));
    tokio::select! {
        _ = pump(channel, sizes, &core, size) => {}
        _ = &mut stop => {}
    }
    *lock(&core.link) = None;
    drop(conn);
}

/// The next chunk of a stream that may not exist (then never).
async fn next_on(ch: &mut Option<Channel>) -> Chunk {
    match ch {
        Some(ch) => ch.recv().await,
        None => std::future::pending().await,
    }
}

async fn pump(mut channel: Channel, mut sizes: Option<Channel>, core: &Core, mut size: (u16, u16)) {
    let mut seq = 0u64;
    let mut lines: Vec<u8> = Vec::new();
    loop {
        let chunk = tokio::select! {
            chunk = channel.recv() => chunk,
            chunk = next_on(&mut sizes) => {
                let Chunk::Data(bytes) = chunk else {
                    sizes = None; // the attach stream reports its own end
                    continue;
                };
                if let Some(ch) = &sizes {
                    ch.credit(bytes.len());
                }
                lines.extend_from_slice(&bytes);
                let before = size;
                while let Some(end) = lines.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = lines.drain(..=end).collect();
                    let msg: Value = serde_json::from_slice(&line).unwrap_or(Value::Null);
                    let params = msg.get("params");
                    let next = match msg.get("method").and_then(Value::as_str) {
                        Some("event") if params.and_then(|p| p.get("type")) == Some(&json!("size")) => {
                            grid(params.and_then(|p| p.get("data")))
                        }
                        Some("missed") => grid(params.and_then(|p| p.pointer("/terminals/0"))),
                        _ => None,
                    };
                    size = next.unwrap_or(size);
                }
                // An empty frame at the new size: the viewer's grid follows even before output does.
                if size != before && seq > 0 {
                    seq += 1;
                    let _ = core.events.send(SessionEvent::Frame(Frame {
                        seq,
                        width: size.0,
                        height: size.1,
                        full: false,
                        bytes: Vec::new(),
                        credit: 0,
                    }));
                }
                continue;
            }
        };
        match chunk {
            Chunk::Resize(cols, rows) => size = (cols, rows),
            Chunk::Data(raw) => {
                let (shown, mouse) = {
                    let mut vt = lock(&core.vt);
                    let before = vt.modes.wants_mouse();
                    let shown = vt.feed(&raw);
                    let after = vt.modes.wants_mouse();
                    (shown, (after != before).then_some(after))
                };
                let first = seq == 0;
                if shown.is_empty() && !first {
                    channel.credit(raw.len()); // nothing for the viewer to write out
                } else {
                    seq += 1;
                    let bytes = if first {
                        [RESET, shown.as_slice()].concat()
                    } else {
                        shown
                    };
                    if first {
                        *lock(&core.phase) = Phase::Live;
                        let _ = core.events.send(SessionEvent::Live);
                    }
                    let _ = core.events.send(SessionEvent::Frame(Frame {
                        seq,
                        width: size.0,
                        height: size.1,
                        full: first,
                        bytes,
                        credit: raw.len(),
                    }));
                }
                if let Some(on) = mouse {
                    let _ = core.events.send(SessionEvent::Mouse(on));
                }
            }
            Chunk::Closed(reason) => {
                let why = reason
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("closed")
                    .to_string();
                return core.close(close_category(&why), why);
            }
            Chunk::Lost => return core.close(CloseCategory::Error, "lost herdr-core"),
        }
    }
}

/// A finished terminal's `reason`, as Briardr's close categories.
fn close_category(reason: &str) -> CloseCategory {
    match reason {
        "exited" => CloseCategory::Exited,
        "killed" | "removed" => CloseCategory::PaneGone,
        _ => CloseCategory::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::core::conn::{encode, CLOSE, CREDIT};
    use crate::scratch::{Scratch, WAIT};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    async fn next(rx: &mut mpsc::UnboundedReceiver<SessionEvent>) -> SessionEvent {
        tokio::time::timeout(WAIT, rx.recv())
            .await
            .expect("event timed out")
            .expect("event channel closed")
    }

    async fn recv<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> T {
        tokio::time::timeout(WAIT, rx.recv())
            .await
            .expect("timed out")
            .expect("closed")
    }

    /// A core that answers hello, `events.subscribe` (channel 5) and `terminal.attach` (channel 3),
    /// writes what `push` gets, and reports each call and every frame the viewer sends.
    struct Fake {
        target: CoreTarget,
        calls: mpsc::UnboundedReceiver<(String, Value)>,
        frames: mpsc::UnboundedReceiver<(u32, u8, Vec<u8>)>,
        push: mpsc::UnboundedSender<Vec<u8>>,
    }

    fn fake_core(dir: &Scratch) -> Fake {
        let socket = dir.join("c.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (calls_tx, calls) = mpsc::unbounded_channel();
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let (push, mut out) = mpsc::unbounded_channel::<Vec<u8>>();
        let reply = push.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let (mut read, mut write) = reader.into_inner().into_split();
            let hello = json!({"jsonrpc": "2.0", "id": 0,
                "result": {"generation": 1, "framed": true, "features": ["automatic_input"]}});
            write
                .write_all(format!("{hello}\n").as_bytes())
                .await
                .unwrap();
            tokio::spawn(async move {
                while let Some(bytes) = out.recv().await {
                    if write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
            });
            let mut header = [0u8; 9];
            while read.read_exact(&mut header).await.is_ok() {
                let channel = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
                let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
                let mut payload = vec![0u8; len];
                if read.read_exact(&mut payload).await.is_err() {
                    break;
                }
                if channel != 0 {
                    let _ = frames_tx.send((channel, header[4], payload));
                    continue;
                }
                let req: Value = serde_json::from_slice(&payload).unwrap();
                let method = req["method"].as_str().unwrap_or("").to_string();
                let result = if method == "events.subscribe" {
                    json!({"channel": 5, "seq": 1, "terminals": [{"id": "t4", "cols": 90, "rows": 30}]})
                } else {
                    json!({"channel": 3, "terminal": {"cols": 100, "rows": 30}, "automatic_input": true})
                };
                let msg = json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
                let _ = reply.send(encode(0, DATA, msg.to_string().as_bytes()));
                let _ = calls_tx.send((method, req["params"].clone()));
            }
        });
        Fake {
            target: CoreTarget {
                socket,
                timeout: WAIT,
            },
            calls,
            frames,
            push,
        }
    }

    #[tokio::test]
    async fn observe_streams_paint_and_output_at_the_terminal_size() {
        let dir = Scratch::new("cstream");
        let mut fake = fake_core(&dir);
        let (_session, mut rx) = CoreSession::start(&fake.target, "t4", StreamMode::Observe, None);
        let (method, params) = recv(&mut fake.calls).await;
        assert_eq!(
            method, "terminal.attach",
            "an observer follows no size events"
        );
        assert_eq!(
            (
                params["observe"].clone(),
                params["resize_notifications"].clone()
            ),
            (json!(true), json!(true))
        );
        let mut frames = encode(3, RESIZE, &[120, 0, 40, 0]);
        frames.extend(encode(3, DATA, b"paint"));
        frames.extend(encode(3, DATA, b"more"));
        frames.extend(encode(3, CLOSE, br#"{"reason":"exited","exit_code":0}"#));
        fake.push.send(frames).unwrap();
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        let SessionEvent::Frame(first) = next(&mut rx).await else {
            panic!("no frame")
        };
        assert_eq!((first.width, first.height, first.full), (120, 40, true));
        assert_eq!(
            (first.bytes.as_slice(), first.credit),
            (b"\x1bcpaint".as_slice(), 5)
        );
        let SessionEvent::Frame(second) = next(&mut rx).await else {
            panic!("no frame")
        };
        assert_eq!(
            (second.bytes.as_slice(), second.full),
            (b"more".as_slice(), false)
        );
        assert_eq!(
            next(&mut rx).await,
            SessionEvent::Closed {
                category: CloseCategory::Exited,
                reason: "exited".into()
            }
        );
    }

    #[tokio::test]
    async fn control_types_replies_and_resizes_for_the_programs_modes() {
        let dir = Scratch::new("cstream-ctl");
        let mut fake = fake_core(&dir);
        let (session, mut rx) =
            CoreSession::start(&fake.target, "t4", StreamMode::Control, Some((70, 20)));
        assert_eq!(recv(&mut fake.calls).await.0, "events.subscribe");
        let (method, params) = recv(&mut fake.calls).await;
        assert_eq!(method, "terminal.attach");
        assert_eq!(
            (
                params["observe"].clone(),
                params["cols"].clone(),
                params["rows"].clone()
            ),
            (json!(false), json!(70), json!(20))
        );
        assert_eq!(recv(&mut fake.frames).await, (3, FOCUS, vec![1]));
        assert!(
            !session.input(&Input::Text("x".into())),
            "nothing typed before the first paint"
        );

        fake.push
            .send(encode(3, DATA, b"$ \x1b[?2004h\x1b[?1000h"))
            .unwrap();
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        let SessionEvent::Frame(first) = next(&mut rx).await else {
            panic!("no frame")
        };
        assert_eq!(
            (first.bytes.as_slice(), first.credit, first.width),
            (b"\x1bc$ \x1b[?2004h".as_slice(), 18, 100)
        );
        assert_eq!(next(&mut rx).await, SessionEvent::Mouse(true));

        assert!(session.input(&Input::Keys(vec!["ctrl+c".into()])));
        assert!(session.input(&Input::Paste {
            text: "p".into(),
            bracketed: true
        }));
        assert!(session.reply(b"\x1b[?1;2c"));
        session.resize(1000, 1);
        assert_eq!(recv(&mut fake.frames).await, (3, DATA, b"\x03".to_vec()));
        assert_eq!(
            recv(&mut fake.frames).await,
            (3, DATA, b"\x1b[200~p\x1b[201~".to_vec())
        );
        assert_eq!(
            recv(&mut fake.frames).await,
            (3, AUTOMATIC_INPUT, b"\x1b[?1;2c".to_vec())
        );
        assert_eq!(
            recv(&mut fake.frames).await,
            (3, RESIZE, vec![0xf4, 1, 2, 0])
        );

        // Output that is only a reporting mode shows nothing, and is credited at once.
        fake.push.send(encode(3, DATA, b"\x1b[?1006h")).unwrap();
        assert_eq!(
            recv(&mut fake.frames).await,
            (3, CREDIT, 8u32.to_le_bytes().to_vec())
        );
        session.mouse(MouseAction::Down, MouseButton::Left, 1, 2, 0);
        session.scroll(Direction::Up, 0, 0);
        assert_eq!(
            recv(&mut fake.frames).await,
            (3, DATA, b"\x1b[<0;2;3M".to_vec())
        );
        assert_eq!(
            recv(&mut fake.frames).await,
            (3, DATA, b"\x1b[<64;1;1M".to_vec())
        );

        // The Herdr app became the active viewer and set its size.
        let ev = json!({"jsonrpc": "2.0", "method": "event",
            "params": {"seq": 2, "type": "size", "terminal": "t4", "data": {"cols": 120, "rows": 40}}});
        let line = format!("{ev}\n");
        fake.push.send(encode(5, DATA, line.as_bytes())).unwrap();
        let SessionEvent::Frame(resized) = next(&mut rx).await else {
            panic!("no frame")
        };
        assert_eq!(
            (resized.width, resized.height, resized.bytes.len()),
            (120, 40, 0)
        );
        let credit = (line.len() as u32).to_le_bytes().to_vec();
        assert_eq!(recv(&mut fake.frames).await, (5, CREDIT, credit));
    }

    #[tokio::test]
    async fn a_missing_core_closes_as_an_error() {
        let dir = Scratch::new("cstream-none");
        let target = CoreTarget {
            socket: dir.join("none.sock"),
            timeout: Duration::from_secs(5),
        };
        let (_session, mut rx) = CoreSession::start(&target, "t1", StreamMode::Observe, None);
        let SessionEvent::Closed { category, .. } = next(&mut rx).await else {
            panic!("not closed")
        };
        assert_eq!(category, CloseCategory::Error);
    }

    #[tokio::test]
    async fn kill_reports_released_once() {
        let dir = Scratch::new("cstream-kill");
        let mut fake = fake_core(&dir);
        let (session, mut rx) = CoreSession::start(&fake.target, "t1", StreamMode::Observe, None);
        recv(&mut fake.calls).await;
        fake.push.send(encode(3, DATA, b"x")).unwrap();
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        let _ = next(&mut rx).await;
        session.kill();
        assert_eq!(
            next(&mut rx).await,
            SessionEvent::Closed {
                category: CloseCategory::Released,
                reason: "closed by briardr".into()
            }
        );
        drop(session);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(rx.try_recv().is_err(), "a second close");
    }
}
