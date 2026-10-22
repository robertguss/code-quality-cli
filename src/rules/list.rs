//! `qgate rules`: every rule with its id, kind, level, source and rationale (R5).

use std::path::Path;

use serde_json::{Value, json};

use super::{Level, Rule, all, find, is_family, level_for};
use crate::cli::{Finding, Format, Report, UsageError};
use crate::config::{self, CONFIG_FILE, Config, VERSION};
use crate::git::Repo;

const HINT: &str = "qgate rules   (lists every rule id and family)";

/// List the rules `id` or `family` select (every rule when both are `None`),
/// at the levels the repo around `cwd` sets.
pub fn list(
    cwd: &Path,
    id: Option<&str>,
    family: Option<&str>,
    format: Format,
) -> anyhow::Result<Report> {
    let rules = select(id, family)?;
    let mut report = Report::new("rules");
    let config = repo_config(cwd, &mut report)?;
    let rows: Vec<(&Rule, Level)> = rules.iter().map(|r| (*r, level_for(&config, r))).collect();
    report.extend(rows.iter().map(|(rule, level)| finding(rule, *level)));
    if format == Format::Json {
        report.set(
            "rules",
            rows.iter().map(|(r, l)| row(r, *l)).collect::<Vec<_>>(),
        );
    }
    Ok(report.finish())
}

fn select(id: Option<&str>, family: Option<&str>) -> Result<Vec<&'static Rule>, UsageError> {
    if let Some(id) = id {
        let rule = find(id).ok_or_else(|| {
            if is_family(id) {
                UsageError::new(format!("`{id}` is a family, not a rule id"))
                    .with_hint(format!("qgate rules --family {id}"))
            } else {
                unknown("rule", id)
            }
        })?;
        return Ok(vec![rule]);
    }
    let Some(family) = family else {
        return Ok(all().to_vec());
    };
    let rules: Vec<_> = all()
        .iter()
        .copied()
        .filter(|r| r.family() == family)
        .collect();
    if rules.is_empty() {
        return Err(unknown("family", family));
    }
    Ok(rules)
}

fn unknown(what: &str, name: &str) -> UsageError {
    UsageError::new(format!("unknown {what} `{name}`")).with_hint(HINT)
}

/// The repo's config: the working-tree `qgate.toml`, else the committed one,
/// else the starter defaults (also outside a git repo). A version-pin
/// mismatch only adds a warning, since levels are still worth showing.
fn repo_config(cwd: &Path, report: &mut Report) -> anyhow::Result<Config> {
    let Ok(repo) = Repo::discover(cwd) else {
        return Ok(Config::starter());
    };
    let text = std::fs::read_to_string(repo.root.join(CONFIG_FILE))
        .ok()
        .or_else(|| repo.try_output(&["show", &format!("HEAD:{CONFIG_FILE}")]));
    let Some(text) = text else {
        return Ok(Config::starter());
    };
    let config = Config::parse_unchecked(&text, CONFIG_FILE)?;
    if config.qgate_version != VERSION {
        report.extend([Finding::warning(
            "config",
            format!(
                "{CONFIG_FILE} pins qgate {} but this binary is {VERSION}; levels are read from it anyway",
                config.qgate_version
            ),
        )
        .with_fix(config::install_command(&config.qgate_version))]);
    }
    Ok(config)
}

fn finding(rule: &Rule, level: Level) -> Finding {
    Finding::info(
        rule.id,
        format!(
            "{}, {}, {}, from {}: {}. Fix: {}.",
            level.as_str(),
            rule.kind.as_str(),
            rule.language,
            rule.source,
            rule.rationale,
            rule.fix
        ),
    )
}

fn row(rule: &Rule, level: Level) -> Value {
    json!({
        "id": rule.id,
        "family": rule.family(),
        "kind": rule.kind.as_str(),
        "level": level.as_str(),
        "language": rule.language,
        "source": rule.source,
        "rationale": rule.rationale,
        "fix": rule.fix,
        "since": rule.since,
    })
}
