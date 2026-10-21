mod support;

use qgate::checks::exclude::{Excludes, matches_default};
use qgate::checks::length::{Measurement, measure, measure_with_macros};
use qgate::checks::{FILE_LENGTH, FUNCTION_LENGTH, PARSE_ERROR};
use qgate::config::{Config, Limits};
use qgate::git::Snapshot;
use support::repo::TestRepo;

/// (file name, text before the function, function opener, body line, closer, text after)
const LANGUAGES: &[(&str, &str, &str, &str, &str, &str)] = &[
    ("a.rs", "", "fn target() {", "    let x = 1;", "}", ""),
    ("a.py", "", "def target():", "    x = 1", "", ""),
    ("a.js", "", "function target() {", "  x = 1;", "}", ""),
    ("a.ts", "", "function target(): void {", "  x = 1;", "}", ""),
    (
        "a.tsx",
        "",
        "function target(): void {",
        "  x = 1;",
        "}",
        "",
    ),
    (
        "a.go",
        "package main\n",
        "func target() {",
        "\tx = 1",
        "}",
        "",
    ),
    (
        "A.java",
        "class A {\n",
        "void target() {",
        "  x = 1;",
        "}",
        "}\n",
    ),
    ("a.c", "", "void target(void) {", "  x = 1;", "}", ""),
    ("a.cpp", "", "void target() {", "  x = 1;", "}", ""),
    ("a.rb", "", "def target", "  x = 1", "end", ""),
    ("a.sh", "", "target() {", "  echo hi", "}", ""),
    (
        "A.cs",
        "class A {\n",
        "void target() {",
        "  x = 1;",
        "}",
        "}\n",
    ),
    (
        "a.php",
        "<?php\n",
        "function target() {",
        "  $x = 1;",
        "}",
        "",
    ),
];

fn function_of(lines: usize, lang: &(&str, &str, &str, &str, &str, &str)) -> String {
    let (_, before, open, body, close, after) = *lang;
    let closer = usize::from(!close.is_empty());
    let mut s = format!("{before}{open}\n");
    for _ in 0..lines - 1 - closer {
        s.push_str(body);
        s.push('\n');
    }
    if !close.is_empty() {
        s.push_str(close);
        s.push('\n');
    }
    s.push_str(after);
    s
}

fn limits(function_lines: u32) -> Limits {
    Limits {
        function_lines,
        file_lines: 500,
    }
}

fn target(ms: &[Measurement]) -> &Measurement {
    ms.iter()
        .find(|m| m.rule == FUNCTION_LENGTH && m.symbol.as_deref().unwrap().contains("target"))
        .unwrap_or_else(|| panic!("no target function in {ms:?}"))
}

#[test]
fn sixty_line_function_in_every_language() {
    for lang in LANGUAGES {
        let src = function_of(60, lang);
        let fifty = measure(lang.0, src.as_bytes(), limits(50));
        let m = target(&fifty.measurements);
        assert_eq!(m.value, 60, "{}", lang.0);
        assert!(m.over_limit(), "{} should fail a 50-line limit", lang.0);
        assert!(
            fifty.warnings.is_empty(),
            "{}: {:?}",
            lang.0,
            fifty.warnings
        );
        let sixty = measure(lang.0, src.as_bytes(), limits(60));
        assert!(!target(&sixty.measurements).over_limit(), "{}", lang.0);
    }
}

#[test]
fn same_name_in_different_classes_is_distinct() {
    let src = "class A:\n    def run(self):\n        pass\n\nclass B:\n    def run(self):\n        pass\n";
    let ms = measure("m.py", src.as_bytes(), limits(50)).measurements;
    let names: Vec<_> = ms.iter().filter_map(|m| m.symbol.clone()).collect();
    assert_eq!(names, ["A::run#0", "B::run#0"]);

    let rs = "struct S;\nimpl S { fn new() {} }\nstruct T;\nimpl T { fn new() {} }\nfn new() {}\nfn new() {}\n";
    let names: Vec<_> = measure("m.rs", rs.as_bytes(), limits(50))
        .measurements
        .into_iter()
        .filter_map(|m| m.symbol)
        .collect();
    assert_eq!(names, ["S::new#0", "T::new#0", "new#0", "new#1"]);

    let js = "class A { run() {} }\nconst helper = () => {};\nexport const obj = { go: function () {} };\n";
    let names: Vec<_> = measure("m.js", js.as_bytes(), limits(50))
        .measurements
        .into_iter()
        .filter_map(|m| m.symbol)
        .collect();
    assert_eq!(names, ["A::run#0", "helper#0", "go#0"]);
}

#[test]
fn unknown_language_gets_file_length_only() {
    let src = "line\n".repeat(600);
    let r = measure("notes.txt", src.as_bytes(), limits(50));
    assert_eq!(r.measurements.len(), 1);
    assert_eq!(r.measurements[0].rule, FILE_LENGTH);
    assert_eq!(r.measurements[0].value, 600);
    assert!(r.measurements[0].over_limit());
}

#[test]
fn binary_files_are_skipped() {
    let r = measure("img.png", b"\x89PNG\x00\x00binary", limits(50));
    assert!(r.measurements.is_empty());
}

#[test]
fn default_excludes() {
    for p in [
        "package-lock.json",
        "web/yarn.lock",
        "Cargo.lock",
        "vendor/lib.go",
        "a/vendor/lib.go",
        "node_modules/x/index.js",
        "dist/app.js",
        "static/app.min.js",
    ] {
        assert!(matches_default(p), "{p}");
    }
    for p in ["src/main.rs", "vendored.rs", "distance.py"] {
        assert!(!matches_default(p), "{p}");
    }
}

#[test]
fn attributes_and_config_excludes_apply() {
    let t = TestRepo::new();
    t.write(
        ".gitattributes",
        "gen/** linguist-generated\nthird/** linguist-vendored=true\n",
    )
    .write("gen/a.rs", "x")
    .write("third/b.rs", "x")
    .write("testdata/c.rs", "x")
    .write("src/d.rs", "x")
    .add(&["."]);
    let repo = t.repo();
    let snap = Snapshot::index(&repo).unwrap();
    let mut config = Config::starter();
    config.exclude = vec!["testdata/**".into()];
    let ex = Excludes::new(&repo, &snap, &config).unwrap();
    assert!(ex.is_excluded("gen/a.rs"));
    assert!(ex.is_excluded("third/b.rs"));
    assert!(ex.is_excluded("testdata/c.rs"));
    assert!(ex.is_excluded("qgate.baseline"));
    assert!(!ex.is_excluded("src/d.rs"));
}

#[test]
fn broken_syntax_warns_and_still_measures_file() {
    let src = "fn broken( {\n".to_string() + &"let x = ;\n".repeat(10);
    let r = measure("b.rs", src.as_bytes(), limits(50));
    assert!(r.warnings.iter().any(|w| w.rule == PARSE_ERROR));
    assert!(
        r.measurements
            .iter()
            .any(|m| m.rule == FILE_LENGTH && m.value == 11)
    );
}

#[test]
fn crlf_and_lf_measure_the_same() {
    let lf = function_of(60, &LANGUAGES[0]);
    let crlf = lf.replace('\n', "\r\n");
    let a = measure("a.rs", lf.as_bytes(), limits(50)).measurements;
    let b = measure("a.rs", crlf.as_bytes(), limits(50)).measurements;
    let values = |ms: &[Measurement]| ms.iter().map(|m| m.value).collect::<Vec<_>>();
    assert_eq!(values(&a), values(&b));
}

#[test]
fn body_hash_ignores_name_comments_and_whitespace() {
    let a = "fn one() {\n    let x = 1; // note\n}\n";
    let b = "fn two() {\n\n        let x   =   1;\n}\n";
    let c = "fn one() {\n    let x = 2;\n}\n";
    let h = |s: &str| measure("a.rs", s.as_bytes(), limits(50)).measurements[1].body_hash;
    assert_eq!(h(a), h(b));
    assert_ne!(h(a), h(c));
}

const MACRO_C: &str = "#define MUST_USE __attribute__((warn_unused_result))\n\
#define PRINTF(a, b) __attribute__((format(printf, a, b)))\n\
\n\
MUST_USE static int first(int x) {\n\
    int y = x + 1;\n\
    return y;\n\
}\n\
\n\
static MUST_USE PRINTF(1,\n\
    2) int second(const char *f, ...)\n\
{\n\
    return 0;\n\
}\n";

/// Unknown macros on one line each: tree-sitter recovers with single-line errors.
const MACRO_C_ONE_LINE: &str = "MUST_USE static int first(int x) {\n\
    int y = x + 1;\n\
    return y;\n\
}\n\
\n\
static MUST_USE bool second(void) {\n\
    return true;\n\
}\n";

fn c_functions(r: &qgate::checks::length::FileLengths) -> Vec<(String, u32, u32)> {
    r.measurements
        .iter()
        .filter(|m| m.rule == FUNCTION_LENGTH)
        .map(|m| (m.symbol.clone().unwrap(), m.line, m.value))
        .collect()
}

#[test]
fn single_line_macro_errors_do_not_warn_per_file() {
    let r = measure("m.c", MACRO_C_ONE_LINE.as_bytes(), limits(50));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(r.recovered_errors);
    assert_eq!(
        c_functions(&r),
        vec![
            ("first#0".to_string(), 1, 4),
            ("second#0".to_string(), 6, 3)
        ]
    );
    // A macro whose arguments span lines leaves a multi-line error: warn per file.
    let r = measure("m.c", MACRO_C.as_bytes(), limits(50));
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
}

#[test]
fn declared_attribute_macros_parse_cleanly_with_the_same_lines() {
    let macros = vec!["MUST_USE".to_string(), "PRINTF".to_string()];
    let r = measure_with_macros("m.c", MACRO_C.as_bytes(), limits(50), &macros);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(!r.recovered_errors);
    assert_eq!(
        c_functions(&r),
        vec![
            ("first#0".to_string(), 4, 4),
            ("second#0".to_string(), 9, 5)
        ]
    );
    let plain = measure_with_macros(
        "m.c",
        b"int f(void) { return MUST_USE; }\n",
        limits(50),
        &macros,
    );
    assert_eq!(c_functions(&plain).len(), 1);
}

#[test]
fn blanking_keeps_offsets_and_skips_directives() {
    let src = "#define M x\nM int f(void);\nMM M(a,\nb) M2\n";
    let out = qgate::grammars::blank_macros(src, &["M".to_string()]);
    assert_eq!(out, "#define M x\n  int f(void);\nMM     \n   M2\n");
}

#[test]
fn recovered_parse_errors_are_one_summary_per_run() {
    let t = TestRepo::new();
    t.write("a.c", MACRO_C_ONE_LINE)
        .write("b.c", MACRO_C_ONE_LINE)
        .add(&["."]);
    let repo = t.repo();
    let snap = Snapshot::index(&repo).unwrap();
    let config = Config::starter();
    let ex = Excludes::new(&repo, &snap, &config).unwrap();
    let paths = vec!["a.c".to_string(), "b.c".to_string()];
    let m = qgate::baseline::measure_paths(&repo, &snap, &ex, &config, &paths).unwrap();
    assert_eq!(m.warnings.len(), 1, "{:?}", m.warnings);
    assert_eq!(m.warnings[0].rule, PARSE_ERROR);
    assert!(
        m.warnings[0]
            .message
            .starts_with("a.c and 1 other file parsed")
    );
}
