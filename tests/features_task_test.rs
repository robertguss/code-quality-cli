//! Task IDs for new features in `qgate run` (R6, KTD10, KTD11).

mod support;

use support::gate::{self, commit, findings, gated, run};
use support::repo::TestRepo;

/// Add feature `search` (task `task`) on branch `branch` and commit it as `message`.
fn searched(branch: &str, task: &str, message: &str) -> TestRepo {
    let t = gated();
    t.git(&["checkout", "-q", "-b", branch]);
    t.write("src/lib.rs", "pub mod core;\npub mod search;\n");
    t.write("src/search/mod.rs", "\n");
    gate::add_feature(
        &t,
        &format!(
            "[[feature]]\nname = \"search\"\npurpose = \"Find\"\ntask = \"{task}\"\nfolders = [\"src/search\"]\n"
        ),
    );
    t.commit_all(message);
    t
}

fn push(t: &TestRepo) -> (bool, serde_json::Value) {
    run(t, &["run", "--stage", "push", "--base", "main"])
}

#[test]
fn a_task_in_the_branch_name_passes() {
    let t = searched("feat/QG-31-search", "QG-31", "Add search");
    let (passed, report) = push(&t);
    assert!(passed, "{report}");
}

#[test]
fn a_task_in_a_pushed_commit_message_passes() {
    let t = searched("search", "QG-31", "QG-31: Add search");
    let (passed, report) = push(&t);
    assert!(passed, "{report}");
}

#[test]
fn a_task_found_nowhere_fails_the_push_naming_where_it_looked() {
    let t = searched("search", "QG-31", "Add search");
    let (passed, report) = push(&t);
    assert!(!passed);
    let task = findings(&report, "feature-task");
    assert_eq!(task.len(), 1, "{task:?}");
    assert!(task[0].contains("branch `search`"), "{task:?}");
    assert!(task[0].contains("1 pushed commit message"), "{task:?}");
}

#[test]
fn a_task_a_similar_id_contains_does_not_count() {
    let t = searched("QG-310-search", "QG-31", "Add search");
    let (passed, _) = push(&t);
    assert!(!passed);
}

#[test]
fn at_commit_a_missing_task_is_pending_not_an_error() {
    let t = gated();
    t.git(&["checkout", "-q", "-b", "search"]);
    t.write("src/search/mod.rs", "\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"search\"\npurpose = \"Find\"\ntask = \"QG-31\"\nfolders = [\"src/search\"]\n",
    );
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert_eq!(findings(&report, "feature-task-pending").len(), 1);
}

#[test]
fn a_new_feature_without_a_task_fails_at_commit() {
    let t = gated();
    t.write("src/search/mod.rs", "\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"search\"\npurpose = \"Find\"\nfolders = [\"src/search\"]\n",
    );
    let (passed, report) = commit(&t);
    assert!(!passed);
    let task = findings(&report, "feature-task");
    assert!(task.iter().any(|m| m.contains("has no task")), "{task:?}");
}

#[test]
fn a_new_placeholder_needs_no_task() {
    let t = gated();
    t.write("src/misc/mod.rs", "\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"misc\"\nplaceholder = true\nfolders = [\"src/misc\"]\n",
    );
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert!(findings(&report, "feature-task").is_empty());
}

#[test]
fn the_pattern_comes_from_qgate_toml() {
    let t = gated();
    let config = t.read("qgate.toml");
    t.write(
        "qgate.toml",
        &format!("{config}\n[tasks]\npattern = '#[0-9]+'\n"),
    );
    t.write("qgate.features.toml", &gate::MAP.replace("QG-1", "#1"));
    t.commit_all("issues");
    t.git(&["checkout", "-q", "-b", "issue-#42"]);
    t.write("src/search/mod.rs", "\n");
    gate::add_feature(
        &t,
        "[[feature]]\nname = \"search\"\npurpose = \"Find\"\ntask = \"#42\"\nfolders = [\"src/search\"]\n",
    );
    t.commit_all("Add search");
    let (passed, report) = push(&t);
    assert!(passed, "{report}");
}

#[test]
fn ci_reads_the_branch_from_github_head_ref() {
    let t = searched("search", "QG-31", "Add search");
    t.git(&["checkout", "-q", "--detach"]);
    let out = t
        .qgate()
        .args([
            "run", "--stage", "push", "--base", "main", "--format", "json",
        ])
        .env("GITHUB_HEAD_REF", "QG-31-search")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
