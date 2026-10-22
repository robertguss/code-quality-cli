//! A starter map of placeholder features drafted from the code (R7):
//! `qgate features init` and `qgate install`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use anyhow::Result;

use super::inventory::{self, EntryPoint, Scope, Tree, layout};
use super::map::{Feature, FeatureMap, contains};
use super::{INIT_FIX, MAP_FILE};
use crate::checks::exclude::Excludes;
use crate::cli::{Finding, Report, UsageError};
use crate::config::{self, Config};
use crate::git::{Repo, Snapshot};

const HEADER: &str = "# What this repo is for, and where each feature's code lives.
# Drafted by `qgate features init`. Name each placeholder: give it a `purpose`
# and a `task`, rename it, and drop `placeholder = true`.

";

/// One placeholder per top-level module, then the remaining entry points
/// grouped by their folder. Sorted, so the same tree drafts the same map.
pub fn draft(tree: &Tree, entries: &[EntryPoint]) -> FeatureMap {
    let mut map = FeatureMap::default();
    let mut taken = BTreeSet::new();
    for m in layout::modules(&tree.roots, &tree.files) {
        let (folders, entry_points) = if m.is_dir {
            (vec![m.path.clone()], Vec::new())
        } else {
            (Vec::new(), vec![m.path.clone()])
        };
        let name = unique(&mut taken, &m.name, &m.path);
        map.features.push(placeholder(name, folders, entry_points));
    }
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in entries.iter().filter(|e| map.owner(&e.path).is_none()) {
        let dir = e.path.rsplit_once('/').map_or("", |(d, _)| d);
        let files = groups.entry(dir.to_string()).or_default();
        if !files.contains(&e.path) {
            files.push(e.path.clone());
        }
    }
    for (dir, files) in groups {
        let base = if dir.is_empty() { "root" } else { &dir };
        let name = unique(&mut taken, &base.replace('/', "-"), base);
        let feature = if ownable(tree, &dir) {
            placeholder(name, vec![dir], Vec::new())
        } else {
            placeholder(name, Vec::new(), files)
        };
        map.features.push(feature);
    }
    map.features.sort_by(|a, b| a.name.cmp(&b.name));
    map
}

fn placeholder(name: String, folders: Vec<String>, entry_points: Vec<String>) -> Feature {
    Feature {
        name,
        purpose: None,
        task: None,
        placeholder: true,
        folders,
        entry_points,
    }
}

/// `name`, or the path spelled with dashes when another feature has it.
fn unique(taken: &mut BTreeSet<String>, name: &str, path: &str) -> String {
    let mut name = name.to_string();
    if taken.contains(&name) {
        name = path.replace('/', "-");
    }
    let base = name.clone();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

/// Whether a placeholder can own `dir` without claiming future modules:
/// not a source root or one's ancestor, and not a folder of binaries.
fn ownable(tree: &Tree, dir: &str) -> bool {
    !dir.is_empty()
        && dir.rsplit('/').next() != Some("bin")
        && !tree.roots.iter().any(|r| contains(dir, &r.path))
}

/// The draft for the working tree, committed or not, as TOML, excluding what `config` excludes.
pub fn render(repo: &Repo, config: &Config) -> Result<String> {
    let snapshot = Snapshot::worktree(repo)?;
    let excludes = Excludes::new(repo, &snapshot, config)?;
    let tree = Tree::new(&snapshot, &excludes);
    let entries = inventory::inventory(repo, &tree, Scope::All)?;
    Ok(format!("{HEADER}{}", draft(&tree, &entries).to_toml()))
}

/// `qgate features init`.
pub fn init(repo: &Repo, force: bool, dry_run: bool) -> Result<Report> {
    let start = Instant::now();
    let path = repo.root.join(MAP_FILE);
    if path.exists() && !force && !dry_run {
        return Err(UsageError::new(format!("{MAP_FILE} already exists"))
            .with_hint(format!(
                "{INIT_FIX} --dry-run   (see the draft)\n  {INIT_FIX} --force   (replace it)"
            ))
            .into());
    }
    let (config, _) = config::load_or_worktree(repo, &Snapshot::index(repo)?)?;
    let text = render(repo, &config)?;
    let features = FeatureMap::parse(&text).map_or(0, |m| m.features.len());
    let mut report = Report::new("features init");
    report.set("features", features);
    report.set("path", MAP_FILE);
    if dry_run {
        report.set("dry_run", true);
        report.set("map", text);
    } else {
        std::fs::write(&path, &text)?;
        report.extend([Finding::info(
            "features-drafted",
            "name each placeholder feature, then stage the map",
        )
        .with_fix(format!("git add {MAP_FILE}"))]);
    }
    report.timing("total", start.elapsed());
    Ok(report.finish())
}
