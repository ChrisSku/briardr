// Stream routes: creating work, stream actions, Changes, Docs, Files, uploads and dictation.
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::http::Method;
use serde_json::{json, Value};

use super::{
    action_err, js_number, json_body, now_ms, ok, raw_body, response, stream_gone, upload_body,
    ApiError, App, Ctx, Routes, JSON_LIMIT,
};
use crate::dictation::{transcribe, DictationCategory, MAX_AUDIO_BYTES};
use crate::docs::{list_docs, read_asset, read_doc, DocEntry, DocsError};
use crate::files::{delete_entry, list_dir, read_file, search_files, FileEntry};
use crate::git::{
    changes, file_diff, ChangeKind, ChangedFile, ChangesReport, ChangesState, FileRef, GitError,
};
use crate::uploads::{quote_path, receive_upload, uploads_root, UploadKind};

/// Runs blocking work (git, file reads, uploads) off the async runtime.
async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::Internal)
}

fn require_stream(app: &App, id: &str) -> Result<(), ApiError> {
    if app.stream_exists(id) {
        Ok(())
    } else {
        Err(stream_gone())
    }
}

/// A body field: a missing key is treated like null.
fn field(body: &Value, key: &str) -> Value {
    body.get(key).cloned().unwrap_or(Value::Null)
}

/// Node's `path.normalize` on an absolute or relative path, without touching the disk.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `path.relative(from, to)` for two absolute paths, with `/` separators.
fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    (common..from.len())
        .map(|_| "..".to_string())
        .chain(
            to[common..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>()
        .join("/")
}

/// The docs folder's changed files, relative to its real location (the `changed` set of GET docs).
/// Only resolves paths when there are changed files.
fn changed_docs(cwd: &Path) -> Result<HashSet<String>, std::io::Error> {
    let report = changes(cwd, "git");
    let mut changed = HashSet::new();
    let Some(root) = report.root else {
        return Ok(changed);
    };
    if report.files.is_empty() {
        return Ok(changed);
    }
    let cwd_real = std::fs::canonicalize(cwd)?;
    let root_real = std::fs::canonicalize(&root)?;
    for f in &report.files {
        changed.insert(relative(&cwd_real, &normalize(&root_real.join(&f.path))));
    }
    Ok(changed)
}

fn state_code(state: ChangesState) -> &'static str {
    match state {
        ChangesState::Ok => "ok",
        ChangesState::Clean => "clean",
        ChangesState::NotRepo => "not_repo",
        ChangesState::GitMissing => "git_missing",
        ChangesState::Error => "error",
    }
}

fn change_code(change: ChangeKind) -> &'static str {
    match change {
        ChangeKind::Added => "added",
        ChangeKind::Modified => "modified",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed => "renamed",
        ChangeKind::Copied => "copied",
        ChangeKind::TypeChanged => "type",
        ChangeKind::Untracked => "untracked",
        ChangeKind::Conflict => "conflict",
    }
}

fn changed_file_json(f: &ChangedFile) -> Value {
    json!({
        "path": f.path,
        "origPath": f.orig_path,
        "index": f.index.to_string(),
        "worktree": f.worktree.to_string(),
        "staged": f.staged,
        "unstaged": f.unstaged,
        "untracked": f.untracked,
        "conflicted": f.conflicted,
        "change": change_code(f.change),
        "modeChange": f.mode_change.as_ref().map(|m| json!({"from": m.from, "to": m.to})),
        "binary": f.binary,
        "additions": f.additions,
        "deletions": f.deletions,
        "signature": f.signature,
    })
}

/// The `ChangesReport` JSON; `message` is only set on the error paths and comes last.
fn changes_json(r: &ChangesReport) -> Value {
    let mut out = json!({
        "state": state_code(r.state),
        "root": r.root.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "branch": r.branch,
        "detached": r.detached,
        "headShort": r.head_short,
        "noCommits": r.no_commits,
        "files": r.files.iter().map(changed_file_json).collect::<Vec<_>>(),
        "totalFiles": r.total_files,
        "truncated": r.truncated,
        "asOf": r.as_of,
    });
    if let (Some(message), Value::Object(map)) = (&r.message, &mut out) {
        map.insert("message".into(), json!(message));
    }
    out
}

fn git_code(e: &GitError) -> &'static str {
    match e {
        GitError::GitMissing => "git_missing",
        GitError::NotRepo => "not_repo",
        GitError::Forbidden(_) => "forbidden",
        GitError::Timeout => "timeout",
        GitError::Failed(_) => "error",
    }
}

fn doc_entry_json(d: &DocEntry) -> Value {
    json!({
        "path": d.path,
        "name": d.name,
        "changed": d.changed,
        "readme": d.readme,
        "size": d.size,
    })
}

fn file_entry_json(e: &FileEntry) -> Value {
    json!({
        "name": e.name,
        "path": e.path,
        "dir": e.dir,
        "size": e.size,
        "link": e.link,
        "changed": e.changed,
    })
}

/// The status and code sent for a refused docs or files read.
fn docs_err(e: DocsError) -> ApiError {
    match e {
        // Not a refusal: the generic 500.
        DocsError::Io(_) => ApiError::Internal,
        e => {
            let status = match e {
                DocsError::Outside(_) => 403,
                DocsError::TooLarge(_) => 413,
                DocsError::Unsupported(_) => 415,
                _ => 404,
            };
            action_err(status, e.code(), e.to_string())
        }
    }
}

pub(super) fn register(r: &mut Routes) {
    // --- creating work ---
    r.add(
        Method::POST,
        "/api/workspaces",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            ok(app
                .d
                .actions
                .create_workspace(&field(&b, "name"), &field(&b, "folder"))
                .await?)
        },
    );

    r.add(
        Method::POST,
        "/api/projects",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            ok(app
                .d
                .actions
                .create_project(&field(&b, "workspaceId"), &field(&b, "name"))
                .await?)
        },
    );

    r.add(
        Method::POST,
        "/api/streams",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            ok(app
                .d
                .actions
                .create_stream(
                    &field(&b, "projectId"),
                    &field(&b, "kind"),
                    &field(&b, "name"),
                )
                .await?)
        },
    );

    // --- stream actions ---
    r.add(
        Method::POST,
        "/api/streams/:id/rename",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b = json_body(&mut ctx, JSON_LIMIT).await?;
            app.d
                .actions
                .rename(ctx.param("id"), &field(&b, "name"))
                .await?;
            ok(json!({"ok": true}))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/info",
        false,
        |app: Arc<App>, ctx: Ctx| async move { ok(app.d.actions.info(ctx.param("id")).await?) },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/history",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let id = ctx.param("id");
            if !app.stream_exists(id) {
                return Err(stream_gone());
            }
            let lines = js_number(
                ctx.query
                    .get("lines")
                    .filter(|v| !v.is_empty())
                    .unwrap_or("1000"),
            );
            ok(app.d.actions.history(id, lines).await?)
        },
    );

    // --- changes ---
    r.add(
        Method::GET,
        "/api/streams/:id/changes",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let report = blocking(move || changes(&cwd, "git")).await?;
            ok(changes_json(&report))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/changes/diff",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let report = blocking(move || changes(&cwd, "git")).await?;
            let p = ctx.query.get("path").unwrap_or("");
            let found = report.files.iter().find(|x| x.path == p);
            let (Some(root), Some(f)) = (report.root.clone(), found) else {
                return Err(action_err(
                    404,
                    "file_gone",
                    "That file has no changes anymore.",
                ));
            };
            let signature = f.signature.clone();
            let file = FileRef {
                path: f.path.clone(),
                orig_path: f.orig_path.clone(),
                untracked: f.untracked,
            };
            let no_commits = report.no_commits;
            match blocking(move || file_diff(&root, &file, no_commits, "git")).await? {
                Ok(d) => ok(json!({
                    "path": d.path,
                    "text": d.text,
                    "lines": d.lines,
                    "truncated": d.truncated,
                    "binary": d.binary,
                    "signature": signature,
                })),
                Err(e) => {
                    let status = match e {
                        GitError::Forbidden(_) => 403,
                        _ => 500,
                    };
                    Err(action_err(status, git_code(&e), e.to_string()))
                }
            }
        },
    );

    // --- docs ---
    r.add(
        Method::GET,
        "/api/streams/:id/docs",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let folder = s.folder;
            let listing_cwd = cwd.clone();
            let changed = blocking(move || changed_docs(&cwd))
                .await?
                .map_err(|_| ApiError::Internal)?;
            match blocking(move || list_docs(&listing_cwd, &changed)).await? {
                Ok(list) => ok(json!({
                    "docs": list.docs.iter().map(doc_entry_json).collect::<Vec<_>>(),
                    "total": list.total,
                    "truncated": list.truncated,
                    "folder": folder,
                    "asOf": now_ms(),
                })),
                Err(_) => Err(action_err(
                    404,
                    "folder_missing",
                    "This stream’s folder can’t be read.",
                )),
            }
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/docs/file",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let rel = ctx.query.get("path").unwrap_or("").to_string();
            let doc = blocking(move || read_doc(&cwd, &rel))
                .await?
                .map_err(docs_err)?;
            ok(json!({"path": doc.path, "text": doc.text, "size": doc.size}))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/docs/asset",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let rel = ctx.query.get("path").unwrap_or("").to_string();
            match blocking(move || read_asset(&cwd, &rel)).await? {
                // SVGs can carry script; serve them sandboxed so they render only as images.
                Ok(asset) => Ok(response(
                    200,
                    asset.data,
                    &[
                        ("content-type", asset.content_type.to_string()),
                        ("cache-control", "private, max-age=60".to_string()),
                        (
                            "content-security-policy",
                            "default-src 'none'; style-src 'unsafe-inline'; sandbox".to_string(),
                        ),
                    ],
                )),
                Err(DocsError::Io(_)) => Err(ApiError::Internal),
                Err(e) => {
                    let status = if matches!(e, DocsError::Outside(_)) {
                        403
                    } else {
                        404
                    };
                    Err(action_err(status, e.code(), e.to_string()))
                }
            }
        },
    );

    // --- files ---
    r.add(
        Method::DELETE,
        "/api/streams/:id/files",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let rel = ctx.query.get("path").unwrap_or("").to_string();
            blocking(move || delete_entry(&cwd, &rel))
                .await?
                .map_err(docs_err)?;
            ok(json!({ "ok": true }))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/files",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let rel = ctx.query.get("path").unwrap_or("").to_string();
            let listing_cwd = cwd.clone();
            // A folder that git can't report on just lists without change marks.
            let changed = blocking(move || changed_docs(&cwd))
                .await?
                .unwrap_or_default();
            let list = blocking(move || list_dir(&listing_cwd, &rel, &changed))
                .await?
                .map_err(docs_err)?;
            ok(json!({
                "path": list.path,
                "entries": list.entries.iter().map(file_entry_json).collect::<Vec<_>>(),
                "total": list.total,
                "truncated": list.truncated,
                "asOf": now_ms(),
            }))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/files/search",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let query = ctx.query.get("q").unwrap_or("").to_string();
            let result = blocking(move || {
                let changed = changed_docs(&cwd).unwrap_or_default();
                search_files(&cwd, &query, &changed)
            })
            .await?
            .map_err(docs_err)?;
            ok(json!({
                "entries": result.entries.iter().map(file_entry_json).collect::<Vec<_>>(),
                "truncated": result.truncated,
            }))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/files/file",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let rel = ctx.query.get("path").unwrap_or("").to_string();
            let f = blocking(move || read_file(&cwd, &rel))
                .await?
                .map_err(docs_err)?;
            ok(json!({
                "path": f.path,
                "text": f.text,
                "binary": f.text.is_none(),
                "size": f.size,
            }))
        },
    );

    // --- uploads ---
    r.add(
        Method::POST,
        "/api/streams/:id/uploads",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            require_stream(&app, &id)?;
            let kind = if ctx.query.get("kind") == Some("image") {
                UploadKind::Image
            } else {
                UploadKind::File
            };
            let kind_name = match kind {
                UploadKind::Image => "image",
                UploadKind::File => "file",
            };
            let name = ctx.query.get("name").unwrap_or("").to_string();
            let stored = match upload_body(&mut ctx, kind).await {
                Ok(bytes) => {
                    let size = bytes.len();
                    blocking(move || receive_upload(&bytes, kind, &name, &uploads_root()))
                        .await?
                        .map(|path| (path, size))
                }
                Err(e) => Err(e),
            };
            match stored {
                Ok((path, bytes)) => {
                    app.event("upload_ok", json!({"kind": kind_name, "bytes": bytes}));
                    ok(json!({
                        "path": path.to_string_lossy().into_owned(),
                        "quoted": quote_path(&path),
                        "bytes": bytes,
                    }))
                }
                Err(e) => {
                    app.event(
                        "upload_fail",
                        json!({"kind": kind_name, "status": e.status()}),
                    );
                    if e.status() == 499 {
                        return Ok(response(499, "", &[]));
                    }
                    Err(action_err(e.status(), "upload_failed", e.message()))
                }
            }
        },
    );

    // --- dictation ---
    r.add(
        Method::POST,
        "/api/dictation",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            if !app.d.caps.dictation {
                return Err(app.unsupported("Server dictation"));
            }
            let locale = ctx
                .query
                .get("locale")
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let audio = raw_body(&mut ctx, MAX_AUDIO_BYTES).await?;
            let helper = app
                .d
                .dictation_helper
                .read()
                .ok()
                .and_then(|h| (*h).clone());
            let started = Instant::now();
            match transcribe(helper.as_deref(), &audio, locale.as_deref()).await {
                Ok(text) => {
                    let ms = started.elapsed().as_millis() as u64;
                    app.event("dictation_ok", json!({"ms": ms}));
                    ok(json!({"text": text}))
                }
                Err(e) => {
                    let code = e.category.code();
                    app.event("dictation_fail", json!({"category": code}));
                    let status = if e.category == DictationCategory::Unavailable {
                        501
                    } else {
                        422
                    };
                    Err(action_err(status, code, e.message))
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_match_node() {
        let from = Path::new("/a/b/c");
        assert_eq!(relative(from, Path::new("/a/b/c/d/e.md")), "d/e.md");
        assert_eq!(relative(from, Path::new("/a/x/y.md")), "../../x/y.md");
        assert_eq!(relative(from, from), "");
    }

    #[test]
    fn joins_normalize_parent_segments() {
        assert_eq!(
            normalize(&Path::new("/repo").join("src/../docs/a.md")),
            PathBuf::from("/repo/docs/a.md")
        );
    }

    #[test]
    fn git_and_change_codes_match_ts() {
        assert_eq!(git_code(&GitError::Timeout), "timeout");
        assert_eq!(git_code(&GitError::Failed("x".into())), "error");
        assert_eq!(change_code(ChangeKind::TypeChanged), "type");
        assert_eq!(state_code(ChangesState::GitMissing), "git_missing");
    }
}
