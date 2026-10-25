// `briardr serve`: builds every part of the server and runs the listeners.
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::actions::Actions;
use crate::app::{now_ms, the_host, App, AppDeps, ConfigCell, VERSION};
use crate::auth::Auth;
use crate::config::{load_config, validate_config, BriardrConfig, ConfigError, Paths};
use crate::diag::Diagnostics;
use crate::dictation::ensure_helper;
use crate::herdr::rpc::{herdr_status, HerdrTarget};
use crate::herdr::state::{HerdrReader, HerdrState};
use crate::listener::{self, CertResolver, Handler};
use crate::monitor::{AlertSink, Monitor};
use crate::net::{listen_addresses, missing_local_addresses, tailscale_info, TailscaleInfo};
use crate::notes::Notes;
use crate::platform::{capabilities, host_platform};
use crate::power::Power;
use crate::preview::Previews;
use crate::push::Push;
use crate::terminal_hub::TerminalHub;
use crate::tls::{ca_mobile_config, ensure_local_ca, ensure_tailscale_cert, load_tls, TlsMaterial};
use crate::uploads::{prune_uploads, uploads_root};
use crate::usage::claude::StatuslineTap;
use crate::usage::{Usage, UsageDeps};

const TLS_REFRESH: Duration = Duration::from_secs(30 * 60);
const UPLOAD_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Briardr can't listen yet (an address isn't up); exits with 75 (EX_TEMPFAIL) so the service retries.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ListenError(pub String);

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Listen(#[from] ListenError),
    #[error("{0}")]
    Other(String),
}

/// A running server.
pub struct Running {
    pub app: Arc<App>,
    pub urls: Vec<String>,
    pub monitor: Monitor,
    shutdown: watch::Sender<bool>,
    listeners: Vec<JoinHandle<()>>,
    timers: Vec<JoinHandle<()>>,
}

impl Running {
    /// Gives back what Briardr holds (power assertions), stops Herdr tracking and previews, closes listeners.
    pub async fn close(self) {
        for t in &self.timers {
            t.abort();
        }
        let power = self.app.d.power.clone();
        let _ = tokio::task::spawn_blocking(move || lock(&power).release()).await;
        self.app.d.herdr.stop();
        self.app.d.previews.close_all();
        self.app.backends.stop_all();
        let _ = self.shutdown.send(true);
        for l in self.listeners {
            let _ = tokio::time::timeout(Duration::from_secs(5), l).await;
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn env_map() -> std::collections::HashMap<String, String> {
    std::env::vars().collect()
}

/// A random version-4 UUID in upper case, as `crypto.randomUUID().toUpperCase()`.
fn uuid_upper() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode_upper(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ServeError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ServeError::Other(e.to_string()))
}

/// The certificate for the configured mode, falling back to the local CA when Tailscale has none yet.
async fn obtain_tls(
    cfg: &BriardrConfig,
    dir: &Path,
    ts: &TailscaleInfo,
    names: &[String],
    ips: &[IpAddr],
) -> Result<Option<TlsMaterial>, ServeError> {
    if cfg.tls.mode == "tailscale" {
        if let Some(dns) = ts.dns_name.clone() {
            let d = dir.to_path_buf();
            if let Ok(m) = blocking(move || ensure_tailscale_cert(&d, &dns)).await? {
                return Ok(Some(m));
            }
        }
    }
    let (d, n, i) = (dir.to_path_buf(), names.to_vec(), ips.to_vec());
    blocking(move || ensure_local_ca(&d, &n, &i))
        .await?
        .map_err(|e| ServeError::Other(e.to_string()))?;
    let mode = if cfg.tls.mode == "files" {
        "files"
    } else {
        "local-ca"
    };
    let files = (
        PathBuf::from(cfg.tls.cert_file.clone().unwrap_or_default()),
        PathBuf::from(cfg.tls.key_file.clone().unwrap_or_default()),
    );
    let d = dir.to_path_buf();
    blocking(move || load_tls(&d, mode, Some((&files.0, &files.1))))
        .await?
        .map_err(|e| ServeError::Other(e.to_string()))
}

pub async fn serve(paths: Paths, quiet: bool) -> Result<Running, ServeError> {
    paths.ensure()?;
    let env = env_map();
    let mut cfg = load_config(&paths, &env)?;
    validate_config(&mut cfg)?;
    let caps = capabilities(host_platform(
        env.get("BRIARDR_PLATFORM")
            .map(String::as_str)
            .unwrap_or(std::env::consts::OS),
    ));
    let proxy_mode = cfg.tls.mode == "proxy";
    let web_dir = crate::web_dist(&env);
    let diag = Arc::new(
        Diagnostics::new(paths.logs(), paths.secrets().join("diag-salt"))
            .map_err(|e| ServeError::Other(e.to_string()))?,
    );
    diag.event("server_start", &Default::default());

    // Proxy mode: plain HTTP on loopback only; the proxy on this host is the only way in.
    let (addrs, ts) = if proxy_mode {
        (vec!["127.0.0.1".to_string()], TailscaleInfo::default())
    } else {
        let ts = blocking(tailscale_info).await?;
        (listen_addresses(&cfg.listen, &ts), ts)
    };
    // Binding to an address the host doesn't have fails with a misleading "port in use"; say what's wrong.
    let missing = missing_local_addresses(&addrs);
    if !missing.is_empty() {
        let these = if missing.len() > 1 {
            "these addresses"
        } else {
            "this address"
        };
        return Err(ListenError(format!(
            "Can't listen on {}: no network interface of this host has {these} yet. If it belongs to a WireGuard interface, check that it's up (on Linux: systemctl status wg-quick@wg0). Retrying is safe.",
            missing.join(", ")
        ))
        .into());
    }
    let mut hostnames: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    let push_host = |h: String, list: &mut Vec<String>| {
        if !list.contains(&h) {
            list.push(h);
        }
    };
    for h in &cfg.hostnames {
        push_host(h.to_lowercase(), &mut hostnames);
    }
    if let Some(origin) = cfg.public_origin.as_deref().filter(|o| !o.is_empty()) {
        if let Some(host) = url::Url::parse(origin)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
        {
            push_host(host, &mut hostnames);
        }
    }
    if let Some(dns) = &ts.dns_name {
        push_host(dns.to_lowercase(), &mut hostnames);
        push_host(
            dns.split('.').next().unwrap_or("").to_lowercase(),
            &mut hostnames,
        );
    }
    for ip in &addrs {
        push_host(ip.to_lowercase(), &mut hostnames);
    }

    // TLS. The configured mode is what the user wants; the effective mode is what's in use right now.
    // If Tailscale certificates aren't available yet, serve with the local CA and keep retrying; once
    // they are, swap certificates on the running listeners without a restart.
    let names: Vec<String> = hostnames
        .iter()
        .filter(|h| h.parse::<IpAddr>().is_err())
        .cloned()
        .collect();
    let ips: Vec<IpAddr> = hostnames.iter().filter_map(|h| h.parse().ok()).collect();
    let tls = if proxy_mode {
        None
    } else {
        let m = obtain_tls(&cfg, &paths.tls_dir(), &ts, &names, &ips).await?;
        if cfg.tls.mode == "tailscale" && m.as_ref().is_some_and(|m| m.mode != "tailscale") {
            diag.event(
                "tls_fallback",
                &[("reason".to_string(), json!("tailscale_unavailable"))].into(),
            );
            if !quiet {
                eprintln!("Tailscale HTTPS certificates are not available for this tailnet yet; using the local CA and retrying every 30 minutes.");
            }
        }
        m
    };
    let resolver = match &tls {
        Some(m) => Some(CertResolver::new(m).map_err(ServeError::Other)?),
        None => None,
    };
    let ca_profile = if proxy_mode {
        None
    } else {
        std::fs::read(paths.tls_dir().join("ca.crt"))
            .ok()
            .map(|pem| ca_mobile_config(&pem, &uuid_upper(), &uuid_upper()))
    };
    let tls_mode = Arc::new(RwLock::new((
        cfg.tls.mode.clone(),
        tls.as_ref().map_or("proxy".to_string(), |m| m.mode.clone()),
    )));

    let auth = Arc::new(Auth::new(paths.devices(), paths.pairing()));
    let herdr = HerdrState::new(HerdrTarget::of(&cfg), diag.clone());
    let host = the_host(&caps);
    let actions = Arc::new(Actions::new(herdr.clone(), host.clone()));
    let hub = Arc::new(TerminalHub::new(herdr.clone(), diag.clone(), host.clone()));
    let power = Arc::new(Mutex::new(Power::system(
        paths.power_state(),
        caps.clone(),
        diag.clone(),
    )));
    let public_origin = crate::public_origin_of(&cfg, ts.dns_name.as_deref())?;
    // The VAPID subject. Apple's push service rejects made-up domains (mailto:…@localhost.localdomain,
    // .invalid) with BadJwtToken but accepts an https URL with an IP address and port (checked 2026-10-11).
    let subject = match (
        cfg.public_origin.as_deref().filter(|o| !o.is_empty()),
        &ts.dns_name,
    ) {
        (Some(o), _) => o.to_string(),
        (None, Some(dns)) => format!("https://{dns}"),
        (None, None) => public_origin.clone(),
    };
    let push = Arc::new(
        Push::new(&paths.vapid(), paths.push_subs(), subject, diag.clone())
            .map_err(|e| ServeError::Other(e.to_string()))?,
    );
    let cell = Arc::new(ConfigCell::new(cfg.clone(), paths.clone()));
    let tap = StatuslineTap::new(
        PathBuf::from(cfg.usage.claude_config_dir.clone().unwrap_or_default()),
        paths.state(),
        paths.bin(),
    );
    let usage = {
        let (c1, c2, c3) = (cell.clone(), cell.clone(), cell.clone());
        let keychain = caps.claude_keychain;
        Arc::new(Usage::new(
            paths.usage(),
            tap,
            UsageDeps {
                claude_enabled: Box::new(move || keychain && c1.get().usage.claude_keychain_opt_in),
                codex_enabled: Box::new(move || c2.get().usage.codex_enabled),
                codex_path: Box::new(move || c3.get().usage.codex_path.unwrap_or_default()),
                fetch_claude: None,
                fetch_codex: None,
                consume_codex_reset: None,
            },
            diag.clone(),
        ))
    };
    let previews = Previews::new(
        cell.clone(),
        resolver.clone(),
        addrs.clone(),
        auth.clone(),
        diag.clone(),
    );
    let dictation_helper = Arc::new(RwLock::new(None));
    if caps.dictation && !env.contains_key("BRIARDR_NO_DICTATION") {
        let slot = dictation_helper.clone();
        let (bin, source) = (
            paths.bin(),
            crate::project_root()
                .join("native")
                .join("transcribe.swift"),
        );
        tokio::spawn(async move {
            let helper = ensure_helper(&bin, &source).await;
            *slot.write().unwrap_or_else(|e| e.into_inner()) = helper;
        });
    }

    let app = App::new(AppDeps {
        cfg: cell.clone(),
        auth: auth.clone(),
        herdr: herdr.clone(),
        actions,
        hub,
        diag: diag.clone(),
        power: power.clone(),
        push: push.clone(),
        usage,
        previews,
        notes: Arc::new(Notes::new(paths.state().join("notes"))),
        web_dir,
        allowed_hosts: Arc::new(RwLock::new(
            hostnames.iter().cloned().collect::<HashSet<_>>(),
        )),
        dictation_helper,
        ca_profile,
        tls_mode: tls_mode.clone(),
        caps: caps.clone(),
    });

    let monitor = {
        let c = cell.clone();
        Monitor::new(
            Arc::new(herdr.clone()) as Arc<dyn HerdrReader>,
            push.clone() as Arc<dyn AlertSink>,
            Arc::new(move || c.get()),
            paths.monitor_state(),
            diag.clone(),
        )
    };

    let (shutdown, shutdown_rx) = watch::channel(false);
    let handler: Handler = {
        let app = app.clone();
        Arc::new(move |req, peer| {
            let app = app.clone();
            Box::pin(async move { app.handle(req, peer).await })
        })
    };
    let acceptor = resolver.as_ref().map(|r| r.acceptor());
    let mut listeners = Vec::new();
    let mut urls = Vec::new();
    for addr in &addrs {
        let ip: IpAddr = addr
            .parse()
            .map_err(|_| ServeError::Other(format!("Invalid listen address {addr}")))?;
        let l = TcpListener::bind((ip, cfg.port))
            .await
            .map_err(|e| ServeError::Other(format!("Can't listen on {addr}:{}: {e}", cfg.port)))?;
        listeners.push(tokio::spawn(listener::serve(
            l,
            acceptor.clone(),
            handler.clone(),
            shutdown_rx.clone(),
        )));
        let shown = if addr.contains(':') {
            format!("[{addr}]")
        } else {
            addr.clone()
        };
        urls.push(format!(
            "{}://{shown}:{}",
            if tls.is_some() { "https" } else { "http" },
            cfg.port
        ));
    }

    #[cfg(feature = "test-hooks")]
    crate::test_hooks::set_urls(urls.clone());

    let mut timers = Vec::new();
    // Renew or upgrade certificates in place: new connections get the new certificate, open ones keep theirs.
    if let (Some(resolver), Some(first)) = (resolver.clone(), tls.clone()) {
        let (c, dir, ts, diag, mode) = (
            cell.clone(),
            paths.tls_dir(),
            ts.clone(),
            diag.clone(),
            tls_mode.clone(),
        );
        timers.push(tokio::spawn(async move {
            let mut current = first.cert_pem;
            let mut tick =
                tokio::time::interval_at(tokio::time::Instant::now() + TLS_REFRESH, TLS_REFRESH);
            loop {
                tick.tick().await;
                let cfg = c.get();
                let next = match cfg.tls.mode.as_str() {
                    "tailscale" | "local-ca" => obtain_tls(&cfg, &dir, &ts, &names, &ips)
                        .await
                        .ok()
                        .flatten(),
                    _ => None,
                };
                let ev = |e: &str, m: &str| diag.event(e, &[("mode".to_string(), json!(m))].into());
                match next {
                    Some(m) if m.cert_pem != current => match resolver.replace(&m) {
                        Ok(()) => {
                            ev("tls_reloaded", &m.mode);
                            mode.write().unwrap_or_else(|e| e.into_inner()).1 = m.mode.clone();
                            current = m.cert_pem;
                        }
                        Err(_) => ev("tls_refresh_failed", &cfg.tls.mode),
                    },
                    Some(_) => {}
                    None if matches!(cfg.tls.mode.as_str(), "tailscale" | "local-ca") => {
                        ev("tls_refresh_failed", &cfg.tls.mode)
                    }
                    None => {}
                }
            }
        }));
    }
    let auth_flush = auth.clone();
    timers.push(tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(60),
            Duration::from_secs(60),
        );
        loop {
            tick.tick().await;
            let a = auth_flush.clone();
            let _ = tokio::task::spawn_blocking(move || a.flush_last_seen()).await;
        }
    }));

    let _ = blocking(|| prune_uploads(&uploads_root(), UPLOAD_MAX_AGE)).await;
    herdr.start().await;
    monitor.start();
    // The first poll knows the battery before anything is held, and undoes a disablesleep left by a crash.
    let keep_awake = cfg.power.keep_awake;
    let p = power.clone();
    let first = blocking(move || lock(&p).start(keep_awake, now_ms())).await?;
    app.power_events(&first.events);
    app.broadcast(&json!({"t": "mac", "mac": first.summary, "host": crate::app::caps_json(&caps)}));
    if lock(&power).supported() {
        let a = app.clone();
        timers.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(60),
                Duration::from_secs(60),
            );
            loop {
                tick.tick().await;
                let _ = a.poll_power().await;
            }
        }));
        let (a, p) = (app.clone(), power.clone());
        timers.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(5),
                Duration::from_secs(5),
            );
            loop {
                tick.tick().await;
                let p = p.clone();
                if let Ok(events) =
                    tokio::task::spawn_blocking(move || lock(&p).tick(now_ms())).await
                {
                    a.power_events(&events);
                }
            }
        }));
    }

    if !quiet {
        println!("Briardr {VERSION} is running.");
        println!("  Open: {public_origin}");
        for u in &urls {
            println!("  Listening: {u}");
        }
        let session = if cfg.herdr_session.is_empty() {
            String::new()
        } else {
            format!(" (session {})", cfg.herdr_session)
        };
        let state = if herdr.health().ok {
            "connected".to_string()
        } else {
            match herdr_status(&HerdrTarget::of(&cfg)).await.problem {
                Some(problem) => crate::setup::herdr_hint(&problem, &cfg.herdr_session),
                None => "running, connecting".to_string(),
            }
        };
        println!("  Herdr: {}{session} — {state}", cfg.herdr_path);
        match &tls {
            Some(m) if m.mode != cfg.tls.mode => println!(
                "  Certificates: {} (wanted {}; retrying)",
                m.mode, cfg.tls.mode
            ),
            Some(m) => println!("  Certificates: {}", m.mode),
            None => println!(
                "  Certificates: from the reverse proxy (trusted proxies: {})",
                cfg.trusted_proxies.join(", ")
            ),
        }
        let platform = match caps.platform {
            crate::platform::HostPlatform::Macos => "macos",
            crate::platform::HostPlatform::Linux => "linux",
            crate::platform::HostPlatform::Other => "other",
        };
        println!("  Host: {platform}; data: {}", paths.root.display());
    }

    Ok(Running {
        app,
        urls,
        monitor,
        shutdown,
        listeners,
        timers,
    })
}
