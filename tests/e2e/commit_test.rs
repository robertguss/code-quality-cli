use std::time::Instant;

use super::support::repo::TestRepo;
use super::{func, git, onboard};

#[test]
fn clean_change_commits_and_long_function_is_rejected() {
    // F1
    let t = TestRepo::new();
    t.write("src/a.rs", &func("small", 10));
    onboard(&t);
    t.write("src/b.rs", &func("fine", 20));
    t.git(&["add", "src/b.rs"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "clean"]);
    assert!(ok, "{out}");

    t.write("src/c.rs", &func("long", 70));
    t.git(&["add", "src/c.rs"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "long"]);
    assert!(!ok);
    assert!(
        out.contains("src/c.rs:1: error[function-length] function `long` is 70 lines (limit 50)"),
        "{out}"
    );
    assert_eq!(t.git(&["log", "--format=%s", "-1"]), "clean");
}

#[test]
fn onboarding_existing_violations_then_new_one_fails() {
    // F2
    let t = TestRepo::new();
    t.write("legacy.rs", &func("old", 90));
    t.commit_all("legacy code");
    t.qgate().arg("install").assert().success();
    t.qgate().args(["baseline", "create"]).assert().success();
    t.git(&["add", "-A"]);
    // Unapproved, the baseline and config fail.
    let (ok, out) = git(&t, &["commit", "-q", "-m", "gate"]);
    assert!(!ok);
    assert!(
        out.contains("unapproved-change") && out.contains("fix: qgate approve"),
        "{out}"
    );
    t.qgate().arg("approve").assert().success();
    let (ok, out) = git(&t, &["commit", "-q", "-m", "gate"]);
    assert!(ok, "{out}");

    t.write("legacy.rs", &(func("old", 90) + &func("fresh", 60)));
    t.git(&["add", "legacy.rs"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "more"]);
    assert!(!ok);
    assert!(out.contains("`fresh` is 60 lines"), "{out}");
    assert!(!out.contains("`old`"), "{out}");
}

#[test]
fn raising_a_limit_needs_approval_end_to_end() {
    // AE2, AE7
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 10));
    onboard(&t);
    let cfg = t
        .read("qgate.toml")
        .replace("function_lines = 50", "function_lines = 80");
    t.write("qgate.toml", &cfg)
        .write("a.rs", &func("small", 70));
    t.git(&["add", "-A"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "raise"]);
    assert!(!ok);
    assert!(out.contains("function_lines 50 -> 80"), "{out}");
    t.qgate().arg("approve").assert().success();
    let cfg = cfg.replace("function_lines = 80", "function_lines = 90");
    t.write("qgate.toml", &cfg).add(&["qgate.toml"]);
    let (ok, _) = git(&t, &["commit", "-q", "-m", "raise more"]);
    assert!(!ok, "a further edit voids the approval");
}

#[test]
fn install_is_idempotent() {
    let t = TestRepo::new();
    t.qgate().arg("install").assert().success();
    let hooks = || {
        (
            t.read(".git/hooks/pre-commit"),
            t.read(".git/hooks/pre-push"),
            t.read(".gitattributes"),
            t.read(".claude/settings.json"),
            t.git(&["config", "--get", "merge.qgate-baseline.driver"]),
        )
    };
    let first = hooks();
    let out = t
        .qgate()
        .args(["install", "--format", "json"])
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(report["data"]["changed"], serde_json::json!([]));
    assert_eq!(first, hooks());
    let settings: serde_json::Value = serde_json::from_str(&first.3).unwrap();
    let cmd = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(cmd.ends_with("agent-hook claude || exit 2"), "{cmd}");
}

#[test]
fn install_dry_run_writes_nothing() {
    let t = TestRepo::new();
    t.qgate().args(["install", "--dry-run"]).assert().success();
    assert!(!t.path().join(".git/hooks/pre-commit").exists());
    assert!(!t.path().join("qgate.toml").exists());
}

#[test]
fn existing_hook_refuses_then_chains() {
    let t = TestRepo::new();
    t.write(
        ".git/hooks/pre-commit",
        "#!/bin/sh\n# husky\necho ran > chained-ran.txt\n",
    );
    std::fs::set_permissions(
        t.path().join(".git/hooks/pre-commit"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let out = t.qgate().arg("install").assert().code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(
        err.contains("husky") && err.contains("qgate install --chain"),
        "{err}"
    );
    t.qgate().args(["install", "--chain"]).assert().success();
    t.write("a.rs", &func("small", 5));
    onboard(&t);
    assert!(
        t.path().join("chained-ran.txt").exists(),
        "the original hook still runs"
    );
}

#[test]
fn hooks_use_absolute_path_not_a_path_shim() {
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 5));
    onboard(&t);
    let hook = t.read(".git/hooks/pre-commit");
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_qgate"))
        .canonicalize()
        .unwrap();
    assert!(
        hook.contains(&format!("{} run --stage commit", exe.display())),
        "{hook}"
    );
    let shim = tempfile::tempdir().unwrap();
    std::fs::write(shim.path().join("qgate"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(
        shim.path().join("qgate"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    t.write("b.rs", &func("long", 70));
    t.git(&["add", "b.rs"]);
    let out = super::support::repo::git_cmd(t.path())
        .args(["commit", "-q", "-m", "x"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn markdown_only_commit_is_fast() {
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 5));
    onboard(&t);
    t.write("README.md", "# hello\n");
    t.git(&["add", "README.md"]);
    let start = Instant::now();
    let (ok, out) = git(&t, &["commit", "-q", "-m", "docs"]);
    assert!(ok, "{out}");
    assert!(
        start.elapsed().as_secs_f64() < 1.0,
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn commit_with_pathspec_checks_only_that_commit() {
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 5));
    onboard(&t);
    t.write("long.rs", &func("long", 70))
        .write("notes.md", "notes\n");
    t.git(&["add", "long.rs", "notes.md"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "notes only", "notes.md"]);
    assert!(ok, "{out}");
    let (ok, _) = git(&t, &["commit", "-q", "-m", "long"]);
    assert!(!ok);
}

#[test]
fn staged_long_version_is_checked_not_unstaged_short_one() {
    // AE9
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 5));
    onboard(&t);
    t.write("b.rs", &func("long", 70));
    t.git(&["add", "b.rs"]);
    t.write("b.rs", &func("long", 10));
    let before = t.fingerprint();
    let (ok, _) = git(&t, &["commit", "-q", "-m", "x"]);
    assert!(!ok);
    assert_eq!(
        before,
        t.fingerprint(),
        "a run changes nothing in the working tree or index"
    );
}

#[test]
fn inline_ignore_fails_commit() {
    // AE8
    let t = TestRepo::new();
    t.write("app.js", "const a = 1;\n");
    onboard(&t);
    t.write("app.js", "// eslint-disable-next-line\nconst a = 1;\n");
    t.git(&["add", "app.js"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "x"]);
    assert!(!ok);
    assert!(
        out.contains("inline ignore: app.js adds 1x `eslint-disable`"),
        "{out}"
    );
}

#[test]
fn json_output_and_version_pin() {
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 5));
    onboard(&t);
    t.write("b.rs", &func("long", 70));
    t.git(&["add", "b.rs"]);
    let out = t
        .qgate()
        .args(["run", "--stage", "commit", "--format", "json"])
        .assert()
        .code(1);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["passed"], false);
    assert_eq!(v["findings"][0]["rule"], "function-length");
    assert!(v["timings_ms"]["total"].is_number());
    assert!(v["fixes"].is_array());

    let cfg = t
        .read("qgate.toml")
        .replace(qgate::config::VERSION, "9.9.9");
    t.write("qgate.toml", &cfg).add(&["qgate.toml"]);
    let out = t
        .qgate()
        .args(["run", "--stage", "commit"])
        .assert()
        .code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(
        err.contains("cargo install") && err.contains("v9.9.9"),
        "{err}"
    );
}

#[test]
fn duplicate_of_untouched_helper_fails_commit() {
    // AE4
    let t = TestRepo::new();
    t.write("util.rs", &func("helper", 12));
    onboard(&t);
    t.write(
        "feature.rs",
        &func("helper", 12).replace("fn helper()", "fn copy()"),
    );
    t.git(&["add", "feature.rs"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "copy"]);
    assert!(!ok);
    assert!(
        out.contains("error[duplication]") && out.contains("also at util.rs:"),
        "{out}"
    );
}

/// Commit-hook budget: `cargo test --release --test e2e -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_commit_hook_mid_size_repo() {
    let t = TestRepo::new();
    for i in 0..2000 {
        let mut src = String::new();
        for j in 0..20 {
            src.push_str(&func(&format!("f_{i}_{j}"), 12));
        }
        t.write(&format!("src/m{}/f{i}.rs", i % 40), &src);
    }
    t.commit_all("mid-size repo");
    onboard(&t);
    for i in 0..10 {
        let path = format!("src/m{i}/f{i}.rs");
        let text = t.read(&path) + &func(&format!("added_{i}"), 15);
        t.write(&path, &text);
    }
    t.git(&["add", "-A"]);
    let start = Instant::now();
    t.qgate()
        .args(["run", "--stage", "commit"])
        .assert()
        .success();
    let elapsed = start.elapsed();
    println!("warm commit run over 2000 files (40k functions), 10 changed: {elapsed:?}");
    assert!(elapsed.as_secs_f64() < 10.0);
}
