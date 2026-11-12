// The same actions on herdr-core, under the Herdr app. Spaces and tabs are the app's layout document's
// (`herdr::core::layout`); terminals are the core's. Terminals start as the app starts them, labelled
// with their space and tab, so the app shows them there (it splits the tab's last pane). Closing
// ends terminals: the app closes a tab with its last terminal and a space with its last tab, and
// until it does, Briardr doesn't show a tab with none. The app keeps its focus: Briardr never moves it.
use std::path::Path;

use futures_util::future::join_all;
use serde_json::{json, Value};

use super::{err, from_herdr, ActionError, Actions};
use crate::herdr::core::layout;
use crate::herdr::core::model::OTHER;
use crate::herdr::core::state::CoreState;
use crate::herdr::model::{Project, Stream, StreamKind, Workspace};
use crate::herdr::state::HerdrReader;

/// The terminal identity the Herdr app gives its terminals.
fn env() -> Value {
    json!({"TERM": "xterm-256color", "COLORTERM": "truecolor", "TERM_PROGRAM": "herdr"})
}

/// An agent as the Herdr app starts one: in the person's shell (on macOS a login shell, for its
/// PATH), which stays when the agent exits.
fn agent_argv(program: &str, shell: Option<String>) -> Vec<String> {
    let sh = shell
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    let fish = Path::new(&sh).file_name().and_then(|s| s.to_str()) == Some("fish");
    let login = cfg!(target_os = "macos");
    let script = match (fish, login) {
        (true, true) => "command $argv[2..]; exec $argv[1] -li",
        (true, false) => "command $argv[2..]; exec $argv[1] -i",
        (false, true) => "\"$@\"; exec \"$0\" -li",
        (false, false) => "\"$@\"; exec \"$0\" -i",
    };
    let flags = if login { "-lic" } else { "-ic" };
    vec![sh.clone(), flags.into(), script.into(), sh, program.into()]
}

fn not_a_space() -> ActionError {
    err(
        409,
        "not_a_space",
        "These terminals aren’t in a space of the Herdr app.",
    )
}

fn not_in_layout() -> ActionError {
    err(
        409,
        "not_in_layout",
        "The Herdr app hasn’t placed this yet. Open it once, then try again.",
    )
}

impl Actions {
    pub(super) fn core(&self) -> Option<CoreState> {
        match self.herdr.backend() {
            crate::herdr::backend::Backend::Core(c) => Some(c),
            crate::herdr::backend::Backend::Tui(_) => None,
        }
    }

    /// Starts a terminal, labelled for the space and tab `place` names (none: Other terminals).
    async fn start(
        &self,
        argv: Vec<String>,
        cwd: Option<&str>,
        place: Option<(&str, &str)>,
    ) -> Result<String, ActionError> {
        let mut labels = json!({"herdr.seen": "0"});
        if let Some((space, tab)) = place {
            labels["herdr.space"] = json!(space);
            labels["herdr.tab"] = json!(tab);
        }
        let params =
            json!({"argv": argv, "cwd": cwd, "env": env(), "labels": labels, "keep": true});
        let r = self.core_call("terminal.run", params).await?;
        r.get("address")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| self.internal())
    }

    async fn edit_layout(
        &self,
        c: &CoreState,
        change: impl Fn(&mut Value) -> bool,
    ) -> Result<bool, ActionError> {
        c.edit_layout(change)
            .await
            .map_err(|e| from_herdr(&e, &self.host))
    }

    /// Ends these terminals, all at once. One already gone is fine.
    async fn end(&self, terminals: Vec<String>) -> Result<(), ActionError> {
        let kills = terminals
            .iter()
            .map(|t| self.core_call("terminal.kill", json!({"terminal": t})));
        for result in join_all(kills).await {
            match result {
                Err(e) if e.code != "herdr_not_found" => return Err(e),
                _ => {}
            }
        }
        self.refresh().await
    }

    pub(super) async fn core_create_workspace(
        &self,
        c: &CoreState,
        name: Option<String>,
        cwd: String,
    ) -> Result<Value, ActionError> {
        let (space, tab, pane) = (
            layout::fresh_id("sp"),
            layout::fresh_id("tb"),
            layout::fresh_id("p"),
        );
        let terminal = self
            .start(Vec::new(), Some(&cwd), Some((&space, &tab)))
            .await?;
        let name = name.unwrap_or_default();
        self.edit_layout(c, |doc| {
            layout::add_space(doc, &space, &name, &cwd, &tab, &pane, &terminal);
            true
        })
        .await?;
        self.refresh().await?;
        Ok(
            json!({"workspaceId": space, "projectId": tab, "streamId": terminal, "focusUnchanged": true}),
        )
    }

    pub(super) async fn core_create_project(
        &self,
        c: &CoreState,
        ws: &Workspace,
        name: Option<String>,
    ) -> Result<Value, ActionError> {
        if ws.id == OTHER {
            return Err(not_a_space());
        }
        // The app adopts a labelled terminal only into a tab it has: a new tab is written here.
        if !layout::has_space(c.layout().as_ref(), &ws.id) {
            return Err(not_in_layout());
        }
        let (tab, pane) = (layout::fresh_id("tb"), layout::fresh_id("p"));
        let cwd = ws.cwd.as_deref().filter(|c| !c.is_empty());
        let terminal = self.start(Vec::new(), cwd, Some((&ws.id, &tab))).await?;
        let name = name.unwrap_or_default();
        let added = self
            .edit_layout(c, |doc| {
                layout::add_tab(doc, &ws.id, &tab, &name, &pane, &terminal)
            })
            .await?;
        if !added {
            self.end(vec![terminal]).await?;
            return Err(err(
                404,
                "workspace_gone",
                "That workspace no longer exists.",
            ));
        }
        self.refresh().await?;
        Ok(json!({"projectId": tab, "streamId": terminal, "focusUnchanged": true}))
    }

    pub(super) async fn core_create_stream(
        &self,
        project: &Project,
        cwd: Option<String>,
        kind: &str,
    ) -> Result<Value, ActionError> {
        let argv = match kind {
            "shell" => Vec::new(),
            program => agent_argv(program, std::env::var("SHELL").ok()),
        };
        let place =
            (project.id != OTHER).then_some((project.workspace_id.as_str(), project.id.as_str()));
        let terminal = self.start(argv, cwd.as_deref(), place).await?;
        self.refresh().await?;
        Ok(json!({"streamId": terminal, "note": null, "focusUnchanged": true}))
    }

    /// Names a space or a tab in the layout document.
    pub(super) async fn core_rename(
        &self,
        c: &CoreState,
        id: &str,
        name: &str,
    ) -> Result<Value, ActionError> {
        if id == OTHER {
            return Err(not_a_space());
        }
        if !self
            .edit_layout(c, |doc| layout::rename(doc, id, name))
            .await?
        {
            return Err(not_in_layout());
        }
        self.refresh().await?;
        Ok(Value::Null)
    }

    /// Ends the terminals of what `belongs` picks.
    pub(super) async fn core_close(
        &self,
        belongs: impl Fn(&Stream) -> bool,
    ) -> Result<Value, ActionError> {
        let terminals = self
            .herdr
            .model()
            .map(|m| {
                m.streams
                    .into_iter()
                    .filter(|s| belongs(s))
                    .map(|s| s.id)
                    .collect()
            })
            .unwrap_or_default();
        self.end(terminals).await?;
        Ok(Value::Null)
    }

    /// Records that the person has seen an agent's last turn, or marks it unread, in `herdr.seen`
    /// (the Herdr app's docs/labels.md), so every Herdr client agrees. Herdr TUI keeps no such label:
    /// there the client's own record is all there is.
    pub async fn mark_seen(&self, stream_id: &str, seen: bool) -> Result<Value, ActionError> {
        let stream = self
            .herdr
            .model()
            .and_then(|m| m.streams.into_iter().find(|s| s.id == stream_id))
            .ok_or_else(|| err(404, "stream_gone", "That stream no longer exists."))?;
        let (Some(c), StreamKind::Agent) = (self.core(), stream.kind) else {
            return Ok(json!({"shared": false}));
        };
        let Some(record) = c.terminal(stream_id) else {
            return Err(err(404, "stream_gone", "That stream no longer exists."));
        };
        let turn = record.get("turn").and_then(Value::as_u64).unwrap_or(0);
        let last = record
            .pointer("/labels/herdr.seen")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u64>().ok());
        // Seen only raises it (and joins a terminal without it); unread is the one lower write.
        let next = if seen {
            last.is_none_or(|l| l < turn).then_some(turn)
        } else {
            turn.checked_sub(1)
        };
        if let Some(n) = next {
            let labels = json!({"herdr.seen": n.to_string()});
            self.core_call(
                "terminal.label",
                json!({"terminal": stream_id, "labels": labels}),
            )
            .await?;
        }
        Ok(json!({"shared": true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_start_as_the_herdr_app_starts_them() {
        let argv = agent_argv("claude", Some("/bin/zsh".into()));
        let (flags, script) = if cfg!(target_os = "macos") {
            ("-lic", "\"$@\"; exec \"$0\" -li")
        } else {
            ("-ic", "\"$@\"; exec \"$0\" -i")
        };
        assert_eq!(argv, ["/bin/zsh", flags, script, "/bin/zsh", "claude"]);
        assert_eq!(agent_argv("codex", None)[0], "/bin/sh");
        assert!(agent_argv("codex", Some("/opt/bin/fish".into()))[2].starts_with("command $argv"));
    }
}
