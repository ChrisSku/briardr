// The Files surface: browsing, searching, reading and deleting entries, confined to the stream's
// folder exactly like Docs (every path goes through `resolve_inside`).
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::ops::ControlFlow;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::docs::{locale_compare, resolve_inside, DocsError};
use crate::git::{run_git, GIT_TIMEOUT};

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
            let abs = if link {
                match fs::canonicalize(entry.path()) {
                    Ok(real) if real.starts_with(root) => real,
                    _ => continue,
                }
            } else {
                entry.path()
            };
            let Ok(meta) = fs::metadata(&abs) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
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
                size: meta.len(),
                link,
            };
            if visit(found).is_break() {
                return Ok(true);
            }
        }
    }
    Ok(cut)
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMatch {
    pub path: String,
    /// 1-based.
    pub line: u64,
    /// The line, or the part of a long one around the match.
    pub text: String,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSearch {
    pub matches: Vec<ContentMatch>,
    /// Results or the search were capped; more matches may exist.
    pub truncated: bool,
}

/// Case-insensitive search for `query` in the folder's text files. In a Git repository `git grep` does
/// it, over tracked and untracked files: what `.gitignore` names (dependencies, build output) is left
/// out, which is what makes it quick and its results the project's own. Elsewhere, every text file of
/// at most MAX_FILE_BYTES is read (see `walk_files` for what it visits).
pub fn search_contents(
    folder: &Path,
    query: &str,
    changed: &HashSet<String>,
    git_path: &str,
) -> Result<ContentSearch, DocsError> {
    let root = fs::canonicalize(folder)?;
    let query = query.trim();
    let mut result = ContentSearch {
        matches: Vec::new(),
        truncated: false,
    };
    if query.is_empty() {
        return Ok(result);
    }
    let per_file = MAX_MATCHES_PER_FILE.to_string();
    let grep = run_git(
        &root,
        &[
            "grep",
            "-n",
            "--column",
            "-I",
            "-i",
            "-F",
            "--untracked",
            "--no-color",
            "-z",
            "-m",
            &per_file,
            "-e",
            query,
            "--",
            ".",
        ],
        &[1],
        GIT_TIMEOUT,
        git_path,
    );
    match grep {
        Ok(res) => {
            // `path\0line\0column\0text` per line.
            for record in String::from_utf8_lossy(&res.stdout).split('\n') {
                let mut parts = record.splitn(4, '\0');
                let (Some(path), Some(line), Some(column), Some(text)) =
                    (parts.next(), parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let (Ok(line), Ok(column)) = (line.parse(), column.parse::<usize>()) else {
                    continue;
                };
                if result.matches.len() == MAX_CONTENT_MATCHES {
                    result.truncated = true;
                    break;
                }
                result.matches.push(ContentMatch {
                    changed: changed.contains(path),
                    path: path.to_string(),
                    line,
                    text: excerpt(text, column.saturating_sub(1)),
                });
            }
        }
        // Not a repository, no Git, or a Git too old for these options: read the files here.
        Err(_) => {
            let needle = query.to_lowercase();
            let matches = &mut result.matches;
            result.truncated = walk_files(
                &root,
                Duration::from_millis(1500),
                |_| true,
                |f| {
                    if f.size > MAX_FILE_BYTES {
                        return ControlFlow::Continue(());
                    }
                    let Ok(bytes) = fs::read(&f.abs) else {
                        return ControlFlow::Continue(());
                    };
                    if is_binary(&bytes) {
                        return ControlFlow::Continue(());
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    let mut in_file = 0;
                    for (i, line) in text.lines().enumerate() {
                        let Some(at) = line.to_lowercase().find(&needle) else {
                            continue;
                        };
                        if matches.len() == MAX_CONTENT_MATCHES {
                            return ControlFlow::Break(());
                        }
                        matches.push(ContentMatch {
                            changed: changed.contains(&f.path),
                            path: f.path.clone(),
                            line: i as u64 + 1,
                            // Lowercasing can move byte offsets; the excerpt only needs to be near.
                            text: excerpt(line, at.min(line.len())),
                        });
                        in_file += 1;
                        if in_file == MAX_MATCHES_PER_FILE {
                            break;
                        }
                    }
                    ControlFlow::Continue(())
                },
            )?;
            result
                .matches
                .sort_by(|a, b| locale_compare(&a.path, &b.path).then(a.line.cmp(&b.line)));
        }
    }
    Ok(result)
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
    use crate::scratch::Scratch;
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

    #[cfg(unix)]
    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
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

    /// (path, line, text) of each match.
    fn found(result: &ContentSearch) -> Vec<(&str, u64, &str)> {
        result
            .matches
            .iter()
            .map(|m| (m.path.as_str(), m.line, m.text.as_str()))
            .collect()
    }

    #[test]
    fn content_search_outside_a_repository_reads_text_files_and_skips_binary_ones() {
        let (_scratch, root) = layout();
        write(
            &root.join("src/a.rs"),
            b"fn main() {\n    let Needle = 1;\n}\n",
        );
        write(&root.join("notes.md"), b"first\r\nthe NEEDLE again\r\n");
        write(&root.join("bin.dat"), b"needle\0binary");
        write(&root.join(".git/config"), b"needle in git internals");
        let changed = HashSet::from(["notes.md".to_string()]);
        let result = search_contents(&root, " needle ", &changed, "git").unwrap();
        assert_eq!(
            found(&result),
            [
                ("notes.md", 2, "the NEEDLE again"),
                ("src/a.rs", 2, "let Needle = 1;")
            ]
        );
        assert!(result.matches[0].changed && !result.matches[1].changed);
        assert!(!result.truncated);
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
            b"needle in a dependency\n-O sh\n",
        );
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        write(&root.join("untracked.txt"), b"one\nan untracked needle\n");
        // A pattern that looks like an option is still only a pattern (and Git still does the search:
        // the dependency's line isn't found).
        write(&root.join("opt.txt"), b"-O sh\n");

        let result = search_contents(&root, "needle", &HashSet::new(), "git").unwrap();
        assert_eq!(
            found(&result),
            [
                ("src/tracked.rs", 1, "// needle, tracked"),
                ("untracked.txt", 2, "an untracked needle")
            ]
        );
        let option = search_contents(&root, "-O sh", &HashSet::new(), "git").unwrap();
        assert_eq!(found(&option), [("opt.txt", 1, "-O sh")]);
        // From a folder inside the repository, paths are that folder's.
        let inside = search_contents(&root.join("src"), "needle", &HashSet::new(), "git").unwrap();
        assert_eq!(found(&inside), [("tracked.rs", 1, "// needle, tracked")]);
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
        assert_eq!(
            result
                .matches
                .iter()
                .filter(|m| m.path == "f00.txt")
                .count(),
            MAX_MATCHES_PER_FILE
        );

        let long = format!("{}needle{}", "a".repeat(1000), "b".repeat(1000));
        let cut = excerpt(&long, 1000);
        assert_eq!(cut.chars().count(), MAX_MATCH_TEXT + 2);
        assert!(cut.starts_with('…') && cut.ends_with('…') && cut.contains("needle"));
        assert_eq!(excerpt("\t  indented é line\r", 5), "indented é line");
    }

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
