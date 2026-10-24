// Herdr executable discovery and control-socket JSON-RPC. Env is scrubbed of HERDR_* before every spawn.
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

#[derive(Debug, Error)]
#[error("herdr {code}: {message}")]
pub struct HerdrError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrTarget {
    pub herdr_path: String,
    pub session: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrStatus {
    pub ok: bool,
    pub running: bool,
    pub version: Option<String>,
    pub socket: Option<PathBuf>,
    /// "not_found" | "not_running" | "incompatible" | "error" | None
    pub problem: Option<String>,
}

pub fn scrubbed_env() -> Vec<(String, String)> {
    scrub_vars(std::env::vars_os().map(|(k, v)| {
        (
            k.to_string_lossy().into_owned(),
            v.to_string_lossy().into_owned(),
        )
    }))
}

fn scrub_vars(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    vars.into_iter()
        .filter(|(k, _)| !k.starts_with("HERDR_"))
        .collect()
}

pub fn session_args(t: &HerdrTarget) -> Vec<String> {
    if t.session.is_empty() {
        Vec::new()
    } else {
        vec!["--session".to_string(), t.session.clone()]
    }
}

pub async fn herdr_status(t: &HerdrTarget) -> HerdrStatus {
    let mut cmd = Command::new(&t.herdr_path);
    cmd.args(session_args(t))
        .arg("status")
        .env_clear()
        .envs(scrubbed_env())
        .kill_on_drop(true);
    let stdout = match tokio::time::timeout(t.timeout, cmd.output()).await {
        Ok(Ok(out)) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(Err(e)) if e.kind() == io::ErrorKind::NotFound => {
            return HerdrStatus {
                ok: false,
                running: false,
                version: None,
                socket: None,
                problem: Some("not_found".to_string()),
            };
        }
        _ => String::new(),
    };
    parse_status(&stdout)
}

/// The `server:` block of `herdr status` output. A block ends at the next line that starts
/// with a non-space character (a newline followed by `\S`).
fn server_block(text: &str) -> &str {
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let starts_block = line.chars().next().is_some_and(|c| !c.is_whitespace());
        match start {
            Some(s) if starts_block => return &text[s..offset],
            Some(_) => {}
            None if line.starts_with("server:") => start = Some(offset),
            None => {}
        }
        offset += line.len();
    }
    start.map_or("", |s| &text[s..])
}

/// The first indented `key: value` line in a block. An empty value reads as absent.
fn block_value(block: &str, key: &str) -> Option<String> {
    for line in block.lines() {
        let indented = line.trim_start();
        if indented.len() == line.len() {
            continue;
        }
        if let Some(rest) = indented.strip_prefix(key).and_then(|r| r.strip_prefix(':')) {
            let value = rest.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

fn parse_status(text: &str) -> HerdrStatus {
    let server = server_block(text);
    let status = block_value(server, "status");
    let socket = block_value(server, "socket").map(PathBuf::from);
    let version = block_value(server, "version");
    let compatible = block_value(server, "endpoint_compatible");
    if status.as_deref() != Some("running") {
        let problem = if text.is_empty() {
            "error"
        } else {
            "not_running"
        };
        return HerdrStatus {
            ok: false,
            running: false,
            version,
            socket,
            problem: Some(problem.to_string()),
        };
    }
    if compatible.is_some_and(|c| c != "yes") {
        return HerdrStatus {
            ok: false,
            running: true,
            version,
            socket,
            problem: Some("incompatible".to_string()),
        };
    }
    let ok = socket.is_some();
    HerdrStatus {
        ok,
        running: true,
        version,
        socket,
        problem: if ok { None } else { Some("error".to_string()) },
    }
}

/// One request per connection. Errors: timeout | unreachable | socket | closed | the Herdr error code.
pub async fn herdr_request(
    socket: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, HerdrError> {
    match tokio::time::timeout(timeout, request_once(socket, method, params)).await {
        Ok(result) => result,
        Err(_) => Err(HerdrError {
            code: "timeout".to_string(),
            message: format!(
                "Herdr didn't answer {method} within {} ms",
                timeout.as_millis()
            ),
        }),
    }
}

async fn request_once(socket: &Path, method: &str, params: Value) -> Result<Value, HerdrError> {
    let id = format!("k{}", random_hex::<6>());
    let stream = UnixStream::connect(socket).await.map_err(connect_error)?;
    let (read_half, mut write_half) = stream.into_split();
    write_line(
        &mut write_half,
        &json!({ "id": id, "method": method, "params": params }),
    )
    .await
    .map_err(|_| socket_error())?;
    let mut reader = BufReader::new(read_half);
    let mut buf = Vec::new();
    loop {
        let line = next_line(&mut reader, &mut buf)
            .await
            .map_err(|_| socket_error())?
            .ok_or_else(closed_error)?;
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let answers = msg
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|mid| mid == id || mid.is_empty());
        if !answers {
            continue;
        }
        if let Some(err) = msg.get("error").filter(|e| truthy(e)) {
            return Err(HerdrError {
                code: error_code(err),
                message: error_message(err),
            });
        }
        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
    }
}

/// Events from a long-lived subscription. Connection failures arrive as `End`, not as an error return.
#[derive(Debug, Clone, PartialEq)]
pub enum SubEvent {
    Started,
    Event {
        name: String,
        data: Value,
    },
    /// Sent exactly once, last. Reason is `closed_by_briardr`, `closed`, `socket`, or a Herdr error code.
    End(String),
}

pub struct Subscription {
    pub events: UnboundedReceiver<SubEvent>,
    /// Ends the subscription with reason `closed_by_briardr`, once.
    pub close: Box<dyn FnOnce() + Send>,
}

/// Starts `events.subscribe` on its own task. Returns at once; the task reports through `events`.
pub fn herdr_subscribe(socket: &Path, subscriptions: Vec<Value>) -> Subscription {
    let (tx, events) = mpsc::unbounded_channel();
    let (close_tx, close_rx) = oneshot::channel::<()>();
    let socket = socket.to_path_buf();
    tokio::spawn(async move {
        let reason = tokio::select! {
            reason = run_subscription(&socket, subscriptions, &tx) => reason,
            () = close_requested(close_rx) => "closed_by_briardr".to_string(),
        };
        let _ = tx.send(SubEvent::End(reason));
    });
    Subscription {
        events,
        close: Box::new(move || {
            let _ = close_tx.send(());
        }),
    }
}

/// Resolves when the close hook is called. A dropped Subscription does not close the connection.
async fn close_requested(close_rx: oneshot::Receiver<()>) {
    if close_rx.await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Runs the subscription until it ends and returns the End reason.
async fn run_subscription(
    socket: &Path,
    subscriptions: Vec<Value>,
    tx: &UnboundedSender<SubEvent>,
) -> String {
    let id = format!("sub{}", random_hex::<4>());
    let Ok(stream) = UnixStream::connect(socket).await else {
        return "socket".to_string();
    };
    let (read_half, mut write_half) = stream.into_split();
    let request = json!({
        "id": id,
        "method": "events.subscribe",
        "params": { "subscriptions": subscriptions },
    });
    if write_line(&mut write_half, &request).await.is_err() {
        return "socket".to_string();
    }
    let mut reader = BufReader::new(read_half);
    let mut buf = Vec::new();
    loop {
        let line = match next_line(&mut reader, &mut buf).await {
            Ok(Some(line)) => line,
            Ok(None) => return "closed".to_string(),
            Err(_) => return "socket".to_string(),
        };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(err) = msg.get("error").filter(|e| truthy(e)) {
            return error_code(err);
        }
        let sent = if msg.pointer("/result/type").and_then(Value::as_str)
            == Some("subscription_started")
        {
            tx.send(SubEvent::Started)
        } else if let Some(event) = msg.get("event").filter(|e| truthy(e)) {
            let name = text(event);
            let data = msg.get("data").cloned().unwrap_or(Value::Null);
            tx.send(SubEvent::Event { name, data })
        } else {
            Ok(())
        };
        // The consumer is gone, so nobody is left to hear the reason.
        if sent.is_err() {
            return "closed_by_briardr".to_string();
        }
    }
}

fn random_hex<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    rand::fill(&mut bytes);
    hex::encode(bytes)
}

async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, msg: &Value) -> io::Result<()> {
    let mut line = msg.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes()).await
}

/// The next complete line without its newline, or None at EOF. A partial last line is dropped.
async fn next_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> io::Result<Option<String>> {
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', buf).await?;
        if n == 0 {
            return Ok(None);
        }
        if buf.last() == Some(&b'\n') {
            return Ok(Some(
                String::from_utf8_lossy(&buf[..buf.len() - 1]).into_owned(),
            ));
        }
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn text(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

fn error_code(err: &Value) -> String {
    err.get("code")
        .filter(|c| truthy(c))
        .map_or_else(|| "error".to_string(), text)
}

fn error_message(err: &Value) -> String {
    err.get("message")
        .filter(|m| truthy(m))
        .map_or_else(|| "Herdr error".to_string(), text)
}

fn connect_error(e: io::Error) -> HerdrError {
    match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => HerdrError {
            code: "unreachable".to_string(),
            message: "Herdr socket unavailable".to_string(),
        },
        _ => socket_error(),
    }
}

fn socket_error() -> HerdrError {
    HerdrError {
        code: "socket".to_string(),
        message: "Herdr socket unavailable".to_string(),
    }
}

fn closed_error() -> HerdrError {
    HerdrError {
        code: "closed".to_string(),
        message: "Herdr closed the connection".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use tokio::net::unix::OwnedWriteHalf;
    use tokio::net::UnixListener;

    /// Where a test's fake Herdr server listens, in a scratch directory with a short name (a socket path
    /// can't be much longer than 100 bytes).
    fn socket(dir: &Scratch) -> PathBuf {
        dir.join("s.sock")
    }

    /// Accepts one connection and reads its request line.
    async fn accept_request(listener: &UnixListener) -> (Value, OwnedWriteHalf) {
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = stream.into_split();
        let mut line = String::new();
        BufReader::new(read_half)
            .read_line(&mut line)
            .await
            .unwrap();
        (serde_json::from_str(&line).unwrap(), write_half)
    }

    async fn reply(writer: &mut OwnedWriteHalf, msg: Value) {
        writer
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
    }

    fn target(session: &str) -> HerdrTarget {
        HerdrTarget {
            herdr_path: "herdr".to_string(),
            session: session.to_string(),
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn herdr_rpc_scrubbed_env_drops_herdr_keys() {
        let vars = [
            ("HERDR_SOCKET", "/x"),
            ("HERDR_PANE_ID", "3"),
            ("HOME", "/home/k"),
            ("PATH", "/bin"),
            ("HERDRX", "keep"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let scrubbed = scrub_vars(vars);
        let keys: Vec<&str> = scrubbed.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["HOME", "PATH", "HERDRX"]);
        assert!(!scrubbed_env().iter().any(|(k, _)| k.starts_with("HERDR_")));
    }

    #[test]
    fn herdr_rpc_session_args_both_ways() {
        assert_eq!(
            session_args(&target("briardr-dev")),
            vec!["--session", "briardr-dev"]
        );
        assert!(session_args(&target("")).is_empty());
    }

    #[test]
    fn herdr_rpc_status_running_block() {
        let text = "client:\n  version: 0.9.0\n\nserver:\n  status: running\n  version: 0.5.2\n  socket: /tmp/herdr.sock\n  endpoint_compatible: yes\n";
        assert_eq!(
            parse_status(text),
            HerdrStatus {
                ok: true,
                running: true,
                version: Some("0.5.2".to_string()),
                socket: Some(PathBuf::from("/tmp/herdr.sock")),
                problem: None,
            }
        );
    }

    #[test]
    fn herdr_rpc_status_not_running_block() {
        let text = "server:\n  status: stopped\n  version: 0.5.2\n  socket: /tmp/herdr.sock\n";
        assert_eq!(
            parse_status(text),
            HerdrStatus {
                ok: false,
                running: false,
                version: Some("0.5.2".to_string()),
                socket: Some(PathBuf::from("/tmp/herdr.sock")),
                problem: Some("not_running".to_string()),
            }
        );
    }

    #[test]
    fn herdr_rpc_status_incompatible_block() {
        let text = "server:\n  status: running\n  version: 0.4.0\n  socket: /tmp/h.sock\n  endpoint_compatible: no\n";
        assert_eq!(
            parse_status(text),
            HerdrStatus {
                ok: false,
                running: true,
                version: Some("0.4.0".to_string()),
                socket: Some(PathBuf::from("/tmp/h.sock")),
                problem: Some("incompatible".to_string()),
            }
        );
    }

    #[test]
    fn herdr_rpc_status_empty_output() {
        assert_eq!(
            parse_status(""),
            HerdrStatus {
                ok: false,
                running: false,
                version: None,
                socket: None,
                problem: Some("error".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn herdr_rpc_status_missing_binary_is_not_found() {
        let t = HerdrTarget {
            herdr_path: "/nonexistent-briardr-dir/herdr".to_string(),
            session: String::new(),
            timeout: Duration::from_secs(5),
        };
        let status = herdr_status(&t).await;
        assert_eq!(status.problem.as_deref(), Some("not_found"));
        assert!(!status.ok && !status.running);
    }

    #[tokio::test]
    async fn herdr_rpc_request_round_trip() {
        let dir = Scratch::new("rpc-req");
        let listener = UnixListener::bind(socket(&dir)).unwrap();
        let server = tokio::spawn(async move {
            let (req, mut w) = accept_request(&listener).await;
            reply(
                &mut w,
                json!({ "id": req["id"], "result": { "panes": [] } }),
            )
            .await;
            req
        });
        let result = herdr_request(
            &socket(&dir),
            "pane.list",
            json!({ "session": "briardr-dev" }),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(result, json!({ "panes": [] }));
        let req = server.await.unwrap();
        assert_eq!(req["method"], "pane.list");
        assert_eq!(req["params"], json!({ "session": "briardr-dev" }));
        let id = req["id"].as_str().unwrap();
        assert_eq!(id.len(), 13);
        assert!(id.starts_with('k'));
    }

    #[tokio::test]
    async fn herdr_rpc_request_server_error_returns_code() {
        let dir = Scratch::new("rpc-err");
        let listener = UnixListener::bind(socket(&dir)).unwrap();
        tokio::spawn(async move {
            let (req, mut w) = accept_request(&listener).await;
            reply(
                &mut w,
                json!({ "id": req["id"], "error": { "code": "pane_not_found", "message": "gone" } }),
            )
            .await;
        });
        let err = herdr_request(&socket(&dir), "pane.get", json!({}), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(err.code, "pane_not_found");
        assert_eq!(err.message, "gone");
    }

    #[tokio::test]
    async fn herdr_rpc_request_timeout() {
        let dir = Scratch::new("rpc-timeout");
        let listener = UnixListener::bind(socket(&dir)).unwrap();
        tokio::spawn(async move {
            let (_req, _w) = accept_request(&listener).await;
            std::future::pending::<()>().await;
        });
        let err = herdr_request(
            &socket(&dir),
            "pane.list",
            json!({}),
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "timeout");
        assert_eq!(err.message, "Herdr didn't answer pane.list within 200 ms");
    }

    #[tokio::test]
    async fn herdr_rpc_request_missing_socket_is_unreachable() {
        let dir = Scratch::new("rpc-missing");
        let err = herdr_request(
            &socket(&dir),
            "pane.list",
            json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "unreachable");
        assert_eq!(err.message, "Herdr socket unavailable");
    }

    #[tokio::test]
    async fn herdr_rpc_subscribe_started_event_then_closed() {
        let dir = Scratch::new("rpc-sub");
        let listener = UnixListener::bind(socket(&dir)).unwrap();
        let server = tokio::spawn(async move {
            let (req, mut w) = accept_request(&listener).await;
            reply(
                &mut w,
                json!({ "id": req["id"], "result": { "type": "subscription_started" } }),
            )
            .await;
            reply(
                &mut w,
                json!({ "event": "pane.changed", "data": { "x": 1 } }),
            )
            .await;
            req
        });
        let mut sub = herdr_subscribe(&socket(&dir), vec![json!({ "kind": "pane" })]);
        assert_eq!(sub.events.recv().await, Some(SubEvent::Started));
        assert_eq!(
            sub.events.recv().await,
            Some(SubEvent::Event {
                name: "pane.changed".to_string(),
                data: json!({ "x": 1 }),
            })
        );
        assert_eq!(
            sub.events.recv().await,
            Some(SubEvent::End("closed".to_string()))
        );
        assert_eq!(sub.events.recv().await, None);
        let req = server.await.unwrap();
        assert_eq!(req["method"], "events.subscribe");
        assert_eq!(
            req["params"],
            json!({ "subscriptions": [{ "kind": "pane" }] })
        );
        let id = req["id"].as_str().unwrap();
        assert_eq!(id.len(), 11);
        assert!(id.starts_with("sub"));
    }

    #[tokio::test]
    async fn herdr_rpc_subscribe_close_ends_once() {
        let dir = Scratch::new("rpc-close");
        let listener = UnixListener::bind(socket(&dir)).unwrap();
        tokio::spawn(async move {
            let (req, mut w) = accept_request(&listener).await;
            reply(
                &mut w,
                json!({ "id": req["id"], "result": { "type": "subscription_started" } }),
            )
            .await;
            std::future::pending::<()>().await;
        });
        let mut sub = herdr_subscribe(&socket(&dir), Vec::new());
        assert_eq!(sub.events.recv().await, Some(SubEvent::Started));
        (sub.close)();
        assert_eq!(
            sub.events.recv().await,
            Some(SubEvent::End("closed_by_briardr".to_string()))
        );
        assert_eq!(sub.events.recv().await, None);
    }

    #[tokio::test]
    async fn herdr_rpc_subscribe_missing_socket_ends_with_socket() {
        let dir = Scratch::new("rpc-subgone");
        let mut sub = herdr_subscribe(&socket(&dir), Vec::new());
        assert_eq!(
            sub.events.recv().await,
            Some(SubEvent::End("socket".to_string()))
        );
        assert_eq!(sub.events.recv().await, None);
    }
}
