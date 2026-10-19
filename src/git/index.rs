//! Snapshots of a tree's files (the staged index or a commit's tree) and a
//! batch reader for their blobs.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

use anyhow::{Context, Result, bail};

use super::{Repo, split_nul};

/// Where a snapshot's content comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The index git will commit (honors `GIT_INDEX_FILE` set by `git commit <path>`).
    Index,
    /// A tree or commit id.
    Tree(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub mode: String,
    pub oid: String,
}

impl Entry {
    /// A regular file (not a symlink or submodule).
    pub fn is_file(&self) -> bool {
        self.mode == "100644" || self.mode == "100755"
    }
}

/// Every path in a tree with its blob id. Reading content goes through a [`BlobReader`].
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub source: Source,
    pub entries: BTreeMap<String, Entry>,
}

impl Snapshot {
    pub fn index(repo: &Repo) -> Result<Snapshot> {
        let out = repo.output_bytes(&["ls-files", "--stage", "-z"])?;
        let mut entries = BTreeMap::new();
        for record in split_nul(&out) {
            // "<mode> <oid> <stage>\t<path>"
            let (meta, path) = record.split_once('\t').context("bad ls-files record")?;
            let mut parts = meta.split(' ');
            let (Some(mode), Some(oid), Some(stage)) = (parts.next(), parts.next(), parts.next())
            else {
                bail!("bad ls-files record: {record}");
            };
            if stage != "0" {
                bail!(
                    crate::cli::UsageError::new(format!("{path} has unresolved merge conflicts"))
                        .with_hint(format!("resolve the conflict, then: git add {path}"))
                );
            }
            entries.insert(
                path.to_string(),
                Entry {
                    mode: mode.to_string(),
                    oid: oid.to_string(),
                },
            );
        }
        Ok(Snapshot {
            source: Source::Index,
            entries,
        })
    }

    pub fn tree(repo: &Repo, treeish: &str) -> Result<Snapshot> {
        let out = repo.output_bytes(&["ls-tree", "-r", "-z", "--full-tree", treeish])?;
        let mut entries = BTreeMap::new();
        for record in split_nul(&out) {
            // "<mode> <type> <oid>\t<path>"
            let (meta, path) = record.split_once('\t').context("bad ls-tree record")?;
            let mut parts = meta.split(' ');
            let (Some(mode), Some(_kind), Some(oid)) = (parts.next(), parts.next(), parts.next())
            else {
                bail!("bad ls-tree record: {record}");
            };
            entries.insert(
                path.to_string(),
                Entry {
                    mode: mode.to_string(),
                    oid: oid.to_string(),
                },
            );
        }
        Ok(Snapshot {
            source: Source::Tree(treeish.to_string()),
            entries,
        })
    }

    pub fn empty(repo: &Repo) -> Result<Snapshot> {
        Ok(Snapshot {
            source: Source::Tree(repo.empty_tree()?),
            entries: BTreeMap::new(),
        })
    }

    /// The tree at `rev`, or the empty tree when `rev` is `None`.
    pub fn at(repo: &Repo, rev: Option<&str>) -> Result<Snapshot> {
        match rev {
            Some(rev) => Snapshot::tree(repo, rev),
            None => Snapshot::empty(repo),
        }
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.get(path)
    }

    /// Regular files only, in path order.
    pub fn files(&self) -> impl Iterator<Item = (&String, &Entry)> {
        self.entries.iter().filter(|(_, e)| e.is_file())
    }
}

/// A long-lived `git cat-file --batch` process. Not shared across threads;
/// create one per worker.
pub struct BlobReader {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl BlobReader {
    pub fn new(repo: &Repo) -> Result<BlobReader> {
        let mut child = repo
            .command()
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting git cat-file")?;
        let stdin = child.stdin.take().context("cat-file stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("cat-file stdout")?);
        Ok(BlobReader {
            child,
            stdin,
            stdout,
        })
    }

    pub fn read(&mut self, oid: &str) -> Result<Vec<u8>> {
        writeln!(self.stdin, "{oid}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header)?;
        // "<oid> <type> <size>" or "<oid> missing"
        let mut parts = header.split_whitespace();
        let (_, kind, size) = (parts.next(), parts.next(), parts.next());
        let Some(size) = size.and_then(|s| s.parse::<usize>().ok()) else {
            bail!("object {oid} is {}", kind.unwrap_or("missing"));
        };
        let mut buf = vec![0; size + 1]; // content plus trailing newline
        self.stdout.read_exact(&mut buf)?;
        buf.pop();
        Ok(buf)
    }
}

impl Drop for BlobReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// True when content looks binary: a NUL byte in the first 8000 bytes, as git decides.
pub fn is_binary(content: &[u8]) -> bool {
    content.iter().take(8000).any(|b| *b == 0)
}
