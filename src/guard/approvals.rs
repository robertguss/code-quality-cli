//! Approval records: `.git/qgate/approvals/<sha256 of the loosening diff>`,
//! local and never committed. Any change to the loosening voids the approval.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use super::{Loosening, loosening};
use crate::cli::{Finding, Report, UsageError};
use crate::git::{Repo, Snapshot, changes};

pub fn dir(repo: &Repo) -> PathBuf {
    repo.state_dir().join("approvals")
}

pub fn is_approved(repo: &Repo, hash: &str) -> bool {
    dir(repo).join(hash).is_file()
}

/// The loosening of the staged change, or of one commit against its first parent.
pub fn pending(repo: &Repo, commit: Option<&str>) -> Result<Loosening> {
    match commit {
        None => {
            let parent = Snapshot::at(repo, repo.head().as_deref())?;
            let current = Snapshot::index(repo)?;
            loosening(repo, &parent, &current, &changes::staged(repo)?)
        }
        Some(rev) => {
            let Some(sha) = repo.rev_parse(&format!("{rev}^{{commit}}")) else {
                return Err(UsageError::new(format!("no commit `{rev}`"))
                    .with_hint("qgate approve --commit <sha>   (see git log --oneline)")
                    .into());
            };
            let parent_rev = repo.rev_parse(&format!("{sha}^1"));
            let parent = Snapshot::at(repo, parent_rev.as_deref())?;
            let current = Snapshot::tree(repo, &sha)?;
            let old = match &parent_rev {
                Some(p) => p.clone(),
                None => repo.empty_tree()?,
            };
            loosening(
                repo,
                &parent,
                &current,
                &changes::between(repo, &old, &sha)?,
            )
        }
    }
}

pub fn approve(repo: &Repo, commit: Option<&str>, dry_run: bool) -> Result<Report> {
    let start = Instant::now();
    let l = pending(repo, commit)?;
    let mut report = Report::new("approve");
    let hash = l.hash();
    let state = if l.is_empty() {
        "nothing to approve"
    } else if is_approved(repo, &hash) {
        "already approved"
    } else if dry_run {
        "would approve"
    } else {
        std::fs::create_dir_all(dir(repo))?;
        std::fs::write(dir(repo).join(&hash), l.text())?;
        "approved"
    };
    report.set("status", state);
    if !l.is_empty() {
        report.set("approval", hash);
    }
    for (path, line) in &l.lines {
        let f = Finding::info("loosening", line.clone());
        report.extend([match path {
            Some(p) => f.at(p, None),
            None => f,
        }]);
    }
    report.timing("total", start.elapsed());
    Ok(report.finish())
}
