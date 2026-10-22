mod support;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use support::repo::TestRepo;

fn qgate() -> Command {
    Command::cargo_bin("qgate").unwrap()
}

const SUBCOMMANDS: &[&[&str]] = &[
    &["install"],
    &["run"],
    &["baseline"],
    &["baseline", "create"],
    &["baseline", "prune"],
    &["baseline", "merge"],
    &["agent-hook"],
    &["update"],
    &["usage"],
    &["rules"],
];

#[test]
fn every_help_has_runnable_examples() {
    for path in SUBCOMMANDS {
        let out = qgate().args(*path).arg("--help").assert().success();
        let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        let examples = text
            .split("Examples:\n")
            .nth(1)
            .unwrap_or_else(|| panic!("no Examples in {path:?} help"));
        let first = examples.lines().next().unwrap().trim();
        assert!(first.contains("qgate "), "{path:?}: {first}");
    }
}

#[test]
fn missing_required_flag_fails_fast_with_example() {
    qgate()
        .arg("run")
        .write_stdin("")
        .timeout(std::time::Duration::from_secs(5))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--stage"))
        .stderr(predicate::str::contains("qgate run --stage commit"));
}

#[test]
fn unknown_value_fails_with_example() {
    qgate()
        .args(["run", "--stage", "deploy"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Examples:"));
}

#[test]
fn usage_spec_lists_every_command_and_flag() {
    let out = qgate().arg("usage").assert().success();
    let spec = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    for cmd in [
        "cmd install",
        "cmd run",
        "cmd baseline",
        "cmd create",
        "cmd prune",
        "cmd merge",
        "cmd agent-hook",
        "cmd claude",
        "cmd rules",
    ] {
        assert!(spec.contains(cmd), "missing {cmd}");
    }
    for flag in [
        "--stage",
        "--format",
        "--dry-run",
        "--chain",
        "--force",
        "--hook-input",
        "--remote",
        "--skill",
        "--family",
    ] {
        assert!(spec.contains(&format!("flag {flag}")), "missing {flag}");
    }
}

#[test]
fn skill_prints_the_bundled_skill_for_this_version() {
    qgate()
        .arg("--skill")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("---\nname: qgate\n"))
        .stdout(predicate::str::contains(format!(
            "<!-- qgate {}.",
            env!("CARGO_PKG_VERSION")
        )));
}

#[test]
fn skill_refuses_a_command() {
    qgate()
        .args(["--skill", "install"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--skill takes no command"));
}

/// The `rules` rows of `cmd` run as `qgate rules <args> --format json`.
fn rule_rows(mut cmd: Command, args: &[&str]) -> Vec<Value> {
    let out = cmd
        .arg("rules")
        .args(args)
        .args(["--format", "json"])
        .assert()
        .success();
    let report: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    report["data"]["rules"].as_array().unwrap().clone()
}

fn row<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no row for {id}"))
}

/// A `qgate` command in a temp dir that is not inside any git repo.
fn outside_repo(dir: &tempfile::TempDir) -> Command {
    let mut cmd = qgate();
    cmd.current_dir(dir.path())
        .env("GIT_CEILING_DIRECTORIES", dir.path().parent().unwrap())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    cmd
}

#[test]
fn rules_lists_every_rule_with_defaults_outside_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let rows = rule_rows(outside_repo(&dir), &[]);
    assert_eq!(rows.len(), qgate::rules::all().len());
    for id in [
        "function-length",
        "file-length",
        "duplication",
        "escape-hatch/rust-unwrap",
    ] {
        let r = row(&rows, id);
        for field in [
            "id",
            "family",
            "kind",
            "level",
            "language",
            "source",
            "rationale",
            "fix",
            "since",
        ] {
            assert!(
                r[field].as_str().is_some_and(|s| !s.is_empty()),
                "{id}.{field}"
            );
        }
    }
    for rule in qgate::rules::all() {
        assert_eq!(row(&rows, rule.id)["level"], rule.default_level().as_str());
    }
    let unwrap = row(&rows, "escape-hatch/rust-unwrap");
    assert_eq!(unwrap["kind"], "exact");
    assert_eq!(unwrap["family"], "escape-hatch");
}

#[test]
fn rules_reads_levels_from_the_repo_config() {
    let t = TestRepo::new();
    t.write(
        "qgate.toml",
        &format!(
            "qgate_version = \"{}\"\n[rules]\n\"escape-hatch\" = \"warn\"\n",
            qgate::config::VERSION
        ),
    );
    let rows = rule_rows(t.qgate(), &[]);
    let escape: Vec<_> = rows
        .iter()
        .filter(|r| r["family"] == "escape-hatch")
        .collect();
    assert!(!escape.is_empty());
    for r in escape {
        assert_eq!(r["level"], "warn", "{}", r["id"]);
    }
    assert_eq!(row(&rows, "function-length")["level"], "error");
}

#[test]
fn rules_reads_the_committed_config_when_the_worktree_has_none() {
    let t = TestRepo::new();
    t.write(
        "qgate.toml",
        &format!(
            "qgate_version = \"{}\"\n[rules]\n\"function-length\" = \"off\"\n",
            qgate::config::VERSION
        ),
    );
    t.commit_all("config");
    t.remove("qgate.toml");
    let rows = rule_rows(t.qgate(), &[]);
    assert_eq!(row(&rows, "function-length")["level"], "off");
}

#[test]
fn rules_still_reads_levels_when_the_version_pin_differs() {
    let t = TestRepo::new();
    t.write(
        "qgate.toml",
        "qgate_version = \"0.0.1\"\n[rules]\n\"escape-hatch/rust-unwrap\" = \"off\"\n",
    );
    t.qgate()
        .args(["rules", "escape-hatch/rust-unwrap"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pins qgate 0.0.1"))
        .stdout(predicate::str::contains(
            "info[escape-hatch/rust-unwrap] off,",
        ));
}

#[test]
fn rules_with_an_id_shows_only_that_rule() {
    let dir = tempfile::tempdir().unwrap();
    let out = outside_repo(&dir)
        .args(["rules", "escape-hatch/rust-unwrap"])
        .assert()
        .success();
    let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let rules: Vec<_> = text
        .lines()
        .filter_map(|l| l.strip_prefix("info["))
        .collect();
    assert_eq!(rules.len(), 1, "{text}");
    assert!(
        rules[0].starts_with("escape-hatch/rust-unwrap] error, exact"),
        "{text}"
    );
}

#[test]
fn rules_with_an_unknown_id_or_family_is_a_usage_error() {
    for args in [&["rules", "nope"][..], &["rules", "--family", "nope"]] {
        qgate()
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("`nope`"))
            .stderr(predicate::str::contains("qgate rules"));
    }
}

#[test]
fn rules_family_filters_to_one_family() {
    let dir = tempfile::tempdir().unwrap();
    let rows = rule_rows(outside_repo(&dir), &["--family", "debug-output"]);
    assert!(rows.iter().any(|r| r["id"] == "debug-output/rust-println"));
    for r in &rows {
        assert_eq!(r["family"], "debug-output", "{}", r["id"]);
    }
}
