pub mod actions;
pub mod app;
pub mod auth;
pub mod config;
pub mod deploy;
pub mod diag;
pub mod dictation;
pub mod docs;
pub mod files;
pub mod git;
pub mod herdr;
pub mod http;
pub mod listener;
pub mod monitor;
pub mod net;
pub mod notes;
pub mod platform;
pub mod plugins;
pub mod power;
pub mod preview;
pub mod push;
pub mod serve;
pub mod setup;
pub mod static_files;
pub mod terminal_hub;
pub mod tls;
pub mod update;
pub mod uploads;
pub mod usage;

#[cfg(feature = "test-hooks")]
pub mod test_hooks;

#[cfg(test)]
mod scratch;

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::signal::unix::{signal, SignalKind};

use crate::app::{caps_json, js_number};
use crate::auth::Auth;
use crate::config::{
    default_data_dir, load_config, parse_public_origin, save_config, validate_config,
    BriardrConfig, ConfigError, Paths,
};
use crate::deploy::{caddyfile, systemd_units, UnitOptions};
use crate::herdr::rpc::{herdr_status, HerdrTarget};
use crate::net::{missing_local_addresses, tailscale_info, valid_network, TailscaleInfo};
use crate::platform::{capabilities, host_platform};
use crate::serve::ServeError;

const USAGE: &str = "Usage: briardr [serve|setup|pair|devices|revoke <id>|status|configure|caddyfile|systemd-units <dir>|install-agent|uninstall-agent|install-power|install-cli|plugin <command>|create plugin|skill [install|print]|update [--check]|--version]";

const CONFIGURE_HELP: &str = r#"Usage: briardr configure [options]
  --mode proxy|local-ca|tailscale|files   how HTTPS is provided (proxy: a reverse proxy on this host)
  --public-origin URL                     the URL devices open, e.g. https://briardr.example.com
  --preview-origin URL                    proxy mode: preview slot origins with {n}, e.g. https://p{n}.briardr.example.com
                                          ("" turns previews off in proxy mode)
  --listen ADDR[,ADDR]                    direct modes: addresses to listen on besides loopback (IPv4, or
                                          "tailscale"; "" for loopback only), e.g. 10.8.0.1 for WireGuard
  --allowed-networks CIDR[,CIDR]          direct modes: IPv4 networks whose devices may connect, e.g. 10.8.0.0/24
                                          ("" for none; loopback and Tailscale are always allowed)
  --herdr-path PATH                       absolute path to the herdr executable
  --port N                                Briardr's port (proxy mode: the loopback HTTP port)
  --dry-run                               validate and print the result without saving
Changes are validated first; the previous config.json is kept as config.json.bak."#;

/// Times as `toLocaleTimeString()` and `toLocaleString()` show them in en-US, via `date`.
const TIME_FORMAT: &str = "%-I:%M:%S %p";
const DATE_TIME_FORMAT: &str = "%-m/%-d/%Y, %-I:%M:%S %p";

#[derive(Debug)]
enum CliError {
    Config(String),
    Listen(String),
    Other(String),
}

type CliResult = Result<i32, CliError>;

impl From<ConfigError> for CliError {
    fn from(e: ConfigError) -> Self {
        match e {
            ConfigError::Invalid(message) => CliError::Config(message),
            other => CliError::Other(other.to_string()),
        }
    }
}

impl From<ServeError> for CliError {
    fn from(e: ServeError) -> Self {
        match e {
            ServeError::Config(e) => e.into(),
            ServeError::Listen(e) => CliError::Listen(e.0),
            ServeError::Other(message) => CliError::Other(message),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Other(e.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        CliError::Other(e.to_string())
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let code = match cli(args).await {
        Ok(code) => code,
        Err(CliError::Config(message)) => {
            eprintln!("Briardr: {message}");
            78
        }
        Err(CliError::Listen(message)) => {
            eprintln!("Briardr: {message}");
            75
        }
        Err(CliError::Other(message)) => {
            eprintln!("Briardr: {message}");
            1
        }
    };
    std::process::exit(code);
}

struct Ctx {
    paths: Paths,
    env: HashMap<String, String>,
    home: PathBuf,
}

async fn cli(args: Vec<String>) -> CliResult {
    if matches!(
        args.first().map(String::as_str),
        Some("--version" | "-V" | "version")
    ) {
        println!("briardr {}", crate::app::VERSION);
        return Ok(0);
    }
    let env: HashMap<String, String> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let home = PathBuf::from(env_value(&env, "HOME").unwrap_or_else(|| "/".to_string()));
    // The data dir follows the real OS (`process.platform`), never the BRIARDR_PLATFORM override.
    let os = if is_macos() {
        "darwin"
    } else {
        std::env::consts::OS
    };
    let root = default_data_dir(os, &env, &home);
    let ctx = Ctx {
        paths: Paths::new(root),
        env,
        home,
    };
    let paths = ctx.paths.clone();
    blocking(move || paths.ensure()).await??;

    let cmd = args
        .first()
        .map(String::as_str)
        .filter(|c| !c.is_empty())
        .unwrap_or("serve");
    let rest = args.get(1..).unwrap_or_default();
    match cmd {
        "serve" => serve_command(&ctx).await,
        "setup" => setup_command(&ctx, rest).await,
        "pair" => pair(&ctx).await,
        "devices" => devices(&ctx).await,
        "revoke" => revoke(&ctx, rest).await,
        "status" => status(&ctx).await,
        "configure" => configure(&ctx, rest).await,
        "caddyfile" => caddyfile_command(&ctx, rest).await,
        "systemd-units" => systemd_units_command(&ctx, rest).await,
        "install-agent" => install_agent(&ctx).await,
        "install-power" => install_power().await,
        "uninstall-agent" => uninstall_agent(&ctx).await,
        "plugin" | "plugins" => plugin_command(&ctx, rest.to_vec()).await,
        "update" => update_command(&ctx, rest).await,
        // `briardr create plugin …` reads better than `briardr plugin create …`; both work.
        "create" if rest.first().map(String::as_str) == Some("plugin") => {
            let mut args = vec!["create".to_string()];
            args.extend(rest[1..].iter().cloned());
            plugin_command(&ctx, args).await
        }
        "skill" => {
            let args = rest.to_vec();
            let env = plugin_env(&ctx);
            blocking(move || crate::plugins::create::skill_command(&env, &args))
                .await?
                .map_err(CliError::Other)
        }
        "install-cli" => {
            let env = plugin_env(&ctx);
            blocking(move || crate::plugins::create::install_cli(&env))
                .await?
                .map_err(CliError::Other)
        }
        _ => {
            println!("{USAGE}");
            Ok(2)
        }
    }
}

fn plugin_env(ctx: &Ctx) -> crate::plugins::cli::Env {
    crate::plugins::cli::Env {
        paths: ctx.paths.clone(),
        root: project_root(),
        web_dir: web_dist(&ctx.env),
        home: ctx.home.clone(),
    }
}

async fn plugin_command(ctx: &Ctx, args: Vec<String>) -> CliResult {
    let env = plugin_env(ctx);
    blocking(move || crate::plugins::cli::run(&env, &args))
        .await?
        .map_err(CliError::Other)
}

async fn serve_command(ctx: &Ctx) -> CliResult {
    let dist = web_dist(&ctx.env);
    if tokio::fs::metadata(dist.join("index.html")).await.is_err() {
        return Err(CliError::Config(format!(
            "The web client isn’t built ({}/index.html is missing). Run `mise run build` in {}.",
            dist.display(),
            project_root().display()
        )));
    }
    let running = serve::serve(ctx.paths.clone(), false).await?;
    let updates = running.app.d.updates.clone();
    tokio::select! {
        stopped = wait_for_stop_signal() => stopped?,
        // Installed from the app: launchd or systemd starts the new version once this one exits.
        () = updates.restart_requested() => println!("Restarting on the new version."),
    }
    running.close().await;
    Ok(0)
}

async fn wait_for_stop_signal() -> std::io::Result<()> {
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    Ok(())
}

async fn pair(ctx: &Ctx) -> CliResult {
    let cfg = load_valid_config(ctx).await?;
    let paths = ctx.paths.clone();
    let (code, expires_at) =
        blocking(move || Auth::new(paths.devices(), paths.pairing()).create_pairing_code()).await?;
    let dns_name = if cfg.tls.mode == "proxy" {
        None
    } else {
        tailscale().await?.dns_name
    };
    let origin = public_origin_of(&cfg, dns_name.as_deref())?;
    let time = local_time(expires_at, TIME_FORMAT).await;
    println!("\n  Pairing code:  {code}\n");
    println!("  Open {origin} on the new device and enter it.");
    println!("  It works once and expires at {time}.\n");
    Ok(0)
}

async fn devices(ctx: &Ctx) -> CliResult {
    let paths = ctx.paths.clone();
    let devices = blocking(move || Auth::new(paths.devices(), paths.pairing()).list()).await?;
    for d in devices {
        let paired = local_time(d.created_at, DATE_TIME_FORMAT).await;
        let seen = local_time(d.last_seen_at.unwrap_or(0), DATE_TIME_FORMAT).await;
        println!("{}  {}  paired {paired}  last seen {seen}", d.id, d.name);
    }
    Ok(0)
}

async fn revoke(ctx: &Ctx, rest: &[String]) -> CliResult {
    let id = rest.first().cloned().unwrap_or_default();
    let paths = ctx.paths.clone();
    let removed =
        blocking(move || Auth::new(paths.devices(), paths.pairing()).revoke(&id)).await??;
    println!(
        "{}",
        if removed {
            "Revoked."
        } else {
            "No such device."
        }
    );
    Ok(0)
}

async fn status(ctx: &Ctx) -> CliResult {
    let mut cfg = load_config_only(ctx).await?;
    let problem = validate_config(&mut cfg).err().map(config_message);
    let st = herdr_status(&HerdrTarget::of(&cfg)).await;
    let proxy = cfg.tls.mode == "proxy";
    let ts = if proxy {
        TailscaleInfo::default()
    } else {
        tailscale().await?
    };
    let caps = capabilities(host_platform(&host_platform_name(&ctx.env)));
    let caps_value = caps_json(&caps);
    let public_origin = if problem.is_some() {
        None
    } else {
        Some(public_origin_of(&cfg, ts.dns_name.as_deref())?)
    };
    let preview_origins = if proxy {
        json!(cfg
            .public_preview_origin
            .as_deref()
            .filter(|o| !o.is_empty()))
    } else {
        let last = i64::from(cfg.preview_port_start) + i64::from(cfg.preview_port_count) - 1;
        json!(format!("ports {}-{last}", cfg.preview_port_start))
    };
    let listen_missing = if proxy {
        Vec::new()
    } else {
        let ips: Vec<String> = cfg
            .listen
            .iter()
            .filter(|a| is_ip_literal(a))
            .cloned()
            .collect();
        missing_local_addresses(&ips)
    };
    let web_built = tokio::fs::metadata(web_dist(&ctx.env).join("index.html"))
        .await
        .is_ok();
    let doc = json!({
        "dataDir": ctx.paths.root.display().to_string(),
        "platform": caps_value["platform"],
        "capabilities": caps_value,
        "mode": cfg.tls.mode,
        "publicOrigin": public_origin,
        "previewOrigins": preview_origins,
        "configProblem": problem,
        "herdrPath": cfg.herdr_path,
        "herdrSession": cfg.herdr_session,
        "herdr": {
            "ok": st.ok,
            "running": st.running,
            "version": st.version,
            "socket": st.socket.map(|p| p.display().to_string()),
            "problem": st.problem,
        },
        "tailscale": { "dnsName": ts.dns_name, "ips": ts.ips },
        "port": cfg.port,
        "listen": if proxy { json!(["127.0.0.1 (http, behind the proxy)"]) } else { json!(cfg.listen) },
        "listenMissing": listen_missing,
        "allowedNetworks": if proxy { json!([]) } else { json!(cfg.allowed_networks) },
        "tls": cfg.tls.mode,
        "webBuilt": web_built,
    });
    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(i32::from(problem.is_some()))
}

async fn configure(ctx: &Ctx, rest: &[String]) -> CliResult {
    if rest.iter().any(|a| a == "--help") || rest.is_empty() {
        println!("{CONFIGURE_HELP}");
        return Ok(if rest.is_empty() { 2 } else { 0 });
    }
    let before = load_config_only(ctx).await?;
    let (cfg, dry_run) = apply_configure(&before, rest)?;
    if dry_run {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
        return Ok(0);
    }
    let file = ctx.paths.config();
    let backup = backup_path(&file);
    let exists = tokio::fs::try_exists(&file).await.unwrap_or(false);
    if exists && serde_json::to_string(&cfg)? == serde_json::to_string(&before)? {
        println!("Unchanged: {}", file.display());
        return Ok(0);
    }
    if exists {
        tokio::fs::copy(&file, &backup).await?;
    }
    let (paths, to_save) = (ctx.paths.clone(), cfg.clone());
    blocking(move || save_config(&paths, &to_save)).await??;
    let previous = if tokio::fs::try_exists(&backup).await.unwrap_or(false) {
        format!(" (previous: {})", backup.display())
    } else {
        String::new()
    };
    println!(
        "Saved {}{previous}. Restart Briardr to apply.",
        file.display()
    );
    Ok(0)
}

async fn setup_command(ctx: &Ctx, rest: &[String]) -> CliResult {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", setup::HELP);
        return Ok(0);
    }
    let cfg = load_config_only(ctx).await?;
    let configured = tokio::fs::try_exists(ctx.paths.config())
        .await
        .unwrap_or(false);
    setup::intro();
    let yes = rest.iter().any(|a| a == "--yes");
    let herdr = setup::herdr(&cfg, &ctx.home, yes)
        .await
        .map_err(CliError::Other)?;
    // Without a config, the default finds Herdr in ~/.local/bin.
    if let Some(path) = herdr.installed.filter(|_| configured) {
        configure(
            ctx,
            &["--herdr-path".to_string(), path.display().to_string()],
        )
        .await?;
    }
    setup::optional().await;
    setup::next(configured);
    Ok(i32::from(!herdr.usable))
}

async fn caddyfile_command(ctx: &Ctx, rest: &[String]) -> CliResult {
    let cfg = load_valid_config(ctx).await?;
    if cfg.tls.mode != "proxy" {
        return Err(CliError::Config(
            "The Caddyfile is for proxy mode. Run `briardr configure --mode proxy --public-origin https://…` first."
                .to_string(),
        ));
    }
    let internal_tls = rest.iter().any(|a| a == "--internal-tls");
    let text = caddyfile(&cfg, internal_tls).map_err(CliError::Other)?;
    let mut out = tokio::io::stdout();
    out.write_all(text.as_bytes()).await?;
    out.flush().await?;
    Ok(0)
}

async fn systemd_units_command(ctx: &Ctx, rest: &[String]) -> CliResult {
    let out = match rest.first() {
        Some(dir) if !dir.is_empty() && !dir.starts_with('-') => PathBuf::from(dir),
        _ => {
            return Err(CliError::Config(
                "Usage: briardr systemd-units <output-directory> [--network-unit UNIT]…"
                    .to_string(),
            ))
        }
    };
    let network: Vec<String> = rest
        .iter()
        .enumerate()
        .filter(|(i, _)| *i > 0 && rest[i - 1] == "--network-unit")
        .map(|(_, a)| a.clone())
        .collect();
    let cfg = load_valid_config(ctx).await?;
    let env = &ctx.env;
    let user = id_output("-un")
        .await
        .or_else(|| env_value(env, "USER"))
        .or_else(|| env_value(env, "LOGNAME"))
        .unwrap_or_default();
    let group = group_name(&user).await;
    // The service runs this binary (setup-linux.sh builds it in the checkout).
    let binary = tokio::fs::canonicalize(std::env::current_exe()?)
        .await?
        .display()
        .to_string();
    let herdr = tokio::fs::canonicalize(&cfg.herdr_path)
        .await?
        .display()
        .to_string();
    let units = systemd_units(&UnitOptions {
        user,
        group,
        home: ctx.home.display().to_string(),
        checkout: project_root().display().to_string(),
        binary,
        herdr,
        herdr_session: cfg.herdr_session.clone(),
        data_dir: ctx.paths.root.display().to_string(),
        path: env_value(env, "BRIARDR_SERVICE_PATH")
            .or_else(|| env_value(env, "PATH"))
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".to_string()),
        network,
    })
    .map_err(CliError::Other)?;
    tokio::fs::create_dir_all(&out).await?;
    let mut written = Vec::new();
    for (name, text) in &units {
        let file = out.join(name);
        tokio::fs::write(&file, text).await?;
        written.push(file.display().to_string());
    }
    println!("Wrote {}.", written.join(" and "));
    Ok(0)
}

/// `briardr update`: the latest release in place of this one, then the LaunchAgent restarted on it.
async fn update_command(ctx: &Ctx, rest: &[String]) -> CliResult {
    let (mut check, mut yes) = (false, false);
    for arg in rest {
        match arg.as_str() {
            "--check" => check = true,
            "--yes" | "-y" => yes = true,
            _ => {
                println!("{}", update::HELP);
                return Ok(if arg == "--help" || arg == "-h" { 0 } else { 2 });
            }
        }
    }
    let source = update::Source::from_env(&ctx.env);
    let latest = update::latest(&source).await.map_err(CliError::Other)?;
    let current = crate::app::VERSION;
    if !update::is_newer(&latest, current) {
        println!("Briardr {current} is the latest release.");
        return Ok(0);
    }
    println!(
        "Briardr {latest} is available; this is {current}. What’s new: {}",
        update::notes_url(&latest)
    );
    if check {
        return Ok(0);
    }
    let installation = update::this_installation();
    let update::Installation::Release(dir) = &installation else {
        let (how, command) = update::how_to_update(&installation, &latest).unwrap_or_default();
        println!("{how}\n  {command}");
        return Ok(1);
    };
    if !yes {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            println!("Run `briardr update --yes` to install it.");
            return Ok(1);
        }
        print!("Install Briardr {latest} in {}? [Y/n] ", dir.display());
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "" | "y" | "yes") {
            return Ok(1);
        }
    }
    println!("Downloading Briardr {latest}…");
    update::install(&source, &latest, dir)
        .await
        .map_err(CliError::Other)?;
    println!(
        "Installed Briardr {latest} in {} (checksum verified).",
        dir.display()
    );
    if is_macos() && agent_runs(&launch_agent_path(&ctx.home), dir, &ctx.home) {
        let uid = tokio::process::Command::new("id")
            .arg("-u")
            .output()
            .await?;
        let target = format!(
            "gui/{}/{}",
            String::from_utf8_lossy(&uid.stdout).trim(),
            update::AGENT_LABEL
        );
        let loaded = tokio::process::Command::new("launchctl")
            .args(["print", &target])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success());
        if loaded {
            let restarted = tokio::process::Command::new("launchctl")
                .args(["kickstart", "-k", &target])
                .status()
                .await
                .is_ok_and(|s| s.success());
            if restarted {
                println!("Restarted Briardr on the new version. Herdr and your agents keep running; devices reconnect.");
                return Ok(0);
            }
        }
    }
    println!("Restart Briardr to run the new version. Herdr and your agents keep running.");
    Ok(0)
}

/// Whether the LaunchAgent at `plist` runs the Briardr installed in `dir`, directly or through the link
/// `briardr install-cli` makes (as install.sh checks).
fn agent_runs(plist: &Path, dir: &Path, home: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(plist) else {
        return false;
    };
    let exe = dir.join("bin").join("briardr");
    let link = home.join(".local").join("bin").join("briardr");
    text.contains(&*plist_escape(&exe.to_string_lossy()))
        || (text.contains(&*plist_escape(&link.to_string_lossy()))
            && std::fs::canonicalize(&link).ok() == std::fs::canonicalize(&exe).ok())
}

async fn install_agent(ctx: &Ctx) -> CliResult {
    if !is_macos() {
        return Err(CliError::Config(
            "install-agent writes a macOS LaunchAgent. On Linux, use `briardr systemd-units` (scripts/setup-linux.sh does this for you)."
                .to_string(),
        ));
    }
    let dest = launch_agent_path(&ctx.home);
    let exe = std::env::current_exe()?;
    let path_env = env_value(&ctx.env, "PATH").unwrap_or_else(|| "/usr/bin:/bin".to_string());
    let text = plist_text(&exe.to_string_lossy(), &ctx.paths.logs(), &path_env);
    tokio::fs::write(&dest, text).await?;
    println!(
        "Wrote {}\nStart it with:  launchctl bootstrap gui/$(id -u) {}",
        dest.display(),
        dest.display()
    );
    Ok(0)
}

/// Lets Always on keep the Mac awake with the lid closed: scripts/install-power.sh (compiled in, so a
/// release runs it too) installs a sudoers rule for `pmset disablesleep` and nothing else.
async fn install_power() -> CliResult {
    if !is_macos() {
        return Err(CliError::Config(
            "install-power is for Always on, on a Mac.".to_string(),
        ));
    }
    let status = tokio::process::Command::new("/bin/sh")
        .args(["-c", include_str!("../../scripts/install-power.sh")])
        .status()
        .await?;
    Ok(status.code().unwrap_or(1))
}

async fn uninstall_agent(ctx: &Ctx) -> CliResult {
    let dest = launch_agent_path(&ctx.home);
    println!(
        "Stop it with:  launchctl bootout gui/$(id -u) {}",
        dest.display()
    );
    match tokio::fs::remove_file(&dest).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(0)
}

/// Runs blocking config and file work off the async runtime.
async fn blocking<T, F>(f: F) -> Result<T, CliError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CliError::Other(e.to_string()))
}

async fn load_config_only(ctx: &Ctx) -> Result<BriardrConfig, CliError> {
    let (paths, env) = (ctx.paths.clone(), ctx.env.clone());
    let cfg = blocking(move || load_config(&paths, &env)).await??;
    Ok(cfg)
}

async fn load_valid_config(ctx: &Ctx) -> Result<BriardrConfig, CliError> {
    let (paths, env) = (ctx.paths.clone(), ctx.env.clone());
    let cfg = blocking(move || -> Result<BriardrConfig, ConfigError> {
        let mut cfg = load_config(&paths, &env)?;
        validate_config(&mut cfg)?;
        Ok(cfg)
    })
    .await??;
    Ok(cfg)
}

async fn tailscale() -> Result<TailscaleInfo, CliError> {
    blocking(tailscale_info).await
}

fn config_message(e: ConfigError) -> String {
    match e {
        ConfigError::Invalid(message) => message,
        other => other.to_string(),
    }
}

fn env_value(env: &HashMap<String, String>, key: &str) -> Option<String> {
    env.get(key).filter(|v| !v.is_empty()).cloned()
}

fn is_macos() -> bool {
    std::env::consts::OS == "macos"
}

/// The `BRIARDR_PLATFORM` override, else the OS in the names `default_data_dir` and `host_platform` use.
fn host_platform_name(env: &HashMap<String, String>) -> String {
    env_value(env, "BRIARDR_PLATFORM").unwrap_or_else(|| {
        if is_macos() {
            "darwin".to_string()
        } else {
            std::env::consts::OS.to_string()
        }
    })
}

/// The web client to serve: `BRIARDR_WEB_DIST` (another build, for benchmarks and experiments, without
/// touching web/dist), else web/dist in the project root.
pub fn web_dist(env: &HashMap<String, String>) -> PathBuf {
    match env_value(env, "BRIARDR_WEB_DIST") {
        Some(dir) => std::path::absolute(&dir).unwrap_or_else(|_| PathBuf::from(dir)),
        None => project_root().join("web").join("dist"),
    }
}

/// The Briardr checkout or release: `BRIARDR_PROJECT_ROOT`, else where the real executable is (see
/// `project_root_from_exe`), else the current directory.
pub fn project_root() -> PathBuf {
    if let Some(root) = std::env::var_os("BRIARDR_PROJECT_ROOT").filter(|r| !r.is_empty()) {
        return PathBuf::from(root);
    }
    // Through the link `briardr install-cli` makes, too.
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) {
        if let Some(root) = project_root_from_exe(&exe) {
            return root;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// A checkout's executable is in its `server/target`; a release's (scripts/package.ts) is `bin/briardr`,
/// next to the client in `web/dist`.
fn project_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let text = exe.to_string_lossy();
    if let Some(i) = text.rfind("/server/target/") {
        return Some(PathBuf::from(&text[..i]));
    }
    let root = exe.parent().filter(|dir| dir.ends_with("bin"))?.parent()?;
    root.join("web")
        .join("dist")
        .is_dir()
        .then(|| root.to_path_buf())
}

fn is_ipv4_literal(s: &str) -> bool {
    s.parse::<Ipv4Addr>().is_ok()
}

/// Node's `net.isIP`: an IPv4 address, or an IPv6 address with an optional zone.
fn is_ip_literal(s: &str) -> bool {
    is_ipv4_literal(s)
        || s.split_once('%')
            .map_or(s, |(addr, _)| addr)
            .parse::<Ipv6Addr>()
            .is_ok()
}

/// The one URL devices open: publicOrigin when configured (always in proxy mode), else derived from
/// the Tailscale name or the first listen address.
pub fn public_origin_of(
    cfg: &BriardrConfig,
    dns_name: Option<&str>,
) -> Result<String, ConfigError> {
    if let Some(origin) = cfg.public_origin.as_deref().filter(|o| !o.is_empty()) {
        return parse_public_origin(origin, "publicOrigin");
    }
    let remote = if cfg.listen.iter().any(|l| l == "tailscale") {
        dns_name.filter(|d| !d.is_empty())
    } else {
        cfg.listen
            .iter()
            .find(|a| is_ip_literal(a))
            .map(String::as_str)
    };
    let port = if cfg.port == 443 {
        String::new()
    } else {
        format!(":{}", cfg.port)
    };
    Ok(match remote {
        Some(host) if host.contains(':') => format!("https://[{host}]{port}"),
        Some(host) => format!("https://{host}{port}"),
        None => format!("https://localhost:{}", cfg.port),
    })
}

fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, ConfigError> {
    let value = args
        .get(*i)
        .cloned()
        .ok_or_else(|| ConfigError::Invalid(format!("{flag} needs a value.")))?;
    *i += 1;
    Ok(value)
}

/// A comma-separated list: trimmed, empty items dropped.
fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

fn check_listen(entry: String) -> Result<String, ConfigError> {
    if entry != "tailscale" && (!is_ipv4_literal(&entry) || entry == "0.0.0.0") {
        let quoted = serde_json::to_string(&entry).unwrap_or_default();
        return Err(ConfigError::Invalid(format!(
            "--listen takes IPv4 addresses of this host or \"tailscale\" (got {quoted}); 0.0.0.0 isn't allowed."
        )));
    }
    Ok(entry)
}

fn check_network(entry: String) -> Result<String, ConfigError> {
    if valid_network(&entry).is_none() {
        let quoted = serde_json::to_string(&entry).unwrap_or_default();
        return Err(ConfigError::Invalid(format!(
            "--allowed-networks takes IPv4 addresses or CIDRs such as 10.8.0.0/24 (got {quoted}); 0.0.0.0/0 isn't allowed."
        )));
    }
    Ok(entry)
}

/// Apply `briardr configure` options to a copy of `cfg`. Returns the new config and whether `--dry-run` was given.
pub fn apply_configure(
    cfg: &BriardrConfig,
    args: &[String],
) -> Result<(BriardrConfig, bool), ConfigError> {
    let mut next = cfg.clone();
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        match a {
            "--dry-run" => dry_run = true,
            "--mode" => next.tls.mode = take_value(args, &mut i, a)?,
            "--public-origin" => next.public_origin = Some(take_value(args, &mut i, a)?),
            "--preview-origin" => next.public_preview_origin = Some(take_value(args, &mut i, a)?),
            "--listen" => {
                next.listen = list(&take_value(args, &mut i, a)?)
                    .into_iter()
                    .map(check_listen)
                    .collect::<Result<Vec<_>, _>>()?
            }
            "--allowed-networks" => {
                next.allowed_networks = list(&take_value(args, &mut i, a)?)
                    .into_iter()
                    .map(check_network)
                    .collect::<Result<Vec<_>, _>>()?
            }
            "--herdr-path" => {
                let p = take_value(args, &mut i, a)?;
                if !Path::new(&p).is_absolute() {
                    return Err(ConfigError::Invalid(
                        "--herdr-path must be absolute.".to_string(),
                    ));
                }
                next.herdr_path = p;
            }
            "--port" => {
                let n = js_number(&take_value(args, &mut i, a)?);
                if !(n.fract() == 0.0 && (1.0..=65535.0).contains(&n)) {
                    return Err(ConfigError::Invalid(
                        "--port must be a port number.".to_string(),
                    ));
                }
                next.port = n as u16;
            }
            _ => {
                return Err(ConfigError::Invalid(format!(
                    "Unknown option {a}.\n{CONFIGURE_HELP}"
                )))
            }
        }
    }
    validate_config(&mut next)?;
    Ok((next, dry_run))
}

fn backup_path(file: &Path) -> PathBuf {
    let mut backup = file.as_os_str().to_owned();
    backup.push(".bak");
    PathBuf::from(backup)
}

fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join("app.briardr.server.plist")
}

fn plist_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;")
}

/// The LaunchAgent that runs `exe serve` at login, with logs in `logs`.
fn plist_text(exe: &str, logs: &Path, path_env: &str) -> String {
    let exe = plist_escape(exe);
    let path_env = plist_escape(path_env);
    let stdout = plist_escape(&logs.join("stdout.log").display().to_string());
    let stderr = plist_escape(&logs.join("stderr.log").display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>app.briardr.server</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>serve</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>{path_env}</string></dict>
  <key>StandardOutPath</key><string>{stdout}</string>
  <key>StandardErrorPath</key><string>{stderr}</string>
</dict></plist>
"#
    )
}

/// Local time from epoch milliseconds, formatted by `date`.
async fn local_time(ms: u64, format: &str) -> String {
    let secs = (ms / 1000).to_string();
    let mut cmd = tokio::process::Command::new("date");
    if is_macos() {
        cmd.args(["-r", &secs]);
    } else {
        cmd.args(["-d", &format!("@{secs}")]);
    }
    cmd.arg(format!("+{format}"));
    match cmd.output().await {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => ms.to_string(),
    }
}

async fn id_output(flag: &str) -> Option<String> {
    let out = tokio::process::Command::new("id")
        .arg(flag)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The primary group name from /etc/group, else the user name.
async fn group_name(user: &str) -> String {
    let gid = id_output("-g").await.unwrap_or_default();
    let text = tokio::fs::read_to_string("/etc/group")
        .await
        .unwrap_or_default();
    text.lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .find(|f| !gid.is_empty() && f.len() > 2 && f[2] == gid.as_str())
        .map(|f| f[0].to_string())
        .unwrap_or_else(|| user.to_string())
}

#[cfg(test)]
mod main_tests {
    use super::*;
    use crate::config::default_config;
    use crate::scratch::Scratch;

    const ORIGIN: &str = "https://briardr.example.com";
    const PREVIEW: &str = "https://p{n}.briardr.example.com";

    fn base() -> BriardrConfig {
        default_config(&HashMap::new(), Path::new("/home/briardr"))
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn apply_err(list: &[&str]) -> String {
        apply_configure(&base(), &args(list))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn proxy_mode_needs_a_public_origin() {
        assert!(apply_err(&["--mode", "proxy"]).contains("publicOrigin"));
    }

    #[test]
    fn herdr_path_must_be_absolute() {
        assert!(apply_err(&["--herdr-path", "herdr"]).contains("absolute"));
    }

    #[test]
    fn unknown_options_print_the_help() {
        let msg = apply_err(&["--bogus"]);
        assert!(msg.contains("Unknown option --bogus."));
        assert!(msg.contains(CONFIGURE_HELP));
    }

    #[test]
    fn flags_need_values() {
        assert!(apply_err(&["--listen"]).contains("--listen needs a value."));
    }

    #[test]
    fn listen_takes_ipv4_or_tailscale() {
        for bad in [
            "0.0.0.0",
            "::",
            "fd00::1",
            "wg0",
            "10.8.0.0/24",
            "010.8.0.1",
        ] {
            assert!(
                apply_configure(&base(), &args(&["--listen", bad])).is_err(),
                "{bad}"
            );
        }
        let (cfg, _) =
            apply_configure(&base(), &args(&["--listen", "10.8.0.1, tailscale"])).expect("listen");
        assert_eq!(cfg.listen, ["10.8.0.1", "tailscale"]);
    }

    #[test]
    fn listen_and_allowed_networks_can_be_cleared() {
        let (cfg, _) = apply_configure(&base(), &args(&["--listen", "", "--allowed-networks", ""]))
            .expect("cleared");
        assert!(cfg.listen.is_empty());
        assert!(cfg.allowed_networks.is_empty());
    }

    #[test]
    fn allowed_networks_reject_open_and_malformed_entries() {
        for bad in [
            "0.0.0.0/0",
            "0.0.0.0",
            "10.8.0.0/0",
            "10.8.0.0/33",
            "10.8.0.0/24/",
            "fd00::/64",
            "bad",
        ] {
            assert!(
                apply_configure(&base(), &args(&["--allowed-networks", bad])).is_err(),
                "{bad}"
            );
        }
        let (cfg, _) = apply_configure(&base(), &args(&["--allowed-networks", "10.8.0.0/24"]))
            .expect("network");
        assert_eq!(cfg.allowed_networks, ["10.8.0.0/24"]);
    }

    #[test]
    fn port_must_be_a_port_number() {
        for bad in ["0", "65536", "7443.5", "abc", ""] {
            assert!(
                apply_err(&["--port", bad]).contains("--port must be a port number."),
                "{bad}"
            );
        }
        let (cfg, _) = apply_configure(&base(), &args(&["--port", "8443"])).expect("port");
        assert_eq!(cfg.port, 8443);
    }

    #[test]
    fn proxy_setup_is_applied_and_dry_run_is_reported() {
        let original = base();
        let (cfg, dry_run) = apply_configure(
            &original,
            &args(&[
                "--dry-run",
                "--mode",
                "proxy",
                "--public-origin",
                ORIGIN,
                "--preview-origin",
                PREVIEW,
                "--port",
                "7443",
            ]),
        )
        .expect("proxy");
        assert!(dry_run);
        assert_eq!(cfg.tls.mode, "proxy");
        assert_eq!(cfg.public_origin.as_deref(), Some(ORIGIN));
        assert_eq!(original.tls.mode, "local-ca");
    }

    #[test]
    fn public_origin_follows_the_tailscale_name_or_nothing() {
        let mut cfg = base();
        cfg.port = 7443;
        cfg.public_origin = None;
        cfg.listen = vec!["tailscale".to_string()];
        assert_eq!(
            public_origin_of(&cfg, Some("mac.tail1.ts.net")).expect("origin"),
            "https://mac.tail1.ts.net:7443"
        );
        cfg.listen = Vec::new();
        assert_eq!(
            public_origin_of(&cfg, None).expect("origin"),
            "https://localhost:7443"
        );
        cfg.public_origin = Some(ORIGIN.to_string());
        cfg.listen = vec!["tailscale".to_string()];
        assert_eq!(
            public_origin_of(&cfg, Some("mac.tail1.ts.net")).expect("origin"),
            ORIGIN
        );
    }

    #[test]
    fn wireguard_addresses_are_origins() {
        let mut cfg = base();
        cfg.port = 7443;
        cfg.public_origin = None;
        cfg.listen = vec!["10.8.0.1".to_string()];
        assert_eq!(
            public_origin_of(&cfg, None).expect("origin"),
            "https://10.8.0.1:7443"
        );
        cfg.listen = vec!["fd00::1".to_string()];
        assert_eq!(
            public_origin_of(&cfg, None).expect("origin"),
            "https://[fd00::1]:7443"
        );
        cfg.port = 443;
        assert_eq!(
            public_origin_of(&cfg, None).expect("origin"),
            "https://[fd00::1]"
        );
    }

    #[test]
    fn project_root_is_the_checkout_above_the_cargo_target_dir() {
        assert_eq!(
            project_root_from_exe(Path::new("/home/u/briardr-app/server/target/debug/briardr")),
            Some(PathBuf::from("/home/u/briardr-app"))
        );
        assert_eq!(
            project_root_from_exe(Path::new("/usr/local/bin/briardr")),
            None
        );
    }

    #[test]
    fn project_root_is_the_release_with_bin_next_to_the_client() {
        let release = Scratch::new("release");
        let exe = release.join("bin").join("briardr");
        assert_eq!(project_root_from_exe(&exe), None);
        std::fs::create_dir_all(release.join("web").join("dist")).unwrap();
        assert_eq!(project_root_from_exe(&exe), Some(release.to_path_buf()));
        assert_eq!(project_root_from_exe(&release.join("briardr")), None);
    }

    #[test]
    fn plist_escapes_ampersands_and_angle_brackets_only() {
        let text = plist_text("/opt/a&b<c>\"d'", Path::new("/data/logs"), "/usr/bin:/bin");
        assert!(text.contains(
            "<array><string>/opt/a&amp;b&lt;c>\"d'</string><string>serve</string></array>"
        ));
        assert!(text.contains("<key>StandardOutPath</key><string>/data/logs/stdout.log</string>"));
        assert!(text.ends_with("</plist>\n"));
    }

    #[test]
    fn configure_help_is_the_documented_text() {
        assert!(CONFIGURE_HELP.starts_with("Usage: briardr configure [options]\n"));
        assert!(CONFIGURE_HELP.ends_with(
            "Changes are validated first; the previous config.json is kept as config.json.bak."
        ));
    }
}
