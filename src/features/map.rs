//! `qgate.features.toml`: each feature's purpose, task and owned folders
//! (KTD1), and which feature owns a path.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{MAP_FILE, MAP_RULE};
use crate::cli::Finding;
use crate::git::{BlobReader, Repo, Snapshot};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureMap {
    #[serde(default, rename = "feature")]
    pub features: Vec<Feature>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    pub name: String,
    /// What the feature is for, in one line. Placeholders have none yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// The task that created it, such as `QG-31`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Drafted by `qgate features init` and not named yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placeholder: bool,
    /// Folders the feature owns, with every entry point inside them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    /// Files owned one by one: entry points outside the feature's folders, or ones qgate can't detect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_points: Vec<String>,
}

/// The map in a snapshot, if there is one.
pub enum Loaded {
    Missing,
    /// It doesn't parse; the finding says why.
    Broken(Finding),
    Map(FeatureMap),
}

impl FeatureMap {
    /// Parse map text, normalizing folders and paths. Errors name the problem.
    pub fn parse(text: &str) -> Result<FeatureMap, String> {
        let mut map: FeatureMap =
            toml::from_str(text).map_err(|e| e.message().trim().to_string())?;
        for f in &mut map.features {
            for p in f.folders.iter_mut().chain(&mut f.entry_points) {
                *p = p.trim_end_matches('/').to_string();
            }
        }
        Ok(map)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("map serializes")
    }

    pub fn get(&self, name: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.name == name)
    }

    /// The feature that owns `path`: one listing it, else the one with the
    /// nearest folder containing it (R1).
    pub fn owner(&self, path: &str) -> Option<&Feature> {
        if let Some(f) = self
            .features
            .iter()
            .find(|f| f.entry_points.iter().any(|p| p == path))
        {
            return Some(f);
        }
        self.features
            .iter()
            .flat_map(|f| f.folders.iter().map(move |d| (d, f)))
            .filter(|(d, _)| contains(d, path))
            .max_by_key(|(d, _)| d.len())
            .map(|(_, f)| f)
    }

    /// Problems that make the map wrong, as `feature-map` errors.
    pub fn validate(&self, task_pattern: &regex::Regex) -> Vec<Finding> {
        let mut problems = Vec::new();
        let mut names = BTreeMap::new();
        let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for f in &self.features {
            if names.insert(f.name.as_str(), ()).is_some() {
                problems.push(format!("feature `{}` is declared twice", f.name));
            }
            if f.folders.is_empty() && f.entry_points.is_empty() {
                problems.push(format!(
                    "feature `{}` owns no folders and lists no entry points",
                    f.name
                ));
            }
            if !f.placeholder && f.purpose.as_deref().is_none_or(|p| p.trim().is_empty()) {
                problems.push(format!(
                    "feature `{}` has no `purpose`; say in one line what it is for",
                    f.name
                ));
            }
            if let Some(task) = &f.task
                && !is_task(task_pattern, task)
            {
                problems.push(format!(
                    "feature `{}` has task `{task}`, which is not a task ID matching `{}`",
                    f.name,
                    task_pattern.as_str()
                ));
            }
            for d in &f.folders {
                let list = owners.entry(d.as_str()).or_default();
                if !list.contains(&f.name.as_str()) {
                    list.push(&f.name);
                }
            }
        }
        for (folder, list) in owners.into_iter().filter(|(_, l)| l.len() > 1) {
            let names: Vec<String> = list.iter().map(|n| format!("`{n}`")).collect();
            problems.push(format!(
                "folder `{folder}` is owned by {}; give it to one feature",
                names.join(" and ")
            ));
        }
        problems
            .into_iter()
            .map(|m| Finding::error(MAP_RULE, m).at(MAP_FILE, None))
            .collect()
    }
}

/// Whether `text` is exactly one task ID.
pub fn is_task(pattern: &regex::Regex, text: &str) -> bool {
    pattern.find(text).is_some_and(|m| m.as_str() == text)
}

/// Whether `folder` is `path` or one of its ancestors.
pub fn contains(folder: &str, path: &str) -> bool {
    folder.is_empty()
        || path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn parse_loaded(text: &str) -> Loaded {
    match FeatureMap::parse(text) {
        Ok(map) => Loaded::Map(map),
        Err(e) => Loaded::Broken(Finding::error(MAP_RULE, e).at(MAP_FILE, None)),
    }
}

/// Load the map from a snapshot (the staged index at commit, the pushed tree at push).
pub fn load(repo: &Repo, snapshot: &Snapshot) -> Result<Loaded> {
    let Some(entry) = snapshot.get(MAP_FILE) else {
        return Ok(Loaded::Missing);
    };
    let bytes = BlobReader::new(repo)?.read(&entry.oid)?;
    Ok(parse_loaded(&String::from_utf8_lossy(&bytes)))
}

/// Like [`load`], but prefers the working-tree file, for commands the
/// developer runs by hand.
pub fn load_or_worktree(repo: &Repo, snapshot: &Snapshot) -> Result<Loaded> {
    match std::fs::read_to_string(repo.root.join(MAP_FILE)) {
        Ok(text) => Ok(parse_loaded(&text)),
        Err(_) => load(repo, snapshot),
    }
}
