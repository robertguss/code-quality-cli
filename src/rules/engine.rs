//! The rules engine: run each enabled rule over one parsed file and keep the
//! occurrences that count, after scope, entry-point, `allow` glob and
//! `qgate: allow` filtering (KTD4, KTD8 to KTD10).

use std::ops::Range;

use globset::{Glob, GlobSet, GlobSetBuilder};
use tree_sitter::{Node, Tree};

use super::{Level, Rule, Scope, allow, entry, find, lang, level_for};
use crate::classify::{Role, classify};
use crate::config::Config;
use crate::grammars::Lang;

/// One counted occurrence of a rule in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub rule: &'static str,
    pub line: u32,
    /// The occurrence's line, trimmed, to tell new occurrences from moved ones.
    pub text: String,
}

/// The rules a run counts: every registered occurrence rule not set to `off`.
#[derive(Clone, Debug)]
pub struct RuleSet {
    enabled: Vec<&'static Rule>,
    /// `[rules.debug-output] allow` globs.
    output_allow: GlobSet,
}

impl RuleSet {
    /// Count nothing, for callers that only want lengths.
    pub fn none() -> RuleSet {
        RuleSet {
            enabled: Vec::new(),
            output_allow: GlobSet::empty(),
        }
    }

    pub fn from_config(config: &Config) -> RuleSet {
        let enabled = super::all()
            .iter()
            .copied()
            .filter(|r| r.counted && level_for(config, r) != Level::Off)
            .collect();
        let mut b = GlobSetBuilder::new();
        for g in config.output_globs() {
            // Config::validate has already rejected bad globs.
            if let Ok(g) = Glob::new(g) {
                b.add(g);
            }
        }
        RuleSet {
            enabled,
            output_allow: b.build().unwrap_or_else(|_| GlobSet::empty()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.enabled.is_empty()
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.iter().any(|r| r.id == id)
    }
}

/// The occurrences of every enabled rule in one parsed file, by line.
pub fn scan(path: &str, lang: Lang, tree: &Tree, src: &str, rules: &RuleSet) -> Vec<Occurrence> {
    if rules.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<_> = lang::hits(lang, path, tree, src)
        .into_iter()
        .filter(|h| rules.is_enabled(h.rule))
        .collect();
    // Two query patterns can capture the same node; two nodes on one line both count.
    hits.sort_by_key(|h| (h.byte, h.rule));
    hits.dedup();
    if hits.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = src.lines().collect();
    let line_text = |n: u32| lines.get(n as usize - 1).copied().unwrap_or("");
    let test_file = classify(path) != Role::Production || is_rust_test_file(path, lang);
    let mut test_regions: Option<Vec<Range<usize>>> = None;
    let mut entry_point: Option<bool> = None;
    let mut out: Vec<Occurrence> = Vec::new();
    for h in hits {
        let Some(rule) = find(h.rule) else { continue };
        if rule.scope != Scope::Everywhere {
            if test_file {
                continue;
            }
            let regions = test_regions.get_or_insert_with(|| match lang {
                Lang::Rust => rust_test_regions(tree, src),
                _ => Vec::new(),
            });
            if regions.iter().any(|r| r.contains(&h.byte)) {
                continue;
            }
        }
        if rule.scope == Scope::Output
            && (*entry_point.get_or_insert_with(|| entry::is_entry(path, lang, tree, src))
                || rules.output_allow.is_match(path))
        {
            continue;
        }
        let text = line_text(h.line);
        let above = h
            .line
            .checked_sub(1)
            .filter(|n| *n > 0)
            .map_or("", line_text);
        if allow::allows(text, rule) || allow::allows(above, rule) {
            continue;
        }
        out.push(Occurrence {
            rule: rule.id,
            line: h.line,
            text: text.trim().to_string(),
        });
    }
    out.sort_by(|a, b| (a.line, a.rule).cmp(&(b.line, b.rule)));
    out
}

/// `src/foo/tests.rs`, the file behind `#[cfg(test)] mod tests;`.
fn is_rust_test_file(path: &str, lang: Lang) -> bool {
    lang == Lang::Rust && path.rsplit('/').next() == Some("tests.rs")
}

/// Byte ranges of `#[cfg(test)]` modules and `#[test]` functions (KTD8). In
/// tree-sitter-rust an item's attributes are its previous siblings.
fn rust_test_regions(tree: &Tree, src: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    collect_test_regions(tree.root_node(), src, &mut out);
    out
}

fn collect_test_regions(node: Node, src: &str, out: &mut Vec<Range<usize>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "mod_item" | "function_item") && has_test_attribute(child, src) {
            out.push(child.byte_range());
        } else {
            collect_test_regions(child, src, out);
        }
    }
}

fn has_test_attribute(item: Node, src: &str) -> bool {
    let mut prev = item.prev_named_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" if is_test_attribute(&src[p.byte_range()]) => return true,
            "attribute_item" | "line_comment" | "block_comment" => {}
            _ => return false,
        }
        prev = p.prev_named_sibling();
    }
    false
}

/// `#[test]`, `#[cfg(test)]`, and runner attributes such as `#[tokio::test]`.
fn is_test_attribute(text: &str) -> bool {
    let inner: String = text
        .trim_start_matches("#[")
        .trim_end_matches(']')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let path = inner.split_once('(').map_or(inner.as_str(), |(p, _)| p);
    inner == "cfg(test)" || path == "test" || path.ends_with("::test")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attributes() {
        for yes in [
            "#[test]",
            "#[cfg(test)]",
            "#[tokio::test]",
            "#[tokio::test(flavor = \"multi_thread\")]",
        ] {
            assert!(is_test_attribute(yes), "{yes}");
        }
        for no in [
            "#[derive(Debug)]",
            "#[cfg(feature = \"test\")]",
            "#[testing]",
        ] {
            assert!(!is_test_attribute(no), "{no}");
        }
    }
}
