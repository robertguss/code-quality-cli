//! Run the rules engine on one source string.

use qgate::checks::length::measure_with_macros;
use qgate::config::{Config, Limits, VERSION};
use qgate::rules::engine::RuleSet;

/// `(rule, line)` for every counted occurrence in `src` at `path`, under a
/// config holding `extra` (such as a `[rules]` table).
pub fn occurrences_with(path: &str, src: &str, extra: &str) -> Vec<(String, u32)> {
    let text = format!("qgate_version = \"{VERSION}\"\n{extra}");
    let config = Config::parse(&text, "qgate.toml").unwrap();
    let limits = Limits {
        function_lines: 10_000,
        file_lines: 100_000,
    };
    let rules = RuleSet::from_config(&config);
    measure_with_macros(path, src.as_bytes(), limits, &[], &rules)
        .occurrences
        .into_iter()
        .map(|o| (o.rule.to_string(), o.line))
        .collect()
}

pub fn occurrences(path: &str, src: &str) -> Vec<(String, u32)> {
    occurrences_with(path, src, "")
}

/// How many times `rule` counts in `src` at `path`.
pub fn count(path: &str, src: &str, rule: &str) -> usize {
    occurrences(path, src)
        .iter()
        .filter(|(r, _)| r == rule)
        .count()
}
