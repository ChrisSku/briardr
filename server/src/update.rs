// Updating Briardr: which release is the latest, and installing it in place of this one. `briardr update`
// and Settings → About use it, and `briardr serve` asks GitHub once a day while `updates.check` is on.
// Installing does what install.sh does: the release's archive for this computer, checked against the
// release's SHA256SUMS, unpacked next to the installed folder and swapped in. Only a release installation
// (bin/briardr next to web/dist, as install.sh puts it) updates itself; a checkout, the Linux helper's
// included, says what to run instead. A running Briardr keeps running the old version until it restarts:
// when launchd or systemd runs it, it restarts itself after an update from the app.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::{ACCEPT, USER_AGENT};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::VERSION;

pub const HELP: &str = r#"Usage: briardr update [--check] [--yes]
Installs the latest Briardr release in place of this one, checked against the release's checksums, and
restarts the Briardr that runs at login. Herdr and your agents keep running.
  --check   only say whether there's a newer release
  --yes     install without asking"#;

const RELEASES: &str = "https://github.com/ChrisSku/briardr/releases";
const LATEST_API: &str = "https://api.github.com/repos/ChrisSku/briardr/releases/latest";
/// The installer, for an installation that can't update itself.
const INSTALLER: &str = "curl -fsSL https://briardr.dev/install.sh | sh";
/// launchd's label for `briardr install-agent`'s LaunchAgent.
pub const AGENT_LABEL: &str = "app.briardr.server";

/// Where releases come from: GitHub, or for tests `BRIARDR_RELEASES_URL`, a folder with `latest` (the
/// API's answer) and `download/v<version>/` (the assets).
#[derive(Debug, Clone)]
pub struct Source {
    base: Option<String>,
}

impl Source {
    pub fn from_env(env: &HashMap<String, String>) -> Self {
        let base = env
            .get("BRIARDR_RELEASES_URL")
            .map(|b| b.trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty());
        Source { base }
    }

    fn latest_url(&self) -> String {
        match &self.base {
            Some(base) => format!("{base}/latest"),
            None => LATEST_API.to_string(),
        }
    }

    fn download_url(&self, version: &str, file: &str) -> String {
        let base = self.base.as_deref().unwrap_or(RELEASES);
        format!("{base}/download/v{version}/{file}")
    }
}

/// A release's notes on GitHub.
pub fn notes_url(version: &str) -> String {
    format!("{RELEASES}/tag/v{version}")
}

/// "1.2.3" (or "v1.2.3") as numbers; anything else, pre-releases included, is None.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.strip_prefix('v').unwrap_or(v).split('.');
    let mut next = || {
        parts
            .next()
            .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))?
            .parse()
            .ok()
    };
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// Whether `latest` is a newer release than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// The latest release's version, as GitHub reports it.
pub async fn latest(source: &Source) -> Result<String, String> {
    let url = source.latest_url();
    let fail = |e: reqwest::Error| format!("Couldn’t ask GitHub for the latest release: {e}");
    let client = crate::http::client(Duration::from_secs(20)).map_err(fail)?;
    let bytes = client
        .get(&url)
        .header(USER_AGENT, format!("briardr/{VERSION}"))
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(fail)?
        .bytes()
        .await
        .map_err(fail)?;
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("GitHub’s answer about the latest release isn’t JSON: {e}"))?;
    let tag = body["tag_name"].as_str().unwrap_or("");
    let (a, b, c) = parse_version(tag).ok_or_else(|| {
        format!("GitHub’s latest release has a tag Briardr doesn’t understand: {tag:?}")
    })?;
    Ok(format!("{a}.{b}.{c}"))
}

/// How this Briardr is installed, which decides how it's updated.
#[derive(Debug, PartialEq, Eq)]
pub enum Installation {
    /// A release, as install.sh puts it: this folder is replaced by the new one.
    Release(PathBuf),
    /// A Git checkout, built from source.
    Checkout(PathBuf),
    /// Something else (the client from BRIARDR_WEB_DIST, a binary copied by itself).
    Other,
}

/// `root` is the project root (`crate::project_root`), `exe` the real path of the running executable.
pub fn installation(root: &Path, exe: Option<&Path>) -> Installation {
    if root.join(".git").exists() {
        return Installation::Checkout(root.to_path_buf());
    }
    let bin = root.join("bin").join("briardr");
    let is_this = exe.is_some_and(|exe| std::fs::canonicalize(&bin).is_ok_and(|b| b == exe));
    if is_this && root.join("web").join("dist").is_dir() {
        Installation::Release(root.to_path_buf())
    } else {
        Installation::Other
    }
}

/// This Briardr's installation.
pub fn this_installation() -> Installation {
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).ok();
    installation(&crate::project_root(), exe.as_deref())
}

/// What to run to get `version` when this installation can't update itself: where, and the command.
pub fn how_to_update(installation: &Installation, version: &str) -> Option<(String, String)> {
    match installation {
        Installation::Release(_) => None,
        Installation::Checkout(root)
            if root.join("scripts").join("setup-linux.sh").exists()
                && std::env::consts::OS == "linux" =>
        {
            Some((
                format!("This is the Linux helper’s checkout. In {}, run:", root.display()),
                format!("scripts/setup-linux.sh upgrade v{version}"),
            ))
        }
        Installation::Checkout(root) => Some((
            format!("This Briardr runs from a checkout. In {}, run:", root.display()),
            format!("git fetch --tags && git checkout v{version} && mise install && mise run install && mise run build && mise run build:server"),
        )),
        Installation::Other => Some((
            "This Briardr can’t replace itself. Install the new version with:".to_string(),
            INSTALLER.to_string(),
        )),
    }
}

/// The supervisor that starts Briardr again when it exits: launchd (`briardr install-agent`) or systemd.
pub fn supervisor(env: &HashMap<String, String>) -> Option<&'static str> {
    if env.get("XPC_SERVICE_NAME").map(String::as_str) == Some(AGENT_LABEL) {
        Some("launchd")
    } else if env.get("INVOCATION_ID").is_some_and(|id| !id.is_empty()) {
        Some("systemd")
    } else {
        None
    }
}

/// The release archive for this computer, as scripts/package.ts names it.
fn asset() -> Result<String, String> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        other => {
            return Err(format!(
                "Briardr releases are for macOS and Linux, not {other}."
            ))
        }
    };
    match std::env::consts::ARCH {
        arch @ ("aarch64" | "x86_64") => Ok(format!("briardr-{os}-{arch}.tar.gz")),
        other => Err(format!(
            "Briardr releases are for aarch64 and x86_64, not {other}."
        )),
    }
}

async fn download(url: &str) -> Result<Vec<u8>, String> {
    let fail = |e: reqwest::Error| format!("Couldn’t download {url}: {e}");
    let client = crate::http::downloader(Duration::from_secs(300)).map_err(fail)?;
    let response = client
        .get(url)
        .header(USER_AGENT, format!("briardr/{VERSION}"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(fail)?;
    Ok(response.bytes().await.map_err(fail)?.to_vec())
}

/// Downloads `version` and puts it in place of the release installed in `dir`.
pub async fn install(source: &Source, version: &str, dir: &Path) -> Result<(), String> {
    parse_version(version).ok_or_else(|| format!("{version:?} isn’t a Briardr version."))?;
    let asset = asset()?;
    let archive = download(&source.download_url(version, &asset)).await?;
    let sums = download(&source.download_url(version, "SHA256SUMS")).await?;
    let sums = String::from_utf8_lossy(&sums).into_owned();
    let (version, dir) = (version.to_string(), dir.to_path_buf());
    tokio::task::spawn_blocking(move || place(&archive, &sums, &asset, &version, &dir))
        .await
        .map_err(|e| e.to_string())?
}

/// Checks a downloaded release, unpacks it next to `dir` and swaps it in. Nothing changes in `dir`
/// unless the archive matches its checksum and its `bin/briardr` runs and is `version`.
fn place(archive: &[u8], sums: &str, asset: &str, version: &str, dir: &Path) -> Result<(), String> {
    let want = sums
        .lines()
        .find_map(|line| {
            let (hash, file) = line.split_once("  ")?;
            (file.trim() == asset).then(|| hash.trim().to_lowercase())
        })
        .ok_or_else(|| {
            format!("The release’s SHA256SUMS doesn’t list {asset}. Nothing was installed.")
        })?;
    if hex::encode(Sha256::digest(archive)) != want {
        return Err(format!(
            "{asset} doesn’t match the release’s checksum. Nothing was installed."
        ));
    }
    let name = dir
        .file_name()
        .ok_or_else(|| format!("{} isn’t a folder Briardr can replace.", dir.display()))?
        .to_string_lossy()
        .into_owned();
    // Next to the installed folder, so the swap is a rename on the same disk.
    let staging = dir.with_file_name(format!(".{name}.update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = unpack_and_swap(archive, asset, version, dir, &name, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn unpack_and_swap(
    archive: &[u8],
    asset: &str,
    version: &str,
    dir: &Path,
    name: &str,
    staging: &Path,
) -> Result<(), String> {
    let io =
        |what: &str, e: std::io::Error| format!("Couldn’t {what}: {e}. Nothing was installed.");
    let unpacked = staging.join("unpacked");
    std::fs::create_dir_all(&unpacked)
        .map_err(|e| io(&format!("create {}", staging.display()), e))?;
    let file = staging.join(asset);
    std::fs::write(&file, archive).map_err(|e| io(&format!("write {}", file.display()), e))?;
    let tar = Command::new("tar")
        .arg("-xzf")
        .arg(&file)
        .arg("-C")
        .arg(&unpacked)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| io("run tar", e))?;
    if !tar.status.success() {
        return Err(format!(
            "Couldn’t unpack {asset}: {}. Nothing was installed.",
            String::from_utf8_lossy(&tar.stderr).trim()
        ));
    }
    let new = std::fs::read_dir(&unpacked)
        .map_err(|e| io("read the archive", e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("briardr-"))
        })
        .ok_or_else(|| format!("{asset} has no Briardr folder. Nothing was installed."))?;
    // Its first run also waits for macOS to check the new program.
    let reported = Command::new(new.join("bin").join("briardr"))
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if reported.as_deref() != Some(&format!("briardr {version}")) {
        return Err(format!(
            "The downloaded Briardr doesn’t run as Briardr {version} on this computer. Nothing was installed."
        ));
    }
    let old = dir.with_file_name(format!("{name}.old"));
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(dir, &old).map_err(|e| io(&format!("move {} aside", dir.display()), e))?;
    if let Err(e) = std::fs::rename(&new, dir) {
        let _ = std::fs::rename(&old, dir);
        return Err(io(&format!("put the new version in {}", dir.display()), e));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// What the server knows about updates, for Settings → About.
#[derive(Default)]
struct Known {
    latest: Option<String>,
    checked_at: Option<u64>,
    error: Option<String>,
}

/// The server's side: the last check, an install in progress, and the restart that follows one.
pub struct Updates {
    source: Source,
    supervisor: Option<&'static str>,
    known: Mutex<Known>,
    installing: AtomicBool,
    restart: tokio::sync::Notify,
}

impl Updates {
    pub fn new(env: &HashMap<String, String>) -> Self {
        Updates {
            source: Source::from_env(env),
            supervisor: supervisor(env),
            known: Mutex::new(Known::default()),
            installing: AtomicBool::new(false),
            restart: tokio::sync::Notify::new(),
        }
    }

    fn known(&self) -> std::sync::MutexGuard<'_, Known> {
        self.known.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether it's time to check again: never checked, the last check failed, or it's `every` ms old.
    pub fn due(&self, now: u64, every: u64) -> bool {
        let known = self.known();
        known.error.is_some()
            || known
                .checked_at
                .is_none_or(|at| now.saturating_sub(at) >= every)
    }

    /// Asks GitHub for the latest release and remembers the answer.
    pub async fn check(&self) -> Result<String, String> {
        let result = latest(&self.source).await;
        let mut known = self.known();
        known.checked_at = Some(crate::app::now_ms());
        match &result {
            Ok(v) => {
                known.latest = Some(v.clone());
                known.error = None;
            }
            Err(e) => known.error = Some(e.clone()),
        }
        result
    }

    /// The status Settings → About shows; `check` is the `updates.check` setting.
    pub fn status(&self, check: bool) -> Value {
        let known = self.known();
        let latest = known.latest.clone();
        let available = latest.as_deref().is_some_and(|l| is_newer(l, VERSION));
        let how = latest
            .as_deref()
            .filter(|_| available)
            .and_then(|l| how_to_update(&this_installation(), l));
        let (how, command) = how.map_or((None, None), |(how, command)| (Some(how), Some(command)));
        json!({
            "current": VERSION,
            "latest": latest,
            "available": available,
            "checkedAt": known.checked_at,
            "error": known.error,
            "check": check,
            "canInstall": matches!(this_installation(), Installation::Release(_)),
            "how": how,
            "command": command,
            "restarts": self.supervisor.is_some(),
            "installing": self.installing.load(Ordering::SeqCst),
            "notes": latest.as_deref().filter(|_| available).map(notes_url),
        })
    }

    /// Installs `version`, which must be the newer release the last check found. True when Briardr will
    /// restart on it by itself.
    pub async fn install(&self, version: &str) -> Result<bool, String> {
        let latest = self.known().latest.clone();
        if latest.as_deref() != Some(version) || !is_newer(version, VERSION) {
            return Err(format!(
                "Briardr {version} isn’t the newer release Briardr found. Check again."
            ));
        }
        let Installation::Release(dir) = this_installation() else {
            let (how, command) = how_to_update(&this_installation(), version).unwrap_or_default();
            return Err(format!("{how} {command}"));
        };
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err("An update is already being installed.".to_string());
        }
        let result = install(&self.source, version, &dir).await;
        self.installing.store(false, Ordering::SeqCst);
        result?;
        let restarts = self.supervisor.is_some();
        if restarts {
            self.restart.notify_one();
        }
        Ok(restarts)
    }

    /// Resolves when an update asks Briardr to restart (`briardr serve` then exits, and its supervisor
    /// starts the new version).
    pub async fn restart_requested(&self) {
        self.restart.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn versions_are_three_numbers() {
        assert_eq!(parse_version("0.3.5"), Some((0, 3, 5)));
        assert_eq!(parse_version("v10.0.12"), Some((10, 0, 12)));
        for bad in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1.2.3-rc.1",
            "1..2",
            " 1.2.3",
            "../1.2",
        ] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn newer_compares_numbers_not_text() {
        assert!(is_newer("0.3.10", "0.3.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.3.5", "0.3.5"));
        assert!(!is_newer("0.3.4", "0.3.5"));
        assert!(!is_newer("nonsense", "0.3.5"));
    }

    #[test]
    fn releases_come_from_github_or_the_test_folder() {
        let github = Source::from_env(&HashMap::new());
        assert_eq!(github.latest_url(), LATEST_API);
        assert_eq!(
            github.download_url("0.4.0", "SHA256SUMS"),
            "https://github.com/ChrisSku/briardr/releases/download/v0.4.0/SHA256SUMS"
        );
        let env = HashMap::from([(
            "BRIARDR_RELEASES_URL".to_string(),
            "http://127.0.0.1:1/r/".to_string(),
        )]);
        let local = Source::from_env(&env);
        assert_eq!(local.latest_url(), "http://127.0.0.1:1/r/latest");
        assert_eq!(
            local.download_url("0.4.0", "a.tar.gz"),
            "http://127.0.0.1:1/r/download/v0.4.0/a.tar.gz"
        );
    }

    #[test]
    fn supervisors_are_launchd_s_agent_and_systemd() {
        let env = |k: &str, v: &str| HashMap::from([(k.to_string(), v.to_string())]);
        assert_eq!(
            supervisor(&env("XPC_SERVICE_NAME", AGENT_LABEL)),
            Some("launchd")
        );
        // A terminal on macOS has XPC_SERVICE_NAME too, as 0 or the terminal app's.
        assert_eq!(supervisor(&env("XPC_SERVICE_NAME", "0")), None);
        assert_eq!(supervisor(&env("INVOCATION_ID", "4a1b")), Some("systemd"));
        assert_eq!(supervisor(&HashMap::new()), None);
    }

    #[test]
    fn only_a_release_installation_replaces_itself() {
        let dir = Scratch::new("update-kind");
        let release = dir.join("briardr");
        std::fs::create_dir_all(release.join("bin")).unwrap();
        std::fs::create_dir_all(release.join("web").join("dist")).unwrap();
        crate::scratch::write_executable(&release.join("bin").join("briardr"), "#!/bin/sh\n");
        let exe = std::fs::canonicalize(release.join("bin").join("briardr")).unwrap();
        assert_eq!(
            installation(&release, Some(&exe)),
            Installation::Release(release.clone())
        );
        // Another executable using this client (BRIARDR_PROJECT_ROOT) doesn't replace it.
        assert_eq!(
            installation(&release, Some(Path::new("/usr/local/bin/briardr"))),
            Installation::Other
        );
        assert_eq!(
            how_to_update(&Installation::Other, "0.4.0").unwrap().1,
            INSTALLER
        );

        std::fs::create_dir_all(release.join(".git")).unwrap();
        assert_eq!(
            installation(&release, Some(&exe)),
            Installation::Checkout(release.clone())
        );
        let (how, command) =
            how_to_update(&Installation::Checkout(release.clone()), "0.4.0").unwrap();
        assert!(how.contains("checkout"), "{how}");
        assert!(command.contains("git checkout v0.4.0"), "{command}");
    }

    /// A release archive as scripts/package.ts makes it, with a `briardr --version` that says `version`.
    fn archive(dir: &Path, version: &str) -> Vec<u8> {
        let root = dir.join("pkg").join(format!("briardr-{version}-test"));
        std::fs::create_dir_all(root.join("web").join("dist")).unwrap();
        std::fs::write(root.join("web").join("dist").join("index.html"), version).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        crate::scratch::write_executable(
            &root.join("bin").join("briardr"),
            &format!("#!/bin/sh\necho briardr {version}\n"),
        );
        let out = dir.join("a.tar.gz");
        let tar = Command::new("tar")
            .arg("-czf")
            .arg(&out)
            .arg("-C")
            .arg(dir.join("pkg"))
            .arg(format!("briardr-{version}-test"))
            .status()
            .unwrap();
        assert!(tar.success());
        let bytes = std::fs::read(&out).unwrap();
        std::fs::remove_dir_all(dir.join("pkg")).unwrap();
        std::fs::remove_file(out).unwrap();
        bytes
    }

    fn installed(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("web").join("dist").join("index.html")).unwrap()
    }

    #[test]
    fn a_release_replaces_the_installed_one_only_when_it_checks_out() {
        let scratch = Scratch::new("update-place");
        let dir = scratch.join("lib").join("briardr");
        std::fs::create_dir_all(dir.join("web").join("dist")).unwrap();
        std::fs::write(dir.join("web").join("dist").join("index.html"), "0.1.0").unwrap();
        let bytes = archive(&scratch, "0.2.0");
        let sums = format!(
            "{}  briardr-x.tar.gz\n",
            hex::encode(Sha256::digest(&bytes))
        );

        let err = place(b"tampered", &sums, "briardr-x.tar.gz", "0.2.0", &dir).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        let err = place(&bytes, &sums, "briardr-y.tar.gz", "0.2.0", &dir).unwrap_err();
        assert!(err.contains("doesn’t list"), "{err}");
        // The archive must be the version it was downloaded as.
        let err = place(&bytes, &sums, "briardr-x.tar.gz", "0.3.0", &dir).unwrap_err();
        assert!(err.contains("Briardr 0.3.0"), "{err}");
        assert_eq!(installed(&dir), "0.1.0");

        place(&bytes, &sums, "briardr-x.tar.gz", "0.2.0", &dir).unwrap();
        assert_eq!(installed(&dir), "0.2.0");
        let left: Vec<_> = std::fs::read_dir(scratch.join("lib"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("briardr")],
            "no staging or old folder left"
        );
    }
}
