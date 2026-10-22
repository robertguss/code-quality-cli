//! `qgate features show` and `qgate features log` (R5, R8).

mod support;

use support::gate::{self, gated};
use support::repo::TestRepo;

fn json(t: &TestRepo, args: &[&str]) -> serde_json::Value {
    let out = t
        .qgate()
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn with_cli(t: &TestRepo) {
    t.write(
        "src/cli/mod.rs",
        "#[derive(Subcommand)]\nenum Command {\n    Run,\n    Show,\n}\n",
    );
    t.write("src/main.rs", "fn main() {}\n");
    gate::add_feature(
        t,
        "[[feature]]\nname = \"cli\"\npurpose = \"Commands\"\ntask = \"QG-2\"\nfolders = [\"src/cli\"]\nentry_points = [\"src/main.rs\"]\n",
    );
    t.commit_all("cli");
}

#[test]
fn show_one_feature_lists_its_folders_and_entry_points() {
    let t = gated();
    with_cli(&t);
    let v = json(&t, &["features", "show", "cli"]);
    let f = &v["features"][0];
    assert_eq!(v["features"].as_array().unwrap().len(), 1);
    assert_eq!(f["folders"], serde_json::json!(["src/cli"]));
    let kinds: Vec<String> = f["entry_points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{} {}",
                e["kind"].as_str().unwrap(),
                e["name"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        kinds,
        ["binary shop", "command run", "command show", "module cli"]
    );
}

#[test]
fn show_lists_every_feature_and_marks_placeholders() {
    let t = gated();
    t.write("src/misc.rs", "\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"misc\"\nplaceholder = true\nentry_points = [\"src/misc.rs\"]\n",
    );
    let out = t.qgate().args(["features", "show"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("core (QG-1): The shop's core"), "{text}");
    assert!(text.contains("misc [placeholder]"), "{text}");
}

#[test]
fn show_an_unknown_feature_exits_2_naming_the_known_ones() {
    let t = gated();
    let out = t
        .qgate()
        .args(["features", "show", "nope"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("known features: core"));
}

#[test]
fn show_without_a_map_points_at_init() {
    let t = gated();
    t.remove("qgate.features.toml");
    t.commit_all("no map");
    let out = t.qgate().args(["features", "show"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("qgate features init"));
}

#[test]
fn log_lists_map_commits_oldest_first() {
    let t = gated();
    let first = t.git(&["rev-parse", "HEAD"]);
    with_cli(&t);
    t.write("src/core/tax/mod.rs", "\n");
    t.write(
        "qgate.features.toml",
        &t.read("qgate.features.toml")
            .replace("[\"src/core\"]", "[\"src/core\", \"src/core/tax\"]"),
    );
    t.commit_all("tax");
    t.write("README.md", "hi\n");
    t.commit_all("not the map");
    let v = json(&t, &["features", "log"]);
    let entries = v["entries"].as_array().unwrap();
    let subjects: Vec<&str> = entries
        .iter()
        .map(|e| e["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["start", "cli", "tax"]);
    assert_eq!(
        entries[0]["changes"][0],
        "adopted qgate.features.toml with 1 feature"
    );
    assert_eq!(entries[1]["changes"][0], "new feature cli (QG-2)");
    assert_eq!(
        entries[2]["changes"][0],
        "folders added to core: src/core/tax"
    );
    let v = json(&t, &["features", "log", "--since", &first]);
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
}
