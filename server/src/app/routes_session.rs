// Routes for the session, pairing, devices and state, plus the certificate profile.
use std::sync::Arc;

use axum::http::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    action_err, done, json_body_as, ok, refuse, refuse_code, response, send, ApiError, App, Ctx,
    DeviceRef, Done, LiveMessage, RenameBody, Routes, VERSION,
};
use crate::auth::{device_name, Device, DeviceInfo, PairError};
use crate::platform::{HostCapabilities, HostPlatform};
use crate::plugins::PluginInfo;
use crate::power::MacSummary;
use crate::qr;
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

/// A revoke or rename of a device that isn't paired (any more).
fn device_gone() -> ApiError {
    action_err(404, "device_gone", "That device is no longer signed in.")
}

/// `POST /api/pair`: the code shown on the host or another device, and the new device's name.
#[derive(Deserialize)]
struct PairBody {
    code: Option<String>,
    name: Option<String>,
}

/// `POST /api/devices/pairing-code`: a one-time code, the link that opens Briardr with it filled in, and
/// that link as a QR code's rows of modules (`1` dark, `0` light), for the new device's camera.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingCode {
    code: String,
    expires_at: u64,
    link: String,
    /// None when the link is too long for a QR code.
    qr: Option<Vec<String>>,
}

/// `GET /api/session` and `POST /api/pair`: the same startup data for an existing session and a newly
/// paired device.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Session<'a> {
    device: DeviceRef<'a>,
    version: &'static str,
    protocol: u32,
    /// Changes on every server start.
    instance_id: &'a str,
    host: &'a HostCapabilities,
    plugins: Vec<PluginInfo>,
}

/// `GET /api/session` from a device that isn't signed in: enough for the pairing screen's words.
#[derive(Serialize)]
struct SignedOut {
    error: &'static str,
    paired: bool,
    host: PairingHost,
}

#[derive(Serialize)]
struct PairingHost {
    noun: &'static str,
    platform: HostPlatform,
}

/// `GET /api/devices`: every paired device, which of them this is, and which take alerts.
#[derive(Serialize)]
struct Devices {
    devices: Vec<DeviceEntry>,
}

#[derive(Serialize)]
struct DeviceEntry {
    #[serde(flatten)]
    device: DeviceInfo,
    current: bool,
    push: bool,
}

/// `GET /api/mac` and `POST /api/power/keep-awake`: the power state and what the host can do.
#[derive(Serialize)]
pub(super) struct MacState<'a> {
    pub mac: MacSummary,
    pub host: &'a HostCapabilities,
}

async fn session_info<'a>(app: &'a Arc<App>, device: &'a Device) -> Result<Session<'a>, ApiError> {
    Ok(Session {
        device: device.into(),
        version: VERSION,
        protocol: PROTOCOL,
        instance_id: &app.instance_id,
        host: &app.d.caps,
        plugins: super::routes_plugins::plugin_list(app).await?,
    })
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
                let body = SignedOut {
                    error: "unauthenticated",
                    paired: false,
                    host: PairingHost {
                        noun: app.d.caps.noun,
                        platform: app.d.caps.platform,
                    },
                };
                return Ok(send(401, &body, &[]));
            };
            ok(session_info(&app, device).await?)
        },
    );

    r.add(Method::POST, "/api/pair", true, |app: Arc<App>, mut ctx: Ctx| async move {
        if !app.origin_allowed(&ctx.headers) {
            return Ok(refuse_code(403, "bad_origin"));
        }
        let client = ctx.client.clone();
        if !app.pair_allowed(&client) {
            return Ok(refuse(429, "rate_limited", "Too many attempts. Wait a few minutes."));
        }
        let b: PairBody = json_body_as(&mut ctx, 4096).await?;
        let (code, name) = (b.code.unwrap_or_default(), b.name.unwrap_or_default());
        let auth = app.d.auth.clone();
        let result = blocking(move || auth.pair(&code, &name, &client)).await?;
        app.event(
            "pair_attempt",
            json!({
                "ok": result.is_ok(),
                "error": result.as_ref().err().map(PairError::code),
            }),
        );
        match result {
            Err(error) => {
                // On the internet, don't tell strangers whether a code is active right now.
                if app.in_proxy_mode() {
                    return Ok(refuse(
                        400,
                        "invalid",
                        "That code isn’t right, or it’s no longer valid. Check it, or create a new one.",
                    ));
                }
                Ok(refuse(400, error.code(), &pair_message(&error, &app.host())))
            }
            Ok((device, cookie)) => Ok(send(
                200,
                &session_info(&app, &device).await?,
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
                blocking(move || auth.revoke(&id))
                    .await?
                    .map_err(|_| ApiError::Internal)?;
            }
            Ok(send(
                200,
                &Done { ok: true },
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
            let me = &ctx.device()?.id;
            let devices = app
                .d
                .auth
                .list()
                .into_iter()
                .map(|device| DeviceEntry {
                    current: &device.id == me,
                    push: app.d.push.has_device(&device.id),
                    device,
                })
                .collect();
            ok(Devices { devices })
        },
    );

    r.add(
        Method::DELETE,
        "/api/devices/:id",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            let auth = app.d.auth.clone();
            let removed = blocking(move || auth.revoke(&id))
                .await?
                .map_err(|_| ApiError::Internal)?;
            if !removed {
                return Err(device_gone());
            }
            done()
        },
    );

    r.add(
        Method::POST,
        "/api/devices/:id/rename",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            // A missing or `null` name is refused like an empty one.
            let body: RenameBody = json_body_as(&mut ctx, 4096).await?;
            let Some(name) = body.name.as_deref().and_then(device_name) else {
                return Err(action_err(400, "invalid_name", "Enter a name."));
            };
            let id = ctx.param("id").to_string();
            let auth = app.d.auth.clone();
            let renamed = blocking(move || auth.rename(&id, &name))
                .await?
                .map_err(|_| ApiError::Internal)?;
            if !renamed {
                return Err(device_gone());
            }
            done()
        },
    );

    r.add(
        Method::POST,
        "/api/devices/pairing-code",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let auth = app.d.auth.clone();
            let (code, expires_at) = blocking(move || auth.create_pairing_code()).await?;
            let link = qr::pairing_link(&app.pairing_origin(&ctx.headers), &code);
            let body = PairingCode {
                qr: qr::modules(&link),
                link,
                code,
                expires_at,
            };
            ok(body)
        },
    );

    // --- state ---
    r.add(
        Method::GET,
        "/api/state",
        false,
        |app: Arc<App>, _ctx: Ctx| async move { ok(LiveMessage::Model(app.model_state())) },
    );

    r.add(
        Method::GET,
        "/api/mac",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            let mac = app.poll_power().await?;
            ok(MacState {
                mac,
                host: &app.d.caps,
            })
        },
    );

    // --- certificate profile for devices (public: it's a public certificate) ---
    r.add(
        Method::GET,
        "/api/ca.mobileconfig",
        true,
        |app: Arc<App>, _ctx: Ctx| async move {
            let Some(profile) = app.d.ca_profile.clone() else {
                return Ok(refuse_code(404, "not_available"));
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
    fn pair_errors_have_the_codes_and_messages_the_client_shows() {
        assert_eq!(PairError::NoCode.code(), "no_code");
        assert_eq!(PairError::Expired.code(), "expired");
        assert_eq!(PairError::Invalid.code(), "invalid");
        assert_eq!(PairError::Locked.code(), "locked");
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
