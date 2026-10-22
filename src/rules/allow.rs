//! `qgate: allow <rule> -- reason: <text>, until: YYYY-MM-DD` (KTD10): a
//! comment marker that takes one occurrence out of a rule's count, on its own
//! line or, when the comment has the line to itself, the line below. Only a
//! marker that opens its comment counts, the same test the guard uses to judge
//! its reason and expiry (R6), so prose that mentions the marker is neither
//! judged nor honored.

use std::collections::HashMap;

use tree_sitter::Node;

use super::{Rule, covers};

pub const MARKER: &str = "qgate: allow";

/// The rule ids and families a line's marker names, in order. Names are
/// separated by commas or spaces and end at `--` or the end of the line.
pub fn names(line: &str) -> Vec<&str> {
    let Some(at) = line.find(MARKER) else {
        return Vec::new();
    };
    let rest = &line[at + MARKER.len()..];
    let rest = rest.split_once("--").map_or(rest, |(names, _)| names);
    rest.split(|c: char| c == ',' || c.is_whitespace())
        .map(|n| n.trim_end_matches("*/").trim())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Whether `line` carries a marker naming `rule` or its family.
pub fn allows(line: &str, rule: &Rule) -> bool {
    names(line).iter().any(|n| covers(n, rule))
}

/// Lines where one of `patterns` opens a comment, as a directive does
/// (`// eslint-disable-next-line`, `x = 1  # noqa`), by 0-based index, each
/// with whether the comment has the line to itself. A pattern in the middle
/// of prose or in backticks is documentation, not a directive.
pub fn directives<P: AsRef<str>>(root: Node, src: &str, patterns: &[P]) -> HashMap<usize, bool> {
    let mut out = HashMap::new();
    let mut offset = 0;
    for (i, line) in src.split_inclusive('\n').enumerate() {
        for p in patterns.iter().map(AsRef::as_ref) {
            for (at, _) in line.match_indices(p) {
                if let Some(comment) = directive_comment(root, src, offset, offset + at, p.len()) {
                    let own_line = src[offset..comment.max(offset)].trim().is_empty();
                    let e = out.entry(i).or_insert(false);
                    *e |= own_line;
                }
            }
        }
        offset += line.len();
    }
    out
}

/// The start byte of the comment that `src[start..start + len]` opens, if any.
fn directive_comment(
    root: Node,
    src: &str,
    line_start: usize,
    start: usize,
    len: usize,
) -> Option<usize> {
    let mut node = root.descendant_for_byte_range(start, start + len);
    while let Some(n) = node {
        if n.kind().contains("comment") {
            let from = n.start_byte().max(line_start);
            return opens_comment(&src[from..start]).then_some(n.start_byte());
        }
        node = n.parent();
    }
    None
}

/// Only comment punctuation and spaces come before the marker.
fn opens_comment(before: &str) -> bool {
    before
        .chars()
        .all(|c| c.is_whitespace() || "/*#!-%;<{".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammars;

    #[test]
    fn markers_open_their_comment() {
        let pats = ["eslint-disable", MARKER];
        let src = "a(); // eslint-disable-line no-undef\n/* eslint-disable */\n/// An `eslint-disable` with no names\n//! `qgate: allow <rule>` grammar\n// a qgate: allow mention\n";
        let parsed = grammars::parse(grammars::Lang::Rust, src).unwrap();
        let got = directives(parsed.tree.root_node(), src, &pats);
        assert_eq!(got, HashMap::from([(0, false), (1, true)]));
    }
}
