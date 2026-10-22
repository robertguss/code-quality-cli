//! The rule registry (KTD1): every built-in check's id, kind, default level,
//! pstack source, rationale and fix, plus the levels a repo sets for them.

pub mod allow;
pub mod check;
pub mod engine;
pub mod entry;
pub mod lang;
pub mod list;

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::checks::{DUPLICATION, FILE_LENGTH, FUNCTION_LENGTH};
use crate::cli::{Finding, Severity};
use crate::config::Config;

/// How strictly a repo applies a rule. Ordered from loosest to strictest, so
/// a lower level is a loosening.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// Exact rules are settled by syntax and block; heuristic ones may misfire and warn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Exact,
    Heuristic,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Exact => "exact",
            Kind::Heuristic => "heuristic",
        }
    }
}

/// Where an occurrence rule counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every file the built-in checks read.
    Everywhere,
    /// Production files only: not test or dev files, and not Rust test code.
    OutsideTests,
    /// Debug output: also skips entry points, scripts and the repo's `allow` globs (KTD9).
    Output,
}

#[derive(Debug)]
pub struct Rule {
    /// `<family>/<name>`, or a bare name for the length and duplication rules.
    pub id: &'static str,
    pub kind: Kind,
    /// Language the rule reads, or `any`.
    pub language: &'static str,
    /// The pstack skill the rule comes from, or `qgate`.
    pub source: &'static str,
    pub rationale: &'static str,
    /// What to do instead, shown in every finding.
    pub fix: &'static str,
    /// The qgate version the rule first shipped in.
    pub since: &'static str,
    /// Occurrence rules are counted per file and baselined as a count (R4).
    pub counted: bool,
    pub scope: Scope,
}

impl Rule {
    pub fn family(&self) -> &'static str {
        self.id.split_once('/').map_or(self.id, |(f, _)| f)
    }

    pub fn default_level(&self) -> Level {
        match self.kind {
            Kind::Exact => Level::Error,
            Kind::Heuristic => Level::Warn,
        }
    }
}

const BUILT_IN: &[Rule] = &[
    Rule {
        id: FUNCTION_LENGTH,
        kind: Kind::Exact,
        language: "any",
        source: "qgate",
        rationale: "long functions hide more than one job",
        fix: "split the function",
        since: "0.1.0",
        counted: false,
        scope: Scope::Everywhere,
    },
    Rule {
        id: FILE_LENGTH,
        kind: Kind::Exact,
        language: "any",
        source: "qgate",
        rationale: "long files mix concerns and are hard to navigate",
        fix: "split the file",
        since: "0.1.0",
        counted: false,
        scope: Scope::Everywhere,
    },
    Rule {
        id: DUPLICATION,
        kind: Kind::Exact,
        language: "any",
        source: "qgate",
        rationale: "copied code drifts apart when one copy is fixed",
        fix: "extract the shared code",
        since: "0.1.0",
        counted: false,
        scope: Scope::Everywhere,
    },
];

/// Every rule, built-in first, then each language's in registry order.
pub fn all() -> &'static [&'static Rule] {
    static ALL: OnceLock<Vec<&'static Rule>> = OnceLock::new();
    ALL.get_or_init(|| BUILT_IN.iter().chain(lang::rules()).collect())
}

pub fn find(id: &str) -> Option<&'static Rule> {
    all().iter().copied().find(|r| r.id == id)
}

/// True when `name` is a family with at least one namespaced rule.
pub fn is_family(name: &str) -> bool {
    all()
        .iter()
        .any(|r| r.id.contains('/') && r.family() == name)
}

pub fn is_counted(id: &str) -> bool {
    find(id).is_some_and(|r| r.counted)
}

/// Whether `name` (a rule id or family) covers `rule`.
pub fn covers(name: &str, rule: &Rule) -> bool {
    rule.id == name || (rule.id.contains('/') && rule.family() == name)
}

/// What qgate.toml sets for one rule id or family: a level, or a table with
/// an optional level and the rule's options (KTD3).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Setting {
    Level(Level),
    Table(Options),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    /// `debug-output` only: globs where printing is the file's job, such as `src/cli/**`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
}

impl Setting {
    pub fn level(&self) -> Option<Level> {
        match self {
            Setting::Level(l) => Some(*l),
            Setting::Table(o) => o.level,
        }
    }

    pub fn allow(&self) -> &[String] {
        match self {
            Setting::Level(_) => &[],
            Setting::Table(o) => &o.allow,
        }
    }
}

/// The level `config` gives `rule`: its own key, then its family's, then the default.
pub fn level_for(config: &Config, rule: &Rule) -> Level {
    let own = config.rules.get(rule.id).and_then(Setting::level);
    let family = || {
        rule.id
            .contains('/')
            .then(|| config.rules.get(rule.family()).and_then(Setting::level))
            .flatten()
    };
    own.or_else(family).unwrap_or(rule.default_level())
}

/// Apply each rule's level to built-in findings: drop `off` rules and turn
/// errors of `warn` rules into warnings. Findings of unregistered rules
/// (parse errors, loosenings, prunable entries) pass through.
pub fn apply_levels(config: &Config, findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter_map(|mut f| {
            let Some(rule) = find(&f.rule) else {
                return Some(f);
            };
            match level_for(config, rule) {
                Level::Off => None,
                Level::Warn => {
                    if f.severity == Severity::Error {
                        f.severity = Severity::Warning;
                    }
                    Some(f)
                }
                Level::Error => Some(f),
            }
        })
        .collect()
}

/// Every registered rule's level under `config`, for comparing two configs.
pub fn levels(config: &Config) -> Vec<(&'static Rule, Level)> {
    all().iter().map(|r| (*r, level_for(config, r))).collect()
}
