//! Read exactly what is being committed or pushed, by asking git, without
//! touching the working tree or the index.

pub mod changes;
pub mod checkout;
pub mod index;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub use changes::{Change, ChangeKind, PushRef};
pub use checkout::Checkout;
pub use index::{BlobReader, Entry, Snapshot, Source};

pub const ZERO_OID: &str = "0000000000000000000000000000000000000000";

/// A git repository, addressed by its working-tree root.
#[derive(Clone, Debug)]
pub struct Repo {
    pub root: PathBuf,
    /// The common git directory (shared by all worktrees).
    pub git_dir: PathBuf,
}

impl Repo {
    pub fn discover(cwd: &Path) -> Result<Repo> {
        let out = Command::new("git")
            .args(["rev-parse", "--show-toplevel", "--git-common-dir"])
            .current_dir(cwd)
            .stderr(Stdio::null())
            .output()
            .context("running git")?;
        if !out.status.success() {
            bail!(
                crate::cli::UsageError::new(format!(
                    "{} is not inside a git repository",
                    cwd.display()
                ))
                .with_hint("git init && qgate install")
            );
        }
        let text = String::from_utf8(out.stdout)?;
        let mut lines = text.lines();
        let root = PathBuf::from(lines.next().unwrap_or_default());
        let git_dir = PathBuf::from(lines.next().unwrap_or_default());
        let git_dir = if git_dir.is_absolute() {
            git_dir
        } else {
            cwd.join(git_dir)
        };
        Ok(Repo {
            root: root.canonicalize().unwrap_or(root),
            git_dir: git_dir.canonicalize().unwrap_or(git_dir),
        })
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        // Never let a read refresh and rewrite the index.
        cmd.current_dir(&self.root).env("GIT_OPTIONAL_LOCKS", "0");
        cmd
    }

    /// Run git and return stdout as bytes, failing with git's stderr.
    pub fn output_bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        let out = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("running git {}", args.join(" ")))?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out.stdout)
    }

    pub fn output(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8(self.output_bytes(args)?)?)
    }

    /// Run git; `None` when it exits non-zero.
    pub fn try_output(&self, args: &[&str]) -> Option<String> {
        let out = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Resolve a revision to a full object id.
    pub fn rev_parse(&self, rev: &str) -> Option<String> {
        self.try_output(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{object}}"),
        ])
        .filter(|s| !s.is_empty())
    }

    pub fn head(&self) -> Option<String> {
        self.rev_parse("HEAD")
    }

    /// The id of the empty tree in this repo's hash format.
    pub fn empty_tree(&self) -> Result<String> {
        let out = self
            .command()
            .args(["hash-object", "-t", "tree", "--stdin"])
            .stdin(Stdio::null())
            .output()?;
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    /// Where qgate keeps local, uncommitted state (caches, approvals, logs).
    pub fn state_dir(&self) -> PathBuf {
        self.git_dir.join("qgate")
    }

    /// The directory git runs hooks from.
    pub fn hooks_dir(&self) -> Result<PathBuf> {
        let p = PathBuf::from(self.output(&["rev-parse", "--git-path", "hooks"])?.trim());
        Ok(if p.is_absolute() {
            p
        } else {
            self.root.join(p)
        })
    }
}

/// Split NUL-separated git output into strings.
pub(crate) fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}
