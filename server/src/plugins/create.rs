// `briardr create plugin`: asks what the plugin should do, makes it from plugins/template, installs and
// turns it on, installs the briardr-plugins skill for Claude Code and Codex, and starts the agent in the
// new folder with the job. Also `briardr skill install|print` and `briardr install-cli`.
use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::cli::{self, Env};
use super::{discover, validate, Contributes, Manifest, Section, Tab, API_VERSION};

pub const HELP: &str = r#"Usage: briardr create plugin [<name>] [options]
  --description TEXT       what the plugin should do (the agent builds this)
  --kind stream|app|settings
                           a tab in every stream (default), a tab of its own, or a Settings section
  --backend / --no-backend whether it runs code on this computer (default: no)
  --dir FOLDER             where to make it (default: ~/briardr-plugins/<id>)
  --agent claude|codex|none
                           who builds it (default: asks, or the one that's installed)
  --yes                    don't ask; use the defaults and approve the template's backend"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Stream,
    App,
    Settings,
}

struct Answers {
    name: String,
    id: String,
    description: String,
    kind: Kind,
    backend: bool,
    dir: PathBuf,
    agent: Option<String>,
}

pub fn run(env: &Env, args: &[String]) -> Result<i32, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return Ok(0);
    }
    cli::toolchain(env)?;
    let yes = args.iter().any(|a| a == "--yes");
    let ask = !yes && io::stdin().is_terminal();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let flags_with_values = ["--description", "--kind", "--dir", "--agent"];
    let positional = args.iter().enumerate().find(|(i, a)| {
        !a.starts_with("--") && (*i == 0 || !flags_with_values.contains(&args[i - 1].as_str()))
    });

    println!("Let’s make a Briardr plugin.\n");
    let name = match positional.map(|(_, a)| a.clone()) {
        Some(n) => n,
        None if ask => prompt("Name (e.g. “CI status”)", None)?,
        None => return Err(format!("Give the plugin a name.\n{HELP}")),
    };
    let name = clean_name(&name);
    let id = slug(&name);
    if id.is_empty() {
        return Err("Use a name with letters in it.".into());
    }
    let description = match flag("--description") {
        Some(d) => d,
        None if ask => prompt(
            "What should it do? (one or two sentences; the agent builds this)",
            None,
        )?,
        None => format!("{name}, a Briardr plugin."),
    };
    let kind = match flag("--kind").as_deref() {
        Some("stream") => Kind::Stream,
        Some("app") => Kind::App,
        Some("settings") => Kind::Settings,
        Some(other) => return Err(format!("--kind is stream, app or settings, not “{other}”.")),
        None if ask => match choose(
            "Where does it show?",
            &[
                "A tab in every stream, next to Files and Changes",
                "A tab of its own, next to Agents and Workspaces",
                "A section in Settings",
            ],
            0,
        )? {
            0 => Kind::Stream,
            1 => Kind::App,
            _ => Kind::Settings,
        },
        None => Kind::Stream,
    };
    let backend = if args.iter().any(|a| a == "--backend") {
        true
    } else if args.iter().any(|a| a == "--no-backend") || !ask {
        false
    } else {
        prompt_yes_no(
            "Does it need to run code on this computer (read files, call tools or APIs)?",
            false,
        )?
    };
    let dir = match flag("--dir") {
        Some(d) => std::path::absolute(d).map_err(|e| e.to_string())?,
        None => env.home.join("briardr-plugins").join(&id),
    };
    let agents = installed_agents();
    let agent = match flag("--agent").as_deref() {
        Some("none") => None,
        Some(a @ ("claude" | "codex")) => Some(a.to_string()),
        Some(other) => return Err(format!("--agent is claude, codex or none, not “{other}”.")),
        None if !ask => None,
        None => match agents.as_slice() {
            [] => None,
            [only] => prompt_yes_no(&format!("Start {} to build it?", agent_label(only)), true)?
                .then(|| only.clone()),
            many => {
                let mut options: Vec<String> =
                    many.iter().map(|a| agent_label(a).to_string()).collect();
                options.push("Neither, I’ll build it myself".into());
                let i = choose(
                    "Who builds it?",
                    &options.iter().map(String::as_str).collect::<Vec<_>>(),
                    0,
                )?;
                many.get(i).cloned()
            }
        },
    };
    let a = Answers {
        name,
        id,
        description,
        kind,
        backend,
        dir,
        agent,
    };

    check_free(env, &a)?;
    scaffold(env, &a)?;
    println!("\nMade {} in {}", a.id, a.dir.display());
    install_skill(env)?;
    cli::link_folder(env, &a.dir)?;
    cli::build(env, &a.dir)?;
    let on = cli::enable(env, &a.id, yes)?;
    if !on {
        println!(
            "It stays off until you turn it on: briardr plugin enable {}",
            a.id
        );
    }

    let bin = briardr_bin();
    println!(
        "\nOpen a stream in Briardr to see it. After changes: {bin} plugin build {} (devices reload it).",
        a.dir.display()
    );
    let Some(agent) = a.agent.clone() else {
        println!("Build it yourself, or start Claude Code or Codex in that folder: it has the briardr-plugins skill.");
        return Ok(0);
    };
    let job = first_prompt(&a, &bin);
    println!(
        "\nStarting {} in {}…\n",
        agent_label(&agent),
        a.dir.display()
    );
    let err = Command::new(&agent).arg(job).current_dir(&a.dir).exec();
    Err(format!("Couldn’t start {agent}: {err}"))
}

fn first_prompt(a: &Answers, bin: &str) -> String {
    let place = match a.kind {
        Kind::Stream => "a tab in every stream",
        Kind::App => "a tab of its own in Briardr",
        Kind::Settings => "a section in Briardr's Settings",
    };
    format!(
        "Build the Briardr plugin in this folder. What it should do: {}\n\n\
         It shows as {place}{}. Use the briardr-plugins skill. The scaffold works already and is \
         installed and turned on: after each change run `{bin} plugin build .` (devices with Briardr \
         open reload it) and `{bin} plugin check .`. Ask me if what it should do is unclear. When \
         it works, tell me what to look at in Briardr, and commit.",
        a.description,
        if a.backend { ", with a backend (server/main.ts)" } else { "" }
    )
}

fn agent_label(agent: &str) -> &str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        other => other,
    }
}

fn installed_agents() -> Vec<String> {
    ["claude", "codex"]
        .into_iter()
        .filter(|a| on_path(a).is_some())
        .map(str::to_string)
        .collect()
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// How the user (and the agent) run this Briardr: `briardr` when that's this binary, else its full path.
fn briardr_bin() -> String {
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok());
    let found = on_path("briardr").and_then(|p| std::fs::canonicalize(p).ok());
    match (me, found) {
        (Some(me), Some(found)) if me == found => "briardr".to_string(),
        (Some(me), _) => me.display().to_string(),
        _ => "briardr".to_string(),
    }
}

/// Names go into markup and comments as they are; braces and angle brackets would break them.
fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '{' | '}' | '<' | '>' | '"' | '`'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    let out = if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("p-{out}")
    } else {
        out
    };
    out.chars()
        .take(40)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

fn manifest(a: &Answers) -> Manifest {
    let tab = Tab {
        id: a.id.clone(),
        label: a.name.clone(),
        icon: Some("plug".into()),
    };
    let mut contributes = Contributes::default();
    match a.kind {
        Kind::Stream => contributes.stream_tabs.push(tab),
        Kind::App => contributes.app_tabs.push(tab),
        Kind::Settings => contributes.settings.push(Section {
            id: a.id.clone(),
            label: a.name.clone(),
        }),
    }
    Manifest {
        id: a.id.clone(),
        name: a.name.clone(),
        version: "0.1.0".into(),
        description: a.description.clone(),
        briardr: API_VERSION,
        client: Some("src/index.ts".into()),
        backend: a.backend.then(|| super::BackendSpec {
            command: vec!["bun".into(), "server/main.ts".into()],
        }),
        contributes,
    }
}

fn check_free(env: &Env, a: &Answers) -> Result<(), String> {
    let all = discover(&env.paths);
    if all.iter().any(|p| p.id == a.id) {
        return Err(format!(
            "There’s already a plugin “{}”. Pick another name.",
            a.id
        ));
    }
    let mut taken = HashSet::new();
    for m in all
        .iter()
        .filter(|p| p.usable())
        .filter_map(|p| p.manifest.as_ref())
    {
        super::claim(&mut taken, m);
    }
    validate(&manifest(a), &taken)?;
    if a.dir.exists() && std::fs::read_dir(&a.dir).map_or(true, |mut d| d.next().is_some()) {
        return Err(format!(
            "{} already exists. Use --dir for another folder.",
            a.dir.display()
        ));
    }
    Ok(())
}

/// Replaces {{key}} and keeps or drops {{#backend}}…{{/backend}} and {{^backend}}…{{/backend}} lines.
fn render(text: &str, vars: &[(&str, String)], backend: bool) -> String {
    let mut out = String::new();
    let mut keep = vec![true];
    for line in text.split_inclusive('\n') {
        match line.trim() {
            "{{#backend}}" => keep.push(backend),
            "{{^backend}}" => keep.push(!backend),
            "{{/backend}}" => {
                keep.pop();
            }
            _ if keep.iter().all(|k| *k) => out.push_str(line),
            _ => {}
        }
    }
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

fn scaffold(env: &Env, a: &Answers) -> Result<(), String> {
    let template = env.root.join("plugins/template");
    let host = if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this server"
    };
    let vars = [
        ("id", a.id.clone()),
        ("name", a.name.clone()),
        ("description", a.description.replace("-->", "—>")),
        (
            "tab",
            if a.id.contains('-') {
                format!("'{}'", a.id)
            } else {
                a.id.clone()
            },
        ),
        ("briardr", env.root.display().to_string()),
        ("briardr_bin", briardr_bin()),
        ("host", host.to_string()),
    ];
    let (index, component) = match a.kind {
        Kind::Stream => ("src/index.stream.ts", "src/Tab.svelte"),
        Kind::App => ("src/index.app.ts", "src/Screen.svelte"),
        Kind::Settings => ("src/index.settings.ts", "src/Settings.svelte"),
    };
    let mut files: Vec<(&str, &str)> = vec![
        ("tsconfig.json", "tsconfig.json"),
        ("svelte.config.js", "svelte.config.js"),
        ("gitignore", ".gitignore"),
        ("README.md", "README.md"),
        ("AGENTS.md", "AGENTS.md"),
        ("CLAUDE.md", "CLAUDE.md"),
        (index, "src/index.ts"),
        (component, component),
    ];
    if a.backend {
        files.push(("server/main.ts", "server/main.ts"));
        files.push(("server/briardr.ts", "server/briardr.ts"));
    }
    for (from, to) in files {
        let text = std::fs::read_to_string(template.join(from)).map_err(|e| {
            format!(
                "The plugin template is incomplete ({from}: {e}). Is {} a Briardr checkout?",
                env.root.display()
            )
        })?;
        let dest = a.dir.join(to);
        std::fs::create_dir_all(dest.parent().unwrap_or(&a.dir)).map_err(|e| e.to_string())?;
        std::fs::write(&dest, render(&text, &vars, a.backend)).map_err(|e| e.to_string())?;
    }
    let mut m = serde_json::to_string_pretty(&manifest(a)).map_err(|e| e.to_string())?;
    m.push('\n');
    std::fs::write(a.dir.join(super::MANIFEST), m).map_err(|e| e.to_string())?;
    let _ = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&a.dir)
        .status();
    Ok(())
}

/// Where the skill goes: Claude Code's skills, and the shared agent skills folder Codex reads.
fn skill_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".claude/skills/briardr-plugins"),
        home.join(".agents/skills/briardr-plugins"),
    ]
}

fn skill_files(env: &Env) -> Result<Vec<(PathBuf, String)>, String> {
    cli::toolchain(env)?;
    let src = env.root.join("skills/briardr-plugins");
    let vars = [
        ("briardr", env.root.display().to_string()),
        ("briardr_bin", briardr_bin()),
    ];
    let mut out = Vec::new();
    for rel in ["SKILL.md", "references/sdk.md"] {
        let text = std::fs::read_to_string(src.join(rel))
            .map_err(|e| format!("Couldn’t read the skill ({rel}: {e})."))?;
        out.push((PathBuf::from(rel), render(&text, &vars, false)));
    }
    Ok(out)
}

/// Writes (or updates) the briardr-plugins skill for Claude Code and Codex.
pub fn install_skill(env: &Env) -> Result<(), String> {
    let files = skill_files(env)?;
    for dir in skill_dirs(&env.home) {
        for (rel, text) in &files {
            let dest = dir.join(rel);
            std::fs::create_dir_all(dest.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
            std::fs::write(&dest, text).map_err(|e| e.to_string())?;
        }
        println!("Installed the briardr-plugins skill in {}", dir.display());
    }
    Ok(())
}

/// `briardr skill install|print`.
pub fn skill_command(env: &Env, args: &[String]) -> Result<i32, String> {
    match args.first().map(String::as_str) {
        Some("install") | None => install_skill(env).map(|_| 0),
        Some("print") => {
            for (rel, text) in skill_files(env)? {
                if rel != Path::new("SKILL.md") {
                    println!("\n---- {} ----\n", rel.display());
                }
                print!("{text}");
            }
            Ok(0)
        }
        Some(other) => Err(format!(
            "Unknown skill command “{other}”. Usage: briardr skill [install|print]"
        )),
    }
}

/// `briardr install-cli`: links this binary into ~/.local/bin, so `briardr …` works in any folder.
pub fn install_cli(env: &Env) -> Result<i32, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = env.home.join(".local/bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let link = bin.join("briardr");
    if std::fs::symlink_metadata(&link).is_ok() {
        if std::fs::read_link(&link).is_err() {
            return Err(format!(
                "{} exists and isn’t a link; remove it first.",
                link.display()
            ));
        }
        std::fs::remove_file(&link).map_err(|e| e.to_string())?;
    }
    std::os::unix::fs::symlink(&me, &link).map_err(|e| e.to_string())?;
    println!("Linked {} → {}", link.display(), me.display());
    let on_path =
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == bin));
    if !on_path {
        println!(
            "{} isn’t on your PATH yet; add it in your shell’s profile.",
            bin.display()
        );
    }
    Ok(0)
}

fn read_line() -> Result<String, String> {
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

fn prompt(question: &str, default: Option<&str>) -> Result<String, String> {
    loop {
        match default {
            Some(d) => print!("{question} [{d}]: "),
            None => print!("{question}: "),
        }
        io::stdout().flush().map_err(|e| e.to_string())?;
        let line = read_line()?;
        if !line.is_empty() {
            return Ok(line);
        }
        if let Some(d) = default {
            return Ok(d.to_string());
        }
    }
}

fn prompt_yes_no(question: &str, default: bool) -> Result<bool, String> {
    print!("{question} [{}] ", if default { "Y/n" } else { "y/N" });
    io::stdout().flush().map_err(|e| e.to_string())?;
    Ok(match read_line()?.to_lowercase().as_str() {
        "" => default,
        a => a.starts_with('y'),
    })
}

fn choose(question: &str, options: &[&str], default: usize) -> Result<usize, String> {
    println!("{question}");
    for (i, o) in options.iter().enumerate() {
        println!("  {}. {o}", i + 1);
    }
    loop {
        let answer = prompt("Choose", Some(&(default + 1).to_string()))?;
        if let Ok(n) = answer.parse::<usize>() {
            if (1..=options.len()).contains(&n) {
                return Ok(n - 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use crate::scratch::Scratch;
    use serde_json::json;

    #[test]
    fn plugin_commands_from_a_release_say_they_need_a_checkout() {
        let release = Scratch::new("release");
        let env = |root: &Path| Env {
            paths: Paths::new(release.join("data")),
            root: root.to_path_buf(),
            web_dir: release.join("web/dist"),
            home: release.join("home"),
        };
        let err = run(&env(&release), &["CI".into(), "--yes".into()]).unwrap_err();
        assert!(err.contains("is a release without them"), "{err}");
        let err = skill_command(&env(&release), &[]).unwrap_err();
        assert!(err.contains("git clone --branch v"), "{err}");
        assert!(!release.join("home").exists());
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert!(cli::toolchain(&env(checkout)).is_ok());
    }

    #[test]
    fn slugs_are_plugin_ids() {
        assert_eq!(slug("CI status"), "ci-status");
        assert_eq!(slug("  Täst -- Plugin!! "), "t-st-plugin");
        assert_eq!(slug("3D viewer"), "p-3d-viewer");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn render_fills_values_and_backend_blocks() {
        let t = "a {{name}}\n{{#backend}}\nwith\n{{/backend}}\n{{^backend}}\nwithout\n{{/backend}}\nend\n";
        let vars = [("name", "X".to_string())];
        assert_eq!(render(t, &vars, true), "a X\nwith\nend\n");
        assert_eq!(render(t, &vars, false), "a X\nwithout\nend\n");
    }

    #[test]
    fn the_first_prompt_carries_the_job() {
        let a = Answers {
            name: "CI".into(),
            id: "ci".into(),
            description: "Show the last CI run.".into(),
            kind: Kind::Stream,
            backend: true,
            dir: PathBuf::from("/tmp/ci"),
            agent: None,
        };
        let p = first_prompt(&a, "briardr");
        assert!(
            p.contains("Show the last CI run.")
                && p.contains("briardr plugin build .")
                && p.contains("backend")
        );
        let m = manifest(&a);
        assert_eq!(m.contributes.stream_tabs[0].id, "ci");
        assert_eq!(
            json!(m)["backend"]["command"],
            json!(["bun", "server/main.ts"])
        );
    }
}
