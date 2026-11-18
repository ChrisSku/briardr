// Routes for previews, usage, notifications, power, settings and diagnostics.
// The certificate profile is with the session routes.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use axum::http::Method;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    action_err, caps_json, json_body_as, lock, now_ms, ok, response, send, ApiError, App, Ctx,
    Routes, JSON_LIMIT, VERSION,
};
use crate::config::BriardrConfig;
use crate::herdr::backend::Target;
use crate::herdr::core::state::CoreTarget;
use crate::herdr::rpc::{herdr_status, HerdrTarget};
use crate::preview::{OpenError, PreviewInfo};
use crate::push::PrefsChange;

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

/// `POST /api/previews`: the port on the host to preview. Any number reaches the port check.
#[derive(Deserialize)]
struct PreviewOpen {
    port: Option<f64>,
}

/// `POST /api/usage/codex/reset-credit`: `confirm: true`, so a stray request can't spend a reset.
#[derive(Deserialize)]
struct ResetCredit {
    confirm: Option<bool>,
}

/// `POST /api/usage/statusline`: put Briardr's status line into Claude Code's settings (the one whose
/// preview the user saw: its fingerprint), or take it out again.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum StatuslineChange {
    Install { fingerprint: Option<String> },
    Remove,
}

/// `POST /api/notifications/subscribe`: the browser's `PushSubscription.toJSON()`. A part left out is an
/// invalid subscription.
#[derive(Default, Deserialize)]
struct SubscribeBody {
    subscription: Option<BrowserSubscription>,
}

#[derive(Default, Deserialize)]
struct BrowserSubscription {
    endpoint: Option<String>,
    keys: Option<BrowserKeys>,
}

#[derive(Default, Deserialize)]
struct BrowserKeys {
    p256dh: Option<String>,
    auth: Option<String>,
}

/// `POST /api/notifications/ack`: the alert the service worker showed.
#[derive(Deserialize)]
struct AckBody {
    id: Option<String>,
}

/// `POST /api/power/sleep`: the client's own ID for the request, so a retry isn't a second sleep.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SleepBody {
    request_id: Option<String>,
}

/// `POST /api/power/keep-awake`: Always on, on or off (left out: off).
#[derive(Deserialize)]
struct KeepAwakeBody {
    on: Option<bool>,
}

/// `POST /api/update/install`: the version the user chose, which must still be the latest.
#[derive(Deserialize)]
struct InstallBody {
    version: Option<String>,
}

/// `POST /api/diagnostics/client`: the device's own diagnostics events, each with its name in `ev`.
/// Their fields are the client's; only those on the allow-list are recorded (`Diagnostics::record`).
#[derive(Deserialize)]
struct ClientEvents {
    #[serde(default)]
    events: Vec<BTreeMap<String, Value>>,
}

/// What `PUT /api/settings` changes. A setting left out (or `null`) stays as it is.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SettingsChange {
    herdr_backend: Option<String>,
    herdr_path: Option<String>,
    notifications: Option<NotificationsChange>,
    usage: Option<UsageChange>,
    updates: Option<UpdatesChange>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct NotificationsChange {
    include_names: Option<bool>,
    needs_you: Option<bool>,
    finished: Option<bool>,
    ready: Option<bool>,
    low_battery: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct UsageChange {
    claude_keychain_opt_in: Option<bool>,
    codex_enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UpdatesChange {
    check: Option<bool>,
}

impl SettingsChange {
    /// Made both to config.json and to the running configuration (`ConfigCell::change`).
    fn apply(&self, cfg: &mut BriardrConfig) {
        fn set<T: Clone>(setting: &mut T, change: &Option<T>) {
            if let Some(value) = change {
                *setting = value.clone();
            }
        }
        set(&mut cfg.herdr_backend, &self.herdr_backend);
        set(&mut cfg.herdr_path, &self.herdr_path);
        if let Some(n) = &self.notifications {
            set(&mut cfg.notifications.include_names, &n.include_names);
            set(&mut cfg.notifications.needs_you, &n.needs_you);
            set(&mut cfg.notifications.finished, &n.finished);
            set(&mut cfg.notifications.ready, &n.ready);
            set(&mut cfg.notifications.low_battery, &n.low_battery);
        }
        if let Some(u) = &self.usage {
            set(
                &mut cfg.usage.claude_keychain_opt_in,
                &u.claude_keychain_opt_in,
            );
            set(&mut cfg.usage.codex_enabled, &u.codex_enabled);
        }
        if let Some(u) = &self.updates {
            set(&mut cfg.updates.check, &u.check);
        }
    }
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
            let b: PreviewOpen = json_body_as(&mut ctx, JSON_LIMIT).await?;
            let device = ctx.device()?.id.clone();
            if !app.d.previews.available() {
                return Err(app.unsupported("Preview (no preview origins are configured)"));
            }
            let port = b.port.unwrap_or(f64::NAN);
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
            let b: ResetCredit = json_body_as(&mut ctx, 1024).await?;
            if b.confirm != Some(true) {
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
            let change: StatuslineChange = json_body_as(&mut ctx, JSON_LIMIT).await?;
            let usage = app.d.usage.clone();
            let failed = |e: crate::usage::claude::TapError| {
                action_err(409, "statusline_failed", e.to_string())
            };
            match change {
                StatuslineChange::Install { fingerprint } => {
                    let fingerprint = fingerprint.unwrap_or_default();
                    blocking(move || usage.tap.install(&fingerprint))
                        .await?
                        .map_err(failed)?;
                    ok(json!({"ok": true}))
                }
                StatuslineChange::Remove => {
                    let restored = blocking(move || usage.tap.remove())
                        .await?
                        .map_err(failed)?;
                    ok(json!({"ok": true, "restored": restored}))
                }
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
                "prefs": app.d.push.prefs(&device),
            }))
        },
    );

    // A change to what this device wants of its alerts (push::PrefsChange): true or false is its own
    // choice, null goes back to the host's setting; `quiet` is its quiet hours or null. Fields left out
    // stay as they are.
    r.add(
        Method::PUT,
        "/api/notifications/device",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let device = ctx.device()?.id.clone();
            let change: PrefsChange = json_body_as(&mut ctx, JSON_LIMIT).await?;
            if !change.valid() {
                return Err(action_err(
                    400,
                    "invalid",
                    "Those alert settings aren’t valid.",
                ));
            }
            let push = app.d.push.clone();
            let prefs = blocking(move || push.change_prefs(&device, change))
                .await?
                .map_err(|_| ApiError::Internal)?;
            ok(json!({ "prefs": prefs }))
        },
    );

    r.add(
        Method::POST,
        "/api/notifications/subscribe",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let device = ctx.device()?.id.clone();
            let b: SubscribeBody = json_body_as(&mut ctx, JSON_LIMIT).await?;
            let sub = b.subscription.unwrap_or_default();
            let keys = sub.keys.unwrap_or_default();
            let endpoint = sub.endpoint.unwrap_or_default();
            let (p256dh, auth) = (
                keys.p256dh.unwrap_or_default(),
                keys.auth.unwrap_or_default(),
            );
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
            let b: AckBody = json_body_as(&mut ctx, 4096).await?;
            let id = b.id.unwrap_or_default();
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
            let b: SleepBody = json_body_as(&mut ctx, 4096).await?;
            let request_id = b.request_id.unwrap_or_default();
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
            let b: KeepAwakeBody = json_body_as(&mut ctx, 4096).await?;
            let on = b.on.unwrap_or(false);
            let _change = app.d.cfg.lock_changes().await;
            let cell = app.d.cfg.clone();
            blocking(move || cell.change(|c| c.power.keep_awake = on)).await??;
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
            let st = Target::of(&cfg).status().await;
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
                "herdrBackend": cfg.herdr_backend,
                "herdrPath": cfg.herdr_path,
                "herdrSession": cfg.herdr_session,
                "herdrCoreSocket": CoreTarget::of(&cfg).socket.display().to_string(),
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
            let mut change: SettingsChange = json_body_as(&mut ctx, JSON_LIMIT).await?;
            let _change = app.d.cfg.lock_changes().await;
            let cfg = app.cfg();
            let before = Target::of(&cfg);
            if change
                .herdr_backend
                .as_deref()
                .is_some_and(|b| !matches!(b, "tui" | "core"))
            {
                return Err(action_err(
                    400,
                    "invalid_backend",
                    "Choose Herdr TUI or herdr-core.",
                ));
            }
            if let Some(path) = &mut change.herdr_path {
                *path = path.trim().to_string();
                if !Path::new(path).is_absolute() {
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
            }
            let low_battery = change.notifications.as_ref().and_then(|n| n.low_battery);
            if low_battery == Some(true) && !app.d.caps.battery {
                return Err(app.unsupported("Low-battery notifications"));
            }
            let keychain = change.usage.as_ref().and_then(|u| u.claude_keychain_opt_in);
            if keychain == Some(true) && !app.d.caps.claude_keychain {
                return Err(app.unsupported("Reading Claude Code’s sign-in from the Keychain"));
            }
            let cell = app.d.cfg.clone();
            let after = blocking(move || cell.change(|c| change.apply(c))).await??;
            let after = Target::of(&after);
            if after != before {
                app.d.herdr.retarget(after).await;
                // Open streams are the other Herdr's: their screens are told they're gone.
                app.d.hub.close_all();
            }
            ok(json!({"ok": true}))
        },
    );

    // --- updates ---
    r.add(
        Method::GET,
        "/api/update",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            let updates = app.d.updates.clone();
            let check = app.cfg().updates.check;
            ok(blocking(move || updates.status(check)).await?)
        },
    );

    r.add(
        Method::POST,
        "/api/update/check",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            // A failed check is in the status, with what went wrong.
            let _ = app.d.updates.check().await;
            let updates = app.d.updates.clone();
            let check = app.cfg().updates.check;
            ok(blocking(move || updates.status(check)).await?)
        },
    );

    r.add(
        Method::POST,
        "/api/update/install",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b: InstallBody = json_body_as(&mut ctx, JSON_LIMIT).await?;
            let version = b.version.unwrap_or_default();
            match app.d.updates.install(&version).await {
                Ok(restarts) => {
                    app.event("update_installed", json!({"restarts": restarts}));
                    ok(json!({"ok": true, "version": version, "restarts": restarts}))
                }
                Err(message) => {
                    app.event("update_failed", json!({}));
                    Err(action_err(409, "update_failed", message))
                }
            }
        },
    );

    // --- diagnostics ---
    r.add(
        Method::POST,
        "/api/diagnostics/client",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b: ClientEvents = json_body_as(&mut ctx, 256 * 1024).await?;
            let diag = app.d.diag.clone();
            blocking(move || {
                // Only the first 500 events, and only those with a string `ev`, are recorded.
                for fields in b.events.iter().take(500) {
                    if let Some(ev) = fields.get("ev").and_then(Value::as_str) {
                        diag.record("client", ev, fields);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Option<T> {
        serde_json::from_value(v).ok()
    }

    #[test]
    fn bodies_take_what_the_client_sends_and_refuse_other_types() {
        let b: PreviewOpen = parse(json!({"port": 5173})).unwrap();
        assert_eq!(b.port, Some(5173.0));
        let b: PreviewOpen = parse(json!({"port": 1.5})).unwrap();
        assert_eq!(
            b.port,
            Some(1.5),
            "the port check refuses it, as invalid_port"
        );
        assert!(parse::<PreviewOpen>(json!({"port": "5173"})).is_none());

        let install = parse(json!({"action": "install", "fingerprint": "abc"}));
        assert!(
            matches!(install, Some(StatuslineChange::Install { fingerprint: Some(f) }) if f == "abc")
        );
        assert!(matches!(
            parse(json!({"action": "remove"})),
            Some(StatuslineChange::Remove)
        ));
        assert!(parse::<StatuslineChange>(json!({"action": "replace"})).is_none());

        let b: KeepAwakeBody = parse(json!({})).unwrap();
        assert_eq!(b.on, None);
        assert!(parse::<KeepAwakeBody>(json!({"on": 1})).is_none());

        let b: SubscribeBody = parse(json!({"subscription": {"endpoint": "https://push.example/x", "expirationTime": null, "keys": {"p256dh": "k", "auth": "a"}}})).unwrap();
        let sub = b.subscription.unwrap();
        assert_eq!(sub.endpoint.as_deref(), Some("https://push.example/x"));
        assert_eq!(sub.keys.unwrap().auth.as_deref(), Some("a"));
        let b: SubscribeBody =
            parse(json!({"subscription": {"endpoint": "https://push.example/x"}})).unwrap();
        assert!(
            b.subscription.unwrap().keys.is_none(),
            "an invalid subscription, refused as one"
        );
    }

    #[test]
    fn client_events_keep_their_fields_for_the_allow_list() {
        let b: ClientEvents =
            parse(json!({"events": [{"ev": "live_open", "n": 3}, {"n": 1}]})).unwrap();
        assert_eq!(b.events.len(), 2);
        assert_eq!(b.events[0]["ev"], "live_open");
        assert!(parse::<ClientEvents>(json!({})).unwrap().events.is_empty());
        assert!(parse::<ClientEvents>(json!({"events": ["live_open"]})).is_none());
    }
}
