// Read-only Git review for the Changes surface.
// Safety rules, enforced in one place (build_argv / run_git):
//  - only read-only subcommands are allowed; anything else fails before a process starts;
//  - every call runs with --no-optional-locks and GIT_OPTIONAL_LOCKS=0, so `git status` never
//    refreshes or locks the index while an agent is using the repository;
//  - tracked changes are diffed with the plumbing command `diff-index`, which never refreshes the
//    index. Porcelain `git diff <commit>` is allowed only as `diff --no-index`;
//  - external diff drivers, textconv, fsmonitor hooks and pagers are disabled, so repository
//    config can't make Briardr run programs.
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha1::{Digest, Sha1};
use thiserror::Error;

pub const READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "rev-parse",
    "status",
    "diff-index",
    "diff",
    "ls-files",
    "symbolic-ref",
];
pub const MAX_FILES: usize = 500;
pub const MAX_DIFF_LINES: usize = 2000;
pub const GIT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OUTPUT: usize = 32 * 1024 * 1024;
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const SAFE_CONFIG: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.untrackedCache=false",
    "-c",
    "core.pager=cat",
    "-c",
    "diff.external=",
    "-c",
    "color.ui=false",
    "-c",
    "core.quotePath=false",
];

/// Error codes and messages keep their established values.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GitError {
    #[error("Git is not installed")]
    GitMissing,
    #[error("Not a Git repository")]
    NotRepo,
    #[error("{0}")]
    Forbidden(String),
    #[error("Git took too long")]
    Timeout,
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitResult {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Validate an allow-listed invocation and build the full argv the way `run_git` runs it.
/// Pure, so the safety rules can be tested without starting git.
pub fn build_argv(args: &[&str]) -> Result<Vec<String>, GitError> {
    let sub = args.first().copied().unwrap_or("");
    if !READ_ONLY_SUBCOMMANDS.contains(&sub) {
        return Err(GitError::Forbidden(format!("git {sub} is not allowed")));
    }
    if sub == "diff" && !args.contains(&"--no-index") {
        return Err(GitError::Forbidden(
            "git diff is only allowed with --no-index (use diff-index)".into(),
        ));
    }
    let mut rest: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    if (sub == "diff" || sub == "diff-index") && !args.contains(&"--no-ext-diff") {
        rest.splice(
            1..1,
            ["--no-ext-diff".to_string(), "--no-textconv".to_string()],
        );
    }
    let mut argv = vec!["--no-optional-locks".to_string()];
    argv.extend(SAFE_CONFIG.iter().map(|s| s.to_string()));
    argv.extend(rest);
    Ok(argv)
}

/// Run an allow-listed read-only git command in `cwd`. Non-zero exits listed in `ok_codes`
/// are results, not errors.
pub fn run_git(
    cwd: &Path,
    args: &[&str],
    ok_codes: &[i32],
    timeout: Duration,
    git_path: &str,
) -> Result<GitResult, GitError> {
    let argv = build_argv(args)?;
    let mut child = Command::new(git_path)
        .current_dir(cwd)
        .args(&argv)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::GitMissing
            } else {
                GitError::Failed("Git failed".into())
            }
        })?;

    // Both pipes are drained on their own threads so a chatty child can't block on a full pipe.
    // Output past MAX_OUTPUT sets `overflow`, and the wait loop kills the child, as Node's
    // maxBuffer does.
    let overflow = Arc::new(AtomicBool::new(false));
    let out_reader = thread::spawn({
        let pipe = child.stdout.take();
        let flag = Arc::clone(&overflow);
        move || read_capped(pipe, flag)
    });
    let err_reader = thread::spawn({
        let pipe = child.stderr.take();
        let flag = Arc::clone(&overflow);
        move || read_capped(pipe, flag)
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => return Err(GitError::Failed("Git failed".into())),
        }
        let overflowed = overflow.load(Ordering::SeqCst);
        if overflowed || started.elapsed() >= timeout {
            timed_out = !overflowed;
            let _ = child.kill();
            break child
                .wait()
                .map_err(|_| GitError::Failed("Git failed".into()))?;
        }
        thread::sleep(Duration::from_millis(5));
    };
    // A timed-out run returns at once. Its reader threads are not joined, since a grandchild
    // process could still hold the pipes open; they end when those pipes close.
    if timed_out {
        return Err(GitError::Timeout);
    }
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();

    // A run that hit the output cap resolves as code 0 with the partial stdout, as Node does
    // when its maxBuffer kills the child (the error carries no numeric exit code).
    let code = if overflow.load(Ordering::SeqCst) {
        0
    } else {
        status.code().unwrap_or(1)
    };
    if code != 0 && !ok_codes.contains(&code) {
        if stderr.to_lowercase().contains("not a git repository") {
            return Err(GitError::NotRepo);
        }
        let first: String = stderr
            .split('\n')
            .next()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        return Err(GitError::Failed(if first.is_empty() {
            "Git failed".into()
        } else {
            first
        }));
    }
    Ok(GitResult {
        code,
        stdout,
        stderr,
    })
}

/// Read a pipe to the end, keeping at most MAX_OUTPUT bytes. Past the cap the rest is drained
/// and discarded, so the child never blocks on a full pipe while it is being killed.
fn read_capped<R: Read>(pipe: Option<R>, overflow: Arc<AtomicBool>) -> Vec<u8> {
    let mut kept = Vec::new();
    let Some(mut pipe) = pipe else {
        return kept;
    };
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let take = n.min(MAX_OUTPUT - kept.len());
                kept.extend_from_slice(&chunk[..take]);
                if take < n {
                    overflow.store(true, Ordering::SeqCst);
                }
            }
        }
    }
    kept
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChange {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub orig_path: Option<String>,
    /// Porcelain v2 letters: index (staged) and worktree (unstaged). '.' means unchanged.
    pub index: char,
    pub worktree: char,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
    pub change: ChangeKind,
    pub mode_change: Option<ModeChange>,
    pub binary: bool,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    /// Changes whenever the file's content changes; drives "Reviewed on this device".
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangesState {
    Ok,
    Clean,
    NotRepo,
    GitMissing,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesReport {
    pub state: ChangesState,
    pub message: Option<String>,
    pub root: Option<PathBuf>,
    pub branch: Option<String>,
    pub detached: bool,
    pub head_short: Option<String>,
    pub no_commits: bool,
    pub files: Vec<ChangedFile>,
    pub total_files: usize,
    pub truncated: bool,
    pub as_of: u64,
}

/// `lstat` summary of a file: size, mtime in ms (as JS computes `mtimeMs`) and mode.
fn stat_sig(abs: &Path) -> String {
    match fs::symlink_metadata(abs) {
        Ok(md) => {
            let mtime_ms = md.mtime() as f64 * 1000.0 + md.mtime_nsec() as f64 / 1e6;
            format!("{}:{}:{}", md.len(), mtime_ms, md.mode())
        }
        Err(_) => "missing".to_string(),
    }
}

/// First 16 hex digits of SHA-1, the value stored in the existing data.
fn signature(input: &str) -> String {
    hex::encode(Sha1::digest(input.as_bytes()))[..16].to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn xy_chars(xy: &str) -> (char, char) {
    let mut it = xy.chars();
    (it.next().unwrap_or('.'), it.next().unwrap_or('.'))
}

pub fn changes(cwd: &Path, git_path: &str) -> ChangesReport {
    let mut report = ChangesReport {
        state: ChangesState::Ok,
        message: None,
        root: None,
        branch: None,
        detached: false,
        head_short: None,
        no_commits: false,
        files: Vec::new(),
        total_files: 0,
        truncated: false,
        as_of: now_ms(),
    };
    let root = match run_git(
        cwd,
        &["rev-parse", "--show-toplevel"],
        &[],
        GIT_TIMEOUT,
        git_path,
    ) {
        Ok(res) => PathBuf::from(OsStr::from_bytes(res.stdout.trim_ascii())),
        Err(GitError::NotRepo) => {
            report.state = ChangesState::NotRepo;
            return report;
        }
        Err(GitError::GitMissing) => {
            report.state = ChangesState::GitMissing;
            return report;
        }
        Err(e) => {
            report.state = ChangesState::Error;
            report.message = Some(e.to_string());
            return report;
        }
    };
    report.root = Some(root.clone());

    let status = match run_git(
        &root,
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
            "--find-renames",
        ],
        &[],
        GIT_TIMEOUT,
        git_path,
    ) {
        Ok(res) => res.stdout,
        Err(e) => {
            report.state = ChangesState::Error;
            report.message = Some(e.to_string());
            return report;
        }
    };

    let text = String::from_utf8_lossy(&status);
    let parts: Vec<&str> = text.split('\0').collect();
    let mut files = Vec::new();
    let mut oid = String::new();
    let mut i = 0;
    while i < parts.len() {
        let line = parts[i];
        i += 1;
        if line.is_empty() {
            continue;
        }
        if let Some(v) = line.strip_prefix("# branch.oid ") {
            oid = v.to_string();
        } else if let Some(h) = line.strip_prefix("# branch.head ") {
            if h == "(detached)" {
                report.detached = true;
            } else {
                report.branch = Some(h.to_string());
            }
        } else if line.starts_with('1') || line.starts_with('2') {
            let is_rename = line.starts_with('2');
            let need = if is_rename { 10 } else { 9 };
            let f: Vec<&str> = line.splitn(need, ' ').collect();
            if f.len() < need {
                continue;
            }
            let xy = f[1];
            let (m_h, m_i, m_w) = (f[3], f[4], f[5]);
            let (h_h, h_i) = (f[6], f[7]);
            let p = f[need - 1].to_string();
            let orig = if is_rename {
                let o = parts.get(i).map(|s| s.to_string());
                i += 1;
                o
            } else {
                None
            };
            let (index, worktree) = xy_chars(xy);
            let letter = if index != '.' { index } else { worktree };
            let change = if is_rename {
                ChangeKind::Renamed
            } else {
                match letter {
                    'A' => ChangeKind::Added,
                    'D' => ChangeKind::Deleted,
                    'R' => ChangeKind::Renamed,
                    'C' => ChangeKind::Copied,
                    'T' => ChangeKind::TypeChanged,
                    _ => ChangeKind::Modified,
                }
            };
            let mode_from = if m_h != "000000" { m_h } else { m_i };
            let mode_to = if m_w != "000000" { m_w } else { m_i };
            let mode_change =
                if mode_from != mode_to && mode_from != "000000" && mode_to != "000000" {
                    Some(ModeChange {
                        from: mode_from.to_string(),
                        to: mode_to.to_string(),
                    })
                } else {
                    None
                };
            let sig = signature(&format!(
                "{xy}|{m_i}|{m_w}|{h_h}|{h_i}|{}",
                stat_sig(&root.join(&p))
            ));
            files.push(ChangedFile {
                path: p,
                orig_path: orig,
                index,
                worktree,
                staged: index != '.',
                unstaged: worktree != '.',
                untracked: false,
                conflicted: false,
                change,
                mode_change,
                binary: false,
                additions: None,
                deletions: None,
                signature: sig,
            });
        } else if line.starts_with('u') {
            let f: Vec<&str> = line.splitn(11, ' ').collect();
            if f.len() < 11 {
                continue;
            }
            let p = f[10].to_string();
            let (index, worktree) = xy_chars(f[1]);
            let sig = signature(&format!("u|{}", stat_sig(&root.join(&p))));
            files.push(ChangedFile {
                path: p,
                orig_path: None,
                index,
                worktree,
                staged: true,
                unstaged: true,
                untracked: false,
                conflicted: true,
                change: ChangeKind::Conflict,
                mode_change: None,
                binary: false,
                additions: None,
                deletions: None,
                signature: sig,
            });
        } else if line.starts_with('?') {
            let p = line.get(2..).unwrap_or("").to_string();
            let sig = signature(&format!("?|{}", stat_sig(&root.join(&p))));
            files.push(ChangedFile {
                path: p,
                orig_path: None,
                index: '?',
                worktree: '?',
                staged: false,
                unstaged: true,
                untracked: true,
                conflicted: false,
                change: ChangeKind::Untracked,
                mode_change: None,
                binary: false,
                additions: None,
                deletions: None,
                signature: sig,
            });
        }
    }

    report.no_commits = oid == "(initial)";
    report.head_short = if report.no_commits {
        None
    } else {
        Some(oid.chars().take(7).collect())
    };
    report.total_files = files.len();
    report.truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    report.files = files;
    if report.files.is_empty() {
        report.state = ChangesState::Clean;
        return report;
    }

    // Line counts and binary detection for tracked changes against HEAD (or the empty tree).
    // Failures here are not fatal: the list still shows, just without counts.
    let against = if report.no_commits {
        EMPTY_TREE
    } else {
        "HEAD"
    };
    if let Ok(res) = run_git(
        &root,
        &["diff-index", "--numstat", "-z", "-M", against],
        &[],
        GIT_TIMEOUT,
        git_path,
    ) {
        apply_numstat(&mut report.files, &res.stdout);
    }
    for f in report.files.iter_mut() {
        if f.untracked {
            f.binary = is_binary_file(&root.join(&f.path));
        }
    }
    report
}

/// One `--numstat -z` record: `added\tdeleted\tpath`, where `-` marks binary.
/// Returns (binary, additions, deletions, path); a rename has an empty path.
fn parse_numstat(rec: &str) -> Option<(bool, Option<u64>, Option<u64>, &str)> {
    let mut it = rec.splitn(3, '\t');
    let (a, d, p) = (it.next()?, it.next()?, it.next()?);
    let num_ok = |s: &str| s == "-" || (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()));
    if !num_ok(a) || !num_ok(d) {
        return None;
    }
    // Regex `.` does not match these line terminators, so such a record never matches.
    if p.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let binary = a == "-";
    let additions = if binary { None } else { a.parse().ok() };
    let deletions = if binary { None } else { d.parse().ok() };
    Some((binary, additions, deletions, p))
}

fn apply_numstat(files: &mut [ChangedFile], out: &[u8]) {
    let text = String::from_utf8_lossy(out);
    let recs: Vec<&str> = text.split('\0').collect();
    let mut hits: Vec<(usize, bool, Option<u64>, Option<u64>)> = Vec::new();
    {
        let index: HashMap<&str, usize> = files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.as_str(), i))
            .collect();
        let mut i = 0;
        while i < recs.len() {
            let rec = recs[i];
            i += 1;
            if rec.is_empty() {
                continue;
            }
            let Some((binary, add, del, mut p)) = parse_numstat(rec) else {
                continue;
            };
            if p.is_empty() {
                // Rename: the old path comes first, the new path second.
                i += 1;
                p = recs.get(i).copied().unwrap_or("");
                i += 1;
            }
            if let Some(&idx) = index.get(p) {
                hits.push((idx, binary, add, del));
            }
        }
    }
    for (idx, binary, add, del) in hits {
        files[idx].binary = binary;
        files[idx].additions = add;
        files[idx].deletions = del;
    }
}

/// Binary if the first 8000 bytes contain a NUL. Read errors count as text.
fn is_binary_file(abs: &Path) -> bool {
    let mut buf = [0u8; 8000];
    match fs::File::open(abs).and_then(|mut f| f.read(&mut buf)) {
        Ok(n) => buf[..n].contains(&0),
        Err(_) => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub text: String,
    pub lines: usize,
    pub truncated: bool,
    pub binary: bool,
}

/// The file to diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub path: String,
    pub orig_path: Option<String>,
    pub untracked: bool,
}

/// Lexical path normalization, like Node's `path.resolve` (no symlink resolution).
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

fn is_binary_line(line: &str) -> bool {
    // Matches /^Binary files .* differ$/ on one line.
    line.strip_prefix("Binary files ")
        .and_then(|rest| rest.strip_suffix(" differ"))
        .is_some()
}

/// Unified diff for one file against HEAD, capped at MAX_DIFF_LINES lines.
pub fn file_diff(
    root: &Path,
    file: &FileRef,
    no_commits: bool,
    git_path: &str,
) -> Result<FileDiff, GitError> {
    let root_abs = normalize(root);
    let abs = if Path::new(&file.path).is_absolute() {
        normalize(Path::new(&file.path))
    } else {
        normalize(&root.join(&file.path))
    };
    if abs == root_abs || !abs.starts_with(&root_abs) {
        return Err(GitError::Forbidden("Path outside repository".into()));
    }

    let out = if file.untracked {
        // --no-index compares two paths on disk and never touches the repository or its index.
        run_git(
            root,
            &[
                "diff",
                "--no-index",
                "--no-color",
                "--",
                "/dev/null",
                &file.path,
            ],
            &[1],
            GIT_TIMEOUT,
            git_path,
        )?
    } else {
        let against = if no_commits { EMPTY_TREE } else { "HEAD" };
        let mut args = vec!["diff-index", "-p", "--no-color", "-M", against, "--"];
        if let Some(orig) = &file.orig_path {
            args.push(orig.as_str());
        }
        args.push(file.path.as_str());
        run_git(root, &args, &[], GIT_TIMEOUT, git_path)?
    };

    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let all: Vec<&str> = text.split('\n').collect();
    let binary = all.iter().any(|l| is_binary_line(l));
    let truncated = all.len() > MAX_DIFF_LINES;
    let body = if truncated {
        all[..MAX_DIFF_LINES].join("\n")
    } else {
        text.clone()
    };
    Ok(FileDiff {
        path: file.path.clone(),
        text: body,
        lines: all.len(),
        truncated,
        binary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::{write_executable, Scratch};

    fn strs(argv: &[String]) -> Vec<&str> {
        argv.iter().map(String::as_str).collect()
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn allowlist_refuses_mutating_and_porcelain_diff() {
        assert!(matches!(
            build_argv(&["commit", "-m", "x"]),
            Err(GitError::Forbidden(_))
        ));
        assert!(matches!(
            build_argv(&["checkout", "."]),
            Err(GitError::Forbidden(_))
        ));
        assert!(matches!(build_argv(&[]), Err(GitError::Forbidden(_))));
        assert!(matches!(
            build_argv(&["diff", "HEAD"]),
            Err(GitError::Forbidden(_))
        ));
        assert!(build_argv(&["diff", "--no-index", "--", "/dev/null", "a"]).is_ok());
        assert!(build_argv(&["status", "--porcelain=v2"]).is_ok());
    }

    #[test]
    fn argv_disables_locks_pagers_and_external_diff() {
        let argv = build_argv(&["diff-index", "--numstat", "HEAD"]).unwrap();
        let argv = strs(&argv);
        assert_eq!(argv[0], "--no-optional-locks");
        assert!(argv.contains(&"core.fsmonitor=false"));
        assert!(argv.contains(&"core.quotePath=false"));
        assert_eq!(
            &argv[argv.len() - 5..],
            [
                "diff-index",
                "--no-ext-diff",
                "--no-textconv",
                "--numstat",
                "HEAD"
            ]
        );
    }

    #[test]
    fn external_diff_flags_are_not_duplicated_when_given() {
        let argv = build_argv(&["diff-index", "--no-ext-diff", "-p", "HEAD"]).unwrap();
        let argv = strs(&argv);
        assert_eq!(argv.iter().filter(|a| **a == "--no-ext-diff").count(), 1);
        assert!(!argv.contains(&"--no-textconv"));
    }

    #[test]
    fn porcelain_status_parses_each_entry_kind() {
        let scratch = Scratch::new("git-status");
        let dir = scratch.path();
        git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("keep.txt"), "one\n").unwrap();
        fs::write(dir.join("gone.txt"), "x\n").unwrap();
        fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);

        fs::write(dir.join("keep.txt"), "one\ntwo\n").unwrap();
        fs::remove_file(dir.join("gone.txt")).unwrap();
        fs::write(dir.join("new.txt"), "hi\n").unwrap();
        git(dir, &["add", "new.txt"]);
        git(dir, &["mv", "bin.dat", "moved.dat"]);
        fs::write(dir.join("untracked.txt"), "a\n").unwrap();
        fs::write(dir.join("untracked.bin"), [0u8, 0, 1]).unwrap();

        let report = changes(dir, "git");
        assert_eq!(report.state, ChangesState::Ok, "{:?}", report.message);
        assert_eq!(report.branch.as_deref(), Some("main"));
        assert!(!report.detached);
        assert!(!report.no_commits);
        assert_eq!(report.head_short.as_ref().map(|s| s.len()), Some(7));
        assert_eq!(report.total_files, 6);
        assert!(!report.truncated);

        let by = |p: &str| report.files.iter().find(|f| f.path == p).unwrap();
        let keep = by("keep.txt");
        assert_eq!(keep.change, ChangeKind::Modified);
        assert_eq!((keep.additions, keep.deletions), (Some(1), Some(0)));
        assert!(!keep.binary && !keep.staged && keep.unstaged);
        assert_eq!(by("gone.txt").change, ChangeKind::Deleted);
        let new = by("new.txt");
        assert_eq!(new.change, ChangeKind::Added);
        assert!(new.staged);
        let moved = by("moved.dat");
        assert_eq!(moved.change, ChangeKind::Renamed);
        assert_eq!(moved.orig_path.as_deref(), Some("bin.dat"));
        assert!(moved.binary);
        let ut = by("untracked.txt");
        assert!(ut.untracked && !ut.binary);
        assert_eq!(ut.change, ChangeKind::Untracked);
        assert!(by("untracked.bin").binary);
        assert!(report.files.iter().all(|f| f.signature.len() == 16));

        let diff = file_diff(
            dir,
            &FileRef {
                path: "keep.txt".into(),
                orig_path: None,
                untracked: false,
            },
            false,
            "git",
        )
        .unwrap();
        assert!(diff.text.contains("+two"), "{}", diff.text);
        assert!(!diff.truncated && !diff.binary);

        let ud = file_diff(
            dir,
            &FileRef {
                path: "untracked.txt".into(),
                orig_path: None,
                untracked: true,
            },
            false,
            "git",
        )
        .unwrap();
        assert!(ud.text.contains("+a"), "{}", ud.text);
        let ub = file_diff(
            dir,
            &FileRef {
                path: "untracked.bin".into(),
                orig_path: None,
                untracked: true,
            },
            false,
            "git",
        )
        .unwrap();
        assert!(ub.binary);
    }

    #[test]
    fn file_diff_refuses_paths_outside_the_repository() {
        let root = Path::new("/repo/root");
        for p in ["../escape", "/etc/passwd", "sub/../../x", "."] {
            let file = FileRef {
                path: p.into(),
                orig_path: None,
                untracked: false,
            };
            assert_eq!(
                file_diff(root, &file, false, "git").unwrap_err(),
                GitError::Forbidden("Path outside repository".into()),
                "{p}"
            );
        }
    }

    #[test]
    fn numstat_records_parse_binary_and_rename_markers() {
        assert_eq!(
            parse_numstat("3\t1\tsrc/a.rs"),
            Some((false, Some(3), Some(1), "src/a.rs"))
        );
        assert_eq!(
            parse_numstat("-\t-\tpic.png"),
            Some((true, None, None, "pic.png"))
        );
        assert_eq!(parse_numstat("1\t1\t"), Some((false, Some(1), Some(1), "")));
        assert_eq!(parse_numstat("x\t1\tp"), None);
        assert_eq!(parse_numstat("1\t1"), None);
    }

    #[test]
    fn binary_line_matches_the_diff_marker_only() {
        assert!(is_binary_line("Binary files a/x and b/x differ"));
        assert!(is_binary_line("Binary files  differ"));
        assert!(!is_binary_line("Binary files differ"));
        assert!(!is_binary_line("+Binary files a and b differ"));
    }

    #[test]
    fn diff_truncates_at_the_line_cap() {
        let scratch = Scratch::new("git-trunc");
        let dir = scratch.path();
        git(dir, &["init", "-q", "-b", "main"]);
        let body: String = (0..MAX_DIFF_LINES + 50)
            .map(|i| format!("line {i}\n"))
            .collect();
        fs::write(dir.join("big.txt"), body).unwrap();
        let diff = file_diff(
            dir,
            &FileRef {
                path: "big.txt".into(),
                orig_path: None,
                untracked: true,
            },
            true,
            "git",
        )
        .unwrap();
        assert!(diff.truncated);
        assert!(diff.lines > MAX_DIFF_LINES);
        assert_eq!(diff.text.split('\n').count(), MAX_DIFF_LINES);
    }

    #[test]
    fn missing_git_binary_is_reported_as_git_missing() {
        let scratch = Scratch::new("git-missing");
        let err = run_git(
            scratch.path(),
            &["status"],
            &[],
            GIT_TIMEOUT,
            "/nonexistent/briardr-git",
        )
        .unwrap_err();
        assert_eq!(err, GitError::GitMissing);
    }

    #[test]
    fn slow_git_is_killed_at_the_timeout() {
        let scratch = Scratch::new("git-slow");
        let script = scratch.join("fake-git");
        write_executable(&script, "#!/bin/sh\nsleep 5\n");
        let started = Instant::now();
        let err = run_git(
            scratch.path(),
            &["status"],
            &[],
            Duration::from_millis(200),
            script.to_str().unwrap(),
        )
        .unwrap_err();
        assert_eq!(err, GitError::Timeout);
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn output_past_the_cap_is_killed_and_returned_as_partial_success() {
        let scratch = Scratch::new("git-overflow");
        let script = scratch.join("fake-git");
        write_executable(&script, "#!/bin/sh\nhead -c 40000000 /dev/zero\n");
        let res = run_git(
            scratch.path(),
            &["status"],
            &[],
            GIT_TIMEOUT,
            script.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(res.code, 0);
        assert_eq!(res.stdout.len(), MAX_OUTPUT);
    }
}
