//! Change sets: what a commit or a push changes, with renames detected.

use std::collections::BTreeSet;

use anyhow::{Result, bail};

use super::{Repo, ZERO_OID, split_nul};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    /// A whole-file rename, possibly with edits.
    Renamed {
        from: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
}

impl Change {
    /// The path this file had before the change.
    pub fn old_path(&self) -> &str {
        match &self.kind {
            ChangeKind::Renamed { from } => from,
            _ => &self.path,
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.kind == ChangeKind::Deleted
    }
}

/// What the next commit changes: the index against HEAD (or the empty tree).
pub fn staged(repo: &Repo) -> Result<Vec<Change>> {
    let base = match repo.head() {
        Some(head) => head,
        None => repo.empty_tree()?,
    };
    let out = repo.output_bytes(&["diff-index", "--cached", "-M", "-z", "--raw", &base])?;
    parse_raw(&out)
}

/// What changes from `old` to `new` (tree-ish ids).
pub fn between(repo: &Repo, old: &str, new: &str) -> Result<Vec<Change>> {
    let out = repo.output_bytes(&["diff-tree", "-r", "-M", "-z", "--raw", old, new])?;
    parse_raw(&out)
}

/// Parse `git diff --raw -z` output.
fn parse_raw(bytes: &[u8]) -> Result<Vec<Change>> {
    let fields = split_nul(bytes);
    let mut changes = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let meta = &fields[i];
        // ":<old mode> <new mode> <old oid> <new oid> <status>"
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        if parts.len() < 5 {
            bail!("bad diff record: {meta}");
        }
        let oid = |s: &str| (s != ZERO_OID && !s.chars().all(|c| c == '0')).then(|| s.to_string());
        let (old_oid, new_oid) = (oid(parts[2]), oid(parts[3]));
        let status = parts[4].chars().next().unwrap_or('M');
        let path_a = fields.get(i + 1).cloned().unwrap_or_default();
        i += 2;
        let (path, kind) = match status {
            'A' => (path_a, ChangeKind::Added),
            'D' => (path_a, ChangeKind::Deleted),
            'R' => {
                let path_b = fields.get(i).cloned().unwrap_or_default();
                i += 1;
                (path_b, ChangeKind::Renamed { from: path_a })
            }
            'C' => {
                let path_b = fields.get(i).cloned().unwrap_or_default();
                i += 1;
                (path_b, ChangeKind::Added)
            }
            _ => (path_a, ChangeKind::Modified),
        };
        let old_oid = if kind == ChangeKind::Added {
            None
        } else {
            old_oid
        };
        changes.push(Change {
            path,
            kind,
            old_oid,
            new_oid,
        });
    }
    Ok(changes)
}

/// Paths whose working-tree content differs from the index.
pub fn unstaged_paths(repo: &Repo) -> Result<BTreeSet<String>> {
    let out = repo.output_bytes(&["diff", "--name-only", "-z"])?;
    Ok(split_nul(&out).into_iter().collect())
}

/// Untracked files that are not ignored.
pub fn untracked_paths(repo: &Repo) -> Result<BTreeSet<String>> {
    let out = repo.output_bytes(&["ls-files", "--others", "--exclude-standard", "-z"])?;
    Ok(split_nul(&out).into_iter().collect())
}

/// One line of pre-push hook input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushRef {
    pub local_ref: String,
    pub local_oid: String,
    pub remote_ref: String,
    pub remote_oid: String,
}

impl PushRef {
    pub fn is_delete(&self) -> bool {
        self.local_oid.chars().all(|c| c == '0')
    }

    fn remote_known(&self) -> bool {
        !self.remote_oid.chars().all(|c| c == '0')
    }
}

/// Parse git's pre-push stdin: `<local ref> <local oid> <remote ref> <remote oid>` per line.
pub fn parse_pre_push(input: &str) -> Vec<PushRef> {
    input
        .lines()
        .filter_map(|line| {
            let mut p = line.split_whitespace();
            Some(PushRef {
                local_ref: p.next()?.to_string(),
                local_oid: p.next()?.to_string(),
                remote_ref: p.next()?.to_string(),
                remote_oid: p.next()?.to_string(),
            })
        })
        .collect()
}

/// The remote's default branch, as a local remote-tracking ref.
pub fn remote_default_branch(repo: &Repo, remote: &str) -> Option<String> {
    if let Some(r) =
        repo.try_output(&["symbolic-ref", "-q", &format!("refs/remotes/{remote}/HEAD")])
        && repo.rev_parse(&r).is_some()
    {
        return Some(r);
    }
    ["main", "master", "trunk"]
        .iter()
        .map(|b| format!("refs/remotes/{remote}/{b}"))
        .find(|r| repo.rev_parse(r).is_some())
}

/// What a pushed ref is compared against: the merge base with the remote
/// default branch, else the remote's old commit, else nothing (the empty tree).
pub fn push_base(repo: &Repo, remote: &str, push: &PushRef) -> Option<String> {
    if let Some(default) = remote_default_branch(repo, remote)
        && let Some(base) = repo.try_output(&["merge-base", &push.local_oid, &default])
        && !base.is_empty()
    {
        return Some(base);
    }
    if push.remote_known()
        && repo
            .rev_parse(&format!("{}^{{commit}}", push.remote_oid))
            .is_some()
    {
        return Some(push.remote_oid.clone());
    }
    None
}

/// Commits in `base..tip`, oldest first, each with its first parent (if any).
pub fn commits(
    repo: &Repo,
    base: Option<&str>,
    tip: &str,
) -> Result<Vec<(String, Option<String>)>> {
    let range = match base {
        Some(b) => format!("{b}..{tip}"),
        None => tip.to_string(),
    };
    let out = repo.output(&["rev-list", "--reverse", "--parents", &range])?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let mut ids = l.split(' ');
            let commit = ids.next()?.to_string();
            Some((commit, ids.next().map(str::to_string)))
        })
        .collect())
}
