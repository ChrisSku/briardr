// Device pairing and authentication (secrets/devices.json, secrets/pairing.json).
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::config::{read_json, write_file_atomic, ConfigError};

pub const COOKIE: &str = "__Host-briardr";
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTVWXYZ23456789"; // no 0/O/1/I/L/U
const CODE_TTL_MS: u64 = 10 * 60 * 1000;
pub const MAX_CLIENT_ATTEMPTS: u32 = 5;
pub const MAX_CODE_ATTEMPTS: u32 = 20;
const COOKIE_MAX_AGE_S: u64 = 400 * 24 * 3600;
const LAST_SEEN_THROTTLE_MS: u64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub token_hash: String,
    pub created_at: u64,
    pub last_seen_at: Option<u64>,
}

/// A device as the API shows it; the token hash stays inside Auth.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub last_seen_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct PairingState {
    code_hash: String,
    expires_at: u64,
    attempts: u32,
    /// Wrong guesses per client address (hashed), for this code only.
    #[serde(skip_serializing_if = "Option::is_none")]
    by_client: Option<BTreeMap<String, u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    NoCode,
    Expired,
    Invalid,
    Locked,
}

#[derive(Debug)]
pub struct Auth {
    pub devices_file: PathBuf,
    pub pairing_file: PathBuf,
    /// Guards the devices and the pairing file, so attempt counts can't race.
    devices: Mutex<Vec<Device>>,
    last_seen_dirty: AtomicBool,
    revoked: broadcast::Sender<String>,
}

impl Auth {
    pub fn new(devices_file: PathBuf, pairing_file: PathBuf) -> Self {
        let devices = read_json::<Vec<Device>>(&devices_file, Vec::new());
        Auth {
            devices_file,
            pairing_file,
            devices: Mutex::new(devices),
            last_seen_dirty: AtomicBool::new(false),
            revoked: broadcast::channel(16).0,
        }
    }
    fn lock(&self) -> MutexGuard<'_, Vec<Device>> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub fn list(&self) -> Vec<DeviceInfo> {
        self.lock()
            .iter()
            .map(|d| DeviceInfo {
                id: d.id.clone(),
                name: d.name.clone(),
                created_at: d.created_at,
                last_seen_at: d.last_seen_at,
            })
            .collect()
    }
    /// Receives the id of each revoked device, so live connections can be closed.
    pub fn subscribe_revoked(&self) -> broadcast::Receiver<String> {
        self.revoked.subscribe()
    }
    /// Create a one-time pairing code. Only its hash is stored. Returns the code and its expiry in ms.
    pub fn create_pairing_code(&self) -> (String, u64) {
        let _guard = self.lock();
        let mut bytes = [0u8; 8];
        rand::fill(&mut bytes);
        let raw: String = bytes
            .iter()
            .map(|b| char::from(CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()]))
            .collect();
        let expires_at = now_ms() + CODE_TTL_MS;
        let state = PairingState {
            code_hash: sha256(&raw),
            expires_at,
            attempts: 0,
            by_client: None,
        };
        // A failed write leaves no usable code, so the caller gets no_code on redeem.
        let _ = write_pairing(&self.pairing_file, &state);
        (format!("{}-{}", &raw[..4], &raw[4..]), expires_at)
    }
    /// Returns the device and the cookie value on success.
    pub fn pair(
        &self,
        code: &str,
        name: &str,
        client: &str,
    ) -> Result<(Device, String), PairError> {
        let mut devices = self.lock();
        let Some(mut state) = read_json::<Option<PairingState>>(&self.pairing_file, None) else {
            return Err(PairError::NoCode);
        };
        if now_ms() > state.expires_at {
            let _ = self.remove_pairing_file();
            return Err(PairError::Expired);
        }
        if state.attempts >= MAX_CODE_ATTEMPTS {
            let _ = self.remove_pairing_file();
            return Err(PairError::Locked);
        }
        let who = sha256(client)[..16].to_string();
        let client_attempts = state.by_client.as_ref().and_then(|m| m.get(&who)).copied();
        if client_attempts.unwrap_or(0) >= MAX_CLIENT_ATTEMPTS {
            return Err(PairError::Locked);
        }
        if !safe_eq_hex(&sha256(&normalize_code(code)), &state.code_hash) {
            state.attempts += 1;
            let by_client = state.by_client.get_or_insert_with(BTreeMap::new);
            let count = by_client.entry(who).or_insert(0);
            *count += 1;
            let client_attempts = *count;
            if state.attempts >= MAX_CODE_ATTEMPTS {
                let _ = self.remove_pairing_file();
            } else if write_pairing(&self.pairing_file, &state).is_err() {
                // Fail closed: an unsaved count would let guesses continue.
                return Err(PairError::Locked);
            }
            return Err(
                if state.attempts >= MAX_CODE_ATTEMPTS || client_attempts >= MAX_CLIENT_ATTEMPTS {
                    PairError::Locked
                } else {
                    PairError::Invalid
                },
            );
        }
        // A code that stays on disk after a match could be redeemed again.
        if self.remove_pairing_file().is_err() {
            return Err(PairError::Locked);
        }
        let mut token_bytes = [0u8; 32];
        rand::fill(&mut token_bytes);
        let token = URL_SAFE_NO_PAD.encode(token_bytes);
        let mut id_bytes = [0u8; 9];
        rand::fill(&mut id_bytes);
        let now = now_ms();
        let device = Device {
            id: URL_SAFE_NO_PAD.encode(id_bytes),
            name: clean_name(name),
            token_hash: sha256(&token),
            created_at: now,
            last_seen_at: Some(now),
        };
        let mut next = devices.clone();
        next.push(device.clone());
        save_devices(&self.devices_file, &next).map_err(|_| PairError::Invalid)?;
        *devices = next;
        let cookie = format!("{}.{}", device.id, token);
        Ok((device, cookie))
    }
    pub fn revoke(&self, id: &str) -> bool {
        let mut devices = self.lock();
        let kept: Vec<Device> = devices.iter().filter(|d| d.id != id).cloned().collect();
        if kept.len() == devices.len() {
            return false;
        }
        // Keep the device if the change can't be saved, so the file and memory still agree.
        if save_devices(&self.devices_file, &kept).is_err() {
            return false;
        }
        *devices = kept;
        let _ = self.revoked.send(id.to_string());
        true
    }
    /// Device for the request's cookie, if any.
    pub fn authenticate(&self, cookie_header: Option<&str>) -> Option<Device> {
        let value = cookie_header.and_then(|h| read_cookie(h, COOKIE))?;
        let dot = value.find('.')?;
        if dot == 0 {
            return None;
        }
        let (id, token) = (&value[..dot], &value[dot + 1..]);
        let mut devices = self.lock();
        let device = devices.iter_mut().find(|d| d.id == id)?;
        if !safe_eq_hex(&sha256(token), &device.token_hash) {
            return None;
        }
        let now = now_ms();
        if now.saturating_sub(device.last_seen_at.unwrap_or(0)) > LAST_SEEN_THROTTLE_MS {
            device.last_seen_at = Some(now);
            self.last_seen_dirty.store(true, Ordering::Relaxed);
        }
        Some(device.clone())
    }
    /// Save the last-seen times that `authenticate` changed. The caller runs this about once a minute.
    pub fn flush_last_seen(&self) {
        let devices = self.lock();
        if self.last_seen_dirty.swap(false, Ordering::Relaxed)
            && save_devices(&self.devices_file, &devices).is_err()
        {
            self.last_seen_dirty.store(true, Ordering::Relaxed);
        }
    }
    pub fn set_cookie_header(&self, value: &str) -> String {
        format!("{COOKIE}={value}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE_S}")
    }
    pub fn clear_cookie_header(&self) -> String {
        format!("{COOKIE}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0")
    }
    /// Removes the pairing file; a missing file counts as removed.
    fn remove_pairing_file(&self) -> io::Result<()> {
        match fs::remove_file(&self.pairing_file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

fn sha256(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn safe_eq_hex(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

fn save_devices(file: &Path, devices: &[Device]) -> Result<(), ConfigError> {
    let data = serde_json::to_string_pretty(devices)?;
    write_file_atomic(file, data.as_bytes(), 0o600)
}

fn write_pairing(file: &Path, state: &PairingState) -> Result<(), ConfigError> {
    let data = serde_json::to_vec(state)?;
    write_file_atomic(file, &data, 0o600)
}

/// Strips control characters, keeps 60 characters, and falls back to "Device".
fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '\u{0}'..='\u{1f}'))
        .take(60)
        .collect();
    if cleaned.is_empty() {
        "Device".to_string()
    } else {
        cleaned
    }
}

pub fn normalize_code(code: &str) -> String {
    code.to_uppercase()
        .chars()
        .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        .collect()
}

pub fn read_cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    // The first well-formed match wins; duplicates planted by other ports are ignored.
    header.split(';').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key.trim() == name).then(|| value.trim())
    })
}

/// Fixed-window limiter keyed by string (client address, device, ...).
#[derive(Debug)]
pub struct RateLimiter {
    pub limit: u32,
    pub window_ms: u64,
    pub max_keys: usize,
    pub hits: std::collections::HashMap<String, (u32, Instant)>,
}

impl RateLimiter {
    pub fn new(limit: u32, window_ms: u64) -> Self {
        RateLimiter {
            limit,
            window_ms,
            max_keys: 10_000,
            hits: Default::default(),
        }
    }
    pub fn allow(&mut self, key: &str) -> bool {
        let now = Instant::now();
        if self.hits.len() >= self.max_keys && !self.hits.contains_key(key) {
            self.hits.retain(|_, (_, reset)| now <= *reset);
            // Still full: forget entries in iteration order.
            let excess = self
                .hits
                .len()
                .saturating_sub(self.max_keys.saturating_sub(1));
            let victims: Vec<String> = self.hits.keys().take(excess).cloned().collect();
            for k in victims {
                self.hits.remove(&k);
            }
        }
        match self.hits.get_mut(key) {
            Some((n, reset)) if now <= *reset => {
                *n = n.saturating_add(1);
                *n <= self.limit
            }
            _ => {
                let reset = now + Duration::from_millis(self.window_ms);
                self.hits.insert(key.to_string(), (1, reset));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("briardr-auth-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn auth_in(dir: &Path) -> Auth {
        Auth::new(dir.join("devices.json"), dir.join("pairing.json"))
    }

    fn wrong_code(code: &str) -> String {
        let first = if code.starts_with('A') { 'B' } else { 'A' };
        format!("{first}{}", &code[1..])
    }

    fn keys_of(value: &serde_json::Value) -> BTreeSet<&str> {
        value
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    #[test]
    fn normalize_code_strips_case_and_separators() {
        assert_eq!(normalize_code("abcd-efgh"), "ABCDEFGH");
        assert_eq!(normalize_code(" 2 3-x_y "), "23XY");
    }

    #[test]
    fn read_cookie_rules() {
        let header =
            "a=1; __Host-briardr=first; __Host-briardr=second; nodash; x=  ; __Host-briardr=";
        assert_eq!(read_cookie(header, "__Host-briardr"), Some("first"));
        assert_eq!(read_cookie(header, "x"), Some(""));
        assert_eq!(read_cookie(header, "a"), Some("1"));
        assert_eq!(read_cookie(header, "nodash"), None);
        assert_eq!(read_cookie("k=", "k"), Some(""));
        assert_eq!(read_cookie("other=1", "k"), None);
    }

    #[test]
    fn safe_eq_hex_compares() {
        assert!(safe_eq_hex("abcd", "abcd"));
        assert!(!safe_eq_hex("abcd", "abce"));
        assert!(!safe_eq_hex("abcd", "abc"));
    }

    #[test]
    fn sha256_matches_known_digest() {
        assert_eq!(
            sha256("ABCDEFGH"),
            "9ac2197d9258257b1ae8463e4214e4cd0a578bc1517f2415928b91be4283fc48"
        );
    }

    #[test]
    fn rate_limiter_allows_then_denies_and_resets() {
        let mut limiter = RateLimiter::new(2, 60_000);
        assert!(limiter.allow("a"));
        assert!(limiter.allow("a"));
        assert!(!limiter.allow("a"));
        assert!(limiter.allow("b"));

        let mut short = RateLimiter::new(1, 1);
        assert!(short.allow("a"));
        assert!(!short.allow("a"));
        std::thread::sleep(Duration::from_millis(5));
        assert!(short.allow("a"));
    }

    #[test]
    fn rate_limiter_caps_keys() {
        let mut limiter = RateLimiter::new(5, 60_000);
        limiter.max_keys = 3;
        for i in 0..10 {
            assert!(limiter.allow(&format!("client-{i}")));
            assert!(limiter.hits.len() <= 3);
        }
        assert!(limiter.hits.contains_key("client-9"));
    }

    #[test]
    fn pair_authenticate_revoke_flow() {
        let dir = scratch_dir("flow");
        let auth = auth_in(&dir);
        let mut revoked = auth.subscribe_revoked();
        let (code, expires_at) = auth.create_pairing_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(expires_at > now_ms());

        let (device, cookie) = auth
            .pair(&code, "Phone\u{7}", "client-a")
            .unwrap_or_else(|e| panic!("pair failed: {e:?}"));
        assert_eq!(device.name, "Phone");
        assert!(cookie.starts_with(&format!("{}.", device.id)));
        assert_eq!(auth.pair(&code, "x", "client-a"), Err(PairError::NoCode));

        let header = format!("__Host-briardr={cookie}");
        assert_eq!(auth.authenticate(Some(&header)), Some(device.clone()));
        assert_eq!(auth.authenticate(None), None);
        assert_eq!(auth.authenticate(Some("__Host-briardr=.nope")), None);
        assert_eq!(auth.list().len(), 1);
        assert_eq!(auth.list()[0].id, device.id);

        assert!(auth.revoke(&device.id));
        assert!(!auth.revoke(&device.id));
        assert_eq!(auth.authenticate(Some(&header)), None);
        assert_eq!(revoked.try_recv().ok(), Some(device.id.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_codes_lock_client_then_code() {
        let dir = scratch_dir("locks");
        let auth = auth_in(&dir);
        let (code, _) = auth.create_pairing_code();
        let wrong = wrong_code(&normalize_code(&code));

        for _ in 0..4 {
            assert_eq!(auth.pair(&wrong, "x", "c1"), Err(PairError::Invalid));
        }
        assert_eq!(auth.pair(&wrong, "x", "c1"), Err(PairError::Locked));
        assert_eq!(auth.pair(&wrong, "x", "c1"), Err(PairError::Locked));
        // Other clients each get five more guesses; the 20th overall withdraws the code.
        for client in ["c2", "c3"] {
            for _ in 0..5 {
                let _ = auth.pair(&wrong, "x", client);
            }
        }
        for _ in 0..4 {
            assert_eq!(auth.pair(&wrong, "x", "c4"), Err(PairError::Invalid));
        }
        assert_eq!(auth.pair(&wrong, "x", "c4"), Err(PairError::Locked));
        assert_eq!(auth.pair(&code, "x", "c5"), Err(PairError::NoCode));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_shapes_match_ts() {
        let dir = scratch_dir("shape");
        let auth = auth_in(&dir);
        let (code, _) = auth.create_pairing_code();
        let pairing_path = dir.join("pairing.json");
        let read = |p: &Path| -> serde_json::Value {
            serde_json::from_str(&fs::read_to_string(p).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null)
        };
        assert_eq!(
            keys_of(&read(&pairing_path)),
            BTreeSet::from(["attempts", "codeHash", "expiresAt"])
        );

        let wrong = wrong_code(&normalize_code(&code));
        assert_eq!(auth.pair(&wrong, "x", "c1"), Err(PairError::Invalid));
        assert_eq!(
            keys_of(&read(&pairing_path)),
            BTreeSet::from(["attempts", "byClient", "codeHash", "expiresAt"])
        );

        let (device, _) = auth
            .pair(&code, "x", "c1")
            .unwrap_or_else(|e| panic!("pair failed: {e:?}"));
        let devices = read(&dir.join("devices.json"));
        let first = devices.as_array().and_then(|a| a.first()).cloned();
        assert_eq!(
            keys_of(&first.unwrap_or(serde_json::Value::Null)),
            BTreeSet::from(["createdAt", "id", "lastSeenAt", "name", "tokenHash"])
        );
        assert_eq!(device.name, "x");
        let _ = fs::remove_dir_all(&dir);
    }
}
