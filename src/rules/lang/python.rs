//! Python rules.

use super::prelude::*;

pub const RULES: &[Rule] = &[
    Rule {
        id: "escape-hatch/py-any",
        kind: Kind::Exact,
        language: "python",
        source: ESCAPE,
        rationale: "an `Any` annotation turns off type checking for everything it touches",
        fix: "annotate the real type, a `Protocol`, a `TypeVar` or `object`",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/py-cast",
        kind: Kind::Exact,
        language: "python",
        source: ESCAPE,
        rationale: "`typing.cast` tells the type checker to trust an unchecked claim",
        fix: "narrow the type with `isinstance` or a `TypeGuard`, or fix the source's annotation",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "escape-hatch/py-unannotated-public",
        kind: Kind::Exact,
        language: "python",
        source: ESCAPE,
        rationale: "an unannotated parameter of a public function is silently `Any` to callers",
        fix: "annotate every parameter of public functions and methods",
        since: "0.6.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
    Rule {
        id: "swallowed-error/py-bare-pass",
        kind: Kind::Exact,
        language: "python",
        source: SWALLOW,
        rationale: "`except:` or `except Exception:` with only `pass` hides every error, bugs included",
        fix: "catch the specific exception you expect, and handle, log or re-raise it",
        since: "0.6.0",
        counted: true,
        scope: Scope::Everywhere,
    },
    Rule {
        id: "debug-output/py-print",
        kind: Kind::Exact,
        language: "python",
        source: DEBUG,
        rationale: "printing from library code bypasses the program's output and logging",
        fix: "return the value or use `logging`; print only from `__main__.py`, scripts or an output module listed in `[rules.debug-output] allow`",
        since: "0.6.0",
        counted: true,
        scope: Scope::Output,
    },
    Rule {
        id: "debug-output/py-breakpoint",
        kind: Kind::Exact,
        language: "python",
        source: DEBUG,
        rationale: "`breakpoint()` and `pdb.set_trace()` are leftover debugging aids that stop the program",
        fix: "remove the breakpoint",
        since: "0.6.0",
        counted: true,
        scope: Scope::OutsideTests,
    },
];

const QUERY: &str = r#"
((identifier) @any (#eq? @any "Any"))

(call function: (identifier) @call)

(call
  function: (attribute
    object: (identifier) @_object
    attribute: (identifier) @attr-call)
  (#any-of? @_object "typing" "pdb"))

(import_from_statement
  module_name: (dotted_name) @_module
  name: (dotted_name) @cast-import
  (#eq? @_module "typing")
  (#eq? @cast-import "cast"))

(except_clause) @except

(function_definition) @def
"#;

pub fn hits(file: &File, out: &mut Vec<Hit>) {
    static Q: OnceLock<Query> = OnceLock::new();
    let src = file.src;
    let mut cast_imported = false;
    let mut bare_casts = Vec::new();
    each_capture(
        query(Lang::Python, &Q, QUERY),
        file.tree,
        src,
        |name, node| {
            let rule = match name {
                "any" if is_any_annotation(node, src) => "escape-hatch/py-any",
                "call" => match text(node, src) {
                    "print" if !under_main_guard(node, src) => "debug-output/py-print",
                    "breakpoint" => "debug-output/py-breakpoint",
                    "cast" => {
                        bare_casts.push(Hit::at("escape-hatch/py-cast", node));
                        return;
                    }
                    _ => return,
                },
                "attr-call" => match qualified(node, src) {
                    Some("typing.cast") => "escape-hatch/py-cast",
                    Some("pdb.set_trace") => "debug-output/py-breakpoint",
                    _ => return,
                },
                "cast-import" => {
                    cast_imported = true;
                    return;
                }
                "except" if swallows(node, src) => "swallowed-error/py-bare-pass",
                "def" if is_public_api(node, src) && has_unannotated_param(node, src) => {
                    "escape-hatch/py-unannotated-public"
                }
                _ => return,
            };
            out.push(Hit::at(rule, node));
        },
    );
    if cast_imported {
        out.extend(bare_casts);
    }
}

/// `object.attribute` text for the attribute name `node` of an attribute call.
/// Inside `if __name__ == "__main__":`, where a module runs as a script.
fn under_main_guard(node: Node, src: &str) -> bool {
    let mut at = node.parent();
    while let Some(n) = at {
        if n.kind() == "if_statement"
            && n.child_by_field_name("condition").is_some_and(|c| {
                let t: String = text(c, src)
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                t.replace('\'', "\"") == "__name__==\"__main__\""
            })
        {
            return true;
        }
        at = n.parent();
    }
    false
}

fn qualified<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    node.parent().map(|a| text(a, src))
}

/// `Any` or `typing.Any` inside a type annotation.
fn is_any_annotation(ident: Node, src: &str) -> bool {
    if let Some(parent) = ident.parent().filter(|p| p.kind() == "attribute") {
        let is_attr = parent.child_by_field_name("attribute") == Some(ident);
        let typing = parent
            .child_by_field_name("object")
            .is_some_and(|o| text(o, src) == "typing");
        if !(is_attr && typing) {
            return false;
        }
    }
    std::iter::successors(ident.parent(), |n| n.parent()).any(|n| n.kind() == "type")
}

/// A bare, `Exception` or `BaseException` handler whose body is only `pass` or `...`.
fn swallows(clause: Node, src: &str) -> bool {
    let mut cursor = clause.walk();
    let mut caught = None;
    let mut body = None;
    for child in clause.named_children(&mut cursor) {
        match child.kind() {
            "block" => body = Some(child),
            "comment" => {}
            _ => caught = Some(child),
        }
    }
    let caught = match caught {
        Some(c) if c.kind() == "as_pattern" => c.named_child(0),
        c => c,
    };
    let broad = caught.is_none_or(|c| matches!(text(c, src), "Exception" | "BaseException"));
    broad && body.is_some_and(|b| only_placeholders(b, src))
}

/// A block holding nothing but `pass`, `...` and comments.
fn only_placeholders(block: Node, src: &str) -> bool {
    let mut cursor = block.walk();
    block.named_children(&mut cursor).all(|s| match s.kind() {
        "pass_statement" | "comment" => true,
        "expression_statement" => text(s, src) == "...",
        _ => false,
    })
}

/// A public module-level function or a public method directly in a class body.
fn is_public_api(def: Node, src: &str) -> bool {
    let public = def
        .child_by_field_name("name")
        .is_some_and(|n| is_public_name(text(n, src)));
    public && container(def).is_some()
}

/// Dunder names are public; any other leading underscore is private.
fn is_public_name(name: &str) -> bool {
    !name.starts_with('_') || (name.starts_with("__") && name.ends_with("__") && name.len() > 4)
}

/// Where `def` sits: `Some(false)` at module level, `Some(true)` in a class
/// body, `None` anywhere else (nested in a function).
fn container(def: Node) -> Option<bool> {
    let mut parent = def.parent()?;
    if parent.kind() == "decorated_definition" {
        parent = parent.parent()?;
    }
    match parent.kind() {
        "module" => Some(false),
        "block" => (parent.parent()?.kind() == "class_definition").then_some(true),
        _ => None,
    }
}

/// Any parameter without a type, ignoring a method's leading `self` or `cls`.
fn has_unannotated_param(def: Node, src: &str) -> bool {
    let Some(params) = def.child_by_field_name("parameters") else {
        return false;
    };
    let method = container(def) == Some(true);
    let mut cursor = params.walk();
    params
        .named_children(&mut cursor)
        .enumerate()
        .any(|(i, p)| match p.kind() {
            "identifier" => !(method && i == 0 && matches!(text(p, src), "self" | "cls")),
            "default_parameter"
            | "list_splat_pattern"
            | "dictionary_splat_pattern"
            | "tuple_pattern" => true,
            _ => false,
        })
}
