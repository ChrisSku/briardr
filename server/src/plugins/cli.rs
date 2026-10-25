// `briardr plugin …`: list, link, build and turn plugins on or off on this host. Builds run Briardr's own
// toolchain (bun scripts/plugin-build.ts in the checkout); a running server notices every change
// within a second and tells devices.
use std::io::{self, BufRead, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{backend_hash, discover, read_manifest, web_runtime, Plugin, PluginState};
use crate::config::Paths;

pub const HELP: &str = r#"Usage: briardr plugin <command>
  list                  plugins on this host, whether they're on, and whether they're built
  link <folder>         install the plugin in <folder> (a link, so you keep working on it there) and build it
  unlink <id>           uninstall a linked plugin (its folder stays)
  build [<id>|--all]    build a plugin's client for this Briardr (all installed plugins with --all)
  enable <id> [--yes]   turn a plugin on for every device (asks before running a backend unless --yes)
  disable <id>          turn a plugin off
  dev [<folder>]        link, turn on and rebuild on every change; devices reload it (default: this folder)
  check [<folder>]      type-check a plugin against @briardr/sdk (default: this folder)
  create [<name>]       make a new plugin and hand it to Claude Code or Codex (also: briardr create plugin)"#;

pub struct Env {
    pub paths: Paths,
    /// The Briardr checkout (scripts/plugin-build.ts, plugins/template, skills/).
    pub root: PathBuf,
    /// The web client the server serves (its runtime.json says what plugins must be built for).
    pub web_dir: PathBuf,
    pub home: PathBuf,
}

pub fn run(env: &Env, args: &[String]) -> Result<i32, String> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("list") | None => list(env),
        Some("link") => link(
            env,
            arg(1).ok_or("Which folder? briardr plugin link <folder>")?,
        ),
        Some("unlink") => unlink(
            env,
            arg(1).ok_or("Which plugin? briardr plugin unlink <id>")?,
        ),
        Some("build") => build_command(env, arg(1)),
        Some("enable") => {
            let id = arg(1).ok_or("Which plugin? briardr plugin enable <id>")?;
            enable(env, id, args.iter().any(|a| a == "--yes")).map(|_| 0)
        }
        Some("disable") => {
            let id = arg(1).ok_or("Which plugin? briardr plugin disable <id>")?;
            set_enabled(env, id, false).map(|_| 0)
        }
        Some("dev") => dev(env, arg(1)),
        Some("check") => check(env, arg(1)),
        Some("create") => super::create::run(env, &args[1..]),
        Some("help" | "--help" | "-h") => {
            println!("{HELP}");
            Ok(0)
        }
        Some(other) => Err(format!("Unknown plugin command “{other}”.\n{HELP}")),
    }
}

fn find(env: &Env, id: &str) -> Result<Plugin, String> {
    discover(&env.paths)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("There’s no plugin “{id}”. `briardr plugin list` shows them."))
}

fn list(env: &Env) -> Result<i32, String> {
    let state = PluginState::load(&env.paths);
    let runtime = web_runtime(&env.web_dir);
    for p in discover(&env.paths) {
        let name = p.manifest.as_ref().map_or("", |m| m.name.as_str());
        let on = if p.active(&state) { "on " } else { "off" };
        let mut notes = Vec::new();
        if p.builtin {
            notes.push("built in".to_string());
        } else {
            if let Some(dir) = &p.dir {
                let target = std::fs::read_link(dir).unwrap_or_else(|_| dir.clone());
                notes.push(target.display().to_string());
            }
            match p.build_status(runtime.as_deref()) {
                "missing" => notes.push("not built".into()),
                "stale" => notes.push("built for another Briardr: rebuild".into()),
                _ => {}
            }
            let backend = p.manifest.as_ref().and_then(|m| m.backend.as_ref());
            if let Some(b) = backend {
                if state.approved.get(&p.id) != Some(&backend_hash(b)) && state.is_enabled(&p.id) {
                    notes.push(
                        "backend command changed: approve with `briardr plugin enable`".into(),
                    );
                }
            }
        }
        if let Some(problem) = &p.problem {
            notes.push(problem.clone());
        }
        println!("{on}  {:<14} {:<14} {}", p.id, name, notes.join(" · "));
    }
    Ok(0)
}

fn link(env: &Env, folder: &str) -> Result<i32, String> {
    let dir = std::path::absolute(folder).map_err(|e| e.to_string())?;
    let id = link_folder(env, &dir)?;
    build(env, &dir)?;
    println!(
        "Installed {id}. Turn it on with `briardr plugin enable {id}` or in Settings → Plugins."
    );
    Ok(0)
}

/// Links `dir` into the plugins folder under its manifest's ID; already linked there is fine.
pub(super) fn link_folder(env: &Env, dir: &Path) -> Result<String, String> {
    let m = read_manifest(dir)?;
    let target = env.paths.plugins().join(&m.id);
    if let Ok(existing) = std::fs::read_link(&target) {
        if existing == dir {
            return Ok(m.id);
        }
        return Err(format!(
            "“{}” is already installed from {}. Unlink it first: briardr plugin unlink {}",
            m.id,
            existing.display(),
            m.id
        ));
    }
    if target.exists() {
        return Err(format!(
            "A plugin “{}” is already installed in {}.",
            m.id,
            target.display()
        ));
    }
    std::fs::create_dir_all(env.paths.plugins()).map_err(|e| e.to_string())?;
    symlink(dir, &target).map_err(|e| format!("Couldn’t link the plugin: {e}"))?;
    if let Some(problem) = find(env, &m.id)?.problem {
        let _ = std::fs::remove_file(&target);
        return Err(problem);
    }
    Ok(m.id)
}

fn unlink(env: &Env, id: &str) -> Result<i32, String> {
    let target = env.paths.plugins().join(id);
    if std::fs::read_link(&target).is_err() {
        return Err(format!(
            "“{id}” isn’t a linked plugin. Remove {} yourself to uninstall it.",
            target.display()
        ));
    }
    set_enabled(env, id, false)?;
    std::fs::remove_file(&target).map_err(|e| e.to_string())?;
    println!("Uninstalled {id}.");
    Ok(0)
}

fn build_command(env: &Env, which: Option<&str>) -> Result<i32, String> {
    let dirs: Vec<PathBuf> = match which {
        Some("--all") => discover(&env.paths)
            .into_iter()
            .filter(|p| !p.builtin)
            .filter_map(|p| p.dir)
            .collect(),
        // An installed plugin's ID, else a folder.
        Some(id) if !id.contains('/') && id != "." => {
            vec![find(env, id)?
                .dir
                .ok_or("Built-in plugins are part of Briardr’s own build.")?]
        }
        folder => vec![std::path::absolute(folder.unwrap_or(".")).map_err(|e| e.to_string())?],
    };
    let mut failed = 0;
    for dir in dirs {
        if build(env, &dir).is_err() {
            failed += 1;
        }
    }
    Ok(if failed > 0 { 1 } else { 0 })
}

/// Plugins are made and built with the checkout's toolchain (scripts/, plugins/template, skills/ and
/// Bun); a release (scripts/package.ts) has none.
pub(super) fn toolchain(env: &Env) -> Result<(), String> {
    if env.root.join("scripts/plugin-build.ts").is_file() {
        return Ok(());
    }
    Err(format!(
        "Making and building plugins needs Briardr’s checkout and its tools, and {} is a release without them. \
         Clone the version you run (git clone --branch v{} https://github.com/ChrisSku/briardr.git), run \
         `mise install && mise run install` in it, and run plugin commands with BRIARDR_PROJECT_ROOT set to it.",
        env.root.display(),
        crate::app::VERSION
    ))
}

/// One of the toolchain's scripts (scripts/<name>), run with Bun.
fn script(env: &Env, name: &str) -> Result<Command, String> {
    toolchain(env)?;
    let mut cmd = bun(&env.root);
    cmd.arg(env.root.join("scripts").join(name));
    Ok(cmd)
}

/// `bun`: from PATH, else the one the checkout's mise.toml pins (installed by `mise install`).
pub(super) fn bun(root: &Path) -> Command {
    let on_path = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("bun").is_file()));
    if on_path {
        return Command::new("bun");
    }
    let pinned = Command::new("mise")
        .args(["which", "bun"])
        .current_dir(root)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|p| !p.is_empty());
    // Not found: running "bun" fails with a message that says so.
    Command::new(pinned.unwrap_or_else(|| "bun".to_string()))
}

pub(super) fn build(env: &Env, dir: &Path) -> Result<(), String> {
    let status = script(env, "plugin-build.ts")?
        .arg(dir)
        .status()
        .map_err(|e| {
            format!(
                "Couldn’t run bun ({e}). Briardr builds plugins with the Bun from its checkout."
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("The build of {} failed.", dir.display()))
    }
}

fn check(env: &Env, folder: Option<&str>) -> Result<i32, String> {
    let dir =
        match folder {
            Some(id) if !id.contains('/') && id != "." && find(env, id).is_ok() => find(env, id)?
                .dir
                .ok_or("Built-in plugins are checked with Briardr itself.")?,
            other => std::path::absolute(other.unwrap_or(".")).map_err(|e| e.to_string())?,
        };
    if !dir.join("tsconfig.json").is_file() {
        return Err(format!(
            "{} has no tsconfig.json. Plugins made with `briardr plugin create` have one.",
            dir.display()
        ));
    }
    let status = script(env, "plugin-check.ts")?
        .arg(&dir)
        .status()
        .map_err(|e| format!("Couldn’t run bun ({e})."))?;
    Ok(status.code().unwrap_or(1))
}

fn set_enabled(env: &Env, id: &str, on: bool) -> Result<(), String> {
    let mut state = PluginState::load(&env.paths);
    state.set_enabled(id, on);
    state.save(&env.paths).map_err(|e| e.to_string())?;
    if !on {
        println!("Turned {id} off.");
    }
    Ok(())
}

/// Turns a plugin on; a backend's command is shown and needs a yes (or `--yes`).
pub(super) fn enable(env: &Env, id: &str, yes: bool) -> Result<bool, String> {
    let p = find(env, id)?;
    if let Some(problem) = &p.problem {
        return Err(problem.clone());
    }
    let mut state = PluginState::load(&env.paths);
    if let Some(b) = p.manifest.as_ref().and_then(|m| m.backend.as_ref()) {
        let hash = backend_hash(b);
        if state.approved.get(id) != Some(&hash) {
            println!(
                "{id} runs a program on this computer, with your user’s access to its files:\n\n    {}\n    (in {})\n",
                b.command.join(" "),
                p.dir
                    .as_ref()
                    .map(|d| std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()))
                    .map_or(String::new(), |d| d.display().to_string())
            );
            if !yes && !confirm("Turn it on?")? {
                println!("Left off.");
                return Ok(false);
            }
            state.approved.insert(id.to_string(), hash);
        }
    }
    state.set_enabled(id, true);
    state.save(&env.paths).map_err(|e| e.to_string())?;
    println!("Turned {id} on for every device.");
    Ok(true)
}

pub(super) fn confirm(question: &str) -> Result<bool, String> {
    print!("{question} [y/N] ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn dev(env: &Env, folder: Option<&str>) -> Result<i32, String> {
    let dir = std::path::absolute(folder.unwrap_or(".")).map_err(|e| e.to_string())?;
    let id = link_folder(env, &dir)?;
    build(env, &dir)?;
    if !find(env, &id)?.active(&PluginState::load(&env.paths)) && !enable(env, &id, false)? {
        return Ok(1);
    }
    println!("Open a stream in Briardr to see {id}. Changes rebuild and reload on every device.");
    let status = script(env, "plugin-build.ts")?
        .arg(&dir)
        .arg("--watch")
        .stdin(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    Ok(status.code().unwrap_or(1))
}
