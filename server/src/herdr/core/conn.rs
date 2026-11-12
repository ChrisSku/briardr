// One framed connection to herdr-core (its schema/README.md): JSON-RPC on channel 0, event and
// terminal streams on their own channels. The socket is found the way `hc` finds it, and Briardr
// never starts a core: one started from Briardr would hand its environment to every terminal.
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch};

use crate::herdr::rpc::HerdrError;

pub const DATA: u8 = 0;
pub const CREDIT: u8 = 1;
pub const CLOSE: u8 = 2;
pub const RESIZE: u8 = 3;
const HEADER: usize = 9;
const MAX_PAYLOAD: usize = 16 << 20;
/// The protocol generation Briardr speaks.
const GENERATION: u64 = 1;

/// What arrives on a stream channel.
#[derive(Debug, PartialEq)]
pub enum Chunk {
    Data(Vec<u8>),
    Resize(u16, u16),
    /// The core ended the stream; the payload says why (`{"reason":"exited",...}`, or null).
    Closed(Value),
    /// The connection dropped.
    Lost,
}

type Reply = Result<(Value, Option<mpsc::UnboundedReceiver<Chunk>>), HerdrError>;

struct Shared {
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    channels: Mutex<HashMap<u32, mpsc::UnboundedSender<Chunk>>>,
    lost: watch::Sender<bool>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn error(code: &str, message: impl Into<String>) -> HerdrError {
    HerdrError {
        code: code.to_string(),
        message: message.into(),
    }
}

/// Cheap to clone; clones share the connection.
#[derive(Clone)]
pub struct CoreConn {
    frames: mpsc::UnboundedSender<Vec<u8>>,
    shared: Arc<Shared>,
    next_id: Arc<AtomicU64>,
    timeout: Duration,
}

impl CoreConn {
    /// Connects, says hello and switches to framed mode. Returns the hello result.
    pub async fn open(socket: &Path, timeout: Duration) -> Result<(CoreConn, Value), HerdrError> {
        let stream = UnixStream::connect(socket)
            .await
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    error("not_running", "herdr-core isn’t running")
                }
                _ => error("unreachable", format!("can’t connect to herdr-core: {e}")),
            })?;
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "hello",
            "params": {"framed": true, "generation": GENERATION}});
        let handshake = async {
            write
                .write_all(format!("{hello}\n").as_bytes())
                .await
                .map_err(|e| error("unreachable", e.to_string()))?;
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(n) if n > 0 => Ok(line),
                _ => Err(error("unreachable", "herdr-core closed the connection")),
            }
        };
        let line = tokio::time::timeout(timeout, handshake)
            .await
            .map_err(|_| error("timeout", "herdr-core didn’t answer"))??;
        let result = rpc_result(serde_json::from_str(&line).unwrap_or(Value::Null))?;
        if result.get("generation").and_then(Value::as_u64) != Some(GENERATION)
            || result.get("framed") != Some(&Value::Bool(true))
        {
            return Err(error(
                "incompatible",
                "this herdr-core speaks another protocol",
            ));
        }
        let (frames, frame_rx) = mpsc::unbounded_channel();
        let conn = CoreConn {
            frames,
            shared: Arc::new(Shared {
                pending: Mutex::new(HashMap::new()),
                channels: Mutex::new(HashMap::new()),
                lost: watch::channel(false).0,
            }),
            next_id: Arc::new(AtomicU64::new(1)),
            timeout,
        };
        tokio::spawn(write_frames(write, frame_rx, conn.shared.clone()));
        tokio::spawn(read_frames(reader, conn.shared.clone()));
        Ok((conn, result))
    }

    /// One request on channel 0.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, HerdrError> {
        self.request(method, params).await.map(|(v, _)| v)
    }

    /// A request whose result names a stream channel (`events.subscribe`, `terminal.attach`).
    pub async fn open_stream(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(Value, Channel), HerdrError> {
        let (result, rx) = self.request(method, params).await?;
        let (Some(id), Some(rx)) = (channel_of(&result), rx) else {
            return Err(error("invalid", format!("{method} named no stream")));
        };
        Ok((
            result,
            Channel {
                id,
                rx,
                conn: self.clone(),
                closed: false,
            },
        ))
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(Value, Option<mpsc::UnboundedReceiver<Chunk>>), HerdrError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.pending).insert(id, tx);
        if *self.shared.lost.borrow() {
            lock(&self.shared.pending).remove(&id);
            return Err(lost());
        }
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send(0, DATA, msg.to_string().as_bytes());
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(lost()),
            Err(_) => {
                lock(&self.shared.pending).remove(&id);
                Err(error(
                    "timeout",
                    format!(
                        "herdr-core didn’t answer {method} within {} ms",
                        self.timeout.as_millis()
                    ),
                ))
            }
        }
    }

    pub fn send(&self, channel: u32, kind: u8, payload: &[u8]) {
        let _ = self.frames.send(encode(channel, kind, payload));
    }

    /// Resolves once the connection has dropped.
    pub async fn lost(&self) {
        let mut rx = self.shared.lost.subscribe();
        let _ = rx.wait_for(|lost| *lost).await;
    }
}

fn lost() -> HerdrError {
    error("unreachable", "lost the connection to herdr-core")
}

fn channel_of(result: &Value) -> Option<u32> {
    result
        .get("channel")
        .and_then(Value::as_u64)
        .and_then(|c| u32::try_from(c).ok())
        .filter(|c| *c != 0)
}

/// A JSON-RPC reply's result, or its error as a `HerdrError` named by the error's stable kind.
fn rpc_result(msg: Value) -> Result<Value, HerdrError> {
    if let Some(e) = msg.get("error") {
        let kind = e
            .pointer("/data/kind")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let message = e.get("message").and_then(Value::as_str).unwrap_or("");
        return Err(error(kind, message));
    }
    match msg.get("result") {
        Some(result) => Ok(result.clone()),
        None => Err(error("invalid", "herdr-core sent no result")),
    }
}

/// One stream channel. Dropping it ends the stream.
pub struct Channel {
    pub id: u32,
    rx: mpsc::UnboundedReceiver<Chunk>,
    conn: CoreConn,
    closed: bool,
}

impl Channel {
    pub async fn recv(&mut self) -> Chunk {
        let chunk = self.rx.recv().await.unwrap_or(Chunk::Lost);
        if matches!(chunk, Chunk::Closed(_) | Chunk::Lost) {
            self.closed = true;
        }
        chunk
    }

    /// Tells the core that `bytes` of this stream's data were written out.
    pub fn credit(&self, bytes: usize) {
        let n = u32::try_from(bytes).unwrap_or(u32::MAX);
        self.conn.send(self.id, CREDIT, &n.to_le_bytes());
    }

    pub fn send(&self, kind: u8, payload: &[u8]) {
        self.conn.send(self.id, kind, payload);
    }

    pub fn sender(&self) -> ChannelSender {
        ChannelSender {
            id: self.id,
            conn: self.conn.clone(),
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        if !self.closed {
            self.conn.send(self.id, CLOSE, &[]);
        }
    }
}

/// Sends frames on a channel whose receiving end lives elsewhere.
#[derive(Clone)]
pub struct ChannelSender {
    id: u32,
    conn: CoreConn,
}

impl ChannelSender {
    pub fn send(&self, kind: u8, payload: &[u8]) {
        self.conn.send(self.id, kind, payload);
    }

    pub fn credit(&self, bytes: usize) {
        let n = u32::try_from(bytes).unwrap_or(u32::MAX);
        self.send(CREDIT, &n.to_le_bytes());
    }
}

pub fn encode(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

async fn write_frames(
    mut write: tokio::net::unix::OwnedWriteHalf,
    mut frames: mpsc::UnboundedReceiver<Vec<u8>>,
    shared: Arc<Shared>,
) {
    while let Some(frame) = frames.recv().await {
        if write.write_all(&frame).await.is_err() {
            break;
        }
    }
    shared.lost.send_replace(true);
}

async fn read_frames<R: AsyncRead + Unpin>(mut reader: BufReader<R>, shared: Arc<Shared>) {
    let mut header = [0u8; HEADER];
    loop {
        if reader.read_exact(&mut header).await.is_err() {
            break;
        }
        let channel = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let kind = header[4];
        let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
        if len > MAX_PAYLOAD {
            break;
        }
        let mut payload = vec![0u8; len];
        if reader.read_exact(&mut payload).await.is_err() {
            break;
        }
        if channel == 0 {
            if kind == DATA {
                on_reply(&shared, &payload);
            }
            continue;
        }
        let chunk = match kind {
            DATA => Chunk::Data(payload),
            RESIZE if payload.len() >= 4 => Chunk::Resize(
                u16::from_le_bytes([payload[0], payload[1]]),
                u16::from_le_bytes([payload[2], payload[3]]),
            ),
            CLOSE => {
                let reason = serde_json::from_slice(&payload).unwrap_or(Value::Null);
                let tx = lock(&shared.channels).remove(&channel);
                if let Some(tx) = tx {
                    let _ = tx.send(Chunk::Closed(reason));
                }
                continue;
            }
            _ => continue,
        };
        if let Some(tx) = lock(&shared.channels).get(&channel) {
            let _ = tx.send(chunk);
        }
    }
    shared.lost.send_replace(true);
    for (_, tx) in lock(&shared.pending).drain() {
        let _ = tx.send(Err(lost()));
    }
    for (_, tx) in lock(&shared.channels).drain() {
        let _ = tx.send(Chunk::Lost);
    }
}

/// A reply on channel 0. One that names a stream channel gets the channel's receiver here, in the
/// reader, so no frame for it can arrive before it's registered.
fn on_reply(shared: &Shared, payload: &[u8]) {
    let Ok(msg) = serde_json::from_slice::<Value>(payload) else {
        return;
    };
    let Some(id) = msg.get("id").and_then(Value::as_u64) else {
        return;
    };
    let Some(tx) = lock(&shared.pending).remove(&id) else {
        return;
    };
    let reply = rpc_result(msg).map(|result| {
        let rx = channel_of(&result).map(|channel| {
            let (ctx, crx) = mpsc::unbounded_channel();
            lock(&shared.channels).insert(channel, ctx);
            crx
        });
        (result, rx)
    });
    let _ = tx.send(reply);
}

/// The socket `hc` would use for `session` (empty: `HC_SESSION`, else "default"), unless `configured`
/// names one: `HC_SOCKET`, else `socket` in herdr-core's config.toml, else the runtime dir, else the
/// session's state dir.
pub fn socket_path(
    session: &str,
    configured: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> PathBuf {
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    let session = match session {
        "" => var("HC_SESSION").unwrap_or_else(|| "default".to_string()),
        s => s.to_string(),
    };
    if let Some(socket) = var("HC_SOCKET") {
        return PathBuf::from(socket);
    }
    let home = PathBuf::from(var("HOME").unwrap_or_else(|| "/".to_string()));
    let config = var("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let toml = std::fs::read_to_string(config.join("herdr-core").join("config.toml"));
    if let Some(socket) = toml.ok().and_then(|t| configured_socket(&t)) {
        return PathBuf::from(socket);
    }
    let run = var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| user_run_dir().filter(|p| p.is_dir()))
        .map(|run| run.join("herdr-core"));
    let dir = run.unwrap_or_else(|| {
        var("XDG_STATE_HOME")
            .map_or_else(|| home.join(".local").join("state"), PathBuf::from)
            .join("herdr-core")
            .join(&session)
    });
    dir.join(format!("{session}.sock"))
}

/// `/run/user/<uid>` on Linux; macOS has none.
fn user_run_dir() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self").ok()?.uid();
    Some(PathBuf::from(format!("/run/user/{uid}")))
}

/// A top-level `socket = "..."` in herdr-core's config.toml.
fn configured_socket(toml: &str) -> Option<String> {
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            return None;
        }
        let Some(value) = line
            .strip_prefix("socket")
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix('='))
            .map(str::trim)
        else {
            continue;
        };
        for quote in ['"', '\''] {
            if let Some(inner) = value
                .strip_prefix(quote)
                .and_then(|r| r.split(quote).next())
            {
                return Some(inner.to_string()).filter(|s| !s.is_empty());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::{Scratch, WAIT};
    use tokio::net::UnixListener;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    /// A fake core: answers hello, then runs `serve` on the framed connection.
    async fn fake_core<F, Fut>(dir: &Path, serve: F) -> PathBuf
    where
        F: FnOnce(UnixStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let socket = dir.join("c.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let hello: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(hello["params"], json!({"framed": true, "generation": 1}));
            let reply = json!({"jsonrpc": "2.0", "id": 0, "result":
                {"generation": 1, "framed": true, "version": "0.1.20", "features": []}});
            let mut stream = reader.into_inner();
            stream
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
            serve(stream).await;
        });
        socket
    }

    async fn read_frame(stream: &mut UnixStream) -> (u32, u8, Vec<u8>) {
        let mut header = [0u8; HEADER];
        stream.read_exact(&mut header).await.unwrap();
        let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await.unwrap();
        (
            u32::from_le_bytes([header[0], header[1], header[2], header[3]]),
            header[4],
            payload,
        )
    }

    #[test]
    fn frames_are_little_endian_channel_kind_length_payload() {
        assert_eq!(
            encode(258, RESIZE, &[1, 2]),
            vec![2, 1, 0, 0, 3, 2, 0, 0, 0, 1, 2]
        );
    }

    #[test]
    fn socket_follows_hc_rules() {
        let none = |_: &str| None;
        assert_eq!(
            socket_path("s", "/x/y.sock", &none),
            PathBuf::from("/x/y.sock")
        );
        let env = env_of(&[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/me")]);
        assert_eq!(
            socket_path("", "", &env),
            PathBuf::from("/run/me/herdr-core/default.sock")
        );
        let env = env_of(&[("HOME", "/h"), ("HC_SESSION", "scratch")]);
        let expected = if user_run_dir().is_some_and(|p| p.is_dir()) {
            user_run_dir().unwrap().join("herdr-core/scratch.sock")
        } else {
            PathBuf::from("/h/.local/state/herdr-core/scratch/scratch.sock")
        };
        assert_eq!(socket_path("", "", &env), expected);
        let env = env_of(&[("HOME", "/h"), ("HC_SOCKET", "/tmp/c.sock")]);
        assert_eq!(socket_path("", "", &env), PathBuf::from("/tmp/c.sock"));
    }

    #[test]
    fn config_socket_is_read_from_the_top_level_only() {
        assert_eq!(
            configured_socket("shell = [\"nu\"]\nsocket = \"/tmp/a.sock\" # mine\n"),
            Some("/tmp/a.sock".to_string())
        );
        assert_eq!(
            configured_socket("socket='/tmp/b.sock'"),
            Some("/tmp/b.sock".to_string())
        );
        assert_eq!(configured_socket("[x]\nsocket = \"/tmp/c.sock\""), None);
        assert_eq!(configured_socket("sockets = 1\n"), None);
    }

    #[tokio::test]
    async fn missing_socket_is_not_running() {
        let dir = Scratch::new("core-missing");
        let err = CoreConn::open(&dir.join("none.sock"), WAIT)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "not_running");
    }

    #[tokio::test]
    async fn silent_core_times_out() {
        let dir = Scratch::new("core-silent");
        let socket = dir.join("c.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let held = tokio::spawn(async move { listener.accept().await });
        let err = CoreConn::open(&socket, Duration::from_millis(200))
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "timeout");
        held.abort();
    }

    #[tokio::test]
    async fn calls_and_streams_share_the_connection() {
        let dir = Scratch::new("core-conn");
        let socket = fake_core(&dir, |mut stream| async move {
            let (ch, kind, payload) = read_frame(&mut stream).await;
            assert_eq!((ch, kind), (0, DATA));
            let req: Value = serde_json::from_slice(&payload).unwrap();
            assert_eq!(req["method"], "terminal.attach");
            // The reply and the stream's first bytes arrive back to back.
            let reply = json!({"jsonrpc": "2.0", "id": req["id"], "result": {"channel": 5}});
            let mut out = encode(0, DATA, reply.to_string().as_bytes());
            out.extend(encode(5, RESIZE, &[80, 0, 24, 0]));
            out.extend(encode(5, DATA, b"hi"));
            stream.write_all(&out).await.unwrap();
            let (ch, kind, payload) = read_frame(&mut stream).await;
            assert_eq!((ch, kind, payload), (5, CREDIT, 2u32.to_le_bytes().to_vec()));
            let (_, _, payload) = read_frame(&mut stream).await;
            let req: Value = serde_json::from_slice(&payload).unwrap();
            let reply = json!({"jsonrpc": "2.0", "id": req["id"],
                "error": {"code": -32000, "message": "no terminal t9", "data": {"kind": "not_found"}}});
            stream
                .write_all(&encode(0, DATA, reply.to_string().as_bytes()))
                .await
                .unwrap();
            stream
                .write_all(&encode(5, CLOSE, br#"{"reason":"exited"}"#))
                .await
                .unwrap();
            let _ = stream.read(&mut [0u8; 1]).await;
        })
        .await;
        let (conn, hello) = CoreConn::open(&socket, WAIT).await.unwrap();
        assert_eq!(hello["version"], "0.1.20");
        let (_, mut ch) = conn
            .open_stream("terminal.attach", json!({"terminal": "t1"}))
            .await
            .unwrap();
        assert_eq!(ch.recv().await, Chunk::Resize(80, 24));
        assert_eq!(ch.recv().await, Chunk::Data(b"hi".to_vec()));
        ch.credit(2);
        let err = conn.call("terminal.get", json!({})).await.err().unwrap();
        assert_eq!(
            (err.code.as_str(), err.message.as_str()),
            ("not_found", "no terminal t9")
        );
        assert_eq!(ch.recv().await, Chunk::Closed(json!({"reason": "exited"})));
    }

    #[tokio::test]
    async fn a_dropped_connection_fails_pending_calls_and_streams() {
        let dir = Scratch::new("core-drop");
        let socket = fake_core(&dir, |mut stream| async move {
            let (_, _, payload) = read_frame(&mut stream).await;
            let req: Value = serde_json::from_slice(&payload).unwrap();
            let reply = json!({"jsonrpc": "2.0", "id": req["id"], "result": {"channel": 2}});
            stream
                .write_all(&encode(0, DATA, reply.to_string().as_bytes()))
                .await
                .unwrap();
            let _ = read_frame(&mut stream).await;
        })
        .await;
        let (conn, _) = CoreConn::open(&socket, WAIT).await.unwrap();
        let (_, mut ch) = conn
            .open_stream("events.subscribe", json!({}))
            .await
            .unwrap();
        let err = conn.call("terminal.list", json!({})).await.err().unwrap();
        assert_eq!(err.code, "unreachable");
        assert_eq!(ch.recv().await, Chunk::Lost);
        tokio::time::timeout(WAIT, conn.lost()).await.unwrap();
    }
}
