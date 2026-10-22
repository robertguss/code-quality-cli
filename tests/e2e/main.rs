//! End-to-end hook runs against real temporary repositories.

#[path = "../support/mod.rs"]
mod support;

mod ci_test;
mod commit_test;
mod push_test;
mod rules_test;
mod skill_test;

use support::repo::TestRepo;

/// A Rust function `name` of exactly `lines` lines.
pub fn func(name: &str, lines: usize) -> String {
    let mut s = format!("fn {name}() {{\n");
    for i in 0..lines - 2 {
        s.push_str(&format!("    let {name}_{i} = \"{name}{i}\";\n"));
    }
    s.push_str("}\n");
    s
}

/// Combined stdout and stderr of a git command, and whether it succeeded.
pub fn git(t: &TestRepo, args: &[&str]) -> (bool, String) {
    let out = t.git_output(args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// Install, record the baseline and commit: a gated repo.
pub fn onboard(t: &TestRepo) {
    t.qgate().arg("install").assert().success();
    t.qgate().args(["baseline", "create"]).assert().success();
    t.git(&["add", "-A"]);
    let (ok, out) = git(t, &["commit", "-q", "-m", "add qgate"]);
    assert!(ok, "onboarding commit failed:\n{out}");
}
