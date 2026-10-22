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

/// Each family's source in Cursor's pstack plugin, shared by every language.
const ESCAPE: &str = "pstack /correct (escape hatches)";
const SWALLOW: &str = "pstack /correct (swallowed errors)";
const DEBUG: &str = "pstack /correct (debug output)";

/// What every language module uses.
mod prelude {
    pub(super) use std::sync::OnceLock;

    pub(super) use tree_sitter::{Node, Query};

    pub(super) use super::{
        DEBUG, ESCAPE, File, Hit, Kinds, SWALLOW, each_capture, each_node, query, text,
    };
    pub(super) use crate::grammars::Lang;
    pub(super) use crate::rules::{Kind, Rule, Scope};
}

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

/// Call `f` with every named node in `tree`, in document order. Cheaper
/// than a query when a rule is a node kind plus a small predicate.
pub(crate) fn each_node<'t>(tree: &'t Tree, mut f: impl FnMut(Node<'t>)) {
    let mut cursor = tree.walk();
    loop {
        if cursor.node().is_named() {
            f(cursor.node());
        }
        if cursor.goto_first_child() || cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                return;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// A set of node kinds by id, so a walk skips other nodes without
/// comparing kind names.
pub(crate) struct Kinds(Vec<bool>);

impl Kinds {
    pub(crate) fn new(lang: Lang, kinds: &[&str]) -> Kinds {
        let language = lang.language();
        let mut set = vec![false; language.node_kind_count()];
        for k in kinds {
            let id = language.id_for_node_kind(k, true);
            assert!(id != 0, "{} has no node kind `{k}`", lang.name());
            set[id as usize] = true;
        }
        Kinds(set)
    }

    pub(crate) fn contains(&self, id: u16) -> bool {
        self.0.get(id as usize).copied().unwrap_or(false)
    }
}

/// A node's source text.
pub(crate) fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}
