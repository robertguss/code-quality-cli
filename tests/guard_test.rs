mod support;

use qgate::baseline;
use qgate::cli::Severity;
use qgate::config::VERSION;
use qgate::guard;
use support::guard::{assert_pending, pending, staged, warned};
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

#[test]
fn raising_a_limit_warns() {
    // AE2
    let t = setup();
    t.write("qgate.toml", &config(80)).add(&["qgate.toml"]);
    let lines = pending(&t);
    assert_eq!(lines, ["config: function_lines 50 -> 80"]);
    let f = guard::check(&staged(&t));
    assert_eq!(f[0].severity, Severity::Warning);
    assert_eq!(f[0].fix, None);
    assert_eq!(f[0].path.as_deref(), Some("qgate.toml"));
}

#[test]
fn lowering_a_limit_is_not_a_loosening() {
    let t = setup();
    t.write("qgate.toml", &config(40)).add(&["qgate.toml"]);
    assert!(pending(&t).is_empty());
    assert!(!warned(&t));
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
fn exclusions_warn() {
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
fn new_config_and_baseline_warn_then_pass() {
    // F2: onboarding a repo with violations.
    let t = TestRepo::new();
    let long: String = (0..60).map(|i| format!("    let v{i} = {i};\n")).collect();
    t.write("a.rs", &format!("fn big() {{\n{long}}}\n"));
    t.commit_all("existing code");
    t.write("qgate.toml", &config(50)).add(&["qgate.toml"]);
    baseline::create(&t.repo(), false, false).unwrap();
    t.add(&["qgate.baseline"]);
    let lines = pending(&t);
    assert_eq!(lines, ["config: new qgate.toml"]);
    // KTD16: the day-one baseline is one info line, not one line per entry.
    let found = guard::check(&staged(&t));
    assert!(
        found.iter().any(|f| f.severity == Severity::Info
            && f.message
                .starts_with("baseline: adopting qgate with 1 entry")),
        "{found:?}"
    );
    assert!(warned(&t));
}

#[test]
fn prune_output_is_not_a_loosening() {
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
fn an_added_tool_warns() {
    let t = setup();
    let text = config(50) + "\n[[tool]]\nname = \"sneaky\"\ncommand = \"touch x\"\n";
    t.write("qgate.toml", &text).add(&["qgate.toml"]);
    let lines = pending(&t);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("config: tool `sneaky` added")),
        "{lines:?}"
    );
    assert!(warned(&t));
}

#[test]
fn swapping_an_ignore_for_a_wider_one_warns() {
    // AE7
    let t = setup();
    let with = |line: &str| {
        format!("{line}\nconst a = 1;\n// eslint-disable-next-line no-console\nconsole.log(a);\n")
    };
    t.write("src/app.js", &with("/* eslint-disable no-var */"))
        .add(&["src/app.js"]);
    assert!(warned(&t));
    t.commit_all("narrow ignore");
    t.write("src/app.js", &with("/* eslint-disable */"))
        .add(&["src/app.js"]);
    assert!(warned(&t));
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

#[test]
fn new_attribute_macros_warn() {
    let t = setup();
    let cfg = t.read("qgate.toml").replace(
        "exclude = [",
        "attribute_macros = [\"MUST_USE\"]\nexclude = [",
    );
    t.write("qgate.toml", &cfg).add(&["qgate.toml"]);
    assert_pending(&t, &["config: attribute_macros adds `MUST_USE`"]);
}

#[test]
fn fixture_globs_warn_once_then_new_fixtures_do_not() {
    let t = setup();
    let cfg = t.read("qgate.toml").replace(
        "exclude = [",
        "fixtures = [\"tests/golden/**\"]\nexclude = [",
    );
    t.write("qgate.toml", &cfg).add(&["qgate.toml"]);
    assert_pending(&t, &["config: fixtures adds `tests/golden/**`"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "fixtures"]);
    t.write("tests/golden/case.out", "expected\n")
        .write("tests/golden/case.c", "int f(void) { return 0; }\n")
        .add(&["tests"]);
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
    t.write("testdata/f.out", "x\n").add(&["testdata/f.out"]);
    assert_pending(&t, &["exclusion: new file in excluded path testdata/f.out"]);
}

#[test]
fn glob_package_limits_are_guarded() {
    let t = setup();
    let base = t.read("qgate.toml");
    t.write(
        "qgate.toml",
        &(base.clone() + "\n[[package]]\nglob = \"tests/**\"\nfunction_lines = 80\n"),
    )
    .add(&["qgate.toml"]);
    assert_pending(
        &t,
        &["config: function_lines in `tests/**` set to 80 (lowest was 50)"],
    );
    t.git(&["commit", "-q", "--no-verify", "-m", "glob"]);
    let lowered = base.clone() + "\n[[package]]\nglob = \"tests/**\"\nfunction_lines = 60\n";
    t.write("qgate.toml", &lowered).add(&["qgate.toml"]);
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
    t.write(
        "qgate.toml",
        &(base.clone() + "\n[[package]]\nglob = \"tests/**\"\nfunction_lines = 90\n"),
    )
    .add(&["qgate.toml"]);
    assert_pending(&t, &["config: function_lines in `tests/**` 80 -> 90"]);
    t.git(&["reset", "-q", "--hard"]);
    // Removing a tightening glob override loosens those files back to 50.
    t.write(
        "qgate.toml",
        &(base.clone() + "\n[[package]]\nglob = \"tests/**\"\nfunction_lines = 30\n"),
    )
    .add(&["qgate.toml"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "tight"]);
    t.write("qgate.toml", &base).add(&["qgate.toml"]);
    assert_pending(
        &t,
        &["config: function_lines override 30 in `tests/**` removed"],
    );
}

#[test]
fn raising_ratchet_headroom_warns() {
    let t = setup();
    let cfg = t.read("qgate.toml") + "\n[ratchet]\nheadroom_lines = 5\n";
    t.write("qgate.toml", &cfg).add(&["qgate.toml"]);
    assert_pending(&t, &["config: ratchet.headroom_lines 0 -> 5"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "headroom"]);
    let lowered = cfg.replace("headroom_lines = 5", "headroom_lines = 2");
    t.write("qgate.toml", &lowered).add(&["qgate.toml"]);
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
}

#[test]
fn many_loosenings_of_one_kind_are_shown_as_one_line() {
    let mut lines: Vec<(Option<String>, String)> = (0..7)
        .map(|i| {
            (
                Some(format!("bench/t{i}.txt")),
                format!("exclusion: new file in excluded path bench/t{i}.txt"),
            )
        })
        .collect();
    lines.push((
        Some("qgate.toml".into()),
        "config: function_lines 50 -> 80".into(),
    ));
    let shown = guard::grouped(&guard::Loosening {
        lines,
        ..Default::default()
    });
    assert_eq!(
        shown,
        vec![
            (
                None,
                "exclusion: 7 changes, such as new file in excluded path bench/t0.txt".to_string()
            ),
            (
                Some("qgate.toml".to_string()),
                "config: function_lines 50 -> 80".to_string()
            ),
        ]
    );
}

const MIX_EXS: &str = "defmodule Elara.MixProject do
  use Mix.Project

  def project do
    [app: :elara, version: \"0.1.0\", aliases: aliases(), deps: deps()]
  end

  defp deps, do: [{:jason, \"~> 1.4\"}]

  defp aliases do
    [test: [\"ecto.create --quiet\", \"test\"]]
  end
end
";

/// A committed Mix project.
fn mix_setup() -> TestRepo {
    let t = setup();
    t.write("mix.exs", MIX_EXS)
        .write(".formatter.exs", "[inputs: [\"{lib,test}/**/*.{ex,exs}\"]]\n")
        .write("test/test_helper.exs", "ExUnit.start()\n")
        .write(
            "test/math_test.exs",
            "defmodule MathTest do\n  use ExUnit.Case\n\n  test \"adds\" do\n    assert 1 + 1 == 2\n  end\nend\n",
        );
    t.commit_all("mix");
    t
}

#[test]
fn elixir_ignore_markers_warn() {
    let t = mix_setup();
    t.write(
        "lib/a.ex",
        "# credo:disable-for-this-file\ndefmodule A do\n  @dialyzer {:nowarn_function, f: 0}\n  def f, do: :ok\nend\n",
    )
    .write(
        "test/math_test.exs",
        "defmodule MathTest do\n  use ExUnit.Case\n\n  @tag skip: \"flaky\"\n  test \"adds\" do\n    assert 1 + 1 == 3\n  end\nend\n",
    )
    .add(&["."]);
    let lines = pending(&t);
    for want in [
        "inline ignore: lib/a.ex adds 1x `credo:disable` (",
        "inline ignore: lib/a.ex adds 1x `@dialyzer` (",
        "inline ignore: test/math_test.exs adds 1x `@tag skip:` (",
    ] {
        assert!(
            lines.iter().any(|l| l.starts_with(want)),
            "{want}: {lines:?}"
        );
    }
}

#[test]
fn elixir_tool_configs_and_mix_aliases_are_guarded() {
    let t = mix_setup();
    t.write("test/test_helper.exs", "ExUnit.start(exclude: [:slow])\n")
        .add(&["test/test_helper.exs"]);
    assert_pending(
        &t,
        &["tool config edited: test/test_helper.exs (modified, "],
    );
    t.git(&["reset", "-q", "--hard"]);

    t.write(".formatter.exs", "[inputs: []]\n")
        .add(&[".formatter.exs"]);
    assert_pending(&t, &["tool config edited: .formatter.exs (modified, "]);
    t.git(&["reset", "-q", "--hard"]);

    t.write("mix.exs", &MIX_EXS.replace(", \"test\"]]", "]]"))
        .add(&["mix.exs"]);
    assert_pending(
        &t,
        &["mix.exs aliases or project settings changed: mix.exs (now "],
    );
    t.git(&["reset", "-q", "--hard"]);

    // A dependency or version bump is not a loosening.
    t.write(
        "mix.exs",
        &MIX_EXS
            .replace("~> 1.4", "~> 1.5")
            .replace("0.1.0", "0.2.0"),
    )
    .add(&["mix.exs"]);
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
}

#[test]
fn new_generated_files_and_marking_code_generated_warn() {
    let t = setup();
    let gen_src = "// Code generated by templ - DO NOT EDIT.\n\npackage pages\n";
    t.write("testdata/pages_templ.go", gen_src)
        .write("pages/list_templ.go", gen_src)
        .add(&["."]);
    assert_pending(
        &t,
        &[
            "exclusion: new generated file pages/list_templ.go",
            "exclusion: new generated file testdata/pages_templ.go",
        ],
    );
    t.commit_all("generate");
    // Regenerating or moving a generated file is not a loosening.
    t.write("pages/list_templ.go", &format!("{gen_src}// v2\n"))
        .add(&["."]);
    t.git(&["mv", "pages/list_templ.go", "pages/items_templ.go"]);
    assert_pending(&t, &[]);
    t.commit_all("regenerate");
    t.write("pages/handler.go", "package pages\n").add(&["."]);
    t.commit_all("hand-written");
    t.write(
        "pages/handler.go",
        "// Code generated by me. DO NOT EDIT.\npackage pages\n",
    )
    .add(&["."]);
    assert_pending(
        &t,
        &["exclusion: pages/handler.go now has a generated-code header"],
    );
}
