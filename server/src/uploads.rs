// Private per-upload folders, PNG header check, pruning.
// Each upload gets its own random 0700 folder under the uploads root. The file is written as a
// hidden partial and renamed only when complete; a failed upload removes only its own partial
// file and folder.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_IMAGE_SIDE: u32 = 4096;
/// Client names are cut to this many UTF-16 units, as JS `slice(0, 120)` counts them.
const MAX_NAME_UNITS: usize = 120;
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadKind {
    Image,
    File,
}

/// The HTTP layer maps these to statuses: TooLarge and TooManyPixels 413, NotPng 415, Io 500.
#[derive(Debug, Error)]
pub enum UploadError {
    #[error("too large")]
    TooLarge,
    #[error("not a PNG")]
    NotPng,
    #[error("image too large in pixels")]
    TooManyPixels,
    #[error("upload I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Content-Length over the kind's limit, refused before reading.
    #[error("declared too large")]
    DeclaredTooLarge(UploadKind),
    /// The client went away mid-upload, or sent less than it declared.
    #[error("cancelled")]
    Cancelled,
    /// Reading the request body failed.
    #[error("read failed")]
    ReadFailed,
}

impl UploadError {
    /// The HTTP status for this error.
    pub fn status(&self) -> u16 {
        match self {
            UploadError::TooLarge
            | UploadError::TooManyPixels
            | UploadError::DeclaredTooLarge(_) => 413,
            UploadError::NotPng => 415,
            UploadError::Io(_) => 500,
            UploadError::Cancelled => 499,
            UploadError::ReadFailed => 400,
        }
    }

    /// The message returned with this error.
    pub fn message(&self) -> &'static str {
        match self {
            UploadError::DeclaredTooLarge(UploadKind::Image) => "Images are limited to 20 MB",
            UploadError::DeclaredTooLarge(UploadKind::File) => "Files are limited to 100 MB",
            UploadError::TooLarge => "Upload is too large",
            UploadError::NotPng => "Images must be uploaded as PNG",
            UploadError::TooManyPixels => "Images are limited to 4096 px on the longest side",
            UploadError::Io(_) => "Could not save the upload",
            UploadError::Cancelled => "Upload cancelled",
            UploadError::ReadFailed => "Upload failed",
        }
    }

    pub fn max_bytes(kind: UploadKind) -> u64 {
        match kind {
            UploadKind::Image => MAX_IMAGE_BYTES,
            UploadKind::File => MAX_FILE_BYTES,
        }
    }
}

/// `BRIARDR_UPLOADS_DIR` when set and non-empty, else `briardr-uploads` under the temp directory.
pub fn uploads_root() -> PathBuf {
    match std::env::var_os("BRIARDR_UPLOADS_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::temp_dir().join("briardr-uploads"),
    }
}

/// Strip path parts and unsafe characters from a client-supplied name.
pub fn sanitize_name(name: &str, fallback: &str) -> String {
    let base: String = posix_basename(name).nfc().collect();
    let kept: String = base
        .chars()
        .filter(|&c| !matches!(c as u32, 0..=0x1f | 0x7f) && !matches!(c, '/' | '\\' | ':'))
        .collect();
    let clean = take_utf16_units(kept.trim_start_matches('.').trim(), MAX_NAME_UNITS);
    if clean.is_empty() {
        fallback.to_string()
    } else {
        clean
    }
}

/// Node `path.basename` on POSIX: trailing slashes are ignored, backslashes are not separators.
fn posix_basename(s: &str) -> &str {
    let trimmed = s.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

fn take_utf16_units(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        let n = c.len_utf16();
        if units + n > max {
            break;
        }
        units += n;
        out.push(c);
    }
    out
}

/// Width and height from a PNG header.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn private_dir_builder() -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
}

/// A new, randomly named private folder (0700) for one upload.
pub fn new_upload_dir(root: &Path) -> Result<PathBuf, UploadError> {
    private_dir_builder().recursive(true).create(root)?;
    let mut id = [0u8; 12];
    rand::fill(&mut id);
    let dir = root.join(hex::encode(id));
    private_dir_builder().create(&dir)?;
    Ok(dir)
}

/// Store one upload and return its final path. The caller has already buffered the body; the
/// size limit is applied to the bytes themselves.
pub fn receive_upload(
    bytes: &[u8],
    kind: UploadKind,
    name: &str,
    root: &Path,
) -> Result<PathBuf, UploadError> {
    let max = match kind {
        UploadKind::Image => MAX_IMAGE_BYTES,
        UploadKind::File => MAX_FILE_BYTES,
    };
    if bytes.len() as u64 > max {
        return Err(UploadError::TooLarge);
    }
    // Validate before anything is written: a rejected image leaves no folder behind.
    if kind == UploadKind::Image {
        let (width, height) = png_size(bytes).ok_or(UploadError::NotPng)?;
        if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
            return Err(UploadError::TooManyPixels);
        }
    }
    let final_name = match kind {
        UploadKind::Image => {
            let stem = sanitize_name(name, "image");
            let stem = match stem.rfind('.') {
                Some(i) => &stem[..i],
                None => stem.as_str(),
            };
            format!("{stem}.png")
        }
        UploadKind::File => sanitize_name(name, "file"),
    };
    let dir = new_upload_dir(root)?;
    let partial = dir.join(format!(".{final_name}.partial"));
    let final_path = dir.join(&final_name);
    if let Err(e) = write_then_rename(&partial, &final_path, bytes) {
        // Remove only our own partial file and (now empty) folder.
        let _ = fs::remove_file(&partial);
        let _ = fs::remove_dir(&dir);
        return Err(e.into());
    }
    Ok(final_path)
}

fn write_then_rename(partial: &Path, final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(partial)?;
    out.write_all(bytes)?;
    drop(out);
    fs::rename(partial, final_path)
}

/// Remove upload entries older than `max_age` (the default is 7 days). Returns how many were
/// removed. A symlink is removed as a link, never followed.
pub fn prune_uploads(root: &Path, max_age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(modified) = fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        let age = now.duration_since(modified).unwrap_or_default();
        if age <= max_age {
            continue;
        }
        let is_real_dir = fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        let ok = if is_real_dir {
            fs::remove_dir_all(&path).is_ok()
        } else {
            fs::remove_file(&path).is_ok()
        };
        if ok {
            removed += 1;
        }
    }
    removed
}

/// Quote a path for pasting into a terminal as a single shell word.
pub fn quote_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '+' | '-'));
    if safe {
        return s.into_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("briardr-uploads-{}-{}", std::process::id(), n));
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

    /// A PNG header (signature, IHDR length and type, dimensions) with no image data.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v
    }

    /// Golden fixtures (recorded outputs), read straight from disk.
    fn fixtures(name: &str) -> Vec<(Vec<Value>, Value)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/uploads");
        let prefix = format!("{name}.");
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .and_then(|f| f.to_str())
                    .is_some_and(|f| f.starts_with(&prefix) && f.ends_with(".json"))
            })
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "no fixtures for {name}");
        paths
            .iter()
            .map(|p| {
                let v: Value = serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap();
                (v["input"].as_array().unwrap().clone(), v["output"].clone())
            })
            .collect()
    }

    #[test]
    fn sanitize_name_matches_ts_fixtures() {
        for (input, output) in fixtures("sanitizeName") {
            let got = sanitize_name(input[0].as_str().unwrap(), input[1].as_str().unwrap());
            assert_eq!(Value::String(got), output, "input {input:?}");
        }
    }

    #[test]
    fn quote_path_matches_ts_fixtures() {
        for (input, output) in fixtures("quotePath") {
            let got = quote_path(Path::new(input[0].as_str().unwrap()));
            assert_eq!(Value::String(got), output, "input {input:?}");
        }
    }

    #[test]
    fn sanitize_name_caps_length_in_utf16_units() {
        let long = "é".repeat(200);
        assert_eq!(sanitize_name(&long, "f").encode_utf16().count(), 120);
        let astral = "😀".repeat(100); // 2 UTF-16 units each
        assert_eq!(sanitize_name(&astral, "f").encode_utf16().count(), 120);
    }

    #[test]
    fn sanitize_name_normalizes_to_nfc() {
        assert_eq!(sanitize_name("e\u{301}.txt", "f"), "\u{e9}.txt");
    }

    #[test]
    fn png_size_reads_header_and_rejects_others() {
        assert_eq!(png_size(&png_header(640, 480)), Some((640, 480)));
        assert_eq!(png_size(&png_header(640, 480)[..23]), None);
        let mut bad = png_header(1, 1);
        bad[12] = b'X';
        assert_eq!(png_size(&bad), None);
        assert_eq!(png_size(b"GIF89a................"), None);
    }

    #[test]
    fn new_upload_dir_is_private_and_unique() {
        let scratch = Scratch::new();
        let root = scratch.path().join("root");
        let a = new_upload_dir(&root).unwrap();
        let b = new_upload_dir(&root).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.parent().unwrap(), root);
        let mode = fs::metadata(&a).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn receive_image_renames_to_png_and_leaves_no_partial() {
        let scratch = Scratch::new();
        let mut bytes = png_header(10, 10);
        bytes.extend_from_slice(&[0; 16]);
        let path = receive_upload(&bytes, UploadKind::Image, "shot.jpg", scratch.path()).unwrap();
        assert_eq!(path.file_name().unwrap(), "shot.png");
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let dir = path.parent().unwrap();
        let names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "partial file was not renamed");
    }

    #[test]
    fn receive_rejects_bad_images_without_leaving_folders() {
        let scratch = Scratch::new();
        let err = receive_upload(
            b"not a png at all, padded out",
            UploadKind::Image,
            "a.png",
            scratch.path(),
        );
        assert!(matches!(err, Err(UploadError::NotPng)));
        let big = png_header(MAX_IMAGE_SIDE + 1, 1);
        let err = receive_upload(&big, UploadKind::Image, "a.png", scratch.path());
        assert!(matches!(err, Err(UploadError::TooManyPixels)));
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
    }

    #[test]
    fn receive_file_keeps_sanitized_name() {
        let scratch = Scratch::new();
        let path = receive_upload(
            b"data",
            UploadKind::File,
            "../../report.pdf",
            scratch.path(),
        )
        .unwrap();
        assert_eq!(path.file_name().unwrap(), "report.pdf");
        assert_eq!(fs::read(&path).unwrap(), b"data");
    }

    #[test]
    fn prune_removes_only_entries_older_than_max_age() {
        let scratch = Scratch::new();
        let old = scratch.path().join("old");
        let fresh = scratch.path().join("fresh");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&fresh).unwrap();
        let past = SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
        fs::File::open(&old).unwrap().set_modified(past).unwrap();
        assert_eq!(
            prune_uploads(scratch.path(), Duration::from_secs(7 * 24 * 3600)),
            1
        );
        assert!(!old.exists());
        assert!(fresh.exists());
    }
}
