// Plugin backends: one process per active plugin that has one, started on its first request and talked
// to over stdin/stdout with newline-delimited JSON. Briardr stays in front: devices reach a backend only
// through /api/plugins/<id>/…, after the usual authentication, and the backend never sees cookies.
//
// Briardr → backend: {"type":"request","id":7,"method":"GET","path":"/items","query":{…},
//                    "body":<JSON>|null,"text":<string>?,"base64":<string>?,"stream":{…}|null,"device":{"id","name"}}
// backend → Briardr: {"type":"response","id":7,"status":200,"headers":{…},"body":<JSON>}   (or "text"/"base64")
//                   {"type":"event","topic":"…","data":<JSON>}   pushed to every device's live connection
// A backend exits when its stdin closes (Briardr stopped or turned the plugin off).
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

use super::{backend_hash, Plugin, API_VERSION};
use crate::config::Paths;

/// How long a request may take before Briardr answers 504.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A line longer than this from a backend ends it (a response body is one line).
const MAX_LINE: usize = 16 * 1024 * 1024;
/// Crashes within CRASH_WINDOW that make Briardr wait CRASH_PAUSE before starting the backend again.
const CRASH_LIMIT: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const CRASH_PAUSE: Duration = Duration::from_secs(30);
/// The log is set aside (as <id>.log.1) when it grows past this at a start.
const LOG_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The plugin has no backend.
    None,
    /// It couldn't start; the message says why (command not found, …).
    Start(String),
    /// It crashed repeatedly; Briardr waits before trying again.
    Crashing,
    /// It exited or closed stdout before answering.
    Gone,
    Timeout,
    /// It answered with something that isn't a response.
    Protocol,
}

impl BackendError {
    pub fn status_code_message(&self) -> (u16, &'static str, String) {
        match self {
            BackendError::None => (404, "no_backend", "This plugin has no backend.".into()),
            BackendError::Start(m) => (502, "backend_start", m.clone()),
            BackendError::Crashing => (
                503,
                "backend_crashing",
                "The plugin’s backend keeps stopping. Briardr will try again shortly; its log is in the data folder under logs/plugins.".into(),
            ),
            BackendError::Gone => (502, "backend_gone", "The plugin’s backend stopped before answering.".into()),
            BackendError::Timeout => (504, "backend_timeout", "The plugin’s backend didn’t answer in time.".into()),
            BackendError::Protocol => (502, "backend_protocol", "The plugin’s backend answered with something Briardr doesn’t understand.".into()),
        }
    }
}

/// A plugin event for devices: (plugin ID, topic, data).
pub type Event = (String, String, Value);

struct Running {
    hash: String,
    /// Cleared when the process closes stdout (exited).
    alive: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    /// Kept so dropping it (stop) kills the process.
    _child: Child,
}

pub struct Backends {
    paths: Paths,
    running: Mutex<HashMap<String, Running>>,
    crashes: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    next_id: AtomicU64,
    events: mpsc::UnboundedSender<Event>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Backends {
    pub fn new(paths: Paths) -> (Backends, mpsc::UnboundedReceiver<Event>) {
        let (events, rx) = mpsc::unbounded_channel();
        let b = Backends {
            paths,
            running: Mutex::new(HashMap::new()),
            crashes: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            events,
        };
        (b, rx)
    }

    /// Sends one request to the plugin's backend, starting it if needed, and waits for the response.
    pub async fn request(&self, plugin: &Plugin, mut req: Value) -> Result<Value, BackendError> {
        let spec = plugin
            .manifest
            .as_ref()
            .and_then(|m| m.backend.as_ref())
            .ok_or(BackendError::None)?;
        let dir = plugin.dir.as_ref().ok_or(BackendError::None)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        req["type"] = json!("request");
        req["id"] = json!(id);
        let (done, answer) = oneshot::channel();
        {
            let mut running = lock(&self.running);
            let hash = backend_hash(spec);
            if running
                .get(&plugin.id)
                .is_some_and(|r| r.hash != hash || !r.alive.load(Ordering::SeqCst))
            {
                running.remove(&plugin.id);
            }
            if !running.contains_key(&plugin.id) {
                if self.crashing(&plugin.id) {
                    return Err(BackendError::Crashing);
                }
                let r = self.start(&plugin.id, dir, &spec.command, hash)?;
                running.insert(plugin.id.clone(), r);
            }
            let r = &running[&plugin.id];
            lock(&r.pending).insert(id, done);
            if r.tx.send(req.to_string()).is_err() {
                lock(&r.pending).remove(&id);
                return Err(BackendError::Gone);
            }
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, answer).await {
            Ok(Ok(v)) if v.get("type").and_then(Value::as_str) == Some("response") => Ok(v),
            Ok(Ok(_)) => Err(BackendError::Protocol),
            Ok(Err(_)) => Err(BackendError::Gone),
            Err(_) => {
                if let Some(r) = lock(&self.running).get(&plugin.id) {
                    lock(&r.pending).remove(&id);
                }
                Err(BackendError::Timeout)
            }
        }
    }

    fn crashing(&self, id: &str) -> bool {
        let mut crashes = lock(&self.crashes);
        let list = crashes.entry(id.to_string()).or_default();
        while list
            .front()
            .is_some_and(|t| t.elapsed() > CRASH_WINDOW.max(CRASH_PAUSE))
        {
            list.pop_front();
        }
        list.len() >= CRASH_LIMIT && list.back().is_some_and(|t| t.elapsed() < CRASH_PAUSE)
    }

    fn start(
        &self,
        id: &str,
        dir: &Path,
        command: &[String],
        hash: String,
    ) -> Result<Running, BackendError> {
        let program = resolve_program(dir, &command[0]).ok_or_else(|| {
            BackendError::Start(format!(
                "Couldn’t find “{}” to start the plugin’s backend. Use a full path in briardr-plugin.json or install it where Briardr looks (PATH, ~/.bun/bin, mise shims, Homebrew).",
                command[0]
            ))
        })?;
        let data = self.paths.plugin_data().join(id);
        let _ = std::fs::DirBuilder::new().recursive(true).create(&data);
        let log = open_log(&self.paths.logs().join("plugins"), id);
        let mut cmd = Command::new(&program);
        cmd.args(&command[1..])
            .current_dir(dir)
            .env("BRIARDR_PLUGIN_ID", id)
            .env("BRIARDR_PLUGIN_DATA", &data)
            .env("BRIARDR_API_VERSION", API_VERSION.to_string())
            .env("PATH", search_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log.map_or_else(Stdio::null, Stdio::from))
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            BackendError::Start(format!("Couldn’t start the plugin’s backend: {e}"))
        })?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(BackendError::Start(
                "The backend has no stdin or stdout.".into(),
            ));
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> = Arc::default();
        let reader_pending = pending.clone();
        let events = self.events.clone();
        let crashes = self.crashes.clone();
        let plugin = id.to_string();
        let started = Instant::now();
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match lines.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) if buf.len() > MAX_LINE => break,
                    Ok(_) => {}
                }
                let Ok(msg) = serde_json::from_slice::<Value>(&buf) else {
                    continue; // stray output (a log line on stdout) is ignored
                };
                match msg.get("type").and_then(Value::as_str) {
                    Some("response") => {
                        let id = msg.get("id").and_then(Value::as_u64).unwrap_or(0);
                        if let Some(done) = lock(&reader_pending).remove(&id) {
                            let _ = done.send(msg);
                        }
                    }
                    Some("event") => {
                        let topic = msg
                            .get("topic")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let data = msg.get("data").cloned().unwrap_or(Value::Null);
                        let _ = events.send((plugin.clone(), topic, data));
                    }
                    _ => {}
                }
            }
            // Ended: anyone still waiting hears it now. A quick exit counts as a crash.
            reader_alive.store(false, Ordering::SeqCst);
            lock(&reader_pending).clear();
            if started.elapsed() < CRASH_WINDOW {
                lock(&crashes)
                    .entry(plugin)
                    .or_default()
                    .push_back(Instant::now());
            }
        });
        Ok(Running {
            hash,
            alive,
            tx,
            pending,
            _child: child,
        })
    }

    /// Stops a plugin's backend (turned off, uninstalled).
    pub fn stop(&self, id: &str) {
        lock(&self.running).remove(id);
    }

    /// Stops the backends of plugins that aren't in `active` any more.
    pub fn retain(&self, active: &[String]) {
        lock(&self.running).retain(|id, _| active.contains(id));
    }

    pub fn stop_all(&self) {
        lock(&self.running).clear();
    }

    pub fn running(&self) -> Vec<String> {
        lock(&self.running).keys().cloned().collect()
    }
}

/// Where Briardr looks for a backend's program: PATH, then where Bun, mise and Homebrew install, since
/// a LaunchAgent or systemd unit starts Briardr with a short PATH.
fn search_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    for extra in [
        format!("{home}/.bun/bin"),
        format!("{home}/.local/share/mise/shims"),
        format!("{home}/.local/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    dirs.join(":")
}

/// A program name on the search path, or a path relative to the plugin folder.
fn resolve_program(dir: &Path, program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = dir.join(program);
        return p.is_file().then_some(p);
    }
    search_path()
        .split(':')
        .map(|d| Path::new(d).join(program))
        .find(|p| p.is_file())
}

fn open_log(dir: &Path, id: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let file = dir.join(format!("{id}.log"));
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&file, dir.join(format!("{id}.log.1")));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{BackendSpec, Contributes, Manifest};
    use crate::scratch::Scratch;
    use std::os::unix::fs::PermissionsExt;

    fn plugin(dir: &Path, script: &str) -> Plugin {
        let file = dir.join("backend.sh");
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        Plugin {
            id: "t".into(),
            builtin: false,
            dir: Some(dir.to_path_buf()),
            manifest: Some(Manifest {
                id: "t".into(),
                name: "T".into(),
                version: String::new(),
                description: String::new(),
                briardr: 1,
                client: None,
                backend: Some(BackendSpec {
                    command: vec!["./backend.sh".into()],
                }),
                contributes: Contributes::default(),
            }),
            build: None,
            problem: None,
        }
    }

    // Answers every request with its id and path; an "event" path also pushes an event first.
    const ECHO: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  path=$(printf '%s' "$line" | sed -E 's/.*"path":"([^"]*)".*/\1/')
  if [ "$path" = "/event" ]; then printf '{"type":"event","topic":"ping","data":%s}\n' "$id"; fi
  printf 'not json\n{"type":"response","id":%s,"status":200,"body":{"path":"%s","pid":%s}}\n' "$id" "$path" "$$"
done
"#;

    #[tokio::test]
    async fn round_trips_requests_and_events() {
        let dir = Scratch::new("backend-echo");
        let (b, mut events) = Backends::new(Paths::new(dir.join("data")));
        let p = plugin(&dir, ECHO);
        let r = b
            .request(&p, json!({"method": "GET", "path": "/items"}))
            .await
            .unwrap();
        assert_eq!(r["status"], 200);
        assert_eq!(r["body"]["path"], "/items");
        let pid = r["body"]["pid"].clone();
        let r = b.request(&p, json!({"path": "/event"})).await.unwrap();
        assert_eq!(r["body"]["pid"], pid, "one process for every request");
        let (plugin_id, topic, _) = events.recv().await.unwrap();
        assert_eq!((plugin_id.as_str(), topic.as_str()), ("t", "ping"));
        b.stop("t");
        let r = b.request(&p, json!({"path": "/again"})).await.unwrap();
        assert_ne!(r["body"]["pid"], pid, "started again after a stop");
        b.stop_all();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_backend_that_exits_fails_the_request_and_repeated_crashes_pause_it() {
        let dir = Scratch::new("backend-crash");
        let (b, _events) = Backends::new(Paths::new(dir.join("data")));
        let p = plugin(&dir, "#!/bin/sh\necho oops >&2\nexit 1\n");
        for _ in 0..CRASH_LIMIT {
            assert_eq!(
                b.request(&p, json!({"path": "/"})).await,
                Err(BackendError::Gone)
            );
            // The reader notices the exit just after the request fails.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            b.request(&p, json!({"path": "/"})).await,
            Err(BackendError::Crashing)
        );
        let log = std::fs::read_to_string(dir.join("data/logs/plugins/t.log")).unwrap();
        assert!(log.contains("oops"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_missing_program_says_so() {
        let dir = Scratch::new("backend-missing");
        let (b, _events) = Backends::new(Paths::new(dir.join("data")));
        let mut p = plugin(&dir, "");
        p.manifest.as_mut().unwrap().backend = Some(BackendSpec {
            command: vec!["no-such-program-briardr".into()],
        });
        let err = b.request(&p, json!({})).await.unwrap_err();
        assert!(matches!(err, BackendError::Start(m) if m.contains("no-such-program-briardr")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
