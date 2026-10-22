//! The feature map through real hooks (F1, F2, AE1, AE6).

use super::support::repo::TestRepo;
use super::{git, onboard};

/// A crate with one module, onboarded (with its drafted map) and pushed.
fn gated_crate() -> (TestRepo, tempfile::TempDir) {
    let t = TestRepo::new();
    let remote = t.add_remote();
    t.write(
        "Cargo.toml",
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    );
    t.write("src/lib.rs", "pub mod core;\n");
    t.write("src/core/mod.rs", "pub fn price() -> u32 {\n    1\n}\n");
    onboard(&t);
    let (ok, out) = git(&t, &["push", "-q", "origin", "main"]);
    assert!(ok, "{out}");
    t.git(&["fetch", "-q", "origin"]);
    t.git(&["checkout", "-q", "-b", "work"]);
    (t, remote)
}

const PUBLISHING: &str = "\n[[feature]]\nname = \"publishing\"\npurpose = \"Publish the catalog\"\ntask = \"QG-31\"\nentry_points = [\"src/bin/publish.rs\"]\n";

/// Commit a new binary: rejected until a feature owns it, then committed as `message`.
fn add_publishing(t: &TestRepo, message: &str) {
    t.write("src/bin/publish.rs", "fn main() {}\n");
    t.add(&["src/bin/publish.rs"]);
    let (ok, out) = git(t, &["commit", "-q", "-m", message]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("error[feature-unowned] binary `publish`"),
        "{out}"
    );
    let map = t.read("qgate.features.toml") + PUBLISHING;
    t.write("qgate.features.toml", &map)
        .add(&["qgate.features.toml"]);
    let (ok, out) = git(t, &["commit", "-q", "-m", message]);
    assert!(ok, "{out}");
    assert!(out.contains("info[feature-task-pending]"), "{out}");
}

#[test]
fn install_drafts_a_map_the_first_commit_passes() {
    let (t, _remote) = gated_crate();
    let map = t.read("qgate.features.toml");
    assert!(map.contains("name = \"core\""), "{map}");
    assert!(map.contains("placeholder = true"), "{map}");
}

#[test]
fn a_new_feature_is_pushed_with_its_task_and_a_scope_line() {
    let (t, _remote) = gated_crate();
    add_publishing(&t, "QG-31: Add publishing preview");
    let (ok, out) = git(&t, &["push", "-q", "origin", "work"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("scope: new feature publishing (QG-31): 1 binary"),
        "{out}"
    );
}

#[test]
fn a_push_without_the_task_is_rejected() {
    let (t, _remote) = gated_crate();
    add_publishing(&t, "QG-12: Add publishing preview");
    let (ok, out) = git(&t, &["push", "-q", "origin", "work"]);
    assert!(!ok, "{out}");
    assert!(out.contains("error[feature-task]"), "{out}");
    assert!(out.contains("branch `work`"), "{out}");
}

#[test]
fn a_route_inside_an_owned_folder_needs_no_map_edit() {
    let (t, _remote) = gated_crate();
    t.write(
        "src/core/api.rs",
        "pub fn app(r: Router) -> Router {\n    r.route(\"/prices\", get(prices))\n}\n",
    );
    t.add(&["src/core/api.rs"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "Add a prices route"]);
    assert!(ok, "{out}");
    let (ok, out) = git(&t, &["push", "-q", "origin", "work"]);
    assert!(ok, "{out}");
    assert!(!out.contains("scope-change"), "{out}");
}
