// Plugins: the built-in features that can be turned off (Files, Changes, Preview, Docs, Notes, Usage)
// and plugins installed in the data directory. Each has a manifest (`briardr-plugin.json`) saying what it
// adds to the app. Turning a plugin off hides it on every device and refuses its routes.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::{read_json, write_file_atomic, ConfigError, Paths};

pub mod backend;
pub mod cli;
pub mod create;

/// The plugin API version this Briardr provides (`"briardr"` in a manifest).
pub const API_VERSION: u32 = 1;

pub const MANIFEST: &str = "briardr-plugin.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub briardr: u32,
    /// The client entry, relative to the plugin folder (built by `briardr plugin build`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendSpec>,
    #[serde(default)]
    pub contributes: Contributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackendSpec {
    /// Program and arguments, run in the plugin folder. Never through a shell.
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contributes {
    #[serde(default)]
    pub stream_tabs: Vec<Tab>,
    #[serde(default)]
    pub app_tabs: Vec<Tab>,
    #[serde(default)]
    pub settings: Vec<Section>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub id: String,
    pub label: String,
}

/// Built-in plugins, in tab order: `plugins/<id>/` in this repository, made like any installed plugin
/// (the same manifest, and a client that uses only @briardr/sdk and Svelte). Their client code is part
/// of the app bundle, so they work on a first launch and offline.
const BUILTIN_MANIFESTS: &[&str] = &[
    include_str!("../../../plugins/files/briardr-plugin.json"),
    include_str!("../../../plugins/changes/briardr-plugin.json"),
    include_str!("../../../plugins/preview/briardr-plugin.json"),
    include_str!("../../../plugins/docs/briardr-plugin.json"),
    include_str!("../../../plugins/notes/briardr-plugin.json"),
    include_str!("../../../plugins/usage/briardr-plugin.json"),
];

pub fn builtins() -> Vec<Manifest> {
    BUILTIN_MANIFESTS
        .iter()
        .map(|m| serde_json::from_str(m).expect("built-in plugin manifest"))
        .collect()
}

pub fn is_builtin(id: &str) -> bool {
    builtins().iter().any(|m| m.id == id)
}

/// Which built-in plugin an API route belongs to; core routes belong to none.
pub fn route_owner(pattern: &str) -> Option<&'static str> {
    const OWNERS: &[(&str, &str)] = &[
        ("/api/streams/:id/files", "files"),
        ("/api/streams/:id/changes", "changes"),
        ("/api/streams/:id/docs", "docs"),
        ("/api/streams/:id/notes", "notes"),
        ("/api/previews", "preview"),
        ("/api/usage", "usage"),
    ];
    OWNERS
        .iter()
        .find(|(prefix, _)| {
            pattern == *prefix
                || pattern
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .map(|(_, id)| *id)
}

/// Plugins turned on or off for every device, and the backend commands the user approved, by plugin
/// ID. Kept apart from config.json so `briardr plugin enable` can change it while the server runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginState {
    #[serde(default)]
    pub enabled: BTreeMap<String, bool>,
    /// sha256 of the approved backend command (`backend_hash`).
    #[serde(default)]
    pub approved: BTreeMap<String, String>,
}

impl PluginState {
    pub fn load(paths: &Paths) -> PluginState {
        read_json(&paths.plugin_state(), PluginState::default())
    }

    pub fn save(&self, paths: &Paths) -> Result<(), ConfigError> {
        let mut data = serde_json::to_string_pretty(self)?;
        data.push('\n');
        write_file_atomic(&paths.plugin_state(), data.as_bytes(), 0o600)
    }

    /// Built-ins are on unless turned off; installed plugins are off unless turned on.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled
            .get(id)
            .copied()
            .unwrap_or_else(|| is_builtin(id))
    }

    /// Turns a plugin on or off; a built-in turned back on leaves no entry.
    pub fn set_enabled(&mut self, id: &str, on: bool) {
        if on && is_builtin(id) {
            self.enabled.remove(id);
        } else {
            self.enabled.insert(id.to_string(), on);
        }
    }
}

/// The plugin state as the server sees it: read again whenever the file changes on disk.
pub struct StateCell {
    paths: Paths,
    cached: Mutex<(Option<SystemTime>, PluginState)>,
}

impl StateCell {
    pub fn new(paths: Paths) -> StateCell {
        let state = PluginState::load(&paths);
        let mtime = modified(&paths.plugin_state());
        StateCell {
            paths,
            cached: Mutex::new((mtime, state)),
        }
    }

    pub fn get(&self) -> PluginState {
        let mtime = modified(&self.paths.plugin_state());
        let mut c = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if c.0 != mtime {
            *c = (mtime, PluginState::load(&self.paths));
        }
        c.1.clone()
    }

    pub fn set(&self, state: PluginState) -> Result<(), ConfigError> {
        let saved = state.save(&self.paths);
        let mtime = modified(&self.paths.plugin_state());
        *self.cached.lock().unwrap_or_else(|e| e.into_inner()) = (mtime, state);
        saved
    }
}

fn modified(file: &Path) -> Option<SystemTime> {
    fs::metadata(file).and_then(|m| m.modified()).ok()
}

/// What approving a backend approves: its command. A changed command needs approval again.
pub fn backend_hash(spec: &BackendSpec) -> String {
    let text = serde_json::to_string(&spec.command).unwrap_or_default();
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Names a plugin or tab ID can't take: core routes and paths the app shell serves.
const RESERVED: &[&str] = &[
    "agents",
    "workspaces",
    "settings",
    "stream",
    "terminal",
    "api",
    "ws",
    "sdk",
    "plugins",
    "assets",
    "icons",
    "briardr",
];

fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !id.ends_with('-')
}

/// Checks an installed plugin's manifest; `taken` holds the plugin and tab IDs already in use.
pub fn validate(m: &Manifest, taken: &HashSet<String>) -> Result<(), String> {
    if !valid_id(&m.id) {
        return Err(format!(
            "“{}” isn’t a valid plugin ID: use lowercase letters, digits and dashes, starting with a letter.",
            m.id
        ));
    }
    if RESERVED.contains(&m.id.as_str()) || taken.contains(&format!("plugin:{}", m.id)) {
        return Err(format!("The plugin ID “{}” is already taken.", m.id));
    }
    if m.briardr != API_VERSION {
        return Err(format!(
            "Made for plugin API {}; this Briardr provides {API_VERSION}.",
            m.briardr
        ));
    }
    if m.name.trim().is_empty() {
        return Err("The manifest has no name.".to_string());
    }
    let tabs = m.contributes.stream_tabs.iter().map(|t| ("stream", &t.id));
    let apps = m.contributes.app_tabs.iter().map(|t| ("app", &t.id));
    let sections = m.contributes.settings.iter().map(|s| ("settings", &s.id));
    for (kind, id) in tabs.chain(apps).chain(sections) {
        if !valid_id(id) || RESERVED.contains(&id.as_str()) {
            return Err(format!("“{id}” can’t be used as an ID."));
        }
        if taken.contains(&format!("{kind}:{id}")) {
            return Err(format!("Another plugin already uses the {kind} ID “{id}”."));
        }
    }
    if m.contributes.stream_tabs.len() + m.contributes.app_tabs.len() + m.contributes.settings.len()
        > 0
        && m.client.is_none()
    {
        return Err("The manifest adds tabs or settings but has no client entry.".to_string());
    }
    if let Some(b) = &m.backend {
        if b.command.is_empty() || b.command[0].trim().is_empty() {
            return Err("The backend has no command.".to_string());
        }
    }
    Ok(())
}

pub(crate) fn claim(taken: &mut HashSet<String>, m: &Manifest) {
    taken.insert(format!("plugin:{}", m.id));
    for t in &m.contributes.stream_tabs {
        taken.insert(format!("stream:{}", t.id));
    }
    for t in &m.contributes.app_tabs {
        taken.insert(format!("app:{}", t.id));
    }
    for s in &m.contributes.settings {
        taken.insert(format!("settings:{}", s.id));
    }
}

/// `dist/build.json`, written by `briardr plugin build` (scripts/plugin-build.ts).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BuildStamp {
    /// The Svelte compiler and plugin API the client was built for (`runtime.json` of the web client).
    pub runtime: String,
    /// The client's entry file in `dist/`; none for a plugin without a client.
    pub entry: Option<String>,
}

/// The runtime this web client provides plugins (`<web dir>/sdk/v1/runtime.json`, written by its build).
pub fn web_runtime(web_dir: &Path) -> Option<String> {
    let v: Value = read_json(&web_dir.join("sdk/v1/runtime.json"), Value::Null);
    v.get("runtime").and_then(Value::as_str).map(str::to_string)
}

/// A plugin as Briardr sees it: built in, or installed with a manifest that may be broken.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub id: String,
    pub builtin: bool,
    pub dir: Option<PathBuf>,
    pub manifest: Option<Manifest>,
    pub build: Option<BuildStamp>,
    /// Why an installed plugin can't be used (bad or unreadable manifest, ID clash).
    pub problem: Option<String>,
}

impl Plugin {
    pub fn usable(&self) -> bool {
        self.manifest.is_some() && self.problem.is_none()
    }

    /// Turned on, usable, and with its backend's current command approved: what Briardr serves and runs.
    pub fn active(&self, state: &PluginState) -> bool {
        let approved = match self.manifest.as_ref().and_then(|m| m.backend.as_ref()) {
            Some(b) => state.approved.get(&self.id) == Some(&backend_hash(b)),
            None => true,
        };
        self.usable() && approved && state.is_enabled(&self.id)
    }

    /// "ok", or why the client can't load: never built ("missing") or built for another runtime ("stale").
    pub fn build_status(&self, runtime: Option<&str>) -> &'static str {
        let wants_client = self.manifest.as_ref().is_some_and(|m| m.client.is_some());
        match &self.build {
            _ if self.builtin => "ok",
            None => "missing",
            Some(b) if Some(b.runtime.as_str()) != runtime => "stale",
            Some(b) if wants_client && b.entry.is_none() => "missing",
            Some(_) => "ok",
        }
    }

    /// Where devices load the client from, when it's built for this runtime.
    pub fn client_url(&self, runtime: Option<&str>) -> Option<String> {
        if self.build_status(runtime) != "ok" {
            return None;
        }
        let entry = self.build.as_ref()?.entry.as_ref()?;
        Some(format!("/plugins/{}/{entry}", self.id))
    }
}

/// A file of an active plugin's client build, for `/plugins/<id>/<rest>`; never outside its `dist/`.
pub fn client_file(paths: &Paths, state: &PluginState, id: &str, rest: &str) -> Option<PathBuf> {
    if rest.is_empty()
        || rest
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || c.starts_with('.'))
    {
        return None;
    }
    let plugin = discover(paths)
        .into_iter()
        .find(|p| p.id == id && !p.builtin)?;
    if !plugin.active(state) {
        return None;
    }
    let dist = fs::canonicalize(plugin.dir?.join("dist")).ok()?;
    let file = fs::canonicalize(dist.join(rest)).ok()?;
    (file.starts_with(&dist) && file.is_file()).then_some(file)
}

/// Every plugin: built-ins in tab order, then installed plugins by ID.
pub fn discover(paths: &Paths) -> Vec<Plugin> {
    let mut taken = HashSet::new();
    let mut out: Vec<Plugin> = builtins()
        .into_iter()
        .map(|m| {
            claim(&mut taken, &m);
            Plugin {
                id: m.id.clone(),
                builtin: true,
                dir: None,
                manifest: Some(m),
                build: None,
                problem: None,
            }
        })
        .collect();
    let mut dirs: Vec<(String, PathBuf)> = fs::read_dir(paths.plugins())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (!name.starts_with('.')).then(|| (name, e.path()))
        })
        .filter(|(_, p)| p.is_dir())
        .collect();
    dirs.sort();
    for (name, dir) in dirs {
        let (manifest, problem) = match read_manifest(&dir) {
            Ok(m) if m.id != name => (
                Some(m.clone()),
                Some(format!(
                    "The folder is named “{name}” but the manifest’s ID is “{}”.",
                    m.id
                )),
            ),
            Ok(m) => {
                let problem = validate(&m, &taken).err();
                if problem.is_none() {
                    claim(&mut taken, &m);
                }
                (Some(m), problem)
            }
            Err(e) => (None, Some(e)),
        };
        let build = read_json(&dir.join("dist/build.json"), None);
        out.push(Plugin {
            id: name,
            builtin: false,
            dir: Some(dir),
            manifest,
            build,
            problem,
        });
    }
    out
}

/// What changes when a plugin is installed, rebuilt, edited or turned on or off on the host: the
/// modification times of the plugin state, each plugin's manifest and its build stamp.
pub fn fingerprint(paths: &Paths) -> Vec<(PathBuf, Option<SystemTime>)> {
    let mut out = vec![(paths.plugin_state(), modified(&paths.plugin_state()))];
    let mut dirs: Vec<PathBuf> = fs::read_dir(paths.plugins())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for d in dirs {
        for f in [d.join(MANIFEST), d.join("dist/build.json")] {
            let m = modified(&f);
            out.push((f, m));
        }
    }
    out
}

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(dir.join(MANIFEST))
        .map_err(|e| format!("Couldn’t read {MANIFEST}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{MANIFEST} isn’t valid: {e}"))
}

/// The plugin list the client and `briardr plugin list` show.
pub fn info_json(p: &Plugin, state: &PluginState, runtime: Option<&str>) -> Value {
    let m = p.manifest.as_ref();
    let backend = m.and_then(|m| m.backend.as_ref()).map(|b| {
        let hash = backend_hash(b);
        json!({
            "command": b.command,
            "approved": state.approved.get(&p.id) == Some(&hash),
            "hash": hash,
        })
    });
    json!({
        "id": p.id,
        "name": m.map_or(p.id.as_str(), |m| m.name.as_str()),
        "description": m.map_or("", |m| m.description.as_str()),
        "version": m.map_or("", |m| m.version.as_str()),
        "builtin": p.builtin,
        "enabled": p.active(state),
        "contributes": m.map(|m| serde_json::to_value(&m.contributes).unwrap_or(Value::Null)),
        "backend": backend,
        "client": p.active(state).then(|| p.client_url(runtime)).flatten(),
        "build": p.build_status(runtime),
        "problem": p.problem,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    fn manifest(v: Value) -> Manifest {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn builtins_parse_in_tab_order() {
        let ids: Vec<String> = builtins().into_iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            ["files", "changes", "preview", "docs", "notes", "usage"]
        );
    }

    #[test]
    fn routes_belong_to_their_plugin() {
        assert_eq!(route_owner("/api/streams/:id/files"), Some("files"));
        assert_eq!(route_owner("/api/streams/:id/files/search"), Some("files"));
        assert_eq!(
            route_owner("/api/streams/:id/changes/diff"),
            Some("changes")
        );
        assert_eq!(route_owner("/api/streams/:id/notes/:note"), Some("notes"));
        assert_eq!(route_owner("/api/previews/:id"), Some("preview"));
        assert_eq!(route_owner("/api/usage/statusline"), Some("usage"));
        assert_eq!(route_owner("/api/streams/:id/uploads"), None);
        assert_eq!(route_owner("/api/streams/:id/info"), None);
        assert_eq!(route_owner("/api/usagex"), None);
    }

    #[test]
    fn builtins_default_on_and_installed_default_off() {
        let mut state = PluginState::default();
        assert!(state.is_enabled("files"));
        assert!(!state.is_enabled("ci"));
        state.set_enabled("files", false);
        state.set_enabled("ci", true);
        assert!(!state.is_enabled("files"));
        assert!(state.is_enabled("ci"));
        state.set_enabled("files", true);
        assert_eq!(state.enabled.get("files"), None);
    }

    #[test]
    fn state_cell_sees_changes_made_by_another_process() {
        let root = Scratch::new("plugin-state");
        let paths = Paths::new(root.path());
        let cell = StateCell::new(paths.clone());
        assert_eq!(cell.get(), PluginState::default());
        let mut other = PluginState::default();
        other.set_enabled("docs", false);
        other.save(&paths).unwrap();
        // A changed modification time is what makes the cell read again.
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(paths.plugin_state())
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(!cell.get().is_enabled("docs"));
    }

    #[test]
    fn validate_rejects_bad_ids_versions_and_clashes() {
        let taken: HashSet<String> = builtins().iter().fold(HashSet::new(), |mut t, m| {
            claim(&mut t, m);
            t
        });
        let ok = manifest(
            json!({"id": "ci", "name": "CI", "briardr": 1, "client": "src/index.ts",
            "contributes": {"streamTabs": [{"id": "ci", "label": "CI"}]}}),
        );
        assert_eq!(validate(&ok, &taken), Ok(()));
        let bad_id = manifest(json!({"id": "CI", "name": "CI", "briardr": 1}));
        assert!(validate(&bad_id, &taken).is_err());
        let reserved = manifest(json!({"id": "settings", "name": "S", "briardr": 1}));
        assert!(validate(&reserved, &taken).is_err());
        let newer = manifest(json!({"id": "ci", "name": "CI", "briardr": 2}));
        assert!(validate(&newer, &taken).unwrap_err().contains("API 2"));
        let clash = manifest(
            json!({"id": "ci", "name": "CI", "briardr": 1, "client": "x.ts",
            "contributes": {"streamTabs": [{"id": "files", "label": "F"}]}}),
        );
        assert!(validate(&clash, &taken)
            .unwrap_err()
            .contains("already uses"));
        let no_client = manifest(json!({"id": "ci", "name": "CI", "briardr": 1,
            "contributes": {"appTabs": [{"id": "ci", "label": "CI"}]}}));
        assert!(validate(&no_client, &taken).is_err());
        let empty_cmd =
            manifest(json!({"id": "ci", "name": "CI", "briardr": 1, "backend": {"command": []}}));
        assert!(validate(&empty_cmd, &taken).is_err());
    }

    #[test]
    fn discover_reads_installed_plugins_and_reports_problems() {
        let root = Scratch::new("plugins");
        let paths = Paths::new(root.path());
        let write = |dir: &str, v: Value| {
            let d = paths.plugins().join(dir);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join(MANIFEST), v.to_string()).unwrap();
        };
        write("ci", json!({"id": "ci", "name": "CI", "briardr": 1}));
        write(
            "misnamed",
            json!({"id": "other", "name": "O", "briardr": 1}),
        );
        fs::create_dir_all(paths.plugins().join("empty")).unwrap();
        let all = discover(&paths);
        let find = |id: &str| all.iter().find(|p| p.id == id).unwrap();
        assert_eq!(all.len(), builtins().len() + 3);
        assert!(find("files").builtin && find("files").usable());
        assert!(find("ci").usable());
        assert!(find("misnamed")
            .problem
            .as_deref()
            .unwrap()
            .contains("folder"));
        assert!(find("empty")
            .problem
            .as_deref()
            .unwrap()
            .contains("Couldn’t read"));
    }

    #[test]
    fn backend_hash_changes_with_the_command() {
        let a = BackendSpec {
            command: vec!["bun".into(), "server.ts".into()],
        };
        let b = BackendSpec {
            command: vec!["bun".into(), "other.ts".into()],
        };
        assert_eq!(backend_hash(&a), backend_hash(&a.clone()));
        assert_ne!(backend_hash(&a), backend_hash(&b));
    }
}
