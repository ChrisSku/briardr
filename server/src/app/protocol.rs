// web/src/protocol.ts: the API's answers and the live and terminal sockets' messages, both ways, as
// TypeScript made from the server's own types (ts-rs reads their serde attributes), so the client's types
// can't drift from what the server sends and takes. Only tests derive `TS`
// (`#[cfg_attr(test, derive(ts_rs::TS))]`), so the server's builds don't carry the generator.
//
// The test fails while the file is out of date; `mise run protocol` writes it.
use std::any::TypeId;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use ts_rs::{Config, TypeVisitor, TS};

use super::{routes_misc, routes_plugins, routes_session, routes_streams};
use crate::{actions, terminal_hub, update};

const HEADER: &str = "\
// What Briardr's API answers, and what its live and terminal sockets send and take: made from the
// server's types by server/src/app/protocol.rs. Don't edit it: change those types and run
// `mise run protocol`.
";

/// What the client gets and sends: both sockets' messages, and the answers of the routes it calls. The
/// types in them come along.
fn roots(t: &mut Types) {
    // /ws/live
    t.visit::<super::LiveMessage<'static>>();
    t.visit::<super::LiveRequest>();
    // /ws/term
    t.visit::<terminal_hub::TermMessage<'static>>();
    t.visit::<terminal_hub::TermRequest>();
    // Every refused request, and the routes with nothing else to say.
    t.visit::<super::ErrorBody<'static>>();
    t.visit::<super::Done>();
    // The answers.
    t.visit::<routes_session::Session<'static>>();
    t.visit::<routes_session::SignedOut>();
    t.visit::<routes_session::MacState<'static>>();
    t.visit::<routes_session::Devices>();
    t.visit::<routes_session::PairingCode>();
    t.visit::<routes_misc::Settings<'static>>();
    t.visit::<routes_misc::NotificationsState>();
    t.visit::<routes_misc::PrefsBody>();
    t.visit::<routes_misc::TestSent>();
    t.visit::<routes_misc::DeliveryBody>();
    t.visit::<routes_misc::Installed>();
    t.visit::<crate::power::SleepAnswer>();
    t.visit::<update::UpdateStatus>();
    t.visit::<routes_plugins::Plugins>();
    t.visit::<routes_streams::Dictated>();
    t.visit::<routes_streams::Uploaded>();
    t.visit::<actions::Agents>();
    t.visit::<actions::CreatedWorkspace>();
    t.visit::<actions::CreatedProject>();
    t.visit::<actions::CreatedStream>();
    t.visit::<actions::Seen>();
    t.visit::<actions::History>();
    t.visit::<actions::StreamInfo>();
    // A push notification's payload, for the service worker.
    t.visit::<crate::push::PushPayload>();
}

/// The declarations, by TypeScript name.
struct Types {
    cfg: Config,
    seen: HashSet<TypeId>,
    decls: BTreeMap<String, String>,
}

impl TypeVisitor for Types {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        // Only declared types have a file of their own; strings, arrays and the like are written inline.
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        let name = T::ident(&self.cfg);
        let decl = format!(
            "{}export {}\n",
            T::docs().unwrap_or_default(),
            lines(&T::decl(&self.cfg))
        );
        // serde_json's Value comes as ts-rs's stand-in for it, the same declaration twice.
        if let Some(other) = self.decls.insert(name.clone(), decl.clone()) {
            assert!(
                other == decl,
                "two types are called {name}: rename one with #[cfg_attr(test, ts(rename = \"…\"))]"
            );
        }
        T::visit_dependencies(self);
    }
}

/// ts-rs writes a declaration on one line: this puts an object's fields, and a union of objects' types, on
/// lines of their own, so the file reads and diffs like the rest of the client.
fn lines(decl: &str) -> String {
    let (head, body) = decl
        .strip_suffix(';')
        .and_then(|d| d.split_once(" = "))
        .expect("a type declaration");
    if let Some(fields) = fields(body) {
        let mut out = format!("{head} = {{\n");
        for field in fields {
            let (docs, field) = match field.rfind("*/") {
                Some(end) => field.split_at(end + 2),
                None => ("", field),
            };
            for line in docs.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let pad = if line.starts_with('*') { "   " } else { "  " };
                out.push_str(&format!("{pad}{line}\n"));
            }
            out.push_str(&format!("  {};\n", inline(field.trim())));
        }
        return out + "};";
    }
    let members = split_top(body, b'|').expect("balanced brackets");
    if members.len() > 1 && members.iter().any(|m| m.contains('{')) {
        let members: Vec<String> = members.iter().map(|m| inline(m.trim())).collect();
        return format!("{head} =\n  | {};", members.join("\n  | "));
    }
    format!("{head} = {};", inline(body))
}

/// An object type's fields, docs included, when `ty` is one.
fn fields(ty: &str) -> Option<Vec<&str>> {
    let inner = ty.strip_prefix('{')?.strip_suffix('}')?;
    // `{ … } | { … }` starts and ends like one object too, but closes a brace inside that it didn't open.
    let parts = split_top(inner, b',')?;
    Some(
        parts
            .into_iter()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .collect(),
    )
}

/// A type written on one line, without ts-rs's comma after an object's last field.
fn inline(ty: &str) -> String {
    ty.replace(", }", " }")
}

/// `s` split at each `sep` outside brackets, strings and doc comments; None when `s` closes a bracket it
/// didn't open.
fn split_top(s: &str, sep: u8) -> Option<Vec<&str>> {
    let (mut parts, mut depth, mut start, mut i) = (Vec::new(), 0, 0, 0);
    let b = s.as_bytes();
    while i < b.len() {
        if s[i..].starts_with("/**") {
            i += s[i..].find("*/").expect("a closed doc comment") + 2;
            continue;
        }
        match b[i] {
            b'"' => i += s[i + 1..].find('"').expect("a closed string") + 1,
            b'{' | b'<' | b'(' | b'[' => depth += 1,
            b'}' | b'>' | b')' | b']' if depth == 0 => return None,
            b'}' | b'>' | b')' | b']' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    Some(parts)
}

fn protocol_ts() -> String {
    let mut types = Types {
        // serde_json writes every integer as a JSON number.
        cfg: Config::new().with_large_int("number"),
        seen: HashSet::new(),
        decls: BTreeMap::new(),
    };
    roots(&mut types);
    let decls: Vec<String> = types.decls.into_values().collect();
    format!("{HEADER}\n{}", decls.join("\n"))
}

#[test]
fn protocol_ts_is_current() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/src/protocol.ts");
    let ts = protocol_ts();
    if std::env::var_os("BRIARDR_WRITE_PROTOCOL").is_some() {
        std::fs::write(&path, ts).unwrap();
        return;
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == ts,
        "web/src/protocol.ts isn't what the server's types make: run `mise run protocol`"
    );
}
