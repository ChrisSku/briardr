// Routes for the session, pairing, devices and state, plus the certificate profile.
use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{
    caps_json, json_body, ok, response, send, str_or_empty, ApiError, App, Ctx, Routes, VERSION,
};
use crate::auth::PairError;
use crate::terminal_hub::PROTOCOL;

/// Runs blocking work (file writes, shelling out) off the async runtime.
async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::Internal)
}

/// The error category the pairing API and diagnostics use (`PairResult.error`).
fn pair_error_code(error: &PairError) -> &'static str {
    match error {
        PairError::NoCode => "no_code",
        PairError::Expired => "expired",
        PairError::Invalid => "invalid",
        PairError::Locked => "locked",
    }
}

/// What the pairing screen says for a refused code; `host` is "the Mac" or "the server".
fn pair_message(error: &PairError, host: &str) -> String {
    match error {
        PairError::NoCode => format!(
            "There’s no active pairing code. Create one on {host} or from a signed-in device."
        ),
        PairError::Expired => "That code expired. Create a new one.".to_string(),
        PairError::Invalid => "That code isn’t right.".to_string(),
        PairError::Locked => "Too many wrong codes. Create a new one.".to_string(),
    }
}

pub(super) fn register(r: &mut Routes) {
    // --- session & pairing (public) ---
    // The host's kind (Mac or server) lets the pairing screen use the right words before sign-in.
    r.add(
        Method::GET,
        "/api/session",
        true,
        |app: Arc<App>, ctx: Ctx| async move {
            let Some(device) = &ctx.device else {
                return Ok(send(
                    401,
                    &json!({
                        "error": "unauthenticated",
                        "paired": false,
                        "host": {
                            "noun": app.d.caps.noun,
                            "platform": caps_json(&app.d.caps)["platform"],
                        },
                    }),
                    &[],
                ));
            };
            let device = json!({"id": device.id, "name": device.name});
            ok(json!({
                "device": device,
                "version": VERSION,
                "protocol": PROTOCOL,
                "instanceId": app.instance_id,
                "host": caps_json(&app.d.caps),
                "plugins": super::routes_plugins::plugin_list(&app).await?,
            }))
        },
    );

    r.add(Method::POST, "/api/pair", true, |app: Arc<App>, mut ctx: Ctx| async move {
        if !app.origin_allowed(&ctx.headers) {
            return Ok(send(403, &json!({"error": "bad_origin"}), &[]));
        }
        let client = ctx.client.clone();
        if !app.pair_allowed(&client) {
            return Ok(send(
                429,
                &json!({"error": "rate_limited", "message": "Too many attempts. Wait a few minutes."}),
                &[],
            ));
        }
        let b = json_body(&mut ctx, 4096).await?;
        let code = str_or_empty(b.get("code"));
        let name = str_or_empty(b.get("name"));
        let auth = app.d.auth.clone();
        let result = blocking(move || auth.pair(&code, &name, &client)).await?;
        app.event(
            "pair_attempt",
            json!({
                "ok": result.is_ok(),
                "error": result.as_ref().err().map(pair_error_code),
            }),
        );
        match result {
            Err(error) => {
                // On the internet, don't tell strangers whether a code is active right now.
                if app.in_proxy_mode() {
                    return Ok(send(
                        400,
                        &json!({"error": "invalid", "message": "That code isn’t right, or it’s no longer valid. Check it, or create a new one."}),
                        &[],
                    ));
                }
                Ok(send(
                    400,
                    &json!({"error": pair_error_code(&error), "message": pair_message(&error, &app.host())}),
                    &[],
                ))
            }
            Ok((device, cookie)) => Ok(send(
                200,
                &json!({"device": {"id": device.id, "name": device.name}}),
                &[("set-cookie", app.d.auth.set_cookie_header(&cookie))],
            )),
        }
    });

    r.add(
        Method::POST,
        "/api/signout",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            if let Some(device) = &ctx.device {
                let id = device.id.clone();
                let auth = app.d.auth.clone();
                blocking(move || auth.revoke(&id)).await?;
            }
            Ok(send(
                200,
                &json!({"ok": true}),
                &[("set-cookie", app.d.auth.clear_cookie_header())],
            ))
        },
    );

    // --- devices ---
    r.add(
        Method::GET,
        "/api/devices",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let me = ctx.device()?.id.clone();
            let devices: Vec<Value> = app
                .d
                .auth
                .list()
                .iter()
                .map(|x| {
                    let mut v = serde_json::to_value(x).unwrap_or(Value::Null);
                    if let Value::Object(map) = &mut v {
                        map.insert("current".into(), json!(x.id == me));
                        map.insert("push".into(), json!(app.d.push.has_device(&x.id)));
                    }
                    v
                })
                .collect();
            ok(json!({"devices": devices}))
        },
    );

    r.add(
        Method::DELETE,
        "/api/devices/:id",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            let auth = app.d.auth.clone();
            let removed = blocking(move || auth.revoke(&id)).await?;
            Ok(send(
                if removed { 200 } else { 404 },
                &json!({"ok": removed}),
                &[],
            ))
        },
    );

    r.add(
        Method::POST,
        "/api/devices/pairing-code",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            let auth = app.d.auth.clone();
            let (code, expires_at) = blocking(move || auth.create_pairing_code()).await?;
            ok(json!({"code": code, "expiresAt": expires_at}))
        },
    );

    // --- state ---
    r.add(
        Method::GET,
        "/api/state",
        false,
        |app: Arc<App>, _ctx: Ctx| async move { ok(app.model_message()) },
    );

    r.add(
        Method::GET,
        "/api/mac",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            let mac = app.poll_power().await?;
            ok(json!({"mac": mac, "host": caps_json(&app.d.caps)}))
        },
    );

    // --- certificate profile for devices (public: it's a public certificate) ---
    r.add(
        Method::GET,
        "/api/ca.mobileconfig",
        true,
        |app: Arc<App>, _ctx: Ctx| async move {
            let Some(profile) = app.d.ca_profile.clone() else {
                return Ok(send(404, &json!({"error": "not_available"}), &[]));
            };
            Ok(response(
                200,
                profile,
                &[
                    (
                        "content-type",
                        "application/x-apple-aspen-config".to_string(),
                    ),
                    (
                        "content-disposition",
                        "attachment; filename=\"Briardr.mobileconfig\"".to_string(),
                    ),
                ],
            ))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_errors_map_to_ts_codes_and_messages() {
        assert_eq!(pair_error_code(&PairError::NoCode), "no_code");
        assert_eq!(pair_error_code(&PairError::Expired), "expired");
        assert_eq!(pair_error_code(&PairError::Invalid), "invalid");
        assert_eq!(pair_error_code(&PairError::Locked), "locked");
        assert_eq!(
            pair_message(&PairError::NoCode, "the Mac"),
            "There’s no active pairing code. Create one on the Mac or from a signed-in device."
        );
        assert_eq!(
            pair_message(&PairError::Invalid, "the Mac"),
            "That code isn’t right."
        );
        assert_eq!(
            pair_message(&PairError::Locked, "the Mac"),
            "Too many wrong codes. Create a new one."
        );
    }
}
