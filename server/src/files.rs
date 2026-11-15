// The Files surface: browsing, searching, reading and deleting entries, confined to the stream's
// folder exactly like Docs (every path goes through `resolve_inside`).
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::ops::ControlFlow;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::docs::{locale_compare, resolve_inside, DocsError};
use crate::git::{run_git, GitError};

pub const MAX_ENTRIES: usize = 2000;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Bytes checked for a NUL when deciding whether a file is binary (git's heuristic).
const SNIFF_BYTES: usize = 8000;
/// Never listed: git internals and Finder droppings.
const HIDDEN: &[&str] = &[".git", ".DS_Store"];
pub const MAX_SEARCH_RESULTS: usize = 200;
pub const MAX_CONTENT_MATCHES: usize = 200;
/// A file full of matches says little more after the first few.
const MAX_MATCHES_PER_FILE: usize = 20;
/// Characters of a matching line that are sent.
const MAX_MATCH_TEXT: usize = 240;
/// How long a search inside files reads before it reports what it has found.
const CONTENT_SEARCH_BUDGET: Duration = Duration::from_millis(1500);
const MAX_SEARCH_VISITED: usize = 50_000;
const MAX_SEARCH_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub link: bool,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirList {
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileText {
    pub path: String,
    /// None for binary files.
    pub text: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSearch {
    pub entries: Vec<FileEntry>,
    /// Results or traversal were capped; more matches may exist.
    pub truncated: bool,
}

/// A file a search comes by: its name, its path under the root, and where and how big it is (a
/// link's target).
struct Found {
    name: String,
    path: String,
    abs: PathBuf,
    size: u64,
    link: bool,
}

/// Visits the files under `root` whose name `wanted` accepts, breadth first, until `visit` breaks, the
/// walk has seen MAX_SEARCH_VISITED entries or has taken `budget`. Never follows directory symlinks
/// (which can cycle), and only includes file symlinks whose targets remain inside the root. Returns
/// whether it stopped before the end, so more may exist.
fn walk_files(
    root: &Path,
    budget: Duration,
    wanted: impl Fn(&str) -> bool,
    mut visit: impl FnMut(Found) -> ControlFlow<()>,
) -> Result<bool, DocsError> {
    let start = Instant::now();
    let mut pending = VecDeque::from([(root.to_path_buf(), 0)]);
    let mut visited = 0;
    let mut cut = false;
    while let Some((dir, depth)) = pending.pop_front() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if dir == root => return Err(e.into()),
            Err(_) => {
                cut = true;
                continue;
            }
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_SEARCH_VISITED || start.elapsed() > budget {
                return Ok(true);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if HIDDEN.contains(&name.as_str()) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < MAX_SEARCH_DEPTH {
                    pending.push_back((entry.path(), depth + 1));
                } else {
                    cut = true;
                }
                continue;
            }
            if !wanted(&name) {
                continue;
            }
            let link = kind.is_symlink();
            let Some((abs, size)) = file_inside(root, entry.path(), link) else {
                continue;
            };
            let path = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(&entry.path())
                .to_string_lossy()
                .into_owned();
            let found = Found {
                name,
                path,
                abs,
                size,
                link,
            };
            if visit(found).is_break() {
                return Ok(true);
            }
        }
    }
    Ok(cut)
}

/// Where the file at `at` is (a link's target) and its size, when it is a file inside `root`. The folder
/// it is in is the caller's to check.
fn file_inside(root: &Path, at: PathBuf, link: bool) -> Option<(PathBuf, u64)> {
    let abs = if link {
        fs::canonicalize(&at)
            .ok()
            .filter(|real| real.starts_with(root))?
    } else {
        at
    };
    let meta = fs::metadata(&abs).ok().filter(|meta| meta.is_file())?;
    Some((abs, meta.len()))
}

/// Case-insensitive filename search, including unopened folders (see `walk_files` for what it visits).
pub fn search_files(
    folder: &Path,
    query: &str,
    changed: &HashSet<String>,
) -> Result<FileSearch, DocsError> {
    let root = fs::canonicalize(folder)?;
    let query = query.trim().to_lowercase();
    let mut result = FileSearch {
        entries: Vec::new(),
        truncated: false,
    };
    if query.is_empty() {
        return Ok(result);
    }
    let entries = &mut result.entries;
    result.truncated = walk_files(
        &root,
        Duration::from_secs(1),
        |name| name.to_lowercase().contains(&query),
        |f| {
            if entries.len() == MAX_SEARCH_RESULTS {
                return ControlFlow::Break(());
            }
            entries.push(FileEntry {
                changed: changed.contains(&f.path),
                name: f.name,
                path: f.path,
                dir: false,
                size: f.size,
                link: f.link,
            });
            ControlFlow::Continue(())
        },
    )?;
    result.entries.sort_by(|a, b| {
        let rank = |e: &FileEntry| {
            let name = e.name.to_lowercase();
            if name == query {
                0
            } else if name.starts_with(&query) {
                1
            } else {
                2
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| locale_compare(&a.path, &b.path))
    });
    Ok(result)
}

/// A line that contains what was searched for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentMatch {
    pub path: String,
    /// 1-based.
    pub line: u64,
    /// The line, or the part of a long one around the match.
    pub text: String,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentSearch {
    pub matches: Vec<ContentMatch>,
    /// Results or the search were capped; more matches may exist.
    pub truncated: bool,
}

/// Case-insensitive search for the text `query` (never a pattern) in the folder's text files of at most
/// MAX_FILE_BYTES, for CONTENT_SEARCH_BUDGET at most. In a Git repository only the files Git lists are
/// read, tracked and untracked: what `.gitignore` names (dependencies, build output) is left out, which
/// keeps the search quick and its results the project's own. Elsewhere, or where Git lists nothing (a
/// folder it ignores), the files `walk_files` visits are read. Every file read is inside the folder.
pub fn search_contents(
    folder: &Path,
    query: &str,
    changed: &HashSet<String>,
    git_path: &str,
) -> Result<ContentSearch, DocsError> {
    let root = fs::canonicalize(folder)?;
    let needle = query.trim().to_lowercase();
    let mut result = ContentSearch {
        matches: Vec::new(),
        truncated: false,
    };
    if needle.is_empty() {
        return Ok(result);
    }
    let start = Instant::now();
    let matches = &mut result.matches;
    let mut look = |abs: &Path, path: &str, size: u64| {
        if size > MAX_FILE_BYTES {
            return ControlFlow::Continue(());
        }
        search_file(abs, path, changed.contains(path), &needle, matches)
    };
    result.truncated = match git_files(&root, git_path) {
        Ok(listed) if !listed.is_empty() => {
            // Whether each folder with listed files is inside the root (one may have become a link
            // since Git saw it), resolved once for all its files.
            let mut inside = HashMap::new();
            listed
                .iter()
                .try_for_each(|rel| {
                    if start.elapsed() > CONTENT_SEARCH_BUDGET {
                        return ControlFlow::Break(());
                    }
                    let dir = rel.parent().unwrap_or(Path::new(""));
                    if !*inside.entry(dir).or_insert_with(|| {
                        fs::canonicalize(root.join(dir)).is_ok_and(|real| real.starts_with(&root))
                    }) {
                        return ControlFlow::Continue(());
                    }
                    let at = root.join(rel);
                    let link = fs::symlink_metadata(&at).is_ok_and(|meta| meta.is_symlink());
                    match file_inside(&root, at, link) {
                        Some((abs, size)) => look(&abs, &rel.to_string_lossy(), size),
                        None => ControlFlow::Continue(()),
                    }
                })
                .is_break()
        }
        Err(GitError::Timeout) => true,
        _ => walk_files(
            &root,
            CONTENT_SEARCH_BUDGET.saturating_sub(start.elapsed()),
            |_| true,
            |f| look(&f.abs, &f.path, f.size),
        )?,
    };
    result
        .matches
        .sort_by(|a, b| locale_compare(&a.path, &b.path).then(a.line.cmp(&b.line)));
    Ok(result)
}

/// The files Git lists in `root`, relative to it and in order: tracked and untracked ones, without what
/// it ignores.
fn git_files(root: &Path, git_path: &str) -> Result<Vec<PathBuf>, GitError> {
    let listed = run_git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        &[],
        CONTENT_SEARCH_BUDGET,
        git_path,
    )?;
    let mut paths: Vec<&[u8]> = listed.stdout.split(|b| *b == 0).collect();
    // After the last NUL: nothing, or a path the output cap cut short.
    paths.pop();
    // A file in a merge conflict is listed once per side.
    paths.sort_unstable();
    paths.dedup();
    Ok(paths
        .into_iter()
        .map(|p| PathBuf::from(OsStr::from_bytes(p)))
        .collect())
}

/// Adds the lines of the text file at `abs` that contain `needle` (lowercase), at most
/// MAX_MATCHES_PER_FILE of them, to `matches` as `path`'s. Breaks at a match beyond
/// MAX_CONTENT_MATCHES.
fn search_file(
    abs: &Path,
    path: &str,
    changed: bool,
    needle: &str,
    matches: &mut Vec<ContentMatch>,
) -> ControlFlow<()> {
    let Ok(bytes) = fs::read(abs) else {
        return ControlFlow::Continue(());
    };
    if is_binary(&bytes) {
        return ControlFlow::Continue(());
    }
    let text = String::from_utf8_lossy(&bytes);
    // Lowercasing keeps every line break, so the lowercase lines pair up with the file's.
    let lower = text.to_lowercase();
    if !lower.contains(needle) {
        return ControlFlow::Continue(());
    }
    let hits = text
        .lines()
        .zip(lower.lines())
        .enumerate()
        .filter_map(|(i, (line, low))| low.find(needle).map(|at| (i, line, at)))
        .take(MAX_MATCHES_PER_FILE);
    for (i, line, at) in hits {
        if matches.len() == MAX_CONTENT_MATCHES {
            return ControlFlow::Break(());
        }
        matches.push(ContentMatch {
            path: path.to_string(),
            line: i as u64 + 1,
            // Lowercasing can move byte offsets; the excerpt only needs to be near.
            text: excerpt(line, at.min(line.len())),
            changed,
        });
    }
    ControlFlow::Continue(())
}

/// A NUL among the first bytes means binary (git's heuristic).
fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0)
}

/// A matching line for the results: without its indentation, and for a long line (minified code) only
/// the part around the match, which starts at byte `at`.
fn excerpt(line: &str, at: usize) -> String {
    let line = line.trim_end_matches('\r');
    let indent = line.len() - line.trim_start().len();
    let chars: Vec<(usize, char)> = line
        .char_indices()
        .skip_while(|(i, _)| *i < indent)
        .collect();
    if chars.len() <= MAX_MATCH_TEXT {
        return chars.iter().map(|(_, c)| c).collect();
    }
    let hit = chars.iter().position(|(i, _)| *i >= at).unwrap_or(0);
    let from = hit
        .saturating_sub(MAX_MATCH_TEXT / 4)
        .min(chars.len() - MAX_MATCH_TEXT);
    let mut out: String = chars[from..from + MAX_MATCH_TEXT]
        .iter()
        .map(|(_, c)| c)
        .collect();
    if from > 0 {
        out.insert(0, '…');
    }
    if from + MAX_MATCH_TEXT < chars.len() {
        out.push('…');
    }
    out
}

/// The entries of one folder inside `folder`: folders first, then files, by name. Symlinks are
/// listed only when they resolve inside the folder. `changed` holds paths relative to the folder;
/// a folder counts as changed when anything under it is.
pub fn list_dir(folder: &Path, rel: &str, changed: &HashSet<String>) -> Result<DirList, DocsError> {
    let root = fs::canonicalize(folder)?;
    let abs = resolve_inside(folder, rel)?;
    if !abs.is_dir() {
        return Err(DocsError::NotFound("Not a folder"));
    }
    let base = abs.strip_prefix(&root).unwrap_or(Path::new(""));
    let mut entries = Vec::new();
    for entry in fs::read_dir(&abs)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let link = file_type.is_symlink();
        let meta = if link {
            match fs::canonicalize(entry.path()) {
                Ok(real) if real.starts_with(&root) => fs::metadata(&real),
                _ => continue,
            }
        } else {
            entry.metadata()
        };
        let Ok(meta) = meta else {
            continue;
        };
        if !meta.is_dir() && !meta.is_file() {
            continue;
        }
        let path = base.join(&name).to_string_lossy().into_owned();
        let dir = meta.is_dir();
        let prefix = format!("{path}/");
        entries.push(FileEntry {
            changed: if dir {
                changed.iter().any(|c| c.starts_with(&prefix))
            } else {
                changed.contains(&path)
            },
            size: if dir { 0 } else { meta.len() },
            name,
            path,
            dir,
            link,
        });
    }
    entries.sort_by(compare_entries);
    let total = entries.len();
    entries.truncate(MAX_ENTRIES);
    Ok(DirList {
        path: base.to_string_lossy().into_owned(),
        entries,
        total,
        truncated: total > MAX_ENTRIES,
    })
}

/// A file's text for viewing, up to 1 MB. Files with a NUL byte near the start count as binary.
pub fn read_file(folder: &Path, rel: &str) -> Result<FileText, DocsError> {
    let root = fs::canonicalize(folder)?;
    let abs = resolve_inside(folder, rel)?;
    let meta = fs::metadata(&abs)?;
    if !meta.is_file() {
        return Err(DocsError::NotFound("Not a file"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(DocsError::TooLarge(format!(
            "This file is {:.1} MB; Files opens files up to 1 MB",
            meta.len() as f64 / 1024.0 / 1024.0
        )));
    }
    let bytes = fs::read(&abs)?;
    let binary = is_binary(&bytes);
    Ok(FileText {
        path: abs
            .strip_prefix(&root)
            .unwrap_or(&abs)
            .to_string_lossy()
            .into_owned(),
        text: (!binary).then(|| String::from_utf8_lossy(&bytes).into_owned()),
        size: meta.len(),
    })
}

/// Delete one entry, never the stream root or git internals. Resolve the parent separately so a
/// symlink is removed as a link, rather than deleting the file or directory it points to.
pub fn delete_entry(folder: &Path, rel: &str) -> Result<(), DocsError> {
    let path = Path::new(rel);
    if rel.is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(name) if name != ".git"))
    {
        return Err(DocsError::Outside(
            "Choose a file or folder inside this stream’s folder",
        ));
    }
    let root = fs::canonicalize(folder)?;
    let real = resolve_inside(&root, rel)?;
    if real == root {
        return Err(DocsError::Outside(
            "The stream’s root folder cannot be deleted",
        ));
    }
    if real
        .strip_prefix(&root)
        .unwrap()
        .components()
        .any(|c| c.as_os_str() == ".git")
    {
        return Err(DocsError::Outside(
            "Git internals cannot be deleted from Files",
        ));
    }
    let parent = resolve_inside(
        &root,
        path.parent()
            .unwrap_or(Path::new(""))
            .to_str()
            .ok_or(DocsError::Outside("Invalid path"))?,
    )?;
    let entry = parent.join(path.file_name().ok_or(DocsError::Outside("Invalid path"))?);
    let meta = fs::symlink_metadata(&entry)?;
    if meta.is_dir() {
        fs::remove_dir_all(entry)?;
    } else {
        fs::remove_file(entry)?;
    }
    Ok(())
}

fn compare_entries(a: &FileEntry, b: &FileEntry) -> Ordering {
    b.dir
        .cmp(&a.dir)
        .then_with(|| locale_compare(&a.name, &b.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::{git, write_executable, Scratch};
    use std::path::PathBuf;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// `base/secret.txt` outside, `base/root/` as the stream's folder.
    fn layout() -> (Scratch, PathBuf) {
        let base = Scratch::new("files");
        write(&base.join("secret.txt"), b"secret");
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        (base, root)
    }

    fn names(list: &DirList) -> Vec<&str> {
        list.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn lists_folders_first_then_files_and_hides_git() {
        let (_base, root) = layout();
        write(&root.join("b.rs"), b"fn main() {}");
        write(&root.join("A.txt"), b"a");
        write(&root.join("src/lib.rs"), b"x");
        write(&root.join(".github/ci.yml"), b"x");
        write(&root.join(".git/HEAD"), b"x");
        write(&root.join(".DS_Store"), b"x");
        let list = list_dir(&root, "", &HashSet::new()).unwrap();
        assert_eq!(names(&list), [".github", "src", "A.txt", "b.rs"]);
        assert_eq!(list.path, "");
        let b = &list.entries[3];
        assert!(!b.dir && b.size == 12 && b.path == "b.rs");
        let sub = list_dir(&root, "src", &HashSet::new()).unwrap();
        assert_eq!(sub.path, "src");
        assert_eq!(sub.entries[0].path, "src/lib.rs");
    }

    #[test]
    fn marks_changed_files_and_the_folders_above_them() {
        let (_base, root) = layout();
        write(&root.join("src/a/x.rs"), b"x");
        write(&root.join("src/b.rs"), b"x");
        write(&root.join("srcx/c.rs"), b"x");
        let changed = HashSet::from(["src/a/x.rs".to_string()]);
        let top = list_dir(&root, "", &changed).unwrap();
        assert!(
            top.entries
                .iter()
                .find(|e| e.name == "src")
                .unwrap()
                .changed
        );
        assert!(
            !top.entries
                .iter()
                .find(|e| e.name == "srcx")
                .unwrap()
                .changed
        );
        let src = list_dir(&root, "src", &changed).unwrap();
        assert!(src.entries.iter().find(|e| e.name == "a").unwrap().changed);
        assert!(
            !src.entries
                .iter()
                .find(|e| e.name == "b.rs")
                .unwrap()
                .changed
        );
    }

    #[cfg(unix)]
    #[test]
    fn lists_symlinks_only_when_they_stay_inside() {
        let (base, root) = layout();
        write(&root.join("real/x.txt"), b"x");
        std::os::unix::fs::symlink(root.join("real"), root.join("inside")).unwrap();
        std::os::unix::fs::symlink(base.join("secret.txt"), root.join("out.txt")).unwrap();
        std::os::unix::fs::symlink(base.path(), root.join("outdir")).unwrap();
        let list = list_dir(&root, "", &HashSet::new()).unwrap();
        assert_eq!(names(&list), ["inside", "real"]);
        assert!(list.entries[0].link && list.entries[0].dir);
        assert!(matches!(
            list_dir(&root, "outdir", &HashSet::new()),
            Err(DocsError::Outside(_))
        ));
        assert!(matches!(
            read_file(&root, "out.txt"),
            Err(DocsError::Outside(_))
        ));
    }

    #[test]
    fn refuses_escapes_and_non_folders() {
        let (_base, root) = layout();
        write(&root.join("f.txt"), b"x");
        assert!(matches!(
            list_dir(&root, "..", &HashSet::new()),
            Err(DocsError::Outside(_))
        ));
        assert!(matches!(
            read_file(&root, "../secret.txt"),
            Err(DocsError::Outside(_))
        ));
        assert!(matches!(
            list_dir(&root, "f.txt", &HashSet::new()),
            Err(DocsError::NotFound("Not a folder"))
        ));
        assert!(matches!(
            read_file(&root, ""),
            Err(DocsError::NotFound("Not a file"))
        ));
    }

    #[test]
    fn caps_the_listing() {
        let (_base, root) = layout();
        for i in 0..MAX_ENTRIES + 3 {
            write(&root.join(format!("f{i:04}")), b"");
        }
        let list = list_dir(&root, "", &HashSet::new()).unwrap();
        assert_eq!(list.entries.len(), MAX_ENTRIES);
        assert_eq!(list.total, MAX_ENTRIES + 3);
        assert!(list.truncated);
    }

    #[test]
    fn searches_nested_filenames_case_insensitively_and_preserves_change_marks() {
        let (_base, root) = layout();
        write(&root.join("src/deep/Needle.ts"), b"x");
        write(&root.join("needle.txt"), b"x");
        write(&root.join("other.txt"), b"needle is only in the contents");
        write(&root.join("needle-folder/unrelated.txt"), b"x");
        write(&root.join(".git/needle"), b"x");
        write(&root.join(".hidden/needle.env"), b"x");
        let changed = HashSet::from(["src/deep/Needle.ts".to_string()]);
        let result = search_files(&root, " NEEDLE ", &changed).unwrap();
        let paths: Vec<_> = result.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [".hidden/needle.env", "needle.txt", "src/deep/Needle.ts"]
        );
        assert!(result.entries[2].changed);
        assert!(result.entries.iter().all(|e| !e.dir));
        assert!(!result.truncated);
        assert!(search_files(&root, "missing", &changed)
            .unwrap()
            .entries
            .is_empty());
        assert!(search_files(&root, "  ", &changed)
            .unwrap()
            .entries
            .is_empty());
    }

    /// (path, line, text) of each match.
    fn found(result: &ContentSearch) -> Vec<(&str, u64, &str)> {
        result
            .matches
            .iter()
            .map(|m| (m.path.as_str(), m.line, m.text.as_str()))
            .collect()
    }

    #[test]
    fn content_search_outside_a_repository_reads_text_files_and_skips_binary_and_big_ones() {
        let (_scratch, root) = layout();
        write(
            &root.join("src/a.rs"),
            b"fn main() {\n    let Needle = 1;\n}\n",
        );
        write(
            &root.join("notes.md"),
            "first\r\nthe NEEDLE again, ÜBER\r\n".as_bytes(),
        );
        write(&root.join("bin.dat"), b"needle\0binary");
        write(&root.join(".git/config"), b"needle in git internals");
        let mut big = b"needle\n".to_vec();
        big.resize(MAX_FILE_BYTES as usize + 1, b'x');
        write(&root.join("big.log"), &big);
        let changed = HashSet::from(["notes.md".to_string()]);
        let result = search_contents(&root, " needle ", &changed, "git").unwrap();
        assert_eq!(
            found(&result),
            [
                ("notes.md", 2, "the NEEDLE again, ÜBER"),
                ("src/a.rs", 2, "let Needle = 1;")
            ]
        );
        assert!(result.matches[0].changed && !result.matches[1].changed);
        assert!(!result.truncated);
        assert_eq!(
            found(&search_contents(&root, "über", &changed, "git").unwrap()),
            [("notes.md", 2, "the NEEDLE again, ÜBER")]
        );
        assert!(search_contents(&root, "  ", &changed, "git")
            .unwrap()
            .matches
            .is_empty());
        // The same without Git on the host.
        let no_git = search_contents(&root, "needle", &changed, "/nonexistent/git").unwrap();
        assert_eq!(found(&no_git), found(&result));
    }

    #[test]
    fn content_search_in_a_repository_leaves_out_what_git_ignores() {
        let (_scratch, root) = layout();
        git(&root, &["init", "-q"]);
        write(&root.join(".gitignore"), b"node_modules/\n");
        write(&root.join("src/tracked.rs"), b"// needle, tracked\n");
        write(
            &root.join("node_modules/dep/index.js"),
            b"needle in a dependency\n",
        );
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        write(&root.join("untracked.txt"), b"one\nan untracked needle\n");
        // What is typed is looked for as it is, not as a pattern.
        write(&root.join("pattern.txt"), b"needles\nneedle.*(\n");

        let result = search_contents(&root, "needle", &HashSet::new(), "git").unwrap();
        assert_eq!(
            found(&result),
            [
                ("pattern.txt", 1, "needles"),
                ("pattern.txt", 2, "needle.*("),
                ("src/tracked.rs", 1, "// needle, tracked"),
                ("untracked.txt", 2, "an untracked needle")
            ]
        );
        let pattern = search_contents(&root, "needle.*(", &HashSet::new(), "git").unwrap();
        assert_eq!(found(&pattern), [("pattern.txt", 2, "needle.*(")]);
        // From a folder inside the repository, paths are that folder's.
        let inside = search_contents(&root.join("src"), "needle", &HashSet::new(), "git").unwrap();
        assert_eq!(found(&inside), [("tracked.rs", 1, "// needle, tracked")]);
        // A folder Git ignores as a whole is read like any other.
        let ignored =
            search_contents(&root.join("node_modules"), "needle", &HashSet::new(), "git").unwrap();
        assert_eq!(
            found(&ignored),
            [("dep/index.js", 1, "needle in a dependency")]
        );
    }

    #[test]
    fn content_search_stops_when_git_takes_too_long() {
        let (base, root) = layout();
        write(&root.join("a.txt"), b"needle");
        let slow_git = base.join("git");
        write_executable(&slow_git, "#!/bin/sh\nexec sleep 10\n");
        let started = Instant::now();
        let result =
            search_contents(&root, "needle", &HashSet::new(), slow_git.to_str().unwrap()).unwrap();
        // Without Git's list, nothing is read: the folder could be all dependencies.
        assert!(result.matches.is_empty() && result.truncated);
        assert!(started.elapsed() < CONTENT_SEARCH_BUDGET * 2);
    }

    #[cfg(unix)]
    #[test]
    fn content_search_reads_nothing_outside_the_folder() {
        let (base, root) = layout();
        write(&base.join("secret.txt"), b"needle outside");
        write(&base.join("elsewhere/x.txt"), b"needle outside");
        write(&root.join("real/inside.txt"), b"needle inside");
        std::os::unix::fs::symlink(root.join("real/inside.txt"), root.join("alias.txt")).unwrap();
        std::os::unix::fs::symlink(base.join("secret.txt"), root.join("escape.txt")).unwrap();
        std::os::unix::fs::symlink(base.path(), root.join("outside")).unwrap();
        let walked = search_contents(&root, "needle", &HashSet::new(), "git").unwrap();
        assert_eq!(
            found(&walked),
            [
                ("alias.txt", 1, "needle inside"),
                ("real/inside.txt", 1, "needle inside")
            ]
        );

        // In a repository, Git lists the links (not what they lead to) and a file under a folder that
        // has since become a link to outside. (The ignored file shows it's Git's list that is read.)
        git(&root, &["init", "-q"]);
        write(&root.join(".gitignore"), b"ignored.txt\n");
        write(&root.join("ignored.txt"), b"needle ignored");
        write(&root.join("swap/x.txt"), b"needle before");
        git(&root, &["add", "."]);
        fs::remove_dir_all(root.join("swap")).unwrap();
        std::os::unix::fs::symlink(base.join("elsewhere"), root.join("swap")).unwrap();
        let listed = search_contents(&root, "needle", &HashSet::new(), "git").unwrap();
        assert_eq!(found(&listed), found(&walked));
    }

    #[test]
    fn content_search_caps_matches_per_file_and_in_all_and_shortens_long_lines() {
        let (_scratch, root) = layout();
        for i in 0..15 {
            write(
                &root.join(format!("f{i:02}.txt")),
                "match\n".repeat(30).as_bytes(),
            );
        }
        let result = search_contents(&root, "match", &HashSet::new(), "git").unwrap();
        assert_eq!(result.matches.len(), MAX_CONTENT_MATCHES);
        assert!(result.truncated);
        // Which files come first is the file system's order (sorted on APFS, not on ext4).
        let mut per_file = HashMap::new();
        for m in &result.matches {
            *per_file.entry(m.path.as_str()).or_insert(0) += 1;
        }
        assert!(
            per_file.values().all(|n| *n == MAX_MATCHES_PER_FILE),
            "{per_file:?}"
        );

        let long = format!("{}needle{}", "a".repeat(1000), "b".repeat(1000));
        let cut = excerpt(&long, 1000);
        assert_eq!(cut.chars().count(), MAX_MATCH_TEXT + 2);
        assert!(cut.starts_with('…') && cut.ends_with('…') && cut.contains("needle"));
        assert_eq!(excerpt("\t  indented é line\r", 5), "indented é line");
    }

    #[cfg(unix)]
    #[test]
    fn search_confines_links_and_does_not_follow_directory_cycles() {
        let (base, root) = layout();
        write(&root.join("real/inside.txt"), b"x");
        std::os::unix::fs::symlink(root.join("real/inside.txt"), root.join("alias.txt")).unwrap();
        std::os::unix::fs::symlink(base.join("secret.txt"), root.join("escape.txt")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("cycle")).unwrap();
        std::os::unix::fs::symlink(base.path(), root.join("outside")).unwrap();
        let result = search_files(&root, ".txt", &HashSet::new()).unwrap();
        assert_eq!(
            result
                .entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["alias.txt", "real/inside.txt"]
        );
        assert!(result.entries[0].link);
        assert!(!result.truncated);
    }

    #[test]
    fn caps_search_results_and_reports_partial_searches() {
        let (_base, root) = layout();
        for i in 0..MAX_SEARCH_RESULTS + 1 {
            write(&root.join(format!("match{i}.txt")), b"");
        }
        let result = search_files(&root, "match", &HashSet::new()).unwrap();
        assert_eq!(result.entries.len(), MAX_SEARCH_RESULTS);
        assert!(result.truncated);
    }

    #[test]
    fn deletes_files_and_nonempty_folders_but_never_the_root_or_escapes() {
        let (base, root) = layout();
        write(&root.join("one.txt"), b"x");
        write(&root.join("nested/deep/two.txt"), b"x");
        write(&root.join(".git/HEAD"), b"keep");
        for rel in [
            "",
            ".",
            "..",
            "../secret.txt",
            "nested/../one.txt",
            ".git",
            ".git/HEAD",
        ] {
            assert!(
                matches!(delete_entry(&root, rel), Err(DocsError::Outside(_))),
                "{rel}"
            );
        }
        assert!(matches!(
            delete_entry(&root, root.to_str().unwrap()),
            Err(DocsError::Outside(_))
        ));
        assert_eq!(fs::read(base.join("secret.txt")).unwrap(), b"secret");
        delete_entry(&root, "one.txt").unwrap();
        delete_entry(&root, "nested").unwrap();
        assert!(!root.join("one.txt").exists());
        assert!(!root.join("nested").exists());
        assert!(root.join(".git/HEAD").exists());
        assert!(delete_entry(&root, "missing.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn deletion_unlinks_symlinks_without_deleting_their_targets() {
        let (base, root) = layout();
        write(&root.join("real/keep.txt"), b"keep");
        write(&root.join(".git/HEAD"), b"keep");
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
        std::os::unix::fs::symlink(root.join("real/keep.txt"), root.join("alias.txt")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("root-alias")).unwrap();
        std::os::unix::fs::symlink(root.join(".git"), root.join("git-alias")).unwrap();
        std::os::unix::fs::symlink(base.path(), root.join("outside")).unwrap();
        for rel in [
            "root-alias",
            "outside",
            "outside/secret.txt",
            "git-alias/HEAD",
        ] {
            assert!(matches!(
                delete_entry(&root, rel),
                Err(DocsError::Outside(_))
            ));
        }
        delete_entry(&root, "alias").unwrap();
        delete_entry(&root, "alias.txt").unwrap();
        assert!(root.join("real/keep.txt").exists());
        assert!(!root.join("alias").exists());
        assert!(!root.join("alias.txt").exists());
        // A link inside a deleted directory must not take its external target with it.
        fs::create_dir(root.join("delete-me")).unwrap();
        std::os::unix::fs::symlink(base.path(), root.join("delete-me/outside")).unwrap();
        delete_entry(&root, "delete-me").unwrap();
        assert!(base.join("secret.txt").exists());
        assert!(root.join(".git/HEAD").exists());
    }

    #[test]
    fn reads_text_flags_binary_and_refuses_big_files() {
        let (_base, root) = layout();
        write(
            &root.join("src/main.rs"),
            "fn main() { println!(\"é\"); }".as_bytes(),
        );
        write(&root.join("bin.dat"), b"\x89PNG\r\n\x1a\n\0\0\0");
        write(
            &root.join("big.log"),
            &vec![b'a'; MAX_FILE_BYTES as usize + 1],
        );
        let f = read_file(&root, "src/main.rs").unwrap();
        assert_eq!(f.path, "src/main.rs");
        assert_eq!(f.text.as_deref(), Some("fn main() { println!(\"é\"); }"));
        let b = read_file(&root, "bin.dat").unwrap();
        assert_eq!(b.text, None);
        assert_eq!(b.size, 11);
        let err = read_file(&root, "big.log").unwrap_err();
        assert_eq!(err.code(), "too_large");
        assert!(err.to_string().contains("Files opens files up to 1 MB"));
    }
}
