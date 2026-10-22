//! The entry-point inventory (R2, KTD6): every binary, route, command,
//! migration and top-level module in the production code of a tree.

pub mod layout;
pub mod rust;

use std::collections::BTreeSet;

use anyhow::Result;
use serde::Serialize;

use crate::checks::exclude::Excludes;
use crate::classify::{Role, classify};
use crate::git::{BlobReader, Repo, Snapshot};
use crate::grammars::Lang;
use crate::tools::scheduler::parallel_map;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Binary,
    Route,
    Command,
    Migration,
    Module,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Binary => "binary",
            Kind::Route => "route",
            Kind::Command => "command",
            Kind::Migration => "migration",
            Kind::Module => "module",
        }
    }
}

/// One way into the code. `path` is the file (or module directory) that
/// declares it; listing that path in the map claims it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EntryPoint {
    pub kind: Kind,
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Which files to inventory (KTD7).
#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    All,
    /// Only entry points declared in, or containing, these paths.
    Paths(&'a [String]),
}

/// A tree's layout and its production files, shared by the inventory and the checks.
pub struct Tree<'a> {
    pub snapshot: &'a Snapshot,
    pub roots: Vec<layout::Root>,
    /// Production files the built-in checks don't exclude.
    pub files: Vec<&'a str>,
}

impl<'a> Tree<'a> {
    pub fn new(snapshot: &'a Snapshot, excludes: &Excludes) -> Tree<'a> {
        let all: Vec<&str> = snapshot.files().map(|(p, _)| p.as_str()).collect();
        let files = all
            .iter()
            .copied()
            .filter(|p| classify(p) == Role::Production && !excludes.is_excluded(p))
            .collect();
        Tree {
            snapshot,
            roots: layout::source_roots(&all),
            files,
        }
    }

    /// Whether `dir` is a source root or holds one.
    pub fn holds_root(&self, dir: &str) -> bool {
        self.roots
            .iter()
            .any(|r| super::map::contains(dir, &r.path))
    }
}

/// Every entry point in `tree` within `scope`, sorted.
pub fn inventory(repo: &Repo, tree: &Tree, scope: Scope) -> Result<Vec<EntryPoint>> {
    let wanted: Option<BTreeSet<&str>> = match scope {
        Scope::All => None,
        Scope::Paths(p) => Some(p.iter().map(String::as_str).collect()),
    };
    let in_scope = |p: &str| wanted.as_ref().is_none_or(|w| w.contains(p));
    let mut out: Vec<EntryPoint> = Vec::new();
    for m in layout::modules(&tree.roots, &tree.files) {
        let touched = match &wanted {
            Some(w) if m.is_dir => w.iter().any(|p| super::map::contains(&m.path, p)),
            _ => in_scope(&m.path),
        };
        if touched {
            out.push(EntryPoint {
                kind: Kind::Module,
                path: m.path,
                name: m.name,
                line: None,
            });
        }
    }
    for p in tree.files.iter().filter(|p| layout::is_migration(p)) {
        if in_scope(p) {
            out.push(EntryPoint {
                kind: Kind::Migration,
                path: p.to_string(),
                name: p.rsplit('/').next().unwrap_or(p).to_string(),
                line: None,
            });
        }
    }
    out.extend(binaries(repo, tree, &in_scope)?);
    let rust: Vec<&str> = tree
        .files
        .iter()
        .copied()
        .filter(|p| Lang::detect(p) == Some(Lang::Rust) && in_scope(p))
        .collect();
    out.extend(declared(repo, tree.snapshot, &rust)?);
    out.sort();
    out.dedup();
    Ok(out)
}

/// Binaries of every crate in the tree within scope. A changed `Cargo.toml`
/// puts all of its binaries in scope, since it can declare one for an old file.
fn binaries(repo: &Repo, tree: &Tree, in_scope: &dyn Fn(&str) -> bool) -> Result<Vec<EntryPoint>> {
    let mut blobs = BlobReader::new(repo)?;
    let mut out = Vec::new();
    for (manifest_path, entry) in tree.snapshot.files() {
        let Some(dir) = manifest_path.strip_suffix("Cargo.toml") else {
            continue;
        };
        if !(dir.is_empty() || dir.ends_with('/')) {
            continue;
        }
        let manifest = String::from_utf8_lossy(&blobs.read(&entry.oid)?).into_owned();
        let dir = dir.trim_end_matches('/');
        let manifest_changed = in_scope(manifest_path);
        for (path, name) in rust::binaries(dir, &manifest, &tree.files) {
            if !(manifest_changed || in_scope(&path)) {
                continue;
            }
            out.push(EntryPoint {
                kind: Kind::Binary,
                path,
                name,
                line: None,
            });
        }
    }
    Ok(out)
}

/// Routes and commands in the given Rust files, read in parallel.
fn declared(repo: &Repo, snapshot: &Snapshot, paths: &[&str]) -> Result<Vec<EntryPoint>> {
    let chunk = paths.len().div_ceil(8).max(1);
    let chunks: Vec<&[&str]> = paths.chunks(chunk).collect();
    let results = parallel_map(&chunks, |part| -> Result<Vec<EntryPoint>> {
        let mut reader = BlobReader::new(repo)?;
        let mut out = Vec::new();
        for path in *part {
            let Some(entry) = snapshot.get(path) else {
                continue;
            };
            let bytes = reader.read(&entry.oid)?;
            let source = String::from_utf8_lossy(&bytes);
            // Skip the parse for the many files that declare neither.
            if source.contains(".route(") || source.contains("Subcommand") {
                out.extend(rust::declared(path, &source));
            }
        }
        Ok(out)
    });
    let mut out = Vec::new();
    for part in results {
        out.extend(part?);
    }
    Ok(out)
}
