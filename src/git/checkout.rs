//! The checkout repo tools run in when the working tree doesn't hold exactly
//! what is being checked. One per repository, at `.git/qgate/checkout`, kept
//! between runs and updated in place: only files that changed are rewritten,
//! so their mtimes stay put and builds inside it stay incremental. Build
//! outputs (`_build`, `target`, `.gradle`) live in it and are never shared
//! with the working tree; installed dependencies are linked in.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};

use super::{Repo, Source};

/// Directories that hold installed dependencies or build output rather than
/// source. Tool findings inside them are never the repo's own code.
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

/// The [`DEPENDENCY_DIRS`] that tools write build output into. The checkout
/// keeps its own, so a tool run never writes into the working tree's.
pub const BUILD_DIRS: &[&str] = &["target", ".gradle", "_build"];

/// The checkout, locked for as long as this value lives. A second run waits
/// for the lock, so concurrent hooks never update it under each other.
pub struct Checkout {
    root: PathBuf,
    _lock: File,
}

impl Checkout {
    /// Lock the checkout, update it to `source`, and link in the working
    /// tree's dependency directories found at the root or under `package_dirs`.
    pub fn create(repo: &Repo, source: &Source, package_dirs: &[String]) -> Result<Checkout> {
        let state = repo.state_dir();
        std::fs::create_dir_all(&state).with_context(|| format!("creating {}", state.display()))?;
        let lock_path = state.join("checkout.lock");
        let lock =
            File::create(&lock_path).with_context(|| format!("opening {}", lock_path.display()))?;
        lock.lock()
            .with_context(|| format!("locking {}", lock_path.display()))?;
        let checkout = Checkout {
            root: state.join("checkout"),
            _lock: lock,
        };
        let links = link_paths(package_dirs);
        checkout.unlink(&links)?;
        let tree = match source {
            Source::Index => staged_tree(repo, &state)?,
            Source::Tree(tree) => tree.clone(),
        };
        let index = state.join("checkout.index");
        if checkout.update(repo, &index, &tree).is_err() {
            // A damaged index or an interrupted update: start again from empty.
            let _ = std::fs::remove_file(&index);
            if checkout.root.exists() {
                std::fs::remove_dir_all(&checkout.root)
                    .with_context(|| format!("removing {}", checkout.root.display()))?;
            }
            checkout.update(repo, &index, &tree)?;
        }
        checkout.link(&repo.root, &links)?;
        Ok(checkout)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Make the checkout match `tree`: rewrite changed or locally edited
    /// files, remove deleted ones, then remove untracked files that aren't
    /// ignored. Ignored files, such as build output, stay.
    fn update(&self, repo: &Repo, index: &Path, tree: &str) -> Result<()> {
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("creating {}", self.root.display()))?;
        let git = |args: &[&str]| -> Result<()> {
            let out = repo
                .command()
                .current_dir(&self.root)
                .env("GIT_DIR", &repo.git_dir)
                .env("GIT_WORK_TREE", &self.root)
                .env("GIT_INDEX_FILE", index)
                .args(args)
                .stdin(Stdio::null())
                .output()?;
            if !out.status.success() {
                bail!(
                    "git {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Ok(())
        };
        git(&["read-tree", "--reset", "-u", tree])?;
        git(&["clean", "-ffdq"])
    }

    /// Remove links left by an earlier run, so moved or deleted dependency
    /// directories don't leave stale ones and `update` sees a clean tree.
    fn unlink(&self, links: &[PathBuf]) -> Result<()> {
        for rel in links {
            let dst = self.root.join(rel);
            if dst.is_symlink() {
                std::fs::remove_file(&dst)
                    .with_context(|| format!("unlinking {}", dst.display()))?;
            }
        }
        Ok(())
    }

    fn link(&self, worktree: &Path, links: &[PathBuf]) -> Result<()> {
        for rel in links {
            let src = worktree.join(rel);
            let dst = self.root.join(rel);
            if src.is_dir() && !dst.exists() {
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::os::unix::fs::symlink(&src, &dst)
                    .with_context(|| format!("linking {}", rel.display()))?;
            }
        }
        Ok(())
    }
}

/// The tree of the index git will commit. `write-tree` runs on a copy,
/// because it saves its cache back into the index it reads.
fn staged_tree(repo: &Repo, state: &Path) -> Result<String> {
    let real = repo
        .root
        .join(repo.output(&["rev-parse", "--git-path", "index"])?.trim());
    if !real.exists() {
        return repo.empty_tree();
    }
    let copy = state.join("staged.index");
    std::fs::copy(&real, &copy).with_context(|| format!("copying {}", real.display()))?;
    let out = repo
        .command()
        .env("GIT_INDEX_FILE", &copy)
        .args(["write-tree"])
        .stdin(Stdio::null())
        .output()?;
    let _ = std::fs::remove_file(&copy);
    if !out.status.success() {
        bail!(
            "git write-tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// Dependency directories to link, at the root and under each package.
fn link_paths(package_dirs: &[String]) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = vec![PathBuf::new()];
    bases.extend(package_dirs.iter().map(PathBuf::from));
    bases
        .iter()
        .flat_map(|base| {
            DEPENDENCY_DIRS
                .iter()
                .filter(|d| !BUILD_DIRS.contains(d))
                .map(move |d| base.join(d))
        })
        .collect()
}
