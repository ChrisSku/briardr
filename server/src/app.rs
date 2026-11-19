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
use std::collections::{BTreeMap, HashMap, HashSet};
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
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Number, Value};
use tokio::sync::mpsc;

use crate::actions::{from_herdr, ActionError, Actions};
use crate::auth::{Auth, Device, RateLimiter};
use crate::config::{
    lock_for, preview_origins, read_config, save_config, BriardrConfig, ConfigError, Paths,
};
use crate::diag::Diagnostics;
use crate::docs::normalize_lexically;
use crate::herdr::backend::Herdr;
use crate::herdr::model::Model;
use crate::herdr::rpc::HerdrError;
use crate::herdr::state::{HerdrEvent, HerdrHealth, HerdrReader, Transition};
use crate::net::{forwarded_client, is_allowed_peer, is_trusted_proxy};
use crate::notes::Notes;
use crate::platform::{HostCapabilities, HostPlatform};
use crate::plugins::PluginInfo;
use crate::power::{MacSummary, Power, PowerEvent};
use crate::preview::Previews;
use crate::push::Push;
use crate::static_files::{extname, static_file};
use crate::terminal_hub::{speaks_protocol, TerminalHub};
use crate::usage::Usage;

#[cfg(test)]
mod protocol;
mod routes_misc;
mod routes_notes;
mod routes_plugins;
mod routes_session;
mod routes_streams;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The largest WebSocket message a client may send (an input or a paste is at most 128K characters).
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

/// A stream whose terminal runs on another machine (herdr-core): its files aren't on this host.
fn elsewhere(machine: &str, host: &str) -> ApiError {
    action_err(
        409,
        "on_another_machine",
        format!("This terminal runs on {machine}, so its files aren’t on {host}."),
    )
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

/// `body` as JSON with `no-store`, plus extra headers.
pub fn send(status: u16, body: &impl Serialize, extra: &[(&str, String)]) -> Resp {
    let mut headers = vec![
        (
            "content-type",
            "application/json; charset=utf-8".to_string(),
        ),
        ("cache-control", "no-store".to_string()),
    ];
    headers.extend(extra.iter().cloned());
    match serde_json::to_vec(body) {
        Ok(json) => response(status, json, &headers),
        // Only a map whose keys aren't text fails, and the API's types have none.
        Err(_) => response(500, Body::empty(), &[]),
    }
}

pub fn ok(body: impl Serialize) -> Result<Resp, ApiError> {
    Ok(send(200, &body, &[]))
}

/// `{ok}`: whether it was done, from a route that has nothing else to say.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Done {
    pub ok: bool,
}

pub fn done() -> Result<Resp, ApiError> {
    ok(Done { ok: true })
}

/// What a refused request says: its code and, for people, what went wrong.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ErrorBody<'a> {
    pub error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub message: Option<&'a str>,
}

/// A refusal with its code and message.
pub fn refuse(status: u16, error: &str, message: &str) -> Resp {
    send(
        status,
        &ErrorBody {
            error,
            message: Some(message),
        },
        &[],
    )
}

/// A refusal with only its code.
pub fn refuse_code(status: u16, error: &str) -> Resp {
    send(
        status,
        &ErrorBody {
            error,
            message: None,
        },
        &[],
    )
}

/// `GET /api/…` of a built-in plugin that is off.
#[derive(Serialize)]
struct PluginDisabled<'a> {
    error: &'static str,
    plugin: &'a str,
    message: &'static str,
}

/// The plain-text 500 for a request that fails before it reaches a route.
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
        None => format!("'self' https://{h}:*"),
        Some([]) => "'self'".to_string(),
        Some(list) => format!("'self' {}", list.join(" ")),
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

/// A host without its port (`:` and digits at the end).
fn strip_port(host: &str) -> &str {
    match host.rfind(':') {
        Some(i) if i + 1 < host.len() && host[i + 1..].bytes().all(|b| b.is_ascii_digit()) => {
            &host[..i]
        }
        _ => host,
    }
}

/// The name in a Host header, lowercased, without its port or an IPv6 address's brackets.
fn host_name(host: &str) -> String {
    let host = host.to_lowercase();
    if let Some(rest) = host.strip_prefix('[') {
        // `[::1]:7443` is `::1`; without the `]`, the last character is dropped all the same.
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
    }
}

/// Whether a host name is this computer's own (localhost or a loopback address), which another device
/// can't open.
fn is_loopback_name(name: &str) -> bool {
    name == "localhost"
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

// ---------- request helpers ----------

/// Read a request body, refusing it as soon as it grows past the limit (413 `too_large`).
pub async fn read_body(headers: &HeaderMap, body: Body, limit: usize) -> Result<Vec<u8>, ApiError> {
    let too_large = || action_err(413, "too_large", "Request too large");
    if declared_length(headers) > limit as u64 {
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

/// The JSON body as `T` (empty = `{}`), 400 `bad_json` when it doesn't parse or doesn't fit `T`.
pub async fn json_body_as<T: DeserializeOwned>(ctx: &mut Ctx, limit: usize) -> Result<T, ApiError> {
    let body = std::mem::replace(&mut ctx.body, Body::empty());
    let buf = read_body(&ctx.headers, body, limit).await?;
    let buf: &[u8] = if buf.is_empty() { b"{}" } else { &buf };
    serde_json::from_slice(buf).map_err(|_| action_err(400, "bad_json", "Invalid JSON"))
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
    let declared = declared_length(&ctx.headers);
    if declared > max {
        return Err(UploadError::DeclaredTooLarge(kind));
    }
    let mut body = std::mem::replace(&mut ctx.body, Body::empty());
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        // A client that went away shows as a body error.
        let frame = frame.map_err(|_| UploadError::Cancelled)?;
        if let Ok(data) = frame.into_data() {
            if (out.len() + data.len()) as u64 > max {
                return Err(UploadError::TooLarge);
            }
            out.extend_from_slice(&data);
        }
    }
    if declared > 0 && (out.len() as u64) < declared {
        return Err(UploadError::Cancelled);
    }
    Ok(out)
}

pub fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The body size a request declares, 0 when it declares none (hyper refuses a Content-Length that
/// isn't a number).
fn declared_length(headers: &HeaderMap) -> u64 {
    header_str(headers, "content-length")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// A percent-encoded path, decoded; None when a `%` isn't followed by two hex digits or the bytes
/// aren't UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let hex = |b: Option<u8>| char::from(b?).to_digit(16);
    let mut bytes = s.bytes();
    let mut out = Vec::with_capacity(s.len());
    while let Some(b) = bytes.next() {
        out.push(match b {
            b'%' => (hex(bytes.next())? * 16 + hex(bytes.next())?) as u8,
            b => b,
        });
    }
    String::from_utf8(out).ok()
}

/// A device as answers name it.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DeviceRef<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

impl<'a> From<&'a Device> for DeviceRef<'a> {
    fn from(d: &'a Device) -> Self {
        DeviceRef {
            id: &d.id,
            name: &d.name,
        }
    }
}

/// The new name of a device, workspace, project or stream (`POST /api/…/:id/rename`).
#[derive(Deserialize)]
pub struct RenameBody {
    pub name: Option<String>,
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
    segments: Vec<Segment>,
    public: bool,
    /// The built-in plugin the route belongs to; refused while that plugin is off.
    plugin: Option<&'static str>,
    h: Handler,
}

/// One part of a route's path, between slashes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    /// `:key`: any one segment that isn't empty.
    Param(String),
    /// A final `*`: the rest of the path, at least one character, as the parameter `rest`.
    Rest,
}

impl Segment {
    fn parse_pattern(pattern: &str) -> Vec<Segment> {
        let parts: Vec<&str> = pattern.trim_start_matches('/').split('/').collect();
        let last = parts.len() - 1;
        parts
            .iter()
            .enumerate()
            .map(|(i, part)| match part.strip_prefix(':') {
                Some(key) => Segment::Param(key.to_string()),
                None if *part == "*" && i == last => Segment::Rest,
                None => Segment::Literal(part.to_string()),
            })
            .collect()
    }
}

/// The parameters of `path` (still percent-encoded) when it matches the route's segments, in order.
fn captures<'p>(segments: &[Segment], path: &'p str) -> Option<Vec<&'p str>> {
    let mut rest = path.strip_prefix('/')?;
    let mut caps = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        if *segment == Segment::Rest {
            return (!rest.is_empty()).then(|| {
                caps.push(rest);
                caps
            });
        }
        let last = i + 1 == segments.len();
        let (part, tail) = match rest.split_once('/') {
            Some((part, tail)) if !last => (part, tail),
            None if last => (rest, ""),
            // More of the path than the route has, or less.
            _ => return None,
        };
        match segment {
            Segment::Literal(literal) if literal == part => {}
            Segment::Param(_) if !part.is_empty() => caps.push(part),
            _ => return None,
        }
        rest = tail;
    }
    Some(caps)
}

/// The route as a regular expression (`:key` is `([^/]+)`, `*` is `(.+)`), as `/__test/routes` lists it
/// for the authentication test, which turns it back into a path.
fn pattern_regex(segments: &[Segment]) -> String {
    let parts: String = segments
        .iter()
        .map(|segment| match segment {
            Segment::Literal(literal) => format!("\\/{literal}"),
            Segment::Param(_) => "\\/([^/]+)".to_string(),
            Segment::Rest => "\\/(.+)".to_string(),
        })
        .collect();
    format!("^{parts}$")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RouteInfo {
    pub method: String,
    /// The route as `pattern_regex` writes it.
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
        self.0.push(Route {
            method,
            segments: Segment::parse_pattern(pattern),
            public,
            plugin: crate::plugins::route_owner(pattern),
            h: Box::new(move |app, ctx| Box::pin(f(app, ctx))),
        });
    }
}

// ---------- the app ----------

/// Shared, live configuration: handlers read it, settings routes change it. config.json isn't only the
/// server's: `briardr configure` changes it while Briardr runs, for the next start, so a settings change
/// is made to the file as it is on disk and to the running configuration, each keeping the rest.
pub struct ConfigCell {
    cfg: RwLock<BriardrConfig>,
    paths: Paths,
    /// For the defaults, where config.json leaves a setting out.
    env: HashMap<String, String>,
    changes: tokio::sync::Mutex<()>,
}

impl ConfigCell {
    pub fn new(cfg: BriardrConfig, paths: Paths, env: HashMap<String, String>) -> Self {
        ConfigCell {
            cfg: RwLock::new(cfg),
            paths,
            env,
            changes: tokio::sync::Mutex::new(()),
        }
    }
    pub fn get(&self) -> BriardrConfig {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// Serializes a complete read/validate/save transaction, including asynchronous validation.
    pub async fn lock_changes(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.changes.lock().await
    }
    /// Makes `change` to config.json, read again under config.lock, and then to the running
    /// configuration, which it returns. A file that can't be read or saved changes neither.
    pub fn change(&self, change: impl Fn(&mut BriardrConfig)) -> Result<BriardrConfig, ApiError> {
        let internal = |_: ConfigError| ApiError::Internal;
        let _held = lock_for(&self.paths.config()).map_err(internal)?;
        let mut on_disk = read_config(&self.paths, &self.env).map_err(internal)?;
        change(&mut on_disk);
        save_config(&self.paths, &on_disk).map_err(internal)?;
        let mut cfg = self.cfg.write().unwrap_or_else(|e| e.into_inner());
        change(&mut cfg);
        Ok(cfg.clone())
    }
    pub fn paths(&self) -> &Paths {
        &self.paths
    }
}

pub struct AppDeps {
    pub cfg: Arc<ConfigCell>,
    pub auth: Arc<Auth>,
    pub herdr: Herdr,
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
    /// The address devices open, as `briardr pair` prints it (`public_origin_of`).
    pub public_origin: String,
    pub dictation_helper: Arc<RwLock<Option<PathBuf>>>,
    pub ca_profile: Option<String>,
    /// (wanted, effective) TLS mode.
    pub tls_mode: Arc<RwLock<(String, String)>>,
    pub caps: HostCapabilities,
    pub updates: Arc<crate::update::Updates>,
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

/// What Briardr sends on /ws/live, by its `t`.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum LiveMessage<'a> {
    /// The first message: `instance_id` changes on every server start, so a client can refresh after
    /// reconnecting to a new one.
    #[serde(rename_all = "camelCase")]
    Hello { instance_id: &'a str },
    /// The streams and Herdr's health (also `GET /api/state`).
    Model(ModelState),
    /// The power state, and what the host can do.
    Mac {
        mac: &'a MacSummary,
        host: &'a HostCapabilities,
    },
    /// The battery fell to the low level (`sleeping`: the Mac goes to sleep).
    Battery {
        level: Option<u32>,
        percent: Option<u32>,
        sleeping: bool,
    },
    /// An agent's state changed.
    Transition(&'a Transition),
    /// The plugins changed on the host.
    Plugins { plugins: &'a [PluginInfo] },
    /// What a plugin's backend pushed for its client.
    PluginEvent {
        plugin: &'a str,
        topic: &'a str,
        data: &'a Value,
    },
    /// A stream's note changed (`revision` 0: it was deleted).
    #[serde(rename_all = "camelCase")]
    Notes {
        stream_id: &'a str,
        note_id: &'a str,
        revision: u64,
    },
    /// The answer to the client's `ping`, with the host's time.
    Pong {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        id: Option<Number>,
        now: u64,
    },
}

/// The streams and Herdr's health, with the host's time and Briardr's version.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ModelState {
    model: Option<Model>,
    herdr: HerdrHealth,
    server_now: u64,
    version: &'static str,
}

/// What a client sends on /ws/live: only `ping`; anything else is ignored.
#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields = nullable))]
#[serde(tag = "t", rename_all = "snake_case")]
enum LiveRequest {
    Ping { id: Option<Number> },
}

pub struct App {
    pub d: AppDeps,
    /// Changes on every server start, so connected clients can refresh after reconnecting.
    instance_id: String,
    /// Which plugins are on (state/plugins.json).
    pub plugins: crate::plugins::StateCell,
    pub backends: crate::plugins::backend::Backends,
    routes: Routes,
    pub site_links: crate::site::SiteLinks,
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
            site_links: crate::site::SiteLinks::default(),
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
                app.broadcast(&LiveMessage::PluginEvent {
                    plugin: &plugin,
                    topic: &topic,
                    data: &data,
                });
            }
        });
    }

    /// Tells devices when plugins change on the host (installed, rebuilt by `briardr plugin dev`, turned
    /// on or off from the command line), so they load the new client without a reload.
    fn spawn_plugin_watch(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let paths = self.d.cfg.paths().clone();
        tokio::spawn(async move {
            let mut last: Option<Vec<(std::path::PathBuf, Option<crate::config::Stamp>)>> = None;
            loop {
                let p = paths.clone();
                let now = tokio::task::spawn_blocking(move || crate::plugins::fingerprint(&p))
                    .await
                    .ok();
                let Some(app) = weak.upgrade() else { return };
                if let (Some(before), Some(after)) = (&last, &now) {
                    // A rebuilt or edited plugin's backend starts again with the new code on its next request.
                    let plugins_dir = paths.plugins();
                    for (file, stamp) in after {
                        let was = before.iter().find(|(f, _)| f == file).map(|(_, m)| m);
                        if was.is_some_and(|m| m != stamp) {
                            if let Ok(rest) = file.strip_prefix(&plugins_dir) {
                                if let Some(id) = rest.components().next() {
                                    app.backends.stop(&id.as_os_str().to_string_lossy());
                                }
                            }
                        }
                    }
                }
                if last.is_some() && now != last {
                    if let Ok(plugins) = routes_plugins::plugin_list(&app).await {
                        // Backends of plugins turned off (or changed) on the host stop.
                        let active: Vec<String> = plugins
                            .iter()
                            .filter(|p| p.enabled)
                            .map(|p| p.id.clone())
                            .collect();
                        app.backends.retain(&active);
                        app.broadcast(&LiveMessage::Plugins { plugins: &plugins });
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
                    HerdrEvent::Transition(t) => app.broadcast(&LiveMessage::Transition(&t)),
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
                let _ = app.d.push.forget_device(&id);
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
                pattern: pattern_regex(&r.segments),
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
        let name = host_name(header_str(headers, "host").unwrap_or(""));
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

    /// Where a new device opens Briardr: where this request reached it (a Host the Host check allowed),
    /// as another device on that network does. Behind a proxy, or from the host itself (localhost), it's
    /// the address `briardr pair` prints.
    pub fn pairing_origin(&self, headers: &HeaderMap) -> String {
        match header_str(headers, "host") {
            Some(host) if !self.in_proxy_mode() && !is_loopback_name(&host_name(host)) => {
                format!("https://{host}")
            }
            _ => self.d.public_origin.clone(),
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
            return res; // an upgraded socket: its handshake answer gets no headers
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
        if let Some(rest) = path.strip_prefix("/site/") {
            return crate::site::serve(&self, rest).await;
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
                &ErrorBody {
                    error: "rate_limited",
                    message: Some("Too many requests. Wait a minute."),
                },
                &[("retry-after", "60".to_string())],
            )
        };
        for (idx, r) in self.routes.0.iter().enumerate() {
            if r.method != method {
                continue;
            }
            let Some(caps) = captures(&r.segments, &path) else {
                continue;
            };
            let keys = r.segments.iter().filter_map(|segment| match segment {
                Segment::Literal(_) => None,
                Segment::Param(key) => Some(key.as_str()),
                Segment::Rest => Some("rest"),
            });
            let mut params = BTreeMap::new();
            for (k, v) in keys.zip(caps) {
                let Some(v) = percent_decode(v) else {
                    return server_error(&self.host());
                };
                params.insert(k.to_string(), v);
            }
            let device = self.authenticate(&parts.headers);
            if device.is_none() && !self.anon_allowed(&client) {
                return rate_limited();
            }
            if !r.public && device.is_none() {
                return refuse(
                    401,
                    "unauthenticated",
                    "This device isn’t signed in to Briardr.",
                );
            }
            let site = header_str(&parts.headers, "sec-fetch-site");
            if site.is_some_and(|s| s != "same-origin" && s != "none") {
                return refuse_code(403, "cross_site");
            }
            if method != Method::GET
                && method != Method::HEAD
                && !self.origin_allowed(&parts.headers)
            {
                return refuse_code(403, "bad_origin");
            }
            if let Some(plugin) = self.routes.0[idx].plugin {
                if !self.plugins.get().is_enabled(plugin) {
                    let body = PluginDisabled {
                        error: "plugin_disabled",
                        plugin,
                        message: "This plugin is turned off. Turn it on in Settings → Plugins.",
                    };
                    return send(404, &body, &[]);
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
                Err(ApiError::Action(e)) => refuse(e.status, &e.code, &e.message),
                Err(ApiError::Herdr(e)) => {
                    let e = from_herdr(&e, &self.host());
                    refuse(e.status, &e.code, &e.message)
                }
                Err(ApiError::Internal) => {
                    self.event(
                        "api_error",
                        json!({"method": method.as_str().to_lowercase(), "category": "exception"}),
                    );
                    let message = format!("Something went wrong on {}.", self.host());
                    refuse(500, "internal", &message)
                }
            };
        }
        if self.authenticate(&parts.headers).is_none() && !self.anon_allowed(&client) {
            return rate_limited();
        }
        refuse_code(404, "not_found")
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
        let Some(rest) = percent_decode(rest) else {
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

    /// The Content-Security-Policy for the page: the usual one, with its inline scripts (the import map and
    /// the theme) allowed by the hashes the build writes next to it (`index.html.csp`, scripts/build.ts).
    /// None without that file.
    async fn page_csp(&self, headers: &HeaderMap, file: &Path) -> Option<String> {
        let mut listing = file.as_os_str().to_owned();
        listing.push(".csp");
        let listed = tokio::fs::read_to_string(listing).await.ok()?;
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
        let Some(mut rel) = percent_decode(pathname) else {
            return server_error(&self.host());
        };
        if rel == "/" || extname(&rel).is_empty() {
            rel = "/index.html".to_string(); // client-side routes
        }
        let root = normalize_lexically(&self.d.web_dir);
        // Relative to the root, as `rel` is absolute; `..` can't leave it.
        let file = normalize_lexically(&root.join(format!(".{rel}")));
        if !file.starts_with(&root) || file == root {
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
        let file_path = file.clone();
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
        if !speaks_protocol(&q) {
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
        let hello = LiveMessage::Hello {
            instance_id: &self.instance_id,
        };
        let mut first = vec![hello, LiveMessage::Model(self.model_state())];
        let summary = lock(&self.d.power).summary().cloned();
        if let Some(mac) = &summary {
            first.push(LiveMessage::Mac {
                mac,
                host: &self.d.caps,
            });
        }
        for m in &first {
            if let Ok(text) = serde_json::to_string(m) {
                let _ = tx.send(Out::Text(text));
            }
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
                        if let Ok(LiveRequest::Ping { id }) = serde_json::from_str(text.as_str()) {
                            let pong = LiveMessage::Pong { id, now: now_ms() };
                            let Ok(pong) = serde_json::to_string(&pong) else { continue };
                            if ws.send(Message::Text(pong.into())).await.is_err() {
                                break;
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
        self.broadcast(&LiveMessage::Mac {
            mac: &report.summary,
            host: &self.d.caps,
        });
        Ok(report.summary)
    }

    /// Power events from `poll` or `tick`. Low battery goes to live sockets and, as a push, to the
    /// devices that take it (the host's setting, or their own choice). Wake and keep-awake events
    /// have no listener.
    pub fn power_events(self: &Arc<Self>, events: &[PowerEvent]) {
        for ev in events {
            let PowerEvent::LowBattery(b) = ev else {
                continue;
            };
            self.broadcast(&LiveMessage::Battery {
                level: b.level,
                percent: b.percent,
                sleeping: b.sleeping,
            });
            if !self.d.caps.battery || !self.d.push.wanted("battery") {
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

    pub fn model_state(&self) -> ModelState {
        ModelState {
            model: self.d.herdr.model(),
            herdr: self.d.herdr.health(),
            server_now: now_ms(),
            version: VERSION,
        }
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
                app.broadcast(&LiveMessage::Model(app.model_state()));
            }
        });
    }

    /// Sends a message to every open live socket.
    pub fn broadcast(&self, msg: &LiveMessage) {
        let Ok(data) = serde_json::to_string(msg) else {
            return;
        };
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
        if let Some(machine) = &s.machine {
            return Err(elsewhere(machine, &self.host()));
        }
        if s.cwd.as_deref().is_none_or(str::is_empty) {
            return Err(action_err(
                409,
                "no_folder",
                "Herdr hasn’t reported a folder for this stream.",
            ));
        }
        Ok(s)
    }

    /// A stream this host can hand files to: it exists, and its terminal runs here.
    pub fn stream_here(&self, id: &str) -> Result<(), ApiError> {
        let s = self
            .d
            .herdr
            .model()
            .and_then(|m| m.streams.into_iter().find(|s| s.id == id))
            .ok_or_else(stream_gone)?;
        match &s.machine {
            Some(machine) => Err(elsewhere(machine, &self.host())),
            None => Ok(()),
        }
    }

    pub fn stream_exists(&self, id: &str) -> bool {
        self.d
            .herdr
            .model()
            .is_some_and(|m| m.streams.iter().any(|s| s.id == id))
    }
}

/// The request's path, normalized as a browser's URL parser does (WHATWG: `.` and `..` segments).
fn pathname(req: &Request<Body>) -> Option<String> {
    let target = req.uri().path_and_query().map_or("/", |p| p.as_str());
    url::Url::parse(&format!("http://h{target}"))
        .ok()
        .map(|u| u.path().to_string())
}

/// Which home-screen icon (or the manifest) a path is: `/icons/<name>` followed only by letters, digits,
/// `_` and dots (`/icons/icon-192.png`), or `/manifest.webmanifest`.
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
    use crate::scratch::Scratch;
    use std::fs;

    #[test]
    fn a_settings_change_keeps_what_another_process_saved_for_the_restart() {
        let root = Scratch::new("config-cell");
        let paths = Paths::new(root.path());
        let env = HashMap::from([("HOME".to_string(), root.display().to_string())]);
        let mut running = read_config(&paths, &env).unwrap();
        running.port = 9443; // as BRIARDR_PORT gives it: the running server's, not the file's
        let cell = ConfigCell::new(running, paths.clone(), env.clone());
        cell.change(|c| c.notifications.ready = true).unwrap();
        let mut on_disk = read_config(&paths, &env).unwrap();
        assert_eq!(on_disk.port, 7443);
        assert!(on_disk.notifications.ready);

        // `briardr configure` while the server runs: for the next start.
        on_disk.allowed_networks = vec!["10.8.0.0/24".to_string()];
        save_config(&paths, &on_disk).unwrap();
        let after = cell.change(|c| c.updates.check = false).unwrap();
        assert!(after.allowed_networks.is_empty(), "applied at the restart");
        assert_eq!((after.port, after.updates.check), (9443, false));
        let on_disk = read_config(&paths, &env).unwrap();
        assert_eq!(on_disk.allowed_networks, ["10.8.0.0/24"]);
        assert_eq!((on_disk.port, on_disk.updates.check), (7443, false));
        assert!(on_disk.notifications.ready);

        // One that can't be read isn't saved over, and the running configuration stays as it is.
        fs::write(paths.config(), "{\"port\":").unwrap();
        assert!(cell.change(|c| c.updates.check = true).is_err());
        assert_eq!(fs::read_to_string(paths.config()).unwrap(), "{\"port\":");
        assert!(!cell.get().updates.check);
    }

    fn route_captures(pattern: &str, path: &str) -> Option<Vec<String>> {
        let segments = Segment::parse_pattern(pattern);
        captures(&segments, path).map(|c| c.into_iter().map(String::from).collect())
    }

    #[test]
    fn routes_match_whole_segments() {
        assert_eq!(route_captures("/api/session", "/api/session"), Some(vec![]));
        assert_eq!(route_captures("/api/session", "/api/session/"), None);
        assert_eq!(route_captures("/api/session", "/api/sessions"), None);
        assert_eq!(route_captures("/api/session", "/api"), None);
        assert_eq!(
            route_captures(
                "/api/streams/:id/notes/:note",
                "/api/streams/w1%3Ap1/notes/abc"
            ),
            Some(vec!["w1%3Ap1".to_string(), "abc".to_string()])
        );
        assert_eq!(
            route_captures("/api/streams/:id/info", "/api/streams//info"),
            None
        );
        assert_eq!(
            route_captures(
                "/api/streams/:id/notes/:note",
                "/api/streams/a/notes/images/x"
            ),
            None,
            "a parameter is one segment"
        );
        assert!(route_captures("/api/ca.mobileconfig", "/api/ca.mobileconfig").is_some());
        assert_eq!(
            route_captures("/api/ca.mobileconfig", "/api/caXmobileconfig"),
            None
        );
        assert_eq!(
            route_captures("/api/plugins/:id/*", "/api/plugins/ci/runs/42"),
            Some(vec!["ci".to_string(), "runs/42".to_string()])
        );
        assert_eq!(
            route_captures("/api/plugins/:id/*", "/api/plugins/ci/"),
            None
        );
        assert_eq!(
            route_captures("/api/plugins/:id/*", "/api/plugins/ci"),
            None
        );
    }

    #[test]
    fn the_route_list_writes_patterns_as_regular_expressions() {
        let regex = |pattern: &str| pattern_regex(&Segment::parse_pattern(pattern));
        assert_eq!(
            regex("/api/streams/:id/info"),
            "^\\/api\\/streams\\/([^/]+)\\/info$"
        );
        assert_eq!(
            regex("/api/plugins/:id/*"),
            "^\\/api\\/plugins\\/([^/]+)\\/(.+)$"
        );
        assert_eq!(regex("/api/ca.mobileconfig"), "^\\/api\\/ca.mobileconfig$");
    }

    #[test]
    fn helpers() {
        assert_eq!(strip_port("a.b:443"), "a.b");
        assert_eq!(strip_port("[::1]:3000"), "[::1]");
        assert_eq!(strip_port("host:"), "host:");
        assert_eq!(host_name("Mac.Tail1.ts.net:7443"), "mac.tail1.ts.net");
        assert_eq!(host_name("[::1]:7443"), "::1");
        assert!(
            is_loopback_name("localhost")
                && is_loopback_name("127.0.0.1")
                && is_loopback_name("::1")
        );
        assert!(!is_loopback_name("10.8.0.1") && !is_loopback_name("mac.tail1.ts.net"));
        assert_eq!(percent_decode("w1%3Ap1").as_deref(), Some("w1:p1"));
        assert_eq!(percent_decode("caf%C3%A9+x").as_deref(), Some("café+x"));
        assert_eq!(percent_decode("%E0%A4%A"), None);
        assert_eq!(percent_decode("%FF"), None);
        assert_eq!(percent_decode("%+1"), None);
        assert_eq!(icon_kind("/icons/icon-192.png"), Some("icon-192"));
        assert_eq!(icon_kind("/icons/icon-192-x.png"), None);
        assert_eq!(icon_kind("/manifest.webmanifest"), Some("manifest"));
        assert_eq!(icon_kind("/app.js"), None);
    }

    #[test]
    fn live_messages_are_tagged_by_t() {
        let json = |m: LiveMessage| serde_json::to_value(m).unwrap();
        assert_eq!(
            json(LiveMessage::Hello { instance_id: "ab" }),
            json!({"t": "hello", "instanceId": "ab"})
        );
        assert_eq!(
            json(LiveMessage::Notes {
                stream_id: "w1:p1",
                note_id: "0123456789abcdef",
                revision: 0
            }),
            json!({"t": "notes", "streamId": "w1:p1", "noteId": "0123456789abcdef", "revision": 0})
        );
        assert_eq!(
            json(LiveMessage::PluginEvent {
                plugin: "ci",
                topic: "run",
                data: &json!({"n": 1})
            }),
            json!({"t": "plugin_event", "plugin": "ci", "topic": "run", "data": {"n": 1}})
        );
        assert_eq!(
            json(LiveMessage::Pong { id: None, now: 5 }),
            json!({"t": "pong", "now": 5})
        );
        let transition = Transition {
            stream_id: "w1:p1".into(),
            terminal_id: "t1".into(),
            from: None,
            to: None,
            kind: crate::herdr::model::StreamKind::Agent,
            tool: "claude".into(),
            state_change_seq: Some(2),
            at: 9,
        };
        let text = serde_json::to_string(&LiveMessage::Transition(&transition)).unwrap();
        assert!(
            text.starts_with(r#"{"t":"transition","streamId":"w1:p1","#),
            "{text}"
        );
        assert!(matches!(
            serde_json::from_value(json!({"t": "ping", "id": 3})),
            Ok(LiveRequest::Ping { id: Some(_) })
        ));
        assert!(serde_json::from_value::<LiveRequest>(json!({"t": "hello"})).is_err());
    }

    #[test]
    fn security_header_frames() {
        let h = security_headers("briardr.ts.net:3000", 3100, None);
        let csp = &h[0].1;
        assert!(csp.contains("connect-src 'self' wss://briardr.ts.net:3000"));
        assert!(csp.contains("frame-src 'self' https://briardr.ts.net:*"));
        let none = security_headers("a", 1, Some(&[]));
        assert!(none[0].1.contains("frame-src 'self'"));
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
