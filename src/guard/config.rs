//! Loosenings in `qgate.toml` and `qgate.baseline`: raised limits, lowered
//! rule levels, new excludes and baseline growth, each with whether it blocks
//! because it loosens an always-block rule (KTD15), plus the entries recorded
//! when a repo adopts qgate or upgrades it (KTD16).

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;

use super::{Configs, Found, RULE, warning_loosenings};
use crate::baseline::{self, BASELINE_FILE, Baseline, Key, Measured, Value, measure_paths};
use crate::checks::exclude::Excludes;
use crate::cli::Finding;
use crate::config::{CONFIG_FILE, Config, compare_versions};
use crate::git::{Change, Repo, Snapshot};
use crate::rules::{self, Level, Rule};

/// Flags the config and baseline loosenings of going from `parent` to
/// `current`, and notes entries adopted as the day-one baseline.
pub(super) fn flag_config_and_baseline(
    repo: &Repo,
    parent: &Snapshot,
    current: &Snapshot,
    changes: &[Change],
    configs: &Configs,
    found: &mut Found,
) -> Result<()> {
    let parent_counts = ParentCounts {
        repo,
        parent,
        excludes: Excludes::new(repo, parent, &configs.parent)?,
        config: &configs.new,
        always_block: &configs.always_block,
    };
    let config_lines = changed_config_lines(current, configs);
    let old_cfg = configs.old.as_ref();
    let growth = baseline_growth(changes, current, old_cfg, &configs.new, &parent_counts)?;
    for (line, blocking) in config_lines {
        found.flag(CONFIG_FILE, format!("config: {line}"), blocking);
    }
    for (line, blocking) in growth.lines {
        found.flag(BASELINE_FILE, format!("baseline: {line}"), blocking);
    }
    if growth.adopted > 0 {
        let message = format!(
            "baseline: adopting qgate with {} as the day-one baseline",
            count_of(growth.adopted, "entry", "entries")
        );
        found
            .findings
            .push(Finding::info(RULE, message).at(BASELINE_FILE, None));
    }
    Ok(())
}

/// Config loosenings with whether each blocks: a new `qgate.toml` is one
/// line, and an edited one is compared with the parent's.
fn changed_config_lines(current: &Snapshot, configs: &Configs) -> Vec<(String, bool)> {
    match &configs.old {
        None if current.get(CONFIG_FILE).is_some() => {
            vec![(format!("new {CONFIG_FILE}"), false)]
        }
        Some(old) => {
            let mut lines = config_loosenings(old, &configs.new);
            lines.extend(exclude_loosenings(old, &configs.new));
            lines
        }
        None => Vec::new(),
    }
}

/// `n` with the singular or plural noun, such as `1 entry` or `3 entries`.
fn count_of(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Counts of rules in the parent snapshot, measured with the new config's
/// rules and the parent's excludes, for judging what a change hides or
/// records (KTD15, KTD16).
struct ParentCounts<'a> {
    repo: &'a Repo,
    parent: &'a Snapshot,
    excludes: Excludes,
    config: &'a Config,
    always_block: &'a [String],
}

impl ParentCounts<'_> {
    fn measure(&self, paths: &[String]) -> Result<Measured> {
        measure_paths(self.repo, self.parent, &self.excludes, self.config, paths)
    }

    fn blocks(&self, rule: &Rule) -> bool {
        always_blocks(self.always_block, rule)
    }
}

/// Whether a list of always-block names covers `rule`.
fn always_blocks(list: &[String], rule: &Rule) -> bool {
    list.iter().any(|n| rules::always_blocked(n, rule))
}

/// Loosenings between two configs, one per line, each with whether it
/// blocks (KTD15). New `exclude` globs are judged by `exclude_loosenings`.
pub fn config_loosenings(old: &Config, new: &Config) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = warning_loosenings(old, new)
        .into_iter()
        .map(|l| (l, false))
        .collect();
    let mut always_block = old.suppressions.always_block.clone();
    always_block.extend(new.suppressions.always_block.iter().cloned());
    out.extend(level_loosenings(old, new, &always_block));
    for e in old
        .suppressions
        .always_block
        .iter()
        .filter(|e| !new.suppressions.always_block.contains(e))
    {
        out.push((format!("suppressions.always_block drops `{e}`"), true));
    }
    out
}

/// Each rule whose level fell, as `rule <id> error -> warn`, compressed to the
/// family when every rule of the family fell the same way.
fn level_loosenings(old: &Config, new: &Config, always_block: &[String]) -> Vec<(String, bool)> {
    let fell: Vec<(&Rule, Level, Level, bool)> = rules::levels(old)
        .into_iter()
        .zip(rules::levels(new))
        .filter(|((_, a), (_, b))| b < a)
        .map(|((r, a), (_, b))| (r, a, b, always_blocks(always_block, r)))
        .collect();
    let mut families: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for &(r, a, b, blocks) in &fell {
        let family = r.family();
        let whole_family = r.id.contains('/')
            && rules::all()
                .iter()
                .filter(|m| m.id.contains('/') && m.family() == family)
                .all(|m| {
                    fell.iter()
                        .any(|f| f.0.id == m.id && (f.1, f.2, f.3) == (a, b, blocks))
                });
        let name = if whole_family { family } else { r.id };
        if whole_family && !families.insert(family) {
            continue;
        }
        out.push((
            format!("rule {name} {} -> {}", a.as_str(), b.as_str()),
            blocks,
        ));
    }
    out
}

/// New `exclude` globs. One that hides always-block occurrences also gets a
/// blocking line per hidden file from `hidden::flag_hidden` (KTD15).
fn exclude_loosenings(old: &Config, new: &Config) -> Vec<(String, bool)> {
    new.exclude
        .iter()
        .filter(|p| !old.exclude.contains(p))
        .map(|p| (format!("exclude adds `{p}`"), false))
        .collect()
}

/// Baseline growth: lines with whether each blocks, plus entries recorded
/// as the day-one baseline.
#[derive(Default)]
struct Growth {
    lines: Vec<(String, bool)>,
    adopted: usize,
}

/// A baseline entry that is new or raised: its key, the parent's value when
/// there was one, and the new value.
type Grown<'a> = (&'a Key, Option<Value>, Value);

/// Baseline entries added or raised, except entries that followed a verified
/// move. Count entries for an always-block rule above the parent's count
/// block (KTD15). Adoption and upgrades record entries up to the parent's
/// count without a line per entry (KTD16).
fn baseline_growth(
    changes: &[Change],
    current: &Snapshot,
    old_cfg: Option<&Config>,
    config: &Config,
    parent: &ParentCounts,
) -> Result<Growth> {
    let old = baseline::from_snapshot(parent.repo, parent.parent)?;
    let new = baseline::from_snapshot(parent.repo, current)?;
    let grown = unmoved_growth(&old, &new, changes, current, config, parent)?;
    if grown.is_empty() {
        return Ok(Growth::default());
    }
    let paths: Vec<String> = grown
        .iter()
        .filter(|(_, _, v)| matches!(v, Value::Count(_)))
        .map(|(k, _, _)| k.path.clone())
        .collect();
    let exemptions = Exemptions {
        measured: parent.measure(&paths)?,
        adoption: parent.parent.get(CONFIG_FILE).is_none()
            && parent.parent.get(BASELINE_FILE).is_none(),
        upgrade_from: old_cfg
            .map(|c| c.qgate_version.as_str())
            .filter(|v| compare_versions(v, &config.qgate_version) != Ordering::Equal),
    };
    Ok(judge_growth(grown, &exemptions, config, parent))
}

/// Entries of `new` added or raised over `old`, less new entries that only
/// followed their code in a verified move.
fn unmoved_growth<'a>(
    old: &Baseline,
    new: &'a Baseline,
    changes: &[Change],
    current: &Snapshot,
    config: &Config,
    parent: &ParentCounts,
) -> Result<Vec<Grown<'a>>> {
    let mut grown: Vec<Grown> = new
        .entries
        .iter()
        .filter_map(|(k, &v)| match old.get(k) {
            Some(was) => v.exceeds(was).then_some((k, Some(was), v)),
            None => Some((k, None, v)),
        })
        .collect();
    if grown.iter().any(|(_, was, _)| was.is_none()) {
        let moves = verified_moves(parent, current, changes, config, old)?;
        grown.retain(|(k, was, v)| {
            was.is_some()
                || !moves.get(*k).is_some_and(|from| {
                    new.get(from).is_none() && old.get(from).is_some_and(|w| !v.exceeds(w))
                })
        });
    }
    Ok(grown)
}

/// What lets a grown entry be recorded without a line of its own (KTD16).
struct Exemptions<'a> {
    /// Parent counts for the paths of grown count entries.
    measured: Measured,
    /// The parent has neither a config nor a baseline: qgate is being adopted.
    adoption: bool,
    /// The parent's qgate_version, when the change moves to another one.
    upgrade_from: Option<&'a str>,
}

impl Exemptions<'_> {
    /// Whether an entry stays within what the parent has; exemptions cover
    /// only that much, and above it is ordinary growth.
    fn within(&self, k: &Key, v: Value) -> bool {
        match v {
            Value::Count(n) => n <= self.measured.count(&k.rule, &k.path).unwrap_or(0),
            _ => true,
        }
    }

    /// Whether `rule` arrived after the version the change upgrades from.
    fn new_rule(&self, rule: Option<&Rule>) -> bool {
        self.upgrade_from.is_some_and(|from| {
            rule.is_some_and(|r| compare_versions(r.since, from) == Ordering::Greater)
        })
    }
}

/// Lines for grown entries, with adopted entries counted and entries for
/// rules new in an upgrade summed into one line.
fn judge_growth(
    grown: Vec<Grown>,
    exemptions: &Exemptions,
    config: &Config,
    parent: &ParentCounts,
) -> Growth {
    let mut out = Growth::default();
    let mut upgraded: BTreeSet<&str> = BTreeSet::new();
    let mut upgraded_entries = 0;
    for (k, was, v) in grown {
        let rule = rules::find(&k.rule);
        let within = exemptions.within(k, v);
        if exemptions.adoption && within {
            out.adopted += 1;
            continue;
        }
        if was.is_none() && within && exemptions.new_rule(rule) {
            upgraded.insert(&k.rule);
            upgraded_entries += 1;
            continue;
        }
        let line = match was {
            Some(was) => format!("raised {} {was} -> {v}", k.describe()),
            None => format!("added {} = {v}", k.describe()),
        };
        let blocks =
            matches!(v, Value::Count(_)) && !within && rule.is_some_and(|r| parent.blocks(r));
        out.lines.push((line, blocks));
    }
    if upgraded_entries > 0 {
        out.lines.push((
            format!(
                "upgrade to {} adds {} for {}",
                config.qgate_version,
                count_of(upgraded_entries, "entry", "entries"),
                count_of(upgraded.len(), "new rule", "new rules")
            ),
            false,
        ));
    }
    out
}

/// Baselined keys that moved with their code, new key -> old key. A count
/// entry moves with a whole-file rename (KTD7).
fn verified_moves(
    parent_counts: &ParentCounts,
    current: &Snapshot,
    changes: &[Change],
    config: &Config,
    old: &Baseline,
) -> Result<HashMap<Key, Key>> {
    let (repo, parent) = (parent_counts.repo, parent_counts.parent);
    let new_paths: Vec<String> = changes
        .iter()
        .filter(|c| !c.is_deleted())
        .map(|c| c.path.clone())
        .collect();
    let old_paths: Vec<String> = changes
        .iter()
        .filter(|c| c.old_oid.is_some())
        .map(|c| c.old_path().to_string())
        .collect();
    let cur = measure_paths(
        repo,
        current,
        &Excludes::new(repo, current, config)?,
        config,
        &new_paths,
    )?;
    let par = measure_paths(
        repo,
        parent,
        &Excludes::new(repo, parent, config)?,
        config,
        &old_paths,
    )?;
    Ok(baseline::find_moves(old, &par, &cur, changes))
}
