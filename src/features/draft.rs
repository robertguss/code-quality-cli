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
use crate::config::{self, CONFIG_FILE, Config};
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
    // `src/foo.rs` and `src/foo/` are one Rust module: one placeholder.
    let mut modules: BTreeMap<(String, String), (Vec<String>, Vec<String>)> = BTreeMap::new();
    for m in layout::modules(&tree.roots, &tree.files) {
        let parent = layout::split(&m.path).0.to_string();
        let (folders, files) = modules.entry((parent, m.name)).or_default();
        if m.is_dir {
            folders.push(m.path);
        } else {
            files.push(m.path);
        }
    }
    for ((parent, name), (folders, entry_points)) in modules {
        let path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        let name = unique(&mut taken, &name, &path);
        map.features.push(placeholder(name, folders, entry_points));
    }
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in entries.iter().filter(|e| map.owner(&e.path).is_none()) {
        let dir = layout::split(&e.path).0;
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

/// The working tree, committed or not, and the config in it, else the
/// starter config `qgate install` would write.
pub fn worktree(repo: &Repo) -> Result<(Snapshot, Config)> {
    let snapshot = Snapshot::worktree(repo)?;
    let config = match snapshot.get(CONFIG_FILE) {
        Some(_) => config::load(repo, &snapshot)?,
        None => Config::starter_for(&repo.root),
    };
    Ok((snapshot, config))
}

/// The draft for the working tree.
pub fn drafted(repo: &Repo) -> Result<FeatureMap> {
    let (snapshot, config) = worktree(repo)?;
    let excludes = Excludes::new(repo, &snapshot, &config)?;
    let tree = Tree::new(&snapshot, &excludes);
    let entries = inventory::inventory(repo, &tree, Scope::All)?;
    Ok(draft(&tree, &entries))
}

/// A drafted map as the text of `qgate.features.toml`.
pub fn text(map: &FeatureMap) -> String {
    format!("{HEADER}{}", map.to_toml())
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
    let map = drafted(repo)?;
    let text = text(&map);
    let features = map.features.len();
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
