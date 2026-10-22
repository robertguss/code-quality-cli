//! Per-language rule queries (KTD4). Each language module owns its rules'
//! registry rows and finds their hits in a parsed tree.

mod elixir;
mod go;
mod python;
mod rust;
mod typescript;

use std::sync::OnceLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor, Tree};

use super::Rule;
use crate::grammars::Lang;

/// The debug-output family, the one rule table that takes `allow` globs.
pub const DEBUG_OUTPUT: &str = "debug-output";

/// One rule match, before scope and `qgate: allow` filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rule: &'static str,
    /// Start byte of the matched node, for test-region checks.
    pub byte: usize,
    /// 1-based line.
    pub line: u32,
}

impl Hit {
    pub fn at(rule: &'static str, node: Node) -> Hit {
        Hit {
            rule,
            byte: node.start_byte(),
            line: node.start_position().row as u32 + 1,
        }
    }
}

/// Every language's rules, in registry order.
pub fn rules() -> impl Iterator<Item = &'static Rule> {
    rust::RULES
        .iter()
        .chain(typescript::RULES)
        .chain(python::RULES)
        .chain(go::RULES)
        .chain(elixir::RULES)
}

/// One parsed file, for the language modules.
pub struct File<'a> {
    pub path: &'a str,
    pub tree: &'a Tree,
    pub src: &'a str,
}

/// Every rule hit in one parsed file, in no particular order.
pub fn hits(lang: Lang, path: &str, tree: &Tree, src: &str) -> Vec<Hit> {
    let file = File { path, tree, src };
    let mut out = Vec::new();
    match lang {
        Lang::Rust => rust::hits(&file, &mut out),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => typescript::hits(lang, &file, &mut out),
        Lang::Python => python::hits(&file, &mut out),
        Lang::Go => go::hits(&file, &mut out),
        Lang::Elixir => elixir::hits(&file, &mut out),
        _ => {}
    }
    out
}

/// Compile `source` for `lang` once, on first use. A query that does not
/// compile is a bug in qgate, so it panics with the grammar's error.
pub(crate) fn query(lang: Lang, cell: &'static OnceLock<Query>, source: &str) -> &'static Query {
    cell.get_or_init(|| {
        Query::new(&lang.language(), source)
            .unwrap_or_else(|e| panic!("{} rule query: {e}", lang.name()))
    })
}

/// Call `f` with each capture's name and node for every match of `query`.
/// Captures whose names start with `_` are helpers and are skipped.
pub(crate) fn each_capture(query: &Query, tree: &Tree, src: &str, mut f: impl FnMut(&str, Node)) {
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());
    while let Some(m) = matches.next() {
        for c in m.captures {
            let name = names[c.index as usize];
            if !name.starts_with('_') {
                f(name, c.node);
            }
        }
    }
}

/// A node's source text.
pub(crate) fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}
