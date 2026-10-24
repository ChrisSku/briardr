// TLS certificates: the local CA and its leaf, Tailscale certificates, the CA profile for devices.
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::net::IpAddr;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("TLS: {0}")]
    Msg(String),
    #[error("TLS I/O: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct TlsMaterial {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub ca_cert_pem: Option<Vec<u8>>,
    /// "tailscale" | "local-ca" | "files" | "proxy"
    pub mode: String,
}

/// Renew the leaf certificate when fewer than 30 days remain.
const LEAF_RENEW_WITHIN_SECS: u64 = 30 * 86400;
/// Renew the Tailscale certificate when fewer than 14 days remain.
const TAILSCALE_RENEW_WITHIN_SECS: u64 = 14 * 86400;
const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub fn ensure_local_ca(
    dir: &Path,
    hostnames: &[String],
    ips: &[IpAddr],
) -> Result<TlsMaterial, TlsError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let ca_key = dir.join("ca.key");
    let ca_crt = dir.join("ca.crt");
    if !ca_key.exists() || !ca_crt.exists() {
        run_cmd(
            "openssl",
            &[
                OsStr::new("ecparam"),
                OsStr::new("-name"),
                OsStr::new("prime256v1"),
                OsStr::new("-genkey"),
                OsStr::new("-noout"),
                OsStr::new("-out"),
                ca_key.as_os_str(),
            ],
            None,
        )?;
        set_mode(&ca_key, 0o600)?;
        run_cmd(
            "openssl",
            &[
                OsStr::new("req"),
                OsStr::new("-x509"),
                OsStr::new("-new"),
                OsStr::new("-key"),
                ca_key.as_os_str(),
                OsStr::new("-sha256"),
                OsStr::new("-days"),
                OsStr::new("3650"),
                OsStr::new("-out"),
                ca_crt.as_os_str(),
                OsStr::new("-subj"),
                OsStr::new("/CN=Briardr Local CA/O=Briardr"),
                OsStr::new("-addext"),
                OsStr::new("basicConstraints=critical,CA:TRUE,pathlen:0"),
                OsStr::new("-addext"),
                OsStr::new("keyUsage=critical,keyCertSign,cRLSign"),
            ],
            None,
        )?;
    }

    let (names, addrs) = cert_names(hostnames, ips);
    let want = stamp(&names, &addrs);
    let leaf_key = dir.join("server.key");
    let leaf_crt = dir.join("server.crt");
    let stamp_path = dir.join("server.names");
    let have = fs::read_to_string(&stamp_path).unwrap_or_default();
    let fresh = is_fresh(&leaf_crt, LEAF_RENEW_WITHIN_SECS);
    if !leaf_needs_renewal(have == want, leaf_key.exists(), fresh) {
        return read_material(&leaf_crt, &leaf_key, Some(&ca_crt), "local-ca");
    }

    let san = san(&names, &addrs);
    let csr = dir.join("server.csr");
    let ext = dir.join("server.ext");
    run_cmd(
        "openssl",
        &[
            OsStr::new("ecparam"),
            OsStr::new("-name"),
            OsStr::new("prime256v1"),
            OsStr::new("-genkey"),
            OsStr::new("-noout"),
            OsStr::new("-out"),
            leaf_key.as_os_str(),
        ],
        None,
    )?;
    set_mode(&leaf_key, 0o600)?;
    run_cmd(
        "openssl",
        &[
            OsStr::new("req"),
            OsStr::new("-new"),
            OsStr::new("-key"),
            leaf_key.as_os_str(),
            OsStr::new("-out"),
            csr.as_os_str(),
            OsStr::new("-subj"),
            OsStr::new("/CN=Briardr"),
        ],
        None,
    )?;
    fs::write(
        &ext,
        format!(
            "basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName={san}\n"
        ),
    )?;
    // 397 days: the longest validity Apple platforms accept for TLS server certificates.
    run_cmd(
        "openssl",
        &[
            OsStr::new("x509"),
            OsStr::new("-req"),
            OsStr::new("-in"),
            csr.as_os_str(),
            OsStr::new("-CA"),
            ca_crt.as_os_str(),
            OsStr::new("-CAkey"),
            ca_key.as_os_str(),
            OsStr::new("-CAcreateserial"),
            OsStr::new("-out"),
            leaf_crt.as_os_str(),
            OsStr::new("-days"),
            OsStr::new("397"),
            OsStr::new("-sha256"),
            OsStr::new("-extfile"),
            ext.as_os_str(),
        ],
        None,
    )?;
    let _ = fs::remove_file(&csr);
    fs::write(&stamp_path, want)?;
    read_material(&leaf_crt, &leaf_key, Some(&ca_crt), "local-ca")
}

pub fn ensure_tailscale_cert(dir: &Path, dns_name: &str) -> Result<TlsMaterial, TlsError> {
    let crt = dir.join("tailscale.crt");
    let key = dir.join("tailscale.key");
    if !is_fresh(&crt, TAILSCALE_RENEW_WITHIN_SECS) {
        run_cmd(
            "tailscale",
            &[
                OsStr::new("cert"),
                OsStr::new("--cert-file"),
                crt.as_os_str(),
                OsStr::new("--key-file"),
                key.as_os_str(),
                OsStr::new(dns_name),
            ],
            Some(TAILSCALE_TIMEOUT),
        )?;
        set_mode(&key, 0o600)?;
    }
    read_material(&crt, &key, None, "tailscale")
}

pub fn load_tls(
    dir: &Path,
    mode: &str,
    files: Option<(&Path, &Path)>,
) -> Result<Option<TlsMaterial>, TlsError> {
    match mode {
        // The proxy holds the public certificate; Briardr loads none.
        "proxy" => Ok(None),
        "files" => match files {
            Some((cert, key)) if !cert.as_os_str().is_empty() && !key.as_os_str().is_empty() => {
                read_material(cert, key, None, "files").map(Some)
            }
            _ => local_ca_material(dir).map(Some),
        },
        "tailscale" => read_material(
            &dir.join("tailscale.crt"),
            &dir.join("tailscale.key"),
            None,
            "tailscale",
        )
        .map(Some),
        _ => local_ca_material(dir).map(Some),
    }
}

fn local_ca_material(dir: &Path) -> Result<TlsMaterial, TlsError> {
    read_material(
        &dir.join("server.crt"),
        &dir.join("server.key"),
        Some(&dir.join("ca.crt")),
        "local-ca",
    )
}

/// An Apple configuration profile that installs the Briardr CA (iOS/iPadOS/macOS).
pub fn ca_mobile_config(ca_pem: &[u8], uuid1: &str, uuid2: &str) -> String {
    let text = String::from_utf8_lossy(ca_pem);
    let der: String = text
        .replace("-----BEGIN CERTIFICATE-----", "")
        .replace("-----END CERTIFICATE-----", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>PayloadContent</key><array><dict>
<key>PayloadCertificateFileName</key><string>briardr-ca.cer</string>
<key>PayloadContent</key><data>{der}</data>
<key>PayloadDisplayName</key><string>Briardr Local CA</string>
<key>PayloadIdentifier</key><string>app.briardr.ca.{uuid2}</string>
<key>PayloadType</key><string>com.apple.security.root</string>
<key>PayloadUUID</key><string>{uuid2}</string>
<key>PayloadVersion</key><integer>1</integer>
</dict></array>
<key>PayloadDisplayName</key><string>Briardr</string>
<key>PayloadDescription</key><string>Lets this device trust your Briardr server (your Mac or your own server) over your private network.</string>
<key>PayloadIdentifier</key><string>app.briardr.profile.{uuid1}</string>
<key>PayloadType</key><string>Configuration</string>
<key>PayloadUUID</key><string>{uuid1}</string>
<key>PayloadVersion</key><integer>1</integer>
</dict></plist>"#
    )
}

/// Sorted, de-duplicated DNS names and IP strings, always including localhost and 127.0.0.1.
fn cert_names(hostnames: &[String], ips: &[IpAddr]) -> (Vec<String>, Vec<String>) {
    let names: BTreeSet<String> = std::iter::once("localhost".to_string())
        .chain(hostnames.iter().cloned())
        .collect();
    let addrs: BTreeSet<String> = std::iter::once("127.0.0.1".to_string())
        .chain(ips.iter().map(|ip| ip.to_string()))
        .collect();
    (names.into_iter().collect(), addrs.into_iter().collect())
}

/// The stamp recorded beside the leaf certificate: names, then "|", then addresses.
fn stamp(names: &[String], addrs: &[String]) -> String {
    format!("{}|{}", names.join(","), addrs.join(","))
}

fn san(names: &[String], addrs: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("DNS:{n}"))
        .chain(addrs.iter().map(|i| format!("IP:{i}")))
        .collect::<Vec<_>>()
        .join(",")
}

/// Renew unless the stamp matches, the key exists and the certificate is not expiring soon.
fn leaf_needs_renewal(stamp_matches: bool, key_exists: bool, cert_fresh: bool) -> bool {
    !(stamp_matches && key_exists && cert_fresh)
}

fn is_fresh(cert: &Path, within_secs: u64) -> bool {
    let secs = within_secs.to_string();
    run_cmd(
        "openssl",
        &[
            OsStr::new("x509"),
            OsStr::new("-checkend"),
            OsStr::new(&secs),
            OsStr::new("-noout"),
            OsStr::new("-in"),
            cert.as_os_str(),
        ],
        None,
    )
    .is_ok()
}

fn read_material(
    cert: &Path,
    key: &Path,
    ca: Option<&Path>,
    mode: &str,
) -> Result<TlsMaterial, TlsError> {
    Ok(TlsMaterial {
        cert_pem: fs::read(cert)?,
        key_pem: fs::read(key)?,
        ca_cert_pem: match ca {
            Some(path) => Some(fs::read(path)?),
            None => None,
        },
        mode: mode.to_string(),
    })
}

fn set_mode(path: &Path, mode: u32) -> Result<(), TlsError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Runs a CLI and fails on a non-zero exit, with its trimmed stderr in the message.
fn run_cmd(program: &str, args: &[&OsStr], timeout: Option<Duration>) -> Result<(), TlsError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| TlsError::Msg(format!("{program} could not start: {e}")))?;
    let status = match timeout {
        None => child.wait()?,
        Some(limit) => {
            let started = Instant::now();
            loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                if started.elapsed() >= limit {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(TlsError::Msg(format!("{program} timed out")));
                }
                thread::sleep(POLL_INTERVAL);
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    Err(failed(program, status, child))
}

fn failed(program: &str, status: ExitStatus, mut child: std::process::Child) -> TlsError {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }
    TlsError::Msg(format!("{program} failed ({status}): {}", stderr.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const FIXED_CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBszCCAVmgAwIBAgIUR2tVd0pK7m1cMAoGCCqGSM49BAMCMBkxFzAVBgNVBAMM
DktlbHBpZSBUZXN0MB4XDTI2MTAwMTAwMDAwMFoXDTM2MTAwMTAwMDAwMFowGTEX
-----END CERTIFICATE-----
";

    const GOLDEN_MOBILECONFIG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>PayloadContent</key><array><dict>
<key>PayloadCertificateFileName</key><string>briardr-ca.cer</string>
<key>PayloadContent</key><data>MIIBszCCAVmgAwIBAgIUR2tVd0pK7m1cMAoGCCqGSM49BAMCMBkxFzAVBgNVBAMMDktlbHBpZSBUZXN0MB4XDTI2MTAwMTAwMDAwMFoXDTM2MTAwMTAwMDAwMFowGTEX</data>
<key>PayloadDisplayName</key><string>Briardr Local CA</string>
<key>PayloadIdentifier</key><string>app.briardr.ca.A1B2C3D4-E5F6-4789-8ABC-DEF012345678</string>
<key>PayloadType</key><string>com.apple.security.root</string>
<key>PayloadUUID</key><string>A1B2C3D4-E5F6-4789-8ABC-DEF012345678</string>
<key>PayloadVersion</key><integer>1</integer>
</dict></array>
<key>PayloadDisplayName</key><string>Briardr</string>
<key>PayloadDescription</key><string>Lets this device trust your Briardr server (your Mac or your own server) over your private network.</string>
<key>PayloadIdentifier</key><string>app.briardr.profile.6F1B2C3D-0000-4A5B-8C9D-0E1F2A3B4C5D</string>
<key>PayloadType</key><string>Configuration</string>
<key>PayloadUUID</key><string>6F1B2C3D-0000-4A5B-8C9D-0E1F2A3B4C5D</string>
<key>PayloadVersion</key><integer>1</integer>
</dict></plist>"#;

    #[test]
    fn san_and_stamp_dedupe_sort_and_include_loopback() {
        let hostnames = vec![
            "mac.tailnet.ts.net".to_string(),
            "localhost".to_string(),
            "mac.local".to_string(),
        ];
        let ips: Vec<IpAddr> = vec![
            "192.168.1.20".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
            "192.168.1.20".parse().unwrap(),
        ];
        let (names, addrs) = cert_names(&hostnames, &ips);
        assert_eq!(names, ["localhost", "mac.local", "mac.tailnet.ts.net"]);
        assert_eq!(addrs, ["127.0.0.1", "192.168.1.20"]);
        assert_eq!(
            stamp(&names, &addrs),
            "localhost,mac.local,mac.tailnet.ts.net|127.0.0.1,192.168.1.20"
        );
        assert_eq!(
            san(&names, &addrs),
            "DNS:localhost,DNS:mac.local,DNS:mac.tailnet.ts.net,IP:127.0.0.1,IP:192.168.1.20"
        );
    }

    #[test]
    fn leaf_renewal_truth_table() {
        // (stamp matches, key exists, cert fresh) -> renew
        assert!(!leaf_needs_renewal(true, true, true));
        assert!(leaf_needs_renewal(false, true, true));
        assert!(leaf_needs_renewal(true, false, true));
        assert!(leaf_needs_renewal(true, true, false));
        assert!(leaf_needs_renewal(false, false, false));
        assert!(leaf_needs_renewal(false, true, false));
        assert!(leaf_needs_renewal(true, false, false));
        assert!(leaf_needs_renewal(false, false, true));
    }

    #[test]
    fn ca_mobile_config_matches_ts_output() {
        let out = ca_mobile_config(
            FIXED_CA_PEM.as_bytes(),
            "6F1B2C3D-0000-4A5B-8C9D-0E1F2A3B4C5D",
            "A1B2C3D4-E5F6-4789-8ABC-DEF012345678",
        );
        assert_eq!(out, GOLDEN_MOBILECONFIG);
    }

    #[test]
    fn ensure_local_ca_creates_leaf_material_in_fresh_dir() {
        let scratch = crate::scratch::Scratch::new("tls");
        let dir = scratch.join("tls"); // doesn't exist yet

        let hostnames = vec!["mac.local".to_string()];
        let ips: Vec<IpAddr> = vec!["192.168.1.20".parse().unwrap()];
        let material = ensure_local_ca(&dir, &hostnames, &ips).expect("ensure_local_ca");

        assert!(dir.join("ca.crt").exists());
        assert!(dir.join("server.crt").exists());
        assert!(dir.join("server.key").exists());
        let mode = fs::metadata(dir.join("server.key"))
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            fs::read_to_string(dir.join("server.names")).expect("stamp"),
            "localhost,mac.local|127.0.0.1,192.168.1.20"
        );
        assert_eq!(material.mode, "local-ca");
        assert!(material.ca_cert_pem.is_some());
    }
}
