//! Go escape hatches, swallowed errors and debug output (U6 to U8).

mod support;

use support::rules::{count, occurrences};

fn server(code: &str) -> String {
    format!("package server\n\n{code}\n")
}

fn body(stmts: &str) -> String {
    server(&format!("func f() {{\n{stmts}\n}}"))
}

#[test]
fn any_and_empty_interface_in_signatures_count() {
    let src = server(
        "func F(a any) {}\nfunc G() interface{} { return nil }\nfunc (r *R) M(xs ...any) {}\nfunc N() (map[string]any, error) { return nil, nil }\nvar h = func(v interface{}) {}",
    );
    let got = occurrences("pkg/a.go", &src);
    let lines: Vec<u32> = got
        .iter()
        .filter(|(r, _)| r == "escape-hatch/go-any")
        .map(|(_, l)| *l)
        .collect();
    assert_eq!(lines, vec![3, 4, 5, 6, 7]);
    assert_eq!(count("pkg/a_test.go", &src, "escape-hatch/go-any"), 5);
}

#[test]
fn non_empty_interfaces_generic_constraints_and_bodies_do_not_count_as_any() {
    let src = server(
        "func F(r interface{ M() }) {}\nfunc G[T any](v T) {}\nfunc H() {\n\tvar x any\n\tvar y interface{}\n\t_, _ = x, y\n}",
    );
    assert_eq!(count("pkg/a.go", &src, "escape-hatch/go-any"), 0);
}

#[test]
fn single_value_type_assertions_count() {
    let src = body(
        "\tv := x.(T)\n\tf(x.(T))\n\tw, ok := x.(T)\n\tw, ok = x.(T)\n\tvar u, ok2 = x.(T)\n\tswitch y := x.(type) {\n\tdefault:\n\t\t_ = y\n\t}",
    );
    let lines: Vec<u32> = occurrences("pkg/a.go", &src)
        .into_iter()
        .filter(|(r, _)| r == "escape-hatch/go-type-assert")
        .map(|(_, l)| l)
        .collect();
    assert_eq!(lines, vec![4, 5]);
}

#[test]
fn panic_counts_outside_package_main_and_tests() {
    let src = body("\tpanic(\"x\")\n\tpanicky()");
    let rule = "escape-hatch/go-panic";
    assert_eq!(count("pkg/server.go", &src, rule), 1);
    assert_eq!(count("pkg/a_test.go", &src, rule), 0);
    let main = "package main\n\nfunc main() {\n\tpanic(\"x\")\n}\n";
    assert_eq!(count("cmd/app/main.go", main, rule), 0);
}

#[test]
fn return_nil_after_err_check_counts() {
    let src = server(
        "func f() error {\n\tif err != nil {\n\t\treturn nil\n\t}\n\treturn nil\n}\nfunc g() (*T, error) {\n\tif err := h(); err != nil {\n\t\treturn nil, nil\n\t}\n\treturn nil, nil\n}",
    );
    let got: Vec<u32> = occurrences("pkg/a.go", &src)
        .into_iter()
        .filter(|(r, _)| r == "swallowed-error/go-return-nil")
        .map(|(_, l)| l)
        .collect();
    assert_eq!(got, vec![4, 10]);
}

#[test]
fn returning_or_handling_the_error_does_not_count() {
    let src = server(
        "func g() (*T, error) {\n\tif err != nil {\n\t\treturn nil, err\n\t}\n\treturn nil, nil\n}\nfunc p() *T {\n\tif err != nil {\n\t\treturn nil\n\t}\n\treturn nil\n}\nfunc q() error {\n\tif err != nil {\n\t\tlog(err)\n\t\treturn nil\n\t}\n\tif cached != nil {\n\t\treturn nil\n\t}\n\treturn nil\n}",
    );
    assert_eq!(count("pkg/a.go", &src, "swallowed-error/go-return-nil"), 0);
}

#[test]
fn blank_assign_of_a_call_is_heuristic() {
    let src = body("\t_ = w.Close()\n\t_ = x\n\t_, _ = f()\n\tv := w.Close()\n\t_ = v");
    let rule = "swallowed-error/blank-assign";
    assert_eq!(occurrences("pkg/a.go", &src), vec![(rule.to_string(), 4)]);
    assert_eq!(
        qgate::rules::find(rule).unwrap().kind,
        qgate::rules::Kind::Heuristic
    );
}

#[test]
fn fmt_print_counts_outside_package_main_and_tests() {
    let src = body(
        "\tfmt.Println(\"a\")\n\tfmt.Printf(\"%d\", 1)\n\tfmt.Print(\"b\")\n\tprintln(\"c\")\n\tprint(\"d\")\n\t_ = fmt.Sprintf(\"e\")\n\tfmt.Fprintf(w, \"f\")",
    );
    let rule = "debug-output/go-print";
    assert_eq!(count("pkg/server.go", &src, rule), 5);
    assert_eq!(count("pkg/x_test.go", &src, rule), 0);
    let main = "package main\n\nfunc main() {\n\tfmt.Println(\"a\")\n}\n";
    assert_eq!(count("cmd/app/main.go", main, rule), 0);
}

#[test]
fn rules_inside_strings_and_comments_do_not_count() {
    let src = body("\ts := \"panic(x) fmt.Println()\"\n\t// v := x.(T)\n\t/* _ = f() */\n\tuse(s)");
    assert!(occurrences("pkg/a.go", &src).is_empty());
}
