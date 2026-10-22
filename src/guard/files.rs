//! Loosenings in the files a change touches: edited tool configs, recipes and
//! scripts, changed manifest sections, new exclusions, and new inline
//! suppressions and ignores.

use std::collections::HashSet;

use anyhow::Result;

use super::{Configs, Found, ignores, mix_exs, read_text, suppressions};
use crate::baseline;
use crate::checks::exclude::{self, Excludes};
use crate::config::{CONFIG_FILE, Tool};
use crate::date::Date;
use crate::git::{BlobReader, Change, ChangeKind, Repo, Snapshot};

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

/// Inline-ignore markers to look for: the built-in table and the config's own.
pub(super) fn ignore_patterns(config: &crate::config::Config) -> Vec<String> {
    let mut patterns: Vec<String> = ignores::BUILT_IN.iter().map(|s| s.to_string()).collect();
    patterns.extend(config.ignore_patterns.iter().cloned());
    patterns
}

/// What each changed file is judged against, built once per change set.
pub(super) struct FileChecks<'a> {
    /// Basenames of the files tools name in `config_files`.
    config_names: HashSet<&'a str>,
    /// Recipe files of the programs tool commands run, such as `Makefile`.
    recipe_names: HashSet<&'static str>,
    /// Repo files a tool command runs directly, such as `./scripts/lint.sh`.
    scripts: HashSet<String>,
    /// The current snapshot's excludes.
    excludes: Excludes,
    policy: suppressions::Policy<'a>,
}

impl<'a> FileChecks<'a> {
    /// Checks for going from `parent` to `current` on the change's date `today`.
    pub(super) fn new(
        repo: &Repo,
        parent: &Snapshot,
        current: &Snapshot,
        configs: &'a Configs,
        patterns: &'a [String],
        today: Date,
    ) -> Result<Self> {
        let tools: Vec<&Tool> = configs
            .old
            .iter()
            .flat_map(|c| &c.tools)
            .chain(&configs.new.tools)
            .collect();
        Ok(FileChecks {
            config_names: tools
                .iter()
                .flat_map(|t| &t.config_files)
                .map(|p| basename(p))
                .collect(),
            recipe_names: RECIPE_FILES
                .iter()
                .filter(|(prog, _)| tools.iter().any(|t| first_word(&t.command) == *prog))
                .flat_map(|(_, names)| names.iter().copied())
                .collect(),
            scripts: tools
                .iter()
                .flat_map(|t| t.command.split_whitespace())
                .map(|w| w.trim_start_matches("./").to_string())
                .filter(|w| parent.get(w).is_some() || current.get(w).is_some())
                .collect(),
            excludes: Excludes::new(repo, current, &configs.new)?,
            policy: suppressions::Policy {
                patterns,
                always_block: &configs.always_block,
                // The parent's horizon, so raising it does not help the change that raises it.
                max_days: configs
                    .old
                    .as_ref()
                    .unwrap_or(&configs.new)
                    .suppressions
                    .max_days,
                today,
            },
        })
    }

    /// Flags what one change loosens in the files it touches.
    pub(super) fn check(
        &self,
        c: &Change,
        blobs: &mut BlobReader,
        parent: &Snapshot,
        current: &Snapshot,
        found: &mut Found,
    ) -> Result<()> {
        let paths: Vec<&str> = match &c.kind {
            ChangeKind::Renamed { from } => vec![from, &c.path],
            _ => vec![&c.path],
        };
        for p in paths.into_iter().filter(|p| self.is_tool_file(p)) {
            let line = format!("tool config edited: {p} ({}, {})", state(c, p), blob_ids(c));
            found.add(Some(p), line);
        }
        let old_text = read_text(blobs, parent, c.old_path())?;
        let new_text = read_text(blobs, current, &c.path)?;
        let (old_s, new_s) = (
            old_text.as_deref().unwrap_or(""),
            new_text.as_deref().unwrap_or(""),
        );
        let lines = manifest_lines(c, old_s, new_s)
            .into_iter()
            .chain(self.exclusion_lines(c, old_text.as_deref(), new_text.as_deref()));
        for line in lines {
            found.add(Some(&c.path), line);
        }
        // qgate.toml names markers in `always_block`; its own diff judges it.
        if new_text.is_some() && c.path != CONFIG_FILE {
            self.check_code(c, old_s, new_s, found);
        }
        Ok(())
    }

    /// Whether `p` configures or defines what a tool does.
    fn is_tool_file(&self, p: &str) -> bool {
        let name = basename(p);
        self.config_names.contains(name)
            || self.recipe_names.contains(name)
            || ELIXIR_TOOL_FILES.contains(&name)
            || self.scripts.contains(p)
    }

    /// New generated files, new generated-code headers and new files in an
    /// excluded path, all of which hide code from the checks.
    fn exclusion_lines(&self, c: &Change, old: Option<&str>, new: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        // A generated file is skipped wherever it lives, so a new one, or
        // existing code marked generated, hides code.
        let generated = new.is_some_and(|t| exclude::is_generated(&c.path, t));
        let was_generated = old.is_some_and(|t| exclude::is_generated(c.old_path(), t));
        if generated && !was_generated {
            out.push(match old {
                None => format!("exclusion: new generated file {}", c.path),
                Some(_) => format!(
                    "exclusion: {} now has a generated-code header ({})",
                    c.path,
                    blob_ids(c)
                ),
            });
        }
        let is_new = matches!(c.kind, ChangeKind::Added | ChangeKind::Renamed { .. });
        if is_new
            && !generated
            && !exclude::is_lockfile(&c.path)
            && self.excludes.is_guarded(&c.path)
            && c.path != baseline::BASELINE_FILE
        {
            out.push(format!("exclusion: new file in excluded path {}", c.path));
        }
        out
    }

    /// New inline suppressions, judged by R6 outside excluded paths, and new
    /// inline ignores anywhere.
    fn check_code(&self, c: &Change, old: &str, new: &str, found: &mut Found) {
        if !self.excludes.is_excluded(&c.path) {
            let judged = suppressions::judge(&c.path, old, new, &self.policy);
            found.findings.extend(judged.findings);
            for l in judged.lines {
                found.add(Some(&c.path), l);
            }
        }
        for (pattern, lines) in ignores::added(old, new, self.policy.patterns) {
            let line = format!(
                "inline ignore: {} adds {}x `{pattern}` ({})",
                c.path,
                lines.len(),
                content_digest(&lines.join("\n"))
            );
            found.add(Some(&c.path), line);
        }
    }
}

/// Changed sections of manifests that define tool commands or settings, and
/// new exclusions in `.gitattributes`.
fn manifest_lines(c: &Change, old: &str, new: &str) -> Vec<String> {
    match basename(&c.path) {
        "package.json" if json_section(old, "scripts") != json_section(new, "scripts") => {
            let digest = content_digest(json_section(new, "scripts").as_deref().unwrap_or(""));
            vec![format!(
                "package scripts changed: {} (now {digest})",
                c.path
            )]
        }
        "pyproject.toml" if toml_section(old, "tool") != toml_section(new, "tool") => {
            let digest = content_digest(toml_section(new, "tool").as_deref().unwrap_or(""));
            vec![format!(
                "pyproject [tool] changed: {} (now {digest})",
                c.path
            )]
        }
        "mix.exs" if mix_exs::guarded_sections(old) != mix_exs::guarded_sections(new) => {
            let digest = content_digest(&mix_exs::guarded_sections(new));
            vec![format!(
                "mix.exs aliases or project settings changed: {} (now {digest})",
                c.path
            )]
        }
        ".gitattributes" => attribute_loosenings(old, new)
            .into_iter()
            .map(|l| format!("exclusion: {} adds `{l}`", c.path))
            .collect(),
        _ => Vec::new(),
    }
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
