// Folder listing and reads, confined to the docs root.
// Every path is resolved through realpath and must stay inside the folder's realpath, which also
// defeats symlinks that point elsewhere.
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

pub const MAX_DOCS: usize = 200;
pub const MAX_DOC_BYTES: u64 = 1024 * 1024;
pub const MAX_ASSET_BYTES: u64 = 10 * 1024 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_VISITED: usize = 20_000;
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "dist",
    "build",
    ".next",
    "target",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
    ".turbo",
    "coverage",
    ".idea",
    ".svn",
    "Pods",
    "DerivedData",
];
const MD_EXTS: &[&str] = &["md", "markdown", "mdown", "mkd"];
const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("avif", "image/avif"),
];

#[derive(Debug, Error)]
pub enum DocsError {
    #[error("{0}")]
    Outside(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    TooLarge(String),
    #[error("docs I/O: {0}")]
    Io(#[from] std::io::Error),
}

impl DocsError {
    /// The machine-readable code sent with each refusal.
    pub fn code(&self) -> &'static str {
        match self {
            DocsError::Outside(_) => "outside",
            DocsError::NotFound(_) => "not_found",
            DocsError::Unsupported(_) => "unsupported",
            DocsError::TooLarge(_) => "too_large",
            DocsError::Io(_) => "io",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocEntry {
    pub path: String,
    pub name: String,
    pub changed: bool,
    pub readme: bool,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocList {
    pub docs: Vec<DocEntry>,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocText {
    pub path: String,
    pub text: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub data: Vec<u8>,
    pub content_type: &'static str,
}

/// Resolve `rel` inside `folder`, refusing anything that escapes it (symlinks included).
pub fn resolve_inside(folder: &Path, rel: &str) -> Result<PathBuf, DocsError> {
    if rel.contains('\0') {
        return Err(DocsError::Outside("Invalid path"));
    }
    let root = fs::canonicalize(folder)?;
    // Lexical normalization first, as Node's path.resolve does, so `link/../x` resolves the same
    // way the docs API resolves it.
    let candidate = normalize_lexically(&root.join(rel));
    let real = fs::canonicalize(&candidate).map_err(|_| DocsError::NotFound("File not found"))?;
    if !real.starts_with(&root) {
        return Err(DocsError::Outside(
            "That file is outside this stream’s folder",
        ));
    }
    Ok(real)
}

/// Markdown files under `folder`, changed ones first, then READMEs, then the rest.
pub fn list_docs(folder: &Path, changed_paths: &HashSet<String>) -> Result<DocList, DocsError> {
    let root = fs::canonicalize(folder)?;
    let mut walk = Walk {
        root: &root,
        changed: changed_paths,
        found: Vec::new(),
        visited: 0,
    };
    walk.visit(&root, 0);
    let mut found = walk.found;
    found.sort_by(compare_entries);
    let total = found.len();
    found.truncate(MAX_DOCS);
    Ok(DocList {
        docs: found,
        total,
        truncated: total > MAX_DOCS,
    })
}

pub fn read_doc(folder: &Path, rel: &str) -> Result<DocText, DocsError> {
    if !is_markdown(rel) {
        return Err(DocsError::Unsupported("Only Markdown files open in Docs"));
    }
    let abs = resolve_inside(folder, rel)?;
    let meta = fs::metadata(&abs)?;
    if !meta.is_file() {
        return Err(DocsError::NotFound("Not a file"));
    }
    if meta.len() > MAX_DOC_BYTES {
        return Err(DocsError::TooLarge(format!(
            "This file is {:.1} MB; Docs opens files up to 1 MB",
            meta.len() as f64 / 1024.0 / 1024.0
        )));
    }
    let root = fs::canonicalize(folder)?;
    let text = String::from_utf8_lossy(&fs::read(&abs)?).into_owned();
    Ok(DocText {
        path: abs
            .strip_prefix(&root)
            .unwrap_or(&abs)
            .to_string_lossy()
            .into_owned(),
        text,
        size: meta.len(),
    })
}

pub fn read_asset(folder: &Path, rel: &str) -> Result<Asset, DocsError> {
    let content_type =
        image_type(rel).ok_or(DocsError::Unsupported("Only images can be embedded"))?;
    let abs = resolve_inside(folder, rel)?;
    let meta = fs::metadata(&abs)?;
    if !meta.is_file() {
        return Err(DocsError::NotFound("Not a file"));
    }
    if meta.len() > MAX_ASSET_BYTES {
        return Err(DocsError::TooLarge("Image too large".to_string()));
    }
    Ok(Asset {
        data: fs::read(&abs)?,
        content_type,
    })
}

struct Walk<'a> {
    root: &'a Path,
    changed: &'a HashSet<String>,
    found: Vec<DocEntry>,
    visited: usize,
}

impl Walk<'_> {
    fn visit(&mut self, dir: &Path, depth: usize) {
        if depth > MAX_DEPTH || self.visited > MAX_VISITED {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            self.visited += 1;
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let abs = entry.path();
            // file_type() does not follow symlinks, so linked folders and files are skipped.
            if file_type.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    self.visit(&abs, depth + 1);
                }
            } else if file_type.is_file() && is_markdown(&name) {
                let rel = abs
                    .strip_prefix(self.root)
                    .unwrap_or(&abs)
                    .to_string_lossy()
                    .into_owned();
                let size = fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
                self.found.push(DocEntry {
                    changed: self.changed.contains(&rel),
                    readme: name.to_ascii_lowercase().starts_with("readme."),
                    path: rel,
                    name,
                    size,
                });
            }
        }
    }
}

fn compare_entries(a: &DocEntry, b: &DocEntry) -> Ordering {
    b.changed
        .cmp(&a.changed)
        .then_with(|| b.readme.cmp(&a.readme))
        .then_with(|| {
            if a.readme {
                let da = Path::new(&a.path).components().count();
                let db = Path::new(&b.path).components().count();
                if da != db {
                    return da.cmp(&db);
                }
            }
            locale_compare(&a.path, &b.path)
        })
}

/// Approximates JS localeCompare for ASCII names: case-insensitive first, raw bytes to break ties.
/// Not identical to ICU collation for punctuation; the golden fixtures should confirm the order.
pub(crate) fn locale_compare(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Mirrors Node's path.resolve normalization: `.` is dropped and `..` pops a segment.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn extension_lower(rel: &str) -> Option<String> {
    Path::new(rel)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

fn is_markdown(rel: &str) -> bool {
    extension_lower(rel).is_some_and(|ext| MD_EXTS.contains(&ext.as_str()))
}

fn image_type(rel: &str) -> Option<&'static str> {
    let ext = extension_lower(rel)?;
    IMAGE_TYPES
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|(_, content_type)| *content_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    /// A scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("briardr-docs-{}-{}", std::process::id(), n));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A docs root with a sibling secret outside it: `base/secret.md` and `base/root/`.
    fn layout() -> (Scratch, PathBuf) {
        let base = Scratch::new();
        write(&base.path().join("secret.md"), b"secret");
        let root = base.path().join("root");
        fs::create_dir_all(&root).unwrap();
        (base, root)
    }

    fn names(list: &DocList) -> Vec<&str> {
        list.docs.iter().map(|d| d.path.as_str()).collect()
    }

    #[test]
    fn resolve_accepts_paths_inside_the_root() {
        let (_base, root) = layout();
        write(&root.join("a/b.md"), b"x");
        let real_root = fs::canonicalize(&root).unwrap();
        assert_eq!(
            resolve_inside(&root, "a/b.md").unwrap(),
            real_root.join("a/b.md")
        );
        assert_eq!(
            resolve_inside(&root, "a/../a/b.md").unwrap(),
            real_root.join("a/b.md")
        );
        assert_eq!(resolve_inside(&root, "").unwrap(), real_root);
    }

    #[test]
    fn resolve_refuses_dotdot_and_absolute_escapes() {
        let (base, root) = layout();
        let secret = base.path().join("secret.md");
        assert!(matches!(
            resolve_inside(&root, "../secret.md"),
            Err(DocsError::Outside(_))
        ));
        assert!(matches!(
            resolve_inside(&root, secret.to_str().unwrap()),
            Err(DocsError::Outside(_))
        ));
    }

    #[test]
    fn resolve_refuses_nul_and_missing_files() {
        let (_base, root) = layout();
        assert!(matches!(
            resolve_inside(&root, "a\0b.md"),
            Err(DocsError::Outside(_))
        ));
        assert!(matches!(
            resolve_inside(&root, "nope.md"),
            Err(DocsError::NotFound(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_refuses_symlinks_that_escape() {
        let (base, root) = layout();
        let secret = base.path().join("secret.md");
        std::os::unix::fs::symlink(&secret, root.join("link.md")).unwrap();
        std::os::unix::fs::symlink(base.path(), root.join("linkdir")).unwrap();
        assert!(matches!(
            resolve_inside(&root, "link.md"),
            Err(DocsError::Outside(_))
        ));
        assert!(matches!(
            resolve_inside(&root, "linkdir/secret.md"),
            Err(DocsError::Outside(_))
        ));
        assert!(matches!(
            read_doc(&root, "link.md"),
            Err(DocsError::Outside(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_matches_node_lexical_dotdot_through_a_symlink() {
        // Node's path.resolve turns `linkdir/../a.md` into `root/a.md` before realpath, so the
        // docs API accepts it. The kernel would resolve `linkdir/..` to the link's parent instead.
        let (base, root) = layout();
        write(&root.join("a.md"), b"x");
        std::os::unix::fs::symlink(base.path(), root.join("linkdir")).unwrap();
        assert_eq!(
            resolve_inside(&root, "linkdir/../a.md").unwrap(),
            fs::canonicalize(&root).unwrap().join("a.md")
        );
    }

    #[test]
    fn list_skips_skip_dirs_hidden_dirs_and_non_markdown() {
        let (_base, root) = layout();
        write(&root.join("README.md"), b"r");
        write(&root.join("b.md"), b"b");
        write(&root.join("a/c.md"), b"c");
        write(&root.join("z/readme.md"), b"z");
        write(&root.join("node_modules/x.md"), b"x");
        write(&root.join(".hidden/y.md"), b"y");
        write(&root.join("notes.txt"), b"t");
        let changed = HashSet::from(["b.md".to_string()]);
        let list = list_docs(&root, &changed).unwrap();
        // Changed first, then READMEs (shallower first), then the rest.
        assert_eq!(names(&list), ["b.md", "README.md", "z/readme.md", "a/c.md"]);
        assert!(list.docs[0].changed);
        assert!(list.docs[1].readme);
        assert_eq!(list.total, 4);
        assert!(!list.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn list_does_not_follow_symlinked_markdown_or_folders() {
        let (base, root) = layout();
        write(&base.path().join("outside/o.md"), b"o");
        std::os::unix::fs::symlink(base.path().join("secret.md"), root.join("link.md")).unwrap();
        std::os::unix::fs::symlink(base.path().join("outside"), root.join("linkdir")).unwrap();
        let list = list_docs(&root, &HashSet::new()).unwrap();
        assert!(list.docs.is_empty());
    }

    #[test]
    fn list_caps_entries_and_reports_the_total() {
        let (_base, root) = layout();
        for i in 0..MAX_DOCS + 1 {
            write(&root.join(format!("f{i:03}.md")), b"x");
        }
        let list = list_docs(&root, &HashSet::new()).unwrap();
        assert_eq!(list.docs.len(), MAX_DOCS);
        assert_eq!(list.total, MAX_DOCS + 1);
        assert!(list.truncated);
    }

    #[test]
    fn list_stops_at_the_depth_limit() {
        let (_base, root) = layout();
        let mut deep = root.clone();
        for i in 1..=9 {
            deep = deep.join(format!("n{i}"));
            if i == 8 {
                write(&deep.join("in.md"), b"in");
            }
        }
        write(&deep.join("out.md"), b"out");
        let list = list_docs(&root, &HashSet::new()).unwrap();
        let paths = names(&list);
        assert!(paths.contains(&"n1/n2/n3/n4/n5/n6/n7/n8/in.md"));
        assert!(!paths.iter().any(|p| p.ends_with("out.md")));
    }

    #[test]
    fn read_doc_returns_text_and_relative_path() {
        let (_base, root) = layout();
        write(&root.join("sub/doc.MD"), "héllo".as_bytes());
        let doc = read_doc(&root, "sub/doc.MD").unwrap();
        assert_eq!(doc.path, "sub/doc.MD");
        assert_eq!(doc.text, "héllo");
        assert_eq!(doc.size, "héllo".len() as u64);
    }

    #[test]
    fn read_doc_refuses_non_markdown_and_folders_and_big_files() {
        let (_base, root) = layout();
        write(&root.join("x.txt"), b"t");
        fs::create_dir_all(root.join("dir.md")).unwrap();
        write(
            &root.join("big.md"),
            &vec![b'a'; MAX_DOC_BYTES as usize + 1],
        );
        assert!(matches!(
            read_doc(&root, "x.txt"),
            Err(DocsError::Unsupported(_))
        ));
        assert!(matches!(
            read_doc(&root, "dir.md"),
            Err(DocsError::NotFound("Not a file"))
        ));
        let err = read_doc(&root, "big.md").unwrap_err();
        assert_eq!(err.code(), "too_large");
        assert!(err.to_string().contains("Docs opens files up to 1 MB"));
    }

    #[test]
    fn read_asset_maps_image_types_and_refuses_others() {
        let (_base, root) = layout();
        write(&root.join("pic.PNG"), b"png");
        write(&root.join("doc.txt"), b"t");
        let asset = read_asset(&root, "pic.PNG").unwrap();
        assert_eq!(asset.content_type, "image/png");
        assert_eq!(asset.data, b"png");
        assert!(matches!(
            read_asset(&root, "doc.txt"),
            Err(DocsError::Unsupported(_))
        ));
    }

    #[test]
    fn read_asset_refuses_oversized_images() {
        let (_base, root) = layout();
        write(
            &root.join("big.png"),
            &vec![0u8; MAX_ASSET_BYTES as usize + 1],
        );
        let err = read_asset(&root, "big.png").unwrap_err();
        assert_eq!(err.code(), "too_large");
    }
}
