// Web Push. Payloads are end-to-end encrypted to each device's
// subscription keys (RFC 8291, aes128gcm, as the web-push npm package sends them), so the browser
// vendor's push service only relays opaque bytes. VAPID (RFC 8292) identifies the server with an
// ES256 JWT. Delivery is tracked honestly: sent -> accepted (push service returned 2xx) ->
// delivered (the device's service worker confirmed).
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes128Gcm;
use base64::prelude::{Engine, BASE64_URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::config::{read_json, write_file_atomic, ConfigError};
use crate::diag::Diagnostics;
use crate::http;

pub const DEFAULT_TTL_SECS: u32 = 3600;

/// Subscriptions beyond this many deliveries are trimmed to the newest `DELIVERIES_KEEP`.
const DELIVERIES_LIMIT: usize = 500;
const DELIVERIES_KEEP: usize = 300;
/// web-push's VAPID JWT lifetime (12 hours).
const JWT_TTL_SECS: u64 = 12 * 60 * 60;
/// aes128gcm record size (rs) written in the payload header, as web-push does.
const RECORD_SIZE: usize = 4096;
/// aes128gcm per-record overhead: the padding delimiter byte plus the GCM tag.
const RECORD_OVERHEAD: usize = 1 + 16;
/// Upper bound on one push request. web-push has none; a hung push service must not hold a send.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PushSub {
    pub device_id: String,
    pub endpoint: String,
    pub keys: PushKeys,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PushPayload {
    pub id: String,
    /// "needs_you" | "finished" | "ready" | "battery" | "test"
    pub kind: String,
    pub title: String,
    pub body: String,
    /// Stream to open when tapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    pub tag: String,
    pub ts: u64,
    /// Agents that need attention when the alert was sent, for the app's icon badge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRecord {
    pub id: String,
    pub kind: String,
    pub device_id: String,
    pub sent_at: u64,
    pub accepted: Option<bool>,
    pub status_code: Option<u16>,
    pub delivered_at: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Error)]
pub enum PushError {
    #[error("Invalid subscription")]
    InvalidSubscription,
    #[error("push storage: {0}")]
    Storage(#[from] ConfigError),
}

/// The VAPID key pair in secrets/vapid.json. Never logged or printed.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VapidKeys {
    public_key: String,
    private_key: String,
}

/// What one push request needs besides the subscription and the encrypted body.
#[derive(Clone)]
pub struct SendOpts {
    pub ttl: u32,
    pub urgency: &'static str,
    pub topic: String,
    pub subject: String,
    pub vapid_public: String,
    pub vapid_private: String,
}

/// `Ok(status)` is a resolved request (web-push's statusCode); `Err(Some(code))` a refused one
/// (WebPushError); `Err(None)` a network or setup failure.
pub type SendFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<u16, Option<u16>>> + Send>>;
/// Sends one encrypted payload (`String` is the plaintext JSON, for the test hook to inspect).
pub type Sender = Arc<dyn Fn(PushSub, String, SendOpts) -> SendFuture + Send + Sync>;

struct State {
    subs: Vec<PushSub>,
    deliveries: Vec<DeliveryRecord>,
    subject: String,
}

struct Inner {
    subs_file: PathBuf,
    state: Mutex<State>,
    /// Serializes writes of the subscriptions file, so the file ends up with the latest state.
    save_lock: Mutex<()>,
}

pub struct Push {
    vapid: VapidKeys,
    diag: Arc<Diagnostics>,
    inner: Arc<Inner>,
    sender: Mutex<Sender>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Push {
    pub fn new(
        vapid_file: &Path,
        subs_file: PathBuf,
        subject: String,
        diag: Arc<Diagnostics>,
    ) -> io::Result<Push> {
        let vapid = match read_json::<Option<VapidKeys>>(vapid_file, None) {
            Some(keys) => keys,
            None => {
                let keys = generate_vapid();
                let data = serde_json::to_vec(&keys).map_err(io::Error::other)?;
                write_file_atomic(vapid_file, &data, 0o600).map_err(io::Error::other)?;
                keys
            }
        };
        let subs = read_json::<Vec<PushSub>>(&subs_file, Vec::new());
        let client = http::client(SEND_TIMEOUT).map_err(io::Error::other)?;
        Ok(Push {
            vapid,
            diag,
            inner: Arc::new(Inner {
                subs_file,
                state: Mutex::new(State {
                    subs,
                    deliveries: Vec::new(),
                    subject,
                }),
                save_lock: Mutex::new(()),
            }),
            sender: Mutex::new(default_sender(client)),
        })
    }

    pub fn public_key(&self) -> String {
        self.vapid.public_key.clone()
    }

    /// The VAPID subject currently in use.
    pub fn subject(&self) -> String {
        lock(&self.inner.state).subject.clone()
    }

    pub fn set_subject(&self, subject: String) {
        lock(&self.inner.state).subject = subject;
    }

    /// Test hook: replaces the network send.
    pub fn set_sender(&self, sender: Sender) {
        *lock(&self.sender) = sender;
    }

    pub fn subscribe(
        &self,
        device_id: &str,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
    ) -> Result<(), PushError> {
        if !endpoint.starts_with("https://") || p256dh.is_empty() || auth.is_empty() {
            return Err(PushError::InvalidSubscription);
        }
        {
            let mut st = lock(&self.inner.state);
            st.subs
                .retain(|s| s.endpoint != endpoint && s.device_id != device_id);
            st.subs.push(PushSub {
                device_id: device_id.to_string(),
                endpoint: endpoint.to_string(),
                keys: PushKeys {
                    p256dh: p256dh.to_string(),
                    auth: auth.to_string(),
                },
                created_at: now_ms(),
            });
        }
        persist(&self.inner)?;
        Ok(())
    }

    pub fn unsubscribe_device(&self, device_id: &str) -> Result<(), PushError> {
        lock(&self.inner.state)
            .subs
            .retain(|s| s.device_id != device_id);
        persist(&self.inner)?;
        Ok(())
    }

    pub fn has_device(&self, device_id: &str) -> bool {
        lock(&self.inner.state)
            .subs
            .iter()
            .any(|s| s.device_id == device_id)
    }

    pub fn device_count(&self) -> usize {
        lock(&self.inner.state).subs.len()
    }

    /// The push endpoint this device subscribed with, so it can tell whether Briardr has its current one.
    pub fn device_endpoint(&self, device_id: &str) -> Option<String> {
        lock(&self.inner.state)
            .subs
            .iter()
            .find(|s| s.device_id == device_id)
            .map(|s| s.endpoint.clone())
    }

    /// Send to every subscribed device (or one device). Resolves with one delivery record per
    /// device, in subscription order.
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        &self,
        kind: &str,
        title: &str,
        body: &str,
        stream_id: Option<String>,
        tag: &str,
        only_device: Option<&str>,
        ttl_secs: u32,
    ) -> Vec<DeliveryRecord> {
        self.send_counted(
            kind,
            title,
            body,
            stream_id,
            tag,
            only_device,
            ttl_secs,
            None,
        )
        .await
    }

    /// `send`, with the number of agents that need attention now (`attention`, for the icon badge).
    #[allow(clippy::too_many_arguments)]
    pub async fn send_counted(
        &self,
        kind: &str,
        title: &str,
        body: &str,
        stream_id: Option<String>,
        tag: &str,
        only_device: Option<&str>,
        ttl_secs: u32,
        attention: Option<u32>,
    ) -> Vec<DeliveryRecord> {
        let only_device = only_device.filter(|d| !d.is_empty());
        let sender = lock(&self.sender).clone();
        let (targets, subject) = {
            let st = lock(&self.inner.state);
            let targets: Vec<PushSub> = st
                .subs
                .iter()
                .filter(|s| only_device.is_none_or(|d| s.device_id == d))
                .cloned()
                .collect();
            (targets, st.subject.clone())
        };
        let urgency = match kind {
            "needs_you" | "battery" | "test" => "high",
            _ => "normal",
        };
        let topic = topic_for(tag);

        let mut set = tokio::task::JoinSet::new();
        for (index, sub) in targets.iter().enumerate() {
            let id = random_id();
            let rec = DeliveryRecord {
                id: id.clone(),
                kind: kind.to_string(),
                device_id: sub.device_id.clone(),
                sent_at: now_ms(),
                accepted: None,
                status_code: None,
                delivered_at: None,
                error: None,
            };
            lock(&self.inner.state).deliveries.push(rec.clone());
            let payload = PushPayload {
                id,
                kind: kind.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                stream_id: stream_id.clone(),
                tag: tag.to_string(),
                ts: now_ms(),
                attention,
            };
            let job = Delivery {
                index,
                inner: self.inner.clone(),
                diag: self.diag.clone(),
                sender: sender.clone(),
                sub: sub.clone(),
                plaintext: serde_json::to_string(&payload).unwrap_or_default(),
                opts: SendOpts {
                    ttl: ttl_secs,
                    urgency,
                    topic: topic.clone(),
                    subject: subject.clone(),
                    vapid_public: self.vapid.public_key.clone(),
                    vapid_private: self.vapid.private_key.clone(),
                },
                rec,
            };
            set.spawn(job.run());
        }

        let mut slots: Vec<Option<DeliveryRecord>> = vec![None; targets.len()];
        while let Some(joined) = set.join_next().await {
            if let Ok((index, rec)) = joined {
                if let Some(slot) = slots.get_mut(index) {
                    *slot = Some(rec);
                }
            }
        }
        trim(&self.inner);
        slots.into_iter().flatten().collect()
    }

    /// Called by the device's service worker when a notification was actually received.
    pub fn ack(&self, id: &str, device_id: &str) -> bool {
        let mut st = lock(&self.inner.state);
        let Some(rec) = st.deliveries.iter_mut().find(|r| r.id == id) else {
            return false;
        };
        if rec.device_id != device_id {
            return false;
        }
        if rec.delivered_at.is_none() {
            let now = now_ms();
            rec.delivered_at = Some(now);
            let latency = now as i64 - rec.sent_at as i64;
            let fields = BTreeMap::from([
                ("kind".to_string(), Value::from(rec.kind.as_str())),
                ("latency_ms".to_string(), Value::from(latency)),
            ]);
            self.diag.event("push_delivered", &fields);
        }
        true
    }

    pub fn delivery(&self, id: &str) -> Option<DeliveryRecord> {
        lock(&self.inner.state)
            .deliveries
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }
}

/// One target's send: the request, the outcome bookkeeping and the diagnostics event.
struct Delivery {
    index: usize,
    inner: Arc<Inner>,
    diag: Arc<Diagnostics>,
    sender: Sender,
    sub: PushSub,
    plaintext: String,
    opts: SendOpts,
    rec: DeliveryRecord,
}

impl Delivery {
    async fn run(self) -> (usize, DeliveryRecord) {
        let Delivery {
            index,
            inner,
            diag,
            sender,
            sub,
            plaintext,
            opts,
            mut rec,
        } = self;
        let outcome = sender(sub.clone(), plaintext, opts).await;
        let mut gone = false;
        match outcome {
            Ok(status) => {
                rec.status_code = Some(status);
                rec.accepted = Some((200..300).contains(&status));
            }
            Err(code) => {
                rec.status_code = code;
                rec.accepted = Some(false);
                rec.error = Some(match code {
                    Some(404) | Some(410) => {
                        gone = true;
                        "subscription_gone".to_string()
                    }
                    Some(code) if code != 0 => format!("push_service_{code}"),
                    _ => "network".to_string(),
                });
            }
        }
        if gone {
            lock(&inner.state)
                .subs
                .retain(|s| s.endpoint != sub.endpoint);
            let saver = inner.clone();
            // The outcome is already recorded; a failed save only leaves the file stale.
            let _ = tokio::task::spawn_blocking(move || persist(&saver)).await;
        }
        {
            let mut st = lock(&inner.state);
            if let Some(slot) = st.deliveries.iter_mut().find(|r| r.id == rec.id) {
                *slot = rec.clone();
            }
        }
        let fields = BTreeMap::from([
            ("kind".to_string(), Value::from(rec.kind.as_str())),
            ("accepted".to_string(), Value::from(rec.accepted)),
            (
                "status".to_string(),
                rec.status_code.map_or(Value::Null, Value::from),
            ),
            (
                "device".to_string(),
                diag.anon(&sub.device_id).map_or(Value::Null, Value::from),
            ),
        ]);
        diag.event("push_sent", &fields);
        (index, rec)
    }
}

fn trim(inner: &Inner) {
    let mut st = lock(&inner.state);
    if st.deliveries.len() > DELIVERIES_LIMIT {
        let excess = st.deliveries.len() - DELIVERIES_KEEP;
        st.deliveries.drain(..excess);
    }
}

/// Write the subscriptions file (owner-only) after every change.
fn persist(inner: &Inner) -> Result<(), ConfigError> {
    let _save = lock(&inner.save_lock);
    let data = {
        let st = lock(&inner.state);
        serde_json::to_string_pretty(&st.subs)?
    };
    write_file_atomic(&inner.subs_file, data.as_bytes(), 0o600)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn random_id() -> String {
    let mut bytes = [0u8; 9];
    rand::fill(&mut bytes[..]);
    BASE64_URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url(sha256(tag)), first 32 characters: the Web Push Topic header.
fn topic_for(tag: &str) -> String {
    let digest = BASE64_URL_SAFE_NO_PAD.encode(Sha256::digest(tag.as_bytes()));
    digest[..32].to_string()
}

fn b64(bytes: &[u8]) -> String {
    BASE64_URL_SAFE_NO_PAD.encode(bytes)
}

/// Lenient decode for subscription keys, as Node's Buffer.from(_, 'base64url') reads them.
fn decode_b64(s: &str) -> Option<Vec<u8>> {
    BASE64_URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok()
}

/// The VAPID key format check (url-safe alphabet, no padding), then decode.
fn decode_vapid_key(s: &str, len: usize) -> Option<Vec<u8>> {
    let url_safe = !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    if !url_safe {
        return None;
    }
    decode_b64(s).filter(|bytes| bytes.len() == len)
}

/// A P-256 secret key from the OS-independent RNG. Retries the (vanishingly rare) invalid scalar.
fn random_secret() -> SecretKey {
    loop {
        let mut bytes = [0u8; 32];
        rand::fill(&mut bytes[..]);
        if let Ok(key) = SecretKey::from_slice(&bytes) {
            return key;
        }
    }
}

fn generate_vapid() -> VapidKeys {
    let secret = random_secret();
    let public = secret.public_key().to_sec1_point(false);
    VapidKeys {
        public_key: b64(public.as_bytes()),
        private_key: b64(&secret.to_bytes()),
    }
}

/// The push service origin, as web-push's `protocol + '//' + host` builds it.
fn push_audience(endpoint: &str) -> Option<String> {
    let url = Url::parse(endpoint).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// The `Authorization` value web-push sends: `vapid t=<jwt>, k=<public key>`.
fn vapid_authorization(
    endpoint: &str,
    subject: &str,
    public_key: &str,
    private_key: &str,
    now_secs: u64,
) -> Option<String> {
    let audience = push_audience(endpoint)?;
    let subject_ok = Url::parse(subject)
        .is_ok_and(|u| !subject.is_empty() && matches!(u.scheme(), "https" | "mailto"));
    if !subject_ok {
        return None;
    }
    decode_vapid_key(public_key, 65)?;
    let private_bytes = decode_vapid_key(private_key, 32)?;
    let signing = SigningKey::from_slice(&private_bytes).ok()?;
    let claims = format!(
        r#"{{"aud":{},"exp":{},"sub":{}}}"#,
        json_string(&audience),
        now_secs + JWT_TTL_SECS,
        json_string(subject)
    );
    let input = format!(
        "{}.{}",
        b64(br#"{"typ":"JWT","alg":"ES256"}"#),
        b64(claims.as_bytes())
    );
    let signature: Signature = signing.sign(input.as_bytes());
    Some(format!(
        "vapid t={input}.{}, k={public_key}",
        b64(&signature.to_bytes())
    ))
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// RFC 8291 aes128gcm body with a fresh salt and sender key, as web-push's sendNotification makes.
fn encrypt(p256dh: &str, auth: &str, plaintext: &[u8]) -> Option<Vec<u8>> {
    let mut salt = [0u8; 16];
    rand::fill(&mut salt[..]);
    encrypt_with(p256dh, auth, plaintext, &salt, &random_secret())
}

/// The payload body for one subscription with a given salt and ephemeral sender key.
fn encrypt_with(
    p256dh: &str,
    auth: &str,
    plaintext: &[u8],
    salt: &[u8; 16],
    sender: &SecretKey,
) -> Option<Vec<u8>> {
    let ua_public = decode_b64(p256dh).filter(|k| k.len() == 65)?;
    let auth_secret = decode_b64(auth).filter(|a| a.len() >= 16)?;
    let receiver = PublicKey::from_sec1_bytes(&ua_public).ok()?;
    let as_public = sender.public_key().to_sec1_point(false);
    let shared = diffie_hellman(sender.to_nonzero_scalar(), receiver.as_affine());

    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(&ua_public);
    info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&auth_secret), shared.raw_secret_bytes())
        .expand(&info, &mut ikm)
        .ok()?;

    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .ok()?;
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).ok()?;
    let cipher = Aes128Gcm::new_from_slice(&cek).ok()?;

    let mut body = Vec::with_capacity(21 + 65 + plaintext.len() + 32);
    body.extend_from_slice(salt);
    body.extend_from_slice(&(RECORD_SIZE as u32).to_be_bytes());
    body.push(as_public.as_bytes().len() as u8);
    body.extend_from_slice(as_public.as_bytes());

    let chunk = RECORD_SIZE - RECORD_OVERHEAD;
    let mut start = 0;
    let mut counter: u64 = 0;
    loop {
        let end = start + chunk;
        let last = end >= plaintext.len();
        let mut record = plaintext[start.min(plaintext.len())..end.min(plaintext.len())].to_vec();
        record.push(if last { 2 } else { 1 });
        let mut record_nonce = nonce;
        for (i, byte) in counter.to_be_bytes()[2..].iter().enumerate() {
            record_nonce[6 + i] ^= byte;
        }
        let sealed = cipher
            .encrypt((&record_nonce).into(), record.as_slice())
            .ok()?;
        body.extend_from_slice(&sealed);
        if last {
            return Some(body);
        }
        start = end;
        counter += 1;
    }
}

fn default_sender(client: reqwest::Client) -> Sender {
    Arc::new(
        move |sub: PushSub, plaintext: String, opts: SendOpts| -> SendFuture {
            let client = client.clone();
            Box::pin(async move { web_push(&client, &sub, &plaintext, &opts).await }) as SendFuture
        },
    )
}

async fn web_push(
    client: &reqwest::Client,
    sub: &PushSub,
    plaintext: &str,
    opts: &SendOpts,
) -> Result<u16, Option<u16>> {
    let now_secs = now_ms() / 1000;
    let authorization = vapid_authorization(
        &sub.endpoint,
        &opts.subject,
        &opts.vapid_public,
        &opts.vapid_private,
        now_secs,
    )
    .ok_or(None)?;
    let body = encrypt(&sub.keys.p256dh, &sub.keys.auth, plaintext.as_bytes()).ok_or(None)?;
    let response = client
        .post(sub.endpoint.as_str())
        .header("TTL", opts.ttl.to_string())
        .header("Urgency", opts.urgency)
        .header("Topic", opts.topic.as_str())
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", authorization)
        .body(body)
        .send()
        .await
        .map_err(|_| None)?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        Ok(status)
    } else {
        Err(Some(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use p256::ecdsa::VerifyingKey;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use p256::ecdsa::signature::Verifier as _;

    fn diagnostics(dir: &Path) -> Arc<Diagnostics> {
        Arc::new(Diagnostics::new(dir.join("diag"), dir.join("diag-salt")).expect("diag"))
    }

    fn new_push(dir: &Path) -> Push {
        Push::new(
            &dir.join("vapid.json"),
            dir.join("subs.json"),
            "https://briardr.test".to_string(),
            diagnostics(dir),
        )
        .expect("push")
    }

    /// A browser-like subscription key pair: the receiver's secret, and its public p256dh.
    fn receiver_keys() -> (SecretKey, String, String) {
        let secret = random_secret();
        let p256dh = b64(secret.public_key().to_sec1_point(false).as_bytes());
        (secret, p256dh, b64(&[7u8; 16]))
    }

    type Seen = Arc<Mutex<Vec<(String, SendOpts)>>>;

    /// Records every (body, opts) it is given and answers with `result`.
    fn fake_sender(result: Result<u16, Option<u16>>) -> (Sender, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let sender: Sender = Arc::new(move |_sub, body: String, opts: SendOpts| {
            lock(&log).push((body, opts));
            Box::pin(async move { result }) as SendFuture
        });
        (sender, seen)
    }

    #[tokio::test]
    async fn accepted_and_delivered_are_separate_confirmed_states() {
        let dir = Scratch::new("push-accepted");
        let push = new_push(&dir);
        let (sender, seen) = fake_sender(Ok(201));
        push.set_sender(sender);
        let (_, p256dh, auth) = receiver_keys();
        push.subscribe("dev1", "https://web.push.apple.com/abc", &p256dh, &auth)
            .expect("subscribe");

        let records = push
            .send(
                "needs_you",
                "Needs you: x",
                "api · backend",
                Some("w1:p1".to_string()),
                "stream:w1:p1",
                None,
                DEFAULT_TTL_SECS,
            )
            .await;
        let rec = &records[0];
        assert_eq!(rec.accepted, Some(true));
        assert_eq!(rec.status_code, Some(201));
        assert_eq!(
            rec.delivered_at, None,
            "not delivered until the device confirms"
        );
        assert!(
            !push.ack(&rec.id, "other-device"),
            "only the target device can confirm"
        );
        assert!(push.ack(&rec.id, "dev1"));
        let first_ack = push.delivery(&rec.id).and_then(|r| r.delivered_at);
        assert!(first_ack.is_some());
        assert!(push.ack(&rec.id, "dev1"));
        assert_eq!(
            push.delivery(&rec.id).and_then(|r| r.delivered_at),
            first_ack
        );

        let seen = lock(&seen);
        let payload: Value = serde_json::from_str(&seen[0].0).expect("json");
        let mut keys: Vec<&str> = payload
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["body", "id", "kind", "streamId", "tag", "title", "ts"]
        );
        assert_eq!(payload["id"], rec.id.as_str());
        assert_eq!(seen[0].1.urgency, "high");
        assert_eq!(seen[0].1.ttl, DEFAULT_TTL_SECS);
    }

    #[tokio::test]
    async fn refused_push_is_not_sent_and_expired_subscription_is_removed() {
        let dir = Scratch::new("push-gone");
        let push = new_push(&dir);
        let (_, p256dh, auth) = receiver_keys();
        push.subscribe("dev1", "https://web.push.apple.com/gone", &p256dh, &auth)
            .expect("subscribe");
        push.set_sender(fake_sender(Err(Some(410))).0);

        let records = push.send("test", "t", "b", None, "test", None, 60).await;
        assert_eq!(records[0].accepted, Some(false));
        assert_eq!(records[0].status_code, Some(410));
        assert_eq!(records[0].error.as_deref(), Some("subscription_gone"));
        assert!(!push.has_device("dev1"));
        let saved = fs::read_to_string(dir.join("subs.json")).expect("subs file");
        assert_eq!(saved, "[]");
    }

    #[tokio::test]
    async fn error_strings_and_resolved_non_2xx() {
        let dir = Scratch::new("push-errors");
        let push = new_push(&dir);
        let (_, p256dh, auth) = receiver_keys();
        push.subscribe("dev1", "https://web.push.apple.com/a", &p256dh, &auth)
            .expect("subscribe");

        push.set_sender(fake_sender(Err(Some(500))).0);
        let rec = push
            .send("finished", "t", "b", None, "x", None, 60)
            .await
            .remove(0);
        assert_eq!(rec.error.as_deref(), Some("push_service_500"));
        assert_eq!(rec.accepted, Some(false));
        assert!(push.has_device("dev1"), "a 5xx keeps the subscription");

        push.set_sender(fake_sender(Err(None)).0);
        let rec = push
            .send("finished", "t", "b", None, "x", None, 60)
            .await
            .remove(0);
        assert_eq!(rec.error.as_deref(), Some("network"));
        assert_eq!(rec.status_code, None);

        push.set_sender(fake_sender(Ok(302)).0);
        let rec = push
            .send("finished", "t", "b", None, "x", None, 60)
            .await
            .remove(0);
        assert_eq!(rec.accepted, Some(false));
        assert_eq!(rec.status_code, Some(302));
        assert_eq!(rec.error, None);
    }

    #[tokio::test]
    async fn only_device_targets_one_subscription_and_urgency_and_topic_follow_the_kind() {
        let dir = Scratch::new("push-targets");
        let push = new_push(&dir);
        let (_, p256dh, auth) = receiver_keys();
        push.subscribe("dev1", "https://web.push.apple.com/1", &p256dh, &auth)
            .expect("subscribe");
        push.subscribe("dev2", "https://web.push.apple.com/2", &p256dh, &auth)
            .expect("subscribe");
        let (sender, seen) = fake_sender(Ok(201));
        push.set_sender(sender);

        let records = push
            .send("ready", "t", "b", None, "stream:w1", Some("dev2"), 90)
            .await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].device_id, "dev2");
        {
            let seen = lock(&seen);
            assert_eq!(seen[0].1.urgency, "normal");
            assert_eq!(seen[0].1.topic, topic_for("stream:w1"));
            assert_eq!(seen[0].1.topic.len(), 32);
            assert_eq!(seen[0].1.ttl, 90);
        }

        let all = push.send("battery", "t", "b", None, "x", None, 60).await;
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].device_id, "dev1", "results in subscription order");
        assert_eq!(lock(&seen).last().map(|s| s.1.urgency), Some("high"));
    }

    #[tokio::test]
    async fn trims_deliveries_to_the_newest_300_past_500() {
        let dir = Scratch::new("push-trim");
        let push = new_push(&dir);
        let (_, p256dh, auth) = receiver_keys();
        push.subscribe("dev1", "https://web.push.apple.com/1", &p256dh, &auth)
            .expect("subscribe");
        push.set_sender(fake_sender(Ok(201)).0);

        let mut ids = Vec::new();
        for _ in 0..501 {
            ids.push(
                push.send("test", "t", "b", None, "x", None, 60)
                    .await
                    .remove(0)
                    .id,
            );
        }
        assert!(push.delivery(&ids[200]).is_none());
        assert!(push.delivery(&ids[201]).is_some());
        assert!(push.delivery(&ids[500]).is_some());
    }

    #[test]
    fn a_device_can_see_its_endpoint_and_the_attention_count_is_optional() {
        let dir = Scratch::new("push-endpoint");
        let push = new_push(&dir);
        assert_eq!(push.device_endpoint("d1"), None);
        push.subscribe("d1", "https://push.example/1", "k", "a")
            .unwrap();
        push.subscribe("d1", "https://push.example/2", "k", "a")
            .unwrap();
        assert_eq!(
            push.device_endpoint("d1").as_deref(),
            Some("https://push.example/2")
        );
        let mut p = PushPayload {
            id: "i".into(),
            kind: "test".into(),
            title: "t".into(),
            body: "b".into(),
            stream_id: None,
            tag: "x".into(),
            ts: 1,
            attention: None,
        };
        assert!(serde_json::to_value(&p).unwrap().get("attention").is_none());
        p.attention = Some(2);
        assert_eq!(serde_json::to_value(&p).unwrap()["attention"], 2);
    }

    #[test]
    fn subscribe_rejects_non_https_and_missing_keys() {
        let dir = Scratch::new("push-subscribe");
        let push = new_push(&dir);
        let (_, p256dh, auth) = receiver_keys();
        for (endpoint, p, a) in [
            (
                "http://web.push.apple.com/x",
                p256dh.as_str(),
                auth.as_str(),
            ),
            ("https://web.push.apple.com/x", "", auth.as_str()),
            ("https://web.push.apple.com/x", p256dh.as_str(), ""),
        ] {
            let err = push
                .subscribe("dev1", endpoint, p, a)
                .expect_err("rejected");
            assert_eq!(err.to_string(), "Invalid subscription");
        }
        assert_eq!(push.device_count(), 0);
    }

    #[test]
    fn subscriptions_file_has_the_ts_shape_and_is_owner_only() {
        let dir = Scratch::new("push-subs-file");
        let push = new_push(&dir);
        let (_, p256dh, auth) = receiver_keys();
        push.subscribe("dev1", "https://web.push.apple.com/s", &p256dh, &auth)
            .expect("subscribe");
        push.subscribe("dev2", "https://web.push.apple.com/t", &p256dh, &auth)
            .expect("subscribe");
        push.subscribe("dev1", "https://web.push.apple.com/u", &p256dh, &auth)
            .expect("subscribe");
        assert_eq!(push.device_count(), 2, "a device keeps one subscription");

        let path = dir.join("subs.json");
        let text = fs::read_to_string(&path).expect("subs file");
        let parsed: Value = serde_json::from_str(&text).expect("json");
        let first = &parsed[0];
        assert_eq!(first["deviceId"], "dev2");
        assert_eq!(first["endpoint"], "https://web.push.apple.com/t");
        assert_eq!(first["keys"]["p256dh"], p256dh.as_str());
        assert_eq!(first["keys"]["auth"], auth.as_str());
        assert!(first["createdAt"].is_u64());
        let reserialized = serde_json::to_string_pretty(&parsed).expect("json");
        assert_eq!(
            text, reserialized,
            "2-space JSON, as JSON.stringify(_, null, 2)"
        );
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let reloaded = new_push(&dir);
        assert!(reloaded.has_device("dev1"));
        assert!(reloaded.has_device("dev2"));
    }

    #[test]
    fn vapid_file_is_created_owner_only_and_reused() {
        let dir = Scratch::new("push-vapid");
        let push = new_push(&dir);
        let path = dir.join("vapid.json");
        let text = fs::read_to_string(&path).expect("vapid file");
        assert!(!text.contains(' ') && !text.contains('\n'), "compact JSON");
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let public = decode_b64(&push.public_key()).expect("public key");
        assert_eq!(public.len(), 65);
        assert_eq!(public[0], 0x04);
        let private = decode_b64(&push.vapid.private_key).expect("private key");
        assert_eq!(private.len(), 32);
        let derived = SecretKey::from_slice(&private)
            .expect("scalar")
            .public_key();
        assert_eq!(derived.to_sec1_point(false).as_bytes(), public.as_slice());

        let again = new_push(&dir);
        assert_eq!(again.public_key(), push.public_key());
        assert_eq!(fs::read_to_string(&path).expect("vapid file"), text);
    }

    #[test]
    fn payload_round_trips_through_an_independent_decrypt_with_the_documented_layout() {
        let (receiver, p256dh, auth) = receiver_keys();
        let sender = random_secret();
        let salt = [0x33u8; 16];
        let plaintext = br#"{"title":"Needs you: secret-name"}"#;
        let body = encrypt_with(&p256dh, &auth, plaintext, &salt, &sender).expect("encrypt");

        // Header: salt(16) || rs(4, BE) || idlen(1) || keyid (the sender's public key).
        assert_eq!(&body[..16], &salt);
        assert_eq!(
            u32::from_be_bytes(body[16..20].try_into().expect("rs")),
            4096
        );
        assert_eq!(body[20], 65);
        let sender_public = sender.public_key().to_sec1_point(false);
        assert_eq!(&body[21..86], sender_public.as_bytes());
        assert_eq!(body.len(), 86 + plaintext.len() + 1 + 16);
        assert!(!body.windows(plaintext.len()).any(|w| w == plaintext));

        let decrypted = decrypt_receiver(&receiver, &p256dh, &auth, &body);
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn multi_record_payloads_use_counter_nonces_and_the_last_record_flag() {
        let (receiver, p256dh, auth) = receiver_keys();
        let plaintext: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let body = encrypt_with(&p256dh, &auth, &plaintext, &[1u8; 16], &random_secret())
            .expect("encrypt");
        assert_eq!(body.len(), 86 + plaintext.len() + 3 * 17);
        assert_eq!(
            decrypt_receiver(&receiver, &p256dh, &auth, &body),
            plaintext
        );
    }

    /// Independent receiver-side decryption (RFC 8291 section 3.4) for the tests.
    fn decrypt_receiver(receiver: &SecretKey, p256dh: &str, auth: &str, body: &[u8]) -> Vec<u8> {
        let salt = &body[..16];
        let rs = u32::from_be_bytes(body[16..20].try_into().expect("rs")) as usize;
        let idlen = body[20] as usize;
        let keyid = &body[21..21 + idlen];
        let sender = PublicKey::from_sec1_bytes(keyid).expect("sender key");
        let shared = diffie_hellman(receiver.to_nonzero_scalar(), sender.as_affine());
        let ua_public = decode_b64(p256dh).expect("ua public");
        let mut info = b"WebPush: info\0".to_vec();
        info.extend_from_slice(&ua_public);
        info.extend_from_slice(keyid);
        let auth_secret = decode_b64(auth).expect("auth");
        let mut ikm = [0u8; 32];
        Hkdf::<Sha256>::new(Some(&auth_secret), shared.raw_secret_bytes())
            .expand(&info, &mut ikm)
            .expect("ikm");
        let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
        let mut cek = [0u8; 16];
        prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
            .expect("cek");
        let mut nonce = [0u8; 12];
        prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
            .expect("nonce");
        let cipher = Aes128Gcm::new_from_slice(&cek).expect("key");

        let mut out = Vec::new();
        let mut offset = 21 + idlen;
        let mut counter: u64 = 0;
        loop {
            let end = (offset + rs).min(body.len());
            let mut record_nonce = nonce;
            for (i, byte) in counter.to_be_bytes()[2..].iter().enumerate() {
                record_nonce[6 + i] ^= byte;
            }
            let mut plain = cipher
                .decrypt((&record_nonce).into(), &body[offset..end])
                .expect("decrypt");
            let delimiter = plain.pop().expect("delimiter");
            out.extend_from_slice(&plain);
            if delimiter == 2 {
                assert_eq!(end, body.len(), "only the final record carries 0x02");
                return out;
            }
            assert_eq!(delimiter, 1);
            offset = end;
            counter += 1;
        }
    }

    #[test]
    fn vapid_authorization_is_a_verifiable_es256_jwt_with_the_web_push_claims() {
        let private_bytes = [0x55u8; 32];
        let secret = SecretKey::from_slice(&private_bytes).expect("scalar");
        let public = b64(secret.public_key().to_sec1_point(false).as_bytes());
        let private = b64(&private_bytes);
        let now = 1_800_000_000u64;
        let auth = vapid_authorization(
            "https://web.push.apple.com/abc?x=1",
            "https://briardr.test",
            &public,
            &private,
            now,
        )
        .expect("authorization");

        let rest = auth.strip_prefix("vapid t=").expect("scheme");
        let (jwt, key) = rest.split_once(", k=").expect("k parameter");
        assert_eq!(key, public);
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            String::from_utf8(BASE64_URL_SAFE_NO_PAD.decode(parts[0]).expect("header"))
                .expect("utf8"),
            r#"{"typ":"JWT","alg":"ES256"}"#
        );
        let claims = String::from_utf8(BASE64_URL_SAFE_NO_PAD.decode(parts[1]).expect("claims"))
            .expect("utf8");
        assert_eq!(
            claims,
            format!(
                r#"{{"aud":"https://web.push.apple.com","exp":{},"sub":"https://briardr.test"}}"#,
                now + 43_200
            )
        );
        let sig_bytes = BASE64_URL_SAFE_NO_PAD.decode(parts[2]).expect("signature");
        assert_eq!(sig_bytes.len(), 64, "raw r || s");
        let signature = Signature::from_slice(&sig_bytes).expect("signature");
        let verifying = VerifyingKey::from(&secret.public_key());
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        verifying
            .verify(signing_input.as_bytes(), &signature)
            .expect("signature verifies with the VAPID public key");

        assert!(vapid_authorization(
            "https://web.push.apple.com/x",
            "ftp://nope",
            &public,
            &private,
            now
        )
        .is_none());
        assert!(vapid_authorization(
            "https://web.push.apple.com/x",
            "https://briardr.test",
            &private,
            &private,
            now
        )
        .is_none());
    }

    /// Serves one HTTP request on localhost, returns the raw request, answers with `status`.
    async fn serve_once(status: u16) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.expect("read");
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + length || n == 0 {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let response =
                format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            socket.write_all(response.as_bytes()).await.expect("write");
            String::from_utf8_lossy(&raw).into_owned()
        });
        (format!("http://{addr}/push/abc"), handle)
    }

    #[tokio::test]
    async fn default_sender_posts_the_encrypted_payload_with_web_push_headers() {
        let (_, p256dh, auth) = receiver_keys();
        let (endpoint, server) = serve_once(201).await;
        let sub = PushSub {
            device_id: "dev1".to_string(),
            endpoint,
            keys: PushKeys { p256dh, auth },
            created_at: 0,
        };
        let client = http::client(SEND_TIMEOUT).expect("client");
        let public = b64(&[4u8; 65]);
        let private = b64(&[0x55u8; 32]);
        let opts = SendOpts {
            ttl: 3600,
            urgency: "high",
            topic: topic_for("stream:w1"),
            subject: "https://briardr.test".to_string(),
            vapid_public: public.clone(),
            vapid_private: private,
        };
        let result = web_push(&client, &sub, "{\"title\":\"hi\"}", &opts).await;
        assert_eq!(result, Ok(201));
        let raw = server.await.expect("server");
        let head = raw
            .split("\r\n\r\n")
            .next()
            .expect("head")
            .to_ascii_lowercase();
        assert!(head.starts_with("post /push/abc http/1.1"));
        assert!(head.contains("\r\nttl: 3600\r\n"));
        assert!(head.contains("\r\nurgency: high\r\n"));
        assert!(head.contains(&format!(
            "\r\ntopic: {}\r\n",
            topic_for("stream:w1").to_ascii_lowercase()
        )));
        assert!(head.contains("\r\ncontent-encoding: aes128gcm\r\n"));
        assert!(head.contains("\r\ncontent-type: application/octet-stream\r\n"));
        assert!(head.contains("\r\ncontent-length: "));
        assert!(head.contains("\r\nauthorization: vapid t="));
        assert!(!raw.contains("hi\"}"), "the body is ciphertext");
    }

    #[tokio::test]
    async fn default_sender_maps_refusals_and_transport_failures() {
        let (_, p256dh, auth) = receiver_keys();
        let (endpoint, server) = serve_once(410).await;
        let sub = PushSub {
            device_id: "dev1".to_string(),
            endpoint,
            keys: PushKeys {
                p256dh: p256dh.clone(),
                auth: auth.clone(),
            },
            created_at: 0,
        };
        let client = http::client(SEND_TIMEOUT).expect("client");
        let opts = SendOpts {
            ttl: 60,
            urgency: "normal",
            topic: topic_for("x"),
            subject: "https://briardr.test".to_string(),
            vapid_public: b64(&[4u8; 65]),
            vapid_private: b64(&[0x55u8; 32]),
        };
        assert_eq!(web_push(&client, &sub, "{}", &opts).await, Err(Some(410)));
        server.await.expect("server");

        // A port nobody listens on: connection refused.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = closed.local_addr().expect("addr").port();
        drop(closed);
        let sub = PushSub {
            endpoint: format!("http://127.0.0.1:{port}/x"),
            ..sub
        };
        assert_eq!(web_push(&client, &sub, "{}", &opts).await, Err(None));

        let bad_keys = PushSub {
            keys: PushKeys {
                p256dh: "AAAA".to_string(),
                auth,
            },
            ..sub
        };
        assert_eq!(web_push(&client, &bad_keys, "{}", &opts).await, Err(None));
    }
}
