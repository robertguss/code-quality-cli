//! The occurrence ratchet (KTD6): a changed file's count of each rule may not
//! rise above the lower of its baseline entry and its count in the parent tree.

use std::collections::{HashMap, HashSet};

use super::engine::Occurrence;
use super::find;
use crate::baseline::{Baseline, Key, Measured, PRUNE_FIX, Value};
use crate::cli::Finding;

/// Count findings for every changed file in `cur`, and prunable infos for
/// count entries of touched files that fell. `moves` maps new keys to the
/// baselined keys they moved from; `old_path_of` maps a path to its name in
/// the parent.
pub fn counts(
    baseline: &Baseline,
    cur: &Measured,
    par: &Measured,
    moves: &HashMap<Key, Key>,
    old_path_of: &HashMap<&str, &str>,
    out: &mut Vec<Finding>,
) {
    let mut paths: Vec<&String> = cur.occurrences.keys().collect();
    paths.sort();
    for path in paths {
        let old_path = old_path_of.get(path.as_str()).copied().unwrap_or(path);
        let now = &cur.occurrences[path];
        let before = par.occurrences.get(old_path);
        let mut rules: Vec<&'static str> = now.iter().map(|o| o.rule).collect();
        rules.sort();
        rules.dedup();
        for id in rules {
            let key = Key::count(id, path);
            let entry = baseline
                .get(&key)
                .or_else(|| moves.get(&key).and_then(|old| baseline.get(old)))
                .map(count_of);
            let parent = par
                .by_path
                .contains_key(old_path)
                .then(|| before.map_or(0, |b| b.iter().filter(|o| o.rule == id).count() as u32));
            let allowed = match (entry, parent) {
                (Some(e), Some(p)) => e.min(p),
                (None, Some(p)) => p,
                (Some(e), None) => e,
                (None, None) => 0,
            };
            let mine: Vec<&Occurrence> = now.iter().filter(|o| o.rule == id).collect();
            let n = mine.len() as u32;
            if n <= allowed {
                continue;
            }
            let old: Vec<&Occurrence> = before
                .into_iter()
                .flatten()
                .filter(|o| o.rule == id)
                .collect();
            for o in newest(&mine, &old, (n - allowed) as usize) {
                out.push(finding(o, n, allowed).at(path, Some(o.line)));
            }
        }
    }
    prunable(baseline, cur, par, moves, out);
}

fn count_of(v: Value) -> u32 {
    match v {
        Value::Count(n) => n,
        _ => 0,
    }
}

fn finding(o: &Occurrence, n: u32, allowed: u32) -> Finding {
    let rule = find(o.rule).expect("occurrences come from registered rules");
    let limit = match allowed {
        0 => "none allowed".to_string(),
        a => format!("{a} allowed"),
    };
    Finding::error(
        rule.id,
        format!("{} ({n} in this file, {limit})", rule.rationale),
    )
    .with_fix(rule.fix)
}

/// The `excess` occurrences in `now` that are new: those whose text has more
/// copies than in `before`, then the last ones in the file.
fn newest<'a>(
    now: &[&'a Occurrence],
    before: &[&Occurrence],
    excess: usize,
) -> Vec<&'a Occurrence> {
    let mut left: HashMap<&str, usize> = HashMap::new();
    for o in before {
        *left.entry(o.text.as_str()).or_default() += 1;
    }
    let mut picked: Vec<usize> = Vec::new();
    for (i, o) in now.iter().enumerate() {
        match left.get_mut(o.text.as_str()) {
            Some(k) if *k > 0 => *k -= 1,
            _ => picked.push(i),
        }
    }
    let chosen: HashSet<usize> = picked.iter().copied().collect();
    let rest = (0..now.len()).rev().filter(|i| !chosen.contains(i));
    let mut all: Vec<usize> = picked.into_iter().chain(rest).take(excess).collect();
    all.sort();
    all.into_iter().map(|i| now[i]).collect()
}

/// Count entries of touched files whose count is now lower.
fn prunable(
    baseline: &Baseline,
    cur: &Measured,
    par: &Measured,
    moves: &HashMap<Key, Key>,
    out: &mut Vec<Finding>,
) {
    let moved_to: HashMap<&Key, &Key> = moves.iter().map(|(n, o)| (o, n)).collect();
    for (key, value) in &baseline.entries {
        let Value::Count(recorded) = *value else {
            continue;
        };
        let touched = cur.by_path.contains_key(&key.path) || par.by_path.contains_key(&key.path);
        if !touched {
            continue;
        }
        let msg = if let Some(new) = moved_to.get(key) {
            if baseline.get(new).is_some() {
                continue;
            }
            format!("{} moved to {}", key.describe(), new.path)
        } else {
            let now = cur.count(&key.rule, &key.path).unwrap_or(0);
            if now >= recorded {
                continue;
            }
            format!("{} fell from {recorded} to {now}", key.describe())
        };
        out.push(
            Finding::info("baseline-prunable", msg)
                .at(&key.path, None)
                .with_fix(PRUNE_FIX),
        );
    }
}
