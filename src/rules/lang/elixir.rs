//! Elixir rules. Definitions, attributes and remote calls are all `call`
//! nodes in tree-sitter-elixir, told apart by their target.

use std::collections::HashSet;
use std::sync::OnceLock;

use tree_sitter::{Node, Query};

use super::{File, Hit, each_capture, query, text};
use crate::grammars::Lang;
use crate::rules::{Kind, Rule, Scope};

const ESCAPE: &str = "pstack /correct (escape hatches)";
const SWALLOW: &str = "pstack /correct (swallowed errors)";
const DEBUG: &str = "pstack /correct (debug output)";

const NO_SPEC: &str = "escape-hatch/ex-no-spec";

pub const RULES: &[Rule] = &[
    Rule {
        id: NO_SPEC,
        kind: Kind::Exact,
        language: "elixir",
        source: ESCAPE,
        rationale: "a public function without a `@spec` gives Dialyzer and readers nothing to check calls against",
        fix: "add a `@spec` for the function before its first clause, or make it `defp`",
        since: "0.6.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
    Rule {
        id: "escape-hatch/ex-to-atom",
        kind: Kind::Exact,
        language: "elixir",
        source: ESCAPE,
        rationale: "`String.to_atom/1` on runtime input fills the atom table, which is never garbage collected",
        fix: "use `String.to_existing_atom/1`, or map the known strings to atoms explicitly",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/ex-eval",
        kind: Kind::Exact,
        language: "elixir",
        source: ESCAPE,
        rationale: "`Code.eval_string` and friends run arbitrary code and bypass the compiler",
        fix: "call the function directly, or dispatch on the input with pattern matching",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "swallowed-error/ex-rescue-nil",
        kind: Kind::Exact,
        language: "elixir",
        source: SWALLOW,
        rationale: "a `rescue _ ->` or `catch _, _ ->` that returns a literal throws the error away unseen",
        fix: "rescue the specific exception and handle it, return `{:error, reason}`, or log and `reraise`",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "debug-output/ex-inspect",
        kind: Kind::Exact,
        language: "elixir",
        source: DEBUG,
        rationale: "`IO.inspect` is a leftover debugging aid",
        fix: "remove the `IO.inspect` call, or use `Logger.debug`",
        since: "0.6.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
    Rule {
        id: "debug-output/ex-puts",
        kind: Kind::Exact,
        language: "elixir",
        source: DEBUG,
        rationale: "printing from library code under `lib/` bypasses the application's logging",
        fix: "use `Logger`, or print only from a Mix task or an output module listed in `[rules.debug-output] allow`",
        since: "0.6.0",
        counted: true,
        scope: Scope::Output,
    },
];

const QUERY: &str = r#"
(call
  target: (dot left: (alias) @_module right: (identifier) @_fun)
  (#eq? @_module "String") (#eq? @_fun "to_atom")) @to-atom

(call
  target: (dot left: (alias) @_module right: (identifier) @_fun)
  (#eq? @_module "Code") (#any-of? @_fun "eval_string" "eval_quoted" "eval_file")) @eval

(call
  target: (dot left: (alias) @_module right: (identifier) @_fun)
  (#eq? @_module "IO") (#any-of? @_fun "inspect" "puts")) @io

(rescue_block (stab_clause) @rescue)
(catch_block (stab_clause) @rescue)

(call target: (identifier) @_def (#eq? @_def "defmodule")) @module
"#;

pub fn hits(file: &File, out: &mut Vec<Hit>) {
    static Q: OnceLock<Query> = OnceLock::new();
    let src = file.src;
    each_capture(
        query(Lang::Elixir, &Q, QUERY),
        file.tree,
        src,
        |name, node| {
            let rule = match name {
                "to-atom" if !literal_argument(node) => "escape-hatch/ex-to-atom",
                "eval" => "escape-hatch/ex-eval",
                "io" => match remote_name(node, src) {
                    Some("inspect") => "debug-output/ex-inspect",
                    Some("puts") if under_lib(file.path) => "debug-output/ex-puts",
                    _ => return,
                },
                "rescue" if swallows(node, src) => "swallowed-error/ex-rescue-nil",
                "module" => return unspecced(node, src, out),
                _ => return,
            };
            out.push(Hit::at(rule, node));
        },
    );
}

/// The function name of a remote call such as `IO.puts(...)`.
fn remote_name<'a>(call: Node, src: &'a str) -> Option<&'a str> {
    let dot = call.child_by_field_name("target")?;
    Some(text(dot.child_by_field_name("right")?, src))
}

/// `lib/app.ex` or `apps/web/lib/web.ex`.
fn under_lib(path: &str) -> bool {
    path.starts_with("lib/") || path.contains("/lib/")
}

/// The call's first argument, or the left side of `x |> call()`, is a literal.
fn literal_argument(call: Node) -> bool {
    let first = arguments(call).and_then(|a| a.named_child(0));
    let first = first.or_else(|| {
        let pipe = call.parent().filter(|p| p.kind() == "binary_operator")?;
        let piped = pipe.child_by_field_name("right")? == call;
        pipe.child_by_field_name("left").filter(|_| piped)
    });
    first.is_some_and(|n| matches!(n.kind(), "string" | "charlist" | "atom"))
}

/// A `rescue` or `catch` clause whose patterns are all ignored and whose
/// body is a literal or empty.
fn swallows(clause: Node, src: &str) -> bool {
    let Some(patterns) = clause.child_by_field_name("left") else {
        return false;
    };
    let mut cursor = patterns.walk();
    let ignored = patterns.named_children(&mut cursor).all(|p| {
        let p = match p.kind() {
            "binary_operator" if operator(p, src) == Some("in") => p.child_by_field_name("left"),
            _ => Some(p),
        };
        p.is_some_and(|p| p.kind() == "identifier" && text(p, src).starts_with('_'))
    });
    let body = clause.child_by_field_name("right");
    let literal = match body {
        None => true,
        // `:error` still tells the caller something failed.
        Some(b) => {
            b.named_child_count() == 1
                && b.named_child(0).is_some_and(|n| {
                    matches!(
                        n.kind(),
                        "nil" | "boolean" | "atom" | "quoted_atom" | "integer" | "float"
                    ) && text(n, src) != ":error"
                })
        }
    };
    ignored && literal
}

/// Push a hit for each public function in `module` with no `@spec` for its
/// name and arity earlier in the module, at its first clause. Callbacks
/// marked `@impl true` or `@impl Behaviour` are skipped.
fn unspecced(module: Node, src: &str, out: &mut Vec<Hit>) {
    let Some(body) = do_block(module) else {
        return;
    };
    let mut specs: HashSet<(&str, usize)> = HashSet::new();
    let mut seen: HashSet<(&str, usize)> = HashSet::new();
    let mut callback = false;
    let mut cursor = body.walk();
    for stmt in body.named_children(&mut cursor) {
        if let Some((attr, value)) = attribute(stmt, src) {
            match attr {
                "spec" => specs.extend(value.and_then(|v| spec_head(v, src))),
                "impl" => callback = value.is_some_and(|v| text(v, src) != "false"),
                _ => {}
            }
            continue;
        }
        let Some((kind, head)) = definition(stmt, src) else {
            continue;
        };
        let was_callback = std::mem::take(&mut callback);
        let Some((name, arity, defaults)) = def_head(head, src) else {
            continue;
        };
        let first = (arity - defaults..=arity).all(|a| seen.insert((name, a)));
        let specced = (arity - defaults..=arity).any(|a| specs.contains(&(name, a)));
        if kind == "def" && first && !was_callback && !specced {
            out.push(Hit::at(NO_SPEC, stmt));
        }
    }
}

/// `@name value` as `(name, value)`.
fn attribute<'a, 't>(node: Node<'t>, src: &'a str) -> Option<(&'a str, Option<Node<'t>>)> {
    if node.kind() != "unary_operator" || operator(node, src) != Some("@") {
        return None;
    }
    let operand = node.child_by_field_name("operand")?;
    match operand.kind() {
        "identifier" => Some((text(operand, src), None)),
        "call" => {
            let name = text(operand.child_by_field_name("target")?, src);
            Some((name, arguments(operand).and_then(|a| a.named_child(0))))
        }
        _ => None,
    }
}

/// `def`, `defp` or `defmacro` and its head.
fn definition<'a, 't>(node: Node<'t>, src: &'a str) -> Option<(&'a str, Node<'t>)> {
    if node.kind() != "call" {
        return None;
    }
    let target = node.child_by_field_name("target")?;
    if target.kind() != "identifier" {
        return None;
    }
    let kind = text(target, src);
    kind.starts_with("def")
        .then(|| Some((kind, arguments(node)?.named_child(0)?)))
        .flatten()
}

/// The name, arity and number of default arguments of a def head such as
/// `f(a, b \\ 1) when a > 0`. `None` for a computed name.
fn def_head<'a>(head: Node, src: &'a str) -> Option<(&'a str, usize, usize)> {
    let head = strip_when(head, src)?;
    let (name, args) = name_and_args(head, src)?;
    let defaults = args
        .iter()
        .filter(|a| a.kind() == "binary_operator" && operator(**a, src) == Some("\\\\"))
        .count();
    Some((name, args.len(), defaults))
}

/// The name and arity a `@spec name(types) :: type` describes.
fn spec_head<'a>(spec: Node, src: &'a str) -> Option<(&'a str, usize)> {
    let spec = strip_when(spec, src)?;
    if spec.kind() != "binary_operator" || operator(spec, src) != Some("::") {
        return None;
    }
    let (name, args) = name_and_args(spec.child_by_field_name("left")?, src)?;
    Some((name, args.len()))
}

/// `name` or `name(args)`.
fn name_and_args<'a, 't>(head: Node<'t>, src: &'a str) -> Option<(&'a str, Vec<Node<'t>>)> {
    match head.kind() {
        "identifier" => Some((text(head, src), Vec::new())),
        "call" => {
            let target = head.child_by_field_name("target")?;
            if target.kind() != "identifier" {
                return None;
            }
            let args = arguments(head).map_or_else(Vec::new, |a| {
                let mut cursor = a.walk();
                a.named_children(&mut cursor).collect()
            });
            Some((text(target, src), args))
        }
        _ => None,
    }
}

/// The left side of `x when guard`, or `x` itself.
fn strip_when<'t>(node: Node<'t>, src: &str) -> Option<Node<'t>> {
    match node.kind() {
        "binary_operator" if operator(node, src) == Some("when") => {
            node.child_by_field_name("left")
        }
        _ => Some(node),
    }
}

fn operator<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    node.child_by_field_name("operator").map(|op| text(op, src))
}

fn arguments(call: Node) -> Option<Node> {
    child_of_kind(call, "arguments")
}

fn do_block(call: Node) -> Option<Node> {
    child_of_kind(call, "do_block")
}

fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|c| c.kind() == kind)
}
