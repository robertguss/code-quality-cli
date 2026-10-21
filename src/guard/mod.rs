//! Guarded changes (KTD6): any change that loosens the gate needs the
//! developer's approval of that exact change.
//!
//! The loosening diff is a sorted list of lines naming each loosening, such as
//! a raised limit, a new baseline entry, an edited tool config or a new inline
//! ignore. Tightenings, prunes and lowered limits never appear in it.

pub mod approvals;
pub mod ignores;
mod mix_exs;
pub mod signed;

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::Result;

use crate::baseline::{self, Baseline, measure_paths};
use crate::checks::exclude::{self, Excludes};
use crate::cli::Finding;
use crate::config::{CONFIG_FILE, Config, Limits, Tool};
use crate::git::{BlobReader, Change, ChangeKind, Repo, Snapshot};

pub const RULE: &str = "unapproved-change";

/// Programs whose recipe files define what a tool command does.
const RECIPE_FILES: &[(&str, &[&str])] = &[
    ("make", &["Makefile", "makefile", "GNUmakefile"]),
    ("just", &["justfile", "Justfile", ".justfile"]),
    ("task", &["Taskfile.yml", "Taskfile.yaml"]),
];

/// Tool configs guarded in every repo: they only exist to configure an Elixir
/// tool (the formatter, Credo, Dialyxir, ExCoveralls, ExUnit's start options),
/// and Mix tools are often run through scripts the guard can't see into.
const ELIXIR_TOOL_FILES: &[&str] = &[
    ".formatter.exs",
    ".credo.exs",
    ".dialyzer_ignore.exs",
    "coveralls.json",
    "test_helper.exs",
];

/// Everything in one change that loosens the gate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loosening {
    /// Sorted, one loosening per line, each with the path it concerns when there is one.
    pub lines: Vec<(Option<String>, String)>,
}

impl Loosening {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn text(&self) -> String {
        self.lines.iter().map(|(_, l)| format!("{l}\n")).collect()
    }

    pub fn hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(self.text().as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
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

/// The loosening introduced by going from `parent` to `current`.
pub fn loosening(
    repo: &Repo,
    parent: &Snapshot,
    current: &Snapshot,
    changes: &[Change],
) -> Result<Loosening> {
    let mut blobs = BlobReader::new(repo)?;
    let mut out: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    let mut add = |path: Option<&str>, line: String| {
        out.insert((path.map(str::to_string), line));
    };

    let old_cfg = match read_text(&mut blobs, parent, CONFIG_FILE)? {
        Some(t) => Some(Config::parse_unchecked(&t, CONFIG_FILE)?),
        None => None,
    };
    let new_cfg = match read_text(&mut blobs, current, CONFIG_FILE)? {
        Some(t) => Config::parse_unchecked(&t, CONFIG_FILE)?,
        None => Config::starter(),
    };
    match &old_cfg {
        None if current.get(CONFIG_FILE).is_some() => {
            add(Some(CONFIG_FILE), format!("config: new {CONFIG_FILE}"));
        }
        Some(old) => {
            for l in config_loosenings(old, &new_cfg) {
                add(Some(CONFIG_FILE), format!("config: {l}"));
            }
        }
        None => {}
    }

    let tools: Vec<&Tool> = old_cfg
        .iter()
        .flat_map(|c| &c.tools)
        .chain(&new_cfg.tools)
        .collect();
    let config_names: HashSet<&str> = tools
        .iter()
        .flat_map(|t| &t.config_files)
        .map(|p| basename(p))
        .collect();
    let recipe_names: HashSet<&str> = RECIPE_FILES
        .iter()
        .filter(|(prog, _)| tools.iter().any(|t| first_word(&t.command) == *prog))
        .flat_map(|(_, names)| names.iter().copied())
        .collect();
    // Repo files a tool command runs directly, such as `./scripts/lint.sh`.
    let scripts: HashSet<String> = tools
        .iter()
        .flat_map(|t| t.command.split_whitespace())
        .map(|w| w.trim_start_matches("./").to_string())
        .filter(|w| parent.get(w).is_some() || current.get(w).is_some())
        .collect();
    let mut patterns: Vec<String> = ignores::BUILT_IN.iter().map(|s| s.to_string()).collect();
    patterns.extend(new_cfg.ignore_patterns.iter().cloned());
    let cur_excludes = Excludes::new(repo, current, &new_cfg)?;

    let signer_files: HashSet<&str> = old_cfg
        .iter()
        .chain(std::iter::once(&new_cfg))
        .filter_map(|c| c.approvals.allowed_signers.as_deref())
        .collect();

    for c in changes.iter().filter(|c| !signed::is_record(&c.path)) {
        if signer_files.contains(c.path.as_str()) || signer_files.contains(c.old_path()) {
            add(
                Some(&c.path),
                format!("approvers changed: {} ({})", c.path, blob_ids(c)),
            );
        }
        let paths: Vec<&str> = match &c.kind {
            ChangeKind::Renamed { from } => vec![from, &c.path],
            _ => vec![&c.path],
        };
        for p in &paths {
            let name = basename(p);
            if config_names.contains(name)
                || recipe_names.contains(name)
                || ELIXIR_TOOL_FILES.contains(&name)
                || scripts.contains(*p)
            {
                add(
                    Some(p),
                    format!("tool config edited: {p} ({}, {})", state(c, p), blob_ids(c)),
                );
            }
        }
        let old_text = read_text(&mut blobs, parent, c.old_path())?;
        let new_text = read_text(&mut blobs, current, &c.path)?;
        let (old_s, new_s) = (
            old_text.as_deref().unwrap_or(""),
            new_text.as_deref().unwrap_or(""),
        );
        match basename(&c.path) {
            "package.json" if json_section(old_s, "scripts") != json_section(new_s, "scripts") => {
                let digest =
                    content_digest(json_section(new_s, "scripts").as_deref().unwrap_or(""));
                add(
                    Some(&c.path),
                    format!("package scripts changed: {} (now {digest})", c.path),
                );
            }
            "pyproject.toml" if toml_section(old_s, "tool") != toml_section(new_s, "tool") => {
                let digest = content_digest(toml_section(new_s, "tool").as_deref().unwrap_or(""));
                add(
                    Some(&c.path),
                    format!("pyproject [tool] changed: {} (now {digest})", c.path),
                );
            }
            "mix.exs" if mix_exs::guarded_sections(old_s) != mix_exs::guarded_sections(new_s) => {
                let digest = content_digest(&mix_exs::guarded_sections(new_s));
                add(
                    Some(&c.path),
                    format!(
                        "mix.exs aliases or project settings changed: {} (now {digest})",
                        c.path
                    ),
                );
            }
            ".gitattributes" => {
                for l in attribute_loosenings(old_s, new_s) {
                    add(Some(&c.path), format!("exclusion: {} adds `{l}`", c.path));
                }
            }
            _ => {}
        }
        // A generated file is skipped wherever it lives, so a new one is never
        // a loosening, but marking existing hand-written code generated is.
        let generated = new_text
            .as_deref()
            .is_some_and(|t| exclude::is_generated(&c.path, t));
        let was_generated = old_text
            .as_deref()
            .is_some_and(|t| exclude::is_generated(c.old_path(), t));
        if generated && old_text.is_some() && !was_generated {
            add(
                Some(&c.path),
                format!(
                    "exclusion: {} now has a generated-code header ({})",
                    c.path,
                    blob_ids(c)
                ),
            );
        }
        let is_new = matches!(c.kind, ChangeKind::Added | ChangeKind::Renamed { .. });
        if is_new
            && !generated
            && !exclude::is_lockfile(&c.path)
            && cur_excludes.is_guarded(&c.path)
            && c.path != baseline::BASELINE_FILE
        {
            add(
                Some(&c.path),
                format!("exclusion: new file in excluded path {}", c.path),
            );
        }
        if new_text.is_some() {
            for (pattern, lines) in ignores::added(old_s, new_s, &patterns) {
                add(
                    Some(&c.path),
                    format!(
                        "inline ignore: {} adds {}x `{pattern}` ({})",
                        c.path,
                        lines.len(),
                        content_digest(&lines.join("\n"))
                    ),
                );
            }
        }
    }

    for l in baseline_growth(repo, parent, current, changes, &new_cfg)? {
        add(Some(baseline::BASELINE_FILE), format!("baseline: {l}"));
    }
    Ok(Loosening {
        lines: out.into_iter().collect(),
    })
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Old and new blob ids, so an approval covers only this exact content.
fn blob_ids(c: &Change) -> String {
    let short = |o: &Option<String>| {
        o.as_deref()
            .map_or("none".to_string(), |o| o[..o.len().min(12)].to_string())
    };
    format!("{} -> {}", short(&c.old_oid), short(&c.new_oid))
}

fn content_digest(text: &str) -> String {
    format!("{:016x}", crate::hash::fnv64(text.as_bytes()))
}

fn first_word(cmd: &str) -> &str {
    cmd.split_whitespace().next().map(basename).unwrap_or("")
}

fn state(c: &Change, p: &str) -> &'static str {
    match &c.kind {
        ChangeKind::Added => "added",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed { from } if from == p => "renamed away",
        ChangeKind::Renamed { .. } => "renamed here",
        ChangeKind::Modified => "modified",
    }
}

fn json_section(text: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get(key).map(|s| s.to_string())
}

fn toml_section(text: &str, key: &str) -> Option<String> {
    let v: toml::Table = toml::from_str(text).ok()?;
    v.get(key).map(|s| s.to_string())
}

/// `.gitattributes` lines added that mark files generated or vendored, or touch a merge driver.
fn attribute_loosenings(old: &str, new: &str) -> Vec<String> {
    let before: HashSet<&str> = old.lines().map(str::trim).collect();
    let after: HashSet<&str> = new.lines().map(str::trim).collect();
    let marks = |l: &str| {
        l.split_whitespace().skip(1).any(|a| {
            exclude::EXCLUDING_ATTRS
                .iter()
                .any(|attr| a == *attr || a == format!("{attr}=true") || a == format!("{attr}=set"))
        })
    };
    let mut out: Vec<String> = after
        .difference(&before)
        .filter(|l| !l.starts_with('#') && (marks(l) || l.contains("merge=")))
        .map(|l| l.to_string())
        .collect();
    out.extend(
        before
            .difference(&after)
            .filter(|l| !l.starts_with('#') && l.contains("merge="))
            .map(|l| format!("(removed) {l}")),
    );
    out.sort();
    out
}

/// Loosenings between two configs, described one per line.
pub fn config_loosenings(old: &Config, new: &Config) -> Vec<String> {
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
    if new.approvals.allowed_signers != old.approvals.allowed_signers {
        out.push(format!(
            "approvals.allowed_signers {:?} -> {:?}",
            old.approvals.allowed_signers, new.approvals.allowed_signers
        ));
    }
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
    for p in new.exclude.iter().filter(|p| !old.exclude.contains(p)) {
        out.push(format!("exclude adds `{p}`"));
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

/// Baseline entries added or raised, except entries that followed a verified move.
fn baseline_growth(
    repo: &Repo,
    parent: &Snapshot,
    current: &Snapshot,
    changes: &[Change],
    config: &Config,
) -> Result<Vec<String>> {
    let old = baseline::from_snapshot(repo, parent)?;
    let new = baseline::from_snapshot(repo, current)?;
    let added: Vec<_> = new
        .entries
        .iter()
        .filter(|(k, _)| old.get(k).is_none())
        .collect();
    let mut out: Vec<String> = new
        .entries
        .iter()
        .filter_map(|(k, v)| {
            let was = old.get(k)?;
            v.exceeds(was)
                .then(|| format!("raised {} {was} -> {v}", k.describe()))
        })
        .collect();
    if added.is_empty() {
        return Ok(out);
    }
    let moves = verified_moves(repo, parent, current, changes, config, &old)?;
    for (k, v) in added {
        let ok = moves.get(k).is_some_and(|from| {
            new.get(from).is_none() && old.get(from).is_some_and(|was| !v.exceeds(was))
        });
        if !ok {
            out.push(format!("added {} = {v}", k.describe()));
        }
    }
    Ok(out)
}

fn verified_moves(
    repo: &Repo,
    parent: &Snapshot,
    current: &Snapshot,
    changes: &[Change],
    config: &Config,
    old: &Baseline,
) -> Result<std::collections::HashMap<baseline::Key, baseline::Key>> {
    let new_paths: Vec<String> = changes
        .iter()
        .filter(|c| !c.is_deleted())
        .map(|c| c.path.clone())
        .collect();
    let old_paths: Vec<String> = changes
        .iter()
        .filter(|c| c.old_oid.is_some())
        .map(|c| c.old_path().to_string())
        .collect();
    let cur = measure_paths(
        repo,
        current,
        &Excludes::new(repo, current, config)?,
        config,
        &new_paths,
    )?;
    let par = measure_paths(
        repo,
        parent,
        &Excludes::new(repo, parent, config)?,
        config,
        &old_paths,
    )?;
    Ok(baseline::find_moves(old, &par, &cur, changes))
}

/// Whether `loosening` is empty, approved in this clone, or approved by a
/// signed record committed to the repo.
pub fn is_approved(repo: &Repo, loosening: &Loosening, committed: &signed::Committed) -> bool {
    loosening.is_empty()
        || approvals::is_approved(repo, &loosening.hash())
        || committed.approves(loosening)
}

/// Findings for an unapproved loosening; empty when there is none or it is approved.
pub fn check(
    repo: &Repo,
    loosening: &Loosening,
    committed: &signed::Committed,
    fix: &str,
) -> Vec<Finding> {
    if is_approved(repo, loosening, committed) {
        return Vec::new();
    }
    grouped(loosening)
        .into_iter()
        .map(|(path, line)| {
            let f = Finding::error(RULE, format!("{line}; needs the developer's approval"))
                .with_fix(fix.to_string());
            match path {
                Some(p) => f.at(p, None),
                None => f,
            }
        })
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
