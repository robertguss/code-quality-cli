//! Python escape hatches, swallowed errors and debug output (U6 to U8).

mod support;

use support::rules::{count, occurrences};

const UNANNOTATED: &str = "escape-hatch/py-unannotated-public";

#[test]
fn public_module_functions_need_annotated_parameters() {
    let src =
        "def f(x):\n    pass\n\ndef _f(x):\n    pass\n\ndef g(a: int) -> int:\n    return a\n";
    assert_eq!(occurrences("pkg/a.py", src), vec![(UNANNOTATED.into(), 1)]);
    assert_eq!(
        count("tests/test_a.py", src, UNANNOTATED),
        0,
        "public API is shipped code"
    );
}

#[test]
fn every_parameter_form_needs_an_annotation_and_counts_once() {
    let src = "def f(a, b=1, *args, **kw):\n    pass\n\ndef g(a: int, b: int = 1, *args: int, **kw: str):\n    pass\n";
    assert_eq!(occurrences("pkg/a.py", src), vec![(UNANNOTATED.into(), 1)]);
}

#[test]
fn nested_functions_do_not_count() {
    let src = "def f(a: int) -> None:\n    def inner(x):\n        pass\n";
    assert_eq!(count("pkg/a.py", src, UNANNOTATED), 0);
}

#[test]
fn methods_skip_self_but_count_other_unannotated_parameters() {
    let src = "class C:\n    def __init__(self, a: int):\n        pass\n\n    @classmethod\n    def make(cls, b: str):\n        pass\n\n    def m(self, a):\n        pass\n\n    def _p(self, a):\n        pass\n\n    def __eq__(self, other):\n        pass\n";
    assert_eq!(
        occurrences("pkg/a.py", src),
        vec![(UNANNOTATED.into(), 9), (UNANNOTATED.into(), 15)]
    );
}

#[test]
fn decorated_functions_are_found_at_the_def_line() {
    let src = "@app.route(\"/\")\ndef index(request):\n    pass\n";
    assert_eq!(occurrences("pkg/a.py", src), vec![(UNANNOTATED.into(), 2)]);
}

#[test]
fn any_counts_in_annotations_only() {
    let src = "from typing import Any\nimport typing\n\nx: Any = 1\n\ndef f(a: Any) -> None:\n    pass\n\ndef g() -> list[Any]:\n    pass\n\ndef h(b: typing.Any) -> dict[str, typing.Any]:\n    pass\n\ns = \"Any\"\n# Any\nAny = 2\n";
    let rule = "escape-hatch/py-any";
    let lines: Vec<u32> = occurrences("pkg/a.py", src)
        .into_iter()
        .filter(|(r, _)| r == rule)
        .map(|(_, l)| l)
        .collect();
    assert_eq!(
        lines,
        vec![4, 6, 9, 12, 12],
        "each use counts, two on one line too"
    );
}

#[test]
fn typing_cast_counts_and_bare_cast_needs_the_import() {
    let rule = "escape-hatch/py-cast";
    let qualified = "import typing\n\nv = typing.cast(int, w)\n";
    assert_eq!(count("pkg/a.py", qualified, rule), 1);
    let bare = "v = cast(int, w)\n";
    assert_eq!(count("pkg/a.py", bare, rule), 0);
    let imported = "from typing import Any, cast\n\nv = cast(int, w)\n";
    assert_eq!(occurrences("pkg/a.py", imported), vec![(rule.into(), 3)]);
    let other = "from mylib import cast\n\nv = cast(int, w)\n";
    assert_eq!(count("pkg/a.py", other, rule), 0);
}

#[test]
fn broad_excepts_that_only_pass_count() {
    let src = "try:\n    f()\nexcept:\n    pass\n\ntry:\n    f()\nexcept Exception:\n    ...\n\ntry:\n    f()\nexcept BaseException as e:\n    # ignore\n    pass\n\ntry:\n    f()\nexcept ValueError:\n    pass\n\ntry:\n    f()\nexcept Exception as e:\n    log(e)\n";
    assert_eq!(
        occurrences("pkg/a.py", src),
        vec![
            ("swallowed-error/py-bare-pass".into(), 3),
            ("swallowed-error/py-bare-pass".into(), 8),
            ("swallowed-error/py-bare-pass".into(), 13),
        ]
    );
}

#[test]
fn print_counts_outside_tests_entry_points_and_scripts() {
    let src = "print(\"x\")\nlog.print(\"y\")\n";
    let rule = "debug-output/py-print";
    assert_eq!(occurrences("pkg/a.py", src), vec![(rule.into(), 1)]);
    for path in ["pkg/__main__.py", "scripts/x.py", "tests/test_a.py"] {
        assert_eq!(count(path, src, rule), 0, "{path}");
    }
}

#[test]
fn breakpoint_counts_in_entry_points_but_not_tests() {
    let src = "breakpoint()\npdb.set_trace()\n";
    let rule = "debug-output/py-breakpoint";
    assert_eq!(count("pkg/__main__.py", src, rule), 2);
    assert_eq!(count("pkg/a.py", src, rule), 2);
    assert_eq!(count("tests/test_a.py", src, rule), 0);
}

#[test]
fn rules_inside_strings_and_comments_do_not_count() {
    let src = "s = \"print('x') breakpoint() typing.cast(int, v)\"\n# print(x)\n";
    assert!(occurrences("pkg/a.py", src).is_empty());
}

#[test]
fn print_under_the_main_guard_is_a_script() {
    let src = "def f() -> None:\n    print(1)\n\nif __name__ == '__main__':\n    print(f())\n";
    assert_eq!(
        occurrences("pkg/a.py", src),
        vec![("debug-output/py-print".to_string(), 2)]
    );
}

#[test]
fn unannotated_test_functions_are_not_public_api() {
    let src = "def test_get(httpbin):\n    pass\n";
    assert!(occurrences("tests/test_a.py", src).is_empty());
}
