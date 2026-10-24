// One pane's terminal session, backed by
// `herdr terminal session observe|control`. Observe is read-only and never resizes the pane.
// Control resizes the pane to the controller's size while held; release hands it back to the
// Mac's own layout size. Events arrive on a channel.
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, watch};

use super::rpc::{scrubbed_env, session_args, HerdrTarget};

const RELEASE_GRACE: Duration = Duration::from_millis(1500);
const KILL_AFTER_CLOSE: Duration = Duration::from_millis(200);
const READER_DRAIN: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    Observe,
    Control,
}

impl StreamMode {
    pub fn as_str(self) -> &'static str {
        match self {
            StreamMode::Observe => "observe",
            StreamMode::Control => "control",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCategory {
    Busy,
    TakenOver,
    Released,
    PaneGone,
    Exited,
    Error,
}

impl CloseCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            CloseCategory::Busy => "busy",
            CloseCategory::TakenOver => "taken_over",
            CloseCategory::Released => "released",
            CloseCategory::PaneGone => "pane_gone",
            CloseCategory::Exited => "exited",
            CloseCategory::Error => "error",
        }
    }
}

pub fn categorize_close(reason: &str) -> CloseCategory {
    let r = reason.to_lowercase();
    if r.contains("already has an attached client") {
        CloseCategory::Busy
    } else if r.contains("taken over") {
        CloseCategory::TakenOver
    } else if r == "detached" || r.contains("released") {
        CloseCategory::Released
    } else if r.contains("not found")
        || r.contains("no such")
        || r.contains("unknown pane")
        || r.contains("closed")
    {
        CloseCategory::PaneGone
    } else if r.contains("exit") {
        CloseCategory::Exited
    } else {
        CloseCategory::Error
    }
}

fn classify_stderr(text: &str) -> &'static str {
    if text.contains("invalid json command") {
        "invalid_command"
    } else if text.contains("input ignored") {
        "input_ignored"
    } else {
        "stderr"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub width: u16,
    pub height: u16,
    pub full: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Frame(Frame),
    Closed(String),
}

/// One JSON line from `terminal session` stdout. Unknown types, bad JSON and bad base64 are
/// dropped. Base64 is decoded leniently, so a malformed frame is dropped here
/// rather than shown partly decoded.
pub fn parse_line(line: &str) -> Option<Line> {
    let msg: Value = serde_json::from_str(line).ok()?;
    match msg.get("type")?.as_str()? {
        "terminal.frame" => {
            let encoded = msg.get("bytes").and_then(Value::as_str).unwrap_or("");
            Some(Line::Frame(Frame {
                seq: msg.get("seq").and_then(Value::as_u64).unwrap_or(0),
                width: dim(msg.get("width")),
                height: dim(msg.get("height")),
                full: truthy(msg.get("full")),
                bytes: STANDARD.decode(encoded).ok()?,
            }))
        }
        "terminal.closed" => Some(Line::Closed(text(msg.get("reason")))),
        _ => None,
    }
}

fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn dim(v: Option<&Value>) -> u16 {
    v.and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u64::from(u16::MAX)) as u16
}

pub fn clamp_dim(n: i64, min: i64, max: i64) -> u16 {
    n.clamp(min, max) as u16
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Down,
    Up,
    Drag,
    Move,
}

impl MouseAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MouseAction::Down => "down",
            MouseAction::Up => "up",
            MouseAction::Drag => "drag",
            MouseAction::Move => "move",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub fn as_str(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        }
    }
}

/// Arguments after the session flags. A size is only passed when both dimensions are non-zero,
/// and `--takeover` only applies to control.
pub fn terminal_args(
    mode: StreamMode,
    pane_id: &str,
    size: Option<(i64, i64)>,
    takeover: bool,
) -> Vec<String> {
    let mut args = vec![
        "terminal".to_string(),
        "session".into(),
        mode.as_str().into(),
        pane_id.into(),
    ];
    if let Some((cols, rows)) = size.filter(|(c, r)| *c != 0 && *r != 0) {
        args.push("--cols".into());
        args.push(clamp_dim(cols, 2, 500).to_string());
        args.push("--rows".into());
        args.push(clamp_dim(rows, 2, 300).to_string());
    }
    if mode == StreamMode::Control && takeover {
        args.push("--takeover".into());
    }
    args
}

pub fn resize_cmd(cols: i64, rows: i64) -> Value {
    json!({
        "type": "terminal.resize",
        "cols": clamp_dim(cols, 2, 500),
        "rows": clamp_dim(rows, 2, 300),
    })
}

pub fn scroll_cmd(direction: Direction, column: i64, row: i64) -> Value {
    json!({
        "type": "terminal.scroll",
        "direction": direction.as_str(),
        "lines": 1,
        "column": column.max(0),
        "row": row.max(0),
    })
}

pub fn mouse_cmd(
    action: MouseAction,
    button: MouseButton,
    column: i64,
    row: i64,
    modifiers: i64,
) -> Value {
    json!({
        "type": "terminal.mouse",
        "action": action.as_str(),
        "button": button.as_str(),
        "column": column.max(0),
        "row": row.max(0),
        "modifiers": modifiers & 7,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Live,
    Frame(Frame),
    Warning(&'static str),
    Closed {
        category: CloseCategory,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connecting,
    Live,
    Closed,
}

/// State shared by the session handle and its background tasks.
struct Core {
    phase: Mutex<Phase>,
    closed: watch::Sender<bool>,
    events: mpsc::UnboundedSender<SessionEvent>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
}

impl Core {
    fn phase(&self) -> Phase {
        *self.phase.lock().unwrap()
    }

    /// First frame promotes a connecting session to live.
    fn frame(&self, frame: Frame) {
        let went_live = {
            let mut phase = self.phase.lock().unwrap();
            if *phase == Phase::Connecting {
                *phase = Phase::Live;
                true
            } else {
                false
            }
        };
        if went_live {
            let _ = self.events.send(SessionEvent::Live);
        }
        let _ = self.events.send(SessionEvent::Frame(frame));
    }

    /// Emits `Closed` once. Later calls (another close, the process exiting) do nothing.
    fn close(&self, category: CloseCategory, reason: impl Into<String>) {
        {
            let mut phase = self.phase.lock().unwrap();
            if *phase == Phase::Closed {
                return;
            }
            *phase = Phase::Closed;
        }
        self.closed.send_replace(true);
        let _ = self.events.send(SessionEvent::Closed {
            category,
            reason: reason.into(),
        });
    }

    fn request_kill(&self) {
        if let Some(tx) = self.kill.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}

fn request_kill_after(core: &Arc<Core>, delay: Duration) {
    let core = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        core.request_kill();
    });
}

pub struct TerminalSession {
    pub pane_id: String,
    pub mode: StreamMode,
    core: Arc<Core>,
    commands: mpsc::UnboundedSender<String>,
    closed_rx: watch::Receiver<bool>,
}

impl TerminalSession {
    /// Spawns `herdr terminal session <mode> <pane>` and returns the handle plus its event
    /// stream. A failed spawn is reported as a `Closed` event, not an error.
    pub fn start(
        target: &HerdrTarget,
        pane_id: &str,
        mode: StreamMode,
        size: Option<(i64, i64)>,
        takeover: bool,
    ) -> (Self, mpsc::UnboundedReceiver<SessionEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        let (closed, closed_rx) = watch::channel(false);
        let (kill_tx, kill_rx) = oneshot::channel();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let core = Arc::new(Core {
            phase: Mutex::new(Phase::Connecting),
            closed,
            events,
            kill: Mutex::new(Some(kill_tx)),
        });
        let mut cmd = Command::new(&target.herdr_path);
        cmd.args(session_args(target))
            .args(terminal_args(mode, pane_id, size, takeover))
            .env_clear()
            .envs(scrubbed_env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        match cmd.spawn() {
            Ok(child) => drive(child, core.clone(), command_rx, kill_rx),
            Err(_) => core.close(CloseCategory::Error, "spawn failed"),
        }
        let session = TerminalSession {
            pane_id: pane_id.to_string(),
            mode,
            core,
            commands,
            closed_rx,
        };
        (session, rx)
    }

    /// Waiting for the first frame.
    pub fn is_connecting(&self) -> bool {
        self.core.phase() == Phase::Connecting
    }

    /// Streaming frames (state is `live`).
    pub fn is_live(&self) -> bool {
        self.core.phase() == Phase::Live
    }

    fn write(&self, cmd: Value) -> bool {
        if self.mode != StreamMode::Control || self.core.phase() != Phase::Live {
            return false;
        }
        self.commands.send(format!("{cmd}\n")).is_ok()
    }

    pub fn resize(&self, cols: i64, rows: i64) -> bool {
        self.write(resize_cmd(cols, rows))
    }

    pub fn scroll(&self, direction: Direction, column: i64, row: i64) -> bool {
        self.write(scroll_cmd(direction, column, row))
    }

    pub fn mouse(
        &self,
        action: MouseAction,
        button: MouseButton,
        column: i64,
        row: i64,
        modifiers: i64,
    ) -> bool {
        self.write(mouse_cmd(action, button, column, row, modifiers))
    }

    /// Hands control back. A held control session asks Herdr to release and waits up to the
    /// grace period for the close. Otherwise, or after the grace period, the process is killed.
    pub async fn release(&self) {
        let phase = self.core.phase();
        if phase == Phase::Closed {
            return;
        }
        if self.mode == StreamMode::Control && phase == Phase::Live {
            let _ = self
                .commands
                .send(format!("{}\n", json!({ "type": "terminal.release" })));
            let mut closed = self.closed_rx.clone();
            if tokio::time::timeout(RELEASE_GRACE, closed.wait_for(|c| *c))
                .await
                .is_err()
            {
                self.kill();
            }
            return;
        }
        self.kill();
    }

    pub fn kill(&self) {
        self.core.request_kill();
        self.core
            .close(CloseCategory::Released, "closed by briardr");
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.core.request_kill();
    }
}

/// Owns the child: writes commands, reads stdout and stderr, and reaps the exit.
fn drive(
    mut child: Child,
    core: Arc<Core>,
    commands: mpsc::UnboundedReceiver<String>,
    kill: oneshot::Receiver<()>,
) {
    if let Some(stdin) = child.stdin.take() {
        tokio::spawn(write_commands(stdin, commands));
    }
    if let Some(stderr) = child.stderr.take() {
        let events = core.events.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            while next_line(&mut reader, &mut buf).await {
                let text = String::from_utf8_lossy(&buf);
                let _ = events.send(SessionEvent::Warning(classify_stderr(&text)));
            }
        });
    }
    let reader = child.stdout.take().map(|stdout| {
        let core = core.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            while next_line(&mut reader, &mut buf).await {
                let text = String::from_utf8_lossy(&buf);
                match parse_line(text.trim_end()) {
                    Some(Line::Frame(frame)) => core.frame(frame),
                    Some(Line::Closed(reason)) => {
                        core.close(categorize_close(&reason), reason);
                        request_kill_after(&core, KILL_AFTER_CLOSE);
                    }
                    None => {}
                }
            }
        })
    });
    tokio::spawn(async move {
        let mut kill = kill;
        let killed = tokio::select! {
            _ = child.wait() => false,
            _ = &mut kill => true,
        };
        if killed {
            let _ = child.start_kill();
        }
        let _ = child.wait().await;
        if let Some(reader) = reader {
            let _ = tokio::time::timeout(READER_DRAIN, reader).await;
        }
        core.close(CloseCategory::Exited, "exited");
    });
}

async fn write_commands(mut stdin: ChildStdin, mut commands: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = commands.recv().await {
        if stdin.write_all(line.as_bytes()).await.is_err() {
            break;
        }
    }
}

/// Reads one newline-terminated line as raw bytes. Invalid UTF-8 is kept for lossy decoding,
/// so a bad byte cannot stop the reader and leave the pipe to fill.
async fn next_line<R: AsyncRead + Unpin>(reader: &mut BufReader<R>, buf: &mut Vec<u8>) -> bool {
    buf.clear();
    matches!(reader.read_until(b'\n', buf).await, Ok(n) if n > 0)
}

#[cfg(test)]
mod tests {
    use crate::scratch::Scratch;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    const FRAME: &str = r#"printf '%s\n' '{"type":"terminal.frame","seq":1,"width":80,"height":24,"full":true,"bytes":"aGk="}'"#;

    /// Writes an executable `sh` script, a fake herdr, in `dir` and returns its path.
    fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn closed_cmd(reason: &str) -> String {
        format!("printf '%s\\n' '{{\"type\":\"terminal.closed\",\"reason\":\"{reason}\"}}'")
    }

    fn target(path: &str) -> HerdrTarget {
        HerdrTarget {
            herdr_path: path.to_string(),
            session: String::new(),
            timeout: Duration::from_secs(5),
        }
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<SessionEvent>) -> SessionEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timed out")
            .expect("event channel closed")
    }

    async fn assert_no_more_closed(rx: &mut mpsc::UnboundedReceiver<SessionEvent>) {
        tokio::time::sleep(Duration::from_millis(500)).await;
        while let Ok(ev) = rx.try_recv() {
            assert!(
                !matches!(ev, SessionEvent::Closed { .. }),
                "second close: {ev:?}"
            );
        }
    }

    async fn wait_for_file(path: &std::path::Path) -> String {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(path) {
                    if text.contains('\n') {
                        return text;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("file never written")
    }

    fn frame_event() -> SessionEvent {
        SessionEvent::Frame(Frame {
            seq: 1,
            width: 80,
            height: 24,
            full: true,
            bytes: b"hi".to_vec(),
        })
    }

    #[test]
    fn herdr_stream_categorize_close_matches_ts() {
        let cases = [
            (
                "Session already has an attached client",
                CloseCategory::Busy,
            ),
            ("pane was taken over", CloseCategory::TakenOver),
            ("detached", CloseCategory::Released),
            ("client released", CloseCategory::Released),
            ("pane not found", CloseCategory::PaneGone),
            ("unknown pane 3", CloseCategory::PaneGone),
            ("stream closed", CloseCategory::PaneGone),
            ("process exited with status 1", CloseCategory::Exited),
            ("socket hiccup", CloseCategory::Error),
            ("", CloseCategory::Error),
        ];
        for (reason, want) in cases {
            assert_eq!(categorize_close(reason), want, "reason {reason:?}");
        }
    }

    #[test]
    fn herdr_stream_clamp_dims_bounds() {
        assert_eq!(clamp_dim(1, 2, 500), 2);
        assert_eq!(clamp_dim(5000, 2, 500), 500);
        assert_eq!(clamp_dim(9999, 2, 300), 300);
        assert_eq!(clamp_dim(80, 2, 500), 80);
    }

    #[test]
    fn herdr_stream_terminal_args_shape() {
        let observe = terminal_args(StreamMode::Observe, "p1", None, false);
        assert_eq!(observe, ["terminal", "session", "observe", "p1"]);

        let observe_sized = terminal_args(StreamMode::Observe, "p1", Some((80, 24)), true);
        assert_eq!(
            observe_sized,
            ["terminal", "session", "observe", "p1", "--cols", "80", "--rows", "24"]
        );

        let control_zero = terminal_args(StreamMode::Control, "p1", Some((0, 24)), false);
        assert_eq!(control_zero, ["terminal", "session", "control", "p1"]);

        let control = terminal_args(StreamMode::Control, "p1", Some((1, 999)), true);
        assert_eq!(
            control,
            [
                "terminal",
                "session",
                "control",
                "p1",
                "--cols",
                "2",
                "--rows",
                "300",
                "--takeover"
            ]
        );
    }

    #[test]
    fn herdr_stream_command_wire_shapes() {
        assert_eq!(
            resize_cmd(1, 999),
            json!({"type": "terminal.resize", "cols": 2, "rows": 300})
        );
        assert_eq!(
            scroll_cmd(Direction::Up, -3, 5),
            json!({"type": "terminal.scroll", "direction": "up", "lines": 1, "column": 0, "row": 5})
        );
        assert_eq!(
            mouse_cmd(MouseAction::Drag, MouseButton::Right, 4, -1, 15),
            json!({"type": "terminal.mouse", "action": "drag", "button": "right", "column": 4, "row": 0, "modifiers": 7})
        );
    }

    #[test]
    fn herdr_stream_parse_line_frame_and_closed() {
        let frame = parse_line(
            r#"{"type":"terminal.frame","seq":7,"width":120,"height":40,"bytes":"aGk="}"#,
        );
        assert_eq!(
            frame,
            Some(Line::Frame(Frame {
                seq: 7,
                width: 120,
                height: 40,
                full: false,
                bytes: b"hi".to_vec(),
            }))
        );
        assert_eq!(
            parse_line(r#"{"type":"terminal.closed","reason":"pane not found"}"#),
            Some(Line::Closed("pane not found".to_string()))
        );
        assert_eq!(parse_line(r#"{"type":"other"}"#), None);
        assert_eq!(parse_line("not json"), None);
        assert_eq!(
            parse_line(r#"{"type":"terminal.frame","bytes":"!!!"}"#),
            None
        );
    }

    #[tokio::test]
    async fn herdr_stream_observe_frames_then_one_close() {
        let dir = Scratch::new("stream-observe");
        let bin = script(
            &dir,
            "herdr",
            &format!(
                "{FRAME}\n{}\nsleep 30",
                closed_cmd("taken over by another client")
            ),
        );
        let (session, mut rx) =
            TerminalSession::start(&target(&bin), "p1", StreamMode::Observe, None, false);
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        assert_eq!(next(&mut rx).await, frame_event());
        assert_eq!(
            next(&mut rx).await,
            SessionEvent::Closed {
                category: CloseCategory::TakenOver,
                reason: "taken over by another client".to_string(),
            }
        );
        assert!(!session.resize(80, 24), "observe must never resize");
        assert_no_more_closed(&mut rx).await;
    }

    #[tokio::test]
    async fn herdr_stream_control_writes_after_live() {
        let dir = Scratch::new("stream-control");
        let out = dir.join("out.txt");
        let bin = script(
            &dir,
            "herdr",
            &format!(
                "{FRAME}\nread line\nprintf '%s\\n' \"$line\" > '{}'\nsleep 30",
                out.display()
            ),
        );
        let (session, mut rx) =
            TerminalSession::start(&target(&bin), "p1", StreamMode::Control, None, false);
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        assert_eq!(next(&mut rx).await, frame_event());
        assert!(session.resize(100, 30));
        let written = wait_for_file(&out).await;
        assert_eq!(
            serde_json::from_str::<Value>(written.trim()).unwrap(),
            json!({"type": "terminal.resize", "cols": 100, "rows": 30})
        );
    }

    #[tokio::test]
    async fn herdr_stream_observe_release_kills_and_reports_released() {
        let dir = Scratch::new("stream-release-observe");
        let bin = script(&dir, "herdr", &format!("{FRAME}\nsleep 30"));
        let (session, mut rx) =
            TerminalSession::start(&target(&bin), "p1", StreamMode::Observe, None, false);
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        assert_eq!(next(&mut rx).await, frame_event());
        tokio::time::timeout(Duration::from_secs(2), session.release())
            .await
            .expect("observe release must not wait");
        assert_eq!(
            next(&mut rx).await,
            SessionEvent::Closed {
                category: CloseCategory::Released,
                reason: "closed by briardr".to_string(),
            }
        );
        assert_no_more_closed(&mut rx).await;
    }

    #[tokio::test]
    async fn herdr_stream_control_release_waits_for_herdr_close() {
        let dir = Scratch::new("stream-release-control");
        let bin = script(
            &dir,
            "herdr",
            &format!("{FRAME}\nread line\n{}\nsleep 30", closed_cmd("released")),
        );
        let (session, mut rx) =
            TerminalSession::start(&target(&bin), "p1", StreamMode::Control, None, false);
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        assert_eq!(next(&mut rx).await, frame_event());
        tokio::time::timeout(Duration::from_secs(3), session.release())
            .await
            .expect("control release hung");
        assert_eq!(
            next(&mut rx).await,
            SessionEvent::Closed {
                category: CloseCategory::Released,
                reason: "released".to_string(),
            }
        );
        assert_no_more_closed(&mut rx).await;
    }

    #[tokio::test]
    async fn herdr_stream_exit_without_close_line_reports_exited() {
        let dir = Scratch::new("stream-exit");
        let bin = script(&dir, "herdr", FRAME);
        let (_session, mut rx) =
            TerminalSession::start(&target(&bin), "p1", StreamMode::Observe, None, false);
        assert_eq!(next(&mut rx).await, SessionEvent::Live);
        assert_eq!(next(&mut rx).await, frame_event());
        assert_eq!(
            next(&mut rx).await,
            SessionEvent::Closed {
                category: CloseCategory::Exited,
                reason: "exited".to_string(),
            }
        );
        assert_no_more_closed(&mut rx).await;
    }

    #[tokio::test]
    async fn herdr_stream_stderr_warning_is_classified() {
        let dir = Scratch::new("stream-stderr");
        let bin = script(
            &dir,
            "herdr",
            &format!("echo 'input ignored: busy' >&2\n{FRAME}\nsleep 30"),
        );
        let (session, mut rx) =
            TerminalSession::start(&target(&bin), "p1", StreamMode::Observe, None, false);
        let mut warned = false;
        for _ in 0..4 {
            if next(&mut rx).await == SessionEvent::Warning("input_ignored") {
                warned = true;
                break;
            }
        }
        assert!(warned, "no input_ignored warning");
        session.kill();
    }

    #[tokio::test]
    async fn herdr_stream_missing_binary_reports_spawn_failure() {
        let (_session, mut rx) = TerminalSession::start(
            &target("/nonexistent/herdr-kstream"),
            "p1",
            StreamMode::Observe,
            None,
            false,
        );
        assert_eq!(
            next(&mut rx).await,
            SessionEvent::Closed {
                category: CloseCategory::Error,
                reason: "spawn failed".to_string(),
            }
        );
    }
}
