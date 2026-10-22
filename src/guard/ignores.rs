//! Inline ignore comments: compared per file so moved lines are not new ignores.

use std::collections::HashMap;

/// Markers that switch a linter, type checker or coverage tool off for some code.
pub const BUILT_IN: &[&str] = &[
    "eslint-disable",
    "@ts-ignore",
    "@ts-expect-error",
    "@ts-nocheck",
    "noqa",
    "type: ignore",
    "pyright: ignore",
    "pylint: disable",
    "pragma: no cover",
    "nolint",
    "NOLINT",
    "biome-ignore",
    "rubocop:disable",
    "#[allow(",
    "#![allow(",
    "@SuppressWarnings",
    "istanbul ignore",
    "c8 ignore",
    "deno-lint-ignore",
    "@phpstan-ignore",
    "phpcs:ignore",
    "swiftlint:disable",
    "ktlint-disable",
    "NOSONAR",
    "# nosec",
    // Elixir: Credo, Dialyzer, ExCoveralls, skipped ExUnit tests, compiler warnings.
    "credo:disable",
    "@dialyzer",
    "coveralls-ignore",
    "@tag :skip",
    "@tag skip:",
    "@moduletag :skip",
    "@moduletag skip:",
    "@describetag :skip",
    "@describetag skip:",
    "@compile {:no_warn_undefined",
    // qgate's own marker (KTD10).
    crate::rules::allow::MARKER,
];

/// Lines carrying any pattern that one side has and the other lacks,
/// compared like [`added`] but across all patterns at once.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Paired {
    /// New lines, by 0-based line index in the new text, untrimmed.
    pub added: Vec<(usize, String)>,
    /// Old lines the new text no longer has, trimmed.
    pub removed: Vec<String>,
}

pub fn paired(old: &str, new: &str, patterns: &[String]) -> Paired {
    let marked = |l: &str| patterns.iter().any(|p| l.contains(p.as_str()));
    let mut before: HashMap<&str, usize> = HashMap::new();
    for l in old.lines().filter(|l| marked(l)) {
        *before.entry(l.trim()).or_default() += 1;
    }
    let mut out = Paired::default();
    for (i, l) in new.lines().enumerate().filter(|(_, l)| marked(l)) {
        match before.get_mut(l.trim()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.added.push((i, l.to_string())),
        }
    }
    for l in old.lines().filter(|l| marked(l)) {
        if let Some(n) = before.get_mut(l.trim())
            && *n > 0
        {
            *n -= 1;
            out.removed.push(l.trim().to_string());
        }
    }
    out
}

/// Per pattern, the lines in `new` containing it that `old` does not have.
/// Lines are compared by trimmed text as a multiset, so moving a line within
/// the file is not new, but swapping a scoped disable for a file-wide one is.
pub fn added(old: &str, new: &str, patterns: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for p in patterns {
        let mut before: HashMap<&str, usize> = HashMap::new();
        for l in old.lines().filter(|l| l.contains(p.as_str())) {
            *before.entry(l.trim()).or_default() += 1;
        }
        let mut fresh = Vec::new();
        for l in new.lines().filter(|l| l.contains(p.as_str())) {
            match before.get_mut(l.trim()) {
                Some(n) if *n > 0 => *n -= 1,
                _ => fresh.push(l.trim().to_string()),
            }
        }
        if !fresh.is_empty() {
            out.push((p.clone(), fresh));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats() -> Vec<String> {
        BUILT_IN.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn moved_line_is_not_new() {
        let old = "a\n// eslint-disable-next-line\nb\n";
        let new = "b\n// eslint-disable-next-line\na\n";
        assert!(added(old, new, &pats()).is_empty());
    }

    #[test]
    fn new_markers_are_counted() {
        let got = added("x = 1\n", "x = 1  # noqa\n#![allow(dead_code)]\n", &pats());
        assert_eq!(
            got,
            vec![
                ("noqa".to_string(), vec!["x = 1  # noqa".to_string()]),
                (
                    "#![allow(".to_string(),
                    vec!["#![allow(dead_code)]".to_string()]
                ),
            ]
        );
    }
}
