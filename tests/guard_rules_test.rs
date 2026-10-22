//! Loosenings of the rules engine's settings: rule levels, always-block
//! lists and baseline growth for rule counts (KTD15, KTD16).

mod support;

use qgate::baseline;
use qgate::cli::Severity;
use qgate::config::VERSION;
use qgate::guard;
use support::guard::{assert_pending, pending, staged, warned};
use support::repo::TestRepo;

const RUST_SRC: &str = "pub fn g() -> Result<(), ()> {\n    Ok(())\n}\n\npub fn f(x: Option<u8>) -> u8 {\n    g().ok();\n    x.unwrap()\n}\n";

/// A committed Rust repo with one swallowed error and one unwrap in src/a.rs.
fn rs_setup(config_body: &str, baseline: &str) -> TestRepo {
    let t = TestRepo::new();
    t.write("qgate.toml", &rs_config(VERSION, config_body))
        .write("src/a.rs", RUST_SRC);
    if !baseline.is_empty() {
        t.write("qgate.baseline", baseline);
    }
    t.commit_all("init");
    t
}

fn rs_config(version: &str, body: &str) -> String {
    format!("qgate_version = \"{version}\"\n{body}")
}

fn set_config(t: &TestRepo, body: &str) {
    t.write("qgate.toml", &rs_config(VERSION, body))
        .add(&["qgate.toml"]);
}

fn set_baseline(t: &TestRepo, text: &str) {
    t.write("qgate.baseline", text).add(&["qgate.baseline"]);
}

const OK_STATEMENT: &str = "swallowed-error/rust-ok-statement";
const UNWRAP: &str = "escape-hatch/rust-unwrap";

fn entry(rule: &str, path: &str, n: u32) -> String {
    format!("{rule}\t{path}\t-\t{n}\n")
}

/// Blocking loosening errors for the staged change.
fn blocked(t: &TestRepo) -> Vec<String> {
    guard::check(&staged(t))
        .into_iter()
        .filter(|f| f.rule == guard::RULE && f.severity == Severity::Error)
        .map(|f| f.message)
        .collect()
}

#[test]
fn lowering_an_always_block_rule_blocks() {
    let t = rs_setup("", "");
    set_config(&t, &format!("[rules]\n\"{OK_STATEMENT}\" = \"warn\"\n"));
    let b = blocked(&t);
    assert_eq!(b.len(), 1, "{b:?}");
    assert!(
        b[0].starts_with(&format!("config: rule {OK_STATEMENT} error -> warn")),
        "{b:?}"
    );
    let f = guard::check(&staged(&t));
    assert!(
        f[0].fix
            .as_deref()
            .is_some_and(|x| x.contains("git push --no-verify")),
        "{f:?}"
    );
    set_config(&t, "[rules]\n\"debug-output\" = \"warn\"\n");
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
    assert!(warned(&t));
}

#[test]
fn lowered_levels_are_loosening_lines() {
    let t = rs_setup("[rules]\n\"debug-output\" = \"error\"\n", "");
    set_config(&t, "[rules]\n\"debug-output\" = \"warn\"\n");
    assert_eq!(pending(&t), ["config: rule debug-output error -> warn"]);
    t.commit_all("warn");
    set_config(&t, "[rules]\n\"debug-output\" = \"error\"\n");
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
    set_config(
        &t,
        "[rules.debug-output]\nlevel = \"warn\"\nallow = [\"src/cli/**\"]\n",
    );
    assert_eq!(
        pending(&t),
        ["config: rules.debug-output allow adds `src/cli/**`"]
    );
    set_config(&t, "[rules]\n\"debug-output/rust-dbg\" = \"off\"\n");
    assert_eq!(
        pending(&t),
        ["config: rule debug-output/rust-dbg warn -> off"]
    );
}

#[test]
fn shrinking_always_block_blocks_and_growing_it_does_not() {
    let t = rs_setup("", "");
    set_config(
        &t,
        "[suppressions]\nalways_block = [\"@ts-nocheck\", \"@ts-ignore\", \"eslint-disable\"]\n",
    );
    let b = blocked(&t);
    assert_eq!(b.len(), 1, "{b:?}");
    assert!(
        b[0].starts_with("config: suppressions.always_block drops `swallowed-error`"),
        "{b:?}"
    );
    set_config(
        &t,
        "[suppressions]\nalways_block = [\"swallowed-error\", \"@ts-nocheck\", \"@ts-ignore\", \"eslint-disable\", \"escape-hatch\"]\n",
    );
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
}

#[test]
fn raising_max_days_only_warns() {
    let t = rs_setup("", "");
    set_config(&t, "[suppressions]\nmax_days = 365\n");
    assert_eq!(pending(&t), ["config: suppressions.max_days 90 -> 365"]);
    assert!(blocked(&t).is_empty());
}

#[test]
fn raising_an_always_block_count_entry_blocks() {
    let base = entry(OK_STATEMENT, "src/a.rs", 1) + &entry(UNWRAP, "src/a.rs", 1);
    let t = rs_setup("", &base);
    set_baseline(
        &t,
        &(entry(OK_STATEMENT, "src/a.rs", 2) + &entry(UNWRAP, "src/a.rs", 1)),
    );
    let b = blocked(&t);
    assert_eq!(b.len(), 1, "{b:?}");
    assert!(
        b[0].starts_with(&format!(
            "baseline: raised `{OK_STATEMENT}` count in src/a.rs 1 -> 2"
        )),
        "{b:?}"
    );
    set_baseline(
        &t,
        &(entry(OK_STATEMENT, "src/a.rs", 1) + &entry(UNWRAP, "src/a.rs", 2)),
    );
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
    assert_pending(
        &t,
        &[&format!(
            "baseline: raised `{UNWRAP}` count in src/a.rs 1 -> 2"
        )],
    );
}

#[test]
fn adopting_qgate_records_the_day_one_baseline_without_blocking() {
    let t = TestRepo::new();
    t.write("src/a.rs", RUST_SRC);
    t.commit_all("existing code");
    t.write("qgate.toml", &rs_config(VERSION, ""))
        .add(&["qgate.toml"]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.add(&["qgate.baseline"]);
    assert!(t.read("qgate.baseline").contains(OK_STATEMENT));
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
    let info: Vec<_> = guard::check(&staged(&t))
        .into_iter()
        .filter(|f| f.severity == Severity::Info)
        .collect();
    assert_eq!(info.len(), 1, "{info:?}");
    assert!(
        info[0]
            .message
            .starts_with("baseline: adopting qgate with 2 entries"),
        "{info:?}"
    );
    assert_eq!(pending(&t), ["config: new qgate.toml"]);
    // Counts above what the parent has still block.
    set_baseline(
        &t,
        &(entry(OK_STATEMENT, "src/a.rs", 3) + &entry(UNWRAP, "src/a.rs", 1)),
    );
    assert_eq!(blocked(&t).len(), 1, "{:?}", blocked(&t));
}

#[test]
fn re_adding_a_deleted_baseline_with_more_blocks() {
    let base = entry(OK_STATEMENT, "src/a.rs", 1);
    let t = rs_setup("", &base);
    t.remove("qgate.baseline");
    t.commit_all("drop baseline");
    set_baseline(&t, &entry(OK_STATEMENT, "src/a.rs", 2));
    assert_eq!(blocked(&t).len(), 1, "{:?}", blocked(&t));
    set_baseline(&t, &base);
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
}

#[test]
fn upgrading_qgate_groups_new_rule_entries() {
    let long: String = (0..60).map(|i| format!("    let v{i} = {i};\n")).collect();
    let length = "function-length\tsrc/long.rs\tbig#0\t62\n";
    let t = TestRepo::new();
    t.write("qgate.toml", &rs_config("0.4.1", ""))
        .write("src/a.rs", RUST_SRC)
        .write("src/long.rs", &format!("fn big() {{\n{long}}}\n"))
        .write("qgate.baseline", length);
    t.commit_all("v0.4.1");
    set_config(&t, "");
    let upgraded = entry(OK_STATEMENT, "src/a.rs", 1) + &entry(UNWRAP, "src/a.rs", 1) + length;
    set_baseline(&t, &upgraded);
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
    let upgrade = format!("baseline: upgrade to {VERSION} adds 2 entries for 2 new rules");
    assert_eq!(
        pending(&t),
        [
            upgrade.clone(),
            format!("config: qgate_version 0.4.1 -> {VERSION}")
        ]
    );

    set_baseline(&t, &upgraded.replace("\t62\n", "\t70\n"));
    let lines = pending(&t);
    assert!(lines.contains(&upgrade), "{lines:?}");
    assert!(
        lines.contains(&"baseline: raised big#0 in src/long.rs 62 -> 70".to_string()),
        "{lines:?}"
    );

    // A new swallowed error rides along with the upgrade: still blocked.
    t.write(
        "src/a.rs",
        &RUST_SRC.replace("g().ok();", "g().ok();\n    g().ok();"),
    )
    .add(&["src/a.rs"]);
    set_baseline(
        &t,
        &upgraded.replace(
            &entry(OK_STATEMENT, "src/a.rs", 1),
            &entry(OK_STATEMENT, "src/a.rs", 2),
        ),
    );
    assert_eq!(blocked(&t).len(), 1, "{:?}", blocked(&t));
}

#[test]
fn excluding_always_block_occurrences_blocks() {
    let t = rs_setup("", "");
    t.write(
        "src/legacy/old.rs",
        "pub fn h() {\n    crate::a::g().ok();\n}\n",
    )
    .write("src/other/x.rs", "pub fn x() {}\n");
    t.commit_all("more");
    set_config(&t, "exclude = [\"src/legacy/**\"]\n");
    let b = blocked(&t);
    assert_eq!(b.len(), 1, "{b:?}");
    assert!(
        b[0].starts_with("exclusion: newly excluded src/legacy/old.rs holds always-block"),
        "{b:?}"
    );
    set_config(&t, "exclude = [\"src/other/**\"]\n");
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
    assert_eq!(pending(&t), ["config: exclude adds `src/other/**`"]);
}

#[test]
fn excluding_a_file_with_new_always_block_occurrences_blocks() {
    let t = rs_setup("", "");
    t.write("src/b.rs", "pub fn h() {\n    crate::a::g().ok();\n}\n")
        .add(&["src/b.rs"]);
    set_config(&t, "exclude = [\"src/b.rs\"]\n");
    let b = blocked(&t);
    assert!(
        b.iter().any(|l| l.contains("newly excluded src/b.rs")),
        "{b:?}"
    );
}

#[test]
fn marking_a_file_generated_or_vendored_blocks_like_an_exclude() {
    let t = rs_setup("", &entry(OK_STATEMENT, "src/a.rs", 1));
    t.write(".gitattributes", "src/a.rs linguist-generated\n")
        .add(&[".gitattributes"]);
    let b = blocked(&t);
    assert!(
        b.iter().any(|l| l.contains("newly excluded src/a.rs")),
        "{b:?}"
    );
    t.write(".gitattributes", "src/legacy/** linguist-vendored\n")
        .add(&[".gitattributes"]);
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
}

#[test]
fn a_go_generated_header_over_swallowed_errors_blocks() {
    let t = rs_setup("", "");
    let body = "package pkg\n\nfunc F() error {\n\tif err := g(); err != nil {\n\t\treturn nil\n\t}\n\treturn nil\n}\n";
    t.write("pkg/a.go", body);
    t.commit_all("go");
    t.write(
        "pkg/a.go",
        &format!("// Code generated by hand. DO NOT EDIT.\n\n{body}"),
    )
    .add(&["pkg/a.go"]);
    let b = blocked(&t);
    assert!(
        b.iter().any(|l| l.contains("newly excluded pkg/a.go")),
        "{b:?}"
    );
}

#[test]
fn a_count_entry_that_follows_a_rename_is_moved() {
    let t = rs_setup("", &entry(OK_STATEMENT, "src/a.rs", 1));
    t.git(&["mv", "src/a.rs", "src/b.rs"]);
    set_baseline(&t, &entry(OK_STATEMENT, "src/b.rs", 1));
    assert!(blocked(&t).is_empty(), "{:?}", blocked(&t));
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
}

#[test]
fn blocking_lines_are_never_collapsed() {
    let t = rs_setup("", "");
    let mut text = String::new();
    for i in 0..7 {
        t.write(
            &format!("src/n{i}.rs"),
            "pub fn h() {\n    crate::a::g().ok();\n}\n",
        );
        text += &entry(OK_STATEMENT, &format!("src/n{i}.rs"), 1);
    }
    t.add(&["src"]);
    set_baseline(&t, &text);
    assert_eq!(blocked(&t).len(), 7, "{:?}", blocked(&t));
}

#[test]
fn a_blocking_line_committed_without_the_hook_fails_at_push() {
    let t = rs_setup("", "");
    let base = t.git(&["rev-parse", "HEAD"]);
    set_config(&t, &format!("[rules]\n\"{OK_STATEMENT}\" = \"off\"\n"));
    t.commit_all("skip the hook");
    let out = t
        .qgate()
        .args(["run", "--stage", "push", "--base", &base])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}");
    assert!(
        stdout.contains(&format!("rule {OK_STATEMENT} error -> off")),
        "{stdout}"
    );
}

#[test]
fn heuristic_rules_of_an_always_block_family_do_not_block() {
    use qgate::rules::{always_blocked, find};
    let exact = find("swallowed-error/rust-ok-statement").unwrap();
    let heuristic = find("swallowed-error/let-underscore").unwrap();
    assert!(always_blocked("swallowed-error", exact));
    assert!(!always_blocked("swallowed-error", heuristic));
    assert!(always_blocked("swallowed-error/let-underscore", heuristic));
}
