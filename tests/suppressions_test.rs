//! R6: new inline suppressions need a reason and an expiry, always-block ones
//! fail whatever they carry, and expired ones fail where their file is touched.

mod support;

use qgate::cli::{Finding, Severity};
use qgate::config::VERSION;
use qgate::date::Date;
use qgate::git::{Snapshot, changes};
use qgate::guard::{self, Loosening};
use support::repo::{TestRepo, git_cmd};

const TODAY: &str = "2026-10-22";

fn date(s: &str) -> Date {
    Date::parse(s).unwrap()
}

/// A committed repo with a config and one TypeScript and one Rust file.
fn setup(extra_config: &str) -> TestRepo {
    let t = TestRepo::new();
    t.write(
        "qgate.toml",
        &format!("qgate_version = \"{VERSION}\"\n{extra_config}"),
    )
    .write("src/a.ts", "export const a = 1;\n")
    .write("src/b.ts", "export const b = 2;\n")
    .write("src/lib.rs", "pub fn f() {}\n");
    t.commit_all("init");
    t
}

fn staged_on(t: &TestRepo, today: &str) -> Loosening {
    let repo = t.repo();
    let parent = Snapshot::at(&repo, repo.head().as_deref()).unwrap();
    let current = Snapshot::index(&repo).unwrap();
    let changes = changes::staged(&repo).unwrap();
    guard::loosening(&repo, &parent, &current, &changes, date(today)).unwrap()
}

/// R6 errors for the staged change on `TODAY`.
fn r6(t: &TestRepo) -> Vec<Finding> {
    r6_on(t, TODAY)
}

fn r6_on(t: &TestRepo, today: &str) -> Vec<Finding> {
    guard::check(&staged_on(t, today))
        .into_iter()
        .filter(|f| f.rule == "suppression")
        .collect()
}

fn lines(t: &TestRepo) -> Vec<String> {
    staged_on(t, TODAY)
        .lines
        .into_iter()
        .map(|(_, l)| l)
        .collect()
}

/// Stage `src/a.ts` with `line` added above its code.
fn add_line(t: &TestRepo, line: &str) {
    t.write("src/a.ts", &format!("{line}\nexport const a = 1;\n"))
        .add(&["src/a.ts"]);
}

#[track_caller]
fn assert_fails(found: &[Finding], needle: &str) {
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].severity, Severity::Error);
    assert!(found[0].message.contains(needle), "{found:?}");
    assert_eq!(found[0].path.as_deref(), Some("src/a.ts"));
    assert_eq!(found[0].line, Some(1));
}

#[test]
fn a_dated_reasoned_suppression_passes_and_still_warns() {
    let t = setup("");
    add_line(
        &t,
        "// eslint-disable-next-line no-console -- reason: CLI output, until: 2026-12-31",
    );
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
    assert!(
        lines(&t)
            .iter()
            .any(|l| l.starts_with("inline ignore: src/a.ts adds 1x `eslint-disable`")),
        "{:?}",
        lines(&t)
    );
}

#[test]
fn a_missing_reason_or_until_fails() {
    let t = setup("");
    add_line(
        &t,
        "// eslint-disable-next-line no-console -- until: 2026-12-31",
    );
    assert_fails(&r6(&t), "reason:");
    add_line(
        &t,
        "// eslint-disable-next-line no-console -- reason: CLI output",
    );
    assert_fails(&r6(&t), "until:");
    add_line(&t, "// eslint-disable-next-line no-console");
    let found = r6(&t);
    assert_fails(&found, "reason:");
    assert!(found[0].message.contains("until:"), "{found:?}");
}

#[test]
fn an_until_past_the_horizon_fails_unless_the_parent_allows_it() {
    let line = "// eslint-disable-next-line no-console -- reason: x, until: 2027-06-01";
    let t = setup("");
    add_line(&t, line);
    assert_fails(&r6(&t), "90 days");
    // Raising max_days in the same change does not help that change.
    t.write(
        "qgate.toml",
        &format!("qgate_version = \"{VERSION}\"\n[suppressions]\nmax_days = 365\n"),
    )
    .add(&["qgate.toml"]);
    assert_fails(&r6(&t), "90 days");

    let t = setup("[suppressions]\nmax_days = 365\n");
    add_line(&t, line);
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
}

#[test]
fn always_block_markers_fail_whatever_they_carry() {
    let t = setup("");
    add_line(&t, "// @ts-ignore -- reason: x, until: 2026-11-01");
    assert_fails(&r6(&t), "always_block");
    add_line(&t, "/* eslint-disable -- reason: x, until: 2026-11-01 */");
    assert_fails(&r6(&t), "always_block");
    // An eslint-disable naming its rules is judged like any other marker.
    add_line(
        &t,
        "/* eslint-disable no-var -- reason: x, until: 2026-11-01 */",
    );
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
}

#[test]
fn qgate_allow_for_an_always_block_rule_fails() {
    let t = setup("");
    for marker in [
        "// qgate: allow swallowed-error/rust-ok-statement -- reason: x, until: 2026-11-01",
        "// qgate: allow swallowed-error -- reason: x, until: 2026-11-01",
    ] {
        add_line(&t, marker);
        assert_fails(&r6(&t), "always_block");
    }
    add_line(
        &t,
        "// qgate: allow escape-hatch/rust-unwrap -- reason: x, until: 2026-11-01",
    );
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
    assert!(
        lines(&t)
            .iter()
            .any(|l| l.starts_with("inline ignore: src/a.ts adds 1x `qgate: allow`")),
        "{:?}",
        lines(&t)
    );
}

#[test]
fn an_expired_suppression_fails_only_when_its_file_changes() {
    let t = setup("");
    t.write(
        "src/a.ts",
        "// eslint-disable-next-line no-console -- reason: x, until: 2026-10-01\nexport const a = 1;\n",
    );
    t.commit_all("dated");
    t.write("src/b.ts", "export const b = 3;\n")
        .add(&["src/b.ts"]);
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
    t.write(
        "src/a.ts",
        "// eslint-disable-next-line no-console -- reason: x, until: 2026-10-01\nexport const a = 2;\n",
    )
    .add(&["src/a.ts"]);
    assert_fails(&r6(&t), "expired");
    // Before the date it is fine.
    assert!(r6_on(&t, "2026-09-30").is_empty());
}

#[test]
fn moving_an_until_later_warns_with_both_dates() {
    let t = setup("");
    let at = |until: &str| {
        format!(
            "// eslint-disable-next-line no-console -- reason: x, until: {until}\nexport const a = 1;\n"
        )
    };
    t.write("src/a.ts", &at("2026-11-01"));
    t.commit_all("dated");
    t.write("src/a.ts", &at("2026-12-01")).add(&["src/a.ts"]);
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
    assert!(
        lines(&t)
            .iter()
            .any(|l| l == "inline ignore: src/a.ts moves until 2026-11-01 -> 2026-12-01"),
        "{:?}",
        lines(&t)
    );
    // An earlier date is not a loosening.
    t.write("src/a.ts", &at("2026-10-30")).add(&["src/a.ts"]);
    assert!(
        !lines(&t).iter().any(|l| l.contains("moves until")),
        "{:?}",
        lines(&t)
    );
}

#[test]
fn removing_an_until_fails_and_names_the_old_date() {
    let t = setup("");
    t.write(
        "src/a.ts",
        "// eslint-disable-next-line no-console -- reason: x, until: 2026-11-01\nexport const a = 1;\n",
    );
    t.commit_all("dated");
    add_line(&t, "// eslint-disable-next-line no-console -- reason: x");
    assert_fails(&r6(&t), "until:");
    assert!(
        lines(&t)
            .iter()
            .any(|l| l == "inline ignore: src/a.ts moves until 2026-11-01 -> none"),
        "{:?}",
        lines(&t)
    );
}

#[test]
fn a_moved_suppression_is_not_new() {
    let t = setup("");
    t.write(
        "src/a.ts",
        "// eslint-disable-next-line no-console\nexport const a = 1;\nexport const c = 3;\n",
    );
    t.commit_all("undated");
    t.write(
        "src/a.ts",
        "export const c = 3;\n// eslint-disable-next-line no-console\nexport const a = 1;\n",
    )
    .add(&["src/a.ts"]);
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
    assert!(lines(&t).is_empty(), "{:?}", lines(&t));
}

#[test]
fn markers_outside_comments_are_not_judged() {
    let t = setup("");
    t.write(
        "src/lib.rs",
        "pub const M: &str = \"@ts-ignore\";\npub fn f() {}\n",
    )
    .write("README.md", "Write `// @ts-ignore` and see.\n")
    .add(&["."]);
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
    let l = lines(&t);
    assert!(
        l.iter()
            .any(|l| l.starts_with("inline ignore: src/lib.rs adds 1x `@ts-ignore`")),
        "{l:?}"
    );
    assert!(
        l.iter()
            .any(|l| l.starts_with("inline ignore: README.md adds 1x `@ts-ignore`")),
        "{l:?}"
    );
    // The same marker in a Rust comment is judged.
    t.write(
        "src/lib.rs",
        "// @ts-ignore -- reason: x, until: 2026-11-01\npub fn f() {}\n",
    )
    .add(&["src/lib.rs"]);
    let found = r6(&t);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path.as_deref(), Some("src/lib.rs"));
}

/// Commit everything with a fixed committer date.
fn commit_on(t: &TestRepo, day: &str, msg: &str) {
    t.git(&["add", "-A"]);
    let out = git_cmd(t.path())
        .env("GIT_COMMITTER_DATE", format!("{day}T12:00:00+00:00"))
        .args(["commit", "-q", "--no-verify", "-m", msg])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn the_push_date_is_the_tip_commit_date() {
    let t = setup("");
    let base = t.git(&["rev-parse", "HEAD"]);
    t.write(
        "src/a.ts",
        "// eslint-disable-next-line no-console -- reason: x, until: 2026-10-01\nexport const a = 1;\n",
    );
    commit_on(&t, "2026-09-15", "dated");
    let push = |t: &TestRepo| {
        t.qgate()
            .args([
                "run", "--stage", "push", "--base", &base, "--format", "json",
            ])
            .output()
            .unwrap()
    };
    let out = push(&t);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("\"suppression\""),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    t.write(
        "src/a.ts",
        "// eslint-disable-next-line no-console -- reason: x, until: 2026-10-01\nexport const a = 2;\n",
    );
    commit_on(&t, "2026-10-05", "later");
    let out = push(&t);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"suppression\""), "{stdout}");
    assert!(!out.status.success());
}

#[test]
fn an_until_that_is_not_a_date_fails() {
    let t = setup("");
    add_line(
        &t,
        "// eslint-disable-next-line no-console -- reason: x, until: soon",
    );
    assert_fails(&r6(&t), "not a YYYY-MM-DD date");
}

#[test]
fn until_may_be_exactly_max_days_ahead() {
    let t = setup("");
    add_line(
        &t,
        "// eslint-disable-next-line no-console -- reason: x, until: 2027-01-20",
    );
    assert!(r6(&t).is_empty(), "{:?}", r6(&t));
    add_line(
        &t,
        "// eslint-disable-next-line no-console -- reason: x, until: 2027-01-21",
    );
    assert_fails(&r6(&t), "more than 90 days");
}
