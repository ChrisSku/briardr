// Device pairing and authentication (secrets/devices.json, secrets/pairing.json, and secrets/devices.lock,
// which every Briardr process holds while it changes the other two).
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
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

/// The devices file is the truth: `briardr revoke` changes it from another process while Briardr runs.
/// Every Auth reads it again when it changed, and changes it only as it is on disk.
#[derive(Debug)]
pub struct Auth {
    pub devices_file: PathBuf,
    pub pairing_file: PathBuf,
    lock_file: PathBuf,
    /// Guards the devices and the pairing file, so attempt counts can't race.
    devices: Mutex<Devices>,
    last_seen_dirty: AtomicBool,
    revoked: broadcast::Sender<String>,
}

/// The devices as the devices file last said, with the last-seen times `authenticate` changed since.
#[derive(Debug, Default)]
struct Devices {
    list: Vec<Device>,
    /// The file they were read from or saved as; another one there means another process changed it.
    stamp: Option<Stamp>,
}

/// What tells one version of a file from another: a save (`write_file_atomic`) is a new file, so a new
/// inode, and an edit in place changes its size or modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    ino: u64,
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(file: &Path) -> Option<Stamp> {
        let m = fs::metadata(file).ok()?;
        Some(Stamp {
            ino: m.ino(),
            len: m.len(),
            modified: m.modified().ok(),
        })
    }
}

/// The devices and their files held for a change: this process's lock, then the lock file against
/// other Briardr processes (`briardr pair`, `briardr revoke`).
struct Held<'a> {
    devices: MutexGuard<'a, Devices>,
    _files: File,
}

impl Auth {
    /// The devices file is read when they're first needed.
    pub fn new(devices_file: PathBuf, pairing_file: PathBuf) -> Self {
        Auth {
            lock_file: devices_file.with_extension("lock"),
            devices_file,
            pairing_file,
            devices: Mutex::default(),
            last_seen_dirty: AtomicBool::new(false),
            revoked: broadcast::channel(16).0,
        }
    }
    fn lock(&self) -> MutexGuard<'_, Devices> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// The devices, read again first if the devices file changed. One that can't be read changes nothing
    /// until it can: a passing error never signs devices out.
    fn current(&self) -> MutexGuard<'_, Devices> {
        let stamp = Stamp::of(&self.devices_file);
        let mut devices = self.lock();
        if devices.stamp != stamp {
            if let Ok(list) = read_devices(&self.devices_file) {
                self.adopt(&mut devices, list, stamp);
            }
        }
        devices
    }
    /// Holds the devices for a change. The lock file comes first, so a request waiting in `current`
    /// never waits for another process.
    fn hold(&self) -> Result<Held<'_>, ConfigError> {
        let files = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&self.lock_file)?;
        files.lock()?;
        Ok(Held {
            devices: self.lock(),
            _files: files,
        })
    }
    /// `hold`, with the devices read from the file, so a change is made to what is there now and never
    /// brings back a device another process removed.
    fn change(&self) -> Result<Held<'_>, ConfigError> {
        let mut held = self.hold()?;
        let stamp = Stamp::of(&self.devices_file);
        let list = read_devices(&self.devices_file)?;
        self.adopt(&mut held.devices, list, stamp);
        Ok(held)
    }
    /// Takes the devices file's `list`: a device missing from it was revoked by another process and is
    /// signed out here too (its connections closed, its alerts stopped). Last-seen times not saved yet
    /// are kept.
    fn adopt(&self, devices: &mut Devices, mut list: Vec<Device>, stamp: Option<Stamp>) {
        for d in &mut list {
            if let Some(known) = devices.list.iter().find(|k| k.id == d.id) {
                d.last_seen_at = d.last_seen_at.max(known.last_seen_at);
            }
        }
        let gone: Vec<String> = devices
            .list
            .iter()
            .filter(|k| !list.iter().any(|d| d.id == k.id))
            .map(|k| k.id.clone())
            .collect();
        *devices = Devices { list, stamp };
        for id in gone {
            let _ = self.revoked.send(id);
        }
    }
    /// Saves `list` as the devices, in the file and then here, so a failed save changes neither.
    fn save(&self, devices: &mut Devices, list: Vec<Device>) -> Result<(), ConfigError> {
        save_devices(&self.devices_file, &list)?;
        *devices = Devices {
            list,
            stamp: Stamp::of(&self.devices_file),
        };
        Ok(())
    }
    pub fn list(&self) -> Vec<DeviceInfo> {
        self.current()
            .list
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
    /// Reads the devices file again if it changed, so a device revoked by another process loses its live
    /// connections too. The caller runs this every second.
    pub fn reload(&self) {
        drop(self.current());
    }
    /// Create a one-time pairing code. Only its hash is stored. Returns the code and its expiry in ms.
    pub fn create_pairing_code(&self) -> (String, u64) {
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
        if let Ok(_held) = self.hold() {
            let _ = write_pairing(&self.pairing_file, &state);
        }
        (format!("{}-{}", &raw[..4], &raw[4..]), expires_at)
    }
    /// Returns the device and the cookie value on success.
    pub fn pair(
        &self,
        code: &str,
        name: &str,
        client: &str,
    ) -> Result<(Device, String), PairError> {
        let mut held = self.change().map_err(|_| PairError::Invalid)?;
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
        let mut next = held.devices.list.clone();
        next.push(device.clone());
        self.save(&mut held.devices, next)
            .map_err(|_| PairError::Invalid)?;
        let cookie = format!("{}.{}", device.id, token);
        Ok((device, cookie))
    }
    pub fn revoke(&self, id: &str) -> Result<bool, ConfigError> {
        let mut held = self.change()?;
        let devices = &mut held.devices;
        let kept: Vec<Device> = devices
            .list
            .iter()
            .filter(|d| d.id != id)
            .cloned()
            .collect();
        if kept.len() == devices.list.len() {
            return Ok(false);
        }
        // Keep the device if the change can't be saved, so the file and memory still agree.
        self.save(devices, kept)?;
        let _ = self.revoked.send(id.to_string());
        Ok(true)
    }
    /// Renames a device, with the name cleaned as at pairing; its cookie keeps working. `Ok(false)` when
    /// there's no such device.
    pub fn rename(&self, id: &str, name: &str) -> Result<bool, ConfigError> {
        let mut held = self.change()?;
        let mut next = held.devices.list.clone();
        let Some(device) = next.iter_mut().find(|d| d.id == id) else {
            return Ok(false);
        };
        device.name = clean_name(name);
        // Keep the old name if the change can't be saved, so the file and memory still agree.
        self.save(&mut held.devices, next)?;
        Ok(true)
    }
    /// Device for the request's cookie, if any.
    pub fn authenticate(&self, cookie_header: Option<&str>) -> Option<Device> {
        let value = cookie_header.and_then(|h| read_cookie(h, COOKIE))?;
        let dot = value.find('.')?;
        if dot == 0 {
            return None;
        }
        let (id, token) = (&value[..dot], &value[dot + 1..]);
        let mut devices = self.current();
        let device = devices.list.iter_mut().find(|d| d.id == id)?;
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
    /// Save the last-seen times that `authenticate` changed, into the devices file as it is now. The
    /// caller runs this about once a minute.
    pub fn flush_last_seen(&self) {
        if !self.last_seen_dirty.load(Ordering::Relaxed) {
            return;
        }
        // Not read or not saved: they're saved next time.
        let Ok(mut held) = self.change() else { return };
        self.last_seen_dirty.store(false, Ordering::Relaxed);
        let list = held.devices.list.clone();
        if self.save(&mut held.devices, list).is_err() {
            self.last_seen_dirty.store(true, Ordering::Relaxed);
        }
    }
    /// `flush_last_seen` as if a last-seen time had changed (`POST /__test/auth/flush`).
    #[cfg(feature = "test-hooks")]
    pub fn flush_now(&self) {
        self.last_seen_dirty.store(true, Ordering::Relaxed);
        self.flush_last_seen();
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

/// The devices file's devices: none when there is no file yet.
fn read_devices(file: &Path) -> Result<Vec<Device>, ConfigError> {
    match fs::read(file) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn save_devices(file: &Path, devices: &[Device]) -> Result<(), ConfigError> {
    let data = serde_json::to_string_pretty(devices)?;
    write_file_atomic(file, data.as_bytes(), 0o600)
}

fn write_pairing(file: &Path, state: &PairingState) -> Result<(), ConfigError> {
    let data = serde_json::to_vec(state)?;
    write_file_atomic(file, &data, 0o600)
}

/// A device name as Briardr keeps it: control characters removed (`briardr devices` prints it to a
/// terminal), trimmed, at most 60 characters. None when nothing is left.
pub fn device_name(name: &str) -> Option<String> {
    let kept: String = name.chars().filter(|c| !c.is_control()).collect();
    let cut: String = kept.trim().chars().take(60).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_string())
}

/// `device_name`, falling back to "Device".
fn clean_name(name: &str) -> String {
    device_name(name).unwrap_or_else(|| "Device".to_string())
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
    use crate::scratch::Scratch;
    use std::collections::BTreeSet;

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
        let dir = Scratch::new("auth-flow");
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

        assert!(auth.revoke(&device.id).unwrap());
        assert!(!auth.revoke(&device.id).unwrap());
        assert_eq!(auth.authenticate(Some(&header)), None);
        assert_eq!(revoked.try_recv().ok(), Some(device.id.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_device_revoked_by_another_process_is_signed_out_and_stays_out() {
        let dir = Scratch::new("auth-other-process");
        let server = auth_in(&dir);
        let mut revoked = server.subscribe_revoked();
        let (code, _) = server.create_pairing_code();
        let (lost, cookie) = server
            .pair(&code, "Lost phone", "client-a")
            .unwrap_or_else(|e| panic!("pair failed: {e:?}"));
        let (code, _) = server.create_pairing_code();
        let (kept, _) = server
            .pair(&code, "Laptop", "client-b")
            .unwrap_or_else(|e| panic!("pair failed: {e:?}"));
        let header = format!("__Host-briardr={cookie}");
        assert!(server.authenticate(Some(&header)).is_some());

        // `briardr revoke` while the server runs: another process, with an Auth of its own.
        assert!(auth_in(&dir).revoke(&lost.id).unwrap());
        assert_eq!(server.authenticate(Some(&header)), None);
        assert_eq!(revoked.try_recv().ok(), Some(lost.id.clone()));
        assert_eq!(server.list().len(), 1);

        // The server's later saves (last-seen times, a rename) don't bring it back.
        server.last_seen_dirty.store(true, Ordering::Relaxed);
        server.flush_last_seen();
        assert!(server.rename(&kept.id, "Work laptop").unwrap());
        assert!(!server.revoke(&lost.id).unwrap());
        let on_disk: Vec<String> = auth_in(&dir).list().into_iter().map(|d| d.id).collect();
        assert_eq!(on_disk, vec![kept.id.clone()]);
        assert_eq!(server.authenticate(Some(&header)), None);
        assert!(revoked.try_recv().is_err(), "signed out once");
    }

    #[test]
    fn saved_last_seen_times_keep_what_another_process_changed() {
        let dir = Scratch::new("auth-merge");
        let server = auth_in(&dir);
        let (code, _) = server.create_pairing_code();
        let (device, cookie) = server
            .pair(&code, "Phone", "client-a")
            .unwrap_or_else(|e| panic!("pair failed: {e:?}"));
        let seen = Some(now_ms() + 60_000);
        server.lock().list[0].last_seen_at = seen;
        server.last_seen_dirty.store(true, Ordering::Relaxed);

        assert!(auth_in(&dir).rename(&device.id, "Renamed").unwrap());
        server.flush_last_seen();
        let on_disk = auth_in(&dir).list();
        assert_eq!(on_disk[0].name, "Renamed");
        assert_eq!(on_disk[0].last_seen_at, seen);
        let header = format!("__Host-briardr={cookie}");
        assert_eq!(
            server.authenticate(Some(&header)).map(|d| d.name),
            Some("Renamed".to_string())
        );
    }

    #[test]
    fn a_devices_file_that_cant_be_read_changes_nothing() {
        let dir = Scratch::new("auth-unreadable");
        let server = auth_in(&dir);
        let (code, _) = server.create_pairing_code();
        let (device, cookie) = server
            .pair(&code, "Phone", "client-a")
            .unwrap_or_else(|e| panic!("pair failed: {e:?}"));
        let header = format!("__Host-briardr={cookie}");
        fs::write(dir.join("devices.json"), "[{\"id\":").unwrap();

        // Neither signed out on a guess nor saved over: it waits for a file it can read.
        assert!(server.authenticate(Some(&header)).is_some());
        assert!(server.revoke(&device.id).is_err());
        assert!(auth_in(&dir).revoke(&device.id).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("devices.json")).unwrap(),
            "[{\"id\":"
        );
        fs::remove_file(dir.join("devices.json")).unwrap();
        assert_eq!(
            server.authenticate(Some(&header)),
            None,
            "no file, no devices"
        );
    }

    #[test]
    fn device_names_lose_controls_and_spaces_and_stop_at_60() {
        assert_eq!(
            device_name(" Work\u{7}\u{7f}\u{9b} phone\n"),
            Some("Work phone".into())
        );
        assert_eq!(device_name(&"é".repeat(70)), Some("é".repeat(60)));
        assert_eq!(
            device_name(&format!("{} x", "a".repeat(59))),
            Some("a".repeat(59))
        );
        assert_eq!(device_name(" \u{1b}\t "), None);
        assert_eq!(clean_name(""), "Device");
    }

    #[test]
    fn rename_keeps_the_device_signed_in_and_survives_a_restart() {
        let dir = Scratch::new("auth-rename");
        let auth = auth_in(&dir);
        let (code, _) = auth.create_pairing_code();
        let (device, cookie) = auth
            .pair(&code, "Phone", "client-a")
            .unwrap_or_else(|e| panic!("pair failed: {e:?}"));
        let header = format!("__Host-briardr={cookie}");

        assert!(auth.rename(&device.id, "Work\u{7} phone").unwrap());
        assert!(!auth.rename("no-such-device", "x").unwrap());
        assert_eq!(auth.list()[0].name, "Work phone");
        assert_eq!(
            auth.authenticate(Some(&header)).map(|d| d.name),
            Some("Work phone".to_string())
        );
        assert_eq!(auth_in(&dir).list()[0].name, "Work phone");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_codes_lock_client_then_code() {
        let dir = Scratch::new("auth-locks");
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
        let dir = Scratch::new("auth-shape");
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
