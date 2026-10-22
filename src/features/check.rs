//! Ownership checks (R3, R4, KTD5): every entry point belongs to a feature,
//! and every folder and file the map names still exists.

use super::inventory::{EntryPoint, Tree};
use super::map::{FeatureMap, contains};
use super::{MAP_FILE, MAP_RULE, PLACEHOLDER, STALE, UNOWNED, count};
use crate::cli::Finding;

pub fn ownership(map: &FeatureMap, tree: &Tree, entries: &[EntryPoint]) -> Vec<Finding> {
    let mut out: Vec<Finding> = entries
        .iter()
        .filter(|e| map.owner(&e.path).is_none())
        .map(unowned)
        .collect();
    out.extend(stale(map, tree));
    out.extend(root_owners(map, tree));
    let placeholders: Vec<&str> = map
        .features
        .iter()
        .filter(|f| f.placeholder)
        .map(|f| f.name.as_str())
        .collect();
    if !placeholders.is_empty() {
        out.push(
            Finding::info(
                PLACEHOLDER,
                format!(
                    "{} to name: {} (give each a `purpose` and `task` and drop `placeholder = true`)",
                    count(placeholders.len(), "placeholder feature"),
                    placeholders.join(", ")
                ),
            )
            .at(MAP_FILE, None),
        );
    }
    out
}

fn unowned(e: &EntryPoint) -> Finding {
    Finding::error(
        UNOWNED,
        format!(
            "{} `{}` belongs to no feature: add its folder to a feature's `folders` in {MAP_FILE}, \
             or list `{}` under a feature's `entry_points`",
            e.kind.as_str(),
            e.name,
            e.path
        ),
    )
    .at(&e.path, e.line)
}

/// Folders with no files and listed paths that don't exist (R4).
fn stale(map: &FeatureMap, tree: &Tree) -> Vec<Finding> {
    let entries = &tree.snapshot.entries;
    let exists = |p: &str| {
        (p.is_empty() && !entries.is_empty())
            || entries.contains_key(p)
            || entries
                .range(format!("{p}/")..)
                .next()
                .is_some_and(|(k, _)| contains(p, k))
    };
    let mut out = Vec::new();
    for f in &map.features {
        for d in f.folders.iter().filter(|d| !exists(d)) {
            out.push(format!(
                "feature `{}` owns folder `{d}`, which has no files; remove it from the feature",
                f.name
            ));
        }
        for p in f.entry_points.iter().filter(|p| !exists(p)) {
            out.push(format!(
                "feature `{}` lists `{p}`, which does not exist; remove it from `entry_points`",
                f.name
            ));
        }
    }
    out.into_iter()
        .map(|m| Finding::error(STALE, m).at(MAP_FILE, None))
        .collect()
}

/// A feature that owns a whole source root, or a folder holding one, claims
/// every future module (KTD5).
fn root_owners(map: &FeatureMap, tree: &Tree) -> Vec<Finding> {
    map.features
        .iter()
        .flat_map(|f| f.folders.iter().map(move |d| (f, d)))
        .filter(|(_, d)| tree.holds_root(d))
        .map(|(f, d)| {
            Finding::warning(
                MAP_RULE,
                format!(
                    "feature `{}` owns `{d}`, which is or holds a source root, so a new module there joins it without a scope change; \
                     own the folders inside `{d}` instead",
                    f.name
                ),
            )
            .at(MAP_FILE, None)
        })
        .collect()
}
