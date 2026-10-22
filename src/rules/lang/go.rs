//! Go rules.

use std::sync::OnceLock;

use tree_sitter::{Node, Query};

use super::{File, Hit, each_capture, query, text};
use crate::grammars::Lang;
use crate::rules::entry::is_package_main;
use crate::rules::{Kind, Rule, Scope};

const ESCAPE: &str = "pstack /correct (escape hatches)";
const SWALLOW: &str = "pstack /correct (swallowed errors)";
const DEBUG: &str = "pstack /correct (debug output)";

pub const RULES: &[Rule] = &[
    Rule {
        id: "escape-hatch/go-any",
        kind: Kind::Exact,
        language: "go",
        source: ESCAPE,
        rationale: "`any` or `interface{}` in a signature gives up the compiler's type checks",
        fix: "use a concrete type, a small interface, or a type parameter",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/go-type-assert",
        kind: Kind::Exact,
        language: "go",
        source: ESCAPE,
        rationale: "a single-value type assertion `x.(T)` panics when `x` is not a `T`",
        fix: "use the two-value form `v, ok := x.(T)` and handle `!ok`, or a type switch",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/go-panic",
        kind: Kind::Exact,
        language: "go",
        source: ESCAPE,
        rationale: "`panic` in library code crashes the caller instead of reporting an error",
        fix: "return an `error` the caller can handle",
        since: "0.6.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
    Rule {
        id: "swallowed-error/go-return-nil",
        kind: Kind::Exact,
        language: "go",
        source: SWALLOW,
        rationale: "`if err != nil { return nil }` reports success after a failure",
        fix: "return the error, wrapped with context: `return fmt.Errorf(\"...: %w\", err)`",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "swallowed-error/blank-assign",
        kind: Kind::Heuristic,
        language: "go",
        source: SWALLOW,
        rationale: "`_ = f()` drops the call's result, which may be an error",
        fix: "handle the error, or add a comment saying why it can be ignored",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "debug-output/go-print",
        kind: Kind::Exact,
        language: "go",
        source: DEBUG,
        rationale: "printing from library code bypasses the program's output and logging",
        fix: "return the value or log it; print only from `package main` or an output file listed in `[rules.debug-output] allow`",
        since: "0.6.0",
        counted: true,
        scope: Scope::Output,
    },
];

const QUERY: &str = r#"
(call_expression function: (identifier) @call)

((call_expression
  function: (selector_expression
    operand: (identifier) @_pkg
    field: (field_identifier) @fmt))
  (#eq? @_pkg "fmt")
  (#any-of? @fmt "Print" "Printf" "Println"))

((type_identifier) @any (#eq? @any "any"))

(interface_type) @interface

(type_assertion_expression) @assert

((if_statement
  condition: (binary_expression
    left: (identifier) @_err
    operator: "!="
    right: (nil))) @if-err
  (#match? @_err "^err$|Err$"))

((assignment_statement
  left: (expression_list . (identifier) @_blank .)
  right: (expression_list . (call_expression) .)) @blank
  (#eq? @_blank "_"))
"#;

/// Node kinds whose `parameters` and `result` fields form a signature.
const SIGNATURES: &[&str] = &[
    "function_declaration",
    "method_declaration",
    "func_literal",
    "function_type",
    "method_elem",
];

pub fn hits(file: &File, out: &mut Vec<Hit>) {
    static Q: OnceLock<Query> = OnceLock::new();
    let src = file.src;
    let main = is_package_main(file.tree, src);
    each_capture(query(Lang::Go, &Q, QUERY), file.tree, src, |name, node| {
        let rule = match name {
            "call" => match text(node, src) {
                "panic" if !main => "escape-hatch/go-panic",
                "print" | "println" => "debug-output/go-print",
                _ => return,
            },
            "fmt" => "debug-output/go-print",
            "any" if in_signature(node) => "escape-hatch/go-any",
            "interface" if node.named_child_count() == 0 && in_signature(node) => {
                "escape-hatch/go-any"
            }
            "assert" if !comma_ok(node) => "escape-hatch/go-type-assert",
            "if-err" if returns_only_nil(node, src) => "swallowed-error/go-return-nil",
            "blank" => "swallowed-error/blank-assign",
            _ => return,
        };
        out.push(Hit::at(rule, node));
    });
}

/// Whether `node` sits in a function's parameter or result types.
fn in_signature(node: Node) -> bool {
    let mut child = node;
    while let Some(parent) = child.parent() {
        if SIGNATURES.contains(&parent.kind()) {
            return ["parameters", "result"]
                .iter()
                .any(|f| parent.child_by_field_name(f) == Some(child));
        }
        child = parent;
    }
    false
}

/// `v, ok := x.(T)`, `v, ok = x.(T)` or `var v, ok = x.(T)`.
fn comma_ok(assert: Node) -> bool {
    let Some(right) = assert.parent().filter(|p| p.kind() == "expression_list") else {
        return false;
    };
    let Some(stmt) = right.parent() else {
        return false;
    };
    let targets = match stmt.kind() {
        "short_var_declaration" | "assignment_statement" => stmt
            .child_by_field_name("left")
            .map_or(0, |l| l.named_child_count()),
        "var_spec" => stmt
            .children_by_field_name("name", &mut stmt.walk())
            .count(),
        _ => 0,
    };
    right.named_child_count() == 1 && targets == 2
}

/// The `if` body is a lone `return` of only `nil`s: `return nil, nil`, or
/// `return nil` in a function whose only result is `error`.
fn returns_only_nil(stmt: Node, src: &str) -> bool {
    let Some(ret) = stmt
        .child_by_field_name("consequence")
        .and_then(lone_statement)
    else {
        return false;
    };
    if ret.kind() != "return_statement" {
        return false;
    }
    let values: Vec<Node> = ret
        .named_child(0)
        .map(|list| list.named_children(&mut list.walk()).collect())
        .unwrap_or_default();
    let all_nil = !values.is_empty() && values.iter().all(|v| v.kind() == "nil");
    all_nil && (values.len() > 1 || returns_only_error(stmt, src))
}

/// The only statement in `block`, ignoring comments.
fn lone_statement(block: Node) -> Option<Node> {
    let mut cursor = block.walk();
    let mut nodes: Vec<Node> = block.named_children(&mut cursor).collect();
    if let [list] = nodes[..]
        && list.kind() == "statement_list"
    {
        nodes = list.named_children(&mut list.walk()).collect();
    }
    let mut stmts = nodes.into_iter().filter(|n| n.kind() != "comment");
    match (stmts.next(), stmts.next()) {
        (Some(s), None) => Some(s),
        _ => None,
    }
}

/// The enclosing function's result is exactly `error`.
fn returns_only_error(node: Node, src: &str) -> bool {
    let mut func = node.parent();
    while let Some(f) = func {
        if matches!(
            f.kind(),
            "function_declaration" | "method_declaration" | "func_literal"
        ) {
            break;
        }
        func = f.parent();
    }
    let Some(result) = func.and_then(|f| f.child_by_field_name("result")) else {
        return false;
    };
    let ty = match result.kind() {
        "parameter_list" => {
            let params: Vec<Node> = result.named_children(&mut result.walk()).collect();
            match params[..] {
                [p] if p.children_by_field_name("name", &mut p.walk()).count() <= 1 => {
                    p.child_by_field_name("type")
                }
                _ => None,
            }
        }
        _ => Some(result),
    };
    ty.is_some_and(|t| t.kind() == "type_identifier" && text(t, src) == "error")
}
