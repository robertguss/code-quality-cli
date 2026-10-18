//! Inline ignore comments: counted per file so moved lines are not new ignores.

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
];

/// Lines containing each pattern.
pub fn count(text: &str, patterns: &[String]) -> Vec<usize> {
    patterns
        .iter()
        .map(|p| text.lines().filter(|l| l.contains(p.as_str())).count())
        .collect()
}

/// Patterns whose count grew from `old` to `new`, with the increase.
pub fn added(old: &str, new: &str, patterns: &[String]) -> Vec<(String, usize)> {
    let before = count(old, patterns);
    let after = count(new, patterns);
    patterns
        .iter()
        .zip(before.iter().zip(after))
        .filter(|(_, (b, a))| a > *b)
        .map(|(p, (b, a))| (p.clone(), a - b))
        .collect()
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
            vec![("noqa".to_string(), 1), ("#![allow(".to_string(), 1)]
        );
    }
}
