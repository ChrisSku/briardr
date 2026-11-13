// Changes to the Herdr app's layout document (`herdr-app`), made as the app makes them: a space or
// tab is renamed, or added with its first terminal. They work on the raw JSON, so fields this
// Briardr doesn't know keep their values, and only where the app can't do it from labels: it adopts
// a labelled terminal into an existing tab, or into a new space, but never into a new tab of a space
// it has. Splits and closing are the app's: it closes a tab with its last terminal, and a space with
// its last tab.
use serde_json::{json, Value};

/// A fresh id as the app makes them: `prefix` and 32 hex digits.
pub fn fresh_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

fn spaces(doc: &mut Value) -> &mut Vec<Value> {
    if !doc.is_object() {
        *doc = json!({});
    }
    let spaces = doc
        .as_object_mut()
        .expect("an object")
        .entry("spaces")
        .or_insert_with(|| json!([]));
    if !spaces.is_array() {
        *spaces = json!([]);
    }
    spaces.as_array_mut().expect("an array")
}

fn has_id(v: &Value, id: &str) -> bool {
    v.get("id").and_then(Value::as_str) == Some(id)
}

fn find<'a>(doc: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    for space in spaces(doc) {
        if has_id(space, id) {
            return Some(space);
        }
        if let Some(tab) = space
            .get_mut("tabs")
            .and_then(Value::as_array_mut)
            .and_then(|tabs| tabs.iter_mut().find(|t| has_id(t, id)))
        {
            return Some(tab);
        }
    }
    None
}

/// Names the space or tab `id`; false when the document has neither.
pub fn rename(doc: &mut Value, id: &str, name: &str) -> bool {
    match find(doc, id).and_then(Value::as_object_mut) {
        Some(entry) => {
            entry.insert("name".into(), json!(name));
            true
        }
        None => false,
    }
}

/// A tab whose one pane shows `terminal`, as the app writes one (its root's id is the tab's).
fn tab(id: &str, name: &str, pane: &str, terminal: &str) -> Value {
    json!({"id": id, "name": name, "root": {"id": id, "type": "row", "children": [
        {"id": pane, "type": "block", "content": {"terminal": terminal}}]}})
}

/// Adds a space with one tab, after the others. The app may have adopted its terminal already,
/// as an unnamed space under the same id: then it is only named.
pub fn add_space(
    doc: &mut Value,
    id: &str,
    name: &str,
    folder: &str,
    tab_id: &str,
    pane: &str,
    terminal: &str,
) {
    if find(doc, id).is_some() {
        if !name.is_empty() {
            rename(doc, id, name);
        }
        return;
    }
    spaces(doc).push(json!({"id": id, "name": name, "folder": folder,
        "tabs": [tab(tab_id, "", pane, terminal)]}));
}

/// Adds a tab to the space `space`, after its others; false when the document has no such space.
pub fn add_tab(
    doc: &mut Value,
    space: &str,
    id: &str,
    name: &str,
    pane: &str,
    terminal: &str,
) -> bool {
    if find(doc, id).is_some() {
        return name.is_empty() || rename(doc, id, name);
    }
    let Some(entry) = spaces(doc).iter_mut().find(|s| has_id(s, space)) else {
        return false;
    };
    let Some(entry) = entry.as_object_mut() else {
        return false;
    };
    let tabs = entry.entry("tabs").or_insert_with(|| json!([]));
    match tabs.as_array_mut() {
        Some(tabs) => tabs.push(tab(id, name, pane, terminal)),
        None => *tabs = json!([tab(id, name, pane, terminal)]),
    }
    true
}

/// Whether the document has a space `id`.
pub fn has_space(doc: Option<&Value>, id: &str) -> bool {
    doc.and_then(|d| d.get("spaces"))
        .and_then(Value::as_array)
        .is_some_and(|s| s.iter().any(|s| has_id(s, id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Value {
        json!({"spaces": [{"id": "sp1", "name": "", "folder": "/w", "pinned": true,
            "tabs": [{"id": "tb1", "name": "", "root": {"id": "tb1", "type": "row", "children": []}, "later": 1}]}],
            "theirs": "kept"})
    }

    #[test]
    fn renames_touch_only_the_name() {
        let mut d = doc();
        assert!(rename(&mut d, "sp1", "app"));
        assert!(rename(&mut d, "tb1", "review"));
        assert!(!rename(&mut d, "tb9", "x"));
        assert_eq!(d["spaces"][0]["name"], "app");
        assert_eq!(d["spaces"][0]["tabs"][0]["name"], "review");
        assert_eq!(
            (
                d["theirs"].clone(),
                d["spaces"][0]["pinned"].clone(),
                d["spaces"][0]["tabs"][0]["later"].clone()
            ),
            (json!("kept"), json!(true), json!(1)),
            "fields Briardr doesn't know keep their values"
        );
    }

    #[test]
    fn spaces_and_tabs_are_added_as_the_app_writes_them() {
        let mut d = doc();
        assert!(add_tab(&mut d, "sp1", "tb2", "docs", "p1", "t4"));
        assert_eq!(
            d["spaces"][0]["tabs"][1],
            json!({"id": "tb2", "name": "docs", "root": {"id": "tb2", "type": "row", "children": [
                {"id": "p1", "type": "block", "content": {"terminal": "t4"}}]}})
        );
        assert!(
            !add_tab(&mut d, "sp9", "tb3", "", "p2", "t5"),
            "no such space"
        );
        add_space(&mut d, "sp2", "web", "/w/web", "tb4", "p3", "t6");
        assert_eq!(
            (
                d["spaces"][1]["name"].clone(),
                d["spaces"][1]["folder"].clone(),
                d["spaces"][1]["tabs"][0]["id"].clone()
            ),
            (json!("web"), json!("/w/web"), json!("tb4"))
        );
        assert!(has_space(Some(&d), "sp2") && !has_space(None, "sp2"));
    }

    #[test]
    fn what_the_app_adopted_meanwhile_is_only_named() {
        let mut d = doc();
        add_space(&mut d, "sp1", "named", "/elsewhere", "tb9", "p9", "t9");
        assert_eq!(d["spaces"].as_array().unwrap().len(), 1);
        assert_eq!(
            (
                d["spaces"][0]["name"].clone(),
                d["spaces"][0]["folder"].clone()
            ),
            (json!("named"), json!("/w"))
        );
        assert!(add_tab(&mut d, "sp1", "tb1", "again", "p9", "t9"));
        assert_eq!(d["spaces"][0]["tabs"].as_array().unwrap().len(), 1);
        // No document yet: the first space makes one.
        let mut empty = Value::Null;
        add_space(&mut empty, "sp1", "", "/w", "tb1", "p1", "t1");
        assert_eq!(
            empty["spaces"][0]["tabs"][0]["root"]["children"][0]["content"]["terminal"],
            "t1"
        );
        assert_eq!(fresh_id("sp").len(), 34);
    }
}
