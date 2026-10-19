// What the host can do: Mac-only integrations are off elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Macos,
    Linux,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapabilities {
    pub platform: HostPlatform,
    /// "Mac" or "server".
    pub noun: &'static str,
    pub battery: bool,
    pub keep_awake: bool,
    pub sleep: bool,
    pub dictation: bool,
    pub claude_keychain: bool,
}

/// Host platform from the `BRIARDR_PLATFORM` override or the OS.
pub fn host_platform(p: &str) -> HostPlatform {
    match p {
        "darwin" | "macos" => HostPlatform::Macos,
        "linux" => HostPlatform::Linux,
        _ => HostPlatform::Other,
    }
}

/// Capabilities for `platform`. Callers resolve `BRIARDR_PLATFORM` with `host_platform` first.
pub fn capabilities(platform: HostPlatform) -> HostCapabilities {
    let mac = platform == HostPlatform::Macos;
    HostCapabilities {
        platform,
        noun: if mac { "Mac" } else { "server" },
        battery: mac,
        keep_awake: mac,
        sleep: mac,
        dictation: mac,
        claude_keychain: mac,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_platform_maps_names() {
        assert_eq!(host_platform("darwin"), HostPlatform::Macos);
        assert_eq!(host_platform("macos"), HostPlatform::Macos);
        assert_eq!(host_platform("linux"), HostPlatform::Linux);
        assert_eq!(host_platform("win32"), HostPlatform::Other);
        assert_eq!(host_platform("other"), HostPlatform::Other);
        assert_eq!(host_platform(""), HostPlatform::Other);
    }

    #[test]
    fn mac_integrations_only_on_macos() {
        let mac = capabilities(HostPlatform::Macos);
        assert_eq!(mac.noun, "Mac");
        assert!(mac.battery && mac.keep_awake && mac.sleep && mac.dictation && mac.claude_keychain);

        for p in [HostPlatform::Linux, HostPlatform::Other] {
            let c = capabilities(p);
            assert_eq!(c.platform, p);
            assert_eq!(c.noun, "server");
            assert!(!(c.battery || c.keep_awake || c.sleep || c.dictation || c.claude_keychain));
        }
    }
}
