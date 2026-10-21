mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use qgate::cli::output::{Finding, Severity};
use qgate::config::{Config, Stage, VERSION};
use qgate::git::Source;
use qgate::tools::{ToolContext, ToolResults, plan, run_tools};
use support::repo::TestRepo;

fn config(tools: &str) -> Config {
    Config::parse(
        &format!("qgate_version = \"{VERSION}\"\n{tools}"),
        "qgate.toml",
    )
    .unwrap()
}

fn run(t: &TestRepo, cfg: &Config, stage: Stage, source: Source, changed: &[&str]) -> ToolResults {
    let repo = t.repo();
    let ctx = ToolContext {
        repo: &repo,
        config: cfg,
        stage,
        source,
        changed: changed.iter().map(|s| s.to_string()).collect(),
    };
    run_tools(&ctx).unwrap()
}

fn commit(t: &TestRepo, cfg: &Config, changed: &[&str]) -> ToolResults {
    run(t, cfg, Stage::Commit, Source::Index, changed)
}

fn rules(r: &ToolResults) -> Vec<&str> {
    r.findings.iter().map(|f| f.rule.as_str()).collect()
}

fn only(r: &ToolResults) -> &Finding {
    assert_eq!(r.findings.len(), 1, "{:#?}", r.findings);
    &r.findings[0]
}

/// A repo with one commit, so later staging is a real change.
fn repo_with(files: &[(&str, &str)]) -> TestRepo {
    let t = TestRepo::new();
    t.write("README", "hi\n");
    for (p, c) in files {
        t.write(p, c);
    }
    t.commit_all("init");
    t
}

#[test]
fn glob_miss_is_skipped_at_commit() {
    let t = repo_with(&[]);
    t.write("a.py", "x = 1\n").add(&["a.py"]);
    let cfg = config("[[tool]]\nname = \"tsc\"\ncommand = \"exit 1\"\nglobs = [\"**/*.ts\"]\n");
    let r = commit(&t, &cfg, &["a.py"]);
    assert!(r.findings.is_empty() && r.timings.is_empty(), "{r:?}");
}

#[test]
fn push_stage_tool_runs_only_at_push() {
    let t = repo_with(&[]);
    t.write("a.py", "x = 1\n");
    t.commit_all("add");
    let cfg = config("[[tool]]\nname = \"tests\"\ncommand = \"exit 1\"\nstage = \"push\"\n");
    assert!(commit(&t, &cfg, &["a.py"]).timings.is_empty());
    let tip = t.git(&["rev-parse", "HEAD^{tree}"]);
    let r = run(&t, &cfg, Stage::Push, Source::Tree(tip), &[]);
    assert_eq!(rules(&r), ["tests"]);
    assert_eq!(r.timings[0].0, "tool:tests");
}

#[test]
fn files_placeholder_gets_only_matching_changed_files() {
    let t = repo_with(&[]);
    t.write("a.py", "1\n")
        .write("src/b.py", "2\n")
        .write("c.ts", "3\n");
    t.write("it's.py", "4\n");
    t.git(&["add", "-A"]);
    let cfg = config(
        "[[tool]]\nname = \"lint\"\nglobs = [\"**/*.py\"]\n\
         command = \"for f in {files}; do echo \\\"$f:1: error: seen\\\"; done; exit 1\"\n",
    );
    let r = commit(&t, &cfg, &["a.py", "src/b.py", "c.ts", "it's.py"]);
    let mut paths: Vec<_> = r.findings.iter().map(|f| f.path.clone().unwrap()).collect();
    paths.sort();
    assert_eq!(paths, ["a.py", "it's.py", "src/b.py"]);
    assert!(
        r.findings
            .iter()
            .all(|f| f.rule == "lint" && f.severity == Severity::Error)
    );
    assert!(
        r.findings[0]
            .fix
            .as_deref()
            .unwrap()
            .contains("'it'\\''s.py'")
    );
}

#[test]
fn whole_project_tool_runs_once_in_clean_working_tree() {
    let t = repo_with(&[]);
    t.write("a.py", "1\n").write("b.py", "2\n");
    t.git(&["add", "-A"]);
    let out = tempfile::tempdir().unwrap();
    let log = out.path().join("calls");
    let cfg = config(&format!(
        "[[tool]]\nname = \"check\"\ncommand = \"pwd >> {}\"\n",
        log.display()
    ));
    let r = commit(&t, &cfg, &["a.py", "b.py"]);
    assert!(r.findings.is_empty(), "{r:?}");
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.lines().count(), 1);
    assert_eq!(
        Path::new(calls.trim()),
        t.repo().root,
        "nothing dirty, so working tree"
    );
}

#[test]
fn missing_tool_is_tool_missing_with_command() {
    let t = repo_with(&[]);
    t.write("a.py", "1\n").add(&["a.py"]);
    let cfg = config("[[tool]]\nname = \"ruff\"\ncommand = \"no-such-tool-qgate check .\"\n");
    let r = commit(&t, &cfg, &["a.py"]);
    let f = only(&r);
    assert_eq!(f.rule, "tool-missing");
    assert!(
        f.message.contains("`no-such-tool-qgate check .`"),
        "{}",
        f.message
    );
    assert_eq!(
        f.fix.as_deref(),
        Some(
            "install no-such-tool-qgate, or fix tool.ruff.command in qgate.toml (needs qgate approve)"
        )
    );
}

fn alive(pid: i32) -> bool {
    // A zombie waiting for a reaper counts as gone.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z'),
        Err(_) => unsafe { libc::kill(pid, 0) == 0 },
    }
}

#[test]
fn hanging_tool_is_killed_with_its_children() {
    let t = repo_with(&[]);
    t.write("a.py", "1\n").add(&["a.py"]);
    let out = tempfile::tempdir().unwrap();
    let pidfile = out.path().join("pid");
    let cfg = config(&format!(
        "[[tool]]\nname = \"hang\"\ntimeout_secs = 1\n\
         command = \"sleep 100 & echo $! > {}; wait\"\n",
        pidfile.display()
    ));
    let start = Instant::now();
    let r = commit(&t, &cfg, &["a.py"]);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    let f = only(&r);
    assert_eq!(f.rule, "tool-timeout");
    assert!(f.fix.as_deref().unwrap().contains("timeout_secs"));
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid), "child sleep {pid} survived");
}

const SARIF: &str = r#"{"version":"2.1.0","runs":[{"results":[
{"ruleId":"no-eval","level":"error","message":{"text":"eval is bad"},
 "locations":[{"physicalLocation":{"artifactLocation":{"uri":"a.js"},"region":{"startLine":2}}}]},
{"ruleId":"style","level":"warning","message":{"text":"meh"},
 "locations":[{"physicalLocation":{"artifactLocation":{"uri":"a.js"},"region":{"startLine":5}}}]}]}]}"#;

#[test]
fn sarif_output_becomes_per_line_findings() {
    let t = repo_with(&[("out.sarif", SARIF)]);
    t.write("a.js", "eval(x)\n").add(&["a.js"]);
    let cfg = config("[[tool]]\nname = \"semgrep\"\ncommand = \"cat out.sarif; exit 1\"\n");
    let r = commit(&t, &cfg, &["a.js"]);
    assert_eq!(rules(&r), ["semgrep/no-eval", "semgrep/style"]);
    let f = &r.findings[0];
    assert_eq!((f.path.as_deref(), f.line), (Some("a.js"), Some(2)));
    assert_eq!(f.severity, Severity::Error);
    assert_eq!(f.fix.as_deref(), Some("cat out.sarif; exit 1"));
    assert_eq!(r.findings[1].severity, Severity::Warning);

    // The same output from a passing tool only warns.
    let cfg = config("[[tool]]\nname = \"semgrep\"\ncommand = \"cat out.sarif\"\n");
    let r = commit(&t, &cfg, &["a.js"]);
    assert!(r.findings.iter().all(|f| f.severity == Severity::Warning));
}

#[test]
fn eslint_json_output_becomes_per_line_findings_with_relative_paths() {
    let t = repo_with(&[]);
    // ESLint prints absolute paths; the tool runs in a checkout here because
    // an untracked file matches its globs.
    t.write(
        "lint.sh",
        "printf '[{\"filePath\":\"%s/src/a.js\",\"messages\":[{\"ruleId\":\"semi\",\"severity\":2,\
         \"message\":\"Missing semicolon.\",\"line\":3}]}]' \"$PWD\"\nexit 1\n",
    );
    t.write("src/a.js", "x\n");
    t.git(&["add", "-A"]);
    t.write("scratch.js", "untracked\n");
    let cfg =
        config("[[tool]]\nname = \"eslint\"\ncommand = \"sh lint.sh\"\nformat = \"eslint-json\"\n");
    let r = commit(&t, &cfg, &["src/a.js"]);
    let f = only(&r);
    assert_eq!(f.rule, "eslint/semi");
    assert_eq!((f.path.as_deref(), f.line), (Some("src/a.js"), Some(3)));
    assert_eq!(f.message, "Missing semicolon.");
}

#[test]
fn silent_failure_says_to_print_what_failed() {
    let t = repo_with(&[]);
    t.write("a.go", "package a\n").add(&["a.go"]);
    let cfg = config("[[tool]]\nname = \"gofmt\"\ncommand = \"test -z x\"\n");
    let f = only(&commit(&t, &cfg, &["a.go"])).clone();
    assert!(
        f.message
            .starts_with("`gofmt` failed (exit 1) and printed nothing")
    );
    assert!(f.message.contains("print what failed"), "{}", f.message);
}

#[test]
fn unrecognized_failure_is_one_truncated_finding_with_log() {
    let t = repo_with(&[]);
    t.write("a.py", "1\n").add(&["a.py"]);
    let cfg = config("[[tool]]\nname = \"pytest\"\ncommand = \"seq 1 100; exit 3\"\n");
    let r = commit(&t, &cfg, &["a.py"]);
    let f = only(&r);
    assert_eq!(f.rule, "pytest");
    assert!(f.message.starts_with("`pytest` failed (exit 3):"));
    assert!(f.message.contains("lines omitted"));
    assert!(f.message.lines().count() <= 45, "{}", f.message);
    assert!(
        f.message
            .ends_with("full output: .git/qgate/logs/pytest.log"),
        "{}",
        f.message
    );
    assert_eq!(f.fix.as_deref(), Some("seq 1 100; exit 3"));
    let log = t.read(".git/qgate/logs/pytest.log");
    assert!(log.contains("\n1\n") && log.contains("\n50\n") && log.contains("\n100\n"));
}

#[test]
fn dependency_warnings_never_hide_a_real_failure() {
    let t = repo_with(&[]);
    t.write(
        "check.sh",
        "echo 'deps/yamerl/src/yamerl_constr.erl:801: Warning: '\\''catch ...'\\'' is deprecated'\n\
         echo '  1) test adds (Elara.MathTest)'\n\
         echo '     test/elara/math_test.exs:12'\n\
         exit 2\n",
    );
    t.write("lib/a.ex", "1\n").add(&["check.sh", "lib/a.ex"]);
    // As gcc-style lines only the dependency warning parses; the failure
    // itself must still show.
    let cfg = config("[[tool]]\nname = \"check\"\ncommand = \"sh check.sh\"\nformat = \"gcc\"\n");
    let r = commit(&t, &cfg, &["lib/a.ex"]);
    assert_eq!(rules(&r), ["check", "check"], "{:#?}", r.findings);
    let failure = &r.findings[0];
    assert_eq!(failure.severity, Severity::Error);
    assert!(failure.message.starts_with("`check` failed (exit 2):"));
    assert!(failure.message.contains("test/elara/math_test.exs:12"));
    let dep = &r.findings[1];
    assert_eq!(dep.severity, Severity::Warning);
    assert_eq!(
        dep.path.as_deref(),
        Some("deps/yamerl/src/yamerl_constr.erl")
    );

    // Auto recognises the ExUnit failure and reports it at the test.
    let cfg = config("[[tool]]\nname = \"check\"\ncommand = \"sh check.sh\"\n");
    let r = commit(&t, &cfg, &["lib/a.ex"]);
    let f = only(&r);
    assert_eq!(
        (f.rule.as_str(), f.severity),
        ("check/test", Severity::Error)
    );
    assert_eq!(
        (f.path.as_deref(), f.line),
        (Some("test/elara/math_test.exs"), Some(12))
    );
}

#[test]
fn parsed_errors_in_own_code_replace_the_generic_failure() {
    let t = repo_with(&[]);
    t.write(
        "check.sh",
        "echo 'deps/x/a.c:1: error: in a dependency'\necho 'src/a.c:3: error: mine'\nexit 1\n",
    );
    t.write("src/a.c", "1\n").add(&["check.sh", "src/a.c"]);
    let cfg = config("[[tool]]\nname = \"cc\"\ncommand = \"sh check.sh\"\n");
    let r = commit(&t, &cfg, &["src/a.c"]);
    let sev: Vec<_> = r
        .findings
        .iter()
        .map(|f| (f.path.as_deref(), f.severity))
        .collect();
    assert_eq!(
        sev,
        [
            (Some("deps/x/a.c"), Severity::Warning),
            (Some("src/a.c"), Severity::Error)
        ]
    );
}

#[test]
fn unstaged_edits_force_a_checkout_of_staged_content() {
    let t = repo_with(&[("a.py", "v1\n")]);
    t.write("a.py", "staged\n").add(&["a.py"]);
    t.write("a.py", "unstaged\n");
    let cfg = config(
        "[[tool]]\nname = \"check\"\nglobs = [\"*.py\"]\n\
         command = \"grep -q '^staged$' {files} || { echo 'a.py:1: error: saw unstaged'; exit 1; }\"\n",
    );
    let repo = t.repo();
    let ctx = ToolContext {
        repo: &repo,
        config: &cfg,
        stage: Stage::Commit,
        source: Source::Index,
        changed: vec!["a.py".into()],
    };
    assert!(plan(&ctx).unwrap()[0].in_checkout);
    let r = commit(&t, &cfg, &["a.py"]);
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
    assert_eq!(t.read("a.py"), "unstaged\n", "working tree untouched");
}

#[test]
fn untracked_module_forces_checkout_so_the_import_fails() {
    let t = repo_with(&[]);
    t.write("main.py", "import module\n").add(&["main.py"]);
    t.write("module.py", "X = 1\n");
    let cfg = config(
        "[[tool]]\nname = \"mypy\"\nglobs = [\"**/*.py\"]\n\
         command = \"test -f module.py || { echo 'main.py:1: error: Cannot find module  [import]'; exit 1; }\"\n",
    );
    let r = commit(&t, &cfg, &["main.py"]);
    let f = only(&r);
    assert_eq!(f.rule, "mypy/import");
    assert_eq!((f.path.as_deref(), f.line), (Some("main.py"), Some(1)));

    // Staging the module makes the commit self-contained.
    t.add(&["module.py"]);
    assert!(
        commit(&t, &cfg, &["main.py", "module.py"])
            .findings
            .is_empty()
    );
}

#[test]
fn three_slow_tools_run_in_parallel() {
    let t = repo_with(&[]);
    t.write("a.py", "1\n").add(&["a.py"]);
    let cfg = config(
        "[[tool]]\nname = \"one\"\ncommand = \"sleep 1\"\n\
         [[tool]]\nname = \"two\"\ncommand = \"sleep 1\"\n\
         [[tool]]\nname = \"three\"\ncommand = \"sleep 1\"\n",
    );
    let start = Instant::now();
    let r = commit(&t, &cfg, &["a.py"]);
    let elapsed = start.elapsed();
    assert!(r.findings.is_empty());
    assert_eq!(r.timings.len(), 3);
    assert!(elapsed < Duration::from_millis(2500), "{elapsed:?}");
}

mod mix_output {
    use qgate::config::ToolFormat;
    use qgate::tools::parsers::{Level, Parsed, parse, strip_ansi};

    /// Real output from Elixir 1.18 / OTP 25, captured from a Mix project.
    const COMPILE_WARN: &str = include_str!("fixtures/mix/compile_warn.txt");
    const COMPILE_ERR: &str = include_str!("fixtures/mix/compile_err.txt");
    const COMPILE_SYNTAX: &str = include_str!("fixtures/mix/compile_syntax.txt");
    const TEST: &str = include_str!("fixtures/mix/test.txt");
    const FORMAT: &str = include_str!("fixtures/mix/format.txt");

    fn at(p: &Parsed) -> (Option<&str>, Option<u32>, Level) {
        (p.path.as_deref(), p.line, p.level)
    }

    #[test]
    fn warnings_as_errors_compile_failure_is_an_error_at_the_warning() {
        let p = parse(ToolFormat::Mix, COMPILE_WARN, "");
        assert_eq!(p.len(), 1, "{p:#?}");
        assert_eq!(at(&p[0]), (Some("lib/warn.ex"), Some(2), Level::Error));
        assert!(p[0].message.starts_with("variable \"x\" is unused"));
        // Auto detects it ahead of gcc-style lines.
        assert_eq!(parse(ToolFormat::Auto, "", COMPILE_WARN), p);
    }

    #[test]
    fn plain_warnings_stay_warnings_and_errors_are_errors() {
        let warn = COMPILE_WARN.replace(
            "Compilation failed due to warnings while using the --warnings-as-errors option",
            "Generated elara app",
        );
        assert_eq!(parse(ToolFormat::Mix, &warn, "")[0].level, Level::Warning);

        let p = parse(ToolFormat::Mix, COMPILE_ERR, "");
        assert_eq!(p.len(), 1, "{p:#?}");
        assert_eq!(at(&p[0]), (Some("lib/err.ex"), Some(2), Level::Error));
        assert_eq!(p[0].message, "undefined variable \"y\"");

        let p = parse(ToolFormat::Mix, COMPILE_SYNTAX, "");
        assert_eq!(p.len(), 1, "{p:#?}");
        assert_eq!(at(&p[0]), (Some("lib/syn.ex"), Some(5), Level::Error));
    }

    #[test]
    fn pre_1_15_diagnostics_have_the_location_on_the_next_line() {
        let old = "warning: variable \"x\" is unused\n  lib/foo.ex:3: Foo.f/1\n\n";
        let p = parse(ToolFormat::Mix, old, "");
        assert_eq!(at(&p[0]), (Some("lib/foo.ex"), Some(3), Level::Warning));
    }

    #[test]
    fn exunit_failures_point_at_the_test() {
        let p = parse(ToolFormat::Mix, TEST, "");
        assert_eq!(p.len(), 2, "{p:#?}");
        assert_eq!(
            at(&p[0]),
            (Some("test/math_test.exs"), Some(10), Level::Error)
        );
        assert_eq!(
            p[0].message,
            "test raises (Elara.MathTest): ** (RuntimeError) boom"
        );
        assert_eq!(p[1].line, Some(5));
        assert_eq!(
            p[1].message,
            "test add adds two numbers (Elara.MathTest): Assertion with == failed"
        );
        assert!(p.iter().all(|f| f.rule.as_deref() == Some("test")));
        assert_eq!(parse(ToolFormat::Auto, TEST, ""), p);
    }

    #[test]
    fn unformatted_files_are_listed_with_or_without_diffs() {
        let p = parse(ToolFormat::Mix, "", FORMAT);
        assert_eq!(p.len(), 1, "{p:#?}");
        assert_eq!(p[0].path.as_deref(), Some("/work/elara/lib/fmt.ex"));
        assert_eq!(p[0].rule.as_deref(), Some("format"));

        let old = "** (Mix) mix format failed due to --check-formatted.\nThe following files are not formatted:\n\n  * lib/a.ex\n  * test/b_test.exs\n";
        let paths: Vec<_> = parse(ToolFormat::Mix, "", old)
            .into_iter()
            .filter_map(|p| p.path)
            .collect();
        assert_eq!(paths, ["lib/a.ex", "test/b_test.exs"]);
    }

    #[test]
    fn ansi_codes_are_stripped_for_every_format() {
        assert_eq!(strip_ansi("\x1b[1m\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b]0;title\x07ok"), "ok");
        let colored = "\x1b[31ma.c:3: error: boom\x1b[0m";
        let p = parse(ToolFormat::Gcc, colored, "");
        assert_eq!(
            (p[0].path.as_deref(), p[0].message.as_str()),
            (Some("a.c"), "boom")
        );
    }

    #[test]
    fn other_output_is_left_to_the_gcc_parser() {
        assert!(parse(ToolFormat::Mix, "a.c:3: error: boom\n  1) one\n", "").is_empty());
        assert_eq!(parse(ToolFormat::Auto, "a.c:3: error: boom\n", "").len(), 1);
    }
}

#[test]
fn mix_format_paths_from_a_checkout_are_repo_relative() {
    let t = repo_with(&[]);
    t.write(
        "fmt.sh",
        "printf '** (Mix) mix format failed due to --check-formatted.\\nThe following files are not formatted:\\n\\n\\033[1m\\033[31m%s/lib/a.ex\\n\\033[0m\\n' \"$PWD\"\nexit 1\n",
    );
    t.write("lib/a.ex", "x\n");
    t.git(&["add", "-A"]);
    t.write("scratch.ex", "untracked\n");
    let cfg = config("[[tool]]\nname = \"format\"\ncommand = \"sh fmt.sh\"\nformat = \"mix\"\n");
    let r = commit(&t, &cfg, &["lib/a.ex"]);
    let f = only(&r);
    assert_eq!(f.rule, "format/format");
    assert_eq!(f.severity, Severity::Error);
    assert_eq!(f.path.as_deref(), Some("lib/a.ex"));
}
