// Notes routes: per-stream notes, their images, and pasting a note into a terminal.
use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{
    action_err, json_body, ok, response, send, stream_gone, upload_body, ApiError, App, Ctx, Resp,
    Routes,
};
use crate::notes::{NotesError, SaveResult, MAX_NOTES};
use crate::uploads::{UploadError, UploadKind};

const NOTE_BODY_LIMIT: usize = 512 * 1024;

/// `NOTE_ID`: 16 lowercase hex characters.
fn note_id(id: &str) -> Result<String, ApiError> {
    if id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(id.to_string())
    } else {
        Err(note_gone())
    }
}

fn note_gone() -> ApiError {
    action_err(404, "note_gone", "That note no longer exists.")
}

fn require_stream(app: &App, id: &str) -> Result<(), ApiError> {
    if app.stream_exists(id) {
        Ok(())
    } else {
        Err(stream_gone())
    }
}

async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::Internal)
}

/// An upload failure as the routes report it: 499 is an empty response, the rest `upload_failed`.
fn upload_failure(e: &UploadError) -> Result<Resp, ApiError> {
    if e.status() == 499 {
        return Ok(response(499, axum::body::Body::empty(), &[]));
    }
    Err(action_err(e.status(), "upload_failed", e.message()))
}

pub(super) fn register(r: &mut Routes) {
    r.add(
        Method::GET,
        "/api/streams/:id/notes",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let notes = app.d.notes.clone();
            let id = ctx.param("id").to_string();
            let list = blocking(move || notes.list(&id)).await?;
            ok(json!({ "notes": list }))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/notes/:note",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let nid = note_id(ctx.param("note"))?;
            let notes = app.d.notes.clone();
            let id = ctx.param("id").to_string();
            let note = blocking(move || notes.get(&id, &nid))
                .await?
                .ok_or_else(note_gone)?;
            ok(json!({ "note": note }))
        },
    );

    r.add(
        Method::PUT,
        "/api/streams/:id/notes/:note",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let stream = ctx.param("id").to_string();
            require_stream(&app, &stream)?;
            let nid = note_id(ctx.param("note"))?;
            let b = json_body(&mut ctx, NOTE_BODY_LIMIT).await?;
            let invalid = || action_err(400, "invalid", "Invalid note");
            let text = b
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?
                .to_string();
            // Number.isInteger(b.baseRevision) && b.baseRevision >= 0
            let base = b
                .get("baseRevision")
                .and_then(Value::as_f64)
                .filter(|n| n.fract() == 0.0 && *n >= 0.0 && n.is_finite())
                .ok_or_else(invalid)? as u64;
            let notes = app.d.notes.clone();
            let (s, n) = (stream.clone(), nid.clone());
            let result = blocking(move || notes.save(&s, Some(&n), &text, Some(base)))
                .await?
                .map_err(|_| ApiError::Internal)?;
            match result {
                SaveResult::TooLarge => Err(action_err(413, "too_large", "Notes are limited to 256 KB.")),
                SaveResult::TooMany => Err(action_err(
                    409,
                    "too_many",
                    format!("A stream can have up to {MAX_NOTES} notes."),
                )),
                SaveResult::Conflict(note) => {
                    let message = if note.is_some() {
                        "This note changed on another device."
                    } else {
                        "This note was deleted on another device."
                    };
                    Ok(send(
                        409,
                        &json!({"error": "conflict", "message": message, "note": note}),
                        &[],
                    ))
                }
                SaveResult::Ok(note) => {
                    app.broadcast(&json!({"t": "notes", "streamId": stream, "noteId": nid, "revision": note.revision}));
                    ok(json!({ "note": note }))
                }
            }
        },
    );

    r.add(
        Method::DELETE,
        "/api/streams/:id/notes/:note",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let nid = note_id(ctx.param("note"))?;
            let stream = ctx.param("id").to_string();
            let notes = app.d.notes.clone();
            let (s, n) = (stream.clone(), nid.clone());
            let removed = blocking(move || notes.remove(&s, &n))
                .await?
                .map_err(|_| ApiError::Internal)?;
            if removed {
                app.broadcast(
                    &json!({"t": "notes", "streamId": stream, "noteId": nid, "revision": 0}),
                );
            }
            ok(json!({ "ok": true }))
        },
    );

    // A note (or part of one) as it's pasted into the terminal: images become paths an agent can open.
    r.add(
        Method::POST,
        "/api/streams/:id/notes/paste",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let stream = ctx.param("id").to_string();
            require_stream(&app, &stream)?;
            let b = json_body(&mut ctx, NOTE_BODY_LIMIT).await?;
            let text = b
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| action_err(400, "invalid", "Invalid note"))?
                .to_string();
            let notes = app.d.notes.clone();
            let pasted = blocking(move || notes.for_terminal(&stream, &text))
                .await?
                .map_err(|_| ApiError::Internal)?;
            ok(serde_json::to_value(pasted).map_err(|_| ApiError::Internal)?)
        },
    );

    r.add(
        Method::POST,
        "/api/streams/:id/notes/images",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let stream = ctx.param("id").to_string();
            require_stream(&app, &stream)?;
            let bytes = match upload_body(&mut ctx, UploadKind::Image).await {
                Ok(b) => b,
                Err(e) => return upload_failure(&e),
            };
            let notes = app.d.notes.clone();
            match blocking(move || notes.add_image(&stream, &bytes)).await? {
                Ok(name) => ok(json!({ "name": name })),
                Err(NotesError::Upload(e)) => upload_failure(&e),
                Err(_) => upload_failure(&UploadError::Io(std::io::Error::other("save"))),
            }
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/notes/images/:name",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let path = app
                .d
                .notes
                .image_path(ctx.param("id"), ctx.param("name"))
                .ok_or_else(|| action_err(404, "not_found", "Image not found"))?;
            let data = tokio::fs::read(&path)
                .await
                .map_err(|_| action_err(404, "not_found", "Image not found"))?;
            // Names are random and never reused, so the image can be cached for good.
            Ok(response(
                200,
                data,
                &[
                    ("content-type", "image/png".to_string()),
                    (
                        "cache-control",
                        "private, max-age=31536000, immutable".to_string(),
                    ),
                    (
                        "content-security-policy",
                        "default-src 'none'; sandbox".to_string(),
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
    fn note_ids_match_ts_pattern() {
        assert!(note_id("0123456789abcdef").is_ok());
        assert!(note_id("0123456789ABCDEF").is_err());
        assert!(note_id("0123456789abcde").is_err());
        assert!(note_id("../../etc/passwd").is_err());
    }
}
