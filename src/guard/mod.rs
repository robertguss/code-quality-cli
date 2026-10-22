//! Loosenings: changes that weaken the gate, reported as warnings so the
//! developer sees them in the run output. Loosenings of an always-block rule
//! block instead (D42, KTD15), and new inline suppressions are judged by R6.
//!
//! The loosening diff is a sorted list of lines naming each loosening, such as
//! a raised limit, a new baseline entry, an edited tool config or a new inline
//! ignore. Tightenings, prunes and lowered limits never appear in it.

mod config;
mod files;
pub mod ignores;
mod mix_exs;
pub mod suppressions;

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::Result;

use crate::cli::Finding;
use crate::config::{CONFIG_FILE, Config, Limits, Tool};
use crate::date::Date;
use crate::git::{BlobReader, Change, Repo, Snapshot};

pub use config::config_loosenings;

pub const RULE: &str = "loosening";

/// A person's way through a blocking loosening; the agent hook refuses both for agents.
pub const BLOCK_FIX: &str = "git commit --no-verify or git push --no-verify (people only; the agent hook blocks both for agents)";

/// Everything in one change that loosens the gate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loosening {
    /// Sorted, one loosening per line, each with the path it concerns when there is one.
    pub lines: Vec<(Option<String>, String)>,
    /// Loosenings of an always-block rule, shown as errors (KTD15). Sorted like `lines`.
    pub blocks: Vec<(Option<String>, String)>,
    /// R6 suppression errors and the adoption info line (KTD16).
    pub findings: Vec<Finding>,
}

impl Loosening {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.blocks.is_empty() && self.findings.is_empty()
    }

    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .chain(&self.lines)
            .map(|(_, l)| format!("{l}\n"))
            .collect()
    }
}

/// A text file's content in a snapshot; `None` when absent or binary.
fn read_text(blobs: &mut BlobReader, snap: &Snapshot, path: &str) -> Result<Option<String>> {
    match snap.get(path) {
        Some(e) => {
            let b = blobs.read(&e.oid)?;
            Ok((!crate::git::index::is_binary(&b))
                .then(|| String::from_utf8_lossy(&b).into_owned()))
        }
        None => Ok(None),
    }
}

/// The configs a change is judged by.
struct Configs {
    /// The parent's `qgate.toml`, when it has one.
    old: Option<Config>,
    /// The parent's config, or the starter config when it has none.
    parent: Config,
    /// The current `qgate.toml`, or the starter config when there is none.
    new: Config,
    /// Both configs' always-block lists: removing an entry blocks on its own,
    /// so the change is judged by both.
    always_block: Vec<String>,
}

impl Configs {
    fn load(blobs: &mut BlobReader, parent: &Snapshot, current: &Snapshot) -> Result<Self> {
        let old = match read_text(blobs, parent, CONFIG_FILE)? {
            Some(t) => Some(Config::parse_unchecked(&t, CONFIG_FILE)?),
            None => None,
        };
        let new = match read_text(blobs, current, CONFIG_FILE)? {
            Some(t) => Config::parse_unchecked(&t, CONFIG_FILE)?,
            None => Config::starter(),
        };
        let parent = old.clone().unwrap_or_else(Config::starter);
        let mut always_block = parent.suppressions.always_block.clone();
        always_block.extend(new.suppressions.always_block.iter().cloned());
        Ok(Configs {
            old,
            parent,
            new,
            always_block,
        })
    }
}

/// Loosenings found so far, kept sorted and without repeats.
#[derive(Default)]
struct Found {
    lines: BTreeSet<(Option<String>, String)>,
    blocks: BTreeSet<(Option<String>, String)>,
    findings: Vec<Finding>,
}

impl Found {
    /// Adds a warning line.
    fn add(&mut self, path: Option<&str>, line: String) {
        self.lines.insert((path.map(str::to_string), line));
    }

    /// Adds a line about `path`, as a block when `blocking`.
    fn flag(&mut self, path: &str, line: String, blocking: bool) {
        let set = if blocking {
            &mut self.blocks
        } else {
            &mut self.lines
        };
        set.insert((Some(path.to_string()), line));
    }
}

/// The loosening introduced by going from `parent` to `current`, on the
/// change's date `today` (KTD11).
pub fn loosening(
    repo: &Repo,
    parent: &Snapshot,
    current: &Snapshot,
    changes: &[Change],
    today: Date,
) -> Result<Loosening> {
    let mut blobs = BlobReader::new(repo)?;
    let configs = Configs::load(&mut blobs, parent, current)?;
    let mut found = Found::default();
    config::flag_config_and_baseline(repo, parent, current, changes, &configs, &mut found)?;
    let patterns = files::ignore_patterns(&configs.new);
    let checks = files::FileChecks::new(repo, parent, current, &configs, &patterns, today)?;
    for c in changes {
        checks.check(c, &mut blobs, parent, current, &mut found)?;
    }
    Ok(Loosening {
        lines: found.lines.into_iter().collect(),
        blocks: found.blocks.into_iter().collect(),
        findings: found.findings,
    })
}

/// Config loosenings that only warn.
fn warning_loosenings(old: &Config, new: &Config) -> Vec<String> {
    let mut out = Vec::new();
    if old.qgate_version != new.qgate_version {
        out.push(format!(
            "qgate_version {} -> {}",
            old.qgate_version, new.qgate_version
        ));
    }
    let mut dirs: BTreeSet<String> = [String::new()].into();
    dirs.extend(
        old.packages
            .iter()
            .chain(&new.packages)
            .filter_map(|p| p.path.as_deref())
            .map(|p| p.trim_end_matches('/').to_string()),
    );
    for dir in dirs {
        let probe = if dir.is_empty() {
            "_".to_string()
        } else {
            format!("{dir}/_")
        };
        let (a, b) = (old.dir_limits_for(&probe), new.dir_limits_for(&probe));
        let at = if dir.is_empty() {
            String::new()
        } else {
            format!(" in {dir}")
        };
        if b.function_lines > a.function_lines {
            out.push(format!(
                "function_lines{at} {} -> {}",
                a.function_lines, b.function_lines
            ));
        }
        if b.file_lines > a.file_lines {
            out.push(format!(
                "file_lines{at} {} -> {}",
                a.file_lines, b.file_lines
            ));
        }
    }
    out.extend(glob_loosenings(old, new));
    if new.ratchet.headroom_lines > old.ratchet.headroom_lines {
        out.push(format!(
            "ratchet.headroom_lines {} -> {}",
            old.ratchet.headroom_lines, new.ratchet.headroom_lines
        ));
    }
    if old.duplication.enabled && !new.duplication.enabled {
        out.push("duplication disabled".into());
    }
    if new.duplication.min_tokens > old.duplication.min_tokens {
        out.push(format!(
            "duplication.min_tokens {} -> {}",
            old.duplication.min_tokens, new.duplication.min_tokens
        ));
    }
    if new.suppressions.max_days > old.suppressions.max_days {
        out.push(format!(
            "suppressions.max_days {} -> {}",
            old.suppressions.max_days, new.suppressions.max_days
        ));
    }
    for g in new
        .output_globs()
        .iter()
        .filter(|g| !old.output_globs().contains(g))
    {
        out.push(format!("rules.debug-output allow adds `{g}`"));
    }
    // Blanking can hide code from the parser, so every new macro is guarded.
    for m in new
        .attribute_macros
        .iter()
        .filter(|m| !old.attribute_macros.contains(m))
    {
        out.push(format!("attribute_macros adds `{m}`"));
    }
    for p in new.fixtures.iter().filter(|p| !old.fixtures.contains(p)) {
        out.push(format!("fixtures adds `{p}`"));
    }
    for p in old
        .ignore_patterns
        .iter()
        .filter(|p| !new.ignore_patterns.contains(p))
    {
        out.push(format!("ignore_patterns drops `{p}`"));
    }
    let new_tools: BTreeMap<&str, &Tool> = new.tools.iter().map(|t| (t.name.as_str(), t)).collect();
    let old_tools: BTreeMap<&str, &Tool> = old.tools.iter().map(|t| (t.name.as_str(), t)).collect();
    for t in &old.tools {
        match new_tools.get(t.name.as_str()) {
            None => out.push(format!("tool `{}` removed", t.name)),
            Some(n) if *n != t => {
                out.push(format!("tool `{}` changed: {}", t.name, describe_tool(n)))
            }
            _ => {}
        }
    }
    // A new tool runs arbitrary commands inside the hook, so it is guarded too.
    for t in new
        .tools
        .iter()
        .filter(|t| !old_tools.contains_key(t.name.as_str()))
    {
        out.push(format!("tool `{}` added: {}", t.name, describe_tool(t)));
    }
    out
}

/// Loosenings from `[[package]] glob` overrides. A glob can match files under
/// any directory, so a new or raised glob limit is compared with the lowest
/// limit the old config set anywhere, and a removed one with the highest
/// limit the new config sets anywhere.
fn glob_loosenings(old: &Config, new: &Config) -> Vec<String> {
    type Field = fn(&Limits) -> u32;
    type PkgField = fn(&crate::config::Package) -> Option<u32>;
    let fields: [(&str, Field, PkgField); 2] = [
        ("function_lines", |l| l.function_lines, |p| p.function_lines),
        ("file_lines", |l| l.file_lines, |p| p.file_lines),
    ];
    let all = |c: &Config, f: Field, pf: PkgField| -> Vec<u32> {
        std::iter::once(f(&c.limits))
            .chain(c.packages.iter().filter_map(pf))
            .collect()
    };
    let last = |c: &Config, glob: &str, pf: PkgField| -> Option<u32> {
        c.packages
            .iter()
            .filter(|p| p.glob.as_deref() == Some(glob))
            .filter_map(pf)
            .next_back()
    };
    let globs: BTreeSet<&str> = old
        .packages
        .iter()
        .chain(&new.packages)
        .filter_map(|p| p.glob.as_deref())
        .collect();
    let mut out = Vec::new();
    for glob in globs {
        for (name, f, pf) in fields {
            match (last(old, glob, pf), last(new, glob, pf)) {
                (Some(a), Some(b)) if b > a => {
                    out.push(format!("{name} in `{glob}` {a} -> {b}"));
                }
                (None, Some(b)) => {
                    let floor = all(old, f, pf).into_iter().min().unwrap_or(b);
                    if b > floor {
                        out.push(format!(
                            "{name} in `{glob}` set to {b} (lowest was {floor})"
                        ));
                    }
                }
                (Some(a), None) => {
                    let ceiling = all(new, f, pf).into_iter().max().unwrap_or(a);
                    if ceiling > a {
                        out.push(format!("{name} override {a} in `{glob}` removed"));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn describe_tool(t: &Tool) -> String {
    toml::to_string(t)
        .unwrap_or_default()
        .replace('\n', "; ")
        .trim_end_matches("; ")
        .to_string()
}

/// R6 findings, one error per blocking line and one warning per (grouped)
/// loosening line; empty when there is none.
pub fn check(loosening: &Loosening) -> Vec<Finding> {
    let at = |f: Finding, path: Option<String>| match path {
        Some(p) => f.at(p, None),
        None => f,
    };
    let blocking = loosening.blocks.iter().map(|(path, line)| {
        let f = Finding::error(RULE, format!("{line} loosens an always-block rule"))
            .with_fix(BLOCK_FIX);
        at(f, path.clone())
    });
    let warnings = grouped(loosening).into_iter().map(|(path, line)| {
        at(
            Finding::warning(RULE, format!("{line} loosens the gate")),
            path,
        )
    });
    loosening
        .findings
        .iter()
        .cloned()
        .chain(blocking)
        .chain(warnings)
        .collect()
}

/// More than this many loosenings of one kind are shown as one line.
const GROUP_OVER: usize = 5;

/// Loosening lines for display, with large groups of one kind (the text
/// before the first `: `, such as `baseline`) collapsed into a count.
pub fn grouped(loosening: &Loosening) -> Vec<(Option<String>, String)> {
    let kind = |l: &str| l.split_once(": ").map_or(l, |(k, _)| k).to_string();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, l) in &loosening.lines {
        *counts.entry(kind(l)).or_default() += 1;
    }
    let mut shown: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (path, l) in &loosening.lines {
        let k = kind(l);
        let n = counts[&k];
        if n <= GROUP_OVER {
            out.push((path.clone(), l.clone()));
        } else if shown.insert(k.clone()) {
            let example = l.split_once(": ").map_or(l.as_str(), |(_, e)| e);
            out.push((None, format!("{k}: {n} changes, such as {example}")));
        }
    }
    out
}
