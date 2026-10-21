//! Copied-code detection across the whole repo (KTD5): Rabin-Karp rolling
//! hashes over normalized tokens, grouped into clone groups keyed by the hash
//! of the shared block, so moving a member keeps the group's key.

pub mod index;
pub mod tokens;

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::checks::exclude::{Excludes, is_generated};
use crate::git::{BlobReader, Repo, Snapshot};
use crate::hash::Fnv;
use index::TokenCache;
use tokens::Token;

/// A window hash seen more often than this is boilerplate, not a copy.
const MAX_OCCURRENCES: usize = 64;
const BASE: u64 = 0x9e37_79b9_7f4a_7c15;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneGroup {
    /// Hex hash of the shared block's normalized tokens.
    pub key: String,
    /// Length of the shared block in tokens.
    pub tokens: u32,
    pub members: Vec<Location>,
}

impl CloneGroup {
    /// "other.rs:3-20, third.rs:8-25": every member except `path`.
    pub fn others(&self, path: &str, start_line: u32) -> String {
        self.members
            .iter()
            .filter(|m| !(m.path == path && m.start_line == start_line))
            .map(|m| format!("{}:{}-{}", m.path, m.start_line, m.end_line))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub struct Detection {
    pub groups: Vec<CloneGroup>,
    /// Files tokenized this run because the cache did not have them.
    pub tokenized: usize,
}

struct File {
    path: String,
    tokens: Vec<Token>,
}

/// Find clone groups in `snapshot`. With `focus`, only groups with a member in
/// one of those paths are returned (and computed).
pub fn detect(
    repo: &Repo,
    snapshot: &Snapshot,
    excludes: &Excludes,
    min_tokens: u32,
    focus: Option<&HashSet<String>>,
) -> Result<Detection> {
    let (files, tokenized) = load_files(repo, snapshot, excludes)?;
    let w = min_tokens.max(1) as usize;
    let windows: Vec<Vec<u64>> = files.iter().map(|f| window_hashes(&f.tokens, w)).collect();
    let in_focus: Vec<bool> = files
        .iter()
        .map(|f| focus.is_none_or(|set| set.contains(&f.path)))
        .collect();

    // Only window hashes that occur in a focused file can form a reported group.
    let wanted: Option<HashSet<u64>> = focus.map(|_| {
        windows
            .iter()
            .zip(&in_focus)
            .filter(|(_, f)| **f)
            .flat_map(|(ws, _)| ws.iter().copied())
            .collect()
    });
    let mut entries: Vec<(u64, u32, u32)> = Vec::new();
    for (fi, ws) in windows.iter().enumerate() {
        for (pos, h) in ws.iter().enumerate() {
            if wanted.as_ref().is_none_or(|set| set.contains(h)) {
                entries.push((*h, fi as u32, pos as u32));
            }
        }
    }
    entries.sort_unstable();

    let mut groups: HashMap<u64, (u32, BTreeSet<(u32, u32)>)> = HashMap::new();
    for run in entries.chunk_by(|a, b| a.0 == b.0) {
        if run.len() < 2 || run.len() > MAX_OCCURRENCES {
            continue;
        }
        for (i, a) in run.iter().enumerate() {
            for b in &run[i + 1..] {
                let (fa, pa, fb, pb) = (a.1 as usize, a.2 as usize, b.1 as usize, b.2 as usize);
                if !in_focus[fa] && !in_focus[fb] {
                    continue;
                }
                if let Some(len) = maximal_match(&files, fa, pa, fb, pb, w) {
                    let block = block_hash(&files[fa].tokens[pa..pa + len]);
                    let entry = groups.entry(block).or_insert((len as u32, BTreeSet::new()));
                    entry.1.insert((fa as u32, pa as u32));
                    entry.1.insert((fb as u32, pb as u32));
                }
            }
        }
    }

    let mut out: Vec<CloneGroup> = groups
        .into_iter()
        .map(|(hash, (len, members))| {
            let members: Vec<Location> = members
                .into_iter()
                .map(|(f, p)| {
                    let toks = &files[f as usize].tokens;
                    Location {
                        path: files[f as usize].path.clone(),
                        start_line: toks[p as usize].line,
                        end_line: toks[p as usize + len as usize - 1].line,
                    }
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            CloneGroup {
                key: format!("{hash:016x}"),
                tokens: len,
                members,
            }
        })
        .filter(|g| g.members.len() >= 2)
        .collect();
    out.sort_by(|a, b| a.members.cmp(&b.members).then(a.key.cmp(&b.key)));
    Ok(Detection {
        groups: out,
        tokenized,
    })
}

/// The full length of the match starting at these positions, or `None` when
/// it is not a real match, overlaps itself, or does not start here.
fn maximal_match(
    files: &[File],
    fa: usize,
    pa: usize,
    fb: usize,
    pb: usize,
    w: usize,
) -> Option<usize> {
    let (ta, tb) = (&files[fa].tokens, &files[fb].tokens);
    // Within one file, a block may not run into its own copy.
    let cap = if fa == fb {
        pb.abs_diff(pa)
    } else {
        usize::MAX
    };
    if cap < w {
        return None;
    }
    if pa > 0 && pb > 0 && ta[pa - 1].hash == tb[pb - 1].hash {
        return None; // the match starts earlier
    }
    let mut len = 0;
    while len < cap
        && pa + len < ta.len()
        && pb + len < tb.len()
        && ta[pa + len].hash == tb[pb + len].hash
    {
        len += 1;
    }
    (len >= w).then_some(len)
}

fn block_hash(tokens: &[Token]) -> u64 {
    let mut h = Fnv::default();
    for t in tokens {
        h.write_u64(t.hash);
    }
    h.finish()
}

/// Rabin-Karp hash of every `w`-token window.
fn window_hashes(tokens: &[Token], w: usize) -> Vec<u64> {
    if tokens.len() < w {
        return Vec::new();
    }
    let top = (1..w).fold(1u64, |acc, _| acc.wrapping_mul(BASE));
    let mut h = tokens[..w]
        .iter()
        .fold(0u64, |acc, t| acc.wrapping_mul(BASE).wrapping_add(t.hash));
    let mut out = Vec::with_capacity(tokens.len() - w + 1);
    out.push(h);
    for i in w..tokens.len() {
        h = h
            .wrapping_sub(tokens[i - w].hash.wrapping_mul(top))
            .wrapping_mul(BASE)
            .wrapping_add(tokens[i].hash);
        out.push(h);
    }
    out
}

/// Tokens for every checked file, from the cache or by reading and tokenizing the blob.
fn load_files(repo: &Repo, snapshot: &Snapshot, excludes: &Excludes) -> Result<(Vec<File>, usize)> {
    let mut cache = TokenCache::load(&repo.state_dir());
    let targets: Vec<(String, String, String)> = snapshot
        .files()
        .filter(|(p, _)| tokens::is_code(p) && !excludes.is_excluded(p))
        .map(|(p, e)| {
            (
                p.clone(),
                e.oid.clone(),
                index::key(&e.oid, &tokens::tokenizer_tag(p)),
            )
        })
        .collect();
    let misses: Vec<&(String, String, String)> = targets
        .iter()
        .filter(|(_, _, k)| cache.get(k).is_none())
        .collect();

    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let chunk = misses.len().div_ceil(workers).max(1);
    let fresh: Vec<(String, Vec<Token>)> = std::thread::scope(|s| {
        let handles: Vec<_> = misses
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || -> Result<Vec<(String, Vec<Token>)>> {
                    let mut reader = BlobReader::new(repo)?;
                    part.iter()
                        .map(|(path, oid, key)| {
                            let bytes = reader.read(oid)?;
                            let text = String::from_utf8_lossy(&bytes);
                            let toks = if crate::git::index::is_binary(&bytes)
                                || is_generated(path, &text)
                            {
                                Vec::new()
                            } else {
                                tokens::tokenize(path, &text)
                            };
                            Ok((key.clone(), toks))
                        })
                        .collect()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("tokenizer thread"))
            .collect::<Result<Vec<_>>>()
            .map(|v| v.into_iter().flatten().collect())
    })?;
    let tokenized = fresh.len();
    for (key, toks) in fresh {
        cache.insert(key, toks);
    }
    let files = targets
        .iter()
        .map(|(path, _, key)| File {
            path: path.clone(),
            tokens: cache.get(key).cloned().unwrap_or_default(),
        })
        .collect();
    let live: HashSet<String> = targets.into_iter().map(|(_, _, k)| k).collect();
    // A cache that cannot be written only costs speed next time.
    let _ = cache.save(&live);
    Ok((files, tokenized))
}
