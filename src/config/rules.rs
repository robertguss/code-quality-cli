//! The rule-related parts of `qgate.toml`: `[suppressions]`, `[rules]`
//! validation and the version comparison that decides which rules are new.

use serde::{Deserialize, Serialize};

use super::Config;
use crate::cli::UsageError;
use crate::rules::{self, Setting};

/// Compare two `x.y.z` versions numerically; a missing or non-numeric part
/// counts as 0, and anything after `-` or `+` is ignored.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |v: &str| -> Vec<u64> {
        let core = v
            .trim_start_matches('v')
            .split(['-', '+'])
            .next()
            .unwrap_or("");
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let (a, b) = (parts(a), parts(b));
    (0..a.len().max(b.len()))
        .map(|i| {
            let at = |v: &[u64]| v.get(i).copied().unwrap_or(0);
            at(&a).cmp(&at(&b))
        })
        .find(|o| o.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// What a new inline suppression must carry (R6).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suppressions {
    /// Furthest an `until:` date may be after the change that adds it.
    #[serde(default = "default_max_days")]
    pub max_days: u32,
    /// Rule ids, families and marker strings whose suppression always blocks.
    #[serde(default = "default_always_block")]
    pub always_block: Vec<String>,
}

fn default_max_days() -> u32 {
    90
}

/// Swallowed errors hide bugs, and blanket or file-wide type suppressions hide
/// every error at once (KTD12). `eslint-disable` here means one with no rule names.
pub fn default_always_block() -> Vec<String> {
    [
        "swallowed-error",
        "@ts-nocheck",
        "@ts-ignore",
        "eslint-disable",
    ]
    .map(String::from)
    .to_vec()
}

impl Default for Suppressions {
    fn default() -> Self {
        Suppressions {
            max_days: default_max_days(),
            always_block: default_always_block(),
        }
    }
}

impl Suppressions {
    pub(super) fn is_default(&self) -> bool {
        *self == Suppressions::default()
    }
}

impl Config {
    /// Checks `[rules]`: every name is a rule or family, and `allow` globs
    /// are valid and set only where they apply.
    pub(super) fn validate_rules(&self, source: &str) -> Result<(), UsageError> {
        for (name, setting) in &self.rules {
            if rules::find(name).is_none() && !rules::is_family(name) {
                return Err(UsageError::new(format!(
                    "{source}: [rules] has unknown rule `{name}`"
                ))
                .with_hint("qgate rules   (lists every rule id and family)"));
            }
            if !setting.allow().is_empty() && name != rules::lang::DEBUG_OUTPUT {
                return Err(UsageError::new(format!(
                    "{source}: `allow` only applies to [rules.{}], not `{name}`",
                    rules::lang::DEBUG_OUTPUT
                )));
            }
            for pattern in setting.allow() {
                globset::Glob::new(pattern).map_err(|e| {
                    UsageError::new(format!("{source}: invalid glob `{pattern}`: {e}"))
                })?;
            }
        }
        Ok(())
    }

    /// Globs where debug output is the file's job (KTD9).
    pub fn output_globs(&self) -> &[String] {
        self.rules
            .get(rules::lang::DEBUG_OUTPUT)
            .map_or(&[], Setting::allow)
    }
}
