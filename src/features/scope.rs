//! Scope changes (R5, KTD8, KTD9): what one map changes compared with the one before it.

use std::collections::{BTreeMap, BTreeSet};

use super::inventory::{EntryPoint, Kind};
use super::map::{Feature, FeatureMap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeChange {
    /// The first map in the repo: nothing in it is new (KTD8).
    Adopted {
        features: usize,
    },
    New {
        name: String,
        task: Option<String>,
        placeholder: bool,
    },
    /// A placeholder given a name or a purpose, owning the same code (KTD9).
    Named {
        from: String,
        to: String,
    },
    Removed {
        name: String,
    },
    FoldersAdded {
        name: String,
        folders: Vec<String>,
    },
    FoldersRemoved {
        name: String,
        folders: Vec<String>,
    },
    Listed {
        name: String,
        paths: Vec<String>,
    },
}

fn set(v: &[String]) -> BTreeSet<&str> {
    v.iter().map(String::as_str).collect()
}

fn same_code(a: &Feature, b: &Feature) -> bool {
    set(&a.folders) == set(&b.folders) && set(&a.entry_points) == set(&b.entry_points)
}

/// The scope changes from `parent` (`None` when the parent tree has no map) to `current`.
pub fn diff(parent: Option<&FeatureMap>, current: &FeatureMap) -> Vec<ScopeChange> {
    let Some(parent) = parent else {
        return vec![ScopeChange::Adopted {
            features: current.features.len(),
        }];
    };
    let mut out = Vec::new();
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for f in &current.features {
        match parent.get(&f.name) {
            Some(p) => out.extend(changed(p, f)),
            None => {
                let was = parent.features.iter().find(|p| {
                    p.placeholder
                        && current.get(&p.name).is_none()
                        && !named.contains(p.name.as_str())
                        && same_code(p, f)
                });
                match was {
                    Some(p) => {
                        named.insert(&p.name);
                        out.push(ScopeChange::Named {
                            from: p.name.clone(),
                            to: f.name.clone(),
                        });
                    }
                    None => out.push(ScopeChange::New {
                        name: f.name.clone(),
                        task: f.task.clone(),
                        placeholder: f.placeholder,
                    }),
                }
            }
        }
    }
    for p in &parent.features {
        if current.get(&p.name).is_none() && !named.contains(p.name.as_str()) {
            out.push(ScopeChange::Removed {
                name: p.name.clone(),
            });
        }
    }
    out
}

/// Changes to one feature that exists in both maps.
fn changed(p: &Feature, f: &Feature) -> Vec<ScopeChange> {
    let mut out = Vec::new();
    if p.placeholder && !f.placeholder {
        out.push(ScopeChange::Named {
            from: p.name.clone(),
            to: f.name.clone(),
        });
    }
    let added = |new: &[String], old: &[String]| -> Vec<String> {
        new.iter().filter(|x| !old.contains(x)).cloned().collect()
    };
    let name = || f.name.clone();
    let folders = added(&f.folders, &p.folders);
    if !folders.is_empty() {
        out.push(ScopeChange::FoldersAdded {
            name: name(),
            folders,
        });
    }
    let folders = added(&p.folders, &f.folders);
    if !folders.is_empty() {
        out.push(ScopeChange::FoldersRemoved {
            name: name(),
            folders,
        });
    }
    let paths = added(&f.entry_points, &p.entry_points);
    if !paths.is_empty() {
        out.push(ScopeChange::Listed {
            name: name(),
            paths,
        });
    }
    out
}

fn plural(n: usize, word: &str) -> String {
    match n {
        1 => format!("1 {word}"),
        _ => format!("{n} {word}s"),
    }
}

/// "2 routes, 1 module, 1 folder": the entry points `feature` owns, by kind, and its folders.
pub fn summary(feature: &Feature, map: &FeatureMap, entries: &[EntryPoint]) -> Vec<String> {
    let mut counts: BTreeMap<Kind, usize> = BTreeMap::new();
    for e in entries {
        if map.owner(&e.path).is_some_and(|o| o.name == feature.name) {
            *counts.entry(e.kind).or_default() += 1;
        }
    }
    let mut out: Vec<String> = counts
        .into_iter()
        .map(|(k, n)| plural(n, k.as_str()))
        .collect();
    if !feature.folders.is_empty() {
        out.push(plural(feature.folders.len(), "folder"));
    }
    out
}

impl ScopeChange {
    /// One line naming the change, without the size of what it owns.
    pub fn describe(&self) -> String {
        match self {
            ScopeChange::Adopted { features } => format!(
                "adopted {} with {}",
                super::MAP_FILE,
                plural(*features, "feature")
            ),
            ScopeChange::New {
                name,
                placeholder: true,
                ..
            } => format!("new placeholder {name}"),
            ScopeChange::New { name, task, .. } => match task {
                Some(t) => format!("new feature {name} ({t})"),
                None => format!("new feature {name} (no task)"),
            },
            ScopeChange::Named { from, to } if from == to => format!("named placeholder {to}"),
            ScopeChange::Named { from, to } => format!("named placeholder {from} as {to}"),
            ScopeChange::Removed { name } => format!("removed feature {name}"),
            ScopeChange::FoldersAdded { name, folders } => {
                format!("folders added to {name}: {}", folders.join(", "))
            }
            ScopeChange::FoldersRemoved { name, folders } => {
                format!("folders removed from {name}: {}", folders.join(", "))
            }
            ScopeChange::Listed { name, paths } => {
                format!("entry points listed on {name}: {}", paths.join(", "))
            }
        }
    }
}
