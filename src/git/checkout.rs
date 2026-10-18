//! Temporary checkouts of the staged index or a tree, for repo tools that
//! need a real directory. Removed on drop, including on panic.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tempfile::TempDir;

use super::{Repo, Source};

/// Directories reused from the working tree instead of checked out, because
/// they hold installed dependencies or build caches rather than source.
pub const DEPENDENCY_DIRS: &[&str] = &[
    "node_modules",
    ".venv",
    "venv",
    "target",
    ".tox",
    "vendor/bundle",
    ".gradle",
    "_build",
    "deps",
];

pub struct Checkout {
    dir: TempDir,
}

impl Checkout {
    /// Check out `source` into a fresh temporary directory, then symlink in the
    /// working tree's dependency directories found at the root or under `package_dirs`.
    pub fn create(repo: &Repo, source: &Source, package_dirs: &[String]) -> Result<Checkout> {
        let dir = tempfile::Builder::new()
            .prefix("qgate-checkout-")
            .tempdir()
            .context("creating temporary checkout")?;
        let prefix = format!("{}/", dir.path().display());
        match source {
            Source::Index => {
                let status = repo
                    .command()
                    .args([
                        "checkout-index",
                        "--all",
                        "--force",
                        &format!("--prefix={prefix}"),
                    ])
                    .stdin(Stdio::null())
                    .status()?;
                if !status.success() {
                    bail!("git checkout-index failed");
                }
            }
            Source::Tree(tree) => {
                let index = dir.path().join(".qgate-index");
                let run = |args: &[&str]| -> Result<()> {
                    let status = repo
                        .command()
                        .env("GIT_INDEX_FILE", &index)
                        .args(args)
                        .stdin(Stdio::null())
                        .status()?;
                    if !status.success() {
                        bail!("git {} failed", args.join(" "));
                    }
                    Ok(())
                };
                run(&["read-tree", tree])?;
                run(&[
                    "checkout-index",
                    "--all",
                    "--force",
                    &format!("--prefix={prefix}"),
                ])?;
                std::fs::remove_file(&index)?;
            }
        }
        let checkout = Checkout { dir };
        checkout.link_dependencies(&repo.root, package_dirs)?;
        Ok(checkout)
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    fn link_dependencies(&self, worktree: &Path, package_dirs: &[String]) -> Result<()> {
        let mut bases: Vec<PathBuf> = vec![PathBuf::new()];
        bases.extend(package_dirs.iter().map(PathBuf::from));
        for base in bases {
            for name in DEPENDENCY_DIRS {
                let rel = base.join(name);
                let src = worktree.join(&rel);
                let dst = self.root().join(&rel);
                if src.is_dir() && !dst.exists() {
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::os::unix::fs::symlink(&src, &dst)
                        .with_context(|| format!("linking {}", rel.display()))?;
                }
            }
        }
        Ok(())
    }
}
