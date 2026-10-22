//! Scope changes in `qgate run` (R5, KTD8, KTD9).

mod support;

use support::gate::{self, commit, findings, gated, run};
use support::repo::TestRepo;

const SEARCH: &str = "[[feature]]\nname = \"search\"\npurpose = \"Find products\"\ntask = \"QG-31\"\nfolders = [\"src/search\"]\nentry_points = [\"src/bin/reindex.rs\"]\n";

fn branch(t: &TestRepo, name: &str) {
    t.git(&["checkout", "-q", "-b", name]);
}

fn push(t: &TestRepo) -> (bool, serde_json::Value) {
    run(t, &["run", "--stage", "push", "--base", "main"])
}

#[test]
fn a_push_adding_a_feature_reports_one_scope_line() {
    let t = gated();
    branch(&t, "QG-31-search");
    t.write("src/lib.rs", "pub mod core;\npub mod search;\n");
    t.write(
        "src/search/mod.rs",
        "pub fn find() {}\n\npub fn rank() {}\n",
    );
    t.write("src/bin/reindex.rs", "fn main() {}\n");
    gate::add_feature(&t, SEARCH);
    t.commit_all("Add search");
    let (passed, report) = push(&t);
    assert!(passed, "{report}");
    assert_eq!(
        report["data"]["scope"],
        "new feature search (QG-31): 1 binary, 1 module, 1 folder, +4 lines"
    );
    let scope = findings(&report, "scope-change");
    assert_eq!(scope.len(), 1, "{scope:?}");
}

#[test]
fn folders_added_to_a_feature_are_a_scope_change() {
    let t = gated();
    t.write(
        "qgate.features.toml",
        &gate::MAP.replace("[\"src/core\"]", "[\"src/core\", \"src/tax\"]"),
    );
    t.write("src/tax/mod.rs", "\n");
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert_eq!(
        findings(&report, "scope-change"),
        ["scope-change: folders added to core: src/tax"]
    );
    assert!(
        report["data"].get("scope").is_none(),
        "commit sets no scope line"
    );
}

#[test]
fn no_map_edit_means_no_scope_change() {
    let t = gated();
    branch(&t, "tweak");
    t.write("src/core/mod.rs", "pub fn price() -> u32 {\n    2\n}\n");
    t.commit_all("tweak");
    let (passed, report) = push(&t);
    assert!(passed, "{report}");
    assert!(findings(&report, "scope-change").is_empty());
    assert!(report["data"].get("scope").is_none());
}

#[test]
fn the_first_map_is_an_adoption_not_new_features() {
    let t = gated();
    t.remove("qgate.features.toml");
    t.commit_all("no map");
    t.write("qgate.features.toml", gate::MAP);
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert_eq!(
        findings(&report, "scope-change"),
        ["scope-change: adopted qgate.features.toml with 1 feature"]
    );
    assert!(findings(&report, "feature-task").is_empty());
}

#[test]
fn naming_a_placeholder_is_not_a_new_feature() {
    let t = gated();
    t.write("src/lib.rs", "pub mod core;\npub mod misc;\n");
    t.write("src/misc/mod.rs", "\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"misc\"\nplaceholder = true\nfolders = [\"src/misc\"]\n",
    );
    t.commit_all("placeholder");
    let map = t.read("qgate.features.toml").replace(
        "name = \"misc\"\nplaceholder = true\n",
        "name = \"pricing\"\npurpose = \"Prices\"\ntask = \"QG-9\"\n",
    );
    t.write("qgate.features.toml", &map);
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert_eq!(
        findings(&report, "scope-change"),
        ["scope-change: named placeholder misc as pricing"]
    );
    assert!(findings(&report, "feature-task").is_empty());
}
