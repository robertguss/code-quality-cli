//! Approval records: `.git/qgate/approvals/<sha256 of the loosening diff>`,
//! local and never committed. Any change to the loosening voids the approval.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use super::{Loosening, loosening, signed};
use crate::cli::{Finding, Report, UsageError};
use crate::git::{Repo, Snapshot, changes};

pub fn dir(repo: &Repo) -> PathBuf {
    repo.state_dir().join("approvals")
}

pub fn is_approved(repo: &Repo, hash: &str) -> bool {
    dir(repo).join(hash).is_file()
}

/// What to approve: the staged change, one commit, or HEAD since its merge
/// base with a ref.
#[derive(Clone, Copy, Debug, Default)]
pub enum Target<'a> {
    #[default]
    Staged,
    Commit(&'a str),
    Since(&'a str),
}

/// How to record an approval.
#[derive(Clone, Debug, Default)]
pub struct Options<'a> {
    pub target: Target<'a>,
    pub dry_run: bool,
    /// Write a signed record to `.qgate/approvals/` instead of `.git/qgate/`.
    pub sign: bool,
    /// The SSH key to sign with; defaults to git's `user.signingkey`.
    pub key: Option<&'a str>,
}

/// The tree a target's approval is trusted from, and the loosening itself.
fn target_loosening(repo: &Repo, target: Target) -> Result<(Snapshot, Loosening)> {
    match target {
        Target::Staged => {
            let parent = Snapshot::at(repo, repo.head().as_deref())?;
            let current = Snapshot::index(repo)?;
            let l = loosening(repo, &parent, &current, &changes::staged(repo)?)?;
            Ok((parent, l))
        }
        Target::Commit(rev) => {
            let sha = commit_sha(repo, rev)?;
            let parent_rev = repo.rev_parse(&format!("{sha}^1"));
            let parent = Snapshot::at(repo, parent_rev.as_deref())?;
            let current = Snapshot::tree(repo, &sha)?;
            let old = match &parent_rev {
                Some(p) => p.clone(),
                None => repo.empty_tree()?,
            };
            let l = loosening(
                repo,
                &parent,
                &current,
                &changes::between(repo, &old, &sha)?,
            )?;
            Ok((parent, l))
        }
        Target::Since(rev) => {
            let target = commit_sha(repo, rev)?;
            let Some(head) = repo.head() else {
                return Err(UsageError::new("no commits yet").into());
            };
            let Some(base) = repo
                .try_output(&["merge-base", &head, &target])
                .filter(|b| !b.is_empty())
            else {
                return Err(
                    UsageError::new(format!("HEAD has no merge base with `{rev}`"))
                        .with_hint("git fetch --unshallow")
                        .into(),
                );
            };
            let parent = Snapshot::at(repo, Some(&base))?;
            let current = Snapshot::tree(repo, &head)?;
            let l = loosening(
                repo,
                &parent,
                &current,
                &changes::between(repo, &base, &head)?,
            )?;
            Ok((parent, l))
        }
    }
}

fn commit_sha(repo: &Repo, rev: &str) -> Result<String> {
    repo.rev_parse(&format!("{rev}^{{commit}}")).ok_or_else(|| {
        UsageError::new(format!("no commit `{rev}`"))
            .with_hint("qgate approve --commit <sha>   (see git log --oneline)")
            .into()
    })
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

pub fn approve(repo: &Repo, opts: &Options) -> Result<Report> {
    let start = Instant::now();
    let (trusted, l) = target_loosening(repo, opts.target)?;
    let mut report = Report::new("approve");
    let hash = l.hash();
    let state = if l.is_empty() {
        "nothing to approve"
    } else if opts.sign {
        sign_record(repo, opts, &trusted, &l, &mut report)?
    } else if is_approved(repo, &hash) {
        "already approved"
    } else if opts.dry_run {
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
    for (path, line) in super::grouped(&l) {
        let f = Finding::info("loosening", line);
        report.extend([match path {
            Some(p) => f.at(p, None),
            None => f,
        }]);
    }
    report.timing("total", start.elapsed());
    Ok(report.finish())
}

/// Sign the loosening and write `.qgate/approvals/<hash>.sig`, to be committed.
fn sign_record(
    repo: &Repo,
    opts: &Options,
    trusted: &Snapshot,
    l: &Loosening,
    report: &mut Report,
) -> Result<&'static str> {
    let rel = signed::record_path(&l.hash());
    report.set("record", rel.clone());
    if opts.dry_run {
        return Ok("would sign");
    }
    let key = match opts.key {
        Some(k) => k.to_string(),
        None => repo
            .try_output(&["config", "user.signingkey"])
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                UsageError::new("no SSH key to sign with")
                    .with_hint("qgate approve --sign --key ~/.ssh/id_ed25519.pub")
            })?,
    };
    let key = match key.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(&key)),
        None => PathBuf::from(&key),
    };
    let sig = signed::sign(&key, l.text().as_bytes())?;
    let path = repo.root.join(&rel);
    std::fs::create_dir_all(path.parent().expect("record has a parent"))?;
    std::fs::write(&path, &sig)?;
    let mut probe = signed::Committed::default();
    probe.insert(&l.hash(), sig);
    let trusted_signers = signed::Committed::signers_of(repo, trusted)?;
    let verified = match trusted_signers {
        Some(signers) => probe.with_signers(signers).approves(l),
        None => false,
    };
    report.set("verified", verified);
    if !verified {
        report.extend([Finding::warning(
            "approval-untrusted",
            "the signature was written, but its key is not in the allowed_signers of the tree \
             before this change, so the gate will not accept it",
        )
        .at(&rel, None)]);
    }
    report.set("next", format!("git add {rel} && git commit"));
    Ok("signed")
}
