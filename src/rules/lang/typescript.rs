//! TypeScript, TSX and JavaScript rules. The three are separate grammars, and
//! `Query::new` rejects node kinds a grammar lacks, so each gets its own query.

use std::sync::OnceLock;

use tree_sitter::{Node, Query};

use super::{File, Hit, each_capture, query, text};
use crate::grammars::Lang;
use crate::rules::{Kind, Rule, Scope};

const ESCAPE: &str = "pstack /correct (escape hatches)";
const SWALLOW: &str = "pstack /correct (swallowed errors)";
const DEBUG: &str = "pstack /correct (debug output)";

pub const RULES: &[Rule] = &[
    Rule {
        id: "escape-hatch/as-cast",
        kind: Kind::Exact,
        language: "typescript",
        source: ESCAPE,
        rationale: "`x as T` tells the compiler to trust a type it cannot check",
        fix: "narrow the value with a type guard or validate it at the boundary; `as const` is fine",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/any",
        kind: Kind::Exact,
        language: "typescript",
        source: ESCAPE,
        rationale: "`any` turns off type checking for everything it touches",
        fix: "use `unknown` and narrow it, or name the real type",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/non-null",
        kind: Kind::Exact,
        language: "typescript",
        source: ESCAPE,
        rationale: "`x!` asserts a value is not null without checking it",
        fix: "check for `null` or `undefined`, or use `?.` and `??`",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/angle-cast",
        kind: Kind::Exact,
        language: "typescript",
        source: ESCAPE,
        rationale: "`<T>x` tells the compiler to trust a type it cannot check",
        fix: "narrow the value with a type guard or validate it at the boundary",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/function-object-type",
        kind: Kind::Exact,
        language: "typescript",
        source: ESCAPE,
        rationale: "the `Function` type accepts any callable and checks no arguments or result",
        fix: "write the signature, such as `(x: number) => string`",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "swallowed-error/empty-catch",
        kind: Kind::Exact,
        language: "typescript, javascript",
        source: SWALLOW,
        rationale: "a `catch` with an empty or comment-only body hides the error",
        fix: "handle the error, rethrow it, or let it propagate",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "swallowed-error/log-only-catch",
        kind: Kind::Heuristic,
        language: "typescript, javascript",
        source: SWALLOW,
        rationale: "a `catch` that only logs carries on as if the error never happened",
        fix: "rethrow after logging, return an error result, or recover explicitly",
        since: "0.5.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "debug-output/console-log",
        kind: Kind::Exact,
        language: "typescript, javascript",
        source: DEBUG,
        rationale: "`console.log` and `console.debug` are leftover debugging output",
        fix: "remove the call or use the project's logger; print only from `bin/`, `scripts/` or an output module listed in `[rules.debug-output] allow`",
        since: "0.5.0",
        counted: true,
        scope: Scope::Output,
    },
    Rule {
        id: "debug-output/debugger",
        kind: Kind::Exact,
        language: "typescript, javascript",
        source: DEBUG,
        rationale: "a `debugger` statement is a leftover debugging aid",
        fix: "remove the `debugger` statement",
        since: "0.5.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
];

/// Queries every grammar shares: catches, console calls and `debugger`.
macro_rules! common {
    () => {
        r#"
(catch_clause body: (statement_block) @_body) @catch

(call_expression
  function: (member_expression
    object: (identifier) @_obj
    property: (property_identifier) @_prop)
  (#eq? @_obj "console")
  (#any-of? @_prop "log" "debug")) @console-log

(debugger_statement) @debugger
"#
    };
}

/// Type-level escape hatches, in the TypeScript and TSX grammars.
macro_rules! typed {
    () => {
        r#"
(as_expression) @as-cast

(non_null_expression) @non-null

((predefined_type) @any (#eq? @any "any"))

((type_identifier) @function-type (#eq? @function-type "Function"))
"#
    };
}

const JS_QUERY: &str = common!();
const TSX_QUERY: &str = concat!(common!(), typed!());
/// `<T>x` is a `type_assertion` only in the `.ts` grammar; TSX reads it as JSX.
const TS_QUERY: &str = concat!(common!(), typed!(), "(type_assertion) @angle-cast\n");

pub fn hits(lang: Lang, file: &File, out: &mut Vec<Hit>) {
    static TS: OnceLock<Query> = OnceLock::new();
    static TSX: OnceLock<Query> = OnceLock::new();
    static JS: OnceLock<Query> = OnceLock::new();
    let q = match lang {
        Lang::TypeScript => query(lang, &TS, TS_QUERY),
        Lang::Tsx => query(lang, &TSX, TSX_QUERY),
        Lang::JavaScript => query(lang, &JS, JS_QUERY),
        _ => return,
    };
    let src = file.src;
    each_capture(q, file.tree, src, |name, node| {
        let rule = match name {
            "as-cast" if !is_as_const(node, src) => "escape-hatch/as-cast",
            "non-null" => "escape-hatch/non-null",
            "any" => "escape-hatch/any",
            "angle-cast" => "escape-hatch/angle-cast",
            "function-type" if !is_declared_name(node) => "escape-hatch/function-object-type",
            "catch" => match catch_kind(node, src) {
                Some(rule) => rule,
                None => return,
            },
            "console-log" => "debug-output/console-log",
            "debugger" => "debug-output/debugger",
            _ => return,
        };
        out.push(Hit::at(rule, node));
    });
}

/// `x as const`, which narrows a literal instead of overriding a type.
fn is_as_const(cast: Node, src: &str) -> bool {
    let count = cast.child_count();
    count > 0
        && cast
            .child(count - 1)
            .is_some_and(|t| text(t, src) == "const")
}

/// The name of a declaration such as `interface Function {}`, not a use of the type.
fn is_declared_name(ident: Node) -> bool {
    ident.parent().is_some_and(|p| {
        p.kind().ends_with("_declaration") && p.child_by_field_name("name") == Some(ident)
    })
}

/// Which swallowed-error rule a `catch` clause hits, if any.
fn catch_kind(clause: Node, src: &str) -> Option<&'static str> {
    let body = clause.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let stmts: Vec<Node> = body
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect();
    if stmts.is_empty() {
        Some("swallowed-error/empty-catch")
    } else if stmts.iter().all(|s| is_log_call(*s, src)) {
        Some("swallowed-error/log-only-catch")
    } else {
        None
    }
}

/// A statement that is only a call such as `console.error(e)` or `logger.warn(e)`.
fn is_log_call(stmt: Node, src: &str) -> bool {
    let call = (stmt.kind() == "expression_statement")
        .then(|| stmt.named_child(0))
        .flatten()
        .filter(|c| c.kind() == "call_expression");
    let object = call
        .and_then(|c| c.child_by_field_name("function"))
        .filter(|f| f.kind() == "member_expression")
        .and_then(|f| f.child_by_field_name("object"));
    object.is_some_and(|o| {
        let name = text(o, src);
        let last = name.rsplit('.').next().unwrap_or(name);
        matches!(last, "console" | "logger" | "log")
    })
}
