//! R6 (KTD10 to KTD14): a new inline suppression in a comment carries a
//! reason and an `until` date within the horizon, one on the always-block
//! list fails whatever it carries, and an expired one fails in any file the
//! change touches. Markers outside comments, and in files qgate does not
//! parse, stay plain loosening warnings.

use std::collections::HashSet;

use super::ignores::{self, Paired};
use crate::cli::Finding;
use crate::date::Date;
use crate::grammars::{self, Lang};
use crate::rules::{self, allow};

pub const RULE: &str = "suppression";

/// What one change's suppressions are judged against.
pub struct Policy<'a> {
    pub patterns: &'a [String],
    /// The parent's and the new config's lists together.
    pub always_block: &'a [String],
    pub max_days: u32,
    /// The change's date: today at commit, the tip's committer date at push.
    pub today: Date,
}

/// R6 errors and `until` loosening lines for one changed file.
#[derive(Debug, Default)]
pub struct Judged {
    pub findings: Vec<Finding>,
    pub lines: Vec<String>,
}

pub fn judge(path: &str, old: &str, new: &str, policy: &Policy) -> Judged {
    let pairs = ignores::paired(old, new, policy.patterns);
    let mut out = Judged {
        lines: until_loosenings(path, &pairs),
        ..Judged::default()
    };
    let in_comments = comment_lines(path, new, policy.patterns);
    let added: HashSet<usize> = pairs.added.iter().map(|(i, _)| *i).collect();
    for (i, line) in new.lines().enumerate() {
        if !in_comments.contains(&i) {
            continue;
        }
        let fresh = added
            .contains(&i)
            .then(|| new_line_problem(line, policy))
            .flatten();
        if let Some((message, fix)) = fresh.or_else(|| expired(line, policy.today)) {
            out.findings.push(
                Finding::error(RULE, message)
                    .at(path, Some(i as u32 + 1))
                    .with_fix(fix),
            );
        }
    }
    out
}

/// What is wrong with a line the change adds, with its fix.
fn new_line_problem(line: &str, policy: &Policy) -> Option<(String, String)> {
    if let Some(entry) = policy.always_block.iter().find(|e| blocks(line, e)) {
        return Some((
            format!("suppression of `{entry}`, which is on [suppressions] always_block"),
            "fix the code instead of suppressing it".into(),
        ));
    }
    let fix = format!(
        "end the comment with `-- reason: <why>, until: YYYY-MM-DD`, at most {} days ahead",
        policy.max_days
    );
    let mut missing = Vec::new();
    if !has_reason(line) {
        missing.push("reason:");
    }
    match until(line) {
        None => missing.push("until: YYYY-MM-DD"),
        Some(None) => {
            return Some((
                "suppression with an `until:` that is not a YYYY-MM-DD date".into(),
                fix,
            ));
        }
        Some(Some(d)) => {
            let horizon = policy.today.add_days(policy.max_days.into());
            if d > horizon && missing.is_empty() {
                return Some((
                    format!(
                        "suppression until {d} is more than {} days after {}",
                        policy.max_days, policy.today
                    ),
                    fix,
                ));
            }
        }
    }
    (!missing.is_empty()).then(|| {
        (
            format!("new suppression without {}", missing.join(" and ")),
            fix,
        )
    })
}

fn expired(line: &str, today: Date) -> Option<(String, String)> {
    let d = until(line).flatten()?;
    (d < today).then(|| {
        (
            format!("suppression expired on {d}"),
            "fix the code and remove the suppression, or renew it with a new until date".into(),
        )
    })
}

/// Whether `line` suppresses `entry`, an always-block rule id, family or marker.
fn blocks(line: &str, entry: &str) -> bool {
    if rules::find(entry).is_some() || rules::is_family(entry) {
        return allow::names(line).iter().any(|n| {
            rules::all()
                .iter()
                .any(|r| rules::covers(n, r) && rules::always_blocked(entry, r))
        });
    }
    if entry == "eslint-disable" {
        return bare_eslint_disable(line);
    }
    line.contains(entry)
}

/// An `eslint-disable` (or `-line`, `-next-line`) that names no rules, so it
/// switches every rule off.
fn bare_eslint_disable(line: &str) -> bool {
    const MARK: &str = "eslint-disable";
    line.match_indices(MARK).any(|(i, _)| {
        let rest = &line[i + MARK.len()..];
        let rest = rest
            .strip_prefix("-next-line")
            .or_else(|| rest.strip_prefix("-line"))
            .unwrap_or(rest)
            .trim_start();
        rest.is_empty() || rest.starts_with("--") || rest.starts_with("*/")
    })
}

fn has_reason(line: &str) -> bool {
    line.split_once("reason:").is_some_and(|(_, rest)| {
        let rest = rest.trim_start();
        !(rest.is_empty()
            || rest.starts_with(',')
            || rest.starts_with("until:")
            || rest.starts_with("*/"))
    })
}

/// The line's `until:` date: `None` without one, `Some(None)` when it is not a date.
fn until(line: &str) -> Option<Option<Date>> {
    let (_, rest) = line.split_once("until:")?;
    Some(Date::parse(date_token(rest.trim_start())))
}

fn date_token(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
        .unwrap_or(s.len());
    &s[..end]
}

/// The line without its `until:` part, to pair an old line with its edit.
fn without_until(line: &str) -> String {
    match line.split_once("until:") {
        None => line.to_string(),
        Some((before, after)) => {
            let before = before.trim_end().trim_end_matches(',').trim_end();
            let after = after.trim_start();
            format!("{before}{}", &after[date_token(after).len()..])
        }
    }
}

/// `until` dates moved later or removed, from old and new lines that are
/// otherwise equal (KTD14).
fn until_loosenings(path: &str, pairs: &Paired) -> Vec<String> {
    let mut removed: Vec<(String, Date)> = pairs
        .removed
        .iter()
        .filter_map(|l| Some((without_until(l), until(l).flatten()?)))
        .collect();
    let mut out = Vec::new();
    for (_, line) in &pairs.added {
        let key = without_until(line.trim());
        let Some(at) = removed.iter().position(|(k, _)| *k == key) else {
            continue;
        };
        let (_, old) = removed.swap_remove(at);
        match until(line).flatten() {
            Some(new) if new <= old => {}
            Some(new) => out.push(format!("inline ignore: {path} moves until {old} -> {new}")),
            None => out.push(format!("inline ignore: {path} moves until {old} -> none")),
        }
    }
    out
}

/// 0-based indexes of lines where a pattern opens a comment, as a directive
/// does (`// eslint-disable-next-line`, `x = 1  # noqa`). A marker named in
/// the middle of prose or in backticks is documentation, not a suppression.
fn comment_lines(path: &str, src: &str, patterns: &[String]) -> HashSet<usize> {
    let mut out = HashSet::new();
    if !patterns.iter().any(|p| src.contains(p.as_str())) {
        return out;
    }
    let Some(parsed) = Lang::detect(path).and_then(|l| grammars::parse(l, src)) else {
        return out;
    };
    let root = parsed.tree.root_node();
    let mut offset = 0;
    for (i, line) in src.split_inclusive('\n').enumerate() {
        let hit = patterns.iter().any(|p| {
            line.match_indices(p.as_str()).any(|(at, _)| {
                let start = offset + at;
                let mut node = root.descendant_for_byte_range(start, start + p.len());
                while let Some(n) = node {
                    if n.kind().contains("comment") {
                        let from = n.start_byte().max(offset);
                        return opens_comment(&src[from..start]);
                    }
                    node = n.parent();
                }
                false
            })
        });
        if hit {
            out.insert(i);
        }
        offset += line.len();
    }
    out
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

    #[test]
    fn markers_open_their_comment() {
        let pats = ["eslint-disable".to_string(), "qgate: allow".to_string()];
        let src = "a(); // eslint-disable-line no-undef\n/* eslint-disable */\n/// An `eslint-disable` with no names\n//! `qgate: allow <rule>` grammar\n// a qgate: allow mention\n";
        let mut got: Vec<usize> = comment_lines("a.rs", src, &pats).into_iter().collect();
        got.sort();
        assert_eq!(got, vec![0, 1]);
    }

    #[test]
    fn bare_eslint_disables() {
        for bare in [
            "/* eslint-disable */",
            "// eslint-disable-next-line",
            "// eslint-disable-line -- reason: x",
        ] {
            assert!(bare_eslint_disable(bare), "{bare}");
        }
        for named in [
            "/* eslint-disable no-var */",
            "// eslint-disable-next-line no-console -- reason: x",
        ] {
            assert!(!bare_eslint_disable(named), "{named}");
        }
    }

    #[test]
    fn until_parts() {
        let l = "// x -- reason: y, until: 2026-11-01 */";
        assert_eq!(until(l), Some(Date::parse("2026-11-01")));
        assert_eq!(until("// until: soon"), Some(None));
        assert_eq!(until("// x"), None);
        assert_eq!(without_until(l), "// x -- reason: y */");
        assert!(has_reason(l));
        assert!(!has_reason("// x -- reason: , until: 2026-11-01"));
    }
}
