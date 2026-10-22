//! The rules engine and the occurrence ratchet (U4, U5), driven through
//! `escape-hatch/rust-unwrap`.

mod support;

use qgate::baseline::compare::{CheckInput, check};
use qgate::baseline::{self, Baseline, Key, PRUNE_FIX, Value, merge_driver};
use qgate::checks::length::measure_with_macros;
use qgate::cli::{Finding, Severity};
use qgate::config::{self, Config, Limits, VERSION};
use qgate::git::{Snapshot, changes};
use qgate::rules::engine::RuleSet;
use support::repo::TestRepo;

const UNWRAP: &str = "escape-hatch/rust-unwrap";

fn config(extra: &str) -> String {
    format!("qgate_version = \"{VERSION}\"\n[duplication]\nenabled = false\n{extra}")
}

fn repo(extra: &str) -> TestRepo {
    let t = TestRepo::new();
    t.write("qgate.toml", &config(extra));
    t
}

/// A Rust file with one function holding `n` unwraps, one per line.
fn unwraps(n: usize) -> String {
    let mut s = String::from("pub fn f(v: Option<u32>) -> u32 {\n    let mut t = 0;\n");
    for i in 0..n {
        s.push_str(&format!("    t += v.unwrap() + {i};\n"));
    }
    s.push_str("    t\n}\n");
    s
}

fn check_staged(t: &TestRepo) -> Vec<Finding> {
    let repo = t.repo();
    let current = Snapshot::index(&repo).unwrap();
    let parent = Snapshot::at(&repo, repo.head().as_deref()).unwrap();
    let changes = changes::staged(&repo).unwrap();
    let config = config::load(&repo, &current).unwrap();
    let baseline = baseline::from_snapshot(&repo, &current).unwrap();
    let findings = check(&CheckInput {
        repo: &repo,
        config: &config,
        current: &current,
        parent: &parent,
        changes: &changes,
        baseline: &baseline,
    })
    .unwrap();
    qgate::rules::apply_levels(&config, findings)
}

fn of(f: &[Finding], rule: &str, severity: Severity) -> Vec<Finding> {
    f.iter()
        .filter(|f| f.rule == rule && f.severity == severity)
        .cloned()
        .collect()
}

fn count_lines(path: &str, src: &str, cfg: &str) -> Vec<u32> {
    let config = Config::parse(&config(cfg), "qgate.toml").unwrap();
    let limits = Limits {
        function_lines: 1000,
        file_lines: 10_000,
    };
    measure_with_macros(
        path,
        src.as_bytes(),
        limits,
        &[],
        &RuleSet::from_config(&config),
    )
    .occurrences
    .iter()
    .filter(|o| o.rule == UNWRAP)
    .map(|o| o.line)
    .collect()
}

/// `a.rs` committed with `n` unwraps and its baseline entry.
fn baselined(n: usize) -> TestRepo {
    let t = repo("");
    t.write("src/a.rs", &unwraps(n));
    t.write("qgate.baseline", &format!("{UNWRAP}\tsrc/a.rs\t-\t{n}\n"));
    t.commit_all("init");
    t
}

#[test]
fn baselined_count_holds_and_new_occurrence_fails_at_its_line() {
    let t = baselined(2);
    let mut src = unwraps(2);
    src = src.replace(
        "    t\n}",
        "    t\n}\n\npub fn g(o: Option<u8>) -> u8 {\n    o.expect(\"set\")\n}",
    );
    t.write("src/a.rs", &src).add(&["src/a.rs"]);
    let f = check_staged(&t);
    let errs = of(&f, UNWRAP, Severity::Error);
    assert_eq!(errs.len(), 1, "{f:?}");
    assert_eq!(errs[0].line, Some(9), "the new call, not an existing one");
    assert_eq!(errs[0].path.as_deref(), Some("src/a.rs"));
    assert!(
        errs[0].fix.as_deref().is_some_and(|f| f.contains("`?`")),
        "names the fix: {:?}",
        errs[0].fix
    );
    assert!(errs[0].message.contains("3 in this file, 2 allowed"));

    t.write("src/a.rs", &unwraps(2).replace("+ 0", "+ 7"))
        .add(&["src/a.rs"]);
    assert!(of(&check_staged(&t), UNWRAP, Severity::Error).is_empty());
}

#[test]
fn parent_count_is_the_allowance_without_an_entry() {
    let t = repo("");
    t.write("src/a.rs", &unwraps(1));
    t.commit_all("init");
    t.write("src/a.rs", &unwraps(1).replace("+ 0", "+ 9"))
        .add(&["src/a.rs"]);
    assert!(of(&check_staged(&t), UNWRAP, Severity::Error).is_empty());
    t.write("src/a.rs", &unwraps(2)).add(&["src/a.rs"]);
    assert_eq!(of(&check_staged(&t), UNWRAP, Severity::Error).len(), 1);
}

#[test]
fn a_raised_entry_still_allows_only_the_parent_count() {
    let t = baselined(1);
    t.write("qgate.baseline", &format!("{UNWRAP}\tsrc/a.rs\t-\t5\n"))
        .write("src/a.rs", &unwraps(3))
        .add(&["src/a.rs", "qgate.baseline"]);
    let errs = of(&check_staged(&t), UNWRAP, Severity::Error);
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs[0].message.contains("1 allowed"), "{}", errs[0].message);
}

#[test]
fn a_lowered_entry_allows_less_than_the_parent() {
    let t = baselined(3);
    t.write("qgate.baseline", &format!("{UNWRAP}\tsrc/a.rs\t-\t1\n"))
        .write("src/a.rs", &unwraps(3).replace("+ 0", "+ 7"))
        .add(&["src/a.rs", "qgate.baseline"]);
    let errs = of(&check_staged(&t), UNWRAP, Severity::Error);
    assert_eq!(errs.len(), 2, "{errs:?}");
}

#[test]
fn an_entry_for_a_file_new_to_the_parent_is_its_allowance() {
    let t = repo("");
    t.commit_all("init");
    t.write("src/a.rs", &unwraps(2))
        .write("qgate.baseline", &format!("{UNWRAP}\tsrc/a.rs\t-\t1\n"))
        .add(&["src/a.rs", "qgate.baseline"]);
    assert_eq!(of(&check_staged(&t), UNWRAP, Severity::Error).len(), 1);
}

#[test]
fn new_file_without_an_entry_allows_none() {
    let t = repo("");
    t.commit_all("init");
    t.write("src/b.rs", &unwraps(1)).add(&["src/b.rs"]);
    let errs = of(&check_staged(&t), UNWRAP, Severity::Error);
    assert_eq!(errs.len(), 1);
    assert!(
        errs[0].message.contains("none allowed"),
        "{}",
        errs[0].message
    );
}

#[test]
fn test_code_is_not_counted() {
    let src = "pub fn f(v: Option<u8>) -> u8 {\n    v.unwrap()\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        Some(1).unwrap();\n    }\n}\n\n#[test]\nfn loose() {\n    Some(2).unwrap();\n}\n";
    assert_eq!(count_lines("src/lib.rs", src, ""), vec![2]);
    assert!(count_lines("tests/a.rs", src, "").is_empty());
    assert!(count_lines("src/foo/tests.rs", src, "").is_empty());
    assert!(count_lines("benches/b.rs", src, "").is_empty());
}

#[test]
fn unwrap_or_is_not_an_unwrap() {
    let src = "fn f(v: Option<u8>) -> u8 {\n    v.unwrap_or(0) + v.unwrap_or_default()\n}\n";
    assert!(count_lines("src/a.rs", src, "").is_empty());
}

#[test]
fn qgate_allow_marker_removes_an_occurrence() {
    let src = "fn f(v: Option<u8>) -> u8 {\n    // qgate: allow escape-hatch/rust-unwrap -- reason: checked above, until: 2026-12-01\n    let a = v.unwrap();\n    let b = v.unwrap(); // qgate: allow escape-hatch -- reason: x, until: 2026-12-01\n    // qgate: allow debug-output -- reason: wrong rule\n    a + b + v.unwrap()\n}\n";
    assert_eq!(count_lines("src/a.rs", src, ""), vec![6]);
}

#[test]
fn a_marker_after_prose_or_in_a_string_is_not_honored() {
    let src = "fn f(v: Option<u8>) -> u8 {\n    let a = v.unwrap(); // fine here qgate: allow escape-hatch\n    // see qgate: allow escape-hatch\n    let b = v.unwrap();\n    let s = \"qgate: allow escape-hatch\"; v.unwrap()\n}\n";
    assert_eq!(count_lines("src/a.rs", src, ""), vec![2, 4, 5]);
}

#[test]
fn a_trailing_marker_does_not_cover_the_line_below() {
    let src = "fn f(v: Option<u8>) -> u8 {\n    let a = v.unwrap(); // qgate: allow escape-hatch -- reason: x, until: 2026-12-01\n    let b = v.unwrap();\n    a + b\n}\n";
    assert_eq!(count_lines("src/a.rs", src, ""), vec![3]);
}

#[test]
fn a_marker_takes_one_occurrence() {
    let src = "fn f(v: Option<Option<u8>>) -> u8 {\n    // qgate: allow escape-hatch -- reason: x, until: 2026-12-01\n    v.unwrap().unwrap() + v.unwrap()\n}\n";
    assert_eq!(count_lines("src/a.rs", src, ""), vec![3, 3]);
}

#[test]
fn off_rule_is_not_counted_and_its_entry_is_prunable() {
    let t = baselined(2);
    t.write(
        "qgate.toml",
        &config(&format!("[rules]\n\"{UNWRAP}\" = \"off\"\n")),
    );
    t.write("src/a.rs", &unwraps(5))
        .add(&["src/a.rs", "qgate.toml"]);
    let f = check_staged(&t);
    assert!(of(&f, UNWRAP, Severity::Error).is_empty(), "{f:?}");
    assert!(f.iter().any(|f| f.rule == "baseline-prunable"
        && f.fix.as_deref() == Some(PRUNE_FIX)
        && f.message.contains("fell from 2 to 0")));
    assert!(
        count_lines(
            "src/a.rs",
            &unwraps(3),
            &format!("[rules]\n\"{UNWRAP}\" = \"off\"\n")
        )
        .is_empty()
    );
}

#[test]
fn warn_rule_rise_is_a_warning() {
    let t = baselined(1);
    t.write(
        "qgate.toml",
        &config("[rules]\n\"escape-hatch\" = \"warn\"\n"),
    );
    t.write("src/a.rs", &unwraps(3))
        .add(&["src/a.rs", "qgate.toml"]);
    let f = check_staged(&t);
    assert!(of(&f, UNWRAP, Severity::Error).is_empty());
    assert_eq!(of(&f, UNWRAP, Severity::Warning).len(), 2, "{f:?}");
}

#[test]
fn exact_id_overrides_its_family() {
    let t = baselined(1);
    let rules = format!("[rules]\n\"escape-hatch\" = \"warn\"\n\"{UNWRAP}\" = \"error\"\n");
    t.write("qgate.toml", &config(&rules));
    t.write("src/a.rs", &unwraps(2))
        .add(&["src/a.rs", "qgate.toml"]);
    assert_eq!(of(&check_staged(&t), UNWRAP, Severity::Error).len(), 1);
}

#[test]
fn renamed_file_reads_its_parent_count_from_the_old_path() {
    let t = baselined(2);
    t.git(&["mv", "src/a.rs", "src/b.rs"]);
    let f = check_staged(&t);
    assert!(of(&f, UNWRAP, Severity::Error).is_empty(), "{f:?}");
    let r = baseline::prune(&t.repo(), false).unwrap();
    assert!(
        r.findings.iter().any(|f| f.rule == "baseline-moved"),
        "{:?}",
        r.findings
    );
    let b = Baseline::parse(&t.read("qgate.baseline")).unwrap();
    assert_eq!(
        b.get(&Key::count(UNWRAP, "src/b.rs")),
        Some(Value::Count(2))
    );
    assert_eq!(b.get(&Key::count(UNWRAP, "src/a.rs")), None);
}

#[test]
fn baseline_create_writes_count_lines() {
    let t = repo("");
    t.write("src/a.rs", &unwraps(3));
    t.write("src/c.rs", "pub fn clean() {}\n");
    t.add(&["src/a.rs", "src/c.rs", "qgate.toml"]);
    baseline::create(&t.repo(), false, false).unwrap();
    let text = t.read("qgate.baseline");
    assert!(
        text.contains(&format!("{UNWRAP}\tsrc/a.rs\t-\t3\n")),
        "{text}"
    );
    assert!(!text.contains("src/c.rs"));
}

#[test]
fn prune_lowers_and_removes_count_entries() {
    let t = baselined(4);
    t.write("src/a.rs", &unwraps(3)).add(&["src/a.rs"]);
    baseline::prune(&t.repo(), false).unwrap();
    let b = Baseline::parse(&t.read("qgate.baseline")).unwrap();
    assert_eq!(
        b.get(&Key::count(UNWRAP, "src/a.rs")),
        Some(Value::Count(3))
    );
    t.write("src/a.rs", &unwraps(0)).add(&["src/a.rs"]);
    baseline::prune(&t.repo(), false).unwrap();
    let b = Baseline::parse(&t.read("qgate.baseline")).unwrap();
    assert!(b.is_empty(), "{b:?}");
}

#[test]
fn count_entry_without_length_entry_is_not_stale() {
    let t = baselined(2);
    t.write("src/a.rs", &unwraps(2).replace("+ 1", "+ 5"))
        .add(&["src/a.rs"]);
    let f = check_staged(&t);
    assert!(!f.iter().any(|f| f.rule == "baseline-prunable"), "{f:?}");
}

#[test]
fn merge_driver_keeps_the_lower_count() {
    let parse = |n: u32| Baseline::parse(&format!("{UNWRAP}\ta.rs\t-\t{n}\n")).unwrap();
    let merged = merge_driver::merge(&parse(4), &parse(3), &parse(5));
    assert_eq!(
        merged.get(&Key::count(UNWRAP, "a.rs")),
        Some(Value::Count(3))
    );
}

#[test]
fn count_lines_parse_and_describe() {
    let b = Baseline::parse(&format!("{UNWRAP}\tsrc/a.rs\t-\t4\n")).unwrap();
    let key = Key::count(UNWRAP, "src/a.rs");
    assert_eq!(b.get(&key), Some(Value::Count(4)));
    assert_eq!(key.describe(), format!("`{UNWRAP}` count in src/a.rs"));
    let err = Baseline::parse("file-length\ta\t-\t1\nescape-hatch/nope\ta.rs\t-\t1\n").unwrap_err();
    assert!(err.to_string().contains(":2:"), "{err}");
}
