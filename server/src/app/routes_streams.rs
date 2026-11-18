// Stream routes: creating work, stream actions, Changes, Docs, Files, uploads and dictation.
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::http::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    action_err, done, json_body_as, now_ms, ok, raw_body, response, stream_gone, upload_body,
    ApiError, App, Ctx, RenameBody, Routes, JSON_LIMIT,
};
use crate::actions::{NewProject, NewStream, NewWorkspace};
use crate::dictation::{transcribe, DictationCategory, MAX_AUDIO_BYTES};
use crate::docs::{list_docs, normalize_lexically, read_asset, read_doc, DocEntry, DocsError};
use crate::files::{delete_entry, list_dir, read_file, search_contents, search_files, FileEntry};
use crate::git::{
    base_of, changes, changes_since_base, file_diff, head_tree, ChangeKind, ChangedFile,
    ChangesReport, ChangesState, FileRef, GitError,
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

/// `POST /api/streams/:id/seen`: whether the agent's turn has been seen; left out, it has.
#[derive(Deserialize)]
struct SeenBody {
    seen: Option<bool>,
}

/// The way from one absolute path to another, with `/` separators (`../x/y.md`).
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
        changed.insert(relative(
            &cwd_real,
            &normalize_lexically(&root_real.join(&f.path)),
        ));
    }
    Ok(changed)
}

/// Whether Changes asks for everything since the branch the work started from (`since=base`), not
/// only the uncommitted work.
fn since_base(ctx: &Ctx) -> bool {
    ctx.query.get("since") == Some("base")
}

/// What Changes lists: everything since the branch the work started from, or the uncommitted work,
/// which also says whether there is such a branch to offer.
fn changes_report(cwd: &Path, since_base: bool) -> ChangesReport {
    if since_base {
        return changes_since_base(cwd, "git");
    }
    let mut report = changes(cwd, "git");
    if let (Some(root), ChangesState::Ok | ChangesState::Clean) = (&report.root, report.state) {
        report.base = base_of(root, "git");
    }
    report
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

/// `GET /api/streams/:id/changes`: a `ChangesReport`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangesBody<'a> {
    state: &'static str,
    root: Option<String>,
    branch: Option<&'a str>,
    detached: bool,
    head_short: Option<&'a str>,
    no_commits: bool,
    files: Vec<ChangedFileBody<'a>>,
    total_files: usize,
    truncated: bool,
    as_of: u64,
    base: Option<BaseBody<'a>>,
    /// What the files are compared with: `head` (the uncommitted work) or `base`.
    since: &'static str,
    /// Only on the error paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangedFileBody<'a> {
    path: &'a str,
    orig_path: Option<&'a str>,
    index: char,
    worktree: char,
    staged: bool,
    unstaged: bool,
    untracked: bool,
    conflicted: bool,
    change: &'static str,
    mode_change: Option<ModeChangeBody<'a>>,
    binary: bool,
    additions: Option<u64>,
    deletions: Option<u64>,
    signature: &'a str,
}

#[derive(Serialize)]
struct ModeChangeBody<'a> {
    from: &'a str,
    to: &'a str,
}

/// The branch the work started from: its name, the shared commit abbreviated, and HEAD's commits since.
#[derive(Serialize)]
struct BaseBody<'a> {
    name: &'a str,
    short: &'a str,
    ahead: u64,
}

impl<'a> From<&'a ChangedFile> for ChangedFileBody<'a> {
    fn from(f: &'a ChangedFile) -> Self {
        ChangedFileBody {
            path: &f.path,
            orig_path: f.orig_path.as_deref(),
            index: f.index,
            worktree: f.worktree,
            staged: f.staged,
            unstaged: f.unstaged,
            untracked: f.untracked,
            conflicted: f.conflicted,
            change: change_code(f.change),
            mode_change: f.mode_change.as_ref().map(|m| ModeChangeBody {
                from: &m.from,
                to: &m.to,
            }),
            binary: f.binary,
            additions: f.additions,
            deletions: f.deletions,
            signature: &f.signature,
        }
    }
}

impl<'a> From<&'a ChangesReport> for ChangesBody<'a> {
    fn from(r: &'a ChangesReport) -> Self {
        ChangesBody {
            state: state_code(r.state),
            root: r.root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            branch: r.branch.as_deref(),
            detached: r.detached,
            head_short: r.head_short.as_deref(),
            no_commits: r.no_commits,
            files: r.files.iter().map(ChangedFileBody::from).collect(),
            total_files: r.total_files,
            truncated: r.truncated,
            as_of: r.as_of,
            base: r.base.as_ref().map(|b| BaseBody {
                name: &b.name,
                short: b.commit.get(..7).unwrap_or(&b.commit),
                ahead: b.ahead,
            }),
            since: if r.since_base { "base" } else { "head" },
            message: r.message.as_deref(),
        }
    }
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

/// `GET /api/streams/:id/changes/diff`: one file's diff, with the signature it was made for.
#[derive(Serialize)]
struct DiffBody {
    path: String,
    text: String,
    lines: usize,
    truncated: bool,
    binary: bool,
    /// The file's change signature, which marks it reviewed as it is.
    signature: String,
}

/// `GET /api/streams/:id/docs`: the folder's Markdown documents.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocsBody {
    docs: Vec<DocEntry>,
    total: usize,
    truncated: bool,
    folder: Option<String>,
    as_of: u64,
}

/// `GET /api/streams/:id/files`: a folder's entries.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirBody {
    path: String,
    entries: Vec<FileEntry>,
    total: usize,
    truncated: bool,
    as_of: u64,
}

/// `GET /api/streams/:id/files/file`: a file's text, or None for a binary file.
#[derive(Serialize)]
struct FileBody {
    path: String,
    text: Option<String>,
    binary: bool,
    size: u64,
}

/// `POST /api/streams/:id/files/site`: the token of a link that shows the stream's HTML files as pages.
#[derive(Serialize)]
struct SiteLink {
    token: String,
}

/// `POST /api/streams/:id/uploads`: where the upload is on the host, as a path and quoted for a shell.
#[derive(Serialize)]
struct Uploaded {
    path: String,
    quoted: String,
    bytes: usize,
}

/// `POST /api/dictation`: what was said.
#[derive(Serialize)]
struct Dictated {
    text: String,
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
            let req: NewWorkspace = json_body_as(&mut ctx, JSON_LIMIT).await?;
            ok(app.d.actions.create_workspace(req).await?)
        },
    );

    r.add(
        Method::POST,
        "/api/projects",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let req: NewProject = json_body_as(&mut ctx, JSON_LIMIT).await?;
            ok(app.d.actions.create_project(req).await?)
        },
    );

    r.add(
        Method::GET,
        "/api/agents",
        false,
        |app: Arc<App>, _ctx: Ctx| async move { ok(app.d.actions.agents().await) },
    );

    r.add(
        Method::POST,
        "/api/streams",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let req: NewStream = json_body_as(&mut ctx, JSON_LIMIT).await?;
            ok(app.d.actions.create_stream(req).await?)
        },
    );

    // --- renaming and closing ---
    r.add(
        Method::POST,
        "/api/workspaces/:id/rename",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b: RenameBody = json_body_as(&mut ctx, JSON_LIMIT).await?;
            app.d
                .actions
                .rename_workspace(ctx.param("id"), b.name.as_deref())
                .await?;
            done()
        },
    );

    r.add(
        Method::POST,
        "/api/workspaces/:id/close",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            app.d.actions.close_workspace(ctx.param("id")).await?;
            done()
        },
    );

    r.add(
        Method::POST,
        "/api/projects/:id/rename",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b: RenameBody = json_body_as(&mut ctx, JSON_LIMIT).await?;
            app.d
                .actions
                .rename_project(ctx.param("id"), b.name.as_deref())
                .await?;
            done()
        },
    );

    r.add(
        Method::POST,
        "/api/projects/:id/close",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            app.d.actions.close_project(ctx.param("id")).await?;
            done()
        },
    );

    r.add(
        Method::POST,
        "/api/streams/:id/close",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            app.d.actions.close_stream(ctx.param("id")).await?;
            done()
        },
    );

    // --- stream actions ---
    r.add(
        Method::POST,
        "/api/streams/:id/rename",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b: RenameBody = json_body_as(&mut ctx, JSON_LIMIT).await?;
            app.d
                .actions
                .rename(ctx.param("id"), b.name.as_deref())
                .await?;
            done()
        },
    );

    r.add(
        Method::POST,
        "/api/streams/:id/seen",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let b: SeenBody = json_body_as(&mut ctx, JSON_LIMIT).await?;
            let seen = b.seen.unwrap_or(true);
            ok(app.d.actions.mark_seen(ctx.param("id"), seen).await?)
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
            let lines = ctx.query.get("lines").and_then(|n| n.parse().ok());
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
            let since_base = since_base(&ctx);
            let report = blocking(move || changes_report(&cwd, since_base)).await?;
            ok(ChangesBody::from(&report))
        },
    );

    r.add(
        Method::GET,
        "/api/streams/:id/changes/diff",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let cwd = PathBuf::from(s.cwd.unwrap_or_default());
            let since_base = since_base(&ctx);
            let report = blocking(move || {
                if since_base {
                    changes_since_base(&cwd, "git")
                } else {
                    changes(&cwd, "git")
                }
            })
            .await?;
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
            let against = match &report.base {
                Some(base) if report.since_base => base.commit.clone(),
                _ => head_tree(report.no_commits).to_string(),
            };
            match blocking(move || file_diff(&root, &file, &against, "git")).await? {
                Ok(d) => ok(DiffBody {
                    path: d.path,
                    text: d.text,
                    lines: d.lines,
                    truncated: d.truncated,
                    binary: d.binary,
                    signature,
                }),
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
                Ok(list) => ok(DocsBody {
                    docs: list.docs,
                    total: list.total,
                    truncated: list.truncated,
                    folder,
                    as_of: now_ms(),
                }),
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
            ok(doc)
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
                // SVGs and PDFs can carry script; served sandboxed, they only ever render (as images,
                // or through the Files tab's PDF viewer), never run.
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
            done()
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
            ok(DirBody {
                path: list.path,
                entries: list.entries,
                total: list.total,
                truncated: list.truncated,
                as_of: now_ms(),
            })
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
            // `in=contents` looks inside the files instead of at their names.
            if ctx.query.get("in") == Some("contents") {
                let found = blocking(move || {
                    let changed = changed_docs(&cwd).unwrap_or_default();
                    search_contents(&cwd, &query, &changed, "git")
                })
                .await?
                .map_err(docs_err)?;
                return ok(found);
            }
            let result = blocking(move || {
                let changed = changed_docs(&cwd).unwrap_or_default();
                search_files(&cwd, &query, &changed)
            })
            .await?
            .map_err(docs_err)?;
            ok(result)
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
            ok(FileBody {
                binary: f.text.is_none(),
                path: f.path,
                text: f.text,
                size: f.size,
            })
        },
    );

    // A link for showing the stream's HTML files as pages (see site.rs).
    r.add(
        Method::POST,
        "/api/streams/:id/files/site",
        false,
        |app: Arc<App>, ctx: Ctx| async move {
            let s = app.stream_with_folder(ctx.param("id"))?;
            let token = app.site_links.mint(&s.id, &ctx.device()?.id);
            ok(SiteLink { token })
        },
    );

    // --- uploads ---
    r.add(
        Method::POST,
        "/api/streams/:id/uploads",
        false,
        |app: Arc<App>, mut ctx: Ctx| async move {
            let id = ctx.param("id").to_string();
            // The upload lands on this host, where a terminal on another machine can't open it.
            app.stream_here(&id)?;
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
                    ok(Uploaded {
                        path: path.to_string_lossy().into_owned(),
                        quoted: quote_path(&path),
                        bytes,
                    })
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
                    ok(Dictated { text })
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
    use serde_json::Value;

    #[test]
    fn relative_paths_climb_out_with_dot_dot() {
        let from = Path::new("/a/b/c");
        assert_eq!(relative(from, Path::new("/a/b/c/d/e.md")), "d/e.md");
        assert_eq!(relative(from, Path::new("/a/x/y.md")), "../../x/y.md");
        assert_eq!(relative(from, from), "");
    }

    #[test]
    fn git_and_change_codes_are_the_ones_the_client_knows() {
        assert_eq!(git_code(&GitError::Timeout), "timeout");
        assert_eq!(git_code(&GitError::Failed("x".into())), "error");
        assert_eq!(change_code(ChangeKind::TypeChanged), "type");
        assert_eq!(state_code(ChangesState::GitMissing), "git_missing");
    }

    #[test]
    fn changes_answer_has_the_fields_the_client_reads() {
        let mut report = ChangesReport {
            state: ChangesState::Ok,
            message: None,
            root: Some(PathBuf::from("/repo")),
            branch: Some("feature".into()),
            detached: false,
            head_short: Some("abcdef0".into()),
            no_commits: false,
            files: vec![ChangedFile {
                path: "run.sh".into(),
                orig_path: Some("old.sh".into()),
                index: 'R',
                worktree: '.',
                staged: true,
                unstaged: false,
                untracked: false,
                conflicted: false,
                change: ChangeKind::Renamed,
                mode_change: Some(crate::git::ModeChange {
                    from: "100644".into(),
                    to: "100755".into(),
                }),
                binary: false,
                additions: Some(2),
                deletions: None,
                signature: "0123456789abcdef".into(),
            }],
            total_files: 1,
            truncated: false,
            as_of: 5,
            base: Some(crate::git::Base {
                name: "origin/main".into(),
                commit: "1234567890".repeat(4),
                ahead: 3,
            }),
            since_base: true,
        };
        let body = |r: &ChangesReport| serde_json::to_value(ChangesBody::from(r)).unwrap();
        assert_eq!(
            body(&report),
            json!({
                "state": "ok",
                "root": "/repo",
                "branch": "feature",
                "detached": false,
                "headShort": "abcdef0",
                "noCommits": false,
                "files": [{
                    "path": "run.sh",
                    "origPath": "old.sh",
                    "index": "R",
                    "worktree": ".",
                    "staged": true,
                    "unstaged": false,
                    "untracked": false,
                    "conflicted": false,
                    "change": "renamed",
                    "modeChange": {"from": "100644", "to": "100755"},
                    "binary": false,
                    "additions": 2,
                    "deletions": null,
                    "signature": "0123456789abcdef",
                }],
                "totalFiles": 1,
                "truncated": false,
                "asOf": 5,
                "base": {"name": "origin/main", "short": "1234567", "ahead": 3},
                "since": "base",
            })
        );
        report.state = ChangesState::Error;
        report.message = Some("Git took too long".into());
        report.base = None;
        report.since_base = false;
        let out = body(&report);
        assert_eq!(out["message"], "Git took too long");
        assert_eq!(
            (&out["base"], &out["since"]),
            (&Value::Null, &json!("head"))
        );
    }
}
