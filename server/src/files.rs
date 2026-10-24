// The Files surface: browsing, searching, reading and deleting entries, confined to the stream's
// folder exactly like Docs (every path goes through `resolve_inside`).
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

use crate::docs::{locale_compare, resolve_inside, DocsError};

pub const MAX_ENTRIES: usize = 2000;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Bytes checked for a NUL when deciding whether a file is binary (git's heuristic).
const SNIFF_BYTES: usize = 8000;
/// Never listed: git internals and Finder droppings.
const HIDDEN: &[&str] = &[".git", ".DS_Store"];
pub const MAX_SEARCH_RESULTS: usize = 200;
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

/// Case-insensitive filename search, including unopened folders. Never follows directory symlinks
/// (which can cycle), and only includes file symlinks whose targets remain inside the stream root.
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
    let start = Instant::now();
    let mut pending = VecDeque::from([(root.clone(), 0)]);
    let mut visited = 0;
    'walk: while let Some((dir, depth)) = pending.pop_front() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if dir == root => return Err(e.into()),
            Err(_) => {
                result.truncated = true;
                continue;
            }
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_SEARCH_VISITED || start.elapsed() > Duration::from_secs(1) {
                result.truncated = true;
                break 'walk;
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
                    result.truncated = true;
                }
                continue;
            }
            if !name.to_lowercase().contains(&query) {
                continue;
            }
            let link = kind.is_symlink();
            let meta = if link {
                match fs::canonicalize(entry.path()) {
                    Ok(real) if real.starts_with(&root) => fs::metadata(real),
                    _ => continue,
                }
            } else {
                entry.metadata()
            };
            let Ok(meta) = meta else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if result.entries.len() == MAX_SEARCH_RESULTS {
                result.truncated = true;
                break 'walk;
            }
            let path = entry
                .path()
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            result.entries.push(FileEntry {
                changed: changed.contains(&path),
                name,
                path,
                dir: false,
                size: meta.len(),
                link,
            });
        }
    }
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
    let binary = bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0);
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
