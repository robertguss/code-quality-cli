//! Which files the built-in checks skip (KTD12).

use std::collections::HashSet;
use std::io::Write;
use std::process::Stdio;

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::baseline::BASELINE_FILE;
use crate::config::Config;
use crate::git::{Repo, Snapshot, Source, split_nul};
use crate::grammars::Lang;

/// Always excluded: dependency and build output directories, minified files, lockfiles.
pub const DEFAULT_GLOBS: &[&str] = &[
    "**/vendor/**",
    "**/node_modules/**",
    "**/dist/**",
    "**/*.min.*",
];

pub const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
    "Cargo.lock",
    "Gemfile.lock",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
    "mix.lock",
    "Podfile.lock",
    "pubspec.lock",
    "packages.lock.json",
];

pub const EXCLUDING_ATTRS: &[&str] = &["linguist-generated", "linguist-vendored"];

fn glob_set<'a>(patterns: impl IntoIterator<Item = &'a str>) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = Glob::new(p) {
            b.add(g);
        }
    }
    b.build().expect("glob set builds")
}

pub fn is_lockfile(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    LOCKFILES.contains(&name)
}

/// Excluded by the built-in defaults, regardless of config or attributes.
pub fn matches_default(path: &str) -> bool {
    use std::sync::OnceLock;
    static SET: OnceLock<GlobSet> = OnceLock::new();
    let set = SET.get_or_init(|| glob_set(DEFAULT_GLOBS.iter().copied()));
    set.is_match(path) || is_lockfile(path)
}

pub struct Excludes {
    config_globs: GlobSet,
    fixture_globs: GlobSet,
    by_attribute: HashSet<String>,
}

impl Excludes {
    /// Exclusions for every file in `snapshot`, reading `.gitattributes` from that same snapshot.
    pub fn new(repo: &Repo, snapshot: &Snapshot, config: &Config) -> Result<Excludes> {
        let paths: Vec<&str> = snapshot.files().map(|(p, _)| p.as_str()).collect();
        Ok(Excludes {
            config_globs: glob_set(config.exclude.iter().map(String::as_str)),
            fixture_globs: glob_set(config.fixtures.iter().map(String::as_str)),
            by_attribute: marked_paths(repo, &snapshot.source, &paths)?,
        })
    }

    pub fn is_excluded(&self, path: &str) -> bool {
        self.is_guarded(path) || self.is_fixture(path)
    }

    /// Excluded by a rule that hides code, so a new file here needs approval.
    pub fn is_guarded(&self, path: &str) -> bool {
        path == BASELINE_FILE
            || crate::guard::signed::is_record(path)
            || matches_default(path)
            || self.config_globs.is_match(path)
            || self.by_attribute.contains(path)
    }

    /// Under a fixture glob and not a language qgate parses for functions.
    /// Parseable source in a fixture path is checked like any other code, so
    /// fixtures cannot hide real functions.
    pub fn is_fixture(&self, path: &str) -> bool {
        self.fixture_globs.is_match(path) && Lang::detect(path).is_none()
    }
}

/// Paths marked `linguist-generated` or `linguist-vendored` in the snapshot's `.gitattributes`.
pub fn marked_paths(repo: &Repo, source: &Source, paths: &[&str]) -> Result<HashSet<String>> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut cmd = repo.command();
    // Only attributes in the tree count: a user or system attributes file would
    // exclude code without any change the guard can see.
    cmd.env("GIT_ATTR_NOSYSTEM", "1").args([
        "-c",
        "core.attributesFile=/dev/null",
        "check-attr",
        "-z",
        "--stdin",
    ]);
    match source {
        Source::Index => cmd.arg("--cached"),
        Source::Tree(tree) => cmd.arg(format!("--source={tree}")),
    };
    cmd.args(EXCLUDING_ATTRS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().context("running git check-attr")?;
    let mut stdin = child.stdin.take().context("check-attr stdin")?;
    let input: Vec<u8> = paths.iter().flat_map(|p| p.bytes().chain([0])).collect();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output()?;
    writer
        .join()
        .expect("writer thread")
        .context("writing to check-attr")?;
    let fields = split_nul(&out.stdout);
    Ok(fields
        .chunks(3)
        .filter(|c| c.len() == 3 && (c[2] == "set" || c[2] == "true"))
        .map(|c| c[0].clone())
        .collect())
}
