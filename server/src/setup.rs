// `briardr setup`: checks what Briardr needs on this computer, helps with what's missing, and says what's
// next. Herdr is the one thing Briardr can't do without: when it isn't installed, setup offers to install
// the version this Briardr is tested with (deploy/versions.env, compiled in, checksum-verified) into
// ~/.local/bin. A Herdr of another version is left as it is, since replacing it can restart live agents.
// Git, Tailscale and Swift are only reported: each serves one optional part. install.sh runs it after
// installing Briardr, and `briardr serve` points here when Herdr is missing.
use std::fs::Permissions;
use std::io::{self, BufRead, IsTerminal, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::config::BriardrConfig;
use crate::herdr::rpc::{herdr_status, HerdrTarget};

pub const HELP: &str = r#"Usage: briardr setup [--yes]
Checks what Briardr needs on this computer and says what's next. When Herdr isn't installed, it offers
to install the version this Briardr is tested with into ~/.local/bin (--yes: without asking)."#;

const DOCS: &str = "https://github.com/ChrisSku/briardr/blob/main/docs";

/// deploy/versions.env.
const VERSIONS: &str = include_str!("../../deploy/versions.env");

fn pin(key: &str) -> Option<&'static str> {
    VERSIONS
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(str::trim)
}

/// The Herdr this Briardr is tested with, for an OS and CPU as Rust names them.
#[derive(Debug, PartialEq, Eq)]
pub struct HerdrRelease {
    pub version: &'static str,
    pub url: String,
    pub sha256: &'static str,
}

pub fn herdr_release(os: &str, arch: &str) -> Option<HerdrRelease> {
    if !matches!(os, "macos" | "linux") || !matches!(arch, "aarch64" | "x86_64") {
        return None;
    }
    let version = pin("HERDR_VERSION")?;
    let key = format!("HERDR_SHA256_{}_{}", os, arch).to_uppercase();
    Some(HerdrRelease {
        version,
        url: format!(
            "https://github.com/herdrdev/herdr/releases/download/v{version}/herdr-{os}-{arch}"
        ),
        sha256: pin(&key)?,
    })
}

/// How to start Herdr for a session.
fn start_herdr(session: &str) -> String {
    if session.is_empty() {
        "herdr".to_string()
    } else {
        format!("herdr --session {session}")
    }
}

/// What a Herdr problem (`herdr_status`) means and what to do about it, for `briardr serve` and setup.
pub fn herdr_hint(problem: &str, session: &str) -> String {
    match problem {
        "not_found" => "not installed: `briardr setup` installs it".to_string(),
        "not_running" => format!(
            "not running: start it in a terminal with `{}`",
            start_herdr(session)
        ),
        "incompatible" => {
            "a version this Briardr can't use: `briardr setup` says which one it's tested with"
                .to_string()
        }
        _ => "not reachable: check `herdr status`".to_string(),
    }
}

/// What setup found out about Herdr.
pub struct HerdrOutcome {
    /// The Herdr setup installed, whose path the config should name.
    pub installed: Option<PathBuf>,
    /// Herdr is installed and Briardr can use it (it may not be running yet).
    pub usable: bool,
}

struct Out {
    color: bool,
}

impl Out {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn step(&self, title: &str) {
        println!("\n{}", self.paint("1", &format!("==> {title}")));
    }
    fn ok(&self, text: &str) {
        println!("  {} {text}", self.paint("32", "✓"));
    }
    fn warn(&self, text: &str) {
        println!("  {} {text}", self.paint("33", "!"));
    }
    fn fail(&self, text: &str) {
        println!("  {} {text}", self.paint("31", "✗"));
    }
    fn info(&self, text: &str) {
        println!("  · {text}");
    }
}

fn out() -> Out {
    Out {
        color: io::stdout().is_terminal(),
    }
}

/// The version a herdr executable reports (`herdr --version` prints "herdr 0.9.3").
fn herdr_version(path: &str) -> Option<String> {
    let output = Command::new(path)
        .arg("--version")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
}

/// `~` for the home folder, so paths read shorter.
fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if home != Path::new("/") => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// Puts a downloaded Herdr in `dir` as `herdr`, after checking that it's the release's.
fn place_herdr(bytes: &[u8], release: &HerdrRelease, dir: &Path) -> Result<PathBuf, String> {
    let digest = hex::encode(Sha256::digest(bytes));
    if digest != release.sha256 {
        return Err(format!(
            "The download from {} isn't Herdr {}: its SHA-256 is {digest}, not {}. Nothing was installed.",
            release.url, release.version, release.sha256
        ));
    }
    let dest = dir.join("herdr");
    let tmp = dir.join(format!(".herdr-{}", std::process::id()));
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::set_permissions(&tmp, Permissions::from_mode(0o755)))
        .and_then(|()| std::fs::rename(&tmp, &dest))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Couldn’t write {}: {e}", dest.display())
        })?;
    Ok(dest)
}

async fn download(url: &str) -> Result<Vec<u8>, String> {
    let fail = |e: reqwest::Error| format!("Couldn’t download {url}: {e}");
    let client = crate::http::downloader(Duration::from_secs(300)).map_err(fail)?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(fail)?;
    Ok(response.bytes().await.map_err(fail)?.to_vec())
}

fn confirm(question: &str) -> Result<bool, String> {
    print!("  {question} [Y/n] ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        line.trim().to_lowercase().as_str(),
        "" | "y" | "yes"
    ))
}

pub fn intro() {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    println!(
        "Briardr {} setup on {os} ({})",
        crate::app::VERSION,
        std::env::consts::ARCH
    );
}

/// Herdr: there and usable, else installed when the person agrees (or `yes`).
pub async fn herdr(cfg: &BriardrConfig, home: &Path, yes: bool) -> Result<HerdrOutcome, String> {
    let out = out();
    out.step("Herdr");
    let release = herdr_release(std::env::consts::OS, std::env::consts::ARCH);
    let tested = release.as_ref().map_or("", |r| r.version);
    let mut target = HerdrTarget::of(cfg);
    let mut status = herdr_status(&target).await;
    let mut installed = None;
    if status.problem.as_deref() == Some("not_found") {
        let configured = Path::new(&cfg.herdr_path);
        let elsewhere = if crate::config::herdr_fallbacks(home)
            .iter()
            .any(|p| p == configured)
        {
            String::new()
        } else {
            format!(
                ", or at {}, where Briardr’s settings say it is",
                cfg.herdr_path
            )
        };
        out.fail(&format!(
            "Herdr isn’t installed: it isn’t on your PATH, in /opt/homebrew/bin, /usr/local/bin or ~/.local/bin{elsewhere}."
        ));
        let Some(release) = release else {
            out.info(&format!(
                "Install it with Herdr’s guide: https://herdr.dev/docs/install/ (this Briardr is tested with Herdr {}).",
                pin("HERDR_VERSION").unwrap_or("?")
            ));
            return Ok(HerdrOutcome {
                installed: None,
                usable: false,
            });
        };
        let dir = home.join(".local").join("bin");
        let question = format!(
            "Install Herdr {}, the version this Briardr is tested with, into {}?",
            release.version,
            tilde(&dir, home)
        );
        let agreed = if yes {
            true
        } else if io::stdin().is_terminal() {
            confirm(&question)?
        } else {
            out.info(&format!(
                "To install Herdr {}, run `briardr setup` in a terminal, or `briardr setup --yes`.",
                release.version
            ));
            false
        };
        if !agreed {
            out.info("Install it yourself with Herdr’s guide: https://herdr.dev/docs/install/");
            return Ok(HerdrOutcome {
                installed: None,
                usable: false,
            });
        }
        let bytes = download(&release.url).await?;
        let path = place_herdr(&bytes, &release, &dir)?;
        // Its first run also waits for macOS to check the new program, which `herdr status` below,
        // with its short timeout, shouldn't.
        let version = herdr_version(&path.display().to_string())
            .ok_or_else(|| format!("The Herdr installed at {} doesn’t run.", path.display()))?;
        out.ok(&format!(
            "Herdr {version} → {} (checksum verified)",
            tilde(&path, home)
        ));
        let on_path =
            std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == dir));
        if !on_path {
            out.info(&format!(
                "{} isn’t on your PATH: add it in your shell’s profile to run `herdr` by name.",
                tilde(&dir, home)
            ));
        }
        target.herdr_path = path.display().to_string();
        status = herdr_status(&target).await;
        installed = Some(path);
    } else {
        let version = herdr_version(&target.herdr_path).unwrap_or_else(|| "?".to_string());
        let found = format!("Herdr {version} at {}", target.herdr_path);
        if version == tested || tested.is_empty() {
            out.ok(&found);
        } else {
            out.warn(&format!(
                "{found}; this Briardr is tested with {tested}. Left as it is: replacing Herdr can restart live agents."
            ));
        }
    }
    let session = if cfg.herdr_session.is_empty() {
        String::new()
    } else {
        format!(" (session {})", cfg.herdr_session)
    };
    match status.problem.as_deref() {
        None => out.ok(&format!("Herdr is running{session}.")),
        Some("not_running") => out.warn(&format!(
            "Herdr isn’t running{session}. Start it in a terminal with `{}`; Briardr connects when it’s up.",
            start_herdr(&cfg.herdr_session)
        )),
        Some("incompatible") => {
            out.fail(&format!(
                "This Herdr doesn’t work with this Briardr. Install Herdr {tested} (https://github.com/herdrdev/herdr/releases/tag/v{tested}) when no agents are running: replacing Herdr restarts them."
            ));
            return Ok(HerdrOutcome {
                installed,
                usable: false,
            });
        }
        Some(_) => out.warn(&format!(
            "Herdr{session} didn’t answer `herdr status`. Check it in a terminal."
        )),
    }
    Ok(HerdrOutcome {
        installed,
        usable: true,
    })
}

/// The first line a command prints, if it runs.
fn first_line(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string(),
    )
}

/// What the optional parts need, reported only.
pub async fn optional() {
    let out = out();
    out.step("Optional");
    let macos = std::env::consts::OS == "macos";
    match first_line("git", &["--version"]) {
        Some(line) => out.ok(&format!(
            "Git {}, for the Changes tab",
            line.split_whitespace().nth(2).unwrap_or("")
        )),
        None => out.info(&format!(
            "Git isn’t installed. The Changes tab needs it ({}).",
            if macos {
                "xcode-select --install"
            } else {
                "sudo apt install git"
            }
        )),
    }
    let tailscale = tokio::task::spawn_blocking(crate::net::tailscale_info)
        .await
        .unwrap_or_default();
    match tailscale.dns_name {
        Some(name) => out.ok(&format!("Tailscale: this computer is {name} on your tailnet")),
        None => out.info(&format!(
            "Tailscale: no tailnet (or its `tailscale` command isn’t on your PATH). It’s one way for your devices to reach Briardr: {DOCS}/networking.md"
        )),
    }
    if macos {
        match first_line("swiftc", &["--version"]) {
            Some(_) => out.ok("Swift, for on-device dictation"),
            None => out.info(
                "Swift isn’t installed. On-device dictation needs it (xcode-select --install).",
            ),
        }
    }
}

/// What to do next.
pub fn next(configured: bool) {
    let out = out();
    out.step("Next");
    let macos = std::env::consts::OS == "macos";
    if !configured {
        if macos {
            out.info(&format!(
                "Choose how your devices reach this Mac ({DOCS}/macos-setup.md), e.g. over Tailscale: briardr configure --mode tailscale --listen tailscale"
            ));
        } else {
            out.info(&format!(
                "Choose how your devices reach this server: {DOCS}/linux-deployment.md"
            ));
        }
    }
    out.info(&format!(
        "Start Briardr: briardr serve{}",
        if macos {
            " (or at login: briardr install-agent)"
        } else {
            ""
        }
    ));
    out.info("Pair a device: briardr pair");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn herdr_releases_come_from_the_pins() {
        let mac = herdr_release("macos", "aarch64").unwrap();
        assert_eq!(mac.version, pin("HERDR_VERSION").unwrap());
        assert_eq!(
            mac.url,
            format!(
                "https://github.com/herdrdev/herdr/releases/download/v{}/herdr-macos-aarch64",
                mac.version
            )
        );
        for (os, arch) in [
            ("macos", "x86_64"),
            ("linux", "aarch64"),
            ("linux", "x86_64"),
        ] {
            let r = herdr_release(os, arch).unwrap();
            assert_eq!(r.sha256.len(), 64, "{os} {arch}");
            assert!(r.url.ends_with(&format!("/herdr-{os}-{arch}")));
        }
        assert_ne!(
            herdr_release("linux", "x86_64").unwrap().sha256,
            herdr_release("linux", "aarch64").unwrap().sha256
        );
        assert_eq!(herdr_release("windows", "x86_64"), None);
        assert_eq!(herdr_release("linux", "riscv64"), None);
    }

    #[test]
    fn a_pin_is_the_whole_key() {
        assert!(pin("HERDR").is_none());
        assert!(pin("HERDR_SHA256").is_none());
    }

    #[test]
    fn herdr_is_placed_only_when_its_checksum_matches() {
        let dir = Scratch::new("setup-herdr");
        let bin = dir.join("bin");
        let bytes = b"#!/bin/sh\necho herdr 0.0.1\n";
        let sha256 = hex::encode(Sha256::digest(bytes));
        let release = HerdrRelease {
            version: "0.0.1",
            url: "https://example.com/herdr".to_string(),
            sha256: Box::leak(sha256.into_boxed_str()),
        };
        let err = place_herdr(b"something else", &release, &bin).unwrap_err();
        assert!(err.contains("Nothing was installed"), "{err}");
        assert!(!bin.join("herdr").exists());

        let path = place_herdr(bytes, &release, &bin).unwrap();
        assert_eq!(path, bin.join("herdr"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(
            std::fs::read_dir(&bin).unwrap().count(),
            1,
            "no temp file left"
        );
    }

    #[test]
    fn herdr_version_reads_herdr_s_answer() {
        let dir = Scratch::new("setup-version");
        let herdr = dir.join("herdr");
        crate::scratch::write_executable(&herdr, "#!/bin/sh\necho herdr 0.9.3\n");
        assert_eq!(
            herdr_version(&herdr.display().to_string()).as_deref(),
            Some("0.9.3")
        );
        assert_eq!(
            herdr_version(&dir.join("missing").display().to_string()),
            None
        );
    }

    #[test]
    fn hints_say_what_to_do() {
        assert!(herdr_hint("not_found", "").contains("briardr setup"));
        assert!(herdr_hint("not_running", "").ends_with("`herdr`"));
        assert!(herdr_hint("not_running", "work").ends_with("`herdr --session work`"));
        assert!(herdr_hint("incompatible", "").contains("briardr setup"));
        assert!(herdr_hint("error", "").contains("herdr status"));
    }

    #[test]
    fn paths_in_home_read_with_a_tilde() {
        let home = Path::new("/Users/someone");
        assert_eq!(tilde(&home.join(".local/bin"), home), "~/.local/bin");
        assert_eq!(tilde(Path::new("/opt/bin"), home), "/opt/bin");
        assert_eq!(tilde(Path::new("/x"), Path::new("/")), "/x");
    }
}
