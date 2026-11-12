// One terminal's stream from herdr-core (`terminal.attach`), as the same session events Herdr TUI's
// stream gives: the first paint and then the program's raw output arrive as frames at the terminal's
// size. Observe asks the core to send that size (RESIZE) before every repaint and never types or
// resizes. Each attach has its own connection, closed with the session.
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::conn::{Channel, ChannelSender, Chunk, CoreConn};
use super::state::CoreTarget;
use crate::herdr::stream::{CloseCategory, Frame, SessionEvent, StreamMode};

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

struct Core {
    phase: Mutex<Phase>,
    events: mpsc::UnboundedSender<SessionEvent>,
    channel: Mutex<Option<ChannelSender>>,
}

impl Core {
    fn phase(&self) -> Phase {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self, category: CloseCategory, reason: impl Into<String>) {
        {
            let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
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
    /// Attaches to `terminal`. A failure is reported as a `Closed` event, not an error.
    pub fn start(
        target: &CoreTarget,
        terminal: &str,
        mode: StreamMode,
    ) -> (Self, mpsc::UnboundedReceiver<SessionEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        let (stop, stop_rx) = oneshot::channel();
        let core = Arc::new(Core {
            phase: Mutex::new(Phase::Connecting),
            events,
            channel: Mutex::new(None),
        });
        tokio::spawn(drive(
            target.clone(),
            terminal.to_string(),
            mode,
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

    /// The viewer wrote out `bytes` of this stream.
    pub fn credit(&self, bytes: usize) {
        if let Some(ch) = self
            .core
            .channel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            ch.credit(bytes);
        }
    }

    pub async fn release(&self) {
        self.kill();
    }

    pub fn kill(&self) {
        if let Some(stop) = self.stop.lock().unwrap_or_else(|e| e.into_inner()).take() {
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

async fn drive(
    target: CoreTarget,
    terminal: String,
    mode: StreamMode,
    core: Arc<Core>,
    mut stop: oneshot::Receiver<()>,
) {
    let attached = async {
        let (conn, _) = CoreConn::open(&target.socket, target.timeout).await?;
        // An observer's size is never applied; the core sends the terminal's own before painting.
        let params = json!({"terminal": terminal, "cols": 80, "rows": 24, "history": HISTORY,
            "observe": mode == StreamMode::Observe, "resize_notifications": true});
        let (result, channel) = conn.open_stream("terminal.attach", params).await?;
        Ok::<_, crate::herdr::rpc::HerdrError>((conn, result, channel))
    };
    let (conn, result, channel) = tokio::select! {
        r = attached => match r {
            Ok(v) => v,
            Err(e) => {
                let category = if e.code == "not_found" { CloseCategory::PaneGone } else { CloseCategory::Error };
                return core.close(category, e.message);
            }
        },
        _ = &mut stop => return,
    };
    *core.channel.lock().unwrap_or_else(|e| e.into_inner()) = Some(channel.sender());
    let size = |key: &str| {
        result
            .pointer(&format!("/terminal/{key}"))
            .and_then(Value::as_u64)
            .map_or(0, |n| n.min(u64::from(u16::MAX)) as u16)
    };
    let (cols, rows) = (size("cols"), size("rows"));
    tokio::select! {
        _ = pump(channel, &core, (cols, rows)) => {}
        _ = &mut stop => {}
    }
    drop(conn);
}

async fn pump(mut channel: Channel, core: &Core, mut size: (u16, u16)) {
    let mut seq = 0u64;
    loop {
        match channel.recv().await {
            Chunk::Resize(cols, rows) => size = (cols, rows),
            Chunk::Data(bytes) => {
                let first = seq == 0;
                seq += 1;
                let bytes = if first {
                    [RESET, bytes.as_slice()].concat()
                } else {
                    bytes
                };
                if first {
                    *core.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Live;
                    let _ = core.events.send(SessionEvent::Live);
                }
                let _ = core.events.send(SessionEvent::Frame(Frame {
                    seq,
                    width: size.0,
                    height: size.1,
                    full: first,
                    bytes,
                }));
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
    use crate::herdr::core::conn::{encode, CLOSE, DATA, RESIZE};
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

    /// A core that answers hello and one attach, then sends `frames` on channel 3.
    async fn fake_core(dir: &Scratch, frames: Vec<u8>) -> (CoreTarget, oneshot::Receiver<Value>) {
        let socket = dir.join("c.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let mut stream = reader.into_inner();
            let hello =
                json!({"jsonrpc": "2.0", "id": 0, "result": {"generation": 1, "framed": true}});
            stream
                .write_all(format!("{hello}\n").as_bytes())
                .await
                .unwrap();
            let mut header = [0u8; 9];
            stream.read_exact(&mut header).await.unwrap();
            let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await.unwrap();
            let req: Value = serde_json::from_slice(&payload).unwrap();
            let _ = tx.send(req["params"].clone());
            let reply = json!({"jsonrpc": "2.0", "id": req["id"],
                "result": {"channel": 3, "terminal": {"cols": 100, "rows": 30}}});
            let mut out = encode(0, DATA, reply.to_string().as_bytes());
            out.extend(frames);
            stream.write_all(&out).await.unwrap();
            let _ = stream.read(&mut [0u8; 64]).await;
        });
        (
            CoreTarget {
                socket,
                timeout: WAIT,
            },
            rx,
        )
    }

    #[tokio::test]
    async fn observe_streams_paint_and_output_at_the_terminal_size() {
        let dir = Scratch::new("cstream");
        let mut frames = encode(3, RESIZE, &[120, 0, 40, 0]);
        frames.extend(encode(3, DATA, b"paint"));
        frames.extend(encode(3, DATA, b"more"));
        frames.extend(encode(3, CLOSE, br#"{"reason":"exited","exit_code":0}"#));
        let (target, params) = fake_core(&dir, frames).await;
        let (_session, mut rx) = CoreSession::start(&target, "t4", StreamMode::Observe);
        let params = params.await.unwrap();
        assert_eq!(
            (
                params["observe"].clone(),
                params["resize_notifications"].clone()
            ),
            (json!(true), json!(true))
        );
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        let SessionEvent::Frame(first) = next(&mut rx).await else {
            panic!("no frame")
        };
        assert_eq!((first.width, first.height, first.full), (120, 40, true));
        assert_eq!(first.bytes, b"\x1bcpaint");
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
    async fn a_missing_core_closes_as_an_error() {
        let dir = Scratch::new("cstream-none");
        let target = CoreTarget {
            socket: dir.join("none.sock"),
            timeout: Duration::from_secs(5),
        };
        let (_session, mut rx) = CoreSession::start(&target, "t1", StreamMode::Observe);
        let SessionEvent::Closed { category, .. } = next(&mut rx).await else {
            panic!("not closed")
        };
        assert_eq!(category, CloseCategory::Error);
    }

    #[tokio::test]
    async fn kill_reports_released_once() {
        let dir = Scratch::new("cstream-kill");
        let (target, _params) = fake_core(&dir, encode(3, DATA, b"x")).await;
        let (session, mut rx) = CoreSession::start(&target, "t1", StreamMode::Observe);
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
