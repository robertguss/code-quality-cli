//! Rust rules.

use std::sync::OnceLock;

use tree_sitter::Node;

use super::{File, Hit, Kinds, each_node, text};
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

const NUMERIC: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32",
    "f64",
];

/// The node kinds `hits` looks at.
const WATCHED: &[&str] = &[
    "call_expression",
    "expression_statement",
    "unsafe_block",
    "macro_invocation",
    "type_cast_expression",
    "let_declaration",
];

/// Rust's rules are node kinds plus small predicates, so a tree walk finds
/// them faster than a query, which matters for qgate's own large Rust tree.
pub fn hits(file: &File, out: &mut Vec<Hit>) {
    static KINDS: OnceLock<Kinds> = OnceLock::new();
    let kinds = KINDS.get_or_init(|| Kinds::new(Lang::Rust, WATCHED));
    let src = file.src;
    each_node(file.tree, |node| {
        if !kinds.contains(node.kind_id()) {
            return;
        }
        let found = match node.kind() {
            "call_expression" => method_call(node, src),
            "expression_statement" => node
                .named_child(0)
                .filter(|c| c.kind() == "call_expression" && method_name(*c, src) == Some("ok"))
                .filter(|c| no_arguments(*c))
                .map(|c| ("swallowed-error/rust-ok-statement", c)),
            "unsafe_block" => Some(("escape-hatch/rust-unsafe", node)),
            "macro_invocation" => macro_rule(node, src),
            "type_cast_expression" => node
                .child_by_field_name("type")
                .filter(|t| t.kind() == "primitive_type" && NUMERIC.contains(&text(*t, src)))
                .map(|t| ("escape-hatch/rust-numeric-cast", t)),
            "let_declaration" if discards_call(node) => {
                Some(("swallowed-error/let-underscore", node))
            }
            _ => None,
        };
        if let Some((rule, at)) = found {
            out.push(Hit::at(rule, at));
        }
    });
}

/// The method name of `x.name(...)`.
fn method_name<'t>(call: Node<'t>, src: &'t str) -> Option<&'t str> {
    let f = call.child_by_field_name("function")?;
    let name = (f.kind() == "field_expression").then(|| f.child_by_field_name("field"))??;
    Some(text(name, src))
}

fn method_call<'t>(call: Node<'t>, src: &str) -> Option<(&'static str, Node<'t>)> {
    let rule = match method_name(call, src)? {
        "unwrap" | "expect" => "escape-hatch/rust-unwrap",
        "unwrap_or_default" => "swallowed-error/unwrap-or-default",
        _ => return None,
    };
    let name = call
        .child_by_field_name("function")?
        .child_by_field_name("field")?;
    Some((rule, name))
}

fn macro_rule<'t>(node: Node<'t>, src: &str) -> Option<(&'static str, Node<'t>)> {
    let m = node.child_by_field_name("macro")?;
    let name = match m.kind() {
        "scoped_identifier" => m.child_by_field_name("name")?,
        _ => m,
    };
    let rule = match text(name, src) {
        "panic" | "todo" | "unimplemented" | "unreachable" => "escape-hatch/rust-panic",
        "dbg" => "debug-output/rust-dbg",
        "println" | "eprintln" | "print" | "eprint" => "debug-output/rust-println",
        _ => return None,
    };
    Some((rule, name))
}

/// `x.ok()`, not `x.ok(y)`.
fn no_arguments(call: Node) -> bool {
    call.child_by_field_name("arguments")
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
