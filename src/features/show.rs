//! `qgate features show`: where a feature lives (R8).

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use super::inventory::{self, EntryPoint, Scope, Tree};
use super::map::{FeatureMap, Loaded};
use super::{INIT_FIX, MAP_FILE};
use crate::checks::exclude::Excludes;
use crate::cli::{Format, UsageError};
use crate::git::Repo;

#[derive(Debug, Serialize)]
pub struct Shown {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub placeholder: bool,
    pub folders: Vec<String>,
    /// Every entry point the feature owns, through a folder or by listing it.
    pub entry_points: Vec<EntryPoint>,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub features: Vec<Shown>,
    /// Entry points no feature owns, when every feature is shown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unowned: Vec<EntryPoint>,
}

/// The map against the code in the working tree, committed or not.
pub fn show(repo: &Repo, name: Option<&str>) -> Result<Listing> {
    let (snapshot, config) = super::draft::worktree(repo)?;
    let map = match super::map::load(repo, &snapshot)? {
        Loaded::Map(m) => m,
        Loaded::Missing => {
            return Err(UsageError::new(format!("no {MAP_FILE} in this repo"))
                .with_hint(INIT_FIX)
                .into());
        }
        Loaded::Broken(f) => {
            return Err(UsageError::new(format!("{MAP_FILE}: {}", f.message)).into());
        }
    };
    if let Some(n) = name
        && map.get(n).is_none()
    {
        let known: Vec<&str> = map.features.iter().map(|f| f.name.as_str()).collect();
        return Err(UsageError::new(format!("no feature `{n}` in {MAP_FILE}"))
            .with_hint(format!("known features: {}", known.join(", ")))
            .into());
    }
    let excludes = Excludes::new(repo, &snapshot, &config)?;
    let tree = Tree::new(&snapshot, &excludes);
    let entries = inventory::inventory(repo, &tree, Scope::All)?;
    Ok(listing(&map, &entries, name))
}

fn listing(map: &FeatureMap, entries: &[EntryPoint], name: Option<&str>) -> Listing {
    let mut owned: BTreeMap<&str, Vec<EntryPoint>> = BTreeMap::new();
    let mut unowned = Vec::new();
    for e in entries {
        match map.owner(&e.path) {
            Some(f) => owned.entry(&f.name).or_default().push(e.clone()),
            None => unowned.push(e.clone()),
        }
    }
    let features = map
        .features
        .iter()
        .filter(|f| name.is_none_or(|n| f.name == n))
        .map(|f| Shown {
            name: f.name.clone(),
            purpose: f.purpose.clone(),
            task: f.task.clone(),
            placeholder: f.placeholder,
            folders: f.folders.clone(),
            entry_points: owned.remove(f.name.as_str()).unwrap_or_default(),
        })
        .collect();
    if name.is_some() {
        unowned.clear();
    }
    Listing { features, unowned }
}

pub fn render(listing: &Listing, format: Format) -> String {
    if format == Format::Json {
        let mut body = serde_json::to_value(listing).expect("listing serializes");
        body["command"] = "features show".into();
        return format!("{body}\n");
    }
    let mut out = String::new();
    for f in &listing.features {
        let mut head = f.name.clone();
        if let Some(t) = &f.task {
            head.push_str(&format!(" ({t})"));
        }
        if f.placeholder {
            head.push_str(" [placeholder]");
        }
        if let Some(p) = &f.purpose {
            head.push_str(&format!(": {p}"));
        }
        out.push_str(&format!("{head}\n"));
        if !f.folders.is_empty() {
            out.push_str(&format!("  folders: {}\n", f.folders.join(", ")));
        }
        push_entries(&mut out, &f.entry_points);
    }
    if !listing.unowned.is_empty() {
        out.push_str("unowned\n");
        push_entries(&mut out, &listing.unowned);
    }
    out
}

fn push_entries(out: &mut String, entries: &[EntryPoint]) {
    for e in entries {
        let at = match e.line {
            Some(l) => format!("{}:{l}", e.path),
            None => e.path.clone(),
        };
        out.push_str(&format!("  {} {}  {at}\n", e.kind.as_str(), e.name));
    }
}
