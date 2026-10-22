//! `qgate features log`: scope changes from history, one entry per commit
//! that changed the map (KTD2).

use anyhow::Result;
use serde::Serialize;

use super::MAP_FILE;
use super::map::FeatureMap;
use super::scope;
use crate::cli::{Format, UsageError};
use crate::git::Repo;

#[derive(Debug, Serialize)]
pub struct Entry {
    pub commit: String,
    pub date: String,
    pub subject: String,
    pub changes: Vec<String>,
}

/// Commits since `since` (all history when `None`) whose map differs from
/// their first parent's, oldest first. Commits that only reformat the map are left out.
pub fn log(repo: &Repo, since: Option<&str>) -> Result<Vec<Entry>> {
    if let Some(rev) = since
        && repo.rev_parse(&format!("{rev}^{{commit}}")).is_none()
    {
        return Err(UsageError::new(format!("no commit `{rev}` for --since"))
            .with_hint("qgate features log --since origin/main")
            .into());
    }
    let range = match since {
        Some(rev) => format!("{rev}..HEAD"),
        None => "HEAD".to_string(),
    };
    if repo.head().is_none() {
        return Ok(Vec::new());
    }
    let out = repo.output(&[
        "log",
        "--reverse",
        "--format=%H%x1f%P%x1f%cs%x1f%s",
        &range,
        "--",
        MAP_FILE,
    ])?;
    let mut entries = Vec::new();
    for line in out.lines() {
        let mut fields = line.split('\x1f');
        let (Some(commit), Some(parents), Some(date), Some(subject)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let parent = parents.split(' ').next().filter(|p| !p.is_empty());
        let Some(current) = map_at(repo, commit) else {
            continue;
        };
        let before = parent.and_then(|p| map_at(repo, p));
        let changes: Vec<String> = scope::diff(before.as_ref(), &current)
            .iter()
            .map(scope::ScopeChange::describe)
            .collect();
        if changes.is_empty() {
            continue;
        }
        entries.push(Entry {
            commit: commit.to_string(),
            date: date.to_string(),
            subject: subject.to_string(),
            changes,
        });
    }
    Ok(entries)
}

/// The map at `rev`, if it has one that parses.
fn map_at(repo: &Repo, rev: &str) -> Option<FeatureMap> {
    let text = repo.try_output(&["show", &format!("{rev}:{MAP_FILE}")])?;
    FeatureMap::parse(&text).ok()
}

pub fn render(entries: &[Entry], format: Format) -> String {
    if format == Format::Json {
        let body = serde_json::json!({ "command": "features log", "entries": entries });
        return format!("{body}\n");
    }
    if entries.is_empty() {
        return format!("no scope changes: no commit changed {MAP_FILE}\n");
    }
    let mut out = String::new();
    for e in entries {
        out.push_str(&format!(
            "{} {} {}\n",
            &e.commit[..e.commit.len().min(10)],
            e.date,
            e.subject
        ));
        for c in &e.changes {
            out.push_str(&format!("  {c}\n"));
        }
    }
    out
}
