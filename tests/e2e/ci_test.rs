//! `qgate run --stage push --base <ref>` on a CI-style checkout.

use super::support::repo::{TestRepo, git_cmd};
use super::{func, git, on_feature_branch};

/// A gated repo pushed to a bare remote, with a `feature` branch adding a long function.
fn feature_branch() -> (TestRepo, tempfile::TempDir) {
    let (t, remote) = on_feature_branch("a.rs", &func("small", 5));
    t.write("b.rs", &func("long", 70));
    t.git(&["add", "b.rs"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "long"]);
    let (ok, out) = git(&t, &["push", "-q", "--no-verify", "origin", "feature"]);
    assert!(ok, "{out}");
    (t, remote)
}

fn qgate_in(dir: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = assert_cmd::Command::cargo_bin("qgate").unwrap();
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GITHUB_STEP_SUMMARY");
    cmd
}

fn clone(remote: &tempfile::TempDir, extra: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("qgate-ci-")
        .tempdir()
        .unwrap();
    let url = format!("file://{}", remote.path().display());
    let mut args = vec!["clone", "-q", "--branch", "feature"];
    args.extend_from_slice(extra);
    args.extend([url.as_str(), "."]);
    let out = git_cmd(dir.path()).args(&args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = git_cmd(dir.path())
        .args(["fetch", "-q", "origin", "main:refs/remotes/origin/main"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir
}

#[test]
fn github_format_annotates_findings_and_writes_the_step_summary() {
    let (_t, remote) = feature_branch();
    let ci = clone(&remote, &[]);
    let summary = ci.path().join("summary.md");
    let out = qgate_in(ci.path())
        .args([
            "run",
            "--stage",
            "push",
            "--base",
            "origin/main",
            "--format",
            "github",
        ])
        .env("GITHUB_STEP_SUMMARY", &summary)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(
            "::error file=b.rs,line=1,title=qgate function-length::function `long` is 70 lines"
        ),
        "{out}"
    );
    assert!(out.contains("qgate run --stage push: FAIL"), "{out}");
    let summary = std::fs::read_to_string(summary).unwrap();
    assert!(
        summary.contains("| error | function-length | b.rs:1 |"),
        "{summary}"
    );
}

#[test]
fn base_compares_against_the_merge_base_not_the_whole_repo() {
    let (_t, remote) = feature_branch();
    let ci = clone(&remote, &[]);
    let out = qgate_in(ci.path())
        .args([
            "run",
            "--stage",
            "push",
            "--base",
            "origin/main",
            "--format",
            "json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let errors: Vec<_> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "error")
        .collect();
    assert_eq!(errors.len(), 1, "{report:#}");
    assert_eq!(errors[0]["path"], "b.rs");
}

#[test]
fn shallow_clone_without_a_merge_base_is_a_usage_error() {
    let (t, remote) = feature_branch();
    t.git(&["checkout", "-q", "main"]);
    t.write("c.rs", &func("other", 5));
    t.git(&["add", "c.rs"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "main moves on"]);
    let (ok, out) = git(&t, &["push", "-q", "--no-verify", "origin", "main"]);
    assert!(ok, "{out}");
    let ci = clone(&remote, &["--depth", "1"]);
    qgate_in(ci.path())
        .args(["run", "--stage", "push", "--base", "origin/main"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("shallow clone"))
        .stderr(predicates::str::contains("fetch-depth: 0"));
    qgate_in(ci.path())
        .args(["run", "--stage", "push"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("shallow clone"));
}

#[test]
fn base_needs_the_push_stage_and_an_existing_ref() {
    let (_t, remote) = feature_branch();
    let ci = clone(&remote, &[]);
    qgate_in(ci.path())
        .args(["run", "--stage", "commit", "--base", "origin/main"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("only applies to the push stage"));
    qgate_in(ci.path())
        .args(["run", "--stage", "push", "--base", "origin/nope"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("no commit `origin/nope`"));
}
