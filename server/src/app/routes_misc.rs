// Routes for previews, usage, notifications, power, settings and diagnostics.
// The certificate profile is with the session routes.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{
    action_err, caps_json, js_number, js_truthy, json_body, lock, now_ms, ok, response, send,
    str_or_empty, ApiError, App, Ctx, Routes, JSON_LIMIT, VERSION,
};
use crate::herdr::rpc::{herdr_status, HerdrTarget};
use crate::preview::{OpenError, PreviewInfo};

/// Runs blocking work (file writes, shelling out, locks) off the async runtime.
async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::Internal)
}

/// JS `Number(v)` for a JSON body field; a missing field is NaN.
fn js_number_of(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) | Some(Value::Bool(false)) => 0.0,
        Some(Value::Bool(true)) => 1.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_number(s),
        Some(_) => f64::NAN,
    }
}

/// A boolean field of a JSON object, if it is one.
fn bool_field(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

/// A string at a path in a JSON body, or "" when absent (the subscription check refuses "").
fn str_at(v: Option<&Value>, path: &[&str]) -> String {
    let mut cur = v;
    for key in path {
        cur = cur.and_then(|c| c.get(*key));
    }
    cur.and_then(Value::as_str).unwrap_or("").to_string()
}

fn preview_json(p: &PreviewInfo) -> Value {
    let mut v = json!({
        "id": p.id,
        "deviceId": p.device_id,
        "targetPort": p.target_port,
        "listenPort": p.listen_port,
        "createdAt": p.created_at,
    });
    if let Value::Object(map) = &mut v {
        if let Some(origin) = &p.origin {
            map.insert("origin".into(), json!(origin));
        }
        if let Some(launch) = &p.launch {
            map.insert("launch".into(), json!(launch));
        }
    }
    v
}

pub(super) fn register(r: &mut Routes) {
    // --- preview ---
    r.add(
        Method::POST,
        "/api/previews",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            let device = ctx.device()?.id.clone();
            if !app.d.previews.available() {
                return Err(app.unsupported("Preview (no preview origins are configured)"));
            }
            let port = js_number_of(b.get("port"));
            let invalid_port = || {
                Ok(send(
                    400,
                    &json!({
                        "error": "invalid_port",
                        "message": "That isn’t a port Briardr can preview.",
                    }),
                    &[],
                ))
            };
            // Only whole ports from 1 to 65535 reach the previews; anything else is refused here.
            if port.fract() != 0.0 || !(1.0..=65535.0).contains(&port) {
                return invalid_port();
            }
            let target = port as u16;
            match app.d.previews.open(&device, target).await {
                Ok(preview) => ok(json!({"preview": preview_json(&preview)})),
                Err(OpenError::Unavailable) => {
                    Err(app.unsupported("Preview (no preview origins are configured)"))
                }
                Err(OpenError::NothingListening) => Ok(send(
                    404,
                    &json!({
                        "error": "nothing_listening",
                        "message": format!("Nothing is listening on port {target}."),
                    }),
                    &[],
                )),
                Err(OpenError::InvalidPort) => invalid_port(),
                Err(OpenError::NoFreePort) => Ok(send(
                    400,
                    &json!({
                        "error": "no_free_port",
                        "message": "Too many previews are open. Close one first.",
                    }),
                    &[],
                )),
            }
        },
    );

    r.add(
        Method::DELETE,
        "/api/previews/:id",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            let device = ctx.device()?.id.clone();
            let previews = app.d.previews.clone();
            let closed = blocking(move || previews.close(&id, &device)).await?;
            ok(json!({"ok": closed}))
        },
    );

    // --- usage ---
    r.add(
        Method::GET,
        "/api/usage",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let force = ctx.query.get("force") == Some("1");
            let usage = app.d.usage.refresh(force).await;
            ok(json!({"usage": usage, "now": now_ms()}))
        },
    );

    r.add(
        Method::POST,
        "/api/usage/codex/reset-credit",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, 1024).await?;
            if bool_field(&b, "confirm") != Some(true) {
                return Err(action_err(
                    400,
                    "confirm_required",
                    "Confirm to use a reset.",
                ));
            }
            match app.d.usage.use_codex_reset_credit().await {
                Ok(outcome) => ok(json!({
                    "outcome": outcome.as_str(),
                    "usage": app.d.usage.snapshot(),
                })),
                Err(e) => {
                    let status = match e.category.as_str() {
                        "not_enabled" => 409,
                        "rate_limited" => 429,
                        _ => 502,
                    };
                    Err(action_err(status, &e.category, e.message))
                }
            }
        },
    );

    r.add(
        Method::GET,
        "/api/usage/statusline",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            let usage = app.d.usage.clone();
            let (installed, preview) = blocking(move || {
                let installed = usage.tap.installed();
                // A preview that can't be built is reported as null, not as an error.
                let preview = usage
                    .tap
                    .preview()
                    .ok()
                    .and_then(|p| serde_json::to_value(p).ok())
                    .unwrap_or(Value::Null);
                (installed, preview)
            })
            .await?;
            ok(json!({"installed": installed, "preview": preview}))
        },
    );

    r.add(
        Method::POST,
        "/api/usage/statusline",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            let usage = app.d.usage.clone();
            let failed = |e: crate::usage::claude::TapError| {
                action_err(409, "statusline_failed", e.to_string())
            };
            match b.get("action").and_then(Value::as_str) {
                Some("install") => {
                    let fingerprint = str_or_empty(b.get("fingerprint"));
                    blocking(move || usage.tap.install(&fingerprint))
                        .await?
                        .map_err(failed)?;
                    ok(json!({"ok": true}))
                }
                Some("remove") => {
                    let restored = blocking(move || usage.tap.remove())
                        .await?
                        .map_err(failed)?;
                    ok(json!({"ok": true, "restored": restored}))
                }
                _ => Err(action_err(400, "invalid", "Unknown action")),
            }
        },
    );

    // --- notifications ---
    r.add(
        Method::GET,
        "/api/notifications",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let device = ctx.device()?.id.clone();
            ok(json!({
                "publicKey": app.d.push.public_key(),
                "subscribed": app.d.push.has_device(&device),
                "endpoint": app.d.push.device_endpoint(&device),
                "settings": app.cfg().notifications,
            }))
        },
    );

    r.add(
        Method::POST,
        "/api/notifications/subscribe",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let device = ctx.device()?.id.clone();
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            let sub = b.get("subscription");
            let endpoint = str_at(sub, &["endpoint"]);
            let p256dh = str_at(sub, &["keys", "p256dh"]);
            let auth = str_at(sub, &["keys", "auth"]);
            let push = app.d.push.clone();
            blocking(move || push.subscribe(&device, &endpoint, &p256dh, &auth))
                .await?
                .map_err(|_| {
                    action_err(
                        400,
                        "invalid_subscription",
                        "The browser returned an invalid subscription.",
                    )
                })?;
            ok(json!({"ok": true}))
        },
    );

    r.add(
        Method::DELETE,
        "/api/notifications/subscribe",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let device = ctx.device()?.id.clone();
            let push = app.d.push.clone();
            blocking(move || push.unsubscribe_device(&device))
                .await?
                .map_err(|_| ApiError::Internal)?;
            ok(json!({"ok": true}))
        },
    );

    r.add(
        Method::POST,
        "/api/notifications/test",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let device = ctx.device()?.id.clone();
            if !app.d.push.has_device(&device) {
                return Err(action_err(
                    409,
                    "not_subscribed",
                    "Turn on notifications on this device first.",
                ));
            }
            let body = format!("Notifications from your {} are working.", app.d.caps.noun);
            let records = app
                .d
                .push
                .send(
                    "test",
                    "Briardr test",
                    &body,
                    None,
                    "test",
                    Some(&device),
                    300,
                )
                .await;
            let mut out = json!({});
            if let (Some(rec), Some(map)) = (records.first(), out.as_object_mut()) {
                let value = serde_json::to_value(rec).map_err(|_| ApiError::Internal)?;
                map.insert("delivery".into(), value);
            }
            ok(out)
        },
    );

    r.add(
        Method::GET,
        "/api/notifications/deliveries/:id",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            let push = app.d.push.clone();
            // `delivery(id)` is null when unknown, so the key is always there.
            let record = blocking(move || push.delivery(&id)).await?;
            ok(json!({ "delivery": record }))
        },
    );

    r.add(
        Method::POST,
        "/api/notifications/ack",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let device = ctx.device()?.id.clone();
            let b = json_body(&mut ctx, 4096).await?;
            let id = str_or_empty(b.get("id"));
            let push = app.d.push.clone();
            let acked = blocking(move || push.ack(&id, &device)).await?;
            ok(json!({"ok": acked}))
        },
    );

    // --- power ---
    r.add(
        Method::POST,
        "/api/power/sleep",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            if !app.d.caps.sleep {
                return Err(app.unsupported("Sleep"));
            }
            let b = json_body(&mut ctx, 4096).await?;
            let request_id = str_or_empty(b.get("requestId"));
            let power = app.d.power.clone();
            let now = now_ms();
            let body = blocking(move || lock(&power).request_sleep(&request_id, now)).await?;
            let done = body.get("ok").and_then(Value::as_bool) == Some(true);
            Ok(send(if done { 202 } else { 409 }, &body, &[]))
        },
    );

    r.add(
        Method::POST,
        "/api/power/keep-awake",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            if !app.d.caps.keep_awake {
                return Err(app.unsupported("Always on"));
            }
            let b = json_body(&mut ctx, 4096).await?;
            let on = b.get("on").is_some_and(js_truthy);
            let _change = app.d.cfg.lock_changes().await;
            let mut cfg = app.cfg();
            cfg.power.keep_awake = on;
            let cell = app.d.cfg.clone();
            blocking(move || cell.set(cfg)).await??;
            let power = app.d.power.clone();
            let now = now_ms();
            blocking(move || lock(&power).set_keep_awake(on, now)).await?;
            let mac = app.poll_power().await?;
            ok(json!({"mac": mac, "host": caps_json(&app.d.caps)}))
        },
    );

    // --- settings ---
    r.add(
        Method::GET,
        "/api/settings",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            let cfg = app.cfg();
            let st = herdr_status(&HerdrTarget::of(&cfg)).await;
            let (wanted, effective) = app
                .d
                .tls_mode
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let dictation = app
                .d
                .dictation_helper
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .is_some();
            ok(json!({
                "herdrPath": cfg.herdr_path,
                "herdrSession": cfg.herdr_session,
                "herdr": {"running": st.running, "version": st.version, "problem": st.problem},
                "notifications": cfg.notifications,
                "usage": {
                    "claudeKeychainOptIn": app.d.caps.claude_keychain && cfg.usage.claude_keychain_opt_in,
                    "codexEnabled": cfg.usage.codex_enabled,
                },
                "power": cfg.power,
                "tls": {"wanted": wanted, "effective": effective},
                "caProfileAvailable": app.d.ca_profile.is_some(),
                "previews": app.d.previews.available(),
                "dictation": dictation,
                "host": caps_json(&app.d.caps),
                "publicOrigin": cfg.public_origin.filter(|s| !s.is_empty()),
                "version": VERSION,
            }))
        },
    );

    r.add(
        Method::PUT,
        "/api/settings",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            let _change = app.d.cfg.lock_changes().await;
            let mut cfg = app.cfg();
            let mut retarget = None;
            if let Some(raw) = b.get("herdrPath").and_then(Value::as_str) {
                let path = raw.trim().to_string();
                if !Path::new(&path).is_absolute() {
                    return Err(action_err(
                        400,
                        "invalid_path",
                        "Use the full path to the herdr executable.",
                    ));
                }
                let target = HerdrTarget {
                    herdr_path: path.clone(),
                    ..HerdrTarget::of(&cfg)
                };
                let st = herdr_status(&target).await;
                if st.problem.as_deref() == Some("not_found") {
                    return Err(action_err(400, "not_found", "No executable at that path."));
                }
                cfg.herdr_path = path;
                retarget = Some(target);
            }
            if let Some(n) = b.get("notifications").filter(|v| v.is_object()) {
                if bool_field(n, "lowBattery") == Some(true) && !app.d.caps.battery {
                    return Err(app.unsupported("Low-battery notifications"));
                }
                if let Some(v) = bool_field(n, "includeNames") {
                    cfg.notifications.include_names = v;
                }
                if let Some(v) = bool_field(n, "needsYou") {
                    cfg.notifications.needs_you = v;
                }
                if let Some(v) = bool_field(n, "finished") {
                    cfg.notifications.finished = v;
                }
                if let Some(v) = bool_field(n, "ready") {
                    cfg.notifications.ready = v;
                }
                if let Some(v) = bool_field(n, "lowBattery") {
                    cfg.notifications.low_battery = v;
                }
            }
            if let Some(u) = b.get("usage").filter(|v| v.is_object()) {
                if bool_field(u, "claudeKeychainOptIn") == Some(true) && !app.d.caps.claude_keychain
                {
                    return Err(app.unsupported("Reading Claude Code’s sign-in from the Keychain"));
                }
                if let Some(v) = bool_field(u, "claudeKeychainOptIn") {
                    cfg.usage.claude_keychain_opt_in = v;
                }
                if let Some(v) = bool_field(u, "codexEnabled") {
                    cfg.usage.codex_enabled = v;
                }
            }
            let cell = app.d.cfg.clone();
            blocking(move || cell.set(cfg)).await??;
            if let Some(target) = retarget {
                app.d.herdr.retarget(target).await;
            }
            ok(json!({"ok": true}))
        },
    );

    // --- diagnostics ---
    r.add(
        Method::POST,
        "/api/diagnostics/client",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, 256 * 1024).await?;
            // Only the first 500 events, and only those with a string `ev`, are recorded.
            let events: Vec<(String, BTreeMap<String, Value>)> = b
                .get("events")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .take(500)
                        .filter_map(|e| {
                            let obj = e.as_object()?;
                            let ev = obj.get("ev")?.as_str()?.to_string();
                            let fields = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            Some((ev, fields))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let diag = app.d.diag.clone();
            blocking(move || {
                for (ev, fields) in &events {
                    diag.record("client", ev, fields);
                }
            })
            .await?;
            ok(json!({"ok": true}))
        },
    );

    r.add(
        Method::GET,
        "/api/diagnostics",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            let diag = app.d.diag.clone();
            let text = blocking(move || diag.export()).await?;
            Ok(response(
                200,
                text,
                &[
                    (
                        "content-type",
                        "application/x-ndjson; charset=utf-8".to_string(),
                    ),
                    ("cache-control", "no-store".to_string()),
                    (
                        "content-disposition",
                        format!(
                            "attachment; filename=\"briardr-{}-diagnostics.ndjson\"",
                            app.d.caps.noun.to_lowercase()
                        ),
                    ),
                ],
            ))
        },
    );
}
