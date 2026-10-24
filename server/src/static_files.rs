// Content types, cache headers, .br/.gz negotiation.
use std::fs::Metadata;
use std::path::{Path, PathBuf};

const DEFAULT_MIME: &str = "application/octet-stream";
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";

/// Extension (with its dot) to MIME type.
const MIME: &[(&str, &str)] = &[
    (".html", "text/html; charset=utf-8"),
    (".js", "text/javascript; charset=utf-8"),
    (".css", "text/css; charset=utf-8"),
    (".json", "application/json"),
    (".webmanifest", "application/manifest+json"),
    (".png", "image/png"),
    (".svg", "image/svg+xml"),
    (".ico", "image/x-icon"),
    (".woff2", "font/woff2"),
    (".map", "application/json"),
];

/// File types the build writes compressed copies of. Images and fonts are compressed already.
pub const COMPRESSED_TYPES: &[&str] = &[".html", ".js", ".css", ".json", ".webmanifest", ".svg"];

/// Content encodings, best first, with the suffix of the copy the build writes for each.
pub const ENCODINGS: &[(&str, &str)] = &[("br", ".br"), ("gzip", ".gz")];

/// A static file to serve: the path to read (the file or a compressed copy) and its response headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFile {
    pub body: PathBuf,
    pub headers: Vec<(String, String)>,
}

/// `path.extname`: the last dot and what follows it, or "" when there is no dot or only leading dots.
fn extname(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    let Some(dot) = base.rfind('.') else {
        return "";
    };
    if base[..dot].bytes().all(|b| b == b'.') {
        return "";
    }
    &base[dot..]
}

/// Extension to MIME type map. Case-sensitive.
pub fn mime_for(name: &str) -> &'static str {
    let ext = extname(name);
    MIME.iter()
        .find(|(e, _)| *e == ext)
        .map_or(DEFAULT_MIME, |(_, mime)| *mime)
}

/// JS `Number(value) > 0` for a q value. Rust `parse` also takes "inf" and "infinity", which JS reads
/// as NaN (the header is already lowercased, so JS never sees "Infinity"), so any letter other than
/// an exponent marker counts as 0.
fn q_is_positive(value: &str) -> bool {
    let v = value.trim();
    if v.bytes()
        .any(|b| b.is_ascii_alphabetic() && !matches!(b, b'e' | b'E'))
    {
        return false;
    }
    v.parse::<f64>().is_ok_and(|q| q > 0.0)
}

/// Whether `accept_encoding` allows the named encoding.
pub fn accepts(accept_encoding: Option<&str>, name: &str) -> bool {
    let mut any = false;
    for part in accept_encoding.unwrap_or("").to_lowercase().split(',') {
        let mut fields = part.split(';').map(str::trim);
        let token = fields.next().unwrap_or("");
        let q = fields.find(|p| p.starts_with("q="));
        let ok = q.is_none_or(|q| q_is_positive(&q[2..]));
        if token == name {
            return ok;
        }
        if token == "*" {
            any = ok;
        }
    }
    any
}

/// `/\.[0-9a-f]{8,}\.(js|css|png)$/` checked by hand: a lowercase hex run of 8 or more characters
/// between the last two dots, before a `.js`, `.css` or `.png` ending.
fn is_fingerprinted(path: &str) -> bool {
    let Some(stem) = [".js", ".css", ".png"]
        .iter()
        .find_map(|ext| path.strip_suffix(*ext))
    else {
        return false;
    };
    let Some(dot) = stem.rfind('.') else {
        return false;
    };
    let hash = &stem[dot + 1..];
    hash.len() >= 8
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The first accepted encoding whose copy exists next to `file`, is a regular file, and is not older
/// than the file. A copy older than the file is ignored, so a file changed after the build is never
/// answered with its stale copy. Any error reading either file counts as absent.
fn pick_copy(
    file: &Path,
    meta: &Metadata,
    accept_encoding: Option<&str>,
) -> Option<(&'static str, PathBuf)> {
    let file_mtime = meta.modified().ok()?;
    for &(encoding, suffix) in ENCODINGS {
        if !accepts(accept_encoding, encoding) {
            continue;
        }
        let mut copy_name = file.as_os_str().to_owned();
        copy_name.push(suffix);
        let copy = PathBuf::from(copy_name);
        let Ok(copy_meta) = std::fs::metadata(&copy) else {
            continue;
        };
        if !copy_meta.is_file() {
            continue;
        }
        let Ok(copy_mtime) = copy_meta.modified() else {
            continue;
        };
        if copy_mtime < file_mtime {
            continue;
        }
        return Some((encoding, copy));
    }
    None
}

/// The response for a static file that exists (`meta` is its metadata): its type and cache headers, and
/// the compressed copy the request accepts, if the build wrote one. `extra` headers override the rest.
///
/// Uses blocking std::fs calls (metadata of the file and up to two copies). From async code, call it
/// through `spawn_blocking`.
pub fn static_file(
    file: &Path,
    meta: &Metadata,
    rel: &str,
    accept_encoding: Option<&str>,
    extra: &[(String, String)],
) -> StaticFile {
    let name = file.to_string_lossy();
    let immutable = is_fingerprinted(&name) || rel.starts_with("/assets/");
    let cache = if immutable { IMMUTABLE_CACHE } else { NO_CACHE };
    let mut headers = vec![
        ("content-type".to_string(), mime_for(&name).to_string()),
        ("cache-control".to_string(), cache.to_string()),
    ];
    let mut body = file.to_path_buf();
    if COMPRESSED_TYPES.contains(&extname(&name)) {
        headers.push(("vary".to_string(), "accept-encoding".to_string()));
        if let Some((encoding, copy)) = pick_copy(file, meta, accept_encoding) {
            body = copy;
            headers.push(("content-encoding".to_string(), encoding.to_string()));
        }
    }
    for (key, value) in extra {
        headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
        headers.push((key.clone(), value.clone()));
    }
    StaticFile { body, headers }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use serde_json::Value;
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    /// Every `accepts.<case>.json` golden file, sorted by name.
    fn accepts_cases() -> Vec<(String, Value)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/static");
        let entries =
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read fixtures/static: {e}"));
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("accepts.") || !name.ends_with(".json") {
                continue;
            }
            let text = fs::read_to_string(entry.path())
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));
            let value: Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad fixture {name}: {e}"));
            found.push((name, value));
        }
        assert!(!found.is_empty(), "no fixtures for accepts");
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// Writes `name` in `dir` with the given modified time.
    fn write(dir: &Path, name: &str, mtime: SystemTime) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"x").expect("write fixture file");
        File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(mtime))
            .expect("set mtime");
        path
    }

    fn meta(path: &Path) -> Metadata {
        fs::metadata(path).expect("stat fixture file")
    }

    fn header<'a>(sf: &'a StaticFile, name: &str) -> Option<&'a str> {
        sf.headers
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, v)| v.as_str())
    }

    fn count_header(sf: &StaticFile, name: &str) -> usize {
        sf.headers
            .iter()
            .filter(|(k, _)| k.as_str() == name)
            .count()
    }

    #[test]
    fn golden_accepts() {
        for (name, case) in accepts_cases() {
            let input = case
                .get("input")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("{name}: no input array"));
            let header = input.first().and_then(Value::as_str);
            let encoding = input.get(1).and_then(Value::as_str).unwrap_or("");
            let expected = case.get("output").and_then(Value::as_bool);
            assert_eq!(Some(accepts(header, encoding)), expected, "{name}");
        }
    }

    #[test]
    fn accepts_edge_cases() {
        assert!(accepts(Some("*"), "gzip"));
        assert!(!accepts(Some("*;q=0"), "gzip"));
        assert!(accepts(Some("br;q=0.5, gzip"), "gzip"));
        assert!(accepts(Some("GZIP, BR"), "br"));
        assert!(!accepts(Some("identity"), "gzip"));
        assert!(!accepts(None, "gzip"));
        assert!(!accepts(Some(""), "br"));
        assert!(accepts(Some("gzip; q= 0.5"), "gzip"));
        assert!(!accepts(Some("gzip;q=abc"), "gzip"));
        assert!(!accepts(Some("gzip;q=inf"), "gzip"));
        assert!(!accepts(Some("gzip;q=Infinity"), "gzip"));
        assert!(!accepts(Some("gzip;q=0, *"), "gzip"));
    }

    #[test]
    fn mime_for_known_extensions() {
        let cases = [
            ("index.html", "text/html; charset=utf-8"),
            ("app.js", "text/javascript; charset=utf-8"),
            ("site.css", "text/css; charset=utf-8"),
            ("data.json", "application/json"),
            ("app.webmanifest", "application/manifest+json"),
            ("logo.png", "image/png"),
            ("icon.svg", "image/svg+xml"),
            ("favicon.ico", "image/x-icon"),
            ("font.woff2", "font/woff2"),
            ("app.js.map", "application/json"),
            ("assets/app.js", "text/javascript; charset=utf-8"),
        ];
        for (name, mime) in cases {
            assert_eq!(mime_for(name), mime, "{name}");
        }
    }

    #[test]
    fn mime_for_is_case_sensitive_and_defaults() {
        assert_eq!(mime_for("INDEX.HTML"), DEFAULT_MIME);
        assert_eq!(mime_for("data.bin"), DEFAULT_MIME);
        assert_eq!(mime_for(".html"), DEFAULT_MIME);
        assert_eq!(mime_for("README"), DEFAULT_MIME);
    }

    #[test]
    fn extname_follows_path_extname() {
        assert_eq!(extname("x.tar.gz"), ".gz");
        assert_eq!(extname("dir.v2/file"), "");
        assert_eq!(extname("a.b/.html"), "");
        assert_eq!(extname(".."), "");
        assert_eq!(extname("a."), ".");
    }

    #[test]
    fn fingerprint_check_is_lowercase_hex_of_eight_or_more() {
        assert!(is_fingerprinted("main.abcdef12.js"));
        assert!(is_fingerprinted("/a/main.0123abcd.png"));
        assert!(is_fingerprinted("site.0123456789ab.css"));
        assert!(!is_fingerprinted("main.ABCDEF12.js"));
        assert!(!is_fingerprinted("main.abcdef1.js"));
        assert!(!is_fingerprinted("main.abcdef12.map"));
        assert!(!is_fingerprinted("a.b/12345678.js"));
        assert!(!is_fingerprinted("main.abcdefg1.js"));
    }

    #[test]
    fn br_copy_newer_than_file_is_served() {
        let base = SystemTime::now();
        let dir = Scratch::new("static-br");
        let src = write(&dir, "main.js", base - Duration::from_secs(100));
        write(&dir, "main.js.br", base - Duration::from_secs(50));
        let sf = static_file(&src, &meta(&src), "/main.js", Some("gzip, br"), &[]);
        assert_eq!(sf.body, dir.join("main.js.br"));
        assert_eq!(header(&sf, "content-encoding"), Some("br"));
        assert_eq!(header(&sf, "vary"), Some("accept-encoding"));
        assert_eq!(
            header(&sf, "content-type"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(header(&sf, "cache-control"), Some(NO_CACHE));
    }

    #[test]
    fn gzip_copy_used_when_br_not_accepted() {
        let base = SystemTime::now();
        let dir = Scratch::new("static-gz");
        let src = write(&dir, "main.js", base - Duration::from_secs(100));
        write(&dir, "main.js.br", base - Duration::from_secs(50));
        write(&dir, "main.js.gz", base - Duration::from_secs(50));
        let sf = static_file(&src, &meta(&src), "/main.js", Some("gzip"), &[]);
        assert_eq!(sf.body, dir.join("main.js.gz"));
        assert_eq!(header(&sf, "content-encoding"), Some("gzip"));
    }

    #[test]
    fn older_copy_is_ignored() {
        let base = SystemTime::now();
        let dir = Scratch::new("static-old");
        let src = write(&dir, "main.js", base - Duration::from_secs(100));
        write(&dir, "main.js.br", base - Duration::from_secs(200));
        let sf = static_file(&src, &meta(&src), "/main.js", Some("gzip, br"), &[]);
        assert_eq!(sf.body, src);
        assert_eq!(header(&sf, "content-encoding"), None);
        assert_eq!(header(&sf, "vary"), Some("accept-encoding"));
    }

    #[test]
    fn uncompressed_type_has_no_vary_and_no_copy_lookup() {
        let base = SystemTime::now();
        let dir = Scratch::new("static-png");
        let src = write(&dir, "logo.png", base - Duration::from_secs(100));
        write(&dir, "logo.png.br", base - Duration::from_secs(50));
        let sf = static_file(&src, &meta(&src), "/logo.png", Some("gzip, br"), &[]);
        assert_eq!(sf.body, src);
        assert_eq!(header(&sf, "vary"), None);
        assert_eq!(header(&sf, "content-encoding"), None);
        assert_eq!(header(&sf, "content-type"), Some("image/png"));
    }

    #[test]
    fn fingerprinted_file_is_immutable() {
        let base = SystemTime::now();
        let dir = Scratch::new("static-fp");
        let src = write(&dir, "main.abcdef12.js", base - Duration::from_secs(100));
        let sf = static_file(&src, &meta(&src), "/main.abcdef12.js", None, &[]);
        assert_eq!(header(&sf, "cache-control"), Some(IMMUTABLE_CACHE));
    }

    #[test]
    fn assets_path_is_immutable() {
        let base = SystemTime::now();
        let dir = Scratch::new("static-assets");
        let src = write(&dir, "x.woff2", base - Duration::from_secs(100));
        let sf = static_file(&src, &meta(&src), "/assets/x.woff2", None, &[]);
        assert_eq!(header(&sf, "cache-control"), Some(IMMUTABLE_CACHE));
        assert_eq!(header(&sf, "content-type"), Some("font/woff2"));
        assert_eq!(count_header(&sf, "vary"), 0);
    }

    #[test]
    fn extra_headers_override() {
        let base = SystemTime::now();
        let dir = Scratch::new("static-extra");
        let src = write(&dir, "index.html", base - Duration::from_secs(100));
        let extra = [("vary".to_string(), "x".to_string())];
        let sf = static_file(&src, &meta(&src), "/index.html", None, &extra);
        assert_eq!(header(&sf, "vary"), Some("x"));
        assert_eq!(count_header(&sf, "vary"), 1);
    }
}
