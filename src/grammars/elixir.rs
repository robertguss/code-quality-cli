//! Elixir has no function or module node kinds: `def`, `defmodule` and
//! ExUnit's `test` are all `call` nodes, told apart by their target.

use tree_sitter::Node;

use super::{Role, text};

/// Calls measured as functions. Each clause of a multi-clause function is
/// measured on its own.
const FUNCTIONS: &[&str] = &[
    "def",
    "defp",
    "defmacro",
    "defmacrop",
    "defguard",
    "defguardp",
];

/// Calls whose first argument names the functions inside them.
const CONTAINERS: &[&str] = &["defmodule", "defprotocol", "defimpl"];

/// ExUnit blocks, named by their string: `test` is measured, `describe` contains.
const TEST: &str = "test";
const DESCRIBE: &str = "describe";

/// Name for a definition whose name is computed, such as `def unquote(name)()`,
/// so metaprogramming can't hide a long function from the check.
const DYNAMIC: &str = "<dynamic>";

pub(super) fn role(node: Node, src: &str) -> Option<Role> {
    if node.kind() != "call" {
        return None;
    }
    let target = node.child_by_field_name("target")?;
    if target.kind() != "identifier" {
        return None;
    }
    let args = child_of_kind(node, "arguments");
    let first = args.and_then(|a| a.named_child(0));
    match text(target, src) {
        t if FUNCTIONS.contains(&t) => {
            let (name, name_node) = match first.and_then(|f| def_name(f, src)) {
                Some((name, id)) => (name, Some(id)),
                None => (DYNAMIC.to_string(), None),
            };
            Some(Role::Function { name, name_node })
        }
        "defimpl" => {
            let protocol = text(first?, src);
            let name = match args.and_then(|a| keyword(a, "for", src)) {
                Some(ty) => format!("{protocol}.{ty}"),
                None => protocol.to_string(),
            };
            Some(Role::Container(name))
        }
        t if CONTAINERS.contains(&t) => Some(Role::Container(text(first?, src).to_string())),
        TEST => Some(Role::Function {
            name: string_name(first?, src)?,
            name_node: first.map(|f| f.id()),
        }),
        DESCRIBE => Some(Role::Container(string_name(first?, src)?)),
        _ => None,
    }
}

/// The name of `def name(args)`, `def name(args) when guard`, or `def name`.
fn def_name(head: Node, src: &str) -> Option<(String, usize)> {
    let head = match head.kind() {
        "binary_operator"
            if head
                .child_by_field_name("operator")
                .is_some_and(|op| text(op, src) == "when") =>
        {
            head.child_by_field_name("left")?
        }
        _ => head,
    };
    let name = match head.kind() {
        "identifier" => head,
        "call" => head.child_by_field_name("target")?,
        _ => return None,
    };
    (name.kind() == "identifier").then(|| (text(name, src).to_string(), name.id()))
}

/// A `test` or `describe` name: the string's contents, on one line.
fn string_name(node: Node, src: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let raw = text(node, src);
    let inner = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"'));
    Some(
        inner
            .unwrap_or(raw)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The value of keyword `key:` in an argument list, such as `for: Type`.
fn keyword<'a>(args: Node, key: &str, src: &'a str) -> Option<&'a str> {
    let keywords = child_of_kind(args, "keywords")?;
    let mut cursor = keywords.walk();
    let pairs: Vec<_> = keywords.named_children(&mut cursor).collect();
    pairs.into_iter().find_map(|pair| {
        let k = text(pair.child_by_field_name("key")?, src);
        (k.trim_end().trim_end_matches(':') == key)
            .then(|| pair.child_by_field_name("value").map(|v| text(v, src)))
            .flatten()
    })
}

fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|c| c.kind() == kind)
}
