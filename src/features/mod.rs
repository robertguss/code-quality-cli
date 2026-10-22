//! The feature map: what a repo is for and where each feature's code lives.
//! Every entry point must belong to a feature, scope changes are reported,
//! and new features carry a task ID.

pub mod check;
pub mod draft;
pub mod inventory;
pub mod log;
pub mod map;
pub mod scope;
pub mod show;
pub mod task;

use anyhow::Result;

use crate::checks::exclude::Excludes;
use crate::cli::Finding;
use crate::config::{Config, Stage};
use crate::git::{Change, Repo, Snapshot, changes};
use inventory::{Scope, Tree};
use map::{FeatureMap, Loaded};
use scope::ScopeChange;

pub const MAP_FILE: &str = "qgate.features.toml";
pub const INIT_FIX: &str = "qgate features init";

pub const MAP_RULE: &str = "feature-map";
pub const MISSING: &str = "feature-map-missing";
pub const UNOWNED: &str = "feature-unowned";
pub const STALE: &str = "feature-stale";
pub const PLACEHOLDER: &str = "feature-placeholder";
pub const SCOPE: &str = "scope-change";
pub const TASK: &str = "feature-task";
pub const TASK_PENDING: &str = "feature-task-pending";

/// The commits a push sends.
pub struct Push {
    pub base: Option<String>,
    pub tip: String,
}

/// One `qgate run` step's view of the change.
pub struct Input<'a> {
    pub repo: &'a Repo,
    pub config: &'a Config,
    pub stage: Stage,
    pub current: &'a Snapshot,
    pub parent: &'a Snapshot,
    pub changes: &'a [Change],
    /// Push only.
    pub push: Option<&'a Push>,
    pub branch: Option<String>,
}

pub struct Outcome {
    pub findings: Vec<Finding>,
    /// The one-line push summary of scope changes, ready for a PR description.
    pub scope: Option<String>,
}

/// The feature checks for one commit or push (KTD3).
pub fn check(input: &Input) -> Result<Outcome> {
    let map = match map::load(input.repo, input.current)? {
        Loaded::Missing => {
            let f = Finding::info(
                MISSING,
                format!("no {MAP_FILE}, so entry points are not checked against features"),
            )
            .with_fix(INIT_FIX);
            return Ok(Outcome {
                findings: vec![f],
                scope: None,
            });
        }
        Loaded::Broken(f) => {
            return Ok(Outcome {
                findings: vec![f],
                scope: None,
            });
        }
        Loaded::Map(m) => m,
    };
    let pattern = input.config.tasks.regex();
    let mut findings = map.validate(&pattern);
    let excludes = Excludes::new(input.repo, input.current, input.config)?;
    let tree = Tree::new(input.current, &excludes);
    let map_changed = input.parent.get(MAP_FILE) != input.current.get(MAP_FILE);
    let changed: Vec<String> = input
        .changes
        .iter()
        .filter(|c| !c.is_deleted())
        .map(|c| c.path.clone())
        .collect();
    let scope = match input.stage {
        Stage::Commit if !map_changed => Scope::Paths(&changed),
        _ => Scope::All,
    };
    let entries = inventory::inventory(input.repo, &tree, scope)?;
    findings.extend(check::ownership(&map, &tree, &entries));
    if !map_changed {
        return Ok(Outcome {
            findings,
            scope: None,
        });
    }
    let parent = match map::load(input.repo, input.parent)? {
        Loaded::Map(m) => Some(m),
        _ => None,
    };
    let diff = scope::diff(parent.as_ref(), &map);
    let lines = describe(input, &map, &diff, &entries)?;
    findings.extend(
        lines
            .iter()
            .map(|l| Finding::info(SCOPE, l.clone()).at(MAP_FILE, None)),
    );
    findings.extend(tasks(input, &pattern, &diff)?);
    let scope = (input.stage == Stage::Push && !lines.is_empty()).then(|| lines.join("; "));
    Ok(Outcome { findings, scope })
}

/// One line per scope change, new features with the size of what they own.
fn describe(
    input: &Input,
    map: &FeatureMap,
    diff: &[ScopeChange],
    entries: &[inventory::EntryPoint],
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for change in diff {
        let mut line = change.describe();
        if let ScopeChange::New { name, .. } = change
            && let Some(f) = map.get(name)
        {
            let mut parts = scope::summary(f, map, entries);
            if let Some(push) = input.push {
                let owned: Vec<String> = f.folders.iter().chain(&f.entry_points).cloned().collect();
                let added = lines_added(input.repo, push, &owned)?;
                let unit = if added == 1 { "line" } else { "lines" };
                parts.push(format!("+{} {unit}", thousands(added)));
            }
            if !parts.is_empty() {
                line = format!("{line}: {}", parts.join(", "));
            }
        }
        out.push(line);
    }
    Ok(out)
}

/// `1240` as `1,240`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Lines added under `paths` (folders and files) across the push.
fn lines_added(repo: &Repo, push: &Push, paths: &[String]) -> Result<u64> {
    if paths.is_empty() {
        return Ok(0);
    }
    let base = match &push.base {
        Some(b) => b.clone(),
        None => repo.empty_tree()?,
    };
    let mut args = vec!["diff", "--numstat", base.as_str(), push.tip.as_str(), "--"];
    args.extend(paths.iter().map(String::as_str));
    let out = repo.output(&args)?;
    Ok(out
        .lines()
        .filter_map(|l| l.split('\t').next()?.parse::<u64>().ok())
        .sum())
}

/// Task findings for the new, named features in `diff` (R6).
fn tasks(input: &Input, pattern: &regex::Regex, diff: &[ScopeChange]) -> Result<Vec<Finding>> {
    let new: Vec<(&str, Option<&str>)> = diff
        .iter()
        .filter_map(|c| match c {
            ScopeChange::New {
                name,
                task,
                placeholder: false,
            } => Some((name.as_str(), task.as_deref())),
            _ => None,
        })
        .collect();
    if new.is_empty() {
        return Ok(Vec::new());
    }
    let messages = match input.push {
        Some(p) => Some(changes::messages(input.repo, p.base.as_deref(), &p.tip)?),
        None => None,
    };
    Ok(task::check(
        &new,
        &task::Searched {
            pattern,
            branch: input.branch.as_deref(),
            messages: messages.as_deref(),
        },
    ))
}
