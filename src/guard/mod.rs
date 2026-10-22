//! Loosenings: changes that weaken the gate, reported as warnings so the
//! developer sees them in the run output. Loosenings of an always-block rule
//! block instead (D42, KTD15), and new inline suppressions are judged by R6.
//!
//! The loosening diff is a sorted list of lines naming each loosening, such as
//! a raised limit, a new baseline entry, an edited tool config or a new inline
//! ignore. Tightenings, prunes and lowered limits never appear in it.

pub mod ignores;
mod mix_exs;
pub mod suppressions;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::Result;

use crate::baseline::{self, Baseline, Key, Measured, Value, measure_paths};
use crate::checks::exclude::{self, Excludes};
use crate::cli::Finding;
use crate::config::{CONFIG_FILE, Config, Limits, Tool, compare_versions};
use crate::date::Date;
use crate::git::{BlobReader, Change, ChangeKind, Repo, Snapshot};
use crate::rules::{self, Level, Rule};

pub const RULE: &str = "loosening";

/// A person's way through a blocking loosening; the agent hook refuses both for agents.
pub const BLOCK_FIX: &str = "git commit --no-verify or git push --no-verify (people only; the agent hook blocks both for agents)";

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
    let mut out: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    let mut blocks: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    let mut findings: Vec<Finding> = Vec::new();

    let old_cfg = match read_text(&mut blobs, parent, CONFIG_FILE)? {
        Some(t) => Some(Config::parse_unchecked(&t, CONFIG_FILE)?),
        None => None,
    };
    let new_cfg = match read_text(&mut blobs, current, CONFIG_FILE)? {
        Some(t) => Config::parse_unchecked(&t, CONFIG_FILE)?,
        None => Config::starter(),
    };
    let parent_cfg = old_cfg.clone().unwrap_or_else(Config::starter);
    // Removing an entry blocks on its own, so the change is judged by both lists.
    let mut always_block = parent_cfg.suppressions.always_block.clone();
    always_block.extend(new_cfg.suppressions.always_block.iter().cloned());
    let parent_counts = ParentCounts {
        repo,
        parent,
        excludes: Excludes::new(repo, parent, &parent_cfg)?,
        config: &new_cfg,
        always_block: &always_block,
    };
    let mut config_lines = Vec::new();
    match &old_cfg {
        None if current.get(CONFIG_FILE).is_some() => {
            config_lines.push((format!("new {CONFIG_FILE}"), false));
        }
        Some(old) => {
            config_lines = config_loosenings(old, &new_cfg);
            config_lines.extend(exclude_loosenings(old, &new_cfg, &parent_counts)?);
        }
        None => {}
    }
    let growth = baseline_growth(changes, current, old_cfg.as_ref(), &new_cfg, &parent_counts)?;
    let flagged = config_lines
        .into_iter()
        .map(|(l, b)| (CONFIG_FILE, format!("config: {l}"), b))
        .chain(
            growth
                .lines
                .into_iter()
                .map(|(l, b)| (baseline::BASELINE_FILE, format!("baseline: {l}"), b)),
        );
    for (path, line, blocking) in flagged {
        let set = if blocking { &mut blocks } else { &mut out };
        set.insert((Some(path.to_string()), line));
    }
    if growth.adopted > 0 {
        findings.push(
            Finding::info(
                RULE,
                format!(
                    "baseline: adopting qgate with {} as the day-one baseline",
                    count_of(growth.adopted, "entry", "entries")
                ),
            )
            .at(baseline::BASELINE_FILE, None),
        );
    }
    let mut add = |path: Option<&str>, line: String| {
        out.insert((path.map(str::to_string), line));
    };

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
    let policy = suppressions::Policy {
        patterns: &patterns,
        always_block: &always_block,
        // The parent's horizon, so raising it does not help the change that raises it.
        max_days: old_cfg.as_ref().unwrap_or(&new_cfg).suppressions.max_days,
        today,
    };

    for c in changes {
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
        // A generated file is skipped wherever it lives, so a new one, or
        // existing code marked generated, hides code.
        let generated = new_text
            .as_deref()
            .is_some_and(|t| exclude::is_generated(&c.path, t));
        let was_generated = old_text
            .as_deref()
            .is_some_and(|t| exclude::is_generated(c.old_path(), t));
        if generated && !was_generated {
            let line = match old_text {
                None => format!("exclusion: new generated file {}", c.path),
                Some(_) => format!(
                    "exclusion: {} now has a generated-code header ({})",
                    c.path,
                    blob_ids(c)
                ),
            };
            add(Some(&c.path), line);
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
        // qgate.toml names markers in `always_block`; its own diff judges it.
        let code = new_text.is_some() && c.path != CONFIG_FILE;
        if code && !cur_excludes.is_excluded(&c.path) {
            let judged = suppressions::judge(&c.path, old_s, new_s, &policy);
            findings.extend(judged.findings);
            for l in judged.lines {
                add(Some(&c.path), l);
            }
        }
        if code {
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

    Ok(Loosening {
        lines: out.into_iter().collect(),
        blocks: blocks.into_iter().collect(),
        findings,
    })
}

fn count_of(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Counts of rules in the parent snapshot, measured with the new config's
/// rules and the parent's excludes, for judging what a change hides or
/// records (KTD15, KTD16).
struct ParentCounts<'a> {
    repo: &'a Repo,
    parent: &'a Snapshot,
    excludes: Excludes,
    config: &'a Config,
    always_block: &'a [String],
}

impl ParentCounts<'_> {
    fn measure(&self, paths: &[String]) -> Result<Measured> {
        measure_paths(self.repo, self.parent, &self.excludes, self.config, paths)
    }

    fn blocks(&self, rule: &Rule) -> bool {
        always_blocks(self.always_block, rule)
    }
}

/// Whether a list of always-block names covers `rule`.
fn always_blocks(list: &[String], rule: &Rule) -> bool {
    list.iter().any(|n| rules::always_blocked(n, rule))
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Old and new blob ids, naming the exact content that changed.
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

/// Loosenings between two configs, one per line, each with whether it
/// blocks (KTD15). New `exclude` globs are judged by `exclude_loosenings`.
pub fn config_loosenings(old: &Config, new: &Config) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = warning_loosenings(old, new)
        .into_iter()
        .map(|l| (l, false))
        .collect();
    let mut always_block = old.suppressions.always_block.clone();
    always_block.extend(new.suppressions.always_block.iter().cloned());
    out.extend(level_loosenings(old, new, &always_block));
    for e in old
        .suppressions
        .always_block
        .iter()
        .filter(|e| !new.suppressions.always_block.contains(e))
    {
        out.push((format!("suppressions.always_block drops `{e}`"), true));
    }
    out
}

/// Each rule whose level fell, as `rule <id> error -> warn`, compressed to the
/// family when every rule of the family fell the same way.
fn level_loosenings(old: &Config, new: &Config, always_block: &[String]) -> Vec<(String, bool)> {
    let fell: Vec<(&Rule, Level, Level, bool)> = rules::levels(old)
        .into_iter()
        .zip(rules::levels(new))
        .filter(|((_, a), (_, b))| b < a)
        .map(|((r, a), (_, b))| (r, a, b, always_blocks(always_block, r)))
        .collect();
    let mut families: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for &(r, a, b, blocks) in &fell {
        let family = r.family();
        let whole_family = r.id.contains('/')
            && rules::all()
                .iter()
                .filter(|m| m.id.contains('/') && m.family() == family)
                .all(|m| {
                    fell.iter()
                        .any(|f| f.0.id == m.id && (f.1, f.2, f.3) == (a, b, blocks))
                });
        let name = if whole_family { family } else { r.id };
        if whole_family && !families.insert(family) {
            continue;
        }
        out.push((
            format!("rule {name} {} -> {}", a.as_str(), b.as_str()),
            blocks,
        ));
    }
    out
}

/// New `exclude` globs; one blocks when it covers parent files with
/// always-block occurrences (KTD15).
fn exclude_loosenings(
    old: &Config,
    new: &Config,
    parent: &ParentCounts,
) -> Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for p in new.exclude.iter().filter(|p| !old.exclude.contains(p)) {
        let hidden = match globset::Glob::new(p) {
            Ok(g) => {
                let m = g.compile_matcher();
                let paths: Vec<String> = parent
                    .parent
                    .files()
                    .map(|(f, _)| f)
                    .filter(|f| m.is_match(f.as_str()))
                    .cloned()
                    .collect();
                let measured = parent.measure(&paths)?;
                measured
                    .occurrences
                    .values()
                    .flatten()
                    .filter(|o| rules::find(o.rule).is_some_and(|r| parent.blocks(r)))
                    .count()
            }
            Err(_) => 0,
        };
        out.push((format!("exclude adds `{p}`"), hidden > 0));
    }
    Ok(out)
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

/// Baseline growth: lines with whether each blocks, plus entries recorded
/// as the day-one baseline.
#[derive(Default)]
struct Growth {
    lines: Vec<(String, bool)>,
    adopted: usize,
}

/// Baseline entries added or raised, except entries that followed a verified
/// move. Count entries for an always-block rule above the parent's count
/// block (KTD15). Adoption and upgrades record entries up to the parent's
/// count without a line per entry (KTD16).
fn baseline_growth(
    changes: &[Change],
    current: &Snapshot,
    old_cfg: Option<&Config>,
    config: &Config,
    parent: &ParentCounts,
) -> Result<Growth> {
    let old = baseline::from_snapshot(parent.repo, parent.parent)?;
    let new = baseline::from_snapshot(parent.repo, current)?;
    let mut grown: Vec<(&Key, Option<Value>, Value)> = new
        .entries
        .iter()
        .filter_map(|(k, &v)| match old.get(k) {
            Some(was) => v.exceeds(was).then_some((k, Some(was), v)),
            None => Some((k, None, v)),
        })
        .collect();
    if grown.iter().any(|(_, was, _)| was.is_none()) {
        let moves = verified_moves(parent, current, changes, config, &old)?;
        grown.retain(|(k, was, v)| {
            was.is_some()
                || !moves.get(*k).is_some_and(|from| {
                    new.get(from).is_none() && old.get(from).is_some_and(|w| !v.exceeds(w))
                })
        });
    }
    let mut out = Growth::default();
    if grown.is_empty() {
        return Ok(out);
    }
    let paths: Vec<String> = grown
        .iter()
        .filter(|(_, _, v)| matches!(v, Value::Count(_)))
        .map(|(k, _, _)| k.path.clone())
        .collect();
    let measured = parent.measure(&paths)?;
    let adoption = parent.parent.get(CONFIG_FILE).is_none()
        && parent.parent.get(baseline::BASELINE_FILE).is_none();
    let upgrade_from = old_cfg
        .map(|c| c.qgate_version.as_str())
        .filter(|v| compare_versions(v, &config.qgate_version) != Ordering::Equal);
    let mut upgraded: BTreeSet<&str> = BTreeSet::new();
    let mut upgraded_entries = 0;
    for (k, was, v) in grown {
        let rule = rules::find(&k.rule);
        // Exempt only up to what the parent has; above that is ordinary growth.
        let within = match v {
            Value::Count(n) => n <= measured.count(&k.rule, &k.path).unwrap_or(0),
            _ => true,
        };
        if adoption && within {
            out.adopted += 1;
            continue;
        }
        let new_rule =
            |from: &str| rule.is_some_and(|r| compare_versions(r.since, from) == Ordering::Greater);
        if was.is_none() && within && upgrade_from.is_some_and(new_rule) {
            upgraded.insert(&k.rule);
            upgraded_entries += 1;
            continue;
        }
        let line = match was {
            Some(was) => format!("raised {} {was} -> {v}", k.describe()),
            None => format!("added {} = {v}", k.describe()),
        };
        let blocks =
            matches!(v, Value::Count(_)) && !within && rule.is_some_and(|r| parent.blocks(r));
        out.lines.push((line, blocks));
    }
    if upgraded_entries > 0 {
        out.lines.push((
            format!(
                "upgrade to {} adds {} for {}",
                config.qgate_version,
                count_of(upgraded_entries, "entry", "entries"),
                count_of(upgraded.len(), "new rule", "new rules")
            ),
            false,
        ));
    }
    Ok(out)
}

/// Baselined keys that moved with their code, new key -> old key. A count
/// entry moves with a whole-file rename (KTD7).
fn verified_moves(
    parent_counts: &ParentCounts,
    current: &Snapshot,
    changes: &[Change],
    config: &Config,
    old: &Baseline,
) -> Result<std::collections::HashMap<Key, Key>> {
    let (repo, parent) = (parent_counts.repo, parent_counts.parent);
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
