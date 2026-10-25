// Loopback-only test endpoints. Compiled only with `--features test-hooks`, and enabled at runtime
// only with BRIARDR_TEST_HOOKS=1. They stand in for the in-process access the tests have (`s.run.*`):
//   POST /__test/auth/pair {code, name, ip?}  → the `auth.pair` result ({ok, device, cookie} | {ok, error})
//   POST /__test/auth/revoke {id}             → true | false
//   POST /__test/herdr/refresh                → {ok}
//   GET  /__test/routes                       → [{method, pattern, public}]
//   GET  /__test/hub/sent                     → [{conn, method}] (only the length is meaningful)
//   POST /__test/push/capture {status}        → pushes are recorded instead of sent, answered with `status`
//   GET  /__test/push/sent                    → [plaintext payload JSON, …]
//   GET  /__test/push/subject                 → the VAPID subject
//   POST /__test/previews/open {device, port} → the `previews.open` result
//   GET  /__test/urls                         → the listen URLs
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::{Method, Request};
use serde_json::{json, Value};

use crate::app::{read_body, send, App, Resp};
use crate::push::{PushSub, SendFuture, SendOpts, Sender};

pub fn enabled() -> bool {
    std::env::var("BRIARDR_TEST_HOOKS")
        .map(|v| v == "1")
        .unwrap_or(false)
}

static URLS: OnceLock<Vec<String>> = OnceLock::new();
static PUSHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn set_urls(urls: Vec<String>) {
    let _ = URLS.set(urls);
}

fn is_loopback(peer: Option<&str>) -> bool {
    peer.and_then(|p| p.parse::<std::net::IpAddr>().ok())
        .is_some_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            }
        })
}

/// Whether this request is a test hook call: hooks enabled, a loopback peer and a `/__test/` path.
pub fn wants(req: &Request<Body>, peer: Option<&str>) -> bool {
    enabled() && is_loopback(peer) && req.uri().path().starts_with("/__test/")
}

/// Answers a request `wants` accepted.
pub async fn handle(app: &Arc<App>, req: Request<Body>) -> Resp {
    let method = req.method().clone();
    let path = req.uri().path().trim_start_matches("/__test").to_string();
    let (parts, body) = req.into_parts();
    let body: Value = match read_body(&parts.headers, body, 64 * 1024).await {
        Ok(b) if !b.is_empty() => serde_json::from_slice(&b).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let s = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let out = match (method, path.as_str()) {
        (Method::POST, "/auth/pair") => {
            let (code, name) = (s("code"), s("name"));
            let client = body
                .get("ip")
                .and_then(Value::as_str)
                .unwrap_or("local")
                .to_string();
            let auth = app.d.auth.clone();
            let r = tokio::task::spawn_blocking(move || auth.pair(&code, &name, &client))
                .await
                .ok();
            match r {
                Some(Ok((device, cookie))) => {
                    json!({"ok": true, "device": device, "cookie": cookie})
                }
                Some(Err(e)) => {
                    let error = match e {
                        crate::auth::PairError::NoCode => "no_code",
                        crate::auth::PairError::Expired => "expired",
                        crate::auth::PairError::Invalid => "invalid",
                        crate::auth::PairError::Locked => "locked",
                    };
                    json!({"ok": false, "error": error})
                }
                None => json!({"ok": false, "error": "invalid"}),
            }
        }
        (Method::POST, "/auth/revoke") => {
            let (auth, id) = (app.d.auth.clone(), s("id"));
            json!(tokio::task::spawn_blocking(move || auth.revoke(&id))
                .await
                .unwrap_or(false))
        }
        (Method::POST, "/herdr/refresh") => json!({"ok": app.d.herdr.refresh().await.is_ok()}),
        (Method::GET, "/routes") => json!(app.list_routes()),
        (Method::GET, "/hub/controllers") => json!(app.d.hub.controller_count()),
        (Method::GET, "/hub/sent") => {
            json!((0..app.d.hub.sent_count())
                .map(|_| json!({}))
                .collect::<Vec<_>>())
        }
        (Method::POST, "/push/capture") => {
            let status = body.get("status").and_then(Value::as_u64).unwrap_or(201) as u16;
            let sender: Sender = Arc::new(
                move |_sub: PushSub, plaintext: String, _opts: SendOpts| -> SendFuture {
                    PUSHED
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(plaintext);
                    Box::pin(async move {
                        if (200..300).contains(&status) {
                            Ok(status)
                        } else {
                            Err(Some(status))
                        }
                    })
                },
            );
            app.d.push.set_sender(sender);
            json!({"ok": true})
        }
        (Method::GET, "/push/sent") => json!(*PUSHED.lock().unwrap_or_else(|e| e.into_inner())),
        (Method::GET, "/push/subject") => json!(app.d.push.subject()),
        (Method::POST, "/previews/open") => {
            // The `previews.open` result: {ok: true, preview} | {ok: false, error}.
            let port = body.get("port").and_then(Value::as_u64).unwrap_or(0);
            match u16::try_from(port) {
                Ok(port) if port > 0 => match app.d.previews.open(&s("device"), port).await {
                    Ok(preview) => json!({"ok": true, "preview": preview}),
                    Err(e) => json!({"ok": false, "error": e.code()}),
                },
                _ => json!({"ok": false, "error": "invalid_port"}),
            }
        }
        (Method::GET, "/urls") => json!(URLS.get().cloned().unwrap_or_default()),
        _ => return send(404, &json!({"error": "not_found"}), &[]),
    };
    send(200, &out, &[])
}
