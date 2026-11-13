// Briardr's workspace/project/stream model from herdr-core's live terminals and the Herdr app's
// layout documents (`herdr-app`, one per machine): a space is a workspace, a tab a project, a
// terminal a stream. Another machine's ids and terminals are named as the core addresses them
// (`rohan/t7`), and its streams have no folder here.
// The document is the app's own format, read only for names, order and which tab shows which
// terminal, and things are named as the app names them; labels (`herdr.space`, `herdr.tab`) place terminals it doesn't show yet, as the app
// adopts them. Status is what the program reported (OSC 7501), read with the app's `herdr.seen`
// rule (its docs/labels.md).
use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::herdr::model::{
    classify_process, tool_label, AgentState, Model, Project, Stream, StreamKind, Workspace,
};

/// Where terminals nothing places go.
pub const OTHER: &str = "other";

/// An id or address on `machine`, as seen from here; this computer's ("") stay as they are.
pub fn qualify(machine: &str, id: &str) -> String {
    if machine.is_empty() {
        id.to_string()
    } else {
        format!("{machine}/{id}")
    }
}

/// The machine of an id or address seen from here ("" for this computer), and the part local to it.
pub fn split_machine(id: &str) -> (&str, &str) {
    id.split_once('/').unwrap_or(("", id))
}

fn machine_of(machine: &str) -> Option<String> {
    (!machine.is_empty()).then(|| machine.to_string())
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn list<'a>(v: Option<&'a Value>, key: &str) -> &'a [Value] {
    v.and_then(|v| v.get(key))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn label<'a>(t: &'a Value, key: &str) -> Option<&'a str> {
    t.get("labels")
        .and_then(|l| l.get(key))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// The terminals a layout node shows, left to right and top to bottom.
fn leaves<'a>(node: &'a Value, out: &mut Vec<&'a str>) {
    if let Some(t) = node.pointer("/content/terminal").and_then(Value::as_str) {
        out.push(t);
    }
    for child in list(Some(node), "children") {
        leaves(child, out);
    }
}

/// The tool a status report names (`claude-code`), else the foreground program's.
fn tool_of(t: &Value) -> Option<String> {
    let app = t
        .pointer("/status/app")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    match app.as_str() {
        "" => None,
        a if a.starts_with("claude") => Some("claude".to_string()),
        a if a.starts_with("codex") => Some("codex".to_string()),
        a => Some(a.to_string()),
    }
}

/// The app's rule: blocked or error needs you, working works, and anything else is done when a turn
/// ended since a person last looked (`herdr.seen`), else idle. Without the label, the word decides.
fn agent_state(t: &Value) -> AgentState {
    let word = t.pointer("/status/state").and_then(Value::as_str);
    let seen = label(t, "herdr.seen").and_then(|s| s.parse::<u64>().ok());
    let turn = t.get("turn").and_then(Value::as_u64).unwrap_or(0);
    match (word, seen) {
        (Some("blocked" | "error"), _) => AgentState::Blocked,
        (Some("working"), _) => AgentState::Working,
        (Some(_), Some(seen)) if turn > seen => AgentState::Done,
        (Some(_), Some(_)) => AgentState::Idle,
        (Some("done"), None) => AgentState::Done,
        (Some("idle"), None) => AgentState::Idle,
        _ => AgentState::Unknown,
    }
}

fn stream(t: &Value, workspace: &str, project: &str, prev: Option<&Stream>, now: u64) -> Stream {
    let address = text(t, "address").unwrap_or("");
    let foreground = t.get("foreground").filter(|f| f.is_object());
    let program = foreground.and_then(|f| text(f, "name"));
    let agent = t.get("reported").and_then(Value::as_bool) == Some(true)
        || t.get("status").is_some_and(Value::is_object);
    let (kind, tool) = if agent {
        let tool = tool_of(t).unwrap_or_else(|| classify_process(program).1);
        (StreamKind::Agent, tool)
    } else {
        classify_process(program)
    };
    let state = (kind == StreamKind::Agent).then(|| agent_state(t));
    let label = text(t, "name").map(String::from);
    let title = text(t, "title");
    let name = label
        .clone()
        .or_else(|| title.map(String::from))
        .unwrap_or_else(|| match kind {
            StreamKind::Agent | StreamKind::Editor => tool_label(&tool),
            StreamKind::Program => tool.clone(),
            StreamKind::Shell => "Shell".to_string(),
        });
    let cwd = foreground
        .and_then(|f| text(f, "cwd"))
        .or_else(|| text(t, "cwd"));
    let (machine, _) = split_machine(address);
    let state_since = match prev {
        Some(p) if p.state == state => p.state_since,
        _ if state.is_some() => t.get("status_ms").and_then(Value::as_u64).or(Some(now)),
        _ => None,
    };
    Stream {
        id: address.to_string(),
        workspace_id: workspace.to_string(),
        project_id: project.to_string(),
        terminal_id: address.to_string(),
        name,
        label,
        kind,
        tool,
        state,
        state_change_seq: state.and(t.get("turn").and_then(Value::as_u64)),
        state_since,
        cwd: cwd.filter(|_| machine.is_empty()).map(String::from),
        folder: cwd.map(|c| basename(c).to_string()),
        focused: false,
        title: title.map(String::from),
        rows: t.get("rows").and_then(Value::as_u64),
        machine: machine_of(machine),
    }
}

/// `terminals` are the core's records (`events.subscribe` order); `docs` are the `herdr-app`
/// documents by machine ("" for this computer's, which comes first, as the app shows machines).
pub fn normalize(
    terminals: &[Value],
    docs: &BTreeMap<String, Value>,
    prev: Option<&Model>,
    version: Option<&str>,
    now: u64,
) -> Model {
    let live: Vec<&Value> = terminals
        .iter()
        .filter(|t| t.get("state").and_then(Value::as_str) == Some("live"))
        .collect();
    let by_address: HashMap<&str, &Value> = live
        .iter()
        .filter_map(|t| Some((text(t, "address")?, *t)))
        .collect();
    let prev_streams: HashMap<&str, &Stream> = prev
        .map(|m| m.streams.iter().map(|s| (s.id.as_str(), s)).collect())
        .unwrap_or_default();

    let mut workspaces: Vec<Workspace> = Vec::new();
    let mut projects: Vec<Project> = Vec::new();
    // (workspace, project, address) in display order.
    let mut placed: Vec<(String, String, &str)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut folders: HashMap<String, String> = HashMap::new();
    for (machine, doc) in docs {
        for space in list(Some(doc), "spaces") {
            let Some(id) = text(space, "id").map(|id| qualify(machine, id)) else {
                continue;
            };
            workspaces.push(Workspace {
                id: id.clone(),
                name: text(space, "name").unwrap_or("").to_string(),
                number: 0,
                focused: false,
                active_project_id: None,
                cwd: text(space, "folder").map(String::from),
                machine: machine_of(machine),
            });
            if let Some(folder) = text(space, "folder") {
                folders.insert(id.clone(), folder.to_string());
            }
            for tab in list(Some(space), "tabs") {
                let Some(tab_id) = text(tab, "id").map(|t| qualify(machine, t)) else {
                    continue;
                };
                projects.push(Project {
                    id: tab_id.clone(),
                    workspace_id: id.clone(),
                    name: text(tab, "name").unwrap_or("").to_string(),
                    number: 0,
                    focused: false,
                });
                let mut shown = Vec::new();
                if let Some(root) = tab.get("root") {
                    leaves(root, &mut shown);
                }
                for local in shown {
                    let Some((&address, _)) =
                        by_address.get_key_value(qualify(machine, local).as_str())
                    else {
                        continue;
                    };
                    if seen.insert(address) {
                        placed.push((id.clone(), tab_id.clone(), address));
                    }
                }
            }
        }
    }

    // Terminals labelled for a space and tab the document doesn't show (yet): there, as the app adopts them.
    for t in &live {
        let (Some(address), Some(space), Some(tab)) = (
            text(t, "address"),
            label(t, "herdr.space"),
            label(t, "herdr.tab"),
        ) else {
            continue;
        };
        if seen.contains(address) {
            continue;
        }
        // Labels name the space and tab as the terminal's own machine knows them.
        let machine = split_machine(address).0;
        let (space, tab) = (qualify(machine, space), qualify(machine, tab));
        if !workspaces.iter().any(|w| w.id == space) {
            workspaces.push(Workspace {
                id: space.clone(),
                name: String::new(),
                number: 0,
                focused: false,
                active_project_id: None,
                cwd: None,
                machine: machine_of(machine),
            });
        }
        if !projects.iter().any(|p| p.id == tab) {
            projects.push(Project {
                id: tab.clone(),
                workspace_id: space.clone(),
                name: String::new(),
                number: 0,
                focused: false,
            });
        }
        seen.insert(address);
        placed.push((space, tab, address));
    }

    // Helpers an agent started belong to its tab; everything else goes to Other.
    let mut other: Vec<&str> = Vec::new();
    for t in &live {
        let Some(address) = text(t, "address") else {
            continue;
        };
        if seen.contains(address) {
            continue;
        }
        let parent =
            text(t, "parent").map(|p| qualify(split_machine(address).0, split_machine(p).1));
        let parent = parent.and_then(|p| placed.iter().find(|(_, _, a)| *a == p));
        match parent {
            Some((w, p, _)) => {
                let (w, p) = (w.clone(), p.clone());
                placed.push((w, p, address));
            }
            None => other.push(address),
        }
        seen.insert(address);
    }
    if !other.is_empty() {
        workspaces.push(Workspace {
            id: OTHER.to_string(),
            name: "Other terminals".to_string(),
            number: 0,
            focused: false,
            active_project_id: None,
            cwd: None,
            machine: None,
        });
        projects.push(Project {
            id: OTHER.to_string(),
            workspace_id: OTHER.to_string(),
            name: "Terminals".to_string(),
            number: 1,
            focused: false,
        });
        placed.extend(
            other
                .into_iter()
                .map(|a| (OTHER.to_string(), OTHER.to_string(), a)),
        );
    }

    let streams: Vec<Stream> = placed
        .iter()
        .filter_map(|(w, p, address)| {
            let t = by_address.get(address)?;
            Some(stream(t, w, p, prev_streams.get(address).copied(), now))
        })
        .collect();

    // As in the app, a tab closes with its last terminal and a space with its last tab (the
    // document keeps them until the app runs), and an unnamed tab is called by its number.
    projects.retain(|p| streams.iter().any(|s| s.project_id == p.id));
    workspaces.retain(|w| projects.iter().any(|p| p.workspace_id == w.id));
    for (i, w) in workspaces.iter_mut().enumerate() {
        w.number = i as u64 + 1;
        for (j, p) in projects
            .iter_mut()
            .filter(|p| p.workspace_id == w.id)
            .enumerate()
        {
            p.number = j as u64 + 1;
            if p.name.is_empty() {
                p.name = p.number.to_string();
            }
        }
    }

    // An unnamed space is called after its folder, as in the app.
    for w in &mut workspaces {
        let first_cwd = streams
            .iter()
            .find(|s| s.workspace_id == w.id)
            .and_then(|s| s.cwd.clone());
        if w.cwd.is_none() {
            w.cwd = folders.get(&w.id).cloned().or(first_cwd);
        }
        if w.name.is_empty() {
            w.name = w
                .cwd
                .as_deref()
                .map(basename)
                .filter(|b| !b.is_empty())
                .map_or_else(|| format!("Workspace {}", w.number), String::from);
        }
    }

    Model {
        workspaces,
        projects,
        streams,
        focused_stream_id: None,
        herdr_version: version.map(String::from),
        as_of: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn terminal(address: &str, extra: Value) -> Value {
        let mut t = json!({
            "address": address, "state": "live", "labels": {}, "argv": [], "cwd": "/w/app",
            "title": "", "name": null, "status": null, "reported": false, "turn": 0, "rows": 40,
            "foreground": {"pid": 1, "name": "zsh", "argv": ["zsh"], "cwd": "/w/app"},
        });
        if let (Some(t), Value::Object(extra)) = (t.as_object_mut(), extra) {
            t.extend(extra);
        }
        t
    }

    /// This computer's document alone.
    fn here(doc: Value) -> BTreeMap<String, Value> {
        BTreeMap::from([(String::new(), doc)])
    }

    fn doc() -> Value {
        json!({"spaces": [{
            "id": "sp1", "name": "", "folder": "/w/app",
            "tabs": [
                {"id": "tb1", "name": "review", "root": {"id": "r", "type": "row", "children": [
                    {"id": "a", "type": "block", "content": {"terminal": "t2"}},
                    {"id": "b", "type": "block", "content": {"terminal": "t1"}},
                ]}},
                {"id": "tb2", "name": "", "root": {"id": "c", "type": "block", "content": {"terminal": "t9"}}},
            ],
        }]})
    }

    #[test]
    fn spaces_tabs_and_panes_follow_the_document() {
        let mut d = doc();
        let tabs = d["spaces"][0]["tabs"].as_array_mut().unwrap();
        tabs.push(
            json!({"id": "tb3", "name": "", "root": {"id": "tb3", "type": "row", "children": [
            {"id": "e", "type": "block", "content": {"terminal": "t3"}}]}}),
        );
        d["spaces"].as_array_mut().unwrap().push(json!({"id": "sp2", "name": "gone", "folder": "/w",
            "tabs": [{"id": "tb4", "name": "", "root": {"id": "tb4", "type": "block", "content": {"terminal": "t8"}}}]}));
        let ts = vec![
            terminal("t1", json!({})),
            terminal("t2", json!({})),
            terminal("t3", json!({})),
        ];
        let m = normalize(&ts, &here(d), None, Some("0.1.20"), 5);
        assert_eq!(
            m.workspaces.len(),
            1,
            "a space whose terminals all ended closed with them"
        );
        assert_eq!(
            m.workspaces[0].name, "app",
            "an unnamed space is called after its folder"
        );
        let names: Vec<&str> = m.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["review", "2"],
            "a tab whose terminals ended is gone; an unnamed tab is called by its place"
        );
        let order: Vec<&str> = m.streams.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            order,
            ["t2", "t1", "t3"],
            "the split's order, not the core's"
        );
        assert!(m.streams[..2].iter().all(|s| s.project_id == "tb1"));
        assert_eq!(m.streams[0].kind, StreamKind::Shell);
        assert_eq!(m.herdr_version.as_deref(), Some("0.1.20"));
    }

    #[test]
    fn labelled_helpers_and_stray_terminals_are_placed() {
        let ts = vec![
            terminal("t1", json!({})),
            terminal(
                "t3",
                json!({"labels": {"herdr.space": "sp1", "herdr.tab": "tb2"}}),
            ),
            terminal("t4", json!({"parent": "t1"})),
            terminal("t5", json!({})),
            terminal("t6", json!({"state": "finished"})),
        ];
        let m = normalize(&ts, &here(doc()), None, None, 5);
        let at: Vec<(&str, &str)> = m
            .streams
            .iter()
            .map(|s| (s.id.as_str(), s.project_id.as_str()))
            .collect();
        assert_eq!(
            at,
            [("t1", "tb1"), ("t3", "tb2"), ("t4", "tb1"), ("t5", OTHER)]
        );
        assert_eq!(m.workspaces.last().unwrap().name, "Other terminals");
    }

    #[test]
    fn agent_status_uses_the_seen_label() {
        let agent = |status: &str, turn: u64, seen: Option<&str>| {
            let labels = seen.map_or_else(|| json!({}), |s| json!({"herdr.seen": s}));
            terminal(
                "t1",
                json!({"reported": true, "turn": turn, "labels": labels, "status_ms": 77,
                    "status": {"state": status, "app": "claude-code"},
                    "foreground": {"pid": 1, "name": "claude", "argv": [], "cwd": "/w"}}),
            )
        };
        let state = |t: Value| normalize(&[t], &BTreeMap::new(), None, None, 5).streams[0].state;
        assert_eq!(state(agent("idle", 3, Some("2"))), Some(AgentState::Done));
        assert_eq!(state(agent("done", 3, Some("3"))), Some(AgentState::Idle));
        assert_eq!(state(agent("done", 3, None)), Some(AgentState::Done));
        assert_eq!(state(agent("error", 3, None)), Some(AgentState::Blocked));
        assert_eq!(
            state(agent("working", 3, Some("9"))),
            Some(AgentState::Working)
        );
        let s = &normalize(
            &[agent("blocked", 4, None)],
            &BTreeMap::new(),
            None,
            None,
            5,
        )
        .streams[0];
        assert_eq!(
            (s.kind, s.tool.as_str(), s.name.as_str()),
            (StreamKind::Agent, "claude", "Claude")
        );
        assert_eq!((s.state_change_seq, s.state_since), (Some(4), Some(77)));
    }

    #[test]
    fn names_prefer_the_terminal_name_then_its_title() {
        let ts = vec![
            terminal("t1", json!({"name": "web", "title": "vite"})),
            terminal("t2", json!({"title": "vite"})),
            terminal(
                "t3",
                json!({"foreground": {"pid": 1, "name": "nvim", "argv": [], "cwd": "/w/x"}}),
            ),
        ];
        let m = normalize(&ts, &BTreeMap::new(), None, None, 5);
        let names: Vec<&str> = m.streams.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["web", "vite", "Neovim"]);
        assert_eq!(m.streams[0].label.as_deref(), Some("web"));
        assert_eq!(m.streams[2].folder.as_deref(), Some("x"));
    }

    #[test]
    fn another_machines_spaces_are_named_as_the_core_addresses_them() {
        let mut docs = here(doc());
        docs.insert(
            "rohan".into(),
            json!({"spaces": [{"id": "sp9", "name": "api", "folder": "/srv/api",
            "tabs": [{"id": "tb9", "name": "", "root": {"id": "tb9", "type": "row", "children": [
                {"id": "p1", "type": "block", "content": {"terminal": "t1"}}]}}]}]}),
        );
        let labelled = json!({"labels": {"herdr.space": "sp9", "herdr.tab": "tb9"}});
        let ts = vec![
            terminal("t1", json!({})),
            terminal("rohan/t1", json!({})),
            terminal("rohan/t2", labelled),
        ];
        let m = normalize(&ts, &docs, None, None, 5);
        let names: Vec<(&str, Option<&str>)> = m
            .workspaces
            .iter()
            .map(|w| (w.id.as_str(), w.machine.as_deref()))
            .collect();
        assert_eq!(
            names,
            [("sp1", None), ("rohan/sp9", Some("rohan"))],
            "this computer's first"
        );
        let remote: Vec<(&str, &str)> = m
            .streams
            .iter()
            .filter(|s| s.machine.is_some())
            .map(|s| (s.id.as_str(), s.project_id.as_str()))
            .collect();
        assert_eq!(
            remote,
            [("rohan/t1", "rohan/tb9"), ("rohan/t2", "rohan/tb9")]
        );
        let elsewhere = m.streams.iter().find(|s| s.id == "rohan/t1").unwrap();
        assert_eq!(
            (elsewhere.cwd.as_deref(), elsewhere.folder.as_deref()),
            (None, Some("app")),
            "no folder here"
        );
        assert_eq!(
            m.streams
                .iter()
                .find(|s| s.id == "t1")
                .unwrap()
                .cwd
                .as_deref(),
            Some("/w/app")
        );
    }
}
