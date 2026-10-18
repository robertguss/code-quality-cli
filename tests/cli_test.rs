use assert_cmd::Command;
use predicates::prelude::*;

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
    &["approve"],
    &["agent-hook"],
    &["usage"],
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
        "cmd approve",
        "cmd agent-hook",
        "cmd claude",
    ] {
        assert!(spec.contains(cmd), "missing {cmd}");
    }
    for flag in [
        "--stage",
        "--format",
        "--dry-run",
        "--chain",
        "--force",
        "--commit",
        "--hook-input",
        "--remote",
    ] {
        assert!(spec.contains(&format!("flag {flag}")), "missing {flag}");
    }
}
