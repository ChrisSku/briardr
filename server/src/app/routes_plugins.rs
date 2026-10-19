// Plugin routes: the list of plugins and turning them on or off for every device.
use std::sync::Arc;

use tokio::sync::Mutex;

use axum::http::Method;
use serde_json::{json, Value};

use base64::prelude::{Engine, BASE64_STANDARD};

use super::{
    action_err, header_str, json_body, ok, raw_body, response, stream_gone, ApiError, App, Ctx,
    Resp, Routes, JSON_LIMIT,
};
use crate::herdr::state::HerdrReader;
use crate::plugins::{backend_hash, discover, info_json, web_runtime};

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

/// Changes one at a time, so two quick toggles can't each save over the other's config.
static CHANGES: Mutex<()> = Mutex::const_new(());

/// Every plugin with whether it's on; built-ins first, in tab order.
pub(super) async fn plugin_list(app: &Arc<App>) -> Result<Value, ApiError> {
    let paths = app.d.cfg.paths().clone();
    let state = app.plugins.get();
    let web_dir = app.d.web_dir.clone();
    blocking(move || {
        let runtime = web_runtime(&web_dir);
        Value::Array(
            discover(&paths)
                .iter()
                .map(|p| info_json(p, &state, runtime.as_deref()))
                .collect(),
        )
    })
    .await
}

pub(super) fn register(r: &mut Routes) {
    r.add(
        Method::GET,
        "/api/plugins",
        false,
        |app: Arc<App>, _ctx: Ctx| async move { ok(json!({"plugins": plugin_list(&app).await?})) },
    );

    // {enabled, approve?}: a plugin with a backend runs a program on the host, so turning it on
    // needs the hash of the command the user was shown (`backend.hash` in the list).
    r.add(
        Method::PUT,
        "/api/plugins/:id",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            let _one_at_a_time = CHANGES.lock().await;
            let Some(enabled) = b.get("enabled").and_then(Value::as_bool) else {
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
                    if !approved && b.get("approve").and_then(Value::as_str) != Some(hash.as_str()) {
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
            let list = plugin_list(&app).await?;
            app.broadcast(&json!({"t": "plugins", "plugins": list}));
            ok(json!({"plugins": list}))
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
        Some(sid) => {
            let s = app
                .d
                .herdr
                .model()
                .and_then(|m| m.streams.into_iter().find(|s| s.id == sid))
                .ok_or_else(stream_gone)?;
            serde_json::to_value(s).map_err(|_| ApiError::Internal)?
        }
        None => Value::Null,
    };
    let device = ctx.device()?;
    let mut req = json!({
        "method": ctx.method.as_str(),
        "path": format!("/{}", ctx.param("rest")),
        "query": ctx.query.pairs().iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
        "stream": stream,
        "device": {"id": device.id, "name": device.name},
        "body": null,
    });
    let json_type = header_str(&ctx.headers, "content-type").is_some_and(|t| t.contains("json"));
    let raw = raw_body(&mut ctx, JSON_LIMIT).await?;
    if !raw.is_empty() {
        if json_type {
            req["body"] = serde_json::from_slice(&raw)
                .map_err(|_| action_err(400, "bad_json", "Invalid JSON"))?;
        } else {
            match String::from_utf8(raw) {
                Ok(text) => req["text"] = json!(text),
                Err(e) => req["base64"] = json!(BASE64_STANDARD.encode(e.into_bytes())),
            }
        }
    }
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
