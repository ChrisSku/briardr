// HTTP(S) application: security headers, request guards, REST API,
// WebSockets, static files.
// Guards, in order, for every request:
//   1. The peer must be loopback, on the tailnet or an allowed network (also enforced by binding).
//      In proxy mode it must be a trusted proxy (Caddy on loopback); the client's own address then
//      comes from X-Forwarded-For and is used only for rate limits.
//   2. The Host header must be one of Briardr's own names (defeats DNS rebinding).
//   3. /api and /ws require a signed-in device (except pairing and the session probe). Requests
//      without one are rate-limited per client and overall.
//   4. State-changing requests and WebSockets must come from Briardr's own origin (in proxy mode,
//      exactly the configured public origin).
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::FromRequestParts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::actions::{from_herdr, ActionError, Actions};
use crate::auth::{Auth, Device, RateLimiter};
use crate::config::{preview_origins, save_config, BriardrConfig, Paths};
use crate::diag::Diagnostics;
use crate::herdr::rpc::HerdrError;
use crate::herdr::state::{HerdrEvent, HerdrReader, HerdrState};
use crate::net::{forwarded_client, is_allowed_peer, is_trusted_proxy};
use crate::notes::Notes;
use crate::platform::{HostCapabilities, HostPlatform};
use crate::power::{MacSummary, Power, PowerEvent};
use crate::preview::Previews;
use crate::push::Push;
use crate::static_files::static_file;
use crate::terminal_hub::{TerminalHub, PROTOCOL};
use crate::usage::Usage;

mod routes_misc;
mod routes_notes;
mod routes_plugins;
mod routes_session;
mod routes_streams;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Bun's maxPayloadLength for WebSocket messages.
pub const MAX_WS_MESSAGE: usize = 512 * 1024;
/// Default limit for JSON request bodies.
pub const JSON_LIMIT: usize = 1024 * 1024;

pub type Resp = Response<Body>;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// "the Mac" or "the server", as messages name the host.
pub fn the_host(caps: &HostCapabilities) -> String {
    if caps.platform == HostPlatform::Macos {
        "the Mac".to_string()
    } else {
        "the server".to_string()
    }
}

/// The capabilities object the client sees (`host` in /api/session, /api/mac, settings, live `mac`).
pub fn caps_json(caps: &HostCapabilities) -> Value {
    let platform = match caps.platform {
        HostPlatform::Macos => "macos",
        HostPlatform::Linux => "linux",
        HostPlatform::Other => "other",
    };
    json!({
        "platform": platform,
        "noun": caps.noun,
        "battery": caps.battery,
        "keepAwake": caps.keep_awake,
        "sleep": caps.sleep,
        "dictation": caps.dictation,
        "claudeKeychain": caps.claude_keychain,
    })
}

// ---------- errors ----------

/// What a route handler can fail with. `Action` and `Herdr` become their JSON errors; anything else
/// is the generic 500 (after an `api_error` diagnostics event).
#[derive(Debug)]
pub enum ApiError {
    Action(ActionError),
    Herdr(HerdrError),
    Internal,
}

impl From<ActionError> for ApiError {
    fn from(e: ActionError) -> Self {
        ApiError::Action(e)
    }
}

impl From<HerdrError> for ApiError {
    fn from(e: HerdrError) -> Self {
        ApiError::Herdr(e)
    }
}

pub fn action_err(status: u16, code: &str, message: impl Into<String>) -> ApiError {
    ApiError::Action(ActionError {
        status,
        code: code.to_string(),
        message: message.into(),
    })
}

pub fn stream_gone() -> ApiError {
    action_err(404, "stream_gone", "That stream no longer exists.")
}

// ---------- responses ----------

fn header_map(extra: &[(&str, String)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in extra {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            h.insert(name, value);
        }
    }
    h
}

/// A response with the given body and headers (later entries replace earlier ones, like a spread).
pub fn response(status: u16, body: impl Into<Body>, headers: &[(&str, String)]) -> Resp {
    let mut res = Response::new(body.into());
    *res.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    *res.headers_mut() = header_map(headers);
    res
}

/// `this.send`: JSON with `no-store`, plus extra headers.
pub fn send(status: u16, body: &Value, extra: &[(&str, String)]) -> Resp {
    let mut headers = vec![
        (
            "content-type",
            "application/json; charset=utf-8".to_string(),
        ),
        ("cache-control", "no-store".to_string()),
    ];
    headers.extend(extra.iter().cloned());
    response(status, body.to_string(), &headers)
}

pub fn ok(body: Value) -> Result<Resp, ApiError> {
    Ok(send(200, &body, &[]))
}

/// The response Bun's `error` handler gives for an exception outside the route try/catch.
fn server_error(host: &str) -> Resp {
    response(
        500,
        format!("Something went wrong on {host}."),
        &[("content-type", "text/plain; charset=utf-8".to_string())],
    )
}

/// A CSP hash source for a script, `'sha256-<base64>'`, and nothing else.
fn is_script_hash(s: &str) -> bool {
    s.strip_prefix("'sha256-")
        .and_then(|rest| rest.strip_suffix('\''))
        .is_some_and(|b64| {
            b64.len() == 44
                && b64
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        })
}

/// `previews`: the preview origins in proxy mode; None means previews on other ports of this host.
pub fn security_headers(
    host: &str,
    preview_start: u16,
    previews: Option<&[String]>,
) -> Vec<(&'static str, String)> {
    let h = strip_port(host);
    let frames = match previews {
        None => format!("https://{h}:*"),
        Some([]) => "'none'".to_string(),
        Some(list) => list.join(" "),
    };
    let csp = [
        "default-src 'self'".to_string(),
        "script-src 'self'".to_string(),
        "style-src 'self' 'unsafe-inline'".to_string(),
        "img-src 'self' data: blob:".to_string(),
        "media-src 'self' blob:".to_string(),
        "font-src 'self'".to_string(),
        format!("connect-src 'self' wss://{host}"),
        format!("frame-src {frames}"),
        "worker-src 'self'".to_string(),
        "manifest-src 'self'".to_string(),
        "frame-ancestors 'none'".to_string(),
        "base-uri 'none'".to_string(),
        "form-action 'self'".to_string(),
        "object-src 'none'".to_string(),
    ]
    .join("; ");
    vec![
        ("content-security-policy", csp),
        ("x-content-type-options", "nosniff".to_string()),
        ("referrer-policy", "no-referrer".to_string()),
        ("strict-transport-security", "max-age=31536000".to_string()),
        ("cross-origin-opener-policy", "same-origin".to_string()),
        ("cross-origin-resource-policy", "same-origin".to_string()),
        (
            "permissions-policy",
            "microphone=(self), camera=(), geolocation=(), payment=(), usb=()".to_string(),
        ),
        ("x-briardr-preview-start", preview_start.to_string()),
    ]
}

/// `s.replace(/:\d+$/, '')`.
fn strip_port(host: &str) -> &str {
    match host.rfind(':') {
        Some(i) if i + 1 < host.len() && host[i + 1..].bytes().all(|b| b.is_ascii_digit()) => {
            &host[..i]
        }
        _ => host,
    }
}

// ---------- request helpers ----------

/// Read a request body, refusing it as soon as it grows past the limit (413 `too_large`).
pub async fn read_body(headers: &HeaderMap, body: Body, limit: usize) -> Result<Vec<u8>, ApiError> {
    let too_large = || action_err(413, "too_large", "Request too large");
    if js_number(header_str(headers, "content-length").unwrap_or("0")) > limit as f64 {
        return Err(too_large());
    }
    let mut body = body;
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| ApiError::Internal)?;
        if let Ok(data) = frame.into_data() {
            if out.len() + data.len() > limit {
                return Err(too_large());
            }
            out.extend_from_slice(&data);
        }
    }
    Ok(out)
}

/// `this.body`: JSON body (empty = `{}`), 400 `bad_json` when it doesn't parse.
pub async fn json_body(ctx: &mut Ctx, limit: usize) -> Result<Value, ApiError> {
    let body = std::mem::replace(&mut ctx.body, Body::empty());
    let buf = read_body(&ctx.headers, body, limit).await?;
    if buf.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&buf).map_err(|_| action_err(400, "bad_json", "Invalid JSON"))
}

/// The raw body, for uploads and dictation.
pub async fn raw_body(ctx: &mut Ctx, limit: usize) -> Result<Vec<u8>, ApiError> {
    let body = std::mem::replace(&mut ctx.body, Body::empty());
    read_body(&ctx.headers, body, limit).await
}

/// The upload body with the stream checks: a declared size over the kind's limit is
/// refused before reading (413), growing past it is 413, a broken or short body is 499 (cancelled).
pub async fn upload_body(
    ctx: &mut Ctx,
    kind: crate::uploads::UploadKind,
) -> Result<Vec<u8>, crate::uploads::UploadError> {
    use crate::uploads::UploadError;
    let max = UploadError::max_bytes(kind);
    let declared = js_number(header_str(&ctx.headers, "content-length").unwrap_or("0"));
    if declared > max as f64 {
        return Err(UploadError::DeclaredTooLarge(kind));
    }
    let mut body = std::mem::replace(&mut ctx.body, Body::empty());
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        // Bun reports a dropped client as an aborted request; hyper as a body error.
        let frame = frame.map_err(|_| UploadError::Cancelled)?;
        if let Ok(data) = frame.into_data() {
            if (out.len() + data.len()) as u64 > max {
                return Err(UploadError::TooLarge);
            }
            out.extend_from_slice(&data);
        }
    }
    if declared > 0.0 && (out.len() as f64) < declared {
        return Err(UploadError::Cancelled);
    }
    Ok(out)
}

pub fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// JS `Number(s)` for header and query values: trimmed decimal, empty = 0, anything else NaN.
pub fn js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).map_or(f64::NAN, |n| n as f64);
    }
    match t {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ if t
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) =>
        {
            t.parse::<f64>().unwrap_or(f64::NAN)
        }
        _ => f64::NAN,
    }
}

/// `decodeURIComponent`; None where it would throw.
pub fn decode_uri_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub(crate) use crate::actions::{js_string, js_truthy};

/// `String(x || '')` for a JSON body field.
pub fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if js_truthy(v) => js_string(v),
        _ => String::new(),
    }
}

/// URL query values as `URLSearchParams.get` returns them (first match, form-decoded).
#[derive(Debug, Clone, Default)]
pub struct Query(Vec<(String, String)>);

impl Query {
    pub fn pairs(&self) -> &[(String, String)] {
        &self.0
    }
    pub fn parse(q: Option<&str>) -> Self {
        Query(
            url::form_urlencoded::parse(q.unwrap_or("").as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        )
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Everything a route handler sees.
pub struct Ctx {
    pub method: Method,
    pub path: String,
    pub query: Query,
    pub headers: HeaderMap,
    pub body: Body,
    pub params: BTreeMap<String, String>,
    pub device: Option<Device>,
    pub peer: Option<String>,
    /// The client's address: the peer, or behind a trusted proxy the address it forwarded.
    pub client: String,
}

impl Ctx {
    pub fn param(&self, key: &str) -> &str {
        self.params.get(key).map_or("", String::as_str)
    }
    /// The signed-in device; only called on non-public routes, where the guard already required one.
    pub fn device(&self) -> Result<&Device, ApiError> {
        self.device.as_ref().ok_or(ApiError::Internal)
    }
}

// ---------- routing ----------

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<Resp, ApiError>> + Send>>;
type Handler = Box<dyn Fn(Arc<App>, Ctx) -> HandlerFuture + Send + Sync>;

struct Route {
    method: Method,
    pattern: String,
    keys: Vec<String>,
    public: bool,
    /// The built-in plugin the route belongs to; refused while that plugin is off.
    plugin: Option<&'static str>,
    h: Handler,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RouteInfo {
    pub method: String,
    /// The route pattern as JS `RegExp.source` writes it.
    pub pattern: String,
    pub public: bool,
}

/// Route table, filled by the `routes_*` modules in registration order (first match wins).
pub struct Routes(Vec<Route>);

impl Routes {
    pub fn add<F, Fut>(&mut self, method: Method, pattern: &str, public: bool, f: F)
    where
        F: Fn(Arc<App>, Ctx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Resp, ApiError>> + Send + 'static,
    {
        let keys = pattern
            .split('/')
            .filter_map(|s| {
                if s == "*" {
                    Some("rest")
                } else {
                    s.strip_prefix(':')
                }
            })
            .map(str::to_string)
            .collect();
        self.0.push(Route {
            method,
            pattern: pattern.to_string(),
            keys,
            public,
            plugin: crate::plugins::route_owner(pattern),
            h: Box::new(move |app, ctx| Box::pin(f(app, ctx))),
        });
    }
}

/// Matches `path` against a route pattern, compiled to a regex: `:key` is `([^/]+)`, every other
/// character is a regex atom (so `.` matches any character), anchored at both ends.
fn match_route(pattern: &str, path: &str) -> Option<Vec<String>> {
    fn go(p: &[u8], s: &[u8], caps: &mut Vec<String>, src: &str, pi: usize, si: usize) -> bool {
        if pi == p.len() {
            return si == s.len();
        }
        // A final `*` takes the rest of the path, at least one character (`/api/plugins/:id/*`).
        if p[pi] == b'*' && pi + 1 == p.len() {
            if si < s.len() {
                caps.push(src[si..].to_string());
                return true;
            }
            return false;
        }
        if p[pi] == b':' {
            let mut end = pi + 1;
            while end < p.len() && p[end].is_ascii_alphabetic() {
                end += 1;
            }
            if end > pi + 1 {
                let mut j = si;
                while j < s.len() && s[j] != b'/' {
                    j += 1;
                }
                // Greedy with backtracking, like the regex engine.
                while j > si {
                    caps.push(src[si..j].to_string());
                    if go(p, s, caps, src, end, j) {
                        return true;
                    }
                    caps.pop();
                    j -= 1;
                }
                return false;
            }
        }
        if si < s.len() && (p[pi] == b'.' || p[pi] == s[si]) {
            return go(p, s, caps, src, pi + 1, si + 1);
        }
        false
    }
    let mut caps = Vec::new();
    // Character-wise matching on bytes is safe: patterns are ASCII and captures end at '/' or a literal.
    go(pattern.as_bytes(), path.as_bytes(), &mut caps, path, 0, 0).then_some(caps)
}

/// `new RegExp('^' + pattern.replace(/:([a-zA-Z]+)/g, '([^/]+)') + '$').source`.
fn regex_source(pattern: &str) -> String {
    let mut out = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek().is_some_and(|n| n.is_ascii_alphabetic()) {
            while chars.peek().is_some_and(|n| n.is_ascii_alphabetic()) {
                chars.next();
            }
            // JS escapes `/` in `source`, but not inside a character class.
            out.push_str("([^/]+)");
        } else if c == '*' && chars.peek().is_none() {
            out.push_str("(.+)");
        } else if c == '/' {
            out.push_str("\\/");
        } else {
            out.push(c);
        }
    }
    out.push('$');
    out
}

// ---------- the app ----------

/// Shared, live configuration: handlers read it, settings routes replace and save it.
pub struct ConfigCell {
    cfg: RwLock<BriardrConfig>,
    paths: Paths,
}

impl ConfigCell {
    pub fn new(cfg: BriardrConfig, paths: Paths) -> Self {
        ConfigCell {
            cfg: RwLock::new(cfg),
            paths,
        }
    }
    pub fn get(&self) -> BriardrConfig {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// Replaces and saves. A failed save still keeps the new config in memory;
    /// the caller reports the error.
    pub fn set(&self, c: BriardrConfig) -> Result<(), ApiError> {
        *self.cfg.write().unwrap_or_else(|e| e.into_inner()) = c.clone();
        save_config(&self.paths, &c).map_err(|_| ApiError::Internal)
    }
    pub fn paths(&self) -> &Paths {
        &self.paths
    }
}

pub struct AppDeps {
    pub cfg: Arc<ConfigCell>,
    pub auth: Arc<Auth>,
    pub herdr: HerdrState,
    pub actions: Arc<Actions>,
    pub hub: Arc<TerminalHub>,
    pub diag: Arc<Diagnostics>,
    pub power: Arc<Mutex<Power>>,
    pub push: Arc<Push>,
    pub usage: Arc<Usage>,
    pub previews: Arc<Previews>,
    pub notes: Arc<Notes>,
    pub web_dir: PathBuf,
    pub allowed_hosts: Arc<RwLock<HashSet<String>>>,
    pub dictation_helper: Arc<RwLock<Option<PathBuf>>>,
    pub ca_profile: Option<String>,
    /// (wanted, effective) TLS mode.
    pub tls_mode: Arc<RwLock<(String, String)>>,
    pub caps: HostCapabilities,
}

struct LiveSocket {
    id: u64,
    device_id: String,
    tx: mpsc::UnboundedSender<Out>,
}

/// What the app sends to a live socket's task.
enum Out {
    Text(String),
    Close(u16, &'static str),
}

pub struct App {
    pub d: AppDeps,
    /// Changes on every server start, so connected clients can refresh after reconnecting.
    instance_id: String,
    /// Which plugins are on (state/plugins.json).
    pub plugins: crate::plugins::StateCell,
    pub backends: crate::plugins::backend::Backends,
    routes: Routes,
    live: Mutex<Vec<LiveSocket>>,
    next_live: AtomicU64,
    model_pending: AtomicBool,
    pair_limiter: Mutex<RateLimiter>,
    /// All pairing attempts together: bounds guessing from many addresses.
    pair_global: Mutex<RateLimiter>,
    /// Requests without a signed-in device (public routes, failed auth, rejected sockets).
    anon_limiter: Mutex<RateLimiter>,
    anon_global: Mutex<RateLimiter>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl App {
    /// Builds the app and starts its background tasks (Herdr events, revocation). Needs a Tokio runtime.
    pub fn new(d: AppDeps) -> Arc<App> {
        let mut routes = Routes(Vec::new());
        routes_session::register(&mut routes);
        routes_streams::register(&mut routes);
        routes_notes::register(&mut routes);
        routes_misc::register(&mut routes);
        routes_plugins::register(&mut routes);
        let (backends, plugin_events) =
            crate::plugins::backend::Backends::new(d.cfg.paths().clone());
        let app = Arc::new(App {
            instance_id: hex::encode(rand::random::<[u8; 16]>()),
            plugins: crate::plugins::StateCell::new(d.cfg.paths().clone()),
            backends,
            d,
            routes,
            live: Mutex::new(Vec::new()),
            next_live: AtomicU64::new(1),
            model_pending: AtomicBool::new(false),
            pair_limiter: Mutex::new(RateLimiter::new(10, 15 * 60 * 1000)),
            pair_global: Mutex::new(RateLimiter::new(100, 15 * 60 * 1000)),
            anon_limiter: Mutex::new(RateLimiter::new(120, 60 * 1000)),
            anon_global: Mutex::new(RateLimiter::new(2000, 60 * 1000)),
        });
        app.spawn_listeners();
        app.spawn_plugin_watch();
        app.spawn_plugin_events(plugin_events);
        app
    }

    /// Events plugin backends push go to every device's live connection.
    fn spawn_plugin_events(
        self: &Arc<Self>,
        mut events: tokio::sync::mpsc::UnboundedReceiver<crate::plugins::backend::Event>,
    ) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some((plugin, topic, data)) = events.recv().await {
                let Some(app) = weak.upgrade() else { return };
                app.broadcast(
                    &json!({"t": "plugin_event", "plugin": plugin, "topic": topic, "data": data}),
                );
            }
        });
    }

    /// Tells devices when plugins change on the host (installed, rebuilt by `briardr plugin dev`, turned
    /// on or off from the command line), so they load the new client without a reload.
    fn spawn_plugin_watch(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let paths = self.d.cfg.paths().clone();
        tokio::spawn(async move {
            let mut last: Option<Vec<(std::path::PathBuf, Option<std::time::SystemTime>)>> = None;
            loop {
                let p = paths.clone();
                let now = tokio::task::spawn_blocking(move || crate::plugins::fingerprint(&p))
                    .await
                    .ok();
                let Some(app) = weak.upgrade() else { return };
                if let (Some(before), Some(after)) = (&last, &now) {
                    // A rebuilt or edited plugin's backend starts again with the new code on its next request.
                    let plugins_dir = paths.plugins();
                    for (file, mtime) in after {
                        let was = before.iter().find(|(f, _)| f == file).map(|(_, m)| m);
                        if was.is_some_and(|m| m != mtime) {
                            if let Ok(rest) = file.strip_prefix(&plugins_dir) {
                                if let Some(id) = rest.components().next() {
                                    app.backends.stop(&id.as_os_str().to_string_lossy());
                                }
                            }
                        }
                    }
                }
                if last.is_some() && now != last {
                    if let Ok(list) = routes_plugins::plugin_list(&app).await {
                        // Backends of plugins turned off (or changed) on the host stop.
                        let active: Vec<String> = list
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|p| p["enabled"] == json!(true))
                            .filter_map(|p| p["id"].as_str().map(str::to_string))
                            .collect();
                        app.backends.retain(&active);
                        app.broadcast(&json!({"t": "plugins", "plugins": list}));
                    }
                }
                drop(app);
                last = now;
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        });
    }

    fn spawn_listeners(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let mut events = self.d.herdr.events();
        tokio::spawn(async move {
            loop {
                let ev = match events.recv().await {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                };
                let Some(app) = weak.upgrade() else { return };
                match ev {
                    HerdrEvent::Model(_) | HerdrEvent::Health(_) => app.broadcast_model(),
                    HerdrEvent::Transition(t) => {
                        let mut msg = serde_json::to_value(&t).unwrap_or_else(|_| json!({}));
                        if let Value::Object(map) = &mut msg {
                            let mut out = serde_json::Map::new();
                            out.insert("t".into(), json!("transition"));
                            out.extend(std::mem::take(map));
                            msg = Value::Object(out);
                        }
                        app.broadcast(&msg);
                    }
                }
            }
        });
        let weak = Arc::downgrade(self);
        let mut revoked = self.d.auth.subscribe_revoked();
        tokio::spawn(async move {
            loop {
                let id = match revoked.recv().await {
                    Ok(id) => id,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                };
                let Some(app) = weak.upgrade() else { return };
                for s in lock(&app.live).iter().filter(|s| s.device_id == id) {
                    let _ = s.tx.send(Out::Close(4401, "revoked"));
                }
                app.d.hub.close_device(&id, 4401);
                app.d.previews.close_device(&id);
                let _ = app.d.push.unsubscribe_device(&id);
            }
        });
    }

    pub fn cfg(&self) -> BriardrConfig {
        self.d.cfg.get()
    }

    pub fn host(&self) -> String {
        the_host(&self.d.caps)
    }

    /// All registered API routes (used by the authentication test).
    pub fn list_routes(&self) -> Vec<RouteInfo> {
        self.routes
            .0
            .iter()
            .map(|r| RouteInfo {
                method: r.method.to_string(),
                pattern: regex_source(&r.pattern),
                public: r.public,
            })
            .collect()
    }

    /// Proxy mode settings, or None when Briardr serves HTTPS itself.
    fn proxy(&self, cfg: &BriardrConfig) -> Option<(String, Vec<String>)> {
        (cfg.tls.mode == "proxy").then(|| {
            (
                cfg.public_origin.clone().unwrap_or_default(),
                cfg.trusted_proxies.clone(),
            )
        })
    }

    pub fn in_proxy_mode(&self) -> bool {
        self.cfg().tls.mode == "proxy"
    }

    pub fn unsupported(&self, what: &str) -> ApiError {
        action_err(
            501,
            "unsupported",
            format!("{what} isn’t available on {}.", self.host()),
        )
    }

    /// Rate limit for requests without a signed-in device.
    fn anon_allowed(&self, client: &str) -> bool {
        lock(&self.anon_limiter).allow(client) && lock(&self.anon_global).allow("all")
    }

    /// Pairing attempts, per client and overall.
    pub fn pair_allowed(&self, client: &str) -> bool {
        lock(&self.pair_limiter).allow(client) && lock(&self.pair_global).allow("all")
    }

    pub fn host_allowed(&self, headers: &HeaderMap) -> bool {
        let host = header_str(headers, "host").unwrap_or("").to_lowercase();
        let name = if let Some(rest) = host.strip_prefix('[') {
            // host.slice(1, host.indexOf(']')); a missing ']' gives slice(1, -1).
            match rest.find(']') {
                Some(i) => rest[..i].to_string(),
                None => {
                    let mut r = rest.to_string();
                    r.pop();
                    r
                }
            }
        } else {
            strip_port(&host).to_string()
        };
        self.d
            .allowed_hosts
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&name)
    }

    pub fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        let origin = header_str(headers, "origin");
        let site = header_str(headers, "sec-fetch-site");
        if site.is_some_and(|s| s != "same-origin" && s != "none") {
            return false;
        }
        let Some(origin) = origin else {
            return site.is_some_and(|s| s == "same-origin" || s == "none");
        };
        match self.proxy(&self.cfg()) {
            Some((public_origin, _)) => origin == public_origin,
            None => origin == format!("https://{}", header_str(headers, "host").unwrap_or("null")),
        }
    }

    /// Whether the connecting peer may talk to Briardr at all.
    pub fn peer_allowed(&self, peer: Option<&str>) -> bool {
        let cfg = self.cfg();
        match self.proxy(&cfg) {
            Some((_, trusted)) => is_trusted_proxy(peer, &trusted),
            None => is_allowed_peer(peer, &cfg.allowed_networks),
        }
    }

    /// The client's address, for rate limits only (never for authorization).
    pub fn client_of(&self, headers: &HeaderMap, peer: Option<&str>) -> String {
        match self.proxy(&self.cfg()) {
            Some((_, trusted)) => {
                forwarded_client(header_str(headers, "x-forwarded-for"), peer, &trusted)
            }
            None => peer.unwrap_or("unknown").to_string(),
        }
    }

    fn authenticate(&self, headers: &HeaderMap) -> Option<Device> {
        self.d.auth.authenticate(header_str(headers, "cookie"))
    }

    /// The server's fetch handler: guards, dispatch, then security headers on whatever comes back.
    pub async fn handle(self: Arc<Self>, req: Request<Body>, peer: Option<String>) -> Resp {
        #[cfg(feature = "test-hooks")]
        if crate::test_hooks::wants(&req, peer.as_deref()) {
            return crate::test_hooks::handle(&self, req).await;
        }
        let host = header_str(req.headers(), "host")
            .unwrap_or("localhost")
            .to_string();
        let mut res = self.clone().dispatch(req, peer).await;
        if res.status() == StatusCode::SWITCHING_PROTOCOLS {
            return res; // an upgraded socket: Bun returns nothing, so no headers are added
        }
        let cfg = self.cfg();
        let previews = (cfg.tls.mode == "proxy").then(|| preview_origins(&cfg).unwrap_or_default());
        for (k, v) in security_headers(&host, cfg.preview_port_start, previews.as_deref()) {
            if !res.headers().contains_key(k) {
                if let Ok(v) = HeaderValue::from_str(&v) {
                    res.headers_mut().insert(k, v);
                }
            }
        }
        res
    }

    async fn dispatch(self: Arc<Self>, req: Request<Body>, peer: Option<String>) -> Resp {
        if !self.peer_allowed(peer.as_deref()) {
            return response(403, Body::empty(), &[]);
        }
        let client = self.client_of(req.headers(), peer.as_deref());
        let upgrade = header_str(req.headers(), "upgrade").map(str::to_lowercase);
        if upgrade.as_deref() == Some("websocket") {
            return self.handle_upgrade(req, peer, client).await;
        }
        if !self.host_allowed(req.headers()) {
            return response(
                421,
                "Unknown host",
                &[("content-type", "text/plain".to_string())],
            );
        }
        let Some(path) = pathname(&req) else {
            return server_error(&self.host());
        };
        if path.starts_with("/api/") {
            return self.handle_api(req, path, peer, client).await;
        }
        if let Some(rest) = path.strip_prefix("/plugins/") {
            return self.serve_plugin_file(req.headers(), rest).await;
        }
        self.serve_static(req, &path).await
    }

    async fn handle_api(
        self: Arc<Self>,
        req: Request<Body>,
        path: String,
        peer: Option<String>,
        client: String,
    ) -> Resp {
        let (parts, body) = req.into_parts();
        let method = parts.method.clone();
        let rate_limited = || {
            send(
                429,
                &json!({"error": "rate_limited", "message": "Too many requests. Wait a minute."}),
                &[("retry-after", "60".to_string())],
            )
        };
        for (idx, r) in self.routes.0.iter().enumerate() {
            if r.method != method {
                continue;
            }
            let Some(caps) = match_route(&r.pattern, &path) else {
                continue;
            };
            let mut params = BTreeMap::new();
            for (k, v) in r.keys.iter().zip(caps) {
                let Some(v) = decode_uri_component(&v) else {
                    return server_error(&self.host());
                };
                params.insert(k.clone(), v);
            }
            let device = self.authenticate(&parts.headers);
            if device.is_none() && !self.anon_allowed(&client) {
                return rate_limited();
            }
            if !r.public && device.is_none() {
                return send(
                    401,
                    &json!({"error": "unauthenticated", "message": "This device isn’t signed in to Briardr."}),
                    &[],
                );
            }
            let site = header_str(&parts.headers, "sec-fetch-site");
            if site.is_some_and(|s| s != "same-origin" && s != "none") {
                return send(403, &json!({"error": "cross_site"}), &[]);
            }
            if method != Method::GET
                && method != Method::HEAD
                && !self.origin_allowed(&parts.headers)
            {
                return send(403, &json!({"error": "bad_origin"}), &[]);
            }
            if let Some(plugin) = self.routes.0[idx].plugin {
                if !self.plugins.get().is_enabled(plugin) {
                    return send(
                        404,
                        &json!({"error": "plugin_disabled", "plugin": plugin, "message": "This plugin is turned off. Turn it on in Settings → Plugins."}),
                        &[],
                    );
                }
            }
            let ctx = Ctx {
                method: method.clone(),
                path: path.clone(),
                query: Query::parse(parts.uri.query()),
                headers: parts.headers.clone(),
                body,
                params,
                device,
                peer: peer.clone(),
                client: client.clone(),
            };
            let fut = (self.routes.0[idx].h)(self.clone(), ctx);
            return match fut.await {
                Ok(res) => res,
                Err(ApiError::Action(e)) => send(
                    e.status,
                    &json!({"error": e.code, "message": e.message}),
                    &[],
                ),
                Err(ApiError::Herdr(e)) => {
                    let e = from_herdr(&e, &self.host());
                    send(
                        e.status,
                        &json!({"error": e.code, "message": e.message}),
                        &[],
                    )
                }
                Err(ApiError::Internal) => {
                    self.event(
                        "api_error",
                        json!({"method": method.as_str().to_lowercase(), "category": "exception"}),
                    );
                    send(
                        500,
                        &json!({"error": "internal", "message": format!("Something went wrong on {}.", self.host())}),
                        &[],
                    )
                }
            };
        }
        if self.authenticate(&parts.headers).is_none() && !self.anon_allowed(&client) {
            return rate_limited();
        }
        send(404, &json!({"error": "not_found"}), &[])
    }

    /// An installed plugin's client build (`/plugins/<id>/<file>`): signed-in devices only, active plugins
    /// only. Built files have hashed names, so they never change; build.json is never served.
    async fn serve_plugin_file(&self, headers: &HeaderMap, rest: &str) -> Resp {
        let not_found = || {
            response(
                404,
                "Not found",
                &[("content-type", "text/plain".to_string())],
            )
        };
        if self.authenticate(headers).is_none() {
            return response(401, Body::empty(), &[]);
        }
        let Some(rest) = decode_uri_component(rest) else {
            return not_found();
        };
        let Some((id, file)) = rest
            .split_once('/')
            .map(|(a, b)| (a.to_string(), b.to_string()))
        else {
            return not_found();
        };
        if file == "build.json" {
            return not_found();
        }
        let paths = self.d.cfg.paths().clone();
        let state = self.plugins.get();
        let found = tokio::task::spawn_blocking(move || {
            crate::plugins::client_file(&paths, &state, &id, &file)
        })
        .await
        .ok()
        .flatten();
        let Some(found) = found else {
            return not_found();
        };
        match tokio::fs::read(&found).await {
            Ok(bytes) => response(
                200,
                bytes,
                &[
                    (
                        "content-type",
                        crate::static_files::mime_for(&found.to_string_lossy()).to_string(),
                    ),
                    (
                        "cache-control",
                        "private, max-age=31536000, immutable".to_string(),
                    ),
                ],
            ),
            Err(_) => not_found(),
        }
    }

    /// The Content-Security-Policy for the page: the usual one, with its inline import map allowed by the
    /// hash the build writes next to it (`index.html.csp`, scripts/build.ts). None without that file.
    async fn page_csp(&self, headers: &HeaderMap, file: &str) -> Option<String> {
        let listed = tokio::fs::read_to_string(format!("{file}.csp"))
            .await
            .ok()?;
        let hashes: Vec<&str> = listed
            .split_whitespace()
            .filter(|h| is_script_hash(h))
            .collect();
        if hashes.is_empty() {
            return None;
        }
        let host = header_str(headers, "host").unwrap_or("localhost");
        let cfg = self.cfg();
        let previews = (cfg.tls.mode == "proxy").then(|| preview_origins(&cfg).unwrap_or_default());
        let (_, csp) = security_headers(host, cfg.preview_port_start, previews.as_deref())
            .into_iter()
            .find(|(k, _)| *k == "content-security-policy")?;
        Some(csp.replacen(
            "script-src 'self'",
            &format!("script-src 'self' {}", hashes.join(" ")),
            1,
        ))
    }

    async fn serve_static(&self, req: Request<Body>, pathname: &str) -> Resp {
        let Some(mut rel) = decode_uri_component(pathname) else {
            return server_error(&self.host());
        };
        if rel == "/" || extname(&rel).is_empty() {
            rel = "/index.html".to_string(); // client-side routes
        }
        let root = lexical_resolve(&self.d.web_dir, "");
        let file = lexical_resolve(&self.d.web_dir, &format!(".{rel}"));
        if !file.starts_with(&format!("{root}/")) {
            return response(404, Body::empty(), &[]);
        }
        // Home-screen installs fetch these outside the page; record whether they arrive and how.
        let icon = icon_kind(&rel);
        let headers = req.headers();
        let meta = tokio::fs::metadata(&file)
            .await
            .ok()
            .filter(|m| m.is_file());
        if let Some(kind) = icon {
            let mode = header_str(headers, "sec-fetch-dest")
                .unwrap_or("none")
                .to_lowercase();
            self.event(
                "icon_fetch",
                json!({
                    "kind": kind,
                    "status": if meta.is_some() { 200 } else { 404 },
                    "mode": mode,
                    "signed_in": self.authenticate(headers).is_some(),
                }),
            );
        }
        let Some(meta) = meta else {
            return response(
                404,
                "Not found",
                &[("content-type", "text/plain".to_string())],
            );
        };
        let mut extra: Vec<(String, String)> = Vec::new();
        if rel == "/index.html" {
            if let Some(csp) = self.page_csp(headers, &file).await {
                extra.push(("content-security-policy".into(), csp));
            }
        }
        if rel == "/sw.js" {
            extra.push(("service-worker-allowed".into(), "/".into()));
        }
        if icon.is_some() {
            // iOS fetches home-screen icons outside the page's origin; same-origin would make it discard them.
            extra.push(("cross-origin-resource-policy".into(), "cross-origin".into()));
        }
        let accept = header_str(headers, "accept-encoding").map(str::to_string);
        let file_path = PathBuf::from(&file);
        let sf = match tokio::task::spawn_blocking(move || {
            static_file(&file_path, &meta, &rel, accept.as_deref(), &extra)
        })
        .await
        {
            Ok(sf) => sf,
            Err(_) => return server_error(&self.host()),
        };
        match tokio::fs::read(&sf.body).await {
            Ok(bytes) => {
                let hdrs: Vec<(&str, String)> = sf
                    .headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect();
                response(200, bytes, &hdrs)
            }
            Err(_) => response(
                404,
                "Not found",
                &[("content-type", "text/plain".to_string())],
            ),
        }
    }

    // ---------- WebSockets ----------

    async fn handle_upgrade(
        self: Arc<Self>,
        req: Request<Body>,
        peer: Option<String>,
        client: String,
    ) -> Resp {
        let reject = |status: u16| {
            response(
                status,
                Body::empty(),
                &[("connection", "close".to_string())],
            )
        };
        if !self.peer_allowed(peer.as_deref()) || !self.host_allowed(req.headers()) {
            return reject(403);
        }
        let device = self.authenticate(req.headers());
        if device.is_none() && !self.anon_allowed(&client) {
            return reject(429);
        }
        if !self.origin_allowed(req.headers()) {
            return reject(403);
        }
        let Some(device) = device else {
            return reject(401);
        };
        let Some(path) = pathname(&req) else {
            return reject(403);
        };
        let query = Query::parse(req.uri().query());
        let (mut parts, _) = req.into_parts();
        let Ok(ws) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
            return reject(400);
        };
        let ws = ws.max_message_size(MAX_WS_MESSAGE);
        let app = self.clone();
        match path.as_str() {
            "/ws/live" => ws.on_upgrade(move |socket| app.on_live(socket, device, query)),
            "/ws/term" => ws.on_upgrade(move |socket| async move {
                app.d.hub.attach(socket, device.id, query).await;
            }),
            _ => reject(403),
        }
    }

    async fn on_live(self: Arc<Self>, mut ws: WebSocket, device: Device, q: Query) {
        if js_number(q.get("v").unwrap_or("null")) != f64::from(PROTOCOL) {
            let _ = ws
                .send(Message::Close(Some(CloseFrame {
                    code: 4400,
                    reason: "protocol".into(),
                })))
                .await;
            return;
        }
        let id = self.next_live.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = mpsc::unbounded_channel();
        lock(&self.live).push(LiveSocket {
            id,
            device_id: device.id.clone(),
            tx: tx.clone(),
        });
        let anon = self.d.diag.anon(&device.id);
        self.event("live_open", json!({ "device": anon }));
        let _ = tx.send(Out::Text(
            json!({"t": "hello", "instanceId": self.instance_id}).to_string(),
        ));
        let _ = tx.send(Out::Text(self.model_message().to_string()));
        let summary = lock(&self.d.power).summary().cloned();
        if let Some(mac) = summary {
            let _ = tx.send(Out::Text(
                json!({"t": "mac", "mac": mac, "host": caps_json(&self.d.caps)}).to_string(),
            ));
        }
        // Briardr sends its own pings and drops sockets that stop answering them.
        let mut alive = true;
        let mut pings = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(10),
            Duration::from_secs(10),
        );
        loop {
            tokio::select! {
                msg = ws.recv() => match msg {
                    Some(Ok(Message::Text(text))) => {
                        alive = true;
                        if let Ok(m) = serde_json::from_str::<Value>(text.as_str()) {
                            if m.get("t").and_then(Value::as_str) == Some("ping") {
                                let mut pong = serde_json::Map::new();
                                pong.insert("t".into(), json!("pong"));
                                if let Some(id) = m.get("id") {
                                    pong.insert("id".into(), id.clone());
                                }
                                pong.insert("now".into(), json!(now_ms()));
                                if ws.send(Message::Text(Value::Object(pong).to_string().into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => alive = true,
                    Some(Ok(Message::Pong(_))) => alive = true,
                    Some(Ok(Message::Ping(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                },
                out = rx.recv() => match out {
                    Some(Out::Text(s)) => {
                        if ws.send(Message::Text(s.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Out::Close(code, reason)) => {
                        let _ = ws.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                        break;
                    }
                    None => break,
                },
                _ = pings.tick() => {
                    if !alive {
                        break; // terminate: no close handshake
                    }
                    alive = false;
                    if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        lock(&self.live).retain(|s| s.id != id);
    }

    /// Reads the power state now (`power.poll()`), then reacts to the events.
    pub async fn poll_power(self: &Arc<Self>) -> Result<MacSummary, ApiError> {
        let power = self.d.power.clone();
        let report = tokio::task::spawn_blocking(move || lock(&power).poll(now_ms()))
            .await
            .map_err(|_| ApiError::Internal)?;
        self.power_events(&report.events);
        self.broadcast(
            &json!({"t": "mac", "mac": report.summary, "host": caps_json(&self.d.caps)}),
        );
        Ok(report.summary)
    }

    /// Power events from `poll` or `tick`. Low battery goes to live sockets and, when the
    /// setting allows, to every device as a push. Wake and keep-awake events have no listener.
    pub fn power_events(self: &Arc<Self>, events: &[PowerEvent]) {
        for ev in events {
            let PowerEvent::LowBattery(b) = ev else {
                continue;
            };
            self.broadcast(&json!({"t": "battery", "level": b.level, "percent": b.percent, "sleeping": b.sleeping}));
            if !self.cfg().notifications.low_battery || !self.d.caps.battery {
                continue;
            }
            let percent = b.percent.map_or("null".to_string(), |p| p.to_string());
            let level = b.level.map_or("null".to_string(), |l| l.to_string());
            let (title, body) = if b.sleeping {
                (
                    "Mac battery low: going to sleep".to_string(),
                    format!("{percent}% left. The Mac goes to sleep in 30 seconds unless it’s plugged in. Always on resumes once the battery is above {level}%."),
                )
            } else {
                (
                    format!("Mac battery at {percent}%"),
                    format!("{percent}% left. Plug in the Mac to keep your agents running."),
                )
            };
            let push = self.d.push.clone();
            tokio::spawn(async move {
                push.send("battery", &title, &body, None, "battery", None, 3600)
                    .await;
            });
        }
    }

    pub fn model_message(&self) -> Value {
        json!({
            "t": "model",
            "model": self.d.herdr.model(),
            "herdr": self.d.herdr.health(),
            "serverNow": now_ms(),
            "version": VERSION,
        })
    }

    /// Coalesces model broadcasts for 80 ms.
    fn broadcast_model(self: &Arc<Self>) {
        if self.model_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            if let Some(app) = weak.upgrade() {
                app.model_pending.store(false, Ordering::SeqCst);
                let msg = app.model_message();
                app.broadcast(&msg);
            }
        });
    }

    /// Sends a message to every open live socket.
    pub fn broadcast(&self, msg: &Value) {
        let data = msg.to_string();
        for s in lock(&self.live).iter() {
            let _ = s.tx.send(Out::Text(data.clone()));
        }
    }

    /// A diagnostics event from a JSON object of fields.
    pub fn event(&self, ev: &str, fields: Value) {
        let map: BTreeMap<String, Value> = match fields {
            Value::Object(m) => m.into_iter().collect(),
            _ => BTreeMap::new(),
        };
        self.d.diag.event(ev, &map);
    }

    /// The stream with a folder, for Changes and Docs.
    pub fn stream_with_folder(&self, id: &str) -> Result<crate::herdr::model::Stream, ApiError> {
        let s = self
            .d
            .herdr
            .model()
            .and_then(|m| m.streams.into_iter().find(|s| s.id == id))
            .ok_or_else(stream_gone)?;
        if s.cwd.as_deref().is_none_or(str::is_empty) {
            return Err(action_err(
                409,
                "no_folder",
                "Herdr hasn’t reported a folder for this stream.",
            ));
        }
        Ok(s)
    }

    pub fn stream_exists(&self, id: &str) -> bool {
        self.d
            .herdr
            .model()
            .is_some_and(|m| m.streams.iter().any(|s| s.id == id))
    }
}

/// `new URL(req.url).pathname`: WHATWG path normalization of the request target.
fn pathname(req: &Request<Body>) -> Option<String> {
    let target = req.uri().path_and_query().map_or("/", |p| p.as_str());
    url::Url::parse(&format!("http://h{target}"))
        .ok()
        .map(|u| u.path().to_string())
}

/// `path.extname`.
fn extname(p: &str) -> &str {
    let base = p.rsplit('/').next().unwrap_or("");
    match base.rfind('.') {
        Some(0) | None => "",
        Some(i) => &base[i..],
    }
}

/// `path.resolve(root, rel)` for an absolute root: lexical, no symlinks, no trailing slash.
fn lexical_resolve(root: &Path, rel: &str) -> String {
    let joined = format!("{}/{}", root.to_string_lossy(), rel);
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `/^\/(icons\/(apple-touch-icon|icon-192|icon-512|icon-maskable-512)|manifest)[.\w]*$/` group 2,
/// or "manifest" for /manifest.webmanifest.
fn icon_kind(rel: &str) -> Option<&'static str> {
    let rest_ok = |s: &str| {
        s.bytes()
            .all(|b| b == b'.' || b == b'_' || b.is_ascii_alphanumeric())
    };
    if let Some(after) = rel.strip_prefix("/icons/") {
        for name in [
            "apple-touch-icon",
            "icon-192",
            "icon-512",
            "icon-maskable-512",
        ] {
            if after.strip_prefix(name).is_some_and(rest_ok) {
                return Some(name);
            }
        }
    }
    (rel == "/manifest.webmanifest").then_some("manifest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_match_like_the_ts_regex() {
        assert_eq!(match_route("/api/session", "/api/session"), Some(vec![]));
        assert_eq!(match_route("/api/session", "/api/session/"), None);
        assert_eq!(
            match_route(
                "/api/streams/:id/notes/:note",
                "/api/streams/w1%3Ap1/notes/abc"
            ),
            Some(vec!["w1%3Ap1".to_string(), "abc".to_string()])
        );
        assert_eq!(
            match_route("/api/streams/:id/info", "/api/streams//info"),
            None
        );
        // `.` is a regex wildcard in the pattern.
        assert!(match_route("/api/ca.mobileconfig", "/api/caXmobileconfig").is_some());
        assert_eq!(
            match_route("/api/plugins/:id/*", "/api/plugins/ci/runs/42"),
            Some(vec!["ci".to_string(), "runs/42".to_string()])
        );
        assert_eq!(match_route("/api/plugins/:id/*", "/api/plugins/ci/"), None);
        assert_eq!(match_route("/api/plugins/:id/*", "/api/plugins/ci"), None);
        assert_eq!(
            regex_source("/api/streams/:id/info"),
            "^\\/api\\/streams\\/([^/]+)\\/info$"
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(strip_port("a.b:443"), "a.b");
        assert_eq!(strip_port("[::1]:3000"), "[::1]");
        assert_eq!(strip_port("host:"), "host:");
        assert_eq!(decode_uri_component("w1%3Ap1").as_deref(), Some("w1:p1"));
        assert_eq!(decode_uri_component("%E0%A4%A"), None);
        assert_eq!(decode_uri_component("%FF"), None);
        assert_eq!(js_number(""), 0.0);
        assert_eq!(js_number(" 12 "), 12.0);
        assert!(js_number("12abc").is_nan());
        assert_eq!(extname("/a/b.js"), ".js");
        assert_eq!(extname("/a/.hidden"), "");
        assert_eq!(extname("/streams/x"), "");
        assert_eq!(lexical_resolve(Path::new("/w/dist"), "./../x"), "/w/x");
        assert_eq!(
            lexical_resolve(Path::new("/w/dist"), "./index.html"),
            "/w/dist/index.html"
        );
        assert_eq!(icon_kind("/icons/icon-192.png"), Some("icon-192"));
        assert_eq!(icon_kind("/icons/icon-192-x.png"), None);
        assert_eq!(icon_kind("/manifest.webmanifest"), Some("manifest"));
        assert_eq!(icon_kind("/app.js"), None);
    }

    #[test]
    fn security_header_frames() {
        let h = security_headers("briardr.ts.net:3000", 3100, None);
        let csp = &h[0].1;
        assert!(csp.contains("connect-src 'self' wss://briardr.ts.net:3000"));
        assert!(csp.contains("frame-src https://briardr.ts.net:*"));
        let none = security_headers("a", 1, Some(&[]));
        assert!(none[0].1.contains("frame-src 'none'"));
        assert_eq!(none.last().map(|x| x.1.as_str()), Some("1"));
    }

    #[test]
    fn script_hashes_are_only_sha256_sources() {
        let ok = format!("'sha256-{}='", "A".repeat(43));
        assert!(is_script_hash(&ok));
        assert!(!is_script_hash(&ok[..ok.len() - 1]));
        assert!(!is_script_hash("'unsafe-inline'"));
        assert!(!is_script_hash(&format!("'sha256-{}'; x", "A".repeat(44))));
        assert!(!is_script_hash(&format!("'sha256-{}'", "A;".repeat(22))));
    }
}
