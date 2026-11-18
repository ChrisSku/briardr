// Plugin routes: the list of plugins and turning them on or off for every device.
use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use axum::http::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use base64::prelude::{Engine, BASE64_STANDARD};

use super::{
    action_err, header_str, json_body_as, ok, raw_body, response, stream_gone, ApiError, App, Ctx,
    DeviceRef, Resp, Routes, JSON_LIMIT,
};
use crate::herdr::model::Stream;
use crate::herdr::state::HerdrReader;
use crate::plugins::{backend_hash, discover, web_runtime, PluginInfo};

/// Response headers a backend may set; everything else (security headers included) is Briardr's.
const BACKEND_HEADERS: &[&str] = &[
    "content-type",
    "cache-control",
    "content-disposition",
    "etag",
    "last-modified",
];

/// Runs blocking work (reading plugin folders, saving the config) off the async runtime.
async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::Internal)
}

/// `PUT /api/plugins/:id`: on or off. A plugin with a backend runs a program on the host, so turning it
/// on needs the hash of the command the user was shown (`backend.hash` in the list) in `approve`.
#[derive(Deserialize)]
struct PluginChange {
    enabled: Option<bool>,
    approve: Option<String>,
}

/// A request for a plugin's backend (`/api/plugins/<id>/<path>?stream=<stream id>&…`), to which
/// `Backends::request` adds its `type` and `id`. A body that isn't JSON comes as `text`, or as `base64`
/// when it isn't UTF-8.
#[derive(Serialize)]
struct BackendRequest<'a> {
    method: &'a str,
    path: String,
    query: BTreeMap<&'a str, &'a str>,
    stream: Option<Stream>,
    device: DeviceRef<'a>,
    body: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
}

/// Changes one at a time, so two quick toggles can't each save over the other's config.
static CHANGES: Mutex<()> = Mutex::const_new(());

/// `GET /api/plugins` and `PUT /api/plugins/:id`: the plugins as they are now.
#[derive(Serialize)]
struct Plugins {
    plugins: Vec<PluginInfo>,
}

/// Every plugin with whether it's on; built-ins first, in tab order.
pub(super) async fn plugin_list(app: &Arc<App>) -> Result<Vec<PluginInfo>, ApiError> {
    let paths = app.d.cfg.paths().clone();
    let state = app.plugins.get();
    let web_dir = app.d.web_dir.clone();
    blocking(move || {
        let runtime = web_runtime(&web_dir);
        discover(&paths)
            .iter()
            .map(|p| PluginInfo::of(p, &state, runtime.as_deref()))
            .collect()
    })
    .await
}

pub(super) fn register(r: &mut Routes) {
    r.add(
        Method::GET,
        "/api/plugins",
        false,
        |app: Arc<App>, _ctx: Ctx| async move {
            ok(Plugins {
                plugins: plugin_list(&app).await?,
            })
        },
    );

    r.add(
        Method::PUT,
        "/api/plugins/:id",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            let b: PluginChange = json_body_as(&mut ctx, JSON_LIMIT).await?;
            let _one_at_a_time = CHANGES.lock().await;
            let Some(enabled) = b.enabled else {
                return Err(action_err(400, "invalid", "Say whether the plugin should be on."));
            };
            let paths = app.d.cfg.paths().clone();
            let all = blocking(move || discover(&paths)).await?;
            let Some(plugin) = all.into_iter().find(|p| p.id == id) else {
                return Err(action_err(404, "not_found", "There’s no plugin with that ID."));
            };
            let mut state = app.plugins.get();
            if enabled {
                if let Some(problem) = &plugin.problem {
                    return Err(action_err(409, "plugin_broken", problem.clone()));
                }
                let backend = plugin.manifest.as_ref().and_then(|m| m.backend.as_ref());
                if let Some(spec) = backend {
                    let hash = backend_hash(spec);
                    let approved = state.approved.get(&id) == Some(&hash);
                    if !approved && b.approve.as_deref() != Some(hash.as_str()) {
                        return Err(action_err(
                            409,
                            "approval_needed",
                            "This plugin runs a program on the host. Approve its command to turn it on.",
                        ));
                    }
                    state.approved.insert(id.clone(), hash);
                }
            }
            state.set_enabled(&id, enabled);
            let saved = {
                let app = app.clone();
                blocking(move || app.plugins.set(state)).await?
            };
            saved.map_err(|_| ApiError::Internal)?;
            if id == "preview" && !enabled {
                app.d.previews.close_all();
            }
            if !enabled {
                app.backends.stop(&id);
            }
            app.event("plugin_toggle", json!({"builtin": plugin.builtin, "enabled": enabled}));
            let plugins = plugin_list(&app).await?;
            app.broadcast(&json!({"t": "plugins", "plugins": plugins}));
            ok(Plugins { plugins })
        },
    );

    // Requests for a plugin's backend: /api/plugins/<id>/<path>?stream=<stream id>&…
    for method in [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
    ] {
        r.add(method, "/api/plugins/:id/*", false, proxy);
    }
}

async fn proxy(app: Arc<App>, mut ctx: Ctx) -> Result<Resp, ApiError> {
    let id = ctx.param("id").to_string();
    let paths = app.d.cfg.paths().clone();
    let plugin = {
        let id = id.clone();
        blocking(move || {
            discover(&paths)
                .into_iter()
                .find(|p| p.id == id && !p.builtin)
        })
        .await?
    };
    let Some(plugin) = plugin else {
        return Err(action_err(
            404,
            "not_found",
            "There’s no plugin with that ID.",
        ));
    };
    if !plugin.active(&app.plugins.get()) {
        return Err(action_err(
            404,
            "plugin_disabled",
            "This plugin is turned off. Turn it on in Settings → Plugins.",
        ));
    }
    let stream = match ctx.query.get("stream") {
        Some(sid) => Some(
            app.d
                .herdr
                .model()
                .and_then(|m| m.streams.into_iter().find(|s| s.id == sid))
                .ok_or_else(stream_gone)?,
        ),
        None => None,
    };
    let json_type = header_str(&ctx.headers, "content-type").is_some_and(|t| t.contains("json"));
    let raw = raw_body(&mut ctx, JSON_LIMIT).await?;
    let (mut body, mut text, mut base64) = (Value::Null, None, None);
    if !raw.is_empty() {
        if json_type {
            body = serde_json::from_slice(&raw)
                .map_err(|_| action_err(400, "bad_json", "Invalid JSON"))?;
        } else {
            match String::from_utf8(raw) {
                Ok(t) => text = Some(t),
                Err(e) => base64 = Some(BASE64_STANDARD.encode(e.into_bytes())),
            }
        }
    }
    let req = BackendRequest {
        method: ctx.method.as_str(),
        path: format!("/{}", ctx.param("rest")),
        query: ctx
            .query
            .pairs()
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect(),
        stream,
        device: ctx.device()?.into(),
        body,
        text,
        base64,
    };
    let req = serde_json::to_value(req).map_err(|_| ApiError::Internal)?;
    let res = app.backends.request(&plugin, req).await.map_err(|e| {
        let (status, code, message) = e.status_code_message();
        action_err(status, code, message)
    })?;
    let status = res["status"]
        .as_u64()
        .filter(|s| (100..=599).contains(s))
        .unwrap_or(200) as u16;
    let mut headers: Vec<(&str, String)> = Vec::new();
    for name in BACKEND_HEADERS {
        if let Some(v) = res["headers"].get(*name).and_then(Value::as_str) {
            headers.push((name, v.to_string()));
        }
    }
    let mut typed = |default: &str| {
        if !headers.iter().any(|(k, _)| *k == "content-type") {
            headers.push(("content-type", default.to_string()));
        }
    };
    let body: Vec<u8> = if let Some(b64) = res["base64"].as_str() {
        typed("application/octet-stream");
        BASE64_STANDARD
            .decode(b64)
            .map_err(|_| action_err(502, "backend_protocol", "The backend sent invalid base64."))?
    } else if let Some(text) = res["text"].as_str() {
        typed("text/plain; charset=utf-8");
        text.as_bytes().to_vec()
    } else if !res["body"].is_null() {
        typed("application/json");
        res["body"].to_string().into_bytes()
    } else {
        Vec::new()
    };
    Ok(response(status, body, &headers))
}
