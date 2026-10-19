// Diagnostics log. Redaction by construction: only allow-listed string keys pass.
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const LOG_ROTATE_BYTES: u64 = 1024 * 1024;
const LOG_FILE: &str = "connection.log";
/// The only keys whose string values are recorded.
const STRING_KEYS: &[&str] = &[
    "category", "reason", "kind", "mode", "net", "provider", "method", "error", "problem",
    "stream", "device",
];

#[derive(Debug, Clone, PartialEq)]
pub enum DiagValue {
    Num(f64),
    Bool(bool),
    Str(String),
    Null,
}

// Whole numbers are written without a fraction, as JSON.stringify does for the log.
impl Serialize for DiagValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            DiagValue::Num(x) if x.fract() == 0.0 && x.abs() < 9.0e15 => s.serialize_i64(*x as i64),
            DiagValue::Num(x) => s.serialize_f64(*x),
            DiagValue::Bool(b) => s.serialize_bool(*b),
            DiagValue::Str(v) => s.serialize_str(v),
            DiagValue::Null => s.serialize_none(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiagEntry {
    /// ISO-8601 UTC with milliseconds.
    pub t: String,
    /// "server" | "client"
    pub src: String,
    pub ev: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, DiagValue>,
}

pub struct Diagnostics {
    pub dir: PathBuf,
    pub salt_file: PathBuf,
    salt: Vec<u8>,
    /// Current size of connection.log; guards append and rotate together.
    size: Mutex<u64>,
}

impl Diagnostics {
    pub fn new(dir: PathBuf, salt_file: PathBuf) -> io::Result<Self> {
        create_private_dir(&dir)?;
        let salt = match fs::read(&salt_file) {
            Ok(salt) => salt,
            Err(_) => {
                let mut salt = vec![0u8; 16];
                rand::fill(&mut salt);
                if let Some(parent) = salt_file.parent().filter(|p| !p.as_os_str().is_empty()) {
                    create_private_dir(parent)?;
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&salt_file)?;
                file.write_all(&salt)?;
                salt
            }
        };
        let size = fs::metadata(dir.join(LOG_FILE))
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(Diagnostics {
            dir,
            salt_file,
            salt,
            size: Mutex::new(size),
        })
    }

    /// Stable anonymous ID for any identifier (pane IDs, device IDs, ...).
    pub fn anon(&self, id: &str) -> Option<String> {
        if id.is_empty() {
            return None;
        }
        let mac = hex::encode(hmac_sha256(&self.salt, id.as_bytes()));
        Some(format!("a{}", &mac[..10]))
    }

    /// Coerce a value into something safe to log, or drop it.
    pub fn clean(v: &Value, key: &str) -> Option<DiagValue> {
        match v {
            Value::Null => Some(DiagValue::Null),
            Value::Bool(b) => Some(DiagValue::Bool(*b)),
            Value::Number(n) => {
                let x = n.as_f64().filter(|x| x.is_finite())?;
                // JS Math.round semantics, not f64::round.
                Some(DiagValue::Num((x * 1000.0 + 0.5).floor() / 1000.0))
            }
            Value::String(s) => {
                if STRING_KEYS.contains(&key) && is_slug(s) {
                    Some(DiagValue::Str(s.clone()))
                } else {
                    Some(DiagValue::Str("redacted".to_string()))
                }
            }
            Value::Array(_) | Value::Object(_) => None,
        }
    }

    pub fn record(
        &self,
        src: &str,
        ev: &str,
        fields: &BTreeMap<String, Value>,
    ) -> Option<DiagEntry> {
        if !is_key(ev) {
            return None;
        }
        let mut entry = DiagEntry {
            t: now_iso8601(),
            src: src.to_string(),
            ev: ev.to_string(),
            fields: BTreeMap::new(),
        };
        for (k, v) in fields {
            if !is_key(k) || k == "t" || k == "src" || k == "ev" {
                continue;
            }
            if let Some(c) = Self::clean(v, k) {
                entry.fields.insert(k.clone(), c);
            }
        }
        let mut line = serde_json::to_string(&entry).ok()?;
        line.push('\n');
        self.append(&line);
        Some(entry)
    }

    pub fn event(&self, ev: &str, fields: &BTreeMap<String, Value>) {
        self.record("server", ev, fields);
    }

    fn append(&self, line: &str) {
        let bytes = line.len() as u64;
        let mut size = self.size.lock().unwrap_or_else(|e| e.into_inner());
        if *size + bytes > LOG_ROTATE_BYTES {
            let _ = fs::rename(
                self.dir.join(LOG_FILE),
                self.dir.join(format!("{LOG_FILE}.1")),
            );
            *size = 0;
        }
        let written = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.dir.join(LOG_FILE))
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if written.is_ok() {
            *size += bytes;
        }
    }

    /// All retained entries, oldest first (previous file, then current).
    pub fn export(&self) -> String {
        let mut out = String::new();
        for name in [format!("{LOG_FILE}.1"), LOG_FILE.to_string()] {
            if let Ok(bytes) = fs::read(self.dir.join(name)) {
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        out
    }
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// `^[a-z][a-z0-9_]{0,31}$`
fn is_key(s: &str) -> bool {
    is_word(s, |b| b == b'_')
}

/// `^[a-z][a-z0-9_-]{0,31}$`
fn is_slug(s: &str) -> bool {
    is_word(s, |b| b == b'_' || b == b'-')
}

fn is_word(s: &str, extra: impl Fn(u8) -> bool) -> bool {
    let bytes = s.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || extra(b))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        k[..digest.len()].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(data);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner_hash);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// ISO-8601 UTC with milliseconds, e.g. 2026-10-11T08:30:00.123Z.
fn now_iso8601() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = since.as_millis() as i64;
    let (secs, ms) = (millis.div_euclid(1000), millis.rem_euclid(1000));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "briardr-diag-{name}-{}-{nanos}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fields(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn hmac_matches_rfc4231() {
        let key1 = [0x0bu8; 20];
        assert_eq!(
            hex::encode(hmac_sha256(&key1, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn clean_values() {
        let s = |v: &str| Value::String(v.to_string());
        assert_eq!(
            Diagnostics::clean(&s("timeout"), "category"),
            Some(DiagValue::Str("timeout".into()))
        );
        assert_eq!(
            Diagnostics::clean(&s("alice"), "user"),
            Some(DiagValue::Str("redacted".into()))
        );
        assert_eq!(
            Diagnostics::clean(&s("wifi"), "net"),
            Some(DiagValue::Str("wifi".into()))
        );
        for bad in ["/Users/x/project", "10.0.0.1:443", "Bearer abc.def"] {
            assert_eq!(
                Diagnostics::clean(&s(bad), "category"),
                Some(DiagValue::Str("redacted".into()))
            );
        }
        assert_eq!(Diagnostics::clean(&serde_json::json!({"a": 1}), "x"), None);
        assert_eq!(
            Diagnostics::clean(&serde_json::json!(12.34567), "x"),
            Some(DiagValue::Num(12.346))
        );
    }

    #[test]
    fn anon_stable_and_shaped() {
        let dir = TempDir::new("anon");
        let salt = dir.0.join("salt");
        let d = Diagnostics::new(dir.0.clone(), salt.clone()).expect("new");
        let id = d.anon("w1:p1").expect("some");
        assert_eq!(id.len(), 11);
        assert!(id.starts_with('a'));
        assert!(id[1..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let again = Diagnostics::new(dir.0.clone(), salt).expect("new");
        assert_eq!(again.anon("w1:p1"), Some(id));
        assert_eq!(d.anon(""), None);
    }

    #[test]
    fn rotates_and_persists() {
        let dir = TempDir::new("rotate");
        let salt = dir.0.join("salt");
        let d = Diagnostics::new(dir.0.clone(), salt.clone()).expect("new");
        for i in 0..12000u64 {
            d.event(
                "retry_scheduled",
                &fields(&[
                    ("delay_ms", serde_json::json!(i)),
                    ("attempt", serde_json::json!(i % 7)),
                    ("category", serde_json::json!("network")),
                ]),
            );
        }
        let mut names: Vec<String> = fs::read_dir(&dir.0)
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(LOG_FILE))
            .collect();
        names.sort();
        assert_eq!(names, vec!["connection.log", "connection.log.1"]);
        for n in &names {
            let len = fs::metadata(dir.0.join(n)).expect("metadata").len();
            assert!(len <= LOG_ROTATE_BYTES);
        }
        let reopened = Diagnostics::new(dir.0.clone(), salt).expect("new");
        assert!(reopened.export().contains("retry_scheduled"));
    }

    #[test]
    fn record_redacts_privacy() {
        let dir = TempDir::new("privacy");
        let d = Diagnostics::new(dir.0.clone(), dir.0.join("salt")).expect("new");
        let entry = d
            .record(
                "client",
                "live_open",
                &fields(&[
                    (
                        "url",
                        serde_json::json!("https://100.64.0.9:7443/?token=abc"),
                    ),
                    ("user", serde_json::json!("alice")),
                    ("n", serde_json::json!(3)),
                ]),
            )
            .expect("entry");
        assert_eq!(
            entry.fields.get("url"),
            Some(&DiagValue::Str("redacted".into()))
        );
        assert_eq!(
            entry.fields.get("user"),
            Some(&DiagValue::Str("redacted".into()))
        );
        assert_eq!(entry.fields.get("n"), Some(&DiagValue::Num(3.0)));
        let text = fs::read_to_string(dir.0.join(LOG_FILE)).expect("log");
        assert!(!text.contains("token=abc"));
        assert!(!text.contains("100.64.0.9"));
        assert!(text.contains("\"n\":3"));
    }

    #[test]
    fn record_rejects_bad_ev() {
        let dir = TempDir::new("badev");
        let d = Diagnostics::new(dir.0.clone(), dir.0.join("salt")).expect("new");
        assert_eq!(d.record("server", "Bad-Event", &BTreeMap::new()), None);
        assert!(!dir.0.join(LOG_FILE).exists());
    }

    #[test]
    fn record_timestamp_format() {
        let dir = TempDir::new("timestamp");
        let d = Diagnostics::new(dir.0.clone(), dir.0.join("salt")).expect("new");
        let entry = d.record("server", "tick", &BTreeMap::new()).expect("entry");
        let b = entry.t.as_bytes();
        assert_eq!(b.len(), 24, "{}", entry.t);
        for (i, &c) in b.iter().enumerate() {
            match i {
                4 | 7 => assert_eq!(c, b'-'),
                10 => assert_eq!(c, b'T'),
                13 | 16 => assert_eq!(c, b':'),
                19 => assert_eq!(c, b'.'),
                23 => assert_eq!(c, b'Z'),
                _ => assert!(c.is_ascii_digit(), "{}", entry.t),
            }
        }
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(20_372), (2025, 10, 11));
    }
}
