//! `qgate: allow <rule> -- reason: <text>, until: YYYY-MM-DD` (KTD10): a
//! comment marker that takes one occurrence out of a rule's count. It applies
//! to its own line and the line below it. The guard judges its reason and
//! expiry like any other suppression (R6).

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
