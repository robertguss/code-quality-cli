//! The committed baseline of existing violations (KTD4): record them, fail
//! only on new or worse ones, and shrink without ever growing.

pub mod compare;
pub mod format;
pub mod merge_driver;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::Result;

use crate::checks::duplication;
use crate::checks::exclude::Excludes;
use crate::checks::length::{self, Measurement};
use crate::checks::{DUPLICATION, FILE_LENGTH, FUNCTION_LENGTH, PARSE_ERROR};
use crate::cli::{Finding, Report, UsageError};
use crate::config::{self, Config};
use crate::git::{BlobReader, Change, ChangeKind, Repo, Snapshot, changes};
use crate::tools::scheduler::parallel_map;
pub use format::{Baseline, Key, Value};

pub const BASELINE_FILE: &str = "qgate.baseline";
pub const PRUNE_FIX: &str = "qgate baseline prune && git add qgate.baseline";

/// Length measurements of some files in one snapshot, by path.
#[derive(Debug, Default)]
pub struct Measured {
    pub by_path: HashMap<String, Vec<Measurement>>,
    pub warnings: Vec<Finding>,
}

impl Measured {
    pub fn find(&self, key: &Key) -> Option<&Measurement> {
        self.by_path
            .get(&key.path)?
            .iter()
            .find(|m| m.rule == key.rule && m.symbol.as_deref().unwrap_or("-") == key.symbol)
    }

    pub fn all(&self) -> impl Iterator<Item = &Measurement> {
        self.by_path.values().flatten()
    }
}

pub fn key_of(m: &Measurement) -> Key {
    Key::length(m.rule, &m.path, m.symbol.as_deref())
}

/// Measure `paths` in `snapshot`, skipping excluded and missing files.
pub fn measure_paths(
    repo: &Repo,
    snapshot: &Snapshot,
    excludes: &Excludes,
    config: &Config,
    paths: &[String],
) -> Result<Measured> {
    let targets: Vec<(&String, &str)> = paths
        .iter()
        .filter(|p| !excludes.is_excluded(p))
        .filter_map(|p| {
            snapshot
                .get(p)
                .filter(|e| e.is_file())
                .map(|e| (p, e.oid.as_str()))
        })
        .collect();
    let chunk = targets.len().div_ceil(8).max(1);
    let chunks: Vec<&[(&String, &str)]> = targets.chunks(chunk).collect();
    let results = parallel_map(&chunks, |part| -> Result<Vec<length::FileLengths>> {
        let mut reader = BlobReader::new(repo)?;
        part.iter()
            .map(|(path, oid)| {
                let bytes = reader.read(oid)?;
                Ok(length::measure_with_macros(
                    path,
                    &bytes,
                    config.limits_for(path),
                    &config.attribute_macros,
                ))
            })
            .collect()
    });
    let mut out = Measured::default();
    let mut recovered: Vec<String> = Vec::new();
    for part in results {
        for file in part? {
            out.warnings.extend(file.warnings);
            if file.recovered_errors
                && let Some(first) = file.measurements.first()
            {
                recovered.push(first.path.clone());
            }
            if let Some(first) = file.measurements.first() {
                out.by_path.insert(first.path.clone(), file.measurements);
            }
        }
    }
    if let Some(first) = recovered.iter().min() {
        let others = match recovered.len() {
            1 => String::new(),
            2 => " and 1 other file".to_string(),
            n => format!(" and {} other files", n - 1),
        };
        out.warnings.push(Finding::info(
            PARSE_ERROR,
            format!(
                "{first}{others} parsed with small syntax errors the parser recovered from; \
                 function lengths are unaffected. If they come from macros such as \
                 `MUST_USE`, list them in `attribute_macros` in qgate.toml"
            ),
        ));
    }
    Ok(out)
}

/// Baselined length entries whose symbol moved: a whole-file rename, or a
/// function whose normalized body now appears under a new, unbaselined key.
/// Returns new key -> old key.
pub fn find_moves(
    baseline: &Baseline,
    parent: &Measured,
    current: &Measured,
    changes: &[Change],
) -> HashMap<Key, Key> {
    let mut moves = HashMap::new();
    let mut claimed: HashSet<Key> = HashSet::new();
    for old in baseline.entries.keys() {
        if old.rule != FUNCTION_LENGTH && old.rule != FILE_LENGTH {
            continue;
        }
        if !parent.by_path.contains_key(&old.path) || current.find(old).is_some() {
            continue;
        }
        let renamed_to = changes.iter().find_map(|c| match &c.kind {
            ChangeKind::Renamed { from } if *from == old.path => Some(c.path.as_str()),
            _ => None,
        });
        if let Some(to) = renamed_to {
            let new = Key {
                path: to.to_string(),
                ..old.clone()
            };
            if current.find(&new).is_some() && baseline.get(&new).is_none() {
                claimed.insert(new.clone());
                moves.insert(new, old.clone());
                continue;
            }
        }
        if old.rule != FUNCTION_LENGTH {
            continue;
        }
        let Some(hash) = parent.find(old).and_then(|m| m.body_hash) else {
            continue;
        };
        let target = current
            .all()
            .filter(|m| m.rule == FUNCTION_LENGTH && m.body_hash == Some(hash))
            .map(key_of)
            .find(|k| baseline.get(k).is_none() && !claimed.contains(k));
        if let Some(new) = target {
            claimed.insert(new.clone());
            moves.insert(new, old.clone());
        }
    }
    moves
}

/// Read `qgate.baseline` from a snapshot; empty when absent.
pub fn from_snapshot(repo: &Repo, snapshot: &Snapshot) -> Result<Baseline> {
    match snapshot.get(BASELINE_FILE) {
        Some(e) => {
            let bytes = BlobReader::new(repo)?.read(&e.oid)?;
            Baseline::parse(&String::from_utf8_lossy(&bytes))
        }
        None => Ok(Baseline::default()),
    }
}

/// Every current violation in the staged tree, as a baseline.
fn measure_all(repo: &Repo, snapshot: &Snapshot, config: &Config) -> Result<(Baseline, Measured)> {
    let excludes = Excludes::new(repo, snapshot, config)?;
    let paths: Vec<String> = snapshot.files().map(|(p, _)| p.clone()).collect();
    let measured = measure_paths(repo, snapshot, &excludes, config, &paths)?;
    let mut baseline = Baseline::default();
    for m in measured.all().filter(|m| m.over_limit()) {
        baseline.entries.insert(key_of(m), Value::Lines(m.value));
    }
    if config.duplication.enabled {
        let d = duplication::detect(
            repo,
            snapshot,
            &excludes,
            config.duplication.min_tokens,
            None,
        )?;
        for g in d.groups {
            baseline.entries.insert(
                Key::clones(&g.key),
                Value::Clones {
                    members: g.members.len() as u32,
                    tokens: g.tokens,
                },
            );
        }
    }
    Ok((baseline, measured))
}

pub fn create(repo: &Repo, force: bool, dry_run: bool) -> Result<Report> {
    let start = Instant::now();
    let path = repo.root.join(BASELINE_FILE);
    if path.exists() && !force && !dry_run {
        return Err(UsageError::new(format!("{BASELINE_FILE} already exists"))
            .with_hint(format!(
                "{PRUNE_FIX}   (shrink it)\n  qgate baseline create --force   (replace it; growth needs the developer's qgate approve)"
            ))
            .into());
    }
    let snapshot = Snapshot::index(repo)?;
    let config = config::load_or_worktree(repo, &snapshot)?;
    let (baseline, _) = measure_all(repo, &snapshot, &config)?;
    let text = baseline.render();
    let unchanged = std::fs::read_to_string(&path).is_ok_and(|old| old == text);
    let mut report = Report::new("baseline create");
    report.set("entries", baseline.entries.len());
    report.set("path", BASELINE_FILE);
    if dry_run {
        report.set("dry_run", true);
        report.set("baseline", text);
    } else {
        if !unchanged {
            std::fs::write(&path, &text)?;
        }
        report.set("written", !unchanged);
        report.extend([Finding::info(
            "baseline-created",
            "stage the baseline; adding entries needs the developer's approval",
        )
        .with_fix("git add qgate.baseline   (then the developer runs: qgate approve)")]);
    }
    report.timing("total", start.elapsed());
    Ok(report.finish())
}

/// What prune would do to one baseline.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub baseline: Baseline,
    pub lowered: Vec<(Key, Value, Value)>,
    pub removed: Vec<Key>,
    pub moved: Vec<(Key, Key)>,
}

/// Lower or drop entries against the staged tree; follow moved symbols.
/// Never adds a key that did not move and never raises a value.
pub fn prune_against(
    old: &Baseline,
    current: &Measured,
    groups: &HashMap<String, Value>,
    moves: &HashMap<Key, Key>,
) -> Pruned {
    let mut out = Pruned::default();
    let moved_from: HashMap<&Key, &Key> = moves.iter().map(|(n, o)| (o, n)).collect();
    for (key, &value) in &old.entries {
        let (target, now) = if key.rule == DUPLICATION {
            (key.clone(), groups.get(&key.symbol).copied())
        } else if let Some(m) = current.find(key) {
            (key.clone(), m.over_limit().then_some(Value::Lines(m.value)))
        } else if let Some(new) = moved_from.get(key) {
            let m = current.find(new).expect("moves point at measured keys");
            out.moved.push((key.clone(), (*new).clone()));
            (
                (*new).clone(),
                m.over_limit().then_some(Value::Lines(m.value)),
            )
        } else {
            (key.clone(), None)
        };
        match now {
            None => out.removed.push(key.clone()),
            Some(now) => {
                let kept = value.min(now);
                if kept != value {
                    out.lowered.push((target.clone(), value, kept));
                }
                out.baseline.entries.insert(target, kept);
            }
        }
    }
    out
}

pub fn prune(repo: &Repo, dry_run: bool) -> Result<Report> {
    let start = Instant::now();
    let path = repo.root.join(BASELINE_FILE);
    let mut report = Report::new("baseline prune");
    let Ok(text) = std::fs::read_to_string(&path) else {
        report.set("entries", 0);
        report.set("changed", false);
        return Ok(report.finish());
    };
    let old = Baseline::parse(&text)?;
    let snapshot = Snapshot::index(repo)?;
    let config = config::load_or_worktree(repo, &snapshot)?;
    let (all, current) = measure_all(repo, &snapshot, &config)?;
    let groups: HashMap<String, Value> = all
        .entries
        .into_iter()
        .filter(|(k, _)| k.rule == DUPLICATION)
        .map(|(k, v)| (k.symbol, v))
        .collect();
    let parent = Snapshot::at(repo, repo.head().as_deref())?;
    let staged = changes::staged(repo)?;
    let old_paths: Vec<String> = staged
        .iter()
        .filter(|c| c.old_oid.is_some())
        .map(|c| c.old_path().to_string())
        .collect();
    let parent_excludes = Excludes::new(repo, &parent, &config)?;
    let parent_m = measure_paths(repo, &parent, &parent_excludes, &config, &old_paths)?;
    let moves = find_moves(&old, &parent_m, &current, &staged);
    let pruned = prune_against(&old, &current, &groups, &moves);
    let new_text = pruned.baseline.render();
    let changed = new_text != text;
    if changed && !dry_run {
        std::fs::write(&path, &new_text)?;
    }
    report.set("entries", pruned.baseline.entries.len());
    report.set("changed", changed);
    if dry_run {
        report.set("dry_run", true);
    }
    for (key, from, to) in &pruned.lowered {
        report.extend([Finding::info(
            "baseline-lowered",
            format!("{}: {from} -> {to}", key.describe()),
        )]);
    }
    for key in &pruned.removed {
        report.extend([Finding::info(
            "baseline-removed",
            format!("{} is fixed", key.describe()),
        )]);
    }
    for (from, to) in &pruned.moved {
        report.extend([Finding::info(
            "baseline-moved",
            format!("{} moved to {}", from.describe(), to.describe()),
        )]);
    }
    if changed && !dry_run {
        report.extend([
            Finding::info("baseline-pruned", "stage the pruned baseline")
                .with_fix("git add qgate.baseline"),
        ]);
    }
    report.timing("total", start.elapsed());
    Ok(report.finish())
}
