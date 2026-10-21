use super::support::repo::TestRepo;
use super::{func, git, onboard};

fn gated_with_remote() -> (TestRepo, tempfile::TempDir) {
    let t = TestRepo::new();
    let remote = t.add_remote();
    t.write("a.rs", &func("small", 5));
    onboard(&t);
    let (ok, out) = git(&t, &["push", "-q", "origin", "main"]);
    assert!(ok, "{out}");
    t.git(&["fetch", "-q", "origin"]);
    (t, remote)
}

#[test]
fn push_catches_commit_that_skipped_pre_commit() {
    let (t, _remote) = gated_with_remote();
    t.git(&["checkout", "-q", "-b", "feature"]);
    t.write("b.rs", &func("long", 70));
    t.git(&["add", "b.rs"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "sneaky"]);
    let (ok, out) = git(&t, &["push", "-q", "origin", "feature"]);
    assert!(!ok);
    assert!(out.contains("b.rs:1: error[function-length]"), "{out}");
}

#[test]
fn push_of_branch_not_checked_out_checks_its_tip() {
    let (t, _remote) = gated_with_remote();
    t.git(&["checkout", "-q", "-b", "feature"]);
    t.write("b.rs", &func("long", 70));
    t.git(&["add", "b.rs"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "sneaky"]);
    t.git(&["checkout", "-q", "main"]);
    let (ok, out) = git(&t, &["push", "-q", "origin", "feature"]);
    assert!(!ok, "{out}");
}

#[test]
fn rebased_branch_with_unapproved_loosening_fails_at_push() {
    let (t, _remote) = gated_with_remote();
    t.git(&["checkout", "-q", "-b", "feature"]);
    t.write(
        "a.rs",
        &("// eslint-disable\n".to_string() + &func("small", 5)),
    );
    t.git(&["add", "a.rs"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "ignore"]);
    let (ok, out) = git(&t, &["push", "-q", "origin", "feature"]);
    assert!(!ok);
    assert!(out.contains("fix: qgate approve --commit "), "{out}");
    let sha = t.git(&["rev-parse", "HEAD"]);
    t.qgate()
        .args(["approve", "--commit", &sha])
        .assert()
        .success();
    let (ok, out) = git(&t, &["push", "-q", "origin", "feature"]);
    assert!(ok, "{out}");
}

#[test]
fn push_stage_tool_runs_only_at_push() {
    let (t, _remote) = gated_with_remote();
    let cfg = t.read("qgate.toml")
        + "\n[[tool]]\nname = \"suite\"\ncommand = \"echo suite failed; exit 1\"\nstage = \"push\"\n";
    t.write("qgate.toml", &cfg).add(&["qgate.toml"]);
    t.qgate().arg("approve").assert().success();
    let (ok, out) = git(&t, &["commit", "-q", "-m", "add suite"]);
    assert!(ok, "{out}");
    let (ok, out) = git(&t, &["push", "-q", "origin", "main"]);
    assert!(!ok);
    assert!(out.contains("error[suite]"), "{out}");
}

#[test]
fn clean_push_passes_and_deletes_are_skipped() {
    let (t, _remote) = gated_with_remote();
    t.git(&["checkout", "-q", "-b", "feature"]);
    t.write("b.rs", &func("ok", 10));
    t.git(&["add", "b.rs"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "fine"]);
    assert!(ok, "{out}");
    let (ok, out) = git(&t, &["push", "-q", "origin", "feature"]);
    assert!(ok, "{out}");
    let (ok, out) = git(&t, &["push", "-q", "origin", "--delete", "feature"]);
    assert!(ok, "{out}");
}

#[test]
fn tools_never_write_into_the_working_trees_build_dir() {
    let (t, _remote) = gated_with_remote();
    // A build that records each run in _build, as `mix compile` would.
    t.write(".gitignore", "_build/\n");
    t.write("build.sh", "mkdir -p _build && echo built >> _build/runs\n");
    let cfg = t.read("qgate.toml")
        + "\n[[tool]]\nname = \"build\"\ncommand = \"sh build.sh\"\nglobs = [\"**/*.rs\"]\n";
    t.write("qgate.toml", &cfg);
    t.git(&["add", "-A"]);
    t.qgate().arg("approve").assert().success();
    let (ok, out) = git(&t, &["commit", "-q", "-m", "add build"]);
    assert!(ok, "{out}");
    t.write("_build/runs", "mine\n");

    // A partly staged commit runs the tool in the checkout.
    t.write("b.rs", &func("ok", 5)).add(&["b.rs"]);
    t.write("b.rs", &func("ok", 6));
    let (ok, out) = git(&t, &["commit", "-q", "-m", "partly staged"]);
    assert!(ok, "{out}");
    // The push stage always does.
    let (ok, out) = git(&t, &["push", "-q", "origin", "main"]);
    assert!(ok, "{out}");

    assert_eq!(t.read("_build/runs"), "mine\n");
    let checkout_runs = t.path().join(".git/qgate/checkout/_build/runs");
    assert_eq!(
        std::fs::read_to_string(checkout_runs).unwrap(),
        "built\nbuilt\n",
        "the checkout keeps its build output between runs"
    );
}
