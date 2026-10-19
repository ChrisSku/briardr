// Golden fixtures: outputs recorded from the former TypeScript server during the migration (frozen; edit
// them only when the behaviour changes on purpose). Each file is {input, output} or {input, error}.
#![allow(dead_code)]

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub enum Expect {
    Output(Value),
    Error(String),
}

pub struct Fixture {
    pub path: PathBuf,
    pub input: Vec<Value>,
    pub expect: Expect,
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Loads one fixture file, panicking with the path on any malformed shape.
pub fn load_file(path: &Path) -> Fixture {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let v: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("{}: not an object", path.display()));
    let input = obj
        .get("input")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{}: input must be an array", path.display()))
        .clone();
    let expect = match (obj.get("output"), obj.get("error")) {
        (Some(out), None) => Expect::Output(out.clone()),
        (None, Some(Value::String(msg))) => Expect::Error(msg.clone()),
        _ => panic!("{}: needs exactly one of output or error", path.display()),
    };
    Fixture {
        path: path.to_path_buf(),
        input,
        expect,
    }
}

/// Loads every fixture under fixtures/<module>/, sorted by file name.
pub fn load_module(module: &str) -> Vec<Fixture> {
    let dir = fixtures_dir().join(module);
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.expect("readable entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths.iter().map(|p| load_file(p)).collect()
}
