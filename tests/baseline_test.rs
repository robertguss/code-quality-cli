mod support;

use qgate::baseline::compare::{CheckInput, check};
use qgate::baseline::{self, Baseline, Key, PRUNE_FIX, Value, merge_driver};
use qgate::cli::{Finding, Severity};
use qgate::config::{self, VERSION};
use qgate::git::{Snapshot, changes};
use support::repo::TestRepo;

fn repo_with(dup: bool) -> TestRepo {
    let t = TestRepo::new();
    t.write(
        "qgate.toml",
        &format!(
            "qgate_version = \"{VERSION}\"\n[limits]\nfunction_lines = 50\nfile_lines = 1000\n[duplication]\nenabled = {dup}\nmin_tokens = 30\n"
        ),
    );
    t
}

/// A Rust function `name` of exactly `lines` lines; `salt` makes bodies differ.
fn func(name: &str, lines: usize, salt: &str) -> String {
    let mut s = format!("fn {name}() {{\n");
    for i in 0..lines - 2 {
        s.push_str(&format!("    let v{i} = \"{salt}{i}\";\n"));
    }
    s.push_str("}\n");
    s
}

fn check_staged(t: &TestRepo) -> Vec<Finding> {
    let repo = t.repo();
    let current = Snapshot::index(&repo).unwrap();
    let parent = Snapshot::at(&repo, repo.head().as_deref()).unwrap();
    let changes = changes::staged(&repo).unwrap();
    let config = config::load(&repo, &current).unwrap();
    let baseline = baseline::from_snapshot(&repo, &current).unwrap();
    check(&CheckInput {
        repo: &repo,
        config: &config,
        current: &current,
        parent: &parent,
        changes: &changes,
        baseline: &baseline,
    })
    .unwrap()
}

fn errors(f: &[Finding]) -> Vec<&Finding> {
    f.iter().filter(|f| f.severity == Severity::Error).collect()
}

fn has_prune_hint(f: &[Finding]) -> bool {
    f.iter()
        .any(|f| f.rule == "baseline-prunable" && f.fix.as_deref() == Some(PRUNE_FIX))
}

/// Commit `a.rs` with an 80-line `big` and a matching baseline.
fn baselined_80(dup: bool) -> TestRepo {
    let t = repo_with(dup);
    t.write("a.rs", &func("big", 80, "x"));
    t.add(&["."]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.commit_all("baseline");
    assert_eq!(
        Baseline::parse(&t.read("qgate.baseline"))
            .unwrap()
            .get(&Key::length("function-length", "a.rs", Some("big#0"))),
        Some(Value::Lines(80))
    );
    t
}

#[test]
fn ratchet_grow_shrink_regrow() {
    // AE1
    let t = baselined_80(false);
    t.write("a.rs", &func("big", 90, "x")).add(&["a.rs"]);
    let f = check_staged(&t);
    assert_eq!(errors(&f).len(), 1, "{f:?}");
    assert!(errors(&f)[0].message.contains("grew to 90"));

    t.write("a.rs", &func("big", 40, "x")).add(&["a.rs"]);
    let f = check_staged(&t);
    assert!(errors(&f).is_empty(), "{f:?}");
    assert!(has_prune_hint(&f), "{f:?}");
    t.commit_all("shrink without prune");

    t.write("a.rs", &func("big", 70, "x")).add(&["a.rs"]);
    let f = check_staged(&t);
    assert_eq!(errors(&f).len(), 1, "{f:?}");
}

#[test]
fn unchanged_baselined_file_passes_and_new_violation_fails() {
    let t = baselined_80(false);
    t.write("b.rs", &func("other", 60, "y")).add(&["b.rs"]);
    let f = check_staged(&t);
    assert_eq!(errors(&f).len(), 1);
    assert_eq!(errors(&f)[0].path.as_deref(), Some("b.rs"));
    assert!(errors(&f)[0].message.contains("60 lines (limit 50)"));
}

#[test]
fn deleted_function_is_prunable() {
    let t = baselined_80(false);
    t.write("a.rs", "fn small() {}\n").add(&["a.rs"]);
    let f = check_staged(&t);
    assert!(errors(&f).is_empty());
    assert!(has_prune_hint(&f));
}

#[test]
fn prune_is_idempotent_and_never_grows() {
    let t = baselined_80(false);
    t.write("a.rs", &(func("big", 60, "x") + &func("fresh", 70, "z")))
        .add(&["a.rs"]);
    let first = baseline::prune(&t.repo(), false).unwrap();
    assert_eq!(first.data["changed"], true);
    let b = Baseline::parse(&t.read("qgate.baseline")).unwrap();
    assert_eq!(b.entries.len(), 1, "prune must not add `fresh`");
    assert_eq!(
        b.get(&Key::length("function-length", "a.rs", Some("big#0"))),
        Some(Value::Lines(60))
    );
    let second = baseline::prune(&t.repo(), false).unwrap();
    assert_eq!(second.data["changed"], false);

    // Growing back past the pruned value is not "fixed" by prune.
    t.write("a.rs", &func("big", 75, "x")).add(&["a.rs"]);
    baseline::prune(&t.repo(), false).unwrap();
    let b = Baseline::parse(&t.read("qgate.baseline")).unwrap();
    assert_eq!(
        b.get(&Key::length("function-length", "a.rs", Some("big#0"))),
        Some(Value::Lines(60))
    );
}

#[test]
fn prune_dry_run_writes_nothing() {
    let t = baselined_80(false);
    t.write("a.rs", &func("big", 60, "x")).add(&["a.rs"]);
    let before = t.read("qgate.baseline");
    let r = baseline::prune(&t.repo(), true).unwrap();
    assert_eq!(r.data["changed"], true);
    assert_eq!(t.read("qgate.baseline"), before);
}

#[test]
fn whole_file_rename_keeps_limit_after_prune() {
    let t = baselined_80(false);
    t.git(&["mv", "a.rs", "b.rs"]);
    let f = check_staged(&t);
    let errs = errors(&f);
    assert_eq!(errs.len(), 1, "{f:?}");
    assert!(errs[0].message.contains("moved here"));
    assert_eq!(errs[0].fix.as_deref(), Some(PRUNE_FIX));
    let r = baseline::prune(&t.repo(), false).unwrap();
    assert!(r.findings.iter().any(|f| f.rule == "baseline-moved"));
    t.add(&["qgate.baseline"]);
    assert!(errors(&check_staged(&t)).is_empty());
}

#[test]
fn moved_function_keeps_limit_and_cannot_grow() {
    let t = baselined_80(false);
    t.write("a.rs", "fn keep() {}\n")
        .write("other.rs", &func("big", 80, "x"))
        .add(&["."]);
    let f = check_staged(&t);
    assert!(
        errors(&f).iter().all(|e| e.message.contains("moved here")),
        "{f:?}"
    );
    baseline::prune(&t.repo(), false).unwrap();
    t.add(&["qgate.baseline"]);
    assert!(errors(&check_staged(&t)).is_empty());
    t.commit_all("move");
    t.write("other.rs", &func("big", 85, "x"))
        .add(&["other.rs"]);
    assert_eq!(errors(&check_staged(&t)).len(), 1);
}

#[test]
fn renamed_and_changed_function_is_new() {
    let t = baselined_80(false);
    t.write("a.rs", &func("renamed", 80, "changed"))
        .add(&["a.rs"]);
    let f = check_staged(&t);
    assert_eq!(errors(&f).len(), 1);
    assert!(errors(&f)[0].message.contains("80 lines (limit 50)"));
}

#[test]
fn moved_clone_member_keeps_key() {
    let t = repo_with(true);
    let body = func("shared", 20, "s");
    t.write("a.rs", &body)
        .write("b.rs", &body.replace("shared", "copy"))
        .add(&["."]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.commit_all("baseline");
    assert!(t.read("qgate.baseline").contains("duplication\t-\t"));
    std::fs::create_dir_all(t.path().join("moved")).unwrap();
    t.git(&["mv", "b.rs", "moved/b.rs"]);
    assert!(errors(&check_staged(&t)).is_empty());
    // A third copy is worse.
    t.write("c.rs", &body.replace("shared", "third"))
        .add(&["c.rs"]);
    let f = check_staged(&t);
    assert!(!errors(&f).is_empty(), "{f:?}");
    assert!(errors(&f)[0].message.contains("gained copies"));
}

#[test]
fn create_refuses_to_overwrite_without_force() {
    let t = baselined_80(false);
    let err = baseline::create(&t.repo(), false, false).unwrap_err();
    assert!(format!("{err:#}").contains("--force"));
    baseline::create(&t.repo(), true, false).unwrap();
}

#[test]
fn create_uses_unstaged_config_edits_and_says_so() {
    let t = repo_with(false);
    t.write("a.rs", &func("mid", 40, "a"))
        .add(&["qgate.toml", "a.rs"]);
    let staged = baseline::create(&t.repo(), false, true).unwrap();
    assert_eq!(staged.data["config"], "staged");
    assert_eq!(staged.data["entries"], 0);

    // Lower the limit without staging it: the edit still applies.
    let text = t
        .read("qgate.toml")
        .replace("function_lines = 50", "function_lines = 30");
    t.write("qgate.toml", &text);
    let r = baseline::create(&t.repo(), false, true).unwrap();
    assert_eq!(r.data["config"], "working tree (unstaged changes)");
    assert_eq!(r.data["entries"], 1);

    baseline::create(&t.repo(), false, false).unwrap();
    let again = baseline::create(&t.repo(), true, false).unwrap();
    assert_eq!(again.data["written"], false);
    assert_eq!(again.data["reason"], "unchanged from qgate.baseline");
}

fn parse(lines: &[&str]) -> Baseline {
    Baseline::parse(&lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap()
}

#[test]
fn merge_rules() {
    let base = parse(&[
        "function-length\ta.rs\tf#0\t80",
        "function-length\ta.rs\tg#0\t90",
        "file-length\tc.rs\t-\t700",
    ]);
    let ours = parse(&[
        "function-length\ta.rs\tf#0\t70",
        "function-length\ta.rs\tg#0\t90",
        "file-length\tc.rs\t-\t700",
        "file-length\tn.rs\t-\t600",
    ]);
    let theirs = parse(&[
        "function-length\ta.rs\tf#0\t80",
        "function-length\ta.rs\tg#0\t60",
    ]);
    let m = merge_driver::merge(&base, &ours, &theirs);
    assert_eq!(
        m.render(),
        parse(&[
            "function-length\ta.rs\tf#0\t70",
            "function-length\ta.rs\tg#0\t60",
            "file-length\tn.rs\t-\t600"
        ])
        .render()
    );
}

#[test]
fn git_merge_with_driver_has_no_conflict() {
    let t = repo_with(false);
    t.write("a.rs", &(func("f", 80, "a") + &func("g", 90, "b")));
    t.add(&["."]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.write(".gitattributes", "qgate.baseline merge=qgate-baseline\n");
    t.commit_all("base");
    let driver = format!("{} baseline merge %O %A %B", env!("CARGO_BIN_EXE_qgate"));
    t.git(&["config", "merge.qgate-baseline.driver", &driver]);
    let text = t.read("qgate.baseline");
    t.git(&["checkout", "-q", "-b", "side"]);
    t.write("qgate.baseline", &text.replace("\t80\n", "\t70\n"));
    t.commit_all("lower f");
    t.git(&["checkout", "-q", "main"]);
    t.write("qgate.baseline", &text.replace("\t90\n", "\t60\n"));
    t.commit_all("lower g");
    t.git(&["merge", "-q", "--no-edit", "side"]);
    let merged = Baseline::parse(&t.read("qgate.baseline")).unwrap();
    assert_eq!(
        merged.get(&Key::length("function-length", "a.rs", Some("f#0"))),
        Some(Value::Lines(70))
    );
    assert_eq!(
        merged.get(&Key::length("function-length", "a.rs", Some("g#0"))),
        Some(Value::Lines(60))
    );
}

#[test]
fn clone_losing_a_copy_says_what_changed() {
    let t = repo_with(true);
    let body = func("shared", 20, "s");
    t.write("a.rs", &body)
        .write(
            "b.rs",
            &format!(
                "{}{}",
                body.replace("shared", "copy"),
                body.replace("shared", "again")
            ),
        )
        .write("c.rs", &body.replace("shared", "third"))
        .add(&["."]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.commit_all("baseline");
    let prunable = |t: &TestRepo| {
        let f = check_staged(t);
        assert!(errors(&f).is_empty(), "{f:?}");
        f.into_iter()
            .find(|f| f.rule == "baseline-prunable")
            .unwrap()
            .message
    };
    // One copy in b.rs is rewritten; b.rs still holds the other.
    t.write(
        "b.rs",
        &format!(
            "{}{}",
            body.replace("shared", "copy"),
            func("again", 5, "z")
        ),
    )
    .add(&["b.rs"]);
    let msg = prunable(&t);
    assert!(
        msg.contains("now has 3 copies of") && msg.contains("(baseline: 4 of"),
        "{msg}"
    );
    t.git(&["reset", "-q", "--hard"]);
    // The only copy in c.rs is rewritten.
    t.write("c.rs", &func("third", 5, "z")).add(&["c.rs"]);
    let msg = prunable(&t);
    assert!(
        msg.contains("no longer has a copy in the changed files"),
        "{msg}"
    );
}
