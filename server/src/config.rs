// Configuration: data-dir paths, defaults, loading and validation. Owner-only writes (0600) for anything secret.
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("config I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("config JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BriardrConfig {
    pub herdr_path: String,
    pub herdr_session: String,
    pub port: u16,
    pub preview_port_start: u16,
    pub preview_port_count: u16,
    pub listen: Vec<String>,
    pub allowed_networks: Vec<String>,
    pub hostnames: Vec<String>,
    pub tls: TlsConfig,
    #[serde(serialize_with = "ser_opt_str")]
    pub public_origin: Option<String>,
    pub trusted_proxies: Vec<String>,
    #[serde(serialize_with = "ser_opt_str")]
    pub public_preview_origin: Option<String>,
    pub herdr_timeout_ms: u64,
    pub notifications: NotificationsConfig,
    pub usage: UsageConfig,
    pub power: PowerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TlsConfig {
    /// "tailscale" | "local-ca" | "files" | "proxy"
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsConfig {
    pub include_names: bool,
    pub needs_you: bool,
    pub finished: bool,
    pub ready: bool,
    pub low_battery: bool,
    pub debounce_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageConfig {
    pub claude_keychain_opt_in: bool,
    pub codex_enabled: bool,
    #[serde(serialize_with = "ser_opt_str")]
    pub claude_config_dir: Option<String>,
    #[serde(serialize_with = "ser_opt_str")]
    pub codex_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerConfig {
    pub keep_awake: bool,
}

/// Unset strings are written as "" in the config file.
fn ser_opt_str<S: Serializer>(value: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(value.as_deref().unwrap_or(""))
}

/// Every on-disk location, derived from the data directory.
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Paths { root: root.into() }
    }
    pub fn config(&self) -> PathBuf {
        self.root.join("config.json")
    }
    pub fn secrets(&self) -> PathBuf {
        self.root.join("secrets")
    }
    pub fn devices(&self) -> PathBuf {
        self.secrets().join("devices.json")
    }
    pub fn pairing(&self) -> PathBuf {
        self.secrets().join("pairing.json")
    }
    pub fn vapid(&self) -> PathBuf {
        self.secrets().join("vapid.json")
    }
    pub fn tls_dir(&self) -> PathBuf {
        self.secrets().join("tls")
    }
    pub fn push_subs(&self) -> PathBuf {
        self.secrets().join("push-subscriptions.json")
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn usage(&self) -> PathBuf {
        self.state().join("usage.json")
    }
    pub fn statusline_tap(&self) -> PathBuf {
        self.state().join("statusline-rate-limits.json")
    }
    pub fn monitor_state(&self) -> PathBuf {
        self.state().join("monitor.json")
    }
    pub fn power_state(&self) -> PathBuf {
        self.state().join("power.json")
    }
    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }
    /// Installed plugins: one folder (or a link to one) per plugin.
    pub fn plugins(&self) -> PathBuf {
        self.root.join("plugins")
    }
    /// Which plugins are on and which backend commands were approved (plugins::PluginState).
    pub fn plugin_state(&self) -> PathBuf {
        self.state().join("plugins.json")
    }
    /// Data each plugin's backend keeps, one folder per plugin.
    pub fn plugin_data(&self) -> PathBuf {
        self.root.join("plugin-data")
    }

    /// Create the directory tree with owner-only permissions where it matters.
    pub fn ensure(&self) -> Result<(), ConfigError> {
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true).mode(0o700);
        for d in [
            self.root.clone(),
            self.secrets(),
            self.tls_dir(),
            self.state(),
            self.logs(),
            self.bin(),
        ] {
            dirs.create(d)?;
        }
        fs::set_permissions(self.secrets(), fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
}

pub fn default_data_dir(
    platform: &str,
    env: &std::collections::HashMap<String, String>,
    home: &Path,
) -> PathBuf {
    if let Some(dir) = env.get("BRIARDR_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if platform == "darwin" {
        return home
            .join("Library")
            .join("Application Support")
            .join("Briardr");
    }
    let xdg = match env.get("XDG_DATA_HOME") {
        Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
        _ => home.join(".local").join("share"),
    };
    xdg.join("briardr")
}

/// The home directory from `HOME`, falling back to the process home.
fn home_dir(env: &std::collections::HashMap<String, String>) -> PathBuf {
    match env.get("HOME").filter(|h| !h.is_empty()) {
        Some(h) => PathBuf::from(h),
        None => std::env::home_dir().unwrap_or_default(),
    }
}

/// First executable `cmd` on PATH, else the first existing fallback, else the first fallback.
fn which(
    cmd: &str,
    env: &std::collections::HashMap<String, String>,
    fallbacks: &[PathBuf],
) -> String {
    let path = env.get("PATH").map(String::as_str).unwrap_or("");
    for dir in path.split(':') {
        let candidate = Path::new(dir).join(cmd);
        if is_executable(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    for fallback in fallbacks {
        if fallback.exists() {
            return fallback.to_string_lossy().into_owned();
        }
    }
    fallbacks
        .first()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| cmd.to_string())
}

/// Approximates access(X_OK): any execute bit set on an existing path.
fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| (m.permissions().mode() & 0o111) != 0)
}

/// Defaults; `env` supplies PATH for locating herdr and codex, `home` the home directory.
/// Where Briardr looks for Herdr when it isn't on the PATH (`briardr setup` installs it in the last one).
pub fn herdr_fallbacks(home: &Path) -> [PathBuf; 3] {
    [
        PathBuf::from("/opt/homebrew/bin/herdr"),
        PathBuf::from("/usr/local/bin/herdr"),
        home.join(".local").join("bin").join("herdr"),
    ]
}

pub fn default_config(
    env: &std::collections::HashMap<String, String>,
    home: &Path,
) -> BriardrConfig {
    let herdr_fallbacks = herdr_fallbacks(home);
    let codex_fallbacks = [
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
        home.join(".local").join("bin").join("codex"),
    ];
    BriardrConfig {
        herdr_path: which("herdr", env, &herdr_fallbacks),
        herdr_session: String::new(),
        port: 7443,
        preview_port_start: 7444,
        preview_port_count: 16,
        listen: vec!["tailscale".to_string()],
        allowed_networks: Vec::new(),
        hostnames: Vec::new(),
        tls: TlsConfig {
            mode: "local-ca".to_string(),
            cert_file: None,
            key_file: None,
        },
        public_origin: None,
        trusted_proxies: vec!["127.0.0.1".to_string(), "::1".to_string()],
        public_preview_origin: None,
        herdr_timeout_ms: 8000,
        notifications: NotificationsConfig {
            include_names: true,
            needs_you: true,
            finished: true,
            ready: false,
            low_battery: true,
            debounce_ms: 3000,
        },
        usage: UsageConfig {
            claude_keychain_opt_in: false,
            codex_enabled: false,
            claude_config_dir: Some(home.join(".claude").to_string_lossy().into_owned()),
            codex_path: Some(which("codex", env, &codex_fallbacks)),
        },
        power: PowerConfig { keep_awake: false },
    }
}

/// Objects merge recursively; anything else in `over` replaces `base`.
fn deep_merge(base: Value, over: Value) -> Value {
    match (base, over) {
        (Value::Object(mut out), Value::Object(over)) => {
            for (key, value) in over {
                let merged = match (out.remove(&key), value) {
                    (Some(b @ Value::Object(_)), v @ Value::Object(_)) => deep_merge(b, v),
                    (_, v) => v,
                };
                out.insert(key, merged);
            }
            Value::Object(out)
        }
        (_, over) => over,
    }
}

fn parse_port_env(value: &str, name: &str) -> Result<u16, ConfigError> {
    value
        .trim()
        .parse::<u16>()
        .map_err(|_| ConfigError::Invalid(format!("{name} must be a port number from 0 to 65535.")))
}

/// Load config.json, applying `BRIARDR_*` env overrides from `env`; missing file means defaults.
pub fn load_config(
    paths: &Paths,
    env: &std::collections::HashMap<String, String>,
) -> Result<BriardrConfig, ConfigError> {
    let defaults = serde_json::to_value(default_config(env, &home_dir(env)))?;
    let raw = fs::read(paths.config())
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mut cfg: BriardrConfig = serde_json::from_value(deep_merge(defaults, raw))?;
    if let Some(session) = env.get("BRIARDR_HERDR_SESSION") {
        cfg.herdr_session = session.clone();
    }
    if let Some(port) = env.get("BRIARDR_PORT").filter(|v| !v.is_empty()) {
        cfg.port = parse_port_env(port, "BRIARDR_PORT")?;
    }
    if let Some(port) = env.get("BRIARDR_PREVIEW_PORT").filter(|v| !v.is_empty()) {
        cfg.preview_port_start = parse_port_env(port, "BRIARDR_PREVIEW_PORT")?;
    }
    if let Some(listen) = env.get("BRIARDR_LISTEN") {
        cfg.listen = if listen.is_empty() {
            Vec::new()
        } else {
            listen.split(',').map(str::to_string).collect()
        };
    }
    Ok(cfg)
}

/// JSON string literal, as `JSON.stringify` writes it in error messages.
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|s| !s.is_empty())
}

fn is_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `net.isIP` or an IPv4 CIDR without a range check.
fn is_ip_or_ipv4_cidr(p: &str) -> bool {
    if p.parse::<IpAddr>().is_ok() {
        return true;
    }
    let Some((addr, len)) = p.split_once('/') else {
        return false;
    };
    let octets: Vec<&str> = addr.split('.').collect();
    octets.len() == 4 && octets.iter().all(|o| is_ascii_digits(o)) && is_ascii_digits(len)
}

/// Check the settings that decide how Briardr is reached. Throws a ConfigError Briardr reports at
/// startup rather than serving with a half-working setup.
pub fn validate_config(cfg: &mut BriardrConfig) -> Result<(), ConfigError> {
    let mode = cfg.tls.mode.as_str();
    if !matches!(mode, "tailscale" | "local-ca" | "files" | "proxy") {
        return Err(ConfigError::Invalid(format!(
            "tls.mode must be tailscale, local-ca, files or proxy (got {}).",
            json_string(mode)
        )));
    }
    if mode == "files"
        && (non_empty(&cfg.tls.cert_file).is_none() || non_empty(&cfg.tls.key_file).is_none())
    {
        return Err(ConfigError::Invalid(
            "tls.mode \"files\" needs tls.certFile and tls.keyFile.".to_string(),
        ));
    }
    let public_origin = match non_empty(&cfg.public_origin) {
        Some(origin) => Some(parse_public_origin(origin, "publicOrigin")?),
        None => None,
    };
    if let Some(origin) = &public_origin {
        cfg.public_origin = Some(origin.clone());
    }
    if mode != "proxy" {
        return Ok(());
    }
    let Some(public_origin) = public_origin else {
        return Err(ConfigError::Invalid(
            "Proxy mode needs publicOrigin, the URL devices open (for example https://briardr.example.com)."
                .to_string(),
        ));
    };
    if cfg.trusted_proxies.is_empty() {
        return Err(ConfigError::Invalid(
            "Proxy mode needs trustedProxies (normally [\"127.0.0.1\", \"::1\"]).".to_string(),
        ));
    }
    for proxy in &cfg.trusted_proxies {
        if !is_ip_or_ipv4_cidr(proxy) {
            return Err(ConfigError::Invalid(format!(
                "trustedProxies entries must be IP addresses or IPv4 CIDRs (got {}).",
                json_string(proxy)
            )));
        }
    }
    if let Some(template) = non_empty(&cfg.public_preview_origin) {
        if !template.contains("{n}") {
            return Err(ConfigError::Invalid(
                "publicPreviewOrigin needs \"{n}\" for the slot number, for example https://p{n}.briardr.example.com."
                    .to_string(),
            ));
        }
        let origins = preview_origins(cfg)?;
        if origins.contains(&public_origin) {
            return Err(ConfigError::Invalid(
                "Previews need their own origins, different from publicOrigin.".to_string(),
            ));
        }
        let unique: HashSet<&str> = origins.iter().map(String::as_str).collect();
        if unique.len() != origins.len() {
            return Err(ConfigError::Invalid(
                "Each preview slot needs a different origin.".to_string(),
            ));
        }
    }
    Ok(())
}

/// A canonical public origin: https, a host name, optional port, nothing else. Returns it normalized
/// (lowercase, no trailing slash) or throws a ConfigError saying what's wrong.
pub fn parse_public_origin(value: &str, what: &str) -> Result<String, ConfigError> {
    let url = Url::parse(value).map_err(|_| {
        ConfigError::Invalid(format!(
            "{what} must be a URL like https://briardr.example.com (got {}).",
            json_string(value)
        ))
    })?;
    if url.scheme() != "https" {
        return Err(ConfigError::Invalid(format!("{what} must use https.")));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::Invalid(format!(
            "{what} must not contain a user name or password."
        )));
    }
    if value.contains(['?', '#']) || url.path() != "/" || has_path_after_host(value) {
        return Err(ConfigError::Invalid(format!(
            "{what} must be an origin only, without a path, query or fragment."
        )));
    }
    let host = url.host_str().unwrap_or_default();
    if host.is_empty() || host.ends_with('.') {
        return Err(ConfigError::Invalid(format!("{what} needs a host name.")));
    }
    Ok(match url.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    })
}

/// The raw-text check `^https:\/\/[^/]+\/.` (case-insensitive) applied to the input.
/// It catches inputs such as `https://host/./`, whose parsed path is "/" but which this check rejects.
fn has_path_after_host(raw: &str) -> bool {
    let Some(prefix) = raw.get(..8) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case("https://") {
        return false;
    }
    let after = &raw[8..];
    match after.find('/') {
        Some(i) if i > 0 => after[i + 1..]
            .chars()
            .next()
            .is_some_and(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')),
        _ => false,
    }
}

/// The origin of each preview slot in proxy mode (slot n is served on previewPortStart + n - 1).
/// Returns an empty list when the config is invalid, so nothing is allowed; `validate_config`
/// reports the reason.
/// The origin of each preview slot; an invalid slot is an error.
pub fn preview_origins(cfg: &BriardrConfig) -> Result<Vec<String>, ConfigError> {
    let Some(template) = non_empty(&cfg.public_preview_origin) else {
        return Ok(Vec::new());
    };
    (1..=cfg.preview_port_count)
        .map(|n| {
            parse_public_origin(
                &template.replace("{n}", &n.to_string()),
                "publicPreviewOrigin",
            )
        })
        .collect()
}

pub fn save_config(paths: &Paths, cfg: &BriardrConfig) -> Result<(), ConfigError> {
    let mut data = serde_json::to_string_pretty(cfg)?;
    data.push('\n');
    write_file_atomic(&paths.config(), data.as_bytes(), 0o600)
}

fn write_synced(file: &mut fs::File, data: &[u8], mode: u32) -> io::Result<()> {
    file.write_all(data)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.sync_all()
}

/// Write via temp file + rename; `mode` is the unix permission (default 0o600).
pub fn write_file_atomic(file: &Path, data: &[u8], mode: u32) -> Result<(), ConfigError> {
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    fs::DirBuilder::new().recursive(true).create(dir)?;
    let name = file.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "config path has no file name")
    })?;
    let nonce = hex::encode(rand::random::<[u8; 16]>());
    let tmp = dir.join(format!(
        ".{}.{}.{nonce}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(mode);
    let mut handle = options.open(&tmp)?;
    let result = write_synced(&mut handle, data, mode).and_then(|()| fs::rename(&tmp, file));
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Read a JSON file; missing or unparseable yields `fallback`.
pub fn read_json<T: serde::de::DeserializeOwned>(file: &Path, fallback: T) -> T {
    fs::read(file)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use std::collections::HashMap;

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/config")
            .join(format!("{name}.json"));
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {name}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad fixture {name}: {e}"))
    }

    fn to_json<T: Serialize>(value: &T) -> Value {
        serde_json::to_value(value).unwrap_or(Value::Null)
    }

    fn invalid_message(result: Result<(), ConfigError>) -> String {
        match result {
            Err(ConfigError::Invalid(msg)) => msg,
            other => panic!("expected an Invalid error, got {other:?}"),
        }
    }

    fn env_with(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn default_config_matches_fixture() {
        let root = Scratch::new("config-default");
        let home = root.join("home");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap_or_default();
        let env = env_with(&[("PATH", bin.to_str().unwrap_or(""))]);
        let mut actual = to_json(&default_config(&env, &home));
        let mut expected = fixture("defaultConfig.default")["output"].clone();
        for value in [&mut actual, &mut expected] {
            if let Some(obj) = value.as_object_mut() {
                obj.remove("herdrPath");
                if let Some(usage) = obj.get_mut("usage").and_then(Value::as_object_mut) {
                    usage.remove("claudeConfigDir");
                    usage.remove("codexPath");
                }
            }
        }
        assert_eq!(actual, expected);
        let cfg = default_config(&env, &home);
        assert_eq!(
            cfg.usage.claude_config_dir.as_deref(),
            Some(home.join(".claude").to_str().unwrap_or(""))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn which_prefers_executable_on_path() {
        let root = Scratch::new("config-which");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap_or_default();
        let herdr = bin.join("herdr");
        fs::write(&herdr, b"#!/bin/sh\n").unwrap_or_default();
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o644)).unwrap_or_default();
        let env = env_with(&[("PATH", bin.to_str().unwrap_or(""))]);
        let home = root.join("home");
        assert_ne!(
            default_config(&env, &home).herdr_path,
            herdr.to_string_lossy()
        );
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap_or_default();
        assert_eq!(
            default_config(&env, &home).herdr_path,
            herdr.to_string_lossy()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn data_dir_rules() {
        let home = Path::new("/home/kel");
        let none = HashMap::new();
        assert_eq!(
            default_data_dir("darwin", &none, home),
            PathBuf::from("/home/kel/Library/Application Support/Briardr")
        );
        assert_eq!(
            default_data_dir("linux", &none, home),
            PathBuf::from("/home/kel/.local/share/briardr")
        );
        let relative = env_with(&[("XDG_DATA_HOME", "rel")]);
        assert_eq!(
            default_data_dir("linux", &relative, home),
            PathBuf::from("/home/kel/.local/share/briardr")
        );
        let xdg = env_with(&[("XDG_DATA_HOME", "/srv/data")]);
        assert_eq!(
            default_data_dir("linux", &xdg, home),
            PathBuf::from("/srv/data/briardr")
        );
        let explicit = env_with(&[
            ("BRIARDR_DATA_DIR", "/var/briardr"),
            ("XDG_DATA_HOME", "/srv"),
        ]);
        assert_eq!(
            default_data_dir("darwin", &explicit, home),
            PathBuf::from("/var/briardr")
        );
    }

    #[test]
    fn parse_public_origin_fixtures() {
        for name in [
            "parsePublicOrigin.https-host",
            "parsePublicOrigin.trailing-slash-stripped",
        ] {
            let case = fixture(name);
            let input = case["input"][0].as_str().unwrap_or_default();
            assert_eq!(
                parse_public_origin(input, "publicOrigin").ok().as_deref(),
                case["output"].as_str(),
                "{name}"
            );
        }
        let case = fixture("parsePublicOrigin.ftp-rejected");
        let input = case["input"][0].as_str().unwrap_or_default();
        let err = parse_public_origin(input, "publicOrigin")
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            err.as_deref(),
            case["error"]
                .as_str()
                .map(|m| format!("invalid config: {m}"))
                .as_deref()
        );
    }

    #[test]
    fn parse_public_origin_rules() {
        let invalid = |value: &str| match parse_public_origin(value, "publicOrigin") {
            Err(ConfigError::Invalid(msg)) => msg,
            other => panic!("{value}: expected Invalid, got {other:?}"),
        };
        assert_eq!(
            invalid("briardr.lan"),
            "publicOrigin must be a URL like https://briardr.example.com (got \"briardr.lan\")."
        );
        assert_eq!(
            invalid("https://u:p@briardr.lan"),
            "publicOrigin must not contain a user name or password."
        );
        assert_eq!(
            invalid("https://briardr.lan/a"),
            "publicOrigin must be an origin only, without a path, query or fragment."
        );
        assert_eq!(
            invalid("https://briardr.lan?x=1"),
            "publicOrigin must be an origin only, without a path, query or fragment."
        );
        assert_eq!(
            invalid("https://briardr.lan."),
            "publicOrigin needs a host name."
        );
        assert_eq!(
            parse_public_origin("HTTPS://Briardr.LAN:443/", "publicOrigin")
                .ok()
                .as_deref(),
            Some("https://briardr.lan")
        );
        assert_eq!(
            parse_public_origin("https://briardr.lan:8443", "publicOrigin")
                .ok()
                .as_deref(),
            Some("https://briardr.lan:8443")
        );
        assert_eq!(
            parse_public_origin("https://0x7f.1", "publicOrigin")
                .ok()
                .as_deref(),
            Some("https://127.0.0.1")
        );
        assert_eq!(
            parse_public_origin("https://[::1]:9443", "publicOrigin")
                .ok()
                .as_deref(),
            Some("https://[::1]:9443")
        );
    }

    #[test]
    fn preview_origins_fixtures() {
        for name in ["previewOrigins.disabled", "previewOrigins.range"] {
            let case = fixture(name);
            let mut cfg = default_config(&HashMap::new(), Path::new("/nonexistent-home"));
            cfg.public_preview_origin = case["input"][0]["publicPreviewOrigin"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            cfg.preview_port_count =
                u16::try_from(case["input"][0]["previewPortCount"].as_u64().unwrap_or(0))
                    .unwrap_or(0);
            let expected: Vec<String> = case["output"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                preview_origins(&cfg).unwrap_or_else(|e| panic!("{name}: {e}")),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn validate_config_fixtures() {
        let mut bad: BriardrConfig =
            serde_json::from_value(fixture("validateConfig.bad-tls-mode")["input"][0].clone())
                .unwrap_or_else(|e| panic!("bad fixture input: {e}"));
        assert_eq!(
            invalid_message(validate_config(&mut bad)),
            fixture("validateConfig.bad-tls-mode")["error"]
                .as_str()
                .unwrap_or_default()
        );
        let mut good: BriardrConfig =
            serde_json::from_value(fixture("validateConfig.default-valid")["input"][0].clone())
                .unwrap_or_else(|e| panic!("bad fixture input: {e}"));
        assert!(validate_config(&mut good).is_ok());
    }

    #[test]
    fn validate_config_proxy_rules() {
        let mut cfg = default_config(&HashMap::new(), Path::new("/nonexistent-home"));
        cfg.tls.mode = "proxy".to_string();
        assert_eq!(
            invalid_message(validate_config(&mut cfg)),
            "Proxy mode needs publicOrigin, the URL devices open (for example https://briardr.example.com)."
        );
        cfg.public_origin = Some("https://briardr.example.com/".to_string());
        assert!(validate_config(&mut cfg).is_ok());
        cfg.trusted_proxies = vec!["10.0.0.0/8".to_string(), "nope".to_string()];
        assert_eq!(
            invalid_message(validate_config(&mut cfg)),
            "trustedProxies entries must be IP addresses or IPv4 CIDRs (got \"nope\")."
        );
        cfg.trusted_proxies = vec!["127.0.0.1".to_string()];
        cfg.public_preview_origin = Some("https://preview.example.com".to_string());
        assert_eq!(
            invalid_message(validate_config(&mut cfg)),
            "publicPreviewOrigin needs \"{n}\" for the slot number, for example https://p{n}.briardr.example.com."
        );
        cfg.public_preview_origin = Some("https://p{n}.briardr.example.com".to_string());
        cfg.public_origin = Some("https://p1.briardr.example.com".to_string());
        assert_eq!(
            invalid_message(validate_config(&mut cfg)),
            "Previews need their own origins, different from publicOrigin."
        );
        cfg.public_origin = Some("https://briardr.example.com".to_string());
        cfg.public_preview_origin = Some("https://p{n}x.example.com".to_string());
        assert!(validate_config(&mut cfg).is_ok());
        cfg.public_preview_origin = Some("https://{n}.example.com".to_string());
        cfg.preview_port_count = 2;
        assert!(validate_config(&mut cfg).is_ok());
        cfg.public_preview_origin = Some("https://preview.example.com/{n}".to_string());
        assert!(validate_config(&mut cfg).is_err());
    }

    #[test]
    fn load_config_applies_env_overrides() {
        let root = Scratch::new("config-env");
        let paths = Paths::new(root.path());
        let env = env_with(&[
            ("HOME", root.to_str().unwrap_or("")),
            ("BRIARDR_PORT", " 8443 "),
            ("BRIARDR_PREVIEW_PORT", "9000"),
            ("BRIARDR_HERDR_SESSION", ""),
            ("BRIARDR_LISTEN", "a,b"),
        ]);
        let cfg = load_config(&paths, &env).unwrap_or_else(|e| panic!("load failed: {e}"));
        assert_eq!(cfg.port, 8443);
        assert_eq!(cfg.preview_port_start, 9000);
        assert_eq!(cfg.herdr_session, "");
        assert_eq!(cfg.listen, vec!["a".to_string(), "b".to_string()]);
        let empty_listen = env_with(&[("BRIARDR_LISTEN", "")]);
        let cfg = load_config(&paths, &empty_listen).unwrap_or_else(|e| panic!("load failed: {e}"));
        assert!(cfg.listen.is_empty());
        let bad = env_with(&[("BRIARDR_PORT", "nope")]);
        assert!(load_config(&paths, &bad).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_config_merges_file_over_defaults() {
        let root = Scratch::new("config-merge");
        let paths = Paths::new(root.path());
        fs::write(
            paths.config(),
            r#"{"port": 9001, "usage": {"codexEnabled": true}, "tls": {"mode": "files", "certFile": "/c.pem"}}"#,
        )
        .unwrap_or_default();
        let env = env_with(&[("HOME", root.to_str().unwrap_or(""))]);
        let cfg = load_config(&paths, &env).unwrap_or_else(|e| panic!("load failed: {e}"));
        assert_eq!(cfg.port, 9001);
        assert!(cfg.usage.codex_enabled);
        assert_eq!(
            cfg.usage.claude_config_dir,
            Some(root.join(".claude").to_string_lossy().into_owned())
        );
        assert_eq!(cfg.tls.cert_file.as_deref(), Some("/c.pem"));
        assert_eq!(cfg.tls.key_file, None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_config_writes_owner_only_json() {
        let root = Scratch::new("config-save");
        let paths = Paths::new(root.path());
        let env = env_with(&[("HOME", root.to_str().unwrap_or(""))]);
        let cfg = default_config(&env, &root);
        save_config(&paths, &cfg).unwrap_or_else(|e| panic!("save failed: {e}"));
        let text = fs::read_to_string(paths.config()).unwrap_or_default();
        assert!(text.ends_with("}\n"));
        assert!(text.contains("\"publicOrigin\": \"\""));
        let mode = fs::metadata(paths.config())
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn atomic_writes_to_the_same_file_do_not_collide_or_tear() {
        let root = Scratch::new("atomic-concurrent");
        let target = root.join("state.json");
        let barrier = std::sync::Barrier::new(16);
        std::thread::scope(|scope| {
            for i in 0..16u8 {
                let (target, barrier) = (&target, &barrier);
                scope.spawn(move || {
                    let data = vec![b'A' + i; 4096 + usize::from(i)];
                    barrier.wait();
                    for _ in 0..8 {
                        write_file_atomic(target, &data, 0o600).unwrap();
                    }
                });
            }
        });
        let data = fs::read(target).unwrap();
        assert_eq!(data.len(), 4096 + usize::from(data[0] - b'A'));
        assert!(data.iter().all(|b| *b == data[0]));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn write_file_atomic_cleans_up_on_failure() {
        let root = Scratch::new("config-atomic");
        let target = root.join("target");
        fs::create_dir_all(target.join("occupied")).unwrap_or_default();
        assert!(write_file_atomic(&target, b"data", 0o600).is_err());
        let leftovers = fs::read_dir(&root)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_creates_tree_owner_only() {
        let root = Scratch::new("config-ensure");
        let paths = Paths::new(root.join("data"));
        paths
            .ensure()
            .unwrap_or_else(|e| panic!("ensure failed: {e}"));
        for dir in [
            paths.secrets(),
            paths.tls_dir(),
            paths.state(),
            paths.logs(),
            paths.bin(),
        ] {
            assert!(dir.is_dir(), "missing directory");
        }
        let mode = fs::metadata(paths.secrets())
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o700);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_json_falls_back() {
        let root = Scratch::new("config-read-json");
        let file = root.join("value.json");
        assert_eq!(read_json(&file, 7u32), 7);
        fs::write(&file, b"not json").unwrap_or_default();
        assert_eq!(read_json(&file, 7u32), 7);
        fs::write(&file, b"42").unwrap_or_default();
        assert_eq!(read_json(&file, 7u32), 42);
        let _ = fs::remove_dir_all(&root);
    }
}
