//! The ratchet: compare the checked tree's violations with the baseline,
//! holding each baselined entry to the lower of its recorded value and its
//! value in the parent tree (KTD4).

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::{Baseline, Key, Measured, PRUNE_FIX, Value, find_moves, key_of, measure_paths};
use crate::checks::duplication::{self, CloneGroup};
use crate::checks::exclude::Excludes;
use crate::checks::{DUPLICATION, FILE_LENGTH};
use crate::cli::Finding;
use crate::config::Config;
use crate::git::{Change, Repo, Snapshot};

pub struct CheckInput<'a> {
    pub repo: &'a Repo,
    pub config: &'a Config,
    /// What is checked: the staged index, or a pushed tip.
    pub current: &'a Snapshot,
    /// HEAD at commit, the push base at push (possibly the empty tree).
    pub parent: &'a Snapshot,
    pub changes: &'a [Change],
    /// The baseline in the checked tree.
    pub baseline: &'a Baseline,
}

pub fn check(input: &CheckInput) -> Result<Vec<Finding>> {
    let CheckInput {
        repo,
        config,
        current,
        parent,
        changes,
        baseline,
    } = *input;
    let cur_ex = Excludes::new(repo, current, config)?;
    let par_ex = Excludes::new(repo, parent, config)?;
    let new_paths: Vec<String> = changes
        .iter()
        .filter(|c| !c.is_deleted() && !cur_ex.is_excluded(&c.path))
        .map(|c| c.path.clone())
        .collect();
    let old_paths: Vec<String> = changes
        .iter()
        .filter(|c| c.old_oid.is_some())
        .map(|c| c.old_path().to_string())
        .collect();
    let cur = measure_paths(repo, current, &cur_ex, config, &new_paths)?;
    let par = measure_paths(repo, parent, &par_ex, config, &old_paths)?;
    let moves = find_moves(baseline, &par, &cur, changes);
    let old_path_of: HashMap<&str, &str> = changes
        .iter()
        .map(|c| (c.path.as_str(), c.old_path()))
        .collect();

    let mut findings = cur.warnings.clone();
    let headroom = config.ratchet.headroom_lines;
    lengths(
        baseline,
        &cur,
        &par,
        &moves,
        &old_path_of,
        headroom,
        &mut findings,
    );
    stale_lengths(baseline, &cur, &par, &moves, &mut findings);
    if config.duplication.enabled && !new_paths.is_empty() {
        let focus: HashSet<String> = new_paths.iter().cloned().collect();
        let min = config.duplication.min_tokens;
        let now = duplication::detect(repo, current, &cur_ex, min, Some(&focus))?.groups;
        let before =
            if baseline.entries.keys().any(|k| k.rule == DUPLICATION) && !old_paths.is_empty() {
                let focus: HashSet<String> = old_paths.iter().cloned().collect();
                duplication::detect(repo, parent, &par_ex, min, Some(&focus))?.groups
            } else {
                Vec::new()
            };
        clones(baseline, &now, &before, &focus, &mut findings);
    }
    Ok(findings)
}

fn parent_value(par: &Measured, key: &Key, old_path_of: &HashMap<&str, &str>) -> Option<Value> {
    let old = Key {
        path: old_path_of
            .get(key.path.as_str())
            .copied()
            .unwrap_or(&key.path)
            .to_string(),
        ..key.clone()
    };
    par.find(&old).map(|m| Value::Lines(m.value))
}

fn what(key: &Key) -> String {
    if key.rule == FILE_LENGTH {
        "file".to_string()
    } else {
        format!(
            "function `{}`",
            key.symbol
                .rsplit_once('#')
                .map_or(key.symbol.as_str(), |(n, _)| n)
        )
    }
}

fn lengths(
    baseline: &Baseline,
    cur: &Measured,
    par: &Measured,
    moves: &HashMap<Key, Key>,
    old_path_of: &HashMap<&str, &str>,
    headroom: u32,
    out: &mut Vec<Finding>,
) {
    for m in cur.all().filter(|m| m.over_limit()) {
        let key = key_of(m);
        let at = |f: Finding| f.at(&m.path, Some(m.line));
        let (recorded, before) = match (baseline.get(&key), moves.get(&key)) {
            (Some(b), _) => (b, parent_value(par, &key, old_path_of)),
            (None, Some(old)) => {
                out.push(at(Finding::error(
                    m.rule,
                    format!(
                        "baselined {} moved here; its baseline entry must move with it",
                        what(&key)
                    ),
                )
                .with_fix(PRUNE_FIX)));
                (
                    baseline.get(old).expect("move source is baselined"),
                    par.find(old).map(|p| Value::Lines(p.value)),
                )
            }
            (None, None) => {
                out.push(at(Finding::error(
                    m.rule,
                    format!(
                        "{} is {} lines (limit {}); split it up",
                        what(&key),
                        m.value,
                        m.limit
                    ),
                )));
                continue;
            }
        };
        // Headroom is added to the lower of the recorded and parent sizes, and
        // the recorded size never rises, so growth cannot accumulate across changes.
        let held = before.map_or(recorded, |b| recorded.min(b));
        let allowed = match held {
            Value::Lines(n) => Value::Lines(n + headroom),
            other => other,
        };
        let value = Value::Lines(m.value);
        if value.exceeds(allowed) {
            let slack = if headroom > 0 {
                format!(" ({held} plus {headroom} lines of headroom)")
            } else {
                String::new()
            };
            out.push(at(Finding::error(
                m.rule,
                format!(
                    "{} grew to {} lines; a baselined violation may not exceed {allowed}{slack} (limit {})",
                    what(&key),
                    m.value,
                    m.limit
                ),
            )));
        } else if recorded.exceeds(value) {
            out.push(at(Finding::info(
                "baseline-prunable",
                format!("{} shrank from {recorded} to {} lines", what(&key), m.value),
            )
            .with_fix(PRUNE_FIX)));
        }
    }
}

/// Baselined entries on changed paths that are fixed or gone.
fn stale_lengths(
    baseline: &Baseline,
    cur: &Measured,
    par: &Measured,
    moves: &HashMap<Key, Key>,
    out: &mut Vec<Finding>,
) {
    let move_sources: HashSet<&Key> = moves.values().collect();
    for key in baseline.entries.keys().filter(|k| k.rule != DUPLICATION) {
        let touched = cur.by_path.contains_key(&key.path) || par.by_path.contains_key(&key.path);
        if !touched || move_sources.contains(key) {
            continue;
        }
        if cur.find(key).is_none_or(|m| !m.over_limit()) {
            out.push(
                Finding::info(
                    "baseline-prunable",
                    format!("{} is fixed or gone", key.describe()),
                )
                .at(&key.path, None)
                .with_fix(PRUNE_FIX),
            );
        }
    }
}

fn clones(
    baseline: &Baseline,
    now: &[CloneGroup],
    before: &[CloneGroup],
    focus: &HashSet<String>,
    out: &mut Vec<Finding>,
) {
    let value = |g: &CloneGroup| Value::Clones {
        members: g.members.len() as u32,
        tokens: g.tokens,
    };
    let before: HashMap<&str, Value> = before.iter().map(|g| (g.key.as_str(), value(g))).collect();
    for g in now {
        let key = Key::clones(&g.key);
        let reason = match baseline.get(&key) {
            None => None,
            Some(recorded) => {
                let allowed = before
                    .get(g.key.as_str())
                    .map_or(recorded, |b| recorded.min(*b));
                if value(g).exceeds(allowed) {
                    Some(format!(
                        "a baselined duplicate gained copies ({allowed} allowed)"
                    ))
                } else {
                    if recorded.exceeds(value(g)) {
                        out.push(
                            Finding::info(
                                "baseline-prunable",
                                format!(
                                    "{} {}; an edit changed a copy, so its entry can shrink",
                                    key.describe(),
                                    clone_change(recorded, value(g))
                                ),
                            )
                            .with_fix(PRUNE_FIX),
                        );
                    }
                    continue;
                }
            }
        };
        for m in g.members.iter().filter(|m| focus.contains(&m.path)) {
            let mut msg = format!(
                "duplicated block ({} tokens, lines {}-{}) also at {}",
                g.tokens,
                m.start_line,
                m.end_line,
                g.others(&m.path, m.start_line)
            );
            if let Some(r) = &reason {
                msg = format!("{msg}; {r}");
            }
            out.push(
                Finding::error(DUPLICATION, msg + "; extract the shared code")
                    .at(&m.path, Some(m.start_line)),
            );
        }
    }
    let now_keys: HashSet<&str> = now.iter().map(|g| g.key.as_str()).collect();
    for k in before.keys().filter(|k| !now_keys.contains(*k)) {
        let key = Key::clones(k);
        if baseline.get(&key).is_some() {
            out.push(
                Finding::info(
                    "baseline-prunable",
                    format!(
                        "{} no longer has a copy in the changed files; an edit changed or removed one, so its entry can shrink",
                        key.describe()
                    ),
                )
                .with_fix(PRUNE_FIX),
            );
        }
    }
}

/// "now has 2 copies of 60 tokens (baseline: 3 of 60)".
fn clone_change(recorded: Value, now: Value) -> String {
    match (recorded, now) {
        (
            Value::Clones {
                members: m0,
                tokens: t0,
            },
            Value::Clones { members, tokens },
        ) => format!("now has {members} copies of {tokens} tokens (baseline: {m0} of {t0})"),
        _ => format!("shrank from {recorded} to {now}"),
    }
}
