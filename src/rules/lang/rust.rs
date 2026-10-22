//! Rust rules.

use std::sync::OnceLock;

use tree_sitter::{Node, Query, Tree};

use super::{Hit, each_capture, query, text};
use crate::grammars::Lang;
use crate::rules::{Kind, Rule, Scope};

const ESCAPE: &str = "pstack /correct (escape hatches)";
const SWALLOW: &str = "pstack /correct (swallowed errors)";
const DEBUG: &str = "pstack /correct (debug output)";

pub const RULES: &[Rule] = &[
    Rule {
        id: "escape-hatch/rust-unwrap",
        kind: Kind::Exact,
        language: "rust",
        source: ESCAPE,
        rationale: "`.unwrap()` and `.expect()` turn a recoverable error into a crash",
        fix: "return the error with `?`, or handle it with `match` or `unwrap_or`",
        since: "0.5.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
    Rule {
        id: "escape-hatch/rust-unsafe",
        kind: Kind::Exact,
        language: "rust",
        source: ESCAPE,
        rationale: "an `unsafe` block opts out of the borrow checker's guarantees",
        fix: "use a safe API, or wrap the block in a small safe function with a `// SAFETY:` comment",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/rust-panic",
        kind: Kind::Exact,
        language: "rust",
        source: ESCAPE,
        rationale: "`panic!`, `todo!`, `unimplemented!` and `unreachable!` crash instead of reporting an error",
        fix: "return an error the caller can handle, or finish the code path",
        since: "0.5.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
    Rule {
        id: "escape-hatch/rust-numeric-cast",
        kind: Kind::Heuristic,
        language: "rust",
        source: ESCAPE,
        rationale: "`as` between numeric types truncates or wraps silently",
        fix: "use `From`, `TryFrom` or `try_into()`, or keep `as` where truncation is intended",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "swallowed-error/rust-ok-statement",
        kind: Kind::Exact,
        language: "rust",
        source: SWALLOW,
        rationale: "`.ok();` as a statement throws an error away unseen",
        fix: "propagate it with `?`, handle it, or log it before dropping it",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "swallowed-error/let-underscore",
        kind: Kind::Heuristic,
        language: "rust",
        source: SWALLOW,
        rationale: "`let _ = call()` drops the call's result, which may be an error",
        fix: "handle the `Result`, or bind it to a named `_reason` variable that says why it can be ignored",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "swallowed-error/unwrap-or-default",
        kind: Kind::Heuristic,
        language: "rust",
        source: SWALLOW,
        rationale: "`.unwrap_or_default()` on a `Result` hides the error behind a default value",
        fix: "handle the error, or use it only on an `Option` where a default is correct",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "debug-output/rust-dbg",
        kind: Kind::Exact,
        language: "rust",
        source: DEBUG,
        rationale: "`dbg!` is a leftover debugging aid",
        fix: "remove the `dbg!` call",
        since: "0.5.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
    Rule {
        id: "debug-output/rust-println",
        kind: Kind::Exact,
        language: "rust",
        source: DEBUG,
        rationale: "printing from library code bypasses the program's output and logging",
        fix: "return the value or log it; print only from a binary's main file or an output module listed in `[rules.debug-output] allow`",
        since: "0.5.0",
        counted: true,
        scope: Scope::Output,
    },
];

const QUERY: &str = r#"
(call_expression
  function: (field_expression
    field: (field_identifier) @method)
  arguments: (arguments) @_args)

(expression_statement
  (call_expression
    function: (field_expression
      field: (field_identifier) @ok-statement)
    arguments: (arguments) @_args)
  (#eq? @ok-statement "ok"))

(unsafe_block) @unsafe

(macro_invocation macro: (identifier) @macro)
(macro_invocation macro: (scoped_identifier name: (identifier) @macro))

(type_cast_expression type: (primitive_type) @cast)

(let_declaration value: (_) @_value) @let
"#;

const NUMERIC: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32",
    "f64",
];

pub fn hits(tree: &Tree, src: &str, out: &mut Vec<Hit>) {
    static Q: OnceLock<Query> = OnceLock::new();
    each_capture(query(Lang::Rust, &Q, QUERY), tree, src, |name, node| {
        let rule = match name {
            "method" => match text(node, src) {
                "unwrap" | "expect" => "escape-hatch/rust-unwrap",
                "unwrap_or_default" => "swallowed-error/unwrap-or-default",
                _ => return,
            },
            "ok-statement" if no_arguments(node) => "swallowed-error/rust-ok-statement",
            "unsafe" => "escape-hatch/rust-unsafe",
            "macro" => match text(node, src) {
                "panic" | "todo" | "unimplemented" | "unreachable" => "escape-hatch/rust-panic",
                "dbg" => "debug-output/rust-dbg",
                "println" | "eprintln" | "print" | "eprint" => "debug-output/rust-println",
                _ => return,
            },
            "cast" if NUMERIC.contains(&text(node, src)) => "escape-hatch/rust-numeric-cast",
            "let" if discards_call(node) => "swallowed-error/let-underscore",
            _ => return,
        };
        out.push(Hit::at(rule, node));
    });
}

/// `x.ok()`, not `x.ok(y)`; `node` is the method name.
fn no_arguments(method: Node) -> bool {
    let call = method.parent().and_then(|f| f.parent());
    call.and_then(|c| c.child_by_field_name("arguments"))
        .is_some_and(|a| a.named_child_count() == 0)
}

/// `let _ = f(...)` or `let _ = f(...).await`.
fn discards_call(stmt: Node) -> bool {
    let wildcard = stmt
        .child_by_field_name("pattern")
        .is_some_and(|p| p.kind() == "_");
    let value = stmt.child_by_field_name("value");
    let value = match value {
        Some(v) if v.kind() == "await_expression" => v.named_child(0),
        v => v,
    };
    wildcard && value.is_some_and(|v| v.kind() == "call_expression")
}
