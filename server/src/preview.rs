// Show a web server that listens on the host's localhost inside Briardr.
// Each open preview gets its own listener that forwards to the dev server on localhost. The dev server
// itself is never exposed. Closing the preview closes the listener and every connection to the dev server.
//
// Direct modes (Tailscale/LAN): the listener serves HTTPS on another port of Briardr's own addresses,
// with Briardr's certificate, and accepts the device cookie (cookies don't depend on the port).
//
// Proxy mode: the listener serves plain HTTP on loopback and the proxy publishes it as its own origin
// (slot n: publicPreviewOrigin with {n} = n, e.g. https://p3.briardr.example.com). Briardr's device
// cookie is host-only and never reaches that origin, and a preview's pages can't act as Briardr.
// Access is granted by a one-time launch link (a random token, valid for a minute, for this preview
// and device only) that sets a preview session cookie for that origin alone. The session ends when
// the preview closes or the device is revoked. Neither cookie is forwarded to the dev server.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::{CloseFrame as ClientCloseFrame, Message as ClientMessage, WebSocket};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, Uri};
use base64::prelude::{Engine, BASE64_URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as UpstreamCloseFrame;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::app::{header_str, now_ms, response, ConfigCell, Query, Resp};
use crate::auth::{read_cookie, Auth, COOKIE};
use crate::config::{preview_origins, BriardrConfig};
use crate::diag::Diagnostics;
use crate::listener::{serve, CertResolver, Handler, HandlerFuture};
use crate::net::{is_allowed_peer, is_trusted_proxy};

pub const PREVIEW_COOKIE: &str = "__Host-briardr-preview";
/// The launch path on a preview origin. Dev servers don't get requests for it.
pub const LAUNCH_PATH: &str = "/__briardr/launch";
const LAUNCH_TTL_MS: u64 = 60_000;
const SESSION_TTL_MS: u64 = 12 * 3600 * 1000;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Hop-by-hop headers belong to one connection and are never forwarded.
const HOP: [&str; 7] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
];
const TEXT_PLAIN: &str = "text/plain; charset=utf-8";
const NO_STORE: &str = "no-store";
const DENIED: &str = "Open this preview from Briardr.";
const EXPIRED: &str = "This preview link has expired. Open the preview from Briardr again.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub id: String,
    pub device_id: String,
    pub target_port: u16,
    pub listen_port: u16,
    pub created_at: u64,
    /// Proxy mode: the preview's own origin, and a one-time link that signs this device in to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    NothingListening,
    NoFreePort,
    InvalidPort,
    Unavailable,
}

impl OpenError {
    pub fn code(&self) -> &'static str {
        match self {
            OpenError::NothingListening => "nothing_listening",
            OpenError::NoFreePort => "no_free_port",
            OpenError::InvalidPort => "invalid_port",
            OpenError::Unavailable => "unavailable",
        }
    }
}

/// An open preview. The base info never carries the origin or launch link.
struct Active {
    info: PreviewInfo,
    /// The dev server's address as the probe found it ("127.0.0.1" or "::1").
    host: String,
    /// Closing sends `true`; listeners and tunnels watch it.
    shutdown: watch::Sender<bool>,
    /// Proxy mode: hashes of unused launch tokens and of preview sessions, with their expiry.
    tokens: Mutex<HashMap<String, u64>>,
    sessions: Mutex<HashMap<String, u64>>,
}

impl Active {
    fn is_closed(&self) -> bool {
        *self.shutdown.borrow()
    }
}

type UpstreamSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct Previews {
    cfg: Arc<ConfigCell>,
    tls: Option<Arc<CertResolver>>,
    addrs: Vec<String>,
    auth: Arc<Auth>,
    diag: Arc<Diagnostics>,
    active: Mutex<Vec<Arc<Active>>>,
    /// Serializes opens, so two requests can't claim the same listen port.
    open_gate: AsyncMutex<()>,
    client: Client<HttpConnector, Body>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sha256(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn random_token<const N: usize>() -> String {
    let mut buf = [0u8; N];
    rand::fill(&mut buf);
    BASE64_URL_SAFE_NO_PAD.encode(buf)
}

fn proxy_mode(cfg: &BriardrConfig) -> bool {
    cfg.tls.mode == "proxy"
}

/// The origin of the preview served on a listen port (proxy mode only).
fn origin_of(cfg: &BriardrConfig, listen_port: u16) -> Option<String> {
    if !proxy_mode(cfg) {
        return None;
    }
    let index = usize::from(listen_port.checked_sub(cfg.preview_port_start)?);
    preview_origins(cfg).ok()?.get(index).cloned()
}

/// The host (and port, when not the default) of an origin, lowercase, as `URL.host` shows it.
fn origin_host(origin: &str) -> Option<String> {
    let url = Url::parse(origin).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// A close code a WebSocket may send (1005/1006 and friends are only ever reported, never sent).
pub fn sendable_close(code: u16) -> u16 {
    if code == 1000 || code == 1001 || (3000..=4999).contains(&code) {
        code
    } else {
        1000
    }
}

pub async fn probe_port(port: u16, host: &str, timeout: Duration) -> bool {
    matches!(
        tokio::time::timeout(timeout, TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}

pub async fn find_target_host(port: u16) -> Option<String> {
    for host in ["127.0.0.1", "::1"] {
        if probe_port(port, host, PROBE_TIMEOUT).await {
            return Some(host.to_string());
        }
    }
    None
}

/// Remove Briardr's own cookies before forwarding; the dev server never sees them.
pub fn strip_briardr_cookie(cookie: Option<&str>) -> Option<String> {
    let briardr = format!("{COOKIE}=");
    let preview = format!("{PREVIEW_COOKIE}=");
    let kept: Vec<&str> = cookie?
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.starts_with(&briardr) && !p.starts_with(&preview))
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// Drop `frame-ancestors ...;` from a Content-Security-Policy, case-insensitively.
fn strip_frame_ancestors(csp: &str) -> String {
    // ASCII lowercasing keeps byte offsets, so indexes into `csp` stay valid.
    let lower = csp.to_ascii_lowercase();
    let mut out = String::with_capacity(csp.len());
    let mut i = 0;
    while let Some(rel) = lower[i..].find("frame-ancestors") {
        let start = i + rel;
        out.push_str(&csp[i..start]);
        i = match lower[start..].find(';') {
            Some(semi) => start + semi + 1,
            None => csp.len(),
        };
    }
    out.push_str(&csp[i..]);
    out
}

/// The post-launch redirect: a same-origin path, never a protocol-relative or backslash URL.
fn safe_location(next: &str) -> &str {
    if next.starts_with('/') && !next.starts_with("//") && !next.starts_with("/\\") {
        next
    } else {
        "/"
    }
}

fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn set_header(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

fn request_url(uri: &Uri) -> Option<Url> {
    let path = uri.path_and_query().map_or("/", |pq| pq.as_str());
    Url::parse(&format!("http://localhost{path}")).ok()
}

fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(query) if !query.is_empty() => format!("{}?{query}", url.path()),
        _ => url.path().to_string(),
    }
}

fn bad_gateway(port: u16) -> Resp {
    response(
        502,
        format!("Nothing is listening on port {port} anymore."),
        &[("content-type", TEXT_PLAIN.to_string())],
    )
}

fn has_session(p: &Active, headers: &HeaderMap) -> bool {
    let Some(value) = header_str(headers, "cookie")
        .and_then(|c| read_cookie(c, PREVIEW_COOKIE))
        .filter(|v| !v.is_empty())
    else {
        return false;
    };
    let now = now_ms();
    lock(&p.sessions)
        .get(&sha256(value))
        .is_some_and(|exp| *exp > now)
}

/// Resolves when the preview is closed (or its owner is gone).
async fn wait_closed(closed: &mut watch::Receiver<bool>) {
    loop {
        let shut = *closed.borrow_and_update();
        if shut {
            return;
        }
        if closed.changed().await.is_err() {
            return;
        }
    }
}

/// Relays messages between the browser and the dev server until either side closes or the preview closes.
async fn relay(client: WebSocket, upstream: UpstreamSocket, mut closed: watch::Receiver<bool>) {
    let (mut client_tx, mut client_rx) = client.split();
    let (mut up_tx, mut up_rx) = upstream.split();
    loop {
        let open = tokio::select! {
            () = wait_closed(&mut closed) => {
                let _ = up_tx.send(UpstreamMessage::Close(None)).await;
                false
            }
            msg = client_rx.next() => match msg {
                Some(Ok(ClientMessage::Text(text))) => {
                    up_tx.send(UpstreamMessage::Text(text.as_str().into())).await.is_ok()
                }
                Some(Ok(ClientMessage::Binary(bytes))) => {
                    up_tx.send(UpstreamMessage::Binary(bytes)).await.is_ok()
                }
                Some(Ok(ClientMessage::Close(frame))) => {
                    let code = sendable_close(frame.map_or(1000, |f| f.code));
                    let close = UpstreamCloseFrame { code: code.into(), reason: "".into() };
                    let _ = up_tx.send(UpstreamMessage::Close(Some(close))).await;
                    false
                }
                Some(Ok(_)) => true,
                Some(Err(_)) | None => {
                    let close = UpstreamCloseFrame { code: 1000u16.into(), reason: "".into() };
                    let _ = up_tx.send(UpstreamMessage::Close(Some(close))).await;
                    false
                }
            },
            msg = up_rx.next() => match msg {
                Some(Ok(UpstreamMessage::Text(text))) => {
                    client_tx.send(ClientMessage::Text(text.as_str().into())).await.is_ok()
                }
                Some(Ok(UpstreamMessage::Binary(bytes))) => {
                    client_tx.send(ClientMessage::Binary(bytes)).await.is_ok()
                }
                Some(Ok(UpstreamMessage::Close(frame))) => {
                    let (code, reason) = frame.map_or((1000, String::new()), |f| {
                        (u16::from(f.code), f.reason.as_str().to_string())
                    });
                    let close = ClientCloseFrame { code: sendable_close(code), reason: reason.into() };
                    let _ = client_tx.send(ClientMessage::Close(Some(close))).await;
                    false
                }
                Some(Ok(_)) => true,
                Some(Err(_)) | None => {
                    let close = ClientCloseFrame { code: 1000, reason: "".into() };
                    let _ = client_tx.send(ClientMessage::Close(Some(close))).await;
                    false
                }
            },
        };
        if !open {
            break;
        }
    }
}

/// Connects to the dev server first, so the browser gets the subprotocol it actually chose.
async fn forward_upgrade(p: &Active, mut parts: Parts, path_query: &str) -> Resp {
    let port = p.info.target_port;
    let mut closed = p.shutdown.subscribe();
    let target = format!("ws://{}{path_query}", authority(&p.host, port));
    let Ok(mut request) = target.as_str().into_client_request() else {
        return response(502, Body::empty(), &[]);
    };
    {
        let headers = request.headers_mut();
        set_header(headers, "host", &format!("localhost:{port}"));
        set_header(headers, "origin", &format!("http://localhost:{port}"));
        if let Some(cookie) = strip_briardr_cookie(header_str(&parts.headers, "cookie")) {
            set_header(headers, "cookie", &cookie);
        }
        let protocols: Vec<&str> = header_str(&parts.headers, "sec-websocket-protocol")
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if !protocols.is_empty() {
            set_header(headers, "sec-websocket-protocol", &protocols.join(", "));
        }
    }
    let Ok((mut upstream, handshake)) = connect_async(request).await else {
        return response(502, Body::empty(), &[]);
    };
    let still_open = !*closed.borrow_and_update();
    if !still_open {
        let _ = upstream.close(None).await;
        return response(502, Body::empty(), &[]);
    }
    let chosen = handshake
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let Ok(mut upgrade) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
        let _ = upstream.close(None).await;
        return response(400, Body::empty(), &[]);
    };
    if let Some(protocol) = chosen {
        upgrade = upgrade.protocols([protocol]);
    }
    upgrade.on_upgrade(move |socket| relay(socket, upstream, closed))
}

impl Previews {
    pub fn new(
        cfg: Arc<ConfigCell>,
        tls: Option<Arc<CertResolver>>,
        addrs: Vec<String>,
        auth: Arc<Auth>,
        diag: Arc<Diagnostics>,
    ) -> Arc<Previews> {
        let client = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(0)
            .build_http::<Body>();
        Arc::new(Previews {
            cfg,
            tls,
            addrs,
            auth,
            diag,
            active: Mutex::new(Vec::new()),
            open_gate: AsyncMutex::new(()),
            client,
        })
    }

    /// Whether previews can be opened at all (proxy mode needs preview origins).
    pub fn available(&self) -> bool {
        let cfg = self.cfg.get();
        !proxy_mode(&cfg)
            || cfg
                .public_preview_origin
                .as_deref()
                .is_some_and(|o| !o.is_empty())
    }

    pub fn list(&self, device_id: &str) -> Vec<PreviewInfo> {
        lock(&self.active)
            .iter()
            .filter(|p| p.info.device_id == device_id)
            .map(|p| p.info.clone())
            .collect()
    }

    pub async fn open(
        self: &Arc<Self>,
        device_id: &str,
        target: u16,
    ) -> Result<PreviewInfo, OpenError> {
        if !self.available() {
            return Err(OpenError::Unavailable);
        }
        let cfg = self.cfg.get();
        let start = u32::from(cfg.preview_port_start);
        let count = u32::from(cfg.preview_port_count);
        if (start..start + count).contains(&u32::from(target)) {
            return Err(OpenError::InvalidPort);
        }
        let _gate = self.open_gate.lock().await;
        let Some(host) = find_target_host(target).await else {
            return Err(OpenError::NothingListening);
        };
        // Reuse an existing preview of the same port for this device.
        let existing = lock(&self.active)
            .iter()
            .find(|p| p.info.device_id == device_id && p.info.target_port == target)
            .cloned();
        if let Some(existing) = existing {
            return Ok(launch_info(&existing, &cfg));
        }
        let used: Vec<u16> = lock(&self.active)
            .iter()
            .map(|p| p.info.listen_port)
            .collect();
        let tls = if proxy_mode(&cfg) {
            None
        } else {
            self.tls.as_ref().map(|t| t.acceptor())
        };
        for port in start..start + count {
            let Ok(port) = u16::try_from(port) else {
                break;
            };
            if used.contains(&port) {
                continue;
            }
            let mut listeners = Vec::with_capacity(self.addrs.len());
            for addr in &self.addrs {
                match TcpListener::bind((addr.as_str(), port)).await {
                    Ok(listener) => listeners.push(listener),
                    Err(_) => break,
                }
            }
            // A failed bind drops the listeners already bound; the next port is tried.
            if listeners.len() != self.addrs.len() {
                continue;
            }
            let (shutdown, _) = watch::channel(false);
            let preview = Arc::new(Active {
                info: PreviewInfo {
                    id: random_token::<9>(),
                    device_id: device_id.to_string(),
                    target_port: target,
                    listen_port: port,
                    created_at: now_ms(),
                    origin: None,
                    launch: None,
                },
                host: host.clone(),
                shutdown,
                tokens: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
            });
            let handler = self.handler(&preview);
            for listener in listeners {
                tokio::spawn(serve(
                    listener,
                    tls.clone(),
                    handler.clone(),
                    preview.shutdown.subscribe(),
                ));
            }
            lock(&self.active).push(Arc::clone(&preview));
            self.diag.event("preview_open", &BTreeMap::new());
            return Ok(launch_info(&preview, &cfg));
        }
        Err(OpenError::NoFreePort)
    }

    pub fn close(&self, id: &str, device_id: &str) -> bool {
        let preview = {
            let mut active = lock(&self.active);
            let Some(index) = active.iter().position(|p| p.info.id == id) else {
                return false;
            };
            if active[index].info.device_id != device_id {
                return false;
            }
            active.remove(index)
        };
        let _ = preview.shutdown.send_replace(true);
        self.diag.event("preview_close", &BTreeMap::new());
        true
    }

    pub fn close_device(&self, device_id: &str) {
        let ids: Vec<String> = lock(&self.active)
            .iter()
            .filter(|p| p.info.device_id == device_id)
            .map(|p| p.info.id.clone())
            .collect();
        for id in ids {
            self.close(&id, device_id);
        }
    }

    pub fn close_all(&self) {
        let all: Vec<(String, String)> = lock(&self.active)
            .iter()
            .map(|p| (p.info.id.clone(), p.info.device_id.clone()))
            .collect();
        for (id, device_id) in all {
            self.close(&id, &device_id);
        }
    }

    fn handler(self: &Arc<Self>, p: &Arc<Active>) -> Handler {
        let this = Arc::clone(self);
        let p = Arc::clone(p);
        Arc::new(
            move |req: Request<Body>, peer: Option<String>| -> HandlerFuture {
                let this = Arc::clone(&this);
                let p = Arc::clone(&p);
                Box::pin(async move { this.forward(&p, req, peer.as_deref()).await })
            },
        )
    }

    fn allowed(&self, p: &Active, headers: &HeaderMap, proxy: bool) -> bool {
        if p.is_closed() {
            return false;
        }
        if proxy {
            return has_session(p, headers);
        }
        self.auth
            .authenticate(header_str(headers, "cookie"))
            .is_some_and(|device| device.id == p.info.device_id)
    }

    fn launch_event(&self, ok: bool) {
        let mut fields = BTreeMap::new();
        fields.insert("ok".to_string(), Value::Bool(ok));
        self.diag.event("preview_launch", &fields);
    }

    /// Proxy mode: redeem a launch token for a preview session, then go to the requested path.
    fn launch(&self, p: &Active, url: &Url, headers: &HeaderMap) -> Resp {
        let query = Query::parse(url.query());
        let next = query.get("path").filter(|s| !s.is_empty()).unwrap_or("/");
        let location = safe_location(next);
        let token = query.get("t").unwrap_or("");
        let now = now_ms();
        // A link is used up by any attempt with its token.
        let exp = if token.is_empty() {
            None
        } else {
            lock(&p.tokens).remove(&sha256(token))
        };
        if exp.is_some_and(|exp| exp > now) {
            let session = random_token::<32>();
            lock(&p.sessions).insert(sha256(&session), now + SESSION_TTL_MS);
            self.launch_event(true);
            return response(
                303,
                Body::empty(),
                &[
                    ("cache-control", NO_STORE.to_string()),
                    ("referrer-policy", "no-referrer".to_string()),
                    ("location", location.to_string()),
                    (
                        "set-cookie",
                        format!(
                            "{PREVIEW_COOKIE}={session}; Path=/; Secure; HttpOnly; SameSite=Strict"
                        ),
                    ),
                ],
            );
        }
        // A reload of a used link still works while this browser holds a session.
        if has_session(p, headers) {
            return response(
                303,
                Body::empty(),
                &[
                    ("cache-control", NO_STORE.to_string()),
                    ("referrer-policy", "no-referrer".to_string()),
                    ("location", location.to_string()),
                ],
            );
        }
        self.launch_event(false);
        response(
            401,
            EXPIRED,
            &[
                ("cache-control", NO_STORE.to_string()),
                ("referrer-policy", "no-referrer".to_string()),
                ("content-type", TEXT_PLAIN.to_string()),
            ],
        )
    }

    async fn forward(&self, p: &Active, req: Request<Body>, peer: Option<&str>) -> Resp {
        let cfg = self.cfg.get();
        let proxy = proxy_mode(&cfg);
        let peer_ok = if proxy {
            is_trusted_proxy(peer, &cfg.trusted_proxies)
        } else {
            is_allowed_peer(peer, &cfg.allowed_networks)
        };
        if !peer_ok {
            return response(403, Body::empty(), &[]);
        }
        let (parts, body) = req.into_parts();
        let Some(url) = request_url(&parts.uri) else {
            return response(500, Body::empty(), &[]);
        };
        let path_query = path_and_query(&url);
        if proxy {
            let host_ok = origin_of(&cfg, p.info.listen_port)
                .as_deref()
                .and_then(origin_host)
                .is_some_and(|want| {
                    header_str(&parts.headers, "host")
                        .is_some_and(|got| got.eq_ignore_ascii_case(&want))
                });
            if !host_ok {
                return response(
                    421,
                    "Unknown host",
                    &[("content-type", "text/plain".to_string())],
                );
            }
            if url.path() == LAUNCH_PATH {
                return self.launch(p, &url, &parts.headers);
            }
        }
        if !self.allowed(p, &parts.headers, proxy) {
            return response(
                401,
                DENIED,
                &[
                    ("content-type", TEXT_PLAIN.to_string()),
                    ("cache-control", NO_STORE.to_string()),
                ],
            );
        }
        let port = p.info.target_port;
        if header_str(&parts.headers, "upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        {
            return forward_upgrade(p, parts, &path_query).await;
        }
        let mut headers = parts.headers.clone();
        for name in HOP {
            headers.remove(name);
        }
        set_header(&mut headers, "host", &format!("localhost:{port}"));
        match strip_briardr_cookie(header_str(&parts.headers, "cookie")) {
            Some(cookie) => set_header(&mut headers, "cookie", &cookie),
            None => {
                headers.remove(header::COOKIE);
            }
        }
        headers.remove(header::ORIGIN);
        headers.remove(header::REFERER);
        let Ok(uri) = format!("http://{}{path_query}", authority(&p.host, port)).parse::<Uri>()
        else {
            return bad_gateway(port);
        };
        let method = parts.method.clone();
        let upstream_body = if method == Method::GET || method == Method::HEAD {
            Body::empty()
        } else {
            body
        };
        let Ok(mut up_req) = Request::builder()
            .method(method)
            .uri(uri)
            .body(upstream_body)
        else {
            return bad_gateway(port);
        };
        *up_req.headers_mut() = headers;
        let mut closed = p.shutdown.subscribe();
        let sent = tokio::select! {
            res = self.client.request(up_req) => res.ok(),
            () = wait_closed(&mut closed) => None,
        };
        let Some(upstream) = sent else {
            return bad_gateway(port);
        };
        let (up_parts, up_body) = upstream.into_parts();
        let mut headers = up_parts.headers;
        for name in HOP {
            headers.remove(name);
        }
        headers.remove("x-frame-options");
        // Allow Briardr to frame the preview; drop any upstream frame-ancestors restriction.
        let csp = headers
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())
            .map(strip_frame_ancestors);
        if let Some(csp) = csp {
            set_header(&mut headers, "content-security-policy", &csp);
        }
        let mut out = Response::new(Body::new(up_body));
        *out.status_mut() = up_parts.status;
        *out.headers_mut() = headers;
        out
    }
}

/// The preview's info with a fresh one-time launch link (proxy mode).
fn launch_info(p: &Active, cfg: &BriardrConfig) -> PreviewInfo {
    let mut info = p.info.clone();
    let Some(origin) = origin_of(cfg, info.listen_port) else {
        return info;
    };
    let now = now_ms();
    let token = random_token::<24>();
    {
        let mut tokens = lock(&p.tokens);
        tokens.retain(|_, exp| *exp >= now);
        tokens.insert(sha256(&token), now + LAUNCH_TTL_MS);
    }
    info.launch = Some(format!("{origin}{LAUNCH_PATH}?t={token}"));
    info.origin = Some(origin);
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::path::PathBuf;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc;

    use crate::config::{default_config, Paths};

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn sendable_close_keeps_only_sendable_codes() {
        assert_eq!(sendable_close(1000), 1000);
        assert_eq!(sendable_close(1001), 1001);
        assert_eq!(sendable_close(3000), 3000);
        assert_eq!(sendable_close(4999), 4999);
        assert_eq!(sendable_close(1005), 1000);
        assert_eq!(sendable_close(1006), 1000);
        assert_eq!(sendable_close(2999), 1000);
        assert_eq!(sendable_close(5000), 1000);
    }

    #[test]
    fn strip_briardr_cookie_removes_only_briardr_cookies() {
        let header = "a=1; __Host-briardr=dev.tok; __Host-briardr-preview=s; b=2;";
        assert_eq!(
            strip_briardr_cookie(Some(header)).as_deref(),
            Some("a=1; b=2")
        );
        assert_eq!(strip_briardr_cookie(Some("__Host-briardr=x")), None);
        assert_eq!(strip_briardr_cookie(Some("")), None);
        assert_eq!(strip_briardr_cookie(None), None);
    }

    #[test]
    fn frame_ancestors_are_removed_case_insensitively() {
        assert_eq!(
            strip_frame_ancestors("default-src 'self'; Frame-Ancestors 'none'; img-src *"),
            "default-src 'self';  img-src *"
        );
        assert_eq!(strip_frame_ancestors("frame-ancestors 'self'"), "");
        assert_eq!(
            strip_frame_ancestors("script-src 'self'"),
            "script-src 'self'"
        );
    }

    #[test]
    fn launch_redirect_stays_on_the_origin() {
        assert_eq!(safe_location("/app?x=1"), "/app?x=1");
        assert_eq!(safe_location("//evil.example"), "/");
        assert_eq!(safe_location("/\\evil.example"), "/");
        assert_eq!(safe_location("https://evil.example"), "/");
    }

    #[test]
    fn origin_host_includes_non_default_port() {
        assert_eq!(
            origin_host("https://P3.briardr.example.com").as_deref(),
            Some("p3.briardr.example.com")
        );
        assert_eq!(
            origin_host("https://p3.example.com:8443").as_deref(),
            Some("p3.example.com:8443")
        );
        assert_eq!(origin_host("not a url"), None);
    }

    #[test]
    fn open_errors_have_wire_codes() {
        assert_eq!(OpenError::NothingListening.code(), "nothing_listening");
        assert_eq!(OpenError::NoFreePort.code(), "no_free_port");
        assert_eq!(OpenError::InvalidPort.code(), "invalid_port");
        assert_eq!(OpenError::Unavailable.code(), "unavailable");
    }

    #[test]
    fn preview_info_serializes_camel_case_and_omits_empty_links() -> TestResult {
        let info = PreviewInfo {
            id: "p1".into(),
            device_id: "d1".into(),
            target_port: 3000,
            listen_port: 4001,
            created_at: 5,
            origin: None,
            launch: None,
        };
        let value = serde_json::to_value(&info)?;
        assert_eq!(
            value,
            serde_json::json!({"id": "p1", "deviceId": "d1", "targetPort": 3000, "listenPort": 4001, "createdAt": 5})
        );
        Ok(())
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "briardr-preview-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    async fn free_port() -> Result<u16, Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        Ok(listener.local_addr()?.port())
    }

    /// Sends one raw request on a fresh connection and reads the reply until the server closes it.
    async fn raw_request(port: u16, request: &str) -> Result<String, Box<dyn Error>> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream.write_all(request.as_bytes()).await?;
        let mut out = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await??;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    #[tokio::test]
    async fn direct_preview_forwards_for_its_device_only() -> TestResult {
        let dir = temp_dir("direct");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("devices.json"),
            format!(
                r#"[{{"id":"dev1","name":"test","tokenHash":"{}","createdAt":1,"lastSeenAt":null}}]"#,
                sha256("secret")
            ),
        )?;

        // A stand-in dev server: records each request head and answers with a fixed reply.
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let target = upstream.local_addr()?.port();
        let (heads_tx, mut heads_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Ok((mut conn, _)) = upstream.accept().await {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    match conn.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                // The port probe connects and closes without sending a request; skip those.
                if head.is_empty() {
                    continue;
                }
                let _ = heads_tx.send(String::from_utf8_lossy(&head).into_owned());
                let reply = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-Frame-Options: DENY\r\n\
                    Content-Security-Policy: default-src 'self'; frame-ancestors 'none'\r\n\
                    Connection: close\r\n\r\nok";
                let _ = conn.write_all(reply.as_bytes()).await;
                let _ = conn.shutdown().await;
            }
        });

        let mut cfg = default_config(&HashMap::new(), &dir);
        cfg.tls.mode = "local-ca".to_string();
        cfg.preview_port_count = 1;
        cfg.preview_port_start = free_port().await?;
        let listen = cfg.preview_port_start;
        let paths = Paths::new(&dir);
        let auth = Arc::new(Auth::new(
            dir.join("devices.json"),
            dir.join("pairing.json"),
        ));
        let diag = Arc::new(Diagnostics::new(dir.join("diag"), dir.join("diag-salt"))?);
        let previews = Previews::new(
            Arc::new(ConfigCell::new(cfg, paths)),
            None,
            vec!["127.0.0.1".to_string()],
            auth,
            diag,
        );

        let info = previews.open("dev1", target).await.map_err(|e| e.code())?;
        assert_eq!(info.listen_port, listen);
        assert_eq!(info.origin, None);
        assert_eq!(previews.list("dev1").len(), 1);
        assert!(previews.list("dev2").is_empty());

        let host = format!("Host: 127.0.0.1:{listen}\r\n");
        let denied = raw_request(
            listen,
            &format!("GET /a HTTP/1.1\r\n{host}Connection: close\r\n\r\n"),
        )
        .await?;
        assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
        assert!(denied.contains("Open this preview from Briardr."));

        let allowed = format!(
            "GET /a?b=1 HTTP/1.1\r\n{host}Cookie: __Host-briardr=dev1.secret; other=1\r\nOrigin: http://x\r\nConnection: close\r\n\r\n"
        );
        let reply = raw_request(listen, &allowed).await?;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(reply.ends_with("ok"), "{reply}");
        assert!(!reply.to_ascii_lowercase().contains("x-frame-options"));
        assert!(!reply.to_ascii_lowercase().contains("frame-ancestors"));
        let head = tokio::time::timeout(Duration::from_secs(5), heads_rx.recv())
            .await?
            .ok_or("no upstream request")?;
        let lower = head.to_ascii_lowercase();
        assert!(head.starts_with("GET /a?b=1 HTTP/1.1"), "{head}");
        assert!(
            lower.contains(&format!("host: localhost:{target}")),
            "{head}"
        );
        assert!(lower.contains("cookie: other=1\r\n"), "{head}");
        assert!(!lower.contains("__host-briardr"), "{head}");
        assert!(!lower.contains("origin:"), "{head}");

        assert!(!previews.close(&info.id, "dev2"));
        assert!(previews.close(&info.id, "dev1"));
        let mut refused = false;
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", listen)).await.is_err() {
                refused = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(refused, "closed preview still accepts connections");
        assert!(previews.list("dev1").is_empty());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
