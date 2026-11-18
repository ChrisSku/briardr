// Notes routes: per-stream notes, their images, and pasting a note into a terminal.
use std::sync::Arc;

use axum::http::Method;
use serde::Deserialize;
use serde_json::json;

use super::{
    action_err, json_body_as, ok, response, send, stream_gone, upload_body, ApiError, App, Ctx,
    Resp, Routes,
};
use crate::notes::{NotesError, SaveResult, MAX_NOTES};
use crate::uploads::{UploadError, UploadKind};

const NOTE_BODY_LIMIT: usize = 512 * 1024;

/// `PUT /api/streams/:id/notes/:note`: the note's text, saved only over the revision it was edited from
/// (0 for a new note).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NoteSave {
    text: Option<String>,
    base_revision: Option<u64>,
}

/// `POST /api/streams/:id/notes/paste`: the text to paste, a note or part of one.
#[derive(Deserialize)]
struct NotePaste {
    text: Option<String>,
}

fn invalid_note() -> ApiError {
    action_err(400, "invalid", "Invalid note")
}

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
            let b: NoteSave = json_body_as(&mut ctx, NOTE_BODY_LIMIT).await?;
            let (Some(text), Some(base)) = (b.text, b.base_revision) else {
                return Err(invalid_note());
            };
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
            let b: NotePaste = json_body_as(&mut ctx, NOTE_BODY_LIMIT).await?;
            let text = b.text.ok_or_else(invalid_note)?;
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
    fn a_saved_note_names_the_revision_it_was_edited_from() {
        let b: NoteSave = serde_json::from_value(json!({"text": "hi", "baseRevision": 2})).unwrap();
        assert_eq!((b.text.as_deref(), b.base_revision), (Some("hi"), Some(2)));
        let b: NoteSave = serde_json::from_value(json!({"text": "hi"})).unwrap();
        assert_eq!(b.base_revision, None, "refused as an invalid note");
        for base in [json!(-1), json!(1.5), json!("2")] {
            assert!(serde_json::from_value::<NoteSave>(
                json!({"text": "hi", "baseRevision": base})
            )
            .is_err());
        }
    }

    #[test]
    fn note_ids_are_16_lowercase_hex_characters() {
        assert!(note_id("0123456789abcdef").is_ok());
        assert!(note_id("0123456789ABCDEF").is_err());
        assert!(note_id("0123456789abcde").is_err());
        assert!(note_id("../../etc/passwd").is_err());
    }
}
