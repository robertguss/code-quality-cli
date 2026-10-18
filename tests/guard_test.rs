mod support;

use qgate::baseline;
use qgate::config::VERSION;
use qgate::guard::{self, approvals};
use support::repo::TestRepo;

fn config(function_lines: u32) -> String {
    format!(
        "qgate_version = \"{VERSION}\"\nexclude = [\"testdata/**\"]\n\n[limits]\nfunction_lines = {function_lines}\n\n[[tool]]\nname = \"eslint\"\ncommand = \"npx eslint {{files}}\"\nglobs = [\"**/*.js\"]\nconfig_files = [\".eslintrc.json\"]\n\n[[tool]]\nname = \"tests\"\ncommand = \"npm test\"\nstage = \"push\"\n"
    )
}

/// A committed repo with config, an eslint config and package scripts.
fn setup() -> TestRepo {
    let t = TestRepo::new();
    t.write("qgate.toml", &config(50))
        .write(".eslintrc.json", "{\"rules\": {\"no-var\": \"error\"}}\n")
        .write(
            "package.json",
            "{\"name\": \"x\", \"version\": \"1.0.0\", \"scripts\": {\"test\": \"jest\"}}\n",
        )
        .write(
            "src/app.js",
            "const a = 1;\n// eslint-disable-next-line no-console\nconsole.log(a);\nconst b = 2;\n",
        );
    t.commit_all("init");
    t
}

fn pending(t: &TestRepo) -> Vec<String> {
    approvals::pending(&t.repo(), None)
        .unwrap()
        .lines
        .into_iter()
        .map(|(_, l)| l)
        .collect()
}

/// Pending lines match `expected` up to the content digest each line ends with.
#[track_caller]
fn assert_pending(t: &TestRepo, expected: &[&str]) {
    let lines = pending(t);
    assert_eq!(lines.len(), expected.len(), "{lines:?}");
    for (line, want) in lines.iter().zip(expected) {
        assert!(
            line.starts_with(want),
            "{line:?} should start with {want:?}"
        );
    }
}

fn blocked(t: &TestRepo) -> bool {
    let repo = t.repo();
    !guard::check(
        &repo,
        &approvals::pending(&repo, None).unwrap(),
        "qgate approve",
    )
    .is_empty()
}

#[test]
fn raising_a_limit_needs_approval() {
    // AE2
    let t = setup();
    t.write("qgate.toml", &config(80)).add(&["qgate.toml"]);
    let lines = pending(&t);
    assert_eq!(lines, ["config: function_lines 50 -> 80"]);
    let repo = t.repo();
    let f = guard::check(
        &repo,
        &approvals::pending(&repo, None).unwrap(),
        "qgate approve",
    );
    assert_eq!(f[0].fix.as_deref(), Some("qgate approve"));
    assert_eq!(f[0].path.as_deref(), Some("qgate.toml"));
}

#[test]
fn approval_is_voided_by_further_edits() {
    // AE7
    let t = setup();
    t.write("qgate.toml", &config(80)).add(&["qgate.toml"]);
    let r = approvals::approve(&t.repo(), None, false).unwrap();
    assert_eq!(r.data["status"], "approved");
    assert!(!blocked(&t));
    let again = approvals::approve(&t.repo(), None, false).unwrap();
    assert_eq!(again.data["status"], "already approved");
    t.write("qgate.toml", &config(90)).add(&["qgate.toml"]);
    assert!(blocked(&t));
}

#[test]
fn approve_dry_run_records_nothing() {
    let t = setup();
    t.write("qgate.toml", &config(80)).add(&["qgate.toml"]);
    let r = approvals::approve(&t.repo(), None, true).unwrap();
    assert_eq!(r.data["status"], "would approve");
    assert!(blocked(&t));
}

#[test]
fn lowering_a_limit_needs_nothing() {
    let t = setup();
    t.write("qgate.toml", &config(40)).add(&["qgate.toml"]);
    assert!(pending(&t).is_empty());
    assert_eq!(
        approvals::approve(&t.repo(), None, false).unwrap().data["status"],
        "nothing to approve"
    );
}

#[test]
fn inline_ignores_and_tool_configs() {
    // AE8
    let t = setup();
    t.write("src/app.js", "/* eslint-disable */\nconst a = 1;\n// eslint-disable-next-line no-console\nconsole.log(a);\nconst b = 2;\n")
        .add(&["src/app.js"]);
    assert_pending(&t, &["inline ignore: src/app.js adds 1x `eslint-disable`"]);
    t.git(&["reset", "-q", "--hard"]);
    t.write(".eslintrc.json", "{\"rules\": {}}\n")
        .add(&[".eslintrc.json"]);
    assert_pending(&t, &["tool config edited: .eslintrc.json (modified, "]);
}

#[test]
fn moving_an_ignore_line_is_not_new() {
    let t = setup();
    t.write(
        "src/app.js",
        "const b = 2;\nconst a = 1;\n// eslint-disable-next-line no-console\nconsole.log(a);\n",
    )
    .add(&["src/app.js"]);
    assert!(pending(&t).is_empty());
}

#[test]
fn other_ignore_markers() {
    let t = setup();
    t.write("src/a.ts", "// @ts-expect-error\nconst x: number = 'a';\n")
        .write("src/lib.rs", "#![allow(dead_code)]\nfn f() {}\n")
        .add(&["."]);
    let lines = pending(&t);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("inline ignore: src/a.ts adds 1x `@ts-expect-error` (")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("inline ignore: src/lib.rs adds 1x `#![allow(` (")),
        "{lines:?}"
    );
}

#[test]
fn exclusions_need_approval() {
    // AE10
    let t = setup();
    t.write(".gitattributes", "src/app.js linguist-generated\n")
        .add(&[".gitattributes"]);
    assert_pending(
        &t,
        &["exclusion: .gitattributes adds `src/app.js linguist-generated`"],
    );
    t.git(&["reset", "-q", "--hard"]);
    t.git(&["clean", "-fdq"]);
    t.write("vendor/big.js", "x\n").add(&["vendor/big.js"]);
    assert_pending(&t, &["exclusion: new file in excluded path vendor/big.js"]);
    t.git(&["reset", "-q", "--hard"]);
    t.git(&["clean", "-fdq"]);
    t.write("testdata/f.js", "x\n").add(&["testdata/f.js"]);
    assert_pending(&t, &["exclusion: new file in excluded path testdata/f.js"]);
    t.git(&["reset", "-q", "--hard"]);
    t.git(&["clean", "-fdq"]);
    t.write("sub/.eslintrc.json", "{}\n")
        .add(&["sub/.eslintrc.json"]);
    assert_pending(
        &t,
        &["tool config edited: sub/.eslintrc.json (added, none -> "],
    );
}

#[test]
fn package_scripts_are_guarded_but_other_keys_are_not() {
    let t = setup();
    t.write(
        "package.json",
        "{\"name\": \"x\", \"version\": \"1.0.1\", \"scripts\": {\"test\": \"jest\"}}\n",
    )
    .add(&["package.json"]);
    assert!(pending(&t).is_empty());
    t.write(
        "package.json",
        "{\"name\": \"x\", \"version\": \"1.0.1\", \"scripts\": {\"test\": \"true\"}}\n",
    )
    .add(&["package.json"]);
    assert_pending(&t, &["package scripts changed: package.json"]);
}

#[test]
fn tool_changes_and_removals() {
    let t = setup();
    let text = config(50).replace("npm test", "true");
    t.write("qgate.toml", &text).add(&["qgate.toml"]);
    let lines = pending(&t);
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].starts_with("config: tool `tests` changed"),
        "{lines:?}"
    );
}

#[test]
fn new_config_and_baseline_need_approval_then_pass() {
    // F2: onboarding a repo with violations.
    let t = TestRepo::new();
    let long: String = (0..60).map(|i| format!("    let v{i} = {i};\n")).collect();
    t.write("a.rs", &format!("fn big() {{\n{long}}}\n"));
    t.commit_all("existing code");
    t.write("qgate.toml", &config(50)).add(&["qgate.toml"]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.add(&["qgate.baseline"]);
    let lines = pending(&t);
    assert!(
        lines.contains(&"config: new qgate.toml".to_string()),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("baseline: added big#0 in a.rs")),
        "{lines:?}"
    );
    assert!(blocked(&t));
    approvals::approve(&t.repo(), None, false).unwrap();
    assert!(!blocked(&t));
    // Approval records live in .git and are never committed.
    assert_eq!(
        t.git(&["status", "--porcelain", "--", "."])
            .lines()
            .filter(|l| !l.starts_with('A'))
            .count(),
        0
    );
    assert!(
        t.path()
            .join(".git/qgate/approvals")
            .read_dir()
            .unwrap()
            .count()
            == 1
    );
}

#[test]
fn prune_output_needs_no_approval() {
    let t = TestRepo::new();
    let body = |n: usize| -> String { (0..n).map(|i| format!("    let v{i} = {i};\n")).collect() };
    t.write("qgate.toml", &config(50))
        .write("a.rs", &format!("fn big() {{\n{}}}\n", body(60)));
    t.add(&["."]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.commit_all("baseline");
    t.write("a.rs", &format!("fn big() {{\n{}}}\n", body(55)))
        .add(&["a.rs"]);
    baseline::prune(&t.repo(), false).unwrap();
    t.add(&["qgate.baseline"]);
    assert!(t.read("qgate.baseline").contains("\t57\n"));
    assert!(pending(&t).is_empty());
    // Hand-raising the entry is guarded.
    t.write(
        "qgate.baseline",
        &t.read("qgate.baseline").replace("\t57\n", "\t99\n"),
    )
    .add(&["qgate.baseline"]);
    assert_pending(&t, &["baseline: raised big#0 in a.rs 62 -> 99"]);
}

#[test]
fn per_commit_approval_and_cherry_pick() {
    let t = setup();
    t.write("qgate.toml", &config(80)).add(&["qgate.toml"]);
    approvals::approve(&t.repo(), None, false).unwrap();
    let approved = {
        t.git(&["commit", "-q", "--no-verify", "-m", "raise"]);
        t.git(&["rev-parse", "HEAD"])
    };
    let repo = t.repo();
    let l = approvals::pending(&repo, Some(&approved)).unwrap();
    assert!(
        approvals::is_approved(&repo, &l.hash()),
        "commit-time approval covers the commit"
    );

    t.git(&["checkout", "-q", "-b", "side", "HEAD~1"]);
    t.write("qgate.toml", &config(70));
    let side = t.commit_all("raise differently");
    let l = approvals::pending(&repo, Some(&side)).unwrap();
    assert!(!l.is_empty() && !approvals::is_approved(&repo, &l.hash()));
    let r = approvals::approve(&repo, Some(&side), false).unwrap();
    assert_eq!(r.data["status"], "approved");
}

#[test]
fn an_added_tool_needs_approval() {
    let t = setup();
    let text =
        config(50) + "\n[[tool]]\nname = \"sneaky\"\ncommand = \"touch .git/qgate/approvals/x\"\n";
    t.write("qgate.toml", &text).add(&["qgate.toml"]);
    let lines = pending(&t);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("config: tool `sneaky` added")),
        "{lines:?}"
    );
    assert!(blocked(&t));
}

#[test]
fn approving_a_tool_config_does_not_cover_a_second_edit() {
    // AE7
    let t = setup();
    t.write(".eslintrc.json", "{\"rules\": {}}\n")
        .add(&[".eslintrc.json"]);
    approvals::approve(&t.repo(), None, false).unwrap();
    assert!(!blocked(&t));
    t.write(
        ".eslintrc.json",
        "{\"rules\": {}, \"ignorePatterns\": [\"**\"]}\n",
    )
    .add(&[".eslintrc.json"]);
    assert!(blocked(&t));
}

#[test]
fn swapping_an_approved_ignore_for_a_wider_one_needs_approval() {
    // AE7
    let t = setup();
    let with = |line: &str| {
        format!("{line}\nconst a = 1;\n// eslint-disable-next-line no-console\nconsole.log(a);\n")
    };
    t.write("src/app.js", &with("/* eslint-disable no-var */"))
        .add(&["src/app.js"]);
    approvals::approve(&t.repo(), None, false).unwrap();
    assert!(!blocked(&t));
    t.write("src/app.js", &with("/* eslint-disable */"))
        .add(&["src/app.js"]);
    assert!(blocked(&t));
}

#[test]
fn scripts_a_tool_runs_are_guarded() {
    let t = TestRepo::new();
    let text = format!(
        "qgate_version = \"{VERSION}\"\n\n[[tool]]\nname = \"lint\"\ncommand = \"./scripts/lint.sh {{files}}\"\n"
    );
    t.write("qgate.toml", &text)
        .write("scripts/lint.sh", "#!/bin/sh\nshellcheck \"$@\"\n");
    t.commit_all("init");
    t.write("scripts/lint.sh", "#!/bin/sh\nexit 0\n")
        .add(&["scripts/lint.sh"]);
    let lines = pending(&t);
    assert!(
        lines.iter().any(|l| l.contains("scripts/lint.sh")),
        "{lines:?}"
    );
}

#[test]
fn user_attributes_file_does_not_exclude_files() {
    let t = setup();
    let attrs = t.path().join("user-attrs");
    std::fs::write(&attrs, "src/app.js linguist-generated\n").unwrap();
    t.git(&["config", "core.attributesFile", attrs.to_str().unwrap()]);
    let marked = qgate::checks::exclude::marked_paths(
        &t.repo(),
        &qgate::git::Source::Index,
        &["src/app.js"],
    )
    .unwrap();
    assert!(marked.is_empty(), "{marked:?}");
}
