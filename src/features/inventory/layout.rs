//! Source roots, top-level modules and migrations, from paths alone (KTD4, KTD14).

use std::collections::{BTreeMap, BTreeSet};

use crate::grammars::Lang;

/// A directory whose direct children are top-level modules.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Root {
    pub path: String,
    /// Beside a `Cargo.toml`: `bin/` holds binaries, not a module.
    pub cargo: bool,
    /// Under a `go.mod`: only directories holding Go files are modules.
    pub go: bool,
}

/// Crate and package entry files, which are not modules themselves.
const ENTRY_FILES: &[&str] = &[
    "main.rs",
    "lib.rs",
    "mod.rs",
    "__init__.py",
    "index.ts",
    "index.tsx",
    "index.js",
    "index.jsx",
];

/// Go directories expanded one more level into modules.
const GO_GROUPS: &[&str] = &["cmd", "internal", "pkg"];

const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "mix.exs",
    "pyproject.toml",
    "setup.py",
    "go.mod",
];

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// Every directory that holds a file, at any depth.
fn directories(paths: &[&str]) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    for p in paths {
        let mut dir = split(p).0;
        while !dir.is_empty() && dirs.insert(dir.to_string()) {
            dir = split(dir).0;
        }
    }
    dirs
}

/// Direct child directories of `dir`.
fn children<'a>(dirs: &'a BTreeSet<String>, dir: &'a str) -> impl Iterator<Item = &'a String> {
    dirs.iter().filter(move |d| split(d).0 == dir)
}

/// The source roots in a tree, found from its manifests.
pub fn source_roots(paths: &[&str]) -> Vec<Root> {
    let dirs = directories(paths);
    let files: BTreeSet<&str> = paths.iter().copied().collect();
    let mut roots = BTreeSet::new();
    let mut add = |path: String, cargo: bool, go: bool| {
        roots.insert(Root { path, cargo, go });
    };
    for p in paths {
        let (dir, name) = split(p);
        match name {
            "Cargo.toml" | "package.json" if dirs.contains(&join(dir, "src")) => {
                add(join(dir, "src"), name == "Cargo.toml", false);
            }
            "mix.exs" if dirs.contains(&join(dir, "lib")) => {
                let lib = join(dir, "lib");
                let apps: Vec<String> = children(&dirs, &lib).cloned().collect();
                if apps.is_empty() {
                    add(lib, false, false);
                }
                for app in apps {
                    add(app, false, false);
                }
            }
            "pyproject.toml" | "setup.py" => {
                for base in [dir.to_string(), join(dir, "src")] {
                    for pkg in children(&dirs, &base) {
                        if files.contains(join(pkg, "__init__.py").as_str()) {
                            add(pkg.clone(), false, false);
                        }
                    }
                }
            }
            "go.mod" => {
                add(dir.to_string(), false, true);
                for group in GO_GROUPS {
                    let g = join(dir, group);
                    if dirs.contains(&g) {
                        add(g, false, true);
                    }
                }
            }
            _ => {}
        }
    }
    let manifest_at_root = MANIFESTS.iter().any(|m| files.contains(m));
    if !manifest_at_root && dirs.contains("src") {
        add("src".to_string(), false, false);
    }
    roots.into_iter().collect()
}

/// A top-level module: a direct child of a source root.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Module {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
}

/// The top-level modules among `files` (production files only).
pub fn modules(roots: &[Root], files: &[&str]) -> Vec<Module> {
    let mut out: BTreeMap<String, Module> = BTreeMap::new();
    for root in roots {
        let prefix = if root.path.is_empty() {
            String::new()
        } else {
            format!("{}/", root.path)
        };
        for f in files {
            let Some(rest) = f.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(m) = module_of(root, rest) {
                let path = join(&root.path, &m.0);
                out.entry(path.clone()).or_insert(Module {
                    path,
                    name: m.1,
                    is_dir: m.2,
                });
            }
        }
    }
    out.into_values().collect()
}

/// The module a file `rest` (relative to `root`) belongs to: its child path, name and whether it is a directory.
fn module_of(root: &Root, rest: &str) -> Option<(String, String, bool)> {
    let (first, deeper) = match rest.split_once('/') {
        Some((first, _)) => (first, true),
        None => (rest, false),
    };
    if first.starts_with('.') {
        return None;
    }
    if deeper {
        let skip = (root.cargo && first == "bin")
            || (root.go && (GO_GROUPS.contains(&first) || !rest.ends_with(".go")));
        return (!skip).then(|| (first.to_string(), first.to_string(), true));
    }
    if root.go || ENTRY_FILES.contains(&first) || Lang::detect(first).is_none() {
        return None;
    }
    let stem = first.rsplit_once('.').map_or(first, |(s, _)| s);
    Some((first.to_string(), stem.to_string(), false))
}

/// A migration file, under any `migrations/` directory or `alembic/versions/`.
pub fn is_migration(path: &str) -> bool {
    let (dir, name) = split(path);
    let in_dir = dir.split('/').any(|s| s == "migrations")
        || dir == "alembic/versions"
        || dir.ends_with("/alembic/versions");
    let code = name.ends_with(".sql") || Lang::detect(name).is_some();
    in_dir && code && name != "__init__.py"
}
