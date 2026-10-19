// Network checks: allowed peers, trusted proxies, X-Forwarded-For, listen addresses. Peer and proxy trust
// decisions live here.
// Signatures take strings, so the golden fixtures in fixtures/net/ apply directly.

use std::collections::HashSet;
use std::io::Read;
use std::net::Ipv6Addr;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(5);
const TAILSCALE_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TailscaleInfo {
    /// Tailnet addresses only (CGNAT 100.64.0.0/10 or fd7a:115c:a1e0::/48).
    pub ips: Vec<String>,
    pub dns_name: Option<String>,
    pub cert_domains: Vec<String>,
}

/// `isAllowedPeer`: loopback, an explicit allowed IPv4 network, or a Tailscale address.
pub fn is_allowed_peer(addr: Option<&str>, allowed_networks: &[String]) -> bool {
    let Some(addr) = addr.filter(|a| !a.is_empty()) else {
        return false;
    };
    let a = addr.strip_prefix("::ffff:").unwrap_or(addr);
    if a == "::1" || a.starts_with("127.") {
        return true;
    }
    if allowed_networks
        .iter()
        .any(|network| in_ipv4_network(a, network))
    {
        return true;
    }
    if is_ipv4(a) {
        // 100.64.0.0/10 (Tailscale CGNAT range)
        let second = a.split('.').nth(1).and_then(|o| o.parse::<u8>().ok());
        return a.starts_with("100.") && matches!(second, Some(64..=127));
    }
    // Tailscale ULA /48
    is_ipv6(a) && a.to_lowercase().starts_with("fd7a:115c:a1e0:")
}

/// `inIPv4Network`: an IPv4 address against an IPv4 address or CIDR. Malformed networks fail closed.
pub fn in_ipv4_network(addr: &str, network: &str) -> bool {
    let mut parts = network.split('/');
    let base = parts.next().unwrap_or("");
    let prefix = parts.next();
    if parts.next().is_some() || !is_ipv4(addr) || !is_ipv4(base) {
        return false;
    }
    let bits = match prefix {
        None => 32,
        Some(p) => match parse_prefix(p, 0) {
            Some(bits) => bits,
            None => return false,
        },
    };
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    (ipv4_value(addr) & mask) == (ipv4_value(base) & mask)
}

/// `matchesAddress`: an address against an IP or IPv4 CIDR entry (IPv4-mapped IPv6 counts as IPv4).
pub fn matches_address(addr: Option<&str>, entry: &str) -> bool {
    let Some(addr) = addr.filter(|a| !a.is_empty()) else {
        return false;
    };
    let a = normalize_addr(addr);
    let e = normalize_addr(entry);
    if e.contains('/') {
        return in_ipv4_network(&a, &e);
    }
    if is_ipv6(&a) && is_ipv6(&e) {
        return expand_ipv6(&a) == expand_ipv6(&e);
    }
    is_ip(&a) && a == e
}

/// `isTrustedProxy`: any trusted entry matches the peer.
pub fn is_trusted_proxy(addr: Option<&str>, trusted: &[String]) -> bool {
    trusted.iter().any(|entry| matches_address(addr, entry))
}

/// `forwardedClient`: the client behind trusted proxies. Returns "unknown" with no peer and "invalid" for a
/// malformed hop; otherwise the first untrusted hop from the right, or the peer when there is none.
pub fn forwarded_client(xff: Option<&str>, peer: Option<&str>, trusted: &[String]) -> String {
    let fallback = peer.filter(|p| !p.is_empty()).unwrap_or("unknown");
    if !is_trusted_proxy(peer, trusted) {
        return fallback.to_string();
    }
    let hops: Vec<&str> = xff
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .collect();
    for hop in hops.into_iter().rev() {
        let h = strip_brackets(hop);
        if !is_ip(h) {
            return "invalid".to_string();
        }
        if !is_trusted_proxy(Some(h), trusted) {
            return normalize_addr(h);
        }
    }
    fallback.to_string()
}

/// `tailscaleInfo`: runs `tailscale status --json` with a 5 s timeout. Any failure gives the empty info.
/// This blocks the calling thread for up to 5 s; from async code, call it through `spawn_blocking`.
pub fn tailscale_info() -> TailscaleInfo {
    let mut child = match Command::new("tailscale")
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return TailscaleInfo::default(),
    };
    // Read stdout on its own thread so a large status cannot fill the pipe and stall the wait.
    let reader = child.stdout.take().map(|mut out| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });
    let deadline = Instant::now() + TAILSCALE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(TAILSCALE_POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let (Some(status), Some(reader)) = (status, reader) else {
        return TailscaleInfo::default();
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        return TailscaleInfo::default();
    }
    parse_tailscale_status(&String::from_utf8_lossy(&out))
}

/// The JSON-to-struct step of `tailscaleInfo`. Anything that would make the parse throw gives the empty info.
fn parse_tailscale_status(stdout: &str) -> TailscaleInfo {
    let Ok(status) = serde_json::from_str::<Value>(stdout) else {
        return TailscaleInfo::default();
    };
    let Some(root) = status.as_object() else {
        return TailscaleInfo::default();
    };
    if root.get("BackendState").and_then(Value::as_str) != Some("Running") {
        return TailscaleInfo::default();
    }
    let me = root.get("Self").and_then(Value::as_object);

    let ips = match me.and_then(|m| m.get("TailscaleIPs")) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut ips = Vec::new();
            for item in items {
                match item {
                    Value::String(ip) => {
                        if is_allowed_peer(Some(ip), &[]) {
                            ips.push(ip.clone());
                        }
                    }
                    Value::Null | Value::Bool(false) => {}
                    _ => return TailscaleInfo::default(),
                }
            }
            ips
        }
        Some(_) => return TailscaleInfo::default(),
    };

    let dns_name = match me.and_then(|m| m.get("DNSName")) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(Value::String(name)) => {
            let name = name.strip_suffix('.').unwrap_or(name);
            (!name.is_empty()).then(|| name.to_string())
        }
        Some(_) => return TailscaleInfo::default(),
    };

    let cert_domains = match root.get("CertDomains") {
        Some(Value::Array(domains)) => domains
            .iter()
            .filter_map(|d| d.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };

    TailscaleInfo {
        ips,
        dns_name,
        cert_domains,
    }
}

/// `listenAddresses`: 127.0.0.1, then the Tailscale addresses for "tailscale", then literal IPs other
/// than the wildcards. Insertion order, no duplicates.
pub fn listen_addresses(listen: &[String], ts: &TailscaleInfo) -> Vec<String> {
    let mut addrs = vec!["127.0.0.1".to_string()];
    for entry in listen {
        if entry == "tailscale" {
            for ip in &ts.ips {
                push_unique(&mut addrs, ip);
            }
        } else if is_ip(entry) && entry != "0.0.0.0" && entry != "::" {
            push_unique(&mut addrs, entry);
        }
    }
    addrs
}

/// `missingAddresses`: non-loopback addresses not among `have` (the interface addresses, zone IDs removed).
/// Compared case-insensitively; returns the original spelling.
pub fn missing_addresses(addrs: &[String], have: &[String]) -> Vec<String> {
    let have: HashSet<String> = have
        .iter()
        .map(|h| {
            let lower = h.to_lowercase();
            lower.split('%').next().unwrap_or("").to_string()
        })
        .collect();
    addrs
        .iter()
        .filter(|a| {
            !a.starts_with("127.") && a.as_str() != "::1" && !have.contains(&a.to_lowercase())
        })
        .cloned()
        .collect()
}

/// `missingAddresses` against this host's interfaces. std has no interface listing, so each address is
/// checked by binding a UDP socket to it (port 0): that fails exactly when no interface has the address.
pub fn missing_local_addresses(addrs: &[String]) -> Vec<String> {
    let have: Vec<String> = addrs
        .iter()
        .filter(|a| {
            a.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| std::net::UdpSocket::bind((ip, 0)).is_ok())
        })
        .cloned()
        .collect();
    missing_addresses(addrs, &have)
}

/// `validNetwork`: an IPv4 address or CIDR with prefix 1–32 that is not 0.0.0.0. Returns it, or None.
pub fn valid_network(entry: &str) -> Option<String> {
    let mut parts = entry.split('/');
    let base = parts.next().unwrap_or("");
    let prefix = parts.next();
    if parts.next().is_some() || !is_ipv4(base) {
        return None;
    }
    if let Some(p) = prefix {
        parse_prefix(p, 1)?;
    }
    if base == "0.0.0.0" {
        return None;
    }
    Some(entry.to_string())
}

/// Node's `net.isIPv4`: four decimal octets 0–255 with no leading zeros.
fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u8>().is_ok()
        })
}

/// Node's `net.isIPv6`: an IPv6 address, optionally followed by a `%zone` suffix.
fn is_ipv6(s: &str) -> bool {
    let (addr, zone) = match s.split_once('%') {
        Some((addr, zone)) => (addr, Some(zone)),
        None => (s, None),
    };
    let zone_ok = zone.is_none_or(|z| {
        !z.is_empty()
            && z.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':'))
    });
    zone_ok && addr.parse::<Ipv6Addr>().is_ok()
}

fn is_ip(s: &str) -> bool {
    is_ipv4(s) || is_ipv6(s)
}

/// The prefix rule shared by both network checks: 1–2 digits with no leading zero, from `min` to 32.
fn parse_prefix(s: &str, min: u32) -> Option<u32> {
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u32 = s.parse().ok()?;
    if s.len() == 2 && value < 10 {
        return None;
    }
    (min..=32).contains(&value).then_some(value)
}

fn ipv4_value(s: &str) -> u32 {
    s.split('.').fold(0u32, |n, octet| {
        (n << 8) | u32::from(octet.parse::<u8>().unwrap_or(0))
    })
}

/// Lowercases an address and drops the `::ffff:` prefix when a dotted quad follows it.
fn normalize_addr(addr: &str) -> String {
    let lower = addr.to_lowercase();
    match lower.strip_prefix("::ffff:") {
        Some(rest) if is_dotted_quad_shape(rest) => rest.to_string(),
        _ => lower,
    }
}

/// Shape check only (digits and dots); `is_ipv4` does the real validation.
fn is_dotted_quad_shape(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Removes one `[...]` pair and an optional `:port` after it.
fn strip_brackets(hop: &str) -> &str {
    let Some(rest) = hop.strip_prefix('[') else {
        return hop;
    };
    let Some((inner, tail)) = rest.split_once(']') else {
        return hop;
    };
    let port_ok = tail.is_empty()
        || tail
            .strip_prefix(':')
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if inner.is_empty() || !port_ok {
        return hop;
    }
    inner
}

/// Expands an IPv6 address to eight 4-digit groups.
fn expand_ipv6(a: &str) -> String {
    let mut halves = a.splitn(2, "::");
    let head = halves.next().unwrap_or("");
    let tail = halves.next().unwrap_or("");
    let groups_of = |s: &str| -> Vec<String> {
        if s.is_empty() {
            Vec::new()
        } else {
            s.split(':').map(str::to_string).collect()
        }
    };
    let h = groups_of(head);
    let t = groups_of(tail);
    let groups: Vec<String> = if a.contains("::") {
        let fill = 8usize.saturating_sub(h.len() + t.len());
        h.into_iter()
            .chain((0..fill).map(|_| "0".to_string()))
            .chain(t)
            .collect()
    } else {
        h
    };
    groups
        .iter()
        .map(|g| format!("{g:0>4}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn push_unique(list: &mut Vec<String>, addr: &str) {
    if !list.iter().any(|a| a == addr) {
        list.push(addr.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    /// Every `<func>.<case>.json` golden file, sorted by name.
    fn cases(func: &str) -> Vec<(String, Value)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/net");
        let prefix = format!("{func}.");
        let entries =
            std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read fixtures/net: {e}"));
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) || !name.ends_with(".json") {
                continue;
            }
            let text = std::fs::read_to_string(entry.path())
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));
            let value: Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad fixture {name}: {e}"));
            found.push((name, value));
        }
        assert!(!found.is_empty(), "no fixtures for {func}");
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    fn input_of(case: &Value, name: &str) -> Vec<Value> {
        case.get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("{name}: no input array"))
    }

    fn strings(v: Option<&Value>) -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn golden_is_allowed_peer() {
        for (name, case) in cases("isAllowedPeer") {
            let input = input_of(&case, &name);
            let addr = input.first().and_then(Value::as_str);
            let networks = strings(input.get(1));
            let expected = case.get("output").and_then(Value::as_bool);
            assert_eq!(Some(is_allowed_peer(addr, &networks)), expected, "{name}");
        }
    }

    #[test]
    fn golden_is_trusted_proxy() {
        for (name, case) in cases("isTrustedProxy") {
            let input = input_of(&case, &name);
            let addr = input.first().and_then(Value::as_str);
            let trusted = strings(input.get(1));
            let expected = case.get("output").and_then(Value::as_bool);
            assert_eq!(Some(is_trusted_proxy(addr, &trusted)), expected, "{name}");
        }
    }

    #[test]
    fn golden_matches_address() {
        for (name, case) in cases("matchesAddress") {
            let input = input_of(&case, &name);
            let addr = input.first().and_then(Value::as_str);
            let entry = input.get(1).and_then(Value::as_str).unwrap_or("");
            let expected = case.get("output").and_then(Value::as_bool);
            assert_eq!(Some(matches_address(addr, entry)), expected, "{name}");
        }
    }

    #[test]
    fn golden_forwarded_client() {
        for (name, case) in cases("forwardedClient") {
            let input = input_of(&case, &name);
            let xff = input.first().and_then(Value::as_str);
            let peer = input.get(1).and_then(Value::as_str);
            let trusted = strings(input.get(2));
            let expected = case.get("output").and_then(Value::as_str);
            assert_eq!(
                Some(forwarded_client(xff, peer, &trusted).as_str()),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn golden_missing_addresses() {
        for (name, case) in cases("missingAddresses") {
            let input = input_of(&case, &name);
            let addrs = strings(input.first());
            // The second input maps interface name to [{address}]; flatten it to the address list.
            let have: Vec<String> = input
                .get(1)
                .and_then(Value::as_object)
                .map(|ifaces| {
                    ifaces
                        .values()
                        .flat_map(|list| list.as_array().into_iter().flatten())
                        .filter_map(|e| e.get("address").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                missing_addresses(&addrs, &have),
                strings(case.get("output")),
                "{name}"
            );
        }
    }

    #[test]
    fn golden_valid_network() {
        for (name, case) in cases("validNetwork") {
            let input = input_of(&case, &name);
            let entry = input.first().and_then(Value::as_str).unwrap_or("");
            let expected = case
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(valid_network(entry), expected, "{name}");
        }
    }

    #[test]
    fn leading_zero_octets_are_rejected() {
        assert!(!is_ipv4("010.0.0.1"));
        assert!(!is_ipv4("1.2.3.04"));
        assert!(is_ipv4("0.0.0.0"));
        assert!(!in_ipv4_network("10.0.0.1", "010.0.0.0/8"));
        assert!(!is_allowed_peer(Some("100.064.1.1"), &[]));
    }

    #[test]
    fn zone_suffix_is_stripped_for_address_checks() {
        assert!(is_ipv6("fe80::1%lo0"));
        assert!(!is_ipv6("fe80::1%"));
        assert!(!is_allowed_peer(Some("fe80::1%lo0"), &[]));
        assert!(is_allowed_peer(Some("fd7a:115c:a1e0::1%utun3"), &[]));
        assert!(!is_allowed_peer(Some("fd7a:115c:a2e0::1%utun3"), &[]));
        assert!(
            missing_addresses(&["fe80::1".to_string()], &["fe80::1%lo0".to_string()]).is_empty()
        );
    }

    #[test]
    fn ipv4_mapped_addresses_match_as_ipv4() {
        assert!(matches_address(Some("::ffff:10.0.0.5"), "10.0.0.0/8"));
        assert!(matches_address(Some("::FFFF:10.0.0.5"), "10.0.0.5"));
        assert!(matches_address(Some("::ffff:10.0.0.5"), "::ffff:10.0.0.5"));
        assert!(!matches_address(Some("::ffff:10.0.0.5"), "10.0.0.6"));
        assert!(!matches_address(None, "10.0.0.5"));
        assert!(!matches_address(Some(""), ""));
    }

    #[test]
    fn valid_network_rejects_everything_and_zero_prefix() {
        assert_eq!(valid_network("0.0.0.0/0"), None);
        assert_eq!(valid_network("0.0.0.0"), None);
        assert_eq!(valid_network("10.0.0.0/0"), None);
        assert_eq!(valid_network("10.0.0.0/08"), None);
        assert_eq!(valid_network("10.0.0.0/33"), None);
        assert_eq!(
            valid_network("10.0.0.0/32"),
            Some("10.0.0.0/32".to_string())
        );
    }

    #[test]
    fn prefix_zero_is_accepted_by_in_ipv4_network() {
        assert!(in_ipv4_network("8.8.8.8", "0.0.0.0/0"));
        assert!(in_ipv4_network("8.8.8.8", "10.0.0.0/0"));
        assert!(!in_ipv4_network("8.8.8.8", "0.0.0.0/00"));
        assert!(!in_ipv4_network("8.8.8.8", "0.0.0.0/33"));
        assert!(!in_ipv4_network("8.8.8.8", "0.0.0.0/8/9"));
    }

    #[test]
    fn bracketed_hop_with_port_is_unwrapped() {
        let trusted = vec!["127.0.0.1".to_string()];
        assert_eq!(
            forwarded_client(Some("[2001:DB8::5]:443"), Some("127.0.0.1"), &trusted),
            "2001:db8::5"
        );
        assert_eq!(
            forwarded_client(
                Some("198.51.100.4, [2001:db8::5]"),
                Some("127.0.0.1"),
                &trusted
            ),
            "2001:db8::5"
        );
        assert_eq!(
            forwarded_client(Some("1.2.3.4:80"), Some("127.0.0.1"), &trusted),
            "invalid"
        );
        assert_eq!(
            forwarded_client(
                Some("[::ffff:198.51.100.4]:8080"),
                Some("127.0.0.1"),
                &trusted
            ),
            "198.51.100.4"
        );
    }

    #[test]
    fn parse_tailscale_status_running() {
        let text = json!({
            "BackendState": "Running",
            "Self": {
                "TailscaleIPs": ["100.64.0.5", "fd7a:115c:a1e0::5", "10.0.0.1"],
                "DNSName": "host.tail1234.ts.net."
            },
            "CertDomains": ["host.tail1234.ts.net"]
        })
        .to_string();
        assert_eq!(
            parse_tailscale_status(&text),
            TailscaleInfo {
                ips: vec!["100.64.0.5".to_string(), "fd7a:115c:a1e0::5".to_string()],
                dns_name: Some("host.tail1234.ts.net".to_string()),
                cert_domains: vec!["host.tail1234.ts.net".to_string()],
            }
        );
    }

    #[test]
    fn parse_tailscale_status_empty_dns_name_is_none() {
        let text = json!({ "BackendState": "Running", "Self": { "DNSName": "." } }).to_string();
        assert_eq!(parse_tailscale_status(&text), TailscaleInfo::default());
        let text = json!({ "BackendState": "Running", "Self": { "DNSName": "" } }).to_string();
        assert_eq!(parse_tailscale_status(&text), TailscaleInfo::default());
    }

    #[test]
    fn parse_tailscale_status_not_running_is_empty() {
        let text = json!({
            "BackendState": "Stopped",
            "Self": { "TailscaleIPs": ["100.64.0.5"], "DNSName": "host.ts.net." },
            "CertDomains": ["host.ts.net"]
        })
        .to_string();
        assert_eq!(parse_tailscale_status(&text), TailscaleInfo::default());
    }

    #[test]
    fn parse_tailscale_status_malformed_is_empty() {
        assert_eq!(
            parse_tailscale_status("{not json"),
            TailscaleInfo::default()
        );
        assert_eq!(parse_tailscale_status(""), TailscaleInfo::default());
        assert_eq!(parse_tailscale_status("null"), TailscaleInfo::default());
    }

    #[test]
    fn listen_addresses_orders_and_dedupes() {
        let ts = TailscaleInfo {
            ips: vec!["100.64.0.5".to_string()],
            dns_name: None,
            cert_domains: Vec::new(),
        };
        let listen = [
            "tailscale",
            "0.0.0.0",
            "::",
            "10.0.0.2",
            "127.0.0.1",
            "10.0.0.2",
            "not-an-ip",
        ]
        .map(str::to_string);
        assert_eq!(
            listen_addresses(&listen, &ts),
            vec!["127.0.0.1", "100.64.0.5", "10.0.0.2"]
        );
    }
}
