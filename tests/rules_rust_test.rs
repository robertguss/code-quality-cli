//! Rust escape hatches, swallowed errors and debug output (U6 to U8).

mod support;

use support::rules::{count, occurrences, occurrences_with};

fn body(stmts: &str) -> String {
    format!("pub fn f() {{\n{stmts}\n}}\n")
}

#[test]
fn unwrap_and_expect_count_but_unwrap_or_does_not() {
    let src = body(
        "    a.unwrap();\n    b.expect(\"m\");\n    c.unwrap_or(0);\n    d.unwrap_or_else(|| 1);",
    );
    assert_eq!(count("src/a.rs", &src, "escape-hatch/rust-unwrap"), 2);
}

#[test]
fn unsafe_blocks_count_and_unsafe_fns_do_not() {
    let src = "pub unsafe fn raw() {}\npub fn f() {\n    unsafe { raw() }\n}\n";
    assert_eq!(
        occurrences("src/a.rs", src),
        vec![("escape-hatch/rust-unsafe".into(), 3)]
    );
    assert_eq!(
        count("tests/a.rs", src, "escape-hatch/rust-unsafe"),
        1,
        "everywhere"
    );
}

#[test]
fn panic_macros_count_outside_tests() {
    let src = body(
        "    panic!(\"x\");\n    todo!();\n    unimplemented!();\n    unreachable!();\n    assert!(true);\n    std::panic!(\"y\");",
    );
    assert_eq!(count("src/a.rs", &src, "escape-hatch/rust-panic"), 5);
    assert_eq!(count("tests/a.rs", &src, "escape-hatch/rust-panic"), 0);
}

#[test]
fn numeric_cast_is_heuristic_and_other_casts_are_not_counted() {
    let src = body("    let a = x as u32;\n    let b = p as *const u8;\n    let c = y as f64;");
    assert_eq!(count("src/a.rs", &src, "escape-hatch/rust-numeric-cast"), 2);
    let r = qgate::rules::find("escape-hatch/rust-numeric-cast").unwrap();
    assert_eq!(r.kind, qgate::rules::Kind::Heuristic);
}

#[test]
fn ok_statement_counts_and_bound_ok_does_not() {
    let src = body("    f().ok();\n    let v = f().ok();\n    g(f().ok());\n    x.ok(1);");
    assert_eq!(
        occurrences("src/a.rs", &src),
        vec![("swallowed-error/rust-ok-statement".into(), 2)]
    );
}

#[test]
fn let_underscore_and_unwrap_or_default_are_heuristic() {
    let src = body(
        "    let _ = tx.send(v);\n    let _ = send(v).await;\n    let _x = tx.send(v);\n    let _ = value;\n    let n = r.unwrap_or_default();",
    );
    let got = occurrences("src/a.rs", &src);
    assert_eq!(
        got,
        vec![
            ("swallowed-error/let-underscore".into(), 2),
            ("swallowed-error/let-underscore".into(), 3),
            ("swallowed-error/unwrap-or-default".into(), 6),
        ]
    );
    for id in [
        "swallowed-error/let-underscore",
        "swallowed-error/unwrap-or-default",
    ] {
        assert_eq!(
            qgate::rules::find(id).unwrap().kind,
            qgate::rules::Kind::Heuristic
        );
    }
}

#[test]
fn println_counts_in_libraries_but_not_entry_points() {
    let src = body(
        "    println!(\"a\");\n    eprintln!(\"b\");\n    print!(\"c\");\n    format!(\"d\");",
    );
    let rule = "debug-output/rust-println";
    assert_eq!(count("src/lib.rs", &src, rule), 3);
    for entry in [
        "src/main.rs",
        "src/bin/x.rs",
        "build.rs",
        "crates/app/src/main.rs",
        "scripts/gen.rs",
    ] {
        assert_eq!(count(entry, &src, rule), 0, "{entry}");
    }
    assert_eq!(
        count("src/cli/main.rs", &src, rule),
        3,
        "only a crate's src/main.rs"
    );
}

#[test]
fn dbg_is_never_exempt_outside_tests() {
    let src = body("    dbg!(x);\n    println!(\"out\");");
    let cfg = "[rules.debug-output]\nallow = [\"src/cli/**\"]\n";
    assert_eq!(
        occurrences_with("src/cli/output.rs", &src, cfg),
        vec![("debug-output/rust-dbg".into(), 2)]
    );
    assert_eq!(count("src/main.rs", &src, "debug-output/rust-dbg"), 1);
    assert_eq!(count("tests/a.rs", &src, "debug-output/rust-dbg"), 0);
}

#[test]
fn rules_inside_strings_and_comments_do_not_count() {
    let src = body(
        "    let s = \"x.unwrap() println!()\";\n    // y.unwrap(); dbg!(z);\n    /* panic!() */",
    );
    assert!(occurrences("src/a.rs", &src).is_empty());
}
