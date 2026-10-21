//! `qgate.toml`: schema, validation and the version pin.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cli::UsageError;
use crate::git::{BlobReader, Repo, Snapshot};

pub const CONFIG_FILE: &str = "qgate.toml";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/robertguss/code-quality-cli";

pub fn install_command(version: &str) -> String {
    format!("cargo install --locked --git {REPO_URL} --tag v{version} qgate")
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The qgate version this repo's baseline was measured with.
    pub qgate_version: String,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub duplication: Duplication,
    /// Extra glob patterns excluded from the built-in checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Test data globs, such as goldens. Files there skip the built-in checks
    /// and may be added freely, except source files qgate parses for
    /// functions, which are checked like any other code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixtures: Vec<String>,
    /// Extra inline-ignore markers, on top of the built-in table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore_patterns: Vec<String>,
    /// C/C++ macros, such as `MUST_USE`, blanked before parsing so they do not
    /// cause syntax errors. Each use and its `(...)` arguments become spaces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribute_macros: Vec<String>,
    #[serde(default, rename = "tool", skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    /// Per-directory limit overrides for monorepos.
    #[serde(default, rename = "package", skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<Package>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_function_lines")]
    pub function_lines: u32,
    #[serde(default = "default_file_lines")]
    pub file_lines: u32,
}

fn default_function_lines() -> u32 {
    50
}

fn default_file_lines() -> u32 {
    500
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            function_lines: default_function_lines(),
            file_lines: default_file_lines(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Duplication {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_min_tokens")]
    pub min_tokens: u32,
}

fn default_true() -> bool {
    true
}

fn default_min_tokens() -> u32 {
    50
}

impl Default for Duplication {
    fn default() -> Self {
        Duplication {
            enabled: true,
            min_tokens: default_min_tokens(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// Runs on every commit, scoped to the change.
    #[default]
    Commit,
    /// Runs only on push, against the whole pushed tree.
    Push,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Commit => "commit",
            Stage::Push => "push",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolFormat {
    /// Detect SARIF, ESLint JSON or gcc-style lines; otherwise pass through.
    #[default]
    Auto,
    Sarif,
    EslintJson,
    Gcc,
    /// Never parse; a failing exit is one finding with the output.
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    /// Shell command. `{files}` is replaced with the matching changed files.
    pub command: String,
    /// Which files the tool cares about. Empty means every file.
    #[serde(default)]
    pub globs: Vec<String>,
    #[serde(default)]
    pub stage: Stage,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// The tool's own config files, such as `.eslintrc.json`. Edits to these,
    /// or to any file of the same name elsewhere, need approval.
    #[serde(default)]
    pub config_files: Vec<String>,
    #[serde(default)]
    pub format: ToolFormat,
}

impl Tool {
    pub fn timeout_for(&self, stage: Stage) -> u64 {
        self.timeout_secs.unwrap_or(match stage {
            Stage::Commit => 60,
            Stage::Push => 15 * 60,
        })
    }

    pub fn takes_files(&self) -> bool {
        self.command.contains("{files}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Directory relative to the repo root, such as `services/api`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Or a glob, such as `tests/**`. A matching glob wins over any directory;
    /// among globs, the last match wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    #[serde(default)]
    pub function_lines: Option<u32>,
    #[serde(default)]
    pub file_lines: Option<u32>,
}

impl Config {
    /// A config for a repo that has none yet, pinned to this binary.
    pub fn starter() -> Self {
        Config {
            qgate_version: VERSION.to_string(),
            limits: Limits::default(),
            duplication: Duplication::default(),
            exclude: Vec::new(),
            fixtures: Vec::new(),
            ignore_patterns: Vec::new(),
            attribute_macros: Vec::new(),
            tools: Vec::new(),
            packages: Vec::new(),
        }
    }

    /// Parse and validate config text. `source` names the file in errors.
    pub fn parse(text: &str, source: &str) -> Result<Config, UsageError> {
        let config = Config::parse_unchecked(text, source)?;
        config.validate(source)?;
        Ok(config)
    }

    /// Parse without validation, for comparing against an older config that
    /// may pin another version.
    pub fn parse_unchecked(text: &str, source: &str) -> Result<Config, UsageError> {
        toml::from_str(text).map_err(|e| {
            UsageError::new(format!("{source}: {}", e.message().trim())).with_hint(format!(
                "see {REPO_URL}/blob/main/qgate.toml.example for every key"
            ))
        })
    }

    fn validate(&self, source: &str) -> Result<(), UsageError> {
        if self.qgate_version != VERSION {
            return Err(UsageError::new(format!(
                "{source} pins qgate {} but this binary is {VERSION}",
                self.qgate_version
            ))
            .with_hint(install_command(&self.qgate_version)));
        }
        let mut names = BTreeMap::new();
        for tool in &self.tools {
            if names.insert(tool.name.as_str(), ()).is_some() {
                return Err(UsageError::new(format!(
                    "{source}: tool `{}` is declared twice",
                    tool.name
                )));
            }
            if tool.command.trim().is_empty() {
                return Err(UsageError::new(format!(
                    "{source}: tool `{}` has an empty command",
                    tool.name
                )));
            }
        }
        for pkg in &self.packages {
            if pkg.path.is_some() == pkg.glob.is_some() {
                return Err(UsageError::new(format!(
                    "{source}: each [[package]] needs exactly one of `path` or `glob`"
                )));
            }
        }
        for pattern in self
            .exclude
            .iter()
            .chain(&self.fixtures)
            .chain(self.packages.iter().filter_map(|p| p.glob.as_ref()))
            .chain(self.tools.iter().flat_map(|t| &t.globs))
        {
            globset::Glob::new(pattern)
                .map_err(|e| UsageError::new(format!("{source}: invalid glob `{pattern}`: {e}")))?;
        }
        for m in &self.attribute_macros {
            let ok = m.starts_with(|c: char| c == '_' || c.is_ascii_alphabetic())
                && m.chars().all(|c| c == '_' || c.is_ascii_alphanumeric());
            if !ok {
                return Err(UsageError::new(format!(
                    "{source}: attribute_macros entry `{m}` is not an identifier"
                )));
            }
        }
        if self.limits.function_lines == 0 || self.limits.file_lines == 0 {
            return Err(UsageError::new(format!(
                "{source}: limits must be at least 1"
            )));
        }
        Ok(())
    }

    /// Limits that apply to `path`, after the most specific directory
    /// override and then any matching glob override.
    pub fn limits_for(&self, path: &str) -> Limits {
        let mut limits = self.dir_limits_for(path);
        for pkg in &self.packages {
            let Some(glob) = &pkg.glob else { continue };
            let matches = globset::Glob::new(glob)
                .map(|g| g.compile_matcher().is_match(path))
                .unwrap_or(false);
            if matches {
                limits = Limits {
                    function_lines: pkg.function_lines.unwrap_or(limits.function_lines),
                    file_lines: pkg.file_lines.unwrap_or(limits.file_lines),
                };
            }
        }
        limits
    }

    /// Limits for `path` from the top level and directory overrides only.
    pub fn dir_limits_for(&self, path: &str) -> Limits {
        let mut limits = self.limits;
        let mut best = 0;
        for pkg in &self.packages {
            let Some(dir) = &pkg.path else { continue };
            let dir = dir.trim_end_matches('/');
            let inside = path == dir || path.starts_with(&format!("{dir}/"));
            if inside && dir.len() >= best {
                best = dir.len();
                limits = Limits {
                    function_lines: pkg.function_lines.unwrap_or(self.limits.function_lines),
                    file_lines: pkg.file_lines.unwrap_or(self.limits.file_lines),
                };
            }
        }
        limits
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serializes")
    }
}

/// Load `qgate.toml` from a snapshot (the staged index at commit, the pushed tree at push).
pub fn load(repo: &Repo, snapshot: &Snapshot) -> anyhow::Result<Config> {
    let Some(entry) = snapshot.get(CONFIG_FILE) else {
        let hint = if repo.root.join(CONFIG_FILE).exists() {
            format!("git add {CONFIG_FILE}")
        } else {
            "qgate install".to_string()
        };
        return Err(
            UsageError::new(format!("no {CONFIG_FILE} in the checked tree"))
                .with_hint(hint)
                .into(),
        );
    };
    let bytes = BlobReader::new(repo)?.read(&entry.oid)?;
    Ok(Config::parse(
        &String::from_utf8_lossy(&bytes),
        CONFIG_FILE,
    )?)
}

/// Like [`load`], but reads the working-tree file when it is not staged yet or
/// differs from the staged one, for commands the developer runs by hand
/// (`baseline create`, `prune`). Also says which file was used.
pub fn load_or_worktree(
    repo: &Repo,
    snapshot: &Snapshot,
) -> anyhow::Result<(Config, &'static str)> {
    let Ok(text) = std::fs::read_to_string(repo.root.join(CONFIG_FILE)) else {
        return Ok((load(repo, snapshot)?, "staged"));
    };
    let origin = match snapshot.get(CONFIG_FILE) {
        None => "working tree (not staged)",
        Some(entry) if BlobReader::new(repo)?.read(&entry.oid)? != text.as_bytes() => {
            "working tree (unstaged changes)"
        }
        Some(_) => "staged",
    };
    Ok((Config::parse(&text, CONFIG_FILE)?, origin))
}
