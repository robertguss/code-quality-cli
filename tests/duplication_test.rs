mod support;

use std::collections::HashSet;

use qgate::checks::duplication::{Detection, detect};
use qgate::checks::exclude::Excludes;
use qgate::config::Config;
use qgate::git::Snapshot;
use support::repo::TestRepo;

const HELPER: &str = "def parse_record(line, sep, fields):
    parts = line.strip().split(sep)
    if len(parts) != len(fields):
        raise ValueError('expected %d fields, got %d' % (len(fields), len(parts)))
    record = {}
    for name, raw in zip(fields, parts):
        value = raw.strip()
        if value.isdigit():
            value = int(value)
        record[name] = value
    return record
";

fn run(t: &TestRepo, focus: Option<&[&str]>, min_tokens: u32) -> Detection {
    let repo = t.repo();
    let snap = Snapshot::index(&repo).unwrap();
    let ex = Excludes::new(&repo, &snap, &Config::starter()).unwrap();
    let focus: Option<HashSet<String>> = focus.map(|f| f.iter().map(|s| s.to_string()).collect());
    detect(&repo, &snap, &ex, min_tokens, focus.as_ref()).unwrap()
}

fn copied(name: &str) -> String {
    HELPER.replace("parse_record", name)
}

#[test]
fn copy_of_untouched_helper_names_both_locations() {
    // AE4
    let t = TestRepo::new();
    t.write("util.py", &format!("import os\n\n{HELPER}"));
    t.commit_all("helper");
    t.write(
        "feature.py",
        &format!("# new feature\n\n{}", copied("load_row")),
    )
    .add(&["feature.py"]);
    let d = run(&t, Some(&["feature.py"]), 50);
    assert_eq!(d.groups.len(), 1, "{:?}", d.groups);
    let g = &d.groups[0];
    let paths: Vec<_> = g.members.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["feature.py", "util.py"]);
    assert!(g.tokens >= 50);
    assert_eq!(g.members[1].start_line, 3);
    assert!(
        g.others("feature.py", g.members[0].start_line)
            .starts_with("util.py:3-")
    );
}

#[test]
fn whitespace_and_comments_do_not_hide_a_copy() {
    let t = TestRepo::new();
    t.write("a.py", HELPER);
    let noisy = HELPER
        .replace(
            "    record = {}\n",
            "    # build it\n    record   =   {}\n\n",
        )
        .replace("parse_record", "other");
    t.write("b.py", &noisy).add(&["."]);
    let d = run(&t, Some(&["b.py"]), 50);
    assert_eq!(d.groups.len(), 1, "{:?}", d.groups);
}

#[test]
fn generic_lexer_files_are_checked_too() {
    let t = TestRepo::new();
    let kt = "fun total(items: List<Item>): Int {\n    var sum = 0\n    for (item in items) {\n        if (item.active && item.price > 0) {\n            sum += item.price * item.quantity\n        }\n    }\n    return sum + tax(sum) - discount(sum, items.size)\n}\n";
    t.write("a.kt", kt)
        .write("b.kt", &format!("// copy\n{kt}"))
        .add(&["."]);
    assert_eq!(run(&t, None, 40).groups.len(), 1);
}

#[test]
fn blocks_below_minimum_are_ignored() {
    let t = TestRepo::new();
    t.write("a.py", "def f(x):\n    return x * 2 + 1\n");
    t.write("b.py", "def g(x):\n    return x * 2 + 1\n")
        .add(&["."]);
    assert!(run(&t, None, 50).groups.is_empty());
}

#[test]
fn moving_a_member_keeps_key_and_size() {
    let t = TestRepo::new();
    t.write("a.py", HELPER).write("b.py", &copied("two"));
    t.commit_all("two copies");
    let before = run(&t, None, 50);
    std::fs::create_dir_all(t.path().join("moved")).unwrap();
    t.git(&["mv", "b.py", "moved/c.py"]);
    let after = run(&t, Some(&["moved/c.py"]), 50);
    assert_eq!(before.groups.len(), 1);
    assert_eq!(after.groups.len(), 1);
    assert_eq!(before.groups[0].key, after.groups[0].key);
    assert_eq!(
        before.groups[0].members.len(),
        after.groups[0].members.len()
    );
}

#[test]
fn three_copies_form_one_group() {
    let t = TestRepo::new();
    t.write("a.py", HELPER)
        .write("b.py", &copied("b"))
        .write("c.py", &copied("c"))
        .add(&["."]);
    let d = run(&t, Some(&["c.py"]), 50);
    assert_eq!(d.groups.len(), 1);
    assert_eq!(d.groups[0].members.len(), 3);
}

#[test]
fn second_run_rehashes_nothing() {
    let t = TestRepo::new();
    t.write("a.py", HELPER).write("b.py", &copied("b"));
    t.commit_all("init");
    assert_eq!(run(&t, None, 50).tokenized, 2);
    assert_eq!(run(&t, None, 50).tokenized, 0);
    t.write("c.py", "x = 1\n").add(&["c.py"]);
    assert_eq!(run(&t, None, 50).tokenized, 1);
}

#[test]
fn corrupt_cache_is_rebuilt() {
    let t = TestRepo::new();
    t.write("a.py", HELPER)
        .write("b.py", &copied("b"))
        .add(&["."]);
    run(&t, None, 50);
    let cache = t.path().join(".git/qgate/dup-tokens.bin");
    std::fs::write(&cache, b"garbage").unwrap();
    let d = run(&t, None, 50);
    assert_eq!(d.tokenized, 2);
    assert_eq!(d.groups.len(), 1);
    std::fs::remove_file(&cache).unwrap();
    assert_eq!(run(&t, None, 50).groups.len(), 1);
}

/// Benchmark: `cargo test --release --test duplication_test -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_large_repo() {
    let t = TestRepo::new();
    for i in 0..3000 {
        let mut src = String::new();
        for j in 0..40 {
            src.push_str(&format!(
                "def f_{i}_{j}(a, b):\n    total_{j} = a * {i} + b - {j}\n    return helper_{i}(total_{j}, {j})\n\n"
            ));
        }
        t.write(&format!("pkg{}/mod{i}.py", i % 30), &src);
    }
    t.commit_all("big");
    t.write("pkg0/new.py", &format!("{HELPER}\n{}", copied("again")))
        .add(&["pkg0/new.py"]);
    let start = std::time::Instant::now();
    let cold = run(&t, Some(&["pkg0/new.py"]), 50);
    let cold_t = start.elapsed();
    let start = std::time::Instant::now();
    let warm = run(&t, Some(&["pkg0/new.py"]), 50);
    let warm_t = start.elapsed();
    println!(
        "cold {cold_t:?} (tokenized {}), warm {warm_t:?} (tokenized {})",
        cold.tokenized, warm.tokenized
    );
    assert_eq!(warm.groups.len(), 1);
    assert!(warm_t.as_secs_f64() < 5.0);
}

#[test]
fn elixir_copy_matches_after_a_rename() {
    let body = "  def parse_record(line, sep, fields) do
    parts = line |> String.trim() |> String.split(sep)

    if length(parts) != length(fields) do
      raise ArgumentError, \"expected #{length(fields)} fields, got #{length(parts)}\"
    end

    fields
    |> Enum.zip(parts)
    |> Map.new(fn {name, raw} -> {name, String.trim(raw)} end)
  end
";
    let t = TestRepo::new();
    t.write("lib/util.ex", &format!("defmodule Util do\n{body}end\n"));
    t.commit_all("helper");
    let copy = body.replace("parse_record", "load_row");
    t.write(
        "lib/feature.ex",
        &format!("defmodule Feature do\n  # new feature\n{copy}end\n"),
    )
    .add(&["lib/feature.ex"]);
    let d = run(&t, Some(&["lib/feature.ex"]), 50);
    assert_eq!(d.groups.len(), 1, "{:?}", d.groups);
    let paths: Vec<_> = d.groups[0]
        .members
        .iter()
        .map(|m| m.path.as_str())
        .collect();
    assert_eq!(paths, ["lib/feature.ex", "lib/util.ex"]);
}

#[test]
fn generated_go_files_are_not_copies() {
    let func = "func Parse(line string, sep string) (map[string]string, error) {
	parts := strings.Split(strings.TrimSpace(line), sep)
	if len(parts) != 3 {
		return nil, fmt.Errorf(\"expected 3 fields, got %d\", len(parts))
	}
	record := map[string]string{}
	for i, raw := range parts {
		record[fmt.Sprint(i)] = strings.TrimSpace(raw)
	}
	return record, nil
}
";
    let t = TestRepo::new();
    t.write("a.go", &format!("package a\n\n{func}"))
        .write(
            "b_templ.go",
            &format!("// Code generated by templ - DO NOT EDIT.\n\npackage a\n\n{func}"),
        )
        .add(&["."]);
    assert!(run(&t, None, 30).groups.is_empty());
    t.write("c.go", &format!("package a\n\n{func}"))
        .add(&["c.go"]);
    assert_eq!(run(&t, None, 30).groups.len(), 1);
}
