// Smoke test for the golden fixtures: every recorded file has a valid shape and the set is non-empty.
// The comparisons live in each module's inline tests.
mod common;

#[test]
fn every_fixture_has_valid_shape() {
    let mut total = 0;
    let mut modules: Vec<_> = std::fs::read_dir(common::fixtures_dir())
        .expect("fixtures dir exists")
        .map(|e| e.expect("readable entry").path())
        .filter(|p| p.is_dir())
        .collect();
    modules.sort();
    for dir in modules {
        let name = dir
            .file_name()
            .expect("module name")
            .to_string_lossy()
            .into_owned();
        // load_module panics on any malformed file, so a count is the whole check here.
        total += common::load_module(&name).len();
    }
    assert!(total > 0, "no golden fixtures found in server/fixtures");
}
