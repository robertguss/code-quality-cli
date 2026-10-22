//! Ownership checks in `qgate run` (R3, R4, KTD5).

mod support;

use support::gate::{self, commit, findings, gated};

#[test]
fn an_unowned_binary_fails_with_both_edits_named() {
    let t = gated();
    t.write("src/bin/import.rs", "fn main() {}\n");
    let (passed, report) = commit(&t);
    assert!(!passed);
    let unowned = findings(&report, "feature-unowned");
    assert_eq!(unowned.len(), 1, "{unowned:?}");
    assert!(unowned[0].contains("binary `import`"), "{unowned:?}");
    assert!(unowned[0].contains("`folders`"), "{unowned:?}");
    assert!(unowned[0].contains("`entry_points`"), "{unowned:?}");
    let f = &report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "feature-unowned")
        .unwrap();
    assert_eq!(f["path"], "src/bin/import.rs");
}

#[test]
fn a_listed_entry_point_passes() {
    let t = gated();
    t.write("src/bin/import.rs", "fn main() {}\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"import\"\npurpose = \"Imports\"\ntask = \"QG-2\"\nentry_points = [\"src/bin/import.rs\"]\n",
    );
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert!(findings(&report, "feature-unowned").is_empty());
}

#[test]
fn routes_and_migrations_inside_an_owned_folder_pass() {
    let t = gated();
    t.write(
        "src/core/api.rs",
        "pub fn app(r: Router) -> Router {\n    r.route(\"/prices\", get(prices))\n}\n",
    );
    t.write(
        "src/core/migrations/V1__init.sql",
        "create table t (id int);\n",
    );
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert!(findings(&report, "feature-").is_empty(), "{report}");
}

#[test]
fn an_unowned_route_fails_at_its_line() {
    let t = gated();
    t.write("src/lib.rs", "pub mod core;\npub mod api;\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"api\"\nplaceholder = true\nentry_points = [\"src/api.rs\"]\n",
    );
    t.commit_all("api");
    t.write("migrations/V1__init.sql", "create table t (id int);\n");
    let (passed, report) = commit(&t);
    assert!(!passed);
    let unowned = findings(&report, "feature-unowned");
    assert_eq!(unowned.len(), 1, "{unowned:?}");
    assert!(
        unowned[0].starts_with("feature-unowned: migration"),
        "{unowned:?}"
    );
}

#[test]
fn a_folder_with_no_files_is_stale() {
    let t = gated();
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"gone\"\nplaceholder = true\nfolders = [\"src/gone\"]\nentry_points = [\"src/old.rs\"]\n",
    );
    let (passed, report) = commit(&t);
    assert!(!passed);
    let stale = findings(&report, "feature-stale");
    assert_eq!(stale.len(), 2, "{stale:?}");
    assert!(
        stale.iter().any(|s| s.contains("folder `src/gone`")),
        "{stale:?}"
    );
    assert!(
        stale.iter().any(|s| s.contains("lists `src/old.rs`")),
        "{stale:?}"
    );
}

#[test]
fn a_commit_that_leaves_the_map_alone_checks_only_changed_files() {
    let t = gated();
    // An unowned binary committed without the gate.
    t.write("src/bin/old.rs", "fn main() {}\n");
    t.commit_all("old");
    t.write("src/core/mod.rs", "pub fn price() -> u32 {\n    2\n}\n");
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
}

#[test]
fn a_map_edit_checks_every_entry_point() {
    let t = gated();
    t.write("src/bin/old.rs", "fn main() {}\n");
    t.commit_all("old");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"extra\"\nplaceholder = true\nfolders = [\"src/core/extra\"]\n",
    );
    t.write("src/core/extra/mod.rs", "\n");
    let (passed, report) = commit(&t);
    assert!(!passed);
    let unowned = findings(&report, "feature-unowned");
    assert!(unowned.iter().any(|f| f.contains("`old`")), "{unowned:?}");
}

#[test]
fn owning_a_source_root_warns() {
    let t = gated();
    t.write(
        "qgate.features.toml",
        "[[feature]]\nname = \"all\"\npurpose = \"Everything\"\ntask = \"QG-1\"\nfolders = [\"src\"]\n",
    );
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    let warn = findings(&report, "feature-map");
    assert_eq!(warn.len(), 1, "{warn:?}");
    assert!(
        warn.iter().any(|w| w.contains("source root `src`")),
        "{warn:?}"
    );
}

#[test]
fn no_map_is_an_info_pointing_at_init() {
    let t = gated();
    t.remove("qgate.features.toml");
    t.commit_all("no map");
    t.write("src/bin/import.rs", "fn main() {}\n");
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    let missing = findings(&report, "feature-map-missing");
    assert_eq!(missing.len(), 1);
    assert!(
        report["fixes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "qgate features init")
    );
}

#[test]
fn placeholders_are_listed() {
    let t = gated();
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"misc\"\nplaceholder = true\nfolders = [\"src/core\"]\n",
    );
    let (_, report) = commit(&t);
    // Two features owning one folder is a map error; placeholders still list.
    assert!(
        !findings(&report, "feature-placeholder").is_empty(),
        "{report}"
    );
}
